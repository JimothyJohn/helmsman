//! The 3D view's motion is a function of the clock it is given.

use crate::anim3d::{EASE, FLARE, Fade, Glows, pulse_phase};

#[test]
fn a_glow_eases_on_and_off_instead_of_switching() {
    let mut f = Fade::default();
    f.set(false, 0, 0.0);
    assert_eq!(f.level(0.0), 0.0);
    f.set(true, 0, 1.0);
    assert_eq!(f.level(1.0), 0.0, "starts where it was");
    let mid = f.level(1.0 + EASE / 2.0);
    assert!(mid > 0.3 && mid < 0.7, "{mid}");
    assert_eq!(f.level(1.0 + EASE), 1.0);
    assert_eq!(f.level(99.0), 1.0);
    // Off again part of the way up: it comes down from where it was.
    let mut g = Fade::default();
    g.set(true, 0, 0.0);
    g.set(false, 5, 5.0);
    g.set(true, 0, 5.0 + EASE / 2.0);
    let there = g.level(5.0 + EASE / 2.0);
    assert!(there > 0.3 && there < 0.7, "turning round is not a jump: {there}");
}

#[test]
fn it_is_the_same_at_the_same_time() {
    let run = || {
        let mut f = Fade::default();
        f.set(true, 1, 2.0);
        f.set(true, 2, 2.5);
        (f.level(2.4), f.flare(2.6), f.level(9.0))
    };
    assert_eq!(run(), run());
}

#[test]
fn a_change_flares_and_settles_and_the_first_sight_does_not() {
    let mut f = Fade::default();
    f.set(true, 7, 0.0);
    assert_eq!(f.flare(0.0), 0.0, "first sight");
    f.set(true, 7, 1.0);
    assert_eq!(f.flare(1.0), 0.0, "nothing changed");
    f.set(true, 8, 2.0);
    assert!((f.flare(2.0) - 1.0).abs() < 1e-6);
    let later = f.flare(2.0 + FLARE);
    assert!((later - (-1.0f32).exp()).abs() < 1e-3, "{later}");
    assert_eq!(f.flare(60.0), 0.0, "settled");
    assert_eq!(f.flare(1.0), 0.0, "a clock behind the change");
}

#[test]
fn hostile_clocks_give_numbers() {
    let mut f = Fade::default();
    f.set(true, 1, 0.0);
    for t in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0, 1e300] {
        assert!(f.level(t).is_finite(), "{t}");
        assert!(f.flare(t).is_finite(), "{t}");
        assert!(pulse_phase(t, 1.5).is_finite());
    }
}

#[test]
fn moving_says_when_a_frame_is_wanted() {
    let mut f = Fade::default();
    f.set(false, 0, 0.0);
    assert!(!f.moving(1.0));
    f.set(true, 0, 1.0);
    assert!(f.moving(1.0));
    assert!(f.moving(1.0 + EASE / 2.0));
    assert!(!f.moving(1.0 + EASE + 0.1));
    f.set(true, 1, 3.0);
    assert!(f.moving(3.0), "a flare moves");
    assert!(!f.moving(30.0));
}

#[test]
fn a_scene_of_another_shape_starts_over_without_a_flare() {
    let mut g = Glows::default();
    g.update(&[(true, 1), (false, 2)], &[(true, 0)], 0.0);
    assert_eq!(g.solids.len(), 2);
    assert!(!g.moving(0.0));
    g.update(&[(true, 1)], &[], 1.0);
    assert_eq!((g.solids.len(), g.segs.len()), (1, 0));
    assert!(!g.moving(1.0));
    g.update(&[(true, 9)], &[], 2.0);
    assert!(g.moving(2.0), "a change flares");
}

#[test]
fn the_pulse_phase_wraps_and_advances() {
    assert_eq!(pulse_phase(0.0, 2.0), 0.0);
    assert!((pulse_phase(0.25, 2.0) - 0.5).abs() < 1e-6);
    assert!(pulse_phase(0.75, 2.0) < 0.6 && pulse_phase(0.75, 2.0) > 0.4);
}
