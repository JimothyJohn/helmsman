//! Editing the program on the diagram (`crate::diagram_edit`), driven
//! through egui: each gesture and the text it leaves in the file, Undo,
//! the refusals, the places the gestures are off.

use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2, Vec2};
use helmsman_diagram::NodeKind;
use helmsman_diagram::layout::in_port;

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

impl Harness {
    /// The simulation with `src` as its one program file, deployed.
    fn editing(src: &str) -> Self {
        let mut h = Self::code();
        {
            let v = h.app.sim.as_mut().unwrap();
            v.proj.create("edit", None);
            v.proj.files.insert("main.st".into(), src.into());
            v.proj.save();
            v.open_project("edit");
        }
        h.wait(0.4);
        h
    }

    fn file(&self) -> String {
        self.app.sim.as_ref().unwrap().proj.files["main.st"].clone()
    }

    fn deployed(&self) -> String {
        sim_view(self).src.clone()
    }

    /// The screen centre of the first block that is `f`.
    fn block(&self, f: impl Fn(&NodeKind, &str) -> bool) -> Pos2 {
        let v = sim_view(self);
        let n = v
            .diagram()
            .nodes
            .iter()
            .find(|n| f(&n.kind, &n.text))
            .expect("the block");
        // Where it is drawn (a literal folded into its comparison is that
        // one block), in its header: an input's control sits under it.
        let r = v.block_on_screen(n.id).expect("the block is drawn");
        Pos2::new(
            r.center().x,
            r.min.y + (r.height() / 2.0).min(10.0 * v.cams[v.scope].zoom),
        )
    }

    /// The screen position of input `k` of the first block computing `text`.
    fn pin(&self, text: &str, k: usize) -> Pos2 {
        let v = sim_view(self);
        let n = v
            .diagram()
            .nodes
            .iter()
            .find(|n| n.text == text && !matches!(n.kind, NodeKind::Write { .. }))
            .unwrap_or_else(|| panic!("no block computing {text}"));
        v.pin_on_screen(n.id, k).expect("the pin is drawn")
    }

    fn op(&self, op: &str) -> Pos2 {
        self.block(|k, _| matches!(k, NodeKind::Op { op: o } if o == op))
    }

    fn constant(&self, text: &str) -> Pos2 {
        self.block(|k, t| matches!(k, NodeKind::Const { .. }) && t == text)
    }

    /// Clicks into the ST editor and types `s`.
    fn type_in_editor_for_test(&mut self, s: &str) {
        let at = self.rect("st editor").left_top() + Vec2::new(60.0, 6.0);
        self.click(at);
        self.ctrl(Key::End);
        self.frame(vec![Event::Text(s.into())]);
    }

    /// Presses control `key`, and a frame more for what it opened.
    fn tap(&mut self, key: &str) {
        self.press(key);
        self.frame(vec![]);
    }

    fn ctrl(&mut self, key: Key) {
        self.key(Modifiers::COMMAND, key);
    }

    /// Replaces what the focused field holds.
    fn type_over(&mut self, s: &str) {
        if s.is_empty() {
            self.key(Modifiers::NONE, Key::Backspace);
        } else {
            self.frame(vec![Event::Text(s.into())]);
        }
        self.frame(vec![]);
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

    fn stage(&self) -> &'static str {
        self.app.sim.as_ref().unwrap().dedit.stage_name()
    }

    fn popup_error(&self) -> Option<String> {
        self.app.sim.as_ref().unwrap().dedit.error().map(str::to_string)
    }

    fn off_note(&self) -> Option<String> {
        sim_view(self).off_note.as_ref().map(|(_, t)| t.clone())
    }
}

#[test]
fn an_operator_is_changed_from_the_menu_and_the_program_runs_the_change() {
    let mut h = Harness::editing(SRC);
    let at = h.op("OR");
    h.right_click(at);
    assert_eq!(h.stage(), "menu");
    h.tap("diagram menu operator");
    assert_eq!(h.stage(), "operators");
    assert!(h.has("diagram op AND"));
    assert!(
        !h.has("diagram op +"),
        "an arithmetic operator is not offered for an OR"
    );
    h.tap("diagram op AND");
    let want = SRC.replace("(start OR motor)", "(start AND motor)");
    assert_eq!(h.file(), want);
    assert_eq!(h.deployed(), want, "the diagram edit deployed it");
    assert_eq!(h.stage(), "closed");
    assert!(
        h.app.sim.as_ref().unwrap().proj.dirty(),
        "the file is marked unsaved"
    );
}

