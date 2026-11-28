//! The Skipper's wall clock on its page (Nick, 2026-11-26: "The clock on
//! the Skipper must sync to the browser/device so it's an actual
//! timestamp, not a time passed since it went alive"). The page reads
//! `GET /api/node` every 10 s while it watches a Skipper; holding the
//! token, it sets the Skipper's clock from the browser's when the Skipper
//! has none or is more than 2 s off, at most every 30 s. The toolbar shows
//! the Skipper's time as a time of day. Every call goes to the real
//! Skipper router (`node_map_tests::Skipper`); its answers are read by the
//! page's own code.

use crate::box_projects::{Answer, PCall, PReq};
use crate::node_map_tests::{Skipper, TOKEN};
use crate::skipper_clock::{MAX_SKEW_MS, SET_EVERY, SkipperClock};

use super::{App, Harness};

/// 2026-11-26T14:03:27Z.
const NOW: u64 = 1_795_701_807_000;
/// The fake Skipper's uptime (`Skipper::new`).
const UPTIME: u64 = 100_000;

fn answer(status: u16, body: &str) -> Answer {
    Answer {
        status,
        retry_after: None,
        body: body.into(),
    }
}

/// One page instant: what the clock asks at page time `now`, sent to `sk`
/// and answered, until it asks nothing more. `wall`: the browser's clock
/// then; `token`: the page holds the Skipper's token. Every request sent.
fn tick(c: &mut SkipperClock, sk: &mut Skipper, now: f64, wall: u64, token: bool) -> Vec<PReq> {
    let mut sent = Vec::new();
    for _ in 0..8 {
        let out = c.step(now, true);
        if out.is_empty() {
            return sent;
        }
        for r in out {
            let (status, body) = sk.call(&r, token.then_some(TOKEN));
            let a = answer(status, &body);
            match r.call {
                PCall::NodeInfo => c.node_info(&a, now, wall, token),
                PCall::SetTime => c.time_set(&a),
                ref other => panic!("not a clock call: {other:?}"),
            }
            sent.push(r);
        }
    }
    panic!("the clock kept asking at {now}: {sent:?}");
}

fn sets(sent: &[PReq]) -> Vec<u64> {
    sent.iter()
        .filter(|r| matches!(r.call, PCall::SetTime))
        .map(|r| {
            assert_eq!((r.method, r.url.as_str()), ("PUT", "/api/time"));
            let j: serde_json::Value = serde_json::from_slice(r.body.as_ref().unwrap()).unwrap();
            j["unix_ms"].as_u64().unwrap()
        })
        .collect()
}

fn gets(sent: &[PReq]) -> usize {
    sent.iter()
        .filter(|r| matches!(r.call, PCall::NodeInfo))
        .inspect(|r| assert_eq!((r.method, r.url.as_str()), ("GET", "/api/node")))
        .count()
}

/// A Skipper whose clock reads `NOW + skew` now.
fn skewed(skew: i64) -> Skipper {
    let mut sk = Skipper::new();
    assert_eq!(sk.view.uptime_ms, UPTIME);
    sk.view.clock_offset_ms = Some(NOW.checked_add_signed(skew).unwrap() - UPTIME);
    sk
}

#[test]
fn the_node_is_read_every_10_s_of_page_time_and_only_while_watched() {
    let mut c = SkipperClock::default();
    for k in 0..200 {
        assert!(c.step(f64::from(k) * 0.5, false).is_empty(), "not watching");
    }
    let mut sk = skewed(0);
    let mut at = Vec::new();
    for k in 0..=70 {
        let now = 100.0 + f64::from(k) * 0.5;
        if gets(&tick(&mut c, &mut sk, now, NOW, true)) > 0 {
            at.push(now);
        }
    }
    assert_eq!(at.first(), Some(&100.0), "at once: {at:?}");
    assert!(at.windows(2).all(|w| w[1] - w[0] >= 10.0 - 1e-9), "{at:?}");
    assert!(at.len() >= 3, "every 10 s over 35 s: {at:?}");
}

#[test]
fn without_the_token_the_page_never_sets_the_clock() {
    for sk in [Skipper::new(), skewed(-60_000), skewed(3_600_000)] {
        let mut sk = sk;
        let mut c = SkipperClock::default();
        let mut sent = Vec::new();
        for k in 0..=240 {
            sent.extend(tick(&mut c, &mut sk, f64::from(k) * 0.5, NOW, false));
        }
        assert!(gets(&sent) > 0);
        assert_eq!(sets(&sent), Vec::<u64>::new());
    }
}

