//! What each run of a Structured Text file is to the Code tab (Nick,
//! 2026-11-11: the code should read like an IDE's): a keyword, a type, a
//! literal, a comment, a variable the program is wired by, an address.
//! Read off [`helmsman_iec::scan`], so a file that does not compile is
//! coloured as far as it can be told.

use helmsman_iec::scan::{Kind, Piece, Word, scan, word};

use eframe::egui::Color32;

use crate::theme::Theme;

/// What a run of code is, for its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tint {
    /// Names, operators, spacing: the code's own ink.
    Plain,
    Keyword,
    /// An elementary type, or a block's type where a variable is declared
    /// (`pulse : TON`).
    Type,
    /// A number, a time, TRUE or FALSE.
    Literal,
    Comment,
    /// A variable the program is wired by: an input, an output, an
    /// in-out, or one placed at an address (`AT %IX0.0`).
    Io,
    /// A direct address: `%IX0.0`.
    Address,
}

impl Tint {
    /// Its colour on the page `t` (as ink: each stands on the light page
    /// as on the dark one).
    #[must_use]
    pub fn color(self, t: &Theme) -> Color32 {
        match self {
            Self::Plain => t.code,
            Self::Keyword => t.accent,
            Self::Type => t.ink(Color32::from_rgb(0x6E, 0xC6, 0xFF)),
            Self::Literal => t.ink(Color32::from_rgb(0x9B, 0xE5, 0x8C)),
            Self::Comment => t.muted,
            Self::Io => t.ink(Color32::from_rgb(0xD5, 0x9C, 0xFF)),
            Self::Address => t.ink(Color32::from_rgb(0xFF, 0x8A, 0x65)),
        }
    }
}

/// The names a file declares that get a colour of their own, upper-cased:
/// its wired variables and the block types its variables are of.
#[derive(Debug, Default, PartialEq, Eq)]
struct Declared {
    io: Vec<String>,
    types: Vec<String>,
}

/// Reads the VAR sections: each declaration is names, an optional `AT`
/// address, a colon and a type, up to its `;`.
fn declared(src: &str, pieces: &[Piece]) -> Declared {
    let mut out = Declared::default();
    // Inside a VAR section: whether every variable of it is wired.
    let mut section: Option<bool> = None;
    let mut names: Vec<String> = Vec::new();
    let mut placed = false;
    // Past the declaration's colon: the next word is its type.
    let mut typed = false;
    let mut after_colon = false;
    for p in pieces.iter().filter(|p| p.code()) {
        let text = p.text(src);
        let upper = text.to_ascii_uppercase();
        if p.kind == Kind::Word {
            if upper == "END_VAR" {
                section = None;
                continue;
            }
            if upper == "VAR" || upper.starts_with("VAR_") {
                section = Some(matches!(
                    upper.as_str(),
                    "VAR_INPUT" | "VAR_OUTPUT" | "VAR_IN_OUT"
                ));
                names.clear();
                (placed, typed, after_colon) = (false, false, false);
                continue;
            }
        }
        let Some(wired) = section else { continue };
        match (p.kind, text) {
            (Kind::Op, ";") => {
                if wired || placed {
                    out.io.append(&mut names);
                }
                names.clear();
                (placed, typed, after_colon) = (false, false, false);
            }
            (Kind::Op, ":") => after_colon = true,
            (Kind::Word, _) if upper == "AT" && !after_colon => placed = true,
            (Kind::Word, _) if after_colon && !typed => {
                typed = true;
                if word(&upper) == Word::Name {
                    out.types.push(upper);
                }
            }
            (Kind::Word, _) if !after_colon && word(&upper) == Word::Name => names.push(upper),
            _ => {}
        }
    }
    out.io.sort();
    out.io.dedup();
    out.types.sort();
    out.types.dedup();
    out
}

