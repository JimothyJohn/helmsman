//! Names and set values changed by hand on the diagram (Nick,
//! 2026-11-23: "I do want to be able to change set values, labels, and
//! names on the visual panel, but I don't want to manipulate the logic").
//! A double click on a block, or its right-click menu, asks for the new
//! name of the variable it reads, writes or calls, or the new value of
//! the literal it is, and the code follows: a name through
//! `device_form::rename_plan` in every file, a value as that literal's
//! characters replaced where they stand. Nothing else of the program can
//! be changed here. The host saves either as one revision with a History
//! entry, as a described change is.

use eframe::egui;
use helmsman_diagram::{Diagram, NodeId, NodeKind};

use crate::editor::Chrome;
use crate::scope::Files;
use crate::tips::Tip;

/// A literal on the diagram, as the source spells it and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Literal {
    pub text: String,
    pub file: String,
    /// Line of `file`, from 1.
    pub line: u32,
    /// Column in bytes, from 1.
    pub col: u32,
}

/// What a block lets the engineer change by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The variable it reads, writes, calls or counts with.
    Name(String),
    /// The literal it is.
    Value(Literal),
}

/// The variable a path starts with (`grid` of `grid[1].x`); none for a
/// direct address (`%IX0.0`).
fn root(path: &str) -> Option<String> {
    let end = path
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(path.len());
    let name = &path[..end];
    crate::device_form::name_ok(name).then(|| name.to_string())
}

/// `name` as the program spells it (the diagram holds names upper-cased):
/// its first whole-word use in an ST file, else as given.
pub fn spelled(files: &Files, name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    files
        .iter()
        .filter(|(f, _)| helmsman_project::file_name(f) == Some(helmsman_project::FileKind::St))
        .find_map(|(_, text)| {
            let b = text.as_bytes();
            (0..b.len().saturating_sub(name.len() - 1)).find_map(|i| {
                let end = i + name.len();
                (b[i..end].eq_ignore_ascii_case(name.as_bytes())
                    && (i == 0 || !word(b[i - 1]))
                    && b.get(end).is_none_or(|c| !word(*c)))
                .then(|| text[i..end].to_string())
            })
        })
        .unwrap_or_else(|| name.to_string())
}

/// The literal block `node` stands for: itself, or for an operator with
/// one literal among its inputs (`count > 10`, which the diagram draws as
/// one block with the 10 in it) that literal.
#[must_use]
pub fn literal_node(d: &Diagram, node: NodeId) -> Option<NodeId> {
    let kind = |n: NodeId| d.nodes.get(n.0 as usize).map(|n| &n.kind);
    match kind(node)? {
        NodeKind::Const { .. } => Some(node),
        NodeKind::Op { .. } => {
            let mut lits = d
                .wires
                .iter()
                .filter(|w| w.to == node && matches!(kind(w.from), Some(NodeKind::Const { .. })))
                .map(|w| w.from);
            let one = lits.next()?;
            lits.next().is_none().then_some(one)
        }
        _ => None,
    }
}

/// What block `node` of `d` offers: `locate` takes a line of the built
/// program to its file and line there, and the literal must stand in
/// that file exactly where the diagram says (a diagram older than the
/// files offers no value).
#[must_use]
pub fn target(
    d: &Diagram,
    node: NodeId,
    locate: &dyn Fn(u32) -> Option<(String, u32)>,
    files: &Files,
) -> Option<Target> {
    let n = d.nodes.get(node.0 as usize)?;
    let name = |path: &str| root(path).map(|r| Target::Name(spelled(files, &r)));
    match &n.kind {
        NodeKind::Read { path } | NodeKind::Write { path } | NodeKind::Access { path } => name(path),
        NodeKind::Fb { instance, .. } => name(instance),
        NodeKind::For { var } => name(var),
        NodeKind::Const { .. } | NodeKind::Op { .. } => {
            let n = d.nodes.get(literal_node(d, node)?.0 as usize)?;
            let (file, line) = locate(n.pos.line)?;
            let src = files.get(&file)?;
            let at = helmsman_iec::literal_at(src, line, n.pos.col)?;
            Some(Target::Value(Literal {
                text: src[at].to_string(),
                file,
                line,
                col: n.pos.col,
            }))
        }
        NodeKind::Func { .. } | NodeKind::Jump | NodeKind::Fold { .. } => None,
    }
}

