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
            muted: Color32::from_rgb(0x6E, 0x6E, 0x6E),
            rule: Color32::from_rgb(0xD0, 0xD0, 0xD0),
            shade: Color32::from_rgb(0xB8, 0xB8, 0xB8),
            chart: Color32::from_rgb(0xFA, 0xFA, 0xFA),
            idle: Color32::from_rgb(0xB4, 0xB4, 0xB4),
            edge: Color32::from_black_alpha(70),
            veil: Color32::from_rgba_unmultiplied(0xF4, 0xF4, 0xF4, 0xC0),
            code: Color32::from_gray(25),
            bad: Color32::from_rgb(0xFF, 0xD9, 0xD6),
            accent: Color32::from_rgb(0x9C, 0x75, 0x00),
        }
    }
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

/// Keeps egui's theme on the one the layout asks for (`light`), so the
/// page, its widgets and [`Theme`] agree from the first frame, and the
/// text at the sizes above in both.
pub fn apply(ctx: &egui::Context, light: bool) {
    let want = if light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    };
    if ctx.theme() != want {
        ctx.set_theme(want);
    }
    if ctx
        .style_of(ctx.theme())
        .text_styles
        .get(&egui::TextStyle::Body)
        .map(|f| f.size)
        != Some(BODY)
    {
        ctx.all_styles_mut(|s| s.text_styles = text_styles());
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
}
