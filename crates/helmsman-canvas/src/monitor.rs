//! The Monitor (docs/UI.md): phone to wall TV, watch only. Status tiles
//! per program (running or faulted; the scan period and counts only for
//! an engineer who turned "Scan detail" on, kept per browser in
//! [`Chrome::scan`]: card `monitor-scan-time`), then a tile per top-level
//! instance with its outputs (a BOOL a green or red lamp, a number its
//! value) and one for the program's own outputs; a tap goes one level
//! down into that instance, Back and the breadcrumbs go up, and "Open
//! diagram" hands that scope to the Editor.
//!
//! A box's status tiles also carry its scan load (card `scan-stats`): for
//! an engineer with "Scan detail" on, how long the scan takes, how late it
//! starts and the overruns, a tooltip on each; for everyone, an overrun in
//! the last [`OVERRUN_RECENT`] seconds in the attention colour. The
//! simulation runs in the page on no real clock, so it shows none.
//!
//! Where the values come from is a [`Feed`]: the box (`BoxState`, its
//! values stream and every module's traces on one stream) or the
//! simulation in the page (`PlaygroundView`). The tiles do not care which.
//!
//! **Stale data never shows as a colour** (the andon rule): a program
//! whose scans stopped coming for [`STALE_AFTER`] seconds, or any program
//! while the box cannot be reached, is drawn grey with "stale" and the
//! age; no lamp is lit.
//!
//! Nothing here writes: a [`Feed`] is read through `&self`, and the only
//! things a tap changes are which level is shown and the Editor's scope.

use std::collections::HashMap;

use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use helmsman_diagram::Scan;
use helmsman_iec::Value;
use helmsman_iec::ast::{self, PouKind, Section, TypeRef};

use crate::editor::Chrome;
use crate::glyphs;
use crate::hotkeys::Action;
use crate::playground_view::PlaygroundView;
use crate::remote::{BoxState, Link, ScanTiming};
use crate::st_view::TAP;
use crate::st_view::brand::{WHITE, YELLOW};
use crate::theme::theme;
use crate::tips::Tip;

/// Seconds without a new scan after which a program's tiles go grey.
pub const STALE_AFTER: f64 = 3.0;

/// An overrun this recent (seconds) shows on the status tile for
/// everyone, in the attention colour.
pub const OVERRUN_RECENT: f64 = 60.0;

/// Narrower than this (points) and the app opens on the Monitor: a phone.
pub const NARROW: f32 = 600.0;

/// A lamp that is on, and one that is off (IEC 60073 green and red).
pub const GREEN: Color32 = Color32::from_rgb(0x2E, 0xC4, 0x4A);
pub const RED: Color32 = Color32::from_rgb(0xE5, 0x39, 0x35);

/// Whether a program runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Run {
    Starting,
    Running,
    /// Not scanning: the simulation stopped, or a box module an engineer
    /// stopped (who, when the box says), its outputs at their safe values.
    Stopped(Option<String>),
    Held,
    Faulted(String),
}

/// Whether what a program shows is current.
#[derive(Clone, Debug, PartialEq)]
pub enum Fresh {
    Live,
    /// Not current: how long since the last scan came (`None`: none
    /// ever did, or it is not running), and why.
    Stale {
        age: Option<f64>,
        why: String,
    },
}

/// One program as a source hands it to the Monitor.
pub struct Program<'a> {
    pub name: String,
    pub run: Run,
    /// The scan period, ms.
    pub period_ms: Option<f64>,
    pub scans: u64,
    /// Its source, matching `values` (the instances and outputs are read
    /// from it).
    pub src: Option<&'a str>,
    /// Its newest scan.
    pub values: Option<&'a dyn Scan>,
    pub fresh: Fresh,
    /// Why it cannot be shown in full.
    pub note: Option<String>,
    /// Its scan load as a box measured it; `None` in the simulation.
    pub load: Option<Load>,
}

/// A box module's scan load.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Load {
    /// `None` before the first scan, or from a box too old to measure.
    pub timing: Option<ScanTiming>,
    pub overruns: u64,
    /// Seconds since it last overran; `None`: never (or not said).
    pub overrun_ago: Option<f64>,
}

/// Where the Monitor's values come from. Read-only by construction.
pub trait Feed {
    /// The programs running, in order, as of page time `now` (s).
    fn programs(&self, now: f64) -> Vec<Program<'_>>;
    /// Why the source as a whole cannot be trusted now (the box out of
    /// reach, or silent); `None` when it can.
    fn trouble(&self, now: f64) -> Option<String>;
}

