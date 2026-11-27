//! Show Changes in the simulation (docs/UI.md "Show Changes", Nick
//! 2026-11-27): a described change is held, never written on arrival;
//! Show Changes puts it on screen while the current program runs on;
//! Apply writes it, Discard drops it; one step is small. Driven through
//! the page as a person meets it, the Agent's answers scripted.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::rc::Rc;

use eframe::egui::Event;
use helmsman_iec::Value;

use super::{App, Harness};
use crate::instruct_tests::{FOLLOWS, LAMP, follows, reply};
use crate::playground_view::PlaygroundView;

/// A projects store whose text the test can read: what the page saved.
#[derive(Clone, Default)]
struct Store(Rc<RefCell<Option<String>>>);

impl crate::sim_projects::Storage for Store {
    fn load(&self) -> Result<Option<String>, String> {
        Ok(self.0.borrow().clone())
    }
    fn save(&mut self, text: &str) -> Result<(), String> {
        *self.0.borrow_mut() = Some(text.to_string());
        Ok(())
    }
}

/// The page on project `mine` (`main.st` is `main`, its requirement test
/// beside it), the Describe tab up, start pressed and the lamp lit.
fn page_with(main: &str) -> (Harness, Store) {
    let store = Store::default();
    let mut v = PlaygroundView::with_storage(Box::new(store.clone()));
    v.chat.receive(crate::claude::Incoming::Proxy(true));
    v.proj.create("mine", None);
    v.proj.files.insert("main.st".into(), main.into());
    v.proj.add_file("test_lamp.st", FOLLOWS.into());
    v.proj.save();
    v.open_project("mine");
    v.proj.open_file("main.st");
    let mut app = App::sim();
    app.sim = Some(v);
    let mut h = Harness::new(app);
    h.app.chrome.dock(crate::editor::Pane::Claude, true);
    if !h.has("agent instruction") {
        h.press("tab claude");
    }
    h.wait(0.2);
    (h, store)
}

fn sim(h: &mut Harness) -> &mut PlaygroundView {
    h.app.sim.as_mut().expect("the simulation")
}

fn main_st(h: &Harness) -> String {
    h.app.sim.as_ref().unwrap().proj.files["main.st"].clone()
}

/// Types `instruction`, presses Send, and the Agent answers with `code`
/// (the whole file) and a scenario that holds on it.
fn ask(h: &mut Harness, instruction: &str, code: &str, scenario: &str) {
    sim(h).instruct.draft = instruction.into();
    h.press("agent send");
    h.frame(vec![]);
    let answer = format!("Why: as asked.\n\n```st\n{code}```\n\n{scenario}");
    // Answered the same each time it is sent back, as `instruct` does,
    // until it asks nothing more.
    for _ in 0..5 {
        let n = sim(h).link.sent.len();
        reply(sim(h), &answer);
        h.wait(0.2);
        if !sim(h).instruct.busy() || sim(h).link.sent.len() == n {
            return;
        }
    }
    panic!("the instruction never settled: {:?}", sim(h).instruct.outcome);
}

/// The text the last frame drew inside the control `key`.
fn text_of(h: &Harness, key: &str) -> String {
    fn walk(s: &eframe::egui::Shape, r: eframe::egui::Rect, out: &mut String) {
        match s {
            eframe::egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, r, out)),
            eframe::egui::Shape::Text(t) if r.expand(2.0).contains(t.visual_bounding_rect().center()) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            _ => {}
        }
    }
    let r = h.rect(key);
    let mut out = String::new();
    for c in &h.shapes {
        walk(&c.shape, r, &mut out);
    }
    out
}

/// The lamp, inverted: what the scripted Agent proposes in most of these.
fn inverted() -> String {
    LAMP.replace("lamp := start;", "lamp := NOT start;")
}

/// A scenario that fails on [`LAMP`] and holds on [`inverted`].
fn lamp_inverted() -> String {
    "```st scenario\n(* The lamp is off while start is pressed. *)\nPROGRAM test_lamp_inverted\nVAR_INPUT\n  lamp : BOOL;\nEND_VAR\nVAR_OUTPUT\n  start : BOOL;\n  done : BOOL;\nEND_VAR\nVAR\n  n : INT;\n  lamp_off : ASSERT;\nEND_VAR\nstart := TRUE;\nn := n + 1;\nIF n > 3 THEN\n  lamp_off(COND := NOT lamp);\n  done := TRUE;\nEND_IF;\nEND_PROGRAM\n```\n".into()
}

