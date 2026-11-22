//! The 3D canvas's camera: matrices, projection, the pointer's ray, the
//! dive's easing. No GL and no egui: plain `[f32; 3]` and column-major
//! `[f32; 16]`, so native tests cover all of it (`cam3d_tests.rs`).
//!
//! Every function takes hostile input: a zero-size viewport, an eye on its
//! target, a NaN or an infinity. They answer `None` or a sane default and
//! never a NaN.

pub type V3 = [f32; 3];
/// Column-major, as GL takes it.
pub type M4 = [f32; 16];

const UP: V3 = [0.0, 1.0, 0.0];
/// Nearest and farthest the view draws, in world units.
pub const NEAR: f32 = 0.1;
pub const FAR: f32 = 400.0;
/// Vertical field of view, radians (50 degrees).
pub const FOV_Y: f32 = 0.872_664_6;
/// How long a dive or a climb takes, seconds.
pub const FLIGHT: f64 = 0.3;

#[must_use]
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[must_use]
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[must_use]
pub fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

#[must_use]
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[must_use]
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[must_use]
pub fn finite(a: V3) -> bool {
    a.iter().all(|v| v.is_finite())
}

/// `a` as a unit vector; `fallback` when it has no length (or is not a number).
#[must_use]
pub fn unit(a: V3, fallback: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l.is_finite() && l > 1e-9 {
        scale(a, 1.0 / l)
    } else {
        fallback
    }
}

#[must_use]
pub fn lerp(a: V3, b: V3, t: f32) -> V3 {
    add(a, scale(sub(b, a), t))
}

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: V3,
    pub max: V3,
}

impl Aabb {
    #[must_use]
    pub const fn new(min: V3, max: V3) -> Self {
        Self { min, max }
    }

    #[must_use]
    pub fn center(&self) -> V3 {
        scale(add(self.min, self.max), 0.5)
    }

    #[must_use]
    pub fn union(&self, o: &Self) -> Self {
        let mut r = *self;
        for i in 0..3 {
            r.min[i] = r.min[i].min(o.min[i]);
            r.max[i] = r.max[i].max(o.max[i]);
        }
        r
    }

    #[must_use]
    pub fn corners(&self) -> [V3; 8] {
        let (a, b) = (self.min, self.max);
        [
            [a[0], a[1], a[2]],
            [b[0], a[1], a[2]],
            [a[0], b[1], a[2]],
            [b[0], b[1], a[2]],
            [a[0], a[1], b[2]],
            [b[0], a[1], b[2]],
            [a[0], b[1], b[2]],
            [b[0], b[1], b[2]],
        ]
    }
}

/// A ray: where from, and a unit direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: V3,
    pub dir: V3,
}

impl Ray {
    /// The distance along the ray where it enters `b` (0 when it starts
    /// inside), or `None`. The slab test; a direction with a zero part is
    /// a ray parallel to that slab.
    #[must_use]
    pub fn hit(&self, b: &Aabb) -> Option<f32> {
        if !finite(self.origin) || !finite(self.dir) {
            return None;
        }
        let (mut t0, mut t1) = (0.0_f32, f32::INFINITY);
        for i in 0..3 {
            let (o, d) = (self.origin[i], self.dir[i]);
            if d.abs() < 1e-12 {
                if o < b.min[i] || o > b.max[i] {
                    return None;
                }
                continue;
            }
            let (mut a, mut c) = ((b.min[i] - o) / d, (b.max[i] - o) / d);
            if a > c {
                std::mem::swap(&mut a, &mut c);
            }
            t0 = t0.max(a);
            t1 = t1.min(c);
            if t0 > t1 {
                return None;
            }
        }
        t0.is_finite().then_some(t0)
    }
}

/// Where the camera is and what it looks at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub eye: V3,
    pub target: V3,
}

impl Pose {
    /// The same pose, or the default one if any part is not a number.
    #[must_use]
    pub fn sane(self) -> Self {
        if finite(self.eye) && finite(self.target) {
            self
        } else {
            Self::default()
        }
    }

    /// The unit vectors right, up and forward of the camera.
    #[must_use]
    pub fn basis(&self) -> (V3, V3, V3) {
        let f = unit(sub(self.target, self.eye), [0.0, 0.0, -1.0]);
        // Looking straight up or down: any right will do.
        let r = unit(cross(f, UP), [1.0, 0.0, 0.0]);
        let u = cross(r, f);
        (r, u, f)
    }
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            eye: [0.0, 8.0, 10.0],
            target: [0.0, 0.0, 0.0],
        }
    }
}

/// The view direction every framing looks along: from the front and above.
#[must_use]
pub fn view_dir() -> V3 {
    unit([0.0, -0.8, -0.6], [0.0, -1.0, 0.0])
}

/// The pose that frames `b` whole in a viewport of `aspect` (width over
/// height) looking along `dir`: exact for the box's corners, not for a
/// sphere round them, so a long thin board fills the width.
#[must_use]
pub fn framing(b: &Aabb, aspect: f32, dir: V3) -> Pose {
    let aspect = if aspect.is_finite() && aspect > 0.05 {
        aspect
    } else {
        1.0
    };
    let c = b.center();
    let at = Pose {
        eye: sub(c, dir),
        target: c,
    };
    let (r, u, f) = at.basis();
    let (mut er, mut eu, mut ef) = (0.0_f32, 0.0_f32, 0.0_f32);
    for p in b.corners() {
        let d = sub(p, c);
        er = er.max(dot(d, r).abs());
        eu = eu.max(dot(d, u).abs());
        ef = ef.max(dot(d, f).abs());
    }
    let th = (FOV_Y / 2.0).tan();
    // A margin of a tenth round the box.
    let need = (eu / th).max(er / (th * aspect)) * 1.1 + ef;
    let dist = if need.is_finite() { need.max(1.0) } else { 10.0 };
    Pose {
        eye: sub(c, scale(f, dist)),
        target: c,
    }
    .sane()
}

