//! A Device's IODD, read for what a controller needs to use its Process
//! Data: which Device it is, and where each value lies in the input and
//! output octets, by name. Behind the `iodd` feature: it allocates, and
//! reads the XML with `helmsman_iec::xml` (no DOCTYPE, so no entities).
//!
//! Reference (docs/SOURCES.md, "IO-Link"): IO Device Description
//! Specification V1.1.5 (IO-Link Community, order no. 10.012, October
//! 2025), cited as "IODD §x p.y", and the schemas in its package
//! (`IODD1.1.xsd`, `IODD-Datatypes1.1.xsd`).
//!
//! Read: `DeviceIdentity` (§7.4), `DatatypeCollection` (§7.5.2), the
//! simple types, arrays and records (§7.5.3), `ProcessDataCollection`
//! with its conditions (§7.5.5) and the primary language's texts (§7.7.1).
//! Not read: variables, menus, events, the communication profile, other
//! languages, the file's stamp and checksum.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use helmsman_iec::Pos;
use helmsman_iec::xml::{self, Element};

/// The basic data types (IODD §7.5.3.1 p.26-30; their coding is IOL
/// Annex F.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Boolean,
    UInteger,
    Integer,
    Float32,
    String,
    OctetString,
    Time,
    TimeSpan,
}

/// One value inside the Process Data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Its name in the primary language.
    pub name: String,
    /// The record item's or array element's subindex; 0 when the
    /// Process Data is this one value.
    pub subindex: u8,
    /// From the least significant bit of the last octet (IOL Table F.19
    /// rule 3), as [`crate::pd::get`] takes it.
    pub bit_offset: u16,
    pub bit_length: u16,
    pub kind: Kind,
}

/// A Device's input or output Process Data (IODD §7.5.5 p.50-52).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub name: String,
    /// 1 to 256.
    pub bit_length: u16,
    /// In ascending order of subindex.
    pub fields: Vec<Field>,
}

impl Item {
    /// The length the Device states in Direct Parameter page 1 (IODD
    /// §7.5.5 p.51: the bit length up to 16 bits, else rounded up to
    /// whole octets), to compare with [`crate::page::ProcessData::bits`].
    #[must_use]
    pub const fn page_bits(&self) -> u16 {
        if self.bit_length <= 16 {
            self.bit_length
        } else {
            self.bit_length.div_ceil(8) * 8
        }
    }
}

/// When a Device has several Process Data layouts: the one in use is
/// the one whose `value` the named variable holds (IODD §7.5.5 p.49-50).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condition {
    pub variable: String,
    pub subindex: Option<u8>,
    pub value: u8,
}

/// One `ProcessData` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessData {
    pub id: String,
    pub condition: Option<Condition>,
    pub input: Option<Item>,
    pub output: Option<Item>,
}

/// What was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iodd {
    pub vendor_id: u16,
    pub device_id: u32,
    pub vendor_name: String,
    pub device_name: String,
    /// One entry, or one per condition value.
    pub process_data: Vec<ProcessData>,
}

/// Where the file is not an IODD this reader can use, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoddError {
    pub pos: Pos,
    pub msg: String,
}

impl fmt::Display for IoddError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.pos, self.msg)
    }
}

impl core::error::Error for IoddError {}

fn err<T>(at: &Element, msg: String) -> Result<T, IoddError> {
    Err(IoddError { pos: at.pos, msg })
}

fn child<'a>(of: &'a Element, name: &str) -> Result<&'a Element, IoddError> {
    of.child(name)
        .map_or_else(|| err(of, format!("<{}> has no <{name}>", of.name)), Ok)
}

fn attr<'a>(of: &'a Element, name: &str) -> Result<&'a str, IoddError> {
    of.attr(name)
        .map_or_else(|| err(of, format!("<{}> has no {name}=", of.name)), Ok)
}

/// A decimal attribute within `range`.
fn number(of: &Element, name: &str, range: core::ops::RangeInclusive<u32>) -> Result<u32, IoddError> {
    let text = attr(of, name)?;
    match text.parse::<u32>() {
        Ok(value) if range.contains(&value) => Ok(value),
        _ => err(
            of,
            format!(
                "<{}> {name}=\"{}\": not a number from {} to {}",
                of.name,
                clip(text),
                range.start(),
                range.end()
            ),
        ),
    }
}

