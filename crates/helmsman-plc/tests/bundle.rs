//! Bundles, adversarially: the file format refuses every way a bundle can
//! be malformed or tampered with, each with a precise reason; signatures
//! bind the content and the capability manifest; the capability manifest
//! is held to the compiled modules; and the box API exports snapshots as
//! bundles and imports verified ones only as proposals, which the approval
//! policy then treats like any other. Keys are made from fixed seeds here;
//! none is in the repository.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, compile};
use helmsman_plc::api::{
    Api, Approval, Catalog, Config, Devices, Limits, Live, MemAudit, MemProposals, ModuleInfo, Plant, Shared,
    swap,
};
use helmsman_plc::bundle::sign::{TRUST_FILE, write_private};
use helmsman_plc::bundle::{
    self, Bundle, BundleManifest, Capabilities, Error, MAGIC, MAX_BUNDLE, SigningKey, Trust, TrustStore,
    capabilities, verify,
};
use helmsman_plc::manifest::ModuleDir;
use helmsman_plc::policy::FLOOR;
use helmsman_plc::policy::Role;
use helmsman_plc::policy::pin::{Gate, Pins};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, Code, Meta, ModuleFiles, Store};
use helmsman_plc::{Runtime, SignalTable, Window};
use proptest::prelude::*;
use serde_json::{Value as Json, json};

// ---- fixtures -----------------------------------------------------------------

const CONVEYOR_MANIFEST: &str = "\
name     conveyor
author   Test <test@example.com>
scan_ms  10
reads    start
drives   lamp
param    speed real 1.5 min=0 max=10
";

const CONVEYOR_SOURCE: &str = "
PROGRAM conveyor
VAR_INPUT start : BOOL; speed : REAL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
  lamp := start AND speed > 1.0;
END_PROGRAM";

const HORNER_MANIFEST: &str = "\
name     horner
author   Acme Automation
scan_ms  10
reads    start
drives   horn
";

const HORNER_SOURCE: &str = "
PROGRAM horner
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT horn : BOOL; END_VAR
  horn := start;
END_PROGRAM";

/// The box's own map.
const MAP: &str = "\
in  start bool box di:0
in  level bool box di:1
out lamp  bool box do:0 safe=false
out horn  bool box do:1 safe=false
";

/// What a publisher ships beside `horner`: a template naming the signals
/// it expects and a remote I/O block.
const TEMPLATE: &str = "\
device rio modbus-tcp 192.168.3.50:502 unit=1
in  start bool rio di:1
out horn  bool rio coil:0
";

const ENGINEER: &str = "eng-135797";
const OPERATOR: &str = "op-246810";
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn d() -> Defaults {
    Defaults {
        period: Duration::from_millis(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        m: 64,
    }
}

fn files(m: &str, s: &str) -> ModuleFiles {
    ModuleFiles {
        manifest: m.into(),
        code: s.into(),
    }
}

fn horner_content() -> ChangeSet {
    ChangeSet {
        modules: BTreeMap::from([("horner".to_string(), files(HORNER_MANIFEST, HORNER_SOURCE))]),
        io_map: Some(TEMPLATE.into()),
        calendar: None,
    }
}

fn manifest_for(cs: &ChangeSet) -> BundleManifest {
    BundleManifest {
        name: "horn-kit".into(),
        version: "1.0.0".into(),
        author: "Acme Automation <support@acme.example>".into(),
        description: Some("Sounds the horn while start is held.".into()),
        caps: capabilities(cs, &d()).unwrap(),
    }
}

fn publisher() -> SigningKey {
    SigningKey::from_seed(&[7; 32]).unwrap()
}

fn stranger() -> SigningKey {
    SigningKey::from_seed(&[9; 32]).unwrap()
}

fn trust() -> TrustStore {
    let mut t = TrustStore::default();
    t.add("acme", publisher().public()).unwrap();
    t
}

fn signed(cs: ChangeSet) -> Bundle {
    let mut b = Bundle::new(manifest_for(&cs), cs);
    b.sign(&publisher());
    b
}

fn bytes() -> Vec<u8> {
    signed(horner_content()).encode()
}

fn refused(bytes: &[u8]) -> Error {
    verify(bytes, &trust(), &d()).expect_err("must be refused")
}

/// A raw record.
fn rec(tag: u8, data: &[u8]) -> Vec<u8> {
    let mut v = vec![tag];
    v.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
    v.extend_from_slice(data);
    v
}

/// Where `needle` first occurs.
fn find(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len())
        .position(|w| w == needle)
        .expect("present")
}

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "bundle-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---- the format -----------------------------------------------------------------

#[test]
fn a_signed_bundle_verifies_and_its_id_is_the_snapshot_id_of_its_content() {
    let b = signed(horner_content());
    let v = verify(&b.encode(), &trust(), &d()).unwrap();
    assert_eq!(v.bundle, b);
    assert_eq!(
        v.trust,
        Trust::Publisher {
            name: "acme".into(),
            key: bundle::sign::hex_key(&publisher().public())
        }
    );
    assert_eq!(v.bundle.id(), horner_content().id());
    // The same content committed to a store has the same id.
    let dir = tmp("store");
    let mut s = Store::open(&dir).unwrap();
    let id = s
        .commit(
            &horner_content(),
            &Meta {
                author: "t".into(),
                prompt: String::new(),
                approver: None,
                time_ms: 1,
            },
        )
        .unwrap();
    assert_eq!(id, v.bundle.id());
    assert_eq!(v.caps.drives.iter().collect::<Vec<_>>(), ["horn"]);
    assert_eq!(v.caps.devices.iter().collect::<Vec<_>>(), ["rio"]);
    assert_eq!(v.caps.drivers.iter().collect::<Vec<_>>(), ["modbus-tcp"]);
}

#[test]
fn a_tampered_blob_is_refused_as_tampered() {
    let mut raw = bytes();
    let at = find(&raw, b"horn := start");
    raw[at + 8] = b'S'; // horn := Start
    match refused(&raw) {
        Error::Tampered(why) => assert!(
            why.contains("module horner source") && why.contains("blob"),
            "{why}"
        ),
        e => panic!("{e}"),
    }
}

#[test]
fn a_tampered_tree_is_refused() {
    let raw = bytes();
    // A hash digit in the tree changed: it names a blob nobody carries.
    let mut t = raw.clone();
    let tree_at = MAGIC.len() + 5;
    let line = find(&t, b"module horner ") + b"module horner ".len();
    assert!(line > tree_at);
    t[line] = if t[line] == b'0' { b'1' } else { b'0' };
    match refused(&t) {
        Error::Tampered(why) => assert!(why.contains("the tree names blob"), "{why}"),
        e => panic!("{e}"),
    }
}

#[test]
fn content_swapped_and_readdressed_keeps_addressing_but_fails_the_signature() {
    // An attacker replaces the program and fixes up the tree and blobs:
    // content addressing passes, the signature over the old id does not.
    let good = signed(horner_content());
    let mut evil = horner_content();
    evil.modules.get_mut("horner").unwrap().code =
        HORNER_SOURCE.replace("horn := start", "horn := TRUE").into();
    let forged = Bundle {
        manifest: good.manifest.clone(),
        content: evil,
        signatures: good.signatures.clone(),
    };
    match refused(&forged.encode()) {
        Error::BadSignature(why) => assert!(why.contains("does not verify"), "{why}"),
        e => panic!("{e}"),
    }
}

