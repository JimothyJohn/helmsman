//! The trend drawn as a monitor draws a pulse (Nick, 2026-11-23: "I want
//! the trend animation to look like an EKG where the pulse fades and
//! trails and it gives it a sense of life"): a write head moves left to
//! right across each lane, the newest sample is a bright point at the
//! head, the trace behind it fades with age, and when the head reaches
//! the right edge it wraps to the left and writes over the oldest trace,
//! leaving a small blank gap ahead of it. The lane reads left to right:
//! the signal's name, the trace, and its live number beside the head.
//!
//! Both trends (the simulation's and a box's) share this module: the
//! sweep's arithmetic ([`Sweep`]), the lane's painting ([`paint_lane`]),
//! the pop a lane makes when it joins ([`Flashes`]) and which pins make
//! room for it ([`pin_auto`]).

use eframe::egui::{self, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke};

use crate::playground_view::{LANE_HANDLE_W, SCALE_W, lane_points, scale_labels};
use crate::theme::Theme;

/// The blank ahead of the head, as a share of the window.
pub const GAP: f64 = 0.04;
/// How many steps of fading a trace is drawn in.
pub const BANDS: usize = 12;
/// How long a lane that just joined the trend pops, seconds of page time.
pub const FLASH_S: f64 = 1.5;

/// One sweep: `window` ms across the lane, the head at `end` ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sweep {
    window: u64,
    end: u64,
}

/// A lane's samples as the sweep lays them out: `(share across the lane,
/// value)`, oldest first. `[0]` is the trace written before the head last
/// wrapped, on the right of the head; `[1]` is the trace written since,
/// on its left.
pub type Runs = [Vec<(f64, f64)>; 2];

impl Sweep {
    /// A sweep of `window_ms` (at least 1) with the head at `end_ms`.
    #[must_use]
    pub fn new(window_ms: u64, end_ms: u64) -> Self {
        Self {
            window: window_ms.max(1),
            end: end_ms,
        }
    }

    /// The time the head last wrapped at: a multiple of the window.
    const fn cycle(&self) -> u64 {
        self.end - self.end % self.window
    }

    /// Where the head is, 0 (left edge) to 1.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn head(&self) -> f64 {
        (self.end % self.window) as f64 / self.window as f64
    }

    /// The oldest time that is drawn: the blank gap is the rest.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn oldest_ms(&self) -> u64 {
        let gap = (self.window as f64 * GAP) as u64;
        self.end.saturating_sub(self.window - gap)
    }

    /// How far across the lane time `t` is.
    #[allow(clippy::cast_precision_loss)]
    fn share(&self, t: u64) -> f64 {
        let c = self.cycle();
        let w = self.window as f64;
        if t >= c {
            (t - c) as f64 / w
        } else {
            1.0 - (c - t) as f64 / w
        }
    }

    /// How old what is drawn at `share` is, as a share of the window:
    /// 0 at the head, up to `1 - GAP` just before the gap.
    #[must_use]
    pub fn age(&self, newer: bool, share: f64) -> f64 {
        let head = self.head();
        (if newer { head - share } else { head + 1.0 - share }).max(0.0)
    }

    /// `samples` (time ms, value; a step function, oldest first, as
    /// the trends keep them) as the two runs of the sweep. The value in
    /// force when the drawn time starts is carried in from an older
    /// sample, the last value is held to the head, a value that is not
    /// finite is left out, and a step that crosses the wrap is cut there:
    /// the old run ends at the right edge, the new one starts at the left
    /// with the same value.
    pub fn runs(&self, samples: impl Iterator<Item = (u64, f64)>) -> Runs {
        let from = self.oldest_ms();
        let mut out: Runs = [Vec::new(), Vec::new()];
        let mut held = None;
        let mut last: Option<(u64, f64)> = None;
        for (t, v) in samples {
            if !v.is_finite() || t > self.end {
                continue;
            }
            if t < from {
                held = Some(v);
                continue;
            }
            if last.is_none()
                && t > from
                && let Some(h) = held
            {
                self.push(&mut out, &mut last, from, h);
            }
            self.push(&mut out, &mut last, t, v);
        }
        if last.is_none()
            && let Some(h) = held
        {
            self.push(&mut out, &mut last, from, h);
        }
        if let Some((t, v)) = last
            && t < self.end
        {
            self.push(&mut out, &mut last, self.end, v);
        }
        out
    }

    fn push(&self, out: &mut Runs, last: &mut Option<(u64, f64)>, t: u64, v: f64) {
        let c = self.cycle();
        if t >= c && out[1].is_empty() {
            // The first sample since the wrap: close the old run at the
            // right edge and open the new one at the left.
            if let Some((_, prev)) = *last {
                if !out[0].is_empty() {
                    out[0].push((1.0, prev));
                }
                if t > c {
                    out[1].push((0.0, prev));
                }
            }
        }
        out[usize::from(t >= c)].push((self.share(t), v));
        *last = Some((t, v));
    }
}

