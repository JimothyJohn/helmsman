//! The floor screen: the view model (signals to tiles), staleness, and the
//! kiosk view driven headlessly through egui, asserting on what it paints;
//! then against a real box API with a module driving its tiles.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, RawInput, Rect, Shape, Vec2};
use helmsman_iec::{ImageSizes, Value, compile};
use helmsman_plc::api::{
    Api, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, ModuleInfo, Publisher,
    Screen, Shared,
};
use helmsman_plc::screen::{self as layout_file, Colour};
use helmsman_plc::{Module, Runtime, SignalTable, Window};
use serde_json::json;

use crate::andon::{AndonState, Face, Health, REOPEN_AFTER, STALE_AFTER, shift, tiles};
use crate::app::{App, Mode};
use crate::box_tests::sse;
use crate::remote::{Cmd, Fetch, Msg, SignalRow, Stream};

const LAYOUT: &str = "\
helmsman-screen 1
title Hall 2 andon
grid 3 1
code line 0 grey Idle
code line 1 green Running
code line 3 red Down: call maintenance
code light 0 grey Off
code light 1 yellow Material low
tile line1 0,0 line line1_state Line 1
tile line2 1,0 line line2_state Line 2
tile low 2,0 light low_light Material
value line1 line1_parts parts
";

fn layout() -> layout_file::Layout {
    layout_file::parse(LAYOUT).unwrap()
}

fn row(name: &str, value: Option<Value>) -> SignalRow {
    SignalRow {
        name: name.into(),
        dir: "out".into(),
        unit: None,
        value,
        quality: "commanded".into(),
        point_error: None,
    }
}

fn signals(rows: &[(&str, Option<Value>)]) -> HashMap<String, SignalRow> {
    rows.iter().map(|(n, v)| (n.to_string(), row(n, *v))).collect()
}

/// A `values` event as the API renders it.
fn values_event(rows: &[(&str, serde_json::Value)]) -> Msg {
    let sigs: Vec<serde_json::Value> = rows
        .iter()
        .map(|(n, v)| json!({ "name": n, "dir": "out", "value": v, "quality": "commanded" }))
        .collect();
    let data = json!({ "seq": 1, "signals": sigs, "modules": [] }).to_string();
    Msg::Event(Stream::Values, "values".into(), data)
}

fn screen_msg() -> Msg {
    Msg::Got(
        Fetch::Screen,
        Ok(json!({ "source": LAYOUT, "layout": {}, "palette": {} }).to_string()),
    )
}

fn running() -> Msg {
    values_event(&[
        ("line1_state", json!(1)),
        ("line1_parts", json!(1234)),
        ("line2_state", json!(3)),
        ("low_light", json!(true)),
    ])
}

#[test]
fn signals_become_tiles() {
    let l = layout();
    let sigs = signals(&[
        ("line1_state", Some(Value::Int(1))),
        ("line1_parts", Some(Value::Int(1234))),
        ("line2_state", Some(Value::Int(7))),
        ("low_light", Some(Value::Bool(true))),
    ]);
    let t = tiles(&l, &sigs, true);
    assert_eq!(t.len(), 3);
    assert_eq!(
        t[0].face,
        Face::Known {
            colour: Colour::Green,
            text: "Running".into()
        }
    );
    assert_eq!(t[0].value.as_deref(), Some("1234 parts"));
    assert_eq!((t[0].title.as_str(), t[0].cell), ("Line 1", [0, 0, 1, 1]));
    assert_eq!(
        t[1].face,
        Face::Unknown { code: 7 },
        "a code the layout does not name"
    );
    assert_eq!(
        t[2].face,
        Face::Known {
            colour: Colour::Yellow,
            text: "Material low".into()
        },
        "a BOOL is code 1"
    );

    // A signal the box does not have, or that is missing, is no data.
    let mut partial = signals(&[("line1_state", Some(Value::Int(1)))]);
    partial.insert(
        "line2_state".into(),
        SignalRow {
            quality: "missing".into(),
            ..row("line2_state", Some(Value::Int(1)))
        },
    );
    let t = tiles(&l, &partial, true);
    assert_eq!(t[0].value, None, "no parts signal, no number");
    assert_eq!((&t[1].face, &t[2].face), (&Face::NoData, &Face::NoData));

    // Not fresh: nothing shows a colour, nothing shows a number.
    for t in tiles(&l, &sigs, false) {
        assert_eq!((t.face, t.value), (Face::NoData, None), "{}", t.id);
    }
    // A real where a code belongs, an integer past 64 bits: no data.
    let odd = signals(&[
        ("line1_state", Some(Value::Real(1.0))),
        ("line2_state", Some(Value::Int(i128::MAX))),
    ]);
    let t = tiles(&l, &odd, true);
    assert_eq!((&t[0].face, &t[1].face), (&Face::NoData, &Face::NoData));
}

