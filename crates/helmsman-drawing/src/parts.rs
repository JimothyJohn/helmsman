//! The parts a drawing inserts.
//!
//! Every `INSERT` is a part: its block name, the attributes it carries (the
//! `ATTRIB`s on it, and the defaults of the block's `ATTDEF`s for tags it
//! does not carry), its position and rotation in drawing coordinates, and
//! for a block with no attributes at all the `TEXT` and `MTEXT` lying
//! nearby. An `INSERT` inside a block is a part too, placed through the
//! inserts around it. A block that inserts itself, directly or through
//! others, is not expanded again ([`Parts::notes`] says so), and the depth
//! and the number of parts are capped.
//!
//! "Nearby" is a rectangle: the block's own `LINE` / `LWPOLYLINE` extent
//! as inserted, grown by half its larger side on every side. A block with
//! no such geometry has no nearby text. At most [`MAX_NEAR`] texts are
//! kept, the closest first. Text found this way is a hint, and
//! [`crate::matching`] never matches on it alone.
//!
//! Rotation comes from the composed transform, so a mirrored or sheared
//! insert reports the rotation of its x axis.

use crate::dxf::{Block, Drawing, Entity, Insert, Text};

/// Deepest nesting of inserts followed.
pub const MAX_DEPTH: usize = 8;
/// Most parts reported.
pub const MAX_PARTS: usize = 100_000;
/// Most nearby texts kept for one part.
pub const MAX_NEAR: usize = 4;

const TAG_KEYS: &[&str] = &[
    "TAG",
    "TAGNAME",
    "DEVICETAG",
    "DESIGNATION",
    "REFDES",
    "REF",
    "REFERENCE",
    "ITEM",
    "ITEMNO",
    "POSITION",
    "ID",
    "NAME",
    "KKS",
];
const PART_KEYS: &[&str] = &[
    "PARTNO",
    "PARTNUMBER",
    "PN",
    "ORDER",
    "ORDERNO",
    "ORDERNUMBER",
    "ORDERCODE",
    "ARTICLE",
    "ARTICLENO",
    "ARTNO",
    "ARTNR",
    "ARTIKELNR",
    "MATERIAL",
    "CATALOG",
    "CATALOGNO",
    "CATNO",
    "MLFB",
    "SKU",
    "MFGPARTNO",
    "MPN",
    "PARTCODE",
];
const MODEL_KEYS: &[&str] = &["MODEL", "TYPE", "TYPENAME", "DESCRIPTION", "DEVICE"];

/// An attribute of a part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub tag: String,
    pub value: String,
    /// From the block's `ATTDEF`, not from an `ATTRIB` on the insert.
    pub from_default: bool,
}

/// A text near a part.
#[derive(Debug, Clone, PartialEq)]
pub struct NearText {
    pub text: String,
    pub distance: f64,
}

/// One inserted part.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub block: String,
    pub layer: String,
    pub x: f64,
    pub y: f64,
    /// Degrees in 0..360.
    pub rotation: f64,
    /// The first non-empty attribute named like a device tag (`TAG`, `DESIGNATION`, ...).
    pub tag: Option<String>,
    /// The first non-empty attribute named like an order or part number.
    pub part_number: Option<String>,
    /// The first non-empty attribute named `MODEL`, `TYPE`, `DESCRIPTION`.
    pub model: Option<String>,
    pub attributes: Vec<Attribute>,
    pub nearby_text: Vec<NearText>,
    /// The blocks this part sits in, outermost first; empty at the top level.
    pub inside: Vec<String>,
    /// The `INSERT`'s line in the file.
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parts {
    pub parts: Vec<Part>,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy)]
struct M {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

impl M {
    const ID: M = M {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    fn apply(&self, p: (f64, f64)) -> (f64, f64) {
        (
            self.a * p.0 + self.b * p.1 + self.tx,
            self.c * p.0 + self.d * p.1 + self.ty,
        )
    }

    /// self after other: apply `other` first.
    fn then_after(&self, o: &M) -> M {
        let t = self.apply((o.tx, o.ty));
        M {
            a: self.a * o.a + self.b * o.c,
            b: self.a * o.b + self.b * o.d,
            c: self.c * o.a + self.d * o.c,
            d: self.c * o.b + self.d * o.d,
            tx: t.0,
            ty: t.1,
        }
    }

    /// The insert's own placement: scale, rotate, move to its point.
    fn of_insert(i: &Insert) -> M {
        let (s, c) = i.rotation.to_radians().sin_cos();
        M {
            a: c * i.sx,
            b: -s * i.sy,
            c: s * i.sx,
            d: c * i.sy,
            tx: i.x,
            ty: i.y,
        }
    }

    /// The transform of a block's contents once `i` inserts `block`.
    fn contents(&self, i: &Insert, block: &Block) -> M {
        let m = M::of_insert(i);
        let at = M {
            tx: -block.base.0,
            ty: -block.base.1,
            ..M::ID
        };
        self.then_after(&m).then_after(&at)
    }
}

fn key(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn first_of(attrs: &[Attribute], keys: &[&str]) -> Option<String> {
    // Earlier keys in the list win, so `PARTNO` beats `MATERIAL`.
    keys.iter().find_map(|k| {
        attrs
            .iter()
            .find(|a| !a.value.is_empty() && key(&a.tag) == *k)
            .map(|a| a.value.clone())
    })
}

fn extent(block: &Block) -> Option<((f64, f64), (f64, f64))> {
    let mut pts: Vec<(f64, f64)> = Vec::new();
    for e in &block.entities {
        match e {
            Entity::Line(l) => pts.extend([l.from, l.to]),
            Entity::Polyline(p) => pts.extend(p.vertices.iter().copied()),
            _ => {}
        }
    }
    let mut it = pts.into_iter();
    let f = it.next()?;
    Some(it.fold((f, f), |(lo, hi), p| {
        ((lo.0.min(p.0), lo.1.min(p.1)), (hi.0.max(p.0), hi.1.max(p.1)))
    }))
}

struct Walk<'a> {
    d: &'a Drawing,
    out: Parts,
    texts: Vec<&'a Text>,
    capped: bool,
}

impl Walk<'_> {
    fn note(&mut self, s: String) {
        if self.out.notes.len() < 64 {
            self.out.notes.push(s);
        }
    }

