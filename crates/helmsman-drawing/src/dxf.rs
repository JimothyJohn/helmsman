//! The ASCII DXF reader: group-code pairs into a [`Drawing`].
//!
//! Read: the HEADER's `$ACADVER` and `$INSUNITS`; BLOCKS (each block's
//! base point, entities and `ATTDEF`s); and in ENTITIES and in blocks
//! `INSERT` with its `ATTRIB`s, `TEXT`, `MTEXT`, `LINE`, `LWPOLYLINE`.
//! Every other entity type is counted by name in [`Drawing::unsupported`]
//! and every other section named in [`Drawing::sections_not_read`].
//!
//! Strictness: a file that does not end in `0 / EOF`, ends between a group
//! code and its value, has a group code that is not an integer in
//! 0..=1071, or a number that is not finite and within +-1e15 where one is
//! read, is an error. Binary DXF is [`DxfError::Binary`]. Line endings
//! may be LF, CRLF or CR. Bytes that are not UTF-8 are read as Latin-1
//! (what DXF before R2007 mostly is) and [`Drawing::latin1`] says so.

use std::collections::BTreeMap;
use std::fmt;

/// Largest input read, bytes (the library's fetcher has the same cap).
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
/// Most entities in a drawing, blocks included.
pub const MAX_ENTITIES: usize = 1_000_000;
/// Most blocks.
pub const MAX_BLOCKS: usize = 20_000;
/// Longest text kept, bytes; more is cut and noted.
pub const MAX_TEXT: usize = 1024;
/// Largest coordinate, scale or height read.
pub const MAX_COORD: f64 = 1e15;
const MAX_NOTES: usize = 64;

/// Why a file was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DxfError {
    /// Nothing that looks like DXF at the start (or nothing at all).
    NotDxf,
    /// Binary DXF (`AutoCAD Binary DXF`): not read.
    Binary,
    /// Longer than [`MAX_BYTES`].
    TooLarge,
    /// More than the stated number of entities or blocks.
    TooMany(&'static str),
    /// The file ends before `0 / EOF`, or between a group code and its value.
    Truncated { line: usize },
    /// A group code line that is not an integer in 0..=1071.
    BadGroupCode { line: usize },
    /// A number where one was needed, but not a finite one in range.
    BadNumber { line: usize },
    /// Something the structure does not allow there.
    Malformed { line: usize, what: &'static str },
}

impl fmt::Display for DxfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotDxf => write!(f, "not an ASCII DXF file"),
            Self::Binary => write!(f, "binary DXF is not read; save the drawing as ASCII DXF"),
            Self::TooLarge => write!(f, "the file is over {MAX_BYTES} bytes"),
            Self::TooMany(what) => write!(f, "too many {what}"),
            Self::Truncated { line } => write!(f, "the file ends early (line {line})"),
            Self::BadGroupCode { line } => write!(f, "line {line}: not a group code"),
            Self::BadNumber { line } => write!(f, "line {line}: not a usable number"),
            Self::Malformed { line, what } => write!(f, "line {line}: {what}"),
        }
    }
}

impl std::error::Error for DxfError {}

/// One attribute of an inserted block (`ATTRIB`).
#[derive(Debug, Clone, PartialEq)]
pub struct Attrib {
    pub tag: String,
    pub value: String,
    pub x: f64,
    pub y: f64,
}

/// A block inserted (`INSERT`).
#[derive(Debug, Clone, PartialEq)]
pub struct Insert {
    pub block: String,
    pub layer: String,
    pub x: f64,
    pub y: f64,
    /// Degrees, as written.
    pub rotation: f64,
    pub sx: f64,
    pub sy: f64,
    pub attribs: Vec<Attrib>,
    pub line: usize,
}

/// `TEXT` or `MTEXT` (formatting codes removed from the latter).
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub text: String,
    pub layer: String,
    pub x: f64,
    pub y: f64,
    pub height: f64,
    pub rotation: f64,
    pub multiline: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub layer: String,
    pub from: (f64, f64),
    pub to: (f64, f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    pub layer: String,
    pub closed: bool,
    pub vertices: Vec<(f64, f64)>,
}

