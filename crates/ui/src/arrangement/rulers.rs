//! The Arrangement View's rulers: the beat-time ruler with the loop brace
//! (L-ARR-002), the scrub area (L-ARR-003), and the time ruler (L-ARR-016),
//! drawn with egui's painter (arrangement-view R5, R13).

use bl_core::time::{TempoMap, Ticks, TimeSignatureMap};
use bl_model::{ChordRegion, ChordRegionId, Locator, LocatorId, SongStart};
use bl_shell::arrangement::{Horizontal, TransportView};
use egui::{Align2, Color32, Painter, Rect, Stroke, pos2};

use crate::theme::Theme;

/// The fewest points between two labels on a ruler.
const LABEL_SPACING: f64 = 36.0;

/// The fewest points between two tick marks.
const TICK_SPACING: f64 = 5.0;

/// How many bars apart the beat-time ruler labels bars, for bars
/// `bar_points` wide: 1, 2, 4, 8, ... so labels never crowd.
#[must_use]
pub fn bar_label_step(bar_points: f64) -> i64 {
    let mut step = 1;
    while (step as f64) * bar_points < LABEL_SPACING && step < 1 << 24 {
        step *= 2;
    }
    step
}

/// A ruler label: bar numbers from bl-core, so there's no bar 0, and a
/// position before 1.1.1 reads −1.
#[must_use]
pub fn position_label(signatures: &TimeSignatureMap, at: Ticks, depth: u8) -> String {
    let bbs = signatures.to_bbs(at);
    match depth {
        0 => bbs.bar.to_string(),
        1 => format!("{}.{}", bbs.bar, bbs.beat),
        _ => format!("{}.{}.{}", bbs.bar, bbs.beat, bbs.sixteenth),
    }
}

/// Draws the beat-time ruler: bar numbers, beat and sixteenth ticks as the
/// zoom allows, and the loop brace.
pub fn beat_ruler(
    painter: &Painter,
    rect: Rect,
    signatures: &TimeSignatureMap,
    horizontal: Horizontal,
    transport: &TransportView,
    theme: &Theme,
) {
    let colors = &theme.colors;
    painter.rect_filled(rect, 0.0, colors.ruler.0);
    let tpp = horizontal.ticks_per_point;
    let range = horizontal.range(rect.width());
    let x_of = |at: Ticks| rect.left() + horizontal.x(at) as f32;

    // The loop brace, along the ruler's lower edge.
    let (l0, l1) = (x_of(transport.loop_start), x_of(transport.loop_end()));
    if l1 > rect.left() && l0 < rect.right() {
        let brace = Rect::from_min_max(
            pos2(l0.max(rect.left() - 1.0), rect.bottom() - 6.0),
            pos2(l1.min(rect.right() + 1.0), rect.bottom() - 1.0),
        );
        let colour = if transport.loop_on {
            colors.loop_on.0
        } else {
            colors.loop_off.0
        };
        painter.rect_filled(brace, 1.0, colour);
    }

    let tick = Stroke::new(1.0, colors.ruler_tick.0);
    let font = theme.small_font();
    let signature = signatures.signature_at(range.start);
    let bar_points = signature.bar_length().get() as f64 / tpp;
    let beat_points = signature.beat_length().get() as f64 / tpp;
    let sixteenth_points = Ticks::SIXTEENTH.get() as f64 / tpp;
    let label_every = bar_label_step(bar_points);
    let (first, _) = signatures.bar_at(range.start);
    let mut bar = if bar_points >= TICK_SPACING {
        first
    } else {
        first.div_euclid(label_every) * label_every
    };
    let bar_step = if bar_points >= TICK_SPACING {
        1
    } else {
        label_every
    };
    let mut drawn = 0;
    while drawn < 2048 {
        let start = signatures.bar_start(bar);
        if start > range.end {
            break;
        }
        let end = signatures.bar_start(bar + 1);
        let x = x_of(start);
        if x >= rect.left() - 1.0 {
            painter.line_segment([pos2(x, rect.top() + 4.0), pos2(x, rect.bottom())], tick);
        }
        if bar.rem_euclid(label_every) == 0 {
            painter.text(
                pos2(x + 3.0, rect.top() + 1.0),
                Align2::LEFT_TOP,
                position_label(signatures, start, 0),
                font.clone(),
                colors.text_dim.0,
            );
        }
        if bar_step == 1 && beat_points >= TICK_SPACING {
            let beat = signatures.signature_at(start).beat_length();
            let fine = if sixteenth_points >= TICK_SPACING * 2.0 {
                Ticks::SIXTEENTH
            } else {
                beat
            };
            let mut at = start + fine;
            while at < end && at <= range.end {
                let x = x_of(at);
                let on_beat = (at - start).rem_euclid(beat) == Ticks::ZERO;
                let height = if on_beat { 6.0 } else { 3.0 };
                painter.line_segment(
                    [pos2(x, rect.bottom() - height), pos2(x, rect.bottom())],
                    tick,
                );
                let label_depth = if on_beat && beat_points >= LABEL_SPACING * 1.5 {
                    Some(1)
                } else if !on_beat && sixteenth_points >= LABEL_SPACING * 1.5 {
                    Some(2)
                } else {
                    None
                };
                if let Some(depth) = label_depth {
                    painter.text(
                        pos2(x + 3.0, rect.top() + 1.0),
                        Align2::LEFT_TOP,
                        position_label(signatures, at, depth),
                        font.clone(),
                        colors.text_dim.0,
                    );
                }
                at += fine;
            }
        }
        bar += bar_step;
        drawn += 1;
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, colors.separator.0),
    );
}

