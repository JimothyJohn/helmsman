//! Replace in the Code tab's find row: after a Replace the current match
//! is the next one past the text just written, so repeated Replace walks
//! the file once and never replaces its own output (user report: `slew`
//! to `slews` made `slewss`).

use eframe::egui::{self, Event, Key, Modifiers, Pos2};

use super::Harness;
use crate::app::App;

/// A project `mine` whose main.st is `text`, in the Code tab, find open
/// on `query` with `with` in the replace row.
fn open(text: &str, query: &str, with: &str) -> Harness {
    let mut h = Harness::new(App::sim());
    h.app.chrome.show_code();
    {
        let v = h.app.sim.as_mut().unwrap();
        v.proj.create("mine", None);
        v.proj.files.insert("main.st".into(), text.into());
        v.proj.save_as("the text", None);
        v.open_project("mine");
    }
    h.wait(0.3);
    h.key(Modifiers::COMMAND, Key::F);
    h.frame(vec![]);
    h.frame(vec![Event::Text(query.into())]);
    h.frame(vec![]);
    // The real ⇄ button, found by what the frame drew.
    let at = text_at(&h, "⇄").expect("the replace toggle is drawn");
    h.click(at);
    h.wait(0.2);
    assert!(
        h.app.chrome.find.as_ref().unwrap().replace.is_some(),
        "the replace row opens"
    );
    let id = h.editor_id().with("replace");
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.frame(vec![]);
    h.frame(vec![Event::Text(with.into())]);
    h.frame(vec![]);
    assert_eq!(h.app.chrome.find.as_ref().unwrap().replace.as_deref(), Some(with));
    assert_eq!(text_of(&h), text);
    h
}

/// The centre of the first text the last frame drew that is exactly `s`.
fn text_at(h: &Harness, s: &str) -> Option<Pos2> {
    fn walk(shape: &egui::Shape, s: &str) -> Option<Pos2> {
        match shape {
            egui::Shape::Vec(v) => v.iter().find_map(|x| walk(x, s)),
            egui::Shape::Text(t) if t.galley.job.text == s => Some(t.pos + t.galley.rect.center().to_vec2()),
            _ => None,
        }
    }
    h.shapes.iter().find_map(|c| walk(&c.shape, s))
}

fn text_of(h: &Harness) -> String {
    let p = &h.app.sim.as_ref().unwrap().proj;
    p.files[&p.current].clone()
}

fn press_text(h: &mut Harness, s: &str) {
    let at = text_at(h, s).unwrap_or_else(|| panic!("no {s} drawn"));
    h.click(at);
    h.wait(0.1);
}

fn counter(h: &Harness) -> String {
    for n in 1..=12 {
        for m in 1..=12 {
            let s = format!("{n} of {m}");
            if h.drew(&s) {
                return s;
            }
        }
    }
    if h.drew("no match") {
        "no match".into()
    } else {
        String::new()
    }
}

/// Runs Replace until the query matches nothing (or `cap`), returning
/// each step's (text, counter).
fn walk(h: &mut Harness, cap: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for _ in 0..cap {
        press_text(h, "Replace");
        out.push((text_of(h), counter(h)));
    }
    out
}

#[test]
fn replace_slew_with_slews_three_times_never_makes_slewss() {
    let src = "slew := 1;\nx := slew + slew;\n";
    let mut h = open(src, "slew", "slews");
    assert_eq!(counter(&h), "1 of 3");
    let steps = walk(&mut h, 3);
    assert_eq!(steps[0].0, "slews := 1;\nx := slew + slew;\n");
    assert_eq!(steps[1].0, "slews := 1;\nx := slews + slew;\n");
    assert_eq!(steps[2].0, "slews := 1;\nx := slews + slews;\n");
    assert!(!steps[2].0.contains("slewss"));
    // Each match now is a slews, and the walk has come to its end.
    assert!(steps.iter().all(|(t, _)| !t.contains("slewss")), "{steps:?}");
}

#[test]
fn the_counter_stays_truthful_after_each_replace() {
    // With a replacement that still matches, the count stays 3 while the
    // current match walks 2, 3, then wraps.
    let src = "slew := 1;\nx := slew + slew;\n";
    let mut h = open(src, "slew", "slews");
    let steps = walk(&mut h, 3);
    assert_eq!(steps[0].1, "2 of 3");
    assert_eq!(steps[1].1, "3 of 3");
    // The last match was replaced: current wraps to the first remaining
    // match, which is the first slews.
    assert_eq!(steps[2].1, "1 of 3");
}

