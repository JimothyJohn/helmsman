//! A simulated E1200 web console on `127.0.0.1`, for tests (behind the
//! `sim` feature). A `std` `TcpListener` thread; no real device is ever
//! involved.
//!
//! What it models, from the facts mapped on a real E1213 (firmware 3.1,
//! 2026-11-12; `devices/moxa/iologik-e1200-web-console.md` in
//! helmsman-devices):
//!
//! - `GET /` sets the cookies `ChallID` and `MoxaPass`; the log records
//!   whether each later request sent them back.
//! - Each settings page load issues a fresh `token`; `set.htm` accepts a
//!   write only with the token issued for that page's `code`.
//! - **Staged versus running**: a write changes the staged settings and the
//!   export (`06_6.htm` links `ik1213.txt`) renders the staged ones;
//!   `set_ok.htm` makes staged running and the console then drops
//!   connections for a configurable number of attempts (a downtime counted
//!   in connections, so tests do not depend on a clock).
//! - Import (`06_5_1.htm`, multipart): with `NetConfig_OverWrite` the
//!   file's network settings overwrite the unit's, without it they do not.
//! - A password change (`07.htm`, code 7) is decoded the way the page's
//!   script encodes it: `NewPwd` is unmasked with the stored challenge.
//! - A fault: [`Sim::drop_next`] closes the next connections without a
//!   reply (the console's known quirk).
//!
//! What it only assumes (nothing real was seen):
//!
//! - **The token rule.** ASSUMPTION about the real unit: `set.htm` answers
//!   a request whose token is not the latest one issued for its `code`
//!   with a page that lacks "Configuration Complete". What the real unit
//!   does with a stale or missing token is not known.
//! - The reply to a successful password change is the same "Configuration
//!   Complete" page; a wrong `OldPwd` or `ChallID` gets a page without it.
//!   `OldPwd` for a console without a password is the MD5 of the empty
//!   string. None of this was seen on a unit.
//! - Pages show the staged settings.
//! - The export's key names are synthetic except `CONNECTION_WATCHDOG`,
//!   `AUTOCLEAR_SAFEMODE`, `DOnn_SAFE`, `REST_ENABLE` and `EIP_ENABLE`
//!   (which the write-up cites); the network keys (`IP_ADDRESS`, `NETMASK`,
//!   `GATEWAY`) are made up. The pages are minimal HTML with the same form
//!   shapes as the real ones, none of Moxa's markup.
//! - TCP 44818 and 502 are not modelled.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use super::export::Export;
use super::password;
use crate::md5::{hex, md5};

/// The simulated unit's settings (staged or running).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Settings {
    pub mod_desc: String,
    pub mod_loc: String,
    pub idle_enable: bool,
    pub idle_time: u32,
    pub wd_enable: bool,
    pub wd_time: u32,
    pub safe_autoclear: bool,
    /// `DO00..DO07` safe values, true is On.
    pub safe_mode: [bool; 8],
    pub rest_enable: bool,
    pub eip_enable: bool,
    pub ip_address: String,
    pub netmask: String,
    pub gateway: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mod_desc: String::new(),
            mod_loc: String::new(),
            idle_enable: false,
            idle_time: 60,
            wd_enable: false,
            wd_time: 0,
            safe_autoclear: false,
            safe_mode: [false; 8],
            rest_enable: true,
            eip_enable: true,
            // Documentation range (RFC 5737): never a bench address.
            ip_address: "192.0.2.60".into(),
            netmask: "255.255.255.0".into(),
            gateway: "192.0.2.1".into(),
        }
    }
}

const NETWORK_KEYS: [&str; 3] = ["IP_ADDRESS", "NETMASK", "GATEWAY"];

fn onoff(b: bool) -> &'static str {
    if b { "1,(Enabled)" } else { "0,(Disabled)" }
}

