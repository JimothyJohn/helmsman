//! The shell's keys, header and editor gutter: bugs found reading the
//! shell (2026-11-08), each with the regression that failed first.

use eframe::egui::{Event, Key, Modifiers, Vec2};

use super::Harness;
use crate::app::{App, Screen};

impl Harness {
    /// A press and a release of `key`.
    fn chord(&mut self, modifiers: Modifiers, key: Key) {
        for pressed in [true, false] {
            self.frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            }]);
        }
    }

    /// The simulation's open file.
    fn sim_text(&self) -> String {
        let p = &self.app.sim.as_ref().unwrap().proj;
        p.files[&p.current].clone()
    }

    /// Clicks into the ST editor (it takes the keyboard) and types `s`.
    fn type_in_editor(&mut self, s: &str) {
        let at = self.rect("st editor").left_top() + Vec2::new(60.0, 6.0);
        self.click(at);
        self.frame(vec![Event::Text(s.into())]);
    }
}

#[test]
fn ctrl_z_in_the_editor_undoes_one_step() {
    let mut h = Harness::code();
    h.wait(0.3);
    h.type_in_editor("(*X*)");
    // Past the editor's undo grouping time, so each typing is a step.
    h.wait(1.5);
    let with_x = h.sim_text();
    h.frame(vec![Event::Text("(*Y*)".into())]);
    h.wait(1.5);
    assert_ne!(h.sim_text(), with_x);
    h.chord(Modifiers::COMMAND, Key::Z);
    h.wait(0.2);
    assert_eq!(h.sim_text(), with_x, "one Ctrl+Z undoes the last typing only");
    h.chord(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
    h.wait(0.2);
    assert!(h.sim_text().contains("(*Y*)"), "and Ctrl+Shift+Z puts it back");
}

#[test]
fn undo_after_a_file_switch_does_not_bring_the_other_files_text() {
    let mut h = Harness::code();
    h.wait(0.3);
    h.type_in_editor("(*AAA*)");
    h.wait(1.5);
    h.app
        .sim
        .as_mut()
        .unwrap()
        .proj
        .add_file("other.st", "(* other *)\n".into());
    h.wait(0.5);
    let other = h.sim_text();
    assert!(!other.contains("AAA"));
    let at = h.rect("st editor").left_top() + Vec2::new(60.0, 6.0);
    h.click(at);
    h.chord(Modifiers::COMMAND, Key::Z);
    h.wait(0.3);
    assert_eq!(h.sim_text(), other, "the other file's history stays with it");
}

#[test]
fn save_and_deploy_do_nothing_on_the_monitor() {
    let mut h = Harness::code();
    h.wait(0.3);
    h.type_in_editor("!");
    h.wait(0.6);
    assert!(h.has("dirty"));
    h.app.screen = Screen::Monitor;
    h.wait(0.2);
    h.app.sim.as_mut().unwrap().notice = None;
    h.chord(Modifiers::COMMAND, Key::S);
    h.chord(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Enter);
    h.wait(0.2);
    assert!(h.app.sim.as_ref().unwrap().proj.dirty(), "Ctrl+S did not save");
    assert!(
        h.app.sim.as_ref().unwrap().notice.is_none(),
        "Deploy's key did not deploy the hidden project"
    );
    h.app.screen = Screen::Editor;
    h.wait(0.2);
    h.chord(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Enter);
    h.wait(0.2);
    assert!(
        h.app.sim.as_ref().unwrap().notice.is_some(),
        "on the Editor it deploys"
    );
}

/// Every header control's rect, the header's own aside.
fn header_hits(h: &Harness) -> Vec<(String, eframe::egui::Rect)> {
    let header = h.rect("header");
    h.app
        .chrome
        .hits
        .iter()
        .filter(|(k, r)| k != "header" && header.contains_rect(*r) && r.height() < header.height() + 1.0)
        .cloned()
        .collect()
}

/// At a phone's width the right group (title, screens) was drawn over the
/// left (mark, menus): nothing truncated or collapsed.
#[test]
fn no_two_header_controls_overlap_at_a_phones_width() {
    for width in [390.0, 360.0, 320.0] {
        for screen in [Screen::Editor, Screen::Monitor] {
            let mut h = Harness::new(App::sim());
            h.size = Vec2::new(width, 780.0);
            h.app.screen = screen;
            h.wait(0.5);
            let hits = header_hits(&h);
            assert!(hits.len() >= 3, "{hits:?}");
            for (i, (a, ra)) in hits.iter().enumerate() {
                for (b, rb) in &hits[i + 1..] {
                    assert!(
                        !ra.shrink(0.5).intersects(rb.shrink(0.5)),
                        "{a} {ra:?} overlaps {b} {rb:?} at {width} pt on {screen:?}"
                    );
                }
                assert!(ra.right() <= width + 0.5, "{a} {ra:?} is off a {width} pt screen");
            }
            assert!(h.has("bar menu"), "the four menus are one button on a phone");
            assert!(!h.has("bar file"));
        }
    }
}

/// The galleys of the last frame whose text satisfies `f`.
fn galleys(h: &Harness, f: impl Fn(&str) -> bool) -> Vec<std::sync::Arc<eframe::egui::Galley>> {
    fn walk(
        shape: &eframe::egui::Shape,
        f: &impl Fn(&str) -> bool,
        out: &mut Vec<std::sync::Arc<eframe::egui::Galley>>,
    ) {
        match shape {
            eframe::egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, f, out)),
            eframe::egui::Shape::Text(t) if f(&t.galley.job.text) => out.push(t.galley.clone()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, &f, &mut out);
    }
    out
}