#[test]
fn an_input_is_negated_and_the_bubble_taken_off_again() {
    let mut h = Harness::editing(SRC);
    let at = h.pin("START OR MOTOR", 0);
    h.right_click(at);
    h.tap("diagram menu negate");
    let negated = SRC.replace("(start OR motor)", "(NOT start OR motor)");
    assert_eq!(h.file(), negated);
    let at = h.pin("NOT START OR MOTOR", 0);
    h.right_click(at);
    h.tap("diagram menu negate");
    assert_eq!(h.file(), SRC, "toggling twice is the original text");
}

#[test]
fn a_pin_is_connected_to_a_variable_picked_from_the_search() {
    let mut h = Harness::editing(SRC);
    let at = h.pin("COUNT > 10", 0);
    h.right_click(at);
    h.tap("diagram menu connect");
    assert_eq!(h.stage(), "connect");
    h.frame(vec![Event::Text("lim".into())]);
    h.frame(vec![]);
    assert!(h.has("diagram connect LIMIT"));
    assert!(!h.has("diagram connect START"), "the search narrows the list");
    h.tap("diagram connect LIMIT");
    assert_eq!(h.file(), SRC.replace("count > 10", "limit > 10"));
}

#[test]
fn a_constant_is_edited_from_the_menu() {
    let mut h = Harness::editing(SRC);
    let at = h.constant("10");
    h.right_click(at);
    h.tap("diagram menu value");
    assert_eq!(h.stage(), "value");
    h.type_over("25");
    h.tap("diagram value ok");
    assert_eq!(h.file(), SRC.replace("count > 10", "count > 25"));
}

#[test]
fn a_double_click_on_a_constant_edits_it_in_place() {
    let mut h = Harness::editing(SRC);
    let at = h.constant("10");
    h.double_click(at);
    assert_eq!(h.stage(), "value", "the value field opens at once");
    h.type_over("42");
    h.key(Modifiers::NONE, Key::Enter);
    h.frame(vec![]);
    assert_eq!(h.file(), SRC.replace("count > 10", "count > 42"));
}

#[test]
fn a_variable_is_renamed_everywhere_it_is_used() {
    let mut h = Harness::editing(SRC);
    let at = h.block(|k, _| matches!(k, NodeKind::Read { path } if path == "STOP"));
    h.right_click(at);
    h.tap("diagram menu rename");
    assert_eq!(h.stage(), "rename");
    h.type_over("halt");
    h.tap("diagram rename ok");
    let want = SRC.replace("stop", "halt");
    assert_eq!(h.file(), want);
    assert_eq!(h.deployed(), want);
}

#[test]
fn a_statement_is_deleted_from_the_menu() {
    let mut h = Harness::editing(SRC);
    let at = h.block(|k, _| matches!(k, NodeKind::Write { path } if path == "LAMP"));
    h.right_click(at);
    h.tap("diagram menu delete");
    assert_eq!(h.file(), SRC.replace("lamp := count > 10;\n", ""));
}

#[test]
fn ctrl_z_undoes_a_diagram_edit_and_the_program_follows() {
    let mut h = Harness::editing(SRC);
    let at = h.op("OR");
    h.right_click(at);
    h.tap("diagram menu operator");
    h.tap("diagram op AND");
    let edited = h.file();
    assert_ne!(edited, SRC);
    h.ctrl(Key::Z);
    h.wait(0.3);
    assert_eq!(h.file(), SRC, "the exact old text is back");
    assert_eq!(h.deployed(), SRC, "and what runs is the old program again");
    h.key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
    h.wait(0.3);
    assert_eq!(h.file(), edited, "Redo puts the edit back");
    assert_eq!(h.deployed(), edited);
}

#[test]
fn two_edits_are_two_undo_steps() {
    let mut h = Harness::editing(SRC);
    let at = h.op("OR");
    h.right_click(at);
    h.tap("diagram menu operator");
    h.tap("diagram op AND");
    let first = h.file();
    let at = h.constant("10");
    h.right_click(at);
    h.tap("diagram menu value");
    h.type_over("77");
    h.tap("diagram value ok");
    assert_ne!(h.file(), first);
    h.ctrl(Key::Z);
    h.wait(0.3);
    assert_eq!(h.file(), first, "the second edit only");
    h.ctrl(Key::Z);
    h.wait(0.3);
    assert_eq!(h.file(), SRC);
}

