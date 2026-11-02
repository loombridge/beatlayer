//! The chord lane's view commands (WP-6.48; the chord lane spec, R1 and R3):
//! showing and hiding the lane, and selecting a region.
//!
//! The regions are part of the set and their edits are `bl-model`'s
//! `chord_lane.*` commands, each one undo step. What these two change is the
//! view only, so they are in the read class and no undo step. [`register`]
//! adds their specs, which are pure data; `bl-shell` runs them by ID.

use bl_commands::Context::Arrangement;
use bl_commands::{CommandSpec, RegisterError, Registry};
use bl_model::ChordRegionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::arrangement::ArrangementState;

/// `view.chord_lane`: show or hide the chord lane.
pub const VIEW_CHORD_LANE: &str = "view.chord_lane";
/// `chord_lane.select_region`: select a chord region.
pub const SELECT_REGION: &str = "chord_lane.select_region";

/// `chord_lane.add_region` (`bl-model`): add a region, as drawing one does.
pub const ADD_REGION: &str = "chord_lane.add_region";
/// `chord_lane.move_region` (`bl-model`): move or resize a region.
pub const MOVE_REGION: &str = "chord_lane.move_region";
/// `chord_lane.set_region` (`bl-model`): set a region's scale and transposition.
pub const SET_REGION: &str = "chord_lane.set_region";
/// `chord_lane.delete_region` (`bl-model`): delete a region.
pub const DELETE_REGION: &str = "chord_lane.delete_region";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[VIEW_CHORD_LANE, SELECT_REGION];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `view.chord_lane`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChordLaneArgs {
    /// Whether the lane shows. Left out, the switch flips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// Arguments of `chord_lane.select_region`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectRegionArgs {
    /// The region. Left out, no region is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<ChordRegionId>,
}

/// Adds the chord lane's view commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(VIEW_CHORD_LANE, "Chord Lane")
            .description(
                "Shows or hides the chord lane, the Arrangement's lane of regions that give \
                 the tracks that follow it a scale and a transposition (View > Chord Lane). \
                 `on` sets it; left out, the switch flips. The lane is hidden in a new set. \
                 Changes only the view: no undo step, and it is saved with the set. The \
                 result is the Arrangement's state, whose `chord_lane` says whether the lane \
                 shows. Hiding the lane doesn't change how the tracks play: the regions stay.",
            )
            .permission(bl_commands::Permission::Read)
            .context(Arrangement)
            .args::<ChordLaneArgs>()
            .result::<ArrangementState>(),
    )?;
    registry.register(
        CommandSpec::new(SELECT_REGION, "Select Chord Region")
            .description(
                "Selects the chord region `region`, as clicking it does; left out, no region \
                 is selected. The Delete key deletes the selected region. Selecting is never \
                 an undo step and changes nothing in the set. `arrangement.state` reports the \
                 region as `selected_chord_region`.",
            )
            .permission(bl_commands::Permission::Read)
            .context(Arrangement)
            .args::<SelectRegionArgs>()
            .result::<ArrangementState>(),
    )
}
