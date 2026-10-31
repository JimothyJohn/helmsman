//! Electronic Data Sheets: the text file a vendor ships with an
//! EtherNet/IP device to say what it is and how to connect to it.
//!
//! The ODVA specification of the format is not in this
//! repo. What this module relies on, and where it comes from
//! (`docs/SOURCES.md`, "Device sources"):
//!
//! - **The lexical form** is read off two real files: `OpENer`'s
//!   `data/opener_sample_app.eds` (commit `0e1ab7e`) and Moxa's ioLogik
//!   E1213 EDS (`moxa-iologik-e1213-e1213-t-eds-file-software-package-v2.0.eds`,
//!   SHA-256 `3895fbca…d470d`). Both are `[Section]` headers followed by
//!   `Keyword = field, field, ... ;` entries that run across lines until
//!   the `;`; `$` starts a comment to the end of the line; fields may be
//!   empty (`,,`); strings are double-quoted and adjacent strings join
//!   (the E1213's `IconContents` is 377 quoted lines making one value).
//! - **Which field means what** in `ParamN`, `AssemN` and `ConnectionN`
//!   comes from the comments both generators (EZ-EDS 3.23 and 3.27) print
//!   beside each field, e.g. `$ O->T RPI, size, format` and the bit
//!   layout of the two connection words.
//! - **Inferred, not read anywhere:** the backslash escapes `\\`, `\"`,
//!   `\n`, `\t` inside strings (neither file uses one; any other backslash
//!   is kept as written); that keywords and section names compare without
//!   regard to case; and the meaning of the real-time format codes (see
//!   [`RealTimeFormat`]).
//!
//! The parser is strict about structure (an entry outside a section, a
//! missing `;`, a key or section given twice, a string that runs off the
//! end of its line and stray text between fields are all errors, with the
//! line) and bounded ([`MAX_TEXT`], [`MAX_FIELDS`]). It never panics,
//! whatever the input: `tests/eds.rs` checks that on arbitrary text.

use std::fmt;
use std::fmt::Write as _;
use std::time::Duration;

use crate::codec::Reader;
use crate::io::ConnectionPath;

/// Largest EDS accepted, in bytes. The E1213's is 42 KB, most of it an icon.
pub const MAX_TEXT: usize = 1 << 20;
/// Most fields in one entry. `OpENer`'s 32-member assemblies have 70.
pub const MAX_FIELDS: usize = 4096;

/// Why an EDS did not parse, and on which line (1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub what: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EDS line {}: {}", self.line, self.what)
    }
}

impl std::error::Error for Error {}

fn err<T>(line: usize, what: impl Into<String>) -> Result<T, Error> {
    Err(Error {
        line,
        what: what.into(),
    })
}

/// One field of an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    /// Nothing between the separators.
    Empty,
    /// A quoted string (adjacent strings joined, escapes resolved).
    Str(String),
    /// Anything else, trimmed: a number, a date, `Param3`, `EtherNetIP`.
    Word(String),
}

impl Field {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// The text of a word or a string.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Empty => None,
            Self::Str(s) | Self::Word(s) => Some(s),
        }
    }

    /// An unsigned integer word: decimal or `0x` hex.
    #[must_use]
    pub fn uint(&self) -> Option<u64> {
        let Self::Word(w) = self else { return None };
        parse_uint(w)
    }

    /// A signed integer word: optional `-`, then decimal or `0x` hex.
    #[must_use]
    pub fn int(&self) -> Option<i64> {
        let Self::Word(w) = self else { return None };
        match w.strip_prefix('-') {
            Some(rest) => parse_uint(rest).and_then(|v| i64::try_from(v).ok()).map(|v| -v),
            None => parse_uint(w).and_then(|v| i64::try_from(v).ok()),
        }
    }

    /// `PREFIXn` (`Param3`, `Assem101`, case-insensitive prefix): `n`.
    #[must_use]
    pub fn reference(&self, prefix: &str) -> Option<u32> {
        let Self::Word(w) = self else { return None };
        keyword_number(w, prefix)
    }
}