/// What the simulation runs, and the lamp it drives (start is not
/// pressed: [`LAMP`] keeps it off, [`inverted`] on).
fn running(h: &mut Harness) -> (Option<String>, Option<Value>) {
    h.wait(0.3);
    let v = sim(h);
    (
        v.session.build.as_ref().map(|b| b.src.clone()),
        v.session.value("lamp"),
    )
}

/// The page with a change held: `mine` on [`LAMP`], asked to invert the
/// lamp, answered with [`inverted`].
fn held() -> (Harness, Store) {
    let (mut h, store) = page_with(LAMP);
    assert_eq!(running(&mut h), (Some(LAMP.into()), Some(Value::Bool(false))));
    ask(&mut h, "invert the lamp", &inverted(), &lamp_inverted());
    assert!(
        sim(&mut h).instruct.pending.is_some(),
        "{:?}",
        sim(&mut h).instruct.outcome
    );
    (h, store)
}

/// Docs/UI.md "Show Changes": a green answer is held, never written on
/// arrival: nothing is written, saved, run or deployed; the tab shows its
/// size and the outputs it changes by name, then Show Changes, Apply and
/// Discard. Apply writes it as before, one revision that runs at once. A
/// reload drops what was held.
#[test]
fn a_green_answer_is_held_and_nothing_is_written_saved_or_run_until_apply() {
    let (mut h, store) = page_with(LAMP);
    let saved = store.0.borrow().clone();
    let revs = sim(&mut h).proj.revisions().len();
    assert_eq!(running(&mut h), (Some(LAMP.into()), Some(Value::Bool(false))));
    ask(&mut h, "invert the lamp", &inverted(), &lamp_inverted());

    // Held: the file, the store, the history and the scan are as they were.
    assert!(
        sim(&mut h).instruct.pending.is_some(),
        "{:?}",
        sim(&mut h).instruct.outcome
    );
    assert_eq!(main_st(&h), LAMP, "nothing written");
    assert!(!sim(&mut h).proj.dirty(), "nothing unsaved either");
    assert_eq!(*store.0.borrow(), saved, "nothing saved");
    assert_eq!(sim(&mut h).proj.revisions().len(), revs, "no revision");
    assert_eq!(
        running(&mut h),
        (Some(LAMP.into()), Some(Value::Bool(false))),
        "the current program runs on, the lamp still off"
    );
    for k in [
        "agent pending",
        "agent pending size",
        "agent pending outputs",
        "agent show changes",
        "agent apply",
        "agent discard",
    ] {
        assert!(h.has(k), "{k} in the tab");
    }
    let size = text_of(&h, "agent pending size");
    assert!(
        size.contains("2 blocks") || size.contains("1 block"),
        "its size in blocks: {size}"
    );
    assert!(size.contains("2 lines"), "and lines: {size}");
    let outputs = text_of(&h, "agent pending outputs");
    assert!(
        outputs.contains("LAMP"),
        "the output it switches by name: {outputs}"
    );
    assert!(
        !h.has("agent pending refusal"),
        "a small named change may be applied"
    );

    // A reload drops it: the page read back from the store has the file as
    // it was and nothing held.
    {
        let mut again = PlaygroundView::with_storage(Box::new(store.clone()));
        again.open_project("mine");
        assert_eq!(again.proj.files["main.st"], LAMP);
        assert!(again.instruct.pending.is_none());
    }

    // Apply: written, saved, one revision, run at once.
    h.press("agent apply");
    h.wait(0.2);
    assert!(sim(&mut h).instruct.pending.is_none());
    assert_eq!(main_st(&h), inverted());
    assert!(!sim(&mut h).proj.dirty(), "saved");
    assert_ne!(*store.0.borrow(), saved, "the store has it");
    assert_eq!(sim(&mut h).proj.revisions().len(), revs + 1, "one revision");
    assert_eq!(
        running(&mut h),
        (Some(inverted()), Some(Value::Bool(true))),
        "the change runs at once"
    );
}

