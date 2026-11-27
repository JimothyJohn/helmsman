//! The surface colours that follow View › Light mode, beside the brand
//! colours that never do (`st_view::brand`: the yellow, the lamps' green
//! and red, the warning orange, and black where it is ink on yellow).
//!
//! Dark is the default and is exactly what the app drew before there was
//! a light mode. Light is the same drawing on a light page. A caller
//! reads the [`Theme`] from the `Ui` it draws in ([`theme`]) and picks a
//! field by what the colour is for: the page, a panel, text, a wire, a
//! hairline, a chart's ground. Opacity fades (`gamma_multiply`, which
//! scales the alpha too) work on either page, so what fades keeps fading.

use eframe::egui::{self, Color32};

use crate::st_view::brand::{BLACK, GRAPHITE, PANEL, STEEL, WHITE, YELLOW};

/// The dark canvas, the darkest layer: the chrome floats 16 levels above it
/// (`CHROME_DARK`), a widget at rest 14 above that, so the layers separate.
const CANVAS_DARK: Color32 = Color32::from_gray(0x0C);
const CHROME_DARK: Color32 = Color32::from_gray(0x1C);

/// The colours of one theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub dark: bool,
    /// The page: the diagram's and the Monitor's ground.
    pub bg: Color32,
    /// A step off the page: node bodies, tiles, popups, code blocks.
    pub panel: Color32,
    /// Body text, wires carrying a value, ink on an inverse block.
    pub fg: Color32,
    /// Secondary text, a FALSE wire, port names, a disabled glyph.
    pub muted: Color32,
    /// Hairlines and chip grounds: a variable block's head, a switch that
    /// is off, a lane's separator, a tile's edge.
    pub rule: Color32,
    /// A second grey a step past `rule`: an operator's block.
    pub shade: Color32,
    /// A trend's ground.
    pub chart: Color32,
    /// A wire with no scan on it.
    pub idle: Color32,
    /// A block's outline.
    pub edge: Color32,
    /// The translucent ground the glyph strip floats on.
    pub veil: Color32,
    /// The ST editor's text.
    pub code: Color32,
    /// The ground of a source line with a diagnostic on it.
    pub bad: Color32,
    /// Yellow as text (the wordmark, a heading, the crumb you are on):
    /// the brand yellow on the dark page, an ochre that reads on the
    /// light one. Yellow fills, wires and highlights stay yellow.
    pub accent: Color32,
}

impl Theme {
    /// The theme of the page `ui` draws on.
    #[must_use]
    pub fn of(ui: &egui::Ui) -> Self {
        Self::for_mode(ui.visuals().dark_mode)
    }

    /// The dark theme, or the light one.
    #[must_use]
    pub fn for_mode(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }

    /// The default: the app as it always drew.
    #[must_use]
    pub fn dark() -> Self {
        Self {
            dark: true,
            bg: CANVAS_DARK,
            panel: PANEL,
            fg: WHITE,
            muted: STEEL,
            rule: GRAPHITE,
            shade: Color32::from_rgb(0x55, 0x55, 0x55),
            chart: CANVAS_DARK,
            idle: GRAPHITE.gamma_multiply(1.6),
            edge: Color32::from_black_alpha(200),
            veil: CANVAS_DARK.gamma_multiply(0.75),
            code: Color32::from_gray(230),
            bad: Color32::from_rgb(0x4A, 0x16, 0x14),
            accent: YELLOW,
        }
    }

    /// View › Light mode.
    #[must_use]
    pub fn light() -> Self {
        Self {
            dark: false,
            bg: Color32::from_rgb(0xEA, 0xEA, 0xEA),
            panel: WHITE,
            fg: BLACK,
            muted: Color32::from_rgb(0x66, 0x66, 0x66),
            rule: Color32::from_rgb(0xD0, 0xD0, 0xD0),
            shade: Color32::from_rgb(0xB8, 0xB8, 0xB8),
            chart: Color32::from_rgb(0xF6, 0xF6, 0xF6),
            idle: Color32::from_rgb(0xB4, 0xB4, 0xB4),
            edge: Color32::from_black_alpha(70),
            veil: Color32::from_rgba_unmultiplied(0xEA, 0xEA, 0xEA, 0xC0),
            code: Color32::from_gray(25),
            bad: Color32::from_rgb(0xFF, 0xD9, 0xD6),
            accent: Color32::from_rgb(0x7A, 0x5A, 0x00),
        }
    }

