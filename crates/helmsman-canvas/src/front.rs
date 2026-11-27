//! The front door (TODO.md `front-door`, interview 2026-11-14): what a
//! controls engineer who lands on the public page cold sees before any
//! editor. Three things on one screen: what was asked for in plain words,
//! a machine running the program written for it, and the proofs, each a
//! requirement test that throws a fault at the machine (a switch that
//! fails, a module that is lost, an impossible reading) and checks the
//! program holds safe. The visitor then throws the same faults by hand
//! and watches. The editor, the diagram and every tab are one button away
//! ("Open the editor"), on the same project.
//!
//! The machine is a tank that tops itself up (`assets/front/`): `main.st`,
//! its `plant.sim` and five `test_*.st`. It is also a built-in project
//! ([`PROJECT`]), so the editor opens on exactly what the front door ran.
//!
//! [`Front`] is the state, with no screen: its own [`Session`] running
//! the program against the plant, the proofs' results, and the fault the
//! visitor chose. [`ui`] draws it.

use helmsman_iec::Value;
use helmsman_sim::Mode;

use crate::playground::Session;
use crate::requirements::{Results, Row, State};

/// The built-in project's name.
pub const PROJECT: &str = "tank";
/// The program.
pub const MAIN: &str = include_str!("../assets/front/main.st");
/// The machine it runs against.
pub const PLANT: &str = include_str!("../assets/front/plant.sim");
/// The requirement tests, in the order the proofs are shown.
pub const TESTS: [(&str, &str); 5] = [
    (
        "test_fills_and_stops.st",
        include_str!("../assets/front/test_fills_and_stops.st"),
    ),
    (
        "test_high_switch_fails.st",
        include_str!("../assets/front/test_high_switch_fails.st"),
    ),
    (
        "test_switches_lost.st",
        include_str!("../assets/front/test_switches_lost.st"),
    ),
    (
        "test_both_switches_on.st",
        include_str!("../assets/front/test_both_switches_on.st"),
    ),
    (
        "test_alarm_needs_reset.st",
        include_str!("../assets/front/test_alarm_needs_reset.st"),
    ),
];

/// The I/O, named before anything was asked: what each point is.
pub const IO: [(&str, &str); 7] = [
    ("level_low", "input, on when the tank is nearly empty"),
    ("level_high", "input, on when the tank is full"),
    ("level", "input, the level in metres (the tank is 2 m deep)"),
    ("reset", "input, the operator's alarm reset"),
    ("fill", "output, opens the inlet valve"),
    ("alarm", "output"),
    (
        "level_pct",
        "output, the level in percent for the operator's screen",
    ),
];

/// What was asked for, as the visitor reads it: the instruction the
/// Agent was given, one line each ([`instruction`] is the whole of it).
pub const ASKED: [&str; 4] = [
    "Keep the tank topped up: open the inlet when it runs low, close it when it is full.",
    "Never overflow it, even if the high switch fails: a fill takes under 6 s, so one that runs 7 s closes the inlet and raises the alarm.",
    "If the low and high switches are both on, close the inlet and raise the alarm.",
    "An alarm stays, with the inlet closed, until reset is pressed and the switches no longer disagree.",
];

/// The instruction the recorded answer was given (`recorded.txt`): the
/// I/O by name, then [`ASKED`].
#[cfg(test)]
#[must_use]
pub fn instruction() -> String {
    use std::fmt::Write as _;
    let mut s = String::from("A tank that keeps itself topped up. Its I/O, with these names:\n");
    for (name, what) in IO {
        let _ = writeln!(s, "- {name}: {what}");
    }
    s.push_str("What it must do:\n");
    for line in ASKED {
        let _ = writeln!(s, "- {line}");
    }
    s
}

