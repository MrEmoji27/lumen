//! Motion: springs and eased tweens for every animated value in the chrome.

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Id {
    Tab(u64),
    Ghost(u64),
    Group(u64),
    Button(&'static str),
    Panel(&'static str),
    Workspace(usize),
    Dial(usize),
    Essential(usize),
    Chip(&'static str),
    Mark(usize),
    Glance,
    Dl(u64),
    Named(&'static str),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Prop {
    Hover,
    Opacity,
    X,
    Y,
    W,
    H,
    Progress,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ease {
    OutCubic,
    OutQuint,
}

impl Ease {
    pub fn at(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::OutCubic => 1.0 - (1.0 - t).powi(3),
            Ease::OutQuint => 1.0 - (1.0 - t).powi(5),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Spring {
    pub omega: f32,
}

impl Spring {
    pub fn settling(settle_ms: u32) -> Spring {
        let secs = (settle_ms as f32 / 1000.0).clamp(0.02, 2.0);
        Spring { omega: 4.6 / secs }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Config {
    pub enabled: bool,
    pub speed: f32,
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: true, speed: 1.0 }
    }
}

impl Config {
    pub fn of(mode: &str, speed_pct: u32, follow_system: bool, system_allows: bool) -> Config {
        let allowed = animate(mode, follow_system, system_allows);
        Config { enabled: allowed, speed: (speed_pct.max(1) as f32 / 100.0).clamp(0.25, 3.0) }
    }
}

pub fn animate(mode: &str, follow_system: bool, system_allows: bool) -> bool {
    mode == "full" && (!follow_system || system_allows)
}

pub fn ease_out(t: f32) -> f32 {
    Ease::OutCubic.at(t)
}

pub fn system_allows() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
    let mut on = windows::core::BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    on.as_bool()
}

enum Val {
    Tween { from: f32, to: f32, t: f32, dur: f32, ease: Ease },
    Spring { x: f32, v: f32, target: f32, omega: f32 },
}

impl Val {
    fn value(&self) -> f32 {
        match self {
            Val::Tween { from, to, t, ease, .. } => from + (to - from) * ease.at(*t),
            Val::Spring { x, .. } => *x,
        }
    }
    fn target(&self) -> f32 {
        match self {
            Val::Tween { to, .. } => *to,
            Val::Spring { target, .. } => *target,
        }
    }
    fn settled(&self) -> bool {
        match self {
            Val::Tween { t, .. } => *t >= 1.0,
            Val::Spring { x, v, target, .. } => {
                let scale = target.abs().max(1.0);
                (target - x).abs() < 0.002 * scale && v.abs() < 0.05 * scale
            }
        }
    }
    fn step(&mut self, dt: f32) -> f32 {
        match self {
            Val::Tween { t, dur, .. } => {
                if *t < 1.0 {
                    *t = (*t + dt / (*dur).max(1.0 / 1000.0)).min(1.0);
                }
                self.value()
            }
            Val::Spring { x, v, target, omega } => {
                let total = dt.min(0.05);
                let steps = ((total / (1.0 / 240.0)).ceil() as u32).clamp(1, 24);
                let h = total / steps as f32;
                let (w, target) = (*omega, *target);
                for _ in 0..steps {
                    let a = -w * w * (*x - target) - 2.0 * w * *v;
                    *v += a * h;
                    *x += *v * h;
                }
                *x
            }
        }
    }
}

pub struct Motion {
    vals: HashMap<(Id, Prop), Val>,
    touched: HashSet<(Id, Prop)>,
    last: Instant,
    dt: f32,
    moving: bool,
    cfg: Config,
}

impl Default for Motion {
    fn default() -> Self {
        Motion { vals: HashMap::new(), touched: HashSet::new(), last: Instant::now(), dt: 0.0, moving: false, cfg: Config::default() }
    }
}

impl Motion {
    pub fn set_config(&mut self, cfg: Config) {
        self.cfg = cfg;
    }

    pub fn begin(&mut self) {
        let now = Instant::now();
        self.dt = (now - self.last).as_secs_f32().min(0.05);
        self.last = now;
        self.moving = false;
        self.touched.clear();
    }

    pub fn end(&mut self) {
        let touched = &self.touched;
        self.vals.retain(|k, _| touched.contains(k));
    }

    pub fn running(&self) -> bool {
        self.moving
    }

    #[cfg(test)]
    pub fn dt(&self) -> f32 {
        self.dt
    }

    pub fn tween(&mut self, id: Id, p: Prop, target: f32, ms: u32, ease: Ease) -> f32 {
        self.track(id, p, target, ms, ease, target)
    }

    pub fn enter(&mut self, id: Id, p: Prop, from: f32, target: f32, ms: u32, ease: Ease) -> f32 {
        self.track(id, p, target, ms, ease, from)
    }

    pub fn spring(&mut self, id: Id, p: Prop, target: f32, settle_ms: u32) -> f32 {
        let key = (id, p);
        self.touched.insert(key.clone());
        if !self.cfg.enabled {
            self.vals.remove(&key);
            return target;
        }
        let dt = self.dt;
        let v = self.vals.entry(key).or_insert_with(|| Val::Spring { x: target, v: 0.0, target, omega: Spring::settling(settle_ms).omega });
        if let Val::Spring { target: t, omega, .. } = v {
            *t = target;
            *omega = Spring::settling(settle_ms).omega;
        } else {
            *v = Val::Spring { x: v.value(), v: 0.0, target, omega: Spring::settling(settle_ms).omega };
        }
        let out = v.step(dt);
        if !v.settled() {
            self.moving = true;
        }
        out
    }

    pub fn hover(&mut self, id: Id, target: f32) -> f32 {
        self.tween(id, Prop::Hover, target, 140, Ease::OutCubic)
    }

    pub fn restart(&mut self, id: Id, p: Prop, from: f32, target: f32, ms: u32, ease: Ease) -> f32 {
        self.vals.remove(&(id.clone(), p));
        self.enter(id, p, from, target, ms, ease)
    }

    fn track(&mut self, id: Id, p: Prop, target: f32, ms: u32, ease: Ease, start: f32) -> f32 {
        let key = (id, p);
        self.touched.insert(key.clone());
        if !self.cfg.enabled {
            self.vals.remove(&key);
            return target;
        }
        let dur = self.duration(ms);
        let dt = self.dt;
        if !self.vals.contains_key(&key) {
            if start == target {
                return target;
            }
            self.vals.insert(key.clone(), Val::Tween { from: start, to: target, t: 0.0, dur, ease });
        }
        let v = self.vals.get_mut(&key).expect("just seen");
        if v.target() != target {
            let x = v.value();
            let span = (target - x).abs();
            if span < 1e-4 {
                *v = Val::Tween { from: target, to: target, t: 1.0, dur, ease };
            } else {
                let full = match &*v {
                    Val::Tween { from, to, .. } => (*to - *from).abs().max(span),
                    Val::Spring { .. } => span,
                };
                let part = (dur * span / full).clamp(dur * 0.25, dur);
                *v = Val::Tween { from: x, to: target, t: 0.0, dur: part, ease };
            }
        }
        let out = v.step(dt);
        if !v.settled() {
            self.moving = true;
        }
        out
    }

    fn duration(&self, ms: u32) -> f32 {
        ((ms as f32 / self.cfg.speed).clamp(20.0, 2000.0)) / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(m: &mut Motion, dt: f32, work: impl FnOnce(&mut Motion)) {
        m.begin();
        m.dt = dt;
        work(m);
        m.end();
    }

    #[test]
    fn a_tween_settles_on_its_target() {
        let mut m = Motion::default();
        let mut v = 0.0;
        for _ in 0..120 {
            frame(&mut m, 1.0 / 60.0, |m| {
                v = m.enter(Id::probe(), Prop::Opacity, 0.0, 1.0, 160, Ease::OutCubic);
            });
            if !m.running() {
                break;
            }
        }
        assert_eq!(v, 1.0);
        assert!(!m.running(), "a settled tween still asks for frames");
        let mut m = Motion::default();
        let mut frames = 0;
        loop {
            frames += 1;
            frame(&mut m, 1.0 / 60.0, |m| {
                m.enter(Id::probe(), Prop::Opacity, 0.0, 1.0, 160, Ease::OutCubic);
            });
            if !m.running() {
                break;
            }
            assert!(frames < 40, "160 ms at 60 fps took {frames} frames");
        }
        assert!(frames <= 12, "160 ms took {frames} frames");
    }

    #[test]
    fn a_retarget_keeps_where_it_was() {
        let mut m = Motion::default();
        let mut v = 0.0;
        frame(&mut m, 1.0 / 60.0, |m| {
            v = m.enter(Id::probe(), Prop::Hover, 0.0, 1.0, 200, Ease::OutCubic);
        });
        for _ in 0..5 {
            frame(&mut m, 1.0 / 60.0, |m| {
                v = m.enter(Id::probe(), Prop::Hover, 0.0, 1.0, 200, Ease::OutCubic);
            });
        }
        let mid = v;
        assert!(mid > 0.0 && mid < 1.0, "expected to be mid-flight, got {mid}");
        let mut after = 0.0;
        frame(&mut m, 1.0 / 60.0, |m| {
            after = m.tween(Id::probe(), Prop::Hover, 0.0, 200, Ease::OutCubic);
        });
        assert!(after <= mid, "a retarget jumped up: {mid} -> {after}");
        assert!(after >= 0.0, "a retarget overshot: {mid} -> {after}");
        for _ in 0..120 {
            frame(&mut m, 1.0 / 60.0, |m| {
                after = m.tween(Id::probe(), Prop::Hover, 0.0, 200, Ease::OutCubic);
            });
            if !m.running() {
                break;
            }
        }
        assert_eq!(after, 0.0);
        let mut m = Motion::default();
        let mut v = 0.0;
        frame(&mut m, 1.0 / 60.0, |m| {
            v = m.enter(Id::probe(), Prop::Hover, 0.0, 1.0, 200, Ease::OutCubic);
        });
        assert!(v > 0.0 && v < 1.0, "the entrance should have moved: {v}");
        frame(&mut m, 1.0 / 60.0, |m| {
            v = m.tween(Id::probe(), Prop::Hover, v, 200, Ease::OutCubic);
        });
        assert!(!m.running(), "a value retargeted to itself still asks for frames");
    }

    #[test]
    fn a_spring_settles_without_overshooting() {
        let mut m = Motion::default();
        let mut v = 0.0;
        frame(&mut m, 1.0 / 60.0, |m| {
            v = m.spring(Id::Tab(1), Prop::Y, 0.0, 220);
        });
        assert_eq!(v, 0.0);
        let mut seen = Vec::new();
        for _ in 0..240 {
            frame(&mut m, 1.0 / 60.0, |m| {
                v = m.spring(Id::Tab(1), Prop::Y, 100.0, 220);
            });
            seen.push(v);
            if !m.running() {
                break;
            }
        }
        assert!(seen.len() > 5, "the spring landed in {} frames", seen.len());
        assert!((v - 100.0).abs() < 0.5, "spring did not settle: {v}");
        assert!(seen.iter().all(|x| *x <= 100.001), "a critically damped spring overshot");
        let before = seen[4];
        let mut v2 = before;
        for _ in 0..240 {
            frame(&mut m, 1.0 / 60.0, |m| {
                v2 = m.spring(Id::Tab(1), Prop::Y, 0.0, 220);
            });
            if !m.running() {
                break;
            }
        }
        assert!(v2.abs() < 0.5, "retargeted spring did not settle: {v2}");
        assert!(v2 >= -0.5, "the retargeted spring overshot: {v2}");
    }

    #[test]
    fn frame_time_not_frame_count_decides() {
        let mut a = Motion::default();
        let mut b = Motion::default();
        let (mut va, mut vb) = (0.0, 0.0);
        for _ in 0..12 {
            frame(&mut a, 1.0 / 60.0, |m| {
                va = m.enter(Id::probe(), Prop::Opacity, 0.0, 1.0, 200, Ease::OutCubic);
            });
        }
        for _ in 0..6 {
            frame(&mut b, 1.0 / 30.0, |m| {
                vb = m.enter(Id::probe(), Prop::Opacity, 0.0, 1.0, 200, Ease::OutCubic);
            });
        }
        assert!((va - vb).abs() < 0.05, "60 fps got {va}, 30 fps got {vb}");
        let mut c = Motion::default();
        c.last = Instant::now() - std::time::Duration::from_secs(5);
        c.begin();
        assert!(c.dt() <= 0.05, "a stalled frame stepped {}", c.dt());
    }

    #[test]
    fn reduced_motion_is_instant() {
        let mut m = Motion::default();
        m.set_config(Config::of("reduced", 100, true, true));
        frame(&mut m, 1.0 / 60.0, |m| {
            let v = m.enter(Id::probe(), Prop::Opacity, 0.0, 1.0, 200, Ease::OutCubic);
            assert_eq!(v, 1.0);
            let w = m.spring(Id::Tab(2), Prop::Y, 42.0, 200);
            assert_eq!(w, 42.0);
            let h = m.hover(Id::Button("gear"), 1.0);
            assert_eq!(h, 1.0);
        });
        assert!(!m.running());
        m.set_config(Config::of("off", 100, true, true));
        frame(&mut m, 1.0 / 60.0, |m| {
            assert_eq!(m.enter(Id::probe(), Prop::Opacity, 0.0, 1.0, 200, Ease::OutCubic), 1.0);
        });
        m.set_config(Config::of("full", 100, true, false));
        frame(&mut m, 1.0 / 60.0, |m| {
            assert_eq!(m.enter(Id::probe(), Prop::Opacity, 0.0, 1.0, 200, Ease::OutCubic), 1.0);
        });
        m.set_config(Config::of("full", 100, false, false));
        frame(&mut m, 1.0 / 60.0, |m| {
            assert!(m.enter(Id::probe(), Prop::Opacity, 0.0, 1.0, 200, Ease::OutCubic) < 1.0);
        });
    }

    #[test]
    fn nothing_animating_means_no_redraw_request() {
        let mut m = Motion::default();
        frame(&mut m, 1.0 / 60.0, |m| {
            m.enter(Id::Button("gear"), Prop::Hover, 1.0, 1.0, 140, Ease::OutCubic);
            m.tween(Id::Button("gear"), Prop::Hover, 0.0, 140, Ease::OutCubic);
        });
        assert!(!m.running(), "a value already at rest asks for a frame");
        frame(&mut m, 1.0 / 60.0, |_| {});
        assert!(!m.running());
        let mut m = Motion::default();
        frame(&mut m, 1.0 / 60.0, |m| {
            m.enter(Id::Panel("settings"), Prop::Opacity, 0.0, 1.0, 200, Ease::OutCubic);
        });
        for _ in 0..40 {
            frame(&mut m, 1.0 / 60.0, |_| {});
        }
        let mut v = 1.0;
        frame(&mut m, 1.0 / 60.0, |m| {
            v = m.enter(Id::Panel("settings"), Prop::Opacity, 0.0, 1.0, 200, Ease::OutCubic);
        });
        assert!(v < 0.3, "an untouched value was kept alive: {v}");
        let mut slow = Motion::default();
        slow.set_config(Config { enabled: true, speed: 0.5 });
        frame(&mut slow, 1.0 / 60.0, |m| {
            m.enter(Id::probe(), Prop::Opacity, 0.0, 1.0, 200, Ease::OutCubic);
        });
        let mut fast = Motion::default();
        fast.set_config(Config { enabled: true, speed: 2.0 });
        frame(&mut fast, 1.0 / 60.0, |m| {
            m.enter(Id::probe(), Prop::Opacity, 0.0, 1.0, 200, Ease::OutCubic);
        });
        assert!(fast.running() == slow.running());
    }

    #[test]
    fn easings_start_and_finish_where_they_should() {
        for e in [Ease::OutCubic, Ease::OutQuint] {
            assert_eq!(e.at(0.0), 0.0);
            assert_eq!(e.at(1.0), 1.0);
            assert_eq!(e.at(2.0), 1.0);
            let mut last = 0.0;
            for n in 0..=20 {
                let v = e.at(n as f32 / 20.0);
                assert!(v >= last, "{e:?} went backwards at {n}");
                assert!(v >= n as f32 / 20.0 - 0.001);
                last = v;
            }
        }
        assert!(Ease::OutQuint.at(0.4) > Ease::OutCubic.at(0.4), "quint is the softer one later, faster first");
    }

    #[test]
    fn the_settings_decide_whether_motion_runs() {
        assert!(animate("full", false, false));
        assert!(animate("full", true, true));
        assert!(!animate("full", true, false));
        assert!(!animate("reduced", false, true));
        assert!(!animate("reduced", true, true));
        assert!(!animate("off", false, true));
        assert_eq!(Config::of("full", 150, false, true).speed, 1.5);
        assert_eq!(Config::of("full", 0, false, true).speed, 0.25);
        assert_eq!(Config::of("full", 9000, false, true).speed, 3.0);
    }
}

#[cfg(test)]
impl Id {
    fn probe() -> Id {
        Id::Button("test")
    }
}