/// The line numbers drifted one row early after a line that wrapped: the
/// gutter was laid out at a width the editor does not wrap at.
#[test]
fn the_gutter_has_a_row_for_every_row_the_editor_wraps_to() {
    let mut h = Harness::code();
    let mut text = String::from("PROGRAM p\n");
    text.extend((0..60).map(|n| format!("(* {} *)\n", "w".repeat(14 + n))));
    text.push_str("END_PROGRAM\n");
    {
        let p = &mut h.app.sim.as_mut().unwrap().proj;
        let cur = p.current.clone();
        p.files.insert(cur, text.clone());
    }
    h.wait(0.5);
    let editor = galleys(&h, |t| t.starts_with("PROGRAM p"));
    let gutter = galleys(&h, |t| t.starts_with("  1\n"));
    assert_eq!((editor.len(), gutter.len()), (1, 1));
    let (e, g) = (&editor[0], &gutter[0]);
    assert!(e.rows.len() > text.lines().count(), "some lines wrap in the pane");
    assert!(g.rows.len() >= e.rows.len(), "a gutter row for every editor row");
    for (k, row) in e.rows.iter().enumerate() {
        let numbered = k == 0 || e.rows[k - 1].ends_with_newline;
        let shown = g.rows[k].glyphs.iter().any(|c| c.chr.is_ascii_digit());
        assert_eq!(shown, numbered, "row {k}: a number only on a line's first row");
        assert!(
            (g.rows[k].pos.y - row.pos.y).abs() < 0.5,
            "row {k} is at {} in the gutter, {} in the editor",
            g.rows[k].pos.y,
            row.pos.y
        );
    }
}

/// The Code tab's header: "main.st ↵ → ✔ compiles" broke inside the
/// status label once the deploy arrow took its room, "compiles" alone on
/// a line. A label wraps as a whole or not at all. Since 2026-11-23 the
/// header says nothing while the code compiles (Nick): no "compiles".
#[test]
fn the_code_headers_status_labels_are_not_broken_across_lines() {
    let mut h = Harness::code();
    h.wait(0.5);
    {
        let p = &mut h.app.sim.as_mut().unwrap().proj;
        let cur = p.current.clone();
        p.files.get_mut(&cur).unwrap().insert_str(0, "(* edited *)\n");
    }
    h.wait(1.0);
    let before = h.rect("pane code").width();
    let file = h.app.sim.as_ref().unwrap().proj.current.clone();
    assert!(
        galleys(&h, |t| t.contains("compiles")).is_empty() && !h.has("code errors"),
        "code that compiles has no status"
    );
    for needle in [file.as_str(), "not deployed"] {
        let found = galleys(&h, |t| t.contains(needle));
        assert!(
            needle == "not deployed" || !found.is_empty(),
            "the header draws {needle:?}"
        );
        for g in found {
            assert_eq!(g.rows.len(), 1, "{:?} is broken across lines", g.job.text);
        }
    }
    h.wait(0.5);
    assert!(
        (h.rect("pane code").width() - before).abs() < 1.0,
        "the pane keeps its width"
    );
}

