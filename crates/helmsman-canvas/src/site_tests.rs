//! The site view: the view model (the list and each peer's own answer to
//! rows), what it fetches when, what it refuses from a hostile peer, the
//! view driven headlessly through egui (what it paints, where a click
//! goes), then against two real box APIs whose sites heard each other.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, OutputCommand, PointerButton, Pos2, RawInput, Rect, Shape, Vec2};
use helmsman_plc::api::{
    Api, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, Shared, View,
};
use helmsman_plc::site::{Beacon, Site};
use serde_json::{Value as Json, json};

use crate::app::{App, Mode};
use crate::remote::{Cmd, Fetch, Msg};
use crate::site::{Health, MAX_BODY, PEER_TIMEOUT, REFRESH, STALE_AFTER, SiteState};

const ME: &str = "0123456789abcdef";
const P1: &str = "1111111111111111";
const P2: &str = "2222222222222222";

fn status(published: bool, running: u64, held: u64, faulted: u64, down: u64) -> Json {
    json!({
        "published": published, "modules": running + held + faulted,
        "running": running, "held": held, "faulted": faulted,
        "devices": { "up": 1, "down": down, "unknown": 0 },
    })
}

fn peer(id: &str, name: &str, address: &str) -> Json {
    json!({ "id": id, "name": name, "version": "0.1.0", "address": address,
            "last_seen_ms": 1200, "first_seen_ms": 60_000, "conflict": null })
}

fn list(peers: &[Json]) -> Msg {
    let body = json!({
        "site": "line-a",
        "self": { "id": ME, "name": "Press 4", "api_port": 8080, "version": "0.1.0",
                  "status": status(true, 3, 0, 0, 0) },
        "peers": peers,
        "max_peers": 64,
        "dropped": {},
    });
    Msg::Got(Fetch::Site, Ok(body.to_string()))
}

fn answer(id: &str, st: &Json) -> String {
    json!({ "site": "line-a", "self": { "id": id, "name": "x", "api_port": 1, "version": "0.1.0", "status": st },
            "peers": [] })
    .to_string()
}

/// The probe for `id` among `cmds`: its number and URL.
fn probe(cmds: &[Cmd], id: &str) -> Option<(u64, String)> {
    cmds.iter().find_map(|c| match c {
        Cmd::Get(Fetch::Peer { id: i, n }, url) if i == id => Some((*n, url.clone())),
        _ => None,
    })
}

fn two_peers() -> SiteState {
    let mut s = SiteState::new();
    assert_eq!(s.poll(0.0), vec![Cmd::Get(Fetch::Site, "/api/site".into())]);
    s.apply(
        list(&[
            peer(P2, "Weld 2", "10.0.0.12:8080"),
            peer(P1, "Assembly", "10.0.0.11:8081"),
        ]),
        0.1,
    );
    s
}

#[test]
fn this_box_leads_and_each_peer_is_probed_at_its_own_address() {
    let mut s = two_peers();
    let cmds = s.poll(0.2);
    assert_eq!(probe(&cmds, P1).unwrap().1, "http://10.0.0.11:8081/api/site");
    assert_eq!(probe(&cmds, P2).unwrap().1, "http://10.0.0.12:8080/api/site");
    assert_eq!(cmds.len(), 2, "{cmds:?}");
    let lines = s.lines(0.2);
    let names: Vec<&str> = lines.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Press 4", "Assembly", "Weld 2"]);
    assert!(lines[0].this);
    assert_eq!(lines[0].url, None);
    assert_eq!(lines[0].status, "RUNNING · 3 OF 3 MODULES");
    assert_eq!(lines[0].health, Health::Ok);
    assert_eq!(lines[1].url.as_deref(), Some("http://10.0.0.11:8081/"));
    assert_eq!(lines[1].health, Health::Checking);
    assert_eq!(lines[1].seen, "1 s ago");
    assert_eq!(s.site.as_deref(), Some("line-a"));
}

