//! The Arrangement's editing grid: zoom-adaptive or fixed spacing, triplets,
//! and snapping (arrangement-view R11).
//!
//! The grid is laid out from each bar's start, so it follows the time
//! signature and starts again after a change. Its spacings form a ladder:
//! note values that divide the bar evenly (1/32 up to the whole note), then
//! whole bars, 2, 4, 8, and so on. A zoom-adaptive grid takes the finest
//! rung whose lines are at least its setting's distance apart on screen;
//! each setting from Narrowest to Widest doubles that distance, so Narrow
//! Grid and Widen Grid halve and double the lines' density, as Live's do.

use bl_core::time::{Ticks, TimeSignature, TimeSignatureMap};
use bl_shell_spec::arrangement::{AdaptiveGrid, FixedGrid, GridState};

/// The note values the grid uses below a bar: 1/32 to the whole note, as
/// denominators, finest first.
const NOTES: [i64; 6] = [32, 16, 8, 4, 2, 1];

/// Bar multiples above a bar.
const BARS: [i64; 12] = [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048];

/// How far apart, in points, an adaptive setting keeps the lines at least.
#[must_use]
pub fn min_spacing(setting: AdaptiveGrid) -> f64 {
    match setting {
        AdaptiveGrid::Narrowest => 8.0,
        AdaptiveGrid::Narrow => 16.0,
        AdaptiveGrid::Medium => 32.0,
        AdaptiveGrid::Wide => 64.0,
        AdaptiveGrid::Widest => 128.0,
    }
}

/// The grid's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    /// The zoom-adaptive setting, used while `fixed_on` is off.
    pub adaptive: AdaptiveGrid,
    /// The fixed spacing, used while `fixed_on` is on.
    pub fixed: FixedGrid,
    /// Whether the grid is fixed.
    pub fixed_on: bool,
    /// Whether note values divide into triplets.
    pub triplet: bool,
    /// Whether clicks snap to the grid.
    pub snap: bool,
}

impl Default for Grid {
    /// A new set's grid: adaptive at Narrow, snapping on (menus.md,
    /// C-CLIP-ARR-019 and 020). The fixed spacing a new set keeps for
    /// Cmd-5 hasn't been observed; 1/16 until it is.
    fn default() -> Self {
        Self {
            adaptive: AdaptiveGrid::Narrow,
            fixed: FixedGrid::Sixteenth,
            fixed_on: false,
            triplet: false,
            snap: true,
        }
    }
}

/// One rung of the ladder: a spacing and how to name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// The spacing.
    pub ticks: Ticks,
    /// Whole bars, for a spacing of a bar or more.
    pub bars: Option<i64>,
    /// The note value's denominator, for a spacing below a bar (or a whole
    /// note that's also the bar, as in 4/4).
    pub note: Option<i64>,
    /// Whether it's a triplet value.
    pub triplet: bool,
}

impl Step {
    /// How the grid indicator shows it: `1/16`, `1/8T`, `1/1` (a whole note,
    /// which is the bar in 4/4), `1 Bar`, `4 Bars`.
    #[must_use]
    pub fn label(self) -> String {
        match (self.note, self.bars) {
            (Some(n), _) => format!("1/{n}{}", if self.triplet { "T" } else { "" }),
            (None, Some(1)) => "1 Bar".to_owned(),
            (None, Some(b)) => format!("{b} Bars"),
            (None, None) => String::new(),
        }
    }

    /// Whether it's a bar or more apart.
    #[must_use]
    pub fn is_bars(self) -> bool {
        self.bars.is_some()
    }
}

/// The ladder of spacings for a signature, finest first.
fn ladder(signature: TimeSignature, triplet: bool) -> impl Iterator<Item = Step> {
    let bar = signature.bar_length();
    let notes = NOTES.into_iter().filter_map(move |n| {
        let plain = Ticks::WHOLE.get() / n;
        if plain > bar.get() || bar.get() % plain != 0 {
            return None;
        }
        if plain == bar.get() {
            // The whole bar: named as a note in 4/4, but it's the bar rung.
            return Some(Step {
                ticks: bar,
                bars: Some(1),
                note: Some(n),
                triplet: false,
            });
        }
        let ticks = if triplet { plain * 2 / 3 } else { plain };
        Some(Step {
            ticks: Ticks(ticks),
            bars: None,
            note: Some(n),
            triplet,
        })
    });
    let has_whole_bar_note = bar == Ticks::WHOLE;
    let bars = BARS
        .into_iter()
        .filter(move |&b| !(b == 1 && has_whole_bar_note))
        .map(move |b| Step {
            ticks: Ticks(bar.get().saturating_mul(b)),
            bars: Some(b),
            note: None,
            triplet: false,
        });
    notes.chain(bars)
}

impl Grid {
    /// The spacing at a zoom of `ticks_per_point`, under `signature`.
    #[must_use]
    pub fn step(&self, signature: TimeSignature, ticks_per_point: f64) -> Step {
        if self.fixed_on {
            return self.fixed_step(signature);
        }
        let min = min_spacing(self.adaptive);
        let mut last = None;
        for step in ladder(signature, self.triplet) {
            if step.ticks.get() as f64 / ticks_per_point >= min {
                return step;
            }
            last = Some(step);
        }
        last.expect("the ladder is never empty")
    }