/// Where the last frame drew text containing `needle`: the clip it was
/// drawn in, its top-left and its layout.
fn text_at(
    h: &Harness,
    needle: &str,
) -> Vec<(
    eframe::egui::Rect,
    eframe::egui::Pos2,
    std::sync::Arc<eframe::egui::Galley>,
)> {
    fn walk(
        shape: &eframe::egui::Shape,
        clip: eframe::egui::Rect,
        needle: &str,
        out: &mut Vec<(
            eframe::egui::Rect,
            eframe::egui::Pos2,
            std::sync::Arc<eframe::egui::Galley>,
        )>,
    ) {
        match shape {
            eframe::egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, needle, out)),
            eframe::egui::Shape::Text(t) if t.galley.job.text.contains(needle) => {
                out.push((clip, t.pos, t.galley.clone()));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, c.clip_rect, needle, &mut out);
    }
    out
}

/// A long message was one unbroken line in a list that only scrolls
/// down, cut off at the panel's edge.
#[test]
fn a_long_message_wraps_inside_the_messages_panel() {
    let mut h = Harness::new(App::sim());
    h.messages();
    let long = "Deploy of blink refused. ".to_string() + &"the divisor read zero ".repeat(20);
    h.app.sim.as_mut().unwrap().log.error(1.0, long.clone());
    h.wait(0.5);
    let found = text_at(&h, "the divisor read zero");
    assert_eq!(found.len(), 1, "the message is drawn once");
    let (clip, pos, galley) = &found[0];
    assert!(galley.rows.len() > 1, "wrapped to the panel");
    assert!(
        pos.x + galley.size().x <= clip.max.x + 0.5,
        "{} pt wide from {}, the panel ends at {}",
        galley.size().x,
        pos.x,
        clip.max.x
    );
}

/// The tour's card was 380 pt wide whatever the window: under about 396
/// pt its buttons were off the screen.
#[test]
fn the_tour_card_fits_a_narrow_window() {
    for width in [320.0, 360.0, 390.0] {
        let mut h = Harness::new(App::sim());
        h.size = Vec2::new(width, 780.0);
        h.app.tour = Some(crate::tour::Tour::new());
        h.wait(0.5);
        let card = h.rect("tour card");
        assert!(card.min.x >= 0.0 && card.max.x <= width, "{card:?} in {width} pt");
        for key in ["tour start", "tour skip"] {
            let b = h.rect(key);
            assert!(b.max.x <= width && b.min.x >= 0.0, "{key} {b:?} in {width} pt");
        }
    }
}

/// The picker read a whole file before any size check, and a failure
/// vanished: now an oversize file is refused by its size, and a failure
/// is an error in Messages.
#[test]
fn a_file_the_picker_cannot_take_is_told_in_messages() {
    use crate::app::{MAX_PICK, pick_refusal};
    assert_eq!(pick_refusal("a.zip", 1000.0), None);
    assert_eq!(pick_refusal("a.zip", MAX_PICK), None, "the limit itself is taken");
    let why = pick_refusal("big.zip", MAX_PICK + 1.0).expect("refused");
    assert!(why.contains("big.zip") && why.contains("MB"), "{why}");

    let mut h = Harness::new(App::sim());
    h.messages();
    let before = crate::editor::Body::messages(h.app.sim.as_mut().unwrap()).errors();
    h.app.pick_failed(vec![why.clone()], 1.0);
    assert_eq!(
        crate::editor::Body::messages(h.app.sim.as_mut().unwrap()).errors(),
        before + 1
    );
    h.wait(0.3);
    assert!(h.drew("big.zip was not read"), "Messages lists it");
}