fn parse_uint(w: &str) -> Option<u64> {
    if let Some(hex) = w.strip_prefix("0x").or_else(|| w.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u64::from_str_radix(hex, 16).ok()
    } else if !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()) {
        w.parse().ok()
    } else {
        None
    }
}

/// `Param12` with prefix `Param` is `Some(12)`. Digits only after the
/// prefix, no sign, no leading `+`; must fit `u32`.
fn keyword_number(w: &str, prefix: &str) -> Option<u32> {
    let head = w.get(..prefix.len())?;
    if !head.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let digits = &w[prefix.len()..];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// `Keyword = fields;`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub fields: Vec<Field>,
    /// Line the keyword is on.
    pub line: usize,
}

/// `[Name]` and its entries, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub entries: Vec<Entry>,
    pub line: usize,
}

impl Section {
    /// The entry called `key` (case-insensitive).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.key.eq_ignore_ascii_case(key))
    }

    /// Entries named `PREFIXn`, with `n`, in file order.
    pub fn numbered<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = (u32, &'a Entry)> + 'a {
        self.entries
            .iter()
            .filter_map(move |e| keyword_number(&e.key, prefix).map(|n| (n, e)))
    }
}

/// A parsed EDS: its sections, in file order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Eds {
    pub sections: Vec<Section>,
}

struct Lexer<'a> {
    s: &'a [u8],
    pos: usize,
    line: usize,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.pos += 1;
        if c == b'\n' {
            self.line += 1;
        }
        Some(c)
    }

    fn skip_comment(&mut self) {
        while let Some(c) = self.peek() {
            if c == b'\n' {
                break;
            }
            self.pos += 1;
        }
    }

    /// Whitespace, newlines and comments.
    fn skip_blank(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.bump();
                }
                b'$' => self.skip_comment(),
                _ => break,
            }
        }
    }

    /// Spaces and a comment, not past the end of the line.
    fn skip_inline(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'$' => self.skip_comment(),
                _ => break,
            }
        }
    }

    fn section(&mut self) -> Result<String, Error> {
        let line = self.line;
        self.bump(); // '['
        let start = self.pos;
        loop {
            match self.peek() {
                Some(b']') => break,
                Some(b'\n') | None => return err(line, "section header without ]"),
                Some(b'[') => return err(line, "[ inside a section name"),
                Some(_) => self.pos += 1,
            }
        }
        let name = String::from_utf8_lossy(&self.s[start..self.pos])
            .trim()
            .to_string();
        self.pos += 1; // ']'
        if name.is_empty() {
            return err(line, "empty section name");
        }
        self.skip_inline();
        match self.peek() {
            Some(b'\n') | None => Ok(name),
            Some(_) => err(line, format!("text after [{name}]")),
        }
    }

    fn keyword(&mut self) -> Result<String, Error> {
        let line = self.line;
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            let shown = self
                .peek()
                .map_or_else(|| "end of file".to_string(), |c| format!("{:?}", char::from(c)));
            return err(line, format!("expected a keyword, found {shown}"));
        }
        let key = String::from_utf8_lossy(&self.s[start..self.pos]).into_owned();
        self.skip_inline();
        if self.peek() != Some(b'=') {
            return err(line, format!("{key}: expected ="));
        }
        self.pos += 1;
        Ok(key)
    }

    fn string(&mut self) -> Result<String, Error> {
        let line = self.line;
        self.pos += 1; // opening quote
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => return err(line, "string runs past the end of its line"),
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'\\') => out.push(b'\\'),
                        Some(b'"') => out.push(b'"'),
                        Some(b'n') => out.push(b'\n'),
                        Some(b't') => out.push(b'\t'),
                        // Unknown escape: keep both characters as written.
                        _ => {
                            out.push(b'\\');
                            continue;
                        }
                    }
                    self.pos += 1;
                }
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
        // The input is &str and the slice boundaries are ASCII, so this is
        // valid UTF-8; lossy only to stay panic-free.
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn field(&mut self) -> Result<Field, Error> {
        self.skip_blank();
        match self.peek() {
            Some(b'"') => {
                let mut s = self.string()?;
                loop {
                    self.skip_blank();
                    if self.peek() == Some(b'"') {
                        s.push_str(&self.string()?);
                    } else {
                        break;
                    }
                }
                Ok(Field::Str(s))
            }
            Some(b',' | b';') | None => Ok(Field::Empty),
            Some(_) => {
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if matches!(c, b',' | b';' | b'$' | b'"' | b'\n') {
                        break;
                    }
                    self.pos += 1;
                }
                let w = String::from_utf8_lossy(&self.s[start..self.pos])
                    .trim()
                    .to_string();
                Ok(if w.is_empty() {
                    Field::Empty
                } else {
                    Field::Word(w)
                })
            }
        }
    }

    fn entry(&mut self) -> Result<Entry, Error> {
        let line = self.line;
        let key = self.keyword()?;
        let mut fields = Vec::new();
        loop {
            let f = self.field()?;
            if fields.len() == MAX_FIELDS {
                return err(line, format!("{key}: more than {MAX_FIELDS} fields"));
            }
            fields.push(f);
            self.skip_blank();
            match self.bump() {
                Some(b',') => {}
                Some(b';') => break,
                Some(c) => {
                    return err(
                        self.line,
                        format!("{key}: expected , or ; after a field, found {:?}", char::from(c)),
                    );
                }
                None => return err(line, format!("{key}: no ; before the end of the file")),
            }
        }
        // `Key = ;` is one empty field; keep it as no fields.
        if fields == [Field::Empty] {
            fields.clear();
        }
        Ok(Entry { key, fields, line })
    }
}

