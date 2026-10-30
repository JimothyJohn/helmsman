//! Bundles: a snapshot's content, portable and signed, for importing on
//! another box (`bundle-format`; the trust model is `market-floor`'s
//! floor: signature, capability manifest, host-enforced checks).
//!
//! A bundle is one file:
//!
//! ```text
//! helmsman-bundle 1\n                   header (18 bytes)
//! T len tree                            the snapshot tree (store::ChangeSet::tree)
//! M len manifest                        the bundle manifest ([`manifest`])
//! B len blob   (zero or more)           each file the tree names, ascending by SHA-256
//! S len key+sig (zero or more)          Ed25519 signatures ([`sign`]), ascending by key
//! ```
//!
//! Each record is a one-byte type, a big-endian `u32` length and that many
//! bytes; the file ends exactly after the last record. The tree is the
//! store's canonical tree, so **a bundle's id is the snapshot id of the
//! same content** in any store, and a blob is known only by its hash: a
//! changed byte in any file is a blob the tree does not name.
//!
//! [`Bundle::decode`] refuses, before allocating for it, anything over
//! [`MAX_BUNDLE`] or a record claiming more than its type may hold or more
//! than the file has left; records out of order, repeated, or of an unknown
//! type; a blob the tree does not name or a name the tree gives no blob;
//! module names that are not module names (no `/`, `..`, or non-ASCII
//! lookalikes: `store`'s rule); a tree or manifest not in canonical form.
//! [`verify`] then checks every signature, the signer against the box's
//! [`sign::TrustStore`], and the content: every module compiled and held to
//! its own manifest, and the capability manifest exactly what the content
//! can touch ([`manifest::capabilities`]).
//!
//! Nothing here applies anything: on a box a verified bundle becomes a
//! proposal (`api::bundle`), which the approval policy then treats like any
//! other.

pub mod cli;
pub mod manifest;
pub mod sign;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use sha2::{Digest, Sha256};

use crate::manifest::MAX_FILE;
use crate::spec::{Defaults, name_ok};
use crate::store::{ChangeSet, MAX_MODULES, ModuleFiles, SnapshotId};

pub use manifest::{BundleManifest, Capabilities, capabilities};
pub use sign::{Signature, SigningKey, TrustStore};

/// The file's first bytes.
pub const MAGIC: &[u8] = b"helmsman-bundle 1\n";
/// Largest bundle, in bytes.
pub const MAX_BUNDLE: usize = 16 << 20;
/// Largest tree record.
pub const MAX_TREE: usize = 256 << 10;
/// Largest manifest record.
pub const MAX_MANIFEST: usize = 16 << 10;
/// Most signatures on one bundle.
pub const MAX_SIGNATURES: usize = 16;
/// Most blobs: two per module, the map and the calendar.
pub const MAX_BLOBS: usize = 2 * MAX_MODULES + 2;
/// Suggested file extension.
pub const EXTENSION: &str = "hbundle";

const TREE_HEADER: &str = "helmsman-snapshot 1";
const SIG_LEN: usize = 96;

/// Why a bundle is refused. Each says exactly what failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a bundle this build reads: header, framing, sizes, order.
    Malformed(String),
    /// The tree and the blobs disagree: a file was altered, removed or
    /// added.
    Tampered(String),
    /// The bundle manifest does not parse.
    Manifest(Vec<String>),
    /// The content is not one a box would run.
    Content(Vec<String>),
    /// The capability manifest is not what the content can touch.
    Capabilities(Vec<String>),
    /// A signature does not verify (with the key, hex).
    BadSignature(String),
    /// No signature, and the box takes no dev bundles.
    Unsigned,
    /// Signed, but by no publisher this box trusts (the keys, hex).
    Untrusted(Vec<String>),
}

