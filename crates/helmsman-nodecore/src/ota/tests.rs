//! The update path under hostile input and cut power: supervictor's
//! installer, head-parser and policy proptests, ported and extended with a
//! power cut at every write of a whole update.

extern crate std;

use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

use proptest::prelude::*;
use sha2::{Digest, Sha256};

use super::boot::{self, Boot, Confirmed, Decision, MAX_PENDING_BOOTS, Otadata, Step};
use super::image::{self, Chip, HEADER_LEN, Header, ImageError, MAGIC};
use super::install::{Image, InstallError, Installer, SECTOR, Slot, Verified};
use super::record::{self, Copy, Trial};
use super::request::{self, Fetch, Invalid};
use super::sim::{Cut, Fault, Part, SLOT_CAP, SimOta, SimPart, World};
use super::wire::{self, HeadError};

// ---- images ----

fn image_bytes(chip: Chip, size: usize, seed: u64) -> Vec<u8> {
    assert!(size >= HEADER_LEN);
    let mut img = vec![0u8; size];
    img[0] = MAGIC;
    img[1] = 4;
    img[12..14].copy_from_slice(&chip.raw().to_le_bytes());
    img[23] = 1;
    let mut x = (seed << 1) | 1;
    for b in &mut img[HEADER_LEN..] {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x.to_le_bytes()[0];
    }
    img
}

fn want(img: &[u8], chip: Chip) -> Image {
    Image {
        size: u32::try_from(img.len()).unwrap(),
        sha256: Sha256::digest(img).into(),
        chip,
    }
}

/// Installs `img` into slot 1 of a fresh world in `chunks`; the world comes
/// back for inspection.
fn install(
    img: &[u8],
    want: Image,
    chunks: &[usize],
    setup: impl FnOnce(&mut World),
) -> (Result<Verified, InstallError<Fault>>, super::sim::Shared) {
    let w = World::new();
    setup(&mut w.borrow_mut());
    let mut buf = [0u8; SECTOR];
    let result = (|| {
        let mut inst = Installer::new(SimPart::new(&w, Part::Slot1), &mut buf, want)?;
        let mut pos = 0;
        let mut i = 0;
        while pos < img.len() {
            let n = chunks[i % chunks.len()].clamp(1, img.len() - pos);
            inst.feed(&img[pos..pos + n])?;
            pos += n;
            i += 1;
        }
        inst.finish()
    })();
    (result, w)
}

// ---- image header ----

#[test]
fn header_refusals() {
    let good = image_bytes(Chip::Esp32P4, 100, 1);
    let h = Header::parse(&good).unwrap();
    assert_eq!(h.chip, Chip::Esp32P4);
    assert!(h.hash_appended);
    assert_eq!(Header::parse(&good[..23]), Err(ImageError::TooShort));
    assert_eq!(Header::parse(&[]), Err(ImageError::TooShort));
    let mut bad = good.clone();
    bad[0] = 0;
    assert_eq!(Header::parse(&bad), Err(ImageError::BadMagic));
    for segments in [0u8, 17, 255] {
        let mut bad = good.clone();
        bad[1] = segments;
        assert_eq!(Header::parse(&bad), Err(ImageError::BadSegmentCount));
    }
    let mut bad = good.clone();
    bad[12] = 0x77;
    assert_eq!(Header::parse(&bad), Err(ImageError::UnknownChip(0x77)));
    assert!(!image::fits(0, 1000));
    assert!(!image::fits(23, 1000));
    assert!(image::fits(24, 1000));
    assert!(image::fits(1000, 1000));
    assert!(!image::fits(1001, 1000));
}

proptest! {
    #[test]
    fn header_parse_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        let _ = Header::parse(&bytes);
    }
}

// ---- the request ----

