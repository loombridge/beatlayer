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
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::TrackId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::editing::Position;

/// `export.audio`: render the set's Main output to an audio file.
pub const AUDIO: &str = "export.audio";
/// `export.realtime.status`: what the real-time render is doing now.
pub const REALTIME_STATUS: &str = "export.realtime.status";
/// `export.realtime.restart`: start the waiting period again.
pub const REALTIME_RESTART: &str = "export.realtime.restart";
/// `export.realtime.skip`: start the render at once.
pub const REALTIME_SKIP: &str = "export.realtime.skip";
/// `export.realtime.cancel`: stop the render and delete its partial file.
pub const REALTIME_CANCEL: &str = "export.realtime.cancel";
/// `export.realtime.auto_restart`: turn Auto-Restart on Drop-Outs on or off.
pub const REALTIME_AUTO_RESTART: &str = "export.realtime.auto_restart";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    AUDIO,
    REALTIME_STATUS,
    REALTIME_RESTART,
    REALTIME_SKIP,
    REALTIME_CANCEL,
    REALTIME_AUTO_RESTART,
];

/// Whether `id` is one of the export commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// A file type the export writes (the export spec, R5; MP3, WP-2.26).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileType {
    /// WAV.
    Wav,
    /// AIFF (AIFF-C for 32-bit float).
    Aiff,
    /// FLAC: lossless, 16 or 24 bits.
    Flac,
    /// MP3: lossy, 320 kbps CBR, at one of MP3's sample rates.
    Mp3,
}

impl FileType {
    /// The file type's name as Live's File Type chooser shows it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Wav => "WAV",
            Self::Aiff => "AIFF",
            Self::Flac => "FLAC",
            Self::Mp3 => "MP3",
        }
    }
}

/// Which track or tracks to render (the export spec, S2; WP-2.26).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderedTrack {
    /// The Main output (post-fader sum of every track, what you hear).
    #[default]
    Main,
    /// All individual tracks in the set: audio and MIDI tracks that make audio,
    /// group tracks, return tracks, and the Main track, one file each.
    AllTracks,
    /// The tracks currently selected in the Arrangement: writes one file per
    /// selected track that makes audio.
    SelectedTracks,
    /// A single track by its ID.
    Track(TrackId),
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
    /// Noise Shaped: Beatlayer's own noise-shaped dither, for a final
    /// 16- or 24-bit render.
    NoiseShaped,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Arguments of `export.audio`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportArgs {
    /// The file to write, absolute or relative to the app's working
    /// directory, such as `/Music/Mix.wav`. Its folder must exist.
    pub path: String,
    /// The file type. Left out, the path's extension decides: `.wav`;
    /// `.aif` or `.aiff`; `.flac`; `.mp3`. Given, it must agree with the
    /// extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_type: Option<FileType>,
    /// Bits per sample: 16 or 24 (integers), or 32 (floating point; WAV
    /// and AIFF only). MP3 has no bit depth, so it ignores this.
    #[schemars(extend("enum" = [16, 24, 32]))]
    pub bit_depth: u8,
    /// The file's sample rate in hertz, from 8,000 to 384,000, such as
    /// 44100 or 48000. MP3 holds 8,000, 11,025, 12,000, 16,000, 22,050,
    /// 24,000, 32,000, 44,100, or 48,000 Hz.
    #[schemars(range(min = 8000, max = 384_000))]
    pub sample_rate: u32,
    /// The dither below 32 bits. Left out, `triangular`. MP3 ignores it.
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
    /// Which track(s) to render: `main` (the default), `all_tracks`,
    /// `selected_tracks`, or a single track `{ "track": <id> }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_track: Option<RenderedTrack>,
    /// Include return and Main effects when rendering individual tracks.
    /// Unavailable (refused) when `rendered_track` is `main`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_return_and_main_effects: bool,
    /// Render as Loop: the song goes on past the range's end as if Stop were
    /// pressed there, until the output has stayed below -120 dBFS for 100 ms
    /// or 60 s have passed, and that tail is added onto the file's start,
    /// wrapping round if it's longer than the range. The file is still
    /// exactly the range long. Off unless given.
    #[serde(default, skip_serializing_if = "is_false")]
    pub render_as_loop: bool,
    /// How long the render waits before it starts, in seconds, so the
    /// hardware's last sound dies away: 10 unless given, and longer if the
    /// seconds are given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_seconds: Option<u32>,
    /// Whether the render starts again when a dropout spoils it: on unless
    /// told otherwise. With it off, the render carries on and the dropouts
    /// are listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_restart: Option<bool>,
    /// The most times the render may start again after a dropout: no limit
    /// when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
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
    /// The file written (or the base path if multiple files were written).
    pub path: String,
    /// Every file written by the export: one for Main or a single track,
    /// or one per track for all-tracks or selected-tracks exports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Its type.
    pub file_type: FileType,
    /// Bits per sample.
    pub bit_depth: u8,
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels in each frame (describes the first file in a multi-file export).
    pub channels: u16,
    /// Frames in the file (describes the first file in a multi-file export).
    pub frames: u64,
    /// The file's length in seconds (describes the first file in a multi-file export).
    pub seconds: f64,
    /// Where the range started, in ticks from 1.1.1.
    pub start: Ticks,
    /// Where it ended, in ticks: the first position not exported.
    pub end: Ticks,
    /// Where the range came from.
    pub range: RangeSource,
    /// The dither used: `none` at 32 bits.
    pub dither: Dither,
    /// The largest sample magnitude written across all rendered files, in dBFS
    /// (before rounding), or null for silence.
    pub peak_dbfs: Option<f64>,
    /// The gain Normalize applied, in dB: 0 without it (describes the first file
    /// in a multi-file export).
    pub gain_db: f64,
    /// The rate the engine rendered at (describes the first file in a multi-file
    /// export), which is higher than the file's when the file's is below the engine's.
    pub render_rate: u32,
    /// How long the export took, in seconds.
    pub took_seconds: f64,
    /// How many times faster than real time it ran.
    pub speed: f64,
    /// Media that clips name and that couldn't be found, by content hash:
    /// those clips are silent in the export.
    pub missing_media: Vec<String>,
    /// The devices that work with hardware (an external instrument or an
    /// external effect), by the name their title bar shows and their ID:
    /// they send nothing to hardware in an offline render and bring back
    /// silence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub silenced_external_devices: Vec<String>,
    /// Whether the export rendered in real time, on the running engine,
    /// because its path runs through external hardware.
    #[serde(default)]
    pub real_time: bool,
    /// How many times the render started: more than one when a dropout
    /// spoiled it and it started again.
    #[serde(default)]
    pub attempts: u32,
    /// The dropouts the render found, each with the song position it
    /// happened at.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropouts: Vec<Dropout>,
}