/// A piece of the file short enough to put in a message.
fn clip(text: &str) -> String {
    text.chars().take(40).collect()
}

fn narrow<T: TryFrom<u32>>(of: &Element, value: u32) -> Result<T, IoddError> {
    T::try_from(value).map_or_else(|_| err(of, format!("{value} is out of range")), Ok)
}

struct Reader<'a> {
    texts: BTreeMap<&'a str, &'a str>,
    datatypes: BTreeMap<&'a str, &'a Element>,
}

impl<'a> Reader<'a> {
    /// The text a `Name`-like child refers to (IODD §7.7.1 p.76), or the
    /// reference itself when the primary language lacks it.
    fn text(&self, of: &'a Element, name: &str) -> Result<String, IoddError> {
        let id = attr(child(of, name)?, "textId")?;
        Ok(self.texts.get(id).copied().unwrap_or(id).to_string())
    }

    /// The type a `SimpleDatatype` / `Datatype` child or a `DatatypeRef`
    /// names (IODD §7.5.2 p.24: referenced by its id).
    fn datatype(&self, of: &'a Element) -> Result<&'a Element, IoddError> {
        if let Some(reference) = of.child("DatatypeRef") {
            let id = attr(reference, "datatypeId")?;
            return self.datatypes.get(id).copied().map_or_else(
                || err(reference, format!("no Datatype with id \"{}\"", clip(id))),
                Ok,
            );
        }
        of.child("Datatype")
            .or_else(|| of.child("SimpleDatatype"))
            .map_or_else(|| err(of, format!("<{}> has no data type", of.name)), Ok)
    }

    fn item(&self, of: &'a Element) -> Result<Item, IoddError> {
        let bit_length: u16 = narrow(of, number(of, "bitLength", 1..=256)?)?;
        let name = self.text(of, "Name")?;
        let datatype = self.datatype(of)?;
        let mut fields = match attr(datatype, "type")? {
            "RecordT" => self.record(datatype, bit_length)?,
            "ArrayT" => self.array(datatype, &name)?,
            _ => {
                let (kind, bits) = simple(datatype)?;
                alloc::vec![Field {
                    name: name.clone(),
                    subindex: 0,
                    bit_offset: 0,
                    bit_length: bits,
                    kind,
                }]
            }
        };
        fields.sort_by_key(|field| field.subindex);
        let mut used = [false; 256];
        for field in &fields {
            let end = usize::from(field.bit_offset) + usize::from(field.bit_length);
            if end > usize::from(bit_length) {
                return err(
                    datatype,
                    format!(
                        "\"{}\" ends at bit {end} of {bit_length} bits of Process Data",
                        clip(&field.name)
                    ),
                );
            }
            // IOL Table F.19 rule 8: one item per bit position.
            for bit in &mut used[usize::from(field.bit_offset)..end] {
                if core::mem::replace(bit, true) {
                    return err(
                        datatype,
                        format!("\"{}\" shares bits with another item", clip(&field.name)),
                    );
                }
            }
        }
        Ok(Item {
            id: attr(of, "id")?.to_string(),
            name,
            bit_length,
            fields,
        })
    }

    /// IODD §7.5.3.2.3 p.34: record items by subindex and bit offset.
    fn record(&self, of: &'a Element, bit_length: u16) -> Result<Vec<Field>, IoddError> {
        let stated = number(of, "bitLength", 1..=1856)?;
        if stated != u32::from(bit_length) {
            // IODD §7.5.5 p.51: "this 'bitLength' shall equal the
            // 'bitLength' attribute of the record".
            return err(
                of,
                format!("a record of {stated} bits as {bit_length} bits of Process Data"),
            );
        }
        let mut fields = Vec::new();
        for item in of.children_named("RecordItem") {
            let datatype = self.datatype(item)?;
            let (kind, bits) = simple(datatype)?;
            fields.push(Field {
                name: self.text(item, "Name")?,
                subindex: narrow(item, number(item, "subindex", 1..=255)?)?,
                bit_offset: narrow(item, number(item, "bitOffset", 0..=1855)?)?,
                bit_length: bits,
                kind,
            });
        }
        if fields.is_empty() {
            return err(of, "a record with no RecordItem".to_string());
        }
        Ok(fields)
    }

    /// IODD §7.5.3.2.2 p.31 and IOL Table F.17 p.304: `count` elements
    /// of one simple type packed without gaps, the highest subindex at
    /// the right.
    fn array(&self, of: &'a Element, name: &str) -> Result<Vec<Field>, IoddError> {
        let count = number(of, "count", 1..=255)?;
        let (kind, bits) = simple(self.datatype(of)?)?;
        (1..=count)
            .map(|subindex| {
                Ok(Field {
                    name: format!("{name} {subindex}"),
                    subindex: narrow(of, subindex)?,
                    bit_offset: narrow(of, (count - subindex) * u32::from(bits))?,
                    bit_length: bits,
                    kind,
                })
            })
            .collect()
    }
}

/// A simple type's kind and length in bits (IODD §7.5.3.1 p.26-30; IOL
/// Annex F.2 for the fixed lengths: 32 bits of `Float32T`, 8 octets of
/// `TimeT` and `TimeSpanT`).
fn simple(of: &Element) -> Result<(Kind, u16), IoddError> {
    let octets = |of| Ok(narrow::<u16>(of, number(of, "fixedLength", 1..=232)?)? * 8);
    Ok(match attr(of, "type")? {
        "BooleanT" => (Kind::Boolean, 1),
        "UIntegerT" => (Kind::UInteger, narrow(of, number(of, "bitLength", 2..=64)?)?),
        "IntegerT" => (Kind::Integer, narrow(of, number(of, "bitLength", 2..=64)?)?),
        "Float32T" => (Kind::Float32, 32),
        "StringT" => (Kind::String, octets(of)?),
        "OctetStringT" => (Kind::OctetString, octets(of)?),
        "TimeT" => (Kind::Time, 64),
        "TimeSpanT" => (Kind::TimeSpan, 64),
        other => {
            return err(
                of,
                format!("data type \"{}\" where a simple type is needed", clip(other)),
            );
        }
    })
}

/// Reads an IODD.
///
/// # Errors
/// The first thing that is not well-formed XML, not the structure of
/// `IODD1.1.xsd` where this reader looks, or not Process Data a
/// controller can lay out (an item outside its Process Data, two items
/// on one bit, a record of another length than its Process Data).
pub fn parse(src: &str) -> Result<Iodd, IoddError> {
    let root = xml::parse(src).map_err(|e| IoddError {
        pos: e.pos,
        msg: e.msg,
    })?;
    if root.name != "IODevice" {
        return err(
            &root,
            format!("<{}> is not an IODD's <IODevice>", clip(&root.name)),
        );
    }
    let body = child(&root, "ProfileBody")?;
    let identity = child(body, "DeviceIdentity")?;
    let function = child(body, "DeviceFunction")?;
    let mut reader = Reader {
        texts: BTreeMap::new(),
        datatypes: BTreeMap::new(),
    };
    let language = child(child(&root, "ExternalTextCollection")?, "PrimaryLanguage")?;
    for text in language.children_named("Text") {
        reader.texts.insert(attr(text, "id")?, attr(text, "value")?);
    }
    if let Some(collection) = function.child("DatatypeCollection") {
        for datatype in collection.children_named("Datatype") {
            reader.datatypes.insert(attr(datatype, "id")?, datatype);
        }
    }
    let mut process_data = Vec::new();
    for element in child(function, "ProcessDataCollection")?.children_named("ProcessData") {
        let condition = match element.child("Condition") {
            Some(condition) => Some(Condition {
                variable: attr(condition, "variableId")?.to_string(),
                subindex: match condition.attr("subindex") {
                    Some(_) => Some(narrow(condition, number(condition, "subindex", 1..=255)?)?),
                    None => None,
                },
                value: narrow(condition, number(condition, "value", 0..=255)?)?,
            }),
            None => None,
        };
        process_data.push(ProcessData {
            id: attr(element, "id")?.to_string(),
            condition,
            input: element
                .child("ProcessDataIn")
                .map(|item| reader.item(item))
                .transpose()?,
            output: element
                .child("ProcessDataOut")
                .map(|item| reader.item(item))
                .transpose()?,
        });
    }
    // IODD §7.5.5 p.49: with more than one, each has a condition.
    if process_data.len() > 1 && process_data.iter().any(|pd| pd.condition.is_none()) {
        return err(
            function,
            "several ProcessData and one of them without a Condition".to_string(),
        );
    }
    Ok(Iodd {
        vendor_id: narrow(identity, number(identity, "vendorId", 1..=65_535)?)?,
        device_id: number(identity, "deviceId", 1..=16_777_215)?,
        vendor_name: attr(identity, "vendorName")?.to_string(),
        device_name: reader.text(identity, "DeviceName")?,
        process_data,
    })
}

/// Longest signal name a map line gets (helmsman-nodecore's `NAME_MAX`).
const NAME_MAX: usize = 32;

/// `name` as an identifier under `device`: lower case, everything but
/// letters and digits a single `_`, cut to [`NAME_MAX`], and numbered
/// when `taken` already has it.
fn ident(device: &str, name: &str, taken: &mut Vec<String>) -> String {
    let mut out = String::from(device);
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if out.len() == device.len() {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let base: String = out.trim_end_matches('_').chars().take(NAME_MAX - 3).collect();
    let mut unique = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&unique)) {
        unique = format!("{base}_{n}");
        n += 1;
    }
    taken.push(unique.clone());
    unique
}