impl Error {
    /// A short machine-readable name.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "malformed",
            Self::Tampered(_) => "tampered",
            Self::Manifest(_) => "manifest",
            Self::Content(_) => "content",
            Self::Capabilities(_) => "capabilities",
            Self::BadSignature(_) => "bad-signature",
            Self::Unsigned => "unsigned",
            Self::Untrusted(_) => "untrusted",
        }
    }

    /// Every reason, one per line.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        match self {
            Self::Malformed(w) | Self::Tampered(w) | Self::BadSignature(w) => vec![w.clone()],
            Self::Manifest(v) | Self::Content(v) | Self::Capabilities(v) => v.clone(),
            Self::Unsigned => vec!["the bundle carries no signature".into()],
            Self::Untrusted(keys) => keys
                .iter()
                .map(|k| format!("signed by key {k}, which this box does not trust"))
                .collect(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self {
            Self::Malformed(_) => "not a valid bundle",
            Self::Tampered(_) => "content does not match its addresses (tampered)",
            Self::Manifest(_) => "bundle manifest does not parse",
            Self::Content(_) => "content would not run on a box",
            Self::Capabilities(_) => "capability manifest does not match the content",
            Self::BadSignature(_) => "a signature does not verify",
            Self::Unsigned => "unsigned, and this box takes no dev bundles",
            Self::Untrusted(_) => "not signed by a trusted publisher",
        };
        write!(f, "bundle refused: {what}: {}", self.problems().join("; "))
    }
}

impl std::error::Error for Error {}

/// A decoded bundle: content addressing and framing checked, nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    pub manifest: BundleManifest,
    pub content: ChangeSet,
    pub signatures: Vec<Signature>,
}

fn sha256(b: &[u8]) -> [u8; 32] {
    let mut o = [0u8; 32];
    o.copy_from_slice(&Sha256::digest(b));
    o
}

fn hex32(s: &str) -> Option<[u8; 32]> {
    sign::parse_key(s)
}

struct Reader<'a> {
    rest: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn peek(&self) -> Option<u8> {
        self.rest.first().copied()
    }

    /// The next record of type `tag`, at most `cap` bytes.
    fn record(&mut self, what: &str, cap: usize) -> Result<&'a [u8], Error> {
        if self.rest.len() < 5 {
            return Err(Error::Malformed(format!(
                "truncated: {what} record header at byte {} is cut short",
                self.at
            )));
        }
        let len = u32::from_be_bytes([self.rest[1], self.rest[2], self.rest[3], self.rest[4]]) as usize;
        if len > cap {
            return Err(Error::Malformed(format!(
                "{what} record at byte {} claims {len} bytes, more than a {what} may hold ({cap})",
                self.at
            )));
        }
        if len > self.rest.len() - 5 {
            return Err(Error::Malformed(format!(
                "truncated: {what} record at byte {} claims {len} bytes, only {} remain",
                self.at,
                self.rest.len() - 5
            )));
        }
        let data = &self.rest[5..5 + len];
        self.rest = &self.rest[5 + len..];
        self.at += 5 + len;
        Ok(data)
    }
}

fn utf8<'a>(b: &'a [u8], what: &str) -> Result<&'a str, Error> {
    std::str::from_utf8(b).map_err(|_| Error::Malformed(format!("{what} is not UTF-8")))
}

/// What a tree names, in order.
enum Line {
    Module(String, [u8; 32], [u8; 32]),
    Map([u8; 32]),
    Calendar([u8; 32]),
}

