//! Role PINs: stored as PBKDF2-HMAC-SHA256 hashes with a random salt each,
//! checked in constant time, behind a lockout that stops checking after a
//! few wrong PINs and backs off.
//!
//! **Why PBKDF2 here, written out.** The box already carries `sha2` (the
//! snapshot store's content addresses), and HMAC and PBKDF2 over it are a
//! few dozen lines checked below against the published test vectors
//! (RFC 7914 section 11), so this adds no crate. A memory-hard KDF
//! (argon2) would cost an attacker with the file more, but a PIN has so
//! little entropy that no KDF saves one whose hash file is stolen: what
//! protects a PIN is that the file is private (mode 0600 in the state
//! directory) and that the only way to try one is the box API, where the
//! [`Gate`] locks entry after [`super::Lockout::attempts`] wrong PINs.
//!
//! **File** (`pins` in the state directory, mode 0600), one role per line:
//!
//! ```text
//! helmsman-pins 1
//! engineer pbkdf2-sha256 600000 <salt, 32 hex> <hash, 64 hex>
//! ```
//!
//! A PIN never reaches a log, the audit log or a response: the API reads it
//! from a request body and hands it here, and nothing here prints it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::{Lockout, MAX_LOCKOUT, Role};

/// The PIN file's name in the state directory.
pub const FILE: &str = "pins";
/// The PIN file's first line.
pub const HEADER: &str = "helmsman-pins 1";
/// Iterations for a PIN set now (OWASP's 2023 figure for PBKDF2-SHA256).
pub const ITERATIONS: u32 = 600_000;
/// Fewest iterations a PIN file may carry for the box to load it.
pub const MIN_ITERATIONS: u32 = 100_000;
/// Shortest and longest PIN, in characters (printable ASCII, no blanks).
pub const PIN_LEN: (usize, usize) = (6, 64);
/// A PIN typed longer than this, in bytes, is wrong without being hashed
/// as given (PBKDF2 rehashes a key longer than a block every iteration).
pub const MAX_PIN_BYTES: usize = 1024;

const BLOCK: usize = 64;

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    for p in parts {
        inner.update(p);
    }
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

/// PBKDF2-HMAC-SHA256 (RFC 8018 section 5.2), `out.len()` bytes.
pub fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    for (n, chunk) in out.chunks_mut(32).enumerate() {
        let block = u32::try_from(n + 1).expect("fewer than 2^32 blocks");
        let mut u = hmac(password, &[salt, &block.to_be_bytes()]);
        let mut t = u;
        for _ in 1..iterations {
            u = hmac(password, &[&u]);
            for (a, b) in t.iter_mut().zip(u) {
                *a ^= b;
            }
        }
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
}

/// One stored secret (a role's PIN, an account's password): PBKDF2 with
/// its own salt and iteration count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub(crate) iterations: u32,
    pub(crate) salt: [u8; 16],
    pub(crate) hash: [u8; 32],
}

impl Record {
    pub(crate) fn new(pin: &str, iterations: u32, salt: [u8; 16]) -> Self {
        let mut hash = [0u8; 32];
        pbkdf2(pin.as_bytes(), &salt, iterations, &mut hash);
        Self {
            iterations,
            salt,
            hash,
        }
    }

    /// Slow by design (one PBKDF2); the comparison takes the same time
    /// wherever the hashes differ.
    pub(crate) fn matches(&self, pin: &str) -> bool {
        let mut h = [0u8; 32];
        pbkdf2(pin.as_bytes(), &self.salt, self.iterations, &mut h);
        ct_eq(&h, &self.hash)
    }

    /// `pbkdf2-sha256 ITERATIONS SALT HASH`, as the files store it.
    pub(crate) fn render(&self) -> String {
        format!(
            "pbkdf2-sha256 {} {} {}",
            self.iterations,
            hex(&self.salt),
            hex(&self.hash)
        )
    }

    /// [`Record::render`]'s four words back.
    pub(crate) fn parse(words: &[&str]) -> Option<Self> {
        match words {
            ["pbkdf2-sha256", iter, salt, hash] => Some(Self {
                iterations: iter.parse().ok()?,
                salt: unhex::<16>(salt)?,
                hash: unhex::<32>(hash)?,
            }),
            _ => None,
        }
    }
}