impl Eds {
    /// Parses EDS text.
    ///
    /// # Errors
    /// The first structural problem, with its line.
    pub fn parse(text: &str) -> Result<Self, Error> {
        if text.len() > MAX_TEXT {
            return err(1, format!("larger than {MAX_TEXT} bytes"));
        }
        if let Some((n, _)) = text
            .lines()
            .enumerate()
            .find(|(_, l)| l.chars().any(|c| c.is_control() && !matches!(c, '\t' | '\r')))
        {
            return err(n + 1, "control character");
        }
        let mut lx = Lexer {
            s: text.as_bytes(),
            pos: 0,
            line: 1,
        };
        let mut eds = Self::default();
        loop {
            lx.skip_blank();
            match lx.peek() {
                None => break,
                Some(b'[') => {
                    let line = lx.line;
                    let name = lx.section()?;
                    if eds.section(&name).is_some() {
                        return err(line, format!("section [{name}] given twice"));
                    }
                    eds.sections.push(Section {
                        name,
                        entries: Vec::new(),
                        line,
                    });
                }
                Some(_) => {
                    let line = lx.line;
                    let e = lx.entry()?;
                    let Some(section) = eds.sections.last_mut() else {
                        return err(line, format!("{} is outside any [section]", e.key));
                    };
                    if section.get(&e.key).is_some() {
                        return err(line, format!("{} given twice in [{}]", e.key, section.name));
                    }
                    section.entries.push(e);
                }
            }
        }
        Ok(eds)
    }

