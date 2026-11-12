//! Folding in the Code tab (Nick, 2026-11-11: an arrow in the gutter on
//! every block, as an IDE has): a PROGRAM or function block, a VAR
//! section, an IF, CASE or loop folds to its first and last lines.
//!
//! What folds is read off `helmsman_iec::scan::blocks`. A fold is kept by
//! the line its block starts on, for the file it was made in. It opens by
//! itself when something inside it has to be seen: a problem's line, the
//! line a block click went to, the caret moving in.

use helmsman_iec::scan::blocks;

/// A block that folds: the line it starts on and the line it ends on
/// (both stay), with at least one line between them to hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Foldable {
    pub first: u32,
    pub last: u32,
}

impl Foldable {
    /// How many lines folding it hides.
    #[must_use]
    pub const fn hides(self) -> u32 {
        self.last - self.first - 1
    }

    /// Whether folding it hides `line`.
    #[must_use]
    pub const fn covers(self, line: u32) -> bool {
        line > self.first && line < self.last
    }
}

/// The blocks of `text` that fold, in the order they start. Where two
/// start on one line (`IF a THEN FOR …`), the outer one is the line's.
#[must_use]
pub fn foldable(text: &str) -> Vec<Foldable> {
    let mut out: Vec<Foldable> = Vec::new();
    for b in blocks(text) {
        if b.last < b.first + 2 || out.iter().any(|f| f.first == b.first) {
            continue;
        }
        out.push(Foldable {
            first: b.first,
            last: b.last,
        });
    }
    out
}

/// The folds of the file in the Code tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Folds {
    /// The editor the folds belong to (one per file).
    file: Option<eframe::egui::Id>,
    /// The first lines of the folded blocks.
    starts: Vec<u32>,
    /// The caret's line when last looked at.
    caret: Option<u32>,
}

impl Folds {
    /// Folds the block that starts on `line`, or opens it.
    pub fn toggle(&mut self, line: u32) {
        match self.starts.iter().position(|s| *s == line) {
            Some(k) => {
                self.starts.remove(k);
            }
            None => self.starts.push(line),
        }
    }

    /// Whether the block that starts on `line` is folded.
    #[must_use]
    pub fn folded(&self, line: u32) -> bool {
        self.starts.contains(&line)
    }

    /// Opens every fold.
    pub fn open_all(&mut self) {
        self.starts.clear();
    }

    /// The blocks folded now in editor `file`, given what folds there
    /// (`blocks`), the lines that have to be seen (`shown`) and where the
    /// caret is. A fold made in another file is forgotten, as is one
    /// whose block is gone; a fold that covers a line of `shown` opens,
    /// and so does one the caret has just moved into (a caret already
    /// inside when the block was folded stays hidden with it).
    pub fn settle(
        &mut self,
        file: eframe::egui::Id,
        blocks: &[Foldable],
        shown: &[u32],
        caret: Option<u32>,
    ) -> Vec<Foldable> {
        if self.file != Some(file) {
            self.file = Some(file);
            self.starts.clear();
            self.caret = caret;
        }
        let moved = caret.filter(|c| self.caret != Some(*c));
        self.caret = caret;
        let mut folded = Vec::new();
        self.starts.retain(|s| {
            let Some(b) = blocks.iter().find(|b| b.first == *s) else {
                return false;
            };
            if shown.iter().chain(moved.iter()).any(|l| b.covers(*l)) {
                return false;
            }
            folded.push(*b);
            true
        });
        folded.sort_by_key(|b| b.first);
        folded
    }
}

/// Whether `line` is hidden by one of the folded blocks.
#[must_use]
pub fn hidden(folded: &[Foldable], line: u32) -> bool {
    folded.iter().any(|b| b.covers(line))
}

#[cfg(test)]
mod tests {
    use eframe::egui::Id;

    use super::{Foldable, Folds, foldable, hidden};

    const SRC: &str = "PROGRAM p
VAR
  x : INT;
  y : INT;
END_VAR
IF x > 0 THEN
  y := 1;
  CASE y OF
    1: x := 2;
  END_CASE;
END_IF;
IF y = 0 THEN x := 1; END_IF;
WHILE x > 0 DO
END_WHILE;
END_PROGRAM
";