/// Whether `a == b`, in time that depends only on their lengths.
#[must_use]
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether `pin` may be set: [`PIN_LEN`] printable ASCII, no blanks.
#[must_use]
pub fn acceptable(pin: &str) -> bool {
    (PIN_LEN.0..=PIN_LEN.1).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_graphic())
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; N];
    for (k, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[k * 2..k * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// 16 bytes from the OS.
///
/// # Errors
/// `/dev/urandom` cannot be read.
pub fn salt() -> io::Result<[u8; 16]> {
    let mut b = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b)
}

/// The roles' PINs. `Debug` shows which roles have one, nothing more.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Pins {
    records: BTreeMap<Role, Record>,
}

impl std::fmt::Debug for Pins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.records.keys()).finish()
    }
}

impl Pins {
    /// Sets `role`'s PIN, hashed with `iterations` and `salt`.
    ///
    /// # Errors
    /// The PIN is not [`acceptable`], or `iterations` is zero.
    pub fn set(&mut self, role: Role, pin: &str, iterations: u32, salt: [u8; 16]) -> Result<(), String> {
        if !acceptable(pin) {
            return Err(format!(
                "a PIN is {}..={} printable characters, no blanks",
                PIN_LEN.0, PIN_LEN.1
            ));
        }
        if iterations == 0 {
            return Err("iterations must be at least 1".into());
        }
        self.records.insert(role, Record::new(pin, iterations, salt));
        Ok(())
    }

    #[must_use]
    pub fn has(&self, role: Role) -> bool {
        self.records.contains_key(&role)
    }

    /// Whether `pin` is `role`'s PIN. Slow by design (one PBKDF2).
    #[must_use]
    pub fn verify(&self, role: Role, pin: &str) -> bool {
        self.records.get(&role).is_some_and(|r| r.matches(pin))
    }

    /// The file [`Pins::parse`] reads.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = format!("{HEADER}\n");
        for (role, r) in &self.records {
            let _ = writeln!(
                s,
                "{role} pbkdf2-sha256 {} {} {}",
                r.iterations,
                hex(&r.salt),
                hex(&r.hash)
            );
        }
        s
    }

    /// A PIN file; every record must use at least `min_iterations`.
    ///
    /// # Errors
    /// Every bad line, by number.
    pub fn parse(src: &str, min_iterations: u32) -> Result<Self, Vec<(usize, String)>> {
        let mut pins = Self::default();
        let mut errs = Vec::new();
        let mut lines = src.lines().enumerate().filter(|(_, l)| {
            let l = l.trim();
            !l.is_empty() && !l.starts_with('#')
        });
        if lines.next().map(|(_, l)| l.trim()) != Some(HEADER) {
            errs.push((1, format!("expected {HEADER:?} first")));
        }
        for (n, line) in lines {
            let n = n + 1;
            let w: Vec<&str> = line.split_whitespace().collect();
            let rec = match w.as_slice() {
                [role, "pbkdf2-sha256", iter, salt, hash] => Role::parse(role)
                    .zip(iter.parse::<u32>().ok())
                    .zip(unhex::<16>(salt).zip(unhex::<32>(hash)))
                    .map(|((role, iterations), (salt, hash))| {
                        (
                            role,
                            Record {
                                iterations,
                                salt,
                                hash,
                            },
                        )
                    }),
                _ => None,
            };
            match rec {
                None => errs.push((n, "expected: ROLE pbkdf2-sha256 ITERATIONS SALT HASH".into())),
                Some((_, r)) if r.iterations < min_iterations => errs.push((
                    n,
                    format!("{} iterations, fewer than {min_iterations}", r.iterations),
                )),
                Some((role, _)) if pins.records.contains_key(&role) => {
                    errs.push((n, format!("{role} given twice")));
                }
                Some((role, r)) => {
                    pins.records.insert(role, r);
                }
            }
        }
        if errs.is_empty() { Ok(pins) } else { Err(errs) }
    }

    /// Reads `dir/pins`; no file is no PINs (nothing can be applied).
    ///
    /// # Errors
    /// The file cannot be read, is not private, or does not parse.
    pub fn load(dir: &Path, min_iterations: u32) -> Result<Self, String> {
        let path = dir.join(FILE);
        let Some(src) = read_private(&path)? else {
            return Ok(Self::default());
        };
        Self::parse(&src, min_iterations).map_err(|errs| {
            let lines: Vec<String> = errs
                .iter()
                .map(|(n, e)| format!("\n  {}:{n}: {e}", path.display()))
                .collect();
            format!("{} does not parse:{}", path.display(), lines.concat())
        })
    }

    /// Writes `dir/pins`, private, whole: a reader never sees half a file.
    ///
    /// # Errors
    /// The directory or file cannot be written.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        write_private(dir, FILE, &self.render())
    }
}

/// `path`'s text, if it exists; refused if anyone but its owner may read
/// or write it.
pub(crate) fn read_private(path: &Path) -> Result<Option<String>, String> {
    let src = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
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
    Ok(Some(src))
}