    /// A brand colour as text or a thin line on this page: itself on the
    /// dark page, where each reads; on the light one darkened, hue kept,
    /// until it stands [`INK_CONTRAST`] to 1 against the lightest grey a
    /// chip is drawn on. The green of "running", TRUE and FALSE and a
    /// trend's yellow trace stood 1.6 to 2.7 to 1 there (2026-11-10).
    /// Fills, lamps and grounds keep the brand colour.
    #[must_use]
    pub fn ink(self, c: Color32) -> Color32 {
        if self.dark {
            return c;
        }
        let [mut r, mut g, mut b, a] = c.to_array();
        while contrast([r, g, b], LIGHT_CHIP) < INK_CONTRAST {
            // 15/16 a step: no float, and it ends (black stands 17 to 1).
            r -= r.div_ceil(16);
            g -= g.div_ceil(16);
            b -= b.div_ceil(16);
        }
        Color32::from_rgba_unmultiplied(r, g, b, a)
    }
}

/// What [`Theme::ink`] holds a colour to on the light page: WCAG's
/// contrast for body text.
pub const INK_CONTRAST: f32 = 4.5;
/// The light page's chip ground (a button at rest), the darkest ground
/// its text is drawn on.
const LIGHT_CHIP: [u8; 3] = [0xEB, 0xEB, 0xEB];

/// WCAG 2 relative luminance of an sRGB colour.
fn luminance(c: [u8; 3]) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.039_28 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// WCAG 2 contrast ratio of two sRGB colours, 1 to 21.
#[must_use]
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// A label that is never broken across lines: a row of status words
/// (`horizontal_wrapped`) moves a whole label to the next line when it
/// does not fit, instead of cutting "compiles" off its tick.
pub fn whole(text: egui::RichText) -> egui::Label {
    egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend)
}

/// The theme of the page `ui` draws on.
#[must_use]
pub fn theme(ui: &egui::Ui) -> Theme {
    Theme::of(ui)
}

/// The smallest text anywhere around the diagram, in points (Nick,
/// 2026-11-05: "I can't see shit"). `ui_tests` reads every glyph drawn on
/// every screen and tab and refuses one below it. The diagram itself
/// zooms, so its labels follow the zoom.
pub const MIN_TEXT: f32 = 16.0;
/// Body text, buttons and fields.
pub const BODY: f32 = 18.0;
/// Code (the ST pane, values) and headings.
pub const MONO: f32 = 17.0;
pub const HEADING: f32 = 22.0;

/// The text face: Inter, subset to Latin, punctuation, arrows and the
/// shapes the app draws (`assets/fonts`, OFL). A glyph it lacks falls back
/// to egui's own fonts, which stay behind it in the list.
const TEXT_FONT: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
/// Code and values: IBM Plex Mono, subset the same way (OFL).
const MONO_FONT: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");