/// Send is off while a change is held, and says why; nothing is sent.
#[test]
fn send_is_off_while_a_change_is_held() {
    let (mut h, _) = held();
    let n = sim(&mut h).link.sent.len();
    sim(&mut h).instruct.draft = "and latch it".into();
    h.frame(vec![]);
    if h.has("agent send") {
        h.press("agent send");
    }
    h.wait(0.2);
    // Enter in the box sends as Send does.
    let field = h.rect("agent instruction");
    h.click(field.center());
    h.key(eframe::egui::Modifiers::NONE, eframe::egui::Key::Enter);
    h.wait(0.2);
    assert_eq!(sim(&mut h).link.sent.len(), n, "nothing is sent");
    // Why, on the tab or on the Send glyph's tip.
    let said = h.drew("Apply or Discard the change first") || {
        let at = h.rect("agent send").center();
        h.frame(vec![Event::PointerMoved(at)]);
        h.wait(1.5);
        h.drew("Apply or Discard the change first")
    };
    assert!(said, "the tab says why Send is off");
    assert!(sim(&mut h).instruct.pending.is_some(), "the change is still held");
    assert_eq!(main_st(&h), LAMP);
}

/// Show Changes: the proposed file in the Code tab, read-only and marked
/// as a diff; the proposed diagram on the canvas under the banner; the
/// machine runs the current program the whole time. Show current goes
/// back, and Apply on the banner writes it as the tab's does.
#[test]
fn show_changes_draws_the_proposal_while_the_current_program_runs_on() {
    let (mut h, store) = held();
    let saved = store.0.borrow().clone();
    h.press("agent show changes");
    h.app.chrome.show_code();
    h.wait(0.3);
    for k in [
        "changes banner",
        "changes show current",
        "changes apply",
        "changes discard",
        "st proposed",
    ] {
        assert!(h.has(k), "{k} on screen");
    }
    assert!(
        h.drew(crate::change_ui::BANNER),
        "the banner says the machine runs the current program"
    );
    assert!(
        h.drew("Proposed change, not applied: the machine runs the current program"),
        "in the spec's words"
    );
    assert!(h.drew("lamp := NOT start;"), "the proposed line is on screen");
    // The machine runs the current program while the proposal is shown.
    let scans = sim(&mut h).session.scans;
    assert_eq!(
        running(&mut h),
        (Some(LAMP.into()), Some(Value::Bool(false))),
        "the old program drives the lamp"
    );
    assert!(sim(&mut h).session.scans > scans, "and it keeps scanning");
    assert_eq!(main_st(&h), LAMP);
    assert_eq!(*store.0.borrow(), saved);

    // Read-only: typing into the proposed file changes nothing.
    let at = h.rect("st proposed").center();
    h.click(at);
    h.frame(vec![Event::Text("zzz".into())]);
    h.wait(0.1);
    assert_eq!(main_st(&h), LAMP, "the file is untouched");
    assert!(!h.drew("zzz"), "nothing typed lands in the proposal");
    // The tab's Apply lands exactly what was shown.
    h.press("agent apply");
    h.wait(0.2);
    assert!(sim(&mut h).instruct.pending.is_none());
    assert!(
        !h.has("changes banner") && !h.has("st proposed"),
        "the banner goes with it"
    );
    assert_eq!(main_st(&h), inverted());
    assert_eq!(running(&mut h), (Some(inverted()), Some(Value::Bool(true))));
}