/// The Code header since 2026-11-23 (Nick): a magnifier that opens Find
/// and Replace, Deploy as a filled button at the right end (wider than
/// a glyph and, since 2026-11-24, no taller than its row: "too big,
/// green, and awkward"), and the compile status only when it fails, as
/// a red ✖ that goes to the line.
#[test]
fn the_code_header_has_find_a_filled_deploy_at_the_right_and_errors_only_when_there_are() {
    let mut h = Harness::code();
    h.wait(0.5);
    let pane = h.rect("code header rest");
    let (find, wrap, deploy) = (h.rect("code find"), h.rect("code wrap"), h.rect("code deploy"));
    assert!(!h.has("code errors"), "nothing while it compiles");
    assert!(
        (deploy.right() - pane.right()).abs() <= 1.0,
        "Deploy at the right end: {deploy:?} in {pane:?}"
    );
    assert!(deploy.left() > wrap.right() && find.right() <= wrap.left() + 1.0);
    assert!(
        deploy.width() > 1.5 * wrap.width() && (deploy.height() - wrap.height()).abs() < 0.5,
        "wider than a glyph, the row's height: {deploy:?} beside {wrap:?}"
    );
    assert!(
        deploy.left() >= pane.left() - 1.0,
        "inside the row: {deploy:?} in {pane:?}"
    );

    // The magnifier opens the find row, and shuts it.
    assert!(h.app.chrome.find.is_none());
    h.press("code find");
    h.frame(vec![]);
    assert!(h.app.chrome.find.is_some());
    h.press("code find");
    h.frame(vec![]);
    assert!(h.app.chrome.find.is_none());

    // Code that does not compile: the ✖ with the count, and a click goes
    // to the line of the first error.
    {
        let v = h.app.sim.as_mut().unwrap();
        v.proj.create("broken", None);
        v.proj.files.insert(
            "main.st".into(),
            "PROGRAM p\nVAR\n  x : BOOL;\nEND_VAR\nx := ;\nEND_PROGRAM\n".into(),
        );
        v.proj.save();
        v.open_project("broken");
    }
    h.wait(1.5);
    assert!(h.has("code errors"), "the ✖ shows");
    assert!(
        (h.rect("code header rest").right() - pane.right()).abs() < 1.0,
        "the pane keeps its width"
    );
    assert!(h.rect("code errors").right() <= h.rect("code deploy").left());
    h.app.chrome.line = None;
    h.press("code errors");
    h.frame(vec![]);
    assert!(h.app.chrome.line.is_some(), "the click goes to the error's line");
}

/// Replace and Replace all in the find row: the file changes where the
/// matches are and nowhere else, and a read-only file offers no replace.
#[test]
fn replace_writes_one_match_then_all_of_them() {
    let mut h = Harness::code();
    h.wait(0.5);
    let cur = h.app.sim.as_ref().unwrap().proj.current.clone();
    let src = "PROGRAM p\nVAR\n  lamp : BOOL;\n  lamp_ok : BOOL;\nEND_VAR\nlamp := lamp_ok; (* lamp *)\nEND_PROGRAM\n";
    h.app
        .sim
        .as_mut()
        .unwrap()
        .proj
        .files
        .insert(cur.clone(), src.into());
    let mut find = crate::find::Find::default();
    find.query = "lamp".into();
    find.replace = Some("beacon".into());
    // Whole words are on for a name since 2026-11-25; this test pins the
    // substring replace (`lamp_ok` too), so it turns them off.
    find.whole = Some(false);
    h.app.chrome.find = Some(find);
    h.wait(0.3);
    let file = |h: &Harness| h.app.sim.as_ref().unwrap().proj.files[&cur].clone();
    let button = |h: &Harness, text: &str| {
        text_at(h, text)
            .into_iter()
            .find(|(_, _, g)| g.job.text == text)
            .map_or_else(
                || panic!("no {text:?} button"),
                |(_, pos, g)| pos + g.size() / 2.0,
            )
    };
    // The find field has its row and the replace field its own: the count
    // and the buttons start under each, never laid over it (Nick,
    // 2026-11-23: "the buttons are showing up on top of the find field").
    {
        let id = h.editor_id();
        let rect = |id: eframe::egui::Id| h.ctx.read_response(id).expect("drawn").rect;
        let (find, replace) = (rect(id.with("find")), rect(id.with("replace")));
        let arrow = text_at(&h, "▲")
            .first()
            .map(|(_, pos, _)| *pos)
            .expect("the previous-match button");
        let all = button(&h, "Replace all");
        assert!(
            arrow.y >= find.bottom() - 1.0,
            "▲ at {arrow:?} is under the find field {find:?}"
        );
        assert!(arrow.y < replace.top(), "and over the replace field {replace:?}");
        assert!(
            all.y >= replace.bottom() - 1.0,
            "Replace all at {all:?} is under the replace field {replace:?}"
        );
    }
    let at = button(&h, "Replace");
    h.click(at);
    h.wait(0.3);
    assert_eq!(
        file(&h),
        src.replacen("lamp", "beacon", 1),
        "the first match alone"
    );
    let at = button(&h, "Replace all");
    h.click(at);
    h.wait(0.3);
    assert_eq!(file(&h), src.replace("lamp", "beacon"));
}
