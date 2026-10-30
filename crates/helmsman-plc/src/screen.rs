//! The floor screen: a full-screen andon view the box serves, made of
//! tiles that show output signals. A tile is an *output* like a stack
//! light: the I/O map declares it (`out line1_state int screen line1`),
//! a module drives it through a `VAR_OUTPUT` of the same name, and so the
//! one-writer rule, safe values, the approval tiers and the audit apply
//! unchanged. This file is how the tiles look: which signal is a tile's
//! state, what each state code means (a fixed colour and a fixed
//! message), and where the tile sits.
//!
//! Signals stay scalar: a module drives a state *code* (an `INT`, or a
//! `BOOL` read as 0 / 1), never text. The words on the floor are box
//! configuration, reviewed like the rest of it, so a module cannot put
//! arbitrary text in front of the line, and nothing in the scan, the
//! recorder, replay or the sinks learns a string type.
//!
//! ```text
//! helmsman-screen 1
//! # Hall 2, lines 1 and 2
//! title  Hall 2 andon
//! grid   2 1
//! code   line 0 grey   Idle
//! code   line 1 green  Running
//! code   line 2 yellow Material low
//! code   line 3 red    Down: call maintenance
//! tile   line1 0,0 line line1_state Line 1
//! tile   line2 1,0 line line2_state Line 2
//! value  line1 line1_parts parts today
//! ```
//!
//! - `helmsman-screen 1` first: the format and its version.
//! - `title TEXT`: the header (optional).
//! - `grid COLS ROWS`: 1..=8 each.
//! - `code SET N COLOUR TEXT`: in code set SET, state N shows COLOUR and
//!   TEXT. Colours are the fixed andon palette ([`Colour`]); there is no
//!   user theming. A state with no `code` line shows as unknown.
//! - `tile ID COL,ROW[,W,H] SET SIGNAL TITLE`: a tile at that cell (W by
//!   H cells, default 1 by 1) whose state is SIGNAL (`bool` or `int`),
//!   read through code set SET.
//! - `value ID SIGNAL LABEL`: tile ID also shows SIGNAL (`int` or `real`)
//!   as a number with LABEL under it (at most one per tile).
//!
//! TEXT, TITLE and LABEL run to the end of the line (words joined by one
//! space): any Unicode but control and invisible formatting characters,
//! at most [`MAX_TEXT`] characters. A line whose first character is `#`
//! is a comment; elsewhere `#` is text. Tiles must fit the grid and not
//! overlap. [`parse`] checks the file; [`Layout::check`] checks it
//! against the I/O map: every tile's signals are `screen` outputs bound to
//! that tile, and every `screen` output is on a tile.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use helmsman_io::{Binding, Dir, IoMap, Ty};
use serde::Serialize;

use crate::manifest::hidden;

/// The first line of every layout file.
pub const HEADER: &str = "helmsman-screen 1";
/// Largest layout file, in bytes.
pub const MAX_FILE: usize = 64 * 1024;
/// Longest title, message or label, in characters.
pub const MAX_TEXT: usize = 48;
/// Most columns and rows.
pub const MAX_GRID: u8 = 8;
/// Most tiles.
pub const MAX_TILES: usize = 64;
/// Most code sets, and most codes in one set.
pub const MAX_SETS: usize = 32;
pub const MAX_CODES: usize = 64;

/// The andon palette, fixed (no user theming): the stack-light colours of
/// IEC 60073 plus the brand's neutrals. Each has a background and a text
/// colour chosen for contrast from across a floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Colour {
    Green,
    /// The brand's safety yellow.
    Yellow,
    Red,
    Blue,
    White,
    /// The brand's graphite: idle, off.
    Grey,
}

