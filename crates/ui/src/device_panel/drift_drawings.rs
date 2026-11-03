//! What Drift's displays draw (layout L-DEV-065): an envelope's curve, the
//! LFO's wave, the pitch modulation under the oscillators, and the little
//! waveforms that stand for a waveform chooser's choices.

use std::f32::consts::TAU;

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

/// The points of the waveform a chooser's choice names, in a unit square with
/// y down, or `None` for a choice that isn't a waveform.
#[must_use]
pub fn wave_points(name: &str) -> Option<Vec<(f32, f32)>> {
    let sine = |periods: f32, up: bool| -> Vec<(f32, f32)> {
        (0..=32)
            .map(|i| {
                let x = i as f32 / 32.0;
                let y = 0.5 - 0.4 * (TAU * periods * x).sin() * if up { 1.0 } else { -1.0 };
                (x, y)
            })
            .collect()
    };
    Some(match name {
        "Sine" => sine(1.0, true),
        "Triangle" => vec![(0.0, 0.5), (0.25, 0.1), (0.75, 0.9), (1.0, 0.5)],
        "Saw" | "Saw Up" => vec![(0.0, 0.9), (0.9, 0.1), (0.9, 0.9), (1.0, 0.9)],
        "Saw Down" => vec![(0.0, 0.1), (0.0, 0.9), (0.9, 0.1), (1.0, 0.1)],
        "Shark Tooth" => vec![(0.0, 0.9), (0.45, 0.1), (0.5, 0.9), (0.55, 0.1), (1.0, 0.9)],
        "Saturated" => (0..=32)
            .map(|i| {
                let x = i as f32 / 32.0;
                (x, 0.5 - 0.4 * (3.0 * (TAU * x).sin()).tanh())
            })
            .collect(),
        "Pulse" => vec![(0.0, 0.9), (0.0, 0.1), (0.3, 0.1), (0.3, 0.9), (1.0, 0.9)],
        "Rectangle" | "Square" => {
            vec![(0.0, 0.9), (0.0, 0.1), (0.5, 0.1), (0.5, 0.9), (1.0, 0.9)]
        }
        "Sample & Hold" => vec![
            (0.0, 0.6),
            (0.25, 0.6),
            (0.25, 0.2),
            (0.5, 0.2),
            (0.5, 0.8),
            (0.75, 0.8),
            (0.75, 0.4),
            (1.0, 0.4),
        ],
        "Wander" => vec![
            (0.0, 0.5),
            (0.15, 0.2),
            (0.35, 0.7),
            (0.55, 0.3),
            (0.8, 0.8),
            (1.0, 0.5),
        ],
        "Linear Envelope" => vec![(0.0, 0.9), (0.15, 0.1), (1.0, 0.9)],
        "Exponential Envelope" => (0..=24)
            .map(|i| {
                let x = i as f32 / 24.0;
                (x, 0.1 + 0.8 * (1.0 - (-4.0 * x).exp()))
            })
            .collect(),
        _ => return None,
    })
}

/// Draws the waveform of `name` in `rect`, if it is one.
pub fn wave_icon(painter: &Painter, rect: Rect, name: &str, ink: Color32) -> bool {
    let Some(points) = wave_points(name) else {
        return false;
    };
    let at = |(x, y): (f32, f32)| {
        pos2(
            rect.left() + x * rect.width(),
            rect.top() + y * rect.height(),
        )
    };
    painter.add(Shape::line(
        points.into_iter().map(at).collect(),
        Stroke::new(1.0, ink),
    ));
    true
}

/// How long a segment of `ms` looks, from 0 to 1, on a scale that keeps both
/// a millisecond and twenty seconds in view.
fn span(ms: f32) -> f32 {
    (1.0 + ms / 5.0).ln() / (1.0f32 + 20_000.0 / 5.0).ln()
}

