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
/// Vertical field of view, radians (34 degrees: a deep scene keeps its
/// far side readable).
pub const FOV_Y: f32 = 0.6;
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

/// The pose that frames `b` whole in a viewport of `aspect` (width over
/// height) looking along `dir`.
#[cfg(test)]
#[must_use]
pub fn framing(b: &Aabb, aspect: f32, dir: V3) -> Pose {
    framing_points(&b.corners(), aspect, dir)
}

/// The pose that frames the points `pts` (the corners of what is there,
/// not of a box round it, which holds empty corners the near edge of the
/// view would otherwise be spent on).
#[must_use]
pub fn framing_points(pts: &[V3], aspect: f32, dir: V3) -> Pose {
    let aspect = if aspect.is_finite() && aspect > 0.05 {
        aspect
    } else {
        1.0
    };
    let pts: Vec<V3> = pts.iter().copied().filter(|p| finite(*p)).collect();
    let Some(first) = pts.first().copied() else {
        return Pose::default();
    };
    let (mut lo, mut hi) = (first, first);
    for p in &pts {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let mut c = scale(add(lo, hi), 0.5);
    let th = (FOV_Y / 2.0).tan();
    let (r, u, f) = Pose {
        eye: sub(c, dir),
        target: c,
    }
    .basis();
    // The distance at which each point, seen in perspective, is just on
    // screen from `c`: the nearer ones need more room than the middle
    // does. Eight percent of margin round it.
    let need = |c: V3| {
        let mut need = 0.0_f32;
        for p in &pts {
            let d = scale(sub(*p, c), 1.08);
            let side = (dot(d, r).abs() / (th * aspect)).max(dot(d, u).abs() / th);
            need = need.max(side - dot(d, f));
        }
        if need.is_finite() { need.max(1.0) } else { 10.0 }
    };
    // Perspective makes what is near bigger, so the middle of the box is
    // not the middle of the picture: slide the target until the
    // picture's extent is centred, and fit again.
    for _ in 0..4 {
        let dist = need(c);
        let eye = sub(c, scale(f, dist));
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &pts {
            let d = sub(*p, eye);
            let z = dot(d, f).max(NEAR);
            let (nx, ny) = (dot(d, r) / (z * th * aspect), dot(d, u) / (z * th));
            x0 = x0.min(nx);
            x1 = x1.max(nx);
            y0 = y0.min(ny);
            y1 = y1.max(ny);
        }
        let shift = add(
            scale(r, f32::midpoint(x0, x1) * dist * th * aspect),
            scale(u, f32::midpoint(y0, y1) * dist * th),
        );
        c = add(c, shift);
    }
    let dist = need(c);
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

/// How far down and up the camera may look, radians over the floor.
pub const PITCH_MIN: f32 = 0.06;
pub const PITCH_MAX: f32 = 1.5;
/// The angles the view opens at: a little from the left, well above, so
/// the stack's height shows.
pub const YAW0: f32 = -0.55;
pub const PITCH0: f32 = 0.55;

/// An orbit camera: it looks at `target` from `dist` away along the
/// direction `yaw` and `pitch` give. The one state the pointer, the keys
/// and a flight all move; [`Self::pose`] is what the matrices are made of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orbit {
    pub target: V3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            target: [0.0; 3],
            yaw: YAW0,
            pitch: PITCH0,
            dist: 20.0,
        }
    }
}

/// What keeps the camera from losing the scene: where its target may be
/// and how near and far it may stand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub bounds: Aabb,
    pub dist: (f32, f32),
}

impl Limits {
    /// Limits for a scene in `b`: the target stays in the box (with a
    /// margin of a third of its size), the camera between 2 units and six
    /// radii away.
    #[must_use]
    pub fn of(b: &Aabb) -> Self {
        let b = if finite(b.min) && finite(b.max) {
            *b
        } else {
            Aabb::new([-1.0; 3], [1.0; 3])
        };
        let r = sub(b.max, b.min);
        let radius = dot(r, r).sqrt() / 2.0;
        let m = scale(r, 1.0 / 3.0);
        Self {
            bounds: Aabb::new(sub(b.min, m), add(b.max, m)),
            dist: (2.0, (radius * 6.0).max(8.0)),
        }
    }
}

fn num(v: f32, or: f32) -> f32 {
    if v.is_finite() { v } else { or }
}

