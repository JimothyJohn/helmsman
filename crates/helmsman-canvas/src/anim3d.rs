//! What moves in the 3D view, as functions of a clock the caller passes
//! in: a glow that eases on and off instead of switching, and a flare
//! when a block's value changes. No egui, no GL, no wall clock: the same
//! clock gives the same numbers (`anim3d_tests.rs`).

/// How long a glow takes to come up or die down, seconds.
pub const EASE: f64 = 0.25;
/// How long a flare takes to fall to a third of its height, seconds.
pub const FLARE: f64 = 0.3;

/// One thing's glow: eases toward on or off; a change of its value flares.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    from: f32,
    on: bool,
    t0: f64,
    flare_t: f64,
    sig: Option<u64>,
}

impl Default for Fade {
    fn default() -> Self {
        Self {
            from: 0.0,
            on: false,
            t0: f64::NEG_INFINITY,
            flare_t: f64::NEG_INFINITY,
            sig: None,
        }
    }
}

fn smooth(s: f64) -> f32 {
    let s = if s.is_finite() { s.clamp(0.0, 1.0) } else { 1.0 };
    #[allow(clippy::cast_possible_truncation)]
    let s = s as f32;
    s * s * (3.0 - 2.0 * s)
}

impl Fade {
    /// 0 to 1: how lit it is at `now`.
    #[must_use]
    pub fn level(&self, now: f64) -> f32 {
        let to = f32::from(u8::from(self.on));
        if !self.t0.is_finite() {
            return to;
        }
        self.from + (to - self.from) * smooth((now - self.t0) / EASE)
    }

    /// 0 to 1: the flare of its last change, falling.
    #[must_use]
    pub fn flare(&self, now: f64) -> f32 {
        let dt = now - self.flare_t;
        if !dt.is_finite() || dt < 0.0 {
            return 0.0;
        }
        #[allow(clippy::cast_possible_truncation)]
        let f = (-dt / FLARE).exp() as f32;
        if f < 0.01 { 0.0 } else { f }
    }

    /// States this frame's truth at `now`: whether it is lit, and a number
    /// for what it carries. The first sight of a thing does not flare.
    pub fn set(&mut self, on: bool, sig: u64, now: f64) {
        if on != self.on {
            self.from = self.level(now);
            self.on = on;
            self.t0 = now;
        }
        if self.sig.is_some_and(|s| s != sig) {
            self.flare_t = now;
        }
        self.sig = Some(sig);
    }

    /// Whether anything is still moving at `now`: the frame wants another.
    #[must_use]
    pub fn moving(&self, now: f64) -> bool {
        let to = f32::from(u8::from(self.on));
        (self.level(now) - to).abs() > 1e-3 || self.flare(now) > 0.0
    }
}

/// The glow of every solid and segment of a scene, by index.
#[derive(Clone, Debug, Default)]
pub struct Glows {
    pub solids: Vec<Fade>,
    pub segs: Vec<Fade>,
}

impl Glows {
    /// Takes this frame's state: `solids` and `segs` are (lit, signature)
    /// per thing, in the scene's order. A scene of another shape starts
    /// over (no flare for what is new).
    pub fn update(&mut self, solids: &[(bool, u64)], segs: &[(bool, u64)], now: f64) {
        fn one(v: &mut Vec<Fade>, now_state: &[(bool, u64)], now: f64) {
            if v.len() != now_state.len() {
                *v = now_state
                    .iter()
                    .map(|(on, sig)| Fade {
                        on: *on,
                        sig: Some(*sig),
                        ..Fade::default()
                    })
                    .collect();
                return;
            }
            for (f, (on, sig)) in v.iter_mut().zip(now_state) {
                f.set(*on, *sig, now);
            }
        }
        one(&mut self.solids, solids, now);
        one(&mut self.segs, segs, now);
    }

    /// Whether anything is easing or flaring at `now`.
    #[must_use]
    pub fn moving(&self, now: f64) -> bool {
        self.solids.iter().chain(&self.segs).any(|f| f.moving(now))
    }
}

/// The phase of the pulses along a wire at `now`: how many wavelengths
/// they have travelled, `speed` of them a second.
#[must_use]
pub fn pulse_phase(now: f64, speed: f64) -> f32 {
    if !now.is_finite() {
        return 0.0;
    }
    #[allow(clippy::cast_possible_truncation)]
    let p = (now * speed).rem_euclid(1.0) as f32;
    p
}
