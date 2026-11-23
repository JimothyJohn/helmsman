//! A part's match to a device-library record.
//!
//! The library comes in as a slice of [`LibraryRecord`] (the caller maps
//! each entry of the index it holds: vendor, model text, the order numbers
//! in `products`, the `folder` or `wired=` name as `id`). Nothing here
//! reads a file.
//!
//! The identifiers of a record are its order numbers, its model text, the
//! pieces of the model text split at `,` `/` `(` `)` (the library writes
//! `IF6123, IF6124` and `O5D100/O5D102` as one model) and the vendor with
//! the model. Case, spaces and punctuation do not count: `iologik e1213`,
//! `ioLogik-E1213` and `IOLOGIK E1213` are one text. An identifier must be
//! at least 4 characters with a digit in it, and one with a run of `x`
//! (`KQxxxx`, a family) is not an identifier at all.
//!
//! - `Match::Matched`: exactly one record has an identifier equal to one
//!   of the part's own texts (its part number, model, any attribute value,
//!   its block name).
//! - `Match::Ambiguous`: two or more records have, or no record has an
//!   exact hit but some identifier contains, or is contained in, one of the
//!   part's texts (`E1213` for `ioLogik E1213`), or the only hits are in
//!   text lying near the part (a drawing's label is evidence a person has
//!   to confirm, so it never decides), or one record has an exact hit and a
//!   looser hit names another. A list of one still means "a person
//!   confirms this".
//! - `Match::Unknown`: nothing.
//!
//! Records are listed in the order the library gave them.

use crate::parts::Part;
use std::collections::{BTreeSet, HashMap};

/// Longest text compared, characters after normalising.
const MAX_COMPARE: usize = 256;

/// What the matcher needs of a library entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRecord {
    /// What names it to the caller (the `wired=` value, `moxa/iologik-e1213`).
    pub id: String,
    pub vendor: String,
    pub model: String,
    /// The order numbers the record covers.
    pub products: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match<'a> {
    Matched(&'a LibraryRecord),
    Ambiguous(Vec<&'a LibraryRecord>),
    Unknown,
}

pub struct Matcher<'a> {
    records: &'a [LibraryRecord],
    exact: HashMap<String, Vec<usize>>,
    ids: Vec<(String, usize)>,
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .take(MAX_COMPARE)
        .collect()
}

fn usable(raw: &str, n: &str) -> bool {
    n.len() >= 4 && n.bytes().any(|b| b.is_ascii_digit()) && !raw.to_ascii_lowercase().contains("xxx")
}

impl<'a> Matcher<'a> {
    #[must_use]
    pub fn new(records: &'a [LibraryRecord]) -> Self {
        let mut exact: HashMap<String, Vec<usize>> = HashMap::new();
        let mut ids = Vec::new();
        for (i, r) in records.iter().enumerate() {
            let mut raws: Vec<String> = r.products.clone();
            raws.push(r.model.clone());
            raws.extend(r.model.split([',', '/', '(', ')']).map(str::to_owned));
            raws.push(format!("{} {}", r.vendor, r.model));
            let mut seen = BTreeSet::new();
            for raw in raws {
                let n = norm(&raw);
                if usable(&raw, &n) && seen.insert(n.clone()) {
                    exact.entry(n.clone()).or_default().push(i);
                    ids.push((n, i));
                }
            }
        }
        Self { records, exact, ids }
    }

    fn hits(&self, text: &str, exact: &mut BTreeSet<usize>, near: &mut BTreeSet<usize>) {
        let c = norm(text);
        if c.len() < 4 || !c.bytes().any(|b| b.is_ascii_digit()) {
            return;
        }
        if let Some(v) = self.exact.get(&c) {
            exact.extend(v);
            return;
        }
        for (id, i) in &self.ids {
            if c.contains(id.as_str()) || id.contains(c.as_str()) {
                near.insert(*i);
            }
        }
    }

    /// Match one piece of text (no part around it): an exact single hit is
    /// `Matched`, anything looser `Ambiguous`.
    #[must_use]
    pub fn match_text(&self, text: &str) -> Match<'a> {
        let (mut e, mut n) = (BTreeSet::new(), BTreeSet::new());
        self.hits(text, &mut e, &mut n);
        self.decide(&e, &n)
    }

    fn decide(&self, e: &BTreeSet<usize>, n: &BTreeSet<usize>) -> Match<'a> {
        let all: BTreeSet<usize> = e.union(n).copied().collect();
        let v: Vec<&LibraryRecord> = all.iter().map(|&i| &self.records[i]).collect();
        match (e.len(), all.len()) {
            (_, 0) => Match::Unknown,
            (1, 1) => Match::Matched(v[0]),
            _ => Match::Ambiguous(v),
        }
    }

    /// Match a part by its own texts; nearby text can only make it `Ambiguous`.
    #[must_use]
    pub fn match_part(&self, p: &Part) -> Match<'a> {
        let (mut e, mut n) = (BTreeSet::new(), BTreeSet::new());
        let own = [
            p.part_number.as_deref(),
            p.model.as_deref(),
            Some(p.block.as_str()),
        ]
        .into_iter()
        .flatten()
        .chain(p.attributes.iter().map(|a| a.value.as_str()));
        for t in own {
            self.hits(t, &mut e, &mut n);
        }
        let (mut ne, mut nn) = (BTreeSet::new(), BTreeSet::new());
        for t in &p.nearby_text {
            self.hits(&t.text, &mut ne, &mut nn);
        }
        n.extend(ne);
        n.extend(nn);
        self.decide(&e, &n)
    }
}
