//! The autosampler's commands (WP-5.16, IMP-27; the multisampler spec, R64
//! to R73): sampling an instrument into a new multisampler, and stopping a
//! running sample.
//!
//! The command is in the `external` class, because it writes files and may
//! play hardware.

use bl_commands::Context::DeviceView;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{DeviceId, LayerId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `device.autosample`: sample an instrument into a new multisampler (R64).
pub const AUTOSAMPLE: &str = "device.autosample";
/// `device.autosample_open`: open the autosampler's dialog (R64).
pub const AUTOSAMPLE_OPEN: &str = "device.autosample_open";
/// `device.autosample_cancel`: stop a running autosample (R73).
pub const AUTOSAMPLE_CANCEL: &str = "device.autosample_cancel";

/// The undo step's name (R70).
pub const STEP: &str = "Autosample";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[AUTOSAMPLE, AUTOSAMPLE_OPEN, AUTOSAMPLE_CANCEL];

/// Whether `id` is one of these commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `device.autosample`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutosampleArgs {
    /// The instrument to sample: a native instrument, a plug-in instrument,
    /// or the external instrument. The one selected in the Device View when
    /// it is left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The lowest note sampled, from 0 to 127 (C-2 to G8). C1 by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lowest: Option<u8>,
    /// The highest note sampled, from 0 to 127 (C-2 to G8). C6 by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highest: Option<u8>,
    /// How many semitones between the sampled notes, from 1 to 12. 3 by
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u8>,
    /// How many velocity layers each note is sampled at, from 1 to 16. 1 by
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity_layers: Option<u8>,
    /// How long each note is held, in seconds, from 0.1 to 30. 2 by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_length: Option<f64>,
    /// How long the render goes on after the note is released, in seconds,
    /// from 0 to 30. 1 by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_tail: Option<f64>,
    /// How many times each note is sampled, from 1 to 8, for the round
    /// robins. 1 by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round_roins: Option<u8>,
    /// Whether a loop is found in each sample. Off by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detect_loops: Option<bool>,
    /// The name of the instrument the samples are named for, and of the
    /// new track. The instrument's own name by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether to wait for the samples to be made before returning. True by
    /// default; with it off the command starts the samples and returns at
    /// once, and the progress is read from the dialog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
}

/// Arguments of `device.autosample_open`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutosampleOpenArgs {
    /// The instrument to sample. The one selected in the Device View when
    /// it is left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
}

/// What `device.autosample_open` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AutosamplerOpened {
    /// The instrument the dialog is open for.
    pub device: DeviceId,
}

/// One sample the autosampler made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AutosampleFile {
    /// Where the file is.
    pub path: String,
    /// The note it holds.
    pub note: u8,
    /// The velocity it was played at.
    pub velocity: u8,
    /// Which round robin of them it is, from 1.
    pub round_robin: u8,
    /// The layer it became.
    pub layer: LayerId,
    /// Where its loop begins, in frames, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_start: Option<u64>,
    /// Where its loop ends, in frames, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_end: Option<u64>,
}

/// What `device.autosample` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Autosampled {
    /// The MIDI track the multisampler is on, once the samples are made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The multisampler, once the samples are made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// The samples made, in note, velocity and round robin order.
    pub files: Vec<AutosampleFile>,
    /// The samples left without a loop, named.
    pub without_loops: Vec<String>,
    /// How long it took, in seconds.
    pub took: f64,
}

/// What `device.autosample_cancel` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AutosampleCancelled {
    /// Whether a sample was running and has been stopped.
    pub cancelled: bool,
}

/// Registers the autosampler's commands.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(AUTOSAMPLE, "Autosample")
            .description(
                "Samples an instrument across a range of notes and velocities into a new \
                 multisampler on a new MIDI track: the native instrument, the plug-in \
                 instrument, or the external instrument, rendered alone with its current \
                 settings. The notes are the lowest one and every `step` above it up to the \
                 highest, which is always sampled; each is played at `velocity_layers` \
                 velocities, held for `note_length` seconds and followed by `release_tail` \
                 seconds of render, and sampled `round_roins` times for the round robins. \
                 The samples are 32-bit floating-point WAVs in the project's \
                 `Samples/Processed/Autosample` folder, named for the instrument, the note and \
                 the velocity, and they join the set's media. With `detect_loops` on, each \
                 sample gets a sustain loop so a sustained sound holds past the note. The \
                 result is a new MIDI track straight after the source's, holding a multisampler \
                 with one layer per sample, its zones spread between the sampled notes, Round \
                 Robin on in Forward mode where there are round robins, and the volume \
                 envelope's release as long as the release tail. One undo step; undoing it \
                 leaves the files on disk. It writes files, and plays hardware for the external \
                 instrument.",
            )
            .permission(Permission::External)
            .context(DeviceView)
            .args::<AutosampleArgs>()
            .result::<Autosampled>(),
    )?;
    registry.register(
        CommandSpec::new(AUTOSAMPLE_OPEN, "Autosample…")
            .description(
                "Opens the autosampler's dialog for an instrument: the notes and velocities to \
                 sample, the note length and release tail, the round robins, whether a loop is \
                 found in each sample, and the name the samples are given. It changes nothing \
                 in the set; the dialog's Autosample button runs `device.autosample`.",
            )
            .permission(Permission::Read)
            .context(DeviceView)
            .args::<AutosampleOpenArgs>()
            .result::<AutosamplerOpened>(),
    )?;
    registry.register(
        CommandSpec::new(AUTOSAMPLE_CANCEL, "Stop Autosampling")
            .description(
                "Stops a running autosample: the files it wrote are deleted and the set is \
                 left as it was. It does nothing when no autosample is running.",
            )
            .permission(Permission::Edit)
            .context(DeviceView)
            .args::<NoArgs>()
            .result::<AutosampleCancelled>(),
    )?;
    Ok(())
}
