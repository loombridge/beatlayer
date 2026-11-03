//! The line drawings in the Drum Synths' displays (layout L-DEV-047): a kick
//! drum with its beater, a snare, a tom and a pair of hands, drawn in
//! Beatlayer's own line style in the display's accent. Each is made in a box
//! about 40 by 36 points around its centre.

use std::f32::consts::{PI, TAU};

use egui::{Color32, Painter, Pos2, Shape, Stroke, pos2};

/// The drawings: which Drum Synth's display draws which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drawing {
    Kick,
    Snare,
    Tom,
    Clap,
}

impl Drawing {
    /// The drawing of the Drum Synth whose device type is `device_type`.
    #[must_use]
    pub fn of(device_type: &str) -> Option<Self> {
        match device_type {
            bl_devices::ds_kick::DS_KICK => Some(Self::Kick),
            bl_devices::ds_snare::DS_SNARE => Some(Self::Snare),
            bl_devices::ds_tom::DS_TOM => Some(Self::Tom),
            bl_devices::ds_clap::DS_CLAP => Some(Self::Clap),
            _ => None,
        }
    }
}

/// Draws `drawing` around `centre` in `ink`.
pub fn draw(painter: &Painter, drawing: Drawing, centre: Pos2, ink: Color32) {
    let stroke = Stroke::new(1.5, ink);
    let at = |x: f32, y: f32| pos2(centre.x + x, centre.y + y);
    let line = |points: Vec<Pos2>| painter.add(Shape::line(points, stroke));
    // A piece of an ellipse from angle `from` to `to`, in turns of a circle.
    let arc = |x: f32, y: f32, (rx, ry): (f32, f32), (from, to): (f32, f32)| -> Vec<Pos2> {
        (0..=24)
            .map(|i| {
                let a = (from + (to - from) * i as f32 / 24.0) * TAU;
                at(x + rx * a.cos(), y + ry * a.sin())
            })
            .collect()
    };
    match drawing {
        Drawing::Kick => {
            // The front head, the shell back to the far rim, and the beater.
            line(arc(-6.0, 0.0, (7.0, 16.0), (0.0, 1.0)));
            line(vec![at(-6.0, -16.0), at(10.0, -16.0)]);
            line(vec![at(-6.0, 16.0), at(10.0, 16.0)]);
            line(arc(10.0, 0.0, (7.0, 16.0), (-0.25, 0.25)));
            line(vec![at(-18.0, 15.0), at(-9.0, 3.0)]);
            painter.add(Shape::circle_stroke(at(-9.0, 3.0), 2.0, stroke));
        }
        Drawing::Snare => drum(painter, stroke, centre, (17.0, 5.0), (-5.0, 8.0)),
        Drawing::Tom => drum(painter, stroke, centre, (15.0, 5.0), (-11.0, 11.0)),
        Drawing::Clap => {
            // Two hands meeting at an angle, and three strokes of the sound.
            let hand = |flip: f32| {
                let mut points: Vec<Pos2> = [
                    (-14.0, 16.0),
                    (-17.0, 6.0),
                    (-12.0, -4.0),
                    (-5.0, -10.0),
                    (1.0, -7.0),
                    (-2.0, 1.0),
                    (-6.0, 8.0),
                    (-9.0, 16.0),
                ]
                .iter()
                .map(|&(x, y)| at(flip * x + 3.0 * (flip + 1.0), y))
                .collect();
                points.push(points[0]);
                points
            };
            line(hand(1.0));
            line(hand(-1.0));
            for (x, y) in [(-6.0, -16.0), (3.0, -18.0), (12.0, -14.0)] {
                line(vec![at(x, y), at(x + 2.0, y + 4.0)]);
            }
        }
    }
}

/// A drum seen a little from above: its top head, its shell and the rods
/// between the rims. `radii` are the head's across and up, and `span` the
/// heights of the head's centre and the foot rim's.
fn drum(painter: &Painter, stroke: Stroke, centre: Pos2, radii: (f32, f32), span: (f32, f32)) {
    let (rx, ry) = radii;
    let (top, foot) = span;
    let at = |x: f32, y: f32| pos2(centre.x + x, centre.y + y);
    let ellipse = |y: f32, from: f32, to: f32| -> Vec<Pos2> {
        (0..=24)
            .map(|i| {
                let a = from + (to - from) * i as f32 / 24.0;
                at(rx * a.cos(), y + ry * a.sin())
            })
            .collect()
    };
    painter.add(Shape::closed_line(
        ellipse(top, 0.0, TAU - TAU / 24.0),
        stroke,
    ));
    painter.add(Shape::line(ellipse(foot, 0.0, PI), stroke));
    painter.add(Shape::line(vec![at(-rx, top), at(-rx, foot)], stroke));
    painter.add(Shape::line(vec![at(rx, top), at(rx, foot)], stroke));
    // The rods, a zigzag between the rims.
    let steps = 6;
    let zigzag = (0..=steps)
        .map(|i| {
            let x = -rx + 2.0 * rx * i as f32 / steps as f32;
            let y = if i % 2 == 0 { top + ry } else { foot };
            at(x, y)
        })
        .collect();
    painter.add(Shape::line(zigzag, stroke));
}