/// The tank's depth, m (`max=` of the plant).
pub const DEPTH: f64 = 2.0;
/// Where its switches make, m (`low_at=`, `high_at=`).
pub const LOW_AT: f64 = 0.2;
pub const HIGH_AT: f64 = 1.6;
/// When and how the program was written (`recorded.txt`, written by the
/// ignored test `front_door_recording`, which asks the live Agent).
pub const RECORDED: &str = include_str!("../assets/front/recorded.txt");
/// Seconds between one line of the request appearing and the next.
pub const ASK_EVERY: f64 = 0.5;
/// Seconds the page shows the program being written.
pub const WRITING: f64 = 2.0;
/// Seconds between one proof appearing and the next.
pub const REVEAL: f64 = 0.7;
/// Seconds the reset button stays pressed after a click.
const PRESS: f64 = 0.3;

/// A fault the visitor throws at the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fault {
    #[default]
    None,
    /// The high switch never makes.
    HighStuck,
    /// The module the level switches are wired to is gone.
    SwitchesLost,
    /// Both switches read on at once.
    BothOn,
}

impl Fault {
    pub const ALL: [Self; 4] = [Self::None, Self::HighStuck, Self::SwitchesLost, Self::BothOn];

    /// The button's words.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "Everything works",
            Self::HighStuck => "Fail the high switch",
            Self::SwitchesLost => "Lose the level switches",
            Self::BothOn => "Cross the switch wiring",
        }
    }

    /// What it does to the machine, for the button's tip.
    #[must_use]
    pub const fn tip(self) -> &'static str {
        match self {
            Self::None => "Every switch reads what the tank does.",
            Self::HighStuck => {
                "The high switch stays off however full the tank gets. Watch the inlet close anyway, before the brim."
            }
            Self::SwitchesLost => {
                "The module both switches are wired to drops off the network. The program is held and its outputs go to their safe state."
            }
            Self::BothOn => "Both switches read on at once, which no real level can do.",
        }
    }

    /// What the visitor should see happen, once it has.
    #[must_use]
    pub const fn outcome(self) -> &'static str {
        match self {
            Self::None => "The tank fills when it runs low and stops when it is full.",
            Self::HighStuck => {
                "The fill ran too long, so the inlet closed below the brim and the alarm came on."
            }
            Self::SwitchesLost => {
                "With its inputs gone the program is held: the inlet is closed until the switches are back."
            }
            Self::BothOn => {
                "The readings cannot both be true, so the inlet stays closed and the alarm is on."
            }
        }
    }
}

/// A value of [`RECORDED`] (`on`, `requests`, `seconds`), or `?`.
#[must_use]
pub fn recorded(key: &str) -> &'static str {
    RECORDED
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix(": "))
        .map_or("?", str::trim)
}

/// The front door's state.
pub struct Front {
    /// The program running against the plant.
    pub session: Session,
    /// Every proof's result, in [`TESTS`]' order.
    pub proofs: Vec<Row>,
    /// When the page first drew it (page time, s): the proofs appear one
    /// by one from then.
    since: Option<f64>,
    pub fault: Fault,
    /// The highest the level has been since the fault was chosen, m.
    pub peak: f64,
    /// Reset is pressed until (page time, s).
    pressed_until: Option<f64>,
}

impl Default for Front {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs `main` against the front door's plant and tests: one row a test,
/// in [`TESTS`]' order.
#[must_use]
pub fn prove(main: &str) -> Vec<Row> {
    let files = std::iter::once((helmsman_project::MAIN, main)).chain(TESTS);
    let plan = helmsman_iec::spec::plan(files);
    let settings = helmsman_iec::spec::Settings::new(crate::playground::SIZES);
    let rows = Results::from_verdicts(&helmsman_sim::iec::run_all(&plan, settings, PLANT)).rows;
    TESTS
        .iter()
        .filter_map(|(file, _)| rows.iter().find(|r| r.file == *file).cloned())
        .collect()
}

impl Front {
    /// The machine running, every proof run.
    #[must_use]
    pub fn new() -> Self {
        let mut session = Session::new(MAIN);
        session.set_plant(PLANT);
        session.set_running(true);
        Self {
            session,
            proofs: prove(MAIN),
            since: None,
            fault: Fault::None,
            peak: 0.0,
            pressed_until: None,
        }
    }

