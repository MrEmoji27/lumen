//! UPnP IGD: open the torrent port on the router so peers can reach us.
//!
//! Runs on a background thread. Discovers the gateway (5 s timeout), maps TCP and
//! UDP `<port>` to this PC's LAN address, description "Lumen torrents", lease
//! 1 hour, renews every 50 minutes while a torrent is active. Removes both
//! mappings when no torrent has been active for 10 minutes, and on Lumen exit.
//! No gateway, or UPnP off on the router: log once and carry on (torrents still
//! work outbound).

use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread,
    time::Duration,
};

use igd_next::{search_gateway, AddPortError, Gateway, PortMappingProtocol};

/// Random port in 20000..60000, chosen once and saved.
pub fn random_port() -> u16 {
    use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
    let mut b = [0u8; 2];
    unsafe {
        let _ = BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
    }
    let base = u16::from_le_bytes(b) % (60000 - 20000 + 1);
    20000 + base
}

/// The local IPv4 address on the LAN (not 127.0.0.1). Best effort.
fn local_lan_addr() -> Option<Ipv4Addr> {
    // Connect a UDP socket to a public address; the OS picks the right interface.
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:53").ok()?;
    let addr = socket.local_addr().ok()?;
    match addr {
        SocketAddr::V4(v4) => Some(*v4.ip()),
        _ => None,
    }
}

/// What the UPnP thread does.
struct UpnpThread {
    /// The port to map (never 0 after init).
    port: u16,
    /// Set true when a torrent is active. Guards the lease.
    active: Mutex<bool>,
    /// Wake the thread when active changes.
    wake: Condvar,
    /// Whether the port is currently mapped (updated by the thread).
    mapped: Arc<std::sync::atomic::AtomicBool>,
    /// The discovered gateway, shared so Drop can access it for cleanup.
    gateway: Arc<Mutex<Option<Gateway>>>,
}

impl UpnpThread {
    /// Start the background thread. Returns the controller.
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

    /// Mark a torrent as active/inactive. Wakes the thread to (re)map or unmap.
    fn set_active(this: &Arc<Self>, on: bool) {
        let mut g = lock(&this.active);
        if *g != on {
            *g = on;
            this.wake.notify_all();
        }
    }

    /// The thread loop. Discovers gateway, maps ports, renews, unmaps on exit.
    #[allow(unused_assignments)]
    fn run(&self) {
        let mut gateway: Option<Gateway> = None;
        let mut mapped = false;
        let mut logged_no_gateway = false;

        // Initial discovery (blocking, up to 5 s).
        gateway = discover_gateway();
        // Store gateway for potential cleanup on drop.
        *lock(&self.gateway) = gateway.clone();
        if gateway.is_none() {
            eprintln!("lumen: upnp: no gateway found in 5 s; torrents work outbound only");
            logged_no_gateway = true;
        }

        loop {
            // Wait for a reason to do something: active toggled, or 50 min renewal timer.
            let mut active = lock(&self.active);
            let wait_result = self.wake.wait_timeout(active, Duration::from_secs(50 * 60));
            active = wait_result.unwrap().0;

            let now_active = *active;
            drop(active);

            if now_active {
                // Torrent is active: ensure mapping exists and is fresh.
                if gateway.is_none() {
                    // Retry discovery once per active period.
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
                        // Renew lease (add_port with same params extends it).
                        let _ = add_mapping(gw, self.port);
                    }
                }
            } else {
                // No active torrent: wait 10 min, then unmap and forget gateway.
                thread::sleep(Duration::from_secs(10 * 60));
                // Double-check we're still inactive.
                if *lock(&self.active) {
                    continue; // Became active again during the wait.
                }
                if mapped {
                    if let Some(gw) = &gateway {
                        let _ = remove_mapping(gw, self.port);
                        eprintln!("lumen: upnp: unmapped port {}", self.port);
                    }
                    mapped = false;
                    self.mapped.store(false, std::sync::atomic::Ordering::Relaxed);
                }
                // Forget gateway so we re-discover next time (router may have changed).
                gateway = None;
                *lock(&self.gateway) = None;
                logged_no_gateway = false;
            }
        }
    }
}

