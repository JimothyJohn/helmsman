//! Ed25519 signatures over bundles, publisher keys, and the box's trust
//! store.
//!
//! **Why `ring`.** It is already in the lockfile (rustls's crypto provider,
//! behind the `https` feature) and already compiled into every default box
//! build, so signing adds no crate to `Cargo.lock`; it is the `BoringSSL`
//! lineage of Ed25519, verifies strictly (an `S` at or above the group
//! order is refused, so a signature has one encoding), and generates keys
//! from the OS's RNG. `ed25519-dalek` would add `curve25519-dalek`,
//! `ed25519`, `signature` and a second `sha2` beside the store's. The cost:
//! the `box` feature now pulls `ring` (C and assembly, built with `cc`, as
//! `rusqlite`'s bundled `SQLite` already is) even with `https` off.
//!
//! **What is signed**: [`message`], a domain-separated line plus the
//! bundle's id (the SHA-256 of its canonical tree, so every blob) and the
//! SHA-256 of its manifest. The framing is not signed; it carries nothing
//! the tree and manifest do not pin.
//!
//! **Publisher keys** are PKCS#8 (RFC 5958 v2, as `ring` writes them; v1
//! from `openssl genpkey -algorithm ed25519 -outform DER` reads too),
//! written mode 0600 and refused if readable by others. They live with the
//! publisher, never on a box and never in a repository.
//!
//! **Trust store** ([`TRUST_FILE`] in the state directory, mode 0600, like
//! the PIN file):
//!
//! ```text
//! helmsman-trust 1
//! publisher acme ed25519 <public key, 64 hex>
//! dev-bundles allow       # optional; default refuse
//! ```
//!
//! A bundle imports when at least one of its signatures is by a listed
//! publisher and every signature on it verifies. `dev-bundles allow` lets
//! an unsigned or otherwise-signed bundle import too, marked as a dev
//! bundle wherever it shows; it never lets a broken signature through.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use sha2::{Digest, Sha256};

use crate::spec::name_ok;
use crate::store::SnapshotId;

/// The trust store's file name in the state directory.
pub const TRUST_FILE: &str = "trusted-publishers";
/// The trust store's first line.
pub const TRUST_HEADER: &str = "helmsman-trust 1";
/// Most publishers a trust store holds.
pub const MAX_PUBLISHERS: usize = 256;
/// What every signed message starts with.
pub const DOMAIN: &[u8] = b"helmsman-bundle-signature 1\n";

/// An Ed25519 public key.
pub type PublicKey = [u8; 32];

/// One detached signature: who and what.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    pub key: PublicKey,
    pub sig: [u8; 64],
}

/// The bytes a signature covers.
#[must_use]
pub fn message(id: &SnapshotId, manifest: &str) -> Vec<u8> {
    let mut m = DOMAIN.to_vec();
    m.extend_from_slice(id.as_bytes());
    m.extend_from_slice(&Sha256::digest(manifest.as_bytes()));
    m
}

/// Whether `s` is a valid signature of `msg`.
#[must_use]
pub fn verifies(s: &Signature, msg: &[u8]) -> bool {
    UnparsedPublicKey::new(&ED25519, &s.key)
        .verify(msg, &s.sig)
        .is_ok()
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// A public key as 64 lowercase hex digits (as trust stores carry it).
#[must_use]
pub fn hex_key(k: &PublicKey) -> String {
    hex(k)
}

/// 64 lowercase hex digits.
#[must_use]
pub fn parse_key(s: &str) -> Option<PublicKey> {
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0u8; 32];
    for (k, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[k * 2..k * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// A publisher's private key.
pub struct SigningKey(Ed25519KeyPair);

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SigningKey({})", hex(&self.public()))
    }
}

impl SigningKey {
    /// A new key from the OS's RNG, and its PKCS#8 document.
    ///
    /// # Errors
    /// The RNG failed.
    pub fn generate() -> Result<(Self, Vec<u8>), String> {
        let doc = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| "could not generate a key (OS RNG)".to_string())?;
        let k = Self::from_pkcs8(doc.as_ref())?;
        Ok((k, doc.as_ref().to_vec()))
    }

    /// From a PKCS#8 document.
    ///
    /// # Errors
    /// It is not an Ed25519 PKCS#8 key.
    pub fn from_pkcs8(der: &[u8]) -> Result<Self, String> {
        Ed25519KeyPair::from_pkcs8_maybe_unchecked(der)
            .map(Self)
            .map_err(|e| format!("not an Ed25519 PKCS#8 key: {e}"))
    }

    /// From a 32-byte seed: deterministic keys for tests.
    ///
    /// # Errors
    /// Never for 32 bytes; kept fallible as `ring`'s is.
    pub fn from_seed(seed: &[u8; 32]) -> Result<Self, String> {
        Ed25519KeyPair::from_seed_unchecked(seed)
            .map(Self)
            .map_err(|e| format!("bad seed: {e}"))
    }

    #[must_use]
    pub fn public(&self) -> PublicKey {
        let mut k = [0u8; 32];
        k.copy_from_slice(self.0.public_key().as_ref());
        k
    }

    #[must_use]
    pub fn sign(&self, msg: &[u8]) -> Signature {
        let mut sig = [0u8; 64];
        sig.copy_from_slice(self.0.sign(msg).as_ref());
        Signature {
            key: self.public(),
            sig,
        }
    }

    /// Reads a key file, refusing one others can read.
    ///
    /// # Errors
    /// The file cannot be read, is not private, or is not a key.
    pub fn load(path: &Path) -> Result<Self, String> {
        private(path)?;
        let der = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_pkcs8(&der).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Refuses a file group or others can read (Unix).
fn private(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(format!(
                "{}: readable by others (mode {:o}); chmod 600 it",
                path.display(),
                mode & 0o777
            ));
        }
    }
    let _ = path;
    Ok(())
}

