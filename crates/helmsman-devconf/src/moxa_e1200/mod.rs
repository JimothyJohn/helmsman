//! The Moxa ioLogik E1200 web console as a protocol: read the configuration
//! export, read a page's form, change fields and submit, restart, restore
//! from an export, change the password. The console is mapped in
//! `devices/moxa/iologik-e1200-web-console.md` (helmsman-devices); the
//! Python client there (`scripts/moxa-e1200-console.py`) is the behavioural
//! reference.
//!
//! What this does not do, because it is not known how:
//!
//! - **Log in to a console that has a password.** No unit with a password
//!   has been seen, so how it asks for one is unmapped. Only a console
//!   without a password is supported (firmware 3.1 ships without one; 3.3
//!   and later default to "moxa"), plus the password-change request below.
//! - **Confirm a password change on a real unit.** The reply to the change
//!   has not been seen; success here is "the console said Configuration
//!   Complete", which is an assumption (see [`Console::change_password`]).
//! - **Firmware update, Load Factory Default.** `08.htm` is an action and
//!   nothing here fetches it.
//!
//! A write changes the *staged* settings. The export shows the staged
//! settings, and the unit runs them only after [`Console::restart`].

mod export;
mod form;
mod http;
mod password;
#[cfg(feature = "sim")]
pub mod sim;

use std::fmt;
use std::time::{Duration, Instant};

pub use export::{Diff, Entry, Export, MAX_EXPORT, Section};
pub use form::{Field, Form, Kind, encode, query};
pub use password::MAX_PASSWORD;

use http::Http;

/// Pages that act when fetched. Nothing but the named operation may GET
/// them: `08.htm` loads factory defaults, `09.htm` and `set_ok.htm`
/// restart, `set.htm` writes, `06_5_1.htm` imports, `06_4_1.htm` updates
/// the firmware.
const ACTION_PAGES: [&str; 6] = [
    "08.htm",
    "09.htm",
    "set.htm",
    "set_ok.htm",
    "06_5_1.htm",
    "06_4_1.htm",
];

/// Why an operation failed. Never carries a header value, a body or a
/// password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Refused before anything was sent: the input broke a rule.
    Refused(String),
    /// Could not reach the console.
    Unreachable(String),
    /// No reply within the deadline.
    Timeout,
    /// The connection failed.
    Io(std::io::ErrorKind),
    /// The connection closed before the console sent a byte: the dropped
    /// connection the console is known for.
    NoResponse,
    /// The console broke HTTP.
    Protocol(&'static str),
    /// A head or body past its cap.
    TooLarge,
    /// A 3xx; redirects are never followed.
    Redirect(u16),
    /// A status other than 200.
    Status(u16),
    /// A page that is not the shape this client reads.
    Page(&'static str),
    /// The console did not answer "Configuration Complete" (a stale token
    /// is one cause; the console does not say which).
    NotAccepted,
    /// A password change that the console did not confirm. The reply to it
    /// is unmapped on a real unit: the password may or may not have changed.
    PasswordUnconfirmed,
    /// After a restart request the console never stopped answering.
    RestartNotObserved,
    /// After a restart the console did not answer again in time.
    RestartTimeout,
}

impl Error {
    /// A failure of the transport (retried on a read, never on a write).
    #[must_use]
    pub fn is_transport(&self) -> bool {
        matches!(
            self,
            Self::Unreachable(_) | Self::Timeout | Self::Io(_) | Self::NoResponse
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(why) => write!(f, "refused, nothing sent: {why}"),
            Self::Unreachable(why) => write!(f, "unreachable: {why}"),
            Self::Timeout => f.write_str("timed out"),
            Self::Io(k) => write!(f, "connection failed: {k}"),
            Self::NoResponse => f.write_str("the console closed the connection without answering"),
            Self::Protocol(why) => write!(f, "bad response: {why}"),
            Self::TooLarge => f.write_str("response larger than is read"),
            Self::Redirect(s) => write!(f, "redirect ({s}), not followed"),
            Self::Status(s) => write!(f, "the console answered {s}"),
            Self::Page(why) => write!(f, "unexpected page: {why}"),
            Self::NotAccepted => f.write_str(
                "the console did not answer Configuration Complete (a stale token is one cause)",
            ),
            Self::PasswordUnconfirmed => f.write_str(
                "the console did not confirm the password change; its reply is unmapped, so the password may or may not have changed",
            ),
            Self::RestartNotObserved => f.write_str(
                "the console kept answering after the restart request; whether it restarted is unknown",
            ),
            Self::RestartTimeout => f.write_str("the console did not come back after the restart in time"),
        }
    }
}

impl std::error::Error for Error {}

/// Timing and size limits. The defaults are the Python client's (0.3 s
/// between requests, a read retried once) with bounds on everything.
#[derive(Debug, Clone)]
pub struct Config {
    /// Longest wait to connect.
    pub connect: Duration,
    /// One whole request, connect to last byte.
    pub request: Duration,
    /// Least time between the end of one request and the start of the
    /// next: the console drops a connection that comes right after.
    pub gap: Duration,
    /// Wait before the one retry of a read.
    pub retry_wait: Duration,
    /// Response head cap.
    pub max_head: usize,
    /// Response body cap.
    pub max_body: usize,
    /// After a restart request: longest wait for the console to stop answering.
    pub restart_down: Duration,
    /// After it went down: longest wait for it to answer again.
    pub restart_up: Duration,
    /// Between polls while waiting.
    pub poll: Duration,
    /// The TCP port that must accept before a restart counts as finished.
    pub modbus_port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            request: Duration::from_secs(10),
            gap: Duration::from_millis(300),
            retry_wait: Duration::from_secs(1),
            max_head: 16 * 1024,
            max_body: MAX_EXPORT,
            restart_down: Duration::from_secs(15),
            restart_up: Duration::from_secs(120),
            modbus_port: 502,
            poll: Duration::from_secs(1),
        }
    }
}