/// `src` as runs of one tint each, in order, tiling it: each run is where
/// it ends and its tint.
#[must_use]
pub fn tints(src: &str) -> Vec<(usize, Tint)> {
    let pieces = scan(src);
    let names = declared(src, &pieces);
    let mut out: Vec<(usize, Tint)> = Vec::new();
    // A name after a dot is a member (`m.run`), never the variable `run`.
    let mut member = false;
    for p in &pieces {
        let tint = match p.kind {
            Kind::Comment => Tint::Comment,
            Kind::Literal => Tint::Literal,
            Kind::Direct => Tint::Address,
            Kind::Word => {
                let upper = p.text(src).to_ascii_uppercase();
                match word(&upper) {
                    Word::Keyword => Tint::Keyword,
                    Word::Type => Tint::Type,
                    Word::Bool => Tint::Literal,
                    Word::Name if member => Tint::Plain,
                    Word::Name if names.types.binary_search(&upper).is_ok() => Tint::Type,
                    Word::Name if names.io.binary_search(&upper).is_ok() => Tint::Io,
                    Word::Name => Tint::Plain,
                }
            }
            Kind::Space | Kind::Newline | Kind::Op | Kind::Other => Tint::Plain,
        };
        if p.code() {
            member = p.kind == Kind::Op && p.text(src) == ".";
        }
        match out.last_mut() {
            Some(last) if last.1 == tint => last.0 = p.end,
            _ => out.push((p.end, tint)),
        }
    }
    out
}

/// The tint at byte `at` of the text `runs` was read from.
#[must_use]
pub fn tint_at(runs: &[(usize, Tint)], at: usize) -> (usize, Tint) {
    let k = runs.partition_point(|(end, _)| *end <= at);
    runs.get(k).copied().unwrap_or((usize::MAX, Tint::Plain))
}

#[cfg(test)]
mod tests {
    use super::{Tint, tint_at, tints};