/// A comment's worth of a text from the file: one line, no `#`.
fn remark(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() && *c != '#')
        .take(60)
        .collect()
}

impl Iodd {
    /// The lines of an I/O map for this Device on Master port `port`,
    /// in the grammar of helmsman-nodecore's `iolink`: the `device`
    /// line with the identity and the Process Data lengths, then one
    /// `in` or `out` line per value of the `which`-th `ProcessData`
    /// (0 unless the Device has conditional layouts). `BooleanT` becomes
    /// a `bool` point, integers up to 63 bits (64 when signed) an `int`
    /// point; the other types are listed in a comment, not mapped.
    /// `device` is taken as given: an identifier of a few characters.
    ///
    /// `None` when there is no `which`-th layout.
    #[must_use]
    pub fn map_lines(&self, which: usize, device: &str, port: u8) -> Option<String> {
        use fmt::Write as _;
        let layout = self.process_data.get(which)?;
        let mut text = format!(
            "# {} ({}), from its IODD\ndevice {device} iolink port={port} vendor={} device={}",
            remark(&self.device_name),
            remark(&self.vendor_name),
            self.vendor_id,
            self.device_id
        );
        if let Some(input) = &layout.input {
            let _ = write!(text, " in_bits={}", input.bit_length);
        }
        if let Some(output) = &layout.output {
            let _ = write!(text, " out_bits={}", output.bit_length);
        }
        text.push('\n');
        let mut taken = Vec::new();
        for (dir, item) in [("in ", &layout.input), ("out", &layout.output)] {
            for field in item.iter().flat_map(|item| &item.fields) {
                let format = match field.kind {
                    Kind::Boolean => Some(("bool", String::new())),
                    Kind::UInteger if field.bit_length <= 63 => {
                        Some(("int ", format!(":u{}", field.bit_length)))
                    }
                    Kind::Integer => Some(("int ", format!(":i{}", field.bit_length))),
                    _ => None,
                };
                let _ = match format {
                    Some((ty, format)) => writeln!(
                        text,
                        "{dir} {} {ty} {device} pd:{}{format}  # {}",
                        ident(device, &field.name, &mut taken),
                        field.bit_offset,
                        remark(&field.name)
                    ),
                    None => writeln!(
                        text,
                        "# not mapped: {} ({:?}, {} bits at {})",
                        remark(&field.name),
                        field.kind,
                        field.bit_length,
                        field.bit_offset
                    ),
                };
            }
        }
        Some(text)
    }
}