/// A session with one console.
pub struct Console {
    http: Http,
}

/// `[0-9A-Za-z_]{1,32}.htm`.
fn check_page(page: &str) -> Result<(), Error> {
    let stem = page.strip_suffix(".htm").unwrap_or("");
    if stem.is_empty() || stem.len() > 32 || !stem.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(Error::Refused(
            "a page is a console page such as 04_1.htm ([0-9A-Za-z_]{1,32}.htm)".into(),
        ));
    }
    if ACTION_PAGES.contains(&page.to_ascii_lowercase().as_str()) {
        return Err(Error::Refused(format!(
            "{page} acts when fetched; it is reached only by its own operation"
        )));
    }
    Ok(())
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

impl Console {
    /// Opens a session: `GET /` for the cookies the console sets
    /// (`ChallID`, `MoxaPass`) that every later request sends back.
    ///
    /// # Errors
    /// [`Error::Refused`] for a bad address, otherwise the transport's.
    pub fn connect(addr: &str, cfg: Config) -> Result<Self, Error> {
        let (host, port) = http::parse_addr(addr)?;
        let mut c = Self {
            http: Http::new(host, port, cfg),
        };
        c.fetch("/")?;
        Ok(c)
    }

    fn fetch(&mut self, path: &str) -> Result<String, Error> {
        let r = self.http.get(path)?;
        if r.status != 200 {
            return Err(Error::Status(r.status));
        }
        Ok(latin1(&r.body))
    }

    /// The configuration export (`ik12NN.txt`, named by `06_6.htm`). It
    /// shows the staged settings.
    ///
    /// # Errors
    /// The transport's, or [`Error::Page`].
    pub fn export(&mut self) -> Result<Export, Error> {
        let overview = self.fetch("/06_6.htm")?;
        let name = export_name(&overview).ok_or(Error::Page("06_6.htm names no ik12NN.txt export"))?;
        let text = self.fetch(&format!("/{name}"))?;
        Export::parse(&text)
    }

    /// The settings form on `page` (the first with a `code` field), as the
    /// page was just served: its `token` is fresh.
    ///
    /// # Errors
    /// [`Error::Refused`] for a page name outside the pattern or an action
    /// page; the transport's; [`Error::Page`] with no settings form.
    pub fn form(&mut self, page: &str) -> Result<Form, Error> {
        check_page(page)?;
        let html = self.fetch(&format!("/{page}"))?;
        form::parse(&html)?
            .into_iter()
            .find(|f| f.field("code").is_some())
            .ok_or(Error::Page("the page has no settings form (no code field)"))
    }

