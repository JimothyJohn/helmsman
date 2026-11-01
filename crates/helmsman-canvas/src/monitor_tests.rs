//! The Monitor, headless: its tiles against the simulation and against a
//! real box API scanning `programs/line.st`, tapping in and back, stale
//! data going grey, phone routing, the URL, and that it never writes.

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use helmsman_diagram::Scan;
use helmsman_iec::Value;

use crate::app::{App, Screen, Source, wants_monitor, wants_sim};
use crate::box_tests::{Bench, DEMO};
use crate::monitor::{Feed, Group, NARROW, STALE_AFTER, Shape, Tile, TileKind};
use crate::playground::EXAMPLES;
use crate::remote::{Cmd, Fetch, Msg, Stream};
use crate::review::Method;
use crate::st_view::TAP;

/// The box's demo line: two feeders, two stations of two motors each.
const LINE: &str = include_str!("../../helmsman-plc/programs/line.st");

struct Harness {
    ctx: egui::Context,
    app: App,
    time: f64,
    size: Vec2,
}

impl Harness {
    fn new(app: App, size: Vec2) -> Self {
        let mut h = Self {
            ctx: egui::Context::default(),
            app,
            time: 0.0,
            size,
        };
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        self.time += 1.0 / 60.0;
        let Self { ctx, app, .. } = self;
        let mut out = ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear();
    }

    fn click(&mut self, at: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
    }

    fn rect(&self, key: &str) -> Rect {
        self.app.chrome.rect(key).unwrap_or_else(|| {
            let keys: Vec<&String> = self.app.chrome.hits.iter().map(|(k, _)| k).collect();
            panic!("no {key} on screen: {keys:?}")
        })
    }

    fn press(&mut self, key: &str) {
        let at = self.rect(key).center();
        self.click(at);
    }

    fn groups(&mut self) -> Vec<Group> {
        let now = self.time;
        let App {
            monitor,
            boxv,
            sim,
            source,
            ..
        } = &mut self.app;
        let feed: &dyn Feed = match source {
            Source::Box => &boxv.as_ref().unwrap().state,
            Source::Sim => sim.as_ref().unwrap(),
        };
        monitor.groups(feed, now)
    }
}

fn keys(g: &Group) -> Vec<&str> {
    g.tiles.iter().map(|t| t.key.as_str()).collect()
}

fn tile<'a>(g: &'a Group, key: &str) -> &'a Tile {
    g.tiles
        .iter()
        .find(|t| t.key == key)
        .unwrap_or_else(|| panic!("no {key} in {:?}", keys(g)))
}

fn names(t: &Tile) -> Vec<&str> {
    t.lamps.iter().map(|l| l.name.as_str()).collect()
}

/// The simulation running the "Line of stations" example, on the Monitor.
fn sim_line() -> Harness {
    sim_line_at(Vec2::new(1400.0, 860.0))
}

fn sim_line_at(size: Vec2) -> Harness {
    let mut app = App::sim();
    app.open_monitor();
    let mut h = Harness::new(app, size);
    let k = EXAMPLES
        .iter()
        .position(|e| e.name.starts_with("Line of stations"))
        .unwrap();
    let s = h.app.sim.as_mut().unwrap();
    s.load(EXAMPLES[k].src, "the line");
    s.session.set_running(true);
    for _ in 0..30 {
        h.frame(vec![]);
    }
    h
}

