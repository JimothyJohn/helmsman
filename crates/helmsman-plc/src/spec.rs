//! The `--module` argument: `PATH[,name=N][,scan=MS][,i=START+LEN][,q=START+LEN][,m=BYTES]`.
//! Anything left out comes from [`Defaults`]. Window bounds and overlaps
//! are the runtime's to check ([`crate::Runtime::new`]); this only parses.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Window;

/// What a module gets for any key its spec leaves out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Defaults {
    pub period: Duration,
    pub i: Window,
    pub q: Window,
    pub m: usize,
}

/// One module to load, before compiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub path: PathBuf,
    pub name: String,
    pub period: Duration,
    pub i: Window,
    pub q: Window,
    pub m: usize,
}

impl Spec {
    /// A module from a bare path: every setting from `d`, named after the
    /// file (see [`safe_name`]).
    #[must_use]
    pub fn from_path(path: &Path, d: &Defaults) -> Self {
        Self {
            name: path
                .file_stem()
                .map_or_else(|| "module".into(), |s| safe_name(&s.to_string_lossy())),
            path: path.to_path_buf(),
            period: d.period,
            i: d.i,
            q: d.q,
            m: d.m,
        }
    }

    /// Parses a `--module` value.
    ///
    /// # Errors
    /// An empty path, an unknown or repeated key, or a value out of range.
    pub fn parse(s: &str, d: &Defaults) -> Result<Self, String> {
        let mut parts = s.split(',');
        let path = parts.next().unwrap_or_default();
        if path.is_empty() {
            return Err(format!("--module {s:?}: needs a path first"));
        }
        let mut spec = Self::from_path(Path::new(path), d);
        let mut seen: Vec<&str> = Vec::new();
        for part in parts {
            let (key, v) = part
                .split_once('=')
                .ok_or_else(|| format!("--module: expected key=value, got {part:?}"))?;
            if seen.contains(&key) {
                return Err(format!("--module: {key} given twice"));
            }
            seen.push(key);
            match key {
                "name" => {
                    if !name_ok(v) {
                        return Err(format!("--module name: 1..=64 of A-Z a-z 0-9 _ -, got {v:?}"));
                    }
                    spec.name = v.into();
                }
                "scan" => match v.parse::<u64>() {
                    Ok(ms @ 1..=10_000) => spec.period = Duration::from_millis(ms),
                    _ => return Err(format!("--module scan: expected 1..=10000 ms, got {v:?}")),
                },
                "i" | "q" => {
                    let w = window(v)
                        .ok_or_else(|| format!("--module {key}: expected START+LEN in bytes, got {v:?}"))?;
                    if key == "i" {
                        spec.i = w;
                    } else {
                        spec.q = w;
                    }
                }
                "m" => match v.parse::<usize>() {
                    Ok(n @ 0..=65_536) => spec.m = n,
                    _ => return Err(format!("--module m: expected 0..=65536 bytes, got {v:?}")),
                },
                other => return Err(format!("--module: unknown key {other:?}")),
            }
        }
        Ok(spec)
    }
}

/// A module name: 1..=64 of `A-Z a-z 0-9 _ -`, safe in a log line.
pub(crate) fn name_ok(v: &str) -> bool {
    !v.is_empty() && v.len() <= 64 && v.bytes().all(name_byte)
}

const fn name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// A file stem as a module name: other bytes become `_`, so a name never
/// carries a newline or an escape into a log line.
fn safe_name(stem: &str) -> String {
    let name: String = stem
        .chars()
        .take(64)
        .map(|c| {
            if c.is_ascii() && name_byte(c as u8) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() { "module".into() } else { name }
}

pub(crate) fn window(v: &str) -> Option<Window> {
    let (start, len) = v.split_once('+')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(start) || !digits(len) {
        return None;
    }
    Some(Window::new(start.parse().ok()?, len.parse().ok()?))
}
