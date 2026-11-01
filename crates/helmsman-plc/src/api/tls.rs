//! The box's own HTTPS (`box-https`): a self-signed certificate made at
//! first start, kept in the state directory, and its SHA-256 fingerprint,
//! which the box prints at start (the journal) and shows on its page so a
//! person can check what the browser shows before trusting it once.
//!
//! The certificate is written here, not by a crate: a few hundred bytes of
//! DER around a P-256 key from `ring` (already in the box for bundle
//! signatures and random tokens) signed with ECDSA-SHA256. X.509 v3
//! (RFC 5280): subject and issuer `CN=helmsman-plc`, valid from an hour
//! before it was made to 9999-12-31 (RFC 5280 4.1.2.5, "no well-defined
//! expiration date"), `basicConstraints` CA false, `keyUsage`
//! digitalSignature, `extKeyUsage` serverAuth, and `subjectAltName`
//! `localhost`, `127.0.0.1`, `::1` and the host's name when it has one. A
//! browser still warns (no CA vouches for it); the fingerprint is how the
//! person knows it is this box's.
//!
//! [`FILE`] (`tls.pem`, mode 0600) holds the certificate and its PKCS#8
//! key as PEM, so `openssl x509 -in tls.pem -noout -fingerprint -sha256`
//! prints the same fingerprint. Made once, atomically (two first starts at
//! once settle on one), then only read. A file that does not hold a
//! matching certificate and key stops the box rather than being replaced:
//! a new certificate means every browser must trust it again, so that is
//! the operator's call (delete the file).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

use super::photo::{base64, unbase64};

/// The certificate and key file's name in the state directory.
pub const FILE: &str = "tls.pem";

const CERT_PEM: &str = "CERTIFICATE";
const KEY_PEM: &str = "PRIVATE KEY";

/// The box's certificate and key.
#[derive(Clone, PartialEq, Eq)]
pub struct Identity {
    /// The certificate, DER.
    pub cert: Vec<u8>,
    /// Its key, PKCS#8 DER.
    key: Vec<u8>,
    /// Where it lives.
    pub path: PathBuf,
    /// Made by this call (the first start), or read from an earlier one.
    pub new: bool,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("fingerprint", &self.fingerprint())
            .field("path", &self.path)
            .field("new", &self.new)
            .finish_non_exhaustive()
    }
}

impl Identity {
    /// SHA-256 of the certificate's DER, as browsers and `openssl` show
    /// it: upper-case hex pairs joined by `:`.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.cert)
    }

    /// A rustls server configuration serving this certificate (TLS 1.2
    /// and 1.3, `ring`, HTTP/1.1 by ALPN).
    ///
    /// # Errors
    /// rustls refuses the key or the certificate.
    pub fn server_config(&self) -> Result<Arc<ServerConfig>, String> {
        let mut cfg = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(self.cert.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone())),
            )
            .map_err(|e| e.to_string())?;
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(cfg))
    }
}

/// [`Identity::fingerprint`] of a DER certificate.
#[must_use]
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Reads the box's certificate from `dir`, or makes one; `host` goes in
/// its names when it is a usable DNS name. Read again, the same file
/// gives the same certificate, so the fingerprint stays.
///
/// # Errors
/// The directory or file cannot be read or written, the system has no
/// randomness, or the file is there but does not hold a certificate and
/// its key (it is left alone).
pub fn load_or_create(dir: &Path, host: Option<&str>) -> io::Result<Identity> {
    fs::create_dir_all(dir)?;
    let path = dir.join(FILE);
    let mut made = false;
    for _ in 0..4 {
        match fs::read_to_string(&path) {
            Ok(text) => {
                private(&path)?;
                let (cert, key) = parse(&text).map_err(|why| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "{}: {why}; delete it to make a new certificate (browsers will then have to trust the new fingerprint)",
                            path.display()
                        ),
                    )
                })?;
                return Ok(Identity {
                    cert,
                    key,
                    path,
                    new: made,
                });
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let (cert, key) = make(host, SystemTime::now())?;
                let tmp = stage(&path, &pem(&cert, &key), &cert)?;
                // Fails if another start got there first; then read theirs.
                let linked = fs::hard_link(&tmp, &path);
                fs::remove_file(&tmp)?;
                match linked {
                    Ok(()) => made = true,
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "{}: could not settle on a certificate",
        path.display()
    )))
}

#[cfg(unix)]
fn private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn private(_: &Path) -> io::Result<()> {
    Ok(())
}

