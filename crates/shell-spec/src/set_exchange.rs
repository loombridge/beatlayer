//! Merging sets and saving tracks or Session clips as a set (WP-6.35): the
//! commands that bring tracks and device chains from a set file into the open
//! set, and save selected tracks or Session clips as a new set. A set opened in
//! the browser is the browser's (`browser.open_folder` on its rows).
//!
//! They read and write files outside the project, so they belong to the shell
//! rather than to the document. [`register`] adds their specs, which are pure
//! data, so `cargo xtask` can check them and list them in the command
//! reference; `bl-shell` runs them by ID. The behaviour is in the set exchange
//! spec, R1 to R15.

use bl_commands::Context::{Arrangement, Browser, Session, TrackHeaders};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::{ClipId, DeviceId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `set.import`: add tracks, or one track's chain of devices, from a set file
/// to the open set.
pub const IMPORT: &str = "set.import";
/// `set.export_tracks`: save tracks as a new set.
pub const EXPORT_TRACKS: &str = "set.export_tracks";
/// `set.export_clips`: save Session clips as a new set.
pub const EXPORT_CLIPS: &str = "set.export_clips";

/// The undo step a merge of tracks makes (R9): Beatlayer's own name.
pub const IMPORT_TRACKS_STEP: &str = "Import Tracks";
/// The undo step a merge of a chain of devices makes (R9): Live's
/// "Insert Device".
pub const IMPORT_DEVICES_STEP: &str = "Insert Device";
/// The undo step a merge of a chain of devices onto a new track makes (R9):
/// Live's "Insert Device and Track".
pub const IMPORT_DEVICES_AND_TRACK_STEP: &str = "Insert Device and Track";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[IMPORT, EXPORT_TRACKS, EXPORT_CLIPS];

/// Whether `id` is one of the set exchange commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `set.import`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportArgs {
    /// The set file to take tracks from, or a track's chain of devices.
    pub path: String,
    /// The tracks to add, by their IDs in that set (the IDs `browser.state`
    /// gives in the rows of the opened set): one track, a group with every
    /// track inside it, or several, which come in that set's order. Left out
    /// (and `devices` too), every track but its return tracks and Main, as
    /// dropping the set file does. A return track or Main can't be added; add
    /// its chain of devices with `devices`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// Add only the chain of devices of this track of that set (its Devices
    /// row), with every device's state and without automation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<TrackId>,
    /// For tracks, the track of the open set they go after, at its level
    /// (inside its group when it is in one); for a chain, the track it goes
    /// on, after the track's own devices. Left out, the first selected track
    /// (the last one for tracks), or, when none is selected, after the last
    /// track or on a new track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<TrackId>,
    /// Ignore the selection: tracks go after the last track, and a chain goes
    /// on a new track of its kind (a MIDI track for a chain with an instrument
    /// or a MIDI effect, an audio track otherwise), as dropping in a view's
    /// drop area does. Ignored when `at` is given.
    #[serde(default)]
    pub drop_area: bool,
}

/// What `set.import` returns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Imported {
    /// The tracks added, in display order: the new track a chain went on, or
    /// the track it was added to.
    pub tracks: Vec<TrackId>,
    /// The devices added, in the order of the chain, for a chain.
    pub devices: Vec<DeviceId>,
    /// How many empty scenes were added at the end of the set so the Session
    /// clips keep their places.
    pub scenes_added: usize,
    /// How many audio files the added tracks play that couldn't be found.
    pub missing_files: usize,
}

/// Arguments of `set.export_tracks`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportTracksArgs {
    /// The tracks to save: one, a group with the tracks inside it, or
    /// several. Left out, the selected tracks. Main can't be saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// The folder to save the set in.
    pub folder: String,
    /// The set's name, without the extension. Left out, the track's name when
    /// one track is saved, and the open set's otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Answers Collect Files on Export when it is set to Ask: `true` copies
    /// the audio files the tracks play into the new set's project folder,
    /// `false` leaves the new set naming them where they are. Under Always and
    /// Never it is ignored; under Ask, leaving it out refuses the command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<bool>,
}

