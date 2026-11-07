//! The canvas over a slow link (card canvas-slow-link): remote support
//! over a site VPN, where another vendor's online view freezes. The
//! page runs against a real box API behind [`Throttle`], an in-process
//! TCP proxy with a site VPN's round trip, bandwidth and loss, carried by
//! [`Wire`], a native transport that works like `web.rs` (every fetch
//! and stream on its own, answers into an inbox the next frame takes).
//!
//! Loss: a userspace proxy cannot drop a TCP segment (the kernel would
//! resend it at once on loopback), so a lost segment is modelled as what
//! TCP makes of one: the segment and everything behind it on that
//! connection arrive one retransmit timeout late. Which segments are
//! "lost" is seeded per request (its path and how many times it was
//! asked), so a run is the same every time.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, Pos2, RawInput, Rect, Vec2};

use crate::app::{App, Source};
use crate::box_projects::{PCall, PReq};
use crate::box_tests::{Bench, DEMO};
use crate::monitor::Group;
use crate::remote::{Cmd, Msg, Stream};
use crate::review::{Answer, Call, Method, Request};

/// The box's demo line, as the Monitor tests run it.
const LINE: &str = include_str!("../../helmsman-plc/programs/line.st");

/// A site VPN as the card has it: 300 ms round trip, 1 Mbit/s each way,
/// 2 % of segments lost.
const RTT: Duration = Duration::from_millis(300);
const BYTES_PER_S: f64 = 1_000_000.0 / 8.0;
const LOSS: f64 = 0.02;
/// What a lost segment costs: TCP notices and resends it after about a
/// round trip and its minimum retransmit timeout (200 ms).
const RTO: Duration = Duration::from_millis(500);
/// A segment, and the bytes one connection may have on the link at once
/// (its window): beyond that, the sender waits, as TCP's would.
const SEGMENT: usize = 1460;
const WINDOW: usize = 32;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// splitmix64: the proxy's seeded randomness.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    #[allow(clippy::cast_precision_loss)]
    fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / ((1u64 << 53) as f64) < p
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01B3)
    })
}

/// A reply held up on the way back (a VPN stall): the `nth` (0-based)
/// request whose path starts with `path` arrives `extra` late.
#[derive(Debug, Clone)]
struct Hang {
    path: &'static str,
    nth: usize,
    extra: Duration,
}

/// The link both ways: when each direction is free next (one link, so
/// every connection shares it), and how often each path was asked.
struct LinkState {
    free: [Instant; 2],
    asked: BTreeMap<String, usize>,
    hangs: Vec<Hang>,
    /// Hangs that held a reply up.
    hung: Vec<String>,
    lost: usize,
}

/// The throttled proxy in front of a box.
struct Throttle {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    link: Arc<Mutex<LinkState>>,
}

impl Drop for Throttle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wakes the accept loop.
        let _ = TcpStream::connect(self.addr);
    }
}

impl Throttle {
    fn start(upstream: SocketAddr, hangs: Vec<Hang>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let now = Instant::now();
        let link = Arc::new(Mutex::new(LinkState {
            free: [now, now],
            asked: BTreeMap::new(),
            hangs,
            hung: Vec::new(),
            lost: 0,
        }));
        {
            let (stop, link) = (Arc::clone(&stop), Arc::clone(&link));
            thread::spawn(move || {
                for c in listener.incoming() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(c) = c else { continue };
                    let link = Arc::clone(&link);
                    thread::spawn(move || {
                        let _ = connection(c, upstream, &link);
                    });
                }
            });
        }
        Self { addr, stop, link }
    }
}

/// One connection through the link: the request head is read first (for
/// its path, which seeds the loss and picks a hang), then both ways are
/// pumped.
fn connection(client: TcpStream, upstream: SocketAddr, link: &Arc<Mutex<LinkState>>) -> std::io::Result<()> {
    let mut head = Vec::new();
    let mut buf = [0u8; SEGMENT];
    let mut c = client.try_clone()?;
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = c.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        head.extend_from_slice(&buf[..n]);
    }
    let path = String::from_utf8_lossy(&head)
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();
    let (nth, hang) = {
        let mut l = lock(link);
        let n = l.asked.entry(path.clone()).or_default();
        let nth = *n;
        *n += 1;
        let hang = l
            .hangs
            .iter()
            .find(|h| path.starts_with(h.path) && h.nth == nth)
            .map(|h| h.extra);
        if hang.is_some() {
            l.hung.push(path.clone());
        }
        (nth, hang)
    };
    let seed = fnv(&path) ^ (nth as u64).wrapping_mul(0xA24B_AED4_963E_E407);
    let box_side = TcpStream::connect(upstream)?;
    let (up_src, up_dst) = (client.try_clone()?, box_side.try_clone()?);
    let link_up = Arc::clone(link);
    thread::spawn(move || pump(up_src, up_dst, &link_up, 0, Rng::new(seed), head, None));
    pump(box_side, client, link, 1, Rng::new(seed ^ 1), Vec::new(), hang);
    Ok(())
}

