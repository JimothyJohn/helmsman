//! The canvas, served by the API from a directory (`--ui DIR`, what
//! `make canvas-web` builds: `index.html`, the wasm and its JS loader), so
//! any browser on the LAN is the HMI. Same origin as the API: no CORS.
//!
//! Only `GET` reaches here (anything else needs the token and finds no
//! route). Refused with 404, before the file system is asked anything:
//!
//! - a segment outside `[A-Za-z0-9._-]` (after percent-decoding: `%2e%2e`,
//!   `%2f`, `%5c`, `%00`, any non-ASCII such as fullwidth dots or overlong
//!   UTF-8, which the decoder already refuses), empty (`//`, a trailing
//!   `/`), starting with a dot (`..`, `.env`), longer than
//!   [`MAX_SEGMENT`], or more than [`MAX_DEPTH`] of them;
//! - a file type not in [`content_type`]'s list.
//!
//! What passes is joined under the directory and resolved; it is served
//! only if resolving changed nothing (no symlink below the directory, no
//! case-folding match for a differently spelled name) and it is a regular
//! file of at most [`MAX_FILE`] bytes. Files are streamed in chunks, each write within
//! the API's write timeout and the whole file within [`SEND_DEADLINE`].
//!
//! Caching: a file whose name carries trunk's content hash
//! (`helmsman-canvas-0123456789abcdef_bg.wasm`) never changes, so it is
//! cached for a year; everything else (`index.html`) is revalidated on
//! every load, so a redeploy shows up on the next reload. HTML gets a
//! content security policy allowing only this origin, wasm, and the
//! page's own inline scripts by hash.

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

use super::http::{Conn, Response, SECURITY};

/// Longest path segment, bytes.
pub const MAX_SEGMENT: usize = 128;
/// Most path segments.
pub const MAX_DEPTH: usize = 8;
/// Largest file served, bytes.
pub const MAX_FILE: u64 = 64 << 20;
/// Largest HTML file, bytes (it is read whole to hash its scripts).
const MAX_HTML: u64 = 1 << 20;
/// Longest one file may take to send.
pub const SEND_DEADLINE: Duration = Duration::from_secs(120);

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";

/// The UI directory.
#[derive(Debug, Clone)]
pub struct Ui {
    root: PathBuf,
}

/// A file ready to send.
#[derive(Debug)]
pub struct Asset {
    file: File,
    /// Bytes.
    pub len: u64,
    pub content_type: &'static str,
    pub cache: &'static str,
    /// `Content-Security-Policy`, for HTML.
    pub csp: Option<String>,
    /// Already read (HTML): sent instead of the file.
    body: Option<Vec<u8>>,
}

impl Ui {
    /// `dir`, which must be a directory holding `index.html`.
    ///
    /// # Errors
    /// `dir` does not resolve, is not a directory, or has no `index.html`.
    pub fn open(dir: &Path) -> io::Result<Self> {
        let root = dir.canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a directory"));
        }
        if !root.join("index.html").is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no index.html in it (build it with `make canvas-web`)",
            ));
        }
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file `segments` (the request path's, percent-decoded) names;
    /// `/` is `index.html`.
    ///
    /// # Errors
    /// The response to send instead: 404 for anything refused or missing
    /// (one answer, so a probe learns nothing about what exists), 500 if a
    /// file that exists cannot be read.
    pub fn resolve(&self, segments: &[String]) -> Result<Asset, Response> {
        let index = [String::from("index.html")];
        let segments = if segments == [""] { &index[..] } else { segments };
        if segments.len() > MAX_DEPTH || !segments.iter().all(|s| safe_segment(s)) {
            return Err(not_found());
        }
        let name = segments.last().map_or("", String::as_str);
        let Some(content_type) = content_type(name) else {
            return Err(not_found());
        };
        let mut path = self.root.clone();
        path.extend(segments);
        let Ok(real) = path.canonicalize() else {
            return Err(not_found());
        };
        // Exactly the path asked for: no symlink anywhere below the root,
        // and no case-folding file system matching a differently spelled name.
        if real != path {
            return Err(not_found());
        }
        let Ok(file) = File::open(&real) else {
            return Err(not_found());
        };
        let meta = file.metadata().map_err(|_| not_found())?;
        if !meta.is_file() || meta.len() > MAX_FILE {
            return Err(not_found());
        }
        let html = content_type.starts_with("text/html");
        let mut asset = Asset {
            file,
            len: meta.len(),
            content_type,
            cache: if hashed(name) { IMMUTABLE } else { REVALIDATE },
            csp: None,
            body: None,
        };
        if html {
            if asset.len > MAX_HTML {
                return Err(not_found());
            }
            let mut body = Vec::new();
            if (&asset.file).take(MAX_HTML).read_to_end(&mut body).is_err() {
                return Err(Response::error(500, "cannot read the UI"));
            }
            asset.len = body.len() as u64;
            asset.csp = Some(csp(&String::from_utf8_lossy(&body)));
            asset.body = Some(body);
        }
        Ok(asset)
    }
}

