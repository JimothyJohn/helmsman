//! `helmsman-compat`: one contract between two of the four repositories,
//! at the checkouts and programs given. `scripts/compat.sh` runs every
//! contract on every corner; this is one cell of it.
//!
//! ```text
//! helmsman-compat devices  --box BIN --helmsman DIR --devices DIR  [--work DIR] [--json FILE]
//! helmsman-compat relay    --box BIN --helmsman DIR --relay-bin BIN [--latest] [--work DIR] [--json FILE]
//! helmsman-compat live     --box BIN --helmsman DIR --url URL --token FILE [--work DIR] [--json FILE]
//! helmsman-compat hardware --helmsman DIR --hardware DIR [--json FILE]
//! helmsman-compat parity   --box BIN --helmsman DIR [--manifest FILE] [--relay-bin BIN | --url URL --token FILE] [--work DIR] [--json FILE]
//! ```
//!
//! Exit 0 when every check passed, 1 when one failed, 2 when the cell
//! could not run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use helmsman_compat::report::{self, Finding};
use helmsman_compat::{devices, hardware, parity, relay};

const USAGE: &str =
    "usage: helmsman-compat devices|relay|live|hardware|parity [options] (see the crate docs)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut opts: BTreeMap<String, String> = BTreeMap::new();
    let mut flags = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--latest" => flags.push(a.clone()),
            k if k.starts_with("--") => {
                let Some(v) = it.next() else {
                    eprintln!("{k} needs a value\n{USAGE}");
                    return ExitCode::from(2);
                };
                opts.insert(k.trim_start_matches("--").to_string(), v.clone());
            }
            other => {
                eprintln!("unexpected {other:?}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    match run(cmd, &opts, flags.iter().any(|f| f == "--latest")) {
        Ok(findings) => {
            let (text, ok) = report::render(&findings);
            print!("{text}");
            if let Some(p) = opts.get("json") {
                let j: Vec<_> = findings.iter().map(Finding::json).collect();
                if let Err(e) = std::fs::write(p, serde_json::to_string_pretty(&j).unwrap_or_default()) {
                    eprintln!("--json {p}: {e}");
                    return ExitCode::from(2);
                }
            }
            if ok { ExitCode::SUCCESS } else { ExitCode::from(1) }
        }
        Err(e) => {
            eprintln!("helmsman-compat {cmd}: could not run: {e}");
            ExitCode::from(2)
        }
    }
}

fn path(opts: &BTreeMap<String, String>, k: &str) -> Result<PathBuf, String> {
    // Absolute: the box and the relay run in working directories of their own.
    let p = opts.get(k).ok_or_else(|| format!("--{k} is required"))?;
    std::fs::canonicalize(p).map_err(|e| format!("--{k} {p}: {e}"))
}

fn run(cmd: &str, o: &BTreeMap<String, String>, latest: bool) -> Result<Vec<Finding>, String> {
    let work = || -> Result<PathBuf, String> {
        let w = o
            .get("work")
            .map_or_else(|| std::env::temp_dir().join("helmsman-compat"), PathBuf::from);
        let w = helmsman_compat::boxp::fresh_dir(&w, cmd)?;
        std::fs::canonicalize(&w).map_err(|e| format!("{}: {e}", w.display()))
    };
    match cmd {
        "devices" => devices::check(
            &path(o, "box")?,
            &path(o, "helmsman")?,
            &path(o, "devices")?,
            &work()?,
        ),
        "relay" => relay::check(
            &path(o, "box")?,
            &path(o, "helmsman")?,
            &path(o, "relay-bin")?,
            &work()?,
            latest,
        ),
        "live" => {
            let url = o.get("url").ok_or("--url is required")?;
            relay::live(
                &path(o, "box")?,
                &path(o, "helmsman")?,
                url,
                &path(o, "token")?,
                &work()?,
            )
        }
        "hardware" => hardware::check(&path(o, "helmsman")?, Path::new(&path(o, "hardware")?)),
        "parity" => {
            let helmsman = path(o, "helmsman")?;
            // The candidate's features, also when the box is live's: live
            // is held to what dev has (docs/PARITY.md).
            let manifest = o
                .get("manifest")
                .map_or_else(|| Ok(helmsman.join("docs/FEATURES.md")), |_| path(o, "manifest"))?;
            // The agent journeys' side: the stub behind a candidate
            // relay, the deployed relay, or none (they are then not run).
            let side = match (o.get("relay-bin"), o.get("url")) {
                (Some(_), Some(_)) => return Err("--relay-bin and --url are two sides; give one".into()),
                (Some(_), None) => parity::AgentSide::Stub {
                    relay_bin: path(o, "relay-bin")?,
                },
                (None, Some(url)) => parity::AgentSide::Live {
                    url: url.clone(),
                    token: path(o, "token")?,
                },
                (None, None) => parity::AgentSide::None,
            };
            let outcomes = parity::check_with(&path(o, "box")?, &manifest, &work()?, &[], &side)?;
            let (findings, pending) = parity::findings(&outcomes);
            if pending > 0 {
                eprintln!(
                    "helmsman-compat parity: {pending} journeys did not run (not written yet, or no relay for the agent ones; docs/PARITY.md)"
                );
            }
            Ok(findings)
        }
        _ => Err(USAGE.into()),
    }
}