/// egui's text styles at the sizes above: `Small` is never below
/// [`MIN_TEXT`], so a `.small()` label stays legible.
fn text_styles() -> std::collections::BTreeMap<egui::TextStyle, egui::FontId> {
    use egui::{FontFamily, FontId, TextStyle};
    [
        (TextStyle::Small, FontId::new(MIN_TEXT, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(BODY, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(BODY, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(MONO, FontFamily::Monospace)),
        (TextStyle::Heading, FontId::new(HEADING, FontFamily::Proportional)),
    ]
    .into()
}

/// egui's fonts with Inter and Plex Mono in front.
fn fonts() -> egui::FontDefinitions {
    use egui::{FontData, FontFamily};
    use std::sync::Arc;
    let mut f = egui::FontDefinitions::default();
    for (name, data, family) in [
        ("inter", TEXT_FONT, FontFamily::Proportional),
        ("plex-mono", MONO_FONT, FontFamily::Monospace),
    ] {
        f.font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(data)));
        f.families.entry(family).or_default().insert(0, name.to_owned());
    }
    f
}

/// Corners on every widget, popup and window.
const RADIUS: u8 = 6;

/// The tones a widget steps through: at rest, under the pointer, pressed.
/// Fills only; a widget has no outline (a field keeps a faint one on the
/// light page, where its white would vanish on the panel).
struct Tones {
    rest: Color32,
    hover: Color32,
    press: Color32,
    raised: Color32,
    hairline: Color32,
    field_edge: Color32,
    ink: Color32,
    shadow: u8,
}

fn tones(dark: bool) -> Tones {
    if dark {
        Tones {
            rest: Color32::from_gray(0x2A),
            hover: Color32::from_gray(0x37),
            press: Color32::from_gray(0x44),
            raised: Color32::from_gray(0x26),
            hairline: Color32::from_gray(0x30),
            field_edge: Color32::TRANSPARENT,
            ink: Color32::from_gray(0xEE),
            shadow: 150,
        }
    } else {
        Tones {
            rest: Color32::from_gray(0xEB),
            hover: Color32::from_gray(0xE1),
            press: Color32::from_gray(0xD4),
            raised: WHITE,
            hairline: Color32::from_gray(0xE2),
            field_edge: Color32::from_gray(0xDA),
            ink: Color32::from_gray(0x1A),
            shadow: 44,
        }
    }
}

/// The widgets, panels, popups and spacing of one theme: tonal surfaces
/// and no outlines, the yellow (ochre on the light page) as the only
/// colour a control takes, soft small shadows.
fn style(s: &mut egui::Style, dark: bool) {
    use egui::{CornerRadius, Margin, Shadow, Stroke, vec2};
    let t = Theme::for_mode(dark);
    let k = tones(dark);
    let r = CornerRadius::same(RADIUS);
    let none = Stroke::NONE;
    let v = &mut s.visuals;
    v.panel_fill = if dark {
        CHROME_DARK
    } else {
        Color32::from_gray(0xFA)
    };
    v.window_fill = k.raised;
    v.window_stroke = Stroke::new(1.0, k.hairline);
    v.window_corner_radius = CornerRadius::same(RADIUS + 2);
    v.menu_corner_radius = CornerRadius::same(RADIUS + 2);
    let shadow = Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(k.shadow),
    };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.extreme_bg_color = if dark { Color32::from_gray(0x12) } else { WHITE };
    v.faint_bg_color = k.raised;
    v.code_bg_color = k.rest;
    v.hyperlink_color = t.accent;
    v.warn_fg_color = crate::st_view::brand::WARN;
    v.error_fg_color = crate::st_view::brand::RED;
    v.selection.bg_fill = k.hover;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.indent_has_left_vline = false;
    // A slider's thumb is a pill, not egui's round knob, and its track is
    // filled to the thumb in the accent (the checkbox and the caret are
    // painted in `check` and `caret`).
    v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };
    v.slider_trailing_fill = true;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = v.panel_fill;
    w.noninteractive.weak_bg_fill = v.panel_fill;
    w.noninteractive.bg_stroke = Stroke::new(1.0, k.hairline);
    w.noninteractive.fg_stroke = Stroke::new(1.0, k.ink);
    w.noninteractive.corner_radius = r;
    for (wv, fill, edge) in [
        (&mut w.inactive, k.rest, Stroke::new(1.0, k.field_edge)),
        (&mut w.hovered, k.hover, none),
        (&mut w.active, k.press, none),
        (&mut w.open, k.hover, none),
    ] {
        wv.bg_fill = fill;
        wv.weak_bg_fill = fill;
        wv.bg_stroke = edge;
        wv.corner_radius = r;
        wv.expansion = 0.0;
    }
    w.inactive.fg_stroke = Stroke::new(1.0, k.ink);
    w.hovered.fg_stroke = Stroke::new(1.5, t.fg);
    w.active.fg_stroke = Stroke::new(2.0, t.fg);
    w.open.fg_stroke = Stroke::new(1.0, t.fg);

    let sp = &mut s.spacing;
    // On the 4/8 pt grid: 8 between rows as between items.
    sp.item_spacing = vec2(8.0, 8.0);
    sp.button_padding = vec2(10.0, 4.0);
    sp.window_margin = Margin::same(12);
    sp.menu_margin = Margin::same(8);
    sp.menu_spacing = 4.0;
    s.text_styles = text_styles();
}

