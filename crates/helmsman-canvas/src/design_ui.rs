//! The design step's screens (`crate::design_step` holds the state): the
//! entry in the Agent tab (the button, how a design under way is coming
//! along, the card of a design on show and of the project's kept design),
//! the window where the machine is described (and its documents added) and
//! the window where the design is read.
//!
//! Everything here takes plain data and answers with an [`Act`], so the
//! simulation's page and a box's page draw the same screens. The tab's
//! rows wrap and take no width of their own: the panel keeps its width.

use eframe::egui::{self, RichText};

use crate::design_step::{Designer, MAX_DESCRIPTION, sections};
use crate::editor::Chrome;
use crate::st_view::brand::{RED, WARN};
use crate::tips::Tip;

/// What the screens ask their owner to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Send the description.
    Send,
    Stop,
    CarryOn,
    /// Drop the design on show (or a cut one under way).
    Discard,
    /// Keep the design in the project as `design.md`.
    Keep,
    /// Build the next batch of the kept design.
    Build,
    /// Open the file picker for the documents.
    AddFile,
}

/// The project's kept design, as the tab shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub parts: usize,
    pub total: usize,
    pub built: usize,
    /// What stops the design being built from.
    pub problems: Vec<String>,
    /// The next batch, as steps `from + 1` to `to`.
    pub next: Option<(usize, usize)>,
    /// Why Build is off, if it is.
    pub build_blocked: Option<String>,
}

/// What the owner tells the entry.
#[derive(Clone, Debug, Default)]
pub struct EntryView {
    /// Page time, s.
    pub now: f64,
    /// Why the button is off, if it is (not the closed words: those are
    /// said beside it).
    pub blocked: Option<String>,
    /// Why the design on show cannot be kept, if it cannot.
    pub keep_blocked: Option<String>,
    pub kept: Option<Kept>,
}