    /// Changes `changes` on `page` and returns the export lines that
    /// changed. The change is staged: it runs after [`Console::restart`].
    /// A write is never retried.
    ///
    /// # Errors
    /// [`Error::Refused`] (an unknown field, `token` or `code`, `07.htm`:
    /// nothing sent), [`Error::NotAccepted`], the transport's.
    pub fn set(&mut self, page: &str, changes: &[(&str, &str)]) -> Result<Diff, Error> {
        if page.eq_ignore_ascii_case("07.htm") {
            return Err(Error::Refused(
                "07.htm is the password: use change_password".into(),
            ));
        }
        let mut f = self.form(page)?;
        f.apply(changes)?;
        let before = self.export()?;
        self.submit(&f)?;
        let after = self.export()?;
        Ok(Diff::between(&before, &after))
    }

    /// Submits `form` as it stands: `GET /set.htm?<its fields>`. For a form
    /// changed with [`Form::apply`]; [`Console::set`] is this plus the
    /// export diff. Never retried.
    ///
    /// # Errors
    /// [`Error::Refused`] if the form does not post to `set.htm` by GET,
    /// [`Error::NotAccepted`], the transport's.
    pub fn submit(&mut self, form: &Form) -> Result<(), Error> {
        let action = form.action.trim().trim_start_matches('/').to_ascii_lowercase();
        if action != "set.htm" || form.method != "get" {
            return Err(Error::Refused(
                "the form does not submit to set.htm by GET".into(),
            ));
        }
        if form.value("token").is_none() || form.value("code").is_none() {
            return Err(Error::Refused("the form has no token or code".into()));
        }
        let reply = self
            .http
            .once("GET", &format!("/set.htm?{}", form.query()), None, None)?;
        if reply.status != 200 {
            return Err(Error::Status(reply.status));
        }
        if !String::from_utf8_lossy(&reply.body).contains("Configuration Complete") {
            return Err(Error::NotAccepted);
        }
        Ok(())
    }

    /// Save/Restart: `GET /set_ok.htm?token=`. The reply comes in about
    /// 0.1 s, then the unit reboots (about 21 s on the real E1213) and runs
    /// the staged settings; they survive the reboot. HTTP, Modbus and
    /// EtherNet/IP go away and HTTP comes back twice, so this waits for
    /// the whole return (see `wait_back`: bounded by [`Config::restart_down`]
    /// and [`Config::restart_up`]). The request is never retried.
    ///
    /// # Errors
    /// The transport's if the request could not be sent;
    /// [`Error::RestartNotObserved`], [`Error::RestartTimeout`].
    pub fn restart(&mut self) -> Result<(), Error> {
        match self.http.once("GET", "/set_ok.htm?token=", None, None) {
            Ok(r) if r.status != 200 => return Err(Error::Status(r.status)),
            // A unit that goes down mid-reply is the expected case.
            Ok(_) | Err(Error::NoResponse | Error::Io(_) | Error::Timeout | Error::Protocol(_)) => {}
            Err(e) => return Err(e),
        }
        self.wait_back()
    }

    /// Waits for a reboot to finish. The real unit's Save/Restart takes
    /// about 21 s and HTTP answers twice: down at 1.1 s, up again at
    /// 11.4 s while Modbus (TCP 502) is still down, down again at 14.0 s,
    /// up for good at 20.0 s. So "back" is: HTTP was seen down, and
    /// afterwards HTTP answers and TCP [`Config::modbus_port`] accepts. The
    /// EtherNet/IP port is not required (it is off when the protocol is
    /// disabled). Bounded by [`Config::restart_down`] (to see it go down)
    /// and [`Config::restart_up`] (to come back). Takes fresh cookies.
    fn wait_back(&mut self) -> Result<(), Error> {
        let probe = self.http.cfg().request.min(Duration::from_secs(2));
        let (down, up, poll, modbus) = {
            let c = self.http.cfg();
            (c.restart_down, c.restart_up, c.poll, c.modbus_port)
        };
        let start = Instant::now();
        while self.http.once("GET", "/", None, Some(probe)).is_ok() {
            if start.elapsed() > down {
                return Err(Error::RestartNotObserved);
            }
            std::thread::sleep(poll);
        }
        let went_down = Instant::now();
        loop {
            if self.http.once("GET", "/", None, Some(probe)).is_ok() && self.http.tcp_open(modbus, probe) {
                break;
            }
            if went_down.elapsed() > up {
                return Err(Error::RestartTimeout);
            }
            std::thread::sleep(poll);
        }
        // A reboot may issue a new ChallID: start the session over.
        self.http.forget_cookies();
        self.fetch("/")?;
        Ok(())
    }

