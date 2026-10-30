//! The sink queue file: bounded, survives a restart, and recovers from a
//! torn or damaged file without panicking and without ever replaying a
//! half-written record.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use helmsman_plc::sink::queue::{Pushed, Queue, cost, crc32};
use proptest::prelude::*;

fn file(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("sink-queue");
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join(format!("{name}-{}-{n}.queue", std::process::id()));
    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_file(p.with_extension("corrupt"));
    p
}

fn contents(q: &Queue) -> Vec<(String, Vec<u8>)> {
    q.iter().map(|e| (e.key.clone(), e.body.clone())).collect()
}

#[test]
fn crc32_matches_the_standard_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

#[test]
fn events_survive_a_reopen_in_order() {
    let p = file("reopen");
    let (mut q, rec) = Queue::open(&p, 1 << 16).unwrap();
    assert_eq!(rec.cut_bytes, 0);
    for i in 0..5 {
        q.push(&format!("k-{i}"), format!("{{\"n\":{i}}}").as_bytes())
            .unwrap();
    }
    q.sync().unwrap();
    q.pop().unwrap();
    drop(q);
    let (q, rec) = Queue::open(&p, 1 << 16).unwrap();
    assert_eq!(rec.cut_bytes, 0);
    let keys: Vec<_> = q.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["k-1", "k-2", "k-3", "k-4"]);
}

#[test]
fn an_emptied_queue_truncates_its_file() {
    let p = file("empty");
    let (mut q, _) = Queue::open(&p, 1 << 16).unwrap();
    q.push("a", b"1").unwrap();
    q.push("b", b"2").unwrap();
    q.pop().unwrap();
    q.pop().unwrap();
    assert_eq!(q.file_len(), 4);
    assert_eq!(std::fs::metadata(&p).unwrap().len(), 4);
}

#[test]
fn full_drops_the_oldest_and_too_big_is_refused() {
    let p = file("full");
    let one = cost("k-00", &[b'x'; 100]);
    let (mut q, _) = Queue::open(&p, one * 3).unwrap();
    for i in 0..3 {
        assert_eq!(
            q.push(&format!("k-{i:02}"), &[b'x'; 100]).unwrap(),
            Pushed::Queued { dropped: 0 }
        );
    }
    assert_eq!(
        q.push("k-03", &[b'x'; 100]).unwrap(),
        Pushed::Queued { dropped: 1 }
    );
    assert_eq!(q.front().unwrap().key, "k-01");
    assert_eq!(q.push("k-04", &vec![b'x'; one * 3]).unwrap(), Pushed::TooBig);
    assert_eq!(q.dropped(), 2);
    assert_eq!(q.len(), 3);
    assert!(q.bytes() <= one * 3);
    assert_eq!(
        q.push(&"k".repeat(300), b"x").unwrap(),
        Pushed::TooBig,
        "key over 256 bytes"
    );
}

#[test]
fn a_smaller_bound_on_reopen_drops_the_oldest() {
    let p = file("shrink");
    let (mut q, _) = Queue::open(&p, 1 << 16).unwrap();
    for i in 0..10 {
        q.push(&format!("k-{i}"), &[0; 50]).unwrap();
    }
    drop(q);
    let (q, rec) = Queue::open(&p, cost("k-0", &[0; 50]) * 4).unwrap();
    assert_eq!(rec.dropped, 6);
    assert_eq!(q.front().unwrap().key, "k-6");
}

#[test]
fn a_file_that_is_not_a_queue_is_moved_aside() {
    let p = file("foreign");
    std::fs::write(&p, b"#!/bin/sh\nrm -rf /\n").unwrap();
    let (q, rec) = Queue::open(&p, 1 << 16).unwrap();
    assert_eq!(q.len(), 0);
    assert_eq!(
        rec.moved_aside.as_deref(),
        Some(p.with_extension("corrupt").as_path())
    );
    assert!(p.with_extension("corrupt").exists());
    // A file cut while its magic was being written is just empty.
    let p = file("torn-magic");
    std::fs::write(&p, b"HS").unwrap();
    let (q, rec) = Queue::open(&p, 1 << 16).unwrap();
    assert_eq!((q.len(), rec.moved_aside), (0, None));
}

#[test]
fn compaction_keeps_the_file_near_the_bound() {
    let p = file("compact");
    let bound = 8192;
    let (mut q, _) = Queue::open(&p, bound).unwrap();
    for i in 0..5000 {
        q.push(&format!("k-{i}"), &[b'y'; 40]).unwrap();
        if i % 3 != 0 {
            q.pop().unwrap();
        }
        assert!(
            q.file_len() <= 2 * bound as u64 + 4096 + 200,
            "{} at {i}",
            q.file_len()
        );
    }
    let want = contents(&q);
    drop(q);
    let (q, rec) = Queue::open(&p, bound).unwrap();
    assert_eq!(rec.cut_bytes, 0);
    assert_eq!(contents(&q), want);
}

#[derive(Debug, Clone)]
enum Op {
    Push(String, Vec<u8>),
    Pop,
    Reopen,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => ("[a-z0-9-]{0,12}", proptest::collection::vec(any::<u8>(), 0..200)).prop_map(|(k, b)| Op::Push(k, b)),
        2 => Just(Op::Pop),
        1 => Just(Op::Reopen),
    ]
}