impl Feed for BoxState {
    fn programs(&self, now: f64) -> Vec<Program<'_>> {
        self.modules
            .iter()
            .map(|m| {
                let w = self.programs.get(&m.name);
                let at = w.and_then(|w| w.at);
                Program {
                    name: m.name.clone(),
                    run: match m.state.as_str() {
                        "running" => Run::Running,
                        "held" => Run::Held,
                        "stopped" => Run::Stopped(m.stopped_by.clone()),
                        "faulted" => Run::Faulted(m.fault.clone().unwrap_or_default()),
                        _ => Run::Starting,
                    },
                    period_ms: m.scan_ms,
                    scans: m.scans,
                    src: w.and_then(|w| w.lowered.as_ref()).map(|l| l.src.as_str()),
                    values: w.and_then(|w| w.scan.as_ref()).map(|s| s as &dyn Scan),
                    fresh: match at {
                        Some(t) if now - t <= STALE_AFTER => Fresh::Live,
                        Some(t) => Fresh::Stale {
                            age: Some(now - t),
                            why: "no new scan".into(),
                        },
                        None => Fresh::Stale {
                            age: None,
                            why: "no scan yet".into(),
                        },
                    },
                    note: w.and_then(|w| w.problem.clone()),
                    load: Some(Load {
                        timing: m.timing,
                        overruns: m.overruns,
                        overrun_ago: m.overrun_ago,
                    }),
                }
            })
            .collect()
    }

    fn trouble(&self, now: f64) -> Option<String> {
        match self.link() {
            Link::Connected => match self.values_at {
                Some(t) if now - t > STALE_AFTER => {
                    Some(format!("no data from the box for {}", age_text(now - t)))
                }
                None => Some("waiting for the box".into()),
                Some(_) => None,
            },
            Link::Connecting => Some("connecting to the box".into()),
            Link::Reconnecting { .. } => Some("lost the box; reconnecting".into()),
            Link::Unreachable { .. } => Some("box unreachable".into()),
        }
    }
}

impl Feed for PlaygroundView {
    fn programs(&self, _now: f64) -> Vec<Program<'_>> {
        let s = &self.session;
        let Some(b) = &s.build else {
            return Vec::new();
        };
        let (run, fresh) = match (&s.fault, s.running) {
            (Some(f), _) => (
                Run::Faulted(f.clone()),
                Fresh::Stale {
                    age: None,
                    why: "stopped by a fault".into(),
                },
            ),
            (None, true) => (Run::Running, Fresh::Live),
            (None, false) => (
                Run::Stopped(None),
                Fresh::Stale {
                    age: None,
                    why: "stopped".into(),
                },
            ),
        };
        vec![Program {
            name: self.program_name(),
            run,
            period_ms: Some(s.period_ms as f64),
            scans: s.scans,
            src: Some(&b.src),
            values: s.trace.as_ref().map(|t| t as &dyn Scan),
            fresh,
            note: None,
            load: None,
        }]
    }

    fn trouble(&self, _now: f64) -> Option<String> {
        None
    }
}

/// A program's instances and outputs, read from its source in
/// declaration order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shape {
    /// `[0]` is the program; the rest its user function block instances,
    /// depth first.
    pub scopes: Vec<ScopeShape>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeShape {
    /// `""` for the program, `S1.` for instance `s1` (upper-cased, as the
    /// diagram's scopes and a scan's paths are).
    pub prefix: String,
    /// As written: `s1`, or the program's name.
    pub name: String,
    /// Its function block type as written (empty for the program).
    pub fb: String,
    /// Its own instances, as indices into [`Shape::scopes`].
    pub kids: Vec<usize>,
    /// Its outputs: (as written, path to read).
    pub outs: Vec<(String, String)>,
}

impl Shape {
    /// The shape of the first PROGRAM in `src`; empty if it does not
    /// parse or has none.
    #[must_use]
    pub fn of(src: &str) -> Self {
        let Ok(unit) = ast::parse(src) else {
            return Self::default();
        };
        let fbs: HashMap<String, &ast::Pou> = unit
            .pous
            .iter()
            .filter(|p| p.kind == PouKind::FunctionBlock)
            .map(|p| (p.name.to_ascii_uppercase(), p))
            .collect();
        let Some(program) = unit.pous.iter().find(|p| p.kind == PouKind::Program) else {
            return Self::default();
        };
        let mut shape = Self::default();
        let name = written(src, program.pos, &program.name);
        shape.add(src, program, &fbs, "", name, String::new(), 0);
        shape
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        src: &str,
        pou: &ast::Pou,
        fbs: &HashMap<String, &ast::Pou>,
        prefix: &str,
        name: String,
        fb: String,
        depth: usize,
    ) -> usize {
        let me = self.scopes.len();
        let program = pou.kind == PouKind::Program;
        let outs = pou
            .vars
            .iter()
            .filter(|v| {
                let TypeRef::Named(ty, _) = &v.ty else {
                    return false;
                };
                let located_out = program
                    && v.at.as_ref().is_some_and(|(a, _)| {
                        a.trim_start_matches('%').to_ascii_uppercase().starts_with('Q')
                    });
                (v.section == Section::Output || located_out) && !fbs.contains_key(&ty.to_ascii_uppercase())
            })
            .map(|v| {
                (
                    written(src, v.pos, &v.name),
                    format!("{prefix}{}", v.name.to_ascii_uppercase()),
                )
            })
            .collect();
        self.scopes.push(ScopeShape {
            prefix: prefix.to_string(),
            name,
            fb,
            kids: Vec::new(),
            outs,
        });
        if depth >= 8 {
            return me;
        }
        for v in &pou.vars {
            let TypeRef::Named(ty, at) = &v.ty else {
                continue;
            };
            if let Some(inner) = fbs.get(&ty.to_ascii_uppercase()) {
                let p = format!("{prefix}{}.", v.name.to_ascii_uppercase());
                let (name, fb) = (written(src, v.pos, &v.name), written(src, *at, ty));
                let k = self.add(src, inner, fbs, &p, name, fb, depth + 1);
                self.scopes[me].kids.push(k);
            }
        }
        me
    }