/// An envelope's points in `rect`: the start, the peak, the end of the
/// decay, the end of the hold, and the end of the release.
#[must_use]
pub fn envelope_points(rect: Rect, (a, d, s, r): (f32, f32, f32, f32)) -> [Pos2; 5] {
    let inner = rect.shrink2(vec2(6.0, 8.0));
    let (wa, wd, wr) = (span(a), span(d), span(r));
    let hold = 0.5;
    let total = wa + wd + hold + wr;
    let x = |at: f32| inner.left() + inner.width() * at / total;
    let level = |v: f32| inner.bottom() - inner.height() * v;
    [
        pos2(x(0.0), level(0.0)),
        pos2(x(wa), level(1.0)),
        pos2(x(wa + wd), level(s / 100.0)),
        pos2(x(wa + wd + hold), level(s / 100.0)),
        pos2(x(total), level(0.0)),
    ]
}

/// Draws an envelope's curve and its three handles.
pub fn envelope(
    painter: &Painter,
    rect: Rect,
    times: (f32, f32, f32, f32),
    ink: Color32,
    handle: Color32,
) {
    let points = envelope_points(rect, times);
    painter.add(Shape::line(points.to_vec(), Stroke::new(1.5, ink)));
    for at in [points[1], points[2], points[4]] {
        painter.rect_stroke(
            Rect::from_center_size(at, vec2(6.0, 6.0)),
            0.0,
            Stroke::new(1.0, handle),
            egui::StrokeKind::Middle,
        );
    }
}

/// Draws the LFO's wave across `rect`, two periods of the shape it names.
pub fn lfo(painter: &Painter, rect: Rect, name: &str, ink: Color32) {
    let Some(points) = wave_points(name) else {
        return;
    };
    let inner = rect.shrink2(vec2(6.0, 4.0));
    // Two periods side by side: a sine's one is already two humps, as Live's.
    let periods = if name == "Sine" || name == "Triangle" {
        1
    } else {
        2
    };
    let mut line = Vec::new();
    for p in 0..periods {
        for &(x, y) in &points {
            line.push(pos2(
                inner.left() + inner.width() * (p as f32 + x) / periods as f32,
                inner.top() + inner.height() * y,
            ));
        }
    }
    painter.add(Shape::line(line, Stroke::new(1.5, ink)));
}

/// Draws the pitch modulation over a note: Envelope 2 and the LFO, each by
/// its amount where it is a source of one of the two Pitch Mod slots, around
/// a middle line.
pub fn pitch_mod(
    painter: &Painter,
    rect: Rect,
    (envelope_2, lfo_wave): ((f32, f32, f32, f32), &str),
    (env_amount, lfo_amount): (f32, f32),
    ink: Color32,
) {
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, Color32::from_gray(90)),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(vec2(6.0, 8.0));
    let env = envelope_points(rect, envelope_2);
    let wave = wave_points(lfo_wave).unwrap_or_default();
    let steps = 64;
    let line: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            // The envelope's level at `t`, along its five points.
            let x = inner.left() + t * inner.width();
            let envelope = level_at(&env, x, inner);
            let lfo = if wave.is_empty() {
                0.5
            } else {
                let at = (t * 3.0).fract();
                let k = ((at * (wave.len() - 1) as f32) as usize).min(wave.len() - 1);
                1.0 - wave[k].1
            };
            let moved = (envelope - 0.5) * env_amount / 100.0 + (lfo - 0.5) * lfo_amount / 100.0;
            pos2(x, inner.center().y - moved * inner.height())
        })
        .collect();
    painter.add(Shape::line(line, Stroke::new(1.5, ink)));
}

/// The envelope's level, 0 to 1, at screen `x`.
fn level_at(points: &[Pos2; 5], x: f32, inner: Rect) -> f32 {
    let level = |p: Pos2| ((inner.bottom() - p.y) / inner.height()).clamp(0.0, 1.0);
    for pair in points.windows(2) {
        if x <= pair[1].x {
            let span = (pair[1].x - pair[0].x).max(1e-3);
            let t = ((x - pair[0].x) / span).clamp(0.0, 1.0);
            return level(pair[0]) + (level(pair[1]) - level(pair[0])) * t;
        }
    }
    0.0
}