/// Carries `src` to `dst` over direction `dir` of the link: each segment
/// takes its turn on the shared link, then half a round trip, plus a
/// retransmit timeout when it is lost; in order, as TCP delivers.
fn pump(
    mut src: TcpStream,
    mut dst: TcpStream,
    link: &Arc<Mutex<LinkState>>,
    dir: usize,
    mut rng: Rng,
    first: Vec<u8>,
    hang: Option<Duration>,
) {
    let (tx, rx) = mpsc::sync_channel::<(Instant, Vec<u8>)>(WINDOW);
    let writer = {
        let src = src.try_clone();
        thread::spawn(move || {
            let Ok(src) = src else {
                return;
            };
            for (at, bytes) in rx {
                let now = Instant::now();
                if at > now {
                    // Pacing the link, not waiting for a result.
                    thread::sleep(at - now);
                }
                if dst.write_all(&bytes).is_err() {
                    let _ = src.shutdown(Shutdown::Both);
                    return;
                }
            }
            let _ = dst.shutdown(Shutdown::Write);
        })
    };
    let mut last = Instant::now();
    let mut extra = hang.unwrap_or_default();
    let mut send = |bytes: Vec<u8>, rng: &mut Rng| {
        #[allow(clippy::cast_precision_loss)]
        let tx_time = Duration::from_secs_f64(bytes.len() as f64 / BYTES_PER_S);
        let mut at = {
            let mut l = lock(link);
            let start = l.free[dir].max(Instant::now());
            l.free[dir] = start + tx_time;
            if rng.chance(LOSS) {
                l.lost += 1;
                start + tx_time + RTO
            } else {
                start + tx_time
            }
        } + RTT / 2
            + std::mem::take(&mut extra);
        at = at.max(last);
        last = at;
        tx.send((at, bytes)).is_ok()
    };
    let mut ok = first.is_empty() || send(first, &mut rng);
    let mut buf = [0u8; SEGMENT];
    while ok {
        match src.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => ok = send(buf[..n].to_vec(), &mut rng),
        }
    }
    drop(send);
    drop(tx);
    let _ = writer.join();
}

/// What came back, for the next frame.
enum In {
    Box(Msg),
    Project(PCall, Answer),
    Review(Call, Answer),
}

/// Requests in flight, per kind: now, and the most ever at once.
#[derive(Default)]
struct Flight {
    now: BTreeMap<String, usize>,
    most: BTreeMap<String, usize>,
    done: BTreeMap<String, usize>,
}

impl Flight {
    fn start(&mut self, key: &str) {
        let n = self.now.entry(key.into()).or_default();
        *n += 1;
        let m = self.most.entry(key.into()).or_default();
        *m = (*m).max(*n);
    }

    fn end(&mut self, key: &str) {
        if let Some(n) = self.now.get_mut(key) {
            *n -= 1;
        }
        *self.done.entry(key.into()).or_default() += 1;
    }
}

/// The page's transport, natively: like `web.rs`, a fetch or a stream
/// never holds the frame up; what comes back waits in the inbox.
struct Wire {
    addr: SocketAddr,
    inbox: Arc<Mutex<Vec<In>>>,
    flight: Arc<Mutex<Flight>>,
    streams: BTreeMap<String, Arc<AtomicBool>>,
}

impl Drop for Wire {
    fn drop(&mut self) {
        for s in self.streams.values() {
            s.store(true, Ordering::Relaxed);
        }
    }
}

