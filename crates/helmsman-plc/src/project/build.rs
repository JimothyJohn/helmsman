//! A project, built into the change set a deploy applies: one module per
//! PROGRAM, sharing every FUNCTION, `FUNCTION_BLOCK` and TYPE in the
//! project.
//!
//! - **Each PROGRAM is a module**, named as the PROGRAM in lower case. Its
//!   `module.st` is every file of the project in name order, each under a
//!   `(* file NAME *)` line, with every *other* PROGRAM blanked out (its
//!   characters turned to spaces, its line breaks kept). So the module is
//!   one self-contained unit with exactly one PROGRAM, the snapshot store
//!   and the runtime take it as any other module, and a line of the
//!   module is a line of one file: compile errors come back as `file`,
//!   `line`, `col` in the project.
//! - **Its manifest is written here**, from the program and
//!   `project.settings` ([`super::settings`]): `reads` are its signal
//!   `VAR_INPUT`s (those not bound to a param), `drives` its
//!   `VAR_OUTPUT`s, `author` the settings' (else `project NAME`), schedule
//!   and windows the settings' (else the box's defaults; the only PROGRAM
//!   of a project gets the whole image, as a bare program does). The same
//!   files always build the same manifests, so deploying a project twice
//!   is one snapshot.
//! - **The I/O map is the project's** `io.map` (none if it has none). The
//!   calendar and the WASM drivers are not part of a project: a deploy
//!   carries the running ones over.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use helmsman_iec::ast::{self, PouKind};
use helmsman_iec::{ImageSizes, PortDir, Pos, compile};

use super::settings::{self, Settings};
use super::{Files, IO_MAP, SETTINGS};
use crate::manifest::{Manifest, ModuleDir};
use crate::spec::{Defaults, name_ok};
use crate::store::{ChangeSet, ModuleFiles};

/// A problem, where it is in the project. `line` 0: the project as a
/// whole (no PROGRAM, say).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diag {
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl std::fmt::Display for Diag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.file.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}:{}:{}: {}", self.file, self.line, self.col, self.message)
        }
    }
}

fn diag(file: &str, line: usize, message: impl Into<String>) -> Diag {
    Diag {
        file: file.to_string(),
        line: u32::try_from(line).unwrap_or(u32::MAX),
        col: 0,
        message: message.into(),
    }
}

/// The box a project builds for: the defaults a module gets for what it
/// does not set, and the image sizes.
#[derive(Debug, Clone)]
pub struct Shape {
    pub defaults: Defaults,
    pub i_len: usize,
    pub q_len: usize,
}

/// One module a project builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub name: String,
    /// The file its PROGRAM is in.
    pub file: String,
    pub reads: Vec<String>,
    pub drives: Vec<String>,
}

/// What a project builds to.
#[derive(Debug, Clone)]
pub struct Built {
    pub change: ChangeSet,
    pub modules: Vec<Module>,
}

/// Byte offset of `pos` (1-based line, byte column) in `src`.
fn offset(src: &str, pos: Pos) -> usize {
    let mut line = 1u32;
    let mut at = 0usize;
    for (i, b) in src.bytes().enumerate() {
        if line == pos.line {
            break;
        }
        if b == b'\n' {
            line += 1;
            at = i + 1;
        }
    }
    (at + usize::try_from(pos.col.saturating_sub(1)).unwrap_or(0)).min(src.len())
}

