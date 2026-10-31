//! Just enough ZIP for a project folder: written `STORED` (no
//! compression) with fixed timestamps, so the same files always make the
//! same bytes; read `STORED` or `DEFLATE` (what every desktop zipper
//! writes), inflated by `miniz_oxide` under a size limit. Hand-written on
//! the APPNOTE 6.3.10 layout (docs/SOURCES.md): local file headers
//! (§4.3.7), the central directory (§4.3.12) and its end record
//! (§4.3.16). Nothing else: no ZIP64, encryption, multi-disk archives or
//! extra-field interpretation; an archive that needs them is refused.
//!
//! An upload comes from a laptop on the LAN, so it is treated as hostile:
//!
//! - **Sizes are capped before anything is inflated.** At most
//!   [`MAX_ENTRIES`] central records and [`MAX_TOTAL`] bytes of declared
//!   uncompressed size in all; each entry inflates into at most its
//!   declared size (a bomb that lies about its size is cut off there and
//!   refused), and its CRC-32 and length must then match.
//! - **Names are checked by the caller** ([`super::file_name`]): no
//!   directories survive but one common top-level folder (stripped), so
//!   `../`, absolute paths, drive letters and backslashes never name a
//!   file. Entries a Mac adds (`__MACOSX/`, `.DS_Store`) are skipped.
//! - **No links.** An entry whose Unix mode (a Unix-made archive's
//!   external attributes) is a symlink, or anything but a regular file or
//!   a directory, is refused.
//! - Names must be UTF-8 (the language-encoding flag, or plain ASCII);
//!   the local header must agree with the central record about the name
//!   and the method, and the data must lie inside the archive.

use std::fmt::Write as _;

/// Most central directory records an upload may hold.
pub const MAX_ENTRIES: usize = 128;
/// Most uncompressed bytes, declared and actual, over every entry.
pub const MAX_TOTAL: usize = super::MAX_TOTAL;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
/// 1980-01-01 00:00:00, the earliest DOS time: every entry we write.
const DOS_DATE: u16 = (1 << 5) | 1;
const DOS_TIME: u16 = 0;
/// General purpose flag bit 11: the name is UTF-8.
const UTF8: u16 = 1 << 11;

