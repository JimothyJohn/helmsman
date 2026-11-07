//! The guided tour of the simulation (Nick, 2026-11-06, interview):
//! coach marks on the real interface. Each step is a card next to the
//! control it is about, that control outlined, and the step moves on when
//! the visitor does what it asks, read from the app's state rather than
//! from a Next button. Back and Skip tour are always on the card.
//!
//! `/playground/?tour` starts it, as does Help › Take the tour. The steps
//! are the core loop (copy the Blink example under a name, change its
//! time, deploy, watch the output, read the trend), then the Agent tab,
//! then a closing card.
//!
//! The logic is [`Tour`]: a step and what the app showed when the step
//! began ([`Seen`]), advanced by [`Tour::observe`] and the card's buttons
//! ([`Tour::press`]). It knows nothing of egui; [`ui`] draws the card.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Stroke, Vec2};

use crate::editor::Chrome;
use crate::tips::Tip;

/// One step of the tour, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// What the tour is; Start begins it.
    Welcome,
    /// Click Blink in Files: the name popup opens.
    OpenBlink,
    /// Name the copy: a project of the visitor's own opens.
    Name,
    /// Change the program (the blink time); it compiles.
    Edit,
    /// Deploy: the edit is what runs.
    Deploy,
    /// Pick the lamp output (or any block): its block is picked.
    Watch,
    /// Change the trend's window or its signals.
    Trend,
    /// Paste a key and send an instruction, or skip.
    Agent,
    /// The closing card.
    Done,
}

impl Step {
    /// Every step, in order.
    pub const ALL: [Self; 9] = [
        Self::Welcome,
        Self::OpenBlink,
        Self::Name,
        Self::Edit,
        Self::Deploy,
        Self::Watch,
        Self::Trend,
        Self::Agent,
        Self::Done,
    ];

    /// The steps that ask for something (all but the first and last).
    pub const ACTIONS: usize = Self::ALL.len() - 2;

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// Where Back goes. Back from editing goes to the example: the name
    /// popup is only there while it is open.
    #[must_use]
    pub const fn back(self) -> Self {
        match self {
            Self::Welcome | Self::OpenBlink => Self::Welcome,
            Self::Name | Self::Edit => Self::OpenBlink,
            Self::Deploy => Self::Edit,
            Self::Watch => Self::Deploy,
            Self::Trend => Self::Watch,
            Self::Agent => Self::Trend,
            Self::Done => Self::Agent,
        }
    }

    /// The step's number among the ones that ask for something (1 for
    /// [`Self::OpenBlink`]), none for the first and last cards.
    #[must_use]
    pub fn number(self) -> Option<usize> {
        let k = self.index();
        (1..=Self::ACTIONS).contains(&k).then_some(k)
    }
}

/// What the tour reads from the app each frame: just enough to tell
/// whether a step's action was done.
// Independent observations of the app, not a state machine in disguise.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// The simulation's Editor is on screen (not the Monitor, not a box).
    pub editor: bool,
    /// The name popup is open for a copy of the Blink example.
    pub naming_blink: bool,
    /// The open project.
    pub open: String,
    /// It is a built-in example (read-only).
    pub example: bool,
    /// The open project's files as edited, hashed.
    pub source: u64,
    /// The edit is checked and compiles.
    pub compiles: bool,
    /// The program runs, and it is the open project as the editor shows it.
    pub live: bool,
    /// The block picked in the diagram, or the output tapped.
    pub picked: Option<String>,
    /// The trend's window and signals, hashed.
    pub trend: u64,
    /// Requests sent to the Agent so far.
    pub asks: u64,
    /// The Agent has a way to answer (a key typed, or the local proxy).
    pub keyed: bool,
    /// The trend panel is open on the Trend tab.
    pub trend_open: bool,
}

/// What a press on the card asks of the app besides moving the tour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Shut the name popup (Back from naming).
    CloseNaming,
    /// Show the Requirements tab.
    Requirements,
    /// Open the help pages.
    Help,
}

/// The card's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Start,
    Back,
    /// Skip the whole tour.
    Skip,
    /// Skip the Agent step.
    SkipStep,
    /// Show the Requirements tab (the last card).
    Requirements,
    /// Open the help pages (the last card).
    Help,
    Finish,
}

/// The tour's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tour {
    pub step: Step,
    /// What the app showed when the step began: a step moves on when
    /// the app changes from it, not when it already held.
    base: Option<Seen>,
    /// The tour is over (finished or skipped): the app drops it.
    pub ended: bool,
    /// The control the card points at, last frame.
    pub anchor: Option<String>,
}

impl Default for Tour {
    fn default() -> Self {
        Self::new()
    }
}