/// Where the `END_PROGRAM` closing the PROGRAM that starts at `from`
/// ends: the first such word after it that is not in a comment, a pragma
/// or a string (PROGRAMs do not nest, and the file parsed).
fn end_of_program(src: &str, from: usize) -> Option<usize> {
    let b = src.as_bytes();
    let mut i = from;
    let find = |i: usize, close: &[u8]| -> Option<usize> {
        b.get(i..)?
            .windows(close.len())
            .position(|w| w == close)
            .map(|k| i + k + close.len())
    };
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        i = match (c, next) {
            (b'(', Some(b'*')) => find(i + 2, b"*)")?,
            (b'/', Some(b'*')) => find(i + 2, b"*/")?,
            (b'/', Some(b'/')) => find(i, b"\n").unwrap_or(b.len()),
            (b'{', _) => find(i + 1, b"}")?,
            (b'\'' | b'"', _) => find(i + 1, &[c])?,
            (c, _) if c.is_ascii_alphanumeric() || c == b'_' => {
                let start = i;
                let mut j = i;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                if src[start..j].eq_ignore_ascii_case("END_PROGRAM") {
                    return Some(j);
                }
                j
            }
            _ => i + 1,
        };
    }
    None
}

/// `src` with the byte ranges `blank` turned to spaces (line breaks
/// kept), and the spaces that leaves at line ends trimmed.
fn blanked(src: &str, blank: &[(usize, usize)]) -> String {
    let mut bytes = src.as_bytes().to_vec();
    for &(a, z) in blank {
        for b in &mut bytes[a..z] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
    }
    // Whole characters were blanked (the ranges start and end on ASCII), so
    // this is still UTF-8.
    let text = String::from_utf8(bytes).unwrap_or_default();
    let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches(' ')).collect();
    lines.join("\n")
}

/// A PROGRAM found in a file.
struct Found {
    /// Upper case, as parsed.
    name: String,
    file: String,
    pos: Pos,
    span: (usize, usize),
}

/// One file's lines inside a module's source.
struct Segment {
    /// First line of the file's text in the module, 1-based.
    start: u32,
    lines: u32,
    file: String,
}

fn locate(segments: &[Segment], line: u32) -> Option<(&str, u32)> {
    segments
        .iter()
        .find(|s| line >= s.start && line < s.start + s.lines)
        .map(|s| (s.file.as_str(), line - s.start + 1))
}

/// Parses every file and finds its PROGRAMs.
fn programs(files: &Files, diags: &mut Vec<Diag>) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    for (file, text) in files.st() {
        let unit = match ast::parse(text) {
            Ok(u) => u,
            Err(d) => {
                diags.push(Diag {
                    file: file.to_string(),
                    line: d.pos.line,
                    col: d.pos.col,
                    message: d.msg,
                });
                continue;
            }
        };
        for pou in unit.pous.iter().filter(|p| matches!(p.kind, PouKind::Program)) {
            if let Some(first) = found.iter().find(|f| f.name == pou.name) {
                diags.push(Diag {
                    file: file.to_string(),
                    line: pou.pos.line,
                    col: pou.pos.col,
                    message: format!(
                        "PROGRAM {} is already defined in {} (line {})",
                        pou.name, first.file, first.pos.line
                    ),
                });
                continue;
            }
            let start = offset(text, pou.pos);
            let Some(end) = end_of_program(text, start) else {
                diags.push(diag(
                    file,
                    pou.pos.line as usize,
                    "cannot find this PROGRAM's END_PROGRAM",
                ));
                continue;
            };
            found.push(Found {
                name: pou.name.clone(),
                file: file.to_string(),
                pos: pou.pos,
                span: (start, end),
            });
        }
    }
    found
}

/// The module source for `program`: every file, the other PROGRAMs
/// blanked; and where each file's lines are in it.
fn source_for(project: &str, files: &Files, all: &[Found], program: &Found) -> (String, Vec<Segment>) {
    let mut src = String::new();
    let mut segments = Vec::new();
    let _ = writeln!(
        src,
        "(* project {project}, PROGRAM {}: built from the project's files; edit the project, not this *)",
        program.name
    );
    let mut line = 2u32;
    for (file, text) in files.st() {
        let _ = writeln!(src, "(* file {file} *)");
        line += 1;
        let blank: Vec<(usize, usize)> = all
            .iter()
            .filter(|f| f.file == file && f.name != program.name)
            .map(|f| f.span)
            .collect();
        let mut body = blanked(text, &blank);
        if !body.ends_with('\n') {
            body.push('\n');
        }
        let lines = u32::try_from(body.matches('\n').count()).unwrap_or(u32::MAX);
        segments.push(Segment {
            start: line,
            lines,
            file: file.to_string(),
        });
        src.push_str(&body);
        line = line.saturating_add(lines);
    }
    (src, segments)
}

