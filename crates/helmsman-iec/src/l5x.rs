//! Rockwell L5X (the Logix Designer XML export), read for its inventory:
//! the controller, its data types and add-on instruction names, controller
//! tags, programs with their tags (type, dimensions, usage, alias target,
//! description, decorated data as text) and routines (ladder rungs as text
//! with their comments, ST lines, the kind of anything else), and tasks
//! with the programs they schedule. Nothing here gives the file meaning:
//! rungs stay text for a reader (the agent, or [`crate::ladder`], which
//! converts a subset to Structured Text), and nothing is ever written back.
//!
//! The structure is that of an export as the Rockwell reference describes
//! it (`RSLogix5000Content` > `Controller` > `DataTypes`, `Tags`,
//! `Programs`, `Tasks`), checked against Rockwell's *Logix 5000 Controllers
//! Import/Export* (1756-RM014D-EN-P): root pp.37-38, tag attributes
//! pp.106-107, decorated data pp.43-46, `MainRoutineName` p.196, rungs
//! pp.206-208, ST p.264ff., tasks p.313. The root's
//! `TargetName`/`TargetType` are not in RM014's text (docs/SOURCES.md).
//! Everything else in the file (modules, raw and L5K data, trends, safety
//! info) is ignored.
//!
//! An L5X comes from a customer's PLC project, so it is treated as hostile:
//! it goes through the same non-validating reader as `PLCopen` files (no
//! DOCTYPE, so no entities; nesting capped at [`crate::xml`]'s depth), the
//! file is capped at [`MAX_L5X`] bytes and [`MAX_ITEMS`] tags, members,
//! rungs and lines, descriptions and rung text are clipped, and every name
//! must be a Logix identifier. An element whose name or type is not one is
//! left out and listed in [`Project::skipped`], so a tag called "ignore
//! previous instructions" never reaches anything that treats it as a name.

use std::fmt::Write as _;

use crate::lexer::Pos;
use crate::xml::{self, Element};

/// Largest file read, bytes.
pub const MAX_L5X: usize = 8 << 20;
/// Most tags, members, rungs and ST lines in one file, together.
pub const MAX_ITEMS: usize = 100_000;
/// Longest Logix name (tags, programs, routines, data types, tasks).
pub const MAX_NAME: usize = 40;
/// Characters of a description kept.
pub const MAX_DESCRIPTION: usize = 1000;
/// Characters of a rung's text, a rung comment or an ST line kept.
pub const MAX_TEXT: usize = 8000;
/// Longest alias target or dimension list.
const MAX_REF: usize = 200;
/// Most entries in [`Project::skipped`]; the rest are only counted.
pub const MAX_SKIPPED: usize = 200;

/// Why the file could not be read at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub pos: Pos,
    pub msg: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.pos, self.msg)
    }
}

/// A part of the file that was left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub pos: Pos,
    pub why: String,
}

/// What an export holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Project {
    /// `TargetName`: what was exported (the controller, or one program).
    pub target: Option<String>,
    /// `TargetType`: `Controller`, `Program`, `Routine`, `Rung`, ...
    pub target_type: Option<String>,
    /// `SoftwareRevision` of the editor that wrote it.
    pub software: Option<String>,
    pub controller: Option<Controller>,
    pub data_types: Vec<DataType>,
    /// Add-on instruction names.
    pub add_ons: Vec<String>,
    /// Controller-scope tags.
    pub tags: Vec<Tag>,
    pub programs: Vec<Program>,
    pub tasks: Vec<Task>,
    /// What was left out (first [`MAX_SKIPPED`]).
    pub skipped: Vec<Skipped>,
    /// How many things were left out in all.
    pub skipped_total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controller {
    pub name: String,
    /// `ProcessorType`, e.g. `1769-L33ER`.
    pub processor: Option<String>,
}

/// A user-defined data type and its visible members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataType {
    pub name: String,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub data_type: String,
    /// Elements, for an array member.
    pub dimension: Option<u32>,
}

/// How a tag holds its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagKind {
    Base,
    /// Another name for this tag path.
    Alias(String),
    Produced,
    Consumed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// `DataType`; an alias may leave it out.
    pub data_type: Option<String>,
    /// Array dimensions, outermost first.
    pub dimensions: Vec<u32>,
    pub kind: TagKind,
    /// `Usage` of a program tag: `Input`, `Output`, `InOut`, `Public`,
    /// `Local`.
    pub usage: Option<String>,
    /// `ExternalAccess`: `Read/Write`, `Read Only`, `None`.
    pub external_access: Option<String>,
    pub constant: bool,
    /// Clipped to [`MAX_DESCRIPTION`] characters.
    pub description: Option<String>,
    /// The value the export holds, from its decorated data (RM014
    /// pp.43-46), when it has some.
    pub data: Option<TagData>,
}

