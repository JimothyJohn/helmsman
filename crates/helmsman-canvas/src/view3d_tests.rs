//! The 3D view's navigation (a dive and the way back) and its parts that
//! need an egui frame, run with no GL: a paint callback is only a shape
//! to a headless context.

use eframe::egui::{self, Event, Pos2, Rect, Vec2};

use crate::batch3d::Focus;
use crate::cam3d::{Camera, FLIGHT, framing, view_dir};
use crate::playground::{Session, example};
use crate::scene3d::Item;
use crate::st_view::Viewer;
use crate::view3d::{Nav, View3d, parents, scene_of};

fn viewer(src: &str) -> (Viewer, Session) {
    let mut s = Session::new(src);
    s.step();
    let v = Viewer::new(src, s.build.as_ref().unwrap().module.clone().unwrap());
    (v, s)
}

#[test]
fn prefixes_say_who_calls_whom() {
    assert_eq!(
        parents(&["", "M.", "M.N.", "K."]),
        vec![None, Some(0), Some(1), Some(0)]
    );
    assert_eq!(parents(&[]), Vec::<Option<usize>>::new());
    assert_eq!(parents(&["X."]), vec![None], "no program to call it");
    assert_eq!(parents(&["", ""]), vec![None, None]);
}

#[test]
fn a_dive_ends_framed_on_the_pous_blocks_and_the_way_back_on_the_whole() {
    let (v, s) = viewer(example("Traffic light").src);
    let scene = scene_of(&v, s.trace.as_ref().map(|t| t as &dyn helmsman_diagram::Scan));
    let aspect = 1.6;
    let mut nav = Nav::default();
    let over = nav.pose(&scene, aspect, 0.0);
    assert!(nav.dive(&scene, 0, aspect, 5.0));
    assert!(nav.flying());
    assert_eq!(nav.focus, Focus::Dive(0));
    // Under way it is between the two.
    let mid = nav.pose(&scene, aspect, 5.0 + FLIGHT / 2.0);
    assert!(nav.flying());
    assert_ne!(mid, over);
    // Done: exactly the board's framing, every block on screen.
    let end = nav.pose(&scene, aspect, 5.0 + FLIGHT + 0.01);
    assert!(!nav.flying());
    assert_eq!(end, framing(&scene.boards[0], aspect, view_dir()));
    let cam = Camera::new(end, (1000.0, 1000.0 / aspect));
    for s in scene
        .solids
        .iter()
        .filter(|s| matches!(s.item, Some(Item::Block(..))))
    {
        for p in s.bounds.corners() {
            let (x, y, _) = cam.project(p).expect("in front");
            assert!(
                (0.0..=cam.size.0).contains(&x) && (0.0..=cam.size.1).contains(&y),
                "{} {x},{y}",
                s.label
            );
        }
    }
    // Up again.
    assert!(nav.up(&scene, aspect, 9.0));
    assert!(!nav.up(&scene, aspect, 9.0), "already on the way up");
    let back = nav.pose(&scene, aspect, 10.0);
    assert_eq!(back, over);
    assert_eq!(nav.focus, Focus::Overview);
}

#[test]
fn diving_into_the_scope_one_is_in_is_no_flight() {
    let (v, s) = viewer(example("Blink").src);
    let scene = scene_of(&v, s.trace.as_ref().map(|t| t as &dyn helmsman_diagram::Scan));
    let mut nav = Nav::default();
    assert!(nav.dive(&scene, 0, 1.0, 0.0));
    let _ = nav.pose(&scene, 1.0, 9.0);
    assert!(nav.dive(&scene, 0, 1.0, 9.0));
    assert!(!nav.flying());
    assert!(!nav.dive(&scene, 5, 1.0, 9.0), "no such scope");
    assert_eq!(nav.focus, Focus::Dive(0));
}

#[test]
fn a_resize_reframes_without_a_flight() {
    let (v, s) = viewer(example("Blink").src);
    let scene = scene_of(&v, s.trace.as_ref().map(|t| t as &dyn helmsman_diagram::Scan));
    let mut nav = Nav::default();
    let wide = nav.pose(&scene, 2.0, 0.0);
    let tall = nav.pose(&scene, 0.5, 0.1);
    assert_ne!(wide, tall);
    assert!(!nav.flying());
}

/// One frame of the view in an area of `size`.
fn frame(
    ctx: &egui::Context,
    view: &mut View3d,
    v: &mut Viewer,
    s: &Session,
    size: Vec2,
    t: f64,
    events: Vec<Event>,
) {
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        time: Some(t),
        events,
        ..Default::default()
    };
    let scan = s.trace.as_ref().map(|t| t as &dyn helmsman_diagram::Scan);
    let mut out = ctx.run_ui(input, |ui| {
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let area = ui.available_rect_before_wrap();
            view.ui(ui, v, scan);
            view.toggle_ui(ui, area);
        });
    });
    out.textures_delta.clear();
}

#[test]
fn the_view_draws_at_any_size_including_none() {
    let (mut v, s) = viewer(example("Traffic light").src);
    let ctx = egui::Context::default();
    let mut view = View3d::default();
    view.set_on(true);
    for (i, size) in [
        (1200.0, 800.0),
        (1.0, 1.0),
        (0.0, 0.0),
        (3.0, 4000.0),
        (4000.0, 3.0),
    ]
    .into_iter()
    .enumerate()
    {
        frame(
            &ctx,
            &mut view,
            &mut v,
            &s,
            Vec2::new(size.0, size.1),
            i as f64,
            vec![],
        );
    }
    assert!(view.on);
}

#[test]
fn the_toggle_turns_it_on_and_off_and_off_asks_for_the_gl_to_be_freed() {
    let mut view = View3d::default();
    assert!(!view.on, "2D is the default");
    view.set_on(true);
    assert!(view.on);
    assert!(!view.wants_release());
    view.set_on(false);
    assert!(view.wants_release());
}

#[test]
fn escape_climbs_and_a_double_click_on_empty_space_dives_nowhere() {
    let (mut v, s) = viewer(example("Traffic light").src);
    let ctx = egui::Context::default();
    let mut view = View3d::default();
    view.set_on(true);
    let size = Vec2::new(1200.0, 800.0);
    frame(&ctx, &mut view, &mut v, &s, size, 0.0, vec![]);
    let scene = scene_of(&v, None);
    assert!(view.nav.dive(&scene, 0, 1.5, 1.0));
    frame(&ctx, &mut view, &mut v, &s, size, 2.0, vec![]);
    assert_eq!(view.nav.focus, Focus::Dive(0));
    let esc = Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(
        &ctx,
        &mut view,
        &mut v,
        &s,
        size,
        3.0,
        vec![Event::PointerMoved(Pos2::new(600.0, 400.0)), esc],
    );
    assert_eq!(view.nav.focus, Focus::Overview);
}