impl Colour {
    pub const ALL: [Self; 6] = [
        Self::Green,
        Self::Yellow,
        Self::Red,
        Self::Blue,
        Self::White,
        Self::Grey,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Red => "red",
            Self::Blue => "blue",
            Self::White => "white",
            Self::Grey => "grey",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == s)
    }

    /// Background, as RGB.
    #[must_use]
    pub const fn fill(self) -> [u8; 3] {
        match self {
            Self::Green => [0x1E, 0x8E, 0x3E],
            Self::Yellow => [0xFF, 0xC2, 0x0E],
            Self::Red => [0xC6, 0x28, 0x28],
            Self::Blue => [0x00, 0x57, 0xB8],
            Self::White => [0xFF, 0xFF, 0xFF],
            Self::Grey => [0x3A, 0x3A, 0x3A],
        }
    }

    /// Text on [`Colour::fill`], as RGB.
    #[must_use]
    pub const fn ink(self) -> [u8; 3] {
        match self {
            Self::Yellow | Self::White => [0x14, 0x14, 0x14],
            _ => [0xFF, 0xFF, 0xFF],
        }
    }
}

/// What a state code shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct State {
    pub colour: Colour,
    pub text: String,
}

/// A tile's number: a signal and what it counts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Value {
    pub signal: String,
    pub label: String,
}

/// One tile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Tile {
    pub id: String,
    pub col: u8,
    pub row: u8,
    pub w: u8,
    pub h: u8,
    /// The code set its state reads through.
    pub codes: String,
    /// Its state signal (`bool` or `int`).
    pub state: String,
    pub title: String,
    pub value: Option<Value>,
}

/// A parsed layout file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Layout {
    pub title: Option<String>,
    pub cols: u8,
    pub rows: u8,
    /// Code set name -> state code -> what it shows.
    pub sets: BTreeMap<String, BTreeMap<i64, State>>,
    /// In file order.
    pub tiles: Vec<Tile>,
}

impl Layout {
    /// What tile `t`'s state `code` shows, if the layout says.
    #[must_use]
    pub fn state(&self, t: &Tile, code: i64) -> Option<&State> {
        self.sets.get(&t.codes)?.get(&code)
    }

    /// Checks the layout against the I/O map it is shown from.
    ///
    /// # Errors
    /// Every problem: a tile's signal missing from the map, not a `screen`
    /// output on that tile, or of a type the tile cannot show; a `screen`
    /// output on no tile.
    pub fn check(&self, map: &IoMap) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        let mut shown = BTreeSet::new();
        for t in &self.tiles {
            let mut want = |name: &str, what: &str, types: &[Ty]| {
                shown.insert(name.to_string());
                let Some(s) = map.signals.iter().find(|s| s.name == name) else {
                    errs.push(format!("tile {}: no signal {name} in the I/O map", t.id));
                    return;
                };
                if s.dir != Dir::Out || s.binding != (Binding::Screen { tile: t.id.clone() }) {
                    errs.push(format!(
                        "tile {}: {name} must be declared `out {name} TYPE screen {}`",
                        t.id, t.id
                    ));
                }
                if !types.contains(&s.ty) {
                    errs.push(format!(
                        "tile {}: {what} {name} is {:?}; it must be {types:?}",
                        t.id, s.ty
                    ));
                }
            };
            want(&t.state, "state", &[Ty::Bool, Ty::Int]);
            if let Some(v) = &t.value {
                want(&v.signal, "value", &[Ty::Int, Ty::Real]);
            }
        }
        for s in &map.signals {
            if let Binding::Screen { tile } = &s.binding
                && !shown.contains(&s.name)
            {
                errs.push(format!(
                    "{} is a screen output (tile {tile}) that no tile of the layout shows",
                    s.name
                ));
            }
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }
}

/// Why a layout does not parse: a line number (from 1; 0 for the file as
/// a whole) and what is wrong there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub why: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            f.write_str(&self.why)
        } else {
            write!(f, "line {}: {}", self.line, self.why)
        }
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Free text: words joined by one space, 1..=[`MAX_TEXT`] characters,
/// nothing hidden.
fn text(words: &[&str], what: &str) -> Result<String, String> {
    let t = words.join(" ");
    if t.is_empty() {
        return Err(format!("{what}: not empty"));
    }
    if let Some(c) = t.chars().find(|&c| hidden(c)) {
        return Err(format!("{what}: no control or invisible characters, got {c:?}"));
    }
    if t.chars().count() > MAX_TEXT {
        return Err(format!("{what}: at most {MAX_TEXT} characters"));
    }
    Ok(t)
}

fn small(s: &str, lo: u8, hi: u8, what: &str) -> Result<u8, String> {
    s.parse::<u8>()
        .ok()
        .filter(|n| (lo..=hi).contains(n) && s.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| format!("{what}: expected {lo}..={hi}, got {s:?}"))
}