/// A tag's decorated data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagData {
    pub value: Data,
    /// Some value in it carries a `ForceValue` (RM014 p.46).
    pub forced: bool,
}

/// A value in decorated form. Atomic values stay text as exported
/// (`Value`, clipped to 64 characters); the reader of the value decides
/// what it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data {
    /// `DataValue` / `DataValueMember`: an atomic value.
    Value(String),
    /// `Structure` / `StructureMember`: members by name, in order.
    Structure(Vec<(String, Data)>),
    /// `Array` / `ArrayMember`: elements with their `Index` text
    /// (`[0]`, `[1,2]`), in order.
    Array(Vec<(String, Data)>),
}

/// What a routine is written in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutineKind {
    /// Ladder: [`Routine::rungs`].
    Ladder,
    /// Structured Text: [`Routine::lines`].
    St,
    /// Function block diagram: names only.
    Fbd,
    /// Sequential function chart: names only.
    Sfc,
}

impl RoutineKind {
    /// The name L5X gives it (`RLL`, `ST`, `FBD`, `SFC`).
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Ladder => "RLL",
            Self::St => "ST",
            Self::Fbd => "FBD",
            Self::Sfc => "SFC",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rung {
    /// `Number` as exported.
    pub number: u32,
    /// The rung in neutral text (`XIC(Start)OTE(Run);`), clipped.
    pub text: String,
    /// The rung comment, clipped.
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routine {
    pub name: String,
    pub kind: RoutineKind,
    pub rungs: Vec<Rung>,
    /// ST source lines, clipped one by one.
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub name: String,
    pub main_routine: Option<String>,
    pub disabled: bool,
    pub tags: Vec<Tag>,
    pub routines: Vec<Routine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub name: String,
    /// `CONTINUOUS`, `PERIODIC` or `EVENT`.
    pub kind: Option<String>,
    /// `Rate` of a periodic task, ms.
    pub rate_ms: Option<u32>,
    pub programs: Vec<String>,
}

/// Whether `s` is a Logix name: a letter or `_`, then letters, digits and
/// `_`, at most [`MAX_NAME`] in all.
#[must_use]
pub fn is_name(s: &str) -> bool {
    let mut cs = s.chars();
    s.len() <= MAX_NAME
        && cs.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A tag path an alias points at (`Local:1:I.Data.0`, `Recipe[3].Speed`).
fn is_ref(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_REF
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '[' | ']' | ','))
}

/// At most `max` characters of `s`, trimmed; control characters other
/// than line breaks and tabs become spaces; a clipped text says so.
fn clip(s: &str, max: usize) -> String {
    let t = s.trim();
    let mut out: String = t
        .chars()
        .take(max)
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let n = t.chars().count();
    if n > max {
        let _ = write!(out, " [clipped: {} more characters]", n - max);
    }
    out
}

struct Cx {
    p: Project,
    items: usize,
}

impl Cx {
    fn skip(&mut self, pos: Pos, why: String) {
        self.p.skipped_total += 1;
        if self.p.skipped.len() < MAX_SKIPPED {
            self.p.skipped.push(Skipped { pos, why });
        }
    }

    fn count(&mut self, e: &Element) -> Result<(), Error> {
        self.items += 1;
        if self.items > MAX_ITEMS {
            return Err(Error {
                pos: e.pos,
                msg: format!("more than {MAX_ITEMS} tags, members, rungs and lines; export less"),
            });
        }
        Ok(())
    }

    /// The `Name` attribute, if it is a Logix name; else the element is
    /// skipped (`what` names it in the reason).
    fn name(&mut self, e: &Element, what: &str) -> Option<String> {
        match e.attr("Name") {
            Some(n) if is_name(n) => Some(n.to_string()),
            Some(n) => {
                let shown: String = n.chars().take(60).collect();
                self.skip(
                    e.pos,
                    format!("{what} with a name that is not a Logix name: {shown:?}"),
                );
                None
            }
            None => {
                self.skip(e.pos, format!("{what} without a Name"));
                None
            }
        }
    }