/// Draws the time-signature lane (L-ARR-004): a line and the signature at
/// each marker, a selected one in the selection colour, and each bar a
/// marker cuts short hatched.
pub fn signature_lane(
    painter: &Painter,
    rect: Rect,
    signatures: &TimeSignatureMap,
    selected: Option<Ticks>,
    horizontal: Horizontal,
    theme: &Theme,
) {
    let colors = &theme.colors;
    painter.rect_filled(rect, 0.0, colors.bar.0);
    let x_of = |at: Ticks| rect.left() + horizontal.x(at) as f32;
    let hatch = Stroke::new(1.0, colors.ruler_tick.0);
    for bar in signatures.fragmentary_bars() {
        let (x0, x1) = (x_of(bar.start), x_of(bar.end));
        if x1 < rect.left() || x0 > rect.right() {
            continue;
        }
        let shown = Rect::from_min_max(
            pos2(x0.max(rect.left()), rect.top()),
            pos2(x1.min(rect.right()), rect.bottom()),
        );
        let hatched = painter.with_clip_rect(shown);
        let mut x = shown.left() - rect.height();
        while x < shown.right() {
            hatched.line_segment(
                [pos2(x, rect.bottom()), pos2(x + rect.height(), rect.top())],
                hatch,
            );
            x += 4.0;
        }
    }
    for change in signatures.changes() {
        let x = x_of(change.at);
        if x < rect.left() - 40.0 || x > rect.right() {
            continue;
        }
        let colour = if selected == Some(change.at) {
            colors.accent.0
        } else {
            colors.text_dim.0
        };
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, colour),
        );
        painter.text(
            pos2(x + 3.0, rect.center().y),
            Align2::LEFT_CENTER,
            change.signature.to_string(),
            theme.small_font(),
            colour,
        );
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, colors.separator.0),
    );
}

/// Draws the chord lane (L-ARR-024): each region from its start to its end,
/// named for its scale and its transposition, the selected one in the
/// selection colour.
pub fn chord_lane(
    painter: &Painter,
    rect: Rect,
    horizontal: Horizontal,
    regions: &[ChordRegion],
    selected: Option<ChordRegionId>,
    theme: &Theme,
) {
    let colors = &theme.colors;
    painter.rect_filled(rect, 0.0, colors.bar.0);
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, colors.separator.0),
    );
    for region in regions {
        let Some(block) = region_rect(rect, horizontal, region) else {
            continue;
        };
        let chosen = selected == Some(region.id);
        let fill = if chosen {
            colors.accent.0
        } else {
            colors.accent.0.gamma_multiply(0.35)
        };
        painter.rect_filled(block.shrink2(egui::vec2(0.0, 2.0)), 2.0, fill);
        painter.with_clip_rect(block).text(
            pos2(block.left() + 4.0, block.center().y),
            Align2::LEFT_CENTER,
            region.name(),
            theme.small_font(),
            colors.text.0,
        );
    }
}

/// Where a region sits in the lane `lane`, cut to the lane's width; `None`
/// when it is out of view.
#[must_use]
pub fn region_rect(lane: Rect, horizontal: Horizontal, region: &ChordRegion) -> Option<Rect> {
    let x0 = lane.left() + horizontal.x(region.start) as f32;
    let x1 = lane.left() + horizontal.x(region.end) as f32;
    (x1 > lane.left() && x0 < lane.right()).then(|| {
        Rect::from_min_max(
            pos2(x0.max(lane.left()), lane.top()),
            pos2(x1.min(lane.right()), lane.bottom()),
        )
    })
}