/// An attribute definition in a block: its tag and default value.
#[derive(Debug, Clone, PartialEq)]
pub struct AttDef {
    pub tag: String,
    pub default: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Entity {
    Insert(Insert),
    Text(Text),
    Line(Line),
    Polyline(Polyline),
    AttDef(AttDef),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub name: String,
    pub base: (f64, f64),
    pub entities: Vec<Entity>,
}

/// What was read from one file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawing {
    /// `$ACADVER`, such as `AC1027`.
    pub version: Option<String>,
    /// `$INSUNITS` (4 is millimetres).
    pub units: Option<i32>,
    /// Blocks by upper-cased name (DXF block names are case-insensitive).
    pub blocks: BTreeMap<String, Block>,
    /// The ENTITIES section, in file order.
    pub entities: Vec<Entity>,
    /// Entity types that were not read, with how many there were.
    pub unsupported: BTreeMap<String, usize>,
    /// Sections that were not read (`TABLES`, `OBJECTS`, ...).
    pub sections_not_read: Vec<String>,
    /// Oddities that were tolerated (cut texts, an array insert, ...).
    pub notes: Vec<String>,
    /// The bytes were not UTF-8 and were read as Latin-1.
    pub latin1: bool,
}

impl Drawing {
    /// A block by name, any case.
    #[must_use]
    pub fn block(&self, name: &str) -> Option<&Block> {
        self.blocks.get(&name.trim().to_uppercase())
    }

    fn note(&mut self, s: String) {
        match self.notes.len().cmp(&MAX_NOTES) {
            std::cmp::Ordering::Less => self.notes.push(s),
            std::cmp::Ordering::Equal => self.notes.push("more notes were dropped".into()),
            std::cmp::Ordering::Greater => {}
        }
    }
}

/// Read an ASCII DXF file.
///
/// # Errors
/// [`DxfError`] for binary DXF, a file that is not DXF, is over the
/// limits, is truncated or is malformed.
pub fn read(bytes: &[u8]) -> Result<Drawing, DxfError> {
    if bytes.len() > MAX_BYTES {
        return Err(DxfError::TooLarge);
    }
    if bytes.starts_with(b"AutoCAD Binary DXF") {
        return Err(DxfError::Binary);
    }
    let mut d = Drawing::default();
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let owned;
    let text: &str = if let Ok(s) = std::str::from_utf8(bytes) {
        s
    } else {
        d.latin1 = true;
        owned = bytes.iter().map(|&b| b as char).collect::<String>();
        &owned
    };
    let mut p = Parser {
        r: Reader::new(text),
        d,
        entities_seen: 0,
    };
    p.file()?;
    Ok(p.d)
}

struct Pair<'a> {
    code: i32,
    value: &'a str,
    line: usize,
}

struct Reader<'a> {
    s: &'a str,
    pos: usize,
    line: usize,
    peeked: Option<Pair<'a>>,
}

impl<'a> Reader<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            s,
            pos: 0,
            line: 0,
            peeked: None,
        }
    }

    /// The next line, split at LF, CRLF or CR; no empty line after the last terminator.
    fn line(&mut self) -> Option<&'a str> {
        if self.pos >= self.s.len() {
            return None;
        }
        let rest = &self.s[self.pos..];
        let end = rest.find(['\n', '\r']).unwrap_or(rest.len());
        let out = &rest[..end];
        let mut adv = end;
        let b = rest.as_bytes();
        if adv < b.len() {
            adv += if b[adv] == b'\r' && b.get(adv + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
        }
        self.pos += adv;
        self.line += 1;
        Some(out)
    }

    fn raw_pair(&mut self) -> Result<Option<Pair<'a>>, DxfError> {
        let Some(c) = self.line() else { return Ok(None) };
        let line = self.line;
        let code: i32 = c.trim().parse().map_err(|_| DxfError::BadGroupCode { line })?;
        if !(0..=1071).contains(&code) {
            return Err(DxfError::BadGroupCode { line });
        }
        let value = self.line().ok_or(DxfError::Truncated { line: self.line })?;
        Ok(Some(Pair {
            code,
            value,
            line: line + 1,
        }))
    }

    /// The next pair, comments (999) skipped.
    fn next(&mut self) -> Result<Option<Pair<'a>>, DxfError> {
        if let Some(p) = self.peeked.take() {
            return Ok(Some(p));
        }
        loop {
            match self.raw_pair()? {
                Some(p) if p.code == 999 => {}
                other => return Ok(other),
            }
        }
    }

    fn peek(&mut self) -> Result<Option<&Pair<'a>>, DxfError> {
        if self.peeked.is_none() {
            self.peeked = self.next()?;
        }
        Ok(self.peeked.as_ref())
    }

    fn need(&mut self) -> Result<Pair<'a>, DxfError> {
        self.next()?.ok_or(DxfError::Truncated { line: self.line })
    }
}