#[test]
fn a_peers_own_answer_is_its_status() {
    let mut s = two_peers();
    let cmds = s.poll(0.2);
    let (n1, _) = probe(&cmds, P1).unwrap();
    let (n2, _) = probe(&cmds, P2).unwrap();
    s.apply(
        Msg::Got(
            Fetch::Peer { id: P1.into(), n: n1 },
            Ok(answer(P1, &status(true, 1, 1, 1, 2))),
        ),
        0.5,
    );
    s.apply(
        Msg::Got(
            Fetch::Peer { id: P2.into(), n: n2 },
            Ok(answer(P2, &status(false, 0, 0, 0, 0))),
        ),
        0.5,
    );
    let lines = s.lines(0.6);
    assert_eq!(lines[1].status, "1 FAULTED · 1 HELD · 2 DEVICES DOWN");
    assert_eq!(lines[1].health, Health::Fault);
    assert_eq!(lines[2].status, "STARTING");
    assert_eq!(lines[2].health, Health::Warn);
    s.apply(
        Msg::Got(
            Fetch::Peer { id: P1.into(), n: n1 },
            Ok(answer(P1, &status(true, 0, 0, 0, 1))),
        ),
        0.7,
    );
    // Already answered: a second answer to the same probe changes nothing.
    assert_eq!(s.lines(0.8)[1].health, Health::Fault);
    // A status too old is not shown as current.
    assert_eq!(s.lines(0.5 + STALE_AFTER + 1.0)[1].health, Health::Unreachable);
}

#[test]
fn probes_repeat_every_refresh_and_a_silent_peer_times_out_to_unreachable() {
    let mut s = two_peers();
    let (n, _) = probe(&s.poll(1.0), P1).unwrap();
    // In flight: not asked again, however long the frames keep coming.
    assert_eq!(probe(&s.poll(2.0), P1), None);
    assert_eq!(probe(&s.poll(1.0 + PEER_TIMEOUT - 0.1), P1), None);
    // Unanswered past the timeout: failed, and asked again.
    let cmds = s.poll(1.0 + PEER_TIMEOUT);
    let (n2, _) = probe(&cmds, P1).unwrap();
    assert!(n2 > n);
    let line = &s.lines(1.0 + PEER_TIMEOUT)[1];
    assert_eq!(line.health, Health::Unreachable);
    assert!(line.status.starts_with("UNREACHABLE"), "{}", line.status);
    // The old probe's late answer is ignored; the new one counts.
    s.apply(
        Msg::Got(
            Fetch::Peer { id: P1.into(), n },
            Ok(answer(P1, &status(true, 2, 0, 0, 0))),
        ),
        9.5,
    );
    assert_eq!(s.lines(9.5)[1].health, Health::Unreachable);
    s.apply(
        Msg::Got(
            Fetch::Peer { id: P1.into(), n: n2 },
            Ok(answer(P1, &status(true, 2, 0, 0, 0))),
        ),
        9.6,
    );
    assert_eq!(s.lines(9.6)[1].health, Health::Ok);
    // Answered: asked again only after REFRESH from when it was sent.
    let sent = 1.0 + PEER_TIMEOUT;
    assert_eq!(probe(&s.poll(sent + REFRESH - 0.1), P1), None);
    assert!(probe(&s.poll(sent + REFRESH), P1).is_some());
    // A network error is unreachable with the browser's reason.
    let (n3, _) = probe(&s.poll(sent + 2.0 * REFRESH), P2).unwrap();
    s.apply(
        Msg::Got(
            Fetch::Peer { id: P2.into(), n: n3 },
            Err("TypeError: Failed to fetch".into()),
        ),
        30.0,
    );
    assert!(s.lines(30.0)[2].status.contains("Failed to fetch"));
}

#[test]
fn the_list_refreshes_and_peers_that_left_are_dropped() {
    let mut s = two_peers();
    let _ = s.poll(0.2);
    assert_eq!(
        s.poll(REFRESH - 0.1)
            .iter()
            .filter(|c| matches!(c, Cmd::Get(Fetch::Site, _)))
            .count(),
        0
    );
    // The list is in flight until it answers, however long.
    assert!(
        s.poll(REFRESH + 0.1)
            .contains(&Cmd::Get(Fetch::Site, "/api/site".into()))
    );
    assert!(
        !s.poll(3.0 * REFRESH)
            .contains(&Cmd::Get(Fetch::Site, "/api/site".into()))
    );
    s.apply(list(&[peer(P2, "Weld 2", "10.0.0.12:8080")]), 16.0);
    let lines = s.lines(16.0);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1].name, "Weld 2");
    assert!(probe(&s.poll(16.1), P1).is_none());
}