#[test]
fn a_tampered_manifest_is_refused_by_the_signature() {
    let good = signed(horner_content());
    let mut m = good.manifest.clone();
    m.description = Some("Totally harmless.".into());
    let forged = Bundle { manifest: m, ..good };
    assert!(matches!(refused(&forged.encode()), Error::BadSignature(_)));
}

#[test]
fn a_tampered_signature_is_refused_even_when_dev_bundles_are_allowed() {
    let mut raw = bytes();
    let last = raw.len() - 1;
    raw[last] ^= 0x40;
    assert!(matches!(refused(&raw), Error::BadSignature(_)));
    let mut dev = trust();
    dev.dev_bundles = true;
    assert!(matches!(verify(&raw, &dev, &d()), Err(Error::BadSignature(_))));
}

#[test]
fn unsigned_and_untrusted_bundles_are_refused_unless_dev_bundles_are_allowed_and_then_marked() {
    let cs = horner_content();
    let unsigned = Bundle::new(manifest_for(&cs), cs.clone()).encode();
    assert_eq!(refused(&unsigned), Error::Unsigned);
    let mut other = Bundle::new(manifest_for(&cs), cs);
    other.sign(&stranger());
    let other = other.encode();
    let stranger_hex = bundle::sign::hex_key(&stranger().public());
    assert_eq!(refused(&other), Error::Untrusted(vec![stranger_hex.clone()]));
    assert!(refused(&other).to_string().contains(&stranger_hex));

    let mut dev = trust();
    dev.dev_bundles = true;
    let v = verify(&unsigned, &dev, &d()).unwrap();
    assert_eq!(v.trust, Trust::Dev { signers: vec![] });
    assert!(v.trust.describe().contains("DEV BUNDLE"));
    let v = verify(&other, &dev, &d()).unwrap();
    assert_eq!(
        v.trust,
        Trust::Dev {
            signers: vec![stranger_hex]
        }
    );
}

#[test]
fn a_trusted_signature_beside_an_untrusted_one_is_enough() {
    let mut b = signed(horner_content());
    b.sign(&stranger());
    assert_eq!(b.signatures.len(), 2);
    let v = verify(&b.encode(), &trust(), &d()).unwrap();
    assert!(matches!(v.trust, Trust::Publisher { .. }));
}

#[test]
fn a_capability_manifest_that_understates_what_the_modules_drive_is_refused_though_signed() {
    let cs = horner_content();
    let mut m = manifest_for(&cs);
    m.caps.drives.clear(); // "drives nothing"
    let mut b = Bundle::new(m, cs);
    b.sign(&publisher());
    match refused(&b.encode()) {
        Error::Capabilities(why) => {
            assert_eq!(why.len(), 1, "{why:?}");
            assert!(
                why[0].contains("drives") && why[0].contains("horn") && why[0].contains("understated"),
                "{why:?}"
            );
        }
        e => panic!("{e}"),
    }
}

type Edit = Box<dyn Fn(&mut Capabilities)>;

#[test]
fn every_capability_line_is_held_to_the_content() {
    let cs = horner_content();
    let cases: Vec<(&str, Edit)> = vec![
        (
            "reads",
            Box::new(|c| {
                c.reads.clear();
            }),
        ),
        (
            "reads",
            Box::new(|c| {
                c.reads.insert("level".into());
            }),
        ),
        (
            "drives",
            Box::new(|c| {
                c.drives.insert("lamp".into());
            }),
        ),
        ("q-bytes", Box::new(|c| c.q_bytes = 4)),
        (
            "modules",
            Box::new(|c| {
                c.modules.insert("ghost".into());
            }),
        ),
        (
            "devices",
            Box::new(|c| {
                c.devices.clear();
            }),
        ),
        (
            "drivers",
            Box::new(|c| {
                c.drivers.insert("logix".into());
            }),
        ),
    ];
    for (key, edit) in cases {
        let mut m = manifest_for(&cs);
        edit(&mut m.caps);
        let mut b = Bundle::new(m, cs.clone());
        b.sign(&publisher());
        match refused(&b.encode()) {
            Error::Capabilities(why) => assert!(why.iter().all(|w| w.starts_with(key)), "{key}: {why:?}"),
            e => panic!("{key}: {e}"),
        }
    }
}

#[test]
fn q_bytes_and_calendar_drives_count_as_driving() {
    let manifest = "\
name     qmod
author   T
scan_ms  10
q        0+2
reads
drives
";
    let source = "PROGRAM qmod\nVAR x AT %QB0 : BYTE; END_VAR\n  x := 1;\nEND_PROGRAM";
    let cs = ChangeSet {
        modules: BTreeMap::from([("qmod".to_string(), files(manifest, source))]),
        io_map: None,
        calendar: Some("zone UTC\nshift 1 daily 06:00-14:00\ndrive beacon working\n".into()),
    };
    let caps = capabilities(&cs, &d()).unwrap();
    assert_eq!(caps.q_bytes, 2);
    assert_eq!(caps.drives.iter().collect::<Vec<_>>(), ["beacon"]);
    assert!(caps.drives_anything());
    let mut m = manifest_for(&cs);
    m.caps.q_bytes = 0;
    m.caps.drives.clear();
    let mut b = Bundle::new(m, cs);
    b.sign(&publisher());
    match refused(&b.encode()) {
        Error::Capabilities(why) => {
            assert!(
                why.iter()
                    .any(|w| w.contains("q-bytes") && w.contains("understated")),
                "{why:?}"
            );
            assert!(why.iter().any(|w| w.contains("beacon")), "{why:?}");
        }
        e => panic!("{e}"),
    }
}

#[test]
fn a_module_whose_own_manifest_hides_an_output_is_refused_as_content() {
    // The module manifest says it drives only horn; the program also
    // drives lamp. The bundle manifest agrees with the module manifest,
    // and both are signed: the compiled program is what decides.
    let mut cs = horner_content();
    cs.modules.get_mut("horner").unwrap().code = "
PROGRAM horner
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT horn : BOOL; lamp : BOOL; END_VAR
  horn := start; lamp := start;
END_PROGRAM"
        .into();
    let mut b = Bundle::new(manifest_for(&horner_content()), cs);
    b.sign(&publisher());
    match refused(&b.encode()) {
        Error::Content(why) => assert!(why.iter().any(|w| w.to_lowercase().contains("lamp")), "{why:?}"),
        e => panic!("{e}"),
    }
}

#[test]
fn a_module_that_does_not_compile_is_refused_as_content() {
    let mut cs = horner_content();
    cs.modules.get_mut("horner").unwrap().code = "PROGRAM horner\n horn := ;\nEND_PROGRAM".into();
    let b = Bundle {
        manifest: manifest_for(&horner_content()),
        content: cs,
        signatures: Vec::new(),
    };
    let mut dev = trust();
    dev.dev_bundles = true;
    assert!(matches!(verify(&b.encode(), &dev, &d()), Err(Error::Content(_))));
}

