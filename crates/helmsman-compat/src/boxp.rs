//! A box (`helmsman-plc`) built at some ref, started on loopback with its
//! API over plain HTTP, its own state directory and the `echo` program of
//! its own checkout. It dies with this process: `HELMSMAN_EXIT_WITH_STDIN`
//! and a piped stdin the child keeps, and a kill on drop.

use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use helmsman_agent::http::Url;

/// A running box.
pub struct BoxProc {
    child: Child,
    pub url: Url,
    pub token: String,
    log: Arc<Mutex<Vec<String>>>,
}

impl BoxProc {
    /// Starts `bin` with the checkout `helmsman` (for its echo program),
    /// a fresh state directory under `work`, and `extra` arguments.
    ///
    /// # Errors
    /// The box did not start, or did not say where its API is in 60 s.
    pub fn start(bin: &Path, helmsman: &Path, work: &Path, extra: &[OsString]) -> Result<Self, String> {
        let echo = helmsman.join("crates/helmsman-plc/programs/echo.st");
        Self::start_with(bin, Some(&echo), work, extra, |_| Ok(()))
    }

    /// [`Self::start`] with `program` (or none: a `--projects` box boots
    /// its store's head and takes no program on its command line), and
    /// `setup` run on the fresh state directory before the box starts (an
    /// account added with [`add_account`]).
    ///
    /// # Errors
    /// `setup` failed, or as [`Self::start`].
    pub fn start_with(
        bin: &Path,
        program: Option<&Path>,
        work: &Path,
        extra: &[OsString],
        setup: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<Self, String> {
        let state = fresh_dir(work, "state")?;
        setup(&state)?;
        let cwd = fresh_dir(work, "cwd")?;
        let mut child = Command::new(bin)
            .args(program)
            .args(["--opcua", "off", "--https", "off", "--api", "127.0.0.1:0"])
            .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
            .arg("--state-dir")
            .arg(&state)
            .args(extra)
            .current_dir(&cwd)
            .env_remove("STATE_DIRECTORY")
            .env("HELMSMAN_EXIT_WITH_STDIN", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("start {}: {e}", bin.display()))?;
        let stderr = child.stderr.take().ok_or("no stderr")?;
        let log = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = std::sync::mpsc::channel();
        let sink = Arc::clone(&log);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if let Some(rest) = line.strip_prefix("helmsman-plc: API on http://") {
                    let _ = tx.send(rest.split('/').next().unwrap_or_default().to_string());
                }
                sink.lock().unwrap_or_else(PoisonError::into_inner).push(line);
            }
        });
        let mut me = Self {
            child,
            url: Url::parse("http://127.0.0.1:1")?,
            token: String::new(),
            log,
        };
        let addr = rx
            .recv_timeout(Duration::from_secs(60))
            .map_err(|_| format!("the box never said where its API is:\n{}", me.log_tail()))?;
        me.url = Url::parse(&format!("http://{addr}"))?;
        let token_file = state.join("api-token");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(t) = std::fs::read_to_string(&token_file)
                && t.trim().len() == 64
            {
                me.token = t.trim().to_string();
                break;
            }
            if Instant::now() > deadline {
                return Err(format!("no API token in {}", token_file.display()));
            }
            thread::sleep(Duration::from_millis(50));
        }
        Ok(me)
    }

    /// The box's last stderr lines, for a failure's detail.
    #[must_use]
    pub fn log_tail(&self) -> String {
        let log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        let from = log.len().saturating_sub(15);
        redact(&log[from..].join("\n"))
    }
}

impl Drop for BoxProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `text` with every run of 32 or more hex digits (the box's API token,
/// a relay token's tail) replaced: a failure's detail lands in a report.
#[must_use]
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= 32 {
            out.push_str("[redacted]");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if c.is_ascii_hexdigit() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// `work/<name>-<n>`, made new and empty.
///
/// # Errors
/// It cannot be made.
pub fn fresh_dir(work: &Path, name: &str) -> Result<PathBuf, String> {
    for n in 0.. {
        let p = work.join(format!("{name}-{n}"));
        if !p.exists() {
            std::fs::create_dir_all(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            return Ok(p);
        }
    }
    unreachable!()
}

/// The box's `--help` text.
///
/// # Errors
/// It did not run.
pub fn help_text(bin: &Path) -> Result<String, String> {
    let out = Command::new(bin)
        .arg("--help")
        .output()
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(text)
}

/// `bin account add USER ROLE` on the state directory `state`, the
/// password given on stdin as the box reads it.
///
/// # Errors
/// It did not run, or refused.
pub fn add_account(bin: &Path, state: &Path, user: &str, role: &str, password: &str) -> Result<(), String> {
    use std::io::Write as _;
    let mut child = Command::new(bin)
        .args(["account", "add", user, role, "--state-dir"])
        .arg(state)
        .env_remove("STATE_DIRECTORY")
        .current_dir(state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("account add: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        writeln!(stdin, "{password}").map_err(|e| format!("account add: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("account add: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "account add {user} {role}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_never_reach_a_report() {
        let t = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let line = format!("API token (new): {t}; relay hmr1_{t}; port 8080, sha 4a6d37aa");
        let r = redact(&line);
        assert!(!r.contains(t), "{r}");
        assert_eq!(
            r,
            "API token (new): [redacted]; relay hmr1_[redacted]; port 8080, sha 4a6d37aa"
        );
    }
}