    /// The scope with `prefix`.
    #[must_use]
    pub fn find(&self, prefix: &str) -> Option<usize> {
        self.scopes.iter().position(|s| s.prefix == prefix)
    }
}

/// `name` as the source spells it at `pos` (the parser upper-cases
/// names); lower-cased if `pos` is not on it.
fn written(src: &str, pos: ast::Pos, name: &str) -> String {
    let at: String = src
        .lines()
        .nth(pos.line.saturating_sub(1) as usize)
        .map(|l| {
            l.chars()
                .skip(pos.col.saturating_sub(1) as usize)
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect()
        })
        .unwrap_or_default();
    if at.eq_ignore_ascii_case(name) {
        at
    } else {
        name.to_ascii_lowercase()
    }
}

/// One output on a tile.
#[derive(Clone, Debug, PartialEq)]
pub struct Lamp {
    pub name: String,
    pub path: String,
    /// `None`: no value (the scan does not have it).
    pub value: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TileKind {
    /// Running or faulted, and the scan.
    Status(Run),
    /// A scope's own outputs.
    Outputs,
    /// An instance: a tap goes into it.
    Instance,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    /// The name tests and hits know it by: `status line`, `outputs
    /// line/S1.`, `tile line/S1.`.
    pub key: String,
    pub title: String,
    pub sub: String,
    pub kind: TileKind,
    pub lamps: Vec<Lamp>,
    /// Drawn grey: no colour for data that is not current.
    pub stale: Option<String>,
    /// Where a tap goes: (program, instance prefix).
    pub into: Option<(String, String)>,
    /// A status tile's scan load lines, each with its tooltip.
    pub lines: Vec<Line>,
}

/// One line of numbers on a status tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The name tests and hits know it by: `load line/exec`.
    pub key: String,
    pub text: String,
    /// What the number means and what is normal.
    pub tip: String,
    /// Drawn in the attention colour.
    pub warn: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub program: String,
    pub tiles: Vec<Tile>,
    pub note: Option<String>,
}

/// What a tap on the Monitor asked the app for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wish {
    /// The Editor's diagram of `program`, at `prefix`.
    Diagram { program: String, prefix: String },
    /// A transport button: run or stop, step, restart.
    Act(Action),
}

/// The transport as the Monitor shows it: whether there is something to
/// run, and whether it runs. In the simulation: run or stop, step and
/// restart. On a box (`on_box`; card monitor-box-run-stop, an engineer
/// signed in only): stop or run and restart of the program shown, or of
/// every program at the top level, each through the box's audited route.
/// Anyone else's box Monitor has none and stays watch-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transport {
    pub built: bool,
    pub running: bool,
    pub on_box: bool,
}

/// The Monitor's own state: which level is shown. Values are the feed's.
#[derive(Default)]
pub struct Monitor {
    /// The level shown: (program, instance prefix); `None`: every
    /// program's top level.
    pub at: Option<(String, String)>,
    /// Shapes read, by program, with the source they were read from.
    shapes: HashMap<String, (String, Shape)>,
    /// What the last frame asked the app for.
    pub wish: Option<Wish>,
}

/// "12 s", "3 min", "2 h".
#[must_use]
pub fn age_text(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 120.0 {
        format!("{} s", s.floor())
    } else if s < 7200.0 {
        format!("{} min", (s / 60.0).floor())
    } else {
        format!("{} h", (s / 3600.0).floor())
    }
}

fn stale_text(fresh: &Fresh) -> Option<String> {
    match fresh {
        Fresh::Live => None,
        Fresh::Stale { age: Some(a), why } => Some(format!("stale · {} · {why}", age_text(*a))),
        Fresh::Stale { age: None, why } => Some(format!("stale · {why}")),
    }
}

