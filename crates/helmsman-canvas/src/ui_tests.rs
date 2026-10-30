//! The canvas driven headlessly through egui with synthetic pointer and key
//! events, asserting on the project and app state, not on pixels.

use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use helmsman_graph::{Kind, Node, NodeId, Project, Scope, Ty, Wire};

use crate::app::App;
use crate::look::layout;

struct Harness {
    ctx: egui::Context,
    app: App,
    time: f64,
    mods: Modifiers,
}

impl Harness {
    fn new(project: Project) -> Self {
        let mut h = Self {
            ctx: egui::Context::default(),
            app: App::with_project(project),
            time: 0.0,
            mods: Modifiers::NONE,
        };
        h.app.running = false;
        // Two frames: panels settle their sizes, the view fits the graph.
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 860.0))),
            time: Some(self.time),
            events: std::iter::once(Event::ModifiersChanged(self.mods))
                .chain(events)
                .collect(),
            ..Default::default()
        };
        self.time += 1.0 / 60.0;
        let Self { ctx, app, .. } = self;
        let mut out = ctx.run_ui(input, |ui| app.show(ui));
        // No renderer here to upload textures to.
        out.textures_delta.clear();
    }

    fn button(&self, pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: self.mods,
        }
    }

    /// Press at `a`, move to `b` in steps, release.
    fn drag(&mut self, a: Pos2, b: Pos2) {
        self.frame(vec![Event::PointerMoved(a)]);
        self.frame(vec![self.button(a, true)]);
        for i in 1..=6 {
            let t = i as f32 / 6.0;
            self.frame(vec![Event::PointerMoved(a + (b - a) * t)]);
        }
        self.frame(vec![self.button(b, false)]);
        self.frame(vec![]);
    }

    fn click(&mut self, at: Pos2) {
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![self.button(at, true)]);
        self.frame(vec![self.button(at, false)]);
    }

    fn double_click(&mut self, at: Pos2) {
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![self.button(at, true), self.button(at, false)]);
        self.frame(vec![self.button(at, true), self.button(at, false)]);
        self.frame(vec![]);
    }

    fn key(&mut self, key: Key, mods: Modifiers) {
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: mods,
        };
        self.mods = mods;
        self.frame(vec![ev(true)]);
        self.frame(vec![ev(false)]);
        self.mods = Modifiers::NONE;
        self.frame(vec![]);
    }

    fn node(&self, id: NodeId) -> Node {
        self.app.project.graph(self.app.scope()).unwrap().nodes[&id].clone()
    }

    fn screen(&self, p: Pos2) -> Pos2 {
        self.app.views[&self.app.scope()].to_screen(p)
    }

    fn header(&self, id: NodeId) -> Pos2 {
        let l = layout(&self.app.project, &self.node(id));
        self.screen(l.header.center())
    }

    fn widget(&self, id: NodeId) -> Pos2 {
        let l = layout(&self.app.project, &self.node(id));
        self.screen(l.widget.unwrap().center())
    }

    fn port(&self, id: NodeId, out: bool, index: usize) -> Pos2 {
        let l = layout(&self.app.project, &self.node(id));
        let ports = if out { &l.outs } else { &l.ins };
        self.screen(ports[index].1)
    }
}

const CMD: Modifiers = Modifiers::COMMAND;

/// Toggle at the left, NOT in the middle, lamp at the right; nothing wired.
fn three() -> (Project, NodeId, NodeId, NodeId) {
    let mut p = Project::default();
    let t = p
        .add_node(Scope::Root, Node::new(Kind::Toggle { on: false }, [0.0, 0.0]))
        .unwrap();
    let n = p
        .add_node(Scope::Root, Node::new(Kind::Not, [300.0, 0.0]))
        .unwrap();
    let l = p
        .add_node(Scope::Root, Node::new(Kind::Lamp, [600.0, 0.0]))
        .unwrap();
    (p, t, n, l)
}