impl Settings {
    /// The export text, CRLF.
    #[must_use]
    pub fn export(&self) -> String {
        let mut l: Vec<String> = vec![
            "[System Information]".into(),
            "MODEL=ioLogik E1213 (simulated)".into(),
            "FIRMWARE=V3.1".into(),
            "[1. Model]".into(),
            format!("MOD_DESC={}", self.mod_desc),
            format!("MOD_LOC={}", self.mod_loc),
            "[2. I/O Configurations]".into(),
        ];
        for (i, on) in self.safe_mode.iter().enumerate() {
            l.push(format!(
                "DO{i:02}_SAFE={}",
                if *on { "1,(On)" } else { "0,(Off)" }
            ));
        }
        l.push("[4. Network Settings]".into());
        l.push(format!("IP_ADDRESS={}", self.ip_address));
        l.push(format!("NETMASK={}", self.netmask));
        l.push(format!("GATEWAY={}", self.gateway));
        l.push(if self.idle_enable {
            format!("IDLE_TIME={}", self.idle_time)
        } else {
            "IDLE_TIME=0,(Disabled)".into()
        });
        l.push(format!("CONNECTION_WATCHDOG={}", onoff(self.wd_enable)));
        l.push(format!("WD_TIME={}", self.wd_time));
        l.push(format!("AUTOCLEAR_SAFEMODE={}", u8::from(self.safe_autoclear)));
        l.push("[9. Protocol Enable]".into());
        l.push(format!("REST_ENABLE={}", u8::from(self.rest_enable)));
        l.push(format!("EIP_ENABLE={}", u8::from(self.eip_enable)));
        l.push("<END>".into());
        let mut s = l.join("\r\n");
        s.push_str("\r\n");
        s
    }

    /// One export line applied; unknown keys and bad values are ignored,
    /// and the network keys only if `allow_network`.
    fn apply_key(&mut self, key: &str, value: &str, allow_network: bool) {
        let flag = || match value {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        };
        if NETWORK_KEYS.contains(&key) && !allow_network {
            return;
        }
        match key {
            "MOD_DESC" => self.mod_desc = value.into(),
            "MOD_LOC" => self.mod_loc = value.into(),
            "IP_ADDRESS" => self.ip_address = value.into(),
            "NETMASK" => self.netmask = value.into(),
            "GATEWAY" => self.gateway = value.into(),
            "IDLE_TIME" => {
                if let Ok(t) = value.parse::<u32>() {
                    self.idle_enable = t != 0;
                    if t != 0 {
                        self.idle_time = t;
                    }
                }
            }
            "WD_TIME" => {
                if let Ok(t) = value.parse() {
                    self.wd_time = t;
                }
            }
            "CONNECTION_WATCHDOG" => self.wd_enable = flag().unwrap_or(self.wd_enable),
            "AUTOCLEAR_SAFEMODE" => self.safe_autoclear = flag().unwrap_or(self.safe_autoclear),
            "REST_ENABLE" => self.rest_enable = flag().unwrap_or(self.rest_enable),
            "EIP_ENABLE" => self.eip_enable = flag().unwrap_or(self.eip_enable),
            k => {
                if let Some(i) = k
                    .strip_prefix("DO")
                    .and_then(|r| r.strip_suffix("_SAFE"))
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|&i| i < 8)
                {
                    self.safe_mode[i] = flag().unwrap_or(self.safe_mode[i]);
                }
            }
        }
    }
}

/// The shape of a reboot, mapped on a real E1213 (about 21 s: HTTP down
/// at 1.1 s, up again at 11.4 s with Modbus still down, down again at
/// 14.0 s, up with Modbus at 20.0 s). The phases are counted in HTTP
/// connection attempts, not seconds, so tests are fast and deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Restart {
    /// HTTP connections dropped first.
    pub down1: u32,
    /// HTTP connections then answered while TCP "502" is still down.
    pub up1: u32,
    /// HTTP connections dropped again.
    pub down2: u32,
}

impl Default for Restart {
    fn default() -> Self {
        Self {
            down1: 2,
            up1: 2,
            down2: 2,
        }
    }
}

/// How a [`Sim`] starts.
#[derive(Debug, Clone)]
pub struct Options {
    /// The reboot after a `set_ok.htm` or an import.
    pub restart: Restart,
    /// The first `ChallID` cookie.
    pub challenge: u32,
    /// The console's password (`None`: none set).
    pub password: Option<String>,
    /// Starting settings (staged and running).
    pub settings: Settings,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            restart: Restart::default(),
            challenge: 4739,
            password: None,
            settings: Settings::default(),
        }
    }
}