#[test]
fn undoing_typing_does_not_deploy() {
    let mut h = Harness::editing(SRC);
    h.type_in_editor_for_test("(*x*)");
    h.wait(1.5);
    assert_ne!(h.file(), SRC);
    h.ctrl(Key::Z);
    h.wait(0.3);
    assert_eq!(h.deployed(), SRC, "typing and its undo wait for Deploy");
}

#[test]
fn the_edit_is_highlighted_for_a_moment_and_the_editor_goes_to_it() {
    let mut h = Harness::editing(SRC);
    let at = h.op("OR");
    h.right_click(at);
    h.tap("diagram menu operator");
    h.tap("diagram op AND");
    let (range, until) = h.app.chrome.flash.clone().expect("the patch is highlighted");
    assert_eq!(&h.file()[range], "AND");
    assert!(until - h.time <= crate::editor::Chrome::FLASH + 0.01);
    assert_eq!(h.app.chrome.line, Some(10), "the editor is at the patched line");
    h.wait(1.7);
    assert!(
        h.app.chrome.flash.is_none(),
        "it fades after about a second and a half"
    );
}

#[test]
fn an_edited_block_keeps_where_it_was_dragged_to() {
    let mut h = Harness::editing(SRC);
    let from = h.op("OR");
    h.drag(from, from + Vec2::new(0.0, 70.0));
    let moved = sim_view(&h).arrangement().to_text();
    assert!(moved.contains("START OR MOTOR"), "{moved}");
    let at = h.op("OR");
    h.right_click(at);
    h.tap("diagram menu operator");
    h.tap("diagram op AND");
    h.wait(0.3);
    let after = sim_view(&h).arrangement().to_text();
    assert!(
        after.contains("START AND MOTOR"),
        "the new block is the one moved: {after}"
    );
    assert!(!after.contains("START OR MOTOR"), "{after}");
    let v = sim_view(&h);
    let fresh = crate::playground::Build::new(&v.src)
        .0
        .unwrap()
        .module
        .expect("the diagram");
    let (a, b) = (
        v.diagram()
            .nodes
            .iter()
            .find(|n| n.text == "START AND MOTOR")
            .unwrap()
            .rect,
        fresh
            .program()
            .unwrap()
            .diagram
            .nodes
            .iter()
            .find(|n| n.text == "START AND MOTOR")
            .unwrap()
            .rect,
    );
    assert!((a.y - b.y).abs() > 40.0, "{} against {}", a.y, b.y);
}

#[test]
fn the_camera_and_the_pick_survive_an_edit() {
    let mut h = Harness::editing(SRC);
    let empty = h.rect("left").center() + Vec2::new(0.0, 0.0);
    let _ = empty;
    let at = h.op("OR");
    h.right_click(at);
    h.tap("diagram menu operator");
    // Pan the view, then edit: the pan stays.
    let v = sim_view(&h);
    let cam = v.cams[v.scope];
    let (zoom, offset) = (cam.zoom, cam.offset);
    h.tap("diagram op AND");
    h.wait(0.3);
    let v = sim_view(&h);
    let cam = v.cams[v.scope];
    assert!((cam.zoom - zoom).abs() < 1e-4 && (cam.offset - offset).length() < 0.5);
    assert!(v.pick.is_some(), "the block picked by the click is still picked");
}

#[test]
fn a_refusal_is_shown_in_the_engines_words_and_the_file_is_untouched() {
    let mut h = Harness::editing(SRC);
    let at = h.block(|k, _| matches!(k, NodeKind::Read { path } if path == "STOP"));
    for name in ["END_IF", "", "a b", "1x", "lamp"] {
        h.right_click(at);
        h.tap("diagram menu rename");
        h.type_over(name);
        h.tap("diagram rename ok");
        let why = h
            .popup_error()
            .unwrap_or_else(|| panic!("no refusal for {name:?}"));
        assert!(why.starts_with("Not changed: "), "{why}");
        assert!(h.drew(&why), "the popup shows {why}");
        assert_eq!(h.file(), SRC, "{name:?} left the file as it was");
        assert_eq!(h.deployed(), SRC);
    }
    h.right_click(at);
    h.tap("diagram menu rename");
    h.type_over(&"n".repeat(5000));
    h.tap("diagram rename ok");
    assert_eq!(
        h.file(),
        SRC,
        "a name of 5000 letters is refused or fits the file"
    );
}