#[test]
fn size_claims_are_refused_before_anything_is_allocated() {
    // A tree record claiming 4 GiB in a 30-byte file.
    let mut raw = MAGIC.to_vec();
    raw.extend_from_slice(&[b'T', 0xff, 0xff, 0xff, 0xff]);
    raw.extend_from_slice(b"helmsman-snapshot 1\n");
    match Bundle::decode(&raw).unwrap_err() {
        Error::Malformed(why) => assert!(
            why.contains("claims 4294967295 bytes, more than a tree may hold"),
            "{why}"
        ),
        e => panic!("{e}"),
    }
    // A blob claiming just over the per-file cap.
    let good = bytes();
    let b = signed(horner_content());
    let blob_at = MAGIC.len() + 5 + b.content.tree().len() + 5 + b.manifest.render().len();
    assert_eq!(good[blob_at], b'B');
    let mut big = good.clone();
    big[blob_at + 1..blob_at + 5].copy_from_slice(&((1u32 << 20) + 1).to_be_bytes());
    match Bundle::decode(&big).unwrap_err() {
        Error::Malformed(why) => assert!(why.contains("more than a blob may hold"), "{why}"),
        e => panic!("{e}"),
    }
    // A claim within the cap but past the end of the file.
    let mut long = good.clone();
    long[blob_at + 1..blob_at + 5].copy_from_slice(&60_000u32.to_be_bytes());
    match Bundle::decode(&long).unwrap_err() {
        Error::Malformed(why) => assert!(why.contains("truncated") && why.contains("only"), "{why}"),
        e => panic!("{e}"),
    }
    // The whole file over the cap.
    let mut huge = good;
    huge.resize(MAX_BUNDLE + 1, 0);
    match Bundle::decode(&huge).unwrap_err() {
        Error::Malformed(why) => assert!(why.contains("more than a bundle may be"), "{why}"),
        e => panic!("{e}"),
    }
}

#[test]
fn every_truncation_is_refused() {
    let good = bytes();
    for n in 0..good.len() {
        assert!(
            verify(&good[..n], &trust(), &d()).is_err(),
            "a {n}-byte prefix was taken"
        );
    }
    match Bundle::decode(&good[..good.len() - 1]).unwrap_err() {
        Error::Malformed(why) => assert!(why.contains("truncated"), "{why}"),
        e => panic!("{e}"),
    }
}

#[test]
fn trailing_bytes_unknown_records_and_other_versions_are_refused() {
    let mut extra = bytes();
    extra.push(0);
    assert!(
        matches!(Bundle::decode(&extra), Err(Error::Malformed(w)) if w.contains("unexpected record type 0x00"))
    );
    let mut x = bytes();
    x.extend_from_slice(&rec(b'X', b"hi"));
    assert!(matches!(Bundle::decode(&x), Err(Error::Malformed(w)) if w.contains("0x58")));
    let mut v2 = bytes();
    v2[16] = b'2';
    assert!(matches!(Bundle::decode(&v2), Err(Error::Malformed(w)) if w.contains("version")));
    assert!(
        matches!(Bundle::decode(b"PK\x03\x04"), Err(Error::Malformed(w)) if w.contains("not a Helmsman bundle"))
    );
    assert!(matches!(Bundle::decode(b""), Err(Error::Malformed(_))));
}

/// A bundle from raw parts: the tree text, the manifest text, blobs and
/// signature records, in the order given.
fn raw(tree: &str, manifest: &str, blobs: &[&[u8]], sigs: &[[u8; 96]]) -> Vec<u8> {
    let mut v = MAGIC.to_vec();
    v.extend(rec(b'T', tree.as_bytes()));
    v.extend(rec(b'M', manifest.as_bytes()));
    for b in blobs {
        v.extend(rec(b'B', b));
    }
    for s in sigs {
        v.extend(rec(b'S', s));
    }
    v
}

fn sha(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    bundle::sign::hex_key(&Sha256::digest(b).into())
}

fn sorted_blobs<'a>(blobs: &[&'a [u8]]) -> Vec<&'a [u8]> {
    let mut v = blobs.to_vec();
    v.sort_by_key(|b| sha(b));
    v
}

#[test]
fn duplicate_entries_are_refused() {
    let b = signed(horner_content());
    let manifest = b.manifest.render();
    let tree = b.content.tree();
    let blobs = sorted_blobs(&[
        HORNER_MANIFEST.as_bytes(),
        HORNER_SOURCE.as_bytes(),
        TEMPLATE.as_bytes(),
    ]);
    // As encoded: fine.
    let mut sig = [0u8; 96];
    sig[..32].copy_from_slice(&b.signatures[0].key);
    sig[32..].copy_from_slice(&b.signatures[0].sig);
    assert_eq!(raw(&tree, &manifest, &blobs, &[sig]), b.encode());
    // A blob twice.
    let twice = [blobs[0], blobs[0], blobs[1], blobs[2]];
    assert!(
        matches!(Bundle::decode(&raw(&tree, &manifest, &twice, &[sig])), Err(Error::Malformed(w)) if w.contains("appears twice"))
    );
    // Blobs out of order.
    let rev = [blobs[2], blobs[1], blobs[0]];
    assert!(
        matches!(Bundle::decode(&raw(&tree, &manifest, &rev, &[sig])), Err(Error::Malformed(w)) if w.contains("ascending"))
    );
    // A signature twice.
    assert!(
        matches!(Bundle::decode(&raw(&tree, &manifest, &blobs, &[sig, sig])), Err(Error::Malformed(w)) if w.contains("two signatures"))
    );
    // A module twice in the tree.
    let line = tree.lines().find(|l| l.starts_with("module")).unwrap();
    let doubled = tree.replace(line, &format!("{line}\n{line}"));
    assert!(
        matches!(Bundle::decode(&raw(&doubled, &manifest, &blobs, &[sig])), Err(Error::Malformed(w)) if w.contains("named twice"))
    );
    // Two tree records.
    let mut two = MAGIC.to_vec();
    two.extend(rec(b'T', tree.as_bytes()));
    two.extend(rec(b'T', tree.as_bytes()));
    assert!(matches!(Bundle::decode(&two), Err(Error::Malformed(w)) if w.contains("expected the manifest")));
    // An extra blob nobody names.
    let extra = sorted_blobs(&[
        HORNER_MANIFEST.as_bytes(),
        HORNER_SOURCE.as_bytes(),
        TEMPLATE.as_bytes(),
        b"stowaway",
    ]);
    assert!(
        matches!(Bundle::decode(&raw(&tree, &manifest, &extra, &[sig])), Err(Error::Tampered(w)) if w.contains("not named by the tree"))
    );
}