/// Writes `data` to `path`, mode 0600; `fresh` refuses an existing file.
///
/// # Errors
/// The file exists (when `fresh`) or cannot be written.
pub fn write_private(path: &Path, data: &[u8], fresh: bool) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true);
    if fresh {
        opts.create_new(true);
    } else {
        opts.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

/// Publishers this box trusts, and whether it takes dev bundles.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustStore {
    /// By name.
    publishers: BTreeMap<String, PublicKey>,
    /// `dev-bundles allow`: unsigned or untrusted bundles import, marked.
    pub dev_bundles: bool,
}

impl TrustStore {
    /// The publisher who holds `key`, if trusted.
    #[must_use]
    pub fn publisher(&self, key: &PublicKey) -> Option<&str> {
        self.publishers
            .iter()
            .find(|(_, k)| *k == key)
            .map(|(n, _)| n.as_str())
    }

    /// Every publisher, by name.
    pub fn publishers(&self) -> impl Iterator<Item = (&str, &PublicKey)> {
        self.publishers.iter().map(|(n, k)| (n.as_str(), k))
    }

    /// Trusts `key` as `name`.
    ///
    /// # Errors
    /// A bad name, a name or key already listed, or a full store.
    pub fn add(&mut self, name: &str, key: PublicKey) -> Result<(), String> {
        if !name_ok(name) {
            return Err(format!(
                "{name:?} is not a publisher name (1..=64 of A-Z a-z 0-9 _ -)"
            ));
        }
        if self.publishers.contains_key(name) {
            return Err(format!("publisher {name} is already listed"));
        }
        if let Some(other) = self.publisher(&key) {
            return Err(format!("that key is already listed as {other}"));
        }
        if self.publishers.len() >= MAX_PUBLISHERS {
            return Err(format!("more than {MAX_PUBLISHERS} publishers"));
        }
        self.publishers.insert(name.to_string(), key);
        Ok(())
    }

    #[must_use]
    pub fn render(&self) -> String {
        let mut s = format!("{TRUST_HEADER}\n");
        for (n, k) in &self.publishers {
            let _ = writeln!(s, "publisher {n} ed25519 {}", hex(k));
        }
        if self.dev_bundles {
            s.push_str("dev-bundles allow\n");
        }
        s
    }

    /// # Errors
    /// Every problem, as `(line, why)`.
    pub fn parse(src: &str) -> Result<Self, Vec<(usize, String)>> {
        let mut t = Self::default();
        let mut errs = Vec::new();
        let mut lines = src.lines().enumerate().map(|(n, l)| (n + 1, l));
        if lines.next().map(|(_, l)| l.trim()) != Some(TRUST_HEADER) {
            return Err(vec![(1, format!("the first line must be {TRUST_HEADER:?}"))]);
        }
        let mut dev = None;
        for (n, raw) in lines {
            let line = raw.split('#').next().unwrap_or_default();
            let w: Vec<&str> = line.split_whitespace().collect();
            match w.as_slice() {
                [] => {}
                ["publisher", name, "ed25519", key] => match parse_key(key) {
                    Some(k) => {
                        if let Err(e) = t.add(name, k) {
                            errs.push((n, e));
                        }
                    }
                    None => errs.push((n, "the key must be 64 lowercase hex digits".into())),
                },
                ["publisher", ..] => errs.push((n, "want: publisher NAME ed25519 KEY".into())),
                ["dev-bundles", v @ ("allow" | "refuse")] if dev.is_none() => dev = Some(*v == "allow"),
                ["dev-bundles", "allow" | "refuse"] => errs.push((n, "dev-bundles given twice".into())),
                ["dev-bundles", ..] => errs.push((n, "want: dev-bundles allow|refuse".into())),
                _ => errs.push((n, format!("unknown line {:?}", w[0]))),
            }
        }
        t.dev_bundles = dev.unwrap_or(false);
        if errs.is_empty() { Ok(t) } else { Err(errs) }
    }

    /// Reads `dir/trusted-publishers`; no file trusts nobody and takes no
    /// dev bundles.
    ///
    /// # Errors
    /// The file cannot be read, is not private, or does not parse.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(TRUST_FILE);
        let src = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        private(&path)?;
        Self::parse(&src).map_err(|errs| {
            let lines: Vec<String> = errs
                .iter()
                .map(|(n, e)| format!("\n  {}:{n}: {e}", path.display()))
                .collect();
            format!("{} does not parse:{}", path.display(), lines.concat())
        })
    }

    /// Writes `dir/trusted-publishers`, private, whole.
    ///
    /// # Errors
    /// The directory or file cannot be written.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{TRUST_FILE}.{}", std::process::id()));
        write_private(&tmp, self.render().as_bytes(), false)?;
        fs::rename(&tmp, dir.join(TRUST_FILE))
    }
}
