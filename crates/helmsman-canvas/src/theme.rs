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
            bg: BLACK,
            panel: PANEL,
            fg: WHITE,
            muted: STEEL,
            rule: GRAPHITE,
            shade: Color32::from_rgb(0x55, 0x55, 0x55),
            chart: BLACK,
            idle: GRAPHITE.gamma_multiply(1.6),
            edge: Color32::from_black_alpha(200),
            veil: BLACK.gamma_multiply(0.75),
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
            bg: Color32::from_rgb(0xF4, 0xF4, 0xF4),
            panel: WHITE,
            fg: BLACK,
            muted: Color32::from_rgb(0x66, 0x66, 0x66),
            rule: Color32::from_rgb(0xD0, 0xD0, 0xD0),
            shade: Color32::from_rgb(0xB8, 0xB8, 0xB8),
            chart: Color32::from_rgb(0xFA, 0xFA, 0xFA),
            idle: Color32::from_rgb(0xB4, 0xB4, 0xB4),
            edge: Color32::from_black_alpha(70),
            veil: Color32::from_rgba_unmultiplied(0xF4, 0xF4, 0xF4, 0xC0),
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
            rest: Color32::from_gray(0x26),
            hover: Color32::from_gray(0x30),
            press: Color32::from_gray(0x3A),
            raised: Color32::from_gray(0x20),
            hairline: Color32::from_gray(0x2C),
            field_edge: Color32::TRANSPARENT,
            ink: Color32::from_gray(0xE6),
            shadow: 120,
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
            shadow: 30,
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
        Color32::from_gray(0x1A)
    } else {
        Color32::from_gray(0xF7)
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
    v.extreme_bg_color = if dark { Color32::from_gray(0x10) } else { WHITE };
    v.faint_bg_color = k.raised;
    v.code_bg_color = k.rest;
    v.hyperlink_color = t.accent;
    v.warn_fg_color = crate::st_view::brand::WARN;
    v.error_fg_color = crate::st_view::brand::RED;
    v.selection.bg_fill = k.hover;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.indent_has_left_vline = false;

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
    sp.item_spacing = vec2(8.0, 6.0);
    sp.button_padding = vec2(10.0, 4.0);
    sp.window_margin = Margin::same(12);
    sp.menu_margin = Margin::same(8);
    sp.menu_spacing = 4.0;
    s.text_styles = text_styles();
}

/// A tab (or a toggle chip): its label, muted until it is the one on,
/// which gets a 2 pt accent bar under it. Frameless until the pointer is
/// on it, and padded less than a button so a row of tabs stays narrow.
/// `min` is its least size.
pub fn tab(ui: &mut egui::Ui, on: bool, text: &str, min: egui::Vec2) -> egui::Response {
    let t = theme(ui);
    let label = egui::RichText::new(text).color(if on { t.fg } else { t.muted });
    let pad = TAB_PAD;
    let r = ui
        .scope(|ui| {
            ui.spacing_mut().button_padding.x = pad;
            ui.add(
                egui::Button::new(label) // tip: exempt (the caller tips the tab)
                    .selected(on)
                    .frame_when_inactive(false)
                    .min_size(min),
            )
        })
        .inner;
    if on {
        ui.painter().hline(
            (r.rect.left() + pad)..=(r.rect.right() - pad),
            r.rect.bottom() - 1.0,
            egui::Stroke::new(2.0, t.accent),
        );
    }
    r
}

/// A tab's padding either side of its label.
const TAB_PAD: f32 = 6.0;

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
        .interactable(true)
        .pivot(egui::Align2::CENTER_TOP)
        .fixed_pos(at + egui::vec2(0.0, 14.0))
        .constrain(true);
    egui::Modal::new(egui::Id::unique(key)).area(area)
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
        assert_eq!((d.bg, d.fg, d.accent), (BLACK, WHITE, YELLOW));
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
        let grounds = [rgb(l.bg), rgb(l.panel), [0xF7; 3], [0xEB; 3]];
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