/// The status tile's second line: the scan period and count when
/// `detail` (an engineer's choice), else nothing; the numbers are too low
/// level for everyone's wall.
fn status_sub(p: &Program<'_>, detail: bool) -> String {
    if !detail {
        return String::new();
    }
    let period = p
        .period_ms
        .map_or_else(String::new, |ms| format!("scan {ms} ms · "));
    format!("{period}{} scans", p.scans)
}

/// A time given in µs, in the unit that reads best: "8.5 µs", "120 µs",
/// "1.25 ms", "35 ms".
#[must_use]
pub fn micros_text(us: f64) -> String {
    let us = us.max(0.0);
    if us < 10.0 {
        format!("{us:.1} µs")
    } else if us < 1000.0 {
        format!("{us:.0} µs")
    } else if us < 10_000.0 {
        format!("{:.2} ms", us / 1000.0)
    } else {
        format!("{:.0} ms", us / 1000.0)
    }
}

/// The status tile's scan load lines. With `detail` (an engineer's
/// choice): scan time, start lateness and overruns, when the box measured
/// them. Without: only an overrun in the last [`OVERRUN_RECENT`] seconds.
/// None for the simulation, which has no real clock to measure.
fn load_lines(p: &Program<'_>, detail: bool) -> Vec<Line> {
    let Some(load) = p.load else {
        return Vec::new();
    };
    let key = |what: &str| format!("load {}/{what}", p.name);
    let period = p
        .period_ms
        .map_or_else(String::new, |ms| format!(" (the scan period is {ms} ms)"));
    let mut out = Vec::new();
    if detail && let Some(t) = load.timing {
        out.push(Line {
            key: key("exec"),
            text: format!(
                "scan {} · peak {}",
                micros_text(t.exec.mean),
                micros_text(t.exec.max)
            ),
            tip: format!(
                "Scan time: how long the program takes to run once, over its last {n} scans. \
                 Average {avg}, 99 in 100 under {p99}, longest {max}, shortest {min}. \
                 Normal is well under the scan period{period}; a scan that takes longer \
                 makes the next one start late.",
                n = t.window,
                avg = micros_text(t.exec.mean),
                p99 = micros_text(t.exec.p99),
                max = micros_text(t.exec.max),
                min = micros_text(t.exec.min),
            ),
            warn: false,
        });
        out.push(Line {
            key: key("late"),
            text: format!(
                "jitter {} · peak {}",
                micros_text(t.late.mean),
                micros_text(t.late.max)
            ),
            tip: format!(
                "Jitter: how late each scan started after the moment it was due, over its last {n} scans. \
                 Average {avg}, 99 in 100 under {p99}, latest {max}. \
                 Normal is a fraction of a millisecond on a quiet box; a busy computer \
                 or a long scan of another program adds to it.",
                n = t.window,
                avg = micros_text(t.late.mean),
                p99 = micros_text(t.late.p99),
                max = micros_text(t.late.max),
            ),
            warn: false,
        });
    }
    let recent = load.overrun_ago.filter(|&a| a <= OVERRUN_RECENT);
    if detail || recent.is_some() {
        let when = load
            .overrun_ago
            .map_or_else(String::new, |a| format!(" · {} ago", age_text(a)));
        out.push(Line {
            key: key("overruns"),
            text: if load.overruns == 0 {
                "no overruns".into()
            } else {
                format!("overruns {}{when}", load.overruns)
            },
            tip: "Overruns: scan deadlines missed since the box started, because a scan \
                  started too late. The program skips the scans it missed and carries on \
                  at its scan period. Normal is none, or a rare one when the computer is \
                  busy. The line is yellow for a minute after each overrun."
                .into(),
            warn: recent.is_some(),
        });
    }
    out
}

impl Monitor {
    fn shape(&mut self, name: &str, src: Option<&str>) -> Option<&Shape> {
        let src = src?;
        let fresh = self.shapes.get(name).is_none_or(|(s, _)| s != src);
        if fresh {
            self.shapes
                .insert(name.to_string(), (src.to_string(), Shape::of(src)));
        }
        self.shapes.get(name).map(|(_, s)| s)
    }

