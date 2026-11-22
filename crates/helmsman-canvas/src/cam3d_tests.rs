//! The 3D camera's contract: projection and the pointer's ray agree, a
//! framing holds its box, and hostile input (no viewport, NaN, an eye on
//! its target) never gives a NaN.

use crate::cam3d::{
    Aabb, Camera, FLIGHT, Flight, M4, Pose, Ray, V3, ease, finite, framing, look_at, perspective, pick, unit,
    view_dir,
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
            let c = Camera::new(framing(&b, aspect, view_dir()), size);
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
            let p = framing(&b, aspect, view_dir());
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

#[test]
fn a_flight_leaves_arrives_and_survives_a_bad_clock() {
    let a = Pose {
        eye: [0.0, 10.0, 10.0],
        target: [0.0; 3],
    };
    let b = Pose {
        eye: [5.0, 2.0, 3.0],
        target: [5.0, 0.0, 0.0],
    };
    let f = Flight::new(a, b, 100.0);
    assert_eq!(f.at(100.0), (a, false));
    assert_eq!(
        f.at(50.0),
        (a, false),
        "a clock that ran backwards stays at the start"
    );
    let (mid, done) = f.at(100.0 + FLIGHT / 2.0);
    assert!(!done);
    assert!((mid.eye[0] - 2.5).abs() < 1e-3, "halfway at the half: {mid:?}");
    assert_eq!(f.at(100.0 + FLIGHT + 1e-6), (b, true));
    assert_eq!(f.at(1e9), (b, true));
    assert_eq!(f.at(f64::NAN), (b, true));
    assert_eq!(f.at(f64::INFINITY), (b, true));
    let bad = Flight::new(
        Pose {
            eye: [f32::NAN; 3],
            target: [0.0; 3],
        },
        b,
        0.0,
    );
    assert!(finite(bad.at(0.1).0.eye));
}