/// One request the simulator answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logged {
    pub method: String,
    /// The request target, path and query as received.
    pub target: String,
    /// Whether the request carried the `ChallID` and `MoxaPass` cookies.
    pub cookies_sent: bool,
}

impl Logged {
    /// The path without its query.
    #[must_use]
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }
}

struct State {
    staged: Settings,
    running: Settings,
    password: Option<String>,
    challenge: u32,
    tokens: HashMap<String, String>,
    counter: u64,
    drop_next: u32,
    /// Phases of the reboot still to go: (down1, up1, down2) remaining.
    phase: (u32, u32, u32),
    restart: Restart,
    modbus: Option<TcpListener>,
    modbus_addr: SocketAddr,
    dropped: u32,
    reboots: u32,
    log: Vec<Logged>,
}

struct Shared {
    st: Mutex<State>,
    stop: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A running simulated console. Dropping it stops the thread.
pub struct Sim {
    addr: SocketAddr,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Sim {
    /// A console with a default unit, on a free port of `127.0.0.1`.
    ///
    /// # Panics
    /// If the loopback listener cannot be bound.
    #[must_use]
    pub fn start() -> Self {
        Self::start_with(Options::default())
    }

    /// # Panics
    /// If the loopback listener cannot be bound.
    #[must_use]
    pub fn start_with(o: Options) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let modbus = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let modbus_addr = modbus.local_addr().expect("local addr");
        let shared = Arc::new(Shared {
            st: Mutex::new(State {
                staged: o.settings.clone(),
                running: o.settings,
                password: o.password,
                challenge: o.challenge,
                tokens: HashMap::new(),
                counter: 0,
                drop_next: 0,
                phase: (0, 0, 0),
                restart: o.restart,
                modbus: Some(modbus),
                modbus_addr,
                dropped: 0,
                reboots: 0,
                log: Vec::new(),
            }),
            stop: AtomicBool::new(false),
        });
        let s = Arc::clone(&shared);
        let thread = std::thread::spawn(move || serve(&listener, &s));
        Self {
            addr,
            shared,
            thread: Some(thread),
        }
    }

    /// `127.0.0.1:port`, for [`super::Console::connect`].
    #[must_use]
    pub fn addr(&self) -> String {
        self.addr.to_string()
    }

    #[must_use]
    pub fn staged(&self) -> Settings {
        self.shared.lock().staged.clone()
    }

    #[must_use]
    pub fn running(&self) -> Settings {
        self.shared.lock().running.clone()
    }

    #[must_use]
    pub fn password(&self) -> Option<String> {
        self.shared.lock().password.clone()
    }

    /// The `ChallID` the console currently hands out.
    #[must_use]
    pub fn challenge(&self) -> String {
        self.shared.lock().challenge.to_string()
    }

    /// Close the next `n` connections without a reply.
    pub fn drop_next(&self, n: u32) {
        self.shared.lock().drop_next = n;
    }

    /// Connections dropped by [`Sim::drop_next`] or a restart so far.
    #[must_use]
    pub fn dropped(&self) -> u32 {
        self.shared.lock().dropped
    }

    /// The shape of the next reboot.
    pub fn set_restart(&self, r: Restart) {
        self.shared.lock().restart = r;
    }

    /// The port that stands for TCP 502: it accepts only when the unit is
    /// fully up, and refuses while it reboots. Give it to
    /// [`super::Config::modbus_port`].
    #[must_use]
    pub fn modbus_port(&self) -> u16 {
        self.shared.lock().modbus_addr.port()
    }

    /// Whether the unit is fully up (no reboot in progress).
    #[must_use]
    pub fn is_up(&self) -> bool {
        self.shared.lock().phase == (0, 0, 0)
    }

    /// Reboots begun so far (restarts and imports).
    #[must_use]
    pub fn reboots(&self) -> u32 {
        self.shared.lock().reboots
    }