fn num(v: &str, line: usize) -> Result<f64, DxfError> {
    let x: f64 = v.trim().parse().map_err(|_| DxfError::BadNumber { line })?;
    if x.is_finite() && x.abs() <= MAX_COORD {
        Ok(x)
    } else {
        Err(DxfError::BadNumber { line })
    }
}

fn int(v: &str, line: usize) -> Result<i64, DxfError> {
    v.trim().parse().map_err(|_| DxfError::BadNumber { line })
}

type Body<'a> = Vec<(i32, &'a str, usize)>;

fn last<'a>(b: &Body<'a>, code: i32) -> Option<(&'a str, usize)> {
    b.iter().rev().find(|p| p.0 == code).map(|p| (p.1, p.2))
}

fn fnum(b: &Body<'_>, code: i32, default: f64) -> Result<f64, DxfError> {
    last(b, code).map_or(Ok(default), |(v, l)| num(v, l))
}

struct Parser<'a> {
    r: Reader<'a>,
    d: Drawing,
    entities_seen: usize,
}

impl<'a> Parser<'a> {
    fn str_of(&mut self, v: &str) -> String {
        let v = v.trim();
        if v.len() <= MAX_TEXT {
            return v.to_owned();
        }
        let mut n = MAX_TEXT;
        while !v.is_char_boundary(n) {
            n -= 1;
        }
        self.d
            .note(format!("a text of {} bytes was cut to {MAX_TEXT}", v.len()));
        v[..n].to_owned()
    }

    fn sval(&mut self, b: &Body<'_>, code: i32) -> String {
        last(b, code).map_or_else(String::new, |(v, _)| self.str_of(v))
    }

    fn file(&mut self) -> Result<(), DxfError> {
        let first = self.r.next()?.ok_or(DxfError::NotDxf)?;
        if first.code != 0 || !matches!(first.value.trim(), "SECTION" | "EOF") {
            return Err(DxfError::NotDxf);
        }
        self.r.peeked = Some(first);
        loop {
            let p = self.r.need()?;
            if p.code != 0 {
                return Err(DxfError::Malformed {
                    line: p.line,
                    what: "expected SECTION or EOF",
                });
            }
            match p.value.trim() {
                "EOF" => return Ok(()),
                "SECTION" => self.section()?,
                _ => {
                    return Err(DxfError::Malformed {
                        line: p.line,
                        what: "expected SECTION or EOF",
                    });
                }
            }
        }
    }

