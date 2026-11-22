//! How the 3D view looks: the palette, the shapes' proportions and the
//! strength of the glow, in one place. Everything the renderer and the
//! labels take as taste is a field here; a theme (dark or light) is the
//! only input. Colours are sRGB, blended as egui blends.

use eframe::egui::Color32;

use crate::scene3d::{Hue, Role};
use crate::st_view::brand::{GREEN, SELECT, STEEL, YELLOW};
use crate::theme::Theme;

/// The look of one theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub dark: bool,
    /// A tile's corner radius and the height it stands, world units.
    pub radius: f32,
    pub tile_h: f32,
    /// How far a tile is drawn inside its box, so tiles have air round them.
    pub inset: f32,
    /// How many steps the side of a tile is drawn in.
    pub layers: usize,
    /// The soft shadow under a tile: how blurred and how dark.
    pub shadow_sigma: f32,
    pub shadow_opacity: f32,
    /// How much light a lamp, a live wire and a live tile give off.
    pub glow: f32,
    /// A wire's width and a link's, in points.
    pub wire_px: f32,
    pub link_px: f32,
    pub floor: [f32; 3],
    pub tile: [f32; 3],
    pub accent: [f32; 3],
    /// The idle wire.
    pub idle: [f32; 3],
}

fn rgb(c: Color32) -> [f32; 3] {
    let [r, g, b, _] = c.to_array();
    [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn grey(v: f32) -> [f32; 3] {
    [v, v, v]
}

impl Look {
    #[must_use]
    pub fn of(t: &Theme) -> Self {
        let bg = rgb(t.bg);
        // The accent: the brand yellow, calmed.
        let accent = if t.dark {
            mix(rgb(YELLOW), grey(0.62), 0.28)
        } else {
            mix(rgb(t.accent), grey(0.45), 0.2)
        };
        Self {
            dark: t.dark,
            radius: 0.3,
            tile_h: 0.36,
            inset: 0.07,
            layers: 6,
            shadow_sigma: 0.2,
            shadow_opacity: if t.dark { 0.5 } else { 0.2 },
            glow: if t.dark { 1.0 } else { 0.5 },
            wire_px: 2.0,
            link_px: 1.6,
            floor: if t.dark {
                mix(bg, grey(0.2), 0.45)
            } else {
                mix(bg, grey(0.9), 0.55)
            },
            tile: if t.dark { grey(0.175) } else { grey(0.985) },
            accent,
            idle: if t.dark { grey(0.3) } else { grey(0.74) },
        }
    }

    /// The top of a tile of `role`: one neutral with a hint of what it is.
    #[must_use]
    pub fn tile_color(&self, role: Role) -> [f32; 3] {
        let base = self.tile;
        let tint = |c: Color32, k: f32| mix(base, rgb(c), k);
        let lift = |k: f32| {
            if self.dark {
                mix(base, grey(1.0), k)
            } else {
                mix(base, grey(0.0), k)
            }
        };
        let c = match role {
            Role::Board => self.floor,
            Role::Read => tint(SELECT, if self.dark { 0.12 } else { 0.09 }),
            Role::Write => tint(YELLOW, if self.dark { 0.1 } else { 0.12 }),
            Role::Op => lift(0.035),
            Role::Func => tint(STEEL, 0.12),
            Role::Fb => mix(base, self.accent, 0.14),
            Role::Const => lift(-0.02),
            Role::Other => lift(0.02),
            Role::Region => mix(self.floor, self.accent, 0.07),
            Role::Button | Role::Lamp => lift(0.05),
            Role::Numeric => tint(SELECT, 0.15),
            Role::Readout => lift(-0.03),
        };
        c.map(|v| v.clamp(0.0, 1.0))
    }

    /// A lamp's colour: what its name says, else the live green.
    #[must_use]
    pub fn hue(h: Hue) -> [f32; 3] {
        match h {
            Hue::Red => [0.90, 0.27, 0.30],
            Hue::Amber => [0.97, 0.66, 0.15],
            Hue::Green | Hue::Live => mix(rgb(GREEN), grey(0.6), 0.1),
            Hue::Blue => [0.29, 0.56, 0.95],
            Hue::White => [0.96, 0.94, 0.90],
        }
    }

    /// How dark the side of a tile is `k` steps up of `layers`.
    #[must_use]
    pub fn side_shade(&self, k: usize) -> f32 {
        let f = k as f32 / self.layers.max(1) as f32;
        if self.dark { 0.6 + 0.4 * f } else { 0.86 + 0.14 * f }
    }
}