/// The banner's own buttons answer a click where they are drawn: Show
/// current goes back to the current file (still held), Apply writes the
/// change as the tab's does, Discard drops it. Nothing else on the canvas
/// (the run controls float there) takes the click.
#[test]
fn the_banners_show_current_apply_and_discard_answer_a_click() {
    let (mut h, store) = held();
    let saved = store.0.borrow().clone();
    h.press("agent show changes");
    h.app.chrome.show_code();
    h.wait(0.3);
    for k in ["changes show current", "changes apply", "changes discard"] {
        let r = h.rect(k);
        let over: Vec<&String> = h
            .app
            .chrome
            .hits
            .iter()
            .filter(|(o, b)| {
                o != k && !o.starts_with("changes") && b.intersects(r.shrink(1.0)) && b.area() < 200_000.0
            })
            .map(|(o, _)| o)
            .collect();
        assert!(over.is_empty(), "{k} {r:?} is under {over:?}");
    }
    // Show current: the file as it is, no banner, still held.
    h.press("changes show current");
    h.wait(0.2);
    assert!(
        !h.has("changes banner") && !h.has("st proposed"),
        "back to the current file"
    );
    assert!(sim(&mut h).instruct.pending.is_some(), "still held");
    assert_eq!(main_st(&h), LAMP);

    // Shown again, the banner's Discard drops it and changes nothing.
    h.press("agent show changes");
    h.wait(0.2);
    h.press("changes discard");
    h.wait(0.2);
    assert!(
        sim(&mut h).instruct.pending.is_none(),
        "the banner's Discard drops it"
    );
    assert_eq!(main_st(&h).as_bytes(), LAMP.as_bytes());
    assert_eq!(*store.0.borrow(), saved);
    // A change held and shown again: the banner's Apply writes it.
    ask(&mut h, "invert the lamp", &inverted(), &lamp_inverted());
    h.press("agent show changes");
    h.wait(0.2);
    h.press("changes apply");
    h.wait(0.2);
    assert!(sim(&mut h).instruct.pending.is_none());
    assert!(!h.has("changes banner"), "the banner goes with it");
    assert_eq!(main_st(&h), inverted());
    assert_eq!(running(&mut h), (Some(inverted()), Some(Value::Bool(true))));
}

/// Discard in the tab, the change shown or not, drops the held change and changes
/// nothing: the file byte for byte, the store, the history, the scan; and
/// Send works again.
#[test]
fn discard_from_the_tab_shown_or_not_leaves_everything_as_it_was() {
    // The banner's Discard: `the_banners_show_current_apply_and_discard_answer_a_click`.
    for (shown, key) in [(false, "agent discard"), (true, "agent discard")] {
        let (mut h, store) = held();
        let saved = store.0.borrow().clone();
        let revs = sim(&mut h).proj.revisions().len();
        let files = sim(&mut h).proj.files.clone();
        if shown {
            h.press("agent show changes");
            h.wait(0.2);
        }
        h.press(key);
        h.wait(0.2);
        assert!(sim(&mut h).instruct.pending.is_none(), "{key}: dropped");
        assert_eq!(sim(&mut h).proj.files, files, "{key}: every file byte for byte");
        assert_eq!(main_st(&h).as_bytes(), LAMP.as_bytes());
        assert_eq!(*store.0.borrow(), saved, "{key}: nothing saved");
        assert_eq!(sim(&mut h).proj.revisions().len(), revs, "{key}: no revision");
        assert!(!h.has("changes banner"), "{key}: no banner");
        assert_eq!(running(&mut h), (Some(LAMP.into()), Some(Value::Bool(false))));
        // Send is on again.
        let n = sim(&mut h).link.sent.len();
        sim(&mut h).instruct.draft = "comment it".into();
        h.press("agent send");
        h.frame(vec![]);
        assert_eq!(sim(&mut h).link.sent.len(), n + 1, "{key}: Send works again");
    }
}

/// Apply refuses when the file changed since Send: an edit by hand,
/// saved or not, is never written over.
#[test]
fn apply_after_an_edit_by_hand_since_send_is_refused_and_writes_nothing() {
    for save in [false, true] {
        let (mut h, store) = held();
        let edited = format!("{LAMP}(* mine *)\n");
        {
            let v = sim(&mut h);
            v.proj.current_text_mut().unwrap().clone_from(&edited);
            if save {
                v.proj.save();
            }
        }
        h.wait(0.2);
        let saved = store.0.borrow().clone();
        let revs = sim(&mut h).proj.revisions().len();
        h.press("agent apply");
        h.wait(0.2);
        assert_eq!(main_st(&h), edited, "saved={save}: the edit stands");
        assert!(
            !main_st(&h).contains("NOT start"),
            "saved={save}: nothing of the change"
        );
        assert_eq!(*store.0.borrow(), saved, "saved={save}: nothing saved");
        assert_eq!(
            sim(&mut h).proj.revisions().len(),
            revs,
            "saved={save}: no revision"
        );
    }
}