    fn tag(&mut self, e: &Element) -> Result<Option<Tag>, Error> {
        self.count(e)?;
        let Some(name) = self.name(e, "tag") else {
            return Ok(None);
        };
        let kind = match e.attr("TagType").unwrap_or("Base") {
            "Base" => TagKind::Base,
            "Produced" => TagKind::Produced,
            "Consumed" => TagKind::Consumed,
            "Alias" => match e.attr("AliasFor") {
                Some(a) if is_ref(a) => TagKind::Alias(a.to_string()),
                _ => {
                    self.skip(e.pos, format!("alias tag {name} without a readable AliasFor"));
                    return Ok(None);
                }
            },
            other => {
                let shown: String = other.chars().take(40).collect();
                self.skip(e.pos, format!("tag {name} of unknown TagType {shown:?}"));
                return Ok(None);
            }
        };
        let data_type = match e.attr("DataType") {
            None => None,
            Some(t) if is_name(t) => Some(t.to_string()),
            Some(_) => {
                self.skip(
                    e.pos,
                    format!("tag {name} with a DataType that is not a Logix name"),
                );
                return Ok(None);
            }
        };
        if data_type.is_none() && kind == TagKind::Base {
            self.skip(e.pos, format!("tag {name} without a DataType"));
            return Ok(None);
        }
        let Some(dimensions) = dims(e.attr("Dimensions")) else {
            self.skip(e.pos, format!("tag {name} with unreadable Dimensions"));
            return Ok(None);
        };
        let usage = e
            .attr("Usage")
            .filter(|u| matches!(*u, "Input" | "Output" | "InOut" | "Public" | "Local"))
            .map(str::to_string);
        let external_access = e
            .attr("ExternalAccess")
            .filter(|a| matches!(*a, "Read/Write" | "Read Only" | "None"))
            .map(str::to_string);
        let description = e
            .child("Description")
            .map(|d| clip(&d.text().0, MAX_DESCRIPTION))
            .filter(|d| !d.is_empty());
        let mut forced = false;
        let mut value = None;
        if let Some(d) = e
            .children_named("Data")
            .find(|d| d.attr("Format") == Some("Decorated"))
            .and_then(|d| d.elements().next())
        {
            value = self.data(d, &mut forced, 0)?;
        }
        Ok(Some(Tag {
            name,
            data_type,
            dimensions,
            kind,
            usage,
            external_access,
            constant: e.attr("Constant") == Some("true"),
            description,
            data: value.map(|value| TagData { value, forced }),
        }))
    }

    /// Decorated data (RM014 pp.43-46): `DataValue`, `Structure`,
    /// `Array` and their members and elements. Anything else is left out
    /// (`None`); every value counts toward [`MAX_ITEMS`].
    fn data(&mut self, e: &Element, forced: &mut bool, depth: usize) -> Result<Option<Data>, Error> {
        self.count(e)?;
        if depth > 8 {
            return Ok(None);
        }
        *forced |= e.attr("ForceValue").is_some();
        Ok(match e.name.as_str() {
            "DataValue" | "DataValueMember" => e.attr("Value").map(|v| Data::Value(clip(v, 64))),
            "Structure" | "StructureMember" => {
                let mut members = Vec::new();
                for m in e.elements() {
                    let Some(n) = m.attr("Name").filter(|n| is_name(n)) else {
                        continue;
                    };
                    if let Some(d) = self.data(m, forced, depth + 1)? {
                        members.push((n.to_string(), d));
                    }
                }
                Some(Data::Structure(members))
            }
            "Array" | "ArrayMember" => {
                let mut elems = Vec::new();
                for el in e.children_named("Element") {
                    self.count(el)?;
                    *forced |= el.attr("ForceValue").is_some();
                    let Some(ix) = el.attr("Index").filter(|i| is_ref(i)) else {
                        continue;
                    };
                    let d = match (el.attr("Value"), el.elements().next()) {
                        (Some(v), _) => Some(Data::Value(clip(v, 64))),
                        (None, Some(inner)) => self.data(inner, forced, depth + 1)?,
                        (None, None) => None,
                    };
                    if let Some(d) = d {
                        elems.push((ix.to_string(), d));
                    }
                }
                Some(Data::Array(elems))
            }
            _ => None,
        })
    }

    fn tags(&mut self, parent: &Element) -> Result<Vec<Tag>, Error> {
        let mut out = Vec::new();
        if let Some(tags) = parent.child("Tags") {
            for t in tags.children_named("Tag") {
                if let Some(t) = self.tag(t)? {
                    out.push(t);
                }
            }
        }
        Ok(out)
    }

