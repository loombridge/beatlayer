//! Convert Harmony, Melody and Drums to New MIDI Track (IMP-06): the
//! commands, and what a conversion reports.
//!
//! [`register`] adds the commands' specs; `bl-shell` runs them. The
//! behaviour they follow is in the audio to MIDI spec.
//!
//! A person reaches [`CONVERT`] from an audio clip's context menu in either
//! view (C-CLIP-ARR-027a to 027c, C-CLIP-SES-019b to 019d) and from the
//! Create menu (M-CREATE-012 to 014), each item naming its [`ConvertKind`]. An
//! agent calls it directly. Harmony and Melody need the packages the
//! Packages manager downloads on first use; Drums needs none.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{ClipId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::packages::PackageStatus;
use crate::stems::JobStatus;

/// `audio_to_midi.convert`: convert an audio clip to a new MIDI track.
pub const CONVERT: &str = "audio_to_midi.convert";
/// `audio_to_midi.status`: the packages, the download and the conversion.
pub const STATUS: &str = "audio_to_midi.status";
/// `audio_to_midi.cancel`: stop the conversion or the download under way,
/// and close the progress window.
pub const CANCEL: &str = "audio_to_midi.cancel";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[CONVERT, STATUS, CANCEL];

/// Whether `id` is one of the audio to MIDI commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// What a conversion finds in the audio.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConvertKind {
    /// Pitches of polyphonic audio, chords included.
    Harmony,
    /// Pitches of monophonic audio, one at a time.
    Melody,
    /// Hits of unpitched, percussive audio: kick, snare and hi-hat.
    Drums,
}

impl ConvertKind {
    /// The three, in the menus' order.
    pub const ALL: [Self; 3] = [Self::Harmony, Self::Melody, Self::Drums];

    /// The word the new track's name ends with.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Harmony => "Harmony",
            Self::Melody => "Melody",
            Self::Drums => "Drums",
        }
    }

    /// The command's name, which the undo step and the progress window take.
    #[must_use]
    pub const fn command(self) -> &'static str {
        match self {
            Self::Harmony => "Convert Harmony to New MIDI Track",
            Self::Melody => "Convert Melody to New MIDI Track",
            Self::Drums => "Convert Drums to New MIDI Track",
        }
    }

    /// Whether the conversion runs an on-device model, so needs the
    /// packages.
    #[must_use]
    pub const fn needs_packages(self) -> bool {
        !matches!(self, Self::Drums)
    }
}

/// Arguments of `audio_to_midi.convert`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConvertArgs {
    /// What to find: `harmony`, `melody` or `drums`.
    pub kind: ConvertKind,
    /// The audio clip, in the Arrangement or a Session slot. With none, the
    /// one audio clip selected in either view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
}

/// The window a person sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConvertDialogStep {
    /// The packages to download, the first time.
    Packages,
    /// The download's progress.
    Installing,
    /// The conversion's progress.
    Converting,
}

/// The open window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConvertDialogStatus {
    /// What it shows.
    pub step: ConvertDialogStep,
    /// The conversion it is for.
    pub kind: ConvertKind,
    /// The clip it converts.
    pub clip: ClipId,
}

/// What `audio_to_midi.status` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AudioToMidiStatus {
    /// Whether every package Harmony and Melody need is installed. Drums
    /// needs none.
    pub installed: bool,
    /// Where the packages are installed.
    pub folder: String,
    /// The packages Harmony and Melody need.
    pub packages: Vec<PackageStatus>,
    /// The download.
    pub install: JobStatus,
    /// The conversion, the last one when none is running.
    pub conversion: JobStatus,
    /// The kind of the conversion running, or the last one's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ConvertKind>,
    /// The clip being converted, or the last one's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The track the last conversion made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_track: Option<TrackId>,
    /// The clip the last conversion made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_clip: Option<ClipId>,
    /// How many notes the last conversion put in its clip; a conversion
    /// that found none made an empty clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<usize>,
    /// The window, if it's open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<ConvertDialogStatus>,
}

/// Adds the audio to MIDI commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(CONVERT, "Convert to New MIDI Track")
            .description(
                "Converts an audio clip to notes on a new MIDI track: `kind` is `harmony` \
                 (pitches of polyphonic audio, played by an Instrument Rack named Harmony), \
                 `melody` (pitches of monophonic audio, played by an Instrument Rack named \
                 Melody whose first macro, Synth to Piano, moves its sound from a synth to an \
                 electric piano) or `drums` (kick, snare and hi-hat, on notes 36, 38 and 42 of \
                 a Drum Rack named Drums). `clip` is the audio clip, in the Arrangement or a \
                 Session slot; with none, the one audio clip selected. The audio is the \
                 clip's file from its Start marker to its End marker, or through its loop. The \
                 new track goes directly after the clip's track, named after the clip and the \
                 kind, and is selected; its clip starts where the source clip starts (in the \
                 same scene's slot for a Session clip) and is as long, with the source's \
                 loop, markers, name and colour, its notes timed through the source's warp \
                 markers (an unwarped clip at the set's tempo). Transient markers the person \
                 placed or moved are hints: an onset within 30 ms of one moves to it, and a \
                 held note crossing one is split. Harmony and Melody need the packages the \
                 Packages manager downloads on first use; the first time, this opens the \
                 package page instead and runs once they're installed. Returns at once; \
                 `audio_to_midi.status` reports the progress and, once it lands, the new \
                 track and clip, and `song` `tracks` and `clip` read them and the notes \
                 back. The track, its instrument, its clip and its notes are one undo step \
                 named after the command; the source clip is left as it is. Refused for a \
                 MIDI clip, a missing file, or while another conversion or a stem separation \
                 runs.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<ConvertArgs>()
            .result::<AudioToMidiStatus>(),
    )?;
    registry.register(
        CommandSpec::new(STATUS, "Audio to MIDI Status")
            .description(
                "Reads the state of Convert Harmony, Melody and Drums to New MIDI Track: the \
                 conversion under way (its kind, clip and progress) or the last one's outcome \
                 and the track and clip it made, whether its packages are installed (and each \
                 package's name, version, project page and download size), the download's \
                 progress, and the window if it's open.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<AudioToMidiStatus>(),
    )?;
    registry.register(
        CommandSpec::new(CANCEL, "Cancel")
            .description(
                "Stops the conversion or the package download under way and closes the \
                 window. A cancelled conversion changes nothing in the set; a cancelled \
                 download keeps the packages it finished. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<AudioToMidiStatus>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: audio-to-midi R27.
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
        assert_eq!(
            registry.get(CONVERT).unwrap().permission,
            Some(Permission::Edit)
        );
        for id in [STATUS, CANCEL] {
            assert_eq!(registry.get(id).unwrap().permission, Some(Permission::Read));
        }
    }

    // Spec: audio-to-midi R9. Only Harmony and Melody run a model.
    #[test]
    fn only_harmony_and_melody_need_packages() {
        let needing: Vec<ConvertKind> = ConvertKind::ALL
            .into_iter()
            .filter(|kind| kind.needs_packages())
            .collect();
        assert_eq!(needing, [ConvertKind::Harmony, ConvertKind::Melody]);
    }
}
