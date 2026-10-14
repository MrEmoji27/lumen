//! Motion. Every animated value glides toward its target with exponential easing,
//! frame-rate independent. A timer runs only while something still moves; at rest
//! the chrome costs nothing. Values not touched in a frame are dropped, so closed
//! tabs and closed panels leave nothing behind.

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

pub struct Anims {
    vals: HashMap<String, f32>,
    touched: HashSet<String>,
    last: Instant,
    dt: f32,
    /// Something is still moving: keep the frame timer running.
    pub moving: bool,
    /// Off: every value jumps to its target (setting off, or Windows "reduce motion").
    pub enabled: bool,
}

impl Default for Anims {
    fn default() -> Self {
        Anims { vals: HashMap::new(), touched: HashSet::new(), last: Instant::now(), dt: 0.0, moving: false, enabled: true }
    }
}

impl Anims {
    pub fn begin(&mut self) {
        let now = Instant::now();
        // A long gap (window was idle) must not turn into one giant jump of easing.
        self.dt = (now - self.last).as_secs_f32().min(0.05);
        self.last = now;
        self.moving = false;
        self.touched.clear();
    }

    /// Forget values nobody asked for this frame.
    pub fn end(&mut self) {
        let touched = &self.touched;
        self.vals.retain(|k, _| touched.contains(k));
    }

    /// The current value of `key`, eased toward `target`. `tau` is the time constant in
    /// seconds: about 63% of the way after tau, settled after ~4 tau. New keys start at target.
    pub fn get(&mut self, key: &str, target: f32, tau: f32) -> f32 {
        self.ease(key, target, target, tau)
    }

    /// Like `get`, but a new key starts at `start`: entrances.
    pub fn from(&mut self, key: &str, start: f32, target: f32, tau: f32) -> f32 {
        self.ease(key, start, target, tau)
    }

    fn ease(&mut self, key: &str, start: f32, target: f32, tau: f32) -> f32 {
        self.touched.insert(key.to_string());
        if !self.enabled {
            self.vals.insert(key.to_string(), target);
            return target;
        }
        let v = self.vals.entry(key.to_string()).or_insert(start);
        let k = 1.0 - (-self.dt / tau.max(0.001)).exp();
        *v += (target - *v) * k;
        let scale = target.abs().max(1.0);
        if (target - *v).abs() < 0.002 * scale {
            *v = target;
        } else {
            self.moving = true;
        }
        *v
    }
}

/// Windows "Show animations in Windows" (Settings, Accessibility, Visual effects).
pub fn system_allows() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
    let mut on = windows::core::BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    on.as_bool()
}

#[cfg(test)]
mod tests {
    use super::Anims;

    #[test]
    fn eases_and_settles() {
        let mut a = Anims::default();
        a.begin();
        assert_eq!(a.get("x", 0.0, 0.1), 0.0);
        let mut v = 0.0;
        for _ in 0..200 {
            a.begin();
            a.dt = 0.016;
            v = a.get("x", 1.0, 0.1);
            a.end();
        }
        assert_eq!(v, 1.0);
        assert!(!a.moving);
    }

    #[test]
    fn disabled_jumps() {
        let mut a = Anims { enabled: false, ..Default::default() };
        a.begin();
        assert_eq!(a.from("y", 0.0, 5.0, 1.0), 5.0);
    }

    #[test]
    fn untouched_keys_drop() {
        let mut a = Anims::default();
        a.begin();
        a.get("gone", 1.0, 0.1);
        a.end();
        a.begin();
        a.end();
        a.begin();
        assert_eq!(a.from("gone", 0.0, 1.0, 10.0), 0.0 + (1.0 - 0.0) * (1.0 - (-a.dt / 10.0f32).exp()));
    }
}
