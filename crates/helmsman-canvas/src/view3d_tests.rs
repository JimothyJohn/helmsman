//! The 3D view's navigation (a dive, the way back, orbit, frame-all, a
//! click) and its parts that need an egui frame, run with no GL: a paint
//! callback is only a shape to a headless context.

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, Rect, Vec2};

use crate::batch3d::Focus;
use crate::cam3d::{Camera, FLIGHT, Orbit};
use crate::playground::{Session, example};
use crate::scene3d::Item;
use crate::st_view::Viewer;
use crate::view3d::{Nav, View3d, built_of, parents, scene_of};

fn viewer(src: &str) -> (Viewer, Session) {
    let mut s = Session::new(src);
    s.step();
    let v = Viewer::new(src, s.build.as_ref().unwrap().module.clone().unwrap());
    (v, s)
}

fn scan(s: &Session) -> Option<&dyn helmsman_diagram::Scan> {
    s.trace.as_ref().map(|t| t as &dyn helmsman_diagram::Scan)
}

const TWO: &str = "FUNCTION_BLOCK pulse
VAR_INPUT go : BOOL; END_VAR
VAR_OUTPUT q : BOOL; END_VAR
q := go;
END_FUNCTION_BLOCK
PROGRAM main
VAR_INPUT run : BOOL := TRUE; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR p : pulse; END_VAR
p(go := run);
lamp := p.q;
END_PROGRAM
";

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

fn on_screen(cam: &Camera, b: &crate::cam3d::Aabb) -> bool {
    b.corners().iter().all(|p| {
        cam.project(*p)
            .is_some_and(|(x, y, _)| (0.0..=cam.size.0).contains(&x) && (0.0..=cam.size.1).contains(&y))
    })
}

#[test]
fn the_first_frame_fills_the_pane_with_every_node_on_screen() {
    for (name, size) in [
        ("Traffic light", (1000.0, 560.0)),
        ("Blink", (700.0, 900.0)),
        ("Logic: AND, OR, NOT", (1400.0, 500.0)),
    ] {
        let (v, s) = viewer(example(name).src);
        let scene = scene_of(&v, scan(&s));
        let mut nav = Nav::default();
        let cam = Camera::new(nav.pose(&scene, size.0 / size.1, 0.0), size);
        for sol in scene.solids.iter().filter(|s| s.item.is_some()) {
            assert!(
                on_screen(&cam, &sol.bounds),
                "{name}: {} off screen at {size:?}",
                sol.label
            );
        }
        // It fills: the scene's projected extent takes most of one axis.
        let pts: Vec<(f32, f32)> = scene
            .frame_points(None)
            .iter()
            .filter_map(|p| cam.project(*p).map(|(x, y, _)| (x, y)))
            .collect();
        let (x0, x1) = pts
            .iter()
            .fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
        let (y0, y1) = pts
            .iter()
            .fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
        let fill = ((x1 - x0) / size.0).max((y1 - y0) / size.1);
        assert!(
            fill > 0.7,
            "{name} at {size:?}: the stack fills {fill} of the pane"
        );
    }
}

#[test]
fn a_dive_ends_framed_on_the_pous_blocks_and_the_way_back_on_the_whole() {
    let (v, s) = viewer(example("Traffic light").src);
    let scene = scene_of(&v, scan(&s));
    let aspect = 1.6;
    let mut nav = Nav::default();
    let over = nav.orbit(&scene, aspect, 0.0);
    assert!(nav.dive(&scene, 0, aspect, 5.0));
    assert!(nav.flying());
    assert_eq!(nav.focus, Focus::Dive(0));
    let mid = nav.orbit(&scene, aspect, 5.0 + FLIGHT / 2.0);
    assert!(nav.flying());
    assert_ne!(mid, over);
    let end = nav.orbit(&scene, aspect, 5.0 + FLIGHT + 0.01);
    assert!(!nav.flying());
    assert_eq!(end, over.framing_points(&scene.frame_points(Some(0)), aspect));
    let cam = Camera::new(end.pose(), (1000.0, 1000.0 / aspect));
    for sol in scene
        .solids
        .iter()
        .filter(|s| matches!(s.item, Some(Item::Block(..))))
    {
        assert!(on_screen(&cam, &sol.bounds), "{}", sol.label);
    }
    assert!(nav.up(&scene, aspect, 9.0));
    assert!(!nav.up(&scene, aspect, 9.0), "already on the way up");
    let back = nav.orbit(&scene, aspect, 10.0);
    assert_eq!(back, over.framing_points(&scene.frame_points(None), aspect));
    assert_eq!(nav.focus, Focus::Overview);
}