    fn section(&mut self) -> Result<(), DxfError> {
        let n = self.r.need()?;
        if n.code != 2 {
            return Err(DxfError::Malformed {
                line: n.line,
                what: "a section has no name",
            });
        }
        match n.value.trim().to_ascii_uppercase().as_str() {
            "HEADER" => self.header(),
            "BLOCKS" => self.blocks(),
            "ENTITIES" => {
                let e = self.entities("ENDSEC")?;
                self.d.entities = e;
                Ok(())
            }
            other => {
                let name: String = other
                    .chars()
                    .filter(|c| c.is_ascii_graphic() || *c == ' ')
                    .take(64)
                    .collect();
                self.d.sections_not_read.push(name);
                loop {
                    let p = self.r.need()?;
                    if p.code == 0 && p.value.trim() == "ENDSEC" {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn header(&mut self) -> Result<(), DxfError> {
        let mut var = String::new();
        loop {
            let p = self.r.need()?;
            match p.code {
                0 if p.value.trim() == "ENDSEC" => return Ok(()),
                9 => var = p.value.trim().to_ascii_uppercase(),
                1 if var == "$ACADVER" => {
                    let v = self.str_of(p.value);
                    self.d.version = Some(v);
                }
                70 if var == "$INSUNITS" => {
                    self.d.units = i32::try_from(int(p.value, p.line)?).ok();
                }
                _ => {}
            }
        }
    }

    fn blocks(&mut self) -> Result<(), DxfError> {
        loop {
            let p = self.r.need()?;
            if p.code != 0 {
                return Err(DxfError::Malformed {
                    line: p.line,
                    what: "expected BLOCK or ENDSEC",
                });
            }
            match p.value.trim() {
                "ENDSEC" => return Ok(()),
                "BLOCK" => {
                    let body = self.body()?;
                    let name = self.sval(&body, 2);
                    let base = (fnum(&body, 10, 0.0)?, fnum(&body, 20, 0.0)?);
                    let entities = self.entities("ENDBLK")?;
                    if self.d.blocks.len() >= MAX_BLOCKS {
                        return Err(DxfError::TooMany("blocks"));
                    }
                    let key = name.to_uppercase();
                    if self.d.blocks.contains_key(&key) {
                        self.d
                            .note(format!("block {name} is defined twice; the later one is used"));
                    }
                    self.d.blocks.insert(key, Block { name, base, entities });
                }
                _ => {
                    return Err(DxfError::Malformed {
                        line: p.line,
                        what: "expected BLOCK or ENDSEC",
                    });
                }
            }
        }
    }

    /// The pairs up to the next group-code-0 pair.
    fn body(&mut self) -> Result<Body<'a>, DxfError> {
        let mut b = Vec::new();
        while let Some(q) = self.r.peek()? {
            if q.code == 0 {
                break;
            }
            let q = self.r.need()?;
            b.push((q.code, q.value, q.line));
        }
        Ok(b)
    }

    fn unsupported(&mut self, kind: &str) {
        let name: String = kind
            .chars()
            .filter(|c| c.is_ascii_graphic() || *c == ' ')
            .take(64)
            .collect();
        *self.d.unsupported.entry(name).or_insert(0) += 1;
    }

    /// Entities up to `end` (`ENDSEC` or `ENDBLK`), whose body is consumed too.
    fn entities(&mut self, end: &str) -> Result<Vec<Entity>, DxfError> {
        #[derive(PartialEq)]
        enum Open {
            No,
            Insert,
            Polyline,
        }
        let mut out: Vec<Entity> = Vec::new();
        let mut open = Open::No;
        loop {
            let p = self.r.need()?;
            if p.code != 0 {
                return Err(DxfError::Malformed {
                    line: p.line,
                    what: "expected an entity",
                });
            }
            let kind = p.value.trim().to_ascii_uppercase();
            let body = self.body()?;
            if kind == end {
                return Ok(out);
            }
            if kind == "EOF" || kind == "ENDSEC" || kind == "SECTION" {
                return Err(DxfError::Malformed {
                    line: p.line,
                    what: "the section ends early",
                });
            }
            self.entities_seen += 1;
            if self.entities_seen > MAX_ENTITIES {
                return Err(DxfError::TooMany("entities"));
            }
            if open == Open::Insert && kind != "ATTRIB" && kind != "SEQEND" {
                open = Open::No;
            }
            match kind.as_str() {
                "INSERT" => {
                    let ins = self.insert(&body, p.line)?;
                    open = if last(&body, 66).is_some_and(|(v, _)| v.trim() == "1") {
                        Open::Insert
                    } else {
                        Open::No
                    };
                    out.push(Entity::Insert(ins));
                }
                "ATTRIB" => {
                    let a = Attrib {
                        tag: self.sval(&body, 2),
                        value: self.sval(&body, 1),
                        x: fnum(&body, 10, 0.0)?,
                        y: fnum(&body, 20, 0.0)?,
                    };
                    if let (Open::Insert, Some(Entity::Insert(i))) = (&open, out.last_mut()) {
                        i.attribs.push(a);
                    } else {
                        self.unsupported("ATTRIB (outside an INSERT)");
                    }
                }
                "SEQEND" => open = Open::No,
                "POLYLINE" => {
                    self.unsupported("POLYLINE");
                    open = Open::Polyline;
                }
                "VERTEX" if open == Open::Polyline => {}
                "ATTDEF" => out.push(Entity::AttDef(AttDef {
                    tag: self.sval(&body, 2),
                    default: self.sval(&body, 1),
                })),
                "TEXT" => out.push(Entity::Text(self.text(&body, false)?)),
                "MTEXT" => out.push(Entity::Text(self.text(&body, true)?)),
                "LINE" => out.push(Entity::Line(Line {
                    layer: self.sval(&body, 8),
                    from: (fnum(&body, 10, 0.0)?, fnum(&body, 20, 0.0)?),
                    to: (fnum(&body, 11, 0.0)?, fnum(&body, 21, 0.0)?),
                })),
                "LWPOLYLINE" => out.push(Entity::Polyline(self.lwpolyline(&body)?)),
                _ => self.unsupported(&kind),
            }
        }
    }

    fn insert(&mut self, b: &Body<'_>, line: usize) -> Result<Insert, DxfError> {
        let cols = last(b, 70).map_or(Ok(1), |(v, l)| int(v, l))?;
        let rows = last(b, 71).map_or(Ok(1), |(v, l)| int(v, l))?;
        if cols > 1 || rows > 1 {
            self.d.note(format!(
                "line {line}: an array insert ({cols} x {rows}) is read as its first part only"
            ));
        }
        Ok(Insert {
            block: self.sval(b, 2),
            layer: self.sval(b, 8),
            x: fnum(b, 10, 0.0)?,
            y: fnum(b, 20, 0.0)?,
            rotation: fnum(b, 50, 0.0)?,
            sx: fnum(b, 41, 1.0)?,
            sy: fnum(b, 42, 1.0)?,
            attribs: Vec::new(),
            line,
        })
    }

    fn text(&mut self, b: &Body<'_>, multiline: bool) -> Result<Text, DxfError> {
        let raw = if multiline {
            let mut s = String::new();
            for (c, v, _) in b {
                if *c == 3 && s.len() <= MAX_TEXT * 4 {
                    s.push_str(v);
                }
            }
            if let Some((v, _)) = last(b, 1) {
                s.push_str(v);
            }
            strip_mtext(&s)
        } else {
            last(b, 1).map_or_else(String::new, |(v, _)| v.to_owned())
        };
        let aligned = !multiline
            && (last(b, 72).is_some_and(|(v, _)| v.trim() != "0")
                || last(b, 73).is_some_and(|(v, _)| v.trim() != "0"))
            && last(b, 11).is_some();
        let (x, y) = if aligned {
            (fnum(b, 11, 0.0)?, fnum(b, 21, 0.0)?)
        } else {
            (fnum(b, 10, 0.0)?, fnum(b, 20, 0.0)?)
        };
        Ok(Text {
            text: self.str_of(&raw),
            layer: self.sval(b, 8),
            x,
            y,
            height: fnum(b, 40, 0.0)?,
            rotation: fnum(b, 50, 0.0)?,
            multiline,
        })
    }

    fn lwpolyline(&mut self, b: &Body<'_>) -> Result<Polyline, DxfError> {
        let mut vertices = Vec::new();
        let mut pending: Option<f64> = None;
        for &(c, v, l) in b {
            match c {
                10 => pending = Some(num(v, l)?),
                20 => {
                    let x = pending.take().ok_or(DxfError::Malformed {
                        line: l,
                        what: "a vertex has y and no x",
                    })?;
                    vertices.push((x, num(v, l)?));
                }
                _ => {}
            }
        }
        let closed = last(b, 70).map_or(Ok(0), |(v, l)| int(v, l))? & 1 == 1;
        Ok(Polyline {
            layer: self.sval(b, 8),
            closed,
            vertices,
        })
    }
}

/// MTEXT without its formatting: `\P` is a space, `\\` `\{` `\}` are the
/// character, `\X...;` codes and the braces are dropped.
fn strip_mtext(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\\' => match it.next() {
                Some('P' | 'p' | 'N' | '~') => out.push(' '),
                Some(e @ ('\\' | '{' | '}')) => out.push(e),
                Some('U') => {
                    // \U+XXXX: kept as written, it is not decoded here.
                    out.push_str("\\U");
                }
                Some('L' | 'l' | 'O' | 'o' | 'K' | 'k') | None => {}
                Some(_) => {
                    for n in it.by_ref() {
                        if n == ';' {
                            break;
                        }
                    }
                }
            },
            '{' | '}' => {}
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
