//! The UI polish brief's "Make it stick" (Nick, 2026-11-27; docs/UI.md
//! "Look and feel", "Held by tests, not taste"): what the last frame drew,
//! read shape by shape, so the finish cannot quietly regress.
//!
//! - no glyph is clipped by its widget or its block (finish 1 and 8);
//! - no text on the canvas overlaps another block or label (finish 2);
//! - no error or warning colour on a zero count (finish 3);
//! - no brand yellow on a signal's state: wires, chips, I/O cards and
//!   the Signals lanes (decision 3, "TRUE is green, never yellow").

use eframe::egui::{self, Color32, Rect, Shape};

use super::{App, Harness, sim_view};
use crate::editor::Pane;
use crate::st_view::brand;
use crate::theme::Theme;

/// A piece of text the last frame drew, in paint order.
#[derive(Clone, Debug)]
struct Seen {
    text: String,
    rect: Rect,
    clip: Rect,
    color: Color32,
    elided: bool,
    /// Its face and size (its first section's).
    font: Option<egui::FontId>,
    /// Its place in the frame's paint order.
    order: usize,
}

/// A coloured mark the last frame drew: its colour and where.
#[derive(Clone, Copy, Debug)]
struct Paint {
    color: Color32,
    at: Rect,
}

/// Every piece of text the last frame drew, in paint order.
fn texts(h: &Harness) -> Vec<Seen> {
    fn walk(s: &Shape, clip: Rect, out: &mut Vec<Seen>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, out)),
            Shape::Text(t) => {
                let text = t.galley.text().trim().to_string();
                if text.is_empty() {
                    return;
                }
                let color = t
                    .override_text_color
                    .or_else(|| t.galley.job.sections.first().map(|s| s.format.color))
                    .unwrap_or(t.fallback_color);
                let order = out.len();
                out.push(Seen {
                    text,
                    rect: t.galley.rect.translate(t.pos.to_vec2()),
                    clip,
                    color: color.gamma_multiply(t.opacity_factor),
                    elided: t.galley.elided,
                    font: t.galley.job.sections.first().map(|s| s.format.font_id.clone()),
                    order,
                });
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, c.clip_rect, &mut out);
    }
    out
}

/// Every colour the last frame painted with, and where (a mesh's
/// vertices one by one, so a gradient's colours each count).
fn paints(h: &Harness) -> Vec<Paint> {
    paints_of(&h.shapes)
}

/// [`paints`] of `shapes`.
fn paints_of(shapes: &[egui::epaint::ClippedShape]) -> Vec<Paint> {
    let mut out = Vec::new();
    for c in shapes {
        paint_shape(&c.shape, c.clip_rect, &mut out);
    }
    out
}

/// A path's stroke colour when it is one colour.
fn solid(c: &egui::epaint::ColorMode) -> Color32 {
    match c {
        egui::epaint::ColorMode::Solid(c) => *c,
        egui::epaint::ColorMode::UV(_) => Color32::TRANSPARENT,
    }
}

/// The colours of `s` (a mesh's vertices one by one) and where, inside
/// `clip`.
fn paint_shape(s: &Shape, clip: Rect, out: &mut Vec<Paint>) {
    if let Shape::Vec(v) = s {
        for s in v {
            paint_shape(s, clip, out);
        }
        return;
    }
    let at = s.visual_bounding_rect().intersect(clip);
    let mut put = |color: Color32, at: Rect| {
        if color.a() > 8 && at.is_positive() {
            out.push(Paint { color, at });
        }
    };
    match s {
        Shape::Circle(c) => {
            put(c.fill, at);
            put(c.stroke.color, at);
        }
        Shape::Ellipse(e) => {
            put(e.fill, at);
            put(e.stroke.color, at);
        }
        Shape::LineSegment { stroke, .. } => put(stroke.color, at),
        Shape::Path(p) => {
            put(p.fill, at);
            put(solid(&p.stroke.color), at);
        }
        Shape::Rect(r) => {
            put(r.fill, at);
            put(r.stroke.color, at);
        }
        Shape::Text(t) => {
            for sec in &t.galley.job.sections {
                put(t.override_text_color.unwrap_or(sec.format.color), at);
            }
        }
        Shape::Mesh(m) => {
            for v in &m.vertices {
                let p = Rect::from_center_size(v.pos, egui::Vec2::splat(1.0)).intersect(clip);
                put(v.color, p);
            }
        }
        Shape::QuadraticBezier(q) => {
            put(q.fill, at);
            put(solid(&q.stroke.color), at);
        }
        Shape::CubicBezier(c) => {
            put(c.fill, at);
            put(solid(&c.stroke.color), at);
        }
        Shape::Band(b) => {
            put(b.fill, at);
            put(b.stroke.color, at);
        }
        _ => {}
    }
}
/// `c` without its alpha premultiplied in.
fn plain(c: Color32) -> [u8; 3] {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    [r, g, b]
}

