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
        let state = fresh_dir(work, "state")?;
        let cwd = fresh_dir(work, "cwd")?;
        let echo = helmsman.join("crates/helmsman-plc/programs/echo.st");
        let mut child = Command::new(bin)
            .arg(&echo)
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
        log[from..].join("\n")
    }
}

impl Drop for BoxProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
