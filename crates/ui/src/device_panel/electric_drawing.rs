//! The mechanism drawn at the left of Electric's display (layout L-DEV-102):
//! the damper, the tine with its tone bar, the pickup under it and the hammer
//! arm, in Beatlayer's own line style. The parts the shown section's controls
//! act on are in the accent, the others in the display's grey.

use egui::{Color32, Painter, Pos2, Shape, Stroke, pos2};

/// The sections of Electric's display, as its pages name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Hammer,
    Fork,
    DamperPickup,
    Global,
}

impl Section {
    /// The section the page called `page` shows.
    #[must_use]
    pub fn of(page: &str) -> Self {
        match page {
            "Fork" => Self::Fork,
            "Damper/Pickup" => Self::DamperPickup,
            "Global" => Self::Global,
            _ => Self::Hammer,
        }
    }
}

/// Draws the mechanism with its cell's top-left corner at `origin`: `lit` is
/// the ink of the parts the section acts on, `dim` the others'.
pub fn draw(painter: &Painter, origin: Pos2, section: Section, (lit, dim): (Color32, Color32)) {
    let at = |x: f32, y: f32| pos2(origin.x + x, origin.y + y);
    let ink = |on: bool| Stroke::new(1.5, if on { lit } else { dim });
    let line = |points: &[(f32, f32)], stroke: Stroke| {
        painter.add(Shape::line(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            stroke,
        ));
    };
    let closed = |points: &[(f32, f32)], stroke: Stroke| {
        painter.add(Shape::closed_line(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            stroke,
        ));
    };
    // The damper: a small cylinder at the left.
    let damper = ink(section == Section::DamperPickup);
    closed(
        &[(11.8, 33.3), (27.8, 33.3), (27.8, 43.9), (11.8, 43.9)],
        damper,
    );
    line(&[(15.5, 33.3), (15.5, 43.9)], damper);
    line(&[(24.1, 33.3), (24.1, 43.9)], damper);
    line(&[(27.8, 38.6), (31.4, 38.6)], damper);
    // The tine, with the cap of the fork at its end, over the line the
    // pickup's block hangs from.
    let tine = ink(section == Section::Fork);
    closed(
        &[(31.4, 26.4), (75.5, 26.4), (75.5, 34.0), (31.4, 34.0)],
        tine,
    );
    closed(
        &[(75.5, 27.0), (86.0, 27.0), (86.0, 41.0), (75.5, 41.0)],
        tine,
    );
    line(&[(78.4, 27.0), (78.4, 41.0)], tine);
    line(&[(31.4, 38.9), (86.3, 38.9)], tine);
    // The pickup under the tine.
    let pickup = ink(section == Section::DamperPickup);
    closed(
        &[(54.2, 38.9), (63.2, 38.9), (63.2, 43.4), (54.2, 43.4)],
        pickup,
    );
    // The hammer's arm and head.
    let hammer = ink(section == Section::Hammer);
    closed(
        &[
            (31.8, 50.0),
            (37.3, 43.6),
            (68.0, 57.0),
            (71.5, 46.0),
            (80.0, 47.4),
            (75.0, 63.0),
            (66.0, 63.0),
        ],
        hammer,
    );
}