    /// Restores a configuration export (`06_5.htm` to `06_5_1.htm`, a
    /// multipart POST). The file's network settings (the IP) overwrite the
    /// unit's only if `overwrite_network` is true: the page ticks that box
    /// by default, this does not send it unless asked. Returns the export
    /// lines the import changed. Never retried.
    ///
    /// **An import reboots the unit by itself** (mapped on a real E1213,
    /// 2026-11-12), so this waits for it to come back the way
    /// [`Console::restart`] does and the imported settings are already
    /// running. Importing a unit's own export without the network option
    /// changed nothing in its export. The reply is not parsed: success is
    /// judged by the export afterwards, which the returned diff is.
    ///
    /// # Errors
    /// [`Error::Refused`] for a file that is not an export, [`Error::Page`],
    /// the transport's.
    pub fn import(&mut self, file: &[u8], overwrite_network: bool) -> Result<Diff, Error> {
        let text = latin1(file);
        let parsed = Export::parse(&text).map_err(|_| Error::Refused("not an export file".into()))?;
        if !parsed.sections().iter().any(|s| !s.name.is_empty()) {
            return Err(Error::Refused("not an export file (no [section])".into()));
        }
        let token = {
            let html = self.fetch("/06_5.htm")?;
            form::parse(&html)?
                .iter()
                .find_map(|f| f.value("token").map(str::to_string))
                .ok_or(Error::Page("06_5.htm has no form with a token"))?
        };
        let before = self.export()?;
        let boundary = boundary_for(file);
        let mut body: Vec<u8> = Vec::with_capacity(file.len() + 512);
        let mut text_part = |name: &str, value: &str| {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                    .as_bytes(),
            );
        };
        text_part("token", &token);
        if overwrite_network {
            text_part("NetConfig_OverWrite", "1");
        }
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"importfile\"; filename=\"import.txt\"\r\nContent-Type: text/plain\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(file);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"import\"\r\n\r\nImport\r\n--{boundary}--\r\n")
                .as_bytes(),
        );
        let ct = format!("multipart/form-data; boundary={boundary}");
        let reply = self.http.once("POST", "/06_5_1.htm", Some((&ct, &body)), None)?;
        if reply.status != 200 {
            return Err(Error::Status(reply.status));
        }
        // The import reboots the unit by itself (its reply says "System
        // configuration file imported and restart successfully."): wait
        // as a restart does, then read the export.
        self.wait_back()?;
        let after = self.export()?;
        Ok(Diff::between(&before, &after))
    }

    /// Changes the console's password from `old` to `new` (`07.htm`):
    /// `OldPwd`, `NewPwd` and `ChallID` computed from the `ChallID` cookie
    /// as the page's script does. `old` is empty on a console without a
    /// password. A password is printable ASCII, at most 16 characters; an
    /// empty new password is refused.
    ///
    /// ASSUMPTION: the console answers a successful change with the same
    /// "Configuration Complete" page `set.htm` does. No reply to a password
    /// change has been seen on a real unit; if it differs this reports
    /// [`Error::PasswordUnconfirmed`] after a change that worked.
    ///
    /// # Errors
    /// [`Error::Refused`] (nothing sent), [`Error::PasswordUnconfirmed`],
    /// the transport's.
    pub fn change_password(&mut self, old: &str, new: &str) -> Result<(), Error> {
        password::check(old)?;
        password::check(new)?;
        if new.is_empty() {
            return Err(Error::Refused("an empty new password is not supported".into()));
        }
        let f = self.form("07.htm")?;
        if f.value("code") != Some("7") {
            return Err(Error::Page("07.htm's form does not carry code 7"));
        }
        let token = f
            .value("token")
            .ok_or(Error::Page("07.htm's form has no token"))?;
        let action = f.action.trim().trim_start_matches('/');
        if f.method != "get"
            || action.is_empty()
            || check_page(action).is_err() && !action.eq_ignore_ascii_case("set.htm")
        {
            return Err(Error::Page(
                "07.htm's form does not submit by GET to a console page",
            ));
        }
        let cookie = self
            .http
            .cookie("ChallID")
            .ok_or(Error::Page("the console set no ChallID cookie"))?
            .to_string();
        let pairs: Vec<(String, String)> = [
            ("token", token.to_string()),
            ("code", "7".to_string()),
            ("OldPwd", password::old_pwd(old)),
            ("NewPwd", password::new_pwd(&cookie, new)),
            ("ChallID", password::chall_id(&cookie)),
            ("OldPassword", String::new()),
            ("NewPassword", String::new()),
            ("ReTypePassword", String::new()),
            ("Submit", "Submit".to_string()),
        ]
        .map(|(k, v)| (k.to_string(), v))
        .to_vec();
        let path = format!("/{action}?{}", query(&pairs));
        let reply = self.http.once("GET", &path, None, None)?;
        if reply.status != 200 {
            return Err(Error::Status(reply.status));
        }
        if String::from_utf8_lossy(&reply.body).contains("Configuration Complete") {
            Ok(())
        } else {
            Err(Error::PasswordUnconfirmed)
        }
    }
}