#[test]
fn path_like_and_lookalike_module_names_are_refused() {
    let m = HORNER_MANIFEST.as_bytes();
    let s = HORNER_SOURCE.as_bytes();
    let blobs = sorted_blobs(&[m, s]);
    let manifest = manifest_for(&horner_content()).render();
    for name in [
        "../horner",
        "/etc/horner",
        "a/b",
        "..",
        "horner\\x",
        "h\u{043E}rner",  // Cyrillic o
        "horn\u{200B}er", // zero-width space
        "horner\u{202E}", // bidi override
        "HORNER%2F..",
        "",
        &"x".repeat(65),
    ] {
        let tree = format!("helmsman-snapshot 1\nmodule {name} {} {}\n", sha(m), sha(s));
        match Bundle::decode(&raw(&tree, &manifest, &blobs, &[])) {
            Err(Error::Malformed(w)) => assert!(
                w.contains("not a module name") || w.contains("unknown line"),
                "{name:?}: {w}"
            ),
            other => panic!("{name:?}: {other:?}"),
        }
    }
}

#[test]
fn manifests_must_be_canonical_and_clean() {
    let good = manifest_for(&horner_content()).render();
    assert_eq!(BundleManifest::parse(&good).unwrap().render(), good);
    for (bad, want) in [
        (good.trim_end().to_string(), "canonical"),
        (good.replace("name horn-kit", "name ../kit"), "not a name"),
        (good.replace("version 1.0.0", "version 1.0 beta"), "not a version"),
        (
            good.replace("Acme Automation", "Acme \u{202E}noitamotuA"),
            "invisible",
        ),
        (good.replace("drives horn", "drives Horn"), "lowercase"),
        (good.replace("drives horn", "drives horn horn"), "canonical"),
        (good.replace("q-bytes 0", "q-bytes 00"), "whole number"),
        (good.replace("q-bytes 0\n", ""), "q-bytes is missing"),
        (format!("{good}name again\n"), "twice"),
        (format!("{good}# a comment\n"), "unknown key"),
        (
            good.replace("helmsman-bundle-manifest 1", "helmsman-bundle-manifest 2"),
            "first line",
        ),
    ] {
        let errs = BundleManifest::parse(&bad).unwrap_err();
        assert!(errs.iter().any(|e| e.contains(want)), "{want}: {errs:?}");
    }
}

#[test]
fn the_trust_store_is_strict_and_private() {
    let t = trust();
    let text = t.render();
    assert_eq!(TrustStore::parse(&text).unwrap(), t);
    for bad in [
        "helmsman-trust 2\n",
        "helmsman-trust 1\npublisher acme ed25519 xyz\n",
        "helmsman-trust 1\npublisher ../x ed25519 0000000000000000000000000000000000000000000000000000000000000000\n",
        "helmsman-trust 1\ndev-bundles maybe\n",
        "helmsman-trust 1\ndev-bundles allow\ndev-bundles refuse\n",
        "helmsman-trust 1\ntrust-everyone\n",
    ] {
        assert!(TrustStore::parse(bad).is_err(), "{bad}");
    }
    let key = bundle::sign::hex_key(&publisher().public());
    let dup = format!("helmsman-trust 1\npublisher a ed25519 {key}\npublisher b ed25519 {key}\n");
    assert!(TrustStore::parse(&dup).is_err());
    // Missing file: trusts nobody, no dev bundles.
    let dir = tmp("trust");
    assert_eq!(TrustStore::load(&dir).unwrap(), TrustStore::default());
    t.save(&dir).unwrap();
    assert_eq!(TrustStore::load(&dir).unwrap(), t);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(TRUST_FILE);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let e = TrustStore::load(&dir).unwrap_err();
        assert!(e.contains("readable by others"), "{e}");
    }
}

