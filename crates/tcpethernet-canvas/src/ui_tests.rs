//! The canvas driven headlessly through egui with synthetic pointer and key
//! events, asserting on the project and app state, not on pixels.

use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use tcpethernet_graph::{Kind, Node, NodeId, Project, Scope, Ty, Wire};

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
