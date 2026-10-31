//! Drop-ins: documents a person hands the agent (cards `dropin-prose`,
//! `dropin-plc`). A paragraph, an SOP, a Markdown file, or an existing PLC
//! program (a Rockwell L5X export, a `PLCopen` TC6 XML project, Structured
//! Text) is posted to the box, kept in memory, and read by the agent's
//! tools by id, always inside the data envelope.
//!
//! ```text
//! POST /api/dropins?name=N[&kind=K]   token: the document as the body; stored, 201 with its id
//! GET  /api/dropins                   what is stored: id, name, kind, size, chunks (no text)
//! GET  /api/dropins/{id}?chunk=K      one chunk of the text
//! GET  /api/dropins/{id}/plc?view=V&name=N&offset=O
//!                                     a PLC program's inventory: overview, tags (of controller
//!                                     scope or one program), routine (rungs or ST lines), names
//! POST /api/devices/{name}/browse     token: an observed controller's tag list, read-only
//! ```
//!
//! **Hostile by default.** A drop-in is the most hostile text the box
//! sees: tag descriptions and rung comments are anyone's words. It is
//! never parsed into anything that runs: prose stays text, a PLC program
//! becomes an inventory (names that are Logix or ST identifiers, types,
//! text). The agent reads it through the envelope, and nothing it says
//! can widen what the tools reach.
//!
//! **Read-only toward the customer's PLC.** Nothing here ever writes a
//! program back or talks to a controller except `browse`, which lists
//! symbols over a session of its own (`helmsman_io::logix`, read services
//! only; the box build cannot send anything else).
//!
//! **Bounded.** One document is at most [`MAX_DROPIN`] bytes (and the
//! API's body limit), UTF-8 without control characters but line breaks and
//! tabs; the box keeps at most [`MAX_KEPT`] of them and [`MAX_TOTAL`]
//! bytes, dropping the oldest. Text is served in [`CHUNK`]-byte chunks
//! and inventories in pages of at most [`PAGE_BYTES`], so no one tool
//! result outgrows the agent's result limit. Kept in memory only: a
//! restart forgets them. PDF is refused (415): there is no PDF text
//! extraction on the box; paste the text.
//!
//! **Photos** (card `dropin-photo`). A PNG, JPEG or WebP (a whiteboard
//! sketch, a photo of the panel) is a `photo` drop-in: known by its bytes,
//! read from its headers only and held to [`super::photo`]'s limits. It
//! has no text to read; the chat shows it to the model as an image block
//! in the message it is attached to (see [`super::chat::Chat::attach`]).
//!
//! **Watch modules.** A proposal that names a PLC drop-in (or one the box
//! no longer holds) must be a watch module: kind `module`, `watch: true`,
//! driving no output signal and with no `%Q` window (see
//! [`super::proposal`]). The agent loop names every drop-in its
//! conversation read, so a conversation that read a PLC program can only
//! ever propose modules that read and report.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use helmsman_iec::ast::{self, PouKind, Section, TypeDef, TypeRef};
use helmsman_iec::l5x::{self, Project};
use serde_json::{Value as Json, json};

use super::catalog::BrowsedTag;
use super::{Ctx, Request, Response, audit, audit_full, photo, wall};
use crate::manifest;

/// Largest document, bytes.
pub const MAX_DROPIN: usize = 1 << 20;
/// Documents kept at once.
pub const MAX_KEPT: usize = 32;
/// Bytes kept at once, over all documents.
pub const MAX_TOTAL: usize = 16 << 20;
/// Bytes of text per chunk (cut at a line break when one is near).
pub const CHUNK: usize = 6 * 1024;
/// Most bytes of items in one inventory page.
pub const PAGE_BYTES: usize = 20 * 1024;
/// Longest document name, characters.
pub const MAX_NAME: usize = 128;
/// What reading a photo as text answers.
const PHOTO_HAS_NO_TEXT: &str = "this drop-in is a photo: it has no text to read; the box shows the image to the agent in the chat message it is attached to";
/// A browse result is reused for this long.
pub const BROWSE_FRESH: Duration = Duration::from_secs(30);

/// What a drop-in is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Plain text or Markdown: an SOP, a paragraph, a manual page.
    Prose,
    /// A Rockwell L5X export.
    L5x,
    /// A `PLCopen` TC6 XML project.
    Plcopen,
    /// IEC 61131-3 Structured Text.
    St,
    /// A PNG, JPEG or WebP image: a sketch or a photo.
    Photo,
}