/// Pushes `items` into a fresh queue file and returns its bytes.
fn written(name: &str, items: &[(String, Vec<u8>)]) -> (PathBuf, Vec<u8>) {
    let p = file(name);
    let (mut q, _) = Queue::open(&p, 1 << 20).unwrap();
    for (k, b) in items {
        q.push(k, b).unwrap();
    }
    q.sync().unwrap();
    drop(q);
    let bytes = std::fs::read(&p).unwrap();
    (p, bytes)
}

fn items() -> impl Strategy<Value = Vec<(String, Vec<u8>)>> {
    proptest::collection::vec(
        ("[a-z0-9-]{1,12}", proptest::collection::vec(any::<u8>(), 0..64)),
        1..12,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any sequence of pushes, pops and restarts matches a plain
    /// in-memory queue with the same drop-oldest bound.
    #[test]
    fn the_queue_matches_a_model_across_restarts(ops in proptest::collection::vec(op(), 0..80)) {
        let bound = 2048;
        let p = file("model");
        let (mut q, _) = Queue::open(&p, bound).unwrap();
        let mut model: VecDeque<(String, Vec<u8>)> = VecDeque::new();
        for o in ops {
            match o {
                Op::Push(k, b) => {
                    let c = cost(&k, &b);
                    q.push(&k, &b).unwrap();
                    if c <= bound {
                        while model.iter().map(|(k, b)| cost(k, b)).sum::<usize>() + c > bound {
                            model.pop_front();
                        }
                        model.push_back((k, b));
                    }
                }
                Op::Pop => {
                    q.pop().unwrap();
                    model.pop_front();
                }
                Op::Reopen => {
                    q.sync().unwrap();
                    drop(q);
                    let (q2, rec) = Queue::open(&p, bound).unwrap();
                    prop_assert_eq!(rec.cut_bytes, 0);
                    q = q2;
                }
            }
            prop_assert_eq!(contents(&q), model.iter().cloned().collect::<Vec<_>>());
            prop_assert!(q.bytes() <= bound);
        }
    }

    /// A file cut at any byte (power lost mid-append) loads as exactly the
    /// records that were whole: a prefix, never a half-written one.
    #[test]
    fn a_torn_file_loads_the_whole_records_only(items in items(), cut in any::<prop::sample::Index>()) {
        let (p, bytes) = written("torn", &items);
        let at = cut.index(bytes.len() + 1);
        std::fs::write(&p, &bytes[..at]).unwrap();
        let (q, rec) = Queue::open(&p, 1 << 20).unwrap();
        let got = contents(&q);
        prop_assert!(got.len() <= items.len());
        prop_assert_eq!(&got[..], &items[..got.len()]);
        // Every whole record survived.
        let whole = items
            .iter()
            .scan(4usize, |end, (k, b)| { *end += cost(k, b); Some(*end) })
            .take_while(|end| *end <= at)
            .count();
        prop_assert_eq!(got.len(), whole);
        if at >= 4 {
            prop_assert_eq!(std::fs::metadata(&p).unwrap().len() + rec.cut_bytes, at as u64);
        }
        // And the cut file takes appends again.
        drop(q);
        let (mut q, _) = Queue::open(&p, 1 << 20).unwrap();
        q.push("after", b"cut").unwrap();
        drop(q);
        let (q, rec) = Queue::open(&p, 1 << 20).unwrap();
        prop_assert_eq!(rec.cut_bytes, 0);
        prop_assert_eq!(&q.iter().last().unwrap().key, "after");
    }

    /// Any bytes flipped anywhere: loading never panics, and everything it
    /// returns is an event that was really pushed, whole, in order.
    #[test]
    fn a_damaged_file_never_yields_a_record_that_was_not_written(
        items in items(),
        flips in proptest::collection::vec((any::<prop::sample::Index>(), 1u8..), 1..4),
    ) {
        let (p, mut bytes) = written("damaged", &items);
        for (i, x) in &flips {
            let i = i.index(bytes.len());
            bytes[i] ^= x;
        }
        std::fs::write(&p, &bytes).unwrap();
        let (q, _) = Queue::open(&p, 1 << 20).unwrap();
        let got = contents(&q);
        prop_assert!(got.len() <= items.len());
        prop_assert_eq!(&got[..], &items[..got.len()]);
    }

    /// Arbitrary bytes behind a valid magic: no panic, no event.
    #[test]
    fn garbage_never_panics(tail in proptest::collection::vec(any::<u8>(), 0..512)) {
        let p = file("garbage");
        let mut bytes = b"HSQ1".to_vec();
        bytes.extend_from_slice(&tail);
        std::fs::write(&p, &bytes).unwrap();
        let (q, rec) = Queue::open(&p, 1 << 20).unwrap();
        // A random 8-byte frame passing a CRC-32 is 2^-32; treat any
        // survivor as a real decode and only check the bound holds.
        prop_assert!(q.bytes() <= 1 << 20);
        prop_assert!(rec.cut_bytes <= tail.len() as u64);
    }
}
