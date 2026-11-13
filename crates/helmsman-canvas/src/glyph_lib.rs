//! The glyph library (Nick, 2026-11-12: the app should speak the way
//! people do, in pictures before words): a picture for each common thing
//! in a plant and a program, drawn beside every point in the I/O tab and
//! the Devices list, and later on the diagram and in a palette.
//!
//! Each glyph is an SVG in `assets/glyphs/` that the build step rasterizes
//! into an alpha mask (`build.rs`; `GLYPHS` below is its list), so the
//! page carries no SVG code and a glyph is redrawn by replacing its file.
//! The app tints a mask in the theme's colours when it paints it, the way
//! the hand-drawn toolbar glyphs (`glyphs`) take theirs.
//!
//! Which glyph a point gets follows its name's words, then its direction
//! and type ([`for_point`]); a block its type ([`for_block`]); a device its
//! driver ([`for_device`]).

use eframe::egui::{
    self, Color32, ColorImage, Id, Rect, Response, Sense, TextureHandle, TextureOptions, Vec2,
};

include!(concat!(env!("OUT_DIR"), "/glyphs.rs"));

/// A mask's side, pixels (the build step draws them so).
pub const SIDE: usize = 64;

/// The glyph names, in order.
#[cfg(test)]
pub fn names() -> impl Iterator<Item = &'static str> {
    GLYPHS.iter().map(|(n, _)| *n)
}

/// The alpha mask of glyph `name`, `SIDE` by `SIDE`, row-major.
#[must_use]
pub fn mask(name: &str) -> Option<&'static [u8]> {
    GLYPHS.iter().find(|(n, _)| *n == name).map(|(_, m)| *m)
}

/// A name's words, lower-cased: `Start_PB` is `start`, `pb`.
fn words(name: &str) -> Vec<String> {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// The glyph for a program point or signal: by the words of its name
/// first (an emergency stop before a push button, since both say
/// `stop`), then a switch for a BOOL input, a pilot lamp for a BOOL
/// output, a number for anything else.
#[must_use]
pub fn for_point(name: &str, input: bool, bool_ty: bool) -> &'static str {
    let w = words(name);
    let has = |list: &[&str]| w.iter().any(|x| list.contains(&x.as_str()));
    let estop = has(&["estop", "emergency"]) || w.windows(2).any(|p| p[0] == "e" && p[1] == "stop");
    if estop {
        "e-stop"
    } else if has(&[
        "pb", "button", "btn", "push", "start", "stop", "reset", "ack", "jog",
    ]) {
        "push-button"
    } else if has(&[
        "sel", "selector", "mode", "auto", "manual", "man", "hand", "local", "remote",
    ]) {
        "selector"
    } else if has(&["lamp", "light", "pilot", "led", "beacon", "indicator"])
        || crate::lamps::colour(name).is_some()
    {
        "pilot-lamp"
    } else if has(&["alarm", "alm", "fault", "trip", "fail"]) {
        "alarm"
    } else if has(&["interlock", "ilk", "permissive", "permit", "ok"]) {
        "interlock"
    } else if has(&["step", "phase", "seq", "sequence", "stage"]) {
        "step"
    } else if has(&["hmi", "screen", "display"]) {
        "hmi"
    } else if has(&["timer", "delay", "ton", "tof"]) {
        "timer"
    } else if has(&["count", "counter", "cnt", "total"]) {
        "counter"
    } else if has(&["latch", "latched", "hold", "held"]) {
        "latch"
    } else if has(&["edge", "trig", "pulse"]) {
        "edge"
    } else if bool_ty {
        if input { "switch" } else { "pilot-lamp" }
    } else {
        "number"
    }
}

/// The glyph for a standard function block, by its type; `None` for a
/// type the library has no picture of. The diagram and the palette (the
/// next steps of the library) draw these; until then tests pin it.
#[cfg(test)]
#[must_use]
pub fn for_block(fb: &str) -> Option<&'static str> {
    Some(match fb.to_ascii_uppercase().as_str() {
        "TON" | "TOF" | "TP" => "timer",
        "CTU" | "CTD" | "CTUD" => "counter",
        "SR" | "RS" => "latch",
        "R_TRIG" | "F_TRIG" => "edge",
        _ => return None,
    })
}

/// The glyph for a device in the Devices list, by its driver: the box's
/// own channels, the screen, or a remote I/O module.
#[must_use]
pub fn for_device(driver: &str) -> &'static str {
    match driver.to_ascii_lowercase().as_str() {
        "box" => "box",
        "screen" => "hmi",
        _ => "module",
    }
}

/// The texture of glyph `name`, uploaded once per context and kept in
/// its memory; `None` for a name the library lacks.
fn texture(ctx: &egui::Context, name: &str) -> Option<TextureHandle> {
    let id = Id::unique(("glyph-lib", name));
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(t);
    }
    let mask = mask(name)?;
    let mut rgba = Vec::with_capacity(mask.len() * 4);
    for &a in mask {
        rgba.extend_from_slice(&[255, 255, 255, a]);
    }
    let image = ColorImage::from_rgba_unmultiplied([SIDE, SIDE], &rgba);
    let t = ctx.load_texture(format!("glyph:{name}"), image, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    Some(t)
}