/// The frame of floating chrome (a side pane, the bottom pane, the header):
/// the chrome layer's fill, rounded corners, a shadow, and `FLOAT_INSET` of
/// the canvas left around it (Nick, 2026-11-27), as the diagram's corner
/// controls sit.
pub fn float_frame(ui: &egui::Ui) -> egui::Frame {
    let dark = ui.visuals().dark_mode;
    egui::Frame::new()
        .fill(ui.visuals().panel_fill)
        .corner_radius(egui::CornerRadius::same(RADIUS + 4))
        .shadow(egui::Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(tones(dark).shadow),
        })
        .inner_margin(egui::Margin {
            left: 8,
            right: 8,
            top: 8,
            // The corner's radius, so a scrolled list is cut above the curve.
            bottom: 12,
        })
        .outer_margin(egui::Margin::same(FLOAT_INSET))
}

/// How far floating chrome sits in from the window's edges and from its
/// neighbours, in points (8 to 12 by the decision, on the 4 pt grid).
pub const FLOAT_INSET: i8 = 8;

/// A tab (or a toggle chip): its label, muted until it is the one on,
/// which gets a 2 pt accent bar under it. Frameless until the pointer is
/// on it, and padded less than a button so a row of tabs stays narrow.
/// `min` is its least size.
pub fn tab(ui: &mut egui::Ui, on: bool, text: &str, min: egui::Vec2) -> egui::Response {
    let t = theme(ui);
    let label = egui::RichText::new(text).color(if on { t.fg } else { t.muted });
    let pad = TAB_PAD;
    // Not in a `scope`: its child ui starts a fresh row, so a tab in a
    // wrapping layout never moved to the next one.
    let was = ui.spacing().button_padding.x;
    ui.spacing_mut().button_padding.x = pad;
    let r = ui.add(
        egui::Button::new(label) // tip: exempt (the caller tips the tab)
            .selected(on)
            .frame_when_inactive(false)
            .min_size(min),
    );
    ui.spacing_mut().button_padding.x = was;
    if on {
        ui.painter().hline(
            (r.rect.left() + pad)..=(r.rect.right() - pad),
            px(ui, r.rect.bottom() - 1.0),
            egui::Stroke::new(2.0, t.accent),
        );
    }
    r
}

/// A tab's padding either side of its label.
const TAB_PAD: f32 = 6.0;

/// How long a hover or a press takes to ease in or out, seconds (Nick,
/// 2026-11-27: about 100 ms).
pub const EASE: f32 = 0.1;

/// A `from` to `to` mix of `amount` (0 to 1), channel by channel.
#[must_use]
pub fn mix(from: Color32, to: Color32, amount: f32) -> Color32 {
    from.lerp_to_gamma(to, amount.clamp(0.0, 1.0))
}

/// How far a control is toward `on` (0 to 1), eased over [`EASE`], keyed
/// by the response's own id and `key`, so one control can ease two things.
pub fn ease(ui: &egui::Ui, r: &egui::Response, key: &str, on: bool) -> f32 {
    ui.ctx()
        .animate_bool_with_time(r.id.with(("ease", key)), on, EASE)
}

/// A control's ground between its rest (nothing, or `rest`) and the tone
/// of the pointer over it or on it, eased: the hover and press of a
/// painted control, which egui would swap in a frame.
#[must_use]
pub fn eased_fill(ui: &egui::Ui, r: &egui::Response, rest: Color32) -> Color32 {
    let hover = ui.style().visuals.widgets.hovered.bg_fill;
    let press = ui.style().visuals.widgets.active.bg_fill;
    let h = ease(ui, r, "hover", r.hovered() || r.has_focus());
    let p = ease(ui, r, "press", r.is_pointer_button_down_on());
    mix(mix(rest, hover, h), press, p)
}

/// The ring keyboard focus shows on a painted control: the accent, 2 pt,
/// just outside `rect`. Nothing unless `r` has focus.
pub fn focus_ring(ui: &egui::Ui, r: &egui::Response, rect: egui::Rect, radius: u8) {
    if r.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            egui::CornerRadius::same(radius + 2),
            egui::Stroke::new(2.0, theme(ui).accent),
            egui::StrokeKind::Outside,
        );
    }
}