#[test]
fn the_shift_line_comes_from_the_calendars_signals() {
    let b = |x| Some(Value::Bool(x));
    let s = signals(&[
        ("in_shift", b(true)),
        ("shift_id", Some(Value::Int(2))),
        ("on_break", b(false)),
    ]);
    assert_eq!(shift(&s, true).as_deref(), Some("SHIFT 2"));
    assert_eq!(shift(&s, false), None, "stale: nothing");
    let s = signals(&[
        ("in_shift", b(true)),
        ("shift_id", Some(Value::Int(2))),
        ("on_break", b(true)),
    ]);
    assert_eq!(shift(&s, true).as_deref(), Some("SHIFT 2 · BREAK"));
    let s = signals(&[("in_shift", b(false)), ("planned_down", b(false))]);
    assert_eq!(shift(&s, true).as_deref(), Some("NO SHIFT"));
    let s = signals(&[("in_shift", b(false)), ("planned_down", b(true))]);
    assert_eq!(shift(&s, true).as_deref(), Some("PLANNED DOWNTIME"));
    assert_eq!(shift(&signals(&[]), true), None, "no calendar, no line");
}

#[test]
fn a_lost_or_quiet_box_never_leaves_the_last_colour_up() {
    let mut s = AndonState::new();
    assert_eq!(
        s.poll(0.0),
        [
            Cmd::Get(Fetch::Screen, "/api/screen".into()),
            Cmd::Listen(Stream::Values, "/api/events".into())
        ]
    );
    s.apply(screen_msg(), 0.1);
    assert!(s.layout.is_some(), "{:?}", s.problem);
    assert_eq!(s.health(0.1), Health::Connecting);
    assert!(s.tiles(0.1).iter().all(|t| t.face == Face::NoData), "nothing yet");
    s.apply(Msg::Opened(Stream::Values), 0.2);
    s.apply(running(), 0.3);
    assert_eq!(s.health(0.3), Health::Live);
    assert_eq!(s.banner(0.3), None);
    assert!(matches!(
        s.tiles(0.3)[0].face,
        Face::Known {
            colour: Colour::Green,
            ..
        }
    ));

    // Quiet: the stream is open but nothing new comes.
    let late = 0.3 + STALE_AFTER + 0.5;
    assert!(matches!(s.health(late), Health::Stale(_)));
    assert!(
        s.tiles(late)
            .iter()
            .all(|t| t.face == Face::NoData && t.value.is_none())
    );
    assert_eq!(s.banner(late).as_deref(), Some("NO DATA FROM THE BOX FOR 3s"));
    // ... and long quiet reopens the stream, once per period.
    assert_eq!(s.poll(late), []);
    let reopen = 0.3 + REOPEN_AFTER + 0.1;
    assert_eq!(
        s.poll(reopen),
        [Cmd::Listen(Stream::Values, "/api/events".into())]
    );
    assert_eq!(s.poll(reopen + 1.0), [], "not again at once");

    // Back.
    s.apply(running(), reopen + 2.0);
    assert_eq!(s.health(reopen + 2.0), Health::Live);

    // Lost: at once, and the values are forgotten, not just hidden.
    let t = reopen + 3.0;
    s.apply(Msg::Failed(Stream::Values), t);
    assert_eq!(s.health(t), Health::Lost(1));
    assert!(s.signals.is_empty());
    assert!(s.tiles(t).iter().all(|t| t.face == Face::NoData));
    assert_eq!(s.banner(t).as_deref(), Some("CONNECTION LOST · RECONNECTING"));
    let mut now = t;
    for _ in 0..2 {
        now += 100.0;
        assert_eq!(s.poll(now), [Cmd::Listen(Stream::Values, "/api/events".into())]);
        s.apply(Msg::Failed(Stream::Values), now);
    }
    assert_eq!(s.banner(now).as_deref(), Some("BOX UNREACHABLE · RETRYING"));
    // Reopened but no event yet: still nothing shown.
    s.apply(Msg::Opened(Stream::Values), now + 1.0);
    assert!(s.tiles(now + 1.0).iter().all(|t| t.face == Face::NoData));
}