fn tree_lines(tree: &str) -> Result<Vec<Line>, Error> {
    let bad = |w: String| Error::Malformed(format!("tree: {w}"));
    let mut lines = tree.split('\n');
    if lines.next() != Some(TREE_HEADER) {
        return Err(bad(format!("the first line must be {TREE_HEADER:?}")));
    }
    let mut out = Vec::new();
    let mut modules = BTreeSet::new();
    let hash = |h: &str| hex32(h).ok_or_else(|| bad(format!("{h:?} is not a SHA-256 in lowercase hex")));
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let w: Vec<&str> = line.split(' ').collect();
        match w.as_slice() {
            ["module", name, m, s] => {
                if !name_ok(name) {
                    return Err(bad(format!(
                        "{name:?} is not a module name (1..=64 of A-Z a-z 0-9 _ -)"
                    )));
                }
                if !modules.insert(*name) {
                    return Err(bad(format!("module {name} is named twice")));
                }
                if modules.len() > MAX_MODULES {
                    return Err(bad(format!("more than {MAX_MODULES} modules")));
                }
                out.push(Line::Module((*name).to_string(), hash(m)?, hash(s)?));
            }
            ["io-map", h] => out.push(Line::Map(hash(h)?)),
            ["calendar", h] => out.push(Line::Calendar(hash(h)?)),
            _ => {
                let shown: String = line.chars().take(40).flat_map(char::escape_debug).collect();
                return Err(bad(format!("unknown line \"{shown}\"")));
            }
        }
    }
    Ok(out)
}

impl Bundle {
    /// A bundle of `content` with `manifest`, unsigned.
    #[must_use]
    pub const fn new(manifest: BundleManifest, content: ChangeSet) -> Self {
        Self {
            manifest,
            content,
            signatures: Vec::new(),
        }
    }

    /// The content's snapshot id.
    #[must_use]
    pub fn id(&self) -> SnapshotId {
        self.content.id()
    }

    /// What every signature on it covers.
    #[must_use]
    pub fn message(&self) -> Vec<u8> {
        sign::message(&self.id(), &self.manifest.render())
    }

    /// Adds `key`'s signature (replacing an earlier one by the same key).
    pub fn sign(&mut self, key: &SigningKey) {
        let s = key.sign(&self.message());
        self.signatures.retain(|x| x.key != s.key);
        self.signatures.push(s);
        self.signatures.sort_by_key(|x| x.key);
    }