#[test]
fn an_unset_clock_is_set_to_the_browsers_time_and_read_back() {
    let mut sk = Skipper::new();
    assert_eq!(sk.view.clock_offset_ms, None);
    let mut c = SkipperClock::default();
    let sent = tick(&mut c, &mut sk, 50.0, NOW, true);
    assert_eq!(sets(&sent), [NOW], "{sent:?}");
    assert_eq!(gets(&sent), 2, "read, set, then read again at once: {sent:?}");
    assert_eq!(sk.view.unix_ms_at(UPTIME), Some(NOW));
    let w = c.wall(UPTIME).expect("a time once read");
    assert!(w.from_node, "the Skipper says it now");
    // Set and in step: nothing more is sent but the 10 s read.
    let mut more = Vec::new();
    for k in 1..=120 {
        more.extend(tick(&mut c, &mut sk, 50.0 + f64::from(k) * 0.5, NOW, true));
    }
    assert_eq!(sets(&more), Vec::<u64>::new());
}

#[test]
fn a_clock_2000_ms_off_is_left_and_2001_ms_off_is_set() {
    assert_eq!(MAX_SKEW_MS, 2000);
    for (skew, set) in [
        (0, false),
        (-2000, false),
        (2000, false),
        (-2001, true),
        (2001, true),
        (-86_400_000, true),
    ] {
        let mut sk = skewed(skew);
        let mut c = SkipperClock::default();
        let sent = tick(&mut c, &mut sk, 7.0, NOW, true);
        let want: Vec<u64> = if set { vec![NOW] } else { vec![] };
        assert_eq!(sets(&sent), want, "skew {skew} ms");
    }
}

#[test]
fn the_clock_is_set_at_most_every_30_s() {
    assert!((SET_EVERY - 30.0).abs() < 1e-9);
    // A Skipper that forgets the time at once (a restart between): the
    // page would set it at every read without the limit.
    let mut sk = Skipper::new();
    let mut c = SkipperClock::default();
    let mut at = Vec::new();
    for k in 0..=260 {
        let now = 20.0 + f64::from(k) * 0.5;
        let sent = tick(&mut c, &mut sk, now, NOW, true);
        sk.view.clock_offset_ms = None;
        if !sets(&sent).is_empty() {
            at.push(now);
        }
    }
    assert_eq!(at.first(), Some(&20.0), "{at:?}");
    assert!(
        at.windows(2).all(|w| w[1] - w[0] >= 30.0 - 1e-9),
        "at most every 30 s: {at:?}"
    );
    assert!(at.len() >= 3, "but again once 30 s are up: {at:?}");
}

#[test]
fn leaving_the_skipper_starts_the_clock_afresh() {
    let mut sk = Skipper::new();
    let mut c = SkipperClock::default();
    assert_eq!(sets(&tick(&mut c, &mut sk, 0.0, NOW, true)), [NOW]);
    sk.view.clock_offset_ms = None;
    assert!(c.wall(UPTIME).is_some());
    assert_eq!(c.step(4.0, false).len(), 0);
    assert_eq!(c.wall(UPTIME), None, "nothing kept from the Skipper left");
    assert_eq!(c.uptime_now(4.0), None);
    // Back on it 5 s later: read at once, and set at once (the 30 s were
    // the old watch's).
    let sent = tick(&mut c, &mut sk, 5.0, NOW + 5_000, true);
    assert_eq!(gets(&sent), 2, "{sent:?}");
    assert_eq!(sets(&sent), [NOW + 5_000]);
}