#[test]
fn a_box_without_a_screen_or_with_a_bad_one_says_so() {
    let mut s = AndonState::new();
    s.poll(0.0);
    s.apply(Msg::Got(Fetch::Screen, Err("HTTP 404".into())), 0.0);
    assert!(
        s.problem.as_deref().unwrap().contains("--screen"),
        "{:?}",
        s.problem
    );
    assert_eq!(s.poll(1.0), [], "retried after the backoff");
    assert_eq!(s.poll(2.0), [Cmd::Get(Fetch::Screen, "/api/screen".into())]);
    let bad = json!({ "source": "helmsman-screen 1\ngrid 0 0\n" }).to_string();
    s.apply(Msg::Got(Fetch::Screen, Ok(bad)), 2.0);
    assert!(
        s.problem.as_deref().unwrap().contains("does not parse"),
        "{:?}",
        s.problem
    );
    s.apply(Msg::Got(Fetch::Screen, Ok("<html>".into())), 3.0);
    assert!(s.layout.is_none());
    s.apply(screen_msg(), 4.0);
    assert_eq!(s.problem, None);
    // Garbage on the values stream changes nothing.
    s.apply(Msg::Event(Stream::Values, "values".into(), "{".into()), 5.0);
    s.apply(Msg::Event(Stream::Values, "other".into(), "{}".into()), 5.0);
    assert_eq!(s.health(5.0), Health::Connecting);
}

/// What one frame painted: texts, and fills of rectangles.
#[derive(Default, Debug)]
struct Painted {
    texts: Vec<String>,
    fills: Vec<Color32>,
}

fn collect(shape: &Shape, out: &mut Painted) {
    match shape {
        Shape::Text(t) => out.texts.push(t.galley.text().to_string()),
        Shape::Rect(r) => out.fills.push(r.fill),
        Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
        _ => {}
    }
}

struct Kiosk {
    ctx: egui::Context,
    app: App,
}

impl Kiosk {
    fn new() -> Self {
        let mut app = App::with_project(crate::example::project());
        app.mode = Mode::Andon;
        Self {
            ctx: egui::Context::default(),
            app,
        }
    }

    fn feed(&mut self, msgs: Vec<Msg>, now: f64) {
        let ctx = self.ctx.clone();
        self.app
            .andon
            .get_or_insert_with(crate::andon::AndonView::new)
            .step(&ctx, msgs, now);
    }

    fn frame(&mut self, now: f64, size: Vec2) -> Painted {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(now),
            ..Default::default()
        };
        let Self { ctx, app } = self;
        let mut out = ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear();
        let mut p = Painted::default();
        for c in &out.shapes {
            collect(&c.shape, &mut p);
        }
        p
    }
}

fn fill(c: Colour) -> Color32 {
    let [r, g, b] = c.fill();
    Color32::from_rgb(r, g, b)
}

