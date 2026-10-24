//! What a set keeps of the Clip View's MPE tab (the MPE spec, R23 and R27):
//! which expression lanes show and how tall they are, and the clips whose
//! tab has its own grid on.
//!
//! Saved with the set beside the document, like the folds, so changing any
//! of it is no undo step.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::ClipFolds;

/// One of the MPE tab's expression lanes, in the order they stack
/// (the MPE spec, R23, R24).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A lane of the Clip View's MPE tab: `pitch_bend` (first, and only on a track \
                   whose instrument is a Drum Rack), `slide`, `pressure`, `velocity` or \
                   `release_velocity`."
)]
pub enum MpeLane {
    /// The Pitch envelopes of a Drum Rack's notes, from −48 to +48 semitones.
    PitchBend,
    /// Each note's Slide envelope, 0 to 127.
    Slide,
    /// Each note's Pressure envelope, 0 to 127.
    Pressure,
    /// Each note's velocity, 1 to 127.
    Velocity,
    /// Each note's release velocity, 1 to 127.
    ReleaseVelocity,
}

impl MpeLane {
    /// The four lanes the selector toggles switch, in the order they stack.
    pub const TOGGLED: [Self; 4] = [
        Self::Slide,
        Self::Pressure,
        Self::Velocity,
        Self::ReleaseVelocity,
    ];

    /// What the lane is called.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PitchBend => "Pitch Bend",
            Self::Slide => "Slide",
            Self::Pressure => "Pressure",
            Self::Velocity => "Velocity",
            Self::ReleaseVelocity => "Release Velocity",
        }
    }
}

/// How tall a lane is when its height was never changed, in points
/// (L-CLIP-014's lane).
pub const DEFAULT_LANE_HEIGHT: f32 = 43.0;

/// The MPE tab's saved state. A new set shows Slide and Pressure, with every
/// clip's grid off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MpeView {
    /// The lanes whose toggle is on.
    pub shown: BTreeSet<MpeLane>,
    /// Whether the triangle has hidden the lanes and their toggles; the
    /// toggles' states are kept for when it shows them again.
    pub hidden: bool,
    /// The heights of the lanes dragged to another height, in points.
    pub heights: BTreeMap<MpeLane, f32>,
    /// The clips whose MPE tab has its grid on.
    pub grid: ClipFolds,
}

impl Default for MpeView {
    fn default() -> Self {
        Self {
            shown: BTreeSet::from([MpeLane::Slide, MpeLane::Pressure]),
            hidden: false,
            heights: BTreeMap::new(),
            grid: ClipFolds::new(),
        }
    }
}

impl MpeView {
    /// Whether this is what a new set holds, so a set's saved form leaves
    /// it out.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The lanes drawn now, from the top: the Pitch Bend lane first on a
    /// Drum Rack's track (R24), then those toggled on; none while the
    /// triangle has hidden them.
    #[must_use]
    pub fn lanes(&self, drum_rack: bool) -> Vec<MpeLane> {
        if self.hidden {
            return Vec::new();
        }
        drum_rack
            .then_some(MpeLane::PitchBend)
            .into_iter()
            .chain(
                MpeLane::TOGGLED
                    .into_iter()
                    .filter(|l| self.shown.contains(l)),
            )
            .collect()
    }

    /// How tall `lane` is, in points.
    #[must_use]
    pub fn height(&self, lane: MpeLane) -> f32 {
        self.heights
            .get(&lane)
            .copied()
            .unwrap_or(DEFAULT_LANE_HEIGHT)
    }

    /// Whether the lane's selector toggle is on and the toggles show.
    #[must_use]
    pub fn is_shown(&self, lane: MpeLane) -> bool {
        self.shown.contains(&lane) && !self.hidden
    }

    /// Switches a lane's toggle. Switching one on brings the lanes back if
    /// the triangle had hidden them.
    pub fn show(&mut self, lane: MpeLane, on: bool) {
        if lane == MpeLane::PitchBend {
            return;
        }
        if on {
            self.shown.insert(lane);
            self.hidden = false;
        } else {
            self.shown.remove(&lane);
        }
    }