/// What the camera sees through: a pose and the size of the viewport in
/// points (any unit, only the ratio and the mapping to the screen matter).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub pose: Pose,
    pub size: (f32, f32),
}

impl Camera {
    #[must_use]
    pub fn new(pose: Pose, size: (f32, f32)) -> Self {
        Self {
            pose: pose.sane(),
            size,
        }
    }

    /// Whether the viewport has an area to draw in.
    #[must_use]
    pub fn drawable(&self) -> bool {
        self.size.0.is_finite() && self.size.1.is_finite() && self.size.0 >= 1.0 && self.size.1 >= 1.0
    }

    #[must_use]
    pub fn aspect(&self) -> f32 {
        if self.drawable() {
            self.size.0 / self.size.1
        } else {
            1.0
        }
    }

    /// Projection times view.
    #[must_use]
    pub fn view_proj(&self) -> M4 {
        mul(
            &perspective(FOV_Y, self.aspect(), NEAR, FAR),
            &look_at(&self.pose),
        )
    }

    /// The screen position (points from the viewport's top left) and the
    /// depth (0 near, 1 far) of the world point `p`; `None` for a point
    /// behind the camera, outside the depth range or not a number, or a
    /// viewport with no area.
    #[must_use]
    pub fn project(&self, p: V3) -> Option<(f32, f32, f32)> {
        if !self.drawable() || !finite(p) {
            return None;
        }
        let (r, u, f) = self.pose.basis();
        let d = sub(p, self.pose.eye);
        let z = dot(d, f);
        if !(NEAR..=FAR).contains(&z) {
            return None;
        }
        let th = (FOV_Y / 2.0).tan();
        let nx = dot(d, r) / (z * th * self.aspect());
        let ny = dot(d, u) / (z * th);
        let x = (nx * 0.5 + 0.5) * self.size.0;
        let y = (0.5 - ny * 0.5) * self.size.1;
        (x.is_finite() && y.is_finite()).then_some((x, y, (z - NEAR) / (FAR - NEAR)))
    }

    /// The ray through the screen point (`x`, `y`) in the viewport's points.
    #[must_use]
    pub fn ray(&self, x: f32, y: f32) -> Option<Ray> {
        if !self.drawable() || !x.is_finite() || !y.is_finite() {
            return None;
        }
        let (r, u, f) = self.pose.basis();
        let th = (FOV_Y / 2.0).tan();
        let nx = (x / self.size.0 - 0.5) * 2.0;
        let ny = (0.5 - y / self.size.1) * 2.0;
        let dir = add(f, add(scale(r, nx * th * self.aspect()), scale(u, ny * th)));
        Some(Ray {
            origin: self.pose.eye,
            dir: unit(dir, f),
        })
    }
}

/// The nearest of `boxes` the ray enters, as an index into the slice.
#[must_use]
pub fn pick(ray: &Ray, boxes: impl IntoIterator<Item = Aabb>) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, b) in boxes.into_iter().enumerate() {
        if let Some(t) = ray.hit(&b)
            && best.is_none_or(|(_, bt)| t < bt)
        {
            best = Some((i, t));
        }
    }
    best.map(|(i, _)| i)
}

/// Looking from the pose's eye at its target, y up.
#[must_use]
pub fn look_at(p: &Pose) -> M4 {
    let (r, u, f) = p.sane().basis();
    let e = p.sane().eye;
    [
        r[0],
        u[0],
        -f[0],
        0.0,
        r[1],
        u[1],
        -f[1],
        0.0,
        r[2],
        u[2],
        -f[2],
        0.0,
        -dot(r, e),
        -dot(u, e),
        dot(f, e),
        1.0,
    ]
}

/// A GL perspective matrix (clip depth -1 to 1).
#[must_use]
pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> M4 {
    let t = 1.0 / (fov_y / 2.0).tan();
    let mut m = [0.0; 16];
    m[0] = t / aspect;
    m[5] = t;
    m[10] = (far + near) / (near - far);
    m[11] = -1.0;
    m[14] = 2.0 * far * near / (near - far);
    m
}

/// `a` times `b` (column-major).
#[must_use]
pub fn mul(a: &M4, b: &M4) -> M4 {
    let mut m = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            m[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    m
}

/// Slow at both ends: the cubic ease in and out; `t` is clamped to 0..=1.
#[must_use]
pub fn ease(t: f32) -> f32 {
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 1.0 };
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// A move of the camera from one pose to another over [`FLIGHT`] seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flight {
    pub from: Pose,
    pub to: Pose,
    pub t0: f64,
    pub dur: f64,
}

impl Flight {
    #[must_use]
    pub fn new(from: Pose, to: Pose, t0: f64) -> Self {
        Self {
            from: from.sane(),
            to: to.sane(),
            t0,
            dur: FLIGHT,
        }
    }

    /// The pose at page time `now`, and whether the flight is over. A clock
    /// that runs backwards stays at the start; one that is not a number
    /// lands.
    #[must_use]
    pub fn at(&self, now: f64) -> (Pose, bool) {
        let s = if !now.is_finite() || self.dur <= 0.0 {
            1.0
        } else {
            ((now - self.t0) / self.dur).clamp(0.0, 1.0)
        };
        if s >= 1.0 {
            return (self.to, true);
        }
        #[allow(clippy::cast_possible_truncation)]
        let e = ease(s as f32);
        (
            Pose {
                eye: lerp(self.from.eye, self.to.eye, e),
                target: lerp(self.from.target, self.to.target, e),
            },
            false,
        )
    }
}