#[test]
fn the_shape_is_read_in_declaration_order() {
    let s = Shape::of(LINE);
    let top = &s.scopes[0];
    assert_eq!(top.name, "line");
    let kids: Vec<&str> = top.kids.iter().map(|&k| s.scopes[k].name.as_str()).collect();
    assert_eq!(
        kids,
        ["feed1", "feed2", "s1", "s2"],
        "standard blocks (TON) are not tiles"
    );
    let outs: Vec<&str> = top.outs.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        outs,
        ["station1_busy", "station2_busy", "alarm"],
        "located %Q outputs; not the %I input"
    );
    let s1 = &s.scopes[s.find("S1.").unwrap()];
    assert_eq!(s1.fb, "station");
    assert_eq!(
        s1.outs,
        [
            ("busy".into(), "S1.BUSY".into()),
            ("alarm".into(), "S1.ALARM".into())
        ]
    );
    let infeed: Vec<&str> = s1.kids.iter().map(|&k| s.scopes[k].prefix.as_str()).collect();
    assert_eq!(infeed, ["S1.INFEED.", "S1.OUTFEED."]);
    assert_eq!(
        Shape::of("PROGRAM"),
        Shape::default(),
        "a broken source has no shape"
    );
}

#[test]
fn the_simulations_line_has_a_tile_per_station_with_the_scans_values() {
    let mut h = sim_line();
    let groups = h.groups();
    assert_eq!(groups.len(), 1);
    let g = &groups[0];
    assert_eq!(
        keys(g),
        ["status line", "outputs line/", "tile line/S1.", "tile line/S2."]
    );
    assert_eq!(
        tile(g, "status line").kind,
        TileKind::Status(crate::monitor::Run::Running)
    );
    assert_eq!(
        names(tile(g, "outputs line/")),
        ["station1_busy", "station2_busy", "alarm"]
    );
    let trace = h.app.sim.as_ref().unwrap().session.trace.clone().unwrap();
    for t in &g.tiles {
        assert_eq!(t.stale, None, "{t:?}");
        for l in &t.lamps {
            assert!(l.value.is_some(), "{l:?}");
            assert_eq!(l.value, Scan::get(&trace, &l.path), "{l:?}");
        }
    }
    assert_eq!(names(tile(g, "tile line/S1.")), ["busy", "alarm"]);
    // Stopping the program greys it: a held value is not a live one.
    h.app.sim.as_mut().unwrap().session.set_running(false);
    let g = &h.groups()[0];
    assert!(g.tiles[1..].iter().all(|t| t.stale.is_some()), "{g:?}");
}

#[test]
fn a_tap_goes_into_an_instance_and_back_and_crumbs_go_up() {
    let mut h = sim_line();
    h.press("tile line/S1.");
    assert_eq!(h.app.monitor.at, Some(("line".into(), "S1.".into())));
    let g = &h.groups()[0];
    assert_eq!(
        keys(g),
        [
            "status line",
            "outputs line/S1.",
            "tile line/S1.INFEED.",
            "tile line/S1.OUTFEED."
        ]
    );
    assert_eq!(names(tile(g, "tile line/S1.INFEED.")), ["running", "tripped"]);
    h.frame(vec![]);
    h.press("tile line/S1.INFEED.");
    assert_eq!(h.app.monitor.at, Some(("line".into(), "S1.INFEED.".into())));
    let crumbs: Vec<String> = h.app.monitor.crumbs().into_iter().map(|(t, _)| t).collect();
    assert_eq!(crumbs, ["All programs", "line", "s1", "infeed"]);
    h.frame(vec![]);
    h.press("monitor back");
    assert_eq!(h.app.monitor.at, Some(("line".into(), "S1.".into())));
    h.frame(vec![]);
    h.press("monitor crumb 0");
    assert_eq!(h.app.monitor.at, None);
    // The full diagram is one tap away, at the level shown.
    h.frame(vec![]);
    h.press("tile line/S2.");
    h.frame(vec![]);
    h.press("monitor diagram");
    assert_eq!(h.app.screen, Screen::Editor);
    let v = h.app.sim.as_ref().unwrap().viewer.as_ref().unwrap();
    assert_eq!(v.scopes[v.scope].prefix, "S2.");
    h.frame(vec![]);
    h.press("monitor");
    assert_eq!(h.app.screen, Screen::Monitor);
}