/// CRC-32 (ISO-HDLC, the one ZIP uses), table-driven.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (n, slot) in (0u32..).zip(t.iter_mut()) {
            let mut c = n;
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    });
    let mut c = !0u32;
    for &b in data {
        c = t[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

fn put16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// An archive of `files` (name, bytes), in the order given, stored.
///
/// # Errors
/// More than [`MAX_ENTRIES`] files, or a file or the archive past what a
/// ZIP without ZIP64 can say (4 GiB; never near on a project).
pub fn write(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    if files.len() > MAX_ENTRIES {
        return Err(format!("more than {MAX_ENTRIES} files"));
    }
    let n = |v: usize| u32::try_from(v).map_err(|_| "too large for a ZIP without ZIP64".to_string());
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = n(out.len())?;
        let crc = crc32(data);
        let size = n(data.len())?;
        let name_len = u16::try_from(name.len()).map_err(|_| "a name too long".to_string())?;
        put32(&mut out, LOCAL);
        put16(&mut out, 20); // version needed: 2.0
        put16(&mut out, UTF8);
        put16(&mut out, 0); // stored
        put16(&mut out, DOS_TIME);
        put16(&mut out, DOS_DATE);
        put32(&mut out, crc);
        put32(&mut out, size);
        put32(&mut out, size);
        put16(&mut out, name_len);
        put16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        put32(&mut central, CENTRAL);
        put16(&mut central, (3 << 8) | 0x14); // made by: Unix, 2.0
        put16(&mut central, 20);
        put16(&mut central, UTF8);
        put16(&mut central, 0);
        put16(&mut central, DOS_TIME);
        put16(&mut central, DOS_DATE);
        put32(&mut central, crc);
        put32(&mut central, size);
        put32(&mut central, size);
        put16(&mut central, name_len);
        put16(&mut central, 0); // extra
        put16(&mut central, 0); // comment
        put16(&mut central, 0); // disk
        put16(&mut central, 0); // internal attributes
        put32(&mut central, 0o100_644 << 16); // a regular file, rw-r--r--
        put32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = n(out.len())?;
    let cd_size = n(central.len())?;
    out.extend_from_slice(&central);
    let count = u16::try_from(files.len()).map_err(|_| "too many files".to_string())?;
    put32(&mut out, END);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, count);
    put16(&mut out, count);
    put32(&mut out, cd_size);
    put32(&mut out, cd_offset);
    put16(&mut out, 0);
    Ok(out)
}

/// One file read out of an archive: its name as stored (a folder prefix
/// included) and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// What one central directory record says.
struct Record {
    name: String,
    method: u16,
    flags: u16,
    crc: u32,
    compressed: usize,
    size: usize,
    offset: usize,
    /// The Unix mode, when a Unix system made the archive.
    mode: Option<u32>,
}

const fn usize_of(v: u32) -> usize {
    v as usize
}

/// The end of central directory record: where the directory starts, how
/// long it is, and how many records it holds.
fn end_record(zip: &[u8]) -> Result<(usize, usize, usize), String> {
    // The record is 22 bytes plus a comment of up to 65535.
    let lowest = zip.len().saturating_sub(22 + 0xFFFF);
    let at = (lowest..=zip.len().saturating_sub(22))
        .rev()
        .find(|&i| {
            u32_at(zip, i) == Some(END) && 22 + usize::from(u16_at(zip, i + 20).unwrap_or(0)) == zip.len() - i
        })
        .ok_or("not a ZIP archive (no end of central directory record)")?;
    let disk = u16_at(zip, at + 4).unwrap_or(1);
    let cd_disk = u16_at(zip, at + 6).unwrap_or(1);
    let here = u16_at(zip, at + 8).unwrap_or(0);
    let total = u16_at(zip, at + 10).unwrap_or(0);
    let size = u32_at(zip, at + 12).unwrap_or(0);
    let offset = u32_at(zip, at + 16).unwrap_or(0);
    if disk != 0 || cd_disk != 0 || here != total {
        return Err("a multi-disk archive".into());
    }
    if total == 0xFFFF || size == u32::MAX || offset == u32::MAX {
        return Err("a ZIP64 archive (a project never needs one)".into());
    }
    let (offset, size) = (usize_of(offset), usize_of(size));
    if offset.checked_add(size).is_none_or(|e| e > at) {
        return Err("the central directory lies outside the archive".into());
    }
    Ok((offset, size, usize::from(total)))
}

fn records(zip: &[u8]) -> Result<Vec<Record>, String> {
    let (offset, size, count) = end_record(zip)?;
    if count > MAX_ENTRIES {
        return Err(format!("{count} entries; an upload holds at most {MAX_ENTRIES}"));
    }
    let cd = &zip[offset..offset + size];
    let mut at = 0;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let short = || "a truncated central directory".to_string();
        if u32_at(cd, at) != Some(CENTRAL) {
            return Err(short());
        }
        let made_by = u16_at(cd, at + 4).ok_or_else(short)?;
        let flags = u16_at(cd, at + 8).ok_or_else(short)?;
        let method = u16_at(cd, at + 10).ok_or_else(short)?;
        let crc = u32_at(cd, at + 16).ok_or_else(short)?;
        let compressed = u32_at(cd, at + 20).ok_or_else(short)?;
        let size = u32_at(cd, at + 24).ok_or_else(short)?;
        let name_len = usize::from(u16_at(cd, at + 28).ok_or_else(short)?);
        let extra = usize::from(u16_at(cd, at + 30).ok_or_else(short)?);
        let comment = usize::from(u16_at(cd, at + 32).ok_or_else(short)?);
        let external = u32_at(cd, at + 38).ok_or_else(short)?;
        let local = u32_at(cd, at + 42).ok_or_else(short)?;
        let raw = cd.get(at + 46..at + 46 + name_len).ok_or_else(short)?;
        if compressed == u32::MAX || size == u32::MAX || local == u32::MAX {
            return Err("a ZIP64 entry (a project never needs one)".into());
        }
        let name = match std::str::from_utf8(raw) {
            Ok(s) if flags & UTF8 != 0 || raw.is_ascii() => s.to_string(),
            _ => return Err("an entry whose name is not UTF-8".into()),
        };
        out.push(Record {
            name,
            method,
            flags,
            crc,
            compressed: usize_of(compressed),
            size: usize_of(size),
            offset: usize_of(local),
            mode: (made_by >> 8 == 3).then_some(external >> 16),
        });
        at += 46 + name_len + extra + comment;
    }
    Ok(out)
}

