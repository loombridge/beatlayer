//! Capture MIDI (WP-3.14): bringing back the notes just played on a
//! monitoring MIDI track as a clip.
//!
//! `bl-shell` keeps what the engine heard and runs the command.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{ClipId, SceneId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `midi.capture`: Capture MIDI, the Control Bar's button (L-CTRL-015) and
/// Create ▸ Capture MIDI (M-CREATE-009).
pub const CAPTURE: &str = "midi.capture";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[CAPTURE];

/// Whether `id` is one of the Capture MIDI commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// One clip a capture made or changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapturedClip {
    /// The track whose notes it holds.
    pub track: TrackId,
    /// The scene whose slot it is in.
    pub scene: SceneId,
    /// The clip.
    pub clip: ClipId,
    /// Whether the notes joined the clip that was playing, rather than
    /// making a new clip.
    pub merged: bool,
}

/// What `midi.capture` returns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Captured {
    /// The clips, one for each track that was played.
    pub clips: Vec<CapturedClip>,
    /// The song's new tempo, when the capture changed it (only with the
    /// transport stopped); `null` when it stayed.
    pub tempo: Option<f64>,
}

/// Adds the Capture MIDI command to a registry.
///
/// # Errors
///
/// [`RegisterError`] if its ID is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(CAPTURE, "Capture MIDI")
            .description(
                "Capture MIDI (Create ▸ Capture MIDI; the Control Bar's button). Brings back the \
                 notes played on the MIDI tracks that monitor their input, since the last \
                 capture or recording, as one clip on each track that was played, with their \
                 velocities and timing. A track with a clip playing while the transport runs \
                 gets the notes added to that clip: it grows to the loop pass the last note \
                 falls in and its loop moves there, keeping its length. Any other track gets a \
                 new clip in the selected scene's slot if it is empty, else the next empty slot \
                 below, else a scene added at the end; the clip's loop is the whole bars that \
                 cover its last phrase, from the first note-on to the last. With the transport \
                 stopped the first note is on 1.1.1 and the song's tempo may change to the \
                 played tempo; with it running the tempo stays. Control changes, pitch bend and \
                 pressure are not captured yet. Refused when no note was played; \
                 `transport.state` shows `capture_midi_ready`. One undo step.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<NoArgs>()
            .result::<Captured>(),
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
