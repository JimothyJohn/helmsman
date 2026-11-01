//! Snapshots: the contract of `helmsman_plc::store` against a real SQLite
//! file in a temp dir. Content addressing, dedupe, rollback as a new log
//! entry, concurrent writers, tamper detection, schema versions, limits.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use helmsman_io::{Binding, Device, Dir, Signal, Ty, Value};
use helmsman_plc::manifest::{MAX_FILE, Param};
use helmsman_plc::store::{
    ChangeSet, Code, DB_FILE, Diff, Error, Field, Kind, Meta, ModuleChange, ModuleFiles, SCHEMA, SCHEMA_V2,
    SnapshotId, Store, diff,
};
use proptest::prelude::*;
use rusqlite::Connection;

static N: AtomicU64 = AtomicU64::new(0);

/// A fresh, empty directory for one store.
fn fresh(name: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("store")
        .join(format!("{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn open(name: &str) -> (Store, PathBuf) {
    let dir = fresh(name);
    (Store::open(&dir).unwrap(), dir)
}

fn raw(dir: &std::path::Path) -> Connection {
    Connection::open(dir.join(DB_FILE)).unwrap()
}

fn meta(author: &str) -> Meta {
    Meta {
        author: author.into(),
        prompt: "run the belt".into(),
        approver: Some("engineer".into()),
        time_ms: 1_793_000_000_000,
    }
}

fn manifest(name: &str, speed: f64, extra: &str) -> String {
    format!(
        "name     {name}\nauthor   Nick\nscan_ms  50\nreads    start\ndrives   motor\n\
         param    speed real {speed:?} min=0 max=10\n{extra}"
    )
}

const SRC: &str = "PROGRAM p\nVAR_INPUT start : BOOL; speed : REAL; END_VAR\n\
                   VAR_OUTPUT motor : BOOL; END_VAR\nmotor := start;\nEND_PROGRAM\n";

const MAP: &str = "device rio modbus-tcp 127.0.0.1:502 unit=1\n\
                   in  start bool rio di:1\nout motor bool rio coil:0\n";

fn files(name: &str, speed: f64, src: &str) -> (String, ModuleFiles) {
    (
        name.to_string(),
        ModuleFiles {
            manifest: manifest(name, speed, ""),
            code: src.into(),
        },
    )
}

fn set(mods: &[(&str, f64)]) -> ChangeSet {
    ChangeSet {
        modules: mods.iter().map(|(n, s)| files(n, *s, SRC)).collect(),
        io_map: Some(MAP.into()),
        calendar: None,
    }
}

fn count(dir: &std::path::Path, table: &str) -> i64 {
    raw(dir)
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn ids(s: &Store) -> Vec<SnapshotId> {
    s.list().unwrap().into_iter().map(|e| e.id).collect()
}

// ---- content addressing ---------------------------------------------------

#[test]
fn a_new_store_is_empty() {
    let (s, _) = open("empty");
    assert_eq!(s.head().unwrap(), None);
    assert_eq!(s.list().unwrap(), vec![]);
}

#[test]
fn commit_then_get_round_trips_and_the_id_is_the_content_address() {
    let (mut s, _) = open("roundtrip");
    let cs = set(&[("conveyor", 1.5)]);
    let id = s.commit(&cs, &meta("Nick")).unwrap();
    assert_eq!(id, cs.id());
    assert_eq!(s.get(&id).unwrap(), cs);
    let head = s.head().unwrap().unwrap();
    assert_eq!((head.id, head.seq, head.kind), (id, 1, Kind::Apply));
    assert_eq!(head.meta, meta("Nick"));
    assert_eq!(id.to_string().len(), 64);
    assert_eq!(SnapshotId::parse(&id.to_string()), Some(id));
    assert_eq!(SnapshotId::parse("zz"), None);
}

#[test]
fn identical_content_has_one_id_in_any_store_and_any_meta() {
    let (mut a, _) = open("same-a");
    let (mut b, _) = open("same-b");
    let cs = set(&[("conveyor", 1.5), ("lamp", 2.0)]);
    let x = a.commit(&cs, &meta("Nick")).unwrap();
    let y = b.commit(&cs, &meta("someone else")).unwrap();
    assert_eq!(x, y);
    // Any change to content is a different id: a param, a source byte,
    // the map, the calendar, a module name.
    let mut variants = vec![
        set(&[("conveyor", 1.6), ("lamp", 2.0)]),
        set(&[("conveyor", 1.5)]),
        ChangeSet {
            io_map: None,
            ..cs.clone()
        },
        ChangeSet {
            calendar: Some(String::new()),
            ..cs.clone()
        },
    ];
    let mut src = cs.clone();
    src.modules.get_mut("lamp").unwrap().code = format!("{SRC} ").into();
    variants.push(src);
    // The same bytes as a WASM binary are other content.
    let mut wasm = cs.clone();
    wasm.modules.get_mut("lamp").unwrap().code = Code::Wasm(SRC.as_bytes().to_vec());
    variants.push(wasm);
    for v in &variants {
        assert_ne!(v.id(), x, "{v:?}");
    }
}

#[test]
fn an_identical_change_set_dedupes_its_blobs_and_does_not_move_head() {
    let (mut s, dir) = open("dedupe");
    let cs = set(&[("conveyor", 1.5)]);
    let id = s.commit(&cs, &meta("Nick")).unwrap();
    let blobs = count(&dir, "blobs");
    assert_eq!(s.commit(&cs, &meta("Nick again")).unwrap(), id);
    assert_eq!(s.list().unwrap().len(), 1, "same as head: no new entry");
    assert_eq!(count(&dir, "blobs"), blobs);
    assert_eq!(count(&dir, "snapshots"), 1);
    // A, B, A: three entries, A's content stored once.
    let b = s.commit(&set(&[("conveyor", 2.5)]), &meta("Nick")).unwrap();
    assert_eq!(s.commit(&cs, &meta("Nick")).unwrap(), id);
    assert_eq!(ids(&s), vec![id, b, id]);
    assert_eq!(count(&dir, "snapshots"), 2);
}

#[test]
fn equal_files_in_different_modules_are_one_blob() {
    let (mut s, dir) = open("blob-share");
    let cs = ChangeSet {
        modules: [files("a", 1.0, SRC), files("b", 2.0, SRC)].into_iter().collect(),
        io_map: None,
        calendar: None,
    };
    s.commit(&cs, &meta("Nick")).unwrap();
    // two manifests + one shared source
    assert_eq!(count(&dir, "blobs"), 3);
}

// ---- rollback ---------------------------------------------------------------

#[test]
fn rollback_is_a_new_entry_pointing_at_old_content() {
    let (mut s, _) = open("rollback");
    let a = s.commit(&set(&[("conveyor", 1.0)]), &meta("agent")).unwrap();
    let b = s.commit(&set(&[("conveyor", 9.0)]), &meta("agent")).unwrap();
    let back = s.rollback(&a, &meta("Nick")).unwrap();
    assert_eq!(back, a);
    let head = s.head().unwrap().unwrap();
    assert_eq!((head.id, head.seq, head.kind), (a, 3, Kind::Rollback { to: a }));
    assert_eq!(head.meta.author, "Nick");
    // history is kept, newest first
    assert_eq!(ids(&s), vec![a, b, a]);
    assert_eq!(s.get(&b).unwrap(), set(&[("conveyor", 9.0)]));
}

#[test]
fn a_rollback_can_itself_be_rolled_back() {
    let (mut s, _) = open("rollback2");
    let a = s.commit(&set(&[("conveyor", 1.0)]), &meta("agent")).unwrap();
    let b = s.commit(&set(&[("conveyor", 9.0)]), &meta("agent")).unwrap();
    s.rollback(&a, &meta("Nick")).unwrap();
    assert_eq!(s.rollback(&b, &meta("Nick")).unwrap(), b);
    let kinds: Vec<Kind> = s.list().unwrap().into_iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        vec![
            Kind::Rollback { to: b },
            Kind::Rollback { to: a },
            Kind::Apply,
            Kind::Apply
        ]
    );
    assert_eq!(
        s.get(&s.head().unwrap().unwrap().id).unwrap(),
        set(&[("conveyor", 9.0)])
    );
}

#[test]
fn rollback_to_head_is_a_no_op_and_to_an_unknown_id_is_refused() {
    let (mut s, _) = open("rollback-edge");
    let unknown = set(&[("nowhere", 1.0)]).id();
    assert!(matches!(s.rollback(&unknown, &meta("Nick")), Err(Error::NotFound(id)) if id == unknown));
    let a = s.commit(&set(&[("conveyor", 1.0)]), &meta("agent")).unwrap();
    assert_eq!(s.rollback(&a, &meta("Nick")).unwrap(), a);
    assert_eq!(s.list().unwrap().len(), 1);
    assert!(matches!(
        s.rollback(&unknown, &meta("Nick")),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(s.get(&unknown), Err(Error::NotFound(_))));
}

// ---- concurrency ------------------------------------------------------------

#[test]
fn concurrent_commits_from_two_threads_lose_nothing() {
    let dir = fresh("concurrent");
    Store::open(&dir).unwrap();
    let per: u32 = 25;
    let workers: Vec<_> = (0..2)
        .map(|t| {
            let dir = dir.clone();
            thread::spawn(move || {
                let mut s = Store::open(&dir).unwrap();
                (0..per)
                    .map(|i| {
                        let cs = set(&[(&format!("t{t}"), f64::from(i) / 10.0)]);
                        let id = s.commit(&cs, &meta(&format!("thread {t}"))).unwrap();
                        assert_eq!(id, cs.id());
                        id
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mine: Vec<Vec<SnapshotId>> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    let s = Store::open(&dir).unwrap();
    let log = s.list().unwrap();
    assert_eq!(log.len(), usize::try_from(2 * per).unwrap());
    let seqs: Vec<u64> = log.iter().map(|e| e.seq).collect();
    let want: Vec<u64> = (1..=2 * u64::from(per)).rev().collect();
    assert_eq!(seqs, want, "seqs are contiguous: no entry lost or doubled");
    for (t, ids) in mine.iter().enumerate() {
        let theirs: Vec<SnapshotId> = log
            .iter()
            .rev()
            .filter(|e| e.meta.author == format!("thread {t}"))
            .map(|e| e.id)
            .collect();
        assert_eq!(&theirs, ids, "each thread's commits, in its order");
    }
    for e in &log {
        assert_eq!(s.get(&e.id).unwrap().id(), e.id);
    }
}

// ---- tampering and schema -----------------------------------------------------

#[test]
fn stored_history_cannot_be_updated_or_deleted_through_sql() {
    let (mut s, dir) = open("immutable");
    s.commit(&set(&[("conveyor", 1.0)]), &meta("Nick")).unwrap();
    let db = raw(&dir);
    for sql in [
        "UPDATE blobs SET data = x'00'",
        "DELETE FROM blobs",
        "UPDATE snapshots SET tree = ''",
        "DELETE FROM snapshots",
        "UPDATE log SET author = 'mallory'",
        "DELETE FROM log",
    ] {
        let err = db.execute(sql, []).unwrap_err().to_string();
        assert!(err.contains("immutable"), "{sql}: {err}");
    }
}

#[test]
fn a_corrupted_blob_is_detected_on_read() {
    let (mut s, dir) = open("corrupt-blob");
    let cs = set(&[("conveyor", 1.0)]);
    let id = s.commit(&cs, &meta("Nick")).unwrap();
    let db = raw(&dir);
    db.execute_batch("DROP TRIGGER blobs_no_update").unwrap();
    // Flip one byte of the program, keeping it valid UTF-8.
    let n = db
        .execute(
            "UPDATE blobs SET data = CAST(replace(CAST(data AS TEXT), 'motor := start', 'motor := FALSE') AS BLOB)",
            [],
        )
        .unwrap();
    assert!(n >= 1);
    match s.get(&id) {
        Err(Error::Corrupt(why)) => assert!(why.contains("blob"), "{why}"),
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

#[test]
fn a_corrupted_tree_or_log_entry_is_detected_on_read() {
    let (mut s, dir) = open("corrupt-tree");
    let id = s.commit(&set(&[("conveyor", 1.0)]), &meta("Nick")).unwrap();
    s.commit(&set(&[("conveyor", 2.0)]), &meta("Nick")).unwrap();
    let db = raw(&dir);
    db.execute_batch("DROP TRIGGER snapshots_no_update; DROP TRIGGER log_no_update")
        .unwrap();
    db.execute(
        "UPDATE snapshots SET tree = tree || ' ' WHERE id = ?1",
        [id.as_bytes().as_slice()],
    )
    .unwrap();
    assert!(matches!(s.get(&id), Err(Error::Corrupt(_))));
    db.execute("UPDATE log SET author = 'mallory' WHERE seq = 1", [])
        .unwrap();
    assert!(matches!(s.list(), Err(Error::Corrupt(_))));
    // the head entry itself is intact
    assert_eq!(s.head().unwrap().unwrap().seq, 2);
    db.execute("UPDATE log SET approver = NULL WHERE seq = 2", [])
        .unwrap();
    assert!(matches!(s.head(), Err(Error::Corrupt(_))));
}

#[test]
fn a_store_from_a_newer_schema_is_refused() {
    let dir = fresh("newer");
    Store::open(&dir).unwrap();
    raw(&dir).pragma_update(None, "user_version", SCHEMA + 1).unwrap();
    match Store::open(&dir) {
        Err(e @ Error::NewerSchema { found, supported }) => {
            assert_eq!((found, supported), (SCHEMA + 1, SCHEMA));
            let msg = e.to_string();
            assert!(msg.contains("newer"), "{msg}");
        }
        other => panic!("expected NewerSchema, got {:?}", other.err()),
    }
}

#[test]
fn a_file_that_is_not_a_store_is_refused() {
    let dir = fresh("foreign");
    std::fs::create_dir_all(&dir).unwrap();
    raw(&dir).execute_batch("CREATE TABLE t (x)").unwrap();
    assert!(matches!(Store::open(&dir), Err(Error::NotAStore(_))));
    let dir = fresh("garbage");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(DB_FILE), vec![0x5a; 8192]).unwrap();
    assert!(Store::open(&dir).is_err());
}

// ---- validation and limits ------------------------------------------------------

#[test]
fn an_empty_change_set_is_a_snapshot() {
    let (mut s, _) = open("empty-set");
    let empty = ChangeSet::default();
    let id = s.commit(&empty, &meta("Nick")).unwrap();
    assert_eq!(s.get(&id).unwrap(), empty);
    assert_ne!(id, set(&[]).id(), "an empty map is not no map");
}

#[test]
fn a_huge_change_set_round_trips() {
    let (mut s, _) = open("huge");
    let big = "(* x *)\n".repeat(usize::try_from(MAX_FILE).unwrap() / 8);
    let modules: BTreeMap<String, ModuleFiles> = (0..300)
        .map(|i| files(&format!("m{i}"), f64::from(i % 10), &format!("{SRC}(* {i} *)")))
        .chain([files("big", 1.0, &big)])
        .collect();
    let cs = ChangeSet {
        modules,
        io_map: Some(MAP.into()),
        calendar: Some("zone +01:00\n".into()),
    };
    let id = s.commit(&cs, &meta("Nick")).unwrap();
    assert_eq!(s.get(&id).unwrap(), cs);
}

#[test]
fn a_file_over_the_limit_is_refused() {
    let (mut s, _) = open("too-big");
    let mut cs = set(&[("conveyor", 1.0)]);
    cs.modules.get_mut("conveyor").unwrap().code = "x".repeat(usize::try_from(MAX_FILE).unwrap() + 1).into();
    assert!(matches!(s.commit(&cs, &meta("Nick")), Err(Error::Invalid(_))));
    let cs = ChangeSet {
        calendar: Some("x".repeat(usize::try_from(MAX_FILE).unwrap() + 1)),
        ..ChangeSet::default()
    };
    assert!(matches!(s.commit(&cs, &meta("Nick")), Err(Error::Invalid(_))));
    assert_eq!(s.head().unwrap(), None);
}

#[test]
fn malformed_change_sets_are_refused_with_every_reason() {
    let (mut s, _) = open("invalid");
    let mut cs = set(&[("conveyor", 1.0)]);
    // key and manifest name disagree
    cs.modules.insert("other".into(), files("lamp", 1.0, SRC).1);
    // a manifest that does not parse
    cs.modules.insert(
        "broken".into(),
        ModuleFiles {
            manifest: "name broken\n".into(),
            code: SRC.into(),
        },
    );
    // a module name the runtime refuses
    cs.modules
        .insert("bad name".into(), files("bad name", 1.0, SRC).1);
    cs.io_map = Some("nonsense line\n".into());
    match s.commit(&cs, &meta("Nick")) {
        Err(Error::Invalid(why)) => {
            assert_eq!(why.len(), 4, "{why:?}");
            let all = why.join("\n");
            for want in ["other", "broken", "bad name", "io map"] {
                assert!(all.contains(want), "{want} missing from {all}");
            }
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
    assert_eq!(s.head().unwrap(), None);
}

#[test]
fn unicode_meta_round_trips_and_hidden_characters_are_refused() {
    let (mut s, _) = open("unicode");
    let m = Meta {
        author: "Zoë 山田 <zoe@例え.jp> 🚀".into(),
        prompt: "Run the belt — schnell!\n\tthen stop on jam ⚠️\nسلام".into(),
        approver: Some("Ingeniør Øystein".into()),
        time_ms: -1,
    };
    s.commit(&set(&[("conveyor", 1.0)]), &m).unwrap();
    assert_eq!(s.head().unwrap().unwrap().meta, m);
    for bad in [
        Meta {
            author: String::new(),
            ..m.clone()
        },
        Meta {
            author: "evil\u{202E}txt".into(),
            ..m.clone()
        },
        Meta {
            author: "two\nlines".into(),
            ..m.clone()
        },
        Meta {
            prompt: "ansi \u{1b}[31m red".into(),
            ..m.clone()
        },
        Meta {
            approver: Some("zero\u{200B}width".into()),
            ..m.clone()
        },
        Meta {
            approver: Some(String::new()),
            ..m.clone()
        },
        Meta {
            prompt: "p".repeat(64 * 1024 + 1),
            ..m.clone()
        },
    ] {
        assert!(
            matches!(s.commit(&set(&[("conveyor", 2.0)]), &bad), Err(Error::Invalid(_))),
            "{bad:?}"
        );
    }
    assert_eq!(s.list().unwrap().len(), 1);
}

// ---- diff ---------------------------------------------------------------------

fn speed(v: f64) -> Param {
    Param {
        name: "speed".into(),
        value: Value::Real(v),
        range: Some((Value::Real(0.0), Value::Real(10.0))),
        unit: None,
    }
}

fn sig(name: &str, dir: Dir, binding: Binding) -> Signal {
    Signal {
        name: name.into(),
        ty: Ty::Bool,
        dir,
        binding,
        unit: None,
        poll: None,
    }
}

fn dev(port: u16) -> Device {
    Device {
        id: "rio".into(),
        driver: "modbus-tcp".into(),
        endpoint: format!("127.0.0.1:{port}"),
        params: [("unit".to_string(), "1".to_string())].into_iter().collect(),
    }
}

fn coil(n: u8) -> Binding {
    Binding::Device {
        device: "rio".into(),
        point: format!("coil:{n}"),
    }
}

#[test]
fn diff_names_modules_params_and_bindings() {
    let (mut s, _) = open("diff");
    let a = ChangeSet {
        modules: [
            files("conveyor", 1.5, SRC),
            files("old", 1.0, SRC),
            files("lamp", 1.0, SRC),
        ]
        .into_iter()
        .collect(),
        io_map: Some(MAP.into()),
        calendar: None,
    };
    let mut b = a.clone();
    b.modules.remove("old");
    b.modules.extend([files("new", 1.0, SRC)]);
    b.modules.get_mut("conveyor").unwrap().manifest = manifest("conveyor", 2.5, "param beacon bool true\n");
    b.modules.get_mut("lamp").unwrap().code = format!("{SRC}(* tweak *)\n").into();
    b.io_map = Some(
        "device rio modbus-tcp 127.0.0.1:503 unit=1\n\
         in  start bool rio di:1\nout motor bool rio coil:1\nout horn bool box do:0 safe=true\n"
            .into(),
    );
    b.calendar = Some("zone +01:00\n".into());
    let ia = s.commit(&a, &meta("Nick")).unwrap();
    let ib = s.commit(&b, &meta("Nick")).unwrap();
    let d = s.diff(&ia, &ib).unwrap();
    assert_eq!(d, diff(&a, &b));
    let beacon = Param {
        name: "beacon".into(),
        value: Value::Bool(true),
        range: None,
        unit: None,
    };
    let want = Diff {
        modules: vec![
            ModuleChange::Changed {
                name: "conveyor".into(),
                source: false,
                manifest: true,
                params: vec![
                    Field {
                        name: "beacon".into(),
                        from: None,
                        to: Some(beacon),
                    },
                    Field {
                        name: "speed".into(),
                        from: Some(speed(1.5)),
                        to: Some(speed(2.5)),
                    },
                ],
            },
            ModuleChange::Changed {
                name: "lamp".into(),
                source: true,
                manifest: false,
                params: vec![],
            },
            ModuleChange::Added("new".into()),
            ModuleChange::Removed("old".into()),
        ],
        io_map: true,
        devices: vec![Field {
            name: "rio".into(),
            from: Some(dev(502)),
            to: Some(dev(503)),
        }],
        bindings: vec![
            Field {
                name: "horn".into(),
                from: None,
                to: Some((
                    sig(
                        "horn",
                        Dir::Out,
                        Binding::Local {
                            channel: "do:0".into(),
                        },
                    ),
                    Some(Value::Bool(true)),
                )),
            },
            Field {
                name: "motor".into(),
                from: Some((sig("motor", Dir::Out, coil(0)), None)),
                to: Some((sig("motor", Dir::Out, coil(1)), None)),
            },
        ],
        calendar: true,
    };
    assert_eq!(d, want);
    assert!(diff(&a, &a).is_empty());
    assert!(!d.is_empty());
    let back = diff(&b, &a);
    assert_eq!(back.modules.len(), 4);
    assert!(back.calendar && back.io_map);
}

// ---- properties ----------------------------------------------------------------------

fn arb_change_set() -> impl Strategy<Value = ChangeSet> {
    let module = (
        "[a-z][a-z0-9_-]{0,12}",
        0.0f64..10.0,
        "\\PC{0,200}",
        any::<bool>(),
    );
    (
        prop::collection::vec(module, 0..6),
        prop::option::of(Just(MAP.to_string())),
        prop::option::of("\\PC{0,100}"),
    )
        .prop_map(|(mods, io_map, calendar)| ChangeSet {
            modules: mods
                .into_iter()
                .map(|(name, speed, src, crlf)| {
                    let source = if crlf { src.replace('\n', "\r\n") } else { src };
                    (
                        name.clone(),
                        ModuleFiles {
                            manifest: manifest(&name, speed, ""),
                            code: source.into(),
                        },
                    )
                })
                .collect(),
            io_map,
            calendar,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn commit_get_round_trips_any_change_set(cs in arb_change_set(), author in "\\p{L}[\\p{L}\\p{N} ]{0,39}") {
        let (mut s, _) = open("prop");
        let m = Meta { author, prompt: String::new(), approver: None, time_ms: 0 };
        let id = s.commit(&cs, &m).unwrap();
        prop_assert_eq!(id, cs.id());
        prop_assert_eq!(s.get(&id).unwrap(), cs.clone());
        prop_assert_eq!(s.head().unwrap().unwrap().meta, m);
        prop_assert!(diff(&cs, &cs).is_empty());
    }

    #[test]
    fn different_change_sets_have_different_ids(a in arb_change_set(), b in arb_change_set()) {
        prop_assert_eq!(a == b, a.id() == b.id());
    }
}

// ---- WASM modules ---------------------------------------------------------------

/// Stands in for a `module.wasm`: the store neither parses nor loads it,
/// and these bytes are not UTF-8 (every byte value, then the magic).
fn binary() -> Vec<u8> {
    let mut b: Vec<u8> = (0..=255).collect();
    b.extend_from_slice(b"\0asm\x01\0\0\0");
    b
}

fn with_wasm(cs: &ChangeSet, module: &str, bytes: &[u8]) -> ChangeSet {
    let mut cs = cs.clone();
    let m = cs.modules[module].manifest.clone();
    cs.modules.insert(module.into(), ModuleFiles::wasm(m, bytes));
    cs
}

fn sha(b: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).to_vec()
}

fn sha_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    helmsman_plc::bundle::sign::hex_key(&Sha256::digest(b).into())
}

#[test]
fn a_wasm_module_round_trips_byte_identical_through_commit_get_and_rollback() {
    let (mut s, _) = open("wasm-round-trip");
    let st = set(&[("conveyor", 1.0), ("lamp", 1.0)]);
    let first = s.commit(&st, &meta("Nick")).unwrap();
    let cs = with_wasm(&st, "lamp", &binary());
    let id = s.commit(&cs, &meta("Nick")).unwrap();
    assert_eq!(id, cs.id());
    let got = s.get(&id).unwrap();
    assert_eq!(got, cs);
    assert_eq!(
        got.modules["lamp"].code.wasm(),
        Some(binary().as_slice()),
        "byte for byte"
    );
    assert_eq!(got.modules["conveyor"].code.st(), Some(SRC));
    // The tree names the program's kind; ST lines are as they always were.
    let tree = cs.tree();
    let line = format!(
        "\nwasm-module lamp {} {}\n",
        sha_hex(cs.modules["lamp"].manifest.as_bytes()),
        sha_hex(&binary())
    );
    assert!(tree.contains(&line), "{tree}");
    assert!(tree.contains("\nmodule conveyor "), "{tree}");
    // Rolled back to ST and forward to the WASM content again: the same
    // id, the same bytes.
    s.rollback(&first, &meta("Nick")).unwrap();
    assert_eq!(s.rollback(&id, &meta("Nick")).unwrap(), id);
    assert_eq!(s.get(&s.head().unwrap().unwrap().id).unwrap(), cs);
    let d = s.diff(&first, &id).unwrap();
    assert_eq!(
        d.modules,
        [ModuleChange::Changed {
            name: "lamp".into(),
            source: true,
            manifest: false,
            params: Vec::new(),
        }]
    );
}

#[test]
fn st_text_and_a_wasm_binary_of_the_same_bytes_are_different_content() {
    let st = set(&[("lamp", 1.0)]);
    let wasm = with_wasm(&st, "lamp", SRC.as_bytes());
    assert_ne!(st.id(), wasm.id());
    let (mut s, dir) = open("wasm-vs-st");
    let a = s.commit(&st, &meta("Nick")).unwrap();
    let b = s.commit(&wasm, &meta("Nick")).unwrap();
    assert_ne!(a, b);
    assert_eq!(s.get(&a).unwrap(), st);
    assert_eq!(s.get(&b).unwrap(), wasm);
    // Content addressing is by bytes: one blob holds both programs.
    assert_eq!(count(&dir, "blobs"), 3, "manifest, program, map");
}

#[test]
fn a_tampered_wasm_blob_is_detected_on_read() {
    let (mut s, dir) = open("wasm-tamper");
    let cs = with_wasm(&set(&[("lamp", 1.0)]), "lamp", &binary());
    let id = s.commit(&cs, &meta("Nick")).unwrap();
    let db = raw(&dir);
    db.execute_batch("DROP TRIGGER blobs_no_update").unwrap();
    let mut evil = binary();
    evil[100] ^= 0x01;
    let n = db
        .execute(
            "UPDATE blobs SET data = ?1 WHERE hash = ?2",
            rusqlite::params![evil, sha(&binary())],
        )
        .unwrap();
    assert_eq!(n, 1);
    match s.get(&id) {
        Err(Error::Corrupt(why)) => assert!(why.contains("does not match its hash"), "{why}"),
        other => panic!("expected Corrupt, got {other:?}"),
    }
    db.execute("UPDATE blobs SET data = x'' WHERE hash = ?1", [sha(&binary())])
        .unwrap();
    assert!(matches!(s.get(&id), Err(Error::Corrupt(_))), "truncated");
}

/// Inserts a snapshot row whose id is its tree's hash, as a forger
/// with write access to the file would.
fn forge(dir: &std::path::Path, tree: &str) -> SnapshotId {
    raw(dir)
        .execute(
            "INSERT INTO snapshots (id, tree) VALUES (?1, ?2)",
            rusqlite::params![sha(tree.as_bytes()), tree],
        )
        .unwrap();
    SnapshotId::parse(&sha_hex(tree.as_bytes())).unwrap()
}

#[test]
fn a_tree_that_reads_a_binary_blob_as_text_is_corrupt() {
    let (mut s, dir) = open("wasm-as-text");
    let cs = with_wasm(&set(&[("lamp", 1.0)]), "lamp", &binary());
    s.commit(&cs, &meta("Nick")).unwrap();
    // Self-consistent (its id is its hash), but it calls the WASM binary
    // `module.st`: refused, never handed out as a lossy string.
    let id = forge(&dir, &cs.tree().replace("wasm-module lamp", "module lamp"));
    match s.get(&id) {
        Err(Error::Corrupt(why)) => assert!(why.contains("not UTF-8"), "{why}"),
        other => panic!("expected Corrupt, got {other:?}"),
    }
    // A manifest that names the binary blob: the same.
    let (m, w) = (
        sha_hex(cs.modules["lamp"].manifest.as_bytes()),
        sha_hex(&binary()),
    );
    let id = forge(&dir, &cs.tree().replace(&format!("{m} {w}"), &format!("{w} {w}")));
    assert!(matches!(s.get(&id), Err(Error::Corrupt(why)) if why.contains("not UTF-8")));
    // A non-UTF-8 module.st on disk is refused on read.
    let dir = fresh("st-not-utf8");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("module.manifest"), manifest("lamp", 1.0, "")).unwrap();
    std::fs::write(dir.join("module.st"), binary()).unwrap();
    let err = ChangeSet::read(std::slice::from_ref(&dir), None, None).unwrap_err();
    assert!(err.contains("not UTF-8"), "{err}");
}

#[test]
fn a_module_directory_reads_module_wasm_as_bytes_and_refuses_both_programs() {
    let dir = fresh("wasm-moddir");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("module.manifest"), manifest("lamp", 1.0, "")).unwrap();
    std::fs::write(dir.join("module.wasm"), binary()).unwrap();
    let cs = ChangeSet::read(std::slice::from_ref(&dir), None, None).unwrap();
    assert_eq!(cs.modules["lamp"].code, Code::Wasm(binary()));
    std::fs::write(dir.join("module.st"), SRC).unwrap();
    let err = ChangeSet::read(std::slice::from_ref(&dir), None, None).unwrap_err();
    assert!(err.contains("both module.st and module.wasm"), "{err}");
    std::fs::remove_file(dir.join("module.st")).unwrap();
    std::fs::write(
        dir.join("module.wasm"),
        vec![0; usize::try_from(MAX_FILE).unwrap() + 1],
    )
    .unwrap();
    let err = ChangeSet::read(std::slice::from_ref(&dir), None, None).unwrap_err();
    assert!(err.contains("larger than"), "{err}");
}

#[test]
fn an_oversized_wasm_is_refused() {
    let (mut s, _) = open("wasm-too-big");
    let cap = usize::try_from(MAX_FILE).unwrap();
    let st = set(&[("lamp", 1.0)]);
    let at_cap = with_wasm(&st, "lamp", &vec![0xff; cap]);
    s.commit(&at_cap, &meta("Nick")).unwrap();
    let over = with_wasm(&st, "lamp", &vec![0xff; cap + 1]);
    match s.commit(&over, &meta("Nick")) {
        Err(Error::Invalid(why)) => assert!(why[0].contains("larger than"), "{why:?}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
    assert_eq!(s.head().unwrap().unwrap().id, at_cap.id());
}

// ---- a store written before WASM modules ------------------------------------------

/// `tests/fixtures/store-v1/snapshots.db` was written by the build before
/// WASM modules (schema 1): conveyor at speed 1.0 (A), then 2.0 (B), then
/// rolled back to A by `engineer`.
const V1_A: &str = "db1c1a6833401e8fb2fc79d93b84f1dd41030e549d80f968ae5b885ebdead374";
const V1_B: &str = "8791fc7f9e53e9c78416efbe5413f24a14a71debfc3ac69599fb375661dab3a4";

/// A copy of the fixture, so a test never writes the checked-in file.
fn v1_store(name: &str) -> PathBuf {
    let dir = fresh(name);
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/store-v1")
        .join(DB_FILE);
    std::fs::copy(fixture, dir.join(DB_FILE)).unwrap();
    dir
}

fn v1_content(speed: f64) -> ChangeSet {
    ChangeSet {
        modules: [files("conveyor", speed, SRC)].into(),
        io_map: Some(MAP.into()),
        calendar: Some("zone +01:00\n".into()),
    }
}

fn user_version(dir: &std::path::Path) -> i64 {
    raw(dir)
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap()
}

#[test]
fn a_store_from_before_wasm_opens_and_keeps_its_rows_and_ids() {
    let dir = v1_store("v1-open");
    assert_eq!(user_version(&dir), 1);
    let s = Store::open(&dir).unwrap();
    assert_eq!(s.schema().unwrap(), 1, "opening changes nothing");
    let (a, b) = (SnapshotId::parse(V1_A).unwrap(), SnapshotId::parse(V1_B).unwrap());
    let log = s.list().unwrap();
    assert_eq!(
        log.iter().map(|e| e.id).collect::<Vec<_>>(),
        [a, b, a],
        "the hash chain checks"
    );
    assert_eq!(log[0].kind, Kind::Rollback { to: a });
    assert_eq!(log[0].meta.author, "engineer");
    assert_eq!(log[2].meta.time_ms, 1_793_000_000_000);
    // The content, and the ids this build computes for it: an ST
    // snapshot hashes as it always did.
    assert_eq!(s.get(&a).unwrap(), v1_content(1.0));
    assert_eq!(s.get(&b).unwrap(), v1_content(2.0));
    assert_eq!(v1_content(1.0).id(), a);
    assert_eq!(v1_content(2.0).id(), b);
}

#[test]
fn a_store_from_before_wasm_stays_v1_for_st_and_moves_to_v2_with_its_first_wasm_module() {
    let dir = v1_store("v1-upgrade");
    let a = SnapshotId::parse(V1_A).unwrap();
    {
        let mut s = Store::open(&dir).unwrap();
        s.commit(&v1_content(3.0), &meta("Nick")).unwrap();
        s.rollback(&SnapshotId::parse(V1_B).unwrap(), &meta("Nick"))
            .unwrap();
        assert_eq!(s.schema().unwrap(), 1, "an older build still opens it");
        // A WASM change set the store refuses changes nothing either.
        let big = with_wasm(
            &v1_content(3.0),
            "conveyor",
            &vec![0; usize::try_from(MAX_FILE).unwrap() + 1],
        );
        assert!(s.commit(&big, &meta("Nick")).is_err());
        assert_eq!(s.schema().unwrap(), 1);
        let w = with_wasm(&v1_content(3.0), "conveyor", &binary());
        s.commit(&w, &meta("Nick")).unwrap();
        assert_eq!(s.schema().unwrap(), SCHEMA_V2);
    }
    assert_eq!(user_version(&dir), 2);
    let s = Store::open(&dir).unwrap();
    let log = s.list().unwrap();
    assert_eq!(log.len(), 6, "every row kept, the chain intact");
    assert_eq!(log[5].id, a);
    assert_eq!(s.get(&a).unwrap(), v1_content(1.0));
    assert_eq!(
        s.get(&log[0].id).unwrap().modules["conveyor"].code,
        Code::Wasm(binary())
    );
    // Past this build's version: refused, not misread.
    raw(&dir).pragma_update(None, "user_version", SCHEMA + 1).unwrap();
    assert!(matches!(Store::open(&dir), Err(Error::NewerSchema { .. })));
}

#[test]
fn a_store_file_with_no_schema_version_is_not_a_store() {
    let dir = v1_store("v0");
    raw(&dir).pragma_update(None, "user_version", 0).unwrap();
    assert!(matches!(Store::open(&dir), Err(Error::NotAStore(_))));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn any_wasm_bytes_round_trip(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let (mut s, _) = open("prop-wasm");
        let cs = with_wasm(&set(&[("lamp", 1.0)]), "lamp", &bytes);
        let id = s.commit(&cs, &meta("Nick")).unwrap();
        let got = s.get(&id).unwrap();
        prop_assert_eq!(got.modules["lamp"].code.wasm(), Some(bytes.as_slice()));
    }
}