/// Draws the scrub area, with the locators in it (L-ARR-003): each at its
/// position, with its name, a selected one in the selection colour, and the
/// one that holds the song start time in the theme's song start colour.
pub fn scrub_area(
    painter: &Painter,
    rect: Rect,
    horizontal: Horizontal,
    locators: &[Locator],
    selected: &[LocatorId],
    song_start: Option<SongStart>,
    theme: &Theme,
) {
    painter.rect_filled(rect, 0.0, theme.colors.bar.0);
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, theme.colors.separator.0),
    );
    let font = egui::FontId::proportional(10.0);
    let left = rect.left();
    for locator in locators {
        let x = (horizontal.x(locator.position) + f64::from(left)) as f32;
        if x < left - 40.0 || x > rect.right() + 40.0 {
            continue;
        }
        let colour = if song_start == Some(SongStart::Locator(locator.id)) {
            theme.colors.song_start.0
        } else if selected.contains(&locator.id) {
            theme.colors.accent.0
        } else {
            theme.colors.text_dim.0
        };
        let marker = Rect::from_min_max(pos2(x - 4.0, rect.top()), pos2(x + 4.0, rect.bottom()));
        painter.rect_filled(marker, 0.0, colour);
        painter.text(
            pos2(x + 6.0, rect.center().y),
            Align2::LEFT_CENTER,
            &locator.name,
            font.clone(),
            colour,
        );
    }
}

/// The time ruler's label spacings, in seconds.
const SECONDS: [f64; 19] = [
    0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0,
    600.0, 1800.0, 3600.0,
];

/// A time as the time ruler shows it: minutes and seconds, with
/// milliseconds when the labels are less than a second apart.
#[must_use]
pub fn time_label(seconds: f64, with_millis: bool) -> String {
    let millis = (seconds * 1000.0).round() as i64;
    let (minutes, rest) = (millis / 60_000, millis % 60_000);
    if with_millis {
        format!("{minutes}:{:02}:{:03}", rest / 1000, rest % 1000)
    } else {
        format!("{minutes}:{:02}", rest / 1000)
    }
}

/// Draws the time ruler: minutes and seconds through the tempo map.
pub fn time_ruler(
    painter: &Painter,
    rect: Rect,
    tempo: &TempoMap,
    horizontal: Horizontal,
    theme: &Theme,
) {
    let colors = &theme.colors;
    painter.rect_filled(rect, 0.0, colors.ruler.0);
    let range = horizontal.range(rect.width());
    let first = tempo.ticks_to_seconds(range.start.max(Ticks::ZERO));
    let last = tempo.ticks_to_seconds(range.end.max(Ticks::ZERO));
    let seconds_per_point = (last - first) / f64::from(rect.width().max(1.0));
    let step = SECONDS
        .iter()
        .copied()
        .find(|s| s / seconds_per_point.max(1e-12) >= LABEL_SPACING * 1.6)
        .unwrap_or(3600.0);
    let with_millis = step < 1.0;
    let tick = Stroke::new(1.0, colors.ruler_tick.0);
    let mut at = (first / step).floor() * step;
    let mut drawn = 0;
    while at <= last && drawn < 512 {
        if at >= 0.0 {
            let x = rect.left() + horizontal.x(tempo.seconds_to_ticks(at)) as f32;
            painter.line_segment([pos2(x, rect.top()), pos2(x, rect.top() + 4.0)], tick);
            painter.text(
                pos2(x + 3.0, rect.center().y),
                Align2::LEFT_CENTER,
                time_label(at, with_millis),
                theme.small_font(),
                colors.text_dim.0,
            );
        }
        at += step;
        drawn += 1;
    }
}

/// A vertical line at `x` from `top` to `bottom`, as the playhead and the
/// insert marker are drawn.
pub fn marker(painter: &Painter, x: f32, top: f32, bottom: f32, colour: Color32) {
    painter.line_segment([pos2(x, top), pos2(x, bottom)], Stroke::new(1.0, colour));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_spread_out_by_powers_of_two() {
        assert_eq!(bar_label_step(60.0), 1);
        assert_eq!(bar_label_step(30.0), 2);
        assert_eq!(bar_label_step(9.0), 4);
        assert_eq!(bar_label_step(1.0), 64);
    }

    #[test]
    fn there_is_no_bar_zero() {
        // Spec: arrangement-view R5; musical-time "There's no bar 0".
        let map = TimeSignatureMap::default();
        assert_eq!(position_label(&map, Ticks::ZERO, 0), "1");
        assert_eq!(position_label(&map, -Ticks::WHOLE, 0), "-1");
        assert_eq!(position_label(&map, -Ticks::QUARTER, 1), "-1.4");
        assert_eq!(position_label(&map, Ticks::QUARTER, 1), "1.2");
        assert_eq!(position_label(&map, Ticks::SIXTEENTH, 2), "1.1.2");
    }

    #[test]
    fn times_read_as_minutes_and_seconds() {
        assert_eq!(time_label(0.0, false), "0:00");
        assert_eq!(time_label(75.0, false), "1:15");
        assert_eq!(time_label(1.25, true), "0:01:250");
    }
}