    /// One frame at page time `t` (s): the machine moves on, a pressed
    /// reset is let go.
    pub fn tick(&mut self, t: f64) {
        self.since.get_or_insert(t);
        // The scans up to now run before a pressed reset is let go, so a
        // press is seen by the program however long the frame took.
        self.session.advance(t);
        if self.pressed_until.is_some_and(|until| t >= until) {
            self.pressed_until = None;
            self.session.set_input("reset", Value::Bool(false));
        }
        self.peak = self.peak.max(self.level());
    }

    /// Seconds since the page first drew it.
    fn elapsed(&self, t: f64) -> f64 {
        self.since.map_or(0.0, |since| (t - since).max(0.0))
    }

    /// How many lines of the request are on the screen at page time `t`:
    /// they come one by one, as the first run is played back.
    #[must_use]
    pub fn asked_shown(&self, t: f64) -> usize {
        if self.since.is_none() {
            return 0;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = (self.elapsed(t) / ASK_EVERY) as usize + 1;
        n.min(ASKED.len())
    }

    /// While the page shows the program being written: how much of it is
    /// (0 to 1). `None` before the whole request is up and once it is
    /// written.
    #[must_use]
    pub fn writing(&self, t: f64) -> Option<f64> {
        #[allow(clippy::cast_precision_loss)]
        let from = ASKED.len() as f64 * ASK_EVERY;
        let e = self.elapsed(t);
        (self.since.is_some() && e >= from && e < from + WRITING).then(|| (e - from) / WRITING)
    }

    /// Whether the program is on the screen as written.
    #[must_use]
    pub fn written(&self, t: f64) -> bool {
        #[allow(clippy::cast_precision_loss)]
        let lead = ASKED.len() as f64 * ASK_EVERY + WRITING;
        self.since.is_some() && self.elapsed(t) >= lead
    }

    /// How many proofs are on the screen at page time `t`: none until
    /// the program is written, then one by one.
    #[must_use]
    pub fn shown(&self, t: f64) -> usize {
        if !self.written(t) {
            return 0;
        }
        #[allow(clippy::cast_precision_loss)]
        let lead = ASKED.len() as f64 * ASK_EVERY + WRITING;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = ((self.elapsed(t) - lead) / REVEAL) as usize;
        n.min(self.proofs.len())
    }

    /// Machine time the proofs covered, s.
    #[must_use]
    pub fn proved_s(&self) -> u64 {
        self.proofs.iter().map(|r| r.elapsed_ms).sum::<u64>() / 1000
    }

    /// Throws `fault` at the machine (or takes it away).
    pub fn set_fault(&mut self, fault: Fault) {
        self.fault = fault;
        self.peak = self.level();
        let Some(plant) = &mut self.session.plant else {
            return;
        };
        let (low, high) = match fault {
            Fault::None => (Mode::Normal, Mode::Normal),
            Fault::HighStuck => (Mode::Normal, Mode::Stuck(Some(0.0))),
            Fault::SwitchesLost => (Mode::Absent, Mode::Absent),
            Fault::BothOn => (Mode::Stuck(Some(1.0)), Mode::Stuck(Some(1.0))),
        };
        plant.set_mode("level_low", low);
        plant.set_mode("level_high", high);
    }

    /// The operator presses reset at page time `t`.
    pub fn press_reset(&mut self, t: f64) {
        self.session.set_input("reset", Value::Bool(true));
        self.pressed_until = Some(t + PRESS);
    }

    fn flag(&self, name: &str) -> bool {
        matches!(self.session.value(name), Some(Value::Bool(true)))
    }

    /// The tank's level, m.
    #[must_use]
    pub fn level(&self) -> f64 {
        match self.session.value("level") {
            Some(Value::Real(v)) => v,
            _ => 0.0,
        }
    }

    /// The inlet is open.
    #[must_use]
    pub fn filling(&self) -> bool {
        self.flag("fill")
    }

    #[must_use]
    pub fn alarm(&self) -> bool {
        self.flag("alarm")
    }

    /// What the low switch reads.
    #[must_use]
    pub fn low(&self) -> bool {
        self.flag("level_low")
    }

    /// What the high switch reads.
    #[must_use]
    pub fn high(&self) -> bool {
        self.flag("level_high")
    }

    /// The program is held, as a box holds one whose inputs are gone.
    #[must_use]
    pub fn held(&self) -> bool {
        self.session.held
    }
}

// ---- the screen ----

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
};

use crate::editor::Chrome;
use crate::st_view::brand::{BLACK, GRAPHITE, GREEN, RED, STEEL, WARN, WHITE};
use crate::tips::Tip;

/// What the visitor asked of the front door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// On to the editor, on this project.
    OpenEditor,
}