/// Parses a layout file.
///
/// # Errors
/// Every problem, each with its line.
pub fn parse(src: &str) -> Result<Layout, Vec<Error>> {
    let mut errs = Vec::new();
    if src.len() > MAX_FILE {
        return Err(vec![Error {
            line: 0,
            why: format!("larger than {MAX_FILE} bytes"),
        }]);
    }
    let mut lines = src.lines().enumerate().filter(|(_, l)| {
        let t = l.trim_matches([' ', '\t']);
        !t.is_empty() && !t.starts_with('#')
    });
    match lines.next() {
        Some((_, l)) if l.trim_matches([' ', '\t']) == HEADER => {}
        Some((n, _)) => errs.push(Error {
            line: n + 1,
            why: format!("expected {HEADER:?} first"),
        }),
        None => {
            return Err(vec![Error {
                line: 0,
                why: format!("empty: expected {HEADER:?} first"),
            }]);
        }
    }
    let mut d = Draft::default();
    for (n, raw) in lines {
        let words: Vec<&str> = raw.split([' ', '\t']).filter(|w| !w.is_empty()).collect();
        if let Err(why) = d.line(&words) {
            errs.push(Error { line: n + 1, why });
        }
    }
    match d.finish() {
        Ok(l) if errs.is_empty() => Ok(l),
        Ok(_) => Err(errs),
        Err(more) => {
            errs.extend(more.into_iter().map(|why| Error { line: 0, why }));
            Err(errs)
        }
    }
}

#[derive(Default)]
struct Draft {
    title: Option<String>,
    grid: Option<(u8, u8)>,
    sets: BTreeMap<String, BTreeMap<i64, State>>,
    tiles: Vec<Tile>,
    /// Tile id -> its value line.
    values: BTreeMap<String, Value>,
}

impl Draft {
    fn line(&mut self, words: &[&str]) -> Result<(), String> {
        match words {
            ["title", rest @ ..] => {
                let t = text(rest, "title")?;
                if self.title.replace(t).is_some() {
                    return Err("title given twice".into());
                }
                Ok(())
            }
            ["grid", c, r] => {
                let g = (small(c, 1, MAX_GRID, "columns")?, small(r, 1, MAX_GRID, "rows")?);
                if self.grid.replace(g).is_some() {
                    return Err("grid given twice".into());
                }
                Ok(())
            }
            ["code", set, n, colour, rest @ ..] => {
                if !is_ident(set) {
                    return Err(format!(
                        "code set {set:?}: an identifier of at most 32 characters"
                    ));
                }
                let code = n
                    .parse::<i64>()
                    .ok()
                    .filter(|_| n.trim_start_matches('-').bytes().all(|b| b.is_ascii_digit()))
                    .ok_or_else(|| format!("state code {n:?}: a whole number"))?;
                let colour = Colour::parse(colour).ok_or_else(|| {
                    let names: Vec<&str> = Colour::ALL.iter().map(|c| c.name()).collect();
                    format!("colour {colour:?}: one of {}", names.join(", "))
                })?;
                let text = text(rest, "message")?;
                if !self.sets.contains_key(*set) && self.sets.len() >= MAX_SETS {
                    return Err(format!("at most {MAX_SETS} code sets"));
                }
                let codes = self.sets.entry((*set).to_string()).or_default();
                if codes.len() >= MAX_CODES && !codes.contains_key(&code) {
                    return Err(format!("at most {MAX_CODES} codes in a set"));
                }
                if codes.insert(code, State { colour, text }).is_some() {
                    return Err(format!("code {code} of set {set} given twice"));
                }
                Ok(())
            }
            ["tile", id, at, codes, state, rest @ ..] => {
                if !is_ident(id) {
                    return Err(format!("tile id {id:?}: an identifier of at most 32 characters"));
                }
                if self.tiles.iter().any(|t| t.id == *id) {
                    return Err(format!("tile {id} given twice"));
                }
                if self.tiles.len() >= MAX_TILES {
                    return Err(format!("at most {MAX_TILES} tiles"));
                }
                let at: Vec<&str> = at.split(',').collect();
                let (col, row, w, h) = match at.as_slice() {
                    [c, r] => (c, r, &"1", &"1"),
                    [c, r, w, h] => (c, r, w, h),
                    _ => return Err(format!("tile {id}: place it as COL,ROW or COL,ROW,W,H")),
                };
                let col = small(col, 0, MAX_GRID - 1, "column")?;
                let row = small(row, 0, MAX_GRID - 1, "row")?;
                let w = small(w, 1, MAX_GRID, "width")?;
                let h = small(h, 1, MAX_GRID, "height")?;
                for (name, what) in [(codes, "code set"), (state, "state signal")] {
                    if !is_ident(name) {
                        return Err(format!("tile {id}: {what} {name:?} is not an identifier"));
                    }
                }
                self.tiles.push(Tile {
                    id: (*id).into(),
                    col,
                    row,
                    w,
                    h,
                    codes: (*codes).into(),
                    state: (*state).into(),
                    title: text(rest, "tile title")?,
                    value: None,
                });
                Ok(())
            }
            ["value", id, signal, rest @ ..] => {
                if !is_ident(signal) {
                    return Err(format!("value signal {signal:?} is not an identifier"));
                }
                let v = Value {
                    signal: (*signal).into(),
                    label: text(rest, "value label")?,
                };
                if self.values.insert((*id).to_string(), v).is_some() {
                    return Err(format!("tile {id} has one value line at most"));
                }
                Ok(())
            }
            [w @ ("grid" | "code" | "tile" | "value"), ..] => Err(format!("{w}: wrong number of words")),
            [w, ..] => Err(format!("{w:?}: expected title, grid, code, tile or value")),
            [] => Ok(()),
        }
    }