#[test]
fn tiles_are_big_enough_to_tap_and_scale_from_a_phone_to_a_tv() {
    let mut heights = Vec::new();
    for size in [
        Vec2::new(360.0, 740.0),
        Vec2::new(1920.0, 1080.0),
        Vec2::new(3840.0, 2160.0),
    ] {
        let h = sim_line_at(size);
        for key in [
            "status line",
            "outputs line/",
            "tile line/S1.",
            "tile line/S2.",
            "monitor back",
            "monitor diagram",
        ] {
            let r = h.rect(key);
            assert!(r.height() >= TAP && r.width() >= TAP, "{key} at {size:?}: {r:?}");
            assert!(
                r.min.x >= 0.0 && r.max.x <= size.x,
                "{key} off screen at {size:?}: {r:?}"
            );
        }
        let s1 = h.rect("tile line/S1.");
        if size.x < NARROW {
            // One column on a phone, the full width.
            assert!(s1.width() > size.x * 0.8, "{s1:?}");
            assert!(h.rect("tile line/S2.").min.y > s1.max.y, "stacked");
        } else {
            assert!(
                (h.rect("tile line/S2.").min.y - s1.min.y).abs() < 1.0,
                "side by side at {size:?}"
            );
        }
        heights.push(s1.height());
    }
    assert!(heights[2] > heights[1] * 1.5, "bigger on a 4K TV: {heights:?}");
}

/// Carries out what the app asked the box for, against `b`: fetches,
/// and a stream's next `n` events.
fn serve(h: &mut Harness, b: &Bench, sent: &mut Vec<Cmd>) {
    let now = h.time;
    let cmds = std::mem::take(&mut h.app.boxv.as_mut().unwrap().sent);
    sent.extend(cmds.iter().cloned());
    let state = &mut h.app.boxv.as_mut().unwrap().state;
    for c in cmds {
        match c {
            Cmd::Get(what, url) => {
                let (status, body) = b.get(&url);
                let r = if status == 200 {
                    Ok(body)
                } else {
                    Err(format!("HTTP {status}"))
                };
                state.apply(Msg::Got(what, r), now);
            }
            Cmd::Listen(stream, url) => {
                state.apply(Msg::Opened(stream), now);
                let n = if stream == Stream::Traces { 4 } else { 1 };
                for (name, data) in b.events(&url, n) {
                    state.apply(Msg::Event(stream, name, data), now);
                }
            }
            Cmd::Close(_) => {}
        }
    }
}

/// A box running the cell and the line, its page on the Monitor, fed
/// until the line's tiles have values. Returns what the app sent.
fn box_line() -> (Bench, Harness, Vec<Cmd>) {
    let b = Bench::with(&[("cell", DEMO), ("line", LINE)]);
    let mut app = App::boxed();
    app.open_monitor();
    let mut h = Harness::new(app, Vec2::new(1920.0, 1080.0));
    let mut sent = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        serve(&mut h, &b, &mut sent);
        // The streams' next events, as an open EventSource delivers them.
        let now = h.time;
        let state = &mut h.app.boxv.as_mut().unwrap().state;
        for (name, data) in b.events("/api/events", 1) {
            state.apply(Msg::Event(Stream::Values, name, data), now);
        }
        for (name, data) in b.events(crate::remote::TRACES_URL, 2) {
            state.apply(Msg::Event(Stream::Traces, name, data), now);
        }
        h.frame(vec![]);
        let ready = h
            .groups()
            .iter()
            .any(|g| g.program == "line" && g.tiles.len() > 2);
        if ready {
            serve(&mut h, &b, &mut sent);
            h.frame(vec![]);
            return (b, h, sent);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no line tiles: {:?}",
            h.groups()
        );
    }
}