/// Body text and the smallest text, pt (card `ui-legibility`).
const BODY: f32 = 18.0;
const SMALL: f32 = 16.0;
/// Below this width the three parts stack.
pub const STACK_BELOW: f32 = 980.0;
/// The widest the page's content gets.
const MAX_WIDTH: f32 = 1320.0;
/// The machine's picture.
const PICTURE: Vec2 = Vec2::new(340.0, 290.0);
/// The height kept for the sentence under the picture: three lines of it.
const SAYING: f32 = 78.0;
/// Water.
const WATER: Color32 = Color32::from_rgb(0x2E, 0x8B, 0xD6);

/// Draws the front door and says what the visitor asked for.
pub fn ui(ui: &mut egui::Ui, chrome: &mut Chrome, f: &mut Front) -> Option<Act> {
    let t = crate::theme::theme(ui);
    let now = ui.input(|i| i.time);
    f.tick(now);
    // The machine moves and the proofs appear by themselves.
    ui.ctx().request_repaint();
    let mut act = None;
    let page = ui.max_rect();
    ui.painter().rect_filled(page, CornerRadius::ZERO, t.bg);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width().min(MAX_WIDTH);
            let side = ((ui.available_width() - width) / 2.0).max(0.0) + 20.0;
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(side as i8, 18))
                .show(ui, |ui| {
                    ui.set_max_width(width - 40.0);
                    if head(ui, chrome, &t) {
                        act = Some(Act::OpenEditor);
                    }
                    ui.add_space(18.0);
                    if ui.available_width() >= STACK_BELOW {
                        ui.columns(3, |c| {
                            card(&mut c[0], &t, |ui| asked(ui, chrome, &t, f, now));
                            card(&mut c[1], &t, |ui| machine(ui, chrome, &t, f, now));
                            card(&mut c[2], &t, |ui| proofs(ui, chrome, &t, f, now));
                        });
                    } else {
                        // Stacked, the machine comes first: it is what moves.
                        card(ui, &t, |ui| machine(ui, chrome, &t, f, now));
                        ui.add_space(14.0);
                        card(ui, &t, |ui| proofs(ui, chrome, &t, f, now));
                        ui.add_space(14.0);
                        card(ui, &t, |ui| asked(ui, chrome, &t, f, now));
                    }
                    ui.add_space(22.0);
                    if foot(ui, chrome, &t) {
                        act = Some(Act::OpenEditor);
                    }
                });
        });
    act
}

/// One of the page's three parts: a panel a step off the page, its
/// content laid out from the left (a column of `ui.columns` justifies,
/// which spreads a wrapped line's words).
fn card(ui: &mut egui::Ui, t: &crate::theme::Theme, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(t.panel)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(ui.available_width());
                add(ui);
            });
        });
}

/// The wordmark, the way on, and what this page is. `true` when "Open
/// the editor" was clicked.
fn head(ui: &mut egui::Ui, chrome: &mut Chrome, t: &crate::theme::Theme) -> bool {
    let mut open = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("HELMSMAN").color(t.accent).size(22.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui
                .add(egui::Button::new(RichText::new("Open the editor").size(BODY)))
                .tip("The program, its live diagram, its files and the Describe tab, on this same tank.");
            chrome.hit("front open editor", r.rect);
            open = r.clicked();
        });
    });
    ui.add_space(14.0);
    let r = ui.add(
        egui::Label::new(
            RichText::new("A control program that proves itself before it runs the machine")
                .color(t.fg)
                .size(30.0)
                .strong(),
        )
        .wrap(),
    );
    chrome.hit("front headline", r.rect);
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            RichText::new(
                "Helmsman wrote this tank's program from the request below. Each proof is a test that breaks something on the machine and checks the program keeps it safe. Break it yourself with the buttons under the tank.",
            )
            .color(t.muted)
            .size(BODY),
        )
        .wrap(),
    );
    open
}