#[test]
fn a_keyword_is_refused_with_the_words_of_the_engine() {
    let mut h = Harness::editing(SRC);
    let at = h.block(|k, _| matches!(k, NodeKind::Read { path } if path == "STOP"));
    h.right_click(at);
    h.tap("diagram menu rename");
    h.type_over("IF");
    h.tap("diagram rename ok");
    let why = h.popup_error().expect("refused");
    assert!(why.contains("reserved word"), "{why}");
}

#[test]
fn a_value_that_does_not_fit_the_constant_is_refused() {
    let mut h = Harness::editing(SRC);
    let at = h.constant("10");
    for text in ["abc", "2.5", "TRUE", "", "99999999999999999999", "100000"] {
        h.right_click(at);
        h.tap("diagram menu value");
        h.type_over(text);
        h.tap("diagram value ok");
        let why = h
            .popup_error()
            .unwrap_or_else(|| panic!("no refusal for {text:?}"));
        assert_ne!(why, "");
        assert_eq!(h.file(), SRC, "{text:?}: {why}");
        h.tap("diagram value cancel");
        assert_eq!(h.stage(), "closed");
    }
}

#[test]
fn a_variable_of_the_wrong_type_is_refused_when_connected() {
    let mut h = Harness::editing(SRC);
    let at = h.pin("COUNT > 10", 0);
    h.right_click(at);
    h.tap("diagram menu connect");
    h.tap("diagram connect START");
    let why = h.popup_error().expect("refused");
    assert!(why.contains("types do not fit"), "{why}");
    assert_eq!(h.file(), SRC);
    assert_eq!(h.stage(), "connect", "the list stays for another try");
    // A path that is no variable at all.
    h.tap("diagram connect search");
    h.frame(vec![Event::Text("nosuch".into())]);
    h.frame(vec![]);
    h.tap("diagram connect typed");
    let why = h.popup_error().expect("refused");
    assert!(why.starts_with("Not changed: "), "{why}");
    assert_eq!(h.file(), SRC);
}

#[test]
fn gestures_refuse_while_the_code_is_ahead_of_the_diagram() {
    let mut h = Harness::editing(SRC);
    h.type_in_editor_for_test("(*x*)");
    h.wait(1.0);
    let typed = h.file();
    assert_ne!(typed, SRC);
    let at = h.op("OR");
    h.right_click(at);
    assert_eq!(h.stage(), "closed", "no menu");
    let note = h.off_note().expect("a note says why");
    assert!(note.contains("Deploy first"), "{note}");
    assert!(h.drew("Deploy first"));
    assert_eq!(h.file(), typed, "the file is as typed");
    // And the double click on a constant says the same, with no value field.
    let at = h.constant("10");
    h.double_click(at);
    assert_eq!(h.stage(), "closed");
    assert_eq!(h.file(), typed);
}

#[test]
fn gestures_refuse_when_the_code_does_not_compile() {
    let mut h = Harness::editing(SRC);
    h.type_in_editor_for_test("!!");
    h.wait(1.0);
    let diags = h.app.sim.as_ref().unwrap().session.diags.len();
    assert_ne!(diags, 0, "the typing does not compile");
    let broken = h.file();
    let at = h.op("OR");
    h.right_click(at);
    assert_eq!(h.stage(), "closed");
    let note = h.off_note().expect("a note says why");
    assert!(note.contains("does not compile"), "{note}");
    assert_eq!(h.file(), broken);
}

#[test]
fn the_edit_itself_is_refused_for_text_it_was_not_made_for() {
    // The hosts's gate and the engine's check are two locks: take the
    // gate away and an edit made for the old text still does not land.
    let mut h = Harness::editing(SRC);
    let at = h.op("OR");
    h.right_click(at);
    h.tap("diagram menu operator");
    // The code changes under the open menu.
    h.app
        .sim
        .as_mut()
        .unwrap()
        .proj
        .files
        .insert("main.st".into(), format!("(* later *)\n{SRC}"));
    h.wait(0.1);
    h.tap("diagram op AND");
    assert_eq!(h.file(), format!("(* later *)\n{SRC}"), "untouched");
    assert!(h.popup_error().is_some_and(|w| w.contains("Deploy")));
}