#[test]
fn a_box_without_a_site_or_out_of_reach_says_so() {
    let mut s = SiteState::new();
    let _ = s.poll(0.0);
    s.apply(Msg::Got(Fetch::Site, Err("HTTP 404".into())), 0.1);
    assert!(s.problem.as_deref().unwrap().contains("--site NAME"));
    assert_eq!(s.lines(0.1), Vec::new());
    s.apply(Msg::Got(Fetch::Site, Err("TypeError".into())), 0.2);
    assert!(s.problem.as_deref().unwrap().contains("Cannot reach"));
    s.apply(Msg::Got(Fetch::Site, Ok("<html>".into())), 0.3);
    assert!(s.problem.as_deref().unwrap().contains("something else"));
}

#[test]
fn a_hostile_peer_answer_is_never_shown_as_a_status() {
    let mut s = two_peers();
    let ask = |s: &mut SiteState, body: Result<String, String>, now: f64| {
        let cmds = s.poll(now);
        let (n, _) = probe(&cmds, P1).unwrap();
        s.apply(Msg::Got(Fetch::Peer { id: P1.into(), n }, body), now);
        s.lines(now)[1].clone()
    };
    let big = format!(
        "{}{}",
        answer(P1, &status(true, 1, 0, 0, 0)),
        " ".repeat(MAX_BODY)
    );
    let cases = [
        (Ok(big), "answer too large"),
        (
            Ok(answer(P2, &status(true, 1, 0, 0, 0))),
            "answered as box 2222222222222222",
        ),
        (Ok("{}".into()), "not a box's answer"),
        (Ok("[1,2]".into()), "not a box's answer"),
        (
            Ok(answer(P1, &json!({ "published": "yes" }))),
            "not a box's answer",
        ),
        (Ok("\u{0}\u{1b}[31m".into()), "not a box's answer"),
    ];
    for (k, (body, why)) in cases.into_iter().enumerate() {
        let line = ask(&mut s, body, 1.0 + REFRESH * k as f64);
        assert_eq!(line.health, Health::Unreachable, "{why}: {line:?}");
        assert!(line.status.contains(why), "{why}: {line:?}");
    }
}

#[test]
fn only_a_plain_ipv4_address_becomes_a_link() {
    let mut s = SiteState::new();
    let _ = s.poll(0.0);
    let bad = [
        "javascript:alert(1)",
        "10.0.0.5",
        "10.0.0.5:0",
        "0.0.0.0:8080",
        "[::1]:8080",
        "evil.example:80",
        "10.0.0.5:8080/../x",
        "10.0.0.5:8080@evil",
    ];
    let peers: Vec<Json> = bad
        .iter()
        .enumerate()
        .map(|(k, a)| peer(&format!("{k:016x}"), &format!("Box {k}"), a))
        .collect();
    s.apply(list(&peers), 0.1);
    let cmds = s.poll(0.2);
    assert!(
        cmds.iter().all(|c| !matches!(c, Cmd::Get(Fetch::Peer { .. }, _))),
        "{cmds:?}"
    );
    for l in s.lines(0.2).iter().skip(1) {
        assert_eq!(
            (l.url.as_deref(), l.status.as_str()),
            (None, "NO USABLE ADDRESS"),
            "{l:?}"
        );
    }
}

#[test]
fn names_are_cleaned_and_conflicts_and_silence_show() {
    let mut s = SiteState::new();
    let _ = s.poll(0.0);
    let mut odd = peer(
        P1,
        &format!("Line\u{1b}[2J\n{}", "x".repeat(100)),
        "10.0.0.11:8080",
    );
    odd["conflict"] = json!("10.0.0.66");
    odd["last_seen_ms"] = json!(20_000);
    s.apply(list(&[odd]), 0.1);
    let (n, _) = probe(&s.poll(0.2), P1).unwrap();
    s.apply(
        Msg::Got(
            Fetch::Peer { id: P1.into(), n },
            Ok(answer(P1, &status(true, 1, 0, 0, 0))),
        ),
        0.3,
    );
    let l = &s.lines(0.3)[1];
    assert!(!l.name.chars().any(char::is_control), "{:?}", l.name);
    assert_eq!(l.name.chars().count(), 48);
    assert!(
        l.status.starts_with("ID CONFLICT (also 10.0.0.66)"),
        "{}",
        l.status
    );
    assert!(l.status.contains("BEACON SILENT"), "{}", l.status);
    assert_eq!(l.health, Health::Warn);
    assert_eq!(l.seen, "20 s ago");
}

// ---- headless egui ----------------------------------------------------

struct Screen {
    ctx: egui::Context,
    app: App,
}

