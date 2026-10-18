//! Stem separation (IMP-11): Separate Stems to New Audio Tracks,
//! and the packages it needs, downloaded on first use.
//!
//! [`register`] adds the commands' specs; `bl-shell` runs them. The
//! behaviour they follow is in the stem separation spec.
//!
//! A person reaches them from an audio clip's context menu in the
//! Arrangement (C-CLIP-ARR-027) and the Create menu (M-CREATE-015), which
//! run [`OPEN`]: the first time, its dialog lists the packages to download;
//! after that, it offers the stems. An agent calls [`SEPARATE`] directly,
//! after [`INSTALL`] if [`STATUS`] says the packages aren't installed.

use bl_commands::Context::{Arrangement, Global};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::{ClipId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `stems.open`: open Separate Stems to New Audio Tracks' dialog.
pub const OPEN: &str = "stems.open";
/// `stems.close`: close the dialog.
pub const CLOSE: &str = "stems.close";
/// `stems.status`: the packages, the download, and the separation.
pub const STATUS: &str = "stems.status";
/// `stems.install`: download and install the packages.
pub const INSTALL: &str = "stems.install";
/// `stems.separate`: separate an audio clip into stems on new tracks.
pub const SEPARATE: &str = "stems.separate";
/// `stems.cancel`: stop the download or the separation under way.
pub const CANCEL: &str = "stems.cancel";

/// The undo step's name.
pub const STEP: &str = "Separate Stems";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[OPEN, CLOSE, STATUS, INSTALL, SEPARATE, CANCEL];

/// Whether `id` is one of the stem separation commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// A part of the mix to separate (the dialog's four switches).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum StemKind {
    /// Voices.
    Vocals,
    /// Drums and percussion.
    Drums,
    /// Bass.
    Bass,
    /// Everything else.
    Others,
}

impl StemKind {
    /// The four, in the dialog's order.
    pub const ALL: [Self; 4] = [Self::Vocals, Self::Drums, Self::Bass, Self::Others];

    /// Its name, as the dialog and the new track show it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Vocals => "Vocals",
            Self::Drums => "Drums",
            Self::Bass => "Bass",
            Self::Others => "Others",
        }
    }
}

/// How hard the separation works (the dialog's High Speed / High Quality
/// switch).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StemQuality {
    /// One pass: the quicker.
    #[default]
    HighSpeed,
    /// Two passes, averaged: about twice as long, with fewer artefacts.
    HighQuality,
}

/// Arguments of `stems.open`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenArgs {
    /// The audio clip to separate; the selected clip if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
}

/// Arguments of `stems.separate`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SeparateArgs {
    /// The audio clip in the Arrangement to separate.
    pub clip: ClipId,
    /// Which parts to put on new tracks, one or more; all four if left out.
    #[serde(default = "every_stem")]
    pub stems: Vec<StemKind>,
    /// `high_speed` (the default) or `high_quality`.
    #[serde(default)]
    pub quality: StemQuality,
    /// Put the chosen parts together on one track: only with two or three
    /// parts.
    #[serde(default)]
    pub merge: bool,
}

fn every_stem() -> Vec<StemKind> {
    StemKind::ALL.to_vec()
}

/// A package the separation needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageStatus {
    /// Its name.
    pub name: String,
    /// What it does.
    pub purpose: String,
    /// Its version.
    pub version: String,
    /// Its project's page.
    pub url: String,
    /// The download's size in bytes.
    pub size: u64,
    /// Whether it's installed.
    pub installed: bool,
}

/// Where a download or a separation is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Nothing is under way.
    #[default]
    Idle,
    /// Under way: see `progress`.
    Running,
    /// It finished.
    Done,
    /// It failed: see `error`.
    Failed,
    /// It was cancelled.
    Cancelled,
}

/// A download or a separation's progress.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JobStatus {
    /// Where it is.
    pub state: JobState,
    /// How far it is, from 0 to 1.
    pub progress: f32,
    /// What it's doing now, such as `Downloading ONNX Runtime`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stage: String,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The dialog a person sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DialogStep {
    /// The packages to download, the first time.
    Packages,
    /// The download's progress.
    Installing,
    /// The parts and the quality.
    Options,
    /// The separation's progress.
    Separating,
}