    /// The section called `name` (case-insensitive).
    #[must_use]
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Writes the EDS back as text: one entry per line, comments gone.
    /// [`Eds::parse`] of the result gives the same sections and fields.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for sec in &self.sections {
            let _ = writeln!(s, "[{}]", sec.name);
            for e in &sec.entries {
                let _ = write!(s, "    {} =", e.key);
                for (i, f) in e.fields.iter().enumerate() {
                    if i > 0 {
                        s.push(',');
                    }
                    match f {
                        Field::Empty => {}
                        Field::Word(w) => {
                            let _ = write!(s, " {w}");
                        }
                        Field::Str(t) => {
                            s.push_str(" \"");
                            for c in t.chars() {
                                match c {
                                    '\\' => s.push_str("\\\\"),
                                    '"' => s.push_str("\\\""),
                                    '\n' => s.push_str("\\n"),
                                    '\t' => s.push_str("\\t"),
                                    c => s.push(c),
                                }
                            }
                            s.push('"');
                        }
                    }
                }
                s.push_str(";\n");
            }
            s.push('\n');
        }
        s
    }

    fn need(&self, name: &str) -> Result<&Section, Error> {
        self.section(name)
            .map_or_else(|| err(0, format!("no [{name}] section")), Ok)
    }

    /// `[Device]`: who made it and what it is.
    ///
    /// # Errors
    /// A missing `[Device]` or a missing or out-of-range number.
    pub fn identity(&self) -> Result<Identity, Error> {
        let d = self.need("Device")?;
        let num = |key: &str, max: u64| -> Result<u64, Error> {
            let e = d
                .get(key)
                .map_or_else(|| err(d.line, format!("[Device] has no {key}")), Ok)?;
            match e.fields.as_slice() {
                [f] => f
                    .uint()
                    .filter(|v| *v <= max)
                    .map_or_else(|| err(e.line, format!("{key}: want a number up to {max}")), Ok),
                _ => err(e.line, format!("{key}: want one value")),
            }
        };
        let text = |key: &str| -> Option<String> {
            d.get(key)
                .and_then(|e| e.fields.first())
                .and_then(Field::text)
                .map(str::to_string)
        };
        let n16 = |key: &str| num(key, u64::from(u16::MAX)).map(|v| u16::try_from(v).unwrap_or(0));
        let n8 = |key: &str| num(key, u64::from(u8::MAX)).map(|v| u8::try_from(v).unwrap_or(0));
        Ok(Identity {
            vendor_id: n16("VendCode")?,
            device_type: n16("ProdType")?,
            product_code: n16("ProdCode")?,
            major_revision: n8("MajRev")?,
            minor_revision: n8("MinRev")?,
            vendor_name: text("VendName"),
            product_name: text("ProdName"),
            catalog: text("Catalog"),
        })
    }

    /// `[Params]` `ParamN`.
    ///
    /// # Errors
    /// No such parameter, or one whose fields are not what the format
    /// says.
    pub fn param(&self, n: u32) -> Result<Param, Error> {
        let p = self.need("Params")?;
        let (_, e) = p
            .numbered("Param")
            .find(|(k, _)| *k == n)
            .map_or_else(|| err(p.line, format!("no Param{n} in [Params]")), Ok)?;
        Param::from_entry(n, e)
    }

    /// `[Assembly]` `AssemN`.
    ///
    /// # Errors
    /// No such assembly, or one with a bad size or members that don't
    /// add up to it.
    pub fn assembly(&self, n: u32) -> Result<Assembly, Error> {
        let a = self.need("Assembly")?;
        let (_, e) = a
            .numbered("Assem")
            .find(|(k, _)| *k == n)
            .map_or_else(|| err(a.line, format!("no Assem{n} in [Assembly]")), Ok)?;
        Assembly::from_entry(n, e)
    }

    /// Every `AssemN`.
    ///
    /// # Errors
    /// As [`Self::assembly`], for the first bad one. No `[Assembly]` is
    /// no assemblies.
    pub fn assemblies(&self) -> Result<Vec<Assembly>, Error> {
        self.section("Assembly").map_or_else(
            || Ok(Vec::new()),
            |a| {
                a.numbered("Assem")
                    .map(|(n, e)| Assembly::from_entry(n, e))
                    .collect()
            },
        )
    }

    /// Every `[Connection Manager]` `ConnectionN`, resolved against the
    /// params and assemblies it names.
    ///
    /// # Errors
    /// The first connection that does not resolve (see
    /// [`Connection::from_entry`]).
    pub fn connections(&self) -> Result<Vec<Connection>, Error> {
        let cm = self.need("Connection Manager")?;
        cm.numbered("Connection")
            .map(|(n, e)| Connection::from_entry(self, n, e))
            .collect()
    }
}

/// `[Device]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub vendor_id: u16,
    pub device_type: u16,
    pub product_code: u16,
    pub major_revision: u8,
    pub minor_revision: u8,
    pub vendor_name: Option<String>,
    pub product_name: Option<String>,
    pub catalog: Option<String>,
}