/// `n` flags, each read from start: two blocks and two lines each, in a
/// VAR block of its own (two lines more).
fn flags(n: usize) -> (String, String) {
    let decl = (0..n).fold(String::new(), |mut s, k| {
        let _ = writeln!(s, "  x{k} : BOOL;");
        s
    });
    let body = (0..n).fold(String::new(), |mut s, k| {
        let _ = writeln!(s, "x{k} := start;");
        s
    });
    (decl, body)
}

/// [`LAMP`] with `decl` in a VAR block, `body` after the lamp's line and
/// `notes` comment lines before `END_PROGRAM`.
fn grown(decl: &str, body: &str, notes: usize) -> String {
    let notes = (0..notes).fold(String::new(), |mut s, k| {
        let _ = writeln!(s, "(* note {k} *)");
        s
    });
    let vars = if decl.is_empty() {
        String::new()
    } else {
        format!("VAR\n{decl}END_VAR\n")
    };
    LAMP.replace("VAR_OUTPUT\n", &format!("{vars}VAR_OUTPUT\n"))
        .replace("lamp := start;\n", &format!("lamp := start;\n{body}"))
        .replace("END_PROGRAM\n", &format!("{notes}END_PROGRAM\n"))
}

/// One step is at most 8 blocks or 30 lines (docs/UI.md "Show Changes"):
/// exactly 8 blocks and exactly 30 lines apply; 9 blocks or 31 lines do
/// not, and the tab says why in the spec's words.
#[test]
fn exactly_eight_blocks_and_thirty_lines_apply_nine_or_thirty_one_do_not() {
    let (decl4, body4) = flags(4);
    let nine = (format!("{decl4}  y : BOOL;\n"), format!("{body4}y := x0;\n"));
    // (instruction text, file, blocks, lines, may it apply)
    let cases: Vec<(String, usize, usize, bool)> = vec![
        // 4 flags: 8 blocks, 10 lines; 20 notes more: 30 lines.
        (grown(&decl4, &body4, 20), 8, 30, true),
        (grown(&decl4, &body4, 0), 8, 10, true),
        (grown(&nine.0, &nine.1, 0), 9, 12, false),
        (grown("", "", 30), 0, 30, true),
        (grown("", "", 31), 0, 31, false),
        (grown(&decl4, &body4, 21), 8, 31, false),
    ];
    for (code, blocks, lines, ok) in cases {
        let (mut h, store) = page_with(LAMP);
        let saved = store.0.borrow().clone();
        ask(&mut h, "add the flags and notes", &code, &follows());
        assert!(
            sim(&mut h).instruct.pending.is_some(),
            "{blocks} blocks, {lines} lines: held {:?}",
            sim(&mut h).instruct.outcome
        );
        let size = text_of(&h, "agent pending size");
        assert!(
            size.contains(&format!("{blocks} blocks"))
                || (blocks == 0 && !size.contains("block"))
                || size.contains(&format!("{blocks} block,")),
            "{blocks} blocks, {lines} lines: {size}"
        );
        assert!(
            size.contains(&format!("{lines} lines")),
            "{blocks} blocks, {lines} lines: {size}"
        );
        h.press("agent apply");
        h.wait(0.2);
        if ok {
            assert_eq!(main_st(&h), code, "{blocks} blocks, {lines} lines: applied");
        } else {
            let refusal = format!(
                "Too big for one step: {blocks} blocks, {lines} lines (one step is at most 8 blocks or 30 lines). Ask for part of it."
            );
            assert!(
                h.has("agent pending refusal"),
                "{blocks} blocks, {lines} lines: refused"
            );
            let said = text_of(&h, "agent pending refusal").replace('\n', " ");
            assert!(
                said.contains("Too big for one step")
                    && said.contains(&format!(
                        "{lines} lines (one step is at most 8 blocks or 30 lines). Ask for part of it."
                    )),
                "{blocks} blocks, {lines} lines: {said:?}, want {refusal:?}"
            );
            if blocks > 0 {
                assert!(said.contains(&refusal), "{said:?}, want {refusal:?}");
            }
            assert_eq!(
                main_st(&h),
                LAMP,
                "{blocks} blocks, {lines} lines: nothing written"
            );
            assert_eq!(*store.0.borrow(), saved);
        }
    }
}