#[test]
fn a_request_is_checked_field_by_field() {
    let sha = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    let f = Fetch::new("http://192.168.3.10:8000/node.bin", sha).unwrap();
    assert_eq!(f.host, [192, 168, 3, 10]);
    assert_eq!(f.port, 8000);
    assert_eq!(f.path, "/node.bin");
    assert_eq!(request::encode_hex32(&f.sha256), sha);
    assert_eq!(f.authority(), "192.168.3.10:8000");
    assert_eq!(
        Fetch::new("http://10.0.0.1/x?a=b", &sha.to_uppercase())
            .unwrap()
            .port,
        80
    );

    let hostile: &[(&str, &str)] = &[
        ("https://10.0.0.1/x", "TLS"),
        ("ftp://10.0.0.1/x", "scheme"),
        ("HTTP://10.0.0.1/x", "case"),
        ("10.0.0.1/x", "no scheme"),
        ("http://10.0.0.1", "no path"),
        ("http://example.com/x", "a name"),
        ("http://[::1]/x", "ipv6"),
        ("http://10.0.0.1.5/x", "five octets"),
        ("http://10.0.0/x", "three octets"),
        ("http://10.0.0.256/x", "octet over 255"),
        ("http://010.0.0.1/x", "leading zero"),
        ("http://10.0.0.-1/x", "sign"),
        ("http://user:pw@10.0.0.1/x", "userinfo"),
        ("http://10.0.0.1:0/x", "port 0"),
        ("http://10.0.0.1:65536/x", "port over 65535"),
        ("http://10.0.0.1:80a/x", "port text"),
        ("http://10.0.0.1:/x", "empty port"),
        ("http://10.0.0.1/a b", "space in the path"),
        ("http://10.0.0.1/a\r\nHost: evil", "header injection"),
        ("http://10.0.0.1/a\u{e9}", "non-ascii"),
        ("http://10.0.0.1/../etc", "dotdot"),
        ("http://10.0.0.1/x#frag", "fragment"),
        ("", "empty"),
    ];
    for (url, why) in hostile {
        assert!(
            matches!(Fetch::new(url, sha), Err(Invalid::Url(_))),
            "{why}: {url}"
        );
    }
    let long = std::format!("http://10.0.0.1/{}", "a".repeat(request::MAX_PATH));
    assert!(matches!(Fetch::new(&long, sha), Err(Invalid::Url(_))));
    for digest in [
        "",
        "ba78",
        &sha[..63],
        &std::format!("{sha}0"),
        &sha.replace('b', "g"),
    ] {
        assert_eq!(
            Fetch::new("http://10.0.0.1/x", digest),
            Err(Invalid::Digest),
            "{digest}"
        );
    }
}