/// Within `by` a channel of `want` (alpha left out).
fn near(c: Color32, want: Color32, by: u8) -> bool {
    let (a, b) = (plain(c), plain(want));
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= by)
}

/// A grey: no hue to speak of.
fn grey(c: Color32) -> bool {
    let [r, g, b] = plain(c);
    r.max(g).max(b) - r.min(g).min(b) <= 20
}

/// The blocks of the simulation's diagram as drawn: label and screen rect.
fn blocks(h: &Harness) -> Vec<(String, Rect)> {
    let v = sim_view(h);
    let cam = v.cams[v.scope];
    v.shown()
        .nodes
        .iter()
        .map(|n| {
            let r = Rect::from_min_max(
                cam.to_screen(n.rect.x, n.rect.y),
                cam.to_screen(n.rect.right(), n.rect.bottom()),
            );
            (n.label.clone(), r)
        })
        .collect()
}

/// The port names of each block of [`blocks`], in the same order.
fn port_names(h: &Harness) -> Vec<Vec<String>> {
    sim_view(h)
        .shown()
        .nodes
        .iter()
        .map(|n| n.ins.iter().chain(&n.outs).cloned().collect())
        .collect()
}

/// The clip of the diagram's own drawing (the canvas under the crumbs):
/// the one most block labels are drawn with.
fn canvas_clip(h: &Harness, all: &[Seen]) -> Rect {
    let labels: Vec<String> = blocks(h).into_iter().map(|(l, _)| l).collect();
    let mut clips: Vec<(Rect, usize)> = Vec::new();
    for s in all.iter().filter(|s| labels.contains(&s.text)) {
        match clips.iter_mut().find(|(c, _)| *c == s.clip) {
            Some((_, n)) => *n += 1,
            None => clips.push((s.clip, 1)),
        }
    }
    clips
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(c, _)| c)
        .expect("block labels on the canvas")
}

/// The texts drawn on the canvas itself (the diagram's layer, not the
/// glyphs and crumbs floating over it), duplicates painted twice at one
/// place counted once.
fn canvas_texts(h: &Harness) -> Vec<Seen> {
    let all = texts(h);
    let clip = canvas_clip(h, &all);
    let mut out: Vec<Seen> = Vec::new();
    for s in all.into_iter().filter(|s| s.clip == clip) {
        if !out.iter().any(|o| o.text == s.text && o.rect == s.rect) {
            out.push(s);
        }
    }
    out
}

/// The block `s` is drawn in: the smallest that holds its centre.
fn owner(blocks: &[(String, Rect)], s: &Seen) -> Option<usize> {
    blocks
        .iter()
        .enumerate()
        .filter(|(_, (_, r))| r.contains(s.rect.center()))
        .min_by(|a, b| a.1.1.area().total_cmp(&b.1.1.area()))
        .map(|(i, _)| i)
}

/// The simulation running `src` (or a built-in project) for a while, the
/// I/O tab docked right and the Trend shown, so every surface that draws
/// a signal's state is on screen.
fn running(src: Option<&str>, light: bool) -> Harness {
    let mut h = Harness::new(App::sim());
    h.app.chrome.light = light;
    h.app.chrome.dock(Pane::Io, true);
    h.app.chrome.trend = true;
    {
        let v = h.app.sim.as_mut().unwrap();
        match src {
            Some(src) => v.load(src, "test"),
            None => v.open_project(crate::front::PROJECT),
        }
        v.session.set_running(true);
    }
    h.wait(0.3);
    h.press("tab io");
    // Long enough for the tank to fill and for timers to run.
    h.wait(3.0);
    let s = &h.app.sim.as_ref().unwrap().session;
    assert!(s.running, "running: {:?} {:?}", s.fault, s.diag_lines());
    h
}