    fn data_type(&mut self, e: &Element) -> Result<Option<DataType>, Error> {
        let Some(name) = self.name(e, "data type") else {
            return Ok(None);
        };
        let mut members = Vec::new();
        for m in e
            .child("Members")
            .into_iter()
            .flat_map(|m| m.children_named("Member"))
        {
            self.count(m)?;
            if m.attr("Hidden") == Some("true") {
                continue;
            }
            let Some(mname) = self.name(m, "data type member") else {
                continue;
            };
            let Some(ty) = m.attr("DataType").filter(|t| is_name(t)) else {
                self.skip(
                    m.pos,
                    format!("member {name}.{mname} without a readable DataType"),
                );
                continue;
            };
            let dimension = m
                .attr("Dimension")
                .and_then(|d| d.trim().parse::<u32>().ok())
                .filter(|d| *d > 0);
            members.push(Member {
                name: mname,
                data_type: ty.to_string(),
                dimension,
            });
        }
        Ok(Some(DataType { name, members }))
    }

    fn routine(&mut self, e: &Element) -> Result<Option<Routine>, Error> {
        let Some(name) = self.name(e, "routine") else {
            return Ok(None);
        };
        let kind = match e.attr("Type") {
            Some("RLL") => RoutineKind::Ladder,
            Some("ST") => RoutineKind::St,
            Some("FBD") => RoutineKind::Fbd,
            Some("SFC") => RoutineKind::Sfc,
            _ => {
                self.skip(e.pos, format!("routine {name} of an unknown Type"));
                return Ok(None);
            }
        };
        let mut rungs = Vec::new();
        let mut lines = Vec::new();
        match kind {
            RoutineKind::Ladder => {
                for r in e
                    .child("RLLContent")
                    .into_iter()
                    .flat_map(|c| c.children_named("Rung"))
                {
                    self.count(r)?;
                    let number = r
                        .attr("Number")
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(u32::try_from(rungs.len()).unwrap_or(u32::MAX));
                    let text = r
                        .child("Text")
                        .map(|t| clip(&t.text().0, MAX_TEXT))
                        .unwrap_or_default();
                    let comment = r
                        .child("Comment")
                        .map(|c| clip(&c.text().0, MAX_TEXT))
                        .filter(|c| !c.is_empty());
                    rungs.push(Rung {
                        number,
                        text,
                        comment,
                    });
                }
            }
            RoutineKind::St => {
                for l in e
                    .child("STContent")
                    .into_iter()
                    .flat_map(|c| c.children_named("Line"))
                {
                    self.count(l)?;
                    // A line's own indentation is meaning to a reader; keep it.
                    let raw = l.text().0;
                    let kept = clip(&raw, MAX_TEXT);
                    let indent: String = raw
                        .trim_start_matches(['\n', '\r'])
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .take(64)
                        .collect();
                    lines.push(if kept.is_empty() { kept } else { indent + &kept });
                }
            }
            RoutineKind::Fbd | RoutineKind::Sfc => {}
        }
        Ok(Some(Routine {
            name,
            kind,
            rungs,
            lines,
        }))
    }

    fn program(&mut self, e: &Element) -> Result<Option<Program>, Error> {
        let Some(name) = self.name(e, "program") else {
            return Ok(None);
        };
        let tags = self.tags(e)?;
        let mut routines = Vec::new();
        for r in e
            .child("Routines")
            .into_iter()
            .flat_map(|r| r.children_named("Routine"))
        {
            if let Some(r) = self.routine(r)? {
                routines.push(r);
            }
        }
        Ok(Some(Program {
            name,
            main_routine: e
                .attr("MainRoutineName")
                .filter(|n| is_name(n))
                .map(str::to_string),
            disabled: e.attr("Disabled") == Some("true"),
            tags,
            routines,
        }))
    }