/// A shown name: control characters escaped, cut short.
fn shown(name: &str) -> String {
    let mut s: String = name.chars().take(80).flat_map(char::escape_debug).collect();
    if name.chars().count() > 80 {
        let _ = write!(s, "...");
    }
    s
}

/// The regular files of `zip`, in central directory order; directory
/// entries are left out (the caller sees their names only as prefixes).
///
/// # Errors
/// Anything the module docs refuse, said once for the whole archive.
pub fn read(zip: &[u8]) -> Result<Vec<Entry>, String> {
    let recs = records(zip)?;
    let declared = recs.iter().try_fold(0usize, |sum, r| sum.checked_add(r.size));
    if declared.is_none_or(|t| t > MAX_TOTAL) {
        return Err(format!(
            "the archive unpacks to more than {MAX_TOTAL} bytes; a project is at most that"
        ));
    }
    let mut out = Vec::new();
    let mut total = 0usize;
    for r in recs {
        let name = shown(&r.name);
        let kind = r.mode.map(|m| m & 0o170_000);
        let dir = r.name.ends_with('/');
        match kind {
            Some(0o120_000) => return Err(format!("{name}: a symbolic link; only files are taken")),
            Some(0o100_000) | None => {}
            Some(0o040_000) if dir => {}
            Some(0) if !dir => {} // no file type recorded: a plain file
            Some(_) => return Err(format!("{name}: not a regular file")),
        }
        if r.flags & 1 != 0 {
            return Err(format!("{name}: encrypted"));
        }
        if dir {
            if r.size != 0 {
                return Err(format!("{name}: a directory with content"));
            }
            continue;
        }
        let data = entry_data(zip, &r).map_err(|e| format!("{name}: {e}"))?;
        total += data.len();
        if total > MAX_TOTAL {
            return Err(format!("the archive unpacks to more than {MAX_TOTAL} bytes"));
        }
        out.push(Entry { name: r.name, data });
    }
    Ok(out)
}

fn entry_data(zip: &[u8], r: &Record) -> Result<Vec<u8>, String> {
    let at = r.offset;
    if u32_at(zip, at) != Some(LOCAL) {
        return Err("no local header where the directory says".into());
    }
    let short = || "a truncated local header".to_string();
    let method = u16_at(zip, at + 8).ok_or_else(short)?;
    let name_len = usize::from(u16_at(zip, at + 26).ok_or_else(short)?);
    let extra = usize::from(u16_at(zip, at + 28).ok_or_else(short)?);
    let name = zip.get(at + 30..at + 30 + name_len).ok_or_else(short)?;
    if method != r.method || name != r.name.as_bytes() {
        return Err("the local header disagrees with the central directory".into());
    }
    let start = at + 30 + name_len + extra;
    let raw = start
        .checked_add(r.compressed)
        .and_then(|end| zip.get(start..end))
        .ok_or("the data runs past the end of the archive")?;
    let data = match r.method {
        0 if r.compressed == r.size => raw.to_vec(),
        0 => return Err("stored, but its two sizes differ".into()),
        8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, r.size)
            .map_err(|_| "does not inflate to its declared size (corrupt, or a bomb)".to_string())?,
        m => {
            return Err(format!(
                "compression method {m}; only stored and deflate are taken"
            ));
        }
    };
    if data.len() != r.size {
        return Err("inflates to another size than declared".into());
    }
    if crc32(&data) != r.crc {
        return Err("CRC-32 mismatch".into());
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn written_archives_read_back() {
        let files = vec![
            ("a.st".to_string(), b"PROGRAM a END_PROGRAM".to_vec()),
            ("io.map".to_string(), Vec::new()),
        ];
        let z = write(&files).unwrap();
        let back = read(&z).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].name, "a.st");
        assert_eq!(back[0].data, files[0].1);
        assert_eq!(write(&files).unwrap(), z, "deterministic");
    }
}