#[test]
fn a_project_of_several_program_files_is_not_edited_from_the_diagram() {
    let mut h = Harness::editing(SRC);
    h.app
        .sim
        .as_mut()
        .unwrap()
        .proj
        .add_file("more.st", "FUNCTION f : INT\nf := 1;\nEND_FUNCTION\n".into());
    h.wait(0.3);
    let at = h.op("OR");
    h.right_click(at);
    assert_eq!(h.stage(), "closed");
    let note = h.off_note().expect("a note says why");
    assert!(note.contains("one program file"), "{note}");
    assert_eq!(h.file(), SRC);
}

#[test]
fn the_menu_offers_only_what_applies() {
    // A constant has no operator, no rename and no bubble.
    let mut h = Harness::editing(&SRC.replace("lamp := count > 10;", "lamp := count > 10;\nlimit := 7;"));
    let at = h.constant("7");
    h.right_click(at);
    assert!(h.has("diagram menu value"));
    assert!(h.has("diagram menu connect"));
    assert!(!h.has("diagram menu operator"));
    assert!(!h.has("diagram menu rename"));
    // A literal compared against is one block with its comparison
    // (`> 10`): the value, a variable in its place, and the operator.
    let mut h = Harness::editing(SRC);
    let at = h.constant("10");
    h.right_click(at);
    assert!(h.has("diagram menu value"));
    assert!(h.has("diagram menu connect"));
    assert!(h.has("diagram menu operator"));
    assert!(!h.has("diagram menu rename"));
    // A variable has a rename and no operator.
    let at = h.block(|k, _| matches!(k, NodeKind::Read { path } if path == "COUNT"));
    h.right_click(at);
    assert!(h.has("diagram menu rename"));
    assert!(!h.has("diagram menu operator"));
    assert!(!h.has("diagram menu value"));
}

#[test]
fn the_popups_stay_inside_the_diagram_at_a_narrow_width() {
    let mut h = Harness::editing(SRC);
    h.size = Vec2::new(760.0, 640.0);
    h.wait(0.4);
    let at = h.pin("COUNT > 10", 0);
    h.right_click(at);
    h.tap("diagram menu connect");
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

#[test]
fn the_menu_and_popups_draw_nothing_under_the_smallest_text() {
    let mut h = Harness::editing(SRC);
    let at = h.op("OR");
    h.right_click(at);
    assert!(h.small_text().is_empty(), "{:?}", h.small_text());
    h.tap("diagram menu operator");
    assert!(h.small_text().is_empty(), "{:?}", h.small_text());
}

#[test]
fn an_edit_that_turns_a_requirement_test_red_still_runs_and_the_note_names_the_test() {
    let mut h = Harness::editing(SRC);
    {
        let v = h.app.sim.as_mut().unwrap();
        v.proj.create("lamp", None);
        v.proj
            .files
            .insert("main.st".into(), crate::verify_tests::LAMP.into());
        v.proj
            .add_file("test_lamp.st", crate::verify_tests::FOLLOWS.into());
        v.proj.save();
        v.open_project("lamp");
    }
    h.wait(0.4);
    assert!(
        h.app.sim.as_ref().unwrap().tests.warning().is_none(),
        "the test is green first"
    );
    let write = {
        let v = sim_view(&h);
        let n = v
            .diagram()
            .nodes
            .iter()
            .find(|n| matches!(&n.kind, NodeKind::Write { path } if path == "LAMP"))
            .unwrap();
        let (x, y) = in_port(n, 0);
        v.cams[v.scope].to_screen(x, y)
    };
    h.right_click(write);
    h.tap("diagram menu negate");
    let lamp = crate::verify_tests::LAMP;
    assert_eq!(h.file(), lamp.replace("lamp := start", "lamp := NOT start"));
    assert_eq!(
        h.deployed(),
        lamp.replace("lamp := start", "lamp := NOT start"),
        "a red test refuses no deploy: the edit runs"
    );
    let note = h
        .app
        .sim
        .as_ref()
        .unwrap()
        .notice
        .clone()
        .expect("a note names the test");
    assert!(note.contains("test_lamp") && note.contains("is red"), "{note}");
    // The diagram is of the code that runs: the next gesture opens.
    let at = h.block(|k, _| matches!(k, NodeKind::Write { .. }));
    h.right_click(at);
    assert!(h.off_note().is_none(), "{:?}", h.off_note());
}