/// How bright a trace is at `age` (the share of the window it was written
/// ago, 0 to `1 - GAP`): full at the head, a faint trail at the gap.
#[must_use]
pub fn fade(age: f64) -> f32 {
    let a = (1.0 - age / (1.0 - GAP)).clamp(0.0, 1.0);
    #[allow(clippy::cast_possible_truncation)]
    {
        (0.10 + 0.90 * a.powf(1.4)) as f32
    }
}

/// `pts` (a trace in time order) cut into runs of one brightness each:
/// `(brightness, points)`, a run starting at the point the one before
/// ended on so the line stays whole. `age_of` is a point's age as
/// [`Sweep::age`] reads it.
#[must_use]
pub fn bands(pts: &[Pos2], age_of: impl Fn(Pos2) -> f64) -> Vec<(f32, Vec<Pos2>)> {
    let mut out: Vec<(usize, Vec<Pos2>)> = Vec::new();
    for &p in pts {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let band = ((age_of(p) / (1.0 - GAP)).clamp(0.0, 0.999) * BANDS as f64) as usize;
        match out.last_mut() {
            Some((b, run)) if *b == band => run.push(p),
            Some((_, run)) => {
                let join = run.last().copied();
                let mut next: Vec<Pos2> = join.into_iter().collect();
                next.push(p);
                out.push((band, next));
            }
            None => out.push((band, vec![p])),
        }
    }
    out.into_iter()
        .map(|(b, run)| {
            #[allow(clippy::cast_precision_loss)]
            let mid = (b as f64 + 0.5) / BANDS as f64 * (1.0 - GAP);
            (fade(mid), run)
        })
        .collect()
}

/// The lanes that just joined the trend, popping for [`FLASH_S`] from the
/// first frame that saw them: timed from the page's clock, so a test sets
/// the time it likes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flashes(Vec<(String, Option<f64>)>);

impl Flashes {
    /// Marks `name` as just pinned.
    pub fn add(&mut self, name: &str) {
        self.0.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.0.push((name.to_string(), None));
    }

    /// Starts the clock of what is new at page time `now`, drops what has
    /// settled, and says whether anything is still popping (the page asks
    /// for another frame only then).
    pub fn tick(&mut self, now: f64) -> bool {
        for (_, at) in &mut self.0 {
            at.get_or_insert(now);
        }
        self.0.retain(|(_, at)| at.is_some_and(|at| now - at < FLASH_S));
        !self.0.is_empty()
    }

    /// How strongly `name`'s lane pops at `now`, 0 to 1.
    #[must_use]
    pub fn strength(&self, name: &str, now: f64) -> f32 {
        self.0
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .and_then(|(_, at)| *at)
            .map_or(0.0, |at| pulse(now - at))
    }
}

/// A pop `age` seconds after it started: full at once, two swells, and
/// nothing from [`FLASH_S`] on.
#[must_use]
pub fn pulse(age: f64) -> f32 {
    if !(0.0..FLASH_S).contains(&age) {
        return 0.0;
    }
    let swell = 0.5 + 0.5 * (age * std::f64::consts::TAU / 0.75).cos();
    #[allow(clippy::cast_possible_truncation)]
    {
        ((1.0 - age / FLASH_S) * (0.25 + 0.75 * swell)) as f32
    }
}

/// Pins `add` onto `signals` (`cap` at most), each at the bottom, as the
/// trend does when a described change brings variables. `auto` is which
/// pins nobody chose; when the trend is full, the oldest of those makes
/// room, and a trend full of the engineer's own pins takes nothing. At
/// most `cap` names are looked at, the last ones first to be dropped.
/// Returns the names that were pinned.
pub fn pin_auto(
    signals: &mut Vec<String>,
    auto: &mut Vec<String>,
    add: &[String],
    cap: usize,
) -> Vec<String> {
    auto.retain(|a| signals.contains(a));
    let mut pinned = Vec::new();
    let skip = add.len().saturating_sub(cap);
    for name in add.iter().skip(skip) {
        if signals.contains(name) {
            continue;
        }
        if signals.len() >= cap {
            let Some(old) = auto.first().cloned() else {
                continue;
            };
            auto.remove(0);
            signals.retain(|s| *s != old);
        }
        signals.push(name.clone());
        auto.push(name.clone());
        pinned.push(name.clone());
    }
    pinned
}