impl Orbit {
    /// The view direction (from the camera toward the target).
    #[must_use]
    pub fn dir(&self) -> V3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        // yaw 0 looks along -z; pitch looks down.
        unit([sy * cp, -sp, -cy * cp], [0.0, -1.0, 0.0])
    }

    #[must_use]
    pub fn pose(&self) -> Pose {
        let o = self.fixed();
        Pose {
            eye: sub(o.target, scale(o.dir(), o.dist)),
            target: o.target,
        }
    }

    /// The same camera with every part a number and the angles and
    /// distance in range.
    #[must_use]
    pub fn fixed(&self) -> Self {
        Self {
            target: if finite(self.target) {
                self.target
            } else {
                [0.0; 3]
            },
            yaw: num(self.yaw, YAW0).rem_euclid(std::f32::consts::TAU),
            pitch: num(self.pitch, PITCH0).clamp(PITCH_MIN, PITCH_MAX),
            dist: num(self.dist, 20.0).clamp(0.5, 5000.0),
        }
    }

    /// Back inside `l`.
    #[must_use]
    pub fn limited(&self, l: &Limits) -> Self {
        let mut o = self.fixed();
        for i in 0..3 {
            o.target[i] = o.target[i].clamp(l.bounds.min[i], l.bounds.max[i]);
        }
        o.dist = o.dist.clamp(l.dist.0, l.dist.1);
        o
    }

    /// Turns by `dx`, `dy` pixels (a pixel is a hundredth of a radian);
    /// a hostile delta is clamped to a turn.
    #[must_use]
    pub fn orbited(&self, dx: f32, dy: f32, l: &Limits) -> Self {
        let k = 0.01;
        let lim = std::f32::consts::TAU;
        let (dx, dy) = (
            num(dx, 0.0).clamp(-lim / k, lim / k),
            num(dy, 0.0).clamp(-lim / k, lim / k),
        );
        Self {
            yaw: self.yaw - dx * k,
            pitch: self.pitch + dy * k,
            ..*self
        }
        .limited(l)
    }

    /// Moves the view by `dx`, `dy` pixels of a viewport `height` points
    /// tall: the scene follows the pointer.
    #[must_use]
    pub fn panned(&self, dx: f32, dy: f32, height: f32, l: &Limits) -> Self {
        let o = self.fixed();
        let h = if height.is_finite() && height >= 1.0 {
            height
        } else {
            1.0
        };
        let per_px = 2.0 * o.dist * (FOV_Y / 2.0).tan() / h;
        let (r, u, _) = o.pose().basis();
        let d = add(scale(r, -num(dx, 0.0) * per_px), scale(u, num(dy, 0.0) * per_px));
        Self {
            target: add(o.target, d),
            ..o
        }
        .limited(l)
    }

    /// Zooms by `factor` (above 1 closer) about the screen point
    /// (`x`, `y`) of a viewport `size`: the point of the scene under it
    /// stays under it.
    #[must_use]
    pub fn zoomed_at(&self, x: f32, y: f32, size: (f32, f32), factor: f32, l: &Limits) -> Self {
        let o = self.fixed();
        let cam = Camera::new(o.pose(), size);
        let factor = if factor.is_finite() && factor > 0.0 {
            factor.clamp(0.1, 10.0)
        } else {
            1.0
        };
        let dist = (o.dist / factor).clamp(l.dist.0, l.dist.1);
        let Some(ray) = cam.ray(x, y) else {
            return Self { dist, ..o }.limited(l);
        };
        let (_, _, f) = o.pose().basis();
        let c = dot(ray.dir, f);
        if c < 1e-4 {
            return Self { dist, ..o }.limited(l);
        }
        // The point on the plane through the target, at depth `dist`.
        let p = add(ray.origin, scale(ray.dir, o.dist / c));
        let eye = sub(p, scale(ray.dir, dist / c));
        Self {
            target: add(eye, scale(f, dist)),
            dist,
            ..o
        }
        .limited(l)
    }

    /// Frames `b` whole in a viewport of `aspect`, looking along this
    /// camera's angles.
    #[must_use]
    pub fn framing(&self, b: &Aabb, aspect: f32) -> Self {
        self.framing_points(&b.corners(), aspect)
    }

    /// Frames the points `pts`.
    #[must_use]
    pub fn framing_points(&self, pts: &[V3], aspect: f32) -> Self {
        let o = self.fixed();
        let p = framing_points(pts, aspect, o.dir());
        let d = sub(p.eye, p.target);
        Self {
            target: p.target,
            dist: dot(d, d).sqrt(),
            ..o
        }
        .fixed()
    }
}

/// A move of the orbit camera from one state to another, eased over
/// [`FLIGHT`]; the angle takes the short way round.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitFlight {
    pub from: Orbit,
    pub to: Orbit,
    pub t0: f64,
    pub dur: f64,
}

impl OrbitFlight {
    #[must_use]
    pub fn new(from: Orbit, to: Orbit, t0: f64) -> Self {
        Self {
            from: from.fixed(),
            to: to.fixed(),
            t0,
            dur: FLIGHT,
        }
    }

    /// The camera at page time `now`, and whether the flight is over.
    #[must_use]
    pub fn at(&self, now: f64) -> (Orbit, bool) {
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
        let tau = std::f32::consts::TAU;
        let mut dy = (self.to.yaw - self.from.yaw).rem_euclid(tau);
        if dy > tau / 2.0 {
            dy -= tau;
        }
        // A dive keeps a stretch of dolly in the middle: the distance
        // moves by equal factors, as the 2D view's glide does.
        let dist = self.from.dist * (self.to.dist / self.from.dist).powf(e);
        (
            Orbit {
                target: lerp(self.from.target, self.to.target, e),
                yaw: self.from.yaw + dy * e,
                pitch: self.from.pitch + (self.to.pitch - self.from.pitch) * e,
                dist,
            }
            .fixed(),
            false,
        )
    }
}