impl Tour {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            step: Step::Welcome,
            base: None,
            ended: false,
            anchor: None,
        }
    }

    fn enter(&mut self, step: Step, seen: &Seen) {
        self.step = step;
        self.base = Some(seen.clone());
    }

    /// One frame's look at the app: moves on (one step at most) when the
    /// step's action was done since it began.
    pub fn observe(&mut self, seen: &Seen) {
        let Some(base) = self.base.clone() else {
            self.base = Some(seen.clone());
            return;
        };
        if self.ended || !seen.editor {
            return;
        }
        let next = match self.step {
            Step::Welcome | Step::Done => None,
            Step::OpenBlink => seen.naming_blink.then_some(Step::Name),
            Step::Name => {
                if seen.naming_blink {
                    None
                } else if !seen.example && seen.open != base.open {
                    Some(Step::Edit)
                } else {
                    // Cancelled: click Blink again.
                    Some(Step::OpenBlink)
                }
            }
            Step::Edit => {
                (!seen.example && seen.open == base.open && seen.source != base.source && seen.compiles)
                    .then_some(if seen.live { Step::Watch } else { Step::Deploy })
            }
            Step::Deploy => (seen.live && !base.live).then_some(Step::Watch),
            Step::Watch => (seen.picked.is_some() && seen.picked != base.picked).then_some(Step::Trend),
            Step::Trend => (seen.trend != base.trend).then_some(Step::Agent),
            Step::Agent => (seen.asks > base.asks).then_some(Step::Done),
        };
        if let Some(step) = next {
            self.enter(step, seen);
        }
    }

    /// A button on the card, pressed with the app as `seen`.
    pub fn press(&mut self, b: Button, seen: &Seen) -> Option<Effect> {
        match b {
            Button::Start => {
                if self.step == Step::Welcome {
                    self.enter(Step::OpenBlink, seen);
                }
                None
            }
            Button::Back => {
                let from = self.step;
                self.enter(from.back(), seen);
                (from == Step::Name).then_some(Effect::CloseNaming)
            }
            Button::Skip | Button::Finish => {
                self.ended = true;
                None
            }
            Button::SkipStep => {
                if self.step == Step::Agent {
                    self.enter(Step::Done, seen);
                }
                None
            }
            Button::Requirements => Some(Effect::Requirements),
            Button::Help => Some(Effect::Help),
        }
    }

    /// The controls the card may point at, best first: the first one on
    /// screen this frame is it (a closed pane's tab, else the pane's
    /// strip, ...). Empty: the card sits in the middle.
    #[must_use]
    pub fn anchors(&self, seen: &Seen) -> Vec<String> {
        let keys: &[&str] = if !seen.editor && self.step != Step::Welcome && self.step != Step::Done {
            &["editor"]
        } else {
            match self.step {
                Step::Welcome | Step::Done => &[],
                Step::OpenBlink => &["project blink", "pane files", "left"],
                Step::Name => &["name"],
                Step::Edit => &["st editor", "pane code", "left"],
                Step::Deploy => &["file deploy", "bar file"],
                Step::Watch => &["io outputs", "tab io", "right"],
                Step::Trend => &["trend window", "bottom trend"],
                Step::Agent if seen.keyed => &["agent instruction", "tab claude", "right"],
                Step::Agent => &["agent key", "tab claude", "right"],
            }
        };
        keys.iter().map(|k| (*k).to_string()).collect()
    }
}

/// The words on a step's card: its title and its text. `deploy` is the
/// Deploy shortcut as this keyboard writes it.
#[must_use]
pub fn words(step: Step, seen: &Seen, deploy: &str) -> (&'static str, String) {
    if !seen.editor && step != Step::Welcome && step != Step::Done {
        return (
            "Back to the Editor",
            "The tour runs on the Editor of the simulation. Click Editor at the top right to carry on."
                .into(),
        );
    }
    match step {
        Step::Welcome => (
            "A short tour",
            format!(
                "{} steps through the playground: copy an example, change it, deploy it, watch it run, \
                 then hand a change to the agent. Each step moves on when you do what it asks. \
                 The program runs in this page and nothing reaches a controller.",
                spelled(Step::ACTIONS)
            ),
        ),
        Step::OpenBlink => (
            "Open the Blink example",
            "Blink flips a lamp output every 500 ms with a timer that restarts itself. The examples \
             are read-only, so click Blink in Files to make a copy of your own."
                .into(),
        ),
        Step::Name => (
            "Name your copy",
            "Type a name for your program and press Enter. It is kept in this browser under that \
             name, with its own history."
                .into(),
        ),
        Step::Edit => (
            "Change the blink time",
            "In the Code tab, change T#500ms to T#200ms. The editor checks the program as you type \
             and marks an error on its line."
                .into(),
        ),
        Step::Deploy => (
            "Deploy it",
            format!(
                "The edit compiles but is not running yet, as on a real controller. Deploy runs the \
                 requirement tests and starts it: File › Deploy, or {deploy}."
            ),
        ),
        Step::Watch => (
            "Watch the output",
            "The lamp now flips at the rate you set, live under Outputs on the I/O tab. Click lamp \
             there, or a block in the diagram, and the diagram marks the logic that drives it. A \
             program with inputs gets a switch for each one on the same tab."
                .into(),
        ),
        Step::Trend if !seen.trend_open => (
            "Read the trend",
            "The trend runs along the bottom of the Editor. Open it with View › Trend (Alt+2), then \
             drag its window slider."
                .into(),
        ),
        Step::Trend => (
            "Read the trend",
            "The trend draws lamp over time, one step per flip. Drag the window slider to zoom in \
             time, or click a signal's name to pin it or take it off."
                .into(),
        ),
        Step::Agent => (
            "Ask the agent",
            "The Agent tab changes the program from an instruction in plain words. Its answer is \
             compiled and tested before it is written into the file. In the playground it runs on \
             your own API key, held in this page. Paste a key and ask for a change, for example \
             \"keep the lamp on for 1 s and off for 2 s\", or skip this step."
                .into(),
        ),
        Step::Done => (
            "That is the whole loop",
            "You made an example your own, changed it, deployed it and watched it run. The help \
             pages go further: requirements that become tests and gate Deploy, remote I/O in \
             Devices, and the keyboard shortcuts."
                .into(),
        ),
    }
}

