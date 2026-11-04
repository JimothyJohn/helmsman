//! A marketplace's index: one signed JSON file naming the bundles a static
//! site serves, so a box can list them and install one through the bundle
//! path and the approval policy (the `marketplace` skeleton; no accounts,
//! no payments, no review beyond the bundle's own checks).
//!
//! ```text
//! index.json        {"format": "helmsman-index 1", "publisher": "...", "bundles": [...]}
//! index.json.sig    helmsman-index-signature 1\n<key hex> <signature hex>\n
//! horn-kit-1.0.0.hbundle   the files the index names, beside it
//! ```
//!
//! An entry carries what a person reads (name, version, author,
//! description), what a box checks before it trusts the download (the
//! bundle id, its SHA-256 and size) and who signed the bundle. The index
//! is signed by a publisher whose key the box's trust store holds
//! ([`super::TrustStore`]); a bundle fetched from it must then verify on
//! its own ([`super::verify`]), so the index can at most point at a
//! bundle, never vouch for it.
//!
//! [`Index::decode`] refuses, before any allocation for it, anything over
//! [`MAX_INDEX`], more than [`MAX_ENTRIES`] entries, a file name that is
//! not a plain bundle file name, a hash that is not 64 hex digits, a size
//! over a bundle's, and a name or version a bundle manifest would refuse.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::{Map, Value as Json, json};
use sha2::{Digest, Sha256};

use super::sign::{self, PublicKey, Signature, SigningKey, TrustStore};
use super::{Bundle, MAX_BUNDLE};

/// The index file's `format` field.
pub const FORMAT: &str = "helmsman-index 1";
/// Largest index file, bytes.
pub const MAX_INDEX: usize = 1 << 20;
/// Largest signature file, bytes.
pub const MAX_SIG: usize = 4 << 10;
/// Most entries in one index.
pub const MAX_ENTRIES: usize = 256;
/// Longest text field (name, version, author, description, file).
pub const MAX_TEXT: usize = 256;
/// The signature file's first line.
pub const SIG_HEADER: &str = "helmsman-index-signature 1";
const DOMAIN: &[u8] = b"helmsman-index-signature 1\n";

/// One bundle the index offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: Option<String>,
    /// The bundle's id (the snapshot id of its content).
    pub id: String,
    /// SHA-256 of the bundle file, 64 lowercase hex digits.
    pub sha256: String,
    pub size: u64,
    /// The file beside the index: `NAME-VERSION.hbundle`, plain.
    pub file: String,
    /// The public keys the bundle is signed with, hex.
    pub signed_by: Vec<String>,
}

/// The index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Index {
    pub publisher: String,
    pub bundles: Vec<Entry>,
}

/// Why an index is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err<T>(s: impl Into<String>) -> Result<T, Error> {
    Err(Error(s.into()))
}