#[test]
fn dragging_a_header_moves_the_node_as_one_undo_step() {
    let (p, t, _, _) = three();
    let mut h = Harness::new(p);
    let before = h.node(t).pos;
    let a = h.header(t);
    h.drag(a, a + Vec2::new(0.0, 120.0));
    let after = h.node(t).pos;
    assert!(after[1] > before[1] + 50.0, "moved down: {before:?} -> {after:?}");
    h.key(Key::Z, CMD);
    assert_eq!(h.node(t).pos, before);
}

#[test]
fn dragging_output_to_input_wires_them_and_type_mismatch_does_not() {
    let (mut p, t, n, l) = three();
    let c = p
        .add_node(Scope::Root, Node::new(Kind::Const { value: 1.0 }, [0.0, 250.0]))
        .unwrap();
    let mut h = Harness::new(p);
    h.drag(h.port(t, true, 0), h.port(n, false, 0));
    h.drag(h.port(n, true, 0), h.port(l, false, 0));
    // Num into a Bool input: refused.
    h.drag(h.port(c, true, 0), h.port(l, false, 0));
    let wires = &h.app.project.root.wires;
    assert_eq!(
        wires,
        &[
            Wire {
                from: t,
                from_port: 0,
                to: n,
                to_port: 0
            },
            Wire {
                from: n,
                from_port: 0,
                to: l,
                to_port: 0
            }
        ],
        "toast: {:?}",
        h.app.toast
    );
    assert!(h.app.toast.as_ref().is_some_and(|(_, _, err)| *err));
}

#[test]
fn picking_up_a_wired_input_and_dropping_it_on_nothing_removes_it_and_offers_search() {
    let (mut p, t, n, _) = three();
    p.connect(
        Scope::Root,
        Wire {
            from: t,
            from_port: 0,
            to: n,
            to_port: 0,
        },
    )
    .unwrap();
    let mut h = Harness::new(p);
    let a = h.port(n, false, 0);
    h.drag(a, a + Vec2::new(60.0, 200.0));
    assert_eq!(h.app.project.root.wires, []);
    assert!(h.app.ix.busy(), "search popup open for the dangling wire");
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.ix.busy());
    h.key(Key::Z, CMD);
    assert_eq!(h.app.project.root.wires.len(), 1, "undo puts the wire back");
}

#[test]
fn search_adds_the_typed_node_and_wires_a_dangling_end_to_it() {
    let (p, t, _, _) = three();
    let mut h = Harness::new(p);
    let a = h.port(t, true, 0);
    h.drag(a, a + Vec2::new(40.0, 220.0));
    h.frame(vec![Event::Text("ton".into())]);
    h.key(Key::Enter, Modifiers::NONE);
    let g = &h.app.project.root;
    let ton = g
        .nodes
        .iter()
        .find(|(_, n)| matches!(n.kind, Kind::Ton { .. }))
        .map(|(id, _)| *id);
    let ton = ton.expect("TON added");
    assert_eq!(
        g.wires,
        [Wire {
            from: t,
            from_port: 0,
            to: ton,
            to_port: 0
        }]
    );
    h.key(Key::Z, CMD);
    assert_eq!(
        h.app.project.root.nodes.len(),
        3,
        "add + autowire undo as one step"
    );
}