/// The programs the canvas checks run on: the tank, every example.
fn programs() -> Vec<(&'static str, Option<&'static str>)> {
    let mut out = vec![("tank", None)];
    out.extend(crate::playground::EXAMPLES.iter().map(|e| (e.name, Some(e.src))));
    out
}

fn dump(all: &[Seen]) -> String {
    all.iter()
        .map(|s| format!("{:?} {:?}", s.text, s.rect))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- (a) nothing clipped ---------------------------------------------------

/// Finish 1 and 8: a block holds all its text (it sizes to it), nothing
/// on the canvas is cut to an ellipsis, an I/O card's value is never cut
/// and its name is cut only where the card has no room for it, with the
/// whole name on hover (docs/UI.md "An I/O card at rest is its point",
/// finish 1), a text in a widget is not cut at the side by the widget's
/// clip, and a port's name reads on its block, on the tank and every
/// example, running.
#[test]
fn no_glyph_is_clipped_by_its_widget_or_its_block() {
    let mut problems = Vec::new();
    let mut checked = 0;
    for (name, src) in programs() {
        let mut h = running(src, false);
        let bl = blocks(&h);
        let canvas = canvas_texts(&h);
        let clip = canvas_clip(&h, &texts(&h));
        for s in &canvas {
            if s.elided {
                problems.push(format!(
                    "{name}: {:?} is cut to an ellipsis on the canvas",
                    s.text
                ));
            }
            if let Some(i) = owner(&bl, s) {
                let (label, b) = &bl[i];
                if !b.expand(1.0).contains_rect(s.rect) {
                    problems.push(format!(
                        "{name}: {:?} {:?} runs out of its block {label:?} {b:?}",
                        s.text, s.rect
                    ));
                }
            }
        }
        // Widgets anywhere: a text whole in its clip's height (so not a
        // row scrolled half out of view) but cut at either side. The
        // canvas's own texts are the block check's above; a block at the
        // canvas's edge may be panned out of view.
        for s in texts(&h).iter().filter(|s| s.clip != clip) {
            let whole_height = s.rect.top() >= s.clip.top() - 0.5 && s.rect.bottom() <= s.clip.bottom() + 0.5;
            let cut = s.rect.left() < s.clip.left() - 0.5 || s.rect.right() > s.clip.right() + 0.5;
            let seen = s.rect.intersects(s.clip);
            if whole_height && cut && seen {
                problems.push(format!(
                    "{name}: {:?} {:?} is cut at the side by its clip {:?}",
                    s.text, s.rect, s.clip
                ));
            }
        }
        // Finish 8: a port's name stands out from what is under it: the
        // page and every filled rect painted under the text's centre before
        // it, laid on in paint order. 3 to 1, WCAG's floor for a part of
        // the interface that must be seen.
        let grounds = grounds(&h);
        let page = Theme::dark().bg;
        let ports = port_names(&h);
        for s in &canvas {
            let Some(i) = owner(&bl, s) else { continue };
            if !ports[i].iter().any(|p| p.eq_ignore_ascii_case(&s.text)) {
                continue;
            }
            checked += 1;
            let ground = grounds
                .iter()
                .filter(|(order, r, _)| *order <= s.order && r.contains(s.rect.center()))
                .fold(page, |under, (_, _, fill)| over(*fill, under));
            let ink = over(s.color, ground);
            let k = crate::theme::contrast(plain(ink), plain(ground));
            if k < 3.0 {
                problems.push(format!(
                    "{name}: {:?} {:?} reads {k:.2} to 1 on its block ({:?} on {:?})",
                    s.text,
                    s.rect,
                    plain(ink),
                    plain(ground)
                ));
            }
        }
        // Last: it moves the pointer, so the frame the checks above read
        // is not the one it leaves.
        problems.extend(io_cards_cut(&mut h, name));
    }
    problems.dedup();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(checked > 10, "only {checked} port names were read");
}

/// docs/UI.md, "An I/O card at rest is its point" and finish 1: on each
/// I/O card of the right pane, with the pointer off the cards, a value
/// (the live number in the code face) is whole and on its card; a name cut
/// to an ellipsis had no room (laid out whole in its own face it would run
/// past the value less the card's own spacing, or past the card's end less
/// its own margin, where there is no value; both measured on the card: the
/// gap between its icon and its name, and its icon's inset from its left
/// edge); and with the pointer on the card the whole name is drawn, uncut. Short of room is the one reason a name is cut, so a card that
/// keeps room for glyphs that are not shown (they are drawn over the line
/// on hover) cuts a name it need not.
fn io_cards_cut(h: &mut Harness, program: &str) -> Vec<String> {
    let mut problems = Vec::new();
    h.frame(vec![egui::Event::PointerMoved(h.rect("tab io").center())]);
    h.wait(0.5);
    let cards: Vec<(String, Rect)> = h
        .app
        .chrome
        .hits
        .iter()
        .filter_map(|(k, r)| k.strip_prefix("io card ").map(|p| (p.to_string(), *r)))
        .collect();
    let mut cut_names = Vec::new();
    let all = texts(h);
    for (point, card) in &cards {
        // The card's first line: the texts level with its name.
        let on: Vec<&Seen> = all.iter().filter(|s| card.contains(s.rect.center())).collect();
        let Some(label) = on.iter().find(|s| s.text.eq_ignore_ascii_case(point)) else {
            continue;
        };
        let line = |s: &&&Seen| (s.rect.center().y - label.rect.center().y).abs() < label.rect.height() / 2.0;
        let values: Vec<&&Seen> = on
            .iter()
            .filter(line)
            .filter(|s| {
                s.font
                    .as_ref()
                    .is_some_and(|f| f.family == egui::FontFamily::Monospace)
            })
            .collect();
        for v in &values {
            if v.elided || !card.contains_rect(v.rect) {
                problems.push(format!(
                    "{program}: the I/O card of {point:?} cuts its value {:?} ({:?} on {card:?})",
                    v.text, v.rect
                ));
            }
        }
        if !label.elided {
            continue;
        }
        let icon = super::io_cards::icon(h, point);
        let gap = label.rect.left() - icon.right();
        let margin = icon.left() - card.left();
        let end = values
            .iter()
            .map(|v| v.rect.left() - gap)
            .fold(card.right() - margin, f32::min);
        let font = label.font.clone().expect("a name has a face");
        let whole = h.ctx.fonts_mut(|f| {
            f.layout_no_wrap(label.text.clone(), font, Color32::WHITE)
                .size()
                .x
        });
        if label.rect.left() + whole <= end {
            problems.push(format!(
                "{program}: the I/O card of {point:?} cuts {:?} to {:.0} pt though it has {:.0} pt for its {whole:.0}",
                label.text,
                label.rect.width(),
                end - label.rect.left()
            ));
        }
        cut_names.push((point.clone(), label.rect));
    }
    // The whole of each cut name on hover.
    for (point, at) in cut_names {
        h.frame(vec![egui::Event::PointerMoved(at.center())]);
        h.wait(1.5);
        let whole = point.to_lowercase();
        if !texts(h)
            .iter()
            .any(|s| s.text.to_lowercase().contains(&whole) && !s.elided)
        {
            problems.push(format!(
                "{program}: hovering the I/O card of {point:?} does not show its whole name"
            ));
        }
        h.frame(vec![egui::Event::PointerMoved(h.rect("tab io").center())]);
        h.wait(0.5);
    }
    problems
}

/// The filled rects the last frame painted, in paint order, each with
/// the number of texts painted before it (a text's ground has a number
/// no higher than the text's own) and its fill.
fn grounds(h: &Harness) -> Vec<(usize, Rect, Color32)> {
    fn walk(s: &Shape, clip: Rect, n: &mut usize, out: &mut Vec<(usize, Rect, Color32)>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, n, out)),
            Shape::Text(t) if !t.galley.text().trim().is_empty() => *n += 1,
            Shape::Rect(r) if r.fill.a() > 0 => out.push((*n, r.rect.intersect(clip), r.fill)),
            _ => {}
        }
    }
    let mut n = 0;
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, c.clip_rect, &mut n, &mut out);
    }
    out
}