/// Arguments of `set.export_clips`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportClipsArgs {
    /// The Session clips to save. At least one.
    pub clips: Vec<ClipId>,
    /// The folder to save the set in.
    pub folder: String,
    /// The set's name, without the extension. Left out, the open set's name
    /// with " Clips" added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Answers Collect Files on Export when it is set to Ask, as
    /// `set.export_tracks`' does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<bool>,
}

/// What `set.export_tracks` and `set.export_clips` return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportedSet {
    /// The new set file's path.
    pub path: String,
    /// Its name, without the extension.
    pub name: String,
    /// How many audio files were copied into its project folder.
    pub copied: usize,
}

/// Adds the set exchange commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(IMPORT, "Import Tracks from Set")
            .description(
                "Adds tracks from a set file to the open set, as dragging the set or its rows \
                 from the browser does. With neither `tracks` nor `devices`, every track of the \
                 file but its return tracks and Main; with `tracks`, those tracks (a group \
                 brings the tracks inside it); with `devices`, only that track's chain of \
                 devices, with every device's state and without automation, onto a track that \
                 takes it or a new track. An added track keeps its kind, name, colour, mixer, \
                 sends' levels, devices, Arrangement clips, Session clips, take lanes and \
                 automation, with new IDs; its Session clips go in the open set's scenes at the \
                 positions they had, and the open set gets empty scenes at its end when it has \
                 fewer. Its sends go to the open set's return tracks in the same position and \
                 are left out past the last; an input, output or sidechain naming a track that \
                 isn't added goes back to its default or to none; links are dropped. The audio \
                 files are registered from the file's project folder and found by content when \
                 they moved; none is copied. The open set's tempo, time signature, scenes and \
                 loop are unchanged. One undo step. Reads a file outside the project. Read the \
                 new tracks back with the query tool's `song` and `track` objects.",
            )
            .permission(Permission::External)
            .contexts([Arrangement, Session, Browser, TrackHeaders])
            .args::<ImportArgs>()
            .result::<Imported>(),
    )?;
    registry.register(
        CommandSpec::new(EXPORT_TRACKS, "Save Tracks as Set")
            .description(
                "Saves tracks of the open set as a new set in `folder`, as dragging their title \
                 bars onto a folder in the browser does: whole, with their clips, devices, take \
                 lanes and automation, a group with the tracks inside it. The new set also \
                 holds the open set's return tracks with their devices, and its tempo, time \
                 signature and scenes. It is named `name`, the track's name when one track is \
                 saved, or the open set's, and is placed as Save Set As places a set: in the \
                 folder when it is a project folder, otherwise in a new \"<name> Project\" \
                 folder; written whole or not at all. The audio files are copied into its \
                 project folder's `Samples/Imported` or left where they are, as Collect Files on \
                 Export says (`settings.state`'s `library.collect_files_on_export`); under Ask, \
                 `copy` answers for the person and leaving it out refuses the command. The new \
                 row is selected with its name open for editing (`browser.state`'s `renaming`; \
                 `browser.rename_file` finishes it). Changes nothing in the open set. Writes \
                 files outside the project.",
            )
            .permission(Permission::External)
            .contexts([Arrangement, Session, Browser, TrackHeaders])
            .args::<ExportTracksArgs>()
            .result::<ExportedSet>(),
    )?;
    registry.register(
        CommandSpec::new(EXPORT_CLIPS, "Save Clips as Set")
            .description(
                "Saves several Session clips of the open set as a new set in `folder`, as \
                 dragging them onto a folder in the browser does: a track for each track they \
                 are on, in the open set's order, with its kind, name, colour, mixer and devices \
                 but none of its other clips or automation, and each clip in the scene as far \
                 below the first clip's scene as it was in the open set. The new set takes the \
                 open set's tempo and time signature. Named, placed, written and copied as \
                 `set.export_tracks` does, with \" Clips\" added to the open set's name. Changes \
                 nothing in the open set. Writes files outside the project.",
            )
            .permission(Permission::External)
            .contexts([Session, Browser])
            .args::<ExportClipsArgs>()
            .result::<ExportedSet>(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: set-exchange R15.
    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        for id in IDS {
            assert!(registry.contains(id), "{id}");
            assert_eq!(
                registry.get(id).unwrap().permission,
                Some(Permission::External),
                "{id} reads or writes files outside the project"
            );
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
    }
}
