//! The style hints of the file in the Code tab (Nick, 2026-11-11: a
//! third, lighter level under errors and warnings): what
//! `helmsman_iec::hints` says of the text, worked out again only when the
//! text changes, and told to the Messages once the typing has settled.

use std::hash::{Hash, Hasher};

use helmsman_iec::hints::Hint;

/// How long the hints must stand unchanged before the Messages hear of
/// them, seconds: a variable declared a moment before it is used is not
/// news.
pub const SETTLE: f64 = 1.5;

/// The hints of the text last asked about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hints {
    key: Option<u64>,
    list: Vec<Hint>,
    /// When the list last changed, page time.
    changed_at: f64,
    /// The Messages have this list.
    told: bool,
}

impl Hints {
    /// The hints of `text`, at page time `now`.
    pub fn of(&mut self, text: &str, now: f64) -> &[Hint] {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        let key = Some(h.finish());
        if key != self.key {
            self.key = key;
            let list = helmsman_iec::hints::hints(text);
            if list != self.list {
                self.list = list;
                self.changed_at = now;
                self.told = false;
            }
        }
        &self.list
    }

    /// The line for the Messages, once: when the hints have stood for
    /// [`SETTLE`] and there are some.
    pub fn settled(&mut self, now: f64) -> Option<String> {
        if self.told || now - self.changed_at < SETTLE {
            return None;
        }
        self.told = true;
        if self.list.is_empty() {
            return None;
        }
        let shown: Vec<String> = self
            .list
            .iter()
            .take(5)
            .map(|h| format!("line {}  {}", h.pos.line, h.msg))
            .collect();
        let more = self.list.len() - shown.len();
        let tail = if more > 0 {
            format!(" · and {more} more")
        } else {
            String::new()
        };
        Some(format!(
            "{} hint(s) in the open file, which never stop a deploy: {}{tail}",
            self.list.len(),
            shown.join(" · ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{Hints, SETTLE};

    const SLIP: &str = "PROGRAM p\nVAR\n  spare : INT;\nEND_VAR\nEND_PROGRAM\n";

    #[test]
    fn the_messages_hear_of_hints_once_and_only_when_they_have_settled() {
        let mut h = Hints::default();
        assert_eq!(h.of(SLIP, 10.0).len(), 1);
        assert_eq!(h.settled(10.5), None, "still typing");
        // The same text asked about again is not a change.
        assert_eq!(h.of(SLIP, 11.0).len(), 1);
        let line = h.settled(10.0 + SETTLE).expect("settled");
        assert!(line.contains("line 3  spare: declared but never used"), "{line}");
        assert_eq!(h.settled(20.0), None, "said once");
        // Fixed: nothing to say, and nothing said.
        let fixed = SLIP.replace("END_VAR\n", "END_VAR\nspare := spare + 1;\n");
        assert!(h.of(&fixed, 21.0).is_empty());
        assert_eq!(h.settled(30.0), None);
        // Back again: said again.
        h.of(SLIP, 31.0);
        assert!(h.settled(40.0).is_some());
    }
}
