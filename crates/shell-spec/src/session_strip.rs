//! The relative Session mapping strip's commands and New (WP-7.3b; the map
//! modes spec, R14 to R16 and R20): highlighting a scene, launching the
//! highlighted scene or a track's slot at it, and the New button.
//!
//! The highlighted scene is the Session View's selected scene. `bl-shell`
//! runs these commands. Cancelling a scene's launch is `session.cancel_scene_launch`
//! of the engine's session commands.

use bl_commands::Context::{Global, Session};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{SceneId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `session.highlight_scene`: Scene Up, Scene Down and the scene number box.
pub const HIGHLIGHT_SCENE: &str = "session.highlight_scene";
/// `session.launch_highlighted_scene`: the strip's Launch Scene button.
pub const LAUNCH_HIGHLIGHTED_SCENE: &str = "session.launch_highlighted_scene";
/// `session.launch_highlighted`: a track's Launch button in the strip.
pub const LAUNCH_HIGHLIGHTED: &str = "session.launch_highlighted";
/// `session.new_take`: the New button.
pub const NEW_TAKE: &str = "session.new_take";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[
    HIGHLIGHT_SCENE,
    LAUNCH_HIGHLIGHTED_SCENE,
    LAUNCH_HIGHLIGHTED,
    NEW_TAKE,
];

/// Whether `id` is one of the strip's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `session.highlight_scene`: a step or a number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HighlightArgs {
    /// How many scenes to move the highlight: 1 for Scene Down, -1 for Scene
    /// Up, as many as a relative controller's increment says. Stops at the
    /// first and the last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<i32>,
    /// The number of the scene to highlight, from 1 to the scene count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
}

/// What `session.highlight_scene` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Highlighted {
    /// The highlighted scene.
    pub scene: SceneId,
    /// Its number, counting from 1.
    pub number: u32,
}

/// Arguments of `session.launch_highlighted`: the track whose slot at the
/// highlighted scene is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchHighlightedArgs {
    /// The track.
    pub track: TrackId,
    /// The velocity of the press, from 1 to 127; 127 if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<u8>,
    /// The semitones to transpose the clip by, as `session.launch` takes
    /// them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transpose: Option<i8>,
}

/// What `session.new_take` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NewTake {
    /// The scene it highlighted.
    pub scene: SceneId,
    /// Whether it inserted that scene.
    pub inserted: bool,
    /// The armed tracks whose clips it stopped.
    pub stopped: Vec<TrackId>,
}

/// Adds the strip's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(HIGHLIGHT_SCENE, "Highlight Scene")
            .description(
                "Highlights a scene of the Session View, which is selecting it: `step` moves \
                 the highlight that many scenes down (negative up), stopping at the first and \
                 the last, and `number` highlights the scene of that number, from 1 to the \
                 scene count. With no scene selected a step highlights the first. The grid \
                 scrolls so the highlighted scene sits at the vertical centre of the slot \
                 area, as far as the scenes reach. These are the actions of the relative \
                 Session mapping strip's Scene Up, Scene Down and scene number box. Read the \
                 highlighted scene back in `session.state`.",
            )
            .permission(Permission::Read)
            .contexts([Global, Session])
            .args::<HighlightArgs>()
            .result::<Highlighted>(),
    )?;
    registry.register(
        CommandSpec::new(LAUNCH_HIGHLIGHTED_SCENE, "Launch Highlighted Scene")
            .description(
                "Launches the highlighted scene, as its own launch button does; with Select \
                 Next Scene on Launch on, the highlight then moves on to the next scene. \
                 Refused while no scene is highlighted. It changes nothing in the set.",
            )
            .permission(Permission::Read)
            .contexts([Global, Session])
            .args::<NoArgs>()
            .result::<()>(),
    )?;
    registry.register(
        CommandSpec::new(LAUNCH_HIGHLIGHTED, "Launch Highlighted")
            .description(
                "Presses the launch button of the slot at the highlighted scene in `track`, as \
                 `session.launch` does for a slot, with an optional `velocity` and `transpose`; \
                 a track's Launch button of the relative Session mapping strip runs it. \
                 `session.release` lets go of the slot. Refused while no scene is highlighted. \
                 It changes nothing in the set.",
            )
            .permission(Permission::Read)
            .contexts([Global, Session])
            .args::<LaunchHighlightedArgs>()
            .result::<()>(),
    )?;
    registry.register(
        CommandSpec::new(NEW_TAKE, "New")
            .description(
                "The New button, which shows in the Control Bar only in MIDI Map Mode and Key \
                 Map Mode. Stops the clips of every armed track, as their stop buttons do, and \
                 highlights the first scene after the highlighted one whose slots on the armed \
                 tracks are all empty, inserting a scene after the last when there is none, \
                 which is one undo step. With no scene highlighted it starts from the first.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Session])
            .args::<NoArgs>()
            .result::<NewTake>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for id in IDS {
            let spec = registry.get(id).unwrap_or_else(|| panic!("{id}"));
            assert!(spec.permission.is_some(), "{id}");
            assert!(spec.args.is_some() && spec.result.is_some(), "{id}");
            assert!(!spec.description.is_empty(), "{id}");
        }
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
    }
}