    /// The file.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        let mut put = |tag: u8, data: &[u8]| {
            out.push(tag);
            out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_be_bytes());
            out.extend_from_slice(data);
        };
        put(b'T', self.content.tree().as_bytes());
        put(b'M', self.manifest.render().as_bytes());
        let blobs: BTreeMap<[u8; 32], &str> = self
            .content
            .modules
            .values()
            .flat_map(|f| [f.manifest.as_str(), f.source.as_str()])
            .chain(self.content.io_map.as_deref())
            .chain(self.content.calendar.as_deref())
            .map(|b| (sha256(b.as_bytes()), b))
            .collect();
        for b in blobs.values() {
            put(b'B', b.as_bytes());
        }
        let mut sigs = self.signatures.clone();
        sigs.sort_by_key(|s| s.key);
        sigs.dedup_by_key(|s| s.key);
        for s in &sigs {
            let mut d = s.key.to_vec();
            d.extend_from_slice(&s.sig);
            put(b'S', &d);
        }
        out
    }

    /// Reads a bundle file: framing, sizes, order, the tree against the
    /// blobs, the manifest's syntax. Signatures and content are checked by
    /// [`verify`].
    ///
    /// # Errors
    /// [`Error::Malformed`], [`Error::Tampered`] or [`Error::Manifest`].
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BUNDLE {
            return Err(Error::Malformed(format!(
                "{} bytes, more than a bundle may be ({MAX_BUNDLE})",
                bytes.len()
            )));
        }
        let Some(body) = bytes.strip_prefix(MAGIC) else {
            return Err(Error::Malformed(if bytes.starts_with(b"helmsman-bundle ") {
                "a bundle format version this build does not read (it reads 1)".into()
            } else {
                "not a Helmsman bundle (bad header)".into()
            }));
        };
        let mut r = Reader {
            rest: body,
            at: MAGIC.len(),
        };
        let expect = |r: &Reader<'_>, tag: u8, what: &str| match r.peek() {
            Some(t) if t == tag => Ok(()),
            Some(t) => Err(Error::Malformed(format!(
                "byte {}: expected the {what} record, found record type 0x{t:02x}",
                r.at
            ))),
            None => Err(Error::Malformed(format!("truncated: no {what} record"))),
        };
        expect(&r, b'T', "tree")?;
        let tree = utf8(r.record("tree", MAX_TREE)?, "the tree")?;
        expect(&r, b'M', "manifest")?;
        let manifest_text = utf8(r.record("manifest", MAX_MANIFEST)?, "the manifest")?;
        let cap = usize::try_from(MAX_FILE).unwrap_or(usize::MAX);
        // In file order; their order is checked after the tree, so a
        // changed file reads as what it is (tampered), not as misordered.
        let mut order: Vec<(usize, [u8; 32])> = Vec::new();
        let mut blobs: BTreeMap<[u8; 32], &[u8]> = BTreeMap::new();
        while r.peek() == Some(b'B') {
            let at = r.at;
            if blobs.len() >= MAX_BLOBS {
                return Err(Error::Malformed(format!("more than {MAX_BLOBS} blobs")));
            }
            let data = r.record("blob", cap)?;
            let h = sha256(data);
            if blobs.insert(h, data).is_some() {
                return Err(Error::Malformed(format!(
                    "byte {at}: blob {} appears twice",
                    sign::hex(&h)
                )));
            }
            order.push((at, h));
        }
        let mut signatures: Vec<Signature> = Vec::new();
        while r.peek() == Some(b'S') {
            let at = r.at;
            let data = r.record("signature", SIG_LEN)?;
            if data.len() != SIG_LEN {
                return Err(Error::Malformed(format!(
                    "byte {at}: a signature record is {SIG_LEN} bytes (key and signature), not {}",
                    data.len()
                )));
            }
            let mut s = Signature {
                key: [0; 32],
                sig: [0; 64],
            };
            s.key.copy_from_slice(&data[..32]);
            s.sig.copy_from_slice(&data[32..]);
            if let Some(last) = signatures.last() {
                if last.key == s.key {
                    return Err(Error::Malformed(format!(
                        "byte {at}: two signatures by key {}",
                        sign::hex(&s.key)
                    )));
                }
                if last.key > s.key {
                    return Err(Error::Malformed(format!(
                        "byte {at}: signatures are not in ascending key order"
                    )));
                }
            }
            if signatures.len() >= MAX_SIGNATURES {
                return Err(Error::Malformed(format!("more than {MAX_SIGNATURES} signatures")));
            }
            signatures.push(s);
        }
        if let Some(t) = r.peek() {
            return Err(Error::Malformed(format!(
                "byte {}: unexpected record type 0x{t:02x} (or a record out of order)",
                r.at
            )));
        }
        let content = content(tree, &blobs)?;
        if let Some(w) = order.windows(2).find(|w| w[0].1 > w[1].1) {
            return Err(Error::Malformed(format!(
                "byte {}: blobs are not in ascending hash order",
                w[1].0
            )));
        }
        let manifest = BundleManifest::parse(manifest_text).map_err(Error::Manifest)?;
        Ok(Self {
            manifest,
            content,
            signatures,
        })
    }
}