/// An HTTP/1.1 exchange: status, `Retry-After`, the build stamp, body.
fn exchange(
    addr: SocketAddr,
    method: &str,
    url: &str,
    body: Option<(&[u8], &str)>,
) -> Result<(u16, Option<u64>, Option<String>, String), String> {
    let mut s = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;
    let mut req = format!("{method} {url} HTTP/1.1\r\nHost: box\r\nConnection: close\r\n");
    if let Some((b, ctype)) = body {
        let _ = write!(req, "Content-Type: {ctype}\r\nContent-Length: {}\r\n", b.len());
    }
    req.push_str("\r\n");
    let mut bytes = req.into_bytes();
    if let Some((b, _)) = body {
        bytes.extend_from_slice(b);
    }
    s.write_all(&bytes).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).map_err(|e| e.to_string())?;
    let out = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = out.split_once("\r\n\r\n").ok_or("no reply")?;
    let status = head.get(9..12).and_then(|s| s.parse().ok()).ok_or("no status")?;
    let field = |name: &str| {
        head.split("\r\n").skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    };
    Ok((
        status,
        field("Retry-After").and_then(|v| v.parse().ok()),
        field(crate::stamp::HEADER),
        body.to_string(),
    ))
}

fn answer(r: Result<(u16, Option<u64>, Option<String>, String), String>) -> Answer {
    match r {
        Ok((status, retry_after, _, body)) => Answer {
            status,
            retry_after,
            body,
        },
        Err(body) => Answer {
            status: 0,
            retry_after: None,
            body,
        },
    }
}

impl Wire {
    fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            inbox: Arc::default(),
            flight: Arc::default(),
            streams: BTreeMap::new(),
        }
    }

    fn drain(&self) -> Vec<In> {
        std::mem::take(&mut *lock(&self.inbox))
    }

    /// Runs `f` on its own thread as request `key`, its answer into the
    /// inbox.
    fn spawn(&self, key: String, f: impl FnOnce(SocketAddr) -> In + Send + 'static) {
        let (addr, inbox, flight) = (self.addr, Arc::clone(&self.inbox), Arc::clone(&self.flight));
        lock(&flight).start(&key);
        thread::spawn(move || {
            let got = f(addr);
            // Counted done before the page can see it, so the page's next
            // ask is never counted alongside this one.
            lock(&flight).end(&key);
            lock(&inbox).push(got);
        });
    }

    fn run(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Get(what, url) => self.spawn(format!("box {what:?}"), move |addr| {
                let r = exchange(addr, "GET", &url, None);
                let got = match r {
                    Ok((200, _, _, body)) => Ok(body),
                    Ok((s, ..)) => Err(format!("HTTP {s}")),
                    Err(e) => Err(e),
                };
                In::Box(Msg::Got(what, got))
            }),
            Cmd::Listen(stream, url) => {
                self.close(stream);
                let stop = Arc::new(AtomicBool::new(false));
                self.streams.insert(format!("{stream:?}"), Arc::clone(&stop));
                let (addr, inbox) = (self.addr, Arc::clone(&self.inbox));
                thread::spawn(move || listen(addr, stream, &url, &stop, &inbox));
            }
            Cmd::Close(stream) => self.close(stream),
        }
    }

    fn close(&mut self, stream: Stream) {
        if let Some(s) = self.streams.remove(&format!("{stream:?}")) {
            s.store(true, Ordering::Relaxed);
        }
    }

    fn project(&self, q: PReq) {
        self.spawn(format!("project {:?}", q.call), move |addr| {
            let body = q.body.as_deref().map(|b| (b, q.ctype));
            In::Project(q.call, answer(exchange(addr, q.method, &q.url, body)))
        });
    }

    fn review(&self, q: Request) {
        self.spawn(format!("review {:?}", q.call), move |addr| {
            let method = match q.method {
                Method::Get => "GET",
                Method::Post => "POST",
            };
            let body = q.body.as_deref().map(|b| (b.as_bytes(), "application/json"));
            In::Review(q.call, answer(exchange(addr, method, &q.url, body)))
        });
    }
}

