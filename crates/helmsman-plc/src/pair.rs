//! `helmsman-plc pair`: trade a one-time pairing code for this box's relay
//! token (docs/BOX.md "Pairing by code"; the relay's side is its protocol
//! "Pairing by code"). Run by `helmsman-firstboot` on a flashed box, as
//! root, before the PLC service starts.
//!
//! The code is read from a file, never an argument (a command line is
//! visible to every process on the box), and the token is written to a file
//! with mode 0600, never printed. Neither is ever in an error message or
//! on stderr.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use helmsman_agent::http::Timeouts;
use helmsman_agent::relay::{self, Error};

const USE: &str =
    "usage: helmsman-plc pair --relay URL --box-id ID --code-file FILE --out FILE [--wait SECS]";

/// Exit status for a relay that refused the code outright (wrong, used,
/// expired, for another id): trying the same code again cannot work, and
/// trying it more costs the box's tries. `helmsman-firstboot` drops the
/// code file on it.
pub const REFUSED: u8 = 3;

/// How the command ended, when it did not pair.
#[derive(Debug, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
    /// The exit status: [`REFUSED`] for a refusal, 1 for anything else.
    pub code: u8,
}

fn fail(message: impl Into<String>) -> Failure {
    Failure {
        message: message.into(),
        code: 1,
    }
}

struct Args {
    relay: String,
    box_id: String,
    code_file: PathBuf,
    out: PathBuf,
    wait: Duration,
}

fn parse(args: &[String]) -> Result<Args, Failure> {
    let (mut relay, mut box_id, mut code_file, mut out) = (None, None, None, None);
    let mut wait = Duration::from_secs(60);
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = args.get(i + 1).ok_or_else(|| fail(USE))?;
        let slot = match flag {
            "--relay" => &mut relay,
            "--box-id" => &mut box_id,
            "--code-file" => &mut code_file,
            "--out" => &mut out,
            "--wait" => {
                wait = match value.parse::<u64>() {
                    Ok(s @ 0..=600) => Duration::from_secs(s),
                    _ => return Err(fail(format!("--wait: expected 0..=600 seconds\n{USE}"))),
                };
                i += 2;
                continue;
            }
            other => return Err(fail(format!("unknown argument {other}\n{USE}"))),
        };
        if slot.replace(value.clone()).is_some() {
            return Err(fail(format!("{flag} given twice\n{USE}")));
        }
        i += 2;
    }
    match (relay, box_id, code_file, out) {
        (Some(relay), Some(box_id), Some(code_file), Some(out)) => Ok(Args {
            relay,
            box_id,
            code_file: code_file.into(),
            out: out.into(),
            wait,
        }),
        _ => Err(fail(USE)),
    }
}

/// The first line of the code file, as the relay will read it.
fn read_code(path: &Path) -> Result<String, Failure> {
    let text = fs::read_to_string(path)
        .map_err(|e| fail(format!("--code-file {}: {}", path.display(), e.kind())))?;
    let line = text.lines().next().unwrap_or("").trim();
    if !relay::pair_code_ok(line) {
        // Never echo what was in the file: it may be a typo of the code.
        return Err(fail(format!(
            "--code-file {}: not a pairing code (10 letters and digits, like ABCDE-FGHJK)",
            path.display()
        )));
    }
    Ok(line.to_string())
}

/// Writes `token` to `out` readable by its owner only, whole or not at
/// all: a temp file beside it, then a rename.
fn write_token(out: &Path, token: &str) -> Result<(), Failure> {
    let tmp = out.with_extension("pairing-tmp");
    let _ = fs::remove_file(&tmp);
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let result = opts
        .open(&tmp)
        .and_then(|mut f| {
            f.write_all(token.as_bytes())?;
            f.write_all(b"\n")?;
            f.sync_all()
        })
        .and_then(|()| fs::rename(&tmp, out));
    result.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        fail(format!("--out {}: {}", out.display(), e.kind()))
    })
}

/// Whether trying again could work: nothing was sent (the relay was not
/// reachable), or the relay said slow down. Anything else may have spent
/// the code.
fn retryable(e: &Error) -> bool {
    matches!(e, Error::Unreachable(_) | Error::Status { status: 429, .. })
}

fn describe(e: &Error) -> Failure {
    let refused = matches!(e, Error::Status { status: 403, .. });
    let message = match e {
        Error::Status { status: 403, .. } => {
            "the relay refused the code: it is wrong, used, expired or for another box id. \
             Ask the operator for a new one"
                .to_string()
        }
        Error::Status { status: 404, .. } => {
            "this relay has no pairing (is it the right relay URL?)".to_string()
        }
        Error::Status { status: 429, .. } => {
            "the relay says too many tries from here; wait a minute".to_string()
        }
        Error::Timeout | Error::Malformed(_) | Error::CutOff => {
            format!("{e}; the code may be spent, ask the operator for a new one")
        }
        other => other.to_string(),
    };
    Failure {
        message,
        code: if refused { REFUSED } else { 1 },
    }
}

/// `helmsman-plc pair ...`: the one line to print on success.
///
/// # Errors
/// A [`Failure`] with what to print and the exit status.
pub fn run(args: &[String]) -> Result<String, Failure> {
    let a = parse(args)?;
    let url = relay::Url::parse(&a.relay).map_err(|e| fail(format!("--relay: {e}")))?;
    if !relay::box_id_ok(&a.box_id) {
        return Err(fail("--box-id: 1 to 64 of a-z 0-9 -, not starting with -"));
    }
    let code = read_code(&a.code_file)?;
    let timeouts = Timeouts {
        connect: Duration::from_secs(10),
        idle: Duration::from_secs(20),
        total: Duration::from_secs(30),
    };
    let until = Instant::now() + a.wait;
    let mut backoff = Duration::from_secs(1);
    let token = loop {
        match relay::pair(&url, &code, &a.box_id, timeouts) {
            Ok(t) => break t,
            Err(e) if retryable(&e) && Instant::now() + backoff < until => {
                eprintln!("helmsman-plc pair: {e}; trying again in {}s", backoff.as_secs());
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(16));
            }
            Err(e) => return Err(describe(&e)),
        }
    };
    write_token(&a.out, &token)?;
    Ok(format!(
        "helmsman-plc pair: paired as {}; token written to {}\n",
        a.box_id,
        a.out.display()
    ))
}