#[derive(Default)]
struct Painted {
    texts: Vec<(String, Rect)>,
    open: Vec<String>,
}

fn collect(shape: &Shape, out: &mut Painted) {
    match shape {
        Shape::Text(t) => out.texts.push((
            t.galley.text().to_string(),
            t.galley.rect.translate(t.pos.to_vec2()),
        )),
        Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
        _ => {}
    }
}

impl Screen {
    fn new() -> Self {
        let mut app = App::with_project(crate::example::project());
        app.mode = Mode::Site;
        Self {
            ctx: egui::Context::default(),
            app,
        }
    }

    fn feed(&mut self, msgs: Vec<Msg>, now: f64) {
        let ctx = self.ctx.clone();
        self.app
            .site
            .get_or_insert_with(crate::site::SiteView::new)
            .step(&ctx, msgs, now);
    }

    fn frame(&mut self, now: f64, events: Vec<Event>) -> Painted {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 700.0))),
            time: Some(now),
            events,
            ..Default::default()
        };
        let Self { ctx, app } = self;
        let mut out = ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear();
        let mut p = Painted::default();
        for c in &out.shapes {
            collect(&c.shape, &mut p);
        }
        for c in &out.platform_output.commands {
            if let OutputCommand::OpenUrl(u) = c {
                p.open.push(u.url.clone());
            }
        }
        p
    }

    fn click(&mut self, at: Pos2, now: f64) -> Vec<String> {
        let press = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut open = self.frame(now, vec![Event::PointerMoved(at)]).open;
        open.extend(self.frame(now, vec![press(true)]).open);
        open.extend(self.frame(now, vec![press(false)]).open);
        open
    }
}

#[test]
fn the_site_tab_lists_boxes_and_a_click_opens_that_boxs_canvas() {
    let mut sc = Screen::new();
    sc.frame(0.0, vec![]);
    let sent = sc.app.site.as_ref().unwrap().sent.clone();
    assert!(
        sent.contains(&Cmd::Get(Fetch::Site, "/api/site".into())),
        "{sent:?}"
    );
    sc.feed(
        vec![list(&[
            peer(P1, "Assembly", "10.0.0.11:8081"),
            peer(P2, "Weld 2", "10.0.0.12:8080"),
        ])],
        0.1,
    );
    let sent = sc.app.site.as_ref().unwrap().sent.clone();
    let (n1, _) = probe(&sent, P1).unwrap();
    let (n2, _) = probe(&sent, P2).unwrap();
    sc.feed(
        vec![
            Msg::Got(
                Fetch::Peer { id: P1.into(), n: n1 },
                Ok(answer(P1, &status(true, 0, 0, 2, 0))),
            ),
            Msg::Got(
                Fetch::Peer { id: P2.into(), n: n2 },
                Err("TypeError: Failed to fetch".into()),
            ),
        ],
        0.2,
    );
    sc.frame(0.3, vec![]);
    let p = sc.frame(0.3, vec![]);
    let texts: Vec<&str> = p.texts.iter().map(|(t, _)| t.as_str()).collect();
    for want in [
        "line-a",
        "Press 4 (this box)",
        "Assembly",
        "Weld 2",
        "2 FAULTED",
        "10.0.0.11:8081",
        "RUNNING · 3 OF 3 MODULES",
    ] {
        assert!(texts.contains(&want), "{want} not in {texts:?}");
    }
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("UNREACHABLE") && t.contains("Failed to fetch")),
        "{texts:?}"
    );
    let at = |name: &str| p.texts.iter().find(|(t, _)| t == name).unwrap().1.center();
    assert_eq!(sc.click(at("Weld 2"), 0.4), ["http://10.0.0.12:8080/"]);
    let me = p
        .texts
        .iter()
        .find(|(t, _)| t == "Press 4 (this box)")
        .unwrap()
        .1
        .center();
    assert_eq!(sc.click(me, 0.5), Vec::<String>::new());
}

#[test]
fn a_box_without_a_site_shows_why_and_survives_any_window() {
    let mut sc = Screen::new();
    sc.frame(0.0, vec![]);
    sc.feed(vec![Msg::Got(Fetch::Site, Err("HTTP 404".into()))], 0.1);
    let p = sc.frame(0.2, vec![]);
    assert!(
        p.texts.iter().any(|(t, _)| t.contains("--site NAME")),
        "{:?}",
        p.texts
    );
    for size in [
        Vec2::new(1.0, 1.0),
        Vec2::new(320.0, 200.0),
        Vec2::new(4000.0, 3000.0),
    ] {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(0.3),
            ..Default::default()
        };
        let Screen { ctx, app } = &mut sc;
        let mut out = ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear();
    }
}

