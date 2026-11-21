//! What a right click and a double click on the diagram do
//! (`crate::diagram_menu`), driven through egui. The diagram never changes
//! the code (Nick, 2026-11-18): the menu holds "Chat about this" and the
//! io.map items, a double click on a constant does nothing to the file, and
//! a drag moves a block and nothing else.

use eframe::egui::{Event, Modifiers, PointerButton, Pos2, Vec2};
use helmsman_diagram::NodeKind;

use super::{Harness, sim_view};

const SRC: &str = "\
PROGRAM latch
VAR
  start AT %IX0.0 : BOOL;
  stop AT %IX0.1 : BOOL;
  motor AT %QX0.0 : BOOL;
  lamp AT %QX0.1 : BOOL;
  count : INT;
  limit : INT;
END_VAR
motor := (start OR motor) AND NOT stop;
lamp := count > 10;
END_PROGRAM
";

/// What the diagram used to offer to change the code, by control name.
const CODE_ITEMS: [&str; 8] = [
    "diagram menu operator",
    "diagram menu negate",
    "diagram menu connect",
    "diagram menu value",
    "diagram menu rename",
    "diagram menu delete",
    "diagram value field",
    "diagram rename field",
];

impl Harness {
    /// The simulation with `src` as its one program file, deployed.
    fn with_program(src: &str) -> Self {
        let mut h = Self::code();
        {
            let v = h.app.sim.as_mut().unwrap();
            v.proj.create("menu", None);
            v.proj.files.insert("main.st".into(), src.into());
            v.proj.save();
            v.open_project("menu");
        }
        h.wait(0.4);
        h
    }

    fn file(&self) -> String {
        self.app.sim.as_ref().unwrap().proj.files["main.st"].clone()
    }

    fn menu_open(&self) -> bool {
        self.app.sim.as_ref().unwrap().dmenu.is_open()
    }

    /// Opens whatever fold hides block `n` and lets the view settle.
    fn reveal(&mut self, n: helmsman_diagram::NodeId) {
        let v = self.app.sim.as_mut().unwrap().viewer.as_mut().expect("a diagram");
        v.reveal(n);
        v.refit();
        self.frame(vec![]);
        self.frame(vec![]);
    }

    /// The middle of the first block that is `f`, on screen.
    fn block(&mut self, f: impl Fn(&NodeKind, &str) -> bool) -> Pos2 {
        let n = sim_view(self)
            .diagram()
            .nodes
            .iter()
            .find(|n| f(&n.kind, &n.text))
            .expect("the block")
            .id;
        self.reveal(n);
        sim_view(self)
            .block_on_screen(n)
            .expect("the block is drawn")
            .center()
    }

    /// The screen position of input `k` of the first block computing `text`.
    fn pin(&mut self, text: &str, k: usize) -> Pos2 {
        let n = sim_view(self)
            .diagram()
            .nodes
            .iter()
            .find(|n| n.text == text && !matches!(n.kind, NodeKind::Write { .. }))
            .unwrap_or_else(|| panic!("no block computing {text}"))
            .id;
        self.reveal(n);
        sim_view(self).pin_on_screen(n, k).expect("the pin is drawn")
    }

    fn double_click(&mut self, at: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(vec![Event::PointerMoved(at)]);
        for _ in 0..2 {
            self.frame(vec![button(true)]);
            self.frame(vec![button(false)]);
        }
        self.frame(vec![]);
    }
}

#[test]
fn a_right_click_on_a_block_offers_the_chat_and_no_change_to_the_code() {
    let mut h = Harness::with_program(SRC);
    for at in [
        h.block(|k, _| matches!(k, NodeKind::Op { op } if op == "OR")),
        h.block(|k, _| matches!(k, NodeKind::Read { path } if path == "COUNT")),
        h.block(|k, t| matches!(k, NodeKind::Const { .. }) && t == "10"),
    ] {
        h.right_click(at);
        assert!(h.menu_open());
        assert!(h.has("diagram menu chat"));
        for key in CODE_ITEMS {
            assert!(!h.has(key), "{key} is gone");
        }
        h.key(Modifiers::NONE, eframe::egui::Key::Escape);
        assert!(!h.menu_open(), "Escape shuts it");
    }
    assert_eq!(h.file(), SRC, "nothing was written");
}

#[test]
fn a_right_click_on_a_pin_is_about_its_block() {
    let mut h = Harness::with_program(SRC);
    let at = h.pin("COUNT > 10", 0);
    h.right_click(at);
    assert!(h.menu_open());
    assert!(h.has("diagram menu chat"));
    assert_eq!(h.file(), SRC);
}

#[test]
fn a_double_click_on_a_constant_changes_nothing_and_opens_nothing() {
    let mut h = Harness::with_program(SRC);
    let at = h.block(|k, t| matches!(k, NodeKind::Const { .. }) && t == "10");
    h.double_click(at);
    assert!(!h.menu_open());
    assert!(!h.has("diagram value field"));
    assert_eq!(h.file(), SRC);
}

#[test]
fn a_dragged_block_moves_on_the_diagram_and_the_code_stays() {
    let mut h = Harness::with_program(SRC);
    let from = h.block(|k, _| matches!(k, NodeKind::Op { op } if op == "OR"));
    h.drag(from, from + Vec2::new(0.0, 70.0));
    let moved = sim_view(&h).arrangement().to_text();
    assert!(moved.contains("START OR MOTOR"), "{moved}");
    assert_eq!(h.file(), SRC);
}

#[test]
fn the_menu_stays_inside_the_diagram_at_a_narrow_width_and_in_legible_text() {
    let mut h = Harness::with_program(SRC);
    h.size = Vec2::new(760.0, 640.0);
    h.wait(0.4);
    let at = h.pin("COUNT > 10", 0);
    h.right_click(at);
    h.frame(vec![]);
    let popup = h.rect("diagram popup");
    let area = h.app.chrome.diagram;
    assert!(
        area.expand(1.0).contains_rect(popup),
        "the popup {popup:?} is inside the diagram {area:?}"
    );
    assert!(popup.width() <= area.width());
    assert!(
        h.small_text().is_empty(),
        "text under 16 pt: {:?}",
        h.small_text()
    );
}
