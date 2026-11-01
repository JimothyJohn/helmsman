//! The Monitor (docs/UI.md): phone to wall TV, watch only. Status tiles
//! per program (running or faulted; the scan period and counts only for
//! an engineer who turned "Scan detail" on, kept per browser in
//! [`Chrome::scan`]: card `monitor-scan-time`), then a tile per top-level
//! instance with its outputs (a BOOL a green or red lamp, a number its
//! value) and one for the program's own outputs; a tap goes one level
//! down into that instance, Back and the breadcrumbs go up, and "Open
//! diagram" hands that scope to the Editor.
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
use crate::playground_view::PlaygroundView;
use crate::remote::{BoxState, Link};
use crate::st_view::TAP;
use crate::st_view::brand::{BLACK, GRAPHITE, PANEL, STEEL, WHITE, YELLOW};

/// Seconds without a new scan after which a program's tiles go grey.
pub const STALE_AFTER: f64 = 3.0;

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
    Stopped,
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
                Run::Stopped,
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
    pub fn show(&mut self, ui: &mut egui::Ui, feed: &dyn Feed, chrome: &mut Chrome, engineer: bool) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        // Ages tick and data goes stale with no event to wake the page.
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        let trouble = feed.trouble(now);
        let area = ctx.content_rect();
        let k = (area.width() / 1100.0).min(area.height() / 650.0).clamp(1.0, 3.5);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BLACK).inner_margin(12.0 * k))
            .show(ui, |ui| {
                self.top(ui, k, trouble.as_deref(), chrome, engineer);
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
                                        .color(YELLOW),
                                );
                            }
                            if let Some(n) = &g.note {
                                ui.label(egui::RichText::new(n).size(14.0 * k).color(STEEL));
                            }
                            self.grid(ui, &g.tiles, k, chrome);
                            ui.add_space(14.0 * k);
                        }
                        if groups.is_empty() {
                            ui.label(
                                egui::RichText::new("No program is running.")
                                    .size(18.0 * k)
                                    .color(STEEL),
                            );
                        }
                    });
            });
    }

    /// Back, the crumbs, "Open diagram", an engineer's "Scan detail"
    /// toggle, and a red banner when the source cannot be trusted.
    fn top(&mut self, ui: &mut egui::Ui, k: f32, trouble: Option<&str>, chrome: &mut Chrome, engineer: bool) {
        let mut go: Option<Option<(String, String)>> = None;
        let crumbs = self.crumbs();
        ui.horizontal_wrapped(|ui| {
            let size = 16.0 * k;
            let back = ui.add_enabled(
                self.at.is_some(),
                egui::Button::new(egui::RichText::new("‹ Back").size(size).strong())
                    .min_size(Vec2::new(TAP * 1.6 * k, TAP * k)),
            );
            chrome.hit("monitor back", back.rect);
            if back.clicked() {
                self.back();
            }
            for (i, (text, to)) in crumbs.iter().enumerate() {
                if i > 0 {
                    ui.label(egui::RichText::new("›").size(size).color(STEEL));
                }
                let last = i + 1 == crumbs.len();
                let t = egui::RichText::new(text).size(size).monospace();
                let t = if last {
                    t.strong().color(YELLOW)
                } else {
                    t.color(WHITE)
                };
                let r = ui.add(
                    egui::Button::new(t)
                        .frame(false)
                        .min_size(Vec2::new(0.0, TAP * k)),
                );
                chrome.hit(format!("monitor crumb {i}"), r.rect);
                if r.clicked() && !last {
                    go = Some(to.clone());
                }
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
                    .on_hover_text("the scan period and count on each status tile (kept in this browser)");
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
                draw_tile(ui, r, t, k, resp.hovered() && t.into.is_some());
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
    head + body + foot + 12.0 * k
}

fn draw_tile(ui: &egui::Ui, r: Rect, t: &Tile, k: f32, hot: bool) {
    let p = ui.painter_at(r);
    let stale = t.stale.is_some();
    let fill = if stale { Color32::from_gray(0x1C) } else { PANEL };
    let edge = if hot { YELLOW } else { GRAPHITE };
    p.rect(
        r,
        CornerRadius::same(8),
        fill,
        Stroke::new(1.5 * k, edge),
        StrokeKind::Inside,
    );
    let ink = if stale { STEEL } else { WHITE };
    let pad = 14.0 * k;
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
        FontId::monospace(14.0 * k),
        STEEL,
    );
    y += 26.0 * k;
    match &t.kind {
        TileKind::Status(run) => {
            let (word, color) = match run {
                Run::Running => ("RUNNING", GREEN),
                Run::Faulted(_) => ("FAULTED", RED),
                Run::Held => ("HELD", YELLOW),
                Run::Stopped => ("STOPPED", STEEL),
                Run::Starting => ("STARTING", STEEL),
            };
            let color = if stale { STEEL } else { color };
            p.text(
                Pos2::new(r.min.x + pad, y),
                egui::Align2::LEFT_TOP,
                word,
                FontId::proportional(30.0 * k),
                color,
            );
            y += 40.0 * k;
            if let Run::Faulted(f) = run {
                p.text(
                    Pos2::new(r.min.x + pad, y),
                    egui::Align2::LEFT_TOP,
                    f,
                    FontId::proportional(14.0 * k),
                    ink,
                );
                y += 22.0 * k;
            }
        }
        TileKind::Outputs | TileKind::Instance => {
            if t.lamps.is_empty() {
                p.text(
                    Pos2::new(r.min.x + pad, y),
                    egui::Align2::LEFT_TOP,
                    "no outputs",
                    FontId::proportional(16.0 * k),
                    STEEL,
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
                        p.circle_stroke(c, rad, Stroke::new(1.5 * k, STEEL));
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
                    FontId::proportional(18.0 * k),
                    ink,
                );
                if let Some(v) = text {
                    p.text(
                        Pos2::new(r.max.x - pad, mid),
                        egui::Align2::RIGHT_CENTER,
                        v,
                        FontId::monospace(18.0 * k),
                        if stale { STEEL } else { YELLOW },
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
            FontId::proportional(15.0 * k),
            STEEL,
        );
    }
}