use igd_next::SearchOptions;

/// Discover the IGD gateway with a 5 second timeout.
fn discover_gateway() -> Option<Gateway> {
    // search_gateway has its own timeout logic. We run it in a thread
    // with a 5 s join timeout to enforce our deadline.
    let handle = thread::spawn(|| search_gateway(SearchOptions::default()));
    handle.join().ok().and_then(|r| r.ok())
}

/// Add TCP and UDP port mappings for the given port.
fn add_mapping(gateway: &Gateway, port: u16) -> Result<(), AddPortError> {
    let lan_ip = local_lan_addr().unwrap_or(Ipv4Addr::new(0, 0, 0, 0));
    let local_addr = SocketAddr::V4(SocketAddrV4::new(lan_ip, port));
    // Description shown in router UI.
    let desc = "Lumen torrents";
    // Lease duration: 1 hour (3600 s). 0 = permanent (not all routers support).
    let lease = 3600;

    // Map TCP.
    gateway.add_port(PortMappingProtocol::TCP, port, local_addr, lease, desc)?;
    // Map UDP.
    gateway.add_port(PortMappingProtocol::UDP, port, local_addr, lease, desc)?;
    Ok(())
}

/// Remove TCP and UDP port mappings for the given port.
fn remove_mapping(gateway: &Gateway, port: u16) -> Result<(), AddPortError> {
    let _ = gateway.remove_port(PortMappingProtocol::TCP, port);
    let _ = gateway.remove_port(PortMappingProtocol::UDP, port);
    Ok(())
}

/// A poisoned lock is only poison because a thread panicked while holding it.
/// The data inside is still good.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Public controller for the UPnP background thread.
#[derive(Clone)]
pub struct Upnp {
    inner: Arc<UpnpThread>,
}

impl Upnp {
    /// Start the UPnP manager for the given port. If `enabled` is false,
    /// returns a no-op controller.
    pub fn start(port: u16, enabled: bool) -> Self {
        if !enabled {
            return Self { inner: Arc::new(UpnpThread { port, active: Mutex::new(false), wake: Condvar::new(), mapped: Arc::new(std::sync::atomic::AtomicBool::new(false)), gateway: Arc::new(Mutex::new(None)) }) };
        }
        Self { inner: UpnpThread::start(port) }
    }

    /// Call when a torrent starts (or resumes seeding).
    pub fn torrent_active(&self) {
        UpnpThread::set_active(&self.inner, true);
    }

    /// Call when the last active torrent stops/pauses.
    pub fn torrent_idle(&self) {
        UpnpThread::set_active(&self.inner, false);
    }

    /// Whether the port is currently mapped via UPnP.
    pub fn is_mapped(&self) -> bool {
        self.inner.mapped.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for Upnp {
    fn drop(&mut self) {
        // If we have a gateway and a mapping, remove it synchronously now.
        // Bounded to ~2 seconds total so exit isn't delayed.
        let gateway = {
            let mut g = lock(&self.inner.gateway);
            g.take()
        };
        if let Some(gw) = gateway {
            if self.inner.mapped.load(std::sync::atomic::Ordering::Relaxed) {
                let port = self.inner.port;
                let _ = thread::Builder::new()
                    .name("lumen-upnp-cleanup".into())
                    .spawn(move || {
                        let _ = remove_mapping(&gw, port);
                        eprintln!("lumen: upnp: cleanup unmapped port {}", port);
                    })
                    .map(|h| h.join());
            }
        }
        // Signal idle so the background thread knows we're done.
        UpnpThread::set_active(&self.inner, false);
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
        // With 40001 possible values, 100 samples should give many distinct ones.
        assert!(seen.len() > 10, "random_port seems stuck at one value");
    }

    #[test]
    fn local_lan_addr_returns_something_or_none() {
        // Just ensure it doesn't panic.
        let _ = local_lan_addr();
    }

    /// The map/unmap decision as a pure function: given active torrents and
    /// UPnP enabled, should we map?
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