#[test]
fn every_single_byte_change_to_a_signed_bundle_is_detected() {
    let good = bytes();
    for at in 0..good.len() {
        for flip in [0x01u8, 0x80] {
            let mut b = good.clone();
            b[at] ^= flip;
            assert!(
                verify(&b, &trust(), &d()).is_err(),
                "byte {at} ^ {flip:#x} went unnoticed"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn any_single_byte_mutation_of_a_signed_bundle_is_detected(at in any::<prop::sample::Index>(), x in 1u8..=255) {
        let good = bytes();
        let mut b = good.clone();
        let i = at.index(b.len());
        b[i] ^= x;
        prop_assert!(verify(&b, &trust(), &d()).is_err(), "byte {} ^ {:#x}", i, x);
    }
}

fn arb_text() -> impl Strategy<Value = String> {
    prop_oneof![".{0,200}", "[ -~\n]{0,400}"]
}

prop_compose! {
    fn arb_change_set()(
        modules in prop::collection::btree_map("[A-Za-z0-9_-]{1,64}", (arb_text(), arb_text()), 0..6),
        io_map in prop::option::of(arb_text()),
        calendar in prop::option::of(arb_text()),
    ) -> ChangeSet {
        ChangeSet {
            modules: modules.into_iter().map(|(n, (m, s))| (n, ModuleFiles::st(m, s))).collect(),
            io_map,
            calendar,
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// The format carries any snapshot's content, byte for byte, under the
    /// same id (whatever the files hold; the content checks are separate).
    #[test]
    fn export_then_import_round_trips_any_snapshot(cs in arb_change_set(), signed_by_publisher in any::<bool>()) {
        let m = BundleManifest {
            name: "any".into(),
            version: "0.1.0".into(),
            author: "prop".into(),
            description: None,
            caps: Capabilities::default(),
        };
        let mut b = Bundle::new(m, cs.clone());
        if signed_by_publisher {
            b.sign(&publisher());
        }
        let back = Bundle::decode(&b.encode()).unwrap();
        prop_assert_eq!(back.id(), cs.id());
        prop_assert_eq!(&back.content, &cs);
        prop_assert_eq!(&back, &b);
        prop_assert!(bundle::check_signatures(&back).is_ok());
    }
}

// ---- the box API ----------------------------------------------------------------

fn running() -> ChangeSet {
    ChangeSet {
        modules: BTreeMap::from([("conveyor".to_string(), files(CONVEYOR_MANIFEST, CONVEYOR_SOURCE))]),
        io_map: Some(MAP.into()),
        calendar: None,
    }
}

struct Bench {
    _api: Api,
    addr: SocketAddr,
    store: PathBuf,
}

fn bench(trust: Option<TrustStore>) -> Bench {
    let dd = d();
    let md = ModuleDir::from_text(
        Path::new("conveyor"),
        CONVEYOR_MANIFEST,
        CONVEYOR_SOURCE.into(),
        &dd,
    )
    .unwrap();
    let sizes = ImageSizes {
        i: md.spec.i.len,
        q: md.spec.q.len,
        m: md.spec.m,
    };
    let module = md.module(compile(CONVEYOR_SOURCE, sizes).unwrap()).unwrap();
    let parsed = helmsman_io::text::parse(MAP).unwrap();
    let table = SignalTable::new(&parsed.map, &parsed.safe).unwrap();
    let rt = Runtime::with_signals(32, 32, vec![module], &table).unwrap();
    let modules = vec![
        ModuleInfo::of(
            &rt,
            "conveyor",
            "conveyor".into(),
            CONVEYOR_SOURCE.into(),
            Some(md.manifest.clone()),
        )
        .unwrap(),
    ];
    let catalog = Catalog {
        signals: Catalog::signals(&table, Some(&parsed.map), &modules),
        modules,
    };
    let store = tmp("api-store");
    Store::open(&store)
        .unwrap()
        .commit(
            &running(),
            &Meta {
                author: "Test".into(),
                prompt: String::new(),
                approver: None,
                time_ms: 1,
            },
        )
        .unwrap();
    let mut pins = Pins::default();
    pins.set(Role::Operator, OPERATOR, 1, [1; 16]).unwrap();
    pins.set(Role::Engineer, ENGINEER, 1, [2; 16]).unwrap();
    let (handle, _swapper) = swap::channel(Duration::from_secs(1));
    let approval = Approval {
        policy: FLOOR,
        gate: Gate::new(pins, FLOOR.lockout, Arc::new(Instant::now)),
        plant: Plant {
            table: table.clone(),
            i_len: 32,
            q_len: 32,
            defaults: dd,
            record: None,
        },
        swap: handle,
    };
    let cfg = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        token: TOKEN.into(),
        limits: Limits::default(),
        stream_every: Duration::from_millis(20),
        memory: 64,
        log: Arc::new(|_: &str| {}),
    };
    let shared = Shared {
        catalog,
        live: Arc::new(Live::default()),
        devices: Arc::new(Devices::new(["box".to_string()])),
        audit: Box::new(MemAudit::new(10_000)),
        proposals: Box::new(MemProposals::new(100)),
        running: Some(running()),
        snapshots: Some(Mutex::new(Store::open(&store).unwrap())),
        approval: Some(approval),
        chat: None,
    };
    let api = Api::start_with_trust(cfg, shared, None, trust).unwrap();
    Bench {
        addr: api.addr(),
        _api: api,
        store,
    }
}

#[derive(Debug)]
struct Resp {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Json {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {self:?}"))
    }
}

fn request(addr: SocketAddr, method: &str, path: &str, auth: bool, ctype: &str, body: &[u8]) -> Resp {
    let auth = if auth {
        format!("Authorization: Bearer {TOKEN}\r\n")
    } else {
        String::new()
    };
    let mut raw = format!(
        "{method} {path} HTTP/1.1\r\nHost: box\r\n{auth}Content-Type: {ctype}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(&raw).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let split = find(&out, b"\r\n\r\n");
    let head = String::from_utf8_lossy(&out[..split]).into_owned();
    Resp {
        status: head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0),
        head,
        body: out[split + 4..].to_vec(),
    }
}

fn import(b: &Bench, bytes: &[u8]) -> Resp {
    request(
        b.addr,
        "POST",
        "/api/bundles",
        true,
        "application/octet-stream",
        bytes,
    )
}

fn audit(b: &Bench) -> Vec<Json> {
    request(
        b.addr,
        "GET",
        "/api/audit?limit=1000",
        false,
        "application/json",
        b"",
    )
    .json()
    .as_array()
    .cloned()
    .unwrap()
}

fn head(b: &Bench) -> String {
    Store::open(&b.store)
        .unwrap()
        .head()
        .unwrap()
        .unwrap()
        .id
        .to_string()
}

#[test]
fn the_api_exports_a_snapshot_as_an_unsigned_bundle_under_the_same_id() {
    let b = bench(Some(trust()));
    let id = running().id().to_string();
    let r = request(
        b.addr,
        "GET",
        &format!("/api/snapshots/{id}/bundle"),
        false,
        "application/json",
        b"",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert!(
        r.head.contains("Content-Type: application/octet-stream"),
        "{}",
        r.head
    );
    assert!(r.head.contains(&format!("X-Bundle-Id: {id}")), "{}", r.head);
    let bundle = Bundle::decode(&r.body).unwrap();
    assert_eq!(bundle.id().to_string(), id);
    assert_eq!(bundle.content, running());
    assert_eq!(bundle.signatures.len(), 0);
    assert_eq!(bundle.manifest.caps.drives.iter().collect::<Vec<_>>(), ["lamp"]);
    // The capability manifest it carries is exactly the content's.
    assert!(bundle::check_content(&bundle, &d()).is_ok());
    let missing = request(
        b.addr,
        "GET",
        &format!("/api/snapshots/{}/bundle", "0".repeat(64)),
        false,
        "application/json",
        b"",
    );
    assert_eq!(missing.status, 404);
}

#[test]
fn an_exported_bundle_signed_off_box_imports_as_a_proposal_that_the_policy_gates() {
    let b = bench(Some(trust()));
    let before = head(&b);
    let r = import(&b, &bytes());
    assert_eq!(r.status, 201, "{r:?}");
    let p = r.json();
    assert_eq!(p["kind"], "bundle");
    assert_eq!(p["status"], "draft");
    assert_eq!(p["bundle"]["trust"], "publisher");
    assert_eq!(p["bundle"]["dev"], false);
    assert_eq!(p["bundle"]["publisher"]["name"], "acme");
    assert_eq!(p["bundle"]["capabilities"]["drives"], json!(["horn"]));
    assert_eq!(p["touches_outputs"], true);
    assert_eq!(p["bindings"]["unbound"], json!([]));
    assert_eq!(p["bindings"]["bound"].as_array().unwrap().len(), 2, "{p}");
    assert!(
        p["bindings"]["template"]["io_map"]
            .as_str()
            .unwrap()
            .contains("unchanged")
    );
    assert_eq!(p["base"].as_str().unwrap(), before);
    // Adds a module that drives an output: output-affecting.
    assert_eq!(p["approval"]["tier"], "output-affecting", "{p}");
    assert_eq!(p["approval"]["needs"]["role"], "engineer");
    // The proposal adds horner beside conveyor; the box's map is kept.
    let mut want = running();
    want.modules
        .insert("horner".into(), files(HORNER_MANIFEST, HORNER_SOURCE));
    assert_eq!(p["snapshot"].as_str().unwrap(), want.id().to_string());
    let id = p["id"].as_u64().unwrap();
    // Nothing ran: head is what it was.
    assert_eq!(head(&b), before);
    // The approval path takes it like any other proposal.
    let apply = |role: &str, pin: &str| {
        request(
            b.addr,
            "POST",
            &format!("/api/proposals/{id}/apply"),
            true,
            "application/json",
            json!({ "role": role, "pin": pin }).to_string().as_bytes(),
        )
    };
    assert_eq!(apply("operator", OPERATOR).status, 403);
    let r = apply("engineer", ENGINEER);
    assert_eq!(r.status, 428, "a sim replay is required: {r:?}");
    assert_eq!(head(&b), before);
    let log = audit(&b);
    let imported = log.iter().find(|e| e["path"] == "/api/bundles").unwrap();
    assert_eq!(imported["status"], 201);
    assert!(
        imported["detail"].as_str().unwrap().contains("signed by acme"),
        "{imported}"
    );
    assert_eq!(imported["snapshot"].as_str().unwrap(), want.id().to_string());
}

#[test]
fn import_refuses_tampered_untrusted_and_unsigned_bundles_and_audits_each() {
    let b = bench(Some(trust()));
    let mut tampered = bytes();
    let at = find(&tampered, b"horn := start");
    tampered[at] = b'H';
    let r = import(&b, &tampered);
    assert_eq!(r.status, 422, "{r:?}");
    assert_eq!(r.json()["refused"], "tampered");

    let cs = horner_content();
    let mut other = Bundle::new(manifest_for(&cs), cs.clone());
    other.sign(&stranger());
    let r = import(&b, &other.encode());
    assert_eq!(r.status, 403);
    assert_eq!(r.json()["refused"], "untrusted");

    let r = import(&b, &Bundle::new(manifest_for(&cs), cs).encode());
    assert_eq!(r.status, 403);
    assert_eq!(r.json()["refused"], "unsigned");

    let r = import(&b, b"definitely not a bundle");
    assert_eq!(r.status, 422);
    assert_eq!(r.json()["refused"], "malformed");

    let r = request(b.addr, "POST", "/api/bundles", true, "application/json", &bytes());
    assert_eq!(r.status, 415);
    let r = request(
        b.addr,
        "POST",
        "/api/bundles",
        false,
        "application/octet-stream",
        &bytes(),
    );
    assert_eq!(r.status, 401);
    let r = request(b.addr, "GET", "/api/bundles", false, "application/json", b"");
    assert_eq!(r.status, 404);
    let r = request(
        b.addr,
        "PUT",
        "/api/bundles",
        true,
        "application/octet-stream",
        &bytes(),
    );
    assert_eq!(r.status, 405);

    let log = audit(&b);
    let statuses: Vec<u64> = log
        .iter()
        .filter(|e| e["path"] == "/api/bundles")
        .map(|e| e["status"].as_u64().unwrap())
        .collect();
    assert_eq!(statuses, [422, 403, 403, 422, 415, 405], "{log:?}");
    let list = request(b.addr, "GET", "/api/proposals", false, "application/json", b"").json();
    assert_eq!(list, json!([]));
}

#[test]
fn dev_bundles_import_only_when_the_trust_store_allows_them_and_are_marked() {
    let mut t = trust();
    t.dev_bundles = true;
    let b = bench(Some(t));
    let cs = horner_content();
    let r = import(&b, &Bundle::new(manifest_for(&cs), cs).encode());
    assert_eq!(r.status, 201, "{r:?}");
    let p = r.json();
    assert_eq!(p["bundle"]["trust"], "dev");
    assert_eq!(p["bundle"]["dev"], true);
    assert!(p["note"].as_str().unwrap().contains("DEV BUNDLE"), "{p}");
    assert!(
        audit(&b)
            .iter()
            .any(|e| e["detail"].as_str().unwrap().contains("DEV BUNDLE"))
    );
}

#[test]
fn signals_that_do_not_bind_are_reported_and_the_bundle_refused() {
    let b = bench(Some(trust()));
    // Reads a signal the box lacks, drives one another module drives, and
    // its template disagrees on a type.
    let manifest = "\
name     odd
author   Acme
scan_ms  10
reads    pressure level
drives   lamp
";
    let source = "
PROGRAM odd
VAR_INPUT pressure : REAL; level : REAL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
  lamp := pressure > level;
END_PROGRAM";
    let template = "\
in  pressure real box di:5
in  level    real box di:6
out lamp     bool box do:0
";
    let cs = ChangeSet {
        modules: BTreeMap::from([("odd".to_string(), files(manifest, source))]),
        io_map: Some(template.into()),
        calendar: None,
    };
    let r = import(&b, &signed(cs).encode());
    assert_eq!(r.status, 422, "{r:?}");
    let j = r.json();
    let unbound = j["bindings"]["unbound"].as_array().unwrap();
    let why = |s: &str| {
        unbound
            .iter()
            .find(|u| u["signal"] == s)
            .unwrap_or_else(|| panic!("{s}: {j}"))["why"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(why("pressure").contains("no signal pressure"), "{j}");
    assert!(
        why("level").contains("the template says real") && why("level").contains("bool"),
        "{j}"
    );
    assert!(why("lamp").contains("already driven by conveyor"), "{j}");
    assert_eq!(j["bundle"]["name"], "horn-kit");
    let list = request(b.addr, "GET", "/api/proposals", false, "application/json", b"").json();
    assert_eq!(list, json!([]));
}

#[test]
fn a_box_without_a_trust_store_imports_nothing() {
    let b = bench(None);
    let r = import(&b, &bytes());
    assert_eq!(r.status, 501, "{r:?}");
}

// ---- the publisher's CLI ----------------------------------------------------------

fn plc(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .arg("bundle")
        .args(args)
        .env_remove("STATE_DIRECTORY")
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
#[allow(clippy::too_many_lines)]
fn the_cli_makes_keys_packs_signs_verifies_and_keeps_the_trust_store() {
    let dir = tmp("cli");
    let p = |n: &str| dir.join(n).to_str().unwrap().to_string();
    let module = dir.join("horner");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("module.manifest"), HORNER_MANIFEST).unwrap();
    std::fs::write(module.join("module.st"), HORNER_SOURCE).unwrap();
    std::fs::write(dir.join("template.map"), TEMPLATE).unwrap();

    let (ok, out, err) = plc(&["keygen", &p("acme.key"), "--name", "acme"]);
    assert!(ok, "{err}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(p("acme.key")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let key = out.split_whitespace().last().unwrap().to_string();
    assert_eq!(key.len(), 64, "{out}");
    let (ok, _, err) = plc(&["keygen", &p("acme.key")]);
    assert!(!ok && err.contains("never overwrites"), "{err}");

    let (ok, out, err) = plc(&[
        "pack",
        "--module-dir",
        &p("horner"),
        "--io-map",
        &p("template.map"),
        "--name",
        "horn-kit",
        "--version",
        "1.0.0",
        "--author",
        "Acme",
        &p("kit.hbundle"),
    ]);
    assert!(ok, "{err}");
    assert!(out.contains("drives   horn"), "{out}");
    let (ok, out, err) = plc(&["verify", &p("kit.hbundle")]);
    assert!(ok && out.contains("unsigned"), "{out}{err}");

    let (ok, _, err) = plc(&[
        "sign",
        "--key",
        &p("acme.key"),
        "--version",
        "1.0.1",
        &p("kit.hbundle"),
        &p("signed.hbundle"),
    ]);
    assert!(ok, "{err}");
    // Once signed, the manifest cannot be rewritten without voiding it.
    let (ok, _, err) = plc(&[
        "sign",
        "--key",
        &p("acme.key"),
        "--version",
        "2.0.0",
        &p("signed.hbundle"),
        &p("x.hbundle"),
    ]);
    assert!(!ok && err.contains("void"), "{err}");

    // The integrator trusts the key on a box.
    let state = p("state");
    let (ok, _, err) = plc(&["trust", "--state-dir", &state, "add", "acme", &key]);
    assert!(ok, "{err}");
    let (ok, out, _) = plc(&["trust", "--state-dir", &state, "list"]);
    assert!(
        ok && out.contains(&format!("publisher acme ed25519 {key}")) && out.contains("dev-bundles refuse"),
        "{out}"
    );
    let trust_file = dir.join("state").join(TRUST_FILE);
    let (ok, out, err) = plc(&[
        "verify",
        "--trust",
        trust_file.to_str().unwrap(),
        &p("signed.hbundle"),
    ]);
    assert!(ok, "{err}");
    assert!(
        out.contains("horn-kit 1.0.1") && out.contains("signed by acme"),
        "{out}"
    );

    // One flipped byte and it is refused, with the reason, non-zero.
    let mut raw = std::fs::read(p("signed.hbundle")).unwrap();
    let at = find(&raw, b"version 1.0.1") + 8;
    raw[at] = b'9';
    std::fs::write(p("evil.hbundle"), raw).unwrap();
    let (ok, _, err) = plc(&[
        "verify",
        "--trust",
        trust_file.to_str().unwrap(),
        &p("evil.hbundle"),
    ]);
    assert!(!ok && err.contains("does not verify"), "{err}");

    // A key file others can read is refused.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let der = std::fs::read(p("acme.key")).unwrap();
        write_private(&dir.join("loose.key"), &der, true).unwrap();
        std::fs::set_permissions(dir.join("loose.key"), std::fs::Permissions::from_mode(0o644)).unwrap();
        let (ok, _, err) = plc(&[
            "sign",
            "--key",
            &p("loose.key"),
            &p("kit.hbundle"),
            &p("y.hbundle"),
        ]);
        assert!(!ok && err.contains("readable by others"), "{err}");
    }

    let (ok, _, err) = plc(&["frobnicate"]);
    assert!(!ok && err.contains("unknown bundle command"), "{err}");
}

#[test]
fn a_box_refuses_to_start_with_a_trust_store_others_can_read() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("box-trust");
        trust().save(&dir).unwrap();
        std::fs::set_permissions(dir.join(TRUST_FILE), std::fs::Permissions::from_mode(0o644)).unwrap();
        let prog = dir.join("p.st");
        std::fs::write(&prog, "PROGRAM p\nVAR x : INT; END_VAR\n x := 1;\nEND_PROGRAM").unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
            .arg(&prog)
            .args([
                "--state-dir",
                dir.to_str().unwrap(),
                "--encap-port",
                "0",
                "--io-port",
                "0",
                "--opcua",
                "off",
            ])
            .output()
            .unwrap();
        assert!(!out.status.success());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("trusted-publishers") && err.contains("readable by others"),
            "{err}"
        );
    }
}

// ---- WASM modules -----------------------------------------------------------------

/// `horner` as WASM: `horn := start`, and `lamp := start` too when
/// `lamp`. A custom section of every byte value makes it no UTF-8 at all.
fn horner_wasm(lamp: bool) -> Vec<u8> {
    let (import, call) = if lamp {
        (
            r#"(import "helmsman/v1/out" "lamp" (func $lamp (param i32)))"#,
            "(call $lamp (call $start))",
        )
    } else {
        ("", "")
    };
    let mut b = wat::parse_str(format!(
        r#"(module
             (import "helmsman/v1/in" "start" (func $start (result i32)))
             (import "helmsman/v1/out" "horn" (func $horn (param i32)))
             {import}
             (func (export "scan") (call $horn (call $start)) {call}))"#
    ))
    .unwrap();
    // Custom section "x": id 0, size 258 (LEB128), name, 256 bytes.
    b.extend_from_slice(&[0, 0x82, 0x02, 1, b'x']);
    b.extend(0..=255u8);
    assert!(std::str::from_utf8(&b).is_err());
    b
}

fn horner_wasm_content() -> ChangeSet {
    ChangeSet {
        modules: BTreeMap::from([(
            "horner".to_string(),
            ModuleFiles::wasm(HORNER_MANIFEST, horner_wasm(false)),
        )]),
        io_map: Some(TEMPLATE.into()),
        calendar: None,
    }
}

#[test]
fn a_wasm_module_bundles_verifies_and_round_trips_byte_identical() {
    let cs = horner_wasm_content();
    let b = signed(cs.clone());
    assert_eq!(b.manifest.caps.drives.iter().collect::<Vec<_>>(), ["horn"]);
    let file = b.encode();
    let v = verify(&file, &trust(), &d()).unwrap();
    assert_eq!(v.bundle.content, cs);
    assert_eq!(
        v.bundle.content.modules["horner"].code.wasm(),
        Some(horner_wasm(false).as_slice())
    );
    assert_eq!(v.bundle.id(), cs.id());
    // Through a store and out again: the same file, byte for byte.
    let dir = tmp("wasm-bundle-store");
    let mut s = Store::open(&dir).unwrap();
    let meta = Meta {
        author: "Test".into(),
        prompt: String::new(),
        approver: None,
        time_ms: 1,
    };
    let id = s.commit(&v.bundle.content, &meta).unwrap();
    let mut again = Bundle::new(b.manifest.clone(), s.get(&id).unwrap());
    again.signatures.clone_from(&b.signatures);
    assert_eq!(again.encode(), file);
}

#[test]
fn a_tampered_wasm_blob_is_refused_as_tampered() {
    let file = signed(horner_wasm_content()).encode();
    let wasm = horner_wasm(false);
    let at = find(&file, &wasm);
    for k in [at, at + 8, at + wasm.len() - 1] {
        let mut evil = file.clone();
        evil[k] ^= 0x40;
        assert!(matches!(refused(&evil), Error::Tampered(_)), "byte {k}");
    }
}

#[test]
fn a_wasm_blob_the_tree_calls_text_is_refused_as_not_utf8() {
    let cs = horner_wasm_content();
    let manifest = manifest_for(&cs).render();
    let tree = cs.tree().replace("wasm-module horner", "module horner");
    let wasm = horner_wasm(false);
    let blobs = sorted_blobs(&[HORNER_MANIFEST.as_bytes(), &wasm, TEMPLATE.as_bytes()]);
    let e = refused(&raw(&tree, &manifest, &blobs, &[]));
    assert!(
        matches!(&e, Error::Malformed(w) if w.contains("module horner source is not UTF-8")),
        "{e}"
    );
    // A manifest pointing at the binary: the same.
    let (m, w) = (sha(HORNER_MANIFEST.as_bytes()), sha(&wasm));
    let tree = cs.tree().replace(&format!("{m} {w}"), &format!("{w} {w}"));
    let blobs = sorted_blobs(&[&wasm, TEMPLATE.as_bytes()]);
    let e = refused(&raw(&tree, &manifest, &blobs, &[]));
    assert!(
        matches!(&e, Error::Malformed(w) if w.contains("manifest is not UTF-8")),
        "{e}"
    );
    // An unknown program kind is not guessed at.
    let tree = cs.tree().replace("wasm-module horner", "elf-module horner");
    let blobs = sorted_blobs(&[HORNER_MANIFEST.as_bytes(), &wasm, TEMPLATE.as_bytes()]);
    assert!(matches!(
        refused(&raw(&tree, &manifest, &blobs, &[])),
        Error::Malformed(_)
    ));
}

#[test]
fn an_oversized_wasm_blob_is_refused() {
    let cap = usize::try_from(helmsman_plc::manifest::MAX_FILE).unwrap();
    let mut cs = horner_wasm_content();
    // Size is refused before anything parses it: the bytes past the
    // module need not be WASM.
    let mut big = horner_wasm(false);
    big.resize(cap + 1, 0);
    cs.modules.get_mut("horner").unwrap().code = Code::Wasm(big.clone());
    assert!(cs.validate().is_err(), "the store would not take it");
    let tree = cs.tree();
    let manifest = manifest_for(&horner_wasm_content()).render();
    let blobs = sorted_blobs(&[HORNER_MANIFEST.as_bytes(), &big, TEMPLATE.as_bytes()]);
    let e = refused(&raw(&tree, &manifest, &blobs, &[]));
    assert!(
        matches!(&e, Error::Malformed(w) if w.contains("more than a blob may hold")),
        "{e}"
    );
}

#[test]
fn a_wasm_module_whose_manifest_hides_an_import_is_refused_as_content() {
    // horner's manifest says it drives horn; the binary also imports lamp
    // as an output. The bundle manifest agrees with the module manifest
    // and both are signed: the loaded binary is what decides.
    let mut cs = horner_wasm_content();
    cs.modules.get_mut("horner").unwrap().code = Code::Wasm(horner_wasm(true));
    let e = capabilities(&cs, &d()).unwrap_err();
    assert!(e.join("\n").to_lowercase().contains("lamp"), "{e:?}");
    let mut b = Bundle::new(manifest_for(&horner_wasm_content()), cs);
    b.sign(&publisher());
    let e = verify(&b.encode(), &trust(), &d()).unwrap_err();
    assert!(matches!(e, Error::Content(_)), "{e}");
    // Not a WASM module at all: refused as content too.
    let mut cs = horner_wasm_content();
    cs.modules.get_mut("horner").unwrap().code = Code::Wasm(b"\0asm junk".to_vec());
    let mut b = Bundle::new(manifest_for(&horner_wasm_content()), cs);
    b.sign(&publisher());
    assert!(matches!(
        verify(&b.encode(), &trust(), &d()),
        Err(Error::Content(_))
    ));
}

#[test]
fn the_api_exports_a_wasm_snapshot_and_imports_it_signed_as_a_proposal() {
    let b = bench(Some(trust()));
    let mut cs = running();
    cs.modules.extend(horner_wasm_content().modules);
    let id = Store::open(&b.store)
        .unwrap()
        .commit(
            &cs,
            &Meta {
                author: "Test".into(),
                prompt: String::new(),
                approver: None,
                time_ms: 2,
            },
        )
        .unwrap()
        .to_string();
    let r = request(
        b.addr,
        "GET",
        &format!("/api/snapshots/{id}/bundle"),
        false,
        "application/json",
        b"",
    );
    assert_eq!(r.status, 200, "{r:?}");
    let mut exported = Bundle::decode(&r.body).unwrap();
    assert_eq!(exported.content, cs, "byte for byte");
    assert_eq!(exported.id().to_string(), id);
    // The snapshot as JSON shows the imports, never the binary.
    let j = request(
        b.addr,
        "GET",
        &format!("/api/snapshots/{id}"),
        false,
        "application/json",
        b"",
    )
    .json();
    let horner = &j["modules"]["horner"];
    assert_eq!(horner["source"], Json::Null, "{j}");
    assert_eq!(horner["wasm"]["bytes"], horner_wasm(false).len(), "{j}");
    let imports: Vec<String> = horner["wasm"]["imports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            format!(
                "{} {} {}",
                i["module"].as_str().unwrap(),
                i["name"].as_str().unwrap(),
                i["type"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        imports,
        [
            "helmsman/v1/in start () -> (i32)",
            "helmsman/v1/out horn (i32) -> ()"
        ],
        "{j}"
    );
    // Signed off the box and imported: a proposal carrying the binary.
    exported.sign(&publisher());
    let r = import(&b, &exported.encode());
    assert_eq!(r.status, 201, "{r:?}");
    let p = r.json();
    assert_eq!(p["modules"], json!(["conveyor", "horner"]), "{p}");
    assert_eq!(p["snapshot"], json!(id), "the same content, the same id: {p}");
    assert_eq!(
        p["base"],
        json!(running().id().to_string()),
        "what the box runs: {p}"
    );
}

// ---- WASM drivers are files, not bundle content (wasm-driver-home) ------------

/// helmsman-io's Modbus/TCP guest, a `helmsman-driver/v1` driver.
const MODBUS_GUEST: &[u8] = include_bytes!("../../helmsman-io/guests/modbus-tcp.wasm");

/// [`horner_content`] with its template's device on a WASM driver.
fn driver_content() -> ChangeSet {
    let mut cs = horner_content();
    cs.io_map = Some(TEMPLATE.replace("modbus-tcp", "wasm:modbus"));
    cs
}

#[test]
fn a_bundle_whose_map_names_a_wasm_driver_lists_it_and_carries_no_binary() {
    let cs = driver_content();
    let b = signed(cs.clone());
    assert_eq!(
        b.manifest.caps.drivers.iter().collect::<Vec<_>>(),
        ["wasm:modbus"]
    );
    assert!(!cs.tree().contains("wasm-driver"), "{}", cs.tree());
    let file = b.encode();
    let head = &MODBUS_GUEST[..64];
    assert!(file.windows(64).all(|w| w != head), "no driver bytes in the file");
    let v = verify(&file, &trust(), &d()).unwrap();
    assert_eq!(v.bundle.content, cs);
}

/// A bundle an older build (#89) exported with a `wasm-driver` line: it
/// is refused as malformed, saying where drivers live now, whatever the
/// line's name, before any blob is read as one.
#[test]
fn an_older_builds_bundle_carrying_a_driver_is_refused_and_says_why() {
    let cs = driver_content();
    let manifest = manifest_for(&cs).render();
    let template = cs.io_map.clone().unwrap();
    let line = format!("wasm-driver modbus {}", sha(MODBUS_GUEST));
    let tree = cs.tree().replacen("io-map ", &format!("{line}\nio-map "), 1);
    let blobs = sorted_blobs(&[
        HORNER_MANIFEST.as_bytes(),
        HORNER_SOURCE.as_bytes(),
        MODBUS_GUEST,
        template.as_bytes(),
    ]);
    let mut dev = TrustStore::default();
    dev.dev_bundles = true;
    for (why, tree) in [
        ("as #89 wrote it", tree.clone()),
        (
            "a path for a name",
            tree.replace("wasm-driver modbus", "wasm-driver ../modbus"),
        ),
        (
            "a control character in the name",
            tree.replace("wasm-driver modbus", "wasm-driver mo\u{1b}[2Jdbus"),
        ),
    ] {
        match verify(&raw(&tree, &manifest, &blobs, &[]), &dev, &d()) {
            Err(Error::Malformed(w)) => {
                assert!(w.contains("drivers are files"), "{why}: {w}");
                assert!(!w.contains('\u{1b}'), "{why}: the name is escaped: {w}");
            }
            r => panic!("{why}: {r:?}"),
        }
    }
}
