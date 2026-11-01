//! What a Helmsman project is, with no I/O: its names, what its files may
//! hold, and its `.zip` (docs/UI.md "Projects"; ARCHITECTURE "Projects on
//! the box"). The box keeps projects in folders (`helmsman_plc::project`);
//! the browser's simulation keeps them in the page's storage
//! (helmsman-canvas). Both hold them to these rules and read and write the
//! same archives, so a project downloaded from one uploads to the other.
//!
//! - **Names.** A project is 1..=48 of `a-z 0-9 _ -`, starting with a
//!   letter or digit ([`project_name`]): one path segment, lower case so
//!   two projects never differ by case alone (a case-folding file system
//!   would merge them). A file is `NAME.st` with `NAME` 1..=64 of `A-Z a-z
//!   0-9 _ -` starting with a letter or digit, or one of [`IO_MAP`] and
//!   [`SETTINGS`] ([`file_name`]); two files that differ only by case are
//!   refused.
//! - **Contents.** UTF-8 without NUL, other control characters (tab, CR
//!   and LF aside) or bidirectional overrides (no "Trojan Source" in a
//!   program an engineer reviews); at most [`MAX_FILE`] bytes a file,
//!   [`MAX_FILES`] `.st` files and [`MAX_TOTAL`] bytes a project.

pub mod zip;

use std::collections::BTreeMap;

/// The I/O map's file name in a project.
pub const IO_MAP: &str = "io.map";
/// The settings file's name in a project.
pub const SETTINGS: &str = "project.settings";
/// Largest file, bytes.
pub const MAX_FILE: usize = 256 * 1024;
/// Most `.st` files in a project.
pub const MAX_FILES: usize = 64;
/// Largest project, all files together, bytes.
pub const MAX_TOTAL: usize = 900 * 1024;
/// Longest project name.
pub const NAME_LEN: usize = 48;
/// Longest `.st` file stem.
pub const STEM_LEN: usize = 64;

/// Whether `s` is a project name.
#[must_use]
pub fn project_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=NAME_LEN).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

/// What a file in a project is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    St,
    IoMap,
    Settings,
}