/// A dropout a real-time render found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Dropout {
    /// The song position it happened at, in ticks from 1.1.1.
    pub position: Ticks,
    /// What it was: the device's report, or a ring too full.
    pub kind: String,
}

/// What the real-time render is doing now (the external devices spec, R23).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RealtimePhase {
    /// Nothing is rendering.
    Idle,
    /// Waiting for the hardware's sound to die away, with the seconds left.
    Waiting {
        /// The seconds left before the render starts.
        seconds: f64,
    },
    /// Rendering the range.
    Rendering,
}

/// What `export.realtime.status` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RealtimeStatus {
    /// What the render is doing now.
    pub phase: RealtimePhase,
    /// How much of the range is written, from 0 to 1, while it renders.
    pub progress: f64,
    /// How many times the render has started.
    pub attempts: u32,
    /// Whether the render starts again when a dropout spoils it.
    pub auto_restart: bool,
}

/// Arguments of `export.realtime.auto_restart`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutoRestartArgs {
    /// Whether the render starts again when a dropout spoils it.
    pub auto_restart: bool,
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
                "Renders the set's Main output (what you hear) or individual tracks to audio \
                 files, offline and faster than real time, as the File menu's Export Audio/Video \
                 does. `path` is the file to write; `bit_depth` (16, 24, or 32) and `sample_rate` \
                 are required. `rendered_track` chooses `main` (the default), `all_tracks`, \
                 `selected_tracks`, or a single track `{ \"track\": <id> }`. With an individual track, \
                 `include_return_and_main_effects` includes return tracks and Main devices (unavailable \
                 for `main`). Multi-track exports write one file per track beside `path`. The file type \
                 follows the extension (`.wav`, `.aif`/`.aiff`, `.flac`, `.mp3`; FLAC holds 16 or 24 \
                 bits only) or `file_type`. MP3 writes 320 kbps CBR at one of MP3's sample rates, and \
                 ignores `bit_depth` and `dither`. `render_as_loop` adds the tail the song leaves \
                 after the range's end onto the file's start, so the file loops without a cut tail. \
                 The range is `start` and `length` if given, otherwise the Arrangement's time \
                 selection, otherwise the whole set from 1.1.1 to the end of the last clip; it \
                 plays straight through, without the Arrangement loop. Below 32 bits, `dither` adds \
                 `triangular` noise unless told `rectangular` or `none`. `normalize` brings the \
                 highest peak to 0 dBFS. A rate below the engine's is rendered at the engine's and \
                 converted down. Files are written under temporary names and appear only when \
                 complete. Existing files are refused unless `replace` is true. Playback stops \
                 first, and plug-in devices render through the plug-ins the engine plays, lent to \
                 the export and handed back afterwards. An export whose path runs through external \
                 hardware renders in real time, on the running engine, after waiting \
                 `wait_seconds` seconds (10 unless given) so the hardware's sound dies away; \
                 `auto_restart` (on unless told otherwise) starts it again when a dropout spoils \
                 it, up to `max_attempts` times, and with it off the dropouts are listed. \
                 Changes nothing in the set.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<ExportArgs>()
            .result::<ExportResult>(),
    )?;
    // Spec: external-devices R23. The real-time render's dialog.
    registry.register(
        CommandSpec::new(REALTIME_STATUS, "Real-time Rendering Status")
            .description(
                "What the real-time render of an export with external hardware is doing now: \
                 `idle` when nothing is rendering, `waiting` with the seconds left before the \
                 render starts, or `rendering`. It also says how much of the range is written, \
                 how many times the render has started, and whether Auto-Restart on Drop-Outs is \
                 on.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<RealtimeStatus>(),
    )?;
    registry.register(
        CommandSpec::new(REALTIME_RESTART, "Restart Real-time Rendering")
            .description(
                "Starts the waiting period of the real-time render of an export with external \
                 hardware again, so the hardware's sound has the whole period to die away. \
                 Changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<NoArgs>()
            .result::<RealtimeStatus>(),
    )?;
    registry.register(
        CommandSpec::new(REALTIME_SKIP, "Skip Real-time Rendering Wait")
            .description(
                "Starts the real-time render of an export with external hardware at once, \
                 without waiting for the waiting period to run out. Changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<NoArgs>()
            .result::<RealtimeStatus>(),
    )?;
    registry.register(
        CommandSpec::new(REALTIME_CANCEL, "Cancel Real-time Rendering")
            .description(
                "Stops the real-time render of an export with external hardware and deletes its \
                 partial file. The export fails and the file it was writing is left as it was. \
                 Changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<NoArgs>()
            .result::<RealtimeStatus>(),
    )?;
    registry.register(
        CommandSpec::new(REALTIME_AUTO_RESTART, "Set Auto-Restart on Drop-Outs")
            .description(
                "Turns Auto-Restart on Drop-Outs on or off for the real-time render of an export \
                 with external hardware: on, the render starts again when a dropout spoils it; \
                 off, it carries on and the dropouts are listed. Changes nothing in the set.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<AutoRestartArgs>()
            .result::<RealtimeStatus>(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        // Spec: export R15, R16.
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

        // RenderedTrack variants
        for rendered in [
            json!("main"),
            json!("all_tracks"),
            json!("selected_tracks"),
            json!({ "track": 1 }),
        ] {
            let mut with_rendered = ok.clone();
            with_rendered["rendered_track"] = rendered;
            assert!(
                bl_commands::validate(schema, &with_rendered).is_ok(),
                "{with_rendered}"
            );
        }

        let mut with_include = ok.clone();
        with_include["include_return_and_main_effects"] = json!(true);
        assert!(bl_commands::validate(schema, &with_include).is_ok());

        for dither in ["none", "rectangular", "triangular", "noise_shaped"] {
            let mut with_dither = ok.clone();
            with_dither["dither"] = json!(dither);
            assert!(
                bl_commands::validate(schema, &with_dither).is_ok(),
                "{with_dither}"
            );
        }

        // MP3 joins the file types (WP-2.26).
        for file_type in ["wav", "aiff", "flac", "mp3"] {
            let mut with_type = ok.clone();
            with_type["file_type"] = json!(file_type);
            assert!(
                bl_commands::validate(schema, &with_type).is_ok(),
                "{with_type}"
            );
        }

        for bad in [
            json!({ "path": "/a/b.wav", "bit_depth": 8, "sample_rate": 48000 }),
            json!({ "path": "/a/b.wav", "bit_depth": 24 }),
            json!({ "path": "/a/b.wav", "bit_depth": 24, "sample_rate": 100 }),
            json!({ "path": "/a/b.wav", "bit_depth": 24, "sample_rate": 48000, "dither": "pow_r" }),
            json!({ "path": "/a/b.wav", "bit_depth": 24, "sample_rate": 48000, "rendered_track": "invalid" }),
            json!({ "path": "/a/b.wav", "bit_depth": 24, "sample_rate": 48000, "track": 1 }),
            json!({ "path": "/a/b.wav", "bit_depth": 24, "sample_rate": 48000, "file_type": "ogg" }),
        ] {
            assert!(bl_commands::validate(schema, &bad).is_err(), "{bad}");
        }
    }
}
