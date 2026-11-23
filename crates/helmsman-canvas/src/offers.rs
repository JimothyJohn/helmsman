//! What Describe offers when an instruction cannot be carried out as it
//! was written (Nick, 2026-11-23: a paragraph that starts "I can't do
//! this one" is no answer in a tool that is edited by describing). The
//! answer without code may end in `Offer:` lines, the nearest things that
//! can be done, and each becomes a button under one short reason:
//!
//! - `Offer: rename <old> <new>`: the page renames the name everywhere in
//!   the project (`device_form::rename_plan`: io.map, every `.st`, the
//!   requirement tests). The answer itself may change one file only, so a
//!   rename is always the page's to do.
//! - `Offer: instead <instruction>`: another instruction, sent as the
//!   engineer's own with one click.
//!
//! Nothing is done until a button is clicked, and what a rename would
//! touch is under its button. An offer the page cannot carry out (a name
//! the project does not hold, a name that is taken) is not shown.

use eframe::egui;

use crate::device_form::{RenamePlan, name_ok, rename_plan};
use crate::scope::Files;
use crate::tips::Tip;

/// The most offers one answer shows.
pub const MAX_OFFERS: usize = 2;
/// The longest instruction an offer carries, in characters.
pub const MAX_INSTEAD: usize = 300;

const KEY: &str = "offer:";

/// What an offer does when it is clicked.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum What {
    /// Rename `old` to `new` in every file of the project.
    Rename { old: String, new: String },
    /// Send `text` as the next instruction.
    Instead { text: String },
}

/// One offer as the tab shows it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Offer {
    pub what: What,
    /// What a rename touches, one entry a file, as the files stood when
    /// the answer came in; the rename itself is planned again when it is
    /// clicked.
    pub touches: Vec<String>,
}

impl Offer {
    /// The words on its button.
    #[must_use]
    pub fn label(&self) -> String {
        match &self.what {
            What::Rename { old, new } => format!("Rename {old} to {new}"),
            What::Instead { text } => text.clone(),
        }
    }
}

/// An answer's prose without its `Offer:` lines, and the offers those
/// lines spell, in order: at most [`MAX_OFFERS`], none twice. A line that
/// starts as an offer and is none the page knows is dropped with the
/// rest: it is not the engineer's to read.
#[must_use]
pub fn split(prose: &str) -> (String, Vec<What>) {
    let mut text = Vec::new();
    let mut offers: Vec<What> = Vec::new();
    for line in prose.lines() {
        let Some(rest) = after_key(line) else {
            text.push(line);
            continue;
        };
        if let Some(w) = what(rest)
            && offers.len() < MAX_OFFERS
            && !offers.contains(&w)
        {
            offers.push(w);
        }
    }
    (text.join("\n"), offers)
}

/// The rest of `line` when it is an offer line, list marks and emphasis
/// in front of it aside.
fn after_key(line: &str) -> Option<&str> {
    let l = line.trim().trim_start_matches(['*', '_', '#', '-', '>', ' ']);
    let head = l.get(..KEY.len())?;
    head.eq_ignore_ascii_case(KEY).then(|| &l[KEY.len()..])
}

fn what(rest: &str) -> Option<What> {
    let flat: String = rest
        .chars()
        .filter(|c| !c.is_control() || c.is_whitespace())
        .collect::<String>()
        .replace("**", "")
        .replace('`', "");
    let mut words = flat.split_whitespace();
    let verb = words.next()?.to_ascii_lowercase();
    match verb.trim_end_matches(':') {
        "rename" => {
            // `a b`, `a to b` and `a -> b` are all read.
            let names: Vec<&str> = words
                .filter(|w| !w.eq_ignore_ascii_case("to") && *w != "->" && *w != "\u{2192}")
                .map(|w| w.trim_matches(['"', '\'', ',', '.', ';']))
                .collect();
            match names.as_slice() {
                [old, new] if name_ok(old) && name_ok(new) && old != new => Some(What::Rename {
                    old: (*old).to_string(),
                    new: (*new).to_string(),
                }),
                _ => None,
            }
        }
        "instead" => {
            let text = words.collect::<Vec<_>>().join(" ");
            let text = text
                .trim_matches(['"', '\u{201c}', '\u{201d}', ' '])
                .trim_start_matches(':')
                .trim();
            if text.is_empty() {
                return None;
            }
            Some(What::Instead {
                text: text.chars().take(MAX_INSTEAD).collect(),
            })
        }
        _ => None,
    }
}

