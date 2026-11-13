//! The Agent ladder (a ladder of instructions, nothing to do with ladder
//! logic): the page's instruction loop (`crate::instruct`, the
//! checks in `crate::verify`, the retries with what failed) run against a
//! real model through the relay's free chat, one rung at a time, from a
//! variable rename up to a new project. Each rung carries a hidden
//! requirement test the model never sees, so a green answer is also the
//! right change, not only one that passes its own scenarios.
//!
//! Ignored: it spends tokens and needs a relay. Run it against the
//! relay's demo route served locally (the same code, prompt and model as
//! the published playground; `agent-relay-local demo` in the relay repo,
//! `RELAY_DEMO_ORIGINS=http://localhost:8790`):
//!
//! ```text
//! HELMSMAN_LADDER_URL=http://127.0.0.1:8787 \
//!   cargo test -p helmsman-canvas agent_ladder -- --ignored --nocapture
//! ```
//!
//! `HELMSMAN_LADDER_ONLY=rename,period` picks rungs; `HELMSMAN_LADDER_RUNS=3`
//! repeats each (the model varies); `HELMSMAN_LADDER_ORIGIN` is the Origin
//! header the demo route expects (default `http://localhost:8790`). Every
//! request, answer and what failed is written under `target/agent-ladder/`, one
//! file per run, and the summary table goes to stdout and
//! `target/agent-ladder/report.md`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use helmsman_agent::http::{Request, Timeouts, Url, send};
use helmsman_iec::spec::{self, Settings};

use crate::claude::{Chat, Free, Incoming, NoStore, Phase};
use crate::instruct::{Env, Instruct, Kind, Step};
use crate::playground::{EXAMPLES, SIZES};
use crate::scope::Scope;

/// A project's files by name, as `crate::instruct::Env` holds them.
type Files = BTreeMap<String, String>;

/// One instruction of a rung, and how its result is judged.
struct Ask {
    instruction: &'static str,
    /// A requirement test the model never sees, run on the landed file.
    hidden: &'static str,
    /// Words the landed file must hold (case-insensitive).
    expect: &'static [&'static str],
    /// Words it must not.
    forbid: &'static [&'static str],
}

/// Where the instruction applies.
enum Where {
    File,
    Lines(u32, u32),
    New,
}

struct Rung {
    name: &'static str,
    /// `main.st` as the instruction finds it.
    base: &'static str,
    /// Requirement tests the project already holds.
    tests: &'static [(&'static str, &'static str)],
    scope: Where,
    asks: &'static [Ask],
}

fn example(name: &str) -> &'static str {
    EXAMPLES
        .iter()
        .find(|e| e.name == name)
        .map_or_else(|| panic!("no example {name}"), |e| e.src)
}

const BLINK_BROKEN: &str = "PROGRAM blink
VAR_OUTPUT
  lamp : BOOL;