/// A part's heading.
fn heading(ui: &mut egui::Ui, t: &crate::theme::Theme, text: &str) {
    ui.label(RichText::new(text).color(t.accent).size(20.0).strong());
    ui.add_space(8.0);
}

/// What the Agent was asked, in the engineer's words: the I/O by name,
/// then the request, its lines coming one by one on the first run; then
/// the program being written, and once it is, who wrote it and when, with
/// the program behind a fold.
fn asked(ui: &mut egui::Ui, chrome: &mut Chrome, t: &crate::theme::Theme, f: &Front, now: f64) {
    heading(ui, t, "What Helmsman was asked");
    let names: Vec<&str> = IO.iter().map(|(n, _)| *n).collect();
    let r = ui
        .add(
            egui::Label::new(
                RichText::new(format!("The I/O, named first: {}.", names.join(", ")))
                    .color(t.muted)
                    .size(SMALL),
            )
            .wrap(),
        )
        .tip(
            IO.iter()
                .map(|(n, what)| format!("{n}: {what}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    chrome.hit("front io", r.rect);
    ui.add_space(8.0);
    for (k, line) in ASKED.iter().enumerate().take(f.asked_shown(now)) {
        ui.horizontal_top(|ui| {
            ui.label(RichText::new(format!("{}.", k + 1)).color(t.muted).size(BODY));
            let r = ui.add(egui::Label::new(RichText::new(*line).color(t.fg).size(BODY)).wrap());
            chrome.hit(format!("front asked {}", k + 1), r.rect);
        });
        ui.add_space(6.0);
    }
    ui.add_space(8.0);
    let lines = MAIN.lines().count();
    if let Some(part) = f.writing(now) {
        ui.horizontal(|ui| {
            ui.spinner();
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let so_far = (part * lines as f64) as usize;
            ui.label(
                RichText::new(format!("Writing the program: line {so_far} of {lines}"))
                    .color(t.muted)
                    .size(BODY),
            );
        });
        return;
    }
    if !f.written(now) {
        return;
    }
    let r = ui.add(
        egui::Label::new(
            RichText::new(format!(
                "Helmsman wrote the program from this on {}, in {} requests and {} seconds. It is shown as written.",
                recorded("on"),
                recorded("requests"),
                recorded("seconds")
            ))
            .color(t.muted)
            .size(SMALL),
        )
        .wrap(),
    );
    chrome.hit("front recorded", r.rect);
    ui.add_space(4.0);
    let fold = egui::CollapsingHeader::new(RichText::new("The program").color(t.fg).size(BODY))
        .icon(crate::theme::fold_icon)
        .id_salt("front program")
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!("{lines} lines of Structured Text (IEC 61131-3)."))
                    .color(t.muted)
                    .size(SMALL),
            );
            egui::ScrollArea::both()
                .id_salt("front program text")
                .max_height(320.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(MAIN).color(t.code).font(FontId::monospace(SMALL)))
                            .extend(),
                    );
                });
        });
    chrome.hit("front program", fold.header_response.rect);
}

