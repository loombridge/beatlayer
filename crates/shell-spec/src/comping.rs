//! Comping (WP-6.21): inserting and duplicating take lanes, and building a
//! comp from them (copying a take lane's material to the main lane, replacing
//! it with the next or previous take, and moving the boundary between two
//! parts of a comp), in the command registry (decisions D16 and D17). The
//! behaviour is in the comping spec, R4, R5 and R17 to R21; the commands
//! that rename, delete, move and audition take lanes are the document's
//! (`bl-model`).
//!
//! [`register`] adds their specs, and `bl-shell` runs them.

use bl_commands::Context::{Arrangement, TakeLanes};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::{ClipId, TakeLaneId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::editing::Position;

/// `take_lane.insert`: add an empty take lane to each of the tracks.
pub const INSERT: &str = "take_lane.insert";
/// `take_lane.duplicate`: copy take lanes, with their clips.
pub const DUPLICATE: &str = "take_lane.duplicate";
/// `take_lane.copy_to_main_lane`: copy a take lane's material in a range to
/// the main lane.
pub const COPY_TO_MAIN_LANE: &str = "take_lane.copy_to_main_lane";
/// `take_lane.replace_with_take`: replace the main lane's material in a range
/// with the next or previous take's.
pub const REPLACE_WITH_TAKE: &str = "take_lane.replace_with_take";
/// `take_lane.move_comp_edge`: move the boundary between two parts of a comp.
pub const MOVE_COMP_EDGE: &str = "take_lane.move_comp_edge";
/// `take_lane.comp_sources`: which take-lane material the main lane uses.
pub const COMP_SOURCES: &str = "take_lane.comp_sources";
/// `take_lane.audition`: audition the selected take lanes (T).
pub const AUDITION: &str = "take_lane.audition";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    INSERT,
    DUPLICATE,
    COPY_TO_MAIN_LANE,
    REPLACE_WITH_TAKE,
    MOVE_COMP_EDGE,
    COMP_SOURCES,
    AUDITION,
];

/// Whether `id` is one of the comping commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `take_lane.insert`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertArgs {
    /// The audio or MIDI tracks to add a take lane to. Left out, the
    /// selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// Take lanes to add the new ones directly after, for the tracks they
    /// belong to. A track with none of them gets its new lane after its last
    /// take lane.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<TakeLaneId>,
    /// Whether it reaches the tracks linked with the ones it names (the
    /// linked-track editing spec, R15). `true` if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

/// Arguments of `take_lane.duplicate`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateArgs {
    /// The take lanes to copy. Left out, the take lanes whose headers are
    /// selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<TakeLaneId>>,
}

/// Arguments of `take_lane.copy_to_main_lane`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopyToMainLaneArgs {
    /// The take lanes to copy from. Of those on one track, the last listed
    /// is copied from. Left out, the take lanes the selection is on: the
    /// headers selected, the lanes the time selection covers, or the lanes
    /// of the selected clips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<TakeLaneId>>,
    /// Where the range starts, in ticks or as `bar.beat.sixteenth` text.
    /// Left out, the time selection's start, or the selected clips'.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the range ends, after `start`. Left out, the selection's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
    /// Whether it reaches the tracks linked with the ones it names (the
    /// linked-track editing spec, R15). `true` if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

/// Which way `take_lane.replace_with_take` goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TakeDirection {
    /// The next take, the one below.
    Next,
    /// The previous take, the one above.
    Previous,
}

/// Arguments of `take_lane.replace_with_take`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceWithTakeArgs {
    /// The audio or MIDI track whose main lane is changed. Left out, each
    /// audio or MIDI track the selection is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// Where the range starts, in ticks or as `bar.beat.sixteenth` text.
    /// Left out, the time selection's start, or the selected clips'.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the range ends, after `start`. Left out, the selection's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
    /// Whether to take the next or the previous take.
    pub direction: TakeDirection,
    /// The take lane the selection is on, from which the next or previous
    /// take is counted. Left out with `track` given, the selection is on the
    /// main lane, and each of its clips counts from the take lane it uses;
    /// left out with `track`, the selection says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<TakeLaneId>,
    /// Whether it reaches the tracks linked with the ones it names (the
    /// linked-track editing spec, R15). `true` if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