/// `v` on the physical pixel grid: a rect's edge drawn there is crisp at
/// any DPI.
#[must_use]
pub fn px(ui: &egui::Ui, v: f32) -> f32 {
    let ppp = ui.pixels_per_point();
    (v * ppp).round() / ppp
}

/// The fold caret, painted: a chevron pointing right when shut and down
/// when open (`openness` 0 to 1 turns it), 2 pt, round-ended like the
/// glyphs. Beside a title, in a collapsing header, instead of the stock
/// triangle.
pub fn caret(p: &egui::Painter, c: egui::Pos2, openness: f32, color: Color32) {
    let a = openness.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    let (sin, cos) = a.sin_cos();
    let turn = |x: f32, y: f32| egui::pos2(c.x + x * cos - y * sin, c.y + x * sin + y * cos);
    // Centred on the middle of the chevron's reach, not its tip.
    let pts = vec![turn(-2.5, -4.5), turn(2.0, 0.0), turn(-2.5, 4.5)];
    p.add(egui::Shape::line(pts, egui::Stroke::new(2.0, color)));
}

/// The icon of a `CollapsingHeader` (`.icon(theme::fold_icon)`): the
/// [`caret`], muted until the pointer is on the header.
pub fn fold_icon(ui: &mut egui::Ui, openness: f32, r: &egui::Response) {
    let t = theme(ui);
    let hot = ease(ui, r, "fold", r.hovered());
    caret(ui.painter(), r.rect.center(), openness, mix(t.muted, t.fg, hot));
}

/// A checkbox painted to match the glyphs: a 20 pt square (radius 4), its
/// ground eased on hover, the accent with a black tick when ticked, a
/// focus ring from the keyboard. `text` may be empty (a bare box in a
/// table). The response is clicked and changed on a toggle, as egui's.
pub fn check(ui: &mut egui::Ui, on: &mut bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    let text = text.into();
    let bare = text.is_empty();
    let side = CHECK_SIDE;
    let paint = |ui: &mut egui::Ui, on: bool, r: &egui::Response, rect: egui::Rect| {
        let t = theme(ui);
        let rest = if on { t.accent } else { tones(t.dark).rest };
        let fill = if on {
            mix(rest, Color32::WHITE, ease(ui, r, "hover", r.hovered()) * 0.15)
        } else {
            eased_fill(ui, r, rest)
        };
        let radius = 4;
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(radius), fill);
        if !on && !t.dark {
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(radius),
                egui::Stroke::new(1.0, tones(false).field_edge),
                egui::StrokeKind::Inside,
            );
        }
        if on {
            let c = rect.center();
            ui.painter().add(egui::Shape::line(
                vec![
                    egui::pos2(c.x - 5.0, c.y + 0.5),
                    egui::pos2(c.x - 1.5, c.y + 4.0),
                    egui::pos2(c.x + 5.0, c.y - 3.5),
                ],
                egui::Stroke::new(2.0, BLACK),
            ));
        }
        focus_ring(ui, r, rect, radius);
    };
    if bare {
        let (rect, mut r) = ui.allocate_exact_size(egui::Vec2::splat(side), egui::Sense::click());
        if r.clicked() {
            *on = !*on;
            r.mark_changed();
        }
        paint(ui, *on, &r, rect);
        return r;
    }
    let mut rect = egui::Rect::NOTHING;
    let inner = ui.horizontal(|ui| {
        let (b, rb) = ui.allocate_exact_size(egui::Vec2::splat(side), egui::Sense::click());
        rect = b;
        let rl = ui.add(
            egui::Label::new(text)
                .sense(egui::Sense::click())
                .wrap_mode(egui::TextWrapMode::Extend),
        );
        (rb, rl)
    });
    let (rb, rl) = inner.inner;
    let mut r = rb.union(rl);
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    paint(ui, *on, &r, rect);
    r
}