/// The tank, what is happening to it, and the faults to throw.
fn machine(ui: &mut egui::Ui, chrome: &mut Chrome, t: &crate::theme::Theme, f: &mut Front, now: f64) {
    heading(ui, t, "The machine");
    let size = Vec2::new(PICTURE.x.min(ui.available_width()), PICTURE.y);
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    chrome.hit("front machine", r);
    paint_tank(ui.painter(), r, t, f, now);
    ui.add_space(8.0);
    // Room for its longest sentence, so the buttons under it stay put
    // when it changes.
    let (says, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), SAYING), Sense::hover());
    let galley = ui
        .painter()
        .layout(saying(f), FontId::proportional(BODY), t.fg, says.width());
    let said = Rect::from_min_size(says.min, galley.size());
    ui.painter().galley(says.min, galley, t.fg);
    chrome.hit("front saying", said);
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Break it:").color(t.muted).size(SMALL));
        for fault in Fault::ALL {
            let r = ui
                .add(egui::Button::new(RichText::new(fault.label()).size(BODY)).selected(f.fault == fault))
                .tip(fault.tip());
            chrome.hit(format!("front fault {fault:?}"), r.rect);
            if r.clicked() {
                f.set_fault(fault);
            }
        }
        let r = ui
            .add_enabled(
                f.alarm(),
                egui::Button::new(RichText::new("Press reset").size(BODY)),
            )
            .tip("The operator's reset button. It clears the alarm only once the fault is gone.");
        chrome.hit("front reset", r.rect);
        if r.clicked() {
            f.press_reset(now);
        }
    });
}

/// One sentence on what the machine is doing now.
#[must_use]
pub fn saying(f: &Front) -> String {
    let happened = match f.fault {
        Fault::None => false,
        Fault::HighStuck | Fault::BothOn => f.alarm(),
        Fault::SwitchesLost => f.held(),
    };
    if happened {
        return match f.fault {
            Fault::HighStuck => format!(
                "{} Peak {} m of {DEPTH:.0} m.",
                f.fault.outcome(),
                helmsman_iec::shown_real(f.peak)
            ),
            _ => f.fault.outcome().to_string(),
        };
    }
    if f.alarm() {
        "The alarm is latched and the inlet closed. Press reset.".into()
    } else if f.fault == Fault::HighStuck {
        if f.filling() {
            "The high switch is dead. The tank is filling past it.".into()
        } else {
            "The high switch is dead. Wait for the tank to run low and fill.".into()
        }
    } else if f.filling() {
        "Running low: the inlet is open.".into()
    } else {
        "Full enough: the inlet is closed while the process draws it down.".into()
    }
}

