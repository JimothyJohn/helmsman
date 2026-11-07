//! What the Agent's last change added and removed, marked where the
//! engineer looks (Nick, 2026-11-07): added blocks and wires outlined
//! green on the diagram, removed ones drawn as red ghosts where they
//! were; added lines of the Structured Text on a green ground, a red rule
//! where lines were removed (their text in its tooltip).
//!
//! The marks compare the file as it was before the Agent's answer with
//! the answer, and the diagram shown then with the diagram shown now (in
//! the simulation the answer runs at once; on a box the diagram is what
//! runs, so its marks show once the change is deployed). They last while
//! the file is exactly what the Agent wrote: the engineer's first edit
//! clears them. The diffs are `helmsman_diagram::diff`'s, the same the
//! proposal review draws.

use std::rc::Rc;

use helmsman_diagram::diff::{self, Line};
use helmsman_diagram::lower::Module;

/// The Agent's last change to one file.
#[derive(Debug, Clone)]
pub struct AgentChange {
    pub project: String,
    pub file: String,
    /// The file as the answer left it.
    pub after: String,
    /// The diagram's program when the answer landed, to diff against.
    pub baseline: Option<Rc<Module>>,
    lines: StMarks,
}

impl AgentChange {
    /// `before`: the file before the answer (empty when the answer made it).
    #[must_use]
    pub fn new(project: &str, file: &str, before: &str, after: String, baseline: Option<Rc<Module>>) -> Self {
        let lines = StMarks::of(before, &after);
        Self {
            project: project.to_string(),
            file: file.to_string(),
            after,
            baseline,
            lines,
        }
    }

    /// Whether the marks still hold: `project`'s `file` reads exactly what
    /// the Agent wrote. `text`: that file's text now, if it is open.
    #[must_use]
    pub fn holds(&self, project: &str, text: Option<&str>) -> bool {
        self.project == project && text == Some(self.after.as_str())
    }

    /// The Structured Text marks of the change.
    #[must_use]
    pub const fn lines(&self) -> &StMarks {
        &self.lines
    }
}

/// The Structured Text's marks: shown only while the editor's text is
/// `after`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StMarks {
    pub after: String,
    /// Lines of `after` the change added, 1-based.
    pub added: Vec<u32>,
    /// Where lines were removed: the 1-based line of `after` they stood
    /// before (one past the last line: at the end), and their text.
    pub removed: Vec<(u32, Vec<String>)>,
}

impl StMarks {
    /// The marks from `before` to `after`.
    #[must_use]
    pub fn of(before: &str, after: &str) -> Self {
        let mut added = Vec::new();
        let mut removed: Vec<(u32, Vec<String>)> = Vec::new();
        let mut gone: Vec<String> = Vec::new();
        let mut next_new: usize = 1;
        let flush = |gone: &mut Vec<String>, at: usize, removed: &mut Vec<(u32, Vec<String>)>| {
            if !gone.is_empty() {
                removed.push((u32::try_from(at).unwrap_or(u32::MAX), std::mem::take(gone)));
            }
        };
        for l in diff::lines(before, after) {
            match l {
                Line::Removed { text, .. } => gone.push(text),
                Line::Same { new, .. } => {
                    flush(&mut gone, new, &mut removed);
                    next_new = new + 1;
                }
                Line::Added { new, .. } => {
                    flush(&mut gone, new, &mut removed);
                    added.push(u32::try_from(new).unwrap_or(u32::MAX));
                    next_new = new + 1;
                }
            }
        }
        flush(&mut gone, next_new, &mut removed);
        Self {
            after: after.to_string(),
            added,
            removed,
        }
    }

    /// Whether these marks apply to `text`.
    #[must_use]
    pub fn applies(&self, text: &str) -> bool {
        !self.after.is_empty() && self.after == text && !(self.added.is_empty() && self.removed.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_and_removed_lines_are_placed_on_the_new_text() {
        let before = "a\nb\nc\nd\n";
        let after = "a\nB\nc\nnew\nd\n";
        let m = StMarks::of(before, after);
        assert_eq!(m.added, vec![2, 4], "{m:?}");
        assert_eq!(m.removed, vec![(2, vec!["b".to_string()])], "{m:?}");
        assert!(m.applies(after));
        assert!(
            !m.applies("a\nB\nc\nnew\nd\n edited"),
            "any edit and they no longer apply"
        );
    }

    #[test]
    fn lines_removed_at_the_end_stand_past_the_last_line() {
        let m = StMarks::of("a\nb\nc\n", "a\n");
        assert_eq!(m.added, Vec::<u32>::new());
        assert_eq!(m.removed, vec![(2, vec!["b".to_string(), "c".to_string()])]);
    }

    #[test]
    fn a_new_file_is_all_added_and_no_change_marks_nothing() {
        let m = StMarks::of("", "x\ny\n");
        assert_eq!(m.added, vec![1, 2]);
        assert_eq!(m.removed, []);
        let same = StMarks::of("x\n", "x\n");
        assert!(
            !same.applies("x\n"),
            "an answer that changed nothing marks nothing"
        );
    }

    #[test]
    fn the_change_holds_only_while_the_file_is_what_the_agent_wrote() {
        let c = AgentChange::new("p", "main.st", "a\n", "a\nb\n".into(), None);
        assert!(c.holds("p", Some("a\nb\n")));
        assert!(!c.holds("p", Some("a\nb\nc\n")), "edited");
        assert!(!c.holds("q", Some("a\nb\n")), "another project");
        assert!(!c.holds("p", None), "not open");
    }
}