#[test]
fn box_select_then_group_makes_a_block_and_double_click_goes_inside() {
    let (mut p, t, n, l) = three();
    p.connect(
        Scope::Root,
        Wire {
            from: t,
            from_port: 0,
            to: n,
            to_port: 0,
        },
    )
    .unwrap();
    p.connect(
        Scope::Root,
        Wire {
            from: n,
            from_port: 0,
            to: l,
            to_port: 0,
        },
    )
    .unwrap();
    let mut h = Harness::new(p);
    // Box around the NOT only.
    let r = layout(&h.app.project, &h.node(n)).rect.expand(20.0);
    h.drag(h.screen(r.left_top()), h.screen(r.right_bottom()));
    assert_eq!(h.app.selected.iter().copied().collect::<Vec<_>>(), [n]);
    h.key(Key::G, CMD);
    assert_eq!(h.app.project.blocks.len(), 1);
    let inst = *h.app.selected.iter().next().unwrap();
    assert!(matches!(h.node(inst).kind, Kind::Block(_)));
    assert_eq!(h.app.project.root.wires.len(), 2, "rewired through the block");

    h.frame(vec![]);
    h.double_click(h.header(inst));
    assert_eq!(h.app.nav.len(), 1, "went inside");
    assert!(h.app.live_path().is_some());
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.app.nav.is_empty(), "Esc goes back up");
}

#[test]
fn clicking_a_switch_flips_it_and_holding_a_button_drives_the_sim() {
    let mut p = Project::default();
    let t = p
        .add_node(Scope::Root, Node::new(Kind::Toggle { on: false }, [0.0, 0.0]))
        .unwrap();
    let b = p
        .add_node(Scope::Root, Node::new(Kind::Button, [0.0, 200.0]))
        .unwrap();
    let mut h = Harness::new(p);
    h.click(h.widget(t));
    assert_eq!(h.node(t).kind, Kind::Toggle { on: true });

    h.app.running = true;
    let at = h.widget(b);
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![h.button(at, true)]);
    for _ in 0..3 {
        h.frame(vec![]);
    }
    assert_eq!(h.app.sim.held.iter().collect::<Vec<_>>(), [&vec![b]]);
    let held = h.app.sim.root.out(b, 0, Ty::Bool);
    h.frame(vec![h.button(at, false)]);
    h.frame(vec![]);
    assert!(held.as_bool(), "button output true while held");
    assert!(h.app.sim.held.is_empty());
}

#[test]
fn delete_key_removes_the_selection_and_undo_restores_it() {
    let (p, t, _, _) = three();
    let mut h = Harness::new(p);
    h.click(h.header(t));
    h.key(Key::Delete, Modifiers::NONE);
    assert_eq!(h.app.project.root.nodes.len(), 2);
    h.key(Key::Z, CMD);
    assert_eq!(h.app.project.root.nodes.len(), 3);
    h.key(Key::Z, CMD | Modifiers::SHIFT);
    assert_eq!(h.app.project.root.nodes.len(), 2, "redo");
}

#[test]
fn the_example_opens_and_survives_a_click_storm() {
    let mut h = Harness::new(crate::example::project());
    h.app.running = true;
    // Deterministic spray of clicks, drags and double-clicks over the canvas.
    let mut seed = 0x2545_f491_u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..60 {
        let p = Pos2::new(200.0 + (next() % 1000) as f32, 30.0 + (next() % 800) as f32);
        match next() % 4 {
            0 => h.click(p),
            1 => h.double_click(p),
            2 => h.drag(
                p,
                p + Vec2::new((next() % 200) as f32 - 100.0, (next() % 200) as f32 - 100.0),
            ),
            _ => h.key(Key::Escape, Modifiers::NONE),
        }
    }
    let json = h.app.project.to_json();
    assert_eq!(
        Project::from_json(&json).as_ref(),
        Ok(&h.app.project),
        "still a valid project"
    );
}

// ---- the live ST view ----

mod live {
    use eframe::egui::{Event, Modifiers, MouseWheelUnit, Pos2, Vec2};
    use helmsman_diagram::NodeKind;
    use helmsman_graph::Project;
    use helmsman_iec::Value;

    use super::Harness;
    use crate::app::Mode;
    use crate::st_view::{DEMO, Pick, StView, Viewer};