/// Writes `text` to a private file beside `path`, whole and synced.
fn stage(path: &Path, text: &str, cert: &[u8]) -> io::Result<PathBuf> {
    let tag: String = fingerprint(cert)
        .chars()
        .filter(char::is_ascii_hexdigit)
        .take(16)
        .collect();
    let tmp = path.with_file_name(format!(".{FILE}.{tag}"));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    Ok(tmp)
}

// ------------------------------------------------------------------- PEM

fn pem(cert: &[u8], key: &[u8]) -> String {
    let block = |label: &str, der: &[u8]| {
        let b64 = base64(der);
        let lines: Vec<&str> = b64
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).unwrap_or_default())
            .collect();
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            lines.join("\n")
        )
    };
    block(CERT_PEM, cert) + &block(KEY_PEM, key)
}

/// The one block labelled `label` in `text`, decoded.
fn block(text: &str, label: &str) -> Result<Vec<u8>, String> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut found = text.match_indices(&begin);
    let Some((at, _)) = found.next() else {
        return Err(format!("no {label}"));
    };
    if found.next().is_some() {
        return Err(format!("more than one {label}"));
    }
    let rest = &text[at + begin.len()..];
    let stop = rest.find(&end).ok_or_else(|| format!("{label} has no end"))?;
    let b64: String = rest[..stop].split_whitespace().collect();
    unbase64(&b64).ok_or_else(|| format!("{label} is not base64"))
}

/// The certificate and key `text` holds, if the key is a P-256 key whose
/// public half is the certificate's.
fn parse(text: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
    let cert = block(text, CERT_PEM)?;
    let key = block(text, KEY_PEM)?;
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &key, &SystemRandom::new())
        .map_err(|_| "the key is not a P-256 PKCS#8 key".to_string())?;
    if cert.first() != Some(&0x30) || !contains(&cert, &bit_string(pair.public_key().as_ref())) {
        return Err("the certificate is not the key's".into());
    }
    Ok((cert, key))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

// ------------------------------------------------------------------- DER

/// A DER element: `tag`, its definite length, `body`.
fn der(tag: u8, body: &[u8]) -> Vec<u8> {
    let n = body.len();
    let mut out = Vec::with_capacity(n + 4);
    out.push(tag);
    match n {
        0..=0x7F => out.push(u8::try_from(n).unwrap_or(0)),
        0x80..=0xFF => out.extend([0x81, u8::try_from(n).unwrap_or(0)]),
        _ => {
            let n = u16::try_from(n).unwrap_or(u16::MAX);
            out.push(0x82);
            out.extend(n.to_be_bytes());
        }
    }
    out.extend_from_slice(body);
    out
}

fn seq(parts: &[&[u8]]) -> Vec<u8> {
    der(0x30, &parts.concat())
}

fn bit_string(bits: &[u8]) -> Vec<u8> {
    der(0x03, &[&[0u8][..], bits].concat())
}

const OID_ECDSA_SHA256: &[u8] = &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02];
const OID_EC_PUBLIC_KEY: &[u8] = &[0x06, 0x07, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];
const OID_P256: &[u8] = &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
const OID_CN: &[u8] = &[0x06, 0x03, 0x55, 0x04, 0x03];
const OID_SAN: &[u8] = &[0x06, 0x03, 0x55, 0x1D, 0x11];
const OID_BASIC: &[u8] = &[0x06, 0x03, 0x55, 0x1D, 0x13];
const OID_KEY_USAGE: &[u8] = &[0x06, 0x03, 0x55, 0x1D, 0x0F];
const OID_EKU: &[u8] = &[0x06, 0x03, 0x55, 0x1D, 0x25];
const OID_SERVER_AUTH: &[u8] = &[0x06, 0x08, 0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];

/// The subject and issuer's common name.
pub const NAME: &str = "helmsman-plc";

fn extension(oid: &[u8], critical: bool, value: &[u8]) -> Vec<u8> {
    let crit: &[u8] = if critical { &[0x01, 0x01, 0xFF] } else { &[] };
    seq(&[oid, crit, &der(0x04, value)])
}

/// Whether `h` is a DNS name a certificate may carry: labels of letters,
/// digits and inner hyphens, at most 253 characters.
fn dns_name(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
}