#[test]
fn the_time_shown_is_the_skippers_own_else_the_browsers() {
    // The Skipper's own: unix at its uptime, then on with its uptime.
    let mut sk = skewed(0);
    let mut c = SkipperClock::default();
    tick(&mut c, &mut sk, 200.0, NOW + 500, false);
    let w = c.wall(UPTIME + 5_000).unwrap();
    assert!(w.from_node);
    assert_eq!(
        (w.time.as_str(), w.dated.as_str()),
        ("14:03:32", "2026-11-26 14:03:32")
    );
    let w0 = c.wall(UPTIME).unwrap();
    assert_eq!(
        w0.time, "14:03:27",
        "the Skipper's time, not the browser's (500 ms on)"
    );
    // No clock on the Skipper and no token: the browser's clock at the
    // answer stands for the Skipper's uptime then.
    let mut sk = Skipper::new();
    let mut c = SkipperClock::default();
    tick(&mut c, &mut sk, 200.0, NOW, false);
    let w = c.wall(UPTIME + 2_000).unwrap();
    assert!(!w.from_node);
    assert_eq!(w.time, "14:03:29");
    assert_eq!(
        c.uptime_now(203.5),
        Some(UPTIME + 3_500),
        "uptime runs with the page"
    );
    // The tooltip says which clock it is.
    let mine = SkipperClock::tip(&c.wall(UPTIME).unwrap(), "The Skipper's clock");
    let mut sk = skewed(0);
    let mut c2 = SkipperClock::default();
    tick(&mut c2, &mut sk, 200.0, NOW, false);
    let its = SkipperClock::tip(&c2.wall(UPTIME).unwrap(), "The Skipper's clock");
    assert_ne!(mine, its);
    assert!(mine.to_lowercase().contains("browser"), "{mine}");
    assert!(
        mine.contains("2026-11-26 14:03:27") && its.contains("2026-11-26 14:03:27"),
        "{mine} / {its}"
    );
    // Before any reading, nothing to show.
    assert_eq!(SkipperClock::default().wall(UPTIME), None);
}

#[test]
fn a_hostile_answer_is_read_without_harm() {
    for (status, body) in [
        (500, "{}"),
        (200, "not json"),
        (200, "[]"),
        (200, r#"{"uptime_ms": "x", "unix_ms": "y"}"#),
        (200, r#"{"uptime_ms": 100000, "unix_ms": 1e20}"#),
        (200, r#"{"uptime_ms": 100000, "unix_ms": -5}"#),
        (
            200,
            r#"{"uptime_ms": 18446744073709551615, "unix_ms": 18446744073709551615}"#,
        ),
        (
            200,
            r#"{"uptime_ms": 100000, "unix_ms": {"unix_ms": 1795701807000}}"#,
        ),
        (200, r#"{"uptime_ms": -1, "unix_ms": null}"#),
    ] {
        let mut c = SkipperClock::default();
        let first = c.step(0.0, true);
        assert_eq!(first.len(), 1);
        c.node_info(&answer(status, body), 0.0, NOW, true);
        c.time_set(&answer(status, body));
        for r in c.step(1.0, true) {
            if let Some(b) = &r.body {
                let j: serde_json::Value = serde_json::from_slice(b).unwrap();
                assert_eq!(
                    j["unix_ms"].as_u64(),
                    Some(NOW),
                    "only ever the browser's time: {body}"
                );
            }
        }
        if let Some(w) = c.wall(100_000) {
            assert_eq!(w.time.len(), 8, "{body}: {w:?}");
        }
    }
}

/// The toolbar: a Skipper's page shows its time of day, a box's keeps
/// `t=…s`.
#[test]
fn a_skippers_toolbar_shows_its_time_and_a_boxs_keeps_seconds() {
    let bench = crate::box_tests::Bench::new();
    let (state, _) = crate::box_tests::watch(&bench);
    let mut h = Harness::new(App::boxed());
    h.app.boxv.as_mut().unwrap().state = state;
    h.wait(0.3);
    assert!(h.drew("t="), "a box page counts seconds");
    // The same page on a Skipper with its clock set: the Skipper's time.
    let mut sk = skewed(0);
    let r = PReq::get(PCall::NodeInfo, "/api/node".into());
    let (status, body) = sk.call(&r, Some(TOKEN));
    // A Skipper's page: the page watches a remote node (as the sign-in
    // panel leaves it), with its token held.
    {
        let r = h.app.review.as_mut().unwrap();
        r.remote = Some("http://192.168.33.56:8080".into());
        r.token_held = true;
    }
    h.wait(0.1);
    let now = h.time;
    {
        let v = h.app.boxv.as_mut().unwrap();
        assert!(v.watches_node);
        v.state.scan = None;
        v.clock.node_info(&answer(status, &body), now, NOW, true);
    }
    h.wait(0.3);
    assert!(h.drew("14:03:27"), "the Skipper's time of day");
    assert!(!h.drew("t="), "not seconds since it started");
}
