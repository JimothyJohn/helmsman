//! The proposal review screen: the list of drafts, and one draft in full
//! (tier, source diff, diagram diff, manifest or param change, sim replay
//! chart, approve as whoever signed in, then undo). Decisions are
//! [`Review`]'s; this draws them and turns clicks into its calls. In the
//! browser `web::Http` carries the requests; headless, tests carry them.
//!
//! Read-only toward the ST (`canvas-edit-scope`): nothing here edits a
//! draft. Approving is the only thing it sends, through the box's policy.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Vec2,
};
use helmsman_diagram::diff::{self, Change, Line};
use helmsman_diagram::layout::{HEADER, LABEL_H, in_port, in_port_at, out_port, out_port_at};
use helmsman_diagram::{Diagram, NodeId};

use crate::review::{
    Answer, Call, Chart, Delta, Drawn, Open, Phase, ROLES, Refusal, Replay, Request, Review, SHOWN,
    SignalChart, Step, Target, Track, Values, Why,
};
use crate::st_view::brand::{BLACK, YELLOW};
use crate::theme::{Theme, theme};
use crate::tips::Tip;

const RED: Color32 = Color32::from_rgb(255, 110, 100);
/// Removed lines and ghost blocks.
const GONE: Color32 = Color32::from_rgb(0x6A, 0x6A, 0x6A);
/// Gap between the proposed diagram and the ghosts of removed blocks.
const GHOST_GAP: f32 = 60.0;
const DIAGRAM_H: f32 = 420.0;

/// Pan and zoom of the diagram diff: `screen = origin + offset + world * zoom`.
#[derive(Clone, Copy, Debug)]
pub struct Cam {
    pub offset: Vec2,
    pub zoom: f32,
    pub origin: Pos2,
    pub fitted: bool,
}

impl Default for Cam {
    fn default() -> Self {
        Self {
            offset: Vec2::splat(20.0),
            zoom: 1.0,
            origin: Pos2::ZERO,
            fitted: false,
        }
    }
}

impl Cam {
    #[must_use]
    pub fn to_screen(self, x: f32, y: f32) -> Pos2 {
        self.origin + self.offset + Vec2::new(x, y) * self.zoom
    }

    fn rect(self, r: &helmsman_diagram::Rect) -> Rect {
        Rect::from_min_max(self.to_screen(r.x, r.y), self.to_screen(r.right(), r.bottom()))
    }
}

pub struct ReviewView {
    pub state: Review,
    /// The sign-in fields; the password or PIN is emptied when "Sign in"
    /// sends it.
    pub user_input: String,
    pub password_input: String,
    /// The role picked to sign in with its PIN (boxes without accounts).
    pub role_input: String,
    pub pin_input: String,
    /// Which POU of the diagram diff is shown.
    pub pou: usize,
    pub cam: Cam,
    /// Where this frame drew its controls (`user`, `password`, `role R`,
    /// `pin`, `login`, `logout`, `row N`, `replay T`, `go T` for target
    /// `T`'s `Debug`): what a headless test clicks.
    pub rects: Vec<(String, Rect)>,
    #[cfg(target_arch = "wasm32")]
    http: crate::web::Http,
    /// Requests not carried out (no transport here: tests read them).
    #[cfg(not(target_arch = "wasm32"))]
    pub sent: Vec<Request>,
}

impl Default for ReviewView {
    fn default() -> Self {
        Self::new()
    }
}

/// The tier badge's colours: output-affecting in the hazard pair
/// (black on yellow), logic black on white, param white on graphite.
#[must_use]
pub fn tier_colors(t: &Theme, tier: &str) -> (Color32, Color32) {
    match tier {
        "output-affecting" => (YELLOW, BLACK),
        "logic" => (t.fg, t.bg),
        _ => (t.rule, t.fg),
    }
}

fn badge(ui: &mut egui::Ui, text: &str, (bg, fg): (Color32, Color32)) {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text.to_uppercase()).strong().monospace().color(fg));
        });
}

