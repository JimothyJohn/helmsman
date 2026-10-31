//! Photo drop-ins (card `dropin-photo`): a whiteboard sketch or a photo of
//! the line, read from its headers only. The box never decodes pixels;
//! it walks the file's structure to learn what it is and how big, refuses
//! anything it cannot account for byte for byte, and hands the bytes to
//! the model untouched as an `image` content block.
//!
//! **What is taken.** PNG, JPEG and WebP, the Messages API's image formats
//! less GIF (an animation would show the model only its first frame).
//! The format is the bytes' own: the magic number and a full walk of the
//! container. The file name and the request's `Content-Type` are never
//! trusted for it.
//!
//! **What is refused.**
//! - Anything over [`MAX_PHOTO`] bytes. The image goes to the relay as
//!   base64 inside the conversation; relay protocol v1 first took 1 MiB
//!   per request ([`MAX_PHOTO`] is what left room for the rest of a chat,
//!   kept since the limit became 4 MiB: card `dropin-photo-size`);
//!   the model API's own per-image limit (10 MB base64, 5 MB on Bedrock) is
//!   far above it.
//! - A declared width or height of 0 or over [`MAX_SIDE`] (the model API's
//!   8000 px limit), whatever the file's size: a header can claim a
//!   65535 x 65535 image in a few bytes.
//! - A file that does not end where its own structure says it does
//!   (trailing bytes are where a second file hides in a polyglot), a
//!   truncated one, a PNG chunk with a bad CRC or an unknown critical
//!   chunk, a JPEG segment that runs past the end, a WebP whose RIFF size
//!   disagrees with the file, and animations (APNG, animated WebP).
//! - Other image formats (GIF, HEIC, AVIF, BMP, TIFF) by name, so the
//!   person knows to re-export.

/// Largest photo, bytes (see the module docs for why).
pub const MAX_PHOTO: usize = 512 * 1024;
/// Largest width or height, pixels: the Messages API's per-image limit.
pub const MAX_SIDE: u32 = 8000;

/// An image format the box takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
}

impl Format {
    /// The `media_type` of the image block.
    #[must_use]
    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}

/// What a photo's headers say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub format: Format,
    pub width: u32,
    pub height: u32,
}

/// Why a photo is refused: an HTTP status and a reason for the person.
pub type Refused = (u16, String);

fn bad(why: impl Into<String>) -> Refused {
    (422, why.into())
}

/// Whether `b` starts like an image file of any format the box knows of
/// (taken or not), so it is never read as text.
#[must_use]
pub fn is_image(b: &[u8]) -> bool {
    other_format(b).is_some()
        || b.starts_with(PNG_MAGIC)
        || b.starts_with(&[0xFF, 0xD8, 0xFF])
        || (b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP")
}

/// An image format the box knows by its magic but does not take.
fn other_format(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some("GIF");
    }
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        return match &b[8..12] {
            b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1" => Some("HEIC"),
            b"avif" | b"avis" => Some("AVIF"),
            _ => None,
        };
    }
    // "BM", then reserved zeros and a known DIB header size: never text.
    if b.starts_with(b"BM")
        && b.get(6..10) == Some(&[0u8; 4][..])
        && le32(b, 14).is_some_and(|n| matches!(n, 12 | 40 | 52 | 56 | 64 | 108 | 124))
    {
        return Some("BMP");
    }
    if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        return Some("TIFF");
    }
    None
}