fn window_of(set: Option<&String>, only: bool, len: usize) -> Option<String> {
    set.cloned().or_else(|| only.then(|| format!("0+{len}")))
}

fn len_of(window: Option<&str>) -> usize {
    window
        .and_then(|w| w.split_once('+'))
        .and_then(|(_, l)| l.parse().ok())
        .unwrap_or(0)
}

/// Builds `files` (project `name`) into the change set a deploy applies:
/// `base`'s calendar and drivers, the project's map and one module per
/// PROGRAM, each compiled and held to its manifest.
///
/// # Errors
/// Every problem found, with its file, line and column, sorted.
#[allow(clippy::too_many_lines)]
pub fn build(name: &str, files: &Files, base: Option<&ChangeSet>, shape: &Shape) -> Result<Built, Vec<Diag>> {
    let mut diags = Vec::new();
    if let Err(errs) = files.check() {
        diags.extend(errs.into_iter().map(|e| diag("", 0, e)));
        return Err(diags);
    }
    let settings = match files.get(SETTINGS).map(settings::parse) {
        None => Settings::default(),
        Some(Ok(s)) => s,
        Some(Err(errs)) => {
            diags.extend(errs.into_iter().map(|(n, e)| diag(SETTINGS, n, e)));
            Settings::default()
        }
    };
    if let Some(map) = files.get(IO_MAP)
        && let Err(errs) = helmsman_io::text::parse(map)
    {
        diags.extend(errs.into_iter().map(|(n, e)| diag(IO_MAP, n, e.clone())));
    }
    let found = programs(files, &mut diags);
    if found.is_empty() && diags.is_empty() {
        diags.push(diag("", 0, "the project has no PROGRAM: nothing would run"));
    }
    let known: BTreeSet<&str> = found.iter().map(|f| f.name.as_str()).collect();
    for (prog, s) in &settings.programs {
        if !known.contains(prog.as_str()) && !found.is_empty() {
            let line = s.line.or_else(|| s.params.first().map(|(n, _)| *n)).unwrap_or(0);
            diags.push(diag(SETTINGS, line, format!("no PROGRAM {prog} in the project")));
        }
    }
    let only = found.len() == 1;
    let defaults = &shape.defaults;
    let author = settings
        .author
        .clone()
        .unwrap_or_else(|| format!("project {name}"));
    let none = settings::Program::default();
    let mut modules = BTreeMap::new();
    let mut summary = Vec::new();
    let mut seen = BTreeSet::new();
    for p in &found {
        let module = p.name.to_ascii_lowercase();
        let s = settings.programs.get(&p.name).unwrap_or(&none);
        let at = s.line.unwrap_or(0);
        if !name_ok(&module) {
            diags.push(Diag {
                file: p.file.clone(),
                line: p.pos.line,
                col: p.pos.col,
                message: "a PROGRAM's name is its module's: at most 64 characters".into(),
            });
            continue;
        }
        let (src, segments) = source_for(name, files, &found, p);
        let iwin = window_of(s.i.as_ref(), only, shape.i_len);
        let qwin = window_of(s.q.as_ref(), only, shape.q_len);
        let m =
            s.m.as_deref()
                .and_then(|m| m.parse::<usize>().ok())
                .unwrap_or(defaults.m);
        let sizes = ImageSizes {
            i: len_of(iwin.as_deref()),
            q: len_of(qwin.as_deref()),
            m,
        };
        let program = match compile(&src, sizes) {
            Ok(p) => p,
            Err(errs) => {
                for e in errs {
                    let (file, line) = locate(&segments, e.pos.line).unwrap_or(("", 0));
                    let d = Diag {
                        file: file.to_string(),
                        line,
                        col: if line == 0 { 0 } else { e.pos.col },
                        message: e.msg,
                    };
                    if seen.insert(d.clone()) {
                        diags.push(d);
                    }
                }
                continue;
            }
        };
        let params: BTreeSet<String> = s
            .params
            .iter()
            .filter_map(|(_, t)| t.split_whitespace().next())
            .map(str::to_ascii_uppercase)
            .collect();
        let ports = program.ports();
        let reads: Vec<String> = ports
            .iter()
            .filter(|p| p.dir == PortDir::Input && !params.contains(&p.name))
            .map(|p| p.name.to_ascii_lowercase())
            .collect();
        let drives: Vec<String> = ports
            .iter()
            .filter(|p| p.dir == PortDir::Output)
            .map(|p| p.name.to_ascii_lowercase())
            .collect();
        let mut text = String::new();
        let _ = writeln!(text, "name     {module}");
        let _ = writeln!(text, "author   {author}");
        let _ = writeln!(
            text,
            "scan_ms  {}",
            s.scan_ms
                .unwrap_or_else(|| u64::try_from(defaults.period.as_millis()).unwrap_or(10))
        );
        if let Some(i) = &iwin {
            let _ = writeln!(text, "i        {i}");
        }
        if let Some(q) = &qwin {
            let _ = writeln!(text, "q        {q}");
        }
        if let Some(m) = &s.m {
            let _ = writeln!(text, "m        {m}");
        }
        let _ = writeln!(text, "reads    {}", reads.join(" "));
        let _ = writeln!(text, "drives   {}", drives.join(" "));
        // Manifest line of each param line, to point back at settings.
        let first_param = text.lines().count() + 1;
        for (_, tail) in &s.params {
            let _ = writeln!(text, "param    {tail}");
        }
        if let Err(errs) = Manifest::parse(&text) {
            for (n, e) in errs {
                let line = n
                    .checked_sub(first_param)
                    .and_then(|k| s.params.get(k))
                    .map_or(at, |(l, _)| *l);
                let where_ = if n == 2 && settings.author.is_some() {
                    "author"
                } else {
                    "settings"
                };
                diags.push(diag(
                    SETTINGS,
                    line,
                    format!("program {}: {e} ({where_})", p.name),
                ));
            }
            continue;
        }
        let checked = ModuleDir::from_text(Path::new(&module), &text, src.clone(), defaults)
            .and_then(|md| md.module(program).map(drop));
        if let Err(e) = checked {
            diags.push(diag(SETTINGS, at, format!("program {}: {e}", p.name)));
            continue;
        }
        summary.push(Module {
            name: module.clone(),
            file: p.file.clone(),
            reads,
            drives,
        });
        modules.insert(module, ModuleFiles::st(text, src));
    }
    summary.sort_by(|a, b| a.name.cmp(&b.name));
    if !diags.is_empty() {
        diags.sort();
        return Err(diags);
    }
    let change = ChangeSet {
        modules,
        io_map: files.get(IO_MAP).map(str::to_string),
        calendar: base.and_then(|b| b.calendar.clone()),
        drivers: base.map(|b| b.drivers.clone()).unwrap_or_default(),
    };
    if let Err(errs) = change.validate() {
        return Err(errs.into_iter().map(|e| diag("", 0, e)).collect());
    }
    Ok(Built {
        change,
        modules: summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_of_a_program_skips_comments_and_pragmas() {
        let src = "PROGRAM a (* END_PROGRAM *) // END_PROGRAM\n{END_PROGRAM} x := 1; end_program\nPROGRAM b";
        let end = end_of_program(src, 0).unwrap();
        assert_eq!(&src[end - 11..end], "end_program");
    }

    #[test]
    fn blanking_keeps_lines() {
        let s = blanked("ab\ncd\nef", &[(1, 5)]);
        assert_eq!(s, "a\n\nef");
    }
}