/// What one lane is drawn with.
pub struct Lane<'a> {
    pub name: &'a str,
    pub color: Color32,
    /// The live value as text, beside the head; none with no sample.
    pub number: Option<String>,
    /// How strongly the lane pops, 0 to 1 ([`Flashes::strength`]).
    pub pop: f32,
}

/// The x range of the traces in a trend drawn in `rect`.
#[must_use]
pub fn trace_x(rect: Rect) -> (f32, f32) {
    (
        rect.left() + LANE_HANDLE_W,
        (rect.right() - SCALE_W).max(rect.left() + LANE_HANDLE_W + 1.0),
    )
}

/// One lane of the sweep: `top` and `height` its place in `rect`, `runs`
/// its samples ([`Sweep::runs`]) and `range` its scale (`nice_range`).
/// The name goes at the left, the faded trace in its brightness bands,
/// and at the head a bright point, a faint bar and the live number.
#[allow(clippy::too_many_arguments)]
pub fn paint_lane(
    painter: &Painter,
    t: &Theme,
    rect: Rect,
    (top, height): (f32, f32),
    lane: &Lane,
    sweep: &Sweep,
    runs: &Runs,
    range: (f64, f64, f64),
) {
    let (x0, x1) = trace_x(rect);
    let band = Rect::from_min_max(Pos2::new(rect.left(), top), Pos2::new(rect.right(), top + height));
    if lane.pop > 0.0 {
        painter.rect_filled(band, CornerRadius::ZERO, t.accent.gamma_multiply(0.30 * lane.pop));
    }
    // The name, one row, cut with an ellipsis where it would run on into
    // the trace.
    let mut job = egui::text::LayoutJob::simple_singleline(
        lane.name.to_ascii_lowercase(),
        FontId::monospace(crate::theme::MIN_TEXT),
        if lane.pop > 0.0 {
            t.fg.lerp_to_gamma(lane.color, 1.0 - lane.pop)
        } else {
            lane.color
        },
    );
    job.wrap.max_width = LANE_HANDLE_W - 10.0;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let name = painter.layout_job(job);
    painter.galley(
        Pos2::new(rect.left() + 6.0, top + (height - name.size().y) / 2.0),
        name,
        lane.color,
    );
    painter.line_segment(
        [Pos2::new(rect.left(), top), Pos2::new(rect.right(), top)],
        Stroke::new(1.0, t.rule),
    );
    scale_labels(painter, t, rect, top, height, range);
    let (lo, hi, _) = range;
    let y_of = |v: f64| {
        #[allow(clippy::cast_possible_truncation)]
        let f = ((v - lo) / (hi - lo)) as f32;
        top + height - 4.0 - (height - 8.0) * f.clamp(0.0, 1.0)
    };
    let width = x1 - x0;
    #[allow(clippy::cast_possible_truncation)]
    let x_of = |share: f64| x0 + width * share as f32;
    let head_x = x_of(sweep.head());
    painter.line_segment(
        [
            Pos2::new(head_x, top + 2.0),
            Pos2::new(head_x, top + height - 2.0),
        ],
        Stroke::new(1.0, t.rule),
    );
    for (k, run) in runs.iter().enumerate() {
        let pts = lane_points(run.iter().map(|&(s, v)| (x_of(s), y_of(v))));
        if pts.len() < 2 {
            continue;
        }
        let age_of = |p: Pos2| sweep.age(k == 1, f64::from((p.x - x0) / width));
        for (alpha, seg) in bands(&pts, age_of) {
            if seg.len() > 1 {
                painter.add(egui::Shape::line(
                    seg,
                    Stroke::new(1.8, lane.color.gamma_multiply(alpha)),
                ));
            }
        }
    }
    let Some(&(_, now_v)) = runs[1].last().or_else(|| runs[0].last()) else {
        return;
    };
    let tip = Pos2::new(head_x, y_of(now_v));
    painter.circle_filled(tip, 8.0, lane.color.gamma_multiply(0.25));
    painter.circle_filled(tip, 3.5, lane.color.lerp_to_gamma(Color32::WHITE, 0.55));
    if let Some(number) = &lane.number {
        let galley = painter.layout_no_wrap(
            number.clone(),
            FontId::monospace(crate::theme::MIN_TEXT),
            lane.color,
        );
        let size = galley.size();
        let right = head_x + 12.0 + size.x <= x1 + SCALE_W - 8.0;
        let at_x = if right {
            head_x + 12.0
        } else {
            head_x - 12.0 - size.x
        };
        let at = Pos2::new(at_x, (top + (height - size.y) / 2.0).max(top + 1.0));
        painter.rect_filled(
            Rect::from_min_size(at, size).expand(3.0),
            CornerRadius::same(3),
            t.chart.gamma_multiply(0.88),
        );
        painter.galley(at, galley, lane.color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(window: u64, end: u64, s: &[(u64, f64)]) -> Runs {
        Sweep::new(window, end).runs(s.iter().copied())
    }

    /// Before the head has wrapped, everything is the new run, from the
    /// left edge to the head, and the last value is held to the head.
    #[test]
    fn the_first_sweep_runs_from_the_left_edge_to_the_head() {
        let r = runs(1000, 400, &[(0, 1.0), (200, 2.0)]);
        assert_eq!(r[0].len(), 0);
        assert_eq!(r[1], [(0.0, 1.0), (0.2, 2.0), (0.4, 2.0)]);
        assert!((Sweep::new(1000, 400).head() - 0.4).abs() < 1e-9);
    }

    /// Across the wrap the step is cut: the old run ends on the right
    /// edge and the new one starts on the left, both with the value in
    /// force there, and the blank ahead of the head is left out.
    #[test]
    fn a_step_across_the_wrap_is_cut_at_the_edges() {
        // Window 1000, head at 1300 (30% across), wrapped at 1000.
        let r = runs(1000, 1300, &[(0, 5.0), (700, 6.0), (1200, 7.0)]);
        let s = Sweep::new(1000, 1300);
        assert_eq!(s.oldest_ms(), 1300 - 960);
        // The old run starts with the value in force when the drawn time
        // starts (5.0 until 700), just after the blank gap.
        assert_eq!(r[0].first().map(|p| p.1), Some(5.0));
        assert!(
            (r[0][0].0 - 0.34).abs() < 1e-9,
            "starts after the gap: {:?}",
            r[0]
        );
        assert_eq!(r[0].last(), Some(&(1.0, 6.0)), "held to the right edge");
        assert_eq!(r[1].first(), Some(&(0.0, 6.0)), "and taken up at the left");
        assert_eq!(r[1].last(), Some(&(0.3, 7.0)), "held to the head");
        assert!(r[0].iter().all(|p| p.0 > s.head() + GAP - 1e-9));
        assert!(r[1].iter().all(|p| p.0 <= s.head() + 1e-9));
    }

    #[test]
    fn a_window_shorter_than_one_scan_still_draws_the_held_value() {
        // One scan every 10 ms and a 100 ms window: all fine. 1 ms is
        // shorter than a scan: the value in force is the whole lane.
        let r = runs(1, 1000, &[(0, 3.0), (10, 4.0), (990, 4.0)]);
        let all: Vec<f64> = r.iter().flatten().map(|p| p.1).collect();
        assert_ne!(all.len(), 0);
        assert!(all.iter().all(|v| (*v - 4.0).abs() < 1e-9), "{all:?}");
        for (s, _) in r.iter().flatten() {
            assert!((0.0..=1.0).contains(s), "{s}");
        }
        // A zero window is one millisecond, not a division by zero.
        let z = Sweep::new(0, 5);
        assert!(z.head().is_finite());
    }

    #[test]
    fn no_samples_and_not_a_number_draw_nothing() {
        assert_eq!(runs(1000, 500, &[]), [vec![], vec![]]);
        let r = runs(
            1000,
            500,
            &[(0, f64::NAN), (100, f64::INFINITY), (200, f64::NEG_INFINITY)],
        );
        assert_eq!(r, [vec![], vec![]]);
        // A NaN in the middle is skipped; the finite ones around it stay.
        let r = runs(1000, 500, &[(0, 1.0), (100, f64::NAN), (300, 2.0)]);
        assert_eq!(r[1], [(0.0, 1.0), (0.3, 2.0), (0.5, 2.0)]);
    }

    /// A sample after the head (a stale clock) is not drawn, and the
    /// head exactly on the wrap leaves the old run whole.
    #[test]
    fn a_sample_from_the_future_is_left_out_and_the_head_on_the_wrap_is_at_the_left() {
        let r = runs(1000, 1000, &[(600, 1.0), (1500, 9.0)]);
        assert!(r.iter().flatten().all(|p| (p.1 - 1.0).abs() < 1e-9));
        assert_eq!(r[0].last(), Some(&(1.0, 1.0)));
        assert_eq!(r[1].first(), Some(&(0.0, 1.0)));
        assert!(Sweep::new(1000, 1000).head().abs() < 1e-9);
    }

    #[test]
    fn the_trail_fades_with_age_and_never_vanishes() {
        assert!((fade(0.0) - 1.0).abs() < 1e-6);
        let mut before = 1.0;
        for k in 1..=20 {
            let a = fade(f64::from(k) * 0.048);
            assert!(a <= before && a >= 0.1 - 1e-6, "{a}");
            before = a;
        }
        assert!(
            (fade(5.0) - 0.1).abs() < 1e-6,
            "older than the gap is the faintest"
        );
    }

    #[test]
    fn a_trace_is_cut_into_bands_that_join_up_and_fade() {
        let pts: Vec<Pos2> = (0..100).map(|i| Pos2::new(i as f32, 0.0)).collect();
        // Age 0 at the right end, rising to the left.
        let b = bands(&pts, |p| f64::from(99.0 - p.x) / 99.0 * 0.96);
        assert!(b.len() > 6, "{}", b.len());
        for w in b.windows(2) {
            assert!(w[0].0 <= w[1].0, "older runs are fainter");
            assert_eq!(w[0].1.last(), w[1].1.first(), "no break between runs");
        }
        let total: usize = b.iter().map(|(_, r)| r.len()).sum();
        assert_eq!(total, 100 + b.len() - 1);
        assert_eq!(bands(&[], |_| 0.0).len(), 0);
        // Ages outside the range (NaN too) do not panic.
        let odd = bands(&pts[..3], |_| f64::NAN);
        assert_eq!(odd.len(), 1);
    }

    #[test]
    fn a_lane_that_joins_pops_for_a_second_and_a_half_from_its_first_frame() {
        let mut f = Flashes::default();
        f.add("LAMP");
        assert_eq!(f.strength("lamp", 3.0), 0.0, "not timed until a frame saw it");
        assert!(f.tick(10.0));
        assert!(f.strength("lamp", 10.0) > 0.9, "full at once");
        assert!(f.strength("LAMP", 10.4) < f.strength("lamp", 10.0));
        assert!(f.tick(11.4));
        assert!(!f.tick(11.5), "settled");
        assert_eq!(f.strength("lamp", 11.5), 0.0);
        assert_eq!(pulse(-1.0), 0.0);
        assert_eq!(pulse(f64::NAN), 0.0);
        // Nothing waiting: no frames asked for.
        assert!(!Flashes::default().tick(0.0));
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    /// At the cap the oldest automatic pin makes room; the engineer's own
    /// stay, and a trend full of them takes nothing.
    #[test]
    fn at_the_cap_the_oldest_automatic_pin_goes_and_the_engineers_stay() {
        let mut sig = names(&["a", "b", "c"]);
        let mut auto = names(&["b", "c"]);
        assert_eq!(pin_auto(&mut sig, &mut auto, &names(&["d"]), 4), ["d"]);
        assert_eq!(sig, ["a", "b", "c", "d"], "room: at the bottom");
        assert_eq!(pin_auto(&mut sig, &mut auto, &names(&["e", "f"]), 4), ["e", "f"]);
        assert_eq!(
            sig,
            ["a", "d", "e", "f"],
            "b and c, the oldest automatic, made room"
        );
        assert_eq!(auto, ["d", "e", "f"]);
        // Only the engineer's own left: nothing is taken.
        let mut sig = names(&["a", "b"]);
        let mut auto = Vec::new();
        assert_eq!(pin_auto(&mut sig, &mut auto, &names(&["x"]), 2).len(), 0);
        assert_eq!(sig, ["a", "b"]);
        // Nothing added, nothing pinned twice, more than fits keeps the last.
        assert_eq!(pin_auto(&mut sig, &mut auto, &[], 2).len(), 0);
        let mut sig = Vec::new();
        let mut auto = Vec::new();
        assert_eq!(pin_auto(&mut sig, &mut auto, &names(&["p", "p"]), 3).len(), 1);
        let mut sig = Vec::new();
        let mut auto = Vec::new();
        pin_auto(&mut sig, &mut auto, &names(&["1", "2", "3", "4", "5"]), 3);
        assert_eq!(sig, ["3", "4", "5"]);
        // A cap of nothing pins nothing.
        let mut sig = Vec::new();
        assert_eq!(pin_auto(&mut sig, &mut Vec::new(), &names(&["x"]), 0).len(), 0);
    }
}