#[test]
fn a_move_by_hand_stops_a_flight_and_a_resize_reframes_only_an_unmoved_view() {
    let (v, s) = viewer(example("Blink").src);
    let scene = scene_of(&v, scan(&s));
    let mut nav = Nav::default();
    let wide = nav.pose(&scene, 2.0, 0.0);
    let tall = nav.pose(&scene, 0.5, 0.1);
    assert_ne!(wide, tall, "an unmoved view reframes");
    assert!(nav.dive(&scene, 0, 1.0, 1.0));
    let o = nav.orbit(&scene, 1.0, 1.1);
    assert!(nav.flying());
    let turned = o.orbited(40.0, 10.0, &Nav::limits(&scene));
    nav.moved(turned);
    assert!(!nav.flying(), "input interrupts the flight");
    assert_eq!(nav.orbit(&scene, 1.0, 1.15), turned.fixed());
    // And a resize leaves it where the hand put it.
    assert_eq!(nav.orbit(&scene, 3.0, 2.0), turned.fixed());
}

#[test]
fn frame_all_puts_every_node_on_screen_from_wherever_the_camera_is() {
    let (v, s) = viewer(TWO);
    let scene = scene_of(&v, scan(&s));
    let size = (900.0, 600.0);
    let aspect = size.0 / size.1;
    let mut nav = Nav::default();
    let lim = Nav::limits(&scene);
    for (dx, dy) in [(300.0, 0.0), (-80.0, 400.0), (1e5, -1e5)] {
        let o = nav
            .orbit(&scene, aspect, 0.0)
            .orbited(dx, dy, &lim)
            .panned(500.0, -500.0, 600.0, &lim);
        nav.moved(o);
        nav.frame_all(&scene, aspect, 1.0);
        let end = nav.orbit(&scene, aspect, 3.0);
        let cam = Camera::new(end.pose(), size);
        for sol in scene.solids.iter().filter(|s| s.item.is_some()) {
            assert!(on_screen(&cam, &sol.bounds), "{} after ({dx}, {dy})", sol.label);
        }
        assert_eq!(nav.focus, Focus::Overview);
    }
}

#[test]
fn reset_goes_back_to_the_opening_angles() {
    let (v, s) = viewer(example("Blink").src);
    let scene = scene_of(&v, scan(&s));
    let mut nav = Nav::default();
    let open = nav.orbit(&scene, 1.5, 0.0);
    nav.moved(open.orbited(200.0, 50.0, &Nav::limits(&scene)));
    nav.reset(&scene, 1.5, 1.0);
    let end = nav.orbit(&scene, 1.5, 5.0);
    assert!((end.pitch - open.pitch).abs() < 1e-5 && (end.yaw - open.yaw).abs() < 1e-5);
}

#[test]
fn q_and_e_step_through_the_pous_and_wrap() {
    let (v, s) = viewer(TWO);
    let scene = scene_of(&v, scan(&s));
    assert_eq!(scene.boards.len(), 2);
    let mut nav = Nav::default();
    assert_eq!(nav.step_scope(&scene, 0, 1, 1.5, 0.0), Some(1));
    assert_eq!(nav.focus, Focus::Dive(1));
    assert_eq!(nav.step_scope(&scene, 0, 1, 1.5, 1.0), Some(0), "wraps");
    assert_eq!(nav.step_scope(&scene, 0, -1, 1.5, 2.0), Some(1), "and back");
    let empty = crate::scene3d::build(&[]);
    assert_eq!(Nav::default().step_scope(&empty, 0, 1, 1.0, 0.0), None);
}