/// The tank: water to its level, the brim, both switches with what they
/// read, the inlet with its valve, the alarm lamp, and a band across it
/// while the program is held.
fn paint_tank(p: &egui::Painter, area: Rect, t: &crate::theme::Theme, f: &Front, now: f64) {
    let text = |at: Pos2, anchor: egui::Align2, s: &str, colour: Color32| {
        p.text(at, anchor, s, FontId::proportional(SMALL), colour);
    };
    let tank = Rect::from_min_max(
        Pos2::new(area.left() + 70.0, area.top() + 64.0),
        Pos2::new(area.right() - 96.0, area.bottom() - 34.0),
    );
    #[allow(clippy::cast_possible_truncation)]
    let y_of = |m: f64| tank.bottom() - (m / DEPTH).clamp(0.0, 1.0) as f32 * tank.height();
    // Water, then the walls over it (open at the top).
    let level = f.level();
    let water = Rect::from_min_max(Pos2::new(tank.left(), y_of(level)), tank.max);
    p.rect_filled(water, CornerRadius::ZERO, WATER);
    let wall = Stroke::new(3.0, t.fg);
    p.line_segment([tank.left_top(), tank.left_bottom()], wall);
    p.line_segment([tank.left_bottom(), tank.right_bottom()], wall);
    p.line_segment([tank.right_bottom(), tank.right_top()], wall);
    text(
        Pos2::new(tank.center().x, tank.top() - 4.0),
        egui::Align2::CENTER_BOTTOM,
        "brim",
        t.muted,
    );
    // The inlet: a pipe from the left over the wall, its valve, the stream.
    let pipe_y = tank.top() - 34.0;
    let spout = Pos2::new(tank.left() + 34.0, pipe_y);
    let pipe = Stroke::new(6.0, STEEL);
    p.line_segment([Pos2::new(area.left() + 4.0, pipe_y), spout], pipe);
    p.line_segment([spout, Pos2::new(spout.x, pipe_y + 14.0)], pipe);
    let valve = Pos2::new(area.left() + 46.0, pipe_y);
    let open = f.filling();
    p.circle(
        valve,
        13.0,
        if open { GREEN } else { GRAPHITE },
        Stroke::new(2.0, BLACK),
    );
    text(
        Pos2::new(valve.x, valve.y - 16.0),
        egui::Align2::CENTER_BOTTOM,
        if open { "inlet open" } else { "inlet closed" },
        if open { t.ink(GREEN) } else { t.muted },
    );
    if open {
        // The stream, with a ripple so it reads as moving.
        #[allow(clippy::cast_possible_truncation)]
        let wobble = ((now * 9.0).sin() * 1.5) as f32;
        p.line_segment(
            [
                Pos2::new(spout.x, pipe_y + 14.0),
                Pos2::new(spout.x + wobble, water.top().max(pipe_y + 14.0)),
            ],
            Stroke::new(5.0, WATER),
        );
    }
    // The switches, on the right wall at the level each makes.
    let lost = f.fault == Fault::SwitchesLost;
    for (name, at, on) in [("high", HIGH_AT, f.high()), ("low", LOW_AT, f.low())] {
        let c = Pos2::new(tank.right() + 16.0, y_of(at));
        p.line_segment(
            [
                Pos2::new(tank.right() - 10.0, c.y),
                Pos2::new(tank.right() + 6.0, c.y),
            ],
            Stroke::new(2.0, t.fg),
        );
        let (fill, word) = if lost {
            (GRAPHITE, "lost")
        } else if on {
            (GREEN, "on")
        } else {
            (GRAPHITE, "off")
        };
        p.circle(c, 8.0, fill, Stroke::new(1.5, BLACK));
        if lost {
            let d = Vec2::splat(5.0);
            let cross = Stroke::new(2.0, RED);
            p.line_segment([c - d, c + d], cross);
            p.line_segment([c + Vec2::new(-5.0, 5.0), c + Vec2::new(5.0, -5.0)], cross);
        }
        text(
            Pos2::new(c.x + 13.0, c.y),
            egui::Align2::LEFT_CENTER,
            &format!("{name}: {word}"),
            if on && !lost { t.ink(GREEN) } else { t.muted },
        );
    }
    // The level, in the water or just above it.
    text(
        Pos2::new(tank.center().x, (water.top() + 6.0).min(tank.bottom() - 22.0)),
        egui::Align2::CENTER_TOP,
        &format!("{} m", helmsman_iec::shown_real(level)),
        WHITE,
    );
    text(
        Pos2::new(tank.center().x, tank.bottom() + 6.0),
        egui::Align2::CENTER_TOP,
        "the process draws it down",
        t.muted,
    );
    // The alarm lamp, top right: lit and pulsing while on.
    let lamp = Pos2::new(area.right() - 30.0, area.top() + 26.0);
    if f.alarm() {
        #[allow(clippy::cast_possible_truncation)]
        let pulse = 0.25 + 0.2 * ((now * 5.0).sin() as f32).abs();
        p.circle_filled(lamp, 20.0, RED.gamma_multiply(pulse));
        p.circle_filled(lamp, 13.0, RED);
    } else {
        p.circle(lamp, 13.0, RED.linear_multiply(0.18), Stroke::new(1.0, BLACK));
    }
    text(
        Pos2::new(lamp.x, lamp.y + 18.0),
        egui::Align2::CENTER_TOP,
        "alarm",
        if f.alarm() { t.ink(RED) } else { t.muted },
    );
    if f.held() {
        let band = Rect::from_center_size(tank.center(), Vec2::new(tank.width() + 24.0, 34.0));
        p.rect(
            band,
            CornerRadius::same(6),
            BLACK,
            Stroke::new(1.5, WARN),
            StrokeKind::Outside,
        );
        p.text(
            band.center(),
            egui::Align2::CENTER_CENTER,
            "program held, outputs safe",
            FontId::proportional(SMALL),
            WARN,
        );
    }
}

