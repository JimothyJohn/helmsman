//! The Editor's frame, the same on a box (`box_view`) and in the
//! simulation (`playground_view`), as docs/UI.md lays it out: a toolbar
//! under the header, the files and program tree on the left, the live
//! diagram in the centre, the ST beside it on demand ("ST"), and the right
//! panel's tabs (I/O, Trend, Agent). Both sidebars collapse to a strip;
//! which are open is [`Chrome`], kept per browser by the app.
//!
//! What fills each part is the view's ([`Body`]); where it goes is here.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, TextFormat, Vec2,
    text::LayoutJob, text_edit::TextEditState,
};

use crate::st_view::TAP;
use crate::st_view::brand::{GRAPHITE, STEEL, WHITE, YELLOW};

/// Where the app keeps [`Chrome`] in the browser's storage.
pub const CHROME_KEY: &str = "helmsman-canvas/editor";

/// Width of a collapsed sidebar, points.
pub const STRIP: f32 = 30.0;

const RED: Color32 = Color32::from_rgb(255, 110, 100);

/// The right panel's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Io,
    Trend,
    Claude,
}

impl Tab {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Io => "I/O",
            Self::Trend => "Trend",
            Self::Claude => "Agent",
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::Trend => "trend",
            Self::Claude => "claude",
        }
    }
}

/// The Editor's layout (kept per browser) and what was drawn where this
/// frame (tests click by it).
#[derive(Clone, Debug, PartialEq)]
pub struct Chrome {
    /// The left sidebar (files and tree) is open.
    pub left: bool,
    /// The right panel (tabs) is open.
    pub right: bool,
    /// The ST editor is open beside the diagram.
    pub st: bool,
    pub tab: Tab,
    /// The source line a block click sent the ST editor to.
    pub line: Option<u32>,
    /// [`Self::line`] not scrolled to yet.
    jump: Option<u32>,
    /// Controls drawn this frame, by name.
    pub hits: Vec<(String, Rect)>,
}

impl Default for Chrome {
    fn default() -> Self {
        Self {
            left: true,
            right: true,
            st: false,
            tab: Tab::Io,
            line: None,
            jump: None,
            hits: Vec::new(),
        }
    }
}

impl Chrome {
    /// What is kept per browser, as text.
    #[must_use]
    pub fn saved(&self) -> String {
        format!(
            "left={} right={} st={} tab={}",
            u8::from(self.left),
            u8::from(self.right),
            u8::from(self.st),
            self.tab.key()
        )
    }

    /// [`Self::saved`] read back; anything unknown keeps its default.
    #[must_use]
    pub fn restored(text: &str) -> Self {
        let mut c = Self::default();
        for kv in text.split_whitespace() {
            match kv.split_once('=') {
                Some(("left", v)) => c.left = v == "1",
                Some(("right", v)) => c.right = v == "1",
                Some(("st", v)) => c.st = v == "1",
                Some(("tab", v)) => {
                    if let Some(t) = [Tab::Io, Tab::Trend, Tab::Claude]
                        .into_iter()
                        .find(|t| t.key() == v)
                    {
                        c.tab = t;
                    }
                }
                _ => {}
            }
        }
        c
    }

    /// Notes that `key` was drawn at `rect`.
    pub fn hit(&mut self, key: impl Into<String>, rect: Rect) {
        self.hits.push((key.into(), rect));
    }

    /// Where `key` was drawn this frame.
    #[cfg(test)]
    #[must_use]
    pub fn rect(&self, key: &str) -> Option<Rect> {
        self.hits.iter().rev().find(|(k, _)| k == key).map(|(_, r)| *r)
    }

    /// Sends the ST editor to `line` (it scrolls there when next drawn).
    pub fn goto(&mut self, line: u32) {
        if line > 0 {
            self.line = Some(line);
            self.jump = Some(line);
        }
    }
}

