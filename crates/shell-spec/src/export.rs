//! Exporting audio (WP-1.12): the `export.audio` command, its arguments,
//! and its result.
//!
//! [`register`] adds its spec, which is pure data, so `cargo xtask` can
//! check it and list it in the command reference; `bl-shell` runs it by ID,
//! and `bl-cli export` runs it on a headless shell. It writes a file outside
//! the project, so it's in the external class and an agent asks the person
//! first. The behaviour it follows is in the export spec.
//!
//! The File menu's Export Audio/Video... opens Live's export dialog, whose
//! controls aren't recorded in the UI parity reference yet, so the menu item
//! and its key stay pending (the export spec, R16) and the command has no
//! binding: a person reaches it once the dialog is built.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::editing::Position;

/// `export.audio`: render the set's Main output to an audio file.
pub const AUDIO: &str = "export.audio";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[AUDIO];

/// Whether `id` is one of the export commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// A file type the export writes (the export spec, R5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileType {
    /// WAV.
    Wav,
    /// AIFF (AIFF-C for 32-bit float).
    Aiff,
    /// FLAC: lossless, 16 or 24 bits.
    Flac,
}

impl FileType {
    /// The file type's name as Live's File Type chooser shows it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Wav => "WAV",
            Self::Aiff => "AIFF",
            Self::Flac => "FLAC",
        }
    }
}

/// The noise added before rounding below 32 bits (the export spec, R7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dither {
    /// No Dither.
    None,
    /// Rectangular: noise spread evenly over one step.
    Rectangular,
    /// Triangular: the default, and the safest when the file will be
    /// processed further.
    #[default]
    Triangular,
}

/// Arguments of `export.audio`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportArgs {
    /// The file to write, absolute or relative to the app's working
    /// directory, such as `/Music/Mix.wav`. Its folder must exist.
    pub path: String,
    /// The file type. Left out, the path's extension decides: `.wav`;
    /// `.aif` or `.aiff`; `.flac`. Given, it must agree with the extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_type: Option<FileType>,
    /// Bits per sample: 16 or 24 (integers), or 32 (floating point; WAV
    /// and AIFF only).
    #[schemars(extend("enum" = [16, 24, 32]))]
    pub bit_depth: u8,
    /// The file's sample rate in hertz, from 8,000 to 384,000, such as
    /// 44100 or 48000.
    #[schemars(range(min = 8000, max = 384_000))]
    pub sample_rate: u32,
    /// The dither below 32 bits. Left out, `triangular`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dither: Option<Dither>,
    /// Normalize: raise or lower the whole export so its highest peak is at
    /// full scale (0 dBFS).
    #[serde(default)]
    pub normalize: bool,
    /// Where the range starts. Left out, the start of the Arrangement's
    /// time selection, or 1.1.1 without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// How long the range is, in ticks (705,600 to the quarter note). Left
    /// out, to the end of the time selection, or without one to the end of
    /// the set's last clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<Ticks>,
    /// Replace a file already at `path`. Without it, an existing file is
    /// left alone and the export refused.
    #[serde(default)]
    pub replace: bool,
}

/// Where an export's range came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RangeSource {
    /// The `start` and `length` arguments.
    Arguments,
    /// The Arrangement's time selection.
    Selection,
    /// The whole set: 1.1.1 to the end of its last clip.
    Set,
}

/// What `export.audio` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExportResult {
    /// The file written.
    pub path: String,
    /// Its type.
    pub file_type: FileType,
    /// Bits per sample.
    pub bit_depth: u8,
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels in each frame.
    pub channels: u16,
    /// Frames in the file.
    pub frames: u64,
    /// The file's length in seconds.
    pub seconds: f64,
    /// Where the range started, in ticks from 1.1.1.
    pub start: Ticks,
    /// Where it ended, in ticks: the first position not exported.
    pub end: Ticks,
    /// Where the range came from.
    pub range: RangeSource,
    /// The dither used: `none` at 32 bits.
    pub dither: Dither,
    /// The largest sample magnitude written, in dBFS (before rounding), or
    /// null for silence.
    pub peak_dbfs: Option<f64>,
    /// The gain Normalize applied, in dB: 0 without it.
    pub gain_db: f64,
    /// The rate the engine rendered at, which is higher than the file's
    /// when the file's is below the engine's.
    pub render_rate: u32,
    /// How long the export took, in seconds.
    pub took_seconds: f64,
    /// How many times faster than real time it ran.
    pub speed: f64,
    /// Media that clips name and that couldn't be found, by content hash:
    /// those clips are silent in the export.
    pub missing_media: Vec<String>,
}

/// Adds the export commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(AUDIO, "Export Audio")
            .description(
                "Renders the set's Main output (what you hear) to an audio file, offline and \
                 faster than real time, as the File menu's Export Audio/Video does. `path` is the \
                 file to write; `bit_depth` (16, 24, or 32) and `sample_rate` are required. The \
                 file type follows the extension (`.wav`, `.aif`/`.aiff`, `.flac`; FLAC holds 16 \
                 or 24 bits only). The range is `start` and `length` if given, otherwise the \
                 Arrangement's time selection, otherwise the whole set from 1.1.1 to the end of \
                 the last clip; it plays straight through, without the Arrangement loop. Below 32 \
                 bits, `dither` adds `triangular` noise unless told `rectangular` or `none`. \
                 `normalize` brings the highest peak to 0 dBFS. A rate below the engine's is \
                 rendered at the engine's and converted down. The file is written under a \
                 temporary name and appears only when it's complete. An existing file is \
                 refused unless `replace` is true. Changes nothing in the set.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<ExportArgs>()
            .result::<ExportResult>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        // Spec: export R15.
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
        let spec = registry.get(AUDIO).unwrap();
        assert_eq!(spec.permission, Some(Permission::External));
        assert!(spec.bindings.is_empty(), "the dialog is pending (R16)");
        let schema = spec.args.as_ref().unwrap();
        let ok = json!({ "path": "/a/b.wav", "bit_depth": 24, "sample_rate": 48000 });
        assert!(bl_commands::validate(schema, &ok).is_ok());
        for bad in [
            json!({ "path": "/a/b.wav", "bit_depth": 8, "sample_rate": 48000 }),
            json!({ "path": "/a/b.wav", "bit_depth": 24 }),
            json!({ "path": "/a/b.wav", "bit_depth": 24, "sample_rate": 100 }),
            json!({ "path": "/a/b.wav", "bit_depth": 24, "sample_rate": 48000, "dither": "pow_r" }),
        ] {
            assert!(bl_commands::validate(schema, &bad).is_err(), "{bad}");
        }
    }
}