/// An `EventSource`: opened, each complete event as it comes, failed
/// once when the connection ends (unless the page closed it).
fn listen(addr: SocketAddr, stream: Stream, url: &str, stop: &AtomicBool, inbox: &Mutex<Vec<In>>) {
    let push = |m: Msg| {
        if !stop.load(Ordering::Relaxed) {
            lock(inbox).push(In::Box(m));
        }
    };
    let Ok(mut s) = TcpStream::connect(addr) else {
        push(Msg::Failed(stream));
        return;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(50)));
    if write!(s, "GET {url} HTTP/1.1\r\nHost: box\r\n\r\n").is_err() {
        push(Msg::Failed(stream));
        return;
    }
    let mut raw = Vec::new();
    let mut body: Option<usize> = None;
    let mut buf = [0u8; 8192];
    while !stop.load(Ordering::Relaxed) {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => break,
        }
        if body.is_none()
            && let Some(k) = raw.windows(4).position(|w| w == b"\r\n\r\n")
        {
            if !raw.starts_with(b"HTTP/1.1 200") {
                break;
            }
            push(Msg::Opened(stream));
            raw.drain(..k + 4);
            body = Some(0);
        }
        if body.is_some() {
            while let Some(k) = raw.windows(2).position(|w| w == b"\n\n") {
                let block: Vec<u8> = raw.drain(..k + 2).collect();
                let block = String::from_utf8_lossy(&block);
                let (mut name, mut data) = ("message".to_string(), None::<String>);
                for line in block.lines() {
                    if let Some(v) = line.strip_prefix("event: ") {
                        name = v.into();
                    } else if let Some(v) = line.strip_prefix("data: ") {
                        data = Some(data.map_or_else(|| v.into(), |d| format!("{d}\n{v}")));
                    }
                }
                if let Some(d) = data {
                    push(Msg::Event(stream, name, d));
                }
            }
        }
    }
    push(Msg::Failed(stream));
}

/// The canvas's clock runs this much faster than the test's, so its 5 s
/// device round and 30 s proposals badge come round several times in a
/// few seconds; against the link, that is a link this many times worse
/// than the card's on top.
const SPEED: f64 = 8.0;

/// Frames the slowest-frame check skips: the first build the fonts.
const WARM_UP: usize = 10;

/// The page on the Monitor, carried over `wire`, one frame.
struct Page {
    ctx: egui::Context,
    app: App,
    wire: Wire,
    t0: Instant,
    /// Frames drawn.
    frames: usize,
    /// The slowest frame after the first few (which build the fonts).
    slowest: Duration,
}

impl Page {
    fn new(addr: SocketAddr) -> Self {
        let mut app = App::boxed();
        app.open_monitor();
        Self {
            ctx: egui::Context::default(),
            app,
            wire: Wire::new(addr),
            t0: Instant::now(),
            frames: 0,
            slowest: Duration::ZERO,
        }
    }

    fn time(&self) -> f64 {
        self.t0.elapsed().as_secs_f64() * SPEED
    }

    /// Takes in what arrived (as the browser's `tick` does), draws a
    /// frame (timed), and sends what the page asked for.
    fn frame(&mut self) {
        let now = self.time();
        let mut msgs = Vec::new();
        let mut answers = Vec::new();
        for i in self.wire.drain() {
            match i {
                In::Box(m) => msgs.push(m),
                In::Project(c, a) => answers.push((c, a)),
                In::Review(c, a) => {
                    if let Some(r) = &mut self.app.review {
                        r.state.answer(c, &a, now);
                    }
                }
            }
        }
        if let Some(b) = &mut self.app.boxv {
            b.step_all(&self.ctx, msgs, answers, now);
        }
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1920.0, 1080.0))),
            time: Some(now),
            ..Default::default()
        };
        let started = Instant::now();
        let Self { ctx, app, .. } = self;
        let mut out = ctx.run_ui(input, |ui| app.show(ui));
        self.frames += 1;
        if self.frames > WARM_UP {
            self.slowest = self.slowest.max(started.elapsed());
        }
        out.textures_delta.clear();
        if let Some(b) = &mut self.app.boxv {
            for c in std::mem::take(&mut b.sent) {
                self.wire.run(c);
            }
            for q in std::mem::take(&mut b.psent) {
                self.wire.project(q);
            }
        }
        if let Some(r) = &mut self.app.review {
            for q in std::mem::take(&mut r.sent) {
                self.wire.review(q);
            }
        }
    }

    fn groups(&mut self) -> Vec<Group> {
        let now = self.time();
        let detail = self.app.scan_detail();
        let App {
            monitor,
            boxv,
            source,
            ..
        } = &mut self.app;
        assert_eq!(*source, Source::Box);
        monitor.groups(&boxv.as_ref().unwrap().state, now, detail)
    }

    /// The line's tiles all lit with fresh values.
    fn live(&mut self) -> bool {
        self.groups().iter().any(|g| {
            g.program == "line"
                && g.tiles.len() > 2
                && g.tiles
                    .iter()
                    .all(|t| t.stale.is_none() && t.lamps.iter().all(|l| l.value.is_some()))
        })
    }
}

