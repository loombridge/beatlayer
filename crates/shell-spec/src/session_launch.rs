//! The Session View's launch quantisation and stop commands (WP-3.2): the
//! global launch quantisation the Control Bar's Quantization Menu shows
//! (L-CTRL-010), its enabled switch, and stopping the clips of the tracks
//! with a selected slot (S-SES-006), and launching the selection (S-SES-001).
//!
//! The global launch quantisation is saved with the set beside the song,
//! like the view settings, and changing it is no undo step; `bl-shell` runs
//! these commands, keeps the value, and tells the engine it plays the set
//! through. Launching a slot or a scene, and stopping a track or every
//! track, are the engine's own commands: `bl_engine::session`.

use bl_commands::Context::{Global, Session};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{LaunchQuantization, LaunchSettings};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `session.set_quantization`: set the global launch quantisation (S-GQ-001
/// to S-GQ-005 set common ones).
pub const SET_QUANTIZATION: &str = "session.set_quantization";
/// `session.smaller_quantization`: one step smaller down the menu's list
/// (S-SES-024, M-OPT-011).
pub const SMALLER_QUANTIZATION: &str = "session.smaller_quantization";
/// `session.larger_quantization`: one step larger up the menu's list
/// (S-SES-025, M-OPT-012).
pub const LARGER_QUANTIZATION: &str = "session.larger_quantization";
/// `session.triplet_quantization`: the same note value as a triplet, or
/// back (S-SES-026, M-OPT-013).
pub const TRIPLET_QUANTIZATION: &str = "session.triplet_quantization";
/// `session.set_quantization_enabled`: turn quantised launching on or off
/// (S-SES-027, M-OPT-014).
pub const SET_QUANTIZATION_ENABLED: &str = "session.set_quantization_enabled";
/// `session.stop_selected`: stop the clips of the tracks with a selected
/// slot (S-SES-006).
pub const STOP_SELECTED: &str = "session.stop_selected";

/// `session.launch_selected`: launch the selected clips and scenes
/// (S-SES-001).
pub const LAUNCH_SELECTED: &str = "session.launch_selected";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    SET_QUANTIZATION,
    SMALLER_QUANTIZATION,
    LARGER_QUANTIZATION,
    TRIPLET_QUANTIZATION,
    SET_QUANTIZATION_ENABLED,
    STOP_SELECTED,
    LAUNCH_SELECTED,
];

/// Whether `id` is one of the launch quantisation's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `session.set_quantization`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetQuantizationArgs {
    /// The step launches and stops wait for.
    pub quantization: LaunchQuantization,
}