/// A live number (a clock, a value) in the mono face, right-aligned in a
/// slot as wide as `chars` digits, so it does not jitter or move what is
/// beside it as its figures change (Nick, 2026-11-27: mono is for code and
/// live values only, and live numbers have a fixed width).
pub fn live(ui: &mut egui::Ui, text: &str, color: Color32, chars: usize) -> egui::Response {
    let font = egui::FontId::monospace(MONO);
    let (galley, slot) = ui.fonts_mut(|f| {
        (
            f.layout_no_wrap(text.to_owned(), font.clone(), color),
            f.layout_no_wrap("0".repeat(chars), font, color).size().x,
        )
    });
    let size = egui::vec2(slot.max(galley.size().x), galley.size().y);
    let (rect, r) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().galley(
        egui::pos2(rect.right() - galley.size().x, rect.top()),
        galley,
        color,
    );
    r
}

/// The side of a [`check`] box, points.
pub const CHECK_SIDE: f32 = 20.0;

/// Keeps egui's theme on the one the layout asks for (`light`), so the
/// page, its widgets and [`Theme`] agree from the first frame, with the
/// fonts, widgets and text sizes above in both.
pub fn apply(ctx: &egui::Context, light: bool) {
    let want = if light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    };
    if ctx.theme() != want {
        ctx.set_theme(want);
    }
    let id = egui::Id::unique("helmsman-theme-styled");
    if !ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) {
        ctx.set_fonts(fonts());
        ctx.style_mut_of(egui::Theme::Dark, |s| style(s, true));
        ctx.style_mut_of(egui::Theme::Light, |s| style(s, false));
        ctx.data_mut(|d| d.insert_temp(id, true));
        // The fonts take effect on the next pass: lay the first frame out
        // again with them, or it is drawn at the default sizes and every
        // row moves on the second (a click aimed by the first missed).
        ctx.request_discard("fonts installed");
    }
    // Nick, 2026-11-05: a title or a row is not text to select, and a
    // control shows the finger. Labels are not selectable (the Agent's
    // answers opt back in); buttons take the finger here, every other
    // clickable widget in [`pointer_cursor`].
    if ctx.style_of(ctx.theme()).interaction.selectable_labels {
        ctx.all_styles_mut(|s| {
            s.interaction.selectable_labels = false;
            s.visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
        });
    }
}

/// A popup that opens where it was asked for (Nick, 2026-11-05: "going
/// all the way back and forth is very annoying, especially for bulk
/// deletes"): its top centre just under the pointer as it was when the
/// popup appeared, kept there while it stays open, held on the screen.
/// It is still a modal: the page behind is dimmed and takes no input.
#[must_use]
pub fn near_pointer(ctx: &egui::Context, key: &str) -> egui::Modal {
    let id = egui::Id::unique(("near-pointer", key));
    let pass = ctx.cumulative_pass_nr();
    let screen = ctx.content_rect();
    let pointer = ctx
        .input(|i| i.pointer.interact_pos().or(i.pointer.latest_pos()))
        .unwrap_or(screen.center());
    let at = ctx.data_mut(|d| {
        let seen: &mut (u64, egui::Pos2) = d.get_temp_mut_or_insert_with(id, || (0, pointer));
        // Not drawn last pass: this is a new opening, at the pointer now.
        if seen.0 + 1 < pass {
            seen.1 = pointer;
        }
        seen.0 = pass;
        seen.1
    });
    // egui's modal area, less its anchor (an anchor pins it to the centre).
    let area = egui::Area::new(egui::Id::unique(key))
        .kind(egui::UiKind::Modal)
        .sense(egui::Sense::hover())
        .order(egui::Order::Foreground)
        // No fade-in: the question is readable from the first frame it is up.
        .fade_in(false)
        .interactable(true)
        .pivot(egui::Align2::CENTER_TOP)
        .fixed_pos(at + egui::vec2(0.0, 14.0))
        .constrain(true)
        // Inset from the window's edge as the floating chrome is, not flush.
        .constrain_to(screen.shrink(f32::from(FLOAT_INSET)));
    egui::Modal::new(egui::Id::unique(key)).area(area)
}

