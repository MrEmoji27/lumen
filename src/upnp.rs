//! UPnP IGD: open the torrent port on the router so peers can reach us.

use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread,
    time::Duration,
};

use igd_next::{search_gateway, AddPortError, Gateway, PortMappingProtocol};

pub fn random_port() -> u16 {
    use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
    let mut b = [0u8; 2];
    unsafe {
        let _ = BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
    }
    let base = u16::from_le_bytes(b) % (60000 - 20000 + 1);
    20000 + base
}

fn local_lan_addr() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:53").ok()?;
    let addr = socket.local_addr().ok()?;
    match addr {
        SocketAddr::V4(v4) => Some(*v4.ip()),
        _ => None,
    }
}

struct UpnpThread {
    port: u16,
    active: Mutex<bool>,
    wake: Condvar,
    mapped: Arc<std::sync::atomic::AtomicBool>,
    gateway: Arc<Mutex<Option<Gateway>>>,
}

impl UpnpThread {
    fn start(port: u16) -> Arc<Self> {
        let mapped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gateway = Arc::new(Mutex::new(None));
        let this = Arc::new(Self {
            port,
            active: Mutex::new(false),
            wake: Condvar::new(),
            mapped: mapped.clone(),
            gateway: gateway.clone(),
        });
        let weak = Arc::downgrade(&this);
        thread::Builder::new()
            .name("lumen-upnp".into())
            .spawn(move || {
                if let Some(this) = weak.upgrade() {
                    this.run();
                }
            })
            .expect("UPnP thread spawn");
        this
    }

    fn set_active(this: &Arc<Self>, on: bool) {
        let mut g = lock(&this.active);
        if *g != on {
            *g = on;
            this.wake.notify_all();
        }
    }

    #[allow(unused_assignments)]
    fn run(&self) {
        let mut gateway: Option<Gateway> = None;
        let mut mapped = false;
        let mut logged_no_gateway = false;

        gateway = discover_gateway();
        *lock(&self.gateway) = gateway.clone();
        if gateway.is_none() {
            eprintln!("lumen: upnp: no gateway found in 5 s; torrents work outbound only");
            logged_no_gateway = true;
        }

        let mut handled = false;
        loop {
            let guard = lock(&self.active);
            let (guard, _) = self
                .wake
                .wait_timeout_while(guard, Duration::from_secs(50 * 60), |a| *a == handled)
                .unwrap_or_else(|e| e.into_inner());
            let now_active = *guard;
            drop(guard);
            handled = now_active;

            if now_active {
                if gateway.is_none() {
                    gateway = discover_gateway();
                    *lock(&self.gateway) = gateway.clone();
                    if gateway.is_none() && !logged_no_gateway {
                        eprintln!("lumen: upnp: no gateway; torrents work outbound only");
                        logged_no_gateway = true;
                    }
                }
                if let Some(gw) = &gateway {
                    if !mapped {
                        if add_mapping(gw, self.port).is_ok() {
                            mapped = true;
                            self.mapped.store(true, std::sync::atomic::Ordering::Relaxed);
                            eprintln!("lumen: upnp: mapped port {} (TCP+UDP)", self.port);
                        }
                    } else {
                        let _ = add_mapping(gw, self.port);
                    }
                }
            } else {
                let guard = lock(&self.active);
                let (guard, _) = self
                    .wake
                    .wait_timeout_while(guard, Duration::from_secs(10 * 60), |a| !*a)
                    .unwrap_or_else(|e| e.into_inner());
                if *guard {
                    handled = false;
                    continue;
                }
                drop(guard);
                if mapped {
                    if let Some(gw) = &gateway {
                        let _ = remove_mapping(gw, self.port);
                        eprintln!("lumen: upnp: unmapped port {}", self.port);
                    }
                    mapped = false;
                    self.mapped.store(false, std::sync::atomic::Ordering::Relaxed);
                }
                gateway = None;
                *lock(&self.gateway) = None;
                logged_no_gateway = false;
            }
        }
    }
}

use igd_next::SearchOptions;

fn discover_gateway() -> Option<Gateway> {
    let mut opts = SearchOptions::default();
    opts.timeout = Some(Duration::from_secs(5));
    let handle = thread::spawn(move || search_gateway(opts));
    handle.join().ok().and_then(|r| r.ok())
}

fn add_mapping(gateway: &Gateway, port: u16) -> Result<(), AddPortError> {
    let lan_ip = local_lan_addr().unwrap_or(Ipv4Addr::new(0, 0, 0, 0));
    let local_addr = SocketAddr::V4(SocketAddrV4::new(lan_ip, port));
    let desc = "Lumen torrents";
    let lease = 3600;

    gateway.add_port(PortMappingProtocol::TCP, port, local_addr, lease, desc)?;
    gateway.add_port(PortMappingProtocol::UDP, port, local_addr, lease, desc)?;
    Ok(())
}

fn remove_mapping(gateway: &Gateway, port: u16) -> Result<(), AddPortError> {
    let _ = gateway.remove_port(PortMappingProtocol::TCP, port);
    let _ = gateway.remove_port(PortMappingProtocol::UDP, port);
    Ok(())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone)]
pub struct Upnp {
    inner: Arc<UpnpThread>,
}

impl Upnp {
    pub fn start(port: u16, enabled: bool) -> Self {
        if !enabled {
            return Self { inner: Arc::new(UpnpThread { port, active: Mutex::new(false), wake: Condvar::new(), mapped: Arc::new(std::sync::atomic::AtomicBool::new(false)), gateway: Arc::new(Mutex::new(None)) }) };
        }
        Self { inner: UpnpThread::start(port) }
    }

    pub fn torrent_active(&self) {
        UpnpThread::set_active(&self.inner, true);
    }

    pub fn torrent_idle(&self) {
        UpnpThread::set_active(&self.inner, false);
    }

    pub fn is_mapped(&self) -> bool {
        self.inner.mapped.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Upnp {
    pub fn release(&self) {
        UpnpThread::set_active(&self.inner, false);
        let gateway = lock(&self.inner.gateway).take();
        let Some(gw) = gateway else { return };
        if !self.inner.mapped.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let port = self.inner.port;
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = thread::Builder::new().name("lumen-upnp-cleanup".into()).spawn(move || {
            let _ = remove_mapping(&gw, port);
            let _ = tx.send(());
        });
        if spawned.is_ok() && rx.recv_timeout(Duration::from_secs(2)).is_ok() {
            eprintln!("lumen: upnp: unmapped port {port} on exit");
        }
    }
}

impl Drop for Upnp {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{BT_PORT_MIN, BT_PORT_MAX};

    #[test]
    fn random_port_in_range() {
        for _ in 0..1000 {
            let p = random_port();
            assert!((BT_PORT_MIN..=BT_PORT_MAX).contains(&p), "port {} out of range", p);
        }
    }

    #[test]
    fn random_port_not_constant() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            seen.insert(random_port());
        }
        assert!(seen.len() > 10, "random_port seems stuck at one value");
    }

    #[test]
    fn local_lan_addr_returns_something_or_none() {
        let _ = local_lan_addr();
    }

    fn should_map(active_torrents: usize, upnp_enabled: bool) -> bool {
        upnp_enabled && active_torrents > 0
    }

    #[test]
    fn map_decision_pure() {
        assert!(!should_map(0, true));
        assert!(should_map(1, true));
        assert!(should_map(5, true));
        assert!(!should_map(1, false));
        assert!(!should_map(0, false));
    }
}