impl Asset {
    /// Lets an HTML page also fetch from other boxes over `http:` or `https:` (the
    /// site view reads each peer's `GET /api/site`, which is the only
    /// route that answers another origin). Scripts stay this origin's and
    /// the page's own by hash; nothing else changes.
    pub fn allow_peers(&mut self) {
        if let Some(c) = &mut self.csp {
            *c = c.replacen(CONNECT_SELF, "connect-src 'self' http: https:;", 1);
        }
    }
}

/// The connect directive of [`csp`].
const CONNECT_SELF: &str = "connect-src 'self';";

fn not_found() -> Response {
    Response::error(404, "no such resource")
}

/// A segment that can only name an entry of the directory it is in.
#[must_use]
pub fn safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_SEGMENT
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The content type served for a file name; `None`: not served.
#[must_use]
pub fn content_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1;
    Some(match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => return None,
    })
}

/// Whether `name` carries trunk's content hash: `-` and 16 hex digits
/// right after the stem (`app-0123456789abcdef.js`,
/// `app-0123456789abcdef_bg.wasm`).
#[must_use]
pub fn hashed(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("");
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    stem.rsplit_once('-')
        .is_some_and(|(_, h)| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The policy for an HTML page: this origin only, wasm allowed, and each
/// inline `<script>` of `html` by its SHA-256.
#[must_use]
pub fn csp(html: &str) -> String {
    let mut scripts = String::from("'self' 'wasm-unsafe-eval'");
    for body in inline_scripts(html) {
        scripts.push_str(" 'sha256-");
        scripts.push_str(&base64(&Sha256::digest(body.as_bytes())));
        scripts.push('\'');
    }
    format!(
        "default-src 'none'; script-src {scripts}; connect-src 'self'; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    )
}

/// The text of each `<script>` without a `src`.
fn inline_scripts(html: &str) -> Vec<&str> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(open) = lower[at..].find("<script").map(|k| k + at) {
        let Some(tag_end) = lower[open..].find('>').map(|k| k + open) else {
            break;
        };
        let Some(close) = lower[tag_end..].find("</script").map(|k| k + tag_end) else {
            break;
        };
        let tag = &lower[open..tag_end];
        if !tag.contains(" src=") && !tag.contains("\tsrc=") && !tag.contains("\nsrc=") {
            out.push(&html[tag_end + 1..close]);
        }
        at = close;
    }
    out
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (k, &b)| n | (u32::from(b) << (16 - 8 * k)));
        for k in 0..4 {
            if k <= c.len() {
                out.push(char::from(A[(n >> (18 - 6 * k)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Sends `asset`: the head, then the file in chunks, giving up when a
/// write stalls past the stream's write timeout or the whole send passes
/// [`SEND_DEADLINE`].
///
/// # Errors
/// The socket's or the file's, or `TimedOut` past the deadline.
pub fn send(stream: &mut Conn, mut asset: Asset) -> io::Result<()> {
    let started = Instant::now();
    let csp = asset
        .csp
        .as_deref()
        .map_or(String::new(), |c| format!("Content-Security-Policy: {c}\r\n"));
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: {}\r\n{csp}{SECURITY}\r\n",
        asset.content_type, asset.len, asset.cache
    );
    stream.write_all(head.as_bytes())?;
    if let Some(body) = asset.body.take() {
        stream.write_all(&body)?;
        return stream.flush();
    }
    let mut left = asset.len;
    let mut chunk = vec![0u8; 64 * 1024];
    while left > 0 {
        if started.elapsed() > SEND_DEADLINE {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let want = usize::try_from(left.min(chunk.len() as u64)).unwrap_or(chunk.len());
        let n = asset.file.read(&mut chunk[..want])?;
        if n == 0 {
            // Shrank while being sent: the length promised cannot be kept.
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        stream.write_all(&chunk[..n])?;
        left -= n as u64;
    }
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (raw, enc) in cases {
            assert_eq!(base64(raw.as_bytes()), enc, "{raw:?}");
        }
    }

    #[test]
    fn inline_scripts_are_hashed_and_external_ones_are_not() {
        let html = "<html><SCRIPT type=\"module\">import init from '/a.js';init();</SCRIPT>\
                    <script src=\"/b.js\"></script><script>x()</script></html>";
        assert_eq!(inline_scripts(html), ["import init from '/a.js';init();", "x()"]);
        // echo -n 'x()' | openssl dgst -sha256 -binary | base64
        assert!(
            csp(html).contains("'sha256-D6IGS8VMvCoyaR/l0h9tERrBTATY01CoPS7l6xDv0kI='"),
            "{}",
            csp(html)
        );
        assert!(
            csp("<p>no scripts</p>").starts_with("default-src 'none'; script-src 'self' 'wasm-unsafe-eval';")
        );
    }

    #[test]
    fn trunk_hashed_names_are_immutable_and_the_rest_revalidate() {
        assert!(hashed("helmsman-canvas-0123456789abcdef_bg.wasm"));
        assert!(hashed("helmsman-canvas-0123456789abcdef.js"));
        assert!(!hashed("index.html"));
        assert!(!hashed("helmsman-canvas.js"));
        assert!(!hashed("app-0123456789abcdeg.js"));
        assert!(!hashed("app-0123456789abcdef0.js"));
    }
}