    /// Every request answered, oldest first.
    #[must_use]
    pub fn requests(&self) -> Vec<Logged> {
        self.shared.lock().log.clone()
    }

    /// Requests answered for `path` (without the query).
    #[must_use]
    pub fn count(&self, path: &str) -> usize {
        self.shared.lock().log.iter().filter(|l| l.path() == path).count()
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One HTTP connection attempt moves a reboot along; true if it is dropped.
fn advance(st: &mut State) -> bool {
    let (d1, u1, d2) = st.phase;
    let dropped = if d1 > 0 {
        st.phase.0 -= 1;
        true
    } else if u1 > 0 {
        st.phase.1 -= 1;
        false
    } else if d2 > 0 {
        st.phase.2 -= 1;
        true
    } else {
        false
    };
    if st.phase == (0, 0, 0) && st.modbus.is_none() {
        // Up for good: the Modbus port listens again.
        st.modbus = TcpListener::bind(st.modbus_addr).ok();
    }
    dropped
}

/// The unit reboots: staged settings become running, the `ChallID` and the
/// tokens are new, HTTP and the Modbus port go away.
fn begin_reboot(st: &mut State) {
    st.running = st.staged.clone();
    st.challenge += 1;
    st.tokens.clear();
    st.reboots += 1;
    st.modbus = None;
    let r = st.restart;
    st.phase = (r.down1, r.up1, r.down2);
    if st.phase == (0, 0, 0) {
        st.modbus = TcpListener::bind(st.modbus_addr).ok();
    }
}

fn serve(listener: &TcpListener, shared: &Shared) {
    for conn in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(mut s) = conn else { continue };
        {
            let mut st = shared.lock();
            if advance(&mut st) {
                st.dropped += 1;
                continue;
            }
            if st.drop_next > 0 {
                st.drop_next -= 1;
                st.dropped += 1;
                continue;
            }
        }
        let _ = handle_conn(&mut s, shared);
    }
}

struct Request {
    method: String,
    target: String,
    cookie: Option<String>,
    content_type: String,
    body: Vec<u8>,
}

struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    set_cookies: Vec<String>,
}

fn html(body: &str) -> Response {
    Response {
        status: 200,
        content_type: "text/html",
        body: format!("<HTML><BODY>{body}</BODY></HTML>").into_bytes(),
        set_cookies: Vec::new(),
    }
}

fn handle_conn(s: &mut TcpStream, shared: &Shared) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    let Some(req) = read_request(s)? else {
        return Ok(());
    };
    let resp = {
        let mut st = shared.lock();
        let cookies_sent = req
            .cookie
            .as_deref()
            .is_some_and(|c| c.contains("ChallID=") && c.contains("MoxaPass="));
        st.log.push(Logged {
            method: req.method.clone(),
            target: req.target.clone(),
            cookies_sent,
        });
        route(&mut st, &req)
    };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nServer: ioLogik Web Server/1.0\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        resp.status,
        if resp.status == 200 { "OK" } else { "Not Found" },
        resp.content_type,
        resp.body.len()
    );
    for c in &resp.set_cookies {
        head.push_str("Set-Cookie: ");
        head.push_str(c);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())?;
    s.write_all(&resp.body)?;
    s.flush()
}