    fn live() -> Harness {
        let mut h = Harness::new(Project::default());
        h.app.mode = Mode::Live;
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn demo(h: &Harness) -> &StView {
        h.app.st.as_ref().unwrap()
    }

    fn st(h: &Harness) -> &Viewer {
        &demo(h).view
    }

    /// Screen centre of the program's write to `path`.
    fn write_of(h: &Harness, path: &str) -> Pos2 {
        let st = st(h);
        let n = st
            .diagram()
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Write { path: path.into() })
            .unwrap();
        st.cams[st.scope].to_screen(n.rect.x + n.rect.w / 2.0, n.rect.y + n.rect.h / 2.0)
    }

    #[test]
    fn the_demo_compiles_and_the_view_runs_it_with_a_trace_every_frame() {
        assert!(StView::new(DEMO).is_ok());
        let mut h = live();
        let first = demo(&h).sample.as_ref().unwrap().scan;
        h.frame(vec![]);
        let d = demo(&h);
        assert!(d.error.is_none(), "{:?}", d.error);
        let st = &d.view;
        assert!(
            d.sample.as_ref().unwrap().scan > first,
            "a fresh traced scan each frame"
        );
        assert_eq!(
            st.outputs(),
            ["CONVEYOR", "RED", "GREEN"],
            "in order of first assignment"
        );
        let scopes: Vec<&str> = st.scopes.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(scopes, ["PROGRAM CELL", "M : MOTOR"]);
    }

    #[test]
    fn clicking_a_block_shows_its_source_line() {
        let mut h = live();
        let at = write_of(&h, "CONVEYOR");
        h.click(at);
        let (line, text) = st(&h).picked().unwrap();
        assert_eq!(text, "CONVEYOR := M.ON");
        assert_eq!(st(&h).source_line(line), format!("{line:>3} | conveyor := m.on;"));
        // Clicking empty canvas clears it.
        h.click(Pos2::new(1050.0, 845.0));
        assert_eq!(st(&h).pick, None);
    }

    #[test]
    fn the_scripted_shift_runs_the_conveyor_and_says_why() {
        let mut h = live();
        let v = h.app.st.as_mut().unwrap();
        v.scans(100); // 1 s: started at 0.2 s
        let t = &v.sample.as_ref().unwrap().trace;
        assert_eq!(t.get("conveyor"), Some(Value::Bool(true)));
        let why = helmsman_diagram::explain(&v.view.module, t, "conveyor", 4).outline();
        assert!(why[0].starts_with("CONVEYOR = TRUE because line"), "{why:#?}");
        assert!(
            why.iter()
                .any(|l| l.contains("IF NOT FAULT") && l.contains("open")),
            "{why:#?}"
        );
        v.scans(600); // 7 s: jammed
        let t = &v.sample.as_ref().unwrap().trace;
        assert_eq!(t.get("conveyor"), Some(Value::Bool(false)));
        let why = helmsman_diagram::explain(&v.view.module, t, "conveyor", 4).outline();
        assert!(why.iter().any(|l| l.contains("JAM = TRUE")), "{why:#?}");
        h.frame(vec![]);
        assert!(matches!(
            st(&h).pick,
            None | Some(Pick::Node(_) | Pick::Region(_))
        ));
    }

    #[test]
    fn scroll_pans_pinch_zooms_and_the_instance_view_switches_diagrams() {
        let mut h = live();
        let before = st(&h).cams[0];
        h.frame(vec![
            Event::PointerMoved(Pos2::new(700.0, 400.0)),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -120.0),
                modifiers: Modifiers::NONE,
                phase: eframe::egui::TouchPhase::Move,
                source: eframe::egui::MouseWheelSource::Wheel,
            },
        ]);
        for _ in 0..10 {
            h.frame(vec![]);
        }
        let panned = st(&h).cams[0];
        assert!(panned.offset.y < before.offset.y, "{before:?} -> {panned:?}");
        h.frame(vec![Event::Zoom(1.5)]);
        assert!(st(&h).cams[0].zoom > panned.zoom);
        h.app.st.as_mut().unwrap().view.scope = 1;
        h.frame(vec![]);
        assert_eq!(st(&h).diagram().pou, "MOTOR");
        assert!(demo(&h).live().is_some());
        h.app.mode = Mode::Lab;
        h.frame(vec![]);
    }
}