/// `lit`'s file with the literal written `new`, checked: `new` is one
/// literal, the literal still stands where it stood, and a project that
/// built before still builds.
///
/// # Errors
/// In words for the engineer, when any of that does not hold.
pub fn set_value(files: &Files, lit: &Literal, new: &str) -> Result<String, String> {
    let new = new.trim();
    if !helmsman_iec::is_literal(new) {
        return Err(format!(
            "{new:?} is not a value: a number, TRUE or FALSE, a time such as T#5s."
        ));
    }
    if new == lit.text {
        return Err("That is its value already.".into());
    }
    let src = files
        .get(&lit.file)
        .ok_or_else(|| format!("{} is no longer in the project.", lit.file))?;
    let at = helmsman_iec::literal_at(src, lit.line, lit.col)
        .filter(|r| src[r.clone()] == lit.text)
        .ok_or_else(|| format!("{} changed since: {} is no longer there.", lit.file, lit.text))?;
    let mut out = String::with_capacity(src.len() + new.len());
    out.push_str(&src[..at.start]);
    out.push_str(new);
    out.push_str(&src[at.end..]);
    let builds = |text: &str| {
        let plan = helmsman_iec::spec::plan(
            files
                .iter()
                .filter(|(n, _)| helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St))
                .map(|(n, t)| (n.as_str(), if *n == lit.file { text } else { t.as_str() })),
        );
        helmsman_iec::compile(&plan.main, crate::playground::SIZES)
            .map(|_| ())
            .map_err(|d| d.first().map(ToString::to_string).unwrap_or_default())
    };
    if builds(src).is_ok()
        && let Err(why) = builds(&out)
    {
        return Err(format!("{new} does not fit there: {why}"));
    }
    Ok(out)
}

/// The History entry of a value set by hand.
#[must_use]
pub fn value_record(lit: &Literal, new: &str) -> String {
    format!(
        "Instruction: Set {} to {new} on line {}\nFile: {}\nResult: the value changed where it stands; nothing else did.\n",
        lit.text, lit.line, lit.file
    )
}

/// The question "what value?", open on a literal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueAsk {
    pub lit: Literal,
    pub text: String,
    /// The field takes the keyboard when the question opens.
    focused: bool,
}

impl ValueAsk {
    #[must_use]
    pub fn new(lit: Literal) -> Self {
        Self {
            text: lit.text.clone(),
            lit,
            focused: false,
        }
    }

