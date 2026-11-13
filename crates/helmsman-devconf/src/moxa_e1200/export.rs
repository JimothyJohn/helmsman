//! The console's configuration export (`ik12NN.txt`): INI-style text,
//! CRLF, `[Section]` lines and `KEY=value` lines. A value may carry a
//! label, `0,(Disabled)`. The text ends at `<END>`.
//!
//! The export shows the staged settings, not the running ones.

use std::collections::HashMap;

use super::Error;

/// Largest export read or sent (the bench unit's is 17 KiB).
pub const MAX_EXPORT: usize = 1 << 20;
/// Longest key.
const MAX_KEY: usize = 128;

/// One `KEY=value` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    /// Before the `,(label)` suffix.
    pub value: String,
    /// The text inside `,(...)`.
    pub label: Option<String>,
}

/// A `[Section]` and its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub entries: Vec<Entry>,
}

/// A parsed export. The lines are kept, trimmed of their CR, so a
/// [`Diff`] is of what the unit wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    lines: Vec<String>,
    sections: Vec<Section>,
}

impl Export {
    /// Reads export text. Lines before the first section go to a section
    /// named `""`; lines that are neither a section nor `KEY=value` are
    /// kept in [`Export::lines`] and otherwise ignored; reading stops at
    /// `<END>`.
    ///
    /// # Errors
    /// [`Error::Page`] over [`MAX_EXPORT`] bytes or with a NUL byte.
    pub fn parse(text: &str) -> Result<Self, Error> {
        if text.len() > MAX_EXPORT {
            return Err(Error::Page("the export is larger than is read"));
        }
        if text.contains('\0') {
            return Err(Error::Page("the export holds a NUL byte"));
        }
        let mut lines = Vec::new();
        let mut sections: Vec<Section> = Vec::new();
        for raw in text.split('\n') {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            lines.push(line.to_string());
            let t = line.trim();
            if t.eq_ignore_ascii_case("<END>") {
                break;
            }
            if let Some(name) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                sections.push(Section {
                    name: name.to_string(),
                    entries: Vec::new(),
                });
            } else if let Some((k, v)) = t.split_once('=') {
                let key = k.trim();
                if key.is_empty() || key.len() > MAX_KEY {
                    continue;
                }
                if sections.is_empty() {
                    sections.push(Section {
                        name: String::new(),
                        entries: Vec::new(),
                    });
                }
                let (value, label) = split_label(v.trim());
                if let Some(s) = sections.last_mut() {
                    s.entries.push(Entry {
                        key: key.to_string(),
                        value,
                        label,
                    });
                }
            }
        }
        // A trailing newline leaves one empty last "line"; it is not one.
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        Ok(Self { lines, sections })
    }

    /// Every line, CR removed.
    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    #[must_use]
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// The entry `key` in the section called `section`.
    #[must_use]
    pub fn get(&self, section: &str, key: &str) -> Option<&Entry> {
        self.sections
            .iter()
            .find(|s| s.name == section)?
            .entries
            .iter()
            .find(|e| e.key == key)
    }

    /// The first entry called `key` in any section.
    #[must_use]
    pub fn find(&self, key: &str) -> Option<&Entry> {
        self.sections
            .iter()
            .flat_map(|s| &s.entries)
            .find(|e| e.key == key)
    }

    /// The text again with CRLF line ends, the way the unit writes it.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut s = self.lines.join("\r\n");
        s.push_str("\r\n");
        s
    }
}

/// `0,(Disabled)` into `("0", Some("Disabled"))`.
fn split_label(v: &str) -> (String, Option<String>) {
    if let Some((body, k)) = v.strip_suffix(')').and_then(|b| Some((b, b.rfind(",(")?))) {
        return (body[..k].trim().to_string(), Some(body[k + 2..].to_string()));
    }
    (v.to_string(), None)
}

/// What differs between two exports.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diff {
    /// Same-length exports: line by line, `(before, after)`.
    pub changed: Vec<(String, String)>,
    /// Exports of different lengths: lines only in `before`, in order.
    pub removed: Vec<String>,
    /// Exports of different lengths: lines only in `after`, in order.
    pub added: Vec<String>,
}

impl Diff {
    #[must_use]
    pub fn between(before: &Export, after: &Export) -> Self {
        let (b, a) = (&before.lines, &after.lines);
        let mut d = Self::default();
        if b.len() == a.len() {
            for (x, y) in b.iter().zip(a) {
                if x.trim() != y.trim() {
                    d.changed.push((x.trim().to_string(), y.trim().to_string()));
                }
            }
            return d;
        }
        let mut left: HashMap<&str, usize> = HashMap::new();
        for l in a {
            *left.entry(l.trim()).or_default() += 1;
        }
        for l in b {
            match left.get_mut(l.trim()) {
                Some(n) if *n > 0 => *n -= 1,
                _ => d.removed.push(l.trim().to_string()),
            }
        }
        let mut left: HashMap<&str, usize> = HashMap::new();
        for l in b {
            *left.entry(l.trim()).or_default() += 1;
        }
        for l in a {
            match left.get_mut(l.trim()) {
                Some(n) if *n > 0 => *n -= 1,
                _ => d.added.push(l.trim().to_string()),
            }
        }
        d
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty() && self.added.is_empty()
    }