#[test]
fn the_box_line_has_a_group_per_program_with_the_scans_values() {
    let (_b, mut h, sent) = box_line();
    assert!(
        sent.contains(&Cmd::Listen(Stream::Traces, crate::remote::TRACES_URL.into())),
        "one stream for every module: {sent:?}"
    );
    assert!(
        !sent.iter().any(|c| matches!(c, Cmd::Listen(Stream::Trace, _))),
        "no per-module stream while the Monitor shows: {sent:?}"
    );
    let groups = h.groups();
    let programs: Vec<&str> = groups.iter().map(|g| g.program.as_str()).collect();
    assert_eq!(programs, ["cell", "line"], "one group per program, in scan order");
    let g = &groups[1];
    assert_eq!(
        keys(g),
        [
            "status line",
            "outputs line/",
            "tile line/FEED1.",
            "tile line/FEED2.",
            "tile line/S1.",
            "tile line/S2."
        ]
    );
    assert_eq!(
        tile(g, "status line").kind,
        TileKind::Status(crate::monitor::Run::Running)
    );
    assert!(
        tile(g, "status line").sub.contains("scan 2 ms"),
        "{:?}",
        tile(g, "status line")
    );
    let scan = h.app.boxv.as_ref().unwrap().state.programs["line"]
        .scan
        .clone()
        .unwrap();
    for t in &g.tiles {
        assert_eq!(t.stale, None, "{t:?}");
        for l in &t.lamps {
            assert!(matches!(l.value, Some(Value::Bool(_))), "{l:?}");
            assert_eq!(l.value, scan.get(&l.path), "{l:?}");
        }
    }
    assert_eq!(names(tile(g, "tile line/FEED1.")), ["part"]);
    // The cell's motor is a user block too.
    assert_eq!(keys(&groups[0]), ["status cell", "outputs cell/", "tile cell/M."]);
}

#[test]
fn stale_data_greys_out_after_the_stream_stops() {
    let (_b, mut h, _) = box_line();
    assert!(h.groups()[1].tiles.iter().all(|t| t.stale.is_none()));
    // Nothing arrives for longer than the limit: grey, with the age.
    let end = h.time + STALE_AFTER + 1.0;
    while h.time < end {
        h.frame(vec![]);
    }
    let g = &h.groups()[1];
    for t in &g.tiles {
        let s = t
            .stale
            .as_deref()
            .unwrap_or_else(|| panic!("{t:?} still coloured"));
        assert!(s.starts_with("stale"), "{s}");
    }
    assert!(
        tile(g, "tile line/S1.").stale.as_deref().unwrap().contains(" s"),
        "the age"
    );
    assert!(
        h.app.chrome.rect("monitor banner").is_some(),
        "the red NO DATA banner"
    );
    // The values stream fails: still grey, and it says so.
    let now = h.time;
    let state = &mut h.app.boxv.as_mut().unwrap().state;
    state.apply(Msg::Failed(Stream::Values), now);
    h.frame(vec![]);
    let g = &h.groups()[1];
    assert!(
        g.tiles
            .iter()
            .all(|t| t.stale.as_deref().is_some_and(|s| s.contains("box"))),
        "{g:?}"
    );
}

#[test]
fn the_monitor_never_writes() {
    let (b, mut h, mut sent) = box_line();
    // Every tile, crumb and button the Monitor draws, tapped.
    for _ in 0..3 {
        let keys: Vec<String> = h
            .app
            .chrome
            .hits
            .iter()
            .map(|(k, _)| k.clone())
            .filter(|k| {
                k.starts_with("tile ")
                    || k.starts_with("monitor ")
                    || k.starts_with("outputs ")
                    || k.starts_with("status ")
            })
            .filter(|k| k != "monitor diagram")
            .collect();
        for k in keys {
            if h.app.chrome.rect(&k).is_some() {
                h.press(&k);
                serve(&mut h, &b, &mut sent);
                h.frame(vec![]);
            }
        }
    }
    assert_ne!(sent.len(), 0);
    for c in &sent {
        assert!(
            matches!(c, Cmd::Get(Fetch::Modules | Fetch::Source(_) | Fetch::Module(_), u) if u.starts_with("/api/"))
                || matches!(c, Cmd::Listen(..) | Cmd::Close(_)),
            "a write from the Monitor: {c:?}"
        );
    }
    // Projects and forces: reads at most, never a save, deploy or force.
    let psent = &h.app.boxv.as_ref().unwrap().psent;
    assert!(
        psent.iter().all(|r| r.method == "GET" && r.body.is_none()),
        "{psent:?}"
    );
    let review = &h.app.review.as_ref().unwrap().sent;
    assert!(review.iter().all(|r| r.method == Method::Get), "{review:?}");
    // In the simulation: no input moved, nothing sent to Claude.
    let mut h = sim_line();
    let before: Vec<_> = h
        .app
        .sim
        .as_ref()
        .unwrap()
        .session
        .inputs()
        .map(|p| p.value)
        .collect();
    for k in [
        "tile line/S1.",
        "outputs line/S1.",
        "monitor back",
        "tile line/S2.",
        "monitor crumb 0",
        "status line",
    ] {
        h.press(k);
        h.frame(vec![]);
    }
    let s = h.app.sim.as_ref().unwrap();
    let after: Vec<_> = s.session.inputs().map(|p| p.value).collect();
    assert_eq!(before, after);
    assert_eq!(s.link.sent.len(), 0, "nothing to Claude");
}