impl ReviewView {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Review::new(),
            user_input: String::new(),
            password_input: String::new(),
            role_input: ROLES[0].to_string(),
            pin_input: String::new(),
            pou: 0,
            cam: Cam::default(),
            rects: Vec::new(),
            #[cfg(target_arch = "wasm32")]
            http: crate::web::Http::default(),
            #[cfg(not(target_arch = "wasm32"))]
            sent: Vec::new(),
        }
    }

    /// Takes in answers at page time `now`, then sends what is queued.
    pub fn step(&mut self, ctx: &egui::Context, answers: Vec<(Call, Answer)>, now: f64) {
        for (call, a) in answers {
            self.state.answer(call, &a, now);
        }
        let reqs = self.state.poll();
        self.run(ctx, reqs);
        // Keep the lockout countdown ticking.
        if self.state.open.as_ref().is_some_and(|o| {
            o.step.locked_for(now).is_some() || o.undo.as_ref().is_some_and(|u| u.locked_for(now).is_some())
        }) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Takes in what arrived and sends what is queued, without drawing:
    /// the header's badge, while another screen is up.
    pub fn tick(&mut self, ctx: &egui::Context, now: f64) {
        let answers = self.inbox();
        self.step(ctx, answers, now);
    }

    #[cfg(target_arch = "wasm32")]
    fn run(&mut self, ctx: &egui::Context, reqs: Vec<Request>) {
        for r in reqs {
            self.http.send(r, ctx);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    fn run(&mut self, _ctx: &egui::Context, reqs: Vec<Request>) {
        self.sent.extend(reqs);
    }

    #[cfg(target_arch = "wasm32")]
    fn inbox(&mut self) -> Vec<(Call, Answer)> {
        self.http.drain()
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    fn inbox(&mut self) -> Vec<(Call, Answer)> {
        Vec::new()
    }

    /// One frame.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let answers = self.inbox();
        self.step(&ctx, answers, now);
        self.rects.clear();
        // Narrow screens (a tablet held upright): a slimmer list.
        let narrow = ui.available_width() < 900.0;
        egui::Panel::left("review-left")
            .resizable(true)
            .default_size(if narrow { 200.0 } else { 280.0 })
            .show(ui, |ui| egui::ScrollArea::vertical().show(ui, |ui| self.left(ui)));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| self.center(ui, now));
        });
    }

    fn left(&mut self, ui: &mut egui::Ui) {
        let t = theme(ui);
        ui.heading(RichText::new("Proposals").color(t.accent))
            .tip("Drafts the agent made. Approving one goes through the box's policy.");
        ui.separator();
        self.login_ui(ui);
        ui.separator();
        self.drafts_ui(ui);
    }

    /// Who is approving: signed in, or the way in this box takes (a user
    /// and password with accounts, else a role and its PIN).
    pub fn login_ui(&mut self, ui: &mut egui::Ui) {
        let t = theme(ui);
        ui.label(RichText::new("Signed in").strong()).tip(SESSION);
        if let Some(who) = self.state.signed_in() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("● {who}")).color(t.accent))
                    .tip("Approvals and edits are made as this user.");
                let out = ui
                    .button("Sign out")
                    .tip("Ends the session now, here and on the box (Ctrl+F8).");
                self.rects.push(("logout".into(), out.rect));
                if out.clicked() {
                    self.state.logout();
                }
            });
            return;
        }
        let go = match self.state.credentials() {
            None => {
                ui.label(RichText::new("asking the box how to sign in…").color(t.muted));
                return;
            }
            Some("unavailable") => {
                ui.colored_label(
                    RED,
                    "The box's accounts file cannot be used: nobody can sign in (see its journal).",
                );
                return;
            }
            Some("accounts") => self.account_fields(ui),
            Some(_) => self.pin_fields(ui),
        };
        if go {
            if self.state.accounts() {
                let password = std::mem::take(&mut self.password_input);
                self.state.login(&self.user_input, password);
            } else {
                let pin = std::mem::take(&mut self.pin_input);
                self.state.login_pin(&self.role_input, pin);
            }
        }
        if self.state.logging_in {
            ui.spinner();
        }
        if let Some(p) = &self.state.login_problem {
            ui.colored_label(RED, p);
        }
    }

    /// A user name and masked password; whether "Sign in" was asked for.
    fn account_fields(&mut self, ui: &mut egui::Ui) -> bool {
        let user = ui
            .add(
                egui::TextEdit::singleline(&mut self.user_input)
                    .hint_text("user")
                    .desired_width(f32::INFINITY),
            )
            .tip("Your user name on this box.");
        self.rects.push(("user".into(), user.rect));
        let pass = ui
            .add(
                egui::TextEdit::singleline(&mut self.password_input)
                    .password(true)
                    .hint_text("password")
                    .desired_width(f32::INFINITY),
            )
            .tip("Your password. Enter signs in.");
        self.rects.push(("password".into(), pass.rect));
        let enter = pass.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let go = ui
            .add_enabled(!self.state.logging_in, egui::Button::new("Sign in"))
            .tip(sign_in_tip(self.state.logging_in));
        self.rects.push(("login".into(), go.rect));
        go.clicked() || enter
    }

    /// A role and its masked PIN; whether "Sign in" was asked for.
    fn pin_fields(&mut self, ui: &mut egui::Ui) -> bool {
        ui.horizontal_wrapped(|ui| {
            for r in ROLES {
                let b = ui
                    .selectable_value(&mut self.role_input, r.to_string(), r)
                    .tip(format!("Sign in as {r}, with that role's PIN."));
                self.rects.push((format!("role {r}"), b.rect));
            }
        });
        let pin = ui
            .add(
                egui::TextEdit::singleline(&mut self.pin_input)
                    .password(true)
                    .hint_text("PIN")
                    .desired_width(f32::INFINITY),
            )
            .tip("The role's PIN. Enter signs in.");
        self.rects.push(("pin".into(), pin.rect));
        let enter = pin.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let go = ui
            .add_enabled(!self.state.logging_in, egui::Button::new("Sign in"))
            .tip(sign_in_tip(self.state.logging_in));
        self.rects.push(("login".into(), go.rect));
        go.clicked() || enter
    }

    fn drafts_ui(&mut self, ui: &mut egui::Ui) {
        let t = theme(ui);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Drafts").strong())
                .tip("The box's proposals, newest last. Click one to review it.");
            if ui
                .small_button("Refresh")
                .tip("Asks the box for its proposals again.")
                .clicked()
            {
                self.state.refresh();
            }
        });
        if let Some(e) = &self.state.list_error {
            ui.colored_label(RED, e);
        }
        if self.state.list.is_empty() {
            ui.label(RichText::new("No proposals on this box.").color(t.muted));
        }
        let mut chosen = None;
        let open = self.state.open.as_ref().map(|o| o.id);
        for r in &self.state.list {
            ui.horizontal_wrapped(|ui| {
                if let Some(tier) = &r.tier {
                    badge(ui, tier, tier_colors(&t, tier));
                }
                let text = RichText::new(format!("#{} {} · {}", r.id, r.title, r.status)).monospace();
                let row = ui
                    .selectable_label(open == Some(r.id), text)
                    .tip("Click to review this proposal.");
                self.rects.push((format!("row {}", r.id), row.rect));
                if row.clicked() {
                    chosen = Some(r.id);
                }
            });
        }
        if let Some(id) = chosen {
            self.state.open(id);
            self.pou = 0;
            self.cam = Cam::default();
        }
    }

    fn center(&mut self, ui: &mut egui::Ui, now: f64) {
        let t = theme(ui);
        let signed_in = self.state.signed_in();
        let Some(o) = self.state.open.as_mut() else {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Pick a proposal to review it.").color(t.muted));
            });
            return;
        };
        header(ui, o);
        ui.separator();
        if let Some(p) = &o.param {
            section(ui, "Param change");
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{}.{} ({})", p.module, p.name, p.ty))
                        .monospace()
                        .strong(),
                );
                ui.label(RichText::new(&p.from).monospace().color(t.muted));
                ui.label(RichText::new("→").monospace());
                ui.label(RichText::new(&p.to).monospace().color(t.accent).strong());
            });
            let range = match (&p.min, &p.max) {
                (Some(lo), Some(hi)) => format!("declared range {lo} .. {hi}"),
                (Some(lo), None) => format!("declared minimum {lo}"),
                (None, Some(hi)) => format!("declared maximum {hi}"),
                (None, None) => "no declared range".into(),
            };
            ui.label(RichText::new(range).color(t.muted));
        }
        if !o.manifest.is_empty() {
            section(ui, "Manifest");
            manifest(ui, o);
        }
        if o.json["kind"] == "module" {
            section(ui, "Source");
            if o.current.is_none() {
                ui.label(RichText::new("loading what runs now…").color(t.muted));
            } else {
                source(ui, &o.lines);
            }
            section(ui, "Diagram");
            if let Some(p) = &o.problem {
                ui.colored_label(RED, p);
            }
            if let Some(d) = &o.drawn {
                diagram(ui, d, &mut self.pou, &mut self.cam);
            }
        }
        let mut out = Out {
            signed_in,
            ..Out::default()
        };
        section(ui, "Sim replay");
        replay_ui(ui, &o.step, &mut out);
        if let Some(v) = o.values() {
            section(ui, "Running values").tip(VALUES);
            values_ui(ui, &v, &mut out);
        }
        section(ui, "Approve");
        approve_ui(ui, o, now, &mut out);
        if let Some(u) = o.undo.as_mut() {
            section(ui, "Undo").tip("Roll back to what ran before, through the same approval check.");
            replay_ui(ui, u, &mut out);
            step_ui(ui, u, now, "Undo", &mut out);
        }
        if let Some(h) = &self.state.head {
            ui.separator();
            ui.label(
                RichText::new(format!(
                    "Box history head: #{} {} by {} ({})",
                    h["seq"],
                    short_id(h["id"].as_str().unwrap_or("?")),
                    h["approver"].as_str().unwrap_or("?"),
                    h["kind"]["kind"].as_str().unwrap_or("?")
                ))
                .monospace()
                .color(t.muted),
            );
        }
        self.rects.extend(out.rects);
        for a in out.act {
            match a {
                Act::Replay(t) => self.state.replay(&t),
                Act::Approve(t) => self.state.approve(&t, now),
            }
        }
    }
}