    /// Lines changed, removed and added.
    #[must_use]
    pub fn len(&self) -> usize {
        self.changed.len() + self.removed.len() + self.added.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const TEXT: &str = "[System Information]\r\nFIRMWARE=V3.1\r\n[4. Network Settings]\r\nCONNECTION_WATCHDOG=0,(Disabled)\r\nIP=192.0.2.5\r\nNOTE=a=b,(c)\r\nnoise line\r\n=novalue\r\n[9. Protocol Enable]\r\nEIP_ENABLE=1\r\n<END>\r\nAFTER=1\r\n";

    #[test]
    fn sections_keys_and_labels() {
        let e = Export::parse(TEXT).unwrap();
        let names: Vec<_> = e.sections().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            ["System Information", "4. Network Settings", "9. Protocol Enable"]
        );
        let w = e.get("4. Network Settings", "CONNECTION_WATCHDOG").unwrap();
        assert_eq!((w.value.as_str(), w.label.as_deref()), ("0", Some("Disabled")));
        let ip = e.find("IP").unwrap();
        assert_eq!((ip.value.as_str(), ip.label.as_ref()), ("192.0.2.5", None));
        // The first = splits; a later one stays in the value.
        let n = e.find("NOTE").unwrap();
        assert_eq!((n.value.as_str(), n.label.as_deref()), ("a=b", Some("c")));
        assert!(e.find("AFTER").is_none(), "nothing after <END>");
        assert!(e.find("").is_none());
        assert_eq!(e.get("9. Protocol Enable", "EIP_ENABLE").unwrap().value, "1");
    }

    #[test]
    fn text_round_trips_with_crlf() {
        let e = Export::parse("[A]\r\nK=1\r\n<END>\r\n").unwrap();
        assert_eq!(e.to_text(), "[A]\r\nK=1\r\n<END>\r\n");
        assert_eq!(Export::parse(&e.to_text()).unwrap(), e);
    }

    #[test]
    fn refuses_what_is_not_an_export() {
        assert!(Export::parse(&"a=b\n".repeat(MAX_EXPORT)).is_err());
        assert!(Export::parse("[A]\0\n").is_err());
    }

    #[test]
    fn diff_same_length_is_line_by_line() {
        let a = Export::parse("[A]\r\nK=1\r\nL=2\r\n").unwrap();
        let b = Export::parse("[A]\r\nK=1\r\nL=3\r\n").unwrap();
        let d = Diff::between(&a, &b);
        assert_eq!(d.changed, [("L=2".to_string(), "L=3".to_string())]);
        assert!(Diff::between(&a, &a).is_empty());
    }

    #[test]
    fn diff_different_length_lists_added_and_removed() {
        let a = Export::parse("[A]\r\nK=1\r\nL=2\r\n").unwrap();
        let b = Export::parse("[A]\r\nK=1\r\nM=9\r\nL=2\r\nL=2\r\n").unwrap();
        let d = Diff::between(&a, &b);
        assert_eq!(d.removed, Vec::<String>::new());
        assert_eq!(d.added, ["M=9", "L=2"]);
        assert_eq!(d.len(), 2);
    }

    proptest! {
        #[test]
        fn arbitrary_text_never_panics_and_stays_bounded(s in "\\PC{0,3000}") {
            if let Ok(e) = Export::parse(&s) {
                prop_assert!(e.lines().len() <= s.len() + 1);
                let entries: usize = e.sections().iter().map(|x| x.entries.len()).sum();
                prop_assert!(entries <= e.lines().len());
                prop_assert!(Diff::between(&e, &e).is_empty());
            }
        }

        #[test]
        fn line_soup_never_panics(s in "(\\[[a-z0-9. ]{0,8}\\]|[A-Z_]{0,6}=[a-z0-9,() ]{0,10}|<END>|[ -~]{0,10})(\r?\n(\\[[a-z0-9. ]{0,8}\\]|[A-Z_]{0,6}=[a-z0-9,() ]{0,10}|<END>|[ -~]{0,10})){0,60}") {
            let e = Export::parse(&s).unwrap();
            let again = Export::parse(&e.to_text()).unwrap();
            prop_assert_eq!(e.sections(), again.sections());
        }
    }
}