/// Reads a photo's headers: its format and size, or why it is refused.
///
/// # Errors
/// 413 over [`MAX_PHOTO`], 415 for a format the box does not take, 422 for
/// a file whose structure does not hold (see the module docs).
pub fn sniff(b: &[u8]) -> Result<Info, Refused> {
    if b.len() > MAX_PHOTO {
        return Err((
            413,
            format!(
                "a photo is at most {} KiB; send it smaller (about 1500 px on the long side is plenty for a sketch)",
                MAX_PHOTO / 1024
            ),
        ));
    }
    if let Some(name) = other_format(b) {
        return Err((
            415,
            format!("{name} images are not taken; export the photo as PNG, JPEG or WebP"),
        ));
    }
    let info = if b.starts_with(PNG_MAGIC) {
        png(b)?
    } else if b.starts_with(&[0xFF, 0xD8]) {
        jpeg(b)?
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        webp(b)?
    } else {
        return Err((415, "not a PNG, JPEG or WebP image".into()));
    };
    if info.width == 0 || info.height == 0 {
        return Err(bad("the image declares no pixels (a width or height of 0)"));
    }
    if info.width > MAX_SIDE || info.height > MAX_SIDE {
        return Err(bad(format!(
            "the image declares {} x {} px; at most {MAX_SIDE} px a side is taken",
            info.width, info.height
        )));
    }
    Ok(info)
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 3)?;
    Some(u32::from(s[0]) | u32::from(s[1]) << 8 | u32::from(s[2]) << 16)
}

// ------------------------------------------------------------------ PNG

const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// CRC-32 (ISO 3309, as PNG uses it).
const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        #[allow(clippy::cast_possible_truncation)]
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[n] = c;
        n += 1;
    }
    t
};

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |c, &x| {
        CRC_TABLE[((c ^ u32::from(x)) & 0xFF) as usize] ^ (c >> 8)
    })
}

/// The critical chunks a still PNG may carry.
const PNG_CRITICAL: [&[u8; 4]; 4] = [b"IHDR", b"PLTE", b"IDAT", b"IEND"];

fn png(b: &[u8]) -> Result<Info, Refused> {
    let cut = || bad("the PNG is cut short or its chunks do not add up");
    let mut at = PNG_MAGIC.len();
    let mut size = None;
    let mut idat = false;
    loop {
        let len = be32(b, at).ok_or_else(cut)? as usize;
        let ty: [u8; 4] = b
            .get(at + 4..at + 8)
            .ok_or_else(cut)?
            .try_into()
            .map_err(|_| cut())?;
        let end = (at + 12).checked_add(len).ok_or_else(cut)?;
        if len > 0x7FFF_FFFF || end > b.len() {
            return Err(cut());
        }
        if !ty.iter().all(u8::is_ascii_alphabetic) {
            return Err(bad("the PNG has a chunk with a malformed type"));
        }
        let crc = be32(b, at + 8 + len).ok_or_else(cut)?;
        if crc32(&b[at + 4..at + 8 + len]) != crc {
            return Err(bad("a PNG chunk fails its CRC (the file is damaged or altered)"));
        }
        let data = &b[at + 8..at + 8 + len];
        if size.is_none() && &ty != b"IHDR" {
            return Err(bad("the PNG does not start with its header chunk"));
        }
        match &ty {
            b"IHDR" => {
                if size.is_some() || len != 13 {
                    return Err(bad("the PNG header chunk is malformed"));
                }
                let (w, h) = (be32(data, 0).ok_or_else(cut)?, be32(data, 4).ok_or_else(cut)?);
                let (depth, color) = (data[8], data[9]);
                let ok = match color {
                    0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
                    3 => matches!(depth, 1 | 2 | 4 | 8),
                    2 | 4 | 6 => matches!(depth, 8 | 16),
                    _ => false,
                };
                if !ok || data[10] != 0 || data[11] != 0 || data[12] > 1 {
                    return Err(bad("the PNG header declares an impossible image"));
                }
                size = Some((w, h));
            }
            b"IDAT" => idat = true,
            b"acTL" | b"fcTL" | b"fdAT" => {
                return Err(bad(
                    "animated PNGs are not taken (the model would see only the first frame)",
                ));
            }
            b"IEND" => {
                if !idat || len != 0 {
                    return Err(bad("the PNG ends before any image data"));
                }
                if end != b.len() {
                    return Err(bad(
                        "the PNG carries bytes after its end (possibly another file appended); re-export the photo",
                    ));
                }
                let (width, height) = size.ok_or_else(cut)?;
                return Ok(Info {
                    format: Format::Png,
                    width,
                    height,
                });
            }
            _ if ty[0].is_ascii_uppercase() && !PNG_CRITICAL.contains(&&ty) => {
                return Err(bad("the PNG has a critical chunk the box does not know"));
            }
            _ => {}
        }
        at = end;
    }
}