#[test]
fn the_monitor_stays_usable_over_a_slow_site_vpn() {
    let b = Bench::with(&[("cell", DEMO), ("line", LINE)]);
    // Two replies stall well past their poll's period: the map (behind a
    // device round every 5 s of page time) and the proposals list
    // (behind the badge's 30 s). Neither may be asked for again while it
    // is on its way.
    let hold = Duration::from_millis(4500);
    let proxy = Throttle::start(
        b.addr,
        vec![
            Hang {
                path: "/api/io_map",
                nth: 0,
                extra: hold,
            },
            Hang {
                path: "/api/proposals",
                nth: 0,
                extra: hold,
            },
        ],
    );
    let mut page = Page::new(proxy.addr);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut live_after_hold = false;
    // Frames drawn while both held replies were on their way.
    let mut held_frames = 0;
    loop {
        page.frame();
        if page.t0.elapsed() < hold {
            let f = lock(&page.wire.flight);
            let out = |k: &str| f.now.get(k).copied().unwrap_or(0) > 0;
            if lock(&proxy.link).hung.len() == 2 && out("box IoMap") && out("review List") {
                held_frames += 1;
            }
        }
        let held_past = page.t0.elapsed() > hold + RTT * 4;
        if held_past && page.live() {
            let done = lock(&page.wire.flight).done.clone();
            let lists = done.get("review List").copied().unwrap_or(0);
            let maps = done.get("box IoMap").copied().unwrap_or(0);
            if lists >= 2 && maps >= 2 {
                live_after_hold = true;
                break;
            }
        }
        if Instant::now() > deadline {
            break;
        }
        // A frame's pace, 60 a second, not a wait for a result.
        thread::sleep(Duration::from_millis(16));
    }
    let groups = page.groups();
    let flight = lock(&page.wire.flight);
    let link = lock(&proxy.link);
    assert!(
        live_after_hold,
        "the Monitor shows live values and the polls came round again after the stall: done {:?}, \
         asked {:?}, groups {:?}",
        flight.done, link.asked, groups
    );
    let mut hung = link.hung.clone();
    hung.sort();
    assert_eq!(
        hung,
        ["/api/io_map", "/api/proposals"],
        "both stalls held a reply up: {:?}",
        link.asked
    );
    assert!(link.lost > 0, "segments were lost on the way");
    for (kind, most) in &flight.most {
        assert_eq!(*most, 1, "{kind}: {most} at once (all: {:?})", flight.most);
    }
    assert!(
        flight.done.get("box Devices").copied().unwrap_or(0) >= 2,
        "the device round came round again once the map came: {:?}",
        flight.done
    );
    // The page kept drawing while the replies were held up: a frame that
    // waited for one would have been the only frame of the hold, and
    // would have taken the whole hold. (The bound is loose for 6 copies
    // of the suite at once, whose frames take up to half a second.)
    assert!(held_frames >= 20, "{held_frames} frames during the hold");
    assert!(page.slowest < hold / 3, "slowest frame {:?}", page.slowest);
}

#[test]
fn the_proposals_badge_does_not_ask_again_while_its_list_is_on_its_way() {
    // No transport: the list never comes. The badge's 30 s round waits
    // for it rather than stacking a second, third, ... behind it.
    let ctx = egui::Context::default();
    let mut app = App::boxed();
    app.open_monitor();
    let mut lists = 0;
    let frame = |app: &mut App, time: f64| {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
            time: Some(time),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear();
        let r = app.review.as_mut().unwrap();
        let sent = std::mem::take(&mut r.sent);
        sent.into_iter().filter(|q| q.call == Call::List).count()
    };
    for k in 0..=200 {
        lists += frame(&mut app, f64::from(k) * 0.5);
    }
    assert_eq!(lists, 1, "100 s with the first list still on its way");
    let ok = Answer {
        status: 200,
        retry_after: None,
        body: "[]".into(),
    };
    app.review.as_mut().unwrap().state.answer(Call::List, &ok, 100.0);
    let mut again = 0;
    for k in 201..=270 {
        again += frame(&mut app, f64::from(k) * 0.5);
    }
    assert_eq!(again, 1, "the next round, once it came");
    // A list that failed counts as answered too.
    let failed = Answer {
        status: 0,
        retry_after: None,
        body: "network".into(),
    };
    app.review
        .as_mut()
        .unwrap()
        .state
        .answer(Call::List, &failed, 135.0);
    let mut after = 0;
    for k in 271..=340 {
        after += frame(&mut app, f64::from(k) * 0.5);
    }
    assert_eq!(after, 1);
}
