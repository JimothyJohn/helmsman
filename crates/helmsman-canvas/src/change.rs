//! What a described change would do, in numbers and names (Nick,
//! 2026-11-27, "Show Changes": "ensure the user knows EXACTLY what they're
//! doing before making changes and that the changes aren't supposed to be
//! huge"). Pure: the current and the proposed project in, a [`Summary`]
//! out. The Agent tab holds a green answer as a pending change and shows
//! this summary; the Deploy sheet shows the same for the program the box
//! runs against the project.
//!
//! - **Size**: the lines added and removed in the file, and the blocks
//!   added, changed and removed in the diagram ([`helmsman_diagram::diff`],
//!   the diff the proposals review draws).
//! - **Outputs**: every output whose logic changes: one that a changed,
//!   added or removed block writes, or that such a block feeds, through
//!   the wires and through the variables it writes and others read.
//! - **The cap**: one step is at most [`MAX_BLOCKS`] blocks and
//!   [`MAX_LINES`] lines, and may change only outputs the instruction
//!   names ([`named`]) or that sit in the block in focus.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use helmsman_diagram::Module;
use helmsman_diagram::diff::{self, Line};
use helmsman_diagram::model::{Diagram, NodeId, NodeKind, RegionId};
use helmsman_iec::{ImageSizes, PortDir};

use crate::review::Drawn;
use crate::scope::Files;

/// Most blocks (added, changed and removed) one step may touch.
pub const MAX_BLOCKS: usize = 8;
/// Most lines (added and removed) one step may touch.
pub const MAX_LINES: usize = 30;

/// What a change does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub lines_added: usize,
    pub lines_removed: usize,
    pub blocks_added: usize,
    pub blocks_changed: usize,
    pub blocks_removed: usize,
    /// The outputs whose logic changes, as the program names them.
    pub outputs: Vec<String>,
    /// Those of them the instruction (and the block in focus) do not name.
    pub unnamed: Vec<String>,
    /// Why the diagram could not be worked out, when it could not: what
    /// the change touches is then unknown and the change is refused.
    pub unknown: Option<String>,
    /// The file as it is does not compile: its blocks and outputs cannot
    /// be compared, so only the line cap holds.
    pub old_broken: bool,
}

impl Summary {
    /// Blocks added, changed and removed.
    #[must_use]
    pub const fn blocks(&self) -> usize {
        self.blocks_added + self.blocks_changed + self.blocks_removed
    }

    /// Lines added and removed.
    #[must_use]
    pub const fn lines(&self) -> usize {
        self.lines_added + self.lines_removed
    }

    /// Past the cap of one step.
    #[must_use]
    pub const fn over_cap(&self) -> bool {
        self.blocks() > MAX_BLOCKS || self.lines() > MAX_LINES
    }

    /// "3 blocks, 12 lines".
    #[must_use]
    pub fn size_line(&self) -> String {
        let (b, l) = (self.blocks(), self.lines());
        if self.old_broken {
            return format!("{l} line{}", if l == 1 { "" } else { "s" });
        }
        format!(
            "{b} block{}, {l} line{}",
            if b == 1 { "" } else { "s" },
            if l == 1 { "" } else { "s" }
        )
    }

    /// What cannot be said about this change, if anything.
    #[must_use]
    pub const fn note(&self) -> Option<&'static str> {
        if self.old_broken {
            Some(
                "The file as it is does not compile, so its blocks and outputs can't be compared: check the change line by line.",
            )
        } else {
            None
        }
    }

    /// "switches MOTOR, LAMP", or that no output changes.
    #[must_use]
    pub fn outputs_line(&self) -> String {
        if self.outputs.is_empty() {
            "No output changes.".into()
        } else {
            format!("Switches {}.", self.outputs.join(", "))
        }
    }

    /// Why this change cannot be applied, in words, if it cannot.
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        if let Some(why) = &self.unknown {
            return Some(format!(
                "What this changes cannot be worked out ({why}), so it is not applied. Ask for less."
            ));
        }
        if self.over_cap() {
            return Some(format!(
                "Too big for one step: {} (one step is at most {MAX_BLOCKS} blocks or {MAX_LINES} lines). Ask for part of it.",
                self.size_line()
            ));
        }
        if !self.unnamed.is_empty() {
            return Some(format!(
                "It changes {}, which the instruction does not name. Name it, or ask for less.",
                join_names(&self.unnamed)
            ));
        }
        None
    }
}