// ----------------------------------------------------------------- JPEG

fn jpeg(b: &[u8]) -> Result<Info, Refused> {
    let cut = || bad("the JPEG is cut short or its segments do not add up");
    let mut at = 2;
    let mut size: Option<(u32, u32)> = None;
    let mut scanned = false;
    loop {
        if *b.get(at).ok_or_else(cut)? != 0xFF {
            return Err(bad("the JPEG has bytes where a marker belongs"));
        }
        while b.get(at + 1) == Some(&0xFF) {
            at += 1; // fill bytes
        }
        let m = *b.get(at + 1).ok_or_else(cut)?;
        at += 2;
        match m {
            0xD9 => {
                let (width, height) = size.ok_or_else(|| bad("the JPEG has no frame header"))?;
                if !scanned {
                    return Err(bad("the JPEG ends before any image data"));
                }
                if at != b.len() {
                    return Err(bad(
                        "the JPEG carries bytes after its end (possibly another file appended); re-export the photo",
                    ));
                }
                return Ok(Info {
                    format: Format::Jpeg,
                    width,
                    height,
                });
            }
            0x01 => {}
            0x00 | 0xD0..=0xD8 => return Err(bad("the JPEG has a marker out of place")),
            _ => {
                let len = usize::from(be16(b, at).ok_or_else(cut)?);
                let end = at + len;
                if len < 2 || end > b.len() {
                    return Err(cut());
                }
                let seg = &b[at + 2..end];
                match m {
                    0xC0..=0xCF if !matches!(m, 0xC4 | 0xC8 | 0xCC) => {
                        if size.is_some() {
                            return Err(bad("the JPEG has more than one frame (a multi-image file)"));
                        }
                        let h = be16(seg, 1).ok_or_else(cut)?;
                        let w = be16(seg, 3).ok_or_else(cut)?;
                        size = Some((u32::from(w), u32::from(h)));
                    }
                    0xDA => {
                        if size.is_none() {
                            return Err(bad("the JPEG's image data comes before its frame header"));
                        }
                        scanned = true;
                        at = entropy(b, end).ok_or_else(cut)?;
                        continue;
                    }
                    _ => {}
                }
                at = end;
            }
        }
    }
}

/// Skips entropy-coded data from `at` to the next marker (an `0xFF` not
/// followed by a stuffed `0x00` or a restart marker); its offset.
fn entropy(b: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let k = b.get(at..)?.iter().position(|&x| x == 0xFF)?;
        at += k;
        match *b.get(at + 1)? {
            0x00 | 0xD0..=0xD7 => at += 2,
            0xFF => at += 1,
            _ => return Some(at),
        }
    }
}

// ----------------------------------------------------------------- WebP