/// `ik12NN.txt` from the quoted link in `06_6.htm`.
fn export_name(page: &str) -> Option<String> {
    let b = page.as_bytes();
    (1..b.len()).find_map(|i| {
        let q = b[i - 1];
        let name = page.get(i..i + 10)?;
        let ok = (q == b'"' || q == b'\'')
            && name[..4].eq_ignore_ascii_case("ik12")
            && name.as_bytes()[4..6].iter().all(u8::is_ascii_digit)
            && name[6..].eq_ignore_ascii_case(".txt")
            && b.get(i + 10) == Some(&q);
        ok.then(|| name.to_ascii_lowercase())
    })
}

/// A multipart boundary the file does not contain.
fn boundary_for(file: &[u8]) -> String {
    let mut n = 0u32;
    loop {
        let b = format!("----helmsman-devconf-{n:08x}");
        if !file.windows(b.len()).any(|w| w == b.as_bytes()) {
            return b;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_names() {
        for ok in [
            "04_1.htm",
            "11_safeM.htm",
            "07.htm",
            "A.htm",
            &format!("{}.htm", "a".repeat(32)),
        ] {
            assert!(check_page(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            ".htm",
            "04_1",
            "04_1.html",
            "../04_1.htm",
            "a/b.htm",
            "a b.htm",
            "a.htm?x=1",
            "a.htm\r\n",
            &format!("{}.htm", "a".repeat(33)),
            "\u{e9}.htm",
            "08.htm",
            "09.htm",
            "set.htm",
            "set_ok.htm",
            "SET_OK.HTM",
            "06_5_1.htm",
            "06_4_1.htm",
        ] {
            assert!(matches!(check_page(bad), Err(Error::Refused(_))), "{bad:?}");
        }
    }

    #[test]
    fn export_link() {
        assert_eq!(
            export_name("<a href=\"ik1213.txt\">x</a>").as_deref(),
            Some("ik1213.txt")
        );
        assert_eq!(
            export_name("<A HREF='IK1242.TXT'>").as_deref(),
            Some("ik1242.txt")
        );
        for bad in [
            "",
            "ik1213.txt",
            "<a href=\"ik12ab.txt\">",
            "<a href=\"ik1213.txt'>",
            "\"ik121",
        ] {
            assert_eq!(export_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn boundaries_avoid_the_file() {
        let first = boundary_for(b"");
        let mut file = first.clone().into_bytes();
        file.extend_from_slice(b"junk");
        assert_ne!(boundary_for(&file), first);
    }
}
