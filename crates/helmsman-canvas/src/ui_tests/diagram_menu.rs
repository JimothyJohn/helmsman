//! What a right click and a double click on the diagram do
//! (`crate::diagram_menu`), driven through egui. The diagram never changes
//! the program's logic (Nick, 2026-11-18): the menu holds "Chat about this"
//! and the io.map items, and a drag moves a block and nothing else. Names
//! and set values are the diagram's to change (Nick, 2026-11-23,
//! `crate::by_hand`): "Rename…" on a block that names a variable, "Set
//! value…" on a literal, a double click for either, each saved as one
//! revision that Undo takes back.

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

/// What the diagram used to offer to change the logic, by control name.
const LOGIC_ITEMS: [&str; 4] = [
    "diagram menu operator",
    "diagram menu negate",
    "diagram menu connect",
    "diagram menu delete",
];
/// The two by-hand items.
const RENAME: &str = "diagram menu rename";
const SET_VALUE: &str = "diagram menu set value";

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
fn a_right_click_offers_the_chat_a_name_or_a_value_and_never_the_logic() {
    let mut h = Harness::with_program(SRC);
    for (at, rename, value) in [
        (
            h.block(|k, _| matches!(k, NodeKind::Op { op } if op == "OR")),
            false,
            false,
        ),
        (
            h.block(|k, _| matches!(k, NodeKind::Read { path } if path == "COUNT")),
            true,
            false,
        ),
        (
            h.block(|k, t| matches!(k, NodeKind::Const { .. }) && t == "10"),
            false,
            true,
        ),
    ] {
        h.right_click(at);
        h.frame(vec![]);
        assert!(h.menu_open());
        assert!(h.has("diagram menu chat"));
        assert_eq!((h.has(RENAME), h.has(SET_VALUE)), (rename, value));
        for key in LOGIC_ITEMS {
            assert!(!h.has(key), "{key} is gone");
        }
        h.key(Modifiers::NONE, eframe::egui::Key::Escape);
        assert!(!h.menu_open(), "Escape shuts it");
    }
    assert_eq!(h.file(), SRC, "nothing was written");
}

#[test]
fn rename_in_the_menu_renames_everywhere_as_one_revision_and_undo_takes_it_back() {
    let mut h = Harness::with_program(SRC);
    let revs = h.app.sim.as_ref().unwrap().proj.revisions().len();
    let at = h.block(|k, _| matches!(k, NodeKind::Read { path } if path == "COUNT"));
    h.right_click(at);
    h.frame(vec![]);
    h.press(RENAME);
    h.frame(vec![]);
    assert!(h.has("rename form"), "the question opens");
    assert_eq!(
        h.app.sim.as_ref().unwrap().devices_ui.renaming,
        Some(("count".into(), "count".into())),
        "the name as the program spells it"
    );
    h.app.sim.as_mut().unwrap().devices_ui.renaming = Some(("count".into(), "parts".into()));
    h.frame(vec![]);
    h.frame(vec![]);
    h.press("rename ok");
    h.wait(0.3);
    let want = SRC
        .replace("  count : INT;", "  parts : INT;")
        .replace("lamp := count > 10;", "lamp := parts > 10;");
    assert_eq!(h.file(), want);
    let v = h.app.sim.as_ref().unwrap();
    assert!(!v.proj.dirty(), "saved");
    assert_eq!(v.proj.revisions().len(), revs + 1, "one revision");
    assert_eq!(v.proj.revisions().last().unwrap().what, "Renamed count to parts");
    assert_eq!(
        v.instruct.outcome.as_ref().map(|o| o.status.as_str()),
        Some("Renamed count to parts in 1 file.")
    );
    assert!(
        v.session.src.contains("parts > 10"),
        "the program running is the renamed one"
    );
    h.app.sim.as_mut().unwrap().undo_change();
    assert_eq!(h.file(), SRC);
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
fn a_double_click_on_a_constant_asks_its_value_and_set_writes_it_as_one_revision() {
    let mut h = Harness::with_program(SRC);
    let revs = h.app.sim.as_ref().unwrap().proj.revisions().len();
    let at = h.block(|k, t| matches!(k, NodeKind::Const { .. }) && t == "10");
    h.double_click(at);
    assert!(!h.menu_open());
    assert!(h.has("value form"), "the question opens on the value");
    assert_eq!(h.file(), SRC, "nothing yet");
    let ask = |h: &mut Harness, text: &str| {
        h.app.sim.as_mut().unwrap().value_ask.as_mut().expect("open").text = text.into();
        h.frame(vec![]);
        h.frame(vec![]);
    };
    // What is no value, or would not build, cannot be set.
    for bad in ["count + 1", "10; motor := TRUE", "T#5s", ""] {
        ask(&mut h, bad);
        h.press("value ok");
        h.frame(vec![]);
        assert_eq!(h.file(), SRC, "{bad:?} writes nothing");
        assert!(h.has("value form"), "{bad:?} leaves the question open");
    }
    ask(&mut h, "25");
    h.press("value ok");
    h.wait(0.3);
    assert!(!h.has("value form"));
    assert_eq!(h.file(), SRC.replace("count > 10", "count > 25"));
    let v = h.app.sim.as_ref().unwrap();
    assert_eq!(v.proj.revisions().len(), revs + 1, "one revision");
    assert_eq!(v.proj.revisions().last().unwrap().what, "Set 10 to 25");
    assert_eq!(
        v.instruct.outcome.as_ref().map(|o| o.status.as_str()),
        Some("Set 10 to 25 in main.st.")
    );
    assert!(v.session.src.contains("count > 25"), "the program running has it");
    h.app.sim.as_mut().unwrap().undo_change();
    assert_eq!(h.file(), SRC);
}

#[test]
fn escape_leaves_the_value_and_a_double_click_on_an_operator_asks_nothing() {
    let mut h = Harness::with_program(SRC);
    let at = h.block(|k, t| matches!(k, NodeKind::Const { .. }) && t == "10");
    h.double_click(at);
    assert!(h.has("value form"));
    h.key(Modifiers::NONE, eframe::egui::Key::Escape);
    h.frame(vec![]);
    assert!(!h.has("value form"));
    let at = h.block(|k, _| matches!(k, NodeKind::Op { op } if op == "OR"));
    h.double_click(at);
    assert!(!h.has("value form") && !h.has("rename form"));
    assert_eq!(h.file(), SRC);
}

#[test]
fn a_double_click_on_a_variable_asks_its_name() {
    let mut h = Harness::with_program(SRC);
    let at = h.block(|k, _| matches!(k, NodeKind::Read { path } if path == "COUNT"));
    h.double_click(at);
    assert!(h.has("rename form"));
    assert_eq!(h.file(), SRC);
}

#[test]
fn a_double_click_on_an_inputs_block_still_simulates_it_and_asks_no_name() {
    let mut h = Harness::with_program(SRC);
    let at = h.block(|k, _| matches!(k, NodeKind::Read { path } if path == "STOP"));
    h.double_click(at);
    h.frame(vec![]);
    assert!(
        h.app.sim.as_ref().unwrap().sim_popup.is_some(),
        "Simulate this input"
    );
    assert!(!h.has("rename form"));
    assert_eq!(h.app.sim.as_ref().unwrap().devices_ui.renaming, None);
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