fn spelled(n: usize) -> String {
    match n {
        5 => "Five".into(),
        6 => "Six".into(),
        7 => "Seven".into(),
        8 => "Eight".into(),
        n => n.to_string(),
    }
}

/// The card's width, points.
pub const CARD_W: f32 = 380.0;
/// The gap between the card and the control it points at, points.
const GAP: f32 = 28.0;

/// Where a card of `size` goes beside `anchor` on `screen`: to its right,
/// else its left, below, above (the first that fits without covering it),
/// else in the screen's lower middle. No anchor: the middle.
#[must_use]
pub fn place(anchor: Option<Rect>, size: Vec2, screen: Rect) -> Rect {
    let inner = screen.shrink(8.0);
    let Some(a) = anchor else {
        return Rect::from_center_size(inner.center(), size);
    };
    let clamp_y = |y: f32| y.clamp(inner.top(), (inner.bottom() - size.y).max(inner.top()));
    let clamp_x = |x: f32| x.clamp(inner.left(), (inner.right() - size.x).max(inner.left()));
    let tries = [
        Pos2::new(a.right() + GAP, clamp_y(a.center().y - size.y / 2.0)),
        Pos2::new(a.left() - GAP - size.x, clamp_y(a.center().y - size.y / 2.0)),
        Pos2::new(clamp_x(a.center().x - size.x / 2.0), a.bottom() + GAP),
        Pos2::new(clamp_x(a.center().x - size.x / 2.0), a.top() - GAP - size.y),
    ];
    tries
        .into_iter()
        .map(|p| Rect::from_min_size(p, size))
        .find(|r| inner.contains_rect(*r) && !r.intersects(a))
        .unwrap_or_else(|| {
            Rect::from_min_size(
                Pos2::new(
                    clamp_x(inner.center().x - size.x / 2.0),
                    clamp_y(inner.bottom() - size.y),
                ),
                size,
            )
        })
}

/// The card's colours: a yellow edge on the panel colour.
const MARK: Color32 = crate::st_view::brand::YELLOW;