    const SRC: &str = "PROGRAM cell
VAR_INPUT
  start, stop : BOOL;
END_VAR
VAR
  jam AT %IX0.2 : BOOL;
  pulse : TON;
  count : INT := 3; (* parts *)
END_VAR
pulse(IN := start AND NOT jam, PT := T#500ms);
IF pulse.Q THEN
  count := count + 1; // one more
END_IF;
m.start := TRUE;
END_PROGRAM
";

    fn of(src: &str, needle: &str, nth: usize) -> Tint {
        let at = src.match_indices(needle).nth(nth).expect(needle).0;
        tint_at(&tints(src), at).1
    }

    #[test]
    fn each_kind_of_code_has_its_tint() {
        assert_eq!(of(SRC, "PROGRAM", 0), Tint::Keyword);
        assert_eq!(of(SRC, "END_IF", 0), Tint::Keyword);
        assert_eq!(of(SRC, "AND", 0), Tint::Keyword);
        assert_eq!(of(SRC, "BOOL", 0), Tint::Type);
        assert_eq!(
            of(SRC, "TON", 0),
            Tint::Type,
            "a block's type where it is declared"
        );
        assert_eq!(of(SRC, "T#500ms", 0), Tint::Literal);
        assert_eq!(of(SRC, "TRUE", 0), Tint::Literal);
        assert_eq!(of(SRC, "3;", 0), Tint::Literal);
        assert_eq!(of(SRC, "(* parts *)", 0), Tint::Comment);
        assert_eq!(of(SRC, "// one more", 0), Tint::Comment);
        assert_eq!(of(SRC, "%IX0.2", 0), Tint::Address);
        // Inputs and a placed variable, where declared and where used.
        assert_eq!(of(SRC, "start", 0), Tint::Io);
        assert_eq!(of(SRC, "stop", 0), Tint::Io);
        assert_eq!(of(SRC, "jam", 0), Tint::Io);
        assert_eq!(of(SRC, "start", 1), Tint::Io, "in the body too");
        assert_eq!(of(SRC, "jam", 1), Tint::Io);
        // A plain variable, an instance, an operator.
        assert_eq!(of(SRC, "count", 0), Tint::Plain);
        assert_eq!(of(SRC, "pulse", 0), Tint::Plain);
        assert_eq!(of(SRC, ":=", 0), Tint::Plain);
        // A member named like an input is not that input.
        assert_eq!(of(SRC, "start", 2), Tint::Plain);
    }

    #[test]
    fn the_runs_tile_any_text() {
        for src in [
            "",
            "x",
            "(* open",
            "VAR x",
            "é := 'a';",
            SRC,
            "VAR_INPUT a : ; END_VAR b",
        ] {
            let runs = tints(src);
            let mut at = 0;
            for (end, _) in &runs {
                assert!(*end > at, "{src:?}");
                at = *end;
            }
            assert_eq!(at, src.len(), "{src:?}");
            assert_eq!(tint_at(&runs, src.len()).1, Tint::Plain, "past the end");
        }
    }

    /// The editor's layout carries the tints, in both themes, on a plain
    /// line, on the line a block click marked and beside an underlined
    /// error; the text itself is laid out unchanged.
    #[test]
    fn the_editor_lays_the_code_out_in_its_tints() {
        use crate::editor::{Lines, highlight};
        use crate::theme::Theme;
        use eframe::egui::FontId;
        for t in [Theme::dark(), Theme::light()] {
            let lines = Lines {
                errors: &[(12, 3)],
                warnings: &[],
                hints: &[],
            };
            let job = highlight(SRC, &lines, Some(11), &[], &FontId::monospace(17.0), 400.0, &t);
            assert_eq!(job.text, SRC);
            let color = |needle: &str, nth: usize| {
                let at = SRC.match_indices(needle).nth(nth).unwrap().0;
                let s = job
                    .sections
                    .iter()
                    .find(|s| s.byte_range.start.0 <= at && at < s.byte_range.end.0)
                    .unwrap();
                (s.format.color, s.format.italics)
            };
            assert_eq!(color("PROGRAM", 0), (Tint::Keyword.color(&t), false));
            assert_eq!(color("TON", 0), (Tint::Type.color(&t), false));
            assert_eq!(color("T#500ms", 0), (Tint::Literal.color(&t), false));
            assert_eq!(color("(* parts *)", 0), (Tint::Comment.color(&t), true));
            assert_eq!(color("start", 1), (Tint::Io.color(&t), false));
            // Line 11, marked: its keywords keep their colour on the ground.
            assert_eq!(color("THEN", 0).0, Tint::Keyword.color(&t));
            // Line 12, the error's line: the comment after the word still
            // reads as one.
            assert_eq!(color("// one more", 0), (Tint::Comment.color(&t), true));
            // Every tint is its own colour, none the page's ground.
            let all = [
                Tint::Plain,
                Tint::Keyword,
                Tint::Type,
                Tint::Literal,
                Tint::Comment,
                Tint::Io,
                Tint::Address,
            ];
            for (i, a) in all.iter().enumerate() {
                assert_ne!(a.color(&t), t.panel);
                for b in &all[i + 1..] {
                    assert_ne!(a.color(&t), b.color(&t), "{a:?} and {b:?}");
                }
            }
        }
    }

    /// A hinted word is underlined in the hint's colour, thinly and on no
    /// ground; an error or a warning on the same line is what shows.
    #[test]
    fn a_hint_underlines_its_word_unless_the_line_has_worse() {
        use crate::editor::{HINT, Lines, highlight};
        use crate::theme::Theme;
        use eframe::egui::{Color32, FontId};
        let t = Theme::dark();
        let font = FontId::monospace(17.0);
        // Line 8, column 3: `count`.
        let at = SRC.find("count").unwrap();
        let under = |lines: &Lines| {
            let job = highlight(SRC, lines, None, &[], &font, 400.0, &t);
            let s = job
                .sections
                .iter()
                .find(|s| s.byte_range.start.0 <= at && at < s.byte_range.end.0)
                .unwrap();
            (
                s.format.underline.width,
                s.format.underline.color,
                s.format.background,
            )
        };
        let hinted = under(&Lines {
            errors: &[],
            warnings: &[],
            hints: &[(8, 3)],
        });
        assert_eq!(hinted, (1.5, t.ink(HINT), Color32::TRANSPARENT));
        let warned = under(&Lines {
            errors: &[],
            warnings: &[(8, 3)],
            hints: &[(8, 3)],
        });
        assert_eq!(warned.1, crate::st_view::brand::WARN, "the warning shows");
        let plain = under(&Lines {
            errors: &[],
            warnings: &[],
            hints: &[],
        });
        assert_eq!(plain.0, 0.0, "no hint, no underline");
    }

    /// Case is the writer's: `var_input` and `Start` colour as theirs.
    #[test]
    fn case_does_not_matter() {
        let src = "var_input Go : bool; end_var\nx := go;";
        assert_eq!(of(src, "var_input", 0), Tint::Keyword);
        assert_eq!(of(src, "bool", 0), Tint::Type);
        assert_eq!(of(src, "go;", 0), Tint::Io);
    }
}