/// What a click asked for, carried out after drawing.
enum Act {
    Replay(Target),
    Approve(Target),
}

/// What drawing collected: clicks to act on, and where the controls are.
#[derive(Default)]
struct Out {
    act: Vec<Act>,
    rects: Vec<(String, Rect)>,
    /// `alice (engineer)` while someone is logged in: approvals are theirs.
    signed_in: Option<String>,
}

/// What the session note under the sign-in said, now the heading's tooltip.
const SESSION: &str = "The box keeps the session (1 h, or 15 min unused) in a cookie this page \
                       cannot read; signing out ends it on the box. Approvals are made as who \
                       is signed in.";

/// The Sign in button's tooltip.
const fn sign_in_tip(busy: bool) -> &'static str {
    if busy {
        "Signing in: waiting for the box."
    } else {
        "Signs in to this box (Alt+F8)."
    }
}

fn section(ui: &mut egui::Ui, title: &str) -> egui::Response {
    let t = theme(ui);
    ui.add_space(10.0);
    ui.label(RichText::new(title.to_uppercase()).strong().color(t.accent))
}

fn short_id(s: &str) -> String {
    s.chars().take(12).collect()
}

fn header(ui: &mut egui::Ui, o: &Open) {
    let t = theme(ui);
    ui.horizontal_wrapped(|ui| {
        match o.tier() {
            Some(tier) => badge(ui, tier, tier_colors(&t, tier)),
            None => badge(ui, "no tier", (t.rule, t.fg)),
        }
        let title = match o.json["kind"].as_str() {
            Some("module") => format!(
                "#{} {} module {}",
                o.id,
                if o.json["replaces"] == false {
                    "new"
                } else {
                    "changed"
                },
                o.json["name"].as_str().unwrap_or("?")
            ),
            _ => format!(
                "#{} param {}.{}",
                o.id,
                o.json["module"].as_str().unwrap_or("?"),
                o.json["param"].as_str().unwrap_or("?")
            ),
        };
        ui.label(RichText::new(title).heading().color(t.fg));
        if o.json["touches_outputs"] == true {
            badge(ui, "drives outputs", (YELLOW, BLACK));
        }
        if o.applied() {
            badge(ui, "applied", (t.fg, t.bg));
        }
    });
    if o.tier().is_none() {
        ui.colored_label(
            RED,
            "The box gave this draft no tier (it does not run a change set), so it cannot be applied here.",
        );
    }
    for r in o.reasons() {
        ui.label(RichText::new(format!("· {r}")).color(t.muted));
    }
    if let Some((role, replay)) = o.needs() {
        let what = if replay {
            format!("Needs: {role} or above signed in, and a sim replay of exactly this change.")
        } else {
            format!("Needs: {role} or above signed in.")
        };
        ui.label(RichText::new(what).strong());
    }
    if let Some(n) = o.json["note"].as_str() {
        ui.label(
            RichText::new(format!("Agent's note: {n}"))
                .italics()
                .color(t.muted),
        );
    }
    if let Some(a) = o.json["manifest"]["author"].as_str() {
        ui.label(RichText::new(format!("by {a}")).small().color(t.muted));
    }
}