impl Kind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Prose => "prose",
            Self::L5x => "l5x",
            Self::Plcopen => "plcopen",
            Self::St => "st",
            Self::Photo => "photo",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "prose" => Self::Prose,
            "l5x" => Self::L5x,
            "plcopen" => Self::Plcopen,
            "st" => Self::St,
            "photo" => Self::Photo,
            _ => return None,
        })
    }

    /// An existing PLC program (proposals citing it must be watch modules).
    #[must_use]
    pub const fn is_plc(self) -> bool {
        matches!(self, Self::L5x | Self::Plcopen | Self::St)
    }
}

/// A photo drop-in's image: what its headers say, and its bytes untouched.
#[derive(Debug, Clone)]
pub struct Photo {
    pub info: photo::Info,
    pub bytes: Arc<[u8]>,
}

/// What the chat attaches for a drop-in (see [`Store::attachment`]).
#[derive(Debug, Clone)]
pub enum Attachment {
    /// A document read through the tools: its kind and chunk count.
    Text { kind: Kind, chunks: usize },
    /// A photo shown to the model as an image block.
    Photo(Photo),
}

/// One stored document.
#[derive(Debug, Clone)]
pub struct Dropin {
    pub id: u64,
    pub name: String,
    pub kind: Kind,
    pub at: u64,
    text: String,
    /// Byte offsets where chunks start.
    chunks: Vec<usize>,
    /// The inventory of a PLC program.
    plc: Option<Arc<Project>>,
    /// A photo's image.
    photo: Option<Photo>,
}

impl Dropin {
    /// Bytes held for it.
    fn size(&self) -> usize {
        self.text.len() + self.photo.as_ref().map_or(0, |p| p.bytes.len())
    }

    fn meta(&self) -> Json {
        let mut j = json!({
            "id": self.id, "name": self.name, "kind": self.kind.name(), "at": self.at,
            "bytes": self.size(), "chunks": self.chunks.len(),
        });
        if let Some(p) = &self.photo {
            j["media_type"] = json!(p.info.format.media_type());
            j["width"] = json!(p.info.width);
            j["height"] = json!(p.info.height);
        }
        j
    }

    fn chunk(&self, k: usize) -> Option<&str> {
        let start = *self.chunks.get(k)?;
        let end = self.chunks.get(k + 1).copied().unwrap_or(self.text.len());
        Some(&self.text[start..end])
    }
}

/// The drop-ins on this box, newest last.
#[derive(Debug, Default)]
pub struct Store {
    list: VecDeque<Dropin>,
    next: u64,
    bytes: usize,
    /// The last browse of each device, and when.
    browsed: Vec<(String, Instant, Arc<Vec<BrowsedTag>>)>,
}

impl Store {
    fn add(&mut self, mut d: Dropin) -> Json {
        self.next += 1;
        d.id = self.next;
        while self.list.len() >= MAX_KEPT || (self.bytes + d.size() > MAX_TOTAL && !self.list.is_empty()) {
            if let Some(old) = self.list.pop_front() {
                self.bytes -= old.size();
            }
        }
        self.bytes += d.size();
        let meta = d.meta();
        self.list.push_back(d);
        meta
    }

    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Dropin> {
        self.list.iter().find(|d| d.id == id)
    }

    /// Its kind, if the box still holds drop-in `id`.
    #[must_use]
    pub fn kind(&self, id: u64) -> Option<Kind> {
        self.get(id).map(|d| d.kind)
    }

    /// What the chat attaches for drop-in `id`, if the box still holds it.
    #[must_use]
    pub fn attachment(&self, id: u64) -> Option<Attachment> {
        self.get(id).map(|d| match &d.photo {
            Some(p) => Attachment::Photo(p.clone()),
            None => Attachment::Text {
                kind: d.kind,
                chunks: d.chunks.len(),
            },
        })
    }
}

/// Chunk starts: every [`CHUNK`] bytes, pulled back to just after the last
/// line break in the second half of the window, and to a char boundary.
fn chunks(text: &str) -> Vec<usize> {
    let mut out = vec![0];
    let mut at = 0;
    while text.len() - at > CHUNK {
        let mut end = at + CHUNK;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if let Some(nl) = text[at + CHUNK / 2..end].rfind('\n') {
            end = at + CHUNK / 2 + nl + 1;
        }
        out.push(end);
        at = end;
    }
    out
}