/// `t` as X.509 time: `UTCTime` through 2049, `GeneralizedTime` after
/// (RFC 5280 4.1.2.5).
fn time(t: SystemTime) -> Vec<u8> {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (y, m, d) = civil(days);
    let hms = format!("{:02}{:02}{:02}Z", rem / 3600, rem / 60 % 60, rem % 60);
    if (1950..2050).contains(&y) {
        der(0x17, format!("{:02}{m:02}{d:02}{hms}", y % 100).as_bytes())
    } else {
        der(0x18, format!("{y:04}{m:02}{d:02}{hms}").as_bytes())
    }
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian
/// (H. Hinnant, "chrono-Compatible Low-Level Date Algorithms").
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// A new key and a certificate for it, made at `now`.
fn make(host: Option<&str>, now: SystemTime) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let rng = SystemRandom::new();
    let no_rng = || io::Error::other("no system randomness for the TLS key");
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).map_err(|_| no_rng())?;
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
        .map_err(|_| io::Error::other("ring refused the key it made"))?;
    let mut serial = [0u8; 16];
    rng.fill(&mut serial).map_err(|_| no_rng())?;
    // Positive, and minimal as DER wants (no leading zero byte).
    serial[0] = (serial[0] & 0x7F) | 0x40;

    let name = seq(&[&der(0x31, &seq(&[OID_CN, &der(0x0C, NAME.as_bytes())]))]);
    let alg = seq(&[OID_ECDSA_SHA256]);
    let from = now
        .checked_sub(std::time::Duration::from_secs(3600))
        .unwrap_or(now);
    let validity = seq(&[&time(from), &der(0x18, b"99991231235959Z")]);
    let spki = seq(&[
        &seq(&[OID_EC_PUBLIC_KEY, OID_P256]),
        &bit_string(pair.public_key().as_ref()),
    ]);
    let mut alt: Vec<u8> = der(0x82, b"localhost");
    for h in host.into_iter().filter(|h| dns_name(h) && *h != "localhost") {
        alt.extend(der(0x82, h.as_bytes()));
        if !h.contains('.') {
            alt.extend(der(0x82, format!("{h}.local").as_bytes()));
        }
    }
    alt.extend(der(0x87, &[127, 0, 0, 1]));
    alt.extend(der(0x87, &std::net::Ipv6Addr::LOCALHOST.octets()));
    let extensions = seq(&[
        &extension(OID_BASIC, true, &seq(&[])),
        &extension(OID_KEY_USAGE, true, &der(0x03, &[0x07, 0x80])),
        &extension(OID_EKU, false, &seq(&[OID_SERVER_AUTH])),
        &extension(OID_SAN, false, &der(0x30, &alt)),
    ]);
    let tbs = seq(&[
        &[0xA0, 0x03, 0x02, 0x01, 0x02],
        &der(0x02, &serial),
        &alg,
        &name,
        &validity,
        &name,
        &spki,
        &der(0xA3, &extensions),
    ]);
    let sig = pair
        .sign(&rng, &tbs)
        .map_err(|_| io::Error::other("signing the certificate failed"))?;
    let cert = seq(&[&tbs, &alg, &bit_string(sig.as_ref())]);
    Ok((cert, pkcs8.as_ref().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_civil() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        assert_eq!(civil(20_757), (2026, 10, 31));
        assert_eq!(civil(-1), (1969, 12, 31));
        let t = |s: u64| time(UNIX_EPOCH + std::time::Duration::from_secs(s));
        assert_eq!(t(1_793_404_800 + 3_723), der(0x17, b"261031010203Z"));
        assert_eq!(t(2_524_608_000), der(0x18, b"20500101000000Z"));
    }

    #[test]
    fn lengths_take_the_long_form_past_127() {
        assert_eq!(der(0x04, &[0; 3])[..2], [0x04, 3]);
        assert_eq!(der(0x04, &[0; 200])[..3], [0x04, 0x81, 200]);
        assert_eq!(der(0x04, &[0; 300])[..4], [0x04, 0x82, 0x01, 0x2C]);
    }

    #[test]
    fn only_dns_names_go_in_the_certificate() {
        for ok in ["box-1", "line3.plant.example", "a"] {
            assert!(dns_name(ok), "{ok}");
        }
        for bad in ["", "-x", "x-", "a..b", "bad name", "x\ny", "é", &"a".repeat(64)] {
            assert!(!dns_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_made_certificate_round_trips_through_pem() {
        let (cert, key) = make(Some("box-1"), SystemTime::now()).unwrap();
        let text = pem(&cert, &key);
        assert_eq!(parse(&text).unwrap(), (cert.clone(), key.clone()));
        // Another key's certificate is refused.
        let (other, _) = make(None, SystemTime::now()).unwrap();
        assert!(parse(&pem(&other, &key)).is_err());
        assert!(parse(&text.replace(CERT_PEM, "X")).is_err());
        assert!(parse(&format!("{text}{text}")).is_err(), "two of each");
        assert!(parse("").is_err());
        assert!(contains(&cert, b"box-1.local"));
        assert!(contains(&cert, b"localhost"));
    }
}