/// A `ParamN`. Field order per the generators' comments: reserved, link
/// path size, link path, descriptor, data type, data size, name, units,
/// help, min, max, default, then scaling, links and decimal places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub number: u32,
    /// CIP elementary type code (0xC7 UINT, 0xC8 UDINT, ...).
    pub data_type: u8,
    /// Bytes.
    pub data_size: u8,
    pub name: String,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub default: Option<i64>,
}

/// CIP `UINT`.
pub const UINT: u8 = 0xC7;
/// CIP `UDINT`.
pub const UDINT: u8 = 0xC8;

impl Param {
    fn from_entry(number: u32, e: &Entry) -> Result<Self, Error> {
        let f = &e.fields;
        if f.len() < 12 {
            return err(
                e.line,
                format!("Param{number}: {} fields, want at least 12", f.len()),
            );
        }
        let data_type = f[4].uint().and_then(|v| u8::try_from(v).ok()).map_or_else(
            || err(e.line, format!("Param{number}: data type is not a byte")),
            Ok,
        )?;
        let data_size = f[5].uint().and_then(|v| u8::try_from(v).ok()).map_or_else(
            || err(e.line, format!("Param{number}: data size is not a byte")),
            Ok,
        )?;
        let name = match &f[6] {
            Field::Str(s) => s.clone(),
            _ => return err(e.line, format!("Param{number}: name is not a string")),
        };
        // Numeric limits are only read as integers; a REAL param keeps
        // them as None, which is fine for what this module resolves.
        let num = |x: &Field| x.int();
        let (min, max, default) = (num(&f[9]), num(&f[10]), num(&f[11]));
        if let (Some(lo), Some(hi)) = (min, max)
            && lo > hi
        {
            return err(e.line, format!("Param{number}: min {lo} > max {hi}"));
        }
        Ok(Self {
            number,
            data_type,
            data_size,
            name,
            min,
            max,
            default,
        })
    }
}

/// One member of an assembly: its size in bits and what it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub bits: Option<u32>,
    pub reference: Field,
}

/// An `AssemN`: name, path, size in bytes, descriptor, two reserved
/// fields, then `bits, reference` pairs (the comment-free layout both
/// generators write).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembly {
    pub instance: u16,
    pub name: String,
    pub size: Option<u16>,
    pub descriptor: Option<u16>,
    pub members: Vec<Member>,
    pub line: usize,
}

impl Assembly {
    fn from_entry(number: u32, e: &Entry) -> Result<Self, Error> {
        let bad = |why: String| err(e.line, format!("Assem{number}: {why}"));
        let Ok(instance) = u16::try_from(number) else {
            return bad("instance past 65535".into());
        };
        let f = &e.fields;
        if f.len() < 6 {
            return bad(format!("{} fields, want at least 6", f.len()));
        }
        let name = match &f[0] {
            Field::Str(s) => s.clone(),
            Field::Empty => String::new(),
            Field::Word(_) => return bad("name is not a string".into()),
        };
        let size = match &f[2] {
            Field::Empty => None,
            x => match x.uint().and_then(|v| u16::try_from(v).ok()) {
                Some(v) => Some(v),
                None => return bad(format!("size {x:?} is not a number up to 65535")),
            },
        };
        let descriptor = f[3].uint().and_then(|v| u16::try_from(v).ok());
        let rest = &f[6..];
        if rest.len() % 2 == 1 {
            return bad("members come in bits, reference pairs".into());
        }
        let mut members = Vec::with_capacity(rest.len() / 2);
        for pair in rest.as_chunks::<2>().0 {
            let bits = match &pair[0] {
                Field::Empty => None,
                x => match x.uint().and_then(|v| u32::try_from(v).ok()) {
                    Some(v) => Some(v),
                    None => return bad(format!("member size {x:?} is not a bit count")),
                },
            };
            members.push(Member {
                bits,
                reference: pair[1].clone(),
            });
        }
        if let Some(size) = size
            && !members.is_empty()
            && members.iter().all(|m| m.bits.is_some())
        {
            let total: u64 = members.iter().filter_map(|m| m.bits).map(u64::from).sum();
            if total != u64::from(size) * 8 {
                return bad(format!(
                    "members add up to {total} bits, the size says {size} bytes"
                ));
            }
        }
        Ok(Self {
            instance,
            name,
            size,
            descriptor,
            members,
            line: e.line,
        })
    }
}