fn manifest(ui: &mut egui::Ui, o: &Open) {
    let t = theme(ui);
    egui::Grid::new("manifest").striped(true).show(ui, |ui| {
        for r in &o.manifest {
            let mark = match r.delta {
                Delta::Same => " ",
                Delta::Added => "+",
                Delta::Removed => "−",
                Delta::Changed => "~",
            };
            let hot = r.output && r.delta != Delta::Same;
            let color = match r.delta {
                Delta::Same => t.muted,
                Delta::Removed => GONE,
                _ => t.fg,
            };
            ui.label(RichText::new(mark).monospace().strong().color(color));
            ui.label(RichText::new(r.field).monospace().color(t.muted));
            if hot {
                badge(ui, &r.name, (YELLOW, BLACK));
            } else {
                ui.label(RichText::new(&r.name).monospace().color(color));
            }
            let old = r.old.clone().unwrap_or_else(|| "–".into());
            let new = r.new.clone().unwrap_or_else(|| "–".into());
            let text = match (r.field, r.delta) {
                ("drives" | "reads", _) => String::new(),
                (_, Delta::Same) => new,
                _ => format!("{old} → {new}"),
            };
            ui.label(RichText::new(text).monospace().color(color));
            if hot {
                ui.label(RichText::new("can move an output").small().color(t.accent));
            } else {
                ui.label("");
            }
            ui.end_row();
        }
    });
}

/// The source diff, changed lines with a few lines of context around them.
fn source(ui: &mut egui::Ui, lines: &[Line]) {
    const CONTEXT: usize = 3;
    let t = theme(ui);
    if lines.iter().all(|l| !l.is_change()) {
        ui.label(RichText::new("The source is unchanged.").color(t.muted));
        return;
    }
    let near: Vec<bool> = (0..lines.len())
        .map(|k| {
            let lo = k.saturating_sub(CONTEXT);
            let hi = (k + CONTEXT + 1).min(lines.len());
            lines[lo..hi].iter().any(Line::is_change)
        })
        .collect();
    egui::Frame::new()
        .fill(t.bg)
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            let mut skipped = false;
            for (k, l) in lines.iter().enumerate() {
                if !near[k] {
                    if !skipped {
                        ui.label(RichText::new("   ⋯").monospace().color(t.muted));
                    }
                    skipped = true;
                    continue;
                }
                skipped = false;
                let (gutter, fg, bg) = match l {
                    Line::Same { new, .. } => (format!("{new:>4}  "), t.muted, None),
                    Line::Added { new, .. } => (format!("{new:>4} +"), BLACK, Some(YELLOW)),
                    Line::Removed { old, .. } => (format!("{old:>4} −"), t.fg, Some(t.rule)),
                };
                let mut t = RichText::new(format!("{gutter} {}", l.text()))
                    .monospace()
                    .color(fg);
                if let Some(bg) = bg {
                    t = t.background_color(bg);
                }
                if matches!(l, Line::Removed { .. }) {
                    t = t.strikethrough();
                }
                ui.label(t);
            }
        });
}