/// The rename of `old` to `new` across `files` as they stand.
///
/// # Errors
/// What `rename_plan` refuses: a name that is taken or nowhere, a file
/// that does not read.
pub fn plan(files: &Files, old: &str, new: &str) -> Result<RenamePlan, String> {
    let files: Vec<(String, String)> = files
        .iter()
        .filter(|(n, _)| !helmsman_project::agent::is_agent_file(n))
        .map(|(n, t)| (n.clone(), t.clone()))
        .collect();
    rename_plan(&files, old, new)
}

/// The offers of `whats` the page can carry out on `files`, each with
/// what it touches.
#[must_use]
pub fn kept(whats: Vec<What>, files: &Files) -> Vec<Offer> {
    whats
        .into_iter()
        .filter_map(|what| {
            let touches = match &what {
                What::Rename { old, new } => plan(files, old, new).ok()?.lines(),
                What::Instead { .. } => Vec::new(),
            };
            Some(Offer { what, touches })
        })
        .collect()
}

/// What the engineer did with the offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Carry this one out.
    Take(usize),
    /// None of them: they go, the reason stays.
    Leave,
}

/// The offers, a button each and "Leave it" under them. Every row wraps
/// inside the tab's width (a row that does not would widen the panel
/// every frame).
pub fn ui(ui: &mut egui::Ui, chrome: &mut crate::editor::Chrome, offers: &[Offer]) -> Option<Pick> {
    use egui::RichText;

    let t = crate::theme::theme(ui);
    let mut pick = None;
    ui.add_space(6.0);
    for (k, o) in offers.iter().enumerate() {
        let tip = match &o.what {
            What::Rename { .. } => {
                "Renames it in every file listed under the button, the wiring and the tests with the program. Nothing else changes."
            }
            What::Instead { .. } => {
                "Sends this as your instruction. It is checked like any other before anything is written."
            }
        };
        let r = ui
            .add(egui::Button::new(RichText::new(o.label()).strong()).wrap())
            .tip(tip);
        chrome.hit(format!("agent offer {k}"), r.rect);
        if r.clicked() {
            pick = Some(Pick::Take(k));
        }
        if !o.touches.is_empty() {
            let r = ui
                .add(egui::Label::new(RichText::new(o.touches.join(", ")).color(t.muted)).wrap())
                .tip("Every file the rename changes.");
            chrome.hit(format!("agent offer touches {k}"), r.rect);
        }
        ui.add_space(4.0);
    }
    let r = ui
        .add(egui::Button::new("Leave it").wrap())
        .tip("Changes nothing and takes these buttons away.");
    chrome.hit("agent offer leave", r.rect);
    if r.clicked() {
        pick = Some(Pick::Leave);
    }
    pick
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIGHT: &str = "PROGRAM light\nVAR_INPUT\n  run : BOOL;\nEND_VAR\nVAR_OUTPUT\n  red_n : BOOL;\n  yellow_n : BOOL;\nEND_VAR\n(* yellow_n is the amber stage *)\nyellow_n := run AND NOT red_n;\nEND_PROGRAM\n";
    const TEST: &str = "PROGRAM test_amber\nVAR_INPUT\n  yellow_n : BOOL;\nEND_VAR\nVAR_OUTPUT\n  run : BOOL;\n  done : BOOL;\nEND_VAR\nVAR\n  on : ASSERT;\nEND_VAR\nrun := TRUE;\non(COND := yellow_n OR NOT yellow_n);\ndone := TRUE;\nEND_PROGRAM\n";

    fn files() -> Files {
        [
            ("main.st".to_string(), LIGHT.to_string()),
            ("test_amber.st".to_string(), TEST.to_string()),
            (
                helmsman_project::IO_MAP.to_string(),
                "out yellow_n bool sim do:1\n".to_string(),
            ),
        ]
        .into()
    }

    fn rename(old: &str, new: &str) -> What {
        What::Rename {
            old: old.into(),
            new: new.into(),
        }
    }

    #[test]
    fn the_offer_lines_leave_the_reason_and_become_offers() {
        let (text, offers) = split(
            "The lamps are on/off outputs, so a lamp's colour comes from its name.\nOffer: rename yellow_n orange_n\nOffer: instead Add an output orange_n that is on while yellow_n is on\n",
        );
        assert_eq!(
            text,
            "The lamps are on/off outputs, so a lamp's colour comes from its name."
        );
        assert_eq!(
            offers,
            [
                rename("yellow_n", "orange_n"),
                What::Instead {
                    text: "Add an output orange_n that is on while yellow_n is on".into()
                }
            ]
        );
    }

    #[test]
    fn a_rename_is_read_however_the_two_names_are_joined() {
        for line in [
            "Offer: rename yellow_n orange_n",
            "offer: Rename `yellow_n` to `orange_n`",
            "- **Offer:** rename yellow_n -> orange_n.",
            "OFFER: rename: \"yellow_n\" \u{2192} \"orange_n\"",
        ] {
            assert_eq!(split(line).1, [rename("yellow_n", "orange_n")], "{line}");
            assert_eq!(split(line).0, "", "{line}");
        }
    }

    #[test]
    fn what_is_no_offer_the_page_knows_is_dropped_and_never_shown() {
        for line in [
            "Offer: rename yellow_n",
            "Offer: rename yellow_n orange_n extra_n",
            "Offer: rename yellow_n yellow_n",
            "Offer: rename yellow_n 9lives",
            "Offer: rename yellow_n or;DROP",
            "Offer: delete main.st",
            "Offer: instead",
            "Offer: instead \"\"",
            "Offer:",
        ] {
            assert_eq!(split(line), (String::new(), Vec::new()), "{line}");
        }
        // A sentence that only mentions an offer is prose.
        let prose = "I can offer: nothing here.";
        assert_eq!(split(prose), (prose.to_string(), Vec::new()));
    }

    #[test]
    fn at_most_two_offers_none_twice_and_an_instruction_is_cut_and_holds_no_control_character() {
        let long = "x".repeat(MAX_INSTEAD + 50);
        let (_, offers) = split(&format!(
            "Offer: rename a b\nOffer: rename a b\nOffer: instead do\u{7} this {long}\nOffer: rename c d\n"
        ));
        assert_eq!(offers.len(), MAX_OFFERS);
        assert_eq!(offers[0], rename("a", "b"));
        let What::Instead { text } = &offers[1] else {
            panic!("{offers:?}");
        };
        assert_eq!(text.chars().count(), MAX_INSTEAD);
        assert!(text.starts_with("do this x"), "{text}");
    }

    #[test]
    fn a_rename_is_kept_with_every_file_it_touches_and_dropped_when_it_cannot_be_done() {
        let f = files();
        let kept_ = kept(
            vec![
                rename("yellow_n", "orange_n"),
                rename("nowhere_n", "x"),
                rename("yellow_n", "red_n"),
            ],
            &f,
        );
        assert_eq!(kept_.len(), 1, "{kept_:?}");
        assert_eq!(kept_[0].label(), "Rename yellow_n to orange_n");
        assert_eq!(
            kept_[0].touches,
            [
                "io.map: the wiring line",
                "main.st: 2 use(s)",
                "test_amber.st: 3 use(s)"
            ]
        );
    }

    #[test]
    fn the_plan_renames_the_wiring_the_program_and_the_tests_and_no_comment() {
        let p = plan(&files(), "yellow_n", "orange_n").unwrap();
        let text = |f: &str| p.files.iter().find(|(n, ..)| n == f).map(|(_, t, _)| t.as_str());
        assert_eq!(
            text(helmsman_project::IO_MAP),
            Some("out orange_n bool sim do:1\n")
        );
        let main = text("main.st").unwrap();
        assert!(main.contains("orange_n := run AND NOT red_n;"), "{main}");
        assert!(main.contains("(* yellow_n is the amber stage *)"), "{main}");
        assert!(!text("test_amber.st").unwrap().contains("yellow_n"));
    }

    #[test]
    fn an_instruction_offered_is_kept_as_it_is() {
        let o = kept(
            vec![What::Instead {
                text: "Blink red_n".into(),
            }],
            &Files::new(),
        );
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].label(), "Blink red_n");
        assert!(o[0].touches.is_empty());
    }
}