fn webp(b: &[u8]) -> Result<Info, Refused> {
    let cut = || bad("the WebP is cut short or its chunks do not add up");
    let riff = le32(b, 4).ok_or_else(cut)? as usize;
    if riff.checked_add(8) != Some(b.len()) {
        return Err(bad(
            "the WebP's size field disagrees with the file (bytes appended or missing); re-export the photo",
        ));
    }
    let mut at = 12;
    let mut extended: Option<(u32, u32)> = None;
    let mut image: Option<(u32, u32)> = None;
    let mut first = true;
    while at < b.len() {
        let ty: [u8; 4] = b.get(at..at + 4).ok_or_else(cut)?.try_into().map_err(|_| cut())?;
        let len = le32(b, at + 4).ok_or_else(cut)? as usize;
        let start = at + 8;
        let end = start.checked_add(len).ok_or_else(cut)?;
        let next = end + (len & 1);
        if end > b.len() || next > b.len() {
            return Err(cut());
        }
        let data = &b[start..end];
        let shape = || bad("the WebP's chunks are not in a shape the box reads");
        let animated = || bad("animated WebPs are not taken (the model would see only the first frame)");
        match &ty {
            b"VP8X" => {
                if !first {
                    return Err(shape());
                }
                if len < 10 {
                    return Err(cut());
                }
                if data[0] & 0x02 != 0 {
                    return Err(animated());
                }
                extended = Some((
                    le24(data, 4).ok_or_else(cut)? + 1,
                    le24(data, 7).ok_or_else(cut)? + 1,
                ));
            }
            b"ANIM" | b"ANMF" => return Err(animated()),
            b"VP8 " | b"VP8L" => {
                if image.is_some() {
                    return Err(bad("the WebP holds more than one image"));
                }
                if !first && extended.is_none() {
                    return Err(shape());
                }
                image = Some(
                    if &ty == b"VP8 " { vp8(data) } else { vp8l(data) }
                        .ok_or_else(|| bad("the WebP's image header is malformed"))?,
                );
            }
            // Alpha, colour profile, EXIF, XMP: data an extended file may carry.
            _ if extended.is_some() => {}
            _ => return Err(shape()),
        }
        first = false;
        at = next;
    }
    let (width, height) = image.ok_or_else(|| bad("the WebP has no image data"))?;
    if extended.is_some_and(|canvas| canvas != (width, height)) {
        return Err(bad("the WebP's canvas and its image disagree on the size"));
    }
    Ok(Info {
        format: Format::Webp,
        width,
        height,
    })
}

/// A lossy key frame's size (RFC 6386 9.1).
fn vp8(d: &[u8]) -> Option<(u32, u32)> {
    if d.len() < 10 || d[0] & 1 != 0 || d[3..6] != [0x9D, 0x01, 0x2A] {
        return None;
    }
    let w = u16::from_le_bytes([d[6], d[7]]) & 0x3FFF;
    let h = u16::from_le_bytes([d[8], d[9]]) & 0x3FFF;
    Some((u32::from(w), u32::from(h)))
}

/// A lossless image's size (RFC 9649 3.2).
fn vp8l(d: &[u8]) -> Option<(u32, u32)> {
    if d.len() < 5 || d[0] != 0x2F {
        return None;
    }
    let bits = u32::from_le_bytes([d[1], d[2], d[3], d[4]]);
    if bits >> 29 != 0 {
        return None; // version must be 0
    }
    Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
}

// --------------------------------------------------------------- base64