/// The live ST view of a real box, driven headless: the state a browser
/// built from the real API, drawn and clicked through egui.
mod boxed {
    use helmsman_diagram::NodeKind;
    use helmsman_graph::Project;

    use super::Harness;
    use crate::app::{Mode, Source};
    use crate::box_tests::{Bench, watch};
    use crate::box_view::BoxView;
    use crate::remote::{Link, Msg, Stream};

    fn boxed(h: &Harness) -> &BoxView {
        h.app.boxv.as_ref().unwrap()
    }

    #[test]
    fn a_live_box_is_drawn_clicked_and_explained_and_losing_it_offers_the_demo() {
        let bench = Bench::new();
        let (state, _) = watch(&bench);
        let mut h = Harness::new(Project::default());
        h.app.mode = Mode::Live;
        h.app.source = Source::Box;
        h.frame(vec![]);
        assert!(boxed(&h).view.is_none());
        assert_eq!(boxed(&h).waiting(), "Looking for the box…");
        h.app.boxv.as_mut().unwrap().state = state;
        h.frame(vec![]);
        h.frame(vec![]);
        let b = boxed(&h);
        let v = b.view.as_ref().expect("the module is drawn");
        assert_eq!(
            v.because.as_deref(),
            Some("CONVEYOR"),
            "the first output is explained"
        );
        assert_eq!(b.status(h.time).0, "● connected");

        // Click the conveyor's assignment: its source line comes up.
        let n = v
            .diagram()
            .nodes
            .iter()
            .find(|n| {
                n.kind
                    == NodeKind::Write {
                        path: "CONVEYOR".into(),
                    }
            })
            .unwrap();
        let at = v.cams[v.scope].to_screen(n.rect.x + n.rect.w / 2.0, n.rect.y + n.rect.h / 2.0);
        h.click(at);
        let (line, text) = boxed(&h).view.as_ref().unwrap().picked().unwrap();
        assert_eq!(text, "CONVEYOR := M.ON");
        assert!(line > 0);

        // The box goes away: reconnecting, then unreachable, still drawn.
        let t = h.time;
        let b = h.app.boxv.as_mut().unwrap();
        b.state.apply(Msg::Failed(Stream::Values), t);
        assert!(
            b.status(t).0.starts_with("● reconnecting (retry in 2s)"),
            "{:?}",
            b.status(t)
        );
        for k in 1..3 {
            b.state.apply(Msg::Failed(Stream::Values), t + f64::from(k));
        }
        assert!(matches!(b.state.link(), Link::Unreachable { .. }));
        assert!(
            b.status(t + 2.0).0.starts_with("● box unreachable"),
            "{:?}",
            b.status(t + 2.0)
        );
        h.frame(vec![]);
        assert!(boxed(&h).view.is_some(), "the last diagram stays up");
        h.app.boxv.as_mut().unwrap().wants_demo = true;
        h.frame(vec![]);
        assert_eq!(
            h.app.source,
            Source::Demo,
            "the operator can fall back to the demo"
        );
        h.frame(vec![]);
        assert!(h.app.st.is_some());
    }
}

/// Explain mode, driven headless: tap a block or an output, read the
/// box's own explanation, ask the agent (on a box).
mod explained {
    use eframe::egui::Pos2;
    use helmsman_diagram::NodeKind;
    use helmsman_graph::Project;

    use super::Harness;
    use crate::app::{Mode, Source};
    use crate::box_tests::{Bench, watch};
    use crate::box_view::BoxView;
    use crate::explain::{Phase, Reply, Side, SideCmd};
    use crate::remote::{Msg, Stream};
    use crate::st_view::{TAP, Viewer, node_near};