fn diagram(ui: &mut egui::Ui, d: &Drawn, pou: &mut usize, cam: &mut Cam) {
    let t = theme(ui);
    let (added, removed, changed) = d.diff.counts();
    ui.label(format!(
        "{added} block(s) added, {changed} changed, {removed} removed"
    ))
    .tip(
        "Yellow: added. Yellow outline: changed. Dashed grey, below the diagram: removed. \
         Drag to pan, pinch to zoom, double-click to fit.",
    );
    if d.diff.is_empty() {
        ui.label(RichText::new("The diagram is unchanged.").color(t.muted));
    }
    let names: Vec<String> = d.new.pous.iter().map(|p| p.name.clone()).collect();
    let changed_pous = d.diff.pous();
    ui.horizontal_wrapped(|ui| {
        for (k, n) in names.iter().enumerate() {
            let mark = if changed_pous.contains(n) { " •" } else { "" };
            let tip = if mark.is_empty() {
                "Shows this POU's diagram."
            } else {
                "Shows this POU's diagram. • : the change touches it."
            };
            if ui
                .selectable_label(*pou == k, format!("{n}{mark}"))
                .tip(tip)
                .clicked()
                && *pou != k
            {
                *pou = k;
                cam.fitted = false;
            }
        }
    });
    for gone in &d.diff.pous_removed {
        ui.label(
            RichText::new(format!("{gone} removed"))
                .color(GONE)
                .strikethrough(),
        );
    }
    let Some(p) = d.new.pous.get(*pou) else { return };
    let old = d.old.as_ref().and_then(|m| m.pou(&p.name)).map(|q| &q.diagram);
    let width = ui.available_width().max(200.0);
    let (area, response) = ui.allocate_exact_size(Vec2::new(width, DIAGRAM_H), Sense::click_and_drag());
    let removed = d.diff.removed_nodes(&p.name);
    let (ghosts, ghost_wires) = old.map_or_else(
        || (Vec::new(), Vec::new()),
        |o| diff::ghosts(o, &removed, p.diagram.size.1, GHOST_GAP),
    );
    let full_h = ghosts
        .iter()
        .map(|(_, r)| r.bottom())
        .fold(p.diagram.size.1, f32::max);
    if !cam.fitted {
        let (w, h) = (p.diagram.size.0.max(1.0) + 40.0, full_h.max(1.0) + 40.0);
        cam.zoom = (area.width() / w).min(area.height() / h).clamp(0.2, 1.2);
        cam.offset = Vec2::splat(20.0 * cam.zoom);
        cam.fitted = true;
    }
    cam.origin = area.min;
    if response.dragged() {
        cam.offset += response.drag_delta();
    }
    let zoom = ui.ctx().input(egui::InputState::zoom_delta);
    if response.hovered() && (zoom - 1.0).abs() > f32::EPSILON {
        cam.zoom = (cam.zoom * zoom).clamp(0.15, 3.0);
    }
    if response.double_clicked() {
        cam.fitted = false;
    }
    let painter = ui.painter_at(area);
    painter.rect_filled(area, CornerRadius::ZERO, t.bg);
    let marks = Marks {
        added: d.diff.new_nodes(&p.name, Change::Added),
        changed: d.diff.new_nodes(&p.name, Change::Changed),
        regions: d.diff.new_regions(&p.name),
    };
    draw_diff(&painter, &t, area, *cam, &p.diagram, &marks);
    if let Some(o) = old {
        draw_ghosts(&painter, *cam, o, &ghosts, &ghost_wires, p.diagram.size.1);
    }
}

/// What to highlight in the proposed diagram.
pub struct Marks {
    pub added: Vec<NodeId>,
    pub changed: Vec<NodeId>,
    pub regions: Vec<(helmsman_diagram::RegionId, Change)>,
}

fn font(z: f32, size: f32) -> FontId {
    FontId::monospace(size * z)
}

fn draw_diff(painter: &egui::Painter, t: &Theme, area: Rect, cam: Cam, d: &Diagram, m: &Marks) {
    let z = cam.zoom;
    let text = z > 0.35;
    for r in &d.regions {
        let rect = cam.rect(&r.rect);
        let hot = m.regions.iter().find(|(id, _)| *id == r.id).map(|(_, c)| *c);
        let (stroke, width) = match hot {
            Some(_) => (YELLOW, 2.0),
            None => (t.muted.gamma_multiply(0.6), 1.0),
        };
        painter.rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(width * z, stroke),
            StrokeKind::Inside,
        );
        if text {
            let at = rect.min + Vec2::new(8.0, LABEL_H / 2.0) * z;
            painter.text(at, Align2::LEFT_CENTER, &r.label, font(z, 12.0), stroke);
            if let Some(c) = hot {
                let tag = if c == Change::Added { "added" } else { "changed" };
                painter.text(
                    Pos2::new(rect.max.x - 8.0 * z, at.y),
                    Align2::RIGHT_CENTER,
                    tag,
                    font(z, 10.0),
                    t.accent,
                );
            }
        }
    }
    for w in &d.wires {
        let (a, b) = (d.node(w.from), d.node(w.to));
        let (ax, ay) = out_port(a, w.out);
        let (bx, by) = in_port(b, w.inp);
        let hot = m.added.contains(&w.to) || m.added.contains(&w.from);
        let color = if hot { YELLOW } else { t.muted.gamma_multiply(0.8) };
        painter.line_segment(
            [cam.to_screen(ax, ay), cam.to_screen(bx, by)],
            Stroke::new(1.4 * z, color),
        );
    }
    for n in &d.nodes {
        let r = cam.rect(&n.rect);
        if !area.intersects(r) {
            continue;
        }
        let added = m.added.contains(&n.id);
        let changed = m.changed.contains(&n.id);
        let (head, ink, body) = if added {
            (YELLOW, BLACK, t.panel)
        } else if changed {
            (t.rule, t.fg, t.panel)
        } else {
            (t.rule.gamma_multiply(0.7), t.muted, t.panel.gamma_multiply(0.8))
        };
        painter.rect_filled(r, CornerRadius::same(4), body);
        let h = Rect::from_min_size(r.min, Vec2::new(r.width(), HEADER * z));
        painter.rect_filled(h, CornerRadius::same(4), head);
        let outline = if added || changed {
            Stroke::new(2.5 * z, YELLOW)
        } else {
            Stroke::new(1.0 * z, t.edge)
        };
        painter.rect_stroke(r, CornerRadius::same(4), outline, StrokeKind::Outside);
        if text {
            painter.text(h.center(), Align2::CENTER_CENTER, &n.label, font(z, 11.5), ink);
            if changed {
                painter.text(
                    Pos2::new(r.max.x, r.min.y - 2.0 * z),
                    Align2::RIGHT_BOTTOM,
                    "changed",
                    font(z, 9.5),
                    t.accent,
                );
            } else if added {
                painter.text(
                    Pos2::new(r.max.x, r.min.y - 2.0 * z),
                    Align2::RIGHT_BOTTOM,
                    "new",
                    font(z, 9.5),
                    t.accent,
                );
            }
        }
    }
}