    /// The lane header menu's swap: the lane goes off and the first lane
    /// after it that is off goes on. Returns the lane now on, or `None`
    /// when every lane is on already.
    pub fn swap(&mut self, lane: MpeLane) -> Option<MpeLane> {
        let at = MpeLane::TOGGLED.iter().position(|l| *l == lane)?;
        let next = (1..MpeLane::TOGGLED.len())
            .map(|step| MpeLane::TOGGLED[(at + step) % MpeLane::TOGGLED.len()])
            .find(|other| !self.shown.contains(other))?;
        self.shown.remove(&lane);
        self.shown.insert(next);
        self.hidden = false;
        Some(next)
    }

    /// Shows every lane (the triangle clicked with Option or Alt, or with
    /// every toggle off).
    pub fn show_all(&mut self) {
        self.shown.extend(MpeLane::TOGGLED);
        self.hidden = false;
    }

    /// The triangle: hides the shown lanes, shows them again when hidden,
    /// and shows all four when none is on or `all` is set (R23, I-CLICK-048).
    pub fn triangle(&mut self, all: bool) {
        if all || (!self.hidden && self.shown.is_empty()) {
            self.show_all();
        } else {
            self.hidden = !self.hidden;
        }
    }

    /// Whether the clip's MPE tab has its grid on.
    #[must_use]
    pub fn grid_on(&self, clip: crate::ClipId) -> bool {
        self.grid.contains(&clip)
    }

    /// Switches the clip's MPE tab grid.
    pub fn set_grid(&mut self, clip: crate::ClipId, on: bool) {
        if on {
            self.grid.insert(clip);
        } else {
            self.grid.remove(&clip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: mpe R23
    #[test]
    fn a_new_set_shows_slide_and_pressure_and_the_triangle_hides_and_shows_them() {
        let mut view = MpeView::default();
        assert_eq!(view.lanes(false), [MpeLane::Slide, MpeLane::Pressure]);
        assert_eq!(
            view.lanes(true),
            [MpeLane::PitchBend, MpeLane::Slide, MpeLane::Pressure]
        );
        view.triangle(false);
        assert!(view.lanes(true).is_empty());
        view.triangle(false);
        assert_eq!(view.lanes(false), [MpeLane::Slide, MpeLane::Pressure]);
    }

    // Spec: mpe R23, I-CLICK-048
    #[test]
    fn the_triangle_shows_all_four_with_option_or_with_every_toggle_off() {
        let mut view = MpeView::default();
        view.triangle(true);
        assert_eq!(view.lanes(false), MpeLane::TOGGLED);
        view.show(MpeLane::Slide, false);
        view.show(MpeLane::Pressure, false);
        view.show(MpeLane::Velocity, false);
        view.show(MpeLane::ReleaseVelocity, false);
        assert!(view.lanes(false).is_empty());
        view.triangle(false);
        assert_eq!(view.lanes(false), MpeLane::TOGGLED);
    }

    // Spec: mpe R23
    #[test]
    fn a_swap_turns_a_lane_off_and_the_next_lane_that_is_off_on() {
        let mut view = MpeView::default();
        assert_eq!(view.swap(MpeLane::Slide), Some(MpeLane::Velocity));
        assert_eq!(view.lanes(false), [MpeLane::Pressure, MpeLane::Velocity]);
        view.show_all();
        assert_eq!(view.swap(MpeLane::Slide), None, "nothing is off");
    }

    // Spec: mpe R23, R27
    #[test]
    fn the_default_view_is_left_out_of_a_saved_set() {
        let mut view = MpeView::default();
        assert!(view.is_default());
        view.set_grid(crate::ClipId(bl_core::id::Id::new(3).unwrap()), true);
        assert!(!view.is_default());
        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(serde_json::from_str::<MpeView>(&json).unwrap(), view);
    }
}