    /// The question where the pointer was: the value, why it cannot be
    /// that, Set or Cancel. Returns the file's new text once Set is
    /// pressed (or Enter); `open` is false once the question is over.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        chrome: &mut Chrome,
        files: &Files,
        open: &mut bool,
    ) -> Option<String> {
        use egui::RichText;

        let checked = set_value(files, &self.lit, &self.text);
        let mut out = None;
        let mut done = false;
        let m = crate::theme::near_pointer(ctx, "value-set").show(ctx, |ui| {
            ui.set_max_width(360.0);
            let t = crate::theme::theme(ui);
            ui.add(
                egui::Label::new(
                    RichText::new(format!("Set {}", self.lit.text))
                        .strong()
                        .size(20.0),
                )
                .wrap(),
            );
            ui.add_space(8.0);
            let tip = "The new value, written into the code where this one stands. Nothing else changes.";
            let e = ui
                .add(egui::TextEdit::singleline(&mut self.text).desired_width(f32::INFINITY))
                .tip(tip);
            chrome.hit("value text", e.rect);
            if !self.focused {
                e.request_focus();
                self.focused = true;
            }
            let entered = e.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Err(why) = &checked
                && self.text.trim() != self.lit.text
            {
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(why).color(t.ink(crate::st_view::brand::RED))).wrap());
            }
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                let tall = egui::Vec2::new(0.0, crate::st_view::TAP * 0.7);
                let ok = ui
                    .add_enabled(
                        checked.is_ok(),
                        egui::Button::new(RichText::new("Set").strong()).min_size(tall),
                    )
                    .tip("Enter. Writes the value into the code and saves it as one revision.");
                chrome.hit("value ok", ok.rect);
                let no = ui
                    .add(egui::Button::new("Cancel").min_size(tall))
                    .tip("Esc. Leaves the value as it is.");
                chrome.hit("value cancel", no.rect);
                if (ok.clicked() || entered)
                    && let Ok(text) = &checked
                {
                    out = Some(text.clone());
                    done = true;
                } else if no.clicked() {
                    done = true;
                }
            });
        });
        chrome.hit("value form", m.response.rect);
        if done || crate::theme::escaped(&m) {
            *open = false;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "PROGRAM p\nVAR_INPUT\n  run : BOOL;\nEND_VAR\nVAR_OUTPUT\n  lamp : BOOL;\nEND_VAR\nVAR\n  t : TON;\n  n : INT;\nEND_VAR\nt(IN := run, PT := T#5s);\nIF n >= 10 THEN\n  n := 0;\nEND_IF;\nlamp := t.Q;\nEND_PROGRAM\n";

    fn files() -> Files {
        [("main.st".to_string(), SRC.to_string())].into()
    }

    fn diagram() -> Diagram {
        let s = crate::playground::Session::new(SRC);
        let m = s.build.as_ref().and_then(|b| b.module.clone()).expect("compiles");
        crate::st_view::Viewer::new(SRC, m).diagram().clone()
    }

    fn targets() -> Vec<Target> {
        let d = diagram();
        let f = files();
        (0..d.nodes.len())
            .filter_map(|k| {
                target(
                    &d,
                    NodeId(u32::try_from(k).unwrap()),
                    &|l| Some(("main.st".to_string(), l)),
                    &f,
                )
            })
            .collect()
    }

    fn value(text: &str) -> Literal {
        targets()
            .into_iter()
            .find_map(|t| match t {
                Target::Value(l) if l.text == text => Some(l),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no literal {text} among {:?}", targets()))
    }

    #[test]
    fn blocks_offer_the_variable_they_name_or_the_literal_they_are() {
        let names: Vec<String> = targets()
            .into_iter()
            .filter_map(|t| match t {
                Target::Name(n) => Some(n),
                Target::Value(_) => None,
            })
            .collect();
        for n in ["run", "t", "n", "lamp"] {
            assert!(names.iter().any(|x| x == n), "{n} in {names:?}");
        }
        assert_eq!(
            (value("T#5s").line, value("10").line, value("0").line),
            (12, 13, 14)
        );
    }

    #[test]
    fn a_value_is_written_where_it_stands_and_nothing_else_moves() {
        let out = set_value(&files(), &value("T#5s"), " T#750ms ").unwrap();
        assert_eq!(out, SRC.replace("T#5s", "T#750ms"));
        let out = set_value(&files(), &value("10"), "-3").unwrap();
        assert_eq!(out, SRC.replace(">= 10", ">= -3"));
    }

    #[test]
    fn what_is_no_value_or_does_not_fit_or_is_stale_changes_nothing() {
        let f = files();
        for (lit, new, says) in [
            (value("10"), "n + 1", "is not a value"),
            (value("10"), "10; lamp := TRUE", "is not a value"),
            (value("10"), "", "is not a value"),
            (value("10"), "10", "its value already"),
            (value("10"), "T#5s", "does not fit there"),
            (value("T#5s"), "TRUE", "does not fit there"),
        ] {
            let e = set_value(&f, &lit, new).unwrap_err();
            assert!(e.contains(says), "{new:?}: {e}");
        }
        // The file moved on: the literal is not where the diagram says.
        let lit = value("10");
        let moved: Files = [("main.st".to_string(), SRC.replace(">= 10", ">=  10"))].into();
        assert!(
            set_value(&moved, &lit, "12")
                .unwrap_err()
                .contains("changed since")
        );
        assert!(
            set_value(&Files::new(), &lit, "12")
                .unwrap_err()
                .contains("no longer in the project")
        );
    }

    #[test]
    fn a_diagram_older_than_the_file_offers_no_value() {
        let d = diagram();
        let stale: Files = [("main.st".to_string(), format!("(* new first line *)\n{SRC}"))].into();
        let any = (0..d.nodes.len()).any(|k| {
            matches!(
                target(
                    &d,
                    NodeId(u32::try_from(k).unwrap()),
                    &|l| Some(("main.st".to_string(), l)),
                    &stale
                ),
                Some(Target::Value(_))
            )
        });
        assert!(!any);
    }
}