fn draw_ghosts(
    painter: &egui::Painter,
    cam: Cam,
    old: &Diagram,
    ghosts: &[(NodeId, helmsman_diagram::Rect)],
    wires: &[usize],
    below: f32,
) {
    if ghosts.is_empty() {
        return;
    }
    let z = cam.zoom;
    let y = below + GHOST_GAP / 2.0;
    painter.text(
        cam.to_screen(0.0, y),
        Align2::LEFT_CENTER,
        "removed",
        font(z, 12.0),
        GONE,
    );
    let at = |id: NodeId| ghosts.iter().find(|(g, _)| *g == id).map(|(_, r)| *r);
    let stroke = Stroke::new(1.2 * z, GONE);
    for &k in wires {
        let w = old.wires[k];
        let (Some(a), Some(b)) = (at(w.from), at(w.to)) else {
            continue;
        };
        let (ax, ay) = out_port_at(&a, old.node(w.from).kind.headed(), w.out);
        let (bx, by) = in_port_at(&b, old.node(w.to).kind.headed(), w.inp);
        painter.extend(Shape::dashed_line(
            &[cam.to_screen(ax, ay), cam.to_screen(bx, by)],
            stroke,
            5.0 * z,
            4.0 * z,
        ));
    }
    for (id, r) in ghosts {
        let s = cam.rect(r);
        let corners = [
            s.left_top(),
            s.right_top(),
            s.right_bottom(),
            s.left_bottom(),
            s.left_top(),
        ];
        painter.extend(Shape::dashed_line(&corners, stroke, 6.0 * z, 4.0 * z));
        if z > 0.35 {
            let n = old.node(*id);
            painter.text(
                Pos2::new(s.center().x, s.min.y + HEADER * z / 2.0),
                Align2::CENTER_CENTER,
                &n.label,
                font(z, 11.5),
                GONE,
            );
        }
    }
}

fn refusal_ui(ui: &mut egui::Ui, r: &Refusal, now: f64) {
    let t = theme(ui);
    egui::Frame::new()
        .fill(t.panel)
        .stroke(Stroke::new(
            1.0,
            if r.why == Why::Locked { YELLOW } else { t.muted },
        ))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            let code = if r.status == 0 {
                String::new()
            } else {
                format!("{} · ", r.status)
            };
            ui.label(
                RichText::new(format!("{code}the box said: {}", r.error))
                    .monospace()
                    .color(t.fg),
            );
            ui.label(&r.hint);
            if let Some(at) = r.retry_at {
                let left = (at - now).max(0.0).ceil();
                if left > 0.0 {
                    ui.label(
                        RichText::new(format!("PIN entry opens again in {left:.0} s"))
                            .strong()
                            .color(t.accent),
                    );
                } else {
                    ui.label(RichText::new("PIN entry is open again.").color(t.muted));
                }
            }
        });
}

fn replay_ui(ui: &mut egui::Ui, step: &Step, out: &mut Out) {
    let t = theme(ui);
    let running = matches!(step.replay, Replay::Running);
    let label = match step.replay {
        Replay::Done(_) => "Replay again",
        _ => "Run the sim replay",
    };
    ui.horizontal_wrapped(|ui| {
        let b = ui
            .add_enabled(!running, egui::Button::new(label))
            .tip(if running {
                "The replay is running."
            } else {
                "The box replays what runs and this change over its recorded inputs."
            });
        out.rects.push((format!("replay {:?}", step.target), b.rect));
        if b.clicked() {
            out.act.push(Act::Replay(step.target.clone()));
        }
        if running {
            ui.spinner();
            ui.label(RichText::new("replaying…").color(t.muted));
        }
    });
    match &step.replay {
        Replay::Done(c) => chart(ui, c),
        Replay::Failed(r) => refusal_ui(ui, r, 0.0),
        Replay::None | Replay::Running => {}
    }
}

/// The Running values heading's tooltip.
const VALUES: &str = "What the deploy does to the values the box holds now: timers, counters \
                      and every variable keep their value when the new program declares them \
                      with the same name and type. A new variable, or one whose type changed, \
                      starts at its initial value; one the new program no longer declares is \
                      gone. The audit log records what the deploy did.";

/// What the deploy keeps, resets and drops: the box's sentence, then each
/// reset and dropped value marked apart from the kept count. Wrapped to
/// what is visible (the diagram can make the scrolled area wider), with
/// room for the scroll bar.
fn values_ui(ui: &mut egui::Ui, v: &Values, out: &mut Out) {
    let seen = ui.clip_rect().right() - ui.cursor().left() - 12.0;
    let w = ui.available_width().min(seen).max(120.0);
    ui.scope(|ui| {
        ui.set_max_width(w);
        values_rows(ui, v, out);
    });
}

