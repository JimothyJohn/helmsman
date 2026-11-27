//! A side pane's hide button on a thin ribbon along its inner edge (Nick,
//! 2026-11-26): the button at the pane's vertical middle; shut, the strip's
//! expand glyph at the same height. Left and right, driven by the pointer.

use eframe::egui::{Pos2, Rect, Shape};

use super::{App, Harness};
use crate::editor::Pane;

/// The bounds of the strokes and fills drawn inside `area` that are no
/// wider than a glyph (the pane's own background and borders are left out).
fn glyph_bounds(h: &Harness, area: Rect) -> Rect {
    fn walk(s: &Shape, area: Rect, out: &mut Rect) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, area, out)),
            Shape::LineSegment { points, .. } if points.iter().all(|p| area.contains(*p)) => {
                *out = out.union(Rect::from_two_pos(points[0], points[1]));
            }
            Shape::Rect(r)
                if area.contains_rect(r.rect) && r.rect.width() < 30.0 && r.rect.height() < 30.0 =>
            {
                *out = out.union(r.rect);
            }
            _ => {}
        }
    }
    let mut out = Rect::NOTHING;
    for c in &h.shapes {
        walk(&c.shape, area, &mut out);
    }
    out
}

/// The simulation with the I/O tab docked right: a pane on each side.
fn both() -> Harness {
    let mut h = Harness::new(App::sim());
    h.app.chrome.dock(Pane::Io, true);
    h.wait(0.3);
    assert!(h.app.chrome.left && h.app.chrome.right);
    h
}

#[test]
fn the_left_panes_hide_button_is_on_a_thin_ribbon_at_its_inner_edge_and_vertical_middle() {
    let mut h = both();
    let top = h.rect("header").bottom();
    let middle = f32::midpoint(top, h.size.y);
    let button = h.rect("left");
    assert!(button.width() <= 24.0, "a thin ribbon: {button:?}");
    assert!(
        (button.center().y - middle).abs() <= 2.0,
        "at the pane's middle {middle}: {button:?}"
    );
    let edge = h.app.chrome.diagram.left();
    assert!(
        button.right() <= edge && edge - button.right() <= 12.0,
        "along the inner edge {edge}: {button:?}"
    );
    let open_glyph = glyph_bounds(&h, button.shrink(0.5));
    assert!(open_glyph.is_positive(), "a glyph on the button");
    // Shut: the strip's expand glyph at the same height.
    h.press("left");
    h.wait(0.5);
    assert!(!h.app.chrome.left, "the ribbon's button hides the pane");
    let strip = h.rect("left");
    let tabs_bottom = h
        .app
        .chrome
        .hits
        .iter()
        .filter(|(k, r)| {
            (k.starts_with("pane ") || k.starts_with("tab ")) && r.right() <= strip.right() + 1.0
        })
        .map(|(_, r)| r.bottom())
        .fold(top, f32::max);
    let below_tabs = Rect::from_min_max(
        Pos2::new(strip.left() - 8.0, tabs_bottom + 1.0),
        Pos2::new(strip.right() + 8.0, h.size.y),
    );
    let shut_glyph = glyph_bounds(&h, below_tabs);
    assert!(shut_glyph.is_positive(), "the strip has its expand glyph");
    assert!(
        (shut_glyph.center().y - open_glyph.center().y).abs() <= 2.0,
        "the same height shut as open: {shut_glyph:?} vs {open_glyph:?}"
    );
    // A click on the strip brings the pane back.
    h.click(shut_glyph.center());
    h.wait(0.5);
    assert!(h.app.chrome.left, "the strip shows the pane again");
    assert!((h.rect("left").center().y - middle).abs() <= 2.0);
}

#[test]
fn the_right_panes_hide_button_is_on_its_ribbon_at_its_middle_and_the_shut_glyph_matches() {
    let mut h = both();
    let top = h.rect("header").bottom();
    let button = h.rect("right");
    let pane = h.rect("tab io");
    assert!(button.width() <= 24.0, "a thin ribbon: {button:?}");
    assert!(
        button.right() <= pane.left() + 1.0,
        "along the pane's inner edge: {button:?} {pane:?}"
    );
    // The right pane ends where the bottom panel starts.
    let bottom = h.rect("bottom trend").top();
    let middle = f32::midpoint(top, bottom);
    assert!(
        (button.center().y - middle).abs() <= 12.0,
        "at the pane's middle {middle}: {button:?}"
    );
    let open_glyph = glyph_bounds(&h, button.shrink(0.5));
    h.press("right");
    h.wait(0.5);
    assert!(!h.app.chrome.right);
    let strip = h.rect("right");
    let below = Rect::from_min_max(
        Pos2::new(strip.left() - 8.0, h.rect("tab io").bottom() + 1.0),
        Pos2::new(h.size.x, strip.bottom()),
    );
    let shut_glyph = glyph_bounds(&h, below);
    assert!(shut_glyph.is_positive(), "the strip has its expand glyph");
    assert!(
        (shut_glyph.center().y - open_glyph.center().y).abs() <= 2.0,
        "the same height shut as open: {shut_glyph:?} vs {open_glyph:?}"
    );
}
