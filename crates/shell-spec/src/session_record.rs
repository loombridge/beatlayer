//! Recording into the Session View's slots (WP-3.11): the Session Record
//! button and its menu item and shortcut, a slot's own record button,
//! overdubbing the clip that plays, and ending the recording so the clip
//! loops.
//!
//! `bl-shell` runs these commands: it works out which slots record, gives
//! the engine its rings and tells it to start on the launch grid.

use bl_commands::Context::{Global, Session};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{SceneId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `session.record`: the Session Record button (L-CTRL-015, M-PLAY-009).
pub const RECORD: &str = "session.record";
/// `session.record_slot`: the Clip Record button in an empty slot of an
/// armed track.
pub const RECORD_SLOT: &str = "session.record_slot";
/// `session.overdub`: overdub the MIDI clip playing in a slot.
pub const OVERDUB: &str = "session.overdub";
/// `session.stop_recording`: end the recording; its new clips loop.
pub const STOP_RECORDING: &str = "session.stop_recording";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[RECORD, RECORD_SLOT, OVERDUB, STOP_RECORDING];

/// Whether `id` is one of the Session recording commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `session.record_slot` and `session.overdub`: the slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SlotArgs {
    /// The track whose slot it is.
    pub track: TrackId,
    /// The scene whose slot it is.
    pub scene: SceneId,
}

/// One slot a recording goes into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingSlot {
    /// The track whose slot it is.
    pub track: TrackId,
    /// The scene whose slot it is.
    pub scene: SceneId,
    /// Whether the slot's playing clip is overdubbed, rather than a new
    /// clip recorded.
    pub overdub: bool,
}

/// What `session.record`, `session.record_slot` and `session.overdub`
/// return: the slots the recording goes into, or none when `session.record`
/// ended a recording that was under way.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingStarted {
    /// The slots the recording goes into.
    pub slots: Vec<RecordingSlot>,
}

/// Adds the Session recording commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(RECORD, "Record to Session")
            .description(
                "The Session Record button (Playback ▸ Record to Session, Cmd+Shift+F9; \
                 Ctrl+Shift+F9 on Windows and Linux). Records every armed track into the \
                 Session View: a MIDI track whose clip is playing overdubs it, one note pass at \
                 a time; any other armed track records a new clip into the selected scene's \
                 slot if it is empty, else into the next empty slot below it on that track, and \
                 a scene is inserted at the end when there is none. The recording starts on the \
                 next line of the launch quantisation grid while the song plays, and from the \
                 playhead, after any count-in, while it is stopped, which starts the transport. \
                 The clip exists from the start and grows as it records. While a recording is \
                 under way the command ends it instead, as pressing the button again does: the \
                 new clips play on as loops from the next grid line. Audio goes to the \
                 project's Samples/Recorded/; the set must have been saved. Recording captures \
                 input devices, including a microphone, so the person is asked first. To end a \
                 recording without asking, run `session.stop_recording`.",
            )
            .permission(Permission::External)
            .contexts([Global, Session])
            .binding("S-SES-012")
            .binding("S-TRANS-008")
            .args::<NoArgs>()
            .result::<RecordingStarted>(),
    )?;
    registry.register(
        CommandSpec::new(RECORD_SLOT, "Record into Slot")
            .description(
                "The Clip Record button in an empty slot of an armed track: records a new clip \
                 into that slot (`track`, `scene`), starting on the next grid line, or from the \
                 playhead with the transport stopped. The track must be armed and the slot \
                 empty: a slot with a clip has a launch button instead, which \
                 `session.launch` presses. The clip exists from the start and grows as it \
                 records; `session.launch` on the recording slot, or `session.stop_recording`, \
                 ends the recording and loops the clip, and `session.stop_track` ends it \
                 without looping. Audio goes to the project's Samples/Recorded/. Recording \
                 captures input devices, including a microphone, so the person is asked first.",
            )
            .permission(Permission::External)
            .context(Session)
            .args::<SlotArgs>()
            .result::<RecordingStarted>(),
    )?;
    registry.register(
        CommandSpec::new(OVERDUB, "Overdub Slot")
            .description(
                "Records the notes an armed MIDI track plays into the MIDI clip that is playing \
                 in the slot (`track`, `scene`), a loop pass at a time: each pass adds its notes \
                 to the clip and is one undo step. A clip that does not loop overdubs until it \
                 ends. Launch the clip first; audio clips are never overdubbed. Ends like any \
                 recording: `session.stop_recording`, or `session.record` again, which pauses \
                 the overdub while the clip plays on. Recording captures input devices, so the \
                 person is asked first.",
            )
            .permission(Permission::External)
            .context(Session)
            .args::<SlotArgs>()
            .result::<RecordingStarted>(),
    )?;
    registry.register(
        CommandSpec::new(STOP_RECORDING, "Stop Session Recording")
            .description(
                "Ends the Session recording on the next grid line, without asking the person: \
                 the new clips it made play on as loops from there, and an overdubbed clip \
                 plays on as it was. A clip that is still being recorded is a clip of the \
                 length so far, even when nothing was played. Does nothing when no Session \
                 recording is under way.",
            )
            .permission(Permission::Read)
            .contexts([Global, Session])
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

    // Spec: session-recording R9. Starting a recording captures an input
    // device, so it asks; ending one never does.
    #[test]
    fn starting_a_recording_asks_and_ending_one_does_not() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for id in [RECORD, RECORD_SLOT, OVERDUB] {
            assert_eq!(
                registry.get(id).unwrap().permission,
                Some(Permission::External),
                "{id}"
            );
        }
        assert_eq!(
            registry.get(STOP_RECORDING).unwrap().permission,
            Some(Permission::Read)
        );
    }
}