    /// The tiles for the level shown, as of page time `now`; `detail`
    /// puts the scan period and count on the status tiles. A level that
    /// no longer exists (redeployed, renamed) falls back to the top.
    pub fn groups(&mut self, feed: &dyn Feed, now: f64, detail: bool) -> Vec<Group> {
        let programs = feed.programs(now);
        let trouble = feed.trouble(now);
        if let Some((prog, prefix)) = self.at.clone() {
            let found = programs.iter().find(|p| p.name == prog).and_then(|p| {
                let src = p.src;
                self.shape(&prog, src).and_then(|s| s.find(&prefix))
            });
            if found.is_none() {
                self.at = None;
            }
        }
        let mut out = Vec::new();
        for p in &programs {
            let prefix = match &self.at {
                Some((prog, prefix)) if *prog == p.name => prefix.clone(),
                Some(_) => continue,
                None => String::new(),
            };
            // Fresh data and a reachable source, or grey.
            let stale = trouble
                .clone()
                .map(|t| format!("stale · {t}"))
                .or_else(|| stale_text(&p.fresh));
            let status = Tile {
                key: format!("status {}", p.name),
                title: p.name.clone(),
                sub: status_sub(p, detail),
                kind: TileKind::Status(p.run.clone()),
                lamps: Vec::new(),
                stale: trouble.clone().map(|t| format!("stale · {t}")),
                into: None,
                lines: load_lines(p, detail),
            };
            let mut tiles = vec![status];
            let name = p.name.clone();
            let values = p.values;
            if let Some(shape) = self.shape(&name, p.src) {
                let read = |path: &str| values.and_then(|v| v.get(path));
                let lamps = |outs: &[(String, String)]| -> Vec<Lamp> {
                    outs.iter()
                        .map(|(n, path)| Lamp {
                            name: n.clone(),
                            path: path.clone(),
                            value: read(path),
                        })
                        .collect()
                };
                if let Some(k) = shape.find(&prefix) {
                    let scope = &shape.scopes[k];
                    if !scope.outs.is_empty() || k > 0 {
                        let title = if k == 0 {
                            "Outputs".to_string()
                        } else {
                            format!("{} outputs", scope.name)
                        };
                        tiles.push(Tile {
                            key: format!("outputs {name}/{}", scope.prefix),
                            title,
                            sub: if k == 0 {
                                format!("PROGRAM {}", scope.name)
                            } else {
                                scope.fb.clone()
                            },
                            kind: TileKind::Outputs,
                            lamps: lamps(&scope.outs),
                            stale: stale.clone(),
                            into: None,
                            lines: Vec::new(),
                        });
                    }
                    for &kid in &scope.kids {
                        let s = &shape.scopes[kid];
                        tiles.push(Tile {
                            key: format!("tile {name}/{}", s.prefix),
                            title: s.name.clone(),
                            sub: s.fb.clone(),
                            kind: TileKind::Instance,
                            lamps: lamps(&s.outs),
                            stale: stale.clone(),
                            into: Some((name.clone(), s.prefix.clone())),
                            lines: Vec::new(),
                        });
                    }
                }
            }
            out.push(Group {
                program: name,
                tiles,
                note: p.note.clone(),
            });
        }
        out
    }

    /// The breadcrumbs of the level shown: (text, where a tap goes).
    #[must_use]
    pub fn crumbs(&self) -> Vec<(String, Option<(String, String)>)> {
        let mut out = vec![("All programs".to_string(), None)];
        let Some((prog, prefix)) = &self.at else {
            return out;
        };
        out.push((prog.clone(), Some((prog.clone(), String::new()))));
        let Some((_, shape)) = self.shapes.get(prog) else {
            return out;
        };
        let mut p = String::new();
        for seg in prefix.split('.').filter(|s| !s.is_empty()) {
            p = format!("{p}{seg}.");
            let name = shape
                .find(&p)
                .map_or_else(|| seg.to_string(), |k| shape.scopes[k].name.clone());
            out.push((name, Some((prog.clone(), p.clone()))));
        }
        out
    }

    /// One level up.
    pub fn back(&mut self) {
        let crumbs = self.crumbs();
        if crumbs.len() >= 2 {
            self.at.clone_from(&crumbs[crumbs.len() - 2].1);
        }
    }