/// `top` painted over the opaque `under`.
fn over(top: Color32, under: Color32) -> Color32 {
    let [r, g, b, a] = top.to_array();
    let [ur, ug, ub, _] = under.to_array();
    let k = u16::from(255 - a);
    #[allow(clippy::cast_possible_truncation)]
    let mix = |t: u8, u: u8| (u16::from(t) + u16::from(u) * k / 255).min(255) as u8;
    Color32::from_rgb(mix(r, ur), mix(g, ug), mix(b, ub))
}

// ---- (b) nothing collides --------------------------------------------------

/// Finish 2: no text on the canvas lies over a block it is not drawn in
/// (a value badge, a SIM or device tag, a section's title), nor over
/// another text it is not hidden under, and no block has ghost copies of
/// itself behind it, on every program, at the fit the page opens with.
/// The Signals pane's texts do not overprint each other either.
#[test]
fn no_text_on_the_canvas_overlaps_another_block_or_label() {
    let mut problems = Vec::new();
    for (name, src) in programs() {
        let h = running(src, false);
        let bl = blocks(&h);
        let canvas = canvas_texts(&h);
        let grounds = grounds(&h);
        // No ghost of a block: a copy of its outline or ground, its size,
        // a few points off it (finish 2: "stacked ghost outlines behind
        // blocks").
        for (label, b) in &bl {
            for (_, g, _) in &grounds {
                let (dx, dy) = (g.left() - b.left(), g.top() - b.top());
                let same = (g.width() - b.width()).abs() < 1.0 && (g.height() - b.height()).abs() < 1.0;
                let off = |d: f32| d.abs() >= 1.0 && d.abs() <= 12.0;
                if same && off(dx) && off(dy) {
                    problems.push(format!("{name}: a ghost of {label:?} {b:?} at {g:?}"));
                }
            }
        }
        for (i, s) in canvas.iter().enumerate() {
            let mine = owner(&bl, s);
            let r = s.rect.shrink(0.5);
            for (k, (label, b)) in bl.iter().enumerate() {
                // A block inside the one the text is in (a fold's pins).
                let inside_mine = mine.is_some_and(|m| bl[m].1.contains_rect(*b));
                if Some(k) != mine && !inside_mine && r.intersects(*b) {
                    problems.push(format!(
                        "{name}: {:?} {:?} lies over the block {label:?} {b:?}",
                        s.text, s.rect
                    ));
                }
            }
            for t in &canvas[i + 1..] {
                if r.intersects(t.rect.shrink(0.5)) && !covered(&grounds, s, t) {
                    problems.push(format!(
                        "{name}: {:?} {:?} overprints {:?} {:?}",
                        s.text, s.rect, t.text, t.rect
                    ));
                }
            }
        }
        // The Signals pane is read the same way: its lane labels, values and
        // axis labels (TRUE and FALSE on a BOOL lane) do not overprint.
        let plot = h.rect("trend plot");
        let lanes: Vec<Seen> = texts(&h)
            .into_iter()
            .filter(|s| plot.contains(s.rect.center()))
            .collect();
        for (i, s) in lanes.iter().enumerate() {
            for t in &lanes[i + 1..] {
                if s.rect.shrink(0.5).intersects(t.rect.shrink(0.5)) && !covered(&grounds, s, t) {
                    problems.push(format!(
                        "{name}: in the Signals pane {:?} {:?} overprints {:?} {:?}",
                        s.text, s.rect, t.text, t.rect
                    ));
                }
            }
        }
        if name == "tank" {
            assert!(
                canvas.iter().any(|s| s.text == "LEVEL") && bl.len() > 10,
                "the tank is drawn: {}",
                dump(&canvas)
            );
        }
    }
    problems.dedup();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Whether an opaque ground painted between texts `a` and `b` hides the
/// earlier one whole, so only the later one is seen (an input's switch
/// drawn over the value under it). One cut only in part still collides.
fn covered(grounds: &[(usize, Rect, Color32)], a: &Seen, b: &Seen) -> bool {
    let (lo, hi) = if a.order < b.order { (a, b) } else { (b, a) };
    grounds.iter().any(|(n, r, fill)| {
        *n > lo.order && *n <= hi.order && fill.a() == 255 && r.expand(0.5).contains_rect(lo.rect)
    })
}

// ---- (c) no alarm colour at zero -------------------------------------------

/// The Messages' counters and the bottom panel's tab, where a count shows.
const COUNTERS: [&str; 4] = [
    "messages info",
    "messages warnings",
    "messages errors",
    "bottom messages",
];

/// A level's colour: anything but a grey and the brand yellow that marks
/// what is selected or switched on (a filter that is on, the tab shown).
fn level_colour(c: Color32, theme: Theme) -> bool {
    !grey(c) && !near(c, brand::YELLOW, 40) && !near(c, theme.accent, 24)
}

/// The colours painted inside the hook `key`'s rect.
fn colours_at(h: &Harness, key: &str) -> Vec<Color32> {
    let r = h.rect(key);
    paints(h)
        .into_iter()
        .filter(|p| r.contains_rect(p.at))
        .map(|p| p.color)
        .collect()
}

/// Finish 3: a counter at zero has no level colour (no red ring round
/// `⊗ 0`): grey, and the brand yellow of a filter switched on or the tab
/// shown; once its count is not zero it takes its level's colour, which
/// shows the check can see one. Dark and light.
#[test]
fn no_error_or_warning_colour_on_a_zero_count() {
    for light in [false, true] {
        let theme = Theme::for_mode(!light);
        let mut h = Harness::new(App::sim());
        h.app.chrome.light = light;
        h.messages();
        h.app.sim.as_mut().unwrap().log.clear();
        h.wait(0.3);
        assert_eq!(h.app.sim.as_ref().unwrap().log.counts(), [0; 3]);
        for key in COUNTERS {
            let coloured: Vec<_> = colours_at(&h, key)
                .into_iter()
                .filter(|c| level_colour(*c, theme))
                .collect();
            assert!(
                coloured.is_empty(),
                "light {light}: {key} at zero is coloured {:?}",
                coloured.iter().map(|c| plain(*c)).collect::<Vec<_>>()
            );
        }
        // Not zero: the warning's and the error's own colours.
        {
            let log = &mut h.app.sim.as_mut().unwrap().log;
            log.warning(1.0, "a value is never read");
            log.error(2.0, "deploy refused: no engineer session");
        }
        // Past the error's pulse, so its resting colour is what is seen.
        h.wait(3.0);
        for key in ["messages warnings", "messages errors"] {
            assert!(
                colours_at(&h, key).iter().any(|c| level_colour(*c, theme)),
                "light {light}: {key} with one is coloured: {:?}",
                colours_at(&h, key).iter().map(|c| plain(*c)).collect::<Vec<_>>()
            );
        }
        // Cleared again: grey again.
        h.app.sim.as_mut().unwrap().log.clear();
        h.wait(3.0);
        for key in COUNTERS {
            assert!(
                !colours_at(&h, key).iter().any(|c| level_colour(*c, theme)),
                "light {light}: {key} cleared is grey again"
            );
        }
    }
}

// ---- (d) no yellow on a signal ---------------------------------------------

/// The marks on the canvas that can carry a signal's state: everything but
/// the rects as wide as a block they lie on (its body, its head, the stack
/// behind a fold), which say what kind of block it is and never change
/// with a value. Wires, their glow and dots, chips, values and ports stay.
fn signal_marks_on_canvas(h: &Harness, canvas: Rect) -> Vec<Paint> {
    fn walk(s: &Shape, clip: Rect, canvas: Rect, bl: &[Rect], out: &mut Vec<Paint>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, canvas, bl, out)),
            Shape::Rect(r)
                if bl
                    .iter()
                    .any(|b| b.intersects(r.rect) && r.rect.width() >= 0.9 * b.width()) => {}
            _ if clip == canvas => {
                paint_shape(s, clip, out);
            }
            _ => {}
        }
    }
    let bl: Vec<Rect> = blocks(h).into_iter().map(|(_, r)| r).collect();
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, c.clip_rect, canvas, &bl, &mut out);
    }
    out
}