/// What a view puts in the Editor's frame.
pub trait Body {
    /// The toolbar's own part, after the ST toggle.
    fn toolbar(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// Under "Files": the open program and its tree.
    fn files(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The ST editor (read-only on a box).
    fn st(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// The right panel's tabs this view has, in order.
    fn tabs(&self) -> &'static [Tab];
    fn tab(&mut self, ui: &mut egui::Ui, tab: Tab);
    /// The diagram with its breadcrumbs; a block click calls
    /// [`Chrome::goto`].
    fn diagram(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
}

/// A collapsed sidebar's strip as one button: a chevron pointing `left`
/// or right, the strip's full height, tinted under the pointer.
fn strip_toggle(ui: &mut egui::Ui, left: bool, hover: &str) -> egui::Response {
    let size = Vec2::new(
        ui.available_width().max(STRIP - 8.0),
        ui.available_height().max(TAP),
    );
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let r = r.on_hover_text(hover);
    if r.hovered() {
        let tint = ui.style().interact(&r).bg_fill;
        ui.painter().rect_filled(rect, CornerRadius::same(4), tint);
    }
    chevron(ui, rect.center(), left, r.hovered());
    r
}

/// An open sidebar's hide button, the same chevron in the header row.
fn header_toggle(ui: &mut egui::Ui, left: bool, hover: &str) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(TAP * 0.75), Sense::click());
    let r = r.on_hover_text(hover);
    if r.hovered() {
        let tint = ui.style().interact(&r).bg_fill;
        ui.painter().rect_filled(rect, CornerRadius::same(4), tint);
    }
    chevron(ui, rect.center(), left, r.hovered());
    r
}

/// A 16 pt stroked chevron at `c`.
fn chevron(ui: &egui::Ui, c: Pos2, left: bool, hot: bool) {
    let s = 8.0;
    let dir = if left { -1.0 } else { 1.0 };
    let tip = Pos2::new(c.x + dir * s * 0.5, c.y);
    let back = c.x - dir * s * 0.5;
    let stroke = Stroke::new(2.5, if hot { YELLOW } else { STEEL });
    ui.painter().line_segment([Pos2::new(back, c.y - s), tip], stroke);
    ui.painter().line_segment([Pos2::new(back, c.y + s), tip], stroke);
}

/// One frame of the Editor.
pub fn show(ui: &mut egui::Ui, chrome: &mut Chrome, body: &mut dyn Body) {
    egui::Panel::top("ed-tools").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let st = egui::Button::selectable(chrome.st, RichText::new("ST").strong().monospace())
                .min_size(Vec2::new(TAP, 0.0));
            let r = ui.add(st).on_hover_text("the Structured Text beside the diagram");
            chrome.hit("st", r.rect);
            if r.clicked() {
                chrome.st = !chrome.st;
            }
            ui.separator();
            body.toolbar(ui, chrome);
        });
    });

    let mut hits = Vec::new();
    let mut left = chrome.left;
    let flip = egui::Panel::show_switched(
        ui,
        &mut left,
        egui::Panel::left("ed-files-strip")
            .exact_size(STRIP)
            .resizable(false),
        egui::Panel::left("ed-files").resizable(true).default_size(230.0),
        |ui, open| {
            let toggle = if open {
                let mut t = None;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Files").strong().color(YELLOW));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        t = Some(header_toggle(ui, true, "hide the files"));
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("ed-files-scroll")
                    .show(ui, |ui| body.files(ui, chrome));
                t
            } else {
                Some(strip_toggle(ui, false, "show the files"))
            };
            if let Some(t) = toggle {
                hits.push(("left".to_string(), t.rect));
                t.clicked()
            } else {
                false
            }
        },
    )
    .inner;
    if flip {
        left = !left;
    }
    chrome.left = left;

    let tabs = body.tabs();
    if !tabs.contains(&chrome.tab) {
        chrome.tab = tabs.first().copied().unwrap_or_default();
    }
    let mut right = chrome.right;
    let mut tab = chrome.tab;
    let flip = egui::Panel::show_switched(
        ui,
        &mut right,
        egui::Panel::right("ed-side-strip")
            .exact_size(STRIP)
            .resizable(false),
        egui::Panel::right("ed-side").resizable(true).default_size(340.0),
        |ui, open| {
            let toggle = if open {
                let mut t = None;
                ui.horizontal(|ui| {
                    t = Some(header_toggle(ui, false, "hide the panel"));
                    for &k in tabs {
                        let b = egui::Button::selectable(tab == k, RichText::new(k.label()).strong())
                            .min_size(Vec2::new(64.0, TAP * 0.75));
                        let r = ui.add(b);
                        hits.push((format!("tab {}", k.key()), r.rect));
                        if r.clicked() {
                            tab = k;
                        }
                    }
                });
                ui.separator();
                body.tab(ui, tab);
                t
            } else {
                Some(strip_toggle(ui, true, "show the panel"))
            };
            if let Some(t) = toggle {
                hits.push(("right".to_string(), t.rect));
                t.clicked()
            } else {
                false
            }
        },
    )
    .inner;
    if flip {
        right = !right;
    }
    chrome.right = right;
    chrome.tab = tab;
    chrome.hits.extend(hits);

    let mut st = chrome.st;
    egui::Panel::left("ed-st")
        .resizable(true)
        .default_size(220.0)
        .show_collapsible(ui, &mut st, |ui| body.st(ui, chrome));
    chrome.st = st;

    egui::CentralPanel::no_frame().show(ui, |ui| body.diagram(ui, chrome));
}