fn clock(secs: f64) -> String {
    let s = secs as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn kib(bytes: usize) -> String {
    format!("{:.0} KiB", bytes as f64 / 1024.0)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The entry in the Agent tab. `None` when the entry is not shown at all.
pub fn entry_ui(ui: &mut egui::Ui, chrome: &mut Chrome, d: &mut Designer, v: &EntryView) -> Option<Act> {
    if !d.visible() {
        return None;
    }
    let size = crate::theme::BODY;
    let t = crate::theme::theme(ui);
    let mut act = None;
    ui.horizontal_wrapped(|ui| {
        let r = ui
            .add_enabled(
                v.blocked.is_none() && d.closed_line(v.now).is_none(),
                egui::Button::new(RichText::new("Design a machine…").size(size)),
            )
            .tip(v.blocked.clone().or_else(|| d.closed_line(v.now)).unwrap_or_else(|| {
                "Opens a window to describe a whole machine, or to paste or add the scope of work or product documentation it comes from. A strong model writes its control system design: the signals, the parts, the faults, numbered requirements and the order to build it in. It takes a few minutes. You read the design before anything is kept, and it is then built step by step, with a test for each requirement.".into()
            }));
        chrome.hit("design open", r.rect);
        if r.clicked() {
            d.open = true;
        }
    });
    if let Some(line) = d.closed_line(v.now) {
        let r = ui
            .add(egui::Label::new(RichText::new(line).size(size).color(t.muted)).wrap())
            .tip("The design entry answers on its own: the Describe tab carries on while it is closed.");
        chrome.hit("design closed", r.rect);
    }
    if let Some(n) = &d.notice {
        crate::messages::error_link_rich(
            ui,
            chrome,
            "design notice",
            RichText::new(n).size(size).color(t.ink(RED)),
            n,
            "How the last design ended. Nothing was kept.",
        );
    }
    if let Some(p) = d.progress(v.now) {
        progress_ui(ui, chrome, &p, &mut act);
    } else if d.read.is_some() {
        read_card(ui, chrome, d, v, &mut act);
    }
    if d.read.is_none()
        && let Some(k) = &v.kept
    {
        kept_card(ui, chrome, k, &mut act);
    }
    act
}

/// A design being written: the time, the thinking summary, Stop.
fn progress_ui(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    p: &crate::design_step::Progress,
    act: &mut Option<Act>,
) {
    let size = crate::theme::BODY;
    let t = crate::theme::theme(ui);
    if let Some(why) = &p.cut {
        crate::messages::error_link_rich(
            ui,
            chrome,
            "design cut",
            RichText::new(why).size(size).color(t.ink(RED)),
            why,
            "The turn stopped before the end marker, so what came is not a whole turn. Carry on asks for this turn again; Discard drops the design.",
        );
        ui.horizontal_wrapped(|ui| {
            let r = ui
                .button(RichText::new("Carry on").size(size))
                .tip("Asks for this turn again, after the turns that came whole.");
            chrome.hit("design carry on", r.rect);
            if r.clicked() {
                *act = Some(Act::CarryOn);
            }
            let r = ui
                .button(RichText::new("Discard").size(size))
                .tip("Drops the design. Nothing is kept.");
            chrome.hit("design cut discard", r.rect);
            if r.clicked() {
                *act = Some(Act::Discard);
            }
        });
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spinner();
        let r = ui
            .label(
                RichText::new(format!(
                    "Designing, turn {}: {} elapsed, {} written",
                    p.turn,
                    clock(p.secs),
                    kib(p.written)
                ))
                .size(size),
            )
            .tip("A design takes minutes: the first turn of a large one ran eight. The model's thinking summary is shown as it streams.");
        chrome.hit("design progress", r.rect);
        let r = crate::glyphs::action(ui, crate::glyphs::Icon::Stop, true, false)
            .tip("Stop: drops the design being written. Nothing is kept.");
        chrome.hit("design stop", r.rect);
        if r.clicked() {
            *act = Some(Act::Stop);
        }
    });
    if !p.thinking.is_empty() {
        let r = ui
            .add(egui::Label::new(RichText::new(&p.thinking).size(size).color(t.muted)).wrap())
            .tip("The model's own summary of what it is working out, as it streams.");
        chrome.hit("design thinking", r.rect);
    }
}

/// A design that came whole and is not kept yet.
fn read_card(ui: &mut egui::Ui, chrome: &mut Chrome, d: &mut Designer, v: &EntryView, act: &mut Option<Act>) {
    let size = crate::theme::BODY;
    let t = crate::theme::theme(ui);
    let Some(read) = &d.read else { return };
    let mut open = false;
    let line = format!(
        "Design ready: {}, {}, {} in {}.",
        plural(read.design.parts.len(), "part", "parts"),
        plural(read.design.requirements.len(), "requirement", "requirements"),
        plural(read.design.steps.len(), "build step", "build steps"),
        plural(read.turns, "turn", "turns"),
    );
    let r = ui
        .add(egui::Label::new(RichText::new(line).size(size).strong()).wrap())
        .tip("What the design holds, read from its headings.");
    chrome.hit("design card", r.rect);
    if !read.design.summary.is_empty() {
        let r = crate::md_view::label(ui, &read.design.summary, size)
            .tip("The design's own one-sentence summary.");
        chrome.hit("design summary", r.rect);
    }
    problems_ui(ui, chrome, &read.problems, "design problem");
    if let Some(why) = &v.keep_blocked {
        let r = ui
            .add(egui::Label::new(RichText::new(why).size(size).color(t.ink(WARN))).wrap())
            .tip("Why the design cannot be kept in this project.");
        chrome.hit("design keep blocked", r.rect);
    }
    ui.horizontal_wrapped(|ui| {
        let r = ui
            .button(RichText::new("Read the design").size(size))
            .tip("Opens the design in a window, one section at a time.");
        chrome.hit("design read", r.rect);
        open |= r.clicked();
        let r = ui
            .add_enabled(
                v.keep_blocked.is_none() && read.keep_refusal().is_none(),
                egui::Button::new(RichText::new("Keep").strong().size(size)),
            )
            .tip(v.keep_blocked.clone().or_else(|| read.keep_refusal()).unwrap_or_else(|| {
                "Keeps the design with the project as design.md, as one revision. On a box a project's files are written as a whole, so it goes into a copy of the project, in one request. It is then built a few steps at a time.".into()
            }));
        chrome.hit("design keep", r.rect);
        if r.clicked() {
            *act = Some(Act::Keep);
        }
        let r = ui
            .button(RichText::new("Discard").size(size))
            .tip("Drops the design. Nothing is kept.");
        chrome.hit("design discard", r.rect);
        if r.clicked() {
            *act = Some(Act::Discard);
        }
    });
    if open {
        d.window = true;
    }
}

fn problems_ui(ui: &mut egui::Ui, chrome: &mut Chrome, problems: &[String], key: &str) {
    let size = crate::theme::BODY;
    let t = crate::theme::theme(ui);
    for (k, p) in problems.iter().enumerate() {
        crate::messages::error_link_rich(
            ui,
            chrome,
            &format!("{key} {}", k + 1),
            RichText::new(p).size(size).color(t.ink(RED)),
            p,
            "Something in the design that cannot be built from. Build is not offered until the design has none: ask again, with the machine described more closely.",
        );
    }
}

/// The project's kept design: how far the build is, and Build.
fn kept_card(ui: &mut egui::Ui, chrome: &mut Chrome, k: &Kept, act: &mut Option<Act>) {
    let size = crate::theme::BODY;
    let t = crate::theme::theme(ui);
    let r = ui
        .add(
            egui::Label::new(
                RichText::new(format!(
                    "This project's design (design.md): {} over {}. Built {} of {}.",
                    plural(k.total, "step", "steps"),
                    plural(k.parts, "part", "parts"),
                    k.built.min(k.total),
                    k.total
                ))
                .size(size),
            )
            .wrap(),
        )
        .tip("The design kept with the project. It is built a few steps at a time, each checked against the design's requirements.");
    chrome.hit("design kept", r.rect);
    problems_ui(ui, chrome, &k.problems, "design kept problem");
    let Some((from, to)) = k.next else {
        if k.problems.is_empty() && k.total > 0 {
            let r = ui
                .add(egui::Label::new(RichText::new("Every step is built.").size(size).color(t.muted)).wrap())
                .tip("Every step of the build order has been built in this page. Open the Log for what each step did.");
            chrome.hit("design built", r.rect);
        }
        return;
    };
    if !k.problems.is_empty() {
        return;
    }
    let label = format!("Build steps {} to {} of {}", from + 1, to, k.total);
    ui.horizontal_wrapped(|ui| {
        let r = ui
            .add_enabled(
                k.build_blocked.is_none(),
                egui::Button::new(RichText::new(&label).strong().size(size)),
            )
            .tip(k.build_blocked.clone().unwrap_or_else(|| {
                format!(
                    "Builds the design's steps {} to {}: each is checked on a copy, and the file changes once, when they have all passed. The steps after these wait for the next press.",
                    from + 1,
                    to
                )
            }));
        chrome.hit("design build", r.rect);
        if r.clicked() {
            *act = Some(Act::Build);
        }
    });
}

/// The width of a window on a page `screen` wide: room on both sides, no
/// more than reads well.
fn window_width(ctx: &egui::Context) -> f32 {
    (ctx.content_rect().width() - 40.0).clamp(240.0, 640.0)
}

/// The window where the machine is described. `blocked` is why Send cannot
/// go for a reason outside the description.
pub fn ask_window(
    ctx: &egui::Context,
    chrome: &mut Chrome,
    d: &mut Designer,
    blocked: Option<&str>,
) -> Option<Act> {
    if !d.open {
        return None;
    }
    let size = crate::theme::BODY;
    let width = window_width(ctx);
    let tall = (ctx.content_rect().height() * 0.25).max(80.0);
    let mut act = None;
    let m = egui::Modal::new(egui::Id::unique("design-form")).show(ctx, |ui| {
        let t = crate::theme::theme(ui);
        ui.set_width(width);
        ui.label(RichText::new("Design a machine").strong().size(size))
            .tip("Describe the whole machine and what it must do, and add the documents it comes from. A strong model writes its control system design; you read it before anything is kept.");
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("design-scroll")
            .max_height(tall)
            .show(ui, |ui| {
                let r = ui
                    .add(
                        egui::TextEdit::multiline(&mut d.description)
                            .id_salt("design-text")
                            .hint_text("A bottling line: conveyor, rinser, filler, capper, with a reject station and a fault light")
                            .desired_rows(8)
                            .desired_width(f32::INFINITY),
                    )
                    .tip("The machine, in your words: what it makes or moves, its stations, its sensors and actuators, and what must happen when something goes wrong. The description can be left out when a document says it.");
                chrome.hit("design text", r.rect.intersect(ui.clip_rect()));
            });
        let screen = ctx.content_rect().height();
        let tall = ((screen * 0.15).max(60.0), (screen * 0.1).max(40.0));
        if let Some(crate::documents::IntakeAct::AddFile) =
            crate::documents::intake_ui(ui, chrome, &mut d.docs, &d.description, tall)
        {
            act = Some(Act::AddFile);
        }
        let n = d.description.trim().chars().count();
        let over = n > MAX_DESCRIPTION;
        let r = ui
            .add(
                egui::Label::new(
                    RichText::new(format!("{n} of {MAX_DESCRIPTION} characters"))
                        .size(size)
                        .color(if over { t.ink(RED) } else { t.muted }),
                )
                .wrap(),
            )
            .tip("The longest description the design entry takes.");
        chrome.hit("design count", r.rect);
        let why = blocked.map(str::to_string).or_else(|| {
            if n == 0 && d.docs.is_empty() {
                Some("Describe the machine, or add a document, first.".into())
            } else if over {
                Some(format!("A description is at most {MAX_DESCRIPTION} characters."))
            } else {
                d.docs.refusal(d.description.trim())
            }
        });
        if let Some(e) = d.error.as_ref().or(why.as_ref()) {
            crate::messages::error_link_rich(
                ui,
                chrome,
                "design error",
                RichText::new(e).size(size).color(t.ink(RED)),
                e,
                "Why the design cannot be asked for yet.",
            );
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let go = ui
                .add_enabled(why.is_none(), egui::Button::new(RichText::new("Ask for a design").strong().size(size)))
                .tip(why.clone().unwrap_or_else(|| {
                    "Sends the description and the documents. The design takes a few minutes; you can watch it come in, and Stop drops it.".into()
                }));
            chrome.hit("design send", go.rect);
            if go.clicked() {
                act = Some(Act::Send);
            }
            let no = ui
                .button(RichText::new("Close").size(size))
                .tip("Closes this window. What is typed stays until the page is closed.");
            chrome.hit("design close", no.rect);
            if no.clicked() {
                d.open = false;
            }
        });
    });
    chrome.hit("design form", m.response.rect);
    if crate::theme::escaped(&m) {
        d.open = false;
    }
    act
}