    /// One frame: the crumbs and the tiles, filling `ui`. Draws what was
    /// where into `chrome`'s hits. `engineer`: the page is signed in as
    /// one, so the "Scan detail" toggle is offered and honoured.
    /// `transport`: the simulation's run or stop, step and restart, drawn
    /// in the header (Nick, 2026-11-01: play, start, stop on the Monitor).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        feed: &dyn Feed,
        chrome: &mut Chrome,
        engineer: bool,
        transport: Option<Transport>,
    ) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        // Ages tick and data goes stale with no event to wake the page.
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        let trouble = feed.trouble(now);
        let area = ctx.content_rect();
        let k = (area.width() / 1100.0).min(area.height() / 650.0).clamp(1.0, 3.5);
        let t = theme(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(12.0 * k))
            .show(ui, |ui| {
                self.top(ui, k, trouble.as_deref(), chrome, engineer, transport);
                // After the toggle, so a tap shows on the same frame.
                let groups = self.groups(feed, now, engineer && chrome.scan);
                ui.add_space(10.0 * k);
                egui::ScrollArea::vertical()
                    .id_salt("monitor-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for g in &groups {
                            if groups.len() > 1 && self.at.is_none() {
                                ui.label(
                                    egui::RichText::new(&g.program)
                                        .font(FontId::proportional(20.0 * k))
                                        .strong()
                                        .color(t.accent),
                                );
                            }
                            if let Some(n) = &g.note {
                                ui.label(egui::RichText::new(n).size(14.0 * k).color(t.muted));
                            }
                            self.grid(ui, &g.tiles, k, chrome);
                            ui.add_space(14.0 * k);
                        }
                        if groups.is_empty() {
                            ui.label(
                                egui::RichText::new("No program is running.")
                                    .size(18.0 * k)
                                    .color(t.muted),
                            );
                        }
                    });
            });
    }

    /// Back, the crumbs, the simulation's transport, "Open diagram", an
    /// engineer's "Scan detail" toggle, and a red banner when the source
    /// cannot be trusted.
    fn top(
        &mut self,
        ui: &mut egui::Ui,
        k: f32,
        trouble: Option<&str>,
        chrome: &mut Chrome,
        engineer: bool,
        transport: Option<Transport>,
    ) {
        let mut go: Option<Option<(String, String)>> = None;
        let crumbs = self.crumbs();
        let t = theme(ui);
        ui.horizontal_wrapped(|ui| {
            let size = 16.0 * k;
            let back = ui
                .add_enabled(
                    self.at.is_some(),
                    egui::Button::new(egui::RichText::new("‹ Back").size(size).strong())
                        .min_size(Vec2::new(TAP * 1.6 * k, TAP * k)),
                )
                .tip(if self.at.is_some() {
                    "Up one level."
                } else {
                    "Up one level: this is the top already."
                });
            chrome.hit("monitor back", back.rect);
            if back.clicked() {
                self.back();
            }
            for (i, (text, to)) in crumbs.iter().enumerate() {
                if i > 0 {
                    ui.label(egui::RichText::new("›").size(size).color(t.muted));
                }
                let last = i + 1 == crumbs.len();
                let rt = egui::RichText::new(text).size(size).monospace();
                let rt = if last {
                    rt.strong().color(t.accent)
                } else {
                    rt.color(t.fg)
                };
                let r = ui
                    .add(
                        egui::Button::new(rt)
                            .frame(false)
                            .min_size(Vec2::new(0.0, TAP * k)),
                    )
                    .tip(if last {
                        "The level shown now."
                    } else {
                        "Goes back to this level."
                    });
                chrome.hit(format!("monitor crumb {i}"), r.rect);
                if r.clicked() && !last {
                    go = Some(to.clone());
                }
            }
            if let Some(t) = transport {
                self.transport(ui, t, chrome);
            }
            let open = ui
                .add(
                    egui::Button::new(egui::RichText::new("Open diagram").size(size))
                        .min_size(Vec2::new(TAP * 2.5 * k, TAP * k)),
                )
                .on_hover_text("this level's diagram in the Editor");
            chrome.hit("monitor diagram", open.rect);
            if open.clicked() {
                let (program, prefix) = self.at.clone().unwrap_or_default();
                self.wish = Some(Wish::Diagram { program, prefix });
            }
            if engineer {
                let scan = ui
                    .add(
                        egui::Button::selectable(chrome.scan, egui::RichText::new("Scan detail").size(size))
                            .min_size(Vec2::new(TAP * 2.5 * k, TAP * k)),
                    )
                    .on_hover_text(
                        "the scan period and count, scan time, jitter and overruns on each status tile (kept in this browser)",
                    );
                chrome.hit("monitor scan", scan.rect);
                if scan.clicked() {
                    chrome.scan = !chrome.scan;
                }
            }
        });
        if let Some(to) = go {
            self.at = to;
        }
        if let Some(t) = trouble {
            let r = ui.label(
                egui::RichText::new(format!("NO DATA · {t}"))
                    .size(18.0 * k)
                    .strong()
                    .color(WHITE)
                    .background_color(RED),
            );
            chrome.hit("monitor banner", r.rect);
        }
    }

    /// The box modules the transport acts on: the program shown, or every
    /// module at the top level.
    #[must_use]
    pub fn targets(&self, modules: &[crate::remote::ModuleRow]) -> Vec<String> {
        match &self.at {
            Some((program, _)) => modules
                .iter()
                .filter(|m| m.name == *program)
                .map(|m| m.name.clone())
                .collect(),
            None => modules.iter().map(|m| m.name.clone()).collect(),
        }
    }

    /// The transport, the Editor's glyphs under the same keys (`run`,
    /// `step`, `restart`), so the hotkeys' tests and the page's behave
    /// alike on either screen. A box has no step.
    fn transport(&mut self, ui: &mut egui::Ui, t: Transport, chrome: &mut Chrome) {
        if t.on_box {
            let (icon, tip) = if t.running {
                (
                    glyphs::Icon::Stop,
                    "Stop on the box: the program stops scanning and its outputs go to their safe values until you run it again (an engineer's; audited).",
                )
            } else {
                (
                    glyphs::Icon::Run,
                    "Run on the box: the program scans again from where it stopped (an engineer's; audited).",
                )
            };
            let items = [
                ("run", icon, t.built, tip, Action::RunStop),
                (
                    "restart",
                    glyphs::Icon::Restart,
                    t.built,
                    "Restart on the box: every variable back to its initial value, then run (an engineer's; audited).",
                    Action::Restart,
                ),
            ];
            for (key, icon, enabled, tip, act) in items {
                let r = glyphs::button(ui, icon, enabled, false, tip);
                chrome.hit(key, r.rect);
                if r.clicked() {
                    self.wish = Some(Wish::Act(act));
                }
            }
            return;
        }
        let (icon, tip) = if !t.built {
            (
                glyphs::Icon::Run,
                "Run (Ctrl+Enter): the program does not compile yet.",
            )
        } else if t.running {
            (glyphs::Icon::Stop, "Stop (Ctrl+Enter, Shift+F8)")
        } else {
            (glyphs::Icon::Run, "Run (Ctrl+Enter)")
        };
        let items = [
            ("run", icon, t.built, tip, Action::RunStop),
            (
                "step",
                glyphs::Icon::Step,
                t.built && !t.running,
                "Step one scan (F10)",
                Action::Step,
            ),
            (
                "restart",
                glyphs::Icon::Restart,
                t.built,
                "Restart: initial values, clock to zero",
                Action::Restart,
            ),
        ];
        for (key, icon, enabled, tip, act) in items {
            let r = glyphs::button(ui, icon, enabled, false, tip);
            chrome.hit(key, r.rect);
            if r.clicked() {
                self.wish = Some(Wish::Act(act));
            }
        }
    }

    /// Tiles in as many columns as fit, each a finger's width or more.
    fn grid(&mut self, ui: &mut egui::Ui, tiles: &[Tile], k: f32, chrome: &mut Chrome) {
        let gap = 12.0 * k;
        let width = ui.available_width();
        let min = (260.0 * k).min(width);
        let cols = (((width + gap) / (min + gap)).floor() as usize).max(1);
        let w = (width - gap * (cols - 1) as f32) / cols as f32;
        for row in tiles.chunks(cols) {
            let h = row
                .iter()
                .map(|t| tile_height(t, k))
                .fold(TAP * 2.0 * k, f32::max);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
            for (i, t) in row.iter().enumerate() {
                let x = rect.min.x + i as f32 * (w + gap);
                let r = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(w, h));
                let resp = ui.interact(r, ui.scope_id().with(&t.key), Sense::click());
                chrome.hit(t.key.clone(), r);
                let lines = draw_tile(ui, r, t, k, resp.hovered() && t.into.is_some());
                for (line, lr) in t.lines.iter().zip(lines) {
                    chrome.hit(line.key.clone(), lr);
                    let _ = ui
                        .interact(lr, ui.scope_id().with(&line.key), Sense::hover())
                        .on_hover_text(&line.tip);
                }
                if resp.clicked()
                    && let Some(to) = &t.into
                {
                    self.at = Some(to.clone());
                }
            }
            ui.add_space(gap);
        }
    }
}