/// Whether a modal that holds something typed should shut: Escape alone.
/// A click on the dimmed page behind it does nothing (Nick, 2026-11-14: a
/// stray click on coming back to the page shut the "Name this program"
/// question and dropped the Agent's answer with it). A question with
/// nothing to lose (a delete to confirm, a revision on view) still takes
/// a click outside as no, by `should_close`.
#[must_use]
pub fn escaped<T>(m: &egui::ModalResponse<T>) -> bool {
    m.response.should_close()
        || (m.is_top_modal
            && !m.any_popup_open
            && m.response
                .ctx
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)))
}

/// Whether a popup that is not a modal (a window, a panel under a
/// button) should shut now (Nick, 2026-11-11: a click outside a popup
/// closes it, as it does a modal): a press outside every rect of
/// `inside` (the popup, and the button that opens it), or Escape. The
/// press that opened it does not count: nothing shuts in the pass it
/// first shows, by `key`.
#[must_use]
pub fn dismissed(ctx: &egui::Context, key: &str, inside: &[egui::Rect]) -> bool {
    let id = egui::Id::unique(("dismissed", key));
    let pass = ctx.cumulative_pass_nr();
    let fresh = ctx.data_mut(|d| {
        let seen: &mut u64 = d.get_temp_mut_or_insert_with(id, || 0);
        let fresh = *seen + 1 < pass;
        *seen = pass;
        fresh
    });
    if fresh {
        return false;
    }
    let outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !inside.iter().any(|r| r.contains(p)))
    });
    outside || ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

/// Last in a frame: the finger over any enabled widget the pointer is on
/// that takes a click and is not dragged (a row, a checkbox, a combo box,
/// a tab, a fold arrow), unless a widget chose its own cursor (a text
/// field's I-beam, a slider's arrows, a panel edge's resize). A widget
/// that drags (the diagram, a slider) keeps its own.
pub fn pointer_cursor(ctx: &egui::Context) {
    let clickable = ctx.viewport(|vp| {
        vp.interact_widgets.hovered.iter().any(|id| {
            vp.this_pass
                .widgets
                .get(*id)
                .is_some_and(|w| w.enabled && w.sense.senses_click() && !w.sense.senses_drag())
        })
    });
    if clickable && ctx.output(|o| o.cursor_icon) == egui::CursorIcon::Default {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

#[cfg(test)]
mod tests {
    use super::Theme;
    use crate::st_view::brand::{BLACK, WHITE, YELLOW};

    /// The dark theme is the palette the app always had; the light one
    /// inverts the page and the ink and keeps the yellow for fills.
    #[test]
    fn dark_is_the_old_palette_and_light_inverts_the_page() {
        let d = Theme::dark();
        assert!(d.dark);
        assert_eq!((d.bg, d.fg, d.accent), (super::CANVAS_DARK, WHITE, YELLOW));
        let l = Theme::light();
        assert!(!l.dark);
        assert_eq!((l.panel, l.fg), (WHITE, BLACK));
        assert_ne!(l.accent, YELLOW, "yellow text gets an ochre on the light page");
        assert_eq!(Theme::for_mode(true), d);
        assert_eq!(Theme::for_mode(false), l);
    }

    /// Text on the light page reads: the accent, the secondary grey and
    /// every brand colour used as ink stand 4.5 to 1 on the page, a panel
    /// and a chip (the measured 2026-11-10 ratios were 1.6 to 4.2). The
    /// dark page keeps the brand colours as they are.
    #[test]
    fn light_mode_text_stands_out_from_every_ground() {
        use crate::st_view::brand::{GREEN, RED, WARN};
        let l = Theme::light();
        let rgb = |c: eframe::egui::Color32| [c.r(), c.g(), c.b()];
        let grounds = [rgb(l.bg), rgb(l.panel), [0xFA; 3], [0xEB; 3]];
        for (name, c) in [
            ("accent", l.accent),
            ("muted", l.muted),
            ("green", l.ink(GREEN)),
            ("red", l.ink(RED)),
            ("orange", l.ink(WARN)),
            ("yellow", l.ink(YELLOW)),
        ] {
            for g in grounds {
                let k = super::contrast(rgb(c), g);
                assert!(k >= super::INK_CONTRAST, "{name} on {g:?}: {k:.2}");
            }
        }
        let d = Theme::dark();
        for c in [GREEN, RED, WARN, YELLOW] {
            assert_eq!(d.ink(c), c);
        }
    }
}