/// Application types (bits 24–27 of a connection's first word).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AppType {
    ListenOnly,
    InputOnly,
    ExclusiveOwner,
    RedundantOwner,
}

/// What a direction's data carries besides the application bytes, from
/// bits 8–10 (O->T) and 12–14 (T->O) of a connection's second word.
///
/// Inferred: the codes' names are not printed in either EDS read. The
/// meaning comes from what `OpENer` does with the connections its own EDS
/// declares: its exclusive owner says O->T 4 and T->O 0, and `OpENer`
/// consumes O->T with a 32-bit run/idle header and produces T->O without
/// one (`OPENER_CONSUMED_DATA_HAS_RUN_IDLE_HEADER`); its input-only and
/// listen-only connections say O->T 3, with an O->T size of 0, a heartbeat.
/// Other codes are kept as [`RealTimeFormat::Other`] and refused by
/// anything that would have to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealTimeFormat {
    /// 0: the data alone.
    Modeless,
    /// 3: no data, the packet is the heartbeat.
    Heartbeat,
    /// 4: a 32-bit run/idle header before the data.
    RunIdle,
    Other(u8),
}

impl RealTimeFormat {
    const fn from_bits(v: u8) -> Self {
        match v {
            0 => Self::Modeless,
            3 => Self::Heartbeat,
            4 => Self::RunIdle,
            v => Self::Other(v),
        }
    }
}

/// An RPI range in microseconds, from a `ParamN` or a literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rpi {
    pub min: Option<Duration>,
    pub max: Option<Duration>,
    pub default: Option<Duration>,
}

impl Rpi {
    /// Whether `rpi` is inside the range (an open end allows anything).
    #[must_use]
    pub fn allows(&self, rpi: Duration) -> bool {
        self.min.is_none_or(|m| rpi >= m) && self.max.is_none_or(|m| rpi <= m)
    }
}

/// One direction of a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Direction {
    pub rpi: Rpi,
    /// Application bytes (no sequence count, no header).
    pub size: u16,
    pub format: RealTimeFormat,
    /// The `AssemN` the EDS names as the data's format, if any.
    pub assembly: Option<u16>,
    pub point_to_point: bool,
    pub multicast: bool,
}

/// A `ConnectionN`, resolved: what to put in a `Forward_Open`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub number: u32,
    pub name: String,
    pub line: usize,
    /// First word: transport classes (0–15), triggers (16–18),
    /// application types (24–27), direction (31).
    pub trigger_transport: u32,
    /// Second word: sizes, formats, connection types, priorities.
    pub params: u32,
    pub app_types: Vec<AppType>,
    /// Bit 1 of the first word: transport class 1.
    pub class1: bool,
    /// Bit 16: cyclic trigger.
    pub cyclic: bool,
    pub configuration: u16,
    /// Configuration data size the target wants (0 when none).
    pub config_size: u16,
    /// O->T connection point (the first point in the path).
    pub o_to_t_point: u16,
    /// T->O connection point (the second).
    pub t_to_o_point: u16,
    pub o_to_t: Direction,
    pub t_to_o: Direction,
}

/// Largest Class 1 packet a 16-bit `Forward_Open` can ask for: the
/// network parameters' size field is 9 bits.
pub const MAX_CLASS1_PACKET: u16 = 511;

fn hex_path(text: &str) -> Option<Vec<u8>> {
    text.split_ascii_whitespace()
        .map(|b| {
            if b.len() == 2 {
                u8::from_str_radix(b, 16).ok()
            } else {
                None
            }
        })
        .collect()
}