fn values_rows(ui: &mut egui::Ui, v: &Values, out: &mut Out) {
    let t = theme(ui);
    let tip = if v.preview {
        "A preview, worked out when the proposal was made against what runs now. A value that \
         no longer fits its type at the moment of the deploy also starts again; the audit log \
         records what actually happened."
    } else {
        "What the deploy did, as the box reported it when it swapped the program."
    };
    let line = ui
        .add(egui::Label::new(RichText::new(v.sentence()).color(t.fg)).wrap())
        .tip(tip);
    out.rects.push(("state".into(), line.rect));
    if !v.running || (v.reset.is_empty() && v.dropped.is_empty()) {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{} kept", v.kept)).color(t.muted))
            .tip("Values that carry over unchanged.");
        for (name, why) in v.reset.iter().take(SHOWN) {
            let (mark, tip) = if why == "type changed" {
                ("reset", "Starts again at its initial value: its type changed.")
            } else {
                ("new", "New: starts at its initial value.")
            };
            ui.label(
                RichText::new(format!("{mark} {name}"))
                    .monospace()
                    .background_color(YELLOW)
                    .color(BLACK),
            )
            .tip(tip);
        }
        more(ui, v.reset.iter().skip(SHOWN).map(|(n, _)| n.as_str()), "reset");
        for name in v.dropped.iter().take(SHOWN) {
            ui.label(
                RichText::new(format!("drop {name}"))
                    .monospace()
                    .strikethrough()
                    .color(GONE),
            )
            .tip("No longer declared: its value is gone after the deploy.");
        }
        more(ui, v.dropped.iter().skip(SHOWN).map(String::as_str), "dropped");
    });
}

/// "and 40 more reset", the names in its tooltip; nothing when none.
fn more<'a>(ui: &mut egui::Ui, rest: impl Iterator<Item = &'a str>, what: &str) {
    let rest: Vec<&str> = rest.collect();
    if rest.is_empty() {
        return;
    }
    let t = theme(ui);
    ui.label(RichText::new(format!("and {} more {what}", rest.len())).color(t.muted))
        .tip(rest.join(", "));
}

fn approve_ui(ui: &mut egui::Ui, o: &mut Open, now: f64, out: &mut Out) {
    let t = theme(ui);
    if o.tier().is_none() {
        ui.label(RichText::new("Not applicable on this box.").color(t.muted));
        return;
    }
    if o.json["status"] == "applied" && !matches!(o.step.phase, Phase::Applied(_)) {
        ui.label(RichText::new("Already applied.").color(t.muted));
        return;
    }
    step_ui(ui, &mut o.step, now, "Approve and apply", out);
}

fn step_ui(ui: &mut egui::Ui, step: &mut Step, now: f64, verb: &str, out: &mut Out) {
    let t = theme(ui);
    if let Phase::Applied(a) = &step.phase {
        egui::Frame::new()
            .fill(YELLOW)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                let by = a
                    .user
                    .as_ref()
                    .map_or_else(|| format!("{} PIN", a.role), |u| format!("by {u}, {}", a.role));
                ui.label(
                    RichText::new(format!(
                        "Done ({} change, {by}). Now running snapshot {}; before it: {}.",
                        a.tier,
                        short_id(&a.snapshot),
                        short_id(&a.previous)
                    ))
                    .strong()
                    .color(BLACK),
                );
            });
        return;
    }
    let locked = step.locked_for(now);
    let sending = matches!(step.phase, Phase::Sending);
    // The session approves as whoever signed in: no role to pick, no PIN.
    let mut go = false;
    ui.horizontal_wrapped(|ui| {
        match &out.signed_in {
            Some(who) => ui.label(RichText::new(format!("as {who}")).color(t.muted)),
            None => ui.label(RichText::new("not signed in").color(t.muted)),
        }
        .tip("Approvals are made as who is signed in.");
        let why = if out.signed_in.is_none() {
            format!("Sign in (left) to {}.", verb.to_ascii_lowercase())
        } else if let Some(left) = locked {
            format!("The box refuses for now: try again in {left} s.")
        } else if sending {
            "Waiting for the box's answer.".to_string()
        } else {
            format!("{verb}: the box checks its approval policy first.")
        };
        let button = ui
            .add_enabled(
                out.signed_in.is_some() && locked.is_none() && !sending,
                egui::Button::new(RichText::new(verb).strong()),
            )
            .tip(why);
        out.rects.push((format!("go {:?}", step.target), button.rect));
        go = button.clicked();
        if sending {
            ui.spinner();
        }
    });
    if let Replay::Done(c) = &step.replay {
        ui.label(
            RichText::new(format!("cites replay #{}", c.id))
                .small()
                .color(t.muted),
        );
    }
    if go {
        out.act.push(Act::Approve(step.target.clone()));
    }
    if let Phase::Refused(r) = &step.phase {
        refusal_ui(ui, r, now);
    }
}

/// The replay, one chart per signal: what runs (steel) against the
/// proposal (yellow), rises marked, the box's summary above each.
fn chart(ui: &mut egui::Ui, c: &Chart) {
    let t = theme(ui);
    let secs = (c.end_ms.saturating_sub(c.start_ms)) as f64 / 1000.0;
    ui.label(RichText::new(format!("replay #{} over {secs:.1} s of recorded inputs", c.id)).color(t.muted));
    for f in &c.faults {
        ui.colored_label(RED, format!("the proposal faulted: {f}"));
    }
    if c.signals.is_empty() {
        ui.label(RichText::new("No module drives a signal in either version.").color(t.muted));
    }
    // Differences first; the box's order otherwise.
    let mut order: Vec<&SignalChart> = c.signals.iter().collect();
    order.sort_by_key(|s| s.identical);
    for s in order {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&s.signal).monospace().strong());
            let rt = RichText::new(s.summary());
            ui.label(if s.identical {
                rt.color(t.muted)
            } else {
                rt.strong().color(t.fg)
            });
        });
        plot(ui, c, s);
    }
    ui.horizontal(|ui| {
        swatch(ui, t.muted, "what runs now");
        swatch(ui, YELLOW, "the proposal");
        ui.label(RichText::new("▲ a rise (turned on)").small().color(t.muted));
    });
}

