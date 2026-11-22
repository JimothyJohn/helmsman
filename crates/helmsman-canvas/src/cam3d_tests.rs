//! The 3D camera's contract: projection and the pointer's ray agree, a
//! framing holds its box, and hostile input (no viewport, NaN, an eye on
//! its target) never gives a NaN.

use crate::cam3d::{
    Aabb, Camera, FLIGHT, Limits, M4, Orbit, OrbitFlight, PITCH_MAX, PITCH_MIN, Pose, Ray, V3, ease, finite,
    framing, look_at, perspective, pick, unit,
};

fn cam() -> Camera {
    Camera::new(
        Pose {
            eye: [0.0, 8.0, 10.0],
            target: [0.0, 0.0, 0.0],
        },
        (800.0, 600.0),
    )
}

fn all_finite(m: &M4) -> bool {
    m.iter().all(|v| v.is_finite())
}

#[test]
fn a_ray_through_a_projected_point_passes_through_the_point() {
    let c = cam();
    for x in [-3.0, 0.0, 2.5] {
        for y in [0.0, 1.0] {
            for z in [-2.0, 0.0, 4.0] {
                let p = [x, y, z];
                let (sx, sy, _) = c.project(p).expect("in front");
                let r = c.ray(sx, sy).unwrap();
                // The distance from the point to the ray.
                let d = [p[0] - r.origin[0], p[1] - r.origin[1], p[2] - r.origin[2]];
                let along = d[0] * r.dir[0] + d[1] * r.dir[1] + d[2] * r.dir[2];
                let miss = [
                    d[0] - r.dir[0] * along,
                    d[1] - r.dir[1] * along,
                    d[2] - r.dir[2] * along,
                ];
                let off = (miss[0] * miss[0] + miss[1] * miss[1] + miss[2] * miss[2]).sqrt();
                assert!(off < 1e-3, "{p:?} is {off} off its own ray");
            }
        }
    }
}

#[test]
fn the_target_is_the_middle_of_the_screen() {
    let (x, y, _) = cam().project([0.0, 0.0, 0.0]).unwrap();
    assert!((x - 400.0).abs() < 0.01 && (y - 300.0).abs() < 0.01, "{x} {y}");
}

#[test]
fn a_point_behind_the_camera_is_not_on_screen() {
    assert!(cam().project([0.0, 8.0, 40.0]).is_none());
    assert!(cam().project([0.0, 8.0, 10.0]).is_none(), "at the eye");
}

#[test]
fn no_viewport_means_no_projection_and_no_ray_but_a_finite_matrix() {
    for size in [
        (0.0, 0.0),
        (0.0, 600.0),
        (800.0, 0.0),
        (f32::NAN, 5.0),
        (-4.0, 3.0),
        (0.4, 0.4),
    ] {
        let c = Camera::new(Pose::default(), size);
        assert!(!c.drawable(), "{size:?}");
        assert!(c.project([0.0; 3]).is_none(), "{size:?}");
        assert!(c.ray(1.0, 1.0).is_none(), "{size:?}");
        assert!(all_finite(&c.view_proj()), "{size:?}");
        assert!(c.aspect().is_finite() && c.aspect() > 0.0);
    }
}

#[test]
fn not_a_number_in_does_not_come_out() {
    let c = cam();
    assert!(c.project([f32::NAN, 0.0, 0.0]).is_none());
    assert!(c.project([f32::INFINITY, 0.0, 0.0]).is_none());
    assert!(c.ray(f32::NAN, 3.0).is_none());
    let bad = Camera::new(
        Pose {
            eye: [f32::NAN, 0.0, 0.0],
            target: [0.0; 3],
        },
        (100.0, 100.0),
    );
    assert!(finite(bad.pose.eye), "a NaN pose falls back to the default");
    assert!(all_finite(&bad.view_proj()));
}