/// Writes `dir/name`, mode 0600, whole: a reader never sees half a file.
pub(crate) fn write_private(dir: &Path, name: &str, text: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{name}.{}", std::process::id()));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, dir.join(name))
}

/// Why a PIN was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Entry is locked; nothing was checked. Try again after this long.
    Locked(Duration),
    /// Wrong PIN (or no PIN set for the role); counted toward lockout.
    Wrong,
}

/// Where a [`Gate`] reads the time; a test can move it.
pub type Clock = std::sync::Arc<dyn Fn() -> Instant + Send + Sync>;

struct GateState {
    /// Wrong PINs since the last right one.
    failures: u32,
    /// No PIN is checked before this.
    locked_until: Option<Instant>,
}

/// PIN entry for the whole box: one lockout across every role and
/// client, so spreading guesses over roles or connections gains nothing.
/// Checks are serialized (one PBKDF2 at a time), so parallel guesses are
/// no faster than serial ones.
pub struct Gate {
    pins: Pins,
    lockout: Lockout,
    clock: Clock,
    state: Mutex<GateState>,
    /// Per-person logins ([`super::account`]); once the box has an
    /// accounts file they replace the PINs.
    logins: Option<super::account::Logins>,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("pins", &self.pins)
            .field("lockout", &self.lockout)
            .field("logins", &self.logins)
            .finish_non_exhaustive()
    }
}

impl Gate {
    #[must_use]
    pub fn new(pins: Pins, lockout: Lockout, clock: Clock) -> Self {
        Self {
            pins,
            lockout,
            clock,
            state: Mutex::new(GateState {
                failures: 0,
                locked_until: None,
            }),
            logins: None,
        }
    }

    /// The same gate, with per-person logins: while their accounts file
    /// exists, the box takes sessions and refuses PINs.
    #[must_use]
    pub fn with_logins(mut self, logins: super::account::Logins) -> Self {
        self.logins = Some(logins);
        self
    }

    #[must_use]
    pub const fn logins(&self) -> Option<&super::account::Logins> {
        self.logins.as_ref()
    }

    /// Whether `pin` is `role`'s PIN, unless entry is locked. A wrong PIN
    /// past the allowance locks entry for [`Lockout::first`], doubled for
    /// each further wrong one, up to [`MAX_LOCKOUT`]; a right PIN clears the
    /// count.
    ///
    /// # Errors
    /// [`Refused`].
    pub fn check(&self, role: Role, pin: &str) -> Result<(), Refused> {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = (self.clock)();
        if let Some(until) = st.locked_until {
            if now < until {
                return Err(Refused::Locked(until - now));
            }
            st.locked_until = None;
        }
        // An over-long PIN is wrong, but costs the same PBKDF2.
        let long = pin.len() > MAX_PIN_BYTES;
        if self.pins.verify(role, if long { "" } else { pin }) && !long {
            st.failures = 0;
            return Ok(());
        }
        st.failures = st.failures.saturating_add(1);
        if let Some(over) = st.failures.checked_sub(self.lockout.attempts) {
            let factor = 1u32.checked_shl(over.min(31)).unwrap_or(u32::MAX);
            let wait = self.lockout.first.saturating_mul(factor).min(MAX_LOCKOUT);
            st.locked_until = Some(now + wait);
        }
        Err(Refused::Wrong)
    }

    #[must_use]
    pub const fn pins(&self) -> &Pins {
        &self.pins
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|k| u8::from_str_radix(&s[k * 2..k * 2 + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn pbkdf2_matches_rfc7914() {
        // RFC 7914 section 11: PBKDF2-HMAC-SHA256 test vectors.
        let mut out = [0u8; 64];
        pbkdf2(b"passwd", b"salt", 1, &mut out);
        assert_eq!(
            out.to_vec(),
            h("55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc\
               49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783")
        );
        pbkdf2(b"Password", b"NaCl", 80_000, &mut out);
        assert_eq!(
            out.to_vec(),
            h("4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56\
               a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d")
        );
    }

    #[test]
    fn hmac_matches_rfc4231_case_2() {
        assert_eq!(
            hmac(b"Jefe", &[b"what do ya want ", b"for nothing?"]).to_vec(),
            h("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
    }

    #[test]
    fn a_long_key_is_hashed_first() {
        // RFC 4231 case 6: a 131-byte key.
        let key = [0xaau8; 131];
        assert_eq!(
            hmac(&key, &[b"Test Using Larger Than Block-Size Key - Hash Key First"]).to_vec(),
            h("60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54")
        );
    }
}