/// Decision 3: yellow is brand, selection and focus only. With nothing
/// selected or focused, no wire, chip, value or port on the canvas, no
/// I/O card's state and no BOOL lane's trace in the Signals pane is brand
/// yellow (nor, on the light page, its ochre ink), TRUE or FALSE, on the
/// tank and every example. Left out as chrome: the glyphs floating over
/// the diagram and its crumbs, a card's code, diagram and trend glyphs (the
/// trend one is yellow while the point is on the Trend: switched on), and
/// a lane's label (the colour the signal was given, not its state).
#[test]
fn no_yellow_on_any_signal_state() {
    let (mut trues, mut falses) = (0, 0);
    let mut problems: Vec<String> = Vec::new();
    for light in [false, true] {
        for (name, src) in programs() {
            let h = running(src, light);
            let theme = Theme::for_mode(!light);
            let yellow = |c: Color32| near(c, brand::YELLOW, 40) || near(c, theme.accent, 24);
            let canvas = canvas_clip(&h, &texts(&h));
            let hits = &h.app.chrome.hits;
            // The glyphs over the diagram (run, stop, ..., the corner
            // controls) and a card's glyph buttons.
            let chrome: Vec<Rect> = hits
                .iter()
                .filter(|(k, r)| {
                    (canvas.intersects(*r) && !k.starts_with("input ") && !k.starts_with("io "))
                        || ["io code ", "io diagram ", "io trend "]
                            .iter()
                            .any(|g| k.starts_with(g))
                })
                .map(|(_, r)| *r)
                .collect();
            let mut marks: Vec<(String, Paint)> = signal_marks_on_canvas(&h, canvas)
                .into_iter()
                .map(|p| ("the canvas".to_string(), p))
                .collect();
            let all = paints(&h);
            for (k, r) in hits {
                if let Some(point) = k.strip_prefix("io card ") {
                    marks.extend(
                        all.iter()
                            .filter(|p| r.contains(p.at.center()))
                            .map(|p| (format!("the I/O card of {point}"), *p)),
                    );
                }
            }
            // The Signals pane's BOOL lanes: the trace, right of the label.
            if let Some(plot) = h.app.chrome.rect("trend plot") {
                for (k, r) in hits {
                    let Some(sig) = k.strip_prefix("trend lane ") else {
                        continue;
                    };
                    let is_bool = matches!(
                        h.app.sim.as_ref().unwrap().session.value(sig),
                        Some(helmsman_iec::Value::Bool(_))
                    );
                    if !is_bool {
                        continue;
                    }
                    let trace = Rect::from_x_y_ranges(r.right()..=plot.right(), r.y_range());
                    marks.extend(
                        all.iter()
                            .filter(|p| trace.contains(p.at.center()))
                            .map(|p| (format!("the BOOL lane of {sig}"), *p)),
                    );
                }
            }
            let mut found: Vec<String> = marks
                .into_iter()
                .filter(|(_, p)| yellow(p.color) && !chrome.iter().any(|c| c.intersects(p.at)))
                .map(|(what, p)| {
                    format!(
                        "{name}, light {light}: {what}: {:?} at {:?}",
                        plain(p.color),
                        p.at
                    )
                })
                .collect();
            found.dedup();
            problems.extend(found);
            let all = canvas_texts(&h);
            trues += all.iter().filter(|s| s.text == "TRUE").count();
            falses += all.iter().filter(|s| s.text == "FALSE").count();
        }
    }
    assert!(
        problems.is_empty(),
        "yellow on a signal's state:\n{}",
        problems.join("\n")
    );
    // Both states were on screen, so each look was checked.
    assert!(trues > 0 && falses > 0, "TRUE {trues} and FALSE {falses} drawn");
}