/// The window where the design is read: its sections one at a time, and
/// Keep. `keep_blocked` is why Keep is off for a reason outside the design.
pub fn read_window(
    ctx: &egui::Context,
    chrome: &mut Chrome,
    d: &mut Designer,
    keep_blocked: Option<&str>,
) -> Option<Act> {
    if !d.window {
        return None;
    }
    let size = crate::theme::BODY;
    let width = window_width(ctx);
    let tall = (ctx.content_rect().height() * 0.5).max(100.0);
    let mut act = None;
    let mut close = false;
    let Some(read) = &mut d.read else {
        d.window = false;
        return None;
    };
    let parts = sections(&read.design, &read.text);
    let m = egui::Modal::new(egui::Id::unique("design-read")).show(ctx, |ui| {
        let t = crate::theme::theme(ui);
        ui.set_width(width);
        ui.label(RichText::new("The design").strong().size(size))
            .tip("What the strong model wrote. Nothing is kept until Keep.");
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for (k, (name, _)) in parts.iter().enumerate() {
                let on = read.section == k;
                let r = ui
                    .add(egui::Button::new(RichText::new(name).size(size)).selected(on))
                    .tip(format!("Shows the {name} section."));
                chrome.hit(format!("design section {}", k + 1), r.rect);
                if r.clicked() {
                    read.section = k;
                }
            }
        });
        // The summary and what is wrong with the design go with the text
        // under one scroll bar: a window taller than a phone's page has no
        // buttons in reach.
        egui::ScrollArea::vertical()
            .id_salt("design-read-scroll")
            .max_height(tall)
            .show(ui, |ui| {
                ui.set_max_width(width - 14.0);
                if !read.design.summary.is_empty() {
                    let r = crate::md_view::label(ui, &read.design.summary, size)
                        .tip("The design's one-sentence summary.");
                    chrome.hit("design window summary", r.rect.intersect(ui.clip_rect()));
                }
                problems_ui(ui, chrome, &read.problems, "design window problem");
                let at = read.section;
                if read.shown.as_ref().is_none_or(|(k, _)| *k != at) {
                    let body = parts
                        .get(at)
                        .map_or("The design has no sections the page knows.", |(_, b)| b.as_str());
                    read.shown = Some((at, crate::md_view::parse(body)));
                }
                let blocks = read.shown.as_ref().map_or(&[][..], |(_, b)| b.as_slice());
                let r = ui
                    .scope(|ui| crate::md_view::show(ui, blocks, size, "design-body"))
                    .response
                    .tip("The section, set out as Markdown: tables as grids, code as written. design.md keeps the model's text as it wrote it.");
                chrome.hit("design body", r.rect.intersect(ui.clip_rect()));
            });
        ui.add_space(8.0);
        let why = keep_blocked.map(str::to_string).or_else(|| read.keep_refusal());
        if let Some(w) = &why {
            let r = ui
                .add(egui::Label::new(RichText::new(w).size(size).color(t.ink(WARN))).wrap())
                .tip("Why the design cannot be kept in this project.");
            chrome.hit("design window blocked", r.rect);
        }
        ui.horizontal_wrapped(|ui| {
            let r = ui
                .add_enabled(
                    why.is_none(),
                    egui::Button::new(RichText::new("Keep").strong().size(size)),
                )
                .tip(why.clone().unwrap_or_else(|| {
                    "Keeps the design with the project as design.md. On a box it goes into a copy of the project, in one request.".into()
                }));
            chrome.hit("design window keep", r.rect);
            if r.clicked() {
                act = Some(Act::Keep);
            }
            let r = ui
                .button(RichText::new("Discard").size(size))
                .tip("Drops the design. Nothing is kept.");
            chrome.hit("design window discard", r.rect);
            if r.clicked() {
                act = Some(Act::Discard);
            }
            let r = ui
                .button(RichText::new("Close").size(size))
                .tip("Closes this window; the design stays on the Describe tab until you keep or discard it.");
            chrome.hit("design window close", r.rect);
            close |= r.clicked();
        });
    });
    chrome.hit("design read window", m.response.rect);
    if close || crate::theme::escaped(&m) {
        d.window = false;
    }
    act
}