impl Connection {
    /// Resolves a `ConnectionN` entry: 15 fields (two words; O->T RPI,
    /// size, format; T->O RPI, size, format; proxy config size, format;
    /// target config size, format; name; help; path).
    ///
    /// Refused: a size or RPI naming a param that doesn't exist or isn't
    /// a number, a size that disagrees with the assembly the format names,
    /// a heartbeat with data, a packet over [`MAX_CLASS1_PACKET`], a path
    /// that is not literal hex bytes or does not decode as class,
    /// configuration instance and two connection points, a format
    /// assembly that is not the path's point, and an application class
    /// other than Assembly.
    ///
    /// # Errors
    /// Why, with the line.
    #[allow(clippy::too_many_lines)]
    pub fn from_entry(eds: &Eds, number: u32, e: &Entry) -> Result<Self, Error> {
        let line = e.line;
        let bad = |why: String| err(line, format!("Connection{number}: {why}"));
        let f = &e.fields;
        if f.len() != 15 {
            return bad(format!("{} fields, want 15", f.len()));
        }
        let word = |x: &Field, what: &str| -> Result<u32, Error> {
            x.uint().and_then(|v| u32::try_from(v).ok()).map_or_else(
                || err(line, format!("Connection{number}: {what} is not a 32-bit word")),
                Ok,
            )
        };
        let trigger_transport = word(&f[0], "trigger and transport")?;
        let params = word(&f[1], "connection parameters")?;
        let app_types = [
            (24, AppType::ListenOnly),
            (25, AppType::InputOnly),
            (26, AppType::ExclusiveOwner),
            (27, AppType::RedundantOwner),
        ]
        .into_iter()
        .filter(|(bit, _)| trigger_transport & (1 << bit) != 0)
        .map(|(_, t)| t)
        .collect();

        let rpi = |x: &Field| -> Result<Rpi, Error> {
            let us = |v: Option<i64>| -> Result<Option<Duration>, Error> {
                match v {
                    None => Ok(None),
                    Some(v) => u64::try_from(v)
                        .map(|v| Some(Duration::from_micros(v)))
                        .or_else(|_| err(line, format!("Connection{number}: negative RPI"))),
                }
            };
            match x {
                Field::Empty => Ok(Rpi {
                    min: None,
                    max: None,
                    default: None,
                }),
                x => {
                    if let Some(p) = x.reference("Param") {
                        let p = eds.param(p)?;
                        Ok(Rpi {
                            min: us(p.min)?,
                            max: us(p.max)?,
                            default: us(p.default)?,
                        })
                    } else if let Some(v) = x.uint() {
                        let d = Some(Duration::from_micros(v));
                        Ok(Rpi {
                            min: d,
                            max: d,
                            default: d,
                        })
                    } else {
                        err(
                            line,
                            format!("Connection{number}: RPI {x:?} is not a ParamN or a number"),
                        )
                    }
                }
            }
        };
        let assem = |x: &Field| -> Result<Option<Assembly>, Error> {
            match x {
                Field::Empty => Ok(None),
                x => match x.reference("Assem") {
                    Some(n) => eds.assembly(n).map(Some),
                    None => err(line, format!("Connection{number}: format {x:?} is not an AssemN")),
                },
            }
        };
        let size = |x: &Field, a: Option<&Assembly>, what: &str| -> Result<u16, Error> {
            let given = match x {
                Field::Empty => None,
                x => {
                    let v = if let Some(p) = x.reference("Param") {
                        let p = eds.param(p)?;
                        p.default.or(p.max).map_or_else(
                            || {
                                err(
                                    line,
                                    format!("Connection{number}: {what} size Param has no default"),
                                )
                            },
                            Ok,
                        )?
                    } else {
                        x.int().map_or_else(
                            || {
                                err(
                                    line,
                                    format!("Connection{number}: {what} size {x:?} is not a number"),
                                )
                            },
                            Ok,
                        )?
                    };
                    Some(u16::try_from(v).or_else(|_| {
                        err(line, format!("Connection{number}: {what} size {v} out of range"))
                    })?)
                }
            };
            match (given, a.and_then(|a| a.size)) {
                (Some(g), Some(s)) if g != s => err(
                    line,
                    format!("Connection{number}: {what} size {g} but its assembly is {s} bytes"),
                ),
                (Some(g), _) => Ok(g),
                (None, Some(s)) => Ok(s),
                (None, None) => err(
                    line,
                    format!("Connection{number}: {what} size given neither directly nor by its assembly"),
                ),
            }
        };
        let o_assem = assem(&f[4])?;
        let t_assem = assem(&f[7])?;
        let o_format = RealTimeFormat::from_bits(u8::try_from((params >> 8) & 7).unwrap_or(0));
        let t_format = RealTimeFormat::from_bits(u8::try_from((params >> 12) & 7).unwrap_or(0));
        let o_size = if o_format == RealTimeFormat::Heartbeat && f[3].is_empty() && o_assem.is_none() {
            0
        } else {
            size(&f[3], o_assem.as_ref(), "O->T")?
        };
        let t_size = size(&f[6], t_assem.as_ref(), "T->O")?;
        for (fmt, n, what) in [(o_format, o_size, "O->T"), (t_format, t_size, "T->O")] {
            if fmt == RealTimeFormat::Heartbeat && n != 0 {
                return bad(format!("{what} is a heartbeat but carries {n} bytes"));
            }
            let packet = u32::from(n) + 6;
            if packet > u32::from(MAX_CLASS1_PACKET) {
                return bad(format!(
                    "{what} carries {n} bytes, over what a Forward_Open can ask for"
                ));
            }
        }
        let config_size = match (&f[10], assem(&f[11])?) {
            (Field::Empty, Some(a)) => a.size.unwrap_or(0),
            (Field::Empty, None) => 0,
            (x, _) => {
                let v = match x.reference("Param") {
                    Some(p) => eds.param(p)?.default,
                    None => x.int(),
                };
                match v.and_then(|v| u16::try_from(v).ok()) {
                    Some(v) => v,
                    None => return bad(format!("config size {x:?} is not a number")),
                }
            }
        };
        let name = f[12].text().unwrap_or("").to_string();
        let Field::Str(path_text) = &f[14] else {
            return bad("path is not a string".into());
        };
        let Some(path_bytes) = hex_path(path_text) else {
            return bad(format!(
                "path {path_text:?}: only literal hex bytes are supported (no [ParamN] or symbols)"
            ));
        };
        if path_bytes.len() % 2 == 1 || path_bytes.len() > 510 {
            return bad(format!("path is {} bytes, want an even count", path_bytes.len()));
        }
        let words = u8::try_from(path_bytes.len() / 2).unwrap_or(u8::MAX);
        let path = ConnectionPath::decode(&mut Reader::new(&path_bytes), words)
            .or_else(|e| err(line, format!("Connection{number}: path {path_text:?}: {e}")))?;
        if path.class != crate::cip::class::ASSEMBLY {
            return bad(format!(
                "path names class {:#x}, not the Assembly object",
                path.class
            ));
        }
        let (Some(configuration), Some(o_point), Some(t_point)) =
            (path.configuration, path.consumed, path.produced)
        else {
            return bad(format!(
                "path {path_text:?} wants a configuration instance and two connection points"
            ));
        };
        for (a, point, what) in [(&o_assem, o_point, "O->T"), (&t_assem, t_point, "T->O")] {
            if let Some(a) = a
                && a.instance != point
            {
                return bad(format!(
                    "{what} format is Assem{} but the path's point is {point}",
                    a.instance
                ));
            }
        }
        let dir = |rpi, size, format, assembly: Option<Assembly>, shift: u32| Direction {
            rpi,
            size,
            format,
            assembly: assembly.map(|a| a.instance),
            multicast: params & (1 << (shift + 1)) != 0,
            point_to_point: params & (1 << (shift + 2)) != 0,
        };
        Ok(Self {
            number,
            name,
            line,
            trigger_transport,
            params,
            app_types,
            class1: trigger_transport & 0x2 != 0,
            cyclic: trigger_transport & (1 << 16) != 0,
            configuration,
            config_size,
            o_to_t_point: o_point,
            t_to_o_point: t_point,
            o_to_t: dir(rpi(&f[2])?, o_size, o_format, o_assem, 16),
            t_to_o: dir(rpi(&f[5])?, t_size, t_format, t_assem, 20),
        })
    }

    /// Whether the EDS declares this connection as `t`.
    #[must_use]
    pub fn is(&self, t: AppType) -> bool {
        self.app_types.contains(&t)
    }
}