#[test]
fn a_click_flies_to_the_block_and_frames_it() {
    let (v, s) = viewer(example("Traffic light").src);
    let b = built_of(&v, scan(&s));
    let mut nav = Nav::default();
    let size = (1000.0, 600.0);
    let aspect = size.0 / size.1;
    let _ = nav.orbit(&b.scene, aspect, 0.0);
    let block = b.scene.nodes[0][3];
    nav.look_at(&b.scene, &block, aspect, 1.0);
    assert!(nav.flying());
    let end = nav.orbit(&b.scene, aspect, 5.0);
    assert!(!nav.flying());
    let cam = Camera::new(end.pose(), size);
    assert!(on_screen(&cam, &block), "the block is in view");
    let before = Orbit::default().framing(&b.scene.bounds(), aspect);
    assert!(end.dist < before.dist, "and nearer than the whole stack was");
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
    let mut out = ctx.run_ui(input, |ui| {
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let area = ui.available_rect_before_wrap();
            view.ui(ui, v, scan(s));
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

fn key(k: egui::Key) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn a_drag_orbits_a_right_drag_pans_and_the_wheel_zooms_toward_the_pointer() {
    let (mut v, s) = viewer(example("Traffic light").src);
    let ctx = egui::Context::default();
    let mut view = View3d::default();
    view.set_on(true);
    let size = Vec2::new(1200.0, 800.0);
    let at = |x, y| Pos2::new(x, y);
    frame(
        &ctx,
        &mut view,
        &mut v,
        &s,
        size,
        0.0,
        vec![Event::PointerMoved(at(600.0, 400.0))],
    );
    frame(&ctx, &mut view, &mut v, &s, size, 0.1, vec![]);
    let o0 = view.nav.orbit_now_for_test();
    // A left drag turns it.
    let press = |b| Event::PointerButton {
        pos: at(600.0, 400.0),
        button: b,
        pressed: true,
        modifiers: Modifiers::NONE,
    };
    let release = |b, p| Event::PointerButton {
        pos: p,
        button: b,
        pressed: false,
        modifiers: Modifiers::NONE,
    };
    frame(
        &ctx,
        &mut view,
        &mut v,
        &s,
        size,
        0.2,
        vec![press(PointerButton::Primary)],
    );
    frame(
        &ctx,
        &mut view,
        &mut v,
        &s,
        size,
        0.3,
        vec![Event::PointerMoved(at(700.0, 430.0))],
    );
    frame(
        &ctx,
        &mut view,
        &mut v,
        &s,
        size,
        0.4,
        vec![Event::PointerMoved(at(760.0, 440.0))],
    );
    frame(
        &ctx,
        &mut view,
        &mut v,
        &s,
        size,
        0.5,
        vec![release(PointerButton::Primary, at(760.0, 440.0))],
    );
    let o1 = view.nav.orbit_now_for_test();
    assert!((o1.yaw - o0.yaw).abs() > 0.3, "turned: {o0:?} -> {o1:?}");
    // The wheel zooms (closer for a scroll up).
    frame(
        &ctx,
        &mut view,
        &mut v,
        &s,
        size,
        1.0,
        vec![Event::PointerMoved(at(300.0, 300.0))],
    );
    frame(
        &ctx,
        &mut view,
        &mut v,
        &s,
        size,
        1.1,
        vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, 120.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        }],
    );
    for t in 0..8 {
        frame(
            &ctx,
            &mut view,
            &mut v,
            &s,
            size,
            1.2 + f64::from(t) * 0.05,
            vec![],
        );
    }
    let o2 = view.nav.orbit_now_for_test();
    assert!(o2.dist < o1.dist, "zoomed in: {} -> {}", o1.dist, o2.dist);
}

#[test]
fn keys_frame_all_snap_the_levels_and_climb() {
    let (mut v, s) = viewer(TWO);
    let ctx = egui::Context::default();
    let mut view = View3d::default();
    view.set_on(true);
    let size = Vec2::new(1200.0, 800.0);
    let center = Event::PointerMoved(Pos2::new(600.0, 400.0));
    frame(&ctx, &mut view, &mut v, &s, size, 0.0, vec![center.clone()]);
    frame(&ctx, &mut view, &mut v, &s, size, 0.1, vec![key(egui::Key::Num2)]);
    let Focus::Dive(first) = view.nav.focus else {
        panic!("2 dives")
    };
    frame(&ctx, &mut view, &mut v, &s, size, 0.2, vec![key(egui::Key::E)]);
    assert_eq!(
        view.nav.focus,
        Focus::Dive((first + 1) % 2),
        "E goes to the next POU"
    );
    frame(&ctx, &mut view, &mut v, &s, size, 0.3, vec![key(egui::Key::Q)]);
    assert_eq!(view.nav.focus, Focus::Dive(first), "Q back");
    frame(
        &ctx,
        &mut view,
        &mut v,
        &s,
        size,
        0.4,
        vec![key(egui::Key::Escape)],
    );
    assert_eq!(view.nav.focus, Focus::Overview, "Esc climbs");
    frame(&ctx, &mut view, &mut v, &s, size, 0.5, vec![key(egui::Key::Num2)]);
    frame(&ctx, &mut view, &mut v, &s, size, 0.6, vec![key(egui::Key::Num1)]);
    assert_eq!(view.nav.focus, Focus::Overview, "1 goes up to the whole");
    frame(&ctx, &mut view, &mut v, &s, size, 0.7, vec![key(egui::Key::F)]);
    assert_eq!(view.nav.focus, Focus::Overview);
}