/// A plain bundle file name: 1..=`MAX_TEXT` of `A-Z a-z 0-9 . _ -`, no
/// leading dot, ending in `.hbundle`.
#[must_use]
pub fn file_name_ok(f: &str) -> bool {
    f.len() <= MAX_TEXT
        && f.ends_with(&format!(".{}", super::EXTENSION))
        && f.len() > super::EXTENSION.len() + 1
        && !f.starts_with('.')
        && f.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn text_ok(what: &str, v: &str, max: usize) -> Result<(), Error> {
    if v.is_empty() || v.len() > max {
        return err(format!("{what}: 1..={max} bytes"));
    }
    if v.chars().any(|c| {
        c.is_control()
            || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    }) {
        return err(format!("{what}: no control or invisible characters"));
    }
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn hex_ok(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len() / 2)
        .map(|k| u8::from_str_radix(&s[k * 2..k * 2 + 2], 16).ok())
        .collect()
}

/// SHA-256 of `bytes`, hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Versions compared as dotted numbers where both sides are numbers, as
/// text otherwise: `1.10.0` is newer than `1.9.0`.
fn version_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left_parts = left.split(['.', '-', '+']);
    let mut right_parts = right.split(['.', '-', '+']);
    loop {
        match (left_parts.next(), right_parts.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(lp), Some(rp)) => {
                let order = match (lp.parse::<u64>(), rp.parse::<u64>()) {
                    (Ok(ln), Ok(rn)) => ln.cmp(&rn),
                    _ => lp.cmp(rp),
                };
                if order != std::cmp::Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

impl Entry {
    /// What the index says about one bundle file, from the file itself.
    /// The bundle must decode, carry at least one signature and have every
    /// signature verify (the index does not check trust: the box does,
    /// against its own store).
    ///
    /// # Errors
    /// A file that is not a bundle, a bad signature, a file name the index
    /// cannot carry.
    pub fn of(file: &str, bytes: &[u8]) -> Result<Self, Error> {
        if !file_name_ok(file) {
            return err(format!(
                "{file}: a bundle file name is letters, digits, . _ - ending in .{}",
                super::EXTENSION
            ));
        }
        let b = Bundle::decode(bytes).map_err(|e| Error(format!("{file}: {e}")))?;
        super::check_signatures(&b).map_err(|e| Error(format!("{file}: {e}")))?;
        if b.signatures.is_empty() {
            return err(format!(
                "{file}: unsigned; a marketplace lists signed bundles only"
            ));
        }
        let m = &b.manifest;
        Ok(Self {
            name: m.name.clone(),
            version: m.version.clone(),
            author: m.author.clone(),
            description: m.description.clone(),
            id: b.id().to_string(),
            sha256: sha256_hex(bytes),
            size: bytes.len() as u64,
            file: file.to_string(),
            signed_by: b.signatures.iter().map(|s| sign::hex_key(&s.key)).collect(),
        })
    }

    /// Whether `bytes` is the file this entry names: size and hash.
    ///
    /// # Errors
    /// Which differs.
    pub fn check_file(&self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() as u64 != self.size {
            return err(format!(
                "{}: the index says {} bytes, the download is {}",
                self.file,
                self.size,
                bytes.len()
            ));
        }
        if sha256_hex(bytes) != self.sha256 {
            return err(format!(
                "{}: the download's SHA-256 is not the index's",
                self.file
            ));
        }
        Ok(())
    }

    fn check(&self) -> Result<(), Error> {
        text_ok("name", &self.name, 64)?;
        text_ok("version", &self.version, super::manifest::MAX_VERSION)?;
        text_ok("author", &self.author, super::manifest::MAX_AUTHOR)?;
        if let Some(d) = &self.description {
            text_ok("description", d, super::manifest::MAX_DESCRIPTION)?;
        }
        if !file_name_ok(&self.file) {
            return err(format!("{}: not a bundle file name", self.file));
        }
        if !hex_ok(&self.sha256, 64) {
            return err(format!("{}: sha256 is not 64 hex digits", self.file));
        }
        if !hex_ok(&self.id, 64) {
            return err(format!("{}: id is not a snapshot id", self.file));
        }
        if self.size == 0 || self.size > MAX_BUNDLE as u64 {
            return err(format!("{}: size {} is not a bundle's", self.file, self.size));
        }
        if self.signed_by.len() > super::MAX_SIGNATURES || self.signed_by.iter().any(|k| !hex_ok(k, 64)) {
            return err(format!("{}: signed_by is not a list of keys", self.file));
        }
        Ok(())
    }

    fn json(&self) -> Json {
        json!({
            "name": self.name, "version": self.version, "author": self.author,
            "description": self.description, "id": self.id, "sha256": self.sha256,
            "size": self.size, "file": self.file, "signed_by": self.signed_by,
        })
    }

    fn parse(j: &Json) -> Result<Self, Error> {
        let Some(o) = j.as_object() else {
            return err("an entry is an object");
        };
        let known = [
            "name",
            "version",
            "author",
            "description",
            "id",
            "sha256",
            "size",
            "file",
            "signed_by",
        ];
        if let Some(k) = o.keys().find(|k| !known.contains(&k.as_str())) {
            return err(format!("entry: unknown field {k:?}"));
        }
        let s = |k: &str| -> Result<String, Error> {
            o.get(k)
                .and_then(Json::as_str)
                .map(str::to_string)
                .ok_or_else(|| Error(format!("entry: {k} missing or not text")))
        };
        let e = Self {
            name: s("name")?,
            version: s("version")?,
            author: s("author")?,
            description: match o.get("description") {
                None | Some(Json::Null) => None,
                Some(Json::String(d)) => Some(d.clone()),
                Some(_) => return err("entry: description is text or null"),
            },
            id: s("id")?,
            sha256: s("sha256")?,
            size: o
                .get("size")
                .and_then(Json::as_u64)
                .ok_or_else(|| Error("entry: size missing or not a whole number".into()))?,
            file: s("file")?,
            signed_by: match o.get("signed_by") {
                Some(Json::Array(a)) => a
                    .iter()
                    .map(|k| {
                        k.as_str()
                            .map(str::to_string)
                            .ok_or_else(|| Error("entry: signed_by holds keys".into()))
                    })
                    .collect::<Result<_, _>>()?,
                None => Vec::new(),
                Some(_) => return err("entry: signed_by is a list"),
            },
        };
        e.check()?;
        Ok(e)
    }
}

impl Index {
    /// An index of `files` (name, bytes), from the files themselves,
    /// sorted by name then version; two files of one name and version are
    /// refused.
    ///
    /// # Errors
    /// Any file that is not a signed bundle, or a repeated name/version.
    pub fn build(publisher: &str, files: &[(String, Vec<u8>)]) -> Result<Self, Error> {
        text_ok("publisher", publisher, MAX_TEXT)?;
        if files.len() > MAX_ENTRIES {
            return err(format!("at most {MAX_ENTRIES} bundles in one index"));
        }
        let mut bundles: Vec<Entry> = files
            .iter()
            .map(|(f, b)| Entry::of(f, b))
            .collect::<Result<_, _>>()?;
        bundles.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| version_cmp(&a.version, &b.version))
        });
        if let Some(w) = bundles
            .windows(2)
            .find(|w| w[0].name == w[1].name && w[0].version == w[1].version)
        {
            return err(format!(
                "{} {} twice ({} and {})",
                w[0].name, w[0].version, w[0].file, w[1].file
            ));
        }
        Ok(Self {
            publisher: publisher.to_string(),
            bundles,
        })
    }

    /// The index as its file: canonical JSON (sorted keys, one shape), so
    /// the signature covers exactly these bytes.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut o = Map::new();
        o.insert("format".into(), json!(FORMAT));
        o.insert("publisher".into(), json!(self.publisher));
        o.insert(
            "bundles".into(),
            Json::Array(self.bundles.iter().map(Entry::json).collect()),
        );
        let mut s = serde_json::to_string_pretty(&Json::Object(o)).unwrap_or_default();
        s.push('\n');
        s
    }

    /// Reads an index file.
    ///
    /// # Errors
    /// Over [`MAX_INDEX`], not JSON, not this format, or an entry that
    /// fails [`Entry`]'s checks.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_INDEX {
            return err(format!("index over {MAX_INDEX} bytes"));
        }
        let j: Json = serde_json::from_slice(bytes).map_err(|e| Error(format!("index is not JSON: {e}")))?;
        let Some(o) = j.as_object() else {
            return err("index is not an object");
        };
        if o.get("format").and_then(Json::as_str) != Some(FORMAT) {
            return err(format!("index format is not {FORMAT:?}"));
        }
        if let Some(k) = o
            .keys()
            .find(|k| !["format", "publisher", "bundles"].contains(&k.as_str()))
        {
            return err(format!("index: unknown field {k:?}"));
        }
        let publisher = o
            .get("publisher")
            .and_then(Json::as_str)
            .ok_or_else(|| Error("index: publisher missing".into()))?
            .to_string();
        text_ok("publisher", &publisher, MAX_TEXT)?;
        let Some(list) = o.get("bundles").and_then(Json::as_array) else {
            return err("index: bundles missing or not a list");
        };
        if list.len() > MAX_ENTRIES {
            return err(format!("index: more than {MAX_ENTRIES} bundles"));
        }
        let bundles: Vec<Entry> = list.iter().map(Entry::parse).collect::<Result<_, _>>()?;
        let mut seen = BTreeSet::new();
        for e in &bundles {
            if !seen.insert((e.name.as_str(), e.version.as_str())) {
                return err(format!("index: {} {} twice", e.name, e.version));
            }
        }
        Ok(Self { publisher, bundles })
    }

    /// The entry called `name`: at `version`, or the newest without one.
    #[must_use]
    pub fn find(&self, name: &str, version: Option<&str>) -> Option<&Entry> {
        let mut same: Vec<&Entry> = self.bundles.iter().filter(|e| e.name == name).collect();
        if let Some(v) = version {
            return same.into_iter().find(|e| e.version == v);
        }
        same.sort_by(|x, y| version_cmp(&x.version, &y.version));
        same.last().copied()
    }
}