/// The ST, with line numbers, diagnostics marked (`(line, column)`), and
/// the line [`Chrome::goto`] sent it to scrolled into view and marked. A
/// `&mut &str` is read-only (on a box); a `String` is edited in place, up
/// to `limit` characters. Lines wrap at the pane's width; a wrapped line
/// is numbered once, on its first row.
pub fn st_editor(
    ui: &mut egui::Ui,
    id: egui::Id,
    text: &mut dyn egui::TextBuffer,
    diags: &[(u32, u32)],
    limit: usize,
    chrome: &mut Chrome,
) -> egui::Response {
    let font = FontId::monospace(13.0);
    let row = ui.fonts_mut(|f| f.row_height(&font));
    let mut scroll = egui::ScrollArea::vertical().id_salt(id.with("scroll"));
    if let Some(line) = chrome.jump.take() {
        #[allow(clippy::cast_precision_loss)]
        let y = (line.saturating_sub(4)) as f32 * row;
        scroll = scroll.vertical_scroll_offset(y);
        // The caret to the start of the line, for an editor that has one.
        if let Some(mut state) = TextEditState::load(ui.ctx(), id) {
            let at = text
                .as_str()
                .split_inclusive('\n')
                .take(line.saturating_sub(1) as usize)
                .map(|l| l.chars().count())
                .sum::<usize>();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(at))));
            state.store(ui.ctx(), id);
        }
    }
    let at = chrome.line;
    scroll
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let digit = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
                let gutter_w = digit * 3.0;
                // Slack for the editor's own margin and the scroll bar: wider
                // than the pane and a resizable pane grows to fit, every frame.
                let wrap = (ui.available_width() - gutter_w - ui.spacing().item_spacing.x - 32.0).max(120.0);
                // The rows the text takes at that width: each source line
                // numbered on its first row, its wrapped rows blank.
                let galley = ui.fonts_mut(|f| f.layout_job(highlight(text.as_str(), diags, at, &font, wrap)));
                let mut gutter = LayoutJob::default();
                for (n, number) in numbering(galley.rows.iter().map(|r| r.ends_with_newline)) {
                    let bad = diags.iter().any(|(l, _)| *l as usize == n);
                    let here = at.is_some_and(|l| l as usize == n);
                    let color = if bad {
                        RED
                    } else if here {
                        YELLOW
                    } else {
                        STEEL
                    };
                    let label = if number {
                        format!("{n:>3}\n")
                    } else {
                        "   \n".to_string()
                    };
                    gutter.append(&label, 0.0, TextFormat::simple(font.clone(), color));
                }
                gutter.wrap.max_width = f32::INFINITY;
                ui.label(gutter);
                let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                    let job = highlight(buf.as_str(), diags, at, &font, wrap);
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                ui.add(
                    egui::TextEdit::multiline(text)
                        .id(id)
                        .code_editor()
                        .desired_width(wrap)
                        .char_limit(limit)
                        .desired_rows(24)
                        .lock_focus(true)
                        .layouter(&mut layouter),
                )
            })
            .inner
        })
        .inner
}

/// The gutter beside laid-out rows (`ends_with_newline` per row): each
/// row's source line, and whether the row is that line's first (the one
/// that shows the number; a wrapped line's other rows stay blank).
#[must_use]
pub fn numbering(rows: impl Iterator<Item = bool>) -> Vec<(usize, bool)> {
    let mut n = 1;
    let mut first = true;
    rows.map(|ends| {
        let out = (n, first);
        first = ends;
        if ends {
            n += 1;
        }
        out
    })
    .collect()
}

/// The ST as a layout wrapped at `wrap` points: a line with a diagnostic
/// on a dark red ground, its column to the end of the word underlined;
/// the line `at` (a block click sent the editor there) on a graphite
/// ground.
#[must_use]
pub fn highlight(text: &str, diags: &[(u32, u32)], at: Option<u32>, font: &FontId, wrap: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = TextFormat::simple(font.clone(), Color32::from_gray(230));
    let bad_ground = Color32::from_rgb(0x4A, 0x16, 0x14);
    for (k, line) in text.split_inclusive('\n').enumerate() {
        let n = u32::try_from(k + 1).unwrap_or(u32::MAX);
        let Some(&(_, col)) = diags.iter().find(|(l, _)| *l == n) else {
            let format = if at == Some(n) {
                TextFormat {
                    background: GRAPHITE,
                    color: WHITE,
                    ..plain.clone()
                }
            } else {
                plain.clone()
            };
            job.append(line, 0.0, format);
            continue;
        };
        let ground = TextFormat {
            background: bad_ground,
            ..plain.clone()
        };
        // Columns count characters from 1.
        let start = line
            .char_indices()
            .nth(col.saturating_sub(1) as usize)
            .map_or(line.trim_end_matches('\n').len(), |(i, _)| i);
        let rest = &line[start..];
        let word = rest
            .char_indices()
            .find(|(i, c)| *i > 0 && !(c.is_alphanumeric() || *c == '_'))
            .map_or(rest.trim_end_matches('\n').len(), |(i, _)| i)
            .max(
                rest.chars()
                    .next()
                    .filter(|c| *c != '\n')
                    .map_or(0, char::len_utf8),
            );
        job.append(&line[..start], 0.0, ground.clone());
        job.append(
            &rest[..word],
            0.0,
            TextFormat {
                underline: Stroke::new(1.5, RED),
                color: WHITE,
                ..ground.clone()
            },
        );
        job.append(&rest[word..], 0.0, ground);
    }
    job.wrap.max_width = wrap;
    job
}