/// What `s` names in a project, if it names anything.
#[must_use]
pub fn file_name(s: &str) -> Option<FileKind> {
    match s {
        IO_MAP => return Some(FileKind::IoMap),
        SETTINGS => return Some(FileKind::Settings),
        _ => {}
    }
    let stem = s.strip_suffix(".st")?;
    let b = stem.as_bytes();
    ((1..=STEM_LEN).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-'))
    .then_some(FileKind::St)
}

/// Why a file's bytes are refused, if they are.
///
/// # Errors
/// Not UTF-8, too large, or a character the module docs refuse (named by
/// line).
pub fn check_text(bytes: &[u8]) -> Result<&str, String> {
    if bytes.len() > MAX_FILE {
        return Err(format!("larger than {MAX_FILE} bytes"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8".to_string())?;
    for (n, line) in text.split('\n').enumerate() {
        for c in line.chars() {
            let bidi =
                matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}');
            if (c.is_control() && c != '\t' && c != '\r') || bidi {
                return Err(format!(
                    "line {}: character U+{:04X} is not allowed (control or bidirectional formatting)",
                    n + 1,
                    u32::from(c)
                ));
            }
        }
    }
    Ok(text)
}

/// A name for a message: control characters escaped, cut at 200
/// characters.
#[must_use]
pub fn clean(s: &str) -> String {
    const MAX: usize = 200;
    let mut out: String = s.chars().take(MAX).flat_map(char::escape_debug).collect();
    if s.chars().count() > MAX {
        out.push_str("...");
    }
    out
}

/// The folder of a project's WASM drivers, in its folder on a box and in
/// its archive: `drivers/NAME.wasm`.
pub const DRIVERS: &str = "drivers";
/// A driver file's extension.
pub const DRIVER_EXT: &str = ".wasm";
/// Largest driver file, bytes.
pub const MAX_DRIVER: usize = 256 * 1024;
/// Most drivers one project carries.
pub const MAX_DRIVERS: usize = 16;

/// A project's (or a box's) WASM driver binaries, by name.
pub type Drivers = BTreeMap<String, Vec<u8>>;

/// Whether `s` is a WASM driver's name (`wasm:NAME` in a map, the file
/// `drivers/NAME.wasm`): a slug, the rule a project name follows (1..=48
/// of `a-z 0-9 _ -`, starting with a letter or digit), so it is one path
/// segment and two drivers never differ by case alone.
#[must_use]
pub fn driver_name(s: &str) -> bool {
    project_name(s)
}

/// The NAME of an archive path `drivers/NAME.wasm`.
#[must_use]
pub fn driver_path(s: &str) -> Option<&str> {
    s.strip_prefix(DRIVERS)?
        .strip_prefix('/')?
        .strip_suffix(DRIVER_EXT)
        .filter(|n| driver_name(n))
}

/// A project's files in memory, by name (sorted).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files(pub BTreeMap<String, String>);

impl Files {
    /// The `.st` files, by name.
    pub fn st(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .filter(|(n, _)| file_name(n) == Some(FileKind::St))
            .map(|(n, t)| (n.as_str(), t.as_str()))
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Bytes, all files together.
    #[must_use]
    pub fn total(&self) -> usize {
        self.0.values().map(String::len).sum()
    }

    /// Holds a whole set of files to the rules: names, contents, counts,
    /// sizes, and no two names that differ only by case.
    ///
    /// # Errors
    /// Every problem, one per file (or one for the set).
    pub fn check(&self) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        let mut folded: BTreeMap<String, &str> = BTreeMap::new();
        for (name, text) in &self.0 {
            if file_name(name).is_none() {
                errs.push(format!("{}: not a project file name", clean(name)));
                continue;
            }
            if let Err(e) = check_text(text.as_bytes()) {
                errs.push(format!("{name}: {e}"));
            }
            if let Some(other) = folded.insert(name.to_ascii_lowercase(), name) {
                errs.push(format!("{name}: differs from {other} only by case"));
            }
        }
        let st = self.st().count();
        if st > MAX_FILES {
            errs.push(format!("{st} .st files; a project holds at most {MAX_FILES}"));
        }
        if self.total() > MAX_TOTAL {
            errs.push(format!(
                "{} bytes; a project is at most {MAX_TOTAL}",
                self.total()
            ));
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    /// The project as a `.zip`: its files at the archive's root, sorted,
    /// stored, with fixed timestamps (the same files, the same bytes).
    ///
    /// # Errors
    /// More files than an archive holds here ([`zip::MAX_ENTRIES`]).
    pub fn to_zip(&self) -> Result<Vec<u8>, String> {
        let list: Vec<(String, Vec<u8>)> = self
            .0
            .iter()
            .map(|(n, t)| (n.clone(), t.clone().into_bytes()))
            .collect();
        zip::write(&list)
    }
}

/// Files from an archive's entries: one common top-level folder stripped,
/// Mac metadata skipped, every name a project file name, no duplicates.
/// An archive that carries WASM drivers (`drivers/NAME.wasm`) is refused
/// here, since only a box runs them ([`from_zip_with_drivers`]).
///
/// # Errors
/// A name that is not a project file (traversal, an absolute path, a
/// nested folder, anything else), a duplicate, bad contents, a driver.
pub fn from_zip(entries: Vec<zip::Entry>) -> Result<Files, String> {
    let (files, drivers) = from_zip_with_drivers(entries)?;
    if let Some(name) = drivers.keys().next() {
        return Err(format!(
            "{DRIVERS}/{name}{DRIVER_EXT}: a WASM driver; only a box runs those"
        ));
    }
    Ok(files)
}

/// [`from_zip`], keeping the WASM drivers the archive carries: each
/// `drivers/NAME.wasm` (NAME a [`driver_name`]) at most [`MAX_DRIVER`]
/// bytes, at most [`MAX_DRIVERS`] of them, and files and drivers together
/// at most [`MAX_TOTAL`] bytes. What a driver's bytes are is the box's to
/// check (it loads them).
///
/// # Errors
/// As [`from_zip`], and a driver past the size or count rules.
pub fn from_zip_with_drivers(entries: Vec<zip::Entry>) -> Result<(Files, Drivers), String> {
    let keep: Vec<zip::Entry> = entries
        .into_iter()
        .filter(|e| {
            !(e.name.starts_with("__MACOSX/") || e.name == ".DS_Store" || e.name.ends_with("/.DS_Store"))
        })
        .collect();
    let prefix = keep
        .first()
        .and_then(|e| e.name.split_once('/'))
        .map(|(p, _)| p)
        // A folder a zipper made: a plain name, never `..`, `.`, a drive,
        // nor the drivers folder of an archive that holds nothing else.
        .filter(|p| {
            let b = p.as_bytes();
            *p != DRIVERS
                && (1..=128).contains(&b.len())
                && b[0].is_ascii_alphanumeric()
                && b.iter()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b' '))
        })
        .map(|p| format!("{p}/"))
        .filter(|p| keep.iter().all(|e| e.name.starts_with(p.as_str())));
    let mut files = BTreeMap::new();
    let mut drivers = Drivers::new();
    for e in keep {
        let name = prefix
            .as_deref()
            .and_then(|p| e.name.strip_prefix(p))
            .unwrap_or(&e.name)
            .to_string();
        if let Some(driver) = driver_path(&name) {
            if e.data.len() > MAX_DRIVER {
                return Err(format!("{name}: larger than {MAX_DRIVER} bytes"));
            }
            if drivers.insert(driver.to_string(), e.data).is_some() {
                return Err(format!("{name}: twice in the archive"));
            }
            if drivers.len() > MAX_DRIVERS {
                return Err(format!(
                    "more than {MAX_DRIVERS} drivers; a project carries at most that"
                ));
            }
            continue;
        }
        if file_name(&name).is_none() {
            return Err(format!(
                "{}: not a project file (NAME.st, {IO_MAP} or {SETTINGS} at the archive's top level, or {DRIVERS}/NAME{DRIVER_EXT})",
                clean(&e.name)
            ));
        }
        let text = check_text(&e.data).map_err(|why| format!("{name}: {why}"))?;
        if files.insert(name.clone(), text.to_string()).is_some() {
            return Err(format!("{name}: twice in the archive"));
        }
    }
    let files = Files(files);
    files.check().map_err(|e| e.join("; "))?;
    let total = files.total() + drivers.values().map(Vec::len).sum::<usize>();
    if total > MAX_TOTAL {
        return Err(format!("{total} bytes; a project is at most {MAX_TOTAL}"));
    }
    Ok((files, drivers))
}

/// A `.zip`'s bytes read as a project's files, held to every rule.
///
/// # Errors
/// What [`zip::read`] or [`from_zip`] refuse.
pub fn read_zip(bytes: &[u8]) -> Result<Files, String> {
    zip::read(bytes).and_then(from_zip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_rules() {
        assert!(project_name("line-3_b") && !project_name("Line") && !project_name("-x"));
        assert!(!project_name(&"a".repeat(49)) && !project_name("") && !project_name("a/b"));
        assert_eq!(file_name("Main_2.st"), Some(FileKind::St));
        assert_eq!(file_name("io.map"), Some(FileKind::IoMap));
        assert_eq!(file_name("project.settings"), Some(FileKind::Settings));
        for bad in ["../a.st", ".st", "a.ST", "a b.st", "a/b.st", "C:a.st", "a.st\0"] {
            assert_eq!(file_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_project_round_trips_through_its_zip() {
        let f = Files(BTreeMap::from([
            ("main.st".to_string(), "PROGRAM main\nEND_PROGRAM\n".to_string()),
            ("io.map".to_string(), String::new()),
        ]));
        let z = f.to_zip().unwrap();
        assert_eq!(read_zip(&z).unwrap(), f);
        assert_eq!(f.to_zip().unwrap(), z, "the same files, the same bytes");
    }

    fn entry(name: &str, data: &[u8]) -> zip::Entry {
        zip::Entry {
            name: name.into(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn driver_names_are_slugs_and_paths_name_nothing_else() {
        assert!(driver_name("modbus") && driver_name("rio-x_2"));
        assert_eq!(driver_path("drivers/modbus.wasm"), Some("modbus"));
        for bad in [
            "drivers/../x.wasm",
            "drivers/Modbus.wasm",
            "drivers/a/b.wasm",
            "drivers/.wasm",
            "drivers/x.WASM",
            "drivers/x.wasm/",
            "drivers\\x.wasm",
            "x.wasm",
            "../drivers/x.wasm",
            "drivers/%2e%2e.wasm",
            "drivers/x\0.wasm",
            "/drivers/x.wasm",
        ] {
            assert_eq!(driver_path(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn an_archive_carries_drivers_to_a_box_and_never_to_the_simulation() {
        let st = entry("line/main.st", b"PROGRAM main\nEND_PROGRAM\n");
        let wasm = entry("line/drivers/modbus.wasm", b"\0asm\x01\0\0\0");
        let (files, drivers) = from_zip_with_drivers(vec![st.clone(), wasm.clone()]).unwrap();
        assert_eq!(files.0.keys().collect::<Vec<_>>(), ["main.st"]);
        assert_eq!(drivers["modbus"], b"\0asm\x01\0\0\0");
        let e = from_zip(vec![st.clone(), wasm]).unwrap_err();
        assert!(
            e.contains("drivers/modbus.wasm") && e.contains("only a box"),
            "{e}"
        );
        // Only drivers, at the top: the folder is not stripped as a prefix.
        let (_, d) = from_zip_with_drivers(vec![entry("drivers/a.wasm", b"x")]).unwrap();
        assert!(d.contains_key("a"));
        for (why, entries) in [
            ("traversal", vec![entry("drivers/../a.wasm", b"x")]),
            ("upper case", vec![entry("drivers/A.wasm", b"x")]),
            ("nested", vec![entry("drivers/a/b.wasm", b"x")]),
            (
                "twice",
                vec![entry("drivers/a.wasm", b"x"), entry("drivers/a.wasm", b"y")],
            ),
            (
                "too large",
                vec![entry("drivers/a.wasm", &vec![0; MAX_DRIVER + 1])],
            ),
            (
                "too many",
                (0..=MAX_DRIVERS)
                    .map(|n| entry(&format!("drivers/d{n}.wasm"), b"x"))
                    .collect(),
            ),
            (
                "files and drivers past the project's total",
                (0..4)
                    .map(|n| entry(&format!("drivers/d{n}.wasm"), &vec![0; MAX_DRIVER]))
                    .collect(),
            ),
        ] {
            assert!(from_zip_with_drivers(entries).is_err(), "{why}");
        }
    }

    #[test]
    fn text_with_controls_or_bidi_is_refused() {
        assert!(check_text(b"a\tb\r\nc").is_ok());
        assert!(check_text(b"a\0b").is_err());
        assert!(check_text("x\u{202E}y".as_bytes()).is_err());
        assert!(check_text(&[0xFF]).is_err());
    }
}