fn read_request(s: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > 16 * 1024 {
            return Ok(None);
        }
        let n = s.read(&mut chunk)?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, target) = (
        first.next().unwrap_or("").to_string(),
        first.next().unwrap_or("").to_string(),
    );
    let mut cookie = None;
    let mut content_type = String::new();
    let mut len = 0usize;
    for l in lines {
        if let Some((n, v)) = l.split_once(':') {
            match n.trim().to_ascii_lowercase().as_str() {
                "cookie" => cookie = Some(v.trim().to_string()),
                "content-type" => content_type = v.trim().to_string(),
                "content-length" => len = v.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    if len > 2 * 1024 * 1024 {
        return Ok(None);
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = s.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    Ok(Some(Request {
        method,
        target,
        cookie,
        content_type,
        body,
    }))
}

/// Percent-decoding of a query, each byte one Latin-1 character, `+` a space.
pub(crate) fn decode_query(q: &str) -> Vec<(String, String)> {
    fn dec(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'+' => out.push(' '),
                b'%' if i + 2 < b.len() => {
                    let v = std::str::from_utf8(&b[i + 1..i + 3])
                        .ok()
                        .and_then(|h| u8::from_str_radix(h, 16).ok());
                    if let Some(v) = v {
                        out.push(char::from(v));
                        i += 2;
                    } else {
                        out.push('%');
                    }
                }
                c => out.push(char::from(c)),
            }
            i += 1;
        }
        out
    }
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (dec(k), dec(v))
        })
        .collect()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn issue(st: &mut State, code: &str) -> String {
    st.counter += 1;
    let t = format!("{:08x}", st.counter.wrapping_mul(0x9E37_79B1) & 0xFFFF_FFFF);
    st.tokens.insert(code.to_string(), t.clone());
    t
}

fn checked(on: bool) -> &'static str {
    if on { " checked" } else { "" }
}

const COMPLETE: &str = "<H1>Configuration Complete</H1><P>The changes will not take effect until you Save/Restart the I/O Server.</P>";

#[allow(clippy::too_many_lines)]
fn route(st: &mut State, req: &Request) -> Response {
    let (path, query) = req.target.split_once('?').unwrap_or((&req.target, ""));
    match (req.method.as_str(), path) {
        ("GET", "/") => {
            let mut r = html("<FRAMESET></FRAMESET>");
            r.set_cookies = vec![
                format!("ChallID={}; path=/", st.challenge),
                format!(
                    "MoxaPass={:08x}; path=/",
                    st.challenge.wrapping_mul(2_654_435_761)
                ),
            ];
            r
        }
        ("GET", "/06_6.htm") => html("<A href=\"ik1213.txt\">Export</A>"),
        ("GET", "/ik1213.txt") => Response {
            status: 200,
            content_type: "text/plain",
            body: st.staged.export().into_bytes(),
            set_cookies: Vec::new(),
        },
        ("GET", "/04_1.htm") => {
            let t = issue(st, "41");
            let s = &st.staged;
            html(&format!(
                "<FORM method=\"GET\" action=\"set.htm\">\
                 <input type=hidden name=token value=\"{t}\">\
                 <INPUT type=\"text\" name=\"MOD_DESC\" value=\"{}\">\
                 <input type=\"text\" name=\"MOD_LOC\" value=\"{}\">\
                 <input type=\"checkbox\" name=\"EnableIDLE\" value=\"1\"{}>\
                 <input type=\"text\" name=\"IdleTime\" value=\"{}\">\
                 <input type=\"checkbox\" name=\"EnableWD\" value=\"1\"{}>\
                 <input type=\"text\" name=\"WDTime\" value=\"{}\">\
                 <input type=\"checkbox\" name=\"EnableSafeModeAutoClear\" value=\"1\"{}>\
                 <input type=\"submit\" name=\"Submit\" value=\"Submit\">\
                 <input type=\"hidden\" name=\"code\" value=\"41\"></FORM>\
                 <SCRIPT>document.write('<input name=\"injected\" value=\"1\">');</SCRIPT>",
                esc(&s.mod_desc),
                esc(&s.mod_loc),
                checked(s.idle_enable),
                s.idle_time,
                checked(s.wd_enable),
                s.wd_time,
                checked(s.safe_autoclear),
            ))
        }
        ("GET", "/11_safeM.htm") => {
            let t = issue(st, "116");
            let mut f = format!("<form method=get action=set.htm><input type=hidden name=token value={t}>");
            for (i, on) in st.staged.safe_mode.iter().enumerate() {
                let _ = write!(
                    f,
                    "<select name=\"SetSM{i:02}\"><option value=\"0\"{}>Off</option><option value=\"1\"{}>On</option></select>",
                    if *on { "" } else { " selected" },
                    if *on { " selected" } else { "" },
                );
            }
            f.push_str("<input type=submit name=Clear_Safe_Flag1 value=Clear><input type=submit name=Submit value=Submit><input type=hidden name=code value=116></form>");
            html(&f)
        }
        ("GET", "/13_1.htm") => {
            let t = issue(st, "131");
            html(&format!(
                "<form method=get action=set.htm><input type=hidden name=token value={t}>\
                 <input type=checkbox name=REST_ENABLE value=1{}>\
                 <input type=submit name=Submit value=Submit><input type=hidden name=code value=131></form>",
                checked(st.staged.rest_enable)
            ))
        }
        ("GET", "/14_1.htm") => {
            let t = issue(st, "141");
            html(&format!(
                "<form method=get action=set.htm><input type=hidden name=token value={t}>\
                 <input type=hidden name=eipActed value=1>\
                 <input type=checkbox name=EIP_ENABLE value=1{}>\
                 <input type=submit name=Submit value=Submit><input type=hidden name=code value=141></form>",
                checked(st.staged.eip_enable)
            ))
        }
        ("GET", "/07.htm") => {
            let t = issue(st, "7");
            html(&format!(
                "<form method=get action=set.htm><input type=hidden name=token value={t}>\
                 <input type=hidden name=code value=7>\
                 <input type=password name=OldPassword><input type=password name=NewPassword>\
                 <input type=password name=ReTypePassword>\
                 <input type=hidden name=OldPwd><input type=hidden name=NewPwd><input type=hidden name=ChallID>\
                 <input type=submit name=Submit value=Submit></form>"
            ))
        }
        ("GET", "/06_5.htm") => {
            let t = issue(st, "import");
            html(&format!(
                "<form method=post action=06_5_1.htm enctype=\"multipart/form-data\">\
                 <input type=hidden name=token value={t}>\
                 <input type=checkbox name=NetConfig_OverWrite value=1 checked>\
                 <input type=file name=importfile><input type=submit name=import value=Import></form>"
            ))
        }
        ("GET", "/set.htm") => set(st, &decode_query(query)),
        ("GET", "/set_ok.htm") => {
            begin_reboot(st);
            html("<H1>Save/Restart Request</H1>")
        }
        ("POST", "/06_5_1.htm") => import(st, req),
        // Actions the client must never reach by a plain page fetch; the log shows if it did.
        ("GET", "/08.htm" | "/09.htm") => html("<H1>An action page</H1>"),
        _ => Response {
            status: 404,
            content_type: "text/html",
            body: b"<HTML>not found</HTML>".to_vec(),
            set_cookies: Vec::new(),
        },
    }
}