/// "A", "A and B", "A, B and C".
fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Lower case, `_` and runs of whitespace as one space.
fn flat(s: &str) -> String {
    s.to_lowercase()
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `name` appears in `text` as a whole name, in any case, with
/// `_` and a space alike.
#[must_use]
pub fn named(text: &str, name: &str) -> bool {
    let (text, name) = (flat(text), flat(name));
    if name.is_empty() {
        return false;
    }
    let bytes = text.as_bytes();
    let alnum = |b: u8| b.is_ascii_alphanumeric();
    text.match_indices(&name).any(|(at, _)| {
        let before = at == 0 || !alnum(bytes[at - 1]);
        let end = at + name.len();
        let after = end >= bytes.len() || !alnum(bytes[end]);
        before && after
    })
}

/// The variable a path is of: `m.on` and `a[1]` are `M` and `A`.
fn base(path: &str) -> String {
    path.split(['.', '['])
        .next()
        .unwrap_or(path)
        .trim()
        .to_ascii_uppercase()
}

/// A variable, as (POU, name), both upper-cased.
type Var = (String, String);

/// What a module does to each variable: for every variable a block
/// writes, calls or loops over, the structure of what feeds it (its
/// cone), and what the cones read.
#[derive(Default)]
struct Defs {
    sigs: BTreeMap<Var, Vec<String>>,
    reads: BTreeMap<Var, BTreeSet<Var>>,
    /// The function block types the cones call.
    calls: BTreeMap<Var, BTreeSet<String>>,
}

/// One block's cone: its structure, the variables and function blocks
/// under it.
type Cone = (String, BTreeSet<Var>, BTreeSet<String>);

fn cone_of(d: &Diagram, id: NodeId, memo: &mut HashMap<NodeId, Cone>, depth: usize) -> Cone {
    if let Some(c) = memo.get(&id) {
        return c.clone();
    }
    let Some(node) = d.nodes.get(id.0 as usize) else {
        return Cone::default();
    };
    if depth > 400 {
        return ("~".into(), BTreeSet::new(), BTreeSet::new());
    }
    let (mut reads, mut calls) = (BTreeSet::new(), BTreeSet::new());
    let tag = match &node.kind {
        NodeKind::Read { path } => {
            reads.insert((d.pou.clone(), base(path)));
            "read"
        }
        NodeKind::Access { path } => {
            reads.insert((d.pou.clone(), base(path)));
            "access"
        }
        NodeKind::Fb { instance, fb } => {
            reads.insert((d.pou.clone(), base(instance)));
            calls.insert(fb.to_ascii_uppercase());
            "fb"
        }
        NodeKind::Const { .. } => "const",
        NodeKind::Op { .. } => "op",
        NodeKind::Func { name } => {
            calls.insert(name.to_ascii_uppercase());
            "func"
        }
        NodeKind::Write { .. } => "write",
        NodeKind::For { .. } => "for",
        NodeKind::Jump => "jump",
        NodeKind::Fold { .. } => "fold",
    };
    let mut ins = Vec::new();
    for w in d.inputs(id) {
        let (s, r, c) = cone_of(d, w.from, memo, depth + 1);
        reads.extend(r);
        calls.extend(c);
        ins.push(format!("{}{}:{s}", if w.negated { "!" } else { "" }, w.out));
    }
    let sig = format!("{tag} {}({})", node.label, ins.join(","));
    let c = (sig, reads, calls);
    memo.insert(id, c.clone());
    c
}

/// The arms of every statement around `region`, with what gates each.
fn gates(d: &Diagram, region: Option<RegionId>, memo: &mut HashMap<NodeId, Cone>) -> Cone {
    let (mut sig, mut reads, mut calls) = (String::new(), BTreeSet::new(), BTreeSet::new());
    for r in d.chain(region) {
        let arms = d
            .stmts
            .iter()
            .find(|s| s.id == r.stmt)
            .map_or_else(|| vec![r.id], |s| s.arms.clone());
        let _ = write!(sig, "[{}", r.arm);
        for a in arms {
            let g = d.region(a);
            let _ = write!(sig, " {:?}/{}{}", g.kind, g.arm, if g.negated { "!" } else { "" });
            if let Some((n, out)) = g.gate {
                let (s, rs, cs) = cone_of(d, n, memo, 0);
                let _ = write!(sig, "{out}:{s}");
                reads.extend(rs);
                calls.extend(cs);
            }
        }
        sig.push(']');
    }
    (sig, reads, calls)
}

fn defs(m: &Module) -> Defs {
    let mut out = Defs::default();
    for p in &m.pous {
        let d = &p.diagram;
        let mut memo = HashMap::new();
        for n in &d.nodes {
            let name = match &n.kind {
                NodeKind::Write { path } => base(path),
                NodeKind::Fb { instance, .. } => base(instance),
                NodeKind::For { var } => base(var),
                _ => continue,
            };
            let key = (p.name.to_ascii_uppercase(), name);
            let (s, mut reads, mut calls) = cone_of(d, n.id, &mut memo, 0);
            let (g, gr, gc) = gates(d, n.region, &mut memo);
            reads.extend(gr);
            calls.extend(gc);
            out.sigs.entry(key.clone()).or_default().push(format!("{s}{g}"));
            out.reads.entry(key.clone()).or_default().extend(reads);
            out.calls.entry(key).or_default().extend(calls);
        }
    }
    for v in out.sigs.values_mut() {
        v.sort();
    }
    out
}

/// Every variable whose logic differs between `old` and `new`: one whose
/// cones differ, or that reads one that differs, or that calls a function
/// block or function whose body differs.
fn tainted(old: &Module, new: &Module) -> BTreeSet<String> {
    let (a, b) = (defs(old), defs(new));
    let keys: BTreeSet<&Var> = a.sigs.keys().chain(b.sigs.keys()).collect();
    let mut moved: BTreeSet<Var> = keys
        .iter()
        .filter(|k| a.sigs.get(**k) != b.sigs.get(**k))
        .map(|k| (*k).clone())
        .collect();
    loop {
        // A POU with a variable that moved moved as a whole.
        let pous: BTreeSet<&str> = moved.iter().map(|(p, _)| p.as_str()).collect();
        let hit = |k: &Var| {
            [&a, &b].iter().any(|d| {
                d.reads
                    .get(k)
                    .is_some_and(|rs| rs.iter().any(|r| moved.contains(r)))
                    || d.calls
                        .get(k)
                        .is_some_and(|cs| cs.iter().any(|c| pous.contains(c.as_str())))
            })
        };
        let more: Vec<Var> = keys
            .iter()
            .filter(|k| !moved.contains(**k) && hit(k))
            .map(|k| (*k).clone())
            .collect();
        if more.is_empty() {
            break;
        }
        moved.extend(more);
    }
    moved.into_iter().map(|(_, v)| v).collect()
}

/// The names of the program's outputs: its `VAR_OUTPUT`s and the
/// variables located at `%Q`.
fn outputs_of(src: &str, sizes: ImageSizes) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Ok(p) = helmsman_iec::compile(src, sizes) {
        out.extend(
            p.ports()
                .into_iter()
                .filter(|p| p.dir == PortDir::Output)
                .map(|p| p.name),
        );
    }
    if let Ok(unit) = helmsman_iec::ast::parse(src) {
        for pou in &unit.pous {
            for v in &pou.vars {
                if let Some((at, _)) = &v.at
                    && at.to_ascii_uppercase().starts_with("%Q")
                {
                    let n = v.name.to_ascii_uppercase();
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
        }
    }
    out
}

/// The program `files` build, as the Agent's checks and the diagram see it.
#[must_use]
pub fn build_src(files: &Files) -> String {
    crate::sim_projects::SimProjects::build_src_of(files)
}

/// What differs between two builds of a project: the diagram diff, drawn
/// (`None` when a side cannot be drawn, and why).
///
/// # Errors
/// The new build does not compile or lower.
pub fn draw(old_src: &str, new_src: &str, sizes: ImageSizes) -> Result<Drawn, String> {
    let new = crate::review::lower_source(new_src, sizes)?;
    // A side that does not build is a program with nothing in it.
    let old = crate::review::lower_source(old_src, sizes).ok();
    let d = diff::modules(old.as_ref().unwrap_or(&Module { pous: Vec::new() }), &new);
    Ok(Drawn { old, new, diff: d })
}

/// The summary of the change from the project `old` to `new`, whose
/// written file is `file`. `named_by` is what the engineer named: the
/// instruction and the text of the block in focus.
#[must_use]
pub fn summarize(
    old: &Files,
    new: &Files,
    file: &str,
    sizes: ImageSizes,
    named_by: &[&str],
) -> (Summary, Option<Drawn>) {
    let mut s = Summary::default();
    let lines = diff::lines(
        old.get(file).map_or("", String::as_str),
        new.get(file).map_or("", String::as_str),
    );
    s.lines_added = lines.iter().filter(|l| matches!(l, Line::Added { .. })).count();
    s.lines_removed = lines.iter().filter(|l| matches!(l, Line::Removed { .. })).count();
    let (old_src, new_src) = (build_src(old), build_src(new));
    let drawn = match draw(&old_src, &new_src, sizes) {
        Ok(d) => d,
        Err(why) => {
            s.unknown = Some(why);
            return (s, None);
        }
    };
    if !old_src.trim().is_empty() && crate::review::lower_source(&old_src, sizes).is_err() {
        s.old_broken = true;
        return (s, Some(drawn));
    }
    let (added, removed, changed) = drawn.diff.counts();
    (s.blocks_added, s.blocks_removed, s.blocks_changed) = (added, removed, changed);
    let moved = tainted(
        drawn.old.as_ref().unwrap_or(&Module { pous: Vec::new() }),
        &drawn.new,
    );
    let mut outs = outputs_of(&new_src, sizes);
    for o in outputs_of(&old_src, sizes) {
        if !outs.contains(&o) {
            outs.push(o);
        }
    }
    s.outputs = outs.into_iter().filter(|o| moved.contains(&base(o))).collect();
    s.unnamed = s
        .outputs
        .iter()
        .filter(|o| !named_by.iter().any(|t| named(t, o)))
        .cloned()
        .collect();
    (s, Some(drawn))
}

/// The project `old` as `apply` would leave it: its file and the scenarios
/// it passed as requirement tests.
#[must_use]
pub fn proposed(old: &Files, apply: &crate::instruct::Apply) -> Files {
    let mut new = old.clone();
    new.insert(apply.file.clone(), apply.text.clone());
    for (f, t) in &apply.tests {
        new.insert(f.clone(), t.clone());
    }
    new
}

/// What the pending change `apply` does to the project `old`, with what
/// the engineer named: its instruction and the blocks in focus at Send.
#[must_use]
pub fn of_apply(old: &Files, apply: &crate::instruct::Apply, focus: &str, sizes: ImageSizes) -> Summary {
    summarize(
        old,
        &proposed(old, apply),
        &apply.file,
        sizes,
        &[apply.instruction.as_str(), focus],
    )
    .0
}

/// A hash of `text`, to tell whether a file moved since a change was
/// asked for.
#[must_use]
pub fn hash(text: &str) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: ImageSizes = ImageSizes { i: 8, q: 8, m: 64 };

    fn files(text: &str) -> Files {
        std::iter::once(("main.st".to_string(), text.to_string())).collect()
    }

    const BASE: &str = "PROGRAM p\nVAR_INPUT start : BOOL; stop : BOOL; END_VAR\nVAR_OUTPUT motor : BOOL; lamp : BOOL; valve : BOOL; END_VAR\nVAR run : BOOL; END_VAR\nrun := start AND NOT stop;\nmotor := run;\nlamp := start;\nvalve := stop;\nEND_PROGRAM\n";

    fn change(new: &str, named_by: &[&str]) -> Summary {
        summarize(&files(BASE), &files(new), "main.st", SIZES, named_by).0
    }

    #[test]
    fn nothing_differs() {
        let s = change(BASE, &[]);
        assert!(s.blocks() == 0 && s.lines() == 0, "{s:?}");
        assert_eq!(s.outputs.len(), 0);
        assert_eq!(s.refusal(), None);
    }

    #[test]
    fn a_change_to_one_output_names_only_it() {
        let new = BASE.replace("lamp := start;", "lamp := start OR stop;");
        let s = change(&new, &["make the lamp come on with stop too"]);
        assert_eq!(s.outputs, vec!["LAMP".to_string()], "{s:?}");
        assert_eq!(s.unnamed.len(), 0);
        assert_eq!(s.refusal(), None);
        assert!(s.blocks() >= 1 && s.lines() == 2, "{s:?}");
    }

    #[test]
    fn a_change_upstream_moves_what_it_feeds_through_a_variable() {
        // `run` feeds MOTOR across a variable: both the write of RUN and
        // the read of it are in the chain.
        let new = BASE.replace("run := start AND NOT stop;", "run := start OR stop;");
        let s = change(&new, &["motor"]);
        assert_eq!(s.outputs, vec!["MOTOR".to_string()], "{s:?}");
    }

    #[test]
    fn an_output_the_instruction_does_not_name_is_refused() {
        let new = BASE.replace("valve := stop;", "valve := NOT stop;");
        let s = change(&new, &["make the lamp blink"]);
        assert_eq!(s.unnamed, vec!["VALVE".to_string()], "{s:?}");
        assert_eq!(
            s.refusal().as_deref(),
            Some("It changes VALVE, which the instruction does not name. Name it, or ask for less.")
        );
    }

    #[test]
    fn the_block_in_focus_names_its_output() {
        let new = BASE.replace("valve := stop;", "valve := NOT stop;");
        let s = change(&new, &["invert it", "valve := stop;"]);
        assert!(s.unnamed.is_empty(), "{s:?}");
    }

    #[test]
    fn a_name_is_matched_in_any_case_with_underscore_and_space_alike() {
        assert!(named("Turn on MOTOR_RUN when asked", "motor_run"));
        assert!(named("turn on motor run when asked", "MOTOR_RUN"));
        assert!(!named("the motors", "motor"));
        assert!(!named("", "motor"));
        assert!(!named("anything", ""));
    }

    #[test]
    fn past_the_cap_it_says_so_in_the_specs_words() {
        let mut new = String::from(BASE);
        let mut extra = String::new();
        for k in 0..12 {
            let _ = writeln!(extra, "lamp := start OR stop OR (run AND {k} > 1);");
        }
        new = new.replace("lamp := start;", &extra);
        let s = change(&new, &["lamp"]);
        assert!(s.over_cap(), "{s:?}");
        let why = s.refusal().expect("refused");
        assert!(why.starts_with("Too big for one step: "), "{why}");
        assert!(
            why.ends_with("(one step is at most 8 blocks or 30 lines). Ask for part of it."),
            "{why}"
        );
    }

    #[test]
    fn the_caps_are_exactly_eight_blocks_and_thirty_lines() {
        let s = Summary {
            blocks_added: MAX_BLOCKS,
            lines_added: MAX_LINES,
            ..Summary::default()
        };
        assert!(!s.over_cap());
        let s = Summary {
            blocks_added: MAX_BLOCKS + 1,
            ..Summary::default()
        };
        assert!(s.over_cap());
        let s = Summary {
            lines_removed: MAX_LINES + 1,
            ..Summary::default()
        };
        assert!(s.over_cap());
    }

    #[test]
    fn a_removed_block_moves_its_output() {
        let new = BASE.replace("valve := stop;\n", "");
        let s = change(&new, &[]);
        assert_eq!(s.outputs, vec!["VALVE".to_string()], "{s:?}");
        assert!(s.blocks_removed >= 1, "{s:?}");
    }

    #[test]
    fn a_program_that_does_not_build_is_refused_as_unknown() {
        let s = change("PROGRAM p\nthis is not st\n", &["motor"]);
        assert!(s.unknown.is_some());
        assert!(s.refusal().is_some());
    }

    #[test]
    fn a_changed_condition_moves_what_it_gates() {
        let src = "PROGRAM p\nVAR_INPUT a : BOOL; b : BOOL; END_VAR\nVAR_OUTPUT x : BOOL; y : BOOL; END_VAR\nIF a THEN\n x := TRUE;\nELSE\n x := FALSE;\nEND_IF;\ny := b;\nEND_PROGRAM\n";
        let new = src.replace("IF a THEN", "IF a AND b THEN");
        let s = summarize(&files(src), &files(&new), "main.st", SIZES, &[]).0;
        assert_eq!(s.outputs, vec!["X".to_string()], "{s:?}");
    }

    #[test]
    fn a_file_that_does_not_compile_is_held_to_the_line_cap_only() {
        let broken = "PROGRAM p\nthis is not st\n";
        let s = summarize(&files(broken), &files(BASE), "main.st", SIZES, &[]).0;
        assert!(s.old_broken && s.unknown.is_none(), "{s:?}");
        assert_eq!(s.blocks(), 0);
        assert_eq!(s.outputs.len(), 0);
        assert!(s.size_line().ends_with("lines") && !s.size_line().contains("block"));
        assert!(s.note().is_some());
        assert_eq!(s.refusal(), None, "{s:?}");
        let big = BASE.repeat(4);
        let s = summarize(&files(broken), &files(&big), "main.st", SIZES, &[]).0;
        assert!(
            s.unknown.is_some(),
            "a proposed file that does not lower is still refused"
        );
    }

    #[test]
    fn a_hash_tells_a_moved_file() {
        assert_eq!(hash("a"), hash("a"));
        assert_ne!(hash("a"), hash("b"));
    }

    #[test]
    fn the_size_line_counts_in_the_singular() {
        let s = Summary {
            blocks_changed: 1,
            lines_added: 1,
            ..Summary::default()
        };
        assert_eq!(s.size_line(), "1 block, 1 line");
        let s = Summary {
            blocks_changed: 3,
            lines_added: 12,
            ..Summary::default()
        };
        assert_eq!(s.size_line(), "3 blocks, 12 lines");
    }
}