fn swatch(ui: &mut egui::Ui, c: Color32, text: &str) {
    let t = theme(ui);
    let (r, _) = ui.allocate_exact_size(Vec2::new(18.0, 4.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::ZERO, c);
    ui.label(RichText::new(text).small().color(t.muted));
}

/// The value range a plot spans.
fn span(s: &SignalChart) -> (f64, f64) {
    let vals = [&s.current, &s.proposed]
        .into_iter()
        .flatten()
        .flat_map(|t| t.steps.iter().filter_map(|x| x.1));
    let (lo, hi) = vals.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
        (a.min(v), b.max(v))
    });
    if !lo.is_finite() {
        return (0.0, 1.0);
    }
    if (hi - lo).abs() < f64::EPSILON {
        (lo - 1.0, hi + 1.0)
    } else {
        (lo, hi)
    }
}

/// Screen points of a step function over the window.
#[must_use]
pub fn step_points(t: &Track, start: u64, end: u64, rect: Rect, (lo, hi): (f64, f64)) -> Vec<Vec<Pos2>> {
    let x = |ms: u64| {
        let f = (ms.clamp(start, end) - start) as f64 / (end.saturating_sub(start).max(1)) as f64;
        rect.left() + rect.width() * f as f32
    };
    let y = |v: f64| rect.bottom() - rect.height() * ((v - lo) / (hi - lo)) as f32;
    // Runs of known values; a held stretch (`None`) breaks the line.
    let mut runs = Vec::new();
    let mut run: Vec<Pos2> = Vec::new();
    for (k, (at, v)) in t.steps.iter().enumerate() {
        let next = t.steps.get(k + 1).map_or(end, |s| s.0);
        if let Some(v) = v {
            let yy = y(*v);
            if let Some(last) = run.last().copied() {
                run.push(Pos2::new(x(*at), last.y));
            }
            run.push(Pos2::new(x(*at), yy));
            run.push(Pos2::new(x(next), yy));
        } else {
            if run.len() > 1 {
                runs.push(std::mem::take(&mut run));
            }
            run.clear();
        }
    }
    if run.len() > 1 {
        runs.push(run);
    }
    runs
}

fn plot(ui: &mut egui::Ui, c: &Chart, s: &SignalChart) {
    let t = theme(ui);
    let boolean = [&s.current, &s.proposed]
        .into_iter()
        .flatten()
        .any(|t| t.ty == "bool");
    let h = if boolean { 64.0 } else { 90.0 };
    let width = ui.available_width().max(160.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(3), t.chart);
    let inner = rect.shrink2(Vec2::new(8.0, 10.0));
    let range = if boolean { (0.0, 1.0) } else { span(s) };
    p.line_segment(
        [inner.left_bottom(), inner.right_bottom()],
        Stroke::new(1.0, t.rule),
    );
    // The proposal a little lower when both are flat on the same value, so
    // neither hides the other.
    for (track, color, dy) in [(&s.current, t.muted, -1.5), (&s.proposed, YELLOW, 1.5)] {
        let Some(tr) = track else { continue };
        for run in step_points(tr, c.start_ms, c.end_ms, inner, range) {
            let pts: Vec<Pos2> = run.iter().map(|q| *q + Vec2::new(0.0, dy)).collect();
            p.add(Shape::line(pts, Stroke::new(2.0, color)));
        }
        if boolean {
            for at in tr.rises() {
                let x = inner.left()
                    + inner.width()
                        * ((at.clamp(c.start_ms, c.end_ms) - c.start_ms) as f32
                            / (c.end_ms.saturating_sub(c.start_ms).max(1)) as f32);
                let base = if color == YELLOW {
                    rect.bottom() - 1.0
                } else {
                    rect.top() + 9.0
                };
                let tip = base - 7.0;
                p.add(Shape::convex_polygon(
                    vec![
                        Pos2::new(x - 4.0, base),
                        Pos2::new(x + 4.0, base),
                        Pos2::new(x, tip),
                    ],
                    color,
                    Stroke::NONE,
                ));
            }
        }
        if tr.truncated {
            p.text(
                rect.right_top() + Vec2::new(-4.0, 2.0),
                Align2::RIGHT_TOP,
                "chart cut short; counts are exact",
                FontId::proportional(crate::theme::MIN_TEXT),
                t.muted,
            );
        }
    }
    if !boolean {
        p.text(
            inner.left_top(),
            Align2::LEFT_TOP,
            format!("{:.3}", range.1),
            FontId::monospace(crate::theme::MIN_TEXT),
            t.muted,
        );
        p.text(
            inner.left_bottom(),
            Align2::LEFT_BOTTOM,
            format!("{:.3}", range.0),
            FontId::monospace(crate::theme::MIN_TEXT),
            t.muted,
        );
    }
    let on = |ms: u64| ms as f64 / 1000.0;
    response.on_hover_text(format!(
        "on for {:.1} s now, {:.1} s with the proposal",
        on(s.on_ms.0),
        on(s.on_ms.1)
    ));
}