/// The open dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DialogStatus {
    /// What it shows.
    pub step: DialogStep,
    /// The clip it separates.
    pub clip: ClipId,
}

/// What `stems.status` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StemsStatus {
    /// Whether every package is installed, so `stems.separate` can run.
    pub installed: bool,
    /// Where the packages are installed.
    pub folder: String,
    /// The packages.
    pub packages: Vec<PackageStatus>,
    /// The download.
    pub install: JobStatus,
    /// The separation.
    pub separation: JobStatus,
    /// The tracks the last separation made, the group first if it made
    /// one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_tracks: Vec<TrackId>,
    /// The dialog, if it's open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<DialogStatus>,
}

/// Adds the stem separation commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(OPEN, "Separate Stems to New Audio Tracks")
            .description(
                "Opens the Separate Stems dialog for an audio clip in the Arrangement (`clip`, \
                 or the selected clip). The first time, the dialog lists the packages stem \
                 separation needs (ONNX Runtime and the Demucs model), their sizes and where \
                 they install, with Download Required Packages Now and Cancel; once they're \
                 installed it offers Vocals, Drums, Bass and Others, High Speed or High \
                 Quality, and Separate. Changes nothing in the set: an agent calls \
                 `stems.separate` instead.",
            )
            .permission(Permission::Read)
            .context(Arrangement)
            .args::<OpenArgs>()
            .result::<StemsStatus>(),
    )?;
    registry.register(
        CommandSpec::new(CLOSE, "Close Separate Stems")
            .description(
                "Closes the Separate Stems dialog. A download or separation under way goes on; \
                 `stems.cancel` stops it. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<StemsStatus>(),
    )?;
    registry.register(
        CommandSpec::new(STATUS, "Stem Separation Status")
            .description(
                "Reads stem separation's state: whether its packages are installed (and each \
                 package's name, version, project page and download size), where they install, \
                 the download's progress, the separation's progress, the tracks the last \
                 separation made, and the dialog if it's open.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<StemsStatus>(),
    )?;
    registry.register(
        CommandSpec::new(INSTALL, "Download Required Packages Now")
            .description(
                "Downloads and installs the open-source packages stem separation needs, ONNX \
                 Runtime and the Demucs model (about 350 MB together), into Beatlayer's \
                 application-data folder, checking each file's SHA-256. Returns at once; \
                 `stems.status` reports the progress. Installed packages aren't downloaded \
                 again. Changes nothing in the set.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<NoArgs>()
            .result::<StemsStatus>(),
    )?;
    registry.register(
        CommandSpec::new(SEPARATE, STEP)
            .description(
                "Separates an audio clip in the Arrangement into stems on new audio tracks, \
                 with a Demucs network running on this computer. `stems` chooses the parts \
                 (`vocals`, `drums`, `bass`, `others`; all four if left out); `quality` is \
                 `high_speed` (one pass) or `high_quality` (two passes averaged, about twice \
                 as long); `merge` puts two or three parts together on one track. The audio \
                 between the clip's Start and End markers, or through its loop, is separated \
                 at 44.1 kHz and written to the project's Samples/Processed/Stems folder. \
                 Each part gets a track below the clip's track, inside a new group track when \
                 there's more than one, with a copy of the clip playing that part; the \
                 original clip is deactivated. Playback stops first. Returns at once; \
                 `stems.status` reports the progress and the new tracks, and the whole edit is \
                 one undo step. Needs the packages `stems.install` downloads.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SeparateArgs>()
            .result::<StemsStatus>(),
    )?;
    registry.register(
        CommandSpec::new(CANCEL, "Cancel")
            .description(
                "Stops the package download or the separation under way. A cancelled \
                 separation changes nothing in the set; a cancelled download keeps the \
                 packages it finished.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<NoArgs>()
            .result::<StemsStatus>(),
    )
}