/// Arguments of `session.set_quantization_enabled`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetQuantizationEnabledArgs {
    /// Whether launches and stops are quantised; left out, the switch goes
    /// to the other state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Adds the launch quantisation's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(SET_QUANTIZATION, "Set Global Launch Quantization")
            .description(
                "Sets the global launch quantisation: the grid line a clip or a scene launch, \
                 or a stop, waits for while the song plays (`quantization`: `none`, a bar count \
                 like `2_bars`, or a note value from `half` to `thirty_second`, with `_triplet` \
                 forms). The Control Bar's Quantization Menu shows it, and Cmd+6 to Cmd+9 and \
                 Cmd+0 (Ctrl+6 to Ctrl+9 and Ctrl+0 on Windows and Linux) set the common ones. \
                 It's saved with the set beside the song and changing it is no undo step. A \
                 launch or a stop already queued keeps the frame it was queued for.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding(Binding::new("S-GQ-001").arg("quantization", "sixteenth"))
            .binding(Binding::new("S-GQ-002").arg("quantization", "eighth"))
            .binding(Binding::new("S-GQ-003").arg("quantization", "quarter"))
            .binding(Binding::new("S-GQ-004").arg("quantization", "1_bar"))
            .binding(Binding::new("S-GQ-005").arg("quantization", "none"))
            .args::<SetQuantizationArgs>()
            .result::<LaunchSettings>(),
    )?;
    registry.register(
        CommandSpec::new(SMALLER_QUANTIZATION, "Smaller Launch Quantization")
            .description(
                "Moves the global launch quantisation one step smaller down the Quantization \
                 Menu's list: none, 8 bars, 4 bars, 2 bars, 1 bar, 1/2, 1/2T, 1/4, 1/4T, 1/8, \
                 1/8T, 1/16, 1/16T, 1/32. Cmd+1 with the Session View focused, and Options ▸ \
                 Smaller Launch Quantization. The smallest step stays. Saved with the set, and \
                 no undo step.",
            )
            .permission(Permission::Read)
            .context(Session)
            .binding("S-SES-024")
            .args::<NoArgs>()
            .result::<LaunchSettings>(),
    )?;
    registry.register(
        CommandSpec::new(LARGER_QUANTIZATION, "Larger Launch Quantization")
            .description(
                "Moves the global launch quantisation one step larger up the Quantization \
                 Menu's list: none, 8 bars, 4 bars, 2 bars, 1 bar, 1/2, 1/2T, 1/4, 1/4T, 1/8, \
                 1/8T, 1/16, 1/16T, 1/32. Cmd+2 with the Session View focused, and Options ▸ \
                 Larger Launch Quantization. The largest step stays. Saved with the set, and no \
                 undo step.",
            )
            .permission(Permission::Read)
            .context(Session)
            .binding("S-SES-025")
            .args::<NoArgs>()
            .result::<LaunchSettings>(),
    )?;
    registry.register(
        CommandSpec::new(TRIPLET_QUANTIZATION, "Triplet Launch Quantization")
            .description(
                "Makes the global launch quantisation the triplet of its note value, or the \
                 plain value again: 1/2 and 1/2T, 1/4 and 1/4T, 1/8 and 1/8T, 1/16 and 1/16T. \
                 `none` and the bar counts have no triplet, so it changes nothing while one of \
                 them is set. Cmd+3 with the Session View focused, and Options ▸ Triplet Launch \
                 Quantization. Saved with the set, and no undo step.",
            )
            .permission(Permission::Read)
            .context(Session)
            .binding("S-SES-026")
            .args::<NoArgs>()
            .result::<LaunchSettings>(),
    )?;
    registry.register(
        CommandSpec::new(SET_QUANTIZATION_ENABLED, "Launch Quantization Enabled")
            .description(
                "Turns quantised launching on or off (`enabled`; left out, the switch goes to \
                 the other state): off, a launch or a stop takes effect at once whatever the \
                 quantisation says. Cmd+4 with the Session View focused, and Options ▸ Launch \
                 Quantization Enabled, which shows a check mark while it's on. Saved with the \
                 set, and no undo step.",
            )
            .permission(Permission::Read)
            .context(Session)
            .binding(Binding::new("S-SES-027").toggles("enabled"))
            .args::<SetQuantizationEnabledArgs>()
            .result::<LaunchSettings>(),
    )?;
    registry.register(
        CommandSpec::new(STOP_SELECTED, "Stop the Selected Tracks' Clips")
            .description(
                "Stops the clip of every track that has a selected slot in the Session View, \
                 as Cmd+Enter (Ctrl+Enter on Windows and Linux) does: each stops on the next \
                 line of the launch quantisation grid, and launches still queued on those \
                 tracks are dropped. The transport keeps playing, and nothing in the set \
                 changes. Select a slot first.",
            )
            .permission(Permission::Read)
            .context(Session)
            .binding("S-SES-006")
            .args::<NoArgs>()
            .result::<()>(),
    )?;
    registry.register(
        CommandSpec::new(LAUNCH_SELECTED, "Launch the Selected Clips and Scenes")
            .description(
                "Launches every selected slot that holds a clip and every selected scene, as \
                 Enter does in the Session View: each runs `session.launch` or \
                 `session.launch_scene`, so it waits for the launch quantisation grid while the \
                 song plays and starts the transport at once while it's stopped. A selected \
                 empty slot is skipped. Select a slot or a scene first. It changes nothing in \
                 the set.",
            )
            .permission(Permission::Read)
            .context(Session)
            .binding("S-SES-001")
            .args::<NoArgs>()
            .result::<()>(),
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

    #[test]
    fn the_quantisation_commands_are_read_class() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for id in IDS {
            assert_eq!(
                registry.get(id).unwrap().permission,
                Some(Permission::Read),
                "{id}"
            );
        }
    }
}