    /// The fixed spacing, under `signature`.
    #[must_use]
    pub fn fixed_step(&self, signature: TimeSignature) -> Step {
        let bar = signature.bar_length();
        match (self.fixed.bars(), self.fixed.note()) {
            (Some(b), _) => Step {
                ticks: Ticks(bar.get() * i64::from(b)),
                bars: Some(i64::from(b)),
                note: None,
                triplet: false,
            },
            (None, Some(n)) => {
                let plain = Ticks::WHOLE.get() / i64::from(n);
                let ticks = if self.triplet { plain * 2 / 3 } else { plain };
                Step {
                    ticks: Ticks(ticks),
                    bars: None,
                    note: Some(i64::from(n)),
                    triplet: self.triplet,
                }
            }
            (None, None) => unreachable!("every fixed spacing is bars or a note"),
        }
    }

    /// Whether Narrow Grid can run: not at the narrowest setting.
    #[must_use]
    pub fn can_narrow(&self) -> bool {
        if self.fixed_on {
            self.fixed != FixedGrid::ThirtySecond
        } else {
            self.adaptive != AdaptiveGrid::Narrowest
        }
    }

    /// Whether Widen Grid can run: not at the widest setting.
    #[must_use]
    pub fn can_widen(&self) -> bool {
        if self.fixed_on {
            self.fixed != FixedGrid::EightBars
        } else {
            self.adaptive != AdaptiveGrid::Widest
        }
    }

    /// Makes the grid twice as dense, if it can be.
    pub fn narrow(&mut self) -> bool {
        if !self.can_narrow() {
            return false;
        }
        if self.fixed_on {
            self.fixed = step_in(&FixedGrid::ALL, self.fixed, 1);
        } else {
            self.adaptive = step_in(&AdaptiveGrid::ALL, self.adaptive, 1);
        }
        true
    }

    /// Makes the grid half as dense, if it can be.
    pub fn widen(&mut self) -> bool {
        if !self.can_widen() {
            return false;
        }
        if self.fixed_on {
            self.fixed = step_in(&FixedGrid::ALL, self.fixed, -1);
        } else {
            self.adaptive = step_in(&AdaptiveGrid::ALL, self.adaptive, -1);
        }
        true
    }

    /// The grid as the view commands report it, at a zoom.
    #[must_use]
    pub fn state(&self, signature: TimeSignature, ticks_per_point: f64) -> GridState {
        let step = self.step(signature, ticks_per_point);
        GridState {
            adaptive: self.adaptive,
            fixed_spacing: self.fixed,
            fixed: self.fixed_on,
            triplet: self.triplet,
            snap: self.snap,
            step: step.ticks,
            label: step.label(),
        }
    }
}

/// The item `delta` places after `item` in `all` (which holds it).
fn step_in<T: Copy + PartialEq>(all: &[T], item: T, delta: isize) -> T {
    let at = all.iter().position(|x| *x == item).unwrap_or(0);
    let next = at.saturating_add_signed(delta).min(all.len() - 1);
    all[next]
}

/// The grid line nearest `at`, for a spacing of `step` laid out from each
/// bar's start under `signatures`. A spacing of several bars counts bars
/// from 1.1.1.
#[must_use]
pub fn snap(signatures: &TimeSignatureMap, step: Step, at: Ticks) -> Ticks {
    let (bar, into) = signatures.bar_at(at);
    if let Some(bars) = step.bars {
        let base = bar.div_euclid(bars) * bars;
        let start = signatures.bar_start(base);
        let next = signatures.bar_start(base + bars);
        return if (at - start) <= (next - at) {
            start
        } else {
            next
        };
    }
    let start = signatures.bar_start(bar);
    let end = signatures.bar_start(bar + 1);
    let lines = into.get() as f64 / step.ticks.get().max(1) as f64;
    let snapped = (start + Ticks((lines.round() as i64) * step.ticks.get())).min(end);
    if (end - at) < (at - snapped).abs() {
        end
    } else {
        snapped
    }
}