    fn finish(mut self) -> Result<Layout, Vec<String>> {
        let mut errs = Vec::new();
        let Some((cols, rows)) = self.grid else {
            return Err(vec!["no grid line".into()]);
        };
        let mut taken: BTreeMap<(u8, u8), &str> = BTreeMap::new();
        for t in &self.tiles {
            if u16::from(t.col) + u16::from(t.w) > u16::from(cols)
                || u16::from(t.row) + u16::from(t.h) > u16::from(rows)
            {
                errs.push(format!("tile {} does not fit the {cols} by {rows} grid", t.id));
                continue;
            }
            if !self.sets.contains_key(&t.codes) {
                errs.push(format!("tile {}: no code set {}", t.id, t.codes));
            }
            for c in t.col..t.col + t.w {
                for r in t.row..t.row + t.h {
                    if let Some(other) = taken.insert((c, r), &t.id)
                        && other != t.id
                    {
                        errs.push(format!("tiles {other} and {} overlap at {c},{r}", t.id));
                    }
                }
            }
        }
        for (id, v) in std::mem::take(&mut self.values) {
            match self.tiles.iter_mut().find(|t| t.id == id) {
                Some(t) => t.value = Some(v),
                None => errs.push(format!("value for tile {id}, which is not declared")),
            }
        }
        if !errs.is_empty() {
            errs.dedup();
            return Err(errs);
        }
        Ok(Layout {
            title: self.title,
            cols,
            rows,
            sets: self.sets,
            tiles: self.tiles,
        })
    }
}

/// Writes a layout file that [`parse`] reads back as `l`.
#[must_use]
pub fn render(l: &Layout) -> String {
    let mut s = format!("{HEADER}\n");
    if let Some(t) = &l.title {
        let _ = writeln!(s, "title {t}");
    }
    let _ = writeln!(s, "grid {} {}", l.cols, l.rows);
    for (set, codes) in &l.sets {
        for (n, st) in codes {
            let _ = writeln!(s, "code {set} {n} {} {}", st.colour.name(), st.text);
        }
    }
    for t in &l.tiles {
        let _ = write!(s, "tile {} {},{}", t.id, t.col, t.row);
        if (t.w, t.h) != (1, 1) {
            let _ = write!(s, ",{},{}", t.w, t.h);
        }
        let _ = writeln!(s, " {} {} {}", t.codes, t.state, t.title);
    }
    for t in &l.tiles {
        if let Some(v) = &t.value {
            let _ = writeln!(s, "value {} {} {}", t.id, v.signal, v.label);
        }
    }
    s
}