END_VAR
(* The timer restarts itself every 500 ms; each time, the lamp flips. *)
pulse(IN := NOT pulse.Q, PT := T#500ms);
IF pulse.Q THEN
  lamp := NOT lamp;
END_IF;
END_PROGRAM
";

const LATCHES: &str = "(* The motor stays on after start is released. *)
PROGRAM test_latches
VAR_INPUT
  motor : BOOL;
END_VAR
VAR_OUTPUT
  start : BOOL;
  stop : BOOL;
  done : BOOL;
END_VAR
VAR
  n : INT;
  stays_on : ASSERT;
END_VAR
n := n + 1;
start := n < 3;
IF n > 6 THEN
  stays_on(COND := motor);
  done := TRUE;
END_IF;
END_PROGRAM
";

/// `out` has been both on and off within 2 s.
macro_rules! toggles {
    ($out:literal) => {
        concat!(
            "(* ",
            $out,
            " is on at some point and off at some point within 2 s. *)
PROGRAM test_hidden
VAR_INPUT
  ",
            $out,
            " : BOOL;
END_VAR
VAR_OUTPUT
  done : BOOL;
END_VAR
VAR
  t : TON;
  seen_on : BOOL;
  seen_off : BOOL;
  toggles : ASSERT;
END_VAR
IF ",
            $out,
            " THEN
  seen_on := TRUE;
ELSE
  seen_off := TRUE;
END_IF;
t(IN := TRUE, PT := T#2s);
IF t.Q THEN
  toggles(COND := seen_on AND seen_off);
  done := TRUE;
END_IF;
END_PROGRAM
"
        )
    };
}

const HOLDS_A_SECOND: &str = "(* The lamp holds for a full second between flips. *)
PROGRAM test_hidden
VAR_INPUT
  lamp : BOOL;
END_VAR
VAR_OUTPUT
  done : BOOL;
END_VAR
VAR
  n : INT;
  a : BOOL;
  b : BOOL;
  same_within_second : ASSERT;
  flips_after_second : ASSERT;
END_VAR
n := n + 1;
IF n = 10 THEN
  a := lamp;
END_IF;
IF n = 80 THEN
  b := lamp;
  same_within_second(COND := a = b);
END_IF;
IF n = 150 THEN
  flips_after_second(COND := lamp <> b);
  done := TRUE;
END_IF;
END_PROGRAM
";

const ESTOP: &str = "(* With estop held, start does not run the motor; without it, it does. *)
PROGRAM test_hidden
VAR_INPUT
  motor : BOOL;
END_VAR
VAR_OUTPUT
  start : BOOL;
  stop : BOOL;
  estop : BOOL;
  done : BOOL;
END_VAR
VAR
  n : INT;
  off_under_estop : ASSERT;
  runs_after : ASSERT;
END_VAR
n := n + 1;
estop := n <= 10;
start := TRUE;
IF n = 10 THEN
  off_under_estop(COND := NOT motor);
END_IF;
IF n = 20 THEN
  runs_after(COND := motor);
  done := TRUE;
END_IF;
END_PROGRAM
";

const ONE_OF: &str = "(* one is on for exactly one of a and b. *)
PROGRAM test_hidden
VAR_INPUT
  one : BOOL;
END_VAR
VAR_OUTPUT
  a : BOOL;
  b : BOOL;
  done : BOOL;
END_VAR
VAR
  n : INT;
  ff : ASSERT;
  tf : ASSERT;
  ft : ASSERT;
  tt : ASSERT;
END_VAR
n := n + 1;
a := (n > 10 AND n <= 20) OR n > 30;
b := n > 20;
IF n = 10 THEN
  ff(COND := NOT one);
END_IF;
IF n = 20 THEN
  tf(COND := one);
END_IF;
IF n = 30 THEN
  ft(COND := one);
END_IF;
IF n = 40 THEN
  tt(COND := NOT one);
  done := TRUE;
END_IF;
END_PROGRAM
";

const EITHER_ONE_OF: &str = "(* either is on for exactly one of a and b, off for both. *)
PROGRAM test_hidden
VAR_INPUT
  either : BOOL;
END_VAR
VAR_OUTPUT
  a : BOOL;
  b : BOOL;
  done : BOOL;
END_VAR
VAR
  n : INT;
  tf : ASSERT;
  tt : ASSERT;
END_VAR
n := n + 1;
a := n > 10;
b := n > 20;
IF n = 20 THEN
  tf(COND := either);
END_IF;
IF n = 30 THEN
  tt(COND := NOT either);
  done := TRUE;
END_IF;
END_PROGRAM
";

const FIVE: &str = "(* done comes on at the fifth pulse, not before. *)
PROGRAM test_hidden
VAR_INPUT
  done : BOOL;
  count : INT;
END_VAR
VAR_OUTPUT
  pulse : BOOL;
  reset : BOOL;
  test_done : BOOL;
END_VAR
VAR
  n : INT;
  not_yet : ASSERT;
  at_five : ASSERT;
END_VAR
n := n + 1;
pulse := (n MOD 4) = 0;
IF n = 18 THEN
  not_yet(COND := NOT done);
END_IF;
IF n = 22 THEN
  at_five(COND := done);
  test_done := TRUE;
END_IF;
END_PROGRAM
";

const DELAY: &str = "(* The lamp waits 2 s after the switch and drops at once with it. *)
PROGRAM test_hidden
VAR_INPUT
  lamp : BOOL;
END_VAR
VAR_OUTPUT
  switch : BOOL;
  done : BOOL;
END_VAR
VAR
  n : INT;
  off_early : ASSERT;
  on_late : ASSERT;
  off_at_once : ASSERT;
END_VAR
n := n + 1;
switch := n <= 230;
IF n = 100 THEN
  off_early(COND := NOT lamp);
END_IF;
IF n = 230 THEN
  on_late(COND := lamp);
END_IF;
IF n = 240 THEN
  off_at_once(COND := NOT lamp);
  done := TRUE;
END_IF;
END_PROGRAM
";

const HORN: &str = "(* The horn sounds for one second when the motor starts, then stops. *)
PROGRAM test_hidden
VAR_INPUT
  motor : BOOL;
  horn : BOOL;
END_VAR
VAR_OUTPUT
  start : BOOL;
  stop : BOOL;
  done : BOOL;
END_VAR
VAR
  n : INT;
  horn_sounds : ASSERT;
  motor_runs : ASSERT;
  horn_stops : ASSERT;
END_VAR
n := n + 1;
start := n <= 3;
IF n = 50 THEN
  horn_sounds(COND := horn);
  motor_runs(COND := motor);
END_IF;
IF n = 150 THEN
  horn_stops(COND := NOT horn AND motor);
  done := TRUE;
END_IF;
END_PROGRAM
";

const MOMENTARY: &str = "(* The motor runs only while start is held. *)
PROGRAM test_hidden
VAR_INPUT
  motor : BOOL;
END_VAR
VAR_OUTPUT
  start : BOOL;
  stop : BOOL;
  done : BOOL;
END_VAR
VAR
  n : INT;
  runs_held : ASSERT;
  stops_released : ASSERT;
END_VAR
n := n + 1;
start := n <= 10;
IF n = 10 THEN
  runs_held(COND := motor);
END_IF;
IF n = 20 THEN
  stops_released(COND := NOT motor);
  done := TRUE;
END_IF;
END_PROGRAM
";

const TRAFFIC: &str = "(* Red 5 s, green 5 s, yellow 2 s, one lamp at a time, repeating. *)
PROGRAM test_hidden
VAR_INPUT
  red : BOOL;
  yellow : BOOL;
  green : BOOL;
END_VAR
VAR_OUTPUT
  done : BOOL;
END_VAR
VAR
  n : INT;
  red_first : ASSERT;
  then_green : ASSERT;
  then_yellow : ASSERT;
  red_again : ASSERT;
END_VAR
n := n + 1;
IF n = 200 THEN
  red_first(COND := red AND NOT yellow AND NOT green);
END_IF;
IF n = 700 THEN
  then_green(COND := green AND NOT red AND NOT yellow);
END_IF;
IF n = 1100 THEN
  then_yellow(COND := yellow AND NOT red AND NOT green);
END_IF;
IF n = 1300 THEN
  red_again(COND := red AND NOT yellow AND NOT green);
  done := TRUE;
END_IF;
END_PROGRAM
";

fn rungs() -> Vec<Rung> {
    vec![
        Rung {
            name: "rename",
            base: example("Blink"),
            tests: &[],
            scope: Where::File,
            asks: &[Ask {
                instruction: "Rename the output lamp to pilot.",
                hidden: toggles!("pilot"),
                expect: &["pilot"],
                forbid: &["lamp"],
            }],
        },
        Rung {
            name: "period",
            base: example("Blink"),
            tests: &[],
            scope: Where::File,
            asks: &[Ask {
                instruction: "Make the lamp flip every second instead of every 500 ms.",
                hidden: HOLDS_A_SECOND,
                expect: &[],
                forbid: &[],
            }],
        },
        Rung {
            name: "estop",
            base: example("Start/stop latch"),
            tests: &[],
            scope: Where::File,
            asks: &[Ask {
                instruction: "Add an input estop. While estop is on the motor is off and start does nothing.",
                hidden: ESTOP,
                expect: &["estop"],
                forbid: &[],
            }],
        },
        Rung {
            name: "xor",
            base: example("Logic: AND, OR, NOT"),
            tests: &[],
            scope: Where::File,
            asks: &[Ask {
                instruction: "Add an output named one that is on when exactly one of a and b is on.",
                hidden: ONE_OF,
                expect: &["one"],
                forbid: &[],
            }],
        },
        Rung {
            name: "count5",
            base: example("Counter"),
            tests: &[],
            scope: Where::File,
            asks: &[Ask {
                instruction: "Count to 5 instead of 10.",
                hidden: FIVE,
                expect: &[],
                forbid: &[],
            }],
        },
        Rung {
            name: "delay",
            base: example("Read and write"),
            tests: &[],
            scope: Where::File,
            asks: &[Ask {
                instruction: "Turn the lamp on 2 s after the switch goes on, and off as soon as the switch goes off.",
                hidden: DELAY,
                expect: &[],
                forbid: &[],
            }],
        },
        Rung {
            name: "horn",
            base: example("Start/stop latch"),
            tests: &[],
            scope: Where::File,
            asks: &[Ask {
                instruction: "Add an output horn that sounds for 1 s each time the motor starts.",
                hidden: HORN,
                expect: &["horn"],
                forbid: &[],
            }],
        },
        Rung {
            name: "fb",
            base: example("Blink"),
            tests: &[],
            scope: Where::File,
            asks: &[Ask {
                instruction: "Move the blinking into a FUNCTION_BLOCK named FLASHER with an input PERIOD : TIME and an output Q : BOOL, and drive the lamp from one instance of it.",
                hidden: toggles!("lamp"),
                expect: &["function_block flasher"],
                forbid: &[],
            }],
        },
        Rung {
            name: "lines",
            base: example("Logic: AND, OR, NOT"),
            tests: &[],
            scope: Where::Lines(12, 12),
            asks: &[Ask {
                instruction: "Make either mean exactly one of them: off when both are on.",
                hidden: EITHER_ONE_OF,
                expect: &["both := a and b;", "neither := not (a or b);"],
                forbid: &[],
            }],
        },
        Rung {
            name: "fix",
            base: BLINK_BROKEN,
            tests: &[],
            scope: Where::File,
            asks: &[Ask {
                instruction: "Fix the compile error.",
                hidden: toggles!("lamp"),
                expect: &[": ton;"],
                forbid: &[],
            }],
        },
        Rung {
            name: "replace-test",
            base: example("Start/stop latch"),
            tests: &[("test_latches.st", LATCHES)],
            scope: Where::File,
            asks: &[Ask {
                instruction: "Run the motor only while start is held; it must not latch.",
                hidden: MOMENTARY,
                expect: &[],
                forbid: &[],
            }],
        },
        Rung {
            name: "new",
            base: example("Blink"),
            tests: &[],
            scope: Where::New,
            asks: &[Ask {
                instruction: "A traffic light: outputs red, yellow and green. From power-up: red for 5 s, then green for 5 s, then yellow for 2 s, and round again.",
                hidden: TRAFFIC,
                expect: &["red", "yellow", "green"],
                forbid: &[],
            }],
        },
        Rung {
            name: "chain",
            base: example("Blink"),
            tests: &[],
            scope: Where::File,
            asks: &[
                Ask {
                    instruction: "Rename the output lamp to pilot.",
                    hidden: toggles!("pilot"),
                    expect: &["pilot"],
                    forbid: &["lamp"],
                },
                Ask {
                    instruction: "Make it flip every second.",
                    hidden: "(* The pilot holds for a full second between flips. *)
PROGRAM test_hidden
VAR_INPUT
  pilot : BOOL;
END_VAR
VAR_OUTPUT
  done : BOOL;
END_VAR
VAR
  n : INT;
  a : BOOL;
  b : BOOL;
  same_within_second : ASSERT;
  flips_after_second : ASSERT;
END_VAR
n := n + 1;
IF n = 10 THEN
  a := pilot;
END_IF;
IF n = 80 THEN
  b := pilot;
  same_within_second(COND := a = b);
END_IF;
IF n = 150 THEN
  flips_after_second(COND := pilot <> b);
  done := TRUE;
END_IF;
END_PROGRAM
",
                    expect: &["pilot"],
                    forbid: &["lamp"],
                },
            ],
        },
    ]
}

/// What the harness needs from the environment.
struct Setup {
    url: String,
    origin: String,
    only: Vec<String>,
    runs: u32,
    out: PathBuf,
}

impl Setup {
    fn from_env() -> Self {
        let url = std::env::var("HELMSMAN_LADDER_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .expect("HELMSMAN_LADDER_URL: the free chat's address (a local `agent-relay-local demo` is http://127.0.0.1:8787)");
        let out = std::env::var("HELMSMAN_LADDER_OUT").map_or_else(
            |_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/agent-ladder"),
            PathBuf::from,
        );
        Self {
            url: url.trim().trim_end_matches('/').to_string(),
            origin: std::env::var("HELMSMAN_LADDER_ORIGIN")
                .unwrap_or_else(|_| "http://localhost:8790".into()),
            only: std::env::var("HELMSMAN_LADDER_ONLY")
                .map(|s| {
                    s.split(',')
                        .map(|p| p.trim().to_string())
                        .filter(|p| !p.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            runs: std::env::var("HELMSMAN_LADDER_RUNS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1),
            out,
        }
    }
}

/// One request to the model: the body as the page builds it, the answer
/// as the page assembles it.
fn turn(
    setup: &Setup,
    chat: &mut Chat,
    message: &str,
    context: &[crate::claude::Exchange],
) -> Result<(String, f64), String> {
    let (seq, req) = chat.send(message, context)?;
    let url = Url::parse(&req.url).map_err(|e| format!("{}: {e}", req.url))?;
    let path = if url.prefix.is_empty() {
        "/".to_string()
    } else {
        url.prefix.clone()
    };
    let headers = [("origin", setup.origin.as_str()), ("accept", "text/event-stream")];
    let http = Request {
        method: "POST",
        path: &path,
        headers: &headers,
        json: Some(req.body.as_bytes()),
        raw: None,
    };
    let timeouts = Timeouts {
        connect: Duration::from_secs(10),
        idle: Duration::from_secs(300),
        total: Duration::from_secs(900),
    };
    let mut cost = 0.0;
    match send(&url, &http, timeouts) {
        Ok(resp) => {
            let status = resp.status;
            let body = resp
                .read_body(16 << 20)
                .map_err(|e| format!("reading the answer: {e:?}"))?;
            let text = String::from_utf8_lossy(&body).into_owned();
            cost = cost_of(&text);
            if status == 200 {
                chat.receive(Incoming::Chunk { seq, text });
                chat.receive(Incoming::End { seq });
            } else {
                chat.receive(Incoming::Status {
                    seq,
                    status,
                    body: text,
                });
            }
        }
        Err(e) => chat.receive(Incoming::Unreachable {
            seq,
            why: format!("{e:?}"),
        }),
    }
    let answer = chat.answer.as_ref().ok_or("no answer")?;
    match &answer.phase {
        Phase::Done => Ok((answer.text.clone(), cost)),
        Phase::Failed(why) => Err(format!("{why} (after {} chars)", answer.text.len())),
        Phase::Waiting | Phase::Streaming => Err("the answer never ended".into()),
    }
}

/// What the upstream says the turn cost, in dollars (`OpenRouter` puts
/// `usage.cost` on `message_delta`); 0 when it does not say.
fn cost_of(sse: &str) -> f64 {
    sse.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|d| serde_json::from_str::<serde_json::Value>(d).ok())
        .filter_map(|v| v["usage"]["cost"].as_f64())
        .sum()
}

/// How one instruction came out.
struct Outcome {
    rung: String,
    step: usize,
    run: u32,
    kind: String,
    /// Model calls made.
    calls: u32,
    /// The hidden test on the landed file: `passed`, or why not; `-` when
    /// nothing landed.
    hidden: String,
    /// Words expected or forbidden that were wrong.
    words: Vec<String>,
    /// The last attempt's problems (what the page told the model).
    problems: Vec<String>,
    secs: f64,
    /// Dollars, every call together.
    cost: f64,
}

impl Outcome {
    fn ok(&self) -> bool {
        self.kind == "applied" && self.hidden == "passed" && self.words.is_empty()
    }
}

/// `text` with every `(* ... *)` taken out.
fn without_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(k) = rest.find("(*") {
        out.push_str(&rest[..k]);
        match rest[k + 2..].find("*)") {
            Some(e) => rest = &rest[k + 2 + e + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn diagnostics(src: &str) -> Vec<String> {
    match helmsman_iec::compile(src, SIZES) {
        Ok(_) => Vec::new(),
        Err(d) => d.iter().map(ToString::to_string).collect(),
    }
}

fn hidden_verdict(files: &Files, hidden: &str) -> String {
    let mut all: Vec<(&str, &str)> = files
        .iter()
        .filter(|(n, _)| helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St))
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    all.push(("test_hidden.st", hidden));
    let plan = spec::plan(all);
    spec::run_all(&plan, Settings::new(SIZES))
        .into_iter()
        .find(|v| v.file == "test_hidden.st")
        .map_or_else(
            || "the hidden test did not run".to_string(),
            |v| if v.passed() { "passed".into() } else { v.why() },
        )
}

#[allow(clippy::too_many_lines)]
fn run_rung(setup: &Setup, rung: &Rung, run: u32, log: &mut String) -> Vec<Outcome> {
    let mut files = Files::default();
    if !matches!(rung.scope, Where::New) {
        files.insert("main.st".into(), rung.base.to_string());
    }
    for (n, t) in rung.tests {
        files.insert((*n).to_string(), (*t).to_string());
    }
    let scope = match rung.scope {
        Where::File => Scope::File("main.st".into()),
        Where::Lines(from, to) => Scope::Lines {
            file: "main.st".into(),
            from,
            to,
        },
        Where::New => Scope::NewProject,
    };
    let mut chat = Chat::new(Box::new(NoStore));
    chat.receive(Incoming::Proxy(false));
    chat.set_free_base(&setup.url);
    chat.receive(Incoming::Free(Free::Open));
    let mut earlier: Vec<String> = Vec::new();
    let mut outcomes = Vec::new();
    let _ = writeln!(log, "# {} (run {run})\n", rung.name);
    for (k, ask) in rung.asks.iter().enumerate() {
        let started = Instant::now();
        let source = files
            .get("main.st")
            .cloned()
            .unwrap_or_else(|| rung.base.to_string());
        let diags = diagnostics(&source);
        let env = Env {
            project: "ladder",
            scope: scope.clone(),
            files: &files,
            source: &source,
            diagnostics: &diags,
            devices: "",
            settings: Settings::new(SIZES),
        };
        let mut ins = Instruct {
            draft: ask.instruction.to_string(),
            ..Instruct::default()
        };
        let _ = writeln!(log, "## step {}: {}\n", k + 1, ask.instruction);
        let mut calls = 0;
        let mut cost = 0.0;
        let mut applied = None;
        let mut next = match ins.start(&env, &earlier) {
            Ok(a) => Some(a),
            Err(why) => {
                ins.failed(&why);
                None
            }
        };
        while let Some(a) = next.take() {
            calls += 1;
            let _ = writeln!(
                log,
                "### request {calls}{}\n\n```text\n{}\n```\n",
                if a.retry { " (retry)" } else { "" },
                a.message
            );
            match turn(setup, &mut chat, &a.message, &a.context) {
                Ok((answer, c)) => {
                    cost += c;
                    let _ = writeln!(log, "### answer {calls}\n\n{answer}\n");
                    match ins.answered(&answer, &env) {
                        Step::Ask(again) => {
                            if let Some(r) = &ins.run
                                && let Some(att) = r.attempts.last()
                            {
                                let _ = writeln!(log, "### failed {calls}\n");
                                for p in &att.problems {
                                    let _ = writeln!(log, "- {p}");
                                }
                                let _ = writeln!(log);
                            }
                            next = Some(again);
                        }
                        Step::Apply(ap) => {
                            let _ = writeln!(log, "### applied\n\n{}\n", ap.status);
                            applied = Some(ap);
                        }
                        Step::Done => {}
                    }
                }
                Err(why) => {
                    let _ = writeln!(log, "### transport failed {calls}\n\n{why}\n");
                    ins.failed(&why);
                }
            }
        }
        let mut hidden = "-".to_string();
        let mut words = Vec::new();
        if let Some(ap) = applied.take() {
            files.insert(ap.file.clone(), ap.text.clone());
            for (n, t) in &ap.tests {
                files.insert(n.clone(), t.clone());
            }
            ins.applied(&ap, None);
            // Comments do not count: a rename that leaves one is sloppy,
            // not wrong.
            let lower = without_comments(&ap.text).to_ascii_lowercase();
            for w in ask.expect {
                if !lower.contains(w) {
                    words.push(format!("missing {w:?}"));
                }
            }
            for w in ask.forbid {
                if lower.contains(w) {
                    words.push(format!("still has {w:?}"));
                }
            }
            hidden = hidden_verdict(&files, ask.hidden);
            let _ = writeln!(
                log,
                "### landed main.st\n\n```st\n{}```\n\nhidden test: {hidden}\n",
                files.get("main.st").map_or("", String::as_str)
            );
            if !words.is_empty() {
                let _ = writeln!(log, "words: {}\n", words.join(", "));
            }
        }
        let outcome = ins.outcome.clone();
        let kind = outcome.as_ref().map_or("none", |o| match o.kind {
            Kind::Applied => "applied",
            Kind::Refused => "refused",
            Kind::Said => "said",
            Kind::Failed => "failed",
            Kind::Undone => "undone",
        });
        let mut problems: Vec<String> = outcome.as_ref().map(|o| o.problems.clone()).unwrap_or_default();
        if let Some(o) = &outcome
            && problems.is_empty()
            && o.kind != Kind::Applied
        {
            problems.push(o.status.clone());
        }
        if kind != "applied" {
            let _ = writeln!(log, "### outcome: {kind}\n");
            for p in &problems {
                let _ = writeln!(log, "- {p}");
            }
            let _ = writeln!(log);
        }
        earlier.push(ask.instruction.to_string());
        outcomes.push(Outcome {
            rung: rung.name.to_string(),
            step: k + 1,
            run,
            kind: kind.to_string(),
            calls,
            hidden,
            words,
            problems,
            secs: started.elapsed().as_secs_f64(),
            cost,
        });
        if kind != "applied" {
            // The next step of a chain needs this one landed.
            break;
        }
    }
    outcomes
}

#[test]
#[ignore = "spends tokens against a relay: HELMSMAN_LADDER_URL=... cargo test -p helmsman-canvas agent_ladder -- --ignored --nocapture"]
fn agent_ladder() {
    let setup = Setup::from_env();
    std::fs::create_dir_all(&setup.out).expect("target/agent-ladder");
    let mut all = Vec::new();
    for rung in rungs() {
        if !setup.only.is_empty() && !setup.only.iter().any(|o| o == rung.name) {
            continue;
        }
        for run in 1..=setup.runs {
            let mut log = String::new();
            let outcomes = run_rung(&setup, &rung, run, &mut log);
            let path = setup.out.join(format!("{}-{run}.md", rung.name));
            std::fs::write(&path, log).expect("write the run's log");
            for o in &outcomes {
                println!(
                    "{:<13} step {} run {}: {:<8} calls {} hidden {} {} {:.0}s",
                    o.rung,
                    o.step,
                    o.run,
                    o.kind,
                    o.calls,
                    o.hidden,
                    if o.words.is_empty() {
                        String::new()
                    } else {
                        format!("words: {}", o.words.join(", "))
                    },
                    o.secs
                );
                for p in &o.problems {
                    println!("    - {p}");
                }
            }
            all.extend(outcomes);
        }
    }
    let mut report = String::from(
        "| rung | step | run | outcome | calls | hidden test | words | s | $ |\n|---|---|---|---|---|---|---|---|---|\n",
    );
    for o in &all {
        let _ = writeln!(
            report,
            "| {} | {} | {} | {} | {} | {} | {} | {:.0} | {:.4} |",
            o.rung,
            o.step,
            o.run,
            if o.ok() { "ok" } else { &o.kind },
            o.calls,
            o.hidden,
            o.words.join(", "),
            o.secs,
            o.cost
        );
    }
    let passed = all.iter().filter(|o| o.ok()).count();
    let spent: f64 = all.iter().map(|o| o.cost).sum();
    let _ = writeln!(report, "\n{passed} of {} passed, ${spent:.4} spent.", all.len());
    std::fs::write(setup.out.join("report.md"), &report).expect("write the report");
    println!("\n{report}");
    assert_eq!(passed, all.len(), "see {}", setup.out.display());
}

/// A right answer to each rung's last step, by hand: what the hidden test
/// must accept.
fn reference(rung: &str) -> &'static str {
    match rung {
        "rename" | "chain" => {
            "PROGRAM blink
VAR_OUTPUT
  pilot : BOOL;
END_VAR
VAR
  pulse : TON;
END_VAR
pulse(IN := NOT pulse.Q, PT := T#1s);
IF pulse.Q THEN
  pilot := NOT pilot;
END_IF;
END_PROGRAM
"
        }
        "period" => {
            "PROGRAM blink
VAR_OUTPUT
  lamp : BOOL;
END_VAR
VAR
  pulse : TON;
END_VAR
pulse(IN := NOT pulse.Q, PT := T#1s);
IF pulse.Q THEN
  lamp := NOT lamp;
END_IF;
END_PROGRAM
"
        }
        "estop" => {
            "PROGRAM latch
VAR_INPUT
  start : BOOL;
  stop : BOOL;
  estop : BOOL;
END_VAR
VAR_OUTPUT
  motor : BOOL;
END_VAR
motor := (start OR motor) AND NOT stop AND NOT estop;
END_PROGRAM
"
        }
        "xor" => {
            "PROGRAM logic
VAR_INPUT
  a : BOOL;
  b : BOOL;
END_VAR
VAR_OUTPUT
  both : BOOL;
  either : BOOL;
  neither : BOOL;
  one : BOOL;
END_VAR
both := a AND b;
either := a OR b;
neither := NOT (a OR b);
one := a XOR b;
END_PROGRAM
"
        }
        "count5" => {
            "PROGRAM counter
VAR_INPUT
  pulse : BOOL;
  reset : BOOL;
END_VAR
VAR_OUTPUT
  count : INT;
  done : BOOL;
END_VAR
VAR
  c : CTU;
END_VAR
c(CU := pulse, R := reset, PV := 5);
count := c.CV;
done := c.Q;
END_PROGRAM
"
        }
        "delay" => {
            "PROGRAM read_write
VAR_INPUT
  switch : BOOL;
END_VAR
VAR_OUTPUT
  lamp : BOOL;
END_VAR
VAR
  wait : TON;
END_VAR
wait(IN := switch, PT := T#2s);
lamp := wait.Q;
END_PROGRAM
"
        }
        "horn" => {
            "PROGRAM latch
VAR_INPUT
  start : BOOL;
  stop : BOOL;
END_VAR
VAR_OUTPUT
  motor : BOOL;
  horn : BOOL;
END_VAR
VAR
  blast : TP;
END_VAR
motor := (start OR motor) AND NOT stop;
blast(IN := motor, PT := T#1s);
horn := blast.Q;
END_PROGRAM
"
        }
        "fb" => {
            "FUNCTION_BLOCK FLASHER
VAR_INPUT
  PERIOD : TIME;
END_VAR
VAR_OUTPUT
  Q : BOOL;
END_VAR
VAR
  pulse : TON;
END_VAR
pulse(IN := NOT pulse.Q, PT := PERIOD);
IF pulse.Q THEN
  Q := NOT Q;
END_IF;
END_FUNCTION_BLOCK

PROGRAM blink
VAR_OUTPUT
  lamp : BOOL;
END_VAR
VAR
  f : FLASHER;
END_VAR
f(PERIOD := T#500ms);
lamp := f.Q;
END_PROGRAM
"
        }
        "lines" => {
            "PROGRAM logic
VAR_INPUT
  a : BOOL;
  b : BOOL;
END_VAR
VAR_OUTPUT
  both : BOOL;
  either : BOOL;
  neither : BOOL;
END_VAR
both := a AND b;
either := a XOR b;
neither := NOT (a OR b);
END_PROGRAM
"
        }
        "fix" => example("Blink"),
        "replace-test" => {
            "PROGRAM latch
VAR_INPUT
  start : BOOL;
  stop : BOOL;
END_VAR
VAR_OUTPUT
  motor : BOOL;
END_VAR
motor := start AND NOT stop;
END_PROGRAM
"
        }
        "new" => {
            "PROGRAM traffic
VAR_OUTPUT
  red : BOOL;
  yellow : BOOL;
  green : BOOL;
END_VAR
VAR
  cycle : TON;
END_VAR
cycle(IN := NOT cycle.Q, PT := T#12s);
red := cycle.ET < T#5s;
green := cycle.ET >= T#5s AND cycle.ET < T#10s;
yellow := cycle.ET >= T#10s;
END_PROGRAM
"
        }
        other => panic!("no reference for {other}"),
    }
}

#[test]
fn every_hidden_test_is_red_on_the_base_and_green_on_a_reference() {
    for rung in rungs() {
        let ask = rung.asks.last().unwrap();
        let mut base = Files::default();
        if !matches!(rung.scope, Where::New) {
            base.insert("main.st".into(), rung.base.to_string());
        }
        // The FUNCTION_BLOCK rung keeps what the file does (its words are
        // the check); every other rung's hidden test must catch the file
        // as it was.
        if rung.name != "fb" {
            let on_base = hidden_verdict(&base, ask.hidden);
            assert_ne!(
                on_base, "passed",
                "{}: the hidden test passes before the change",
                rung.name
            );
        }
        let mut done = Files::default();
        done.insert("main.st".into(), reference(rung.name).to_string());
        let on_reference = hidden_verdict(&done, ask.hidden);
        assert_eq!(on_reference, "passed", "{}: {on_reference}", rung.name);
        let lower = reference(rung.name).to_ascii_lowercase();
        for w in ask.expect {
            assert!(lower.contains(w), "{}: the reference lacks {w:?}", rung.name);
        }
        for w in ask.forbid {
            assert!(!lower.contains(w), "{}: the reference has {w:?}", rung.name);
        }
    }
}