/// Two outputs: the lamp and the motor's speed.
const TWO: &str = "PROGRAM lamp\nVAR_INPUT\n  start : BOOL;\nEND_VAR\nVAR_OUTPUT\n  lamp : BOOL;\n  motor_speed : INT;\nEND_VAR\nlamp := start;\nmotor_speed := 0;\nEND_PROGRAM\n";

/// A change of the logic of an output the instruction does not name is
/// refused (docs/UI.md "Show Changes"); named in any case, or with `_` as
/// a space, it applies. A word that is only part of the name, or a longer
/// name with it inside, does not name it.
#[test]
fn an_output_the_instruction_does_not_name_is_refused() {
    let faster = TWO.replace(
        "motor_speed := 0;",
        "IF start THEN\n  motor_speed := 10;\nELSE\n  motor_speed := 0;\nEND_IF;",
    );
    let cases = [
        ("run it faster with start", false),
        ("set the motor to 10 with start", false),
        ("set motor_speed_limit to 10", false),
        ("set the motorspeed to 10", false),
        ("set motor_speed to 10 with start", true),
        ("set MOTOR_SPEED to 10 with start", true),
        ("set Motor_Speed to 10 with start", true),
        ("set the motor speed to 10 with start", true),
        ("set the Motor Speed to 10 with start", true),
        ("set motor_speed, to 10", true),
    ];
    let mut wrong = Vec::new();
    for (instruction, ok) in cases {
        let (mut h, store) = page_with(TWO);
        let saved = store.0.borrow().clone();
        ask(&mut h, instruction, &faster, &follows());
        assert!(sim(&mut h).instruct.pending.is_some(), "{instruction}: held");
        let outputs = text_of(&h, "agent pending outputs");
        assert!(outputs.contains("MOTOR_SPEED"), "{instruction}: {outputs}");
        assert!(
            !outputs.contains("LAMP"),
            "{instruction}: the lamp is unchanged: {outputs}"
        );
        h.press("agent apply");
        h.wait(0.2);
        if ok {
            if main_st(&h) != faster {
                wrong.push(format!("{instruction:?} names motor_speed but was not applied"));
            }
        } else if main_st(&h) == TWO {
            assert_eq!(*store.0.borrow(), saved);
            let said = text_of(&h, "agent pending refusal").replace('\n', " ");
            assert!(
                said.contains(
                    "It changes MOTOR_SPEED, which the instruction does not name. Name it, or ask for less."
                ),
                "{instruction:?}: {said:?}"
            );
        } else {
            wrong.push(format!(
                "{instruction:?} does not name motor_speed but was applied"
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A current file that does not compile has no diagram to count blocks
/// on: the change is held to the line cap alone, and the tab says so.
#[test]
fn a_broken_current_file_is_held_to_the_line_cap_with_a_note() {
    let broken = LAMP.replace("lamp := start;", "lamp := ;");
    for (notes, ok) in [(0, true), (40, false)] {
        let (mut h, store) = page_with(&broken);
        let saved = store.0.borrow().clone();
        // The page keeps the broken file as it reads it.
        let broken = main_st(&h);
        assert!(broken.contains("lamp :="), "{broken}");
        let fixed = grown("", "", notes);
        ask(&mut h, "fix the lamp", &fixed, &follows());
        assert!(
            sim(&mut h).instruct.pending.is_some(),
            "held even when the file was broken: {:?}",
            sim(&mut h).instruct.outcome
        );
        assert!(h.has("agent pending note"), "the tab says only the lines count");
        assert_eq!(main_st(&h), broken, "nothing written");
        h.press("agent apply");
        h.wait(0.2);
        if ok {
            assert_eq!(main_st(&h), fixed, "within the line cap: applied");
        } else {
            assert_eq!(main_st(&h), broken, "past the line cap: refused");
            assert_eq!(*store.0.borrow(), saved);
            assert!(h.has("agent pending refusal"));
        }
    }
}

/// A new project (nothing of the engineer's open) is not held: it
/// becomes a project of its own as before, under the name asked for,
/// whatever its size.
#[test]
fn a_new_project_is_not_held() {
    let (mut h, _) = page_with(LAMP);
    sim(&mut h).open_project("blink");
    h.wait(0.2);
    assert_eq!(sim(&mut h).scope(), crate::scope::Scope::NewProject);
    let big = grown(&flags(10).0, &flags(10).1, 40);
    ask(&mut h, "make a lamp with flags", &big, &follows());
    let v = sim(&mut h);
    assert!(v.instruct.pending.is_none(), "not held: {:?}", v.instruct.outcome);
    assert!(
        v.program_name.is_some(),
        "asked for its name: {:?}",
        v.instruct.outcome
    );
    v.program_name.as_mut().unwrap().input = "flags".into();
    v.confirm_program_name();
    h.wait(0.2);
    let v = sim(&mut h);
    assert_eq!(v.proj.open, "flags");
    assert!(
        v.proj.files.values().any(|t| *t == big),
        "{:?}",
        v.proj.files.keys()
    );
}

/// Deploy in the simulation is unchanged: the page's own program takes
/// the edit at once, with no sheet.
#[test]
fn deploy_in_the_simulation_takes_the_edit_with_no_sheet() {
    let (mut h, _) = page_with(LAMP);
    h.app.chrome.show_code();
    h.wait(0.2);
    sim(&mut h)
        .proj
        .current_text_mut()
        .unwrap()
        .clone_from(&inverted());
    h.frame(vec![]);
    h.press("code deploy");
    h.wait(0.3);
    assert!(!h.has("deploy sheet"), "no sheet in the simulation");
    assert_eq!(running(&mut h), (Some(inverted()), Some(Value::Bool(true))));
}

mod naming {
    use proptest::prelude::*;

    use crate::change::named;

    /// An output's name: one to three lower-case words joined by `_`.
    fn name() -> impl Strategy<Value = String> {
        proptest::collection::vec("[a-z]{1,6}", 1..=3).prop_map(|w| w.join("_"))
    }

    /// `name` as an engineer may type it: any case, each `_` a `_` or a
    /// space.
    fn typed(name: &str, upper: &[bool], space: &[bool]) -> String {
        let mut k = 0;
        name.chars()
            .map(|c| {
                k += 1;
                if c == '_' {
                    if space[k % space.len()] { ' ' } else { '_' }
                } else if upper[k % upper.len()] {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect()
    }

    proptest! {
        /// Named in any case, `_` or space, among other words: named.
        #[test]
        fn a_name_typed_in_any_case_with_spaces_for_underscores_is_named(
            n in name(),
            upper in proptest::collection::vec(any::<bool>(), 1..8),
            space in proptest::collection::vec(any::<bool>(), 1..8),
            before in "(set|make|the|please|[0-9]{1,3}|)",
            after in "(to 10|faster|,|\\.|)",
        ) {
            let text = format!("{before} {} {after}", typed(&n, &upper, &space));
            prop_assert!(named(&text, &n), "{text:?} names {n:?}");
        }

        /// A word that only starts or ends like the name is not the name.
        #[test]
        fn a_word_with_the_name_inside_it_is_not_the_name(
            n in name(),
            more in "[a-z0-9]{1,3}",
            before in "(set|make|the|)",
        ) {
            prop_assert!(!named(&format!("{before} {n}{more} now"), &n), "{n}{more}");
            prop_assert!(!named(&format!("{before} {more}{n} now"), &n), "{more}{n}");
        }

        /// Part of the name alone does not name it.
        #[test]
        fn part_of_a_name_does_not_name_it(
            head in "[a-z]{1,6}",
            tail in "[a-z]{1,6}",
        ) {
            let n = format!("{head}_{tail}");
            prop_assert!(!named(&format!("set the {head} now"), &n), "{head} of {n}");
            prop_assert!(!named(&format!("set the {tail} now"), &n), "{tail} of {n}");
        }
    }
}