#[test]
fn a_replacement_that_does_not_hold_the_query_shrinks_the_count() {
    let src = "a a a";
    let mut h = open(src, "a", "b");
    assert_eq!(counter(&h), "1 of 3");
    let steps = walk(&mut h, 3);
    assert_eq!(steps[0], ("b a a".into(), "1 of 2".into()));
    assert_eq!(steps[1], ("b b a".into(), "1 of 1".into()));
    assert_eq!(steps[2].0, "b b b");
    assert_eq!(steps[2].1, "no match");
}

#[test]
fn a_replacement_starting_with_the_query_is_not_replaced_again() {
    let src = "slew slew slew";
    let mut h = open(src, "slew", "slewslew");
    walk(&mut h, 3);
    assert_eq!(text_of(&h), "slewslew slewslew slewslew");
}

#[test]
fn a_replacement_with_the_query_in_its_middle_is_not_replaced_again() {
    let src = "slew; slew; slew;";
    let mut h = open(src, "slew", "xslewx");
    walk(&mut h, 3);
    assert_eq!(text_of(&h), "xslewx; xslewx; xslewx;");
}

#[test]
fn a_replacement_ending_with_the_query_is_not_replaced_again() {
    let src = "a-a-a";
    let mut h = open(src, "a", "aa");
    walk(&mut h, 3);
    assert_eq!(text_of(&h), "aa-aa-aa");
}

#[test]
fn an_empty_replacement_takes_each_match_out() {
    let src = "ab ab ab";
    let mut h = open(src, "ab", "");
    let steps = walk(&mut h, 3);
    assert_eq!(steps[0].0, " ab ab");
    assert_eq!(steps[1].0, "  ab");
    assert_eq!(steps[2].0, "  ");
    assert_eq!(steps[2].1, "no match");
}

#[test]
fn replacing_the_last_match_wraps_to_the_first_remaining() {
    let src = "x y x y x";
    let mut h = open(src, "x", "z");
    // Walk to the last match with the next arrow, then replace it.
    press_text(&mut h, "▼");
    press_text(&mut h, "▼");
    assert_eq!(counter(&h), "3 of 3");
    press_text(&mut h, "Replace");
    assert_eq!(text_of(&h), "x y x y z");
    assert_eq!(counter(&h), "1 of 2");
    // The next Replace writes the first, not the one just written.
    press_text(&mut h, "Replace");
    assert_eq!(text_of(&h), "z y x y z");
    assert_eq!(counter(&h), "1 of 1");
}

#[test]
fn replace_all_writes_each_original_match_once() {
    let src = "slew := 1;\nx := slew + slew;\n";
    let mut h = open(src, "slew", "slews");
    press_text(&mut h, "Replace all");
    assert_eq!(text_of(&h), "slews := 1;\nx := slews + slews;\n");
    // The query still matches the output; the count says so truthfully.
    assert_eq!(counter(&h), "1 of 3");
}

#[test]
fn replace_all_with_a_replacement_holding_the_query_does_not_grow() {
    let src = "aa a";
    let mut h = open(src, "a", "aa");
    press_text(&mut h, "Replace all");
    assert_eq!(text_of(&h), "aaaa aa");
}

#[test]
fn replacing_text_with_multibyte_characters_walks_once() {
    let src = "é slew é slew";
    let mut h = open(src, "slew", "slewé");
    walk(&mut h, 2);
    assert_eq!(text_of(&h), "é slewé é slewé");
}

#[test]
fn replace_after_the_editor_had_the_keyboard_and_a_caret_inside_a_match() {
    let src = "slew := 1;\nx := slew + slew;\n";
    let mut h = Harness::new(App::sim());
    h.app.chrome.show_code();
    {
        let v = h.app.sim.as_mut().unwrap();
        v.proj.create("mine", None);
        v.proj.files.insert("main.st".into(), src.into());
        v.proj.save_as("the text", None);
        v.open_project("mine");
    }
    h.wait(0.3);
    // The user clicks inside the last `slew` first.
    let at = h.rect("st editor").left_top() + egui::Vec2::new(60.0, 6.0);
    h.click(at);
    h.key(Modifiers::COMMAND, Key::F);
    h.frame(vec![]);
    h.frame(vec![Event::Text("slew".into())]);
    h.frame(vec![]);
    press_text(&mut h, "⇄");
    let id = h.editor_id().with("replace");
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.frame(vec![]);
    h.frame(vec![Event::Text("slews".into())]);
    h.frame(vec![]);
    for _ in 0..3 {
        press_text(&mut h, "Replace");
    }
    assert_eq!(text_of(&h), "slews := 1;\nx := slews + slews;\n");
}