    fn live() -> Harness {
        let mut h = Harness::new(Project::default());
        h.app.mode = Mode::Live;
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn st(h: &Harness) -> &Viewer {
        &h.app.st.as_ref().unwrap().view
    }

    /// Screen centre and on-screen width of the first node matching `f`.
    fn node(h: &Harness, f: impl Fn(&NodeKind) -> bool) -> (Pos2, f32) {
        let v = st(h);
        let n = v.diagram().nodes.iter().find(|n| f(&n.kind)).unwrap();
        let cam = v.cams[v.scope];
        (
            cam.to_screen(n.rect.x + n.rect.w / 2.0, n.rect.y + n.rect.h / 2.0),
            n.rect.w * cam.zoom,
        )
    }

    #[test]
    fn tapping_an_assignment_explains_it_in_words_and_highlights_its_line() {
        let mut h = live();
        h.app.st.as_mut().unwrap().scans(100); // started at 0.2 s
        let (at, _) = node(&h, |k| *k == NodeKind::Write { path: "RED".into() });
        h.click(at);
        assert_eq!(st(&h).because.as_deref(), Some("RED"));
        h.frame(vec![]);
        let v = h.app.st.as_mut().unwrap();
        let now = v.now_ms;
        let scan = v.sample.as_ref().map(|s| &s.trace as &dyn helmsman_diagram::Scan);
        let said = v.view.explanation(scan, Some(now)).unwrap();
        let first = &said.sentences[0].text;
        assert!(first.starts_with("Red is OFF because line "), "{said:#?}");
        let line = said.sentences[0].line.unwrap();
        assert_eq!(st(&h).highlight[0], line, "the cited line is drawn highlighted");
        assert!(
            said.sentences.iter().any(|s| s.text.contains("jam is OFF")),
            "{said:#?}"
        );
    }

    #[test]
    fn tapping_a_timer_block_explains_its_output_in_time() {
        let mut h = live();
        h.app.st.as_mut().unwrap().scans(30);
        let (at, _) = node(&h, |k| matches!(k, NodeKind::Fb { fb, .. } if fb == "TON"));
        h.click(at);
        assert_eq!(st(&h).because.as_deref(), Some("BLINK.Q"));
        let v = h.app.st.as_mut().unwrap();
        let now = v.now_ms;
        let scan = v.sample.as_ref().map(|s| &s.trace as &dyn helmsman_diagram::Scan);
        let text = v.view.explanation(scan, Some(now)).unwrap().text();
        assert!(
            text.contains("Blink turns ON after 400 ms of its input being ON."),
            "{text}"
        );
    }

    #[test]
    fn a_tap_near_a_small_block_still_hits_it() {
        let mut h = live();
        h.frame(vec![eframe::egui::Event::Zoom(0.5)]);
        h.frame(vec![]);
        let (at, width) = node(&h, |k| {
            *k == NodeKind::Write {
                path: "CONVEYOR".into(),
            }
        });
        // Just right of the block (assignments are rightmost), within half
        // a tap target.
        let near = at + eframe::egui::Vec2::new(width / 2.0 + TAP / 2.0 - 4.0, 0.0);
        h.click(near);
        assert_eq!(
            st(&h).because.as_deref(),
            Some("CONVEYOR"),
            "zoom {}",
            st(&h).cams[0].zoom
        );
        // Far from anything: nothing new is picked.
        let d = st(&h).diagram();
        assert_eq!(node_near(d, -500.0, -500.0, 22.0), None);
    }

    #[test]
    fn the_demo_explains_offline_and_says_why_it_cannot_ask() {
        let mut h = live();
        h.app.st.as_mut().unwrap().scans(100);
        h.frame(vec![]);
        // No transport, no ask: the demo's panel draws with "Ask why"
        // disabled and its reason; a click on it does nothing.
        assert_eq!(
            crate::st_view::AskUi::demo().blocked,
            Some("The demo runs in this page: asking the agent needs a box with a relay.")
        );
        assert!(h.app.boxv.is_none(), "no box anywhere");
        let v = h.app.st.as_mut().unwrap();
        let now = v.now_ms;
        let scan = v.sample.as_ref().map(|s| &s.trace as &dyn helmsman_diagram::Scan);
        let text = v.view.explanation(scan, Some(now)).unwrap().text();
        assert!(text.starts_with("Conveyor is ON because line "), "{text}");
        assert!(text.contains("it comes from the field"), "{text}");
    }

    #[test]
    fn on_a_box_ask_why_waits_for_agent_and_token_then_streams_the_answer() {
        let bench = Bench::new();
        let (state, _) = watch(&bench);
        let mut h = Harness::new(Project::default());
        h.app.mode = Mode::Live;
        h.app.source = Source::Box;
        h.frame(vec![]);
        h.app.boxv.as_mut().unwrap().state = state;
        h.frame(vec![]);
        h.frame(vec![]);
        let b: &mut BoxView = h.app.boxv.as_mut().unwrap();
        // Connected: it asked the box whether there is an agent, once.
        assert_eq!(b.side_sent, [SideCmd::Index]);
        assert!(b.blocked().unwrap().contains("Checking"));
        let (_, index) = bench.get("/api");
        b.side_in(vec![Side::Index(Ok(index))]);
        assert_eq!(b.agent, Some(false), "this bench has no relay");
        assert!(b.blocked().unwrap().contains("No agent"));
        b.agent = Some(true);
        assert!(b.blocked().unwrap().contains("token"));
        let ctx = h.ctx.clone();
        let b = h.app.boxv.as_mut().unwrap();
        b.ask_why(&ctx, "CONVEYOR");
        assert_eq!(b.side_sent.len(), 1, "blocked: nothing sent");
        b.token = format!(" {} ", "ab".repeat(32));
        assert_eq!(b.blocked(), None);
        b.ask_why(&ctx, "CONVEYOR");
        let sent = b.side_sent.last().unwrap().clone();
        assert_eq!(
            sent,
            SideCmd::Ask {
                token: "ab".repeat(32),
                body: r#"{"module":"cell","path":"CONVEYOR"}"#.into(),
            }
        );
        assert!(
            !format!("{sent:?}").contains(&"ab".repeat(32)),
            "the token never reaches a log line"
        );
        b.ask_why(&ctx, "CONVEYOR");
        assert_eq!(b.side_sent.len(), 2, "one ask at a time");
        b.side_in(vec![
            Side::Ask(Reply::Chunk(
                "event: explain\ndata: {\"scan\":9,\"sentences\":[{\"text\":\"Conveyor is ON.\"}]}\n\nevent: text\ndata: {\"delta\":\"The belt runs: \"}\n\n".into(),
            )),
        ]);
        h.frame(vec![]);
        let b = h.app.boxv.as_mut().unwrap();
        assert_eq!(b.ask.as_ref().unwrap().phase, Phase::Streaming);
        b.side_in(vec![
            Side::Ask(Reply::Chunk("event: text\ndata: {\"delta\":\"start was pressed.\"}\n\nevent: done\ndata: {\"stop\":\"end_turn\",\"why\":null}\n\n".into())),
            Side::Ask(Reply::End),
        ]);
        h.frame(vec![]);
        let a = h.app.boxv.as_ref().unwrap().ask.as_ref().unwrap();
        assert_eq!(a.text, "The belt runs: start was pressed.");
        assert_eq!(a.phase, Phase::Done);

        // The box goes away: asking is blocked again, the answer stays.
        let t = h.time;
        let b = h.app.boxv.as_mut().unwrap();
        for k in 0..3 {
            b.state.apply(Msg::Failed(Stream::Values), t + f64::from(k));
        }
        assert!(b.blocked().unwrap().contains("offline"));
        h.frame(vec![]);
        assert!(h.app.boxv.as_ref().unwrap().ask.is_some());
    }
}