fn tile_height(t: &Tile, k: f32) -> f32 {
    let head = 64.0 * k;
    let body = match &t.kind {
        TileKind::Status(Run::Faulted(_)) => 70.0 * k,
        TileKind::Status(_) => 48.0 * k,
        _ => t.lamps.len().max(1) as f32 * 30.0 * k,
    };
    let foot = if t.stale.is_some() { 26.0 * k } else { 0.0 };
    let extra = match &t.kind {
        TileKind::Status(Run::Stopped(Some(_))) => 22.0 * k,
        _ => 0.0,
    };
    head + body + extra + t.lines.len() as f32 * LINE_H * k + foot + 12.0 * k
}

/// A load line's height, before scaling.
const LINE_H: f32 = 24.0;

/// Draws `t` in `r`; returns where each of its [`Line`]s went.
fn draw_tile(ui: &egui::Ui, r: Rect, t: &Tile, k: f32, hot: bool) -> Vec<Rect> {
    let p = ui.painter_at(r);
    let th = theme(ui);
    let stale = t.stale.is_some();
    // A stale tile sinks halfway back into the page.
    let fill = if stale {
        th.panel.gamma_multiply(0.5)
    } else {
        th.panel
    };
    let edge = if hot { YELLOW } else { th.rule };
    p.rect(
        r,
        CornerRadius::same(8),
        fill,
        Stroke::new(1.5 * k, edge),
        StrokeKind::Inside,
    );
    let ink = if stale { th.muted } else { th.fg };
    let pad = 14.0 * k;
    let mut spots = Vec::new();
    let mut y = r.min.y + pad;
    let title = if t.into.is_some() {
        format!("{} ›", t.title)
    } else {
        t.title.clone()
    };
    p.text(
        Pos2::new(r.min.x + pad, y),
        egui::Align2::LEFT_TOP,
        title,
        FontId::proportional(24.0 * k),
        ink,
    );
    y += 30.0 * k;
    p.text(
        Pos2::new(r.min.x + pad, y),
        egui::Align2::LEFT_TOP,
        &t.sub,
        FontId::monospace((14.0 * k).max(crate::theme::MIN_TEXT)),
        th.muted,
    );
    y += 26.0 * k;
    match &t.kind {
        TileKind::Status(run) => {
            let (word, color) = match run {
                Run::Running => ("RUNNING", GREEN),
                Run::Faulted(_) => ("FAULTED", RED),
                Run::Held => ("HELD", th.accent),
                Run::Stopped(_) => ("STOPPED", th.muted),
                Run::Starting => ("STARTING", th.muted),
            };
            let color = if stale { th.muted } else { color };
            p.text(
                Pos2::new(r.min.x + pad, y),
                egui::Align2::LEFT_TOP,
                word,
                FontId::proportional(30.0 * k),
                color,
            );
            y += 40.0 * k;
            if let Run::Stopped(Some(by)) = run {
                p.text(
                    Pos2::new(r.min.x + pad, y),
                    egui::Align2::LEFT_TOP,
                    format!("outputs at safe values · by {by}"),
                    FontId::proportional((14.0 * k).max(crate::theme::MIN_TEXT)),
                    ink,
                );
                y += 22.0 * k;
            }
            if let Run::Faulted(f) = run {
                p.text(
                    Pos2::new(r.min.x + pad, y),
                    egui::Align2::LEFT_TOP,
                    f,
                    FontId::proportional((14.0 * k).max(crate::theme::MIN_TEXT)),
                    ink,
                );
                y += 22.0 * k;
            }
            for l in &t.lines {
                let color = if stale {
                    th.muted
                } else if l.warn {
                    th.accent
                } else {
                    ink
                };
                p.text(
                    Pos2::new(r.min.x + pad, y),
                    egui::Align2::LEFT_TOP,
                    &l.text,
                    FontId::proportional((15.0 * k).max(crate::theme::MIN_TEXT)),
                    color,
                );
                spots.push(Rect::from_min_size(
                    Pos2::new(r.min.x + pad, y),
                    Vec2::new((r.width() - 2.0 * pad).max(0.0), LINE_H * k),
                ));
                y += LINE_H * k;
            }
        }
        TileKind::Outputs | TileKind::Instance => {
            if t.lamps.is_empty() {
                p.text(
                    Pos2::new(r.min.x + pad, y),
                    egui::Align2::LEFT_TOP,
                    "no outputs",
                    FontId::proportional((16.0 * k).max(crate::theme::MIN_TEXT)),
                    th.muted,
                );
                y += 30.0 * k;
            }
            for l in &t.lamps {
                let mid = y + 12.0 * k;
                let rad = 9.0 * k;
                let c = Pos2::new(r.min.x + pad + rad, mid);
                let (lamp, text) = match l.value {
                    Some(Value::Bool(on)) => (Some(on), None),
                    v => (None, Some(crate::st_view::show(v))),
                };
                match lamp {
                    Some(on) if !stale => {
                        p.circle_filled(c, rad, if on { GREEN } else { RED });
                    }
                    Some(_) => {
                        p.circle_stroke(c, rad, Stroke::new(1.5 * k, th.muted));
                    }
                    None => {}
                }
                let x = if lamp.is_some() || text.is_none() {
                    r.min.x + pad + rad * 2.0 + 10.0 * k
                } else {
                    r.min.x + pad
                };
                p.text(
                    Pos2::new(x, mid),
                    egui::Align2::LEFT_CENTER,
                    &l.name,
                    FontId::proportional((18.0 * k).max(crate::theme::MIN_TEXT)),
                    ink,
                );
                if let Some(v) = text {
                    p.text(
                        Pos2::new(r.max.x - pad, mid),
                        egui::Align2::RIGHT_CENTER,
                        v,
                        FontId::monospace((18.0 * k).max(crate::theme::MIN_TEXT)),
                        if stale { th.muted } else { th.accent },
                    );
                }
                y += 30.0 * k;
            }
        }
    }
    if let Some(s) = &t.stale {
        p.text(
            Pos2::new(r.min.x + pad, y + 4.0 * k),
            egui::Align2::LEFT_TOP,
            s,
            FontId::proportional((15.0 * k).max(crate::theme::MIN_TEXT)),
            th.muted,
        );
    }
    spots
}