/// Calls `line` for every grid line from `from` to `to`, with whether it
/// starts a bar, for a spacing laid out from each bar's start. Stops after
/// `limit` lines.
pub fn lines(
    signatures: &TimeSignatureMap,
    step: Step,
    from: Ticks,
    to: Ticks,
    limit: usize,
    mut line: impl FnMut(Ticks, bool),
) {
    let (first_bar, _) = signatures.bar_at(from);
    let bars = step.bars.unwrap_or(1).max(1);
    let mut bar = first_bar.div_euclid(bars) * bars;
    let mut count = 0;
    loop {
        let start = signatures.bar_start(bar);
        if start > to || count >= limit {
            return;
        }
        if step.bars.is_some() {
            if start >= from {
                line(start, true);
                count += 1;
            }
            bar += bars;
            continue;
        }
        let end = signatures.bar_start(bar + 1);
        let mut at = start;
        while at < end && at <= to && count < limit {
            if at >= from {
                line(at, at == start);
                count += 1;
            }
            at += step.ticks;
        }
        bar += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOUR_FOUR: TimeSignature = TimeSignature::FOUR_FOUR;

    fn bar() -> i64 {
        Ticks::WHOLE.get()
    }

    #[test]
    fn the_adaptive_grid_keeps_lines_its_distance_apart() {
        // Spec: arrangement-view R11.
        let grid = Grid::default();
        // 30 points to the bar: Narrow wants 16 points, so whole bars.
        let tpp = bar() as f64 / 30.0;
        let step = grid.step(FOUR_FOUR, tpp);
        assert_eq!(step.ticks, Ticks::WHOLE);
        assert_eq!(step.label(), "1/1");
        // Twice as close: half bars are 30 points apart.
        let step = grid.step(FOUR_FOUR, tpp / 2.0);
        assert_eq!(step.ticks, Ticks::WHOLE / 2);
        assert_eq!(step.label(), "1/2");
        // Far out: several bars.
        let step = grid.step(FOUR_FOUR, tpp * 4.0);
        assert_eq!(step.bars, Some(4));
        assert_eq!(step.label(), "4 Bars");
        // Each narrower setting halves the spacing.
        let mut narrower = grid;
        assert!(narrower.narrow());
        assert_eq!(narrower.adaptive, AdaptiveGrid::Narrowest);
        assert_eq!(narrower.step(FOUR_FOUR, tpp).ticks, Ticks::WHOLE / 2);
        assert!(!narrower.can_narrow());
        let mut wider = grid;
        assert!(wider.widen());
        assert_eq!(wider.step(FOUR_FOUR, tpp).bars, Some(2));
    }

    #[test]
    fn triplets_divide_notes_in_three_and_leave_bars_alone() {
        let grid = Grid {
            triplet: true,
            ..Grid::default()
        };
        let tpp = bar() as f64 / 300.0;
        let step = grid.step(FOUR_FOUR, tpp);
        assert!(step.triplet);
        assert_eq!(
            step.ticks.get() * 3,
            Ticks::WHOLE.get() / step.note.unwrap() * 2
        );
        assert!(step.label().ends_with('T'));
        let far = grid.step(FOUR_FOUR, bar() as f64 / 10.0);
        assert!(!far.triplet && far.is_bars());
    }

    #[test]
    fn three_four_has_no_half_note_rung() {
        let three = TimeSignature::new(3, 4).unwrap();
        let steps: Vec<String> = ladder(three, false).take(6).map(Step::label).collect();
        assert_eq!(steps, ["1/32", "1/16", "1/8", "1/4", "1 Bar", "2 Bars"]);
    }

    #[test]
    fn fixed_grids_ignore_the_zoom() {
        let grid = Grid {
            fixed_on: true,
            fixed: FixedGrid::Eighth,
            ..Grid::default()
        };
        for tpp in [10.0, 1_000.0, 1_000_000.0] {
            assert_eq!(grid.step(FOUR_FOUR, tpp).ticks, Ticks::WHOLE / 8);
        }
        let mut grid = grid;
        while grid.widen() {}
        assert_eq!(grid.fixed, FixedGrid::EightBars);
        assert_eq!(grid.step(FOUR_FOUR, 1.0).label(), "8 Bars");
    }

    #[test]
    fn snapping_goes_to_the_nearest_line() {
        let map = TimeSignatureMap::default();
        let quarter = Grid {
            fixed_on: true,
            fixed: FixedGrid::Quarter,
            ..Grid::default()
        }
        .fixed_step(FOUR_FOUR);
        assert_eq!(
            snap(&map, quarter, Ticks::QUARTER + Ticks(10)),
            Ticks::QUARTER
        );
        assert_eq!(
            snap(&map, quarter, Ticks::QUARTER * 2 - Ticks(10)),
            Ticks::QUARTER * 2
        );
        let bars = Step {
            ticks: Ticks::WHOLE * 4,
            bars: Some(4),
            note: None,
            triplet: false,
        };
        assert_eq!(snap(&map, bars, Ticks::WHOLE * 3), Ticks::WHOLE * 4);
        assert_eq!(snap(&map, bars, Ticks::WHOLE), Ticks::ZERO);
    }

    #[test]
    fn lines_start_each_bar() {
        let map = TimeSignatureMap::default();
        let step = Grid {
            fixed_on: true,
            fixed: FixedGrid::Half,
            ..Grid::default()
        }
        .fixed_step(FOUR_FOUR);
        let mut found = Vec::new();
        lines(&map, step, Ticks::ZERO, Ticks::WHOLE * 2, 100, |at, bar| {
            found.push((at.get() / (Ticks::WHOLE.get() / 2), bar));
        });
        assert_eq!(
            found,
            [(0, true), (1, false), (2, true), (3, false), (4, true)]
        );
    }
}