    fn insert(&mut self, i: &Insert, parent: &M, path: &mut Vec<String>) {
        if self.out.parts.len() >= MAX_PARTS {
            if !self.capped {
                self.capped = true;
                self.note(format!("more than {MAX_PARTS} parts; the rest are not listed"));
            }
            return;
        }
        let m = parent.then_after(&M::of_insert(i));
        let block = self.d.block(&i.block);
        let mut attributes: Vec<Attribute> = i
            .attribs
            .iter()
            .map(|a| Attribute {
                tag: a.tag.clone(),
                value: a.value.clone(),
                from_default: false,
            })
            .collect();
        if let Some(b) = block {
            for e in &b.entities {
                if let Entity::AttDef(ad) = e
                    && !attributes.iter().any(|a| a.tag.eq_ignore_ascii_case(&ad.tag))
                    && !ad.default.is_empty()
                {
                    attributes.push(Attribute {
                        tag: ad.tag.clone(),
                        value: ad.default.clone(),
                        from_default: true,
                    });
                }
            }
        }
        let at = m.apply((0.0, 0.0));
        let nearby_text = if attributes.is_empty() && path.is_empty() {
            block.map(|b| self.near(b, &m, at)).unwrap_or_default()
        } else {
            Vec::new()
        };
        self.out.parts.push(Part {
            block: i.block.clone(),
            layer: i.layer.clone(),
            x: at.0,
            y: at.1,
            rotation: m.c.atan2(m.a).to_degrees().rem_euclid(360.0),
            tag: first_of(&attributes, TAG_KEYS),
            part_number: first_of(&attributes, PART_KEYS),
            model: first_of(&attributes, MODEL_KEYS),
            attributes,
            nearby_text,
            inside: path.clone(),
            line: i.line,
        });
        let Some(block) = block else { return };
        let nested: Vec<&Insert> = block
            .entities
            .iter()
            .filter_map(|e| if let Entity::Insert(n) = e { Some(n) } else { None })
            .collect();
        if nested.is_empty() {
            return;
        }
        let upper = block.name.to_uppercase();
        if path.iter().any(|p| p.to_uppercase() == upper) {
            self.note(format!(
                "block {} inserts itself; it is not expanded again",
                block.name
            ));
            return;
        }
        if path.len() >= MAX_DEPTH {
            self.note(format!(
                "inserts nested deeper than {MAX_DEPTH} in block {}; not followed",
                block.name
            ));
            return;
        }
        let inner = parent.contents(i, block);
        path.push(block.name.clone());
        for n in nested {
            self.insert(n, &inner, path);
        }
        path.pop();
    }

    fn near(&self, block: &Block, m: &M, at: (f64, f64)) -> Vec<NearText> {
        let Some((lo, hi)) = extent(block) else {
            return Vec::new();
        };
        let corners =
            [lo, (hi.0, lo.1), hi, (lo.0, hi.1)].map(|p| m.apply((p.0 - block.base.0, p.1 - block.base.1)));
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for c in corners {
            x0 = x0.min(c.0);
            y0 = y0.min(c.1);
            x1 = x1.max(c.0);
            y1 = y1.max(c.1);
        }
        let margin = (x1 - x0).max(y1 - y0) * 0.5;
        if margin.is_nan() || margin <= 0.0 {
            return Vec::new();
        }
        let mut v: Vec<NearText> = self
            .texts
            .iter()
            .filter(|t| {
                !t.text.is_empty()
                    && t.x >= x0 - margin
                    && t.x <= x1 + margin
                    && t.y >= y0 - margin
                    && t.y <= y1 + margin
            })
            .map(|t| NearText {
                text: t.text.clone(),
                distance: (t.x - at.0).hypot(t.y - at.1),
            })
            .collect();
        v.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then_with(|| a.text.cmp(&b.text))
        });
        v.truncate(MAX_NEAR);
        v
    }
}

/// The parts of a drawing, in file order (a block's inner parts after the part that holds them).
#[must_use]
pub fn parts(d: &Drawing) -> Parts {
    let texts = d
        .entities
        .iter()
        .filter_map(|e| if let Entity::Text(t) = e { Some(t) } else { None })
        .collect();
    let mut w = Walk {
        d,
        out: Parts::default(),
        texts,
        capped: false,
    };
    let mut path = Vec::new();
    for e in &d.entities {
        if let Entity::Insert(i) = e {
            w.insert(i, &M::ID, &mut path);
        }
    }
    w.out
}