/// The bytes an index signature covers.
fn message(index_bytes: &[u8]) -> Vec<u8> {
    let mut m = DOMAIN.to_vec();
    m.extend_from_slice(&Sha256::digest(index_bytes));
    m
}

/// The signature file for `index_bytes`, by `key`.
#[must_use]
pub fn sign(index_bytes: &[u8], key: &SigningKey) -> String {
    let s = key.sign(&message(index_bytes));
    format!("{SIG_HEADER}\n{} {}\n", sign::hex_key(&s.key), hex(&s.sig))
}

/// Checks a signature file against the index bytes and the box's trust
/// store; the publisher's name as the store has it.
///
/// # Errors
/// Not a signature file, a signature that does not verify, or a key the
/// store does not trust (which key, so an integrator can add it).
pub fn verify_signature(index_bytes: &[u8], sig_text: &str, trust: &TrustStore) -> Result<String, Error> {
    if sig_text.len() > MAX_SIG {
        return err("signature file too large");
    }
    let mut lines = sig_text.lines();
    if lines.next().map(str::trim_end) != Some(SIG_HEADER) {
        return err(format!(
            "not an index signature ({SIG_HEADER:?} expected on the first line)"
        ));
    }
    let Some(line) = lines.next() else {
        return err("signature file has no signature line");
    };
    if lines.any(|l| !l.trim().is_empty()) {
        return err("signature file has more than one signature line");
    }
    let (k, s) = line
        .trim()
        .split_once(' ')
        .ok_or_else(|| Error("signature line is KEY SIGNATURE".into()))?;
    let key: PublicKey =
        sign::parse_key(k).ok_or_else(|| Error("signature key is not 64 hex digits".into()))?;
    let sig_bytes = unhex(s)
        .filter(|v| v.len() == 64)
        .ok_or_else(|| Error("signature is not 128 hex digits".into()))?;
    let mut sig = [0u8; 64];
    sig.copy_from_slice(&sig_bytes);
    let s = Signature { key, sig };
    if !sign::verifies(&s, &message(index_bytes)) {
        return err("the index signature does not verify: the index or its signature changed");
    }
    match trust.publisher(&key) {
        Some(p) => Ok(p.to_string()),
        None => err(format!(
            "the index is signed by a key this box does not trust: {}",
            sign::hex_key(&key)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_plain() {
        assert!(file_name_ok("horn-kit-1.0.0.hbundle"));
        for bad in [
            "",
            ".hbundle",
            "a.zip",
            "../a.hbundle",
            "a/b.hbundle",
            ".hidden.hbundle",
            "sp ace.hbundle",
            "ünï.hbundle",
            &format!("{}.hbundle", "a".repeat(300)),
        ] {
            assert!(!file_name_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn versions_sort_as_numbers_where_they_are_numbers() {
        use std::cmp::Ordering::*;
        assert_eq!(version_cmp("1.10.0", "1.9.0"), Greater);
        assert_eq!(version_cmp("1.0", "1.0.1"), Less);
        assert_eq!(version_cmp("2", "10"), Less);
        assert_eq!(version_cmp("1.0.0", "1.0.0"), Equal);
        assert_eq!(version_cmp("1.0.0-rc1", "1.0.0"), Greater);
    }

    #[test]
    fn garbage_indexes_are_refused_before_they_cost_anything() {
        for bad in [
            b"".as_slice(),
            b"{}",
            b"[]",
            br#"{"format":"helmsman-index 2","publisher":"p","bundles":[]}"#,
            br#"{"format":"helmsman-index 1","publisher":"p","bundles":{}}"#,
            br#"{"format":"helmsman-index 1","publisher":"","bundles":[]}"#,
            br#"{"format":"helmsman-index 1","publisher":"p","bundles":[],"extra":1}"#,
            br#"{"format":"helmsman-index 1","publisher":"p","bundles":[{"name":"a"}]}"#,
            br#"{"format":"helmsman-index 1","publisher":"p","bundles":[{"name":"a","version":"1","author":"x","id":"00","sha256":"00","size":1,"file":"a.hbundle"}]}"#,
            br#"{"format":"helmsman-index 1","publisher":"p\u0000","bundles":[]}"#,
        ] {
            assert!(Index::decode(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
        assert!(Index::decode(&vec![b' '; MAX_INDEX + 1]).is_err());
        let ok = br#"{"format":"helmsman-index 1","publisher":"p","bundles":[]}"#;
        assert_eq!(Index::decode(ok).unwrap().bundles.len(), 0);
    }

    #[test]
    fn a_signature_binds_the_index_bytes_to_a_trusted_key() {
        let key = SigningKey::from_seed(&[3; 32]).unwrap();
        let other = SigningKey::from_seed(&[4; 32]).unwrap();
        let mut trust = TrustStore::default();
        trust.add("acme", key.public()).unwrap();
        let idx = Index {
            publisher: "acme".into(),
            bundles: Vec::new(),
        };
        let bytes = idx.encode();
        let sig = sign(bytes.as_bytes(), &key);
        assert_eq!(verify_signature(bytes.as_bytes(), &sig, &trust).unwrap(), "acme");
        assert!(
            verify_signature(format!("{bytes} ").as_bytes(), &sig, &trust).is_err(),
            "changed bytes"
        );
        let stranger = sign(bytes.as_bytes(), &other);
        let e = verify_signature(bytes.as_bytes(), &stranger, &trust).unwrap_err();
        assert!(e.0.contains("does not trust"), "{e}");
        for bad in [
            "",
            "nope\nx y\n",
            &format!("{SIG_HEADER}\n"),
            &format!("{SIG_HEADER}\nkey sig\n"),
            &format!("{sig}extra line\n"),
        ] {
            assert!(
                verify_signature(bytes.as_bytes(), bad, &trust).is_err(),
                "{bad:?}"
            );
        }
        assert_eq!(
            Index::decode(bytes.as_bytes()).unwrap(),
            idx,
            "encode and decode agree"
        );
    }
}