#[test]
fn a_request_body_is_json_with_two_strings() {
    let sha = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    let ok = std::format!(r#"{{"url":"http://10.0.0.1:9/n.bin","sha256":"{sha}"}}"#);
    assert!(Fetch::from_json(ok.as_bytes()).is_ok());
    for body in [
        String::new(),
        "[]".to_string(),
        "null".to_string(),
        "{}".to_string(),
        std::format!(r#"{{"url":5,"sha256":"{sha}"}}"#),
        r#"{"url":"http://10.0.0.1/x"}"#.to_string(),
        std::format!(r#"{{"sha256":"{sha}"}}"#),
        std::format!(r#"{{"url":["http://10.0.0.1/x"],"sha256":"{sha}"}}"#),
    ] {
        assert_eq!(Fetch::from_json(body.as_bytes()), Err(Invalid::Shape), "{body}");
    }
}

proptest! {
    #[test]
    fn request_parsing_never_panics_and_only_accepts_what_it_can_serve(url in "\\PC{0,80}", digest in "\\PC{0,70}") {
        if let Ok(f) = Fetch::new(&url, &digest) {
            prop_assert!(f.path.starts_with('/'));
            prop_assert!(f.path.bytes().all(|b| b.is_ascii_graphic()));
            prop_assert!(f.port != 0);
            let req = wire::image_request(&f);
            prop_assert_eq!(req.matches("\r\n").count(), 6);
        }
    }

    #[test]
    fn hex_round_trips(d in proptest::array::uniform32(any::<u8>())) {
        let h = request::encode_hex32(&d);
        prop_assert_eq!(request::decode_hex32(&h), Some(d));
    }
}

// ---- the response head ----

#[test]
fn heads_that_must_be_refused() {
    let limit = 0x20_0000;
    let ok = b"HTTP/1.0 200 OK\r\nContent-Length: 100\r\nServer: x\r\n\r\nBODY";
    let (head, at) = wire::parse_head(ok, limit).unwrap();
    assert_eq!((head.status, head.content_length), (200, Some(100)));
    assert_eq!(&ok[at..], b"BODY");
    assert!(wire::parse_head(b"HTTP/1.1 200 OK\r\ncontent-length: 7\r\n\r\n", limit).is_ok());

    let table: &[(&[u8], HeadError)] = &[
        (b"HTTP/1.1 200 OK\r\nContent-Length: 1", HeadError::Incomplete),
        (b"", HeadError::Incomplete),
        (b"HTTP/2 200 OK\r\n\r\n", HeadError::Malformed),
        (b"HTTP/1.1 20 OK\r\n\r\n", HeadError::Malformed),
        (b"HTTP/1.1 2000 OK\r\n\r\n", HeadError::Malformed),
        (b"HTTP/1.1 abc OK\r\n\r\n", HeadError::Malformed),
        (b"200 OK\r\n\r\n", HeadError::Malformed),
        (b"HTTP/1.1 200 OK\r\nno colon here\r\n\r\n", HeadError::Malformed),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: abc\r\n\r\n",
            HeadError::Malformed,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
            HeadError::Malformed,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999\r\n\r\n",
            HeadError::Malformed,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length:\r\n\r\n",
            HeadError::Malformed,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 2097153\r\n\r\n",
            HeadError::TooLarge,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 4294967295\r\n\r\n",
            HeadError::TooLarge,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n",
            HeadError::Unsupported,
        ),
        (
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
            HeadError::Unsupported,
        ),
        (
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: identity\r\n\r\n",
            HeadError::Unsupported,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: \xff\xfe\r\n\r\n",
            HeadError::Malformed,
        ),
    ];
    for (raw, want) in table {
        assert_eq!(
            wire::parse_head(raw, 0x20_0000).unwrap_err(),
            *want,
            "{:?}",
            String::from_utf8_lossy(raw)
        );
    }
    // The same length twice is fine; a response with no length parses and
    // the caller refuses it.
    let twice = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\n";
    assert_eq!(wire::parse_head(twice, limit).unwrap().0.content_length, Some(5));
    let none = wire::parse_head(b"HTTP/1.1 200 OK\r\n\r\n", limit).unwrap().0;
    assert_eq!(none.content_length, None);
    assert_eq!(
        wire::parse_head(b"HTTP/1.1 404 Not Found\r\n\r\n", limit)
            .unwrap()
            .0
            .status,
        404
    );
}

proptest! {
    #[test]
    fn head_parsing_never_panics_and_honours_the_limit(bytes in proptest::collection::vec(any::<u8>(), 0..300), limit in 0u32..5000) {
        if let Ok((head, at)) = wire::parse_head(&bytes, limit) {
            prop_assert!(at <= bytes.len());
            prop_assert!(head.content_length.is_none_or(|n| n <= limit));
        }
    }

    #[test]
    fn a_wellformed_head_survives_any_split(len in 0u32..100_000, split in 0usize..120) {
        let raw = std::format!("HTTP/1.1 200 OK\r\nContent-Length: {len}\r\nX: y\r\n\r\nabc");
        let b = raw.as_bytes();
        let cut = split.min(b.len());
        let partial = wire::parse_head(&b[..cut], 200_000);
        let head_end = raw.find("\r\n\r\n").unwrap() + 4;
        if cut < head_end {
            prop_assert_eq!(partial.unwrap_err(), HeadError::Incomplete);
        } else {
            let (h, at) = partial.unwrap();
            prop_assert_eq!(h.content_length, Some(len));
            prop_assert_eq!(at, head_end);
        }
    }
}

// ---- the installer ----

const CAP: u32 = super::sim::SLOT_CAP_U32;

#[test]
fn installs_any_size_in_any_chunking_and_the_slot_holds_it() {
    for size in [HEADER_LEN, 100, 4095, 4096, 4097, 8192, 20_000, SLOT_CAP] {
        let img = image_bytes(Chip::Esp32P4, size, 1);
        for chunks in [&[1usize][..], &[7, 13], &[4096], &[1500], &[10_000]] {
            let (v, w) = install(&img, want(&img, Chip::Esp32P4), chunks, |_| {});
            let v = v.unwrap_or_else(|e| panic!("size {size} chunks {chunks:?}: {e:?}"));
            assert_eq!(v.size as usize, size);
            assert_eq!(&w.borrow().slots[1][..size], &img[..]);
            assert!(
                w.borrow().slots[1][size..].iter().all(|b| *b == 0xFF),
                "bytes past the image stay erased"
            );
        }
    }
}

#[test]
fn either_slot_takes_an_image() {
    let img = image_bytes(Chip::Esp32P4, 6_000, 9);
    let w = World::new();
    let mut buf = [0u8; SECTOR];
    let mut inst =
        Installer::new(SimPart::new(&w, Part::Slot0), &mut buf, want(&img, Chip::Esp32P4)).unwrap();
    inst.feed(&img).unwrap();
    inst.finish().unwrap();
    assert_eq!(&w.borrow().slots[0][..img.len()], &img[..]);
    assert!(w.borrow().slots[1].iter().all(|b| *b == 0xFF));
}

#[test]
fn a_wrong_digest_is_refused_and_nothing_is_activated() {
    let img = image_bytes(Chip::Esp32P4, 10_000, 2);
    let mut w = want(&img, Chip::Esp32P4);
    w.sha256[0] ^= 1;
    let (r, world) = install(&img, w, &[4096], |_| {});
    assert_eq!(r.unwrap_err(), InstallError::DigestMismatch);
    assert_eq!(world.borrow().selected, 0);
}

#[test]
fn truncated_oversized_and_overrun_images() {
    let img = image_bytes(Chip::Esp32P4, 10_000, 3);
    // The server stops early.
    let w = World::new();
    let mut buf = [0u8; SECTOR];
    let mut inst =
        Installer::new(SimPart::new(&w, Part::Slot1), &mut buf, want(&img, Chip::Esp32P4)).unwrap();
    inst.feed(&img[..9_999]).unwrap();
    assert_eq!(inst.remaining(), 1);
    assert_eq!(
        inst.finish().unwrap_err(),
        InstallError::Truncated {
            got: 9_999,
            want: 10_000
        }
    );
    // Nothing at all.
    let mut buf = [0u8; SECTOR];
    let inst = Installer::new(SimPart::new(&w, Part::Slot1), &mut buf, want(&img, Chip::Esp32P4)).unwrap();
    assert_eq!(
        inst.finish().unwrap_err(),
        InstallError::Truncated { got: 0, want: 10_000 }
    );
    // The server sends more than it said.
    let mut buf = [0u8; SECTOR];
    let mut inst =
        Installer::new(SimPart::new(&w, Part::Slot1), &mut buf, want(&img, Chip::Esp32P4)).unwrap();
    assert_eq!(
        inst.feed(&[img.as_slice(), &[0]].concat()).unwrap_err(),
        InstallError::Overrun
    );
    // An image the slot cannot hold, or that is not one.
    for size in [0u32, 1, 23, CAP + 1, u32::MAX] {
        let mut buf = [0u8; SECTOR];
        let r = Installer::new(
            SimPart::new(&w, Part::Slot1),
            &mut buf,
            Image {
                size,
                sha256: [0; 32],
                chip: Chip::Esp32P4,
            },
        );
        assert_eq!(r.err(), Some(InstallError::TooLarge), "size {size}");
    }
}

#[test]
fn a_bad_header_or_another_chip_is_refused_at_the_first_sector() {
    let mut img = image_bytes(Chip::Esp32P4, 20_000, 4);
    let (r, w) = install(&img, want(&img, Chip::Esp32), &[4096], |_| {});
    assert_eq!(
        r.unwrap_err(),
        InstallError::ChipMismatch { found: Chip::Esp32P4 }
    );
    assert!(
        w.borrow().slots[1].iter().all(|b| *b == 0xFF),
        "no sector was written"
    );
    img[0] = 0x7F;
    let (r, _) = install(&img, want(&img, Chip::Esp32P4), &[100], |_| {});
    assert_eq!(r.unwrap_err(), InstallError::BadImage(ImageError::BadMagic));
    // A whole image of zeros, or text, even with the right digest.
    for junk in [vec![0u8; 5000], b"<html>not found</html>".repeat(300)] {
        let (r, _) = install(&junk, want(&junk, Chip::Esp32P4), &[4096], |_| {});
        assert_eq!(r.unwrap_err(), InstallError::BadImage(ImageError::BadMagic));
    }
}

#[test]
fn flash_that_fails_or_lies_is_caught() {
    let img = image_bytes(Chip::Esp32P4, 20_000, 5);
    let (r, _) = install(&img, want(&img, Chip::Esp32P4), &[4096], |w| {
        w.fail_write = Some((Part::Slot1, 5000));
    });
    assert_eq!(r.unwrap_err(), InstallError::Slot(Fault::WriteFailed));
    let (r, _) = install(&img, want(&img, Chip::Esp32P4), &[4096], |w| {
        w.corrupt_write = Some((Part::Slot1, 9000));
    });
    assert_eq!(r.unwrap_err(), InstallError::ReadbackMismatch);
    // The last, short sector too.
    let (r, _) = install(&img, want(&img, Chip::Esp32P4), &[3000], |w| {
        w.corrupt_write = Some((Part::Slot1, 19_999));
    });
    assert_eq!(r.unwrap_err(), InstallError::ReadbackMismatch);
}

proptest! {
    #[test]
    fn verified_means_the_slot_holds_exactly_the_image(
        size in HEADER_LEN..40_000usize,
        seed in any::<u64>(),
        chunks in proptest::collection::vec(1usize..9000, 1..6),
        flip in proptest::option::of(0usize..40_000),
        cut in proptest::option::of(0usize..40_000),
    ) {
        let img = image_bytes(Chip::Esp32P4, size, seed);
        let w = want(&img, Chip::Esp32P4);
        // What arrives may differ from what was announced.
        let mut sent = img.clone();
        if let Some(f) = flip.filter(|f| *f < sent.len()) { sent[f] ^= 0x01; }
        if let Some(c) = cut { sent.truncate(c.min(sent.len())); }
        let (r, world) = install_sent(&sent, w, &chunks);
        match r {
            Ok(v) => {
                prop_assert_eq!(&sent, &img);
                prop_assert_eq!(v.sha256, w.sha256);
                prop_assert_eq!(&world.borrow().slots[1][..size], &img[..]);
            }
            Err(_) => prop_assert!(sent != img, "the image as announced must install"),
        }
    }
}

fn install_sent(
    sent: &[u8],
    w: Image,
    chunks: &[usize],
) -> (Result<Verified, InstallError<Fault>>, super::sim::Shared) {
    let world = World::new();
    let mut buf = [0u8; SECTOR];
    let r = (|| {
        let mut inst = Installer::new(SimPart::new(&world, Part::Slot1), &mut buf, w)?;
        let mut pos = 0;
        let mut i = 0;
        while pos < sent.len() {
            let n = chunks[i % chunks.len()].clamp(1, sent.len() - pos);
            inst.feed(&sent[pos..pos + n])?;
            pos += n;
            i += 1;
        }
        inst.finish()
    })();
    (r, world)
}

// ---- the record ----

#[test]
fn a_record_round_trips_and_every_damaged_byte_is_refused() {
    let t = Trial::On {
        target: 1,
        previous: 0,
        boots: 2,
    };
    let f = record::encode(t, 77);
    assert_eq!(record::decode(&f), Some((77, t)));
    assert_eq!(
        record::decode(&record::encode(Trial::None, 5)),
        Some((5, Trial::None))
    );
    for i in 0..record::LEN {
        for bit in 0..8 {
            let mut bad = f;
            bad[i] ^= 1 << bit;
            assert_eq!(record::decode(&bad), None, "byte {i} bit {bit}");
        }
    }
    assert_eq!(record::decode(&f[..31]), None);
    assert_eq!(record::decode(&[0xFF; 32]), None, "erased flash is no record");
    assert_eq!(record::decode(&[0; 32]), None);
    // Slots that make no sense, with a good checksum.
    for (target, previous) in [(2u8, 0u8), (0, 2), (1, 1), (0, 0), (255, 0)] {
        let frame = record::encode(
            Trial::On {
                target,
                previous,
                boots: 0,
            },
            1,
        );
        assert_eq!(record::decode(&frame), None, "{target}/{previous}");
    }
}

#[test]
fn the_newer_valid_copy_wins_across_the_wrap() {
    let a = record::encode(Trial::None, 5);
    let b = record::encode(
        Trial::On {
            target: 1,
            previous: 0,
            boots: 0,
        },
        6,
    );
    assert_eq!(record::choose(&a, &b).unwrap().copy, Copy::B);
    assert_eq!(record::choose(&b, &a).unwrap().copy, Copy::A);
    let near = record::encode(Trial::None, u32::MAX);
    let wrapped = record::encode(Trial::None, 0);
    assert_eq!(record::choose(&near, &wrapped).unwrap().copy, Copy::B);
    assert_eq!(record::choose(&[0xFF; 32], &b).unwrap().copy, Copy::B);
    assert_eq!(record::choose(&a, &[0xAB; 32]).unwrap().copy, Copy::A);
    assert_eq!(record::choose(&[0xFF; 32], &[0xFF; 32]), None);
    assert_eq!(record::next(None), (Copy::A, 1));
    let cur = record::choose(&a, &b).unwrap();
    assert_eq!(record::next(Some(&cur)), (Copy::A, 7));
}

// ---- the policy ----

#[test]
fn the_policy_table() {
    let on = |boots| Trial::On {
        target: 1,
        previous: 0,
        boots,
    };
    assert_eq!(boot::decide(Trial::None, 0), Step::Normal);
    assert_eq!(boot::decide(Trial::None, 1), Step::Normal);
    assert_eq!(
        boot::decide(on(0), 0),
        Step::Stale,
        "on trial, but another slot runs"
    );
    assert_eq!(boot::decide(on(0), 1), Step::Count { boots: 1 });
    assert_eq!(
        boot::decide(on(MAX_PENDING_BOOTS - 1), 1),
        Step::Count {
            boots: MAX_PENDING_BOOTS
        }
    );
    assert_eq!(
        boot::decide(on(MAX_PENDING_BOOTS), 1),
        Step::Revert { previous: 0 }
    );
    assert_eq!(
        boot::decide(on(255), 1),
        Step::Revert { previous: 0 },
        "no overflow"
    );
    assert_eq!(MAX_PENDING_BOOTS, 3);
}

proptest! {
    #[test]
    fn a_trial_ends_within_the_budget(target in 0u8..2, running in 0u8..2, boots in any::<u8>()) {
        let trial = Trial::On { target, previous: 1 - target, boots };
        match boot::decide(trial, running) {
            Step::Normal => prop_assert!(false, "a trial is never normal"),
            Step::Stale => prop_assert!(running != target),
            Step::Count { boots: n } => {
                prop_assert!(running == target);
                prop_assert!((1..=MAX_PENDING_BOOTS).contains(&n));
            }
            Step::Revert { previous } => {
                prop_assert!(running == target);
                prop_assert_eq!(previous, 1 - target);
            }
        }
    }
}

fn boot_of(w: &super::sim::Shared) -> Result<Boot<SimPart, SimOta>, boot::Error<Fault, Fault>> {
    Boot::open(
        SimPart::new(w, Part::Record),
        SimOta {
            w: std::rc::Rc::clone(w),
        },
    )
}

#[test]
fn a_new_image_that_never_confirms_is_replaced_at_the_fourth_boot() {
    let w = World::new();
    let mut b = boot_of(&w).unwrap();
    assert_eq!(b.on_boot().unwrap(), Decision::Normal);
    b.start_trial(0, 1).unwrap();
    assert_eq!(w.borrow().selected, 1);
    drop(b);
    for boots in 1..=MAX_PENDING_BOOTS {
        let mut b = boot_of(&w).unwrap();
        assert_eq!(b.on_boot().unwrap(), Decision::Confirming { boots });
    }
    let mut b = boot_of(&w).unwrap();
    assert_eq!(b.on_boot().unwrap(), Decision::Reverted { to: 0 });
    assert_eq!(w.borrow().selected, 0);
    assert_eq!(b.trial(), Trial::None);
    assert_eq!(boot_of(&w).unwrap().on_boot().unwrap(), Decision::Normal);
}

/// The bench, 2026-11-18: the old image's confirm timer fired after the
/// update had selected the new slot and before the reset, ended the new
/// image's trial, and a build that crashed at every boot was never replaced.
#[test]
fn a_late_confirm_from_the_old_image_does_not_end_the_new_trial() {
    let w = World::new();
    let mut old = boot_of(&w).unwrap();
    old.start_trial(0, 1).unwrap();
    assert_eq!(
        w.borrow().selected,
        1,
        "the new slot is selected, the old image still runs"
    );
    assert_eq!(old.confirm(0).unwrap(), Confirmed::NothingToConfirm);
    assert_eq!(
        boot_of(&w).unwrap().confirm(0).unwrap(),
        Confirmed::NothingToConfirm
    );
    // After the reset the new image is still on trial and, never
    // confirming, is replaced.
    for boots in 1..=MAX_PENDING_BOOTS {
        assert_eq!(
            boot_of(&w).unwrap().on_boot().unwrap(),
            Decision::Confirming { boots }
        );
    }
    assert_eq!(
        boot_of(&w).unwrap().on_boot().unwrap(),
        Decision::Reverted { to: 0 }
    );
}

#[test]
fn a_confirmed_image_stays_and_confirming_twice_changes_nothing() {
    let w = World::new();
    let mut b = boot_of(&w).unwrap();
    assert_eq!(
        b.confirm(0).unwrap(),
        Confirmed::NothingToConfirm,
        "nothing is on trial yet"
    );
    b.start_trial(0, 1).unwrap();
    let mut b = boot_of(&w).unwrap();
    assert_eq!(b.on_boot().unwrap(), Decision::Confirming { boots: 1 });
    assert_eq!(b.confirm(1).unwrap(), Confirmed::Yes);
    assert_eq!(b.confirm(1).unwrap(), Confirmed::NothingToConfirm);
    for _ in 0..10 {
        assert_eq!(boot_of(&w).unwrap().on_boot().unwrap(), Decision::Normal);
    }
    assert_eq!(w.borrow().selected, 1);
}

#[test]
fn updating_the_running_slot_is_refused_and_a_usb_flash_clears_a_stale_trial() {
    let w = World::new();
    let mut b = boot_of(&w).unwrap();
    assert_eq!(b.start_trial(0, 0).unwrap_err(), boot::Error::SameSlot);
    assert_eq!(b.start_trial(0, 2).unwrap_err(), boot::Error::SameSlot);
    assert_eq!(w.borrow().ops, 0, "nothing was written");
    b.start_trial(0, 1).unwrap();
    // A cable flash puts slot 0 back with otadata erased.
    w.borrow_mut().selected = 0;
    let mut b = boot_of(&w).unwrap();
    assert_eq!(b.on_boot().unwrap(), Decision::Normal);
    assert_eq!(b.trial(), Trial::None);
}

#[test]
fn a_damaged_record_means_no_trial_never_a_revert() {
    let w = World::new();
    boot_of(&w).unwrap().start_trial(0, 1).unwrap();
    w.borrow_mut().record.fill(0xA5);
    let mut b = boot_of(&w).unwrap();
    assert_eq!(b.trial(), Trial::None);
    assert_eq!(b.on_boot().unwrap(), Decision::Normal);
    assert_eq!(w.borrow().selected, 1, "garbage never switches a slot");
}

// ---- a power cut at every write of a whole update ----

struct Run {
    selected: u8,
    trial: Trial,
    boots_seen: usize,
    ops: usize,
}

/// One update of slot 1 over a running slot 0, then boots until it settles,
/// with the power cut at write number `cut` if given. `confirms` says whether
/// the new image ever confirms itself. At every boot the slot the bootloader
/// would run must hold a whole image.
fn lifecycle(cut: Option<(usize, Cut)>, confirms: bool, old: &[u8], new: &[u8], bad_chip_new: bool) -> Run {
    let w = World::new();
    w.borrow_mut().slots[0][..old.len()].copy_from_slice(old);
    w.borrow_mut().cut_at = cut;
    let target = Image {
        size: u32::try_from(new.len()).unwrap(),
        sha256: Sha256::digest(new).into(),
        chip: if bad_chip_new { Chip::Esp32 } else { Chip::Esp32P4 },
    };

    // The update: fetch into the idle slot, verify, start the trial.
    let attempt = (|| -> Result<(), Fault> {
        let mut b = boot_of(&w).map_err(|_| Fault::PowerCut)?;
        b.on_boot().map_err(|_| Fault::PowerCut)?;
        let mut buf = [0u8; SECTOR];
        let mut inst =
            Installer::new(SimPart::new(&w, Part::Slot1), &mut buf, target).map_err(|_| Fault::PowerCut)?;
        for chunk in new.chunks(1500) {
            inst.feed(chunk).map_err(|_| Fault::PowerCut)?;
        }
        inst.finish().map_err(|_| Fault::PowerCut)?;
        b.start_trial(0, 1).map_err(|_| Fault::PowerCut)
    })();
    let _ = attempt;

    // Boots, until a boot changes nothing; the cut, if it has not come yet,
    // may fall inside any of them.
    let mut boots_seen = 0;
    let mut quiet = 0;
    while boots_seen < 20 && quiet < 2 {
        w.borrow_mut().power_on();
        boots_seen += 1;
        let selected = w.borrow().selected;
        let image = w.borrow().slots[usize::from(selected)].clone();
        let ok_old = image.len() >= old.len() && image[..old.len()] == *old && selected == 0;
        let ok_new = image[..new.len()] == *new && selected == 1;
        assert!(
            ok_old || ok_new,
            "boot {boots_seen} (cut {cut:?}) runs slot {selected}, which holds no whole image"
        );
        let before = (selected, w.borrow().record.clone());
        let Ok(mut b) = boot_of(&w) else { continue };
        let Ok(decision) = b.on_boot() else { continue };
        if confirms && selected == 1 && matches!(decision, Decision::Confirming { .. }) {
            let _ = b.confirm(1);
        }
        let after = (w.borrow().selected, w.borrow().record.clone());
        quiet = if before == after && decision == Decision::Normal {
            quiet + 1
        } else {
            0
        };
    }
    let b = boot_of(&w).expect("no cut is pending any more");
    let selected = w.borrow().selected;
    let trial = b.trial();
    let ops = w.borrow().ops;
    Run {
        selected,
        trial,
        boots_seen,
        ops,
    }
}

#[test]
fn power_cut_at_every_write_of_an_update_that_confirms() {
    let old = image_bytes(Chip::Esp32P4, 9_000, 10);
    let new = image_bytes(Chip::Esp32P4, 12_345, 11);
    let clean = lifecycle(None, true, &old, &new, false);
    assert_eq!((clean.selected, clean.trial), (1, Trial::None));
    assert!(clean.ops >= 5, "{} writes", clean.ops);
    for n in 1..=clean.ops + 1 {
        for how in [Cut::Before, Cut::After] {
            let r = lifecycle(Some((n, how)), true, &old, &new, false);
            // Either the update never took (old image, nothing on trial) or
            // it did and confirmed.
            assert_eq!(r.trial, Trial::None, "cut {n} {how:?}: left a trial running");
            assert!(r.selected <= 1);
            assert!(r.boots_seen <= 20, "cut {n} {how:?}: never settled");
        }
    }
}

#[test]
fn power_cut_at_every_write_of_an_update_that_never_confirms() {
    let old = image_bytes(Chip::Esp32P4, 9_000, 20);
    let new = image_bytes(Chip::Esp32P4, 12_345, 21);
    let clean = lifecycle(None, false, &old, &new, false);
    assert_eq!(
        (clean.selected, clean.trial),
        (0, Trial::None),
        "the old image is back"
    );
    // updates: 4 sectors, 1 record, 1 otadata; boots: 3 counted writes, then a revert (otadata + record)
    for n in 1..=clean.ops + 1 {
        for how in [Cut::Before, Cut::After] {
            let r = lifecycle(Some((n, how)), false, &old, &new, false);
            assert_eq!(
                (r.selected, r.trial),
                (0, Trial::None),
                "cut {n} {how:?}: a broken image must end up replaced by the old one"
            );
        }
    }
}

#[test]
fn an_image_for_another_chip_never_gets_as_far_as_the_bootloader() {
    let old = image_bytes(Chip::Esp32P4, 9_000, 30);
    let new = image_bytes(Chip::Esp32P4, 12_345, 31);
    let r = lifecycle(None, true, &old, &new, true);
    assert_eq!((r.selected, r.trial), (0, Trial::None));
}

#[test]
fn a_cut_during_the_revert_itself_still_ends_on_the_old_image() {
    // Start from a trial that is one boot from reverting.
    for how in [Cut::Before, Cut::After] {
        for n in 1..=3 {
            let w = World::new();
            let mut b = boot_of(&w).unwrap();
            b.start_trial(0, 1).unwrap();
            for _ in 0..MAX_PENDING_BOOTS {
                boot_of(&w).unwrap().on_boot().unwrap();
            }
            let at = w.borrow().ops + n;
            w.borrow_mut().cut_at = Some((at, how));
            let first = boot_of(&w).unwrap().on_boot();
            w.borrow_mut().power_on();
            for _ in 0..4 {
                let _ = boot_of(&w).unwrap().on_boot();
            }
            assert_eq!(
                w.borrow().selected,
                0,
                "cut {n} {how:?} (first boot said {first:?})"
            );
            assert_eq!(boot_of(&w).unwrap().trial(), Trial::None);
        }
    }
}

#[test]
fn otadata_reads_after_the_power_goes_are_errors_not_values() {
    let w = World::new();
    w.borrow_mut().dead = true;
    let mut ota = SimOta {
        w: std::rc::Rc::clone(&w),
    };
    assert_eq!(ota.selected(), Err(Fault::PowerCut));
    assert_eq!(ota.select(1), Err(Fault::PowerCut));
    let mut rec = SimPart::new(&w, Part::Record);
    let mut buf = [0u8; 4];
    assert_eq!(rec.read(0, &mut buf), Err(Fault::PowerCut));
}