/// Why a document is refused: status and message.
type Refused = (u16, String);

/// Checks a body and works out what it is.
fn read_body(body: &[u8], kind: Option<Kind>) -> Result<(String, Kind, Option<Project>), Refused> {
    if body.is_empty() {
        return Err((422, "the document is empty".into()));
    }
    if body.len() > MAX_DROPIN {
        return Err((413, format!("a drop-in is at most {MAX_DROPIN} bytes")));
    }
    let text = std::str::from_utf8(body).map_err(|_| (422, "the document is not UTF-8 text".to_string()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err((
            422,
            "the document carries control characters (only line breaks and tabs are taken)".into(),
        ));
    }
    let text = text.replace("\r\n", "\n");
    let kind = kind.unwrap_or_else(|| sniff(&text));
    let plc = match kind {
        Kind::Prose => None,
        Kind::Photo => return Err((415, "a photo is an image, not text".into())),
        Kind::L5x => Some(l5x::parse(&text).map_err(|e| (422, format!("not a readable L5X export: {e}")))?),
        Kind::Plcopen => {
            let lowered = helmsman_iec::plcopen::lower(&text).map_err(|d| {
                let first = d
                    .first()
                    .map_or_else(String::new, |d| format!("{}: {}", d.pos, d.msg));
                (422, format!("not a readable PLCopen project: {first}"))
            })?;
            Some(from_st(lowered.st(), "plcopen")?)
        }
        Kind::St => Some(from_st(&text, "st")?),
    };
    Ok((text, kind, plc))
}

/// What a document looks like: XML by its root, ST if it parses, else prose.
fn sniff(text: &str) -> Kind {
    let head: String = text.chars().take(4096).collect();
    let t = head.trim_start();
    if t.starts_with('<') {
        if head.contains("<RSLogix5000Content") {
            return Kind::L5x;
        }
        if head.contains("<project") {
            return Kind::Plcopen;
        }
        return Kind::Prose;
    }
    if ast::parse(text).is_ok_and(|u| !u.pous.is_empty()) {
        Kind::St
    } else {
        Kind::Prose
    }
}

fn ty_text(t: &TypeRef) -> (String, Vec<u32>) {
    match t {
        TypeRef::Named(n, _) => (n.clone(), Vec::new()),
        TypeRef::Array { dims, elem, .. } => {
            let (name, _) = ty_text(elem);
            let d = dims
                .iter()
                .map(|(lo, hi)| u32::try_from(hi - lo + 1).unwrap_or(0))
                .collect();
            (name, d)
        }
    }
}

/// A unit's TYPE declarations as data types (struct members listed).
fn st_types(unit: &ast::Unit, p: &mut Project) {
    for t in &unit.types {
        if !l5x::is_name(&t.name) {
            skipped(p, t.pos, "a type whose name is not a short identifier".into());
            continue;
        }
        let members = match &t.def {
            TypeDef::Struct(fields) => fields
                .iter()
                .filter(|f| l5x::is_name(&f.name))
                .map(|f| {
                    let (ty, d) = ty_text(&f.ty);
                    l5x::Member {
                        name: f.name.clone(),
                        data_type: ty,
                        dimension: d.first().copied(),
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        p.data_types.push(l5x::DataType {
            name: t.name.clone(),
            members,
        });
    }
}

fn skipped(p: &mut Project, pos: helmsman_iec::Pos, why: String) {
    p.skipped_total += 1;
    if p.skipped.len() < l5x::MAX_SKIPPED {
        p.skipped.push(l5x::Skipped { pos, why });
    }
}

/// Structured Text as the same inventory an L5X gives: each POU a
/// program with its variables as tags and its source as one ST routine;
/// struct types as data types.
fn from_st(src: &str, source: &str) -> Result<Project, Refused> {
    let unit =
        ast::parse(src).map_err(|d| (422, format!("not readable Structured Text: {}: {}", d.pos, d.msg)))?;
    let lines: Vec<&str> = src.lines().collect();
    let mut p = Project {
        target_type: Some(source.to_string()),
        ..Project::default()
    };
    st_types(&unit, &mut p);
    let starts: Vec<usize> = unit
        .pous
        .iter()
        .map(|u| usize::try_from(u.pos.line.saturating_sub(1)).unwrap_or(0))
        .collect();
    for (k, pou) in unit.pous.iter().enumerate() {
        if !l5x::is_name(&pou.name) {
            skipped(
                &mut p,
                pou.pos,
                "a POU whose name is not a short identifier".into(),
            );
            continue;
        }
        let mut tags = Vec::new();
        for v in &pou.vars {
            if !l5x::is_name(&v.name) {
                skipped(
                    &mut p,
                    v.pos,
                    "a variable whose name is not a short identifier".into(),
                );
                continue;
            }
            let (ty, dims) = ty_text(&v.ty);
            let usage = match v.section {
                Section::Input => "Input",
                Section::Output => "Output",
                Section::InOut => "InOut",
                Section::Var | Section::Temp => "Local",
            };
            tags.push(l5x::Tag {
                name: v.name.clone(),
                data_type: Some(ty),
                dimensions: dims,
                kind: l5x::TagKind::Base,
                usage: Some(usage.into()),
                external_access: None,
                constant: v.constant,
                description: None,
            });
        }
        let from = starts[k].min(lines.len());
        let to = starts
            .get(k + 1)
            .copied()
            .unwrap_or(lines.len())
            .clamp(from, lines.len());
        let body: Vec<String> = lines[from..to].iter().map(|l| (*l).to_string()).collect();
        // Only a PROGRAM runs by itself; function blocks and functions are called.
        let main = matches!(pou.kind, PouKind::Program).then(|| pou.name.clone());
        p.programs.push(l5x::Program {
            name: pou.name.clone(),
            main_routine: main,
            disabled: false,
            tags,
            routines: vec![l5x::Routine {
                name: pou.name.clone(),
                kind: l5x::RoutineKind::St,
                rungs: Vec::new(),
                lines: body,
            }],
        });
    }
    Ok(p)
}

fn tag_json(t: &l5x::Tag, prefix: &str) -> Json {
    let (alias, kind) = match &t.kind {
        l5x::TagKind::Base => (None, "base"),
        l5x::TagKind::Alias(a) => (Some(a.as_str()), "alias"),
        l5x::TagKind::Produced => (None, "produced"),
        l5x::TagKind::Consumed => (None, "consumed"),
    };
    json!({
        "name": format!("{prefix}{}", t.name), "type": t.data_type, "dims": t.dimensions,
        "kind": kind, "alias_for": alias, "usage": t.usage, "access": t.external_access,
        "constant": t.constant, "description": t.description,
    })
}

/// Items from `offset` while they fit [`PAGE_BYTES`] (at least one).
fn page(items: Vec<Json>, offset: usize, what: &str) -> Json {
    let total = items.len();
    let mut out = Vec::new();
    let mut bytes = 0;
    for it in items.into_iter().skip(offset) {
        let n = serde_json::to_vec(&it).map_or(0, |v| v.len());
        if !out.is_empty() && bytes + n > PAGE_BYTES {
            break;
        }
        bytes += n;
        out.push(it);
    }
    let next = offset + out.len();
    json!({
        what: out, "offset": offset, "total": total,
        "next_offset": if next < total { json!(next) } else { Json::Null },
    })
}

fn overview(d: &Dropin, p: &Project) -> Json {
    let rungs: usize = p
        .programs
        .iter()
        .flat_map(|q| &q.routines)
        .map(|r| r.rungs.len())
        .sum();
    let lines: usize = p
        .programs
        .iter()
        .flat_map(|q| &q.routines)
        .map(|r| r.lines.len())
        .sum();
    let routines: usize = p.programs.iter().map(|q| q.routines.len()).sum();
    let program_tags: usize = p.programs.iter().map(|q| q.tags.len()).sum();
    json!({
        "dropin_id": d.id, "kind": d.kind.name(),
        "target": p.target, "target_type": p.target_type, "software": p.software,
        "controller": p.controller.as_ref().map(|c| json!({ "name": c.name, "processor": c.processor })),
        "counts": {
            "controller_tags": p.tags.len(), "program_tags": program_tags, "programs": p.programs.len(),
            "routines": routines, "rungs": rungs, "st_lines": lines, "data_types": p.data_types.len(),
        },
        "data_types": p.data_types.iter().take(100).map(|t| json!({
            "name": t.name,
            "members": t.members.iter().take(64).map(|m| json!({ "name": m.name, "type": m.data_type, "dim": m.dimension })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "add_ons": p.add_ons.iter().take(100).collect::<Vec<_>>(),
        "programs": p.programs.iter().take(200).map(|q| json!({
            "name": q.name, "main_routine": q.main_routine, "disabled": q.disabled, "tags": q.tags.len(),
            "routines": q.routines.iter().take(100).map(|r| json!({
                "name": r.name, "kind": r.kind.code(), "rungs": r.rungs.len(), "st_lines": r.lines.len(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "tasks": p.tasks.iter().take(64).map(|t| json!({
            "name": t.name, "type": t.kind, "rate_ms": t.rate_ms, "programs": t.programs,
        })).collect::<Vec<_>>(),
        "skipped": p.skipped.iter().take(20).map(|s| format!("{}: {}", s.pos, s.why)).collect::<Vec<_>>(),
        "skipped_total": p.skipped_total,
        "next": "view tags (name \"\" for controller scope, or a program), view routine (name PROGRAM/ROUTINE)",
    })
}

/// `GET /api/dropins/{id}/plc`.
fn plc_view(doc: &Dropin, req: &Request) -> Response {
    let Some(prog) = &doc.plc else {
        return Response::error(
            404,
            if doc.photo.is_some() {
                PHOTO_HAS_NO_TEXT
            } else {
                "this drop-in is prose, not a PLC program; read its text in chunks"
            },
        );
    };
    let query = |k: &str| req.query.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let Ok(offset) = query("offset").unwrap_or("0").parse::<usize>() else {
        return Response::error(400, "offset is a whole number");
    };
    let name = query("name").unwrap_or("");
    match query("view").unwrap_or("overview") {
        "overview" => Response::json(200, &overview(doc, prog)),
        "tags" => {
            let tags = if name.is_empty() {
                &prog.tags
            } else if let Some(found) = prog.programs.iter().find(|x| x.name.eq_ignore_ascii_case(name)) {
                &found.tags
            } else {
                return Response::error(404, "no such program in this drop-in");
            };
            let items = tags.iter().map(|t| tag_json(t, "")).collect();
            let mut j = page(items, offset, "tags");
            j["dropin_id"] = json!(doc.id);
            j["scope"] = json!(if name.is_empty() { "controller" } else { name });
            Response::json(200, &j)
        }
        "routine" => {
            let Some((pname, rname)) = name.split_once('/') else {
                return Response::error(400, "name a routine as PROGRAM/ROUTINE");
            };
            let Some(r) = prog
                .programs
                .iter()
                .find(|x| x.name.eq_ignore_ascii_case(pname))
                .and_then(|x| x.routines.iter().find(|r| r.name.eq_ignore_ascii_case(rname)))
            else {
                return Response::error(404, "no such routine in this drop-in");
            };
            let mut j = match r.kind {
                l5x::RoutineKind::Ladder => page(
                    r.rungs
                        .iter()
                        .map(|g| json!({ "number": g.number, "text": g.text, "comment": g.comment }))
                        .collect(),
                    offset,
                    "rungs",
                ),
                l5x::RoutineKind::St => page(
                    r.lines
                        .iter()
                        .enumerate()
                        .map(|(k, l)| json!({ "line": k, "text": l }))
                        .collect(),
                    offset,
                    "lines",
                ),
                l5x::RoutineKind::Fbd | l5x::RoutineKind::Sfc => json!({
                    "note": "function block diagrams and charts are listed by name only; their content is not read",
                }),
            };
            j["dropin_id"] = json!(doc.id);
            j["routine"] = json!(format!("{pname}/{rname}"));
            j["kind"] = json!(r.kind.code());
            Response::json(200, &j)
        }
        "names" => {
            let mut all: Vec<Json> = prog.tags.iter().map(|t| tag_json(t, "")).collect();
            for program in &prog.programs {
                let prefix = format!("Program:{}.", program.name);
                all.extend(program.tags.iter().map(|t| tag_json(t, &prefix)));
            }
            let names: Vec<Json> = all
                .into_iter()
                .map(
                    |t| json!({ "name": t["name"], "type": t["type"], "dims": t["dims"], "kind": t["kind"] }),
                )
                .collect();
            Response::json(200, &json!({ "dropin_id": doc.id, "tags": names }))
        }
        _ => Response::error(400, "view is overview, tags, routine or names"),
    }
}

/// `GET /api/dropins...`.
pub(super) fn get(ctx: &Ctx, req: &Request, segs: &[&str]) -> Response {
    let store = ctx.dropins();
    match segs {
        [] => Response::json(200, &Json::Array(store.list.iter().map(Dropin::meta).collect())),
        [id, rest @ ..] => {
            let Some(d) = super::proposal_id(id).and_then(|id| store.get(id)) else {
                return Response::error(404, "no such drop-in (the box keeps the newest 32, in memory)");
            };
            match rest {
                [] => {
                    let k = req
                        .query
                        .iter()
                        .find(|(n, _)| n == "chunk")
                        .map_or(Ok(0), |(_, v)| v.parse::<usize>());
                    let Ok(k) = k else {
                        return Response::error(400, "chunk is a whole number");
                    };
                    if d.photo.is_some() {
                        return Response::error(404, PHOTO_HAS_NO_TEXT);
                    }
                    let Some(text) = d.chunk(k) else {
                        return Response::error(
                            404,
                            &format!(
                                "no chunk {k}: this drop-in has {} (0 to {})",
                                d.chunks.len(),
                                d.chunks.len() - 1
                            ),
                        );
                    };
                    let mut j = d.meta();
                    j["chunk"] = json!(k);
                    j["text"] = json!(text);
                    Response::json(200, &j)
                }
                ["plc"] => plc_view(d, req),
                _ => Response::error(404, "no such resource"),
            }
        }
    }
}

/// `POST /api/dropins`.
pub(super) fn post(req: &Request, ctx: &Ctx) -> Response {
    let refused = |status: u16, why: &str, detail: &str| match audit(ctx, req, status, detail) {
        Some(_) => Response::error(status, why),
        None => audit_full(),
    };
    let ct = req.content_type.as_deref().unwrap_or("");
    let base = ct.split(';').next().unwrap_or("").trim();
    if base == "application/pdf" {
        return refused(
            415,
            "PDF is not read on the box (no PDF text extraction is built in); paste or upload the document's text instead",
            "dropin: PDF refused",
        );
    }
    let q = |k: &str| req.query.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let kind = match q("kind") {
        None => None,
        Some(k) => match Kind::parse(k) {
            Some(k) => Some(k),
            None => {
                return refused(
                    400,
                    "kind is prose, l5x, plcopen, st or photo",
                    "dropin: bad kind",
                );
            }
        },
    };
    let name = q("name").unwrap_or("").trim().to_string();
    if name.chars().count() > MAX_NAME || name.chars().any(manifest::hidden) {
        return refused(
            422,
            &format!(
                "a name is at most {MAX_NAME} characters, without control or invisible formatting characters"
            ),
            "dropin: bad name",
        );
    }
    // An image is known by its bytes; a Content-Type or kind that says
    // image is held to the same reading.
    if photo::is_image(&req.body) || base.starts_with("image/") || kind == Some(Kind::Photo) {
        return post_photo(req, ctx, base, kind, name);
    }
    let textual = base.starts_with("text/")
        || matches!(
            base,
            "application/xml" | "application/octet-stream" | "application/x-l5x"
        );
    if !textual {
        return refused(
            415,
            "a drop-in is text: text/plain, text/markdown, text/xml or application/xml (L5X, PLCopen), or application/octet-stream holding UTF-8; or a PNG, JPEG or WebP photo",
            "dropin: unsupported Content-Type",
        );
    }
    let (text, kind, plc) = match read_body(&req.body, kind) {
        Ok(r) => r,
        Err((status, why)) => return refused(status, &why, "dropin: refused"),
    };
    let chunks = chunks(&text);
    store(
        ctx,
        req,
        Dropin {
            id: 0,
            name,
            kind,
            at: wall(),
            chunks,
            text,
            plc: plc.map(Arc::new),
            photo: None,
        },
    )
}

/// `POST /api/dropins` for an image: a photo drop-in, read from its
/// headers ([`photo::sniff`]).
fn post_photo(req: &Request, ctx: &Ctx, base: &str, kind: Option<Kind>, name: String) -> Response {
    let refused = |status: u16, why: &str, detail: &str| match audit(ctx, req, status, detail) {
        Some(_) => Response::error(status, why),
        None => audit_full(),
    };
    let image_type = base.starts_with("image/");
    if !(image_type || base == "application/octet-stream") {
        return refused(
            415,
            "a photo goes as image/png, image/jpeg or image/webp (or application/octet-stream)",
            "dropin: image under a text Content-Type",
        );
    }
    if kind.is_some_and(|k| k != Kind::Photo) {
        return refused(
            400,
            "an image is a photo drop-in (kind photo, or no kind)",
            "dropin: image with a text kind",
        );
    }
    let info = match photo::sniff(&req.body) {
        Ok(i) => i,
        Err((status, why)) => return refused(status, &why, "dropin: photo refused"),
    };
    store(
        ctx,
        req,
        Dropin {
            id: 0,
            name,
            kind: Kind::Photo,
            at: wall(),
            text: String::new(),
            chunks: Vec::new(),
            plc: None,
            photo: Some(Photo {
                info,
                bytes: Arc::from(req.body.as_slice()),
            }),
        },
    )
}

/// Stores a checked drop-in: 201 with its metadata, audited.
fn store(ctx: &Ctx, req: &Request, d: Dropin) -> Response {
    let kind = d.kind;
    let shape = d.photo.as_ref().map(|p| {
        format!(
            " {} {}x{} px,",
            p.info.format.media_type(),
            p.info.width,
            p.info.height
        )
    });
    let mut store = ctx.dropins();
    let mut meta = store.add(d);
    let id = meta["id"].as_u64().unwrap_or_default();
    if meta["name"] == "" {
        meta["name"] = json!(format!("dropin-{id}"));
        if let Some(d) = store.list.back_mut() {
            d.name = format!("dropin-{id}");
        }
    }
    if let Some(d) = store.get(id)
        && let Some(p) = &d.plc
    {
        meta["plc"] = overview(d, p)["counts"].clone();
    }
    drop(store);
    let detail = format!(
        "dropin {id}: {},{} {} bytes",
        kind.name(),
        shape.unwrap_or_default(),
        meta["bytes"].as_u64().unwrap_or_default()
    );
    match audit(ctx, req, 201, &detail) {
        Some(_) => Response::json(201, &meta).with("Location", format!("/api/dropins/{id}")),
        None => audit_full(),
    }
}

/// `POST /api/devices/{name}/browse`: the device's tag list, read-only;
/// one browse at a time, a result reused for [`BROWSE_FRESH`].
pub(super) fn browse(req: &Request, ctx: &Ctx, device: &str) -> Response {
    let refused = |status: u16, why: &str, detail: &str| match audit(ctx, req, status, detail) {
        Some(_) => Response::error(status, why),
        None => audit_full(),
    };
    let Some(browser) = ctx.box_.devices.browser(device) else {
        return refused(
            404,
            "no browsable device by that name (only logix-tag devices in the I/O map are)",
            "browse: no such device",
        );
    };
    let cached = ctx
        .dropins()
        .browsed
        .iter()
        .find(|(n, at, _)| n == device && at.elapsed() < BROWSE_FRESH)
        .map(|(_, _, t)| Arc::clone(t));
    let tags = if let Some(t) = cached {
        t
    } else {
        let Ok(_one) = ctx.browsing.try_lock() else {
            return refused(
                409,
                "a browse is already running; try again shortly",
                "browse: busy",
            );
        };
        match browser() {
            Ok(t) => {
                let t = Arc::new(t);
                let mut store = ctx.dropins();
                store.browsed.retain(|(n, _, _)| n != device);
                store
                    .browsed
                    .push((device.to_string(), Instant::now(), Arc::clone(&t)));
                t
            }
            Err(e) => return refused(502, &format!("browsing {device} failed: {e}"), "browse: failed"),
        }
    };
    let body = json!({
        "device": device, "total": tags.len(),
        "tags": tags.iter().map(|t| json!({ "name": t.name, "type": t.ty, "dims": t.dims })).collect::<Vec<_>>(),
    });
    match audit(ctx, req, 200, &format!("browse: {} tags, read-only", tags.len())) {
        Some(_) => Response::json(200, &body),
        None => audit_full(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Whatever bytes arrive, as whatever kind, the body is refused or
        /// read; reading never panics, and what is kept has no control
        /// characters but line breaks and tabs.
        #[test]
        fn any_body_is_read_or_refused(
            body in prop::collection::vec(any::<u8>(), 0..2048),
            kind in prop::option::of(prop::sample::select(vec![Kind::Prose, Kind::L5x, Kind::Plcopen, Kind::St])),
        ) {
            if let Ok((text, _, _)) = read_body(&body, kind) {
                prop_assert!(!text.chars().any(|c| c.is_control() && c != '\n' && c != '\t'));
                let c = chunks(&text);
                prop_assert_eq!(c[0], 0);
            }
        }

        /// Markup-shaped text as an L5X, PLCopen or ST drop-in never panics.
        #[test]
        fn markup_noise_is_read_or_refused(parts in prop::collection::vec(prop::sample::select(vec![
            "<RSLogix5000Content>", "<Controller Name=\"C\">", "<Tags>", "<Tag Name=\"A\" DataType=\"BOOL\"/>",
            "</Tags>", "</Controller>", "</RSLogix5000Content>", "<project>", "</project>", "PROGRAM p",
            "END_PROGRAM", "VAR", "END_VAR", "x : BOOL;", "<![CDATA[", "]]>", "&amp;", "&", "<", ">", "\n",
        ]), 0..60)) {
            let s = parts.concat();
            for kind in [None, Some(Kind::L5x), Some(Kind::Plcopen), Some(Kind::St)] {
                let _ = read_body(s.as_bytes(), kind);
            }
        }
    }

    #[test]
    fn chunks_cover_the_text_at_char_boundaries() {
        let text = "é".repeat(CHUNK) + "\n" + &"x".repeat(100);
        let c = chunks(&text);
        assert!(c.len() >= 2, "{c:?}");
        for w in c.windows(2) {
            assert!(w[1] > w[0] && w[1] - w[0] <= CHUNK);
            assert!(text.is_char_boundary(w[1]));
        }
        assert_eq!(chunks("short"), [0]);
    }

    #[test]
    fn chunks_prefer_line_breaks() {
        let line = "a".repeat(99) + "\n";
        let text = line.repeat(200);
        let c = chunks(&text);
        assert!(c[1..].iter().all(|&s| text.as_bytes()[s - 1] == b'\n'), "{c:?}");
    }

    #[test]
    fn documents_are_sniffed_by_their_shape() {
        assert_eq!(
            sniff("Flash the light when the jam eye is blocked for 5 s."),
            Kind::Prose
        );
        assert_eq!(sniff("# SOP\n\n1. Start the line"), Kind::Prose);
        assert_eq!(sniff("<?xml version=\"1.0\"?>\n<RSLogix5000Content>"), Kind::L5x);
        assert_eq!(
            sniff("<?xml version=\"1.0\"?><project xmlns=\"x\">"),
            Kind::Plcopen
        );
        assert_eq!(
            sniff("PROGRAM p VAR x : BOOL; END_VAR x := TRUE; END_PROGRAM"),
            Kind::St
        );
    }

    #[test]
    fn st_reads_as_an_inventory() {
        let p = from_st(
            "TYPE M : STRUCT run : BOOL; END_STRUCT; END_TYPE\nPROGRAM main\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\nVAR t : ARRAY[1..4] OF INT; END_VAR\n  lamp := start;\nEND_PROGRAM\n",
            "st",
        )
        .unwrap();
        assert_eq!(p.data_types[0].name, "M");
        let prog = &p.programs[0];
        assert!(prog.name.eq_ignore_ascii_case("main"), "{}", prog.name);
        let tags: Vec<(String, Option<&str>, &[u32])> = prog
            .tags
            .iter()
            .map(|t| {
                (
                    t.name.to_ascii_lowercase(),
                    t.usage.as_deref(),
                    t.dimensions.as_slice(),
                )
            })
            .collect();
        assert_eq!(
            tags,
            [
                ("start".to_string(), Some("Input"), &[][..]),
                ("lamp".to_string(), Some("Output"), &[][..]),
                ("t".to_string(), Some("Local"), &[4][..])
            ]
        );
        assert_eq!(
            prog.routines[0].lines.first().map(String::as_str),
            Some("PROGRAM main")
        );
    }
}