/// The change set a tree and its blobs make.
fn content(tree: &str, blobs: &BTreeMap<[u8; 32], &[u8]>) -> Result<ChangeSet, Error> {
    let lines = tree_lines(tree)?;
    let mut used = BTreeSet::new();
    let mut file = |h: [u8; 32], what: &str| -> Result<String, Error> {
        let data = blobs.get(&h).ok_or_else(|| {
            Error::Tampered(format!(
                "{what}: the tree names blob {}, which the bundle does not carry (a file was altered or removed)",
                sign::hex(&h)
            ))
        })?;
        used.insert(h);
        String::from_utf8(data.to_vec()).map_err(|_| Error::Malformed(format!("{what} is not UTF-8")))
    };
    let mut cs = ChangeSet::default();
    for l in lines {
        match l {
            Line::Module(name, m, s) => {
                let files = ModuleFiles {
                    manifest: file(m, &format!("module {name} manifest"))?,
                    source: file(s, &format!("module {name} source"))?,
                };
                cs.modules.insert(name, files);
            }
            Line::Map(h) => cs.io_map = Some(file(h, "io-map")?),
            Line::Calendar(h) => cs.calendar = Some(file(h, "calendar")?),
        }
    }
    if let Some(h) = blobs.keys().find(|h| !used.contains(*h)) {
        return Err(Error::Tampered(format!(
            "blob {} is not named by the tree (a file was altered or added)",
            sign::hex(h)
        )));
    }
    if cs.tree() != tree {
        return Err(Error::Malformed(
            "tree: not in canonical form (header, modules by name, io-map, calendar, one space, a newline after each line)"
                .into(),
        ));
    }
    Ok(cs)
}

/// How a verified bundle is trusted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Signed by this publisher, whom the box trusts.
    Publisher { name: String, key: String },
    /// A dev bundle: unsigned or signed by no trusted publisher, taken
    /// only because the trust store says `dev-bundles allow`.
    Dev { signers: Vec<String> },
}

impl Trust {
    /// One line for people and the audit log.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Publisher { name, .. } => format!("signed by {name}"),
            Self::Dev { signers } if signers.is_empty() => "DEV BUNDLE (unsigned)".into(),
            Self::Dev { .. } => "DEV BUNDLE (signed by no trusted publisher)".into(),
        }
    }
}

/// A bundle that passed [`verify`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub bundle: Bundle,
    pub trust: Trust,
    /// What it can touch, as computed (equal to its manifest's).
    pub caps: Capabilities,
}

/// Checks the signatures of `b`: every one must verify.
///
/// # Errors
/// [`Error::BadSignature`] naming the first that does not.
pub fn check_signatures(b: &Bundle) -> Result<(), Error> {
    let msg = b.message();
    for s in &b.signatures {
        if !sign::verifies(s, &msg) {
            return Err(Error::BadSignature(format!(
                "the signature by key {} does not verify over this content and manifest",
                sign::hex(&s.key)
            )));
        }
    }
    Ok(())
}

/// Checks `b`'s content and that its capability manifest is exactly what
/// the content can touch; `d` sizes `%M` where a manifest does not.
///
/// # Errors
/// [`Error::Content`] or [`Error::Capabilities`].
pub fn check_content(b: &Bundle, d: &Defaults) -> Result<Capabilities, Error> {
    let caps = capabilities(&b.content, d).map_err(Error::Content)?;
    let diff = caps.differences(&b.manifest.caps);
    if diff.is_empty() {
        Ok(caps)
    } else {
        Err(Error::Capabilities(diff))
    }
}

/// Everything the host checks before a bundle may become a proposal, in
/// order: the file ([`Bundle::decode`]), every signature, the signer
/// against `trust`, then the content and its capability manifest.
///
/// # Errors
/// The first check that fails, with every reason it has.
pub fn verify(bytes: &[u8], trust: &TrustStore, d: &Defaults) -> Result<Verified, Error> {
    let bundle = Bundle::decode(bytes)?;
    check_signatures(&bundle)?;
    let trusted = bundle.signatures.iter().find_map(|s| {
        trust
            .publisher(&s.key)
            .map(|n| (n.to_string(), sign::hex(&s.key)))
    });
    let signers: Vec<String> = bundle.signatures.iter().map(|s| sign::hex(&s.key)).collect();
    let trust = match trusted {
        Some((name, key)) => Trust::Publisher { name, key },
        None if trust.dev_bundles => Trust::Dev { signers },
        None if signers.is_empty() => return Err(Error::Unsigned),
        None => return Err(Error::Untrusted(signers)),
    };
    let caps = check_content(&bundle, d)?;
    Ok(Verified { bundle, trust, caps })
}
