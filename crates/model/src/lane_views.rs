//! What a set keeps of the Arrangement's automation lanes (the automation
//! spec, R24 to R26 and R33): what each track's main lane shows, the lanes of
//! its own under the track and whether they show, and each lane's filter.
//!
//! Saved with the set beside the document, so changing any of it is no undo
//! step. A track with no entry has its main lane showing its volume and no
//! lanes of its own, which is what a set saved before lanes of their own
//! opens with.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::automation::{AutomationTarget, MixerParam};
use crate::ids::TrackId;

/// What one automation lane shows: the parameter its choosers picked and
/// whether they list only automated parameters (R27).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LaneChoice {
    /// The parameter shown, or `null` while the lane draws no envelope.
    pub target: Option<AutomationTarget>,
    /// Whether the lane's choosers list only parameters with an envelope.
    pub only_automated: bool,
}

impl LaneChoice {
    /// A lane showing `target`, with its choosers unfiltered.
    #[must_use]
    pub const fn showing(target: Option<AutomationTarget>) -> Self {
        Self {
            target,
            only_automated: false,
        }
    }
}

impl Default for LaneChoice {
    /// A main lane's choice until the choosers pick another: the volume.
    fn default() -> Self {
        Self::showing(Some(AutomationTarget::Mixer {
            param: MixerParam::Volume,
        }))
    }
}

/// One track's automation lanes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TrackLanes {
    /// The main lane, which draws over the track's clips (R24).
    pub main: LaneChoice,
    /// The lanes of its own, top to bottom (R25).
    pub lanes: Vec<LaneChoice>,
    /// Whether the lanes of its own are hidden by the header's toggle (R26).
    pub hidden: bool,
}

impl TrackLanes {
    /// The choice of lane `lane`: 0 is the main lane, and 1 and up the lanes
    /// of its own.
    #[must_use]
    pub fn choice(&self, lane: usize) -> Option<&LaneChoice> {
        match lane {
            0 => Some(&self.main),
            n => self.lanes.get(n - 1),
        }
    }

    /// The choice of lane `lane`, to change.
    pub fn choice_mut(&mut self, lane: usize) -> Option<&mut LaneChoice> {
        match lane {
            0 => Some(&mut self.main),
            n => self.lanes.get_mut(n - 1),
        }
    }

    /// The number of the lane that shows `target`, main lane first.
    #[must_use]
    pub fn lane_showing(&self, target: AutomationTarget) -> Option<usize> {
        (0..=self.lanes.len())
            .find(|lane| self.choice(*lane).and_then(|choice| choice.target) == Some(target))
    }
}

/// The tracks' lanes, as a set saves them. A track whose lanes are all as a
/// new track's have no entry.
pub type LaneViews = BTreeMap<TrackId, TrackLanes>;