/// Arguments of `take_lane.audition`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditionArgs {
    /// The take lanes to audition; of those on one track, the last listed
    /// is the one. Left out, the take lanes the selection is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Vec<TakeLaneId>>,
    /// Switch them on (true) or off (false). Left out, they switch on, or
    /// off when every one of them is auditioned already.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// Whether it reaches the tracks linked with the ones it names (the
    /// linked-track editing spec, R15). `true` if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

/// Arguments of `take_lane.move_comp_edge`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveCompEdgeArgs {
    /// The audio or MIDI track.
    pub track: TrackId,
    /// Where two adjacent main-lane clips meet now.
    pub from: Position,
    /// Where they should meet.
    pub to: Position,
    /// Whether it reaches the tracks linked with the ones it names (the
    /// linked-track editing spec, R15). `true` if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

/// Arguments of `take_lane.comp_sources`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompSourcesArgs {
    /// The audio or MIDI track.
    pub track: TrackId,
}

/// The take lanes `take_lane.insert` and `take_lane.duplicate` made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LanesResult {
    /// The new take lanes, in the order of the tracks and lanes given.
    pub lanes: Vec<TakeLaneId>,
}

/// What the comp commands made on the main lane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompResult {
    /// The new main-lane clips.
    pub created: Vec<ClipId>,
    /// Whether the undo history gained a step.
    pub undo_step: bool,
}

/// A stretch of a main-lane clip that plays what a take-lane clip plays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsedStretch {
    /// The main-lane clip.
    pub clip: ClipId,
    /// The take lane of the clip it plays.
    pub lane: TakeLaneId,
    /// The take-lane clip.
    pub source: ClipId,
    /// Where the stretch starts, in ticks.
    pub start: i64,
    /// Where it ends, in ticks.
    pub end: i64,
}

/// What `take_lane.comp_sources` reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompSourcesResult {
    /// Each stretch of the main lane that plays take-lane material, in
    /// main-lane order. A stretch played by identical clips on several take
    /// lanes is listed once for each.
    pub stretches: Vec<UsedStretch>,
    /// The take lanes none of whose material the main lane uses.
    pub unused_lanes: Vec<TakeLaneId>,
}

/// Adds the comping commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs() {
        registry.register(spec)?;
    }
    Ok(())
}

fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(INSERT, "Insert Take Lane")
            .description(
                "Adds an empty take lane to each of the audio or MIDI `tracks` (left out, the \
                 selected tracks): directly after the take lane of that track listed in \
                 `after` (left out, the take lanes whose headers are selected), or after its \
                 last take lane. Each is named `Take N`, one more than the highest number of a \
                 lane of the track named `Take` and a number, `Take 1` on a track with none. \
                 All of them are one undo step. The tracks' take lanes show afterwards, and \
                 Automation Mode is switched off to show them. The result's `lanes` are the new \
                 lanes' IDs.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .binding("S-COMP-004")
            .args::<InsertArgs>()
            .result::<LanesResult>(),
        CommandSpec::new(DUPLICATE, "Duplicate Take Lane(s)")
            .description(
                "Puts a copy of each of the take `lanes` (left out, the take lanes whose headers \
                 are selected) directly after it, with its name and a copy of each of its clips \
                 under a new ID, in one undo step, and selects the copies. The result's `lanes` \
                 are the copies' IDs.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, TakeLanes])
            .binding("S-COMP-005")
            .args::<DuplicateArgs>()
            .result::<LanesResult>(),
        CommandSpec::new(COPY_TO_MAIN_LANE, "Copy Selection to Main Lane")
            .description(
                "Copies a take lane's material in `start`..`end` to its track's main lane, at \
                 the same place (with `lanes`, `start` and `end` left out, it takes them from \
                 the selection: the take lanes the headers, the time selection or the clips \
                 selected are on, and the time or the clips' span; Enter runs it so): each of the lane's clips the range reaches, cut at the range's \
                 edges as Split cuts, is copied under a new ID onto the main lane, where it \
                 replaces what it covers. Where the lane has no clip in the range, the main \
                 lane is left as it was. Of several `lanes` on one track, the last listed is \
                 copied from; lanes on several tracks each copy to their own track. The copies \
                 are independent of the take-lane clips. It is one undo step, an edit, and \
                 destructive as well when it replaces or shortens a main-lane clip. A frozen \
                 track refuses it.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, TakeLanes])
            .binding("S-COMP-002")
            .args::<CopyToMainLaneArgs>()
            .result::<CompResult>(),
        CommandSpec::new(REPLACE_WITH_TAKE, "Replace with Next or Previous Take")
            .description(
                "Replaces a track's main-lane material in `start`..`end` with the next take's \
                 (`direction` `next`, the take lane below) or the previous take's (`previous`, \
                 the one above). With `track`, `start`, `end` and `lane` left out it takes them \
                 from the selection, on each audio or MIDI track the selection is on, as \
                 Cmd+Down and Cmd+Up do. The take is placed as `take_lane.copy_to_main_lane` places it, skipping \
                 take lanes with no clip in the range. With `lane` given, the selection is on \
                 that take lane and the next or previous take is counted from it; left out, \
                 the selection is on the main lane, and each main-lane clip's part of the \
                 range is counted from the take lane it uses (`take_lane.comp_sources`), a \
                 part that uses none from before the first take lane. A part with no take \
                 lane left in that direction is left as it was. It is one undo step, an edit, \
                 and destructive as well when it replaces or shortens a main-lane clip. A \
                 frozen track refuses it.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<ReplaceWithTakeArgs>()
            .result::<CompResult>(),
        CommandSpec::new(MOVE_COMP_EDGE, "Move Comp Edge")
            .description(
                "Moves the boundary between two adjacent main-lane clips that each play a \
                 take-lane clip (`take_lane.comp_sources`) from `from` to `to`: the earlier \
                 clip's right edge and the later clip's left edge move together, each playing \
                 on what its take-lane clip plays there. The boundary goes no further than the \
                 growing clip's take-lane clip reaches, and leaves each clip at least a tick \
                 long. It is one undo step.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<MoveCompEdgeArgs>()
            .result::<CompResult>(),
        CommandSpec::new(AUDITION, "Audition Take Lane")
            .description(
                "Switches the audition of the take `lanes` (left out, the take lanes the \
                 selection is on: the headers, the time selection or the clips selected are on \
                 them) on or off, one lane for each track: of the lanes listed on one track, the \
                 last. Left without `on`, it switches them on, or off when each is auditioned \
                 already. While a take lane is auditioned its track plays it in place of its \
                 main lane (`take_lane.set_audition`). One undo step, `Audition Take Lane`. T \
                 runs it.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, TakeLanes])
            .binding("S-COMP-003")
            .args::<AuditionArgs>()
            .result::<LanesResult>(),
        CommandSpec::new(COMP_SOURCES, "Comp Sources")
            .description(
                "Reports which take-lane material a track's main lane uses: a main-lane clip \
                 uses a take-lane clip over the stretch where both lie when they play the same \
                 thing there, the same data apart from the Start and End markers, the loop \
                 brace and the fades, with the same clip envelopes and the same contents at \
                 every point. A deactivated main-lane clip uses nothing. `stretches` lists \
                 each, and `unused_lanes` the take lanes none of whose material is used, \
                 which `take_lane.delete_unused` deletes.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .args::<CompSourcesArgs>()
            .result::<CompSourcesResult>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        for id in IDS {
            assert!(registry.contains(id), "{id}");
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
        let class = |id: &str| registry.get(id).unwrap().permission;
        assert_eq!(class(COMP_SOURCES), Some(Permission::Read));
        assert_eq!(class(COPY_TO_MAIN_LANE), Some(Permission::Edit));
    }
}