fn refuse(why: &str) -> Response {
    html(&format!("<H1>Not accepted</H1><P>{}</P>", esc(why)))
}

fn set(st: &mut State, q: &[(String, String)]) -> Response {
    let get = |n: &str| q.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
    let has = |n: &str| q.iter().any(|(k, _)| k == n);
    let (Some(code), Some(token)) = (get("code"), get("token")) else {
        return refuse("missing code or token");
    };
    // ASSUMPTION about the real unit: only the latest token issued for this
    // page's code is accepted.
    if st.tokens.get(code).map(String::as_str) != Some(token) {
        return refuse("stale or unknown token");
    }
    let mut n = st.staged.clone();
    match code {
        "41" => {
            n.mod_desc = get("MOD_DESC").unwrap_or("").into();
            n.mod_loc = get("MOD_LOC").unwrap_or("").into();
            n.idle_enable = has("EnableIDLE");
            if n.idle_enable {
                match get("IdleTime")
                    .and_then(|v| v.parse::<u32>().ok())
                    .filter(|&v| v <= 65_535)
                {
                    Some(v) => n.idle_time = v,
                    None => return refuse("IdleTime"),
                }
            }
            n.wd_enable = has("EnableWD");
            if n.wd_enable {
                match get("WDTime")
                    .and_then(|v| v.parse::<u32>().ok())
                    .filter(|&v| v <= 65_535)
                {
                    Some(v) => n.wd_time = v,
                    None => return refuse("WDTime"),
                }
            }
            n.safe_autoclear = has("EnableSafeModeAutoClear");
        }
        "116" => {
            for i in 0..8 {
                match get(&format!("SetSM{i:02}")) {
                    Some("0") => n.safe_mode[i] = false,
                    Some("1") => n.safe_mode[i] = true,
                    _ => return refuse("SetSM"),
                }
            }
        }
        "131" => n.rest_enable = has("REST_ENABLE"),
        "141" => n.eip_enable = has("EIP_ENABLE"),
        "7" => return change_password(st, get("OldPwd"), get("NewPwd"), get("ChallID")),
        _ => return refuse("unknown page code"),
    }
    st.staged = n;
    html(COMPLETE)
}