#[test]
fn the_kiosk_draws_big_tiles_and_never_a_stale_colour() {
    let mut k = Kiosk::new();
    let p = k.frame(0.0, Vec2::new(1920.0, 1080.0));
    assert!(p.texts.iter().any(|t| t == "Loading the floor screen…"), "{p:?}");
    let sent = k.app.andon.as_ref().unwrap().sent.clone();
    assert!(
        sent.contains(&Cmd::Get(Fetch::Screen, "/api/screen".into())),
        "{sent:?}"
    );

    k.feed(vec![screen_msg(), Msg::Opened(Stream::Values), running()], 0.5);
    let p = k.frame(0.5, Vec2::new(1920.0, 1080.0));
    for want in [
        "Hall 2 andon",
        "Line 1",
        "Running",
        "1234 parts",
        "Down: call maintenance",
        "Material low",
    ] {
        assert!(p.texts.iter().any(|t| t == want), "{want}: {p:?}");
    }
    for c in [Colour::Green, Colour::Red, Colour::Yellow] {
        assert!(p.fills.contains(&fill(c)), "{c:?}: {p:?}");
    }
    assert!(!p.texts.iter().any(|t| t.contains("NO DATA")), "{p:?}");

    // The box goes away: every tile says so, and no state colour is left.
    k.feed(vec![Msg::Failed(Stream::Values)], 1.0);
    let p = k.frame(1.0, Vec2::new(1920.0, 1080.0));
    assert_eq!(p.texts.iter().filter(|t| *t == "NO DATA").count(), 3, "{p:?}");
    assert!(
        p.texts.iter().any(|t| t == "CONNECTION LOST · RECONNECTING"),
        "{p:?}"
    );
    for c in [Colour::Green, Colour::Yellow] {
        assert!(!p.fills.contains(&fill(c)), "{c:?} left up: {p:?}");
    }
    assert!(
        !p.texts.iter().any(|t| t == "Running" || t == "1234 parts"),
        "{p:?}"
    );

    // Back, then quiet: stale without any failure.
    k.feed(vec![Msg::Opened(Stream::Values), running()], 2.0);
    let p = k.frame(2.0, Vec2::new(1920.0, 1080.0));
    assert!(p.texts.iter().any(|t| t == "Running"), "{p:?}");
    let p = k.frame(2.0 + STALE_AFTER + 1.0, Vec2::new(1920.0, 1080.0));
    assert_eq!(p.texts.iter().filter(|t| *t == "NO DATA").count(), 3, "{p:?}");
    assert!(
        p.texts.iter().any(|t| t.starts_with("NO DATA FROM THE BOX")),
        "{p:?}"
    );
}

#[test]
fn the_kiosk_survives_any_window_size() {
    let mut k = Kiosk::new();
    k.feed(vec![screen_msg(), Msg::Opened(Stream::Values), running()], 0.0);
    for (w, h) in [
        (1.0, 1.0),
        (320.0, 240.0),
        (800.0, 1280.0),
        (3840.0, 2160.0),
        (10.0, 4000.0),
    ] {
        let p = k.frame(0.1, Vec2::new(w, h));
        assert!(p.texts.iter().any(|t| t == "Running"), "{w}x{h}: {p:?}");
    }
}

/// A real runtime driving screen tiles through a module, the real API over
/// it serving the layout and the values stream.
struct Bench {
    api: Option<Api>,
    stop: Arc<AtomicBool>,
    scan: Option<JoinHandle<()>>,
    addr: std::net::SocketAddr,
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.api.take();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.scan.take() {
            let _ = t.join();
        }
    }
}

const MAP: &str = "\
out line1_state int  screen line1 safe=3
out line1_parts int  screen line1
out line2_state int  screen line2
out low_light   bool screen low
";

const LINES: &str = "
PROGRAM lines
VAR_OUTPUT line1_state, line2_state : INT; line1_parts : DINT; low_light : BOOL; END_VAR
  line1_state := 1;
  line1_parts := 1234;
  line2_state := 3;
  low_light := TRUE;
END_PROGRAM";