/// The proofs, appearing one by one, then what they add up to.
fn proofs(ui: &mut egui::Ui, chrome: &mut Chrome, t: &crate::theme::Theme, f: &Front, now: f64) {
    heading(ui, t, "The proofs");
    if !f.written(now) {
        ui.add(
            egui::Label::new(
                RichText::new("The proofs run once the program is written.")
                    .color(t.muted)
                    .size(BODY),
            )
            .wrap(),
        );
        return;
    }
    let shown = f.shown(now);
    for (k, row) in f.proofs.iter().take(shown).enumerate() {
        let passed = row.state == State::Passed;
        ui.horizontal_top(|ui| {
            let (mark, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
            paint_mark(ui.painter(), mark, passed);
            let room = ui.available_width();
            ui.vertical(|ui| {
                ui.set_max_width(room);
                let r =
                    ui.add(egui::Label::new(RichText::new(&row.requirement).color(t.fg).size(BODY)).wrap());
                chrome.hit(format!("front proof {}", k + 1), r.rect);
                let under = if passed {
                    format!("Proved over {} of machine time.", seconds(row.elapsed_ms))
                } else {
                    row.why.clone()
                };
                ui.add(
                    egui::Label::new(
                        RichText::new(under)
                            .color(if passed { t.muted } else { t.ink(RED) })
                            .size(SMALL),
                    )
                    .wrap(),
                );
            });
        });
        ui.add_space(8.0);
    }
    if shown < f.proofs.len() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new(format!("Proving {} of {}", shown + 1, f.proofs.len()))
                    .color(t.muted)
                    .size(BODY),
            );
        });
        return;
    }
    ui.add_space(4.0);
    let r = ui.add(egui::Label::new(RichText::new(summary(f)).color(t.fg).size(BODY).strong()).wrap());
    chrome.hit("front summary", r.rect);
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(
            RichText::new("The same tests run on the controller before every deploy. A red one refuses it.")
                .color(t.muted)
                .size(SMALL),
        )
        .wrap(),
    );
}

/// `12 s` or `0.5 s`.
fn seconds(ms: u64) -> String {
    if ms >= 10_000 {
        format!("{} s", ms / 1000)
    } else {
        #[allow(clippy::cast_precision_loss)]
        let s = ms as f64 / 1000.0;
        format!("{} s", helmsman_iec::shown_real(s))
    }
}

/// What the proofs add up to.
#[must_use]
pub fn summary(f: &Front) -> String {
    let n = f.proofs.len();
    let green = f.proofs.iter().filter(|r| r.state == State::Passed).count();
    if green == n {
        format!(
            "All {n} proved: {} s of machine time, checked in this page in a moment.",
            f.proved_s()
        )
    } else {
        format!("{green} of {n} proved. The program would be refused.")
    }
}

/// A green disc with a tick, or a red one with a cross.
fn paint_mark(p: &egui::Painter, r: Rect, passed: bool) {
    let c = r.center();
    p.circle_filled(c, 11.0, if passed { GREEN } else { RED });
    let ink = Stroke::new(2.5, BLACK);
    if passed {
        p.line_segment([c + Vec2::new(-5.0, 0.5), c + Vec2::new(-1.5, 4.0)], ink);
        p.line_segment([c + Vec2::new(-1.5, 4.0), c + Vec2::new(5.5, -4.0)], ink);
    } else {
        p.line_segment([c + Vec2::new(-4.5, -4.5), c + Vec2::new(4.5, 4.5)], ink);
        p.line_segment([c + Vec2::new(-4.5, 4.5), c + Vec2::new(4.5, -4.5)], ink);
    }
}

/// The way on, again, for someone who read to the bottom. `true` when
/// clicked.
fn foot(ui: &mut egui::Ui, chrome: &mut Chrome, t: &crate::theme::Theme) -> bool {
    ui.separator();
    ui.add_space(10.0);
    let mut open = false;
    ui.horizontal_wrapped(|ui| {
        let r = ui
            .add(egui::Button::new(
                RichText::new("Change this program yourself").size(BODY).strong(),
            ))
            .tip("Opens the editor on this tank. Say what to change in plain words; every change is proved the same way before it lands.");
        chrome.hit("front change", r.rect);
        open = r.clicked();
        ui.label(
            RichText::new(
                "Structured Text (IEC 61131-3), EtherNet/IP and Modbus TCP, one controller.",
            )
            .color(t.muted)
            .size(SMALL),
        );
    });
    open
}