fn change_password(st: &mut State, old: Option<&str>, new: Option<&str>, chall: Option<&str>) -> Response {
    let c = st.challenge.to_string();
    if chall != Some(hex(&md5(c.as_bytes())).as_str()) {
        return refuse("ChallID");
    }
    let current = st.password.clone().unwrap_or_default();
    if old != Some(hex(&md5(current.as_bytes())).as_str()) {
        return refuse("old password");
    }
    let Some(pw) = new.and_then(|n| password::unmask(&c, n)) else {
        return refuse("new password");
    };
    st.password = (!pw.is_empty()).then_some(pw);
    html(COMPLETE)
}

fn import(st: &mut State, req: &Request) -> Response {
    let Some(parts) = multipart(&req.content_type, &req.body) else {
        return refuse("not multipart");
    };
    let part = |n: &str| parts.iter().find(|(k, _)| k == n).map(|(_, v)| v);
    let token_ok = part("token")
        .and_then(|t| String::from_utf8(t.clone()).ok())
        .is_some_and(|t| st.tokens.get("import") == Some(&t));
    if !token_ok {
        return refuse("stale or unknown token");
    }
    let Some(file) = part("importfile") else {
        return refuse("no file");
    };
    let text: String = file.iter().map(|&b| char::from(b)).collect();
    let Ok(export) = Export::parse(&text) else {
        return refuse("not an export");
    };
    let overwrite = part("NetConfig_OverWrite").is_some();
    for e in export.sections().iter().flat_map(|s| &s.entries) {
        st.staged.apply_key(&e.key, &e.value, overwrite);
    }
    begin_reboot(st);
    html("<H1>System configuration file imported and restart successfully.</H1>")
}

/// `(field name, content)` of each part of a `multipart/form-data` body.
fn multipart(content_type: &str, body: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let boundary = content_type.split("boundary=").nth(1)?.trim();
    let delim = format!("--{boundary}");
    let d = delim.as_bytes();
    let mut at: Vec<usize> = Vec::new();
    let mut i = 0;
    while i + d.len() <= body.len() {
        if &body[i..i + d.len()] == d {
            at.push(i);
            i += d.len();
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for w in at.windows(2) {
        let part = &body[w[0] + d.len()..w[1]];
        let part = part.strip_prefix(b"\r\n")?;
        let split = part.windows(4).position(|x| x == b"\r\n\r\n")?;
        let headers = String::from_utf8_lossy(&part[..split]).into_owned();
        let name = headers.split("name=\"").nth(1)?.split('"').next()?.to_string();
        let content = part[split + 4..]
            .strip_suffix(b"\r\n")
            .unwrap_or(&part[split + 4..]);
        out.push((name, content.to_vec()));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// What the client encodes, the simulator decodes, byte for byte.
        #[test]
        fn client_encoding_round_trips_through_the_sim(s in "[\\x00-\\u{ff}]{0,100}") {
            let e = format!("k={}", super::super::form::encode(&s));
            let d = decode_query(&e);
            prop_assert_eq!(d, vec![("k".to_string(), s)]);
        }
    }

    #[test]
    fn export_text_parses_and_names_the_cited_keys() {
        let e = Export::parse(&Settings::default().export()).unwrap();
        assert_eq!(
            e.find("CONNECTION_WATCHDOG").unwrap().label.as_deref(),
            Some("Disabled")
        );
        assert_eq!(e.find("DO03_SAFE").unwrap().value, "0");
        assert_eq!(e.find("EIP_ENABLE").unwrap().value, "1");
    }
}
