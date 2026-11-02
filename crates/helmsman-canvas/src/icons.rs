//! One-character icons on the diagram's blocks, so a block says what it
//! is at a glance when its name is small (Nick, 2026-11-01): built-in
//! ones for the standard blocks, and the project's `icons.map` over them.
//!
//! `icons.map` is one rule a line, `WHAT = ICON`. WHAT is a block type
//! (`TON`), a variable or instance name (`conveyor`), or a prefix with a
//! trailing `*` (`motor_*`); case does not matter. ICON is one character;
//! more is cut to the first. `#` starts a comment. For a block its type is
//! tried before its name; the first matching rule wins, and the project's
//! rules come before the built-in ones.

use helmsman_diagram::{Node, NodeKind};

/// The file's name in a project.
pub const FILE: &str = helmsman_project::ICONS;

/// The standard blocks' icons, by type.
const BUILT_IN: &[(&str, &str)] = &[
    ("TON", "T"),
    ("TOF", "T"),
    ("TP", "T"),
    ("CTU", "#"),
    ("CTD", "#"),
    ("CTUD", "#"),
    ("R_TRIG", "↑"),
    ("F_TRIG", "↓"),
    ("SR", "S"),
    ("RS", "R"),
    ("BLINK_PATTERN", "~"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    /// Upper-cased; a prefix when `prefix`.
    what: String,
    prefix: bool,
    icon: String,
}

/// A project's icon rules, parsed from its `icons.map`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Icons {
    /// The text the rules came from, so a redraw can tell a change.
    text: String,
    rules: Vec<Rule>,
}

impl Icons {
    /// The rules in `text`; a line that is not a rule is skipped.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let rules = text
            .lines()
            .filter_map(|line| {
                let line = line.split('#').next().unwrap_or("").trim();
                let (what, icon) = line.split_once('=')?;
                let what = what.trim();
                let icon: String = icon.trim().chars().take(1).collect();
                if what.is_empty() || icon.is_empty() {
                    return None;
                }
                let (what, prefix) = match what.strip_suffix('*') {
                    Some(p) => (p, true),
                    None => (what, false),
                };
                Some(Rule {
                    what: what.to_ascii_uppercase(),
                    prefix,
                    icon,
                })
            })
            .collect();
        Self {
            text: text.to_string(),
            rules,
        }
    }

    /// Re-reads the rules when `text` (the project's file, or none) is
    /// not what they came from; whether it was re-read.
    pub fn update(&mut self, text: Option<&str>) -> bool {
        let text = text.unwrap_or("");
        if text == self.text {
            return false;
        }
        *self = Self::parse(text);
        true
    }

    /// The icon for `n`, if any rule or built-in names its type or name.
    #[must_use]
    pub fn of(&self, n: &Node) -> Option<&str> {
        let (ty, name) = match &n.kind {
            NodeKind::Fb { instance, fb } => (Some(fb.as_str()), Some(instance.as_str())),
            NodeKind::Read { path } | NodeKind::Write { path } | NodeKind::Access { path } => {
                (None, path.split(['.', '[']).next())
            }
            NodeKind::Func { name } => (Some(name.as_str()), None),
            NodeKind::Const { .. } | NodeKind::Op { .. } | NodeKind::For { .. } | NodeKind::Jump => {
                (None, None)
            }
        };
        let keys = [ty, name].into_iter().flatten().map(str::to_ascii_uppercase);
        for key in keys {
            if let Some(r) = self.rules.iter().find(|r| {
                if r.prefix {
                    key.starts_with(&r.what)
                } else {
                    key == r.what
                }
            }) {
                return Some(&r.icon);
            }
        }
        let ty = ty?.to_ascii_uppercase();
        BUILT_IN.iter().find(|(t, _)| *t == ty).map(|(_, i)| *i)
    }
}

#[cfg(test)]
mod tests {
    use helmsman_diagram::{Node, NodeId, NodeKind, Rect};
    use helmsman_iec::Pos;
    use helmsman_iec::trace::StmtId;

    use super::Icons;

    fn node(kind: NodeKind) -> Node {
        Node {
            id: NodeId(0),
            kind,
            label: String::new(),
            text: String::new(),
            ins: Vec::new(),
            outs: Vec::new(),
            stmt: StmtId(0),
            pos: Pos { line: 1, col: 1 },
            region: None,
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
        }
    }

    fn fb(instance: &str, fb: &str) -> Node {
        node(NodeKind::Fb {
            instance: instance.into(),
            fb: fb.into(),
        })
    }

    #[test]
    fn the_standard_blocks_have_icons_and_the_rest_none() {
        let i = Icons::default();
        assert_eq!(i.of(&fb("PULSE", "TON")), Some("T"));
        assert_eq!(i.of(&fb("UP", "R_TRIG")), Some("↑"));
        assert_eq!(i.of(&fb("M", "MOTOR")), None, "a user block");
        assert_eq!(i.of(&node(NodeKind::Read { path: "START".into() })), None);
        assert_eq!(i.of(&node(NodeKind::Op { op: "AND".into() })), None);
    }

    #[test]
    fn the_projects_rules_name_types_names_and_prefixes_and_win() {
        let i = Icons::parse(
            "# icons for the line\nMOTOR = M\nconveyor* = >\nTON = ⏱  # over the built-in\nstart = !\nbad line\n= x\n",
        );
        assert_eq!(i.of(&fb("M1", "MOTOR")), Some("M"), "by type");
        assert_eq!(i.of(&fb("CONVEYOR_2", "MOTOR")), Some("M"), "type before name");
        assert_eq!(
            i.of(&node(NodeKind::Write {
                path: "CONVEYOR_2.RUN".into()
            })),
            Some(">"),
            "a prefix on the path's head"
        );
        assert_eq!(
            i.of(&fb("PULSE", "TON")),
            Some("⏱"),
            "the project's over the built-in"
        );
        assert_eq!(i.of(&node(NodeKind::Read { path: "START".into() })), Some("!"));
        assert_eq!(i.of(&node(NodeKind::Read { path: "STOP".into() })), None);
    }

    #[test]
    fn only_a_changed_file_is_read_again() {
        let mut i = Icons::default();
        assert!(!i.update(None), "no file, nothing to read");
        assert!(i.update(Some("TON = t")));
        assert!(!i.update(Some("TON = t")), "the same text");
        assert_eq!(i.of(&fb("P", "TON")), Some("t"));
        assert!(i.update(None), "the file went");
        assert_eq!(i.of(&fb("P", "TON")), Some("T"), "back to the built-in");
    }
}