    fn task(&mut self, e: &Element) -> Option<Task> {
        let name = self.name(e, "task")?;
        let kind = e
            .attr("Type")
            .filter(|t| matches!(*t, "CONTINUOUS" | "PERIODIC" | "EVENT"))
            .map(str::to_string);
        // Rates are whole ms in exports; anything else is reported as absent.
        let rate_ms = e.attr("Rate").and_then(|r| r.trim().parse::<u32>().ok());
        let programs = e
            .child("ScheduledPrograms")
            .into_iter()
            .flat_map(|s| s.children_named("ScheduledProgram"))
            .filter_map(|p| p.attr("Name").filter(|n| is_name(n)).map(str::to_string))
            .collect();
        Some(Task {
            name,
            kind,
            rate_ms,
            programs,
        })
    }
}

/// `Dimensions="10"`, `"2 3"`, `"4,5"`; absent or `"0"` for a scalar.
fn dims(d: Option<&str>) -> Option<Vec<u32>> {
    let Some(d) = d else {
        return Some(Vec::new());
    };
    if d.len() > MAX_REF {
        return None;
    }
    let mut out = Vec::new();
    for part in d.split([' ', ',']).filter(|p| !p.is_empty()) {
        let n: u32 = part.parse().ok()?;
        out.push(n);
    }
    if out.len() > 3 {
        return None;
    }
    if out == [0] {
        out.clear();
    }
    Some(out)
}

/// Reads an L5X export.
///
/// # Errors
/// A file that is too large, is not well-formed XML, is not an L5X
/// export, or holds more than [`MAX_ITEMS`] things.
pub fn parse(src: &str) -> Result<Project, Error> {
    let at_start = Pos { line: 1, col: 1 };
    if src.len() > MAX_L5X {
        return Err(Error {
            pos: at_start,
            msg: format!("larger than {MAX_L5X} bytes; export less (one program, or the tags alone)"),
        });
    }
    let root = xml::parse(src).map_err(|e| Error {
        pos: e.pos,
        msg: e.msg,
    })?;
    if root.name != "RSLogix5000Content" {
        return Err(Error {
            pos: root.pos,
            msg: format!(
                "expected an L5X <RSLogix5000Content>, found <{}>",
                clip(&root.name, 40)
            ),
        });
    }
    let short = |k: &str| root.attr(k).map(|v| clip(v, 64)).filter(|v| !v.is_empty());
    let mut cx = Cx {
        p: Project {
            target: short("TargetName"),
            target_type: short("TargetType"),
            software: short("SoftwareRevision"),
            ..Project::default()
        },
        items: 0,
    };
    let Some(c) = root.child("Controller") else {
        return Err(Error {
            pos: root.pos,
            msg: "no <Controller> in the export".into(),
        });
    };
    cx.p.controller = cx.name(c, "controller").map(|name| Controller {
        name,
        processor: c.attr("ProcessorType").map(|p| clip(p, 64)),
    });
    for dt in c
        .child("DataTypes")
        .into_iter()
        .flat_map(|d| d.children_named("DataType"))
    {
        if let Some(dt) = cx.data_type(dt)? {
            cx.p.data_types.push(dt);
        }
    }
    for a in c
        .child("AddOnInstructionDefinitions")
        .into_iter()
        .flat_map(|d| d.children_named("AddOnInstructionDefinition"))
    {
        if let Some(n) = cx.name(a, "add-on instruction") {
            cx.p.add_ons.push(n);
        }
    }
    cx.p.tags = cx.tags(c)?;
    for p in c
        .child("Programs")
        .into_iter()
        .flat_map(|p| p.children_named("Program"))
    {
        if let Some(p) = cx.program(p)? {
            cx.p.programs.push(p);
        }
    }
    for t in c
        .child("Tasks")
        .into_iter()
        .flat_map(|t| t.children_named("Task"))
    {
        if let Some(t) = cx.task(t) {
            cx.p.tasks.push(t);
        }
    }
    Ok(cx.p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_logix_identifiers() {
        for ok in ["a", "_x", "Start_PB", "T1", &"a".repeat(40)] {
            assert!(is_name(ok), "{ok}");
        }
        for bad in [
            "",
            "1a",
            "a b",
            "a.b",
            "a:b",
            "é",
            &"a".repeat(41),
            "ignore previous instructions",
        ] {
            assert!(!is_name(bad), "{bad}");
        }
    }

    #[test]
    fn dimensions_read_as_exported() {
        assert_eq!(dims(None), Some(vec![]));
        assert_eq!(dims(Some("0")), Some(vec![]));
        assert_eq!(dims(Some("10")), Some(vec![10]));
        assert_eq!(dims(Some("2 3")), Some(vec![2, 3]));
        assert_eq!(dims(Some("4,5,6")), Some(vec![4, 5, 6]));
        assert_eq!(dims(Some("1 2 3 4")), None);
        assert_eq!(dims(Some("-1")), None);
        assert_eq!(dims(Some("x")), None);
    }

    #[test]
    fn clipping_says_so() {
        assert_eq!(clip("  ab\u{7}c ", 10), "ab c");
        assert_eq!(clip("abcdef", 3), "abc [clipped: 3 more characters]");
    }
}