    fn f(first: u32, last: u32) -> Foldable {
        Foldable { first, last }
    }

    #[test]
    fn a_block_with_lines_between_its_ends_folds() {
        // The one-line IF and the empty WHILE have nothing to hide.
        assert_eq!(foldable(SRC), [f(1, 15), f(2, 5), f(6, 11), f(8, 10)]);
        assert_eq!(f(6, 11).hides(), 4);
        assert!(f(6, 11).covers(7) && f(6, 11).covers(10));
        assert!(!f(6, 11).covers(6) && !f(6, 11).covers(11), "its ends stay");
        // Still being typed: nothing closes, nothing folds.
        assert_eq!(foldable("IF a THEN\nx := 1;\n"), []);
        // Two blocks starting on one line: the outer one is the line's.
        assert_eq!(
            foldable("IF a THEN FOR i := 1 TO 2 DO\nx := 1;\nEND_FOR;\ny := 2;\nEND_IF;"),
            [f(1, 5)]
        );
    }

    #[test]
    fn folds_hide_their_inner_lines_and_open_when_something_inside_must_show() {
        let blocks = foldable(SRC);
        let file = Id::unique("a");
        let mut folds = Folds::default();
        assert_eq!(folds.settle(file, &blocks, &[], None), []);
        folds.toggle(6);
        folds.toggle(2);
        let folded = folds.settle(file, &blocks, &[], Some(1));
        assert_eq!(folded, [f(2, 5), f(6, 11)], "in the order they start");
        assert!(folds.folded(6) && !folds.folded(8));
        for line in 1..=15 {
            let want = matches!(line, 3 | 4 | 7..=10);
            assert_eq!(hidden(&folded, line), want, "line {line}");
        }
        // A problem on line 9, inside the IF: the IF opens, the VAR stays.
        assert_eq!(folds.settle(file, &blocks, &[9], Some(1)), [f(2, 5)]);
        assert!(!folds.folded(6), "and it stays open once the problem is gone");
        assert_eq!(folds.settle(file, &blocks, &[], Some(1)), [f(2, 5)]);
        // The caret moving in opens it; one on a fold's own first or last
        // line does not.
        assert_eq!(folds.settle(file, &blocks, &[], Some(2)), [f(2, 5)]);
        assert_eq!(folds.settle(file, &blocks, &[], Some(5)), [f(2, 5)]);
        assert_eq!(folds.settle(file, &blocks, &[], Some(3)), []);
        // Folded with the caret already inside: it stays folded until the
        // caret moves.
        folds.toggle(2);
        assert_eq!(folds.settle(file, &blocks, &[], Some(3)), [f(2, 5)]);
        assert_eq!(folds.settle(file, &blocks, &[], Some(4)), []);
        // Toggling twice is as it was.
        folds.toggle(6);
        folds.toggle(6);
        assert!(!folds.folded(6));
    }

    #[test]
    fn a_fold_goes_with_its_block_and_its_file() {
        let blocks = foldable(SRC);
        let mut folds = Folds::default();
        folds.settle(Id::unique("a"), &blocks, &[], None);
        folds.toggle(6);
        folds.toggle(99);
        assert_eq!(folds.settle(Id::unique("a"), &blocks, &[], None), [f(6, 11)]);
        assert!(!folds.folded(99), "a line where no block starts holds no fold");
        // The text edited so line 6 starts nothing: the fold is gone.
        let edited = foldable(&SRC.replace("IF x > 0 THEN\n", "(* soon *)\n"));
        assert_eq!(folds.settle(Id::unique("a"), &edited, &[], None), []);
        // Another file: its own folds, none carried over.
        folds.toggle(2);
        assert_eq!(folds.settle(Id::unique("b"), &blocks, &[], None), []);
        folds.toggle(2);
        folds.open_all();
        assert_eq!(folds.settle(Id::unique("b"), &blocks, &[], None), []);
    }
}