#[test]
fn an_eye_on_its_target_and_a_view_straight_down_still_have_a_basis() {
    for pose in [
        Pose {
            eye: [1.0, 2.0, 3.0],
            target: [1.0, 2.0, 3.0],
        },
        Pose {
            eye: [0.0, 10.0, 0.0],
            target: [0.0, 0.0, 0.0],
        },
        Pose {
            eye: [0.0, -10.0, 0.0],
            target: [0.0, 0.0, 0.0],
        },
    ] {
        let (r, u, f) = pose.basis();
        for v in [r, u, f] {
            assert!(finite(v));
            assert!(
                (v.iter().map(|a| a * a).sum::<f32>().sqrt() - 1.0).abs() < 1e-4,
                "{v:?}"
            );
        }
        assert!(all_finite(&look_at(&pose)));
    }
    assert_eq!(unit([0.0; 3], [1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
}

#[test]
fn the_projection_matrix_keeps_the_depth_range() {
    let m = perspective(0.9, 1.5, 0.1, 100.0);
    assert!(all_finite(&m));
    assert!((m[11] + 1.0).abs() < 1e-6);
}

#[test]
fn a_framing_holds_every_corner_of_its_box_on_screen() {
    let boxes = [
        Aabb::new([0.0, 0.0, 0.0], [30.0, 1.0, 4.0]),
        Aabb::new([-5.0, 0.0, -5.0], [5.0, 9.7, 5.0]),
        Aabb::new([2.0, 2.0, 2.0], [2.0, 2.0, 2.0]),
    ];
    for b in boxes {
        for aspect in [0.6, 1.0, 1.78, 3.0] {
            let size = (600.0 * aspect, 600.0);
            let c = Camera::new(framing(&b, aspect, [0.0, -0.8, -0.6]), size);
            for p in b.corners() {
                let (x, y, _) = c
                    .project(p)
                    .unwrap_or_else(|| panic!("{p:?} not in view at {aspect}"));
                assert!(
                    (0.0..=size.0).contains(&x) && (0.0..=size.1).contains(&y),
                    "{p:?} at {x},{y} outside {size:?} (aspect {aspect}, box {b:?})"
                );
            }
        }
    }
}

#[test]
fn a_framing_of_nothing_sensible_is_still_a_pose() {
    let nan = Aabb::new([f32::NAN; 3], [f32::NAN; 3]);
    let inf = Aabb::new([-f32::INFINITY; 3], [f32::INFINITY; 3]);
    for b in [nan, inf] {
        for aspect in [0.0, f32::NAN, -1.0, f32::INFINITY] {
            let p = framing(&b, aspect, [0.0, -0.8, -0.6]);
            assert!(finite(p.eye) && finite(p.target), "{b:?} {aspect}");
        }
    }
    let p = framing(&Aabb::new([0.0; 3], [1.0; 3]), 1.0, [0.0; 3]);
    assert!(finite(p.eye), "no direction");
}

#[test]
fn a_ray_enters_a_box_or_misses_it() {
    let b = Aabb::new([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]);
    let at = |o: V3, d: V3| Ray { origin: o, dir: d };
    assert_eq!(at([0.0, 0.0, 5.0], [0.0, 0.0, -1.0]).hit(&b), Some(4.0));
    assert_eq!(
        at([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]).hit(&b),
        Some(0.0),
        "from inside"
    );
    assert_eq!(at([3.0, 0.0, 5.0], [0.0, 0.0, -1.0]).hit(&b), None, "beside it");
    assert_eq!(at([0.0, 0.0, 5.0], [0.0, 0.0, 1.0]).hit(&b), None, "facing away");
    assert_eq!(
        at([0.0, 5.0, 0.0], [1.0, 0.0, 0.0]).hit(&b),
        None,
        "parallel, outside the slab"
    );
    assert_eq!(at([0.0, 0.0, 5.0], [f32::NAN; 3]).hit(&b), None);
}

#[test]
fn the_nearest_box_wins_the_pick() {
    let near = Aabb::new([-1.0; 3], [1.0; 3]);
    let far = Aabb::new([-1.0, -1.0, -9.0], [1.0, 1.0, -7.0]);
    let r = Ray {
        origin: [0.0, 0.0, 10.0],
        dir: [0.0, 0.0, -1.0],
    };
    assert_eq!(pick(&r, [far, near]), Some(1));
    assert_eq!(pick(&r, [far]), Some(0));
    assert_eq!(pick(&r, std::iter::empty()), None);
}

#[test]
fn the_ease_runs_zero_to_one_slow_at_both_ends() {
    assert_eq!(ease(0.0), 0.0);
    assert_eq!(ease(1.0), 1.0);
    assert!((ease(0.5) - 0.5).abs() < 1e-6);
    let mut last = 0.0;
    for i in 0..=100 {
        let e = ease(i as f32 / 100.0);
        assert!(e >= last, "not monotonic at {i}");
        last = e;
    }
    assert!(ease(0.1) < 0.1 && ease(0.9) > 0.9);
    assert_eq!(ease(-3.0), 0.0);
    assert_eq!(ease(9.0), 1.0);
    assert_eq!(ease(f32::NAN), 1.0);
}

fn limits() -> Limits {
    Limits::of(&Aabb::new([0.0, 0.0, 0.0], [30.0, 16.0, 25.0]))
}

fn sane(o: &Orbit, l: &Limits) {
    assert!(
        finite(o.target) && o.yaw.is_finite() && o.pitch.is_finite() && o.dist.is_finite(),
        "{o:?}"
    );
    assert!((PITCH_MIN..=PITCH_MAX).contains(&o.pitch), "{o:?}");
    assert!(
        o.dist >= l.dist.0 - 1e-3 && o.dist <= l.dist.1 + 1e-3,
        "{o:?} within {:?}",
        l.dist
    );
    for i in 0..3 {
        assert!(
            o.target[i] >= l.bounds.min[i] - 1e-3 && o.target[i] <= l.bounds.max[i] + 1e-3,
            "{o:?}"
        );
    }
    assert!(finite(o.pose().eye), "{o:?}");
}

#[test]
fn orbit_pan_and_zoom_stay_finite_and_in_limits_whatever_they_are_given() {
    let l = limits();
    let hostile = [
        0.0,
        1.0,
        -1.0,
        1e6,
        -1e6,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MAX,
    ];
    let mut o = Orbit::default();
    for &a in &hostile {
        for &b in &hostile {
            o = o.orbited(a, b, &l);
            sane(&o, &l);
            o = o.panned(a, b, 600.0, &l);
            sane(&o, &l);
            o = o.zoomed_at(a, b, (800.0, 600.0), a, &l);
            sane(&o, &l);
            o = o.zoomed_at(400.0, 300.0, (0.0, 0.0), b, &l);
            sane(&o, &l);
            o = o.panned(a, b, f32::NAN, &l);
            sane(&o, &l);
        }
    }
    // A camera that started as garbage is made sane too.
    let junk = Orbit {
        target: [f32::NAN, 1e30, -1e30],
        yaw: f32::NAN,
        pitch: 99.0,
        dist: -4.0,
    };
    sane(&junk.limited(&l), &l);
    let nan_limits = Limits::of(&Aabb::new([f32::NAN; 3], [f32::NAN; 3]));
    sane(&Orbit::default().limited(&nan_limits), &nan_limits);
}

#[test]
fn the_pitch_cannot_go_under_the_floor_or_over_the_top() {
    let l = limits();
    let down = Orbit::default().orbited(0.0, -1e4, &l);
    assert_eq!(down.pitch, PITCH_MIN);
    let up = Orbit::default().orbited(0.0, 1e4, &l);
    assert_eq!(up.pitch, PITCH_MAX);
    assert!(down.pose().eye[1] >= down.target[1], "never below its target");
}

#[test]
fn a_drag_turns_the_view_and_a_pan_moves_the_scene_with_the_pointer() {
    let l = limits();
    let o = Orbit::default();
    let turned = o.orbited(100.0, 0.0, &l);
    assert!((turned.yaw - o.yaw).abs() > 0.5 || (turned.yaw - o.yaw).abs() > 5.0);
    // Dragging right moves what is under the pointer right: the target
    // goes left on screen's right axis.
    let size = (800.0, 600.0);
    let p = [10.0, 8.0, 12.0];
    let before = Camera::new(o.pose(), size).project(p).unwrap();
    let moved = o.panned(50.0, 0.0, size.1, &l);
    let after = Camera::new(moved.pose(), size).project(p).unwrap();
    assert!(after.0 > before.0 + 10.0, "{before:?} -> {after:?}");
}

#[test]
fn zooming_toward_the_pointer_keeps_the_point_under_it() {
    let l = limits();
    let size = (900.0, 600.0);
    let o = Orbit {
        target: [15.0, 6.0, 12.0],
        dist: 40.0,
        ..Orbit::default()
    };
    for (px, py) in [(450.0, 300.0), (100.0, 80.0), (800.0, 500.0), (450.0, 50.0)] {
        for factor in [1.5, 0.7, 3.0] {
            let cam = Camera::new(o.pose(), size);
            // The point of the plane through the target that the pointer is on.
            let r = cam.ray(px, py).unwrap();
            let (_, _, f) = o.pose().basis();
            let c = r.dir[0] * f[0] + r.dir[1] * f[1] + r.dir[2] * f[2];
            let t = o.dist / c;
            let p = [
                r.origin[0] + r.dir[0] * t,
                r.origin[1] + r.dir[1] * t,
                r.origin[2] + r.dir[2] * t,
            ];
            let z = o.zoomed_at(px, py, size, factor, &l);
            // Inside the limits it is a plain zoom, and the point stays.
            if (l.dist.0..=l.dist.1).contains(&(o.dist / factor)) {
                assert!((z.dist - o.dist / factor).abs() < 1e-3, "{z:?}");
                let (sx, sy, _) = Camera::new(z.pose(), size).project(p).unwrap();
                assert!(
                    (sx - px).abs() < 0.05 && (sy - py).abs() < 0.05,
                    "{px},{py} -> {sx},{sy} (x{factor})"
                );
            }
        }
    }
}

#[test]
fn a_zoom_cannot_pass_its_limits_and_a_frame_holds_its_box() {
    let l = limits();
    let mut o = Orbit::default();
    for _ in 0..100 {
        o = o.zoomed_at(400.0, 300.0, (800.0, 600.0), 10.0, &l);
    }
    assert_eq!(o.dist, l.dist.0);
    for _ in 0..100 {
        o = o.zoomed_at(400.0, 300.0, (800.0, 600.0), 0.1, &l);
    }
    assert_eq!(o.dist, l.dist.1);
    let b = Aabb::new([0.0, 0.0, 0.0], [30.0, 16.0, 25.0]);
    for aspect in [0.7, 1.0, 1.6, 2.4] {
        let f = Orbit::default().framing(&b, aspect);
        let size = (600.0 * aspect, 600.0);
        let cam = Camera::new(f.pose(), size);
        for p in b.corners() {
            let (x, y, _) = cam.project(p).unwrap();
            assert!(
                (0.0..=size.0).contains(&x) && (0.0..=size.1).contains(&y),
                "{p:?} {x},{y} at {aspect}"
            );
        }
    }
}

#[test]
fn a_flight_leaves_arrives_takes_the_short_way_round_and_survives_a_bad_clock() {
    let a = Orbit {
        yaw: 0.1,
        ..Orbit::default()
    };
    let b = Orbit {
        target: [5.0, 1.0, 3.0],
        yaw: std::f32::consts::TAU - 0.1,
        pitch: 0.9,
        dist: 9.0,
    };
    let f = OrbitFlight::new(a, b, 100.0);
    assert_eq!(f.at(100.0), (a.fixed(), false));
    assert_eq!(
        f.at(50.0),
        (a.fixed(), false),
        "a clock that ran backwards stays at the start"
    );
    let (mid, done) = f.at(100.0 + FLIGHT / 2.0);
    assert!(!done);
    assert!(
        mid.yaw.rem_euclid(std::f32::consts::TAU) < 0.2 || mid.yaw.rem_euclid(std::f32::consts::TAU) > 6.1,
        "{mid:?}"
    );
    assert_eq!(f.at(100.0 + FLIGHT + 1e-6), (b.fixed(), true));
    assert_eq!(f.at(f64::NAN), (b.fixed(), true));
    assert_eq!(f.at(f64::INFINITY), (b.fixed(), true));
    for t in [100.05, 100.1, 100.2] {
        assert!(finite(f.at(t).0.pose().eye));
    }
}