/// Draws the tour over everything: the outline on the control the step
/// is about, an arrow to it and the card. `seen` is this frame's look at
/// the app, `deploy` the Deploy shortcut. Returns what a press asks of
/// the app.
pub fn ui(
    ctx: &egui::Context,
    tour: &mut Tour,
    seen: &Seen,
    chrome: &mut Chrome,
    deploy: &str,
) -> Option<Effect> {
    let anchors = tour.anchors(seen);
    let found = anchors.iter().find_map(|k| {
        chrome
            .hits
            .iter()
            .rev()
            .find(|(h, _)| h == k)
            .map(|(h, r)| (h.clone(), *r))
    });
    tour.anchor = found.as_ref().map(|(k, _)| k.clone());
    let anchor = found.map(|(_, r)| r);
    // Under the header: the card never hides the menus or the project's
    // name, even when it points at the header.
    let whole = ctx.content_rect();
    let screen = Rect::from_min_max(
        egui::pos2(
            whole.left(),
            (whole.top() + crate::app::HEADER_H + 4.0).min(whole.bottom()),
        ),
        whole.max,
    );
    let id = egui::Id::unique("tour-card");
    let size = Vec2::new(CARD_W, ctx.data(|d| d.get_temp::<f32>(id)).unwrap_or(260.0));
    let card = place(anchor, size, screen);
    let now = ctx.input(|i| i.time);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::unique("tour-mark"),
    ));
    if let Some(a) = anchor {
        // A slow pulse, so the eye finds it.
        let pulse = 0.65 + 0.35 * (now * 3.0).sin().abs() as f32;
        painter.rect_stroke(
            a.expand(4.0),
            CornerRadius::same(6),
            Stroke::new(3.0, MARK.gamma_multiply(pulse)),
            egui::StrokeKind::Outside,
        );
        arrow(&painter, card, a.expand(6.0));
        ctx.request_repaint();
    }
    let (title, body) = words(tour.step, seen, deploy);
    let mut pressed = None;
    let mut hits = Vec::new();
    let shown = egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .fixed_pos(card.min)
        .interactable(true)
        .show(ctx, |ui| {
            let t = crate::theme::theme(ui);
            egui::Frame::popup(ui.style())
                .fill(if t.dark { crate::st_view::brand::GRAPHITE } else { t.panel })
                .stroke(Stroke::new(2.0, MARK))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(14.0)
                .show(ui, |ui| {
                    ui.set_width(CARD_W - 32.0);
                    if let Some(n) = tour.step.number() {
                        ui.label(
                            RichText::new(format!("Step {n} of {}", Step::ACTIONS))
                                .size(crate::theme::MIN_TEXT)
                                .color(t.muted),
                        )
                        .tip("The tour moves on by itself when you do what the card asks.");
                    }
                    ui.label(
                        RichText::new(title)
                            .strong()
                            .size(crate::theme::HEADING)
                            .color(t.accent),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new(body).size(crate::theme::BODY).color(t.fg));
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        let mut button = |ui: &mut egui::Ui, b: Button, text: &str, tip: &str, strong: bool| {
                            let text = if strong {
                                RichText::new(text).strong()
                            } else {
                                RichText::new(text)
                            };
                            let r = ui
                                .add(egui::Button::new(text).min_size(Vec2::new(0.0, crate::st_view::TAP * 0.7)))
                                .tip(tip);
                            hits.push((key(b).to_string(), r.rect));
                            if r.clicked() {
                                pressed = Some(b);
                            }
                        };
                        match tour.step {
                            Step::Welcome => {
                                button(ui, Button::Start, "Start", "Begins the tour on the Blink example.", true);
                            }
                            Step::Agent => {
                                button(
                                    ui,
                                    Button::SkipStep,
                                    "Skip this step",
                                    "Goes to the last card without asking the agent.",
                                    false,
                                );
                            }
                            Step::Done => {
                                button(
                                    ui,
                                    Button::Requirements,
                                    "Requirements",
                                    "Opens the Requirements tab: each requirement is a test Deploy must pass.",
                                    false,
                                );
                                button(ui, Button::Help, "Help pages", "Opens the help pages in a new tab.", false);
                                button(ui, Button::Finish, "Finish", "Ends the tour.", true);
                            }
                            _ => {}
                        }
                        if tour.step != Step::Welcome {
                            button(ui, Button::Back, "Back", "Goes back one step.", false);
                        }
                        if tour.step != Step::Done {
                            button(
                                ui,
                                Button::Skip,
                                "Skip tour",
                                "Ends the tour here and leaves everything as it is. Help › Take the tour starts it again.",
                                false,
                            );
                        }
                    });
                });
        });
    let h = shown.response.rect.height();
    if (h - size.y).abs() > 0.5 {
        ctx.data_mut(|d| d.insert_temp(id, h));
        ctx.request_repaint();
    }
    chrome.hit("tour card", shown.response.rect);
    if let Some(a) = anchor {
        chrome.hit("tour mark", a);
    }
    for (k, r) in hits {
        chrome.hit(k, r);
    }
    pressed.and_then(|b| tour.press(b, seen))
}

/// The hit key of a card button (tests press them by it).
#[must_use]
pub const fn key(b: Button) -> &'static str {
    match b {
        Button::Start => "tour start",
        Button::Back => "tour back",
        Button::Skip => "tour skip",
        Button::SkipStep => "tour skip step",
        Button::Requirements => "tour requirements",
        Button::Help => "tour help",
        Button::Finish => "tour finish",
    }
}

/// A line from the card's nearest edge to the target, with a head.
fn arrow(painter: &egui::Painter, card: Rect, target: Rect) {
    let from = card.clamp(target.center());
    let to = target.clamp(from);
    let d = to - from;
    let len = d.length();
    if len < 4.0 {
        return;
    }
    let dir = d / len;
    let stroke = Stroke::new(3.0, MARK);
    painter.line_segment([from, to], stroke);
    let side = Vec2::new(-dir.y, dir.x);
    let head = [to, to - dir * 12.0 + side * 7.0, to - dir * 12.0 - side * 7.0];
    painter.add(egui::Shape::convex_polygon(head.to_vec(), MARK, Stroke::NONE));
}