impl Bench {
    fn new() -> Self {
        let parsed = helmsman_io::text::parse(MAP).unwrap();
        let screen = Screen {
            source: LAYOUT.into(),
            layout: layout(),
        };
        screen.layout.check(&parsed.map).unwrap();
        let table = SignalTable::new(&parsed.map, &parsed.safe).unwrap();
        let module = Module {
            name: "lines".into(),
            program: compile(LINES, ImageSizes { i: 0, q: 0, m: 0 }).unwrap(),
            period: Duration::from_millis(2),
            i: Window::new(0, 0),
            q: Window::new(0, 0),
            params: Vec::new(),
        };
        let mut rt = Runtime::with_signals(0, 0, vec![module], &table).unwrap();
        let modules = vec![ModuleInfo::of(&rt, "lines", "lines".into(), LINES.into(), None).unwrap()];
        let catalog = Catalog {
            signals: Catalog::signals(&table, Some(&parsed.map), &modules),
            modules,
        };
        let live = Arc::new(Live::default());
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: "0".repeat(64),
            limits: Limits::default(),
            stream_every: Duration::from_millis(10),
            memory: 0,
            log: Arc::new(|_: &str| {}),
        };
        let shared = Shared {
            catalog,
            live: Arc::clone(&live),
            devices: Arc::new(Devices::new(Vec::<String>::new())),
            audit: Box::new(MemAudit::new(10)),
            proposals: Box::new(MemProposals::new(10)),
            running: None,
            snapshots: None,
            approval: None,
            chat: None,
        };
        let extras = Extras {
            ui: None,
            screen: Some(screen),
        };
        let api = Api::start_with(cfg, shared, extras).unwrap();
        let addr = api.addr();
        let stop = Arc::new(AtomicBool::new(false));
        let table = Arc::new(Mutex::new(table));
        let scan = {
            let stop = Arc::clone(&stop);
            let mut publisher = Publisher::new(live, Duration::from_millis(5));
            thread::spawn(move || {
                let t0 = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    let now = t0.elapsed();
                    let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
                    rt.tick_with(now, &[], &mut t);
                    publisher.publish(now, &rt, &t);
                    drop(t);
                    thread::sleep(Duration::from_millis(1));
                }
            })
        };
        Self {
            api: Some(api),
            stop,
            scan: Some(scan),
            addr,
        }
    }
}

/// A browser's two requests, over plain sockets.
struct Client(std::net::SocketAddr);

impl Client {
    fn get(&self, path: &str) -> (u16, String) {
        let mut s = TcpStream::connect(self.0).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: box\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        (head[9..12].parse().unwrap(), body.to_string())
    }

    fn events(&self, path: &str, n: usize) -> Vec<(String, String)> {
        let mut s = TcpStream::connect(self.0).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: box\r\n\r\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut buf, mut chunk) = (String::new(), [0u8; 8192]);
        loop {
            let events = sse(&buf);
            if events.len() >= n {
                return events.into_iter().take(n).collect();
            }
            assert!(Instant::now() < deadline, "{n} events from {path}: {buf:.300}");
            if let Ok(k) = s.read(&mut chunk) {
                assert!(k > 0, "{path} closed: {buf:.300}");
                buf.push_str(&String::from_utf8_lossy(&chunk[..k]));
            }
        }
    }
}

#[test]
fn the_floor_screen_shows_what_a_live_box_drives() {
    let b = Bench::new();
    let bench = Client(b.addr);
    let mut s = AndonState::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut now = 0.0;
    loop {
        for cmd in s.poll(now) {
            match cmd {
                Cmd::Get(what, url) => {
                    let (status, body) = bench.get(&url);
                    let r = if status == 200 {
                        Ok(body)
                    } else {
                        Err(format!("HTTP {status}"))
                    };
                    s.apply(Msg::Got(what, r), now);
                }
                Cmd::Listen(stream, url) => {
                    s.apply(Msg::Opened(stream), now);
                    for (name, data) in bench.events(&url, 1) {
                        s.apply(Msg::Event(stream, name, data), now);
                    }
                }
                Cmd::Close(_) => {}
            }
        }
        let t = s.tiles(now);
        if t.first().is_some_and(|t| t.face != Face::NoData) {
            assert_eq!(
                t[0].face,
                Face::Known {
                    colour: Colour::Green,
                    text: "Running".into()
                },
                "{t:?}"
            );
            assert_eq!(t[0].value.as_deref(), Some("1234 parts"));
            assert_eq!(
                t[1].face,
                Face::Known {
                    colour: Colour::Red,
                    text: "Down: call maintenance".into()
                }
            );
            assert!(matches!(
                t[2].face,
                Face::Known {
                    colour: Colour::Yellow,
                    ..
                }
            ));
            return;
        }
        assert!(Instant::now() < deadline, "no live tiles: {s:?}");
        // The next values event, as the open stream would deliver it.
        for (name, data) in bench.events("/api/events", 1) {
            s.apply(Msg::Event(Stream::Values, name, data), now);
        }
        now += 0.1;
    }
}
