//! Standard MIDI File import and export (WP-2.21): the `midi.import_file`
//! and `midi.export_clip` commands, their arguments, and their results.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them by
//! ID. Both read or write a file outside the project, so they're in the
//! external class and an agent asks the person first. The behaviour they
//! follow is in the MIDI clips spec (WP-2.21).
//!
//! The File menu's Export MIDI Clip... and its key (S-FILE-008) run
//! `midi.export_clip` on the selected clip, which asks where to save it.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{ClipId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `midi.import_file`: read a Standard MIDI File into the Arrangement.
pub const IMPORT: &str = "midi.import_file";
/// `midi.export_clip`: write a MIDI clip out as a Standard MIDI File.
pub const EXPORT: &str = "midi.export_clip";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[IMPORT, EXPORT];

/// Whether `id` is one of the MIDI file commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Arguments of `midi.import_file`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MidiImportArgs {
    /// The Standard MIDI File to read, absolute or relative to the app's
    /// working directory. Its type 0 or type 1 tracks become clips.
    pub path: String,
    /// The MIDI track to put the file's clips on. Left out, each track of the
    /// file with notes gets a new MIDI track below the last one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// Where the first clip starts, in ticks (705,600 to the quarter note).
    /// Left out, the Arrangement's insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Ticks>,
    /// Whether to bring the file's tempo and time signature into the song,
    /// as Live's "Import tempo and time signature data into Arrangement?"
    /// asks. Off unless given, so an agent never changes the song's tempo by
    /// accident.
    #[serde(default, skip_serializing_if = "is_false")]
    pub import_tempo: bool,
}

/// A clip `midi.import_file` placed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportedClip {
    /// The clip.
    pub clip: ClipId,
    /// The track it's on.
    pub track: TrackId,
    /// Where it starts, in ticks.
    pub start: Ticks,
    /// How long it is, in ticks.
    pub length: Ticks,
    /// Its name.
    pub name: String,
}

/// What `midi.import_file` returns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MidiImportResult {
    /// The MIDI tracks it made, in the order they were made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_tracks: Vec<TrackId>,
    /// The clips it placed, one per track of the file that has notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<ImportedClip>,
}

/// Arguments of `midi.export_clip`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MidiExportArgs {
    /// The MIDI clip to write.
    pub clip: ClipId,
    /// The file to write, absolute or relative to the app's working
    /// directory. Its folder must exist. `.mid` or `.midi` by convention.
    /// Left out, the app asks where to save the file, as the clip menus'
    /// Export MIDI Clip... does, and then runs this again with the path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Replace a file already at `path`. Without it, an existing file is left
    /// alone and the export refused.
    #[serde(default)]
    pub replace: bool,
}

/// What `midi.export_clip` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MidiExportResult {
    /// The file written.
    pub path: String,
    /// How many of the clip's notes it holds.
    pub notes: usize,
    /// Where the clip starts in the song, in ticks.
    pub start: Ticks,
    /// How long the clip is, in ticks.
    pub length: Ticks,
    /// The tempo written into the file, in quarter notes per minute: the
    /// song's tempo where the clip starts.
    pub tempo: f64,
    /// The time signature written into the file, as `numerator/denominator`.
    pub signature: String,
}

/// Adds the MIDI file commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(IMPORT, "Import MIDI File")
            .description(
                "Reads a Standard MIDI File (type 0 or type 1) into the Arrangement, as dragging \
                 it in or the Create menu's Import MIDI File does: each of the file's tracks that \
                 has notes becomes a MIDI clip. With no `track`, each becomes a new MIDI track \
                 below the last one, named as a new track is; with a `track`, the file's one \
                 track with notes goes on that MIDI track. Clips start at `at`, or the insert \
                 marker. Each clip keeps the file's leading silence, is named after its track's \
                 name (or `Track N`), carries the file's time signature, loops its whole length, \
                 and lasts from its first note to its last note-off rounded up to a whole bar. \
                 With `import_tempo`, the file's tempo and time signature come into the song as \
                 Live's \"Import tempo and time signature data into Arrangement?\" answer Yes \
                 does: a single-tempo file sets the song's tempo, a file whose tempo changes \
                 writes Song Tempo automation over the clip and restores the tempo at its end, \
                 and a time-signature marker is written at the clip's start and one restoring \
                 the previous signature at its end. Off (the default), the song's tempo, \
                 automation and markers are left alone. A clip placed over part of another \
                 replaces that part, so a track never plays two clips at once; when a clip on \
                 `track` ends after `at`, it's destructive as well as external. One undo step. \
                 Reads a file outside the project.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<MidiImportArgs>()
            .result::<MidiImportResult>(),
    )?;
    registry.register(
        CommandSpec::new(EXPORT, "Export MIDI Clip")
            .description(
                "Writes a MIDI clip's notes as a type 1 Standard MIDI File at `path`, as the \
                 File menu's Export MIDI Clip does: its notes' pitches, starts, lengths and \
                 velocities, its tempo (the song's tempo where the clip starts) and its time \
                 signature. Fails for an audio clip, or for a file already at `path` unless \
                 `replace` is true. With no `path` the app asks where to save the file (as the \
                 clip menus' Export MIDI Clip... does), and runs this again with it; a key or \
                 the File menu's item gives no `clip` and exports the selected clip. Writes a \
                 file outside the project; changes nothing in the set.",
            )
            .permission(Permission::External)
            .context(Global)
            .binding("S-FILE-008")
            .args::<MidiExportArgs>()
            .result::<MidiExportResult>(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");

        let import = registry.get(IMPORT).unwrap();
        assert_eq!(import.permission, Some(Permission::External));
        assert!(import.bindings.is_empty(), "the dialog is pending");
        let schema = import.args.as_ref().unwrap();
        assert!(bl_commands::validate(schema, &json!({ "path": "/a/b.mid" })).is_ok());
        assert!(
            bl_commands::validate(
                schema,
                &json!({ "path": "/a/b.mid", "track": 7, "at": 0, "import_tempo": true })
            )
            .is_ok()
        );
        assert!(bl_commands::validate(schema, &json!({})).is_err());

        let export = registry.get(EXPORT).unwrap();
        assert_eq!(export.permission, Some(Permission::External));
        let schema = export.args.as_ref().unwrap();
        assert!(bl_commands::validate(schema, &json!({ "clip": 3, "path": "/a/b.mid" })).is_ok());
        // Without a path the app asks where to save the file.
        assert!(bl_commands::validate(schema, &json!({ "clip": 3 })).is_ok());
        assert!(bl_commands::validate(schema, &json!({ "path": "/a/b.mid" })).is_err());
    }
}