/// Paints glyph `name` filling `rect` in `color`; false (and nothing
/// painted) for a name the library lacks.
pub fn paint(painter: &egui::Painter, rect: Rect, name: &str, color: Color32) -> bool {
    let Some(t) = texture(painter.ctx(), name) else {
        return false;
    };
    painter.image(
        t.id(),
        rect,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        color,
    );
    true
}

/// Glyph `name` as a `size`-point square in a row, in `color`, with
/// `tip` on hover: what a list draws beside a point's name.
pub fn show(ui: &mut egui::Ui, name: &str, size: f32, color: Color32, tip: &str) -> Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint(ui.painter(), rect, name, color);
    }
    r.on_hover_text(tip)
}

/// What a glyph stands for, for a tooltip.
#[must_use]
pub fn tip(name: &str) -> &'static str {
    match name {
        "push-button" => "Push button",
        "e-stop" => "Emergency stop",
        "selector" => "Selector switch",
        "pilot-lamp" => "Pilot lamp",
        "hmi" => "Screen",
        "switch" => "Switch",
        "number" => "A value",
        "signal" => "A signal",
        "timer" => "Timer",
        "counter" => "Counter",
        "latch" => "Latch",
        "interlock" => "Interlock",
        "alarm" => "Alarm",
        "step" => "Sequence step",
        "edge" => "Edge",
        "module" => "I/O module",
        "box" => "This controller",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_file_became_a_mask_with_ink_and_room() {
        let names: Vec<&str> = names().collect();
        assert!(names.len() >= 17, "{names:?}");
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "sorted and unique");
        for n in &names {
            let m = mask(n).unwrap();
            assert_eq!(m.len(), SIDE * SIDE, "{n}");
            let ink = m.iter().filter(|&&a| a > 128).count();
            let share = ink as f32 / m.len() as f32;
            assert!(
                (0.04..0.75).contains(&share),
                "{n}: {share:.2} of the square is ink"
            );
            assert!(!tip(n).is_empty(), "{n} has a tooltip");
        }
    }

    #[test]
    fn a_points_glyph_follows_its_names_words_then_its_kind() {
        assert_eq!(for_point("START_PB", true, true), "push-button");
        assert_eq!(for_point("stop", true, true), "push-button");
        assert_eq!(for_point("E_STOP", true, true), "e-stop");
        assert_eq!(for_point("estop_ok", true, true), "e-stop");
        assert_eq!(for_point("MODE_SEL", true, true), "selector");
        assert_eq!(for_point("green", false, true), "pilot-lamp");
        assert_eq!(for_point("run_lamp", false, true), "pilot-lamp");
        assert_eq!(for_point("HIGH_LEVEL_ALARM", false, true), "alarm");
        assert_eq!(for_point("guard_interlock", true, true), "interlock");
        assert_eq!(for_point("PHASE", false, false), "step");
        assert_eq!(for_point("run", true, true), "switch");
        assert_eq!(for_point("motor", false, true), "pilot-lamp");
        assert_eq!(for_point("speed", true, false), "number");
        assert_eq!(for_point("TEMP", false, false), "number");
        // Every answer is a glyph the library holds.
        for n in ["START_PB", "E_STOP", "x", "ALARM_1", "count_total", "edge_in"] {
            assert!(mask(for_point(n, true, true)).is_some(), "{n}");
        }
    }

    #[test]
    fn blocks_and_devices_have_their_glyphs() {
        assert_eq!(for_block("TON"), Some("timer"));
        assert_eq!(for_block("ctud"), Some("counter"));
        assert_eq!(for_block("R_TRIG"), Some("edge"));
        assert_eq!(for_block("MOTOR"), None);
        assert_eq!(for_device("box"), "box");
        assert_eq!(for_device("screen"), "hmi");
        assert_eq!(for_device("modbus-tcp"), "module");
        for g in ["timer", "counter", "latch", "edge", "box", "hmi", "module"] {
            assert!(mask(g).is_some(), "{g}");
        }
    }

    #[test]
    fn a_glyph_paints_as_one_tinted_image_and_an_unknown_name_paints_nothing() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let r = show(ui, "timer", 20.0, Color32::RED, "Timer");
            assert_eq!(r.rect.size(), Vec2::splat(20.0));
            assert!(!paint(ui.painter(), r.rect, "no-such-glyph", Color32::RED));
        });
        // No renderer here to upload the glyph's texture to.
        out.textures_delta.clear();
        let images: Vec<Color32> = out
            .shapes
            .iter()
            .flat_map(|c| match &c.shape {
                egui::Shape::Vec(v) => v.clone(),
                s => vec![s.clone()],
            })
            // An image is a mesh on a texture other than the font atlas.
            .filter_map(|s| match s {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => {
                    m.vertices.first().map(|v| v.color)
                }
                _ => None,
            })
            .collect();
        assert_eq!(images, [Color32::RED]);
    }
}