#[test]
fn a_phone_opens_on_the_monitor_and_a_desk_on_the_editor() {
    let h = Harness::new(App::sim(), Vec2::new(390.0, 844.0));
    assert_eq!(h.app.screen, Screen::Monitor);
    let h = Harness::new(App::boxed(), Vec2::new(360.0, 640.0));
    assert_eq!(h.app.screen, Screen::Monitor);
    let mut h = Harness::new(App::sim(), Vec2::new(1400.0, 860.0));
    assert_eq!(h.app.screen, Screen::Editor);
    // The header switches either way, and a resize does not undo it.
    h.press("monitor");
    assert_eq!(h.app.screen, Screen::Monitor);
    let mut p = Harness::new(App::sim(), Vec2::new(390.0, 844.0));
    p.press("editor");
    assert_eq!(p.app.screen, Screen::Editor);
    p.frame(vec![]);
    assert_eq!(p.app.screen, Screen::Editor);
}

#[test]
fn monitor_and_andon_in_the_url_open_the_monitor() {
    for q in [
        "?monitor",
        "?andon",
        "monitor",
        "?x=1&monitor",
        "?andon=1",
        "?playground&monitor",
    ] {
        assert!(wants_monitor(q), "{q}");
    }
    for q in ["", "?", "?monitors", "?editor", "?demo", "?andonx"] {
        assert!(!wants_monitor(q), "{q}");
    }
    // `?andon` watches the box, as the old floor screen did; `?monitor`
    // with the playground watches the simulation.
    assert!(!wants_sim("?andon", None));
    assert!(wants_sim("?playground&monitor", None));
    let mut app = App::boxed();
    app.open_monitor();
    let h = Harness::new(app, Vec2::new(1920.0, 1080.0));
    assert_eq!(h.app.screen, Screen::Monitor, "a wide wall TV too");
}

#[test]
fn open_diagram_on_a_box_opens_that_module_at_that_instance() {
    let (b, mut h, mut sent) = box_line();
    h.press("tile line/S1.");
    h.frame(vec![]);
    h.press("monitor diagram");
    assert_eq!(h.app.screen, Screen::Editor);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        serve(&mut h, &b, &mut sent);
        h.frame(vec![]);
        let bv = h.app.boxv.as_ref().unwrap();
        if let Some(v) = &bv.view {
            assert_eq!(bv.state.selected.as_deref(), Some("line"));
            assert_eq!(v.scopes[v.scope].prefix, "S1.");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no diagram: {:?}",
            bv.state.problem
        );
    }
    // Back in the Editor: its own stream again, the Monitor's closed.
    assert!(sent.contains(&Cmd::Close(Stream::Traces)), "{sent:?}");
    assert!(
        sent.contains(&Cmd::Listen(Stream::Trace, crate::remote::trace_url("line"))),
        "{sent:?}"
    );
}