/// Standard base64 with padding (RFC 4648 4), as the Messages API takes it.
#[must_use]
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = u32::from(c[0]) << 16
            | u32::from(c.get(1).copied().unwrap_or(0)) << 8
            | u32::from(c.get(2).copied().unwrap_or(0));
        for k in 0..4 {
            if k <= c.len() {
                out.push(char::from(ABC[(n >> (18 - 6 * k) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Length of [`base64`]'s output for `n` bytes.
#[must_use]
pub const fn base64_len(n: usize) -> usize {
    n.div_ceil(3) * 4
}

/// Decodes [`base64`]'s form and only that: standard alphabet, padded,
/// no whitespace, unused bits zero. `None` for anything else, so one
/// binary has one spelling.
#[must_use]
pub fn unbase64(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(4) {
        return None;
    }
    let digits = s.trim_end_matches('=');
    if s.len() - digits.len() > 2 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in digits.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = ((acc << 6) | u32::from(v)) & 0xff_ffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    // What is left must be padding's zero bits, and the padding exactly
    // what the length needs.
    (acc & ((1 << bits) - 1) == 0 && base64(&out) == s).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    mod images {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/photo/images.rs"
        ));
    }
    use images::*;

    const ALL: [(&[u8], Format); 6] = [
        (PNG_3X2, Format::Png),
        (JPEG_3X2, Format::Jpeg),
        (JPEG_3X2_PROGRESSIVE, Format::Jpeg),
        (WEBP_3X2_LOSSY, Format::Webp),
        (WEBP_3X2_LOSSLESS, Format::Webp),
        (WEBP_3X2_ALPHA, Format::Webp),
    ];

    fn status(b: &[u8]) -> u16 {
        sniff(b).map_or_else(|(s, _)| s, |_| 200)
    }

    fn why(b: &[u8]) -> String {
        sniff(b).expect_err("refused").1
    }

    /// A PNG chunk with its CRC.
    fn chunk(ty: &[u8], data: &[u8]) -> Vec<u8> {
        let mut c = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
        c.extend_from_slice(ty);
        c.extend_from_slice(data);
        let crc = crc32(&c[4..]);
        c.extend_from_slice(&crc.to_be_bytes());
        c
    }

    /// The 3 x 2 PNG's chunks, with `edit` applied to the list.
    fn png_with(edit: impl FnOnce(&mut Vec<Vec<u8>>)) -> Vec<u8> {
        let mut chunks = Vec::new();
        let mut at = 8;
        while at < PNG_3X2.len() {
            let len = be32(PNG_3X2, at).unwrap() as usize;
            chunks.push(PNG_3X2[at..at + 12 + len].to_vec());
            at += 12 + len;
        }
        edit(&mut chunks);
        let mut out = PNG_MAGIC.to_vec();
        out.extend(chunks.concat());
        out
    }

    fn ihdr(w: u32, h: u32) -> Vec<u8> {
        let mut d = w.to_be_bytes().to_vec();
        d.extend_from_slice(&h.to_be_bytes());
        d.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(b"IHDR", &d)
    }

    /// The baseline JPEG with its frame header's size replaced.
    fn jpeg_sized(w: u16, h: u16) -> Vec<u8> {
        let mut b = JPEG_3X2.to_vec();
        let sof = b.windows(2).position(|p| p == [0xFF, 0xC0]).unwrap();
        b[sof + 5..sof + 7].copy_from_slice(&h.to_be_bytes());
        b[sof + 7..sof + 9].copy_from_slice(&w.to_be_bytes());
        b
    }

    #[test]
    fn real_images_are_read_from_their_headers() {
        for (b, format) in ALL {
            let info = sniff(b).unwrap_or_else(|e| panic!("{format:?}: {e:?}"));
            assert_eq!(
                info,
                Info {
                    format,
                    width: 3,
                    height: 2
                }
            );
            assert!(is_image(b), "{format:?}");
        }
        assert_eq!(Format::Png.media_type(), "image/png");
        assert_eq!(Format::Jpeg.media_type(), "image/jpeg");
        assert_eq!(Format::Webp.media_type(), "image/webp");
    }

    #[test]
    fn every_truncation_is_refused() {
        for (b, format) in ALL {
            for k in 0..b.len() {
                assert!(sniff(&b[..k]).is_err(), "{format:?} cut to {k} bytes was taken");
            }
        }
    }

    #[test]
    fn bytes_after_the_end_are_refused_as_a_possible_polyglot() {
        let zip = b"PK\x03\x04\x14\x00\x00\x00\x08\x00payload.html<script>alert(1)</script>PK\x05\x06";
        for (b, format) in ALL {
            for tail in [&zip[..], b"<html><body>", b"\0", b"\xFF\xD9"] {
                let mut p = b.to_vec();
                p.extend_from_slice(tail);
                assert_eq!(status(&p), 422, "{format:?} + {tail:?}");
            }
        }
        assert!(why(&[PNG_3X2, b"PK"].concat()).contains("after its end"));
        assert!(why(&[JPEG_3X2, b"PK"].concat()).contains("after its end"));
        // A WebP whose RIFF size is fixed up to cover the appended file
        // still has to account for it as chunks.
        let mut w = WEBP_3X2_LOSSY.to_vec();
        w.extend_from_slice(zip);
        let riff = u32::try_from(w.len() - 8).unwrap();
        w[4..8].copy_from_slice(&riff.to_le_bytes());
        assert_eq!(status(&w), 422);
        // A PNG that hides the archive in a chunk after IEND.
        let p = png_with(|c| c.push(chunk(b"zIPx", zip)));
        assert_eq!(status(&p), 422, "{}", why(&p));
    }

    #[test]
    fn declared_dimensions_are_held_to_the_api_limit_whatever_the_file_size() {
        let at = |w, h| png_with(|c| c[0] = ihdr(w, h));
        assert_eq!(status(&at(MAX_SIDE, MAX_SIDE)), 200);
        assert_eq!(status(&at(MAX_SIDE + 1, 1)), 422);
        assert_eq!(status(&at(1, MAX_SIDE + 1)), 422);
        assert_eq!(status(&at(u32::MAX, u32::MAX)), 422);
        assert!(why(&at(65535, 65535)).contains("65535 x 65535"));
        assert_eq!(status(&at(0, 2)), 422);
        assert_eq!(status(&jpeg_sized(8000, 8000)), 200);
        assert_eq!(status(&jpeg_sized(65535, 65535)), 422);
        assert_eq!(status(&jpeg_sized(3, 0)), 422, "a JPEG height of 0 (DNL)");
        // A WebP canvas of 16M x 16M in a 96-byte file.
        let mut w = WEBP_3X2_ALPHA.to_vec();
        w[24..30].copy_from_slice(&[0xFF; 6]);
        assert_eq!(status(&w), 422);
    }

    #[test]
    fn damaged_and_odd_files_are_refused() {
        // A flipped bit in the image data fails the CRC.
        let mut p = PNG_3X2.to_vec();
        p[45] ^= 1;
        assert!(why(&p).contains("CRC"));
        // Header chunk not first, twice, or impossible.
        assert_eq!(status(&png_with(|c| c.swap(0, 1))), 422);
        assert_eq!(status(&png_with(|c| c.insert(1, ihdr(3, 2)))), 422);
        let bad_depth = {
            let mut d = 3u32.to_be_bytes().to_vec();
            d.extend_from_slice(&2u32.to_be_bytes());
            d.extend_from_slice(&[7, 2, 0, 0, 0]);
            chunk(b"IHDR", &d)
        };
        assert_eq!(status(&png_with(|c| c[0] = bad_depth)), 422);
        // No image data; an unknown critical chunk; animation.
        assert_eq!(status(&png_with(|c| drop(c.remove(1)))), 422);
        assert!(why(&png_with(|c| c.insert(1, chunk(b"EVIL", b"x")))).contains("critical"));
        assert!(
            why(&png_with(
                |c| c.insert(1, chunk(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]))
            ))
            .contains("animated")
        );
        // A known ancillary chunk is fine.
        assert_eq!(status(&png_with(|c| c.insert(1, chunk(b"pHYs", &[0; 9])))), 200);
        // A chunk length that runs past the end.
        let mut p = PNG_3X2.to_vec();
        p[33..37].copy_from_slice(&0x7FFF_FFF0u32.to_be_bytes());
        assert_eq!(status(&p), 422);

        // JPEG: two frames, a scan before the frame, a stray restart marker.
        let sof = JPEG_3X2.windows(2).position(|p| p == [0xFF, 0xC0]).unwrap();
        let mut two = JPEG_3X2[..sof + 19].to_vec();
        two.extend_from_slice(&JPEG_3X2[sof..]);
        assert!(why(&two).contains("more than one frame"));
        let sos = JPEG_3X2.windows(2).position(|p| p == [0xFF, 0xDA]).unwrap();
        let mut early = JPEG_3X2[..sof].to_vec();
        early.extend_from_slice(&JPEG_3X2[sos..]);
        assert_eq!(status(&early), 422);
        let mut rst = JPEG_3X2[..2].to_vec();
        rst.extend_from_slice(&[0xFF, 0xD0]);
        rst.extend_from_slice(&JPEG_3X2[2..]);
        assert_eq!(status(&rst), 422);

        // WebP: animation flag, an animation frame, a second image.
        let mut anim = WEBP_3X2_ALPHA.to_vec();
        anim[20] |= 0x02;
        assert!(why(&anim).contains("animated"));
        let mut two = WEBP_3X2_LOSSLESS.to_vec();
        two.extend_from_slice(&WEBP_3X2_LOSSLESS[12..]);
        let riff = u32::try_from(two.len() - 8).unwrap();
        two[4..8].copy_from_slice(&riff.to_le_bytes());
        assert_eq!(status(&two), 422);
        let mut lossy = WEBP_3X2_LOSSY.to_vec();
        lossy[23] = 0; // the key frame's start code
        assert!(why(&lossy).contains("image header"));
    }

    #[test]
    fn the_format_is_the_bytes_own_and_others_are_named() {
        assert_eq!(status(b"GIF89a\x03\x00\x02\x00"), 415);
        assert!(why(b"GIF89a\x03\x00\x02\x00").contains("GIF"));
        assert!(why(b"\0\0\0\x18ftypheic\0\0\0\0mif1heic").contains("HEIC"));
        assert!(why(b"\0\0\0\x1cftypavif\0\0\0\0avifmif1").contains("AVIF"));
        assert!(why(b"II*\0\x08\0\0\0").contains("TIFF"));
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0x46, 0, 0, 0, 0, 0, 0, 0, 0x36, 0, 0, 0, 40, 0, 0, 0]);
        assert!(why(&bmp).contains("BMP"));
        assert_eq!(status(b"%PDF-1.7"), 415);
        assert_eq!(status(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"), 415);
        assert_eq!(status(b""), 415);
        // Text that starts like a magic number is still text.
        assert!(!is_image(b"BMW conveyor, line 3: flash the beacon on a jam"));
        assert!(!is_image(b"RIFF raff on the night shift"));
        assert!(!is_image(b"# SOP"));
        // A PNG signature followed by text is not a PNG.
        assert_eq!(status(&[PNG_MAGIC, b"hello"].concat()), 422);
    }

    #[test]
    fn size_is_capped_before_anything_is_parsed() {
        let mut big = PNG_3X2.to_vec();
        big.resize(MAX_PHOTO + 1, 0);
        assert_eq!(status(&big), 413);
        let junk = vec![0x42; MAX_PHOTO + 1];
        assert_eq!(status(&junk), 413);
    }

    #[test]
    fn unbase64_takes_only_the_canonical_form() {
        for (plain, enc) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(unbase64(enc).as_deref(), Some(plain.as_bytes()), "{enc}");
        }
        for bad in [
            "Zg", "Zg=", "Zh==", "Zm9v\n", " Zm9v", "Zm9v====", "Zm=v", "Z===", "Zm9-", "Zm9_",
        ] {
            assert_eq!(unbase64(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn base64_matches_rfc_4648() {
        for (raw, enc) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(raw.as_bytes()), enc);
            assert_eq!(base64_len(raw.len()), enc.len());
        }
        assert_eq!(base64(&[0xFB, 0xFF, 0xBF]), "+/+/");
    }

    proptest! {
        /// Whatever bytes arrive, the sniffer refuses or reads them without
        /// panicking, and what it reads is within the limits.
        #[test]
        fn any_bytes_are_read_or_refused(b in prop::collection::vec(any::<u8>(), 0..512)) {
            if let Ok(i) = sniff(&b) {
                prop_assert!(i.width >= 1 && i.width <= MAX_SIDE && i.height >= 1 && i.height <= MAX_SIDE);
            }
        }

        /// Real images with bytes flipped: refused or read, never a panic,
        /// never an out-of-range size.
        #[test]
        fn mutated_images_are_read_or_refused(
            which in 0..ALL.len(),
            flips in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8),
        ) {
            let mut b = ALL[which].0.to_vec();
            for (at, x) in flips {
                let k = at.index(b.len());
                b[k] ^= x;
            }
            if let Ok(i) = sniff(&b) {
                prop_assert!(i.width >= 1 && i.width <= MAX_SIDE && i.height >= 1 && i.height <= MAX_SIDE);
            }
        }

        #[test]
        fn unbase64_inverts_base64(b in prop::collection::vec(any::<u8>(), 0..300)) {
            prop_assert_eq!(unbase64(&base64(&b)), Some(b));
        }

        #[test]
        fn base64_has_the_promised_length_and_alphabet(b in prop::collection::vec(any::<u8>(), 0..300)) {
            let e = base64(&b);
            prop_assert_eq!(e.len(), base64_len(b.len()));
            prop_assert!(e.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'='));
        }
    }
}