// ---- against real boxes -----------------------------------------------

/// A box API in a site, with a published (empty) view.
struct RealBox {
    api: Api,
    _site: Site,
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn real_box(id: &str, name: &str, socket: UdpSocket, to: SocketAddr) -> RealBox {
    let port = free_port();
    let me = Beacon::new("line-a", id, name, port, "0.1.0").unwrap();
    let s = Site::start(socket, me, to, Duration::from_millis(50), Arc::new(|_: &str| {})).unwrap();
    let live = Arc::new(Live::default());
    live.offer(Arc::new(View {
        seq: 1,
        ..View::default()
    }));
    let cfg = Config {
        bind: SocketAddr::from(([127, 0, 0, 1], port)),
        token: "0".repeat(64),
        limits: Limits::default(),
        stream_every: Duration::from_millis(10),
        memory: 0,
        log: Arc::new(|_: &str| {}),
    };
    let shared = Shared {
        catalog: Catalog::default(),
        live,
        devices: Arc::new(Devices::new(Vec::<String>::new())),
        audit: Box::new(MemAudit::new(10)),
        proposals: Box::new(MemProposals::new(10)),
        running: None,
        snapshots: None,
        approval: None,
        chat: None,
    };
    let api = Api::start_in_site(cfg, shared, Extras::default(), Some(s.handle())).unwrap();
    RealBox { api, _site: s }
}

/// What a browser's fetch from another origin gets: the body, if the
/// answer lets that origin read it.
fn browser_get(url: &str, origin: &str) -> Result<String, String> {
    let rest = url.strip_prefix("http://").ok_or("not http")?;
    let (host, path) = rest.split_once('/').ok_or("no path")?;
    let mut s = TcpStream::connect(host).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET /{path} HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    let cors = head
        .split("\r\n")
        .any(|l| l.eq_ignore_ascii_case("Access-Control-Allow-Origin: *"));
    if origin != "same" && !cors {
        return Err("blocked by CORS".into());
    }
    if &head[9..12] != "200" {
        return Err(format!("HTTP {}", &head[9..12]));
    }
    Ok(body.to_string())
}

#[test]
fn the_site_view_shows_two_real_boxes_that_heard_each_other() {
    let sa = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sb = UdpSocket::bind("127.0.0.1:0").unwrap();
    let (ua, ub) = (sa.local_addr().unwrap(), sb.local_addr().unwrap());
    let a = real_box(ME, "Press 4", sa, ub);
    let b = real_box(P1, "Assembly", sb, ua);
    let page = format!("http://{}", a.api.addr());
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut s = SiteState::new();
    let mut now = 0.0;
    loop {
        for cmd in s.poll(now) {
            let Cmd::Get(what, url) = cmd else { continue };
            let r = match &what {
                Fetch::Site => browser_get(&format!("{page}{url}"), "same"),
                _ => browser_get(&url, &page),
            };
            s.apply(Msg::Got(what, r), now);
        }
        let lines = s.lines(now);
        if lines.len() == 2 && lines[1].health == Health::Ok {
            assert_eq!(lines[0].name, "Press 4");
            assert_eq!(lines[1].name, "Assembly");
            assert_eq!(lines[1].address, b.api.addr().to_string());
            assert_eq!(lines[1].url, Some(format!("http://{}/", b.api.addr())));
            assert_eq!(lines[1].status, "RUNNING · 0 OF 0 MODULES");
            break;
        }
        assert!(Instant::now() < deadline, "{lines:?} {:?}", s.problem);
        std::thread::sleep(Duration::from_millis(20));
        now += REFRESH;
    }
    // Box B gone: A still lists it (until its beacon expires), unreachable.
    drop(b);
    let mut lines = Vec::new();
    for _ in 0..3 {
        now += REFRESH;
        for cmd in s.poll(now) {
            let Cmd::Get(what, url) = cmd else { continue };
            let r = match &what {
                Fetch::Site => browser_get(&format!("{page}{url}"), "same"),
                _ => browser_get(&url, &page),
            };
            s.apply(Msg::Got(what, r), now);
        }
        lines = s.lines(now);
    }
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[1].health, Health::Unreachable, "{lines:?}");
}
