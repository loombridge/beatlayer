//! Perception (WP-1.16): how an agent hears and sees the set. The
//! `perception.*` commands, their arguments, and their results.
//!
//! - `perception.preview` renders a short range of the set, the whole mix
//!   or one track, offline, and keeps it as a WAV file in memory for the
//!   agent API to hand over (an audio content block, or the resource its
//!   `uri` names).
//! - `perception.analyze` measures a preview, or a range rendered for it,
//!   and reports numbers with units: loudness and true peak, sample peak
//!   and RMS, an octave-band spectrum, tempo, key, and transients.
//! - `perception.capture_view` captures one of the app's views as a PNG
//!   image. Only the app's window can draw one, so the agent API's
//!   `capture_view` tool runs it there; headless it's unavailable.
//!
//! None changes the set or writes a file, so all three are in the read
//! class: an agent never asks first for them. Agents reach them; they have
//! no menu item or key, because this is Beatlayer's own agent feature, not
//! a Live feature. The behaviour they follow is in the agent perception
//! spec.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::TrackId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::editing::Position;
use crate::export::RangeSource;

/// `perception.preview`: render a short audio preview of a range.
pub const PREVIEW: &str = "perception.preview";
/// `perception.analyze`: measure a preview or a range as numbers.
pub const ANALYZE: &str = "perception.analyze";
/// `perception.capture_view`: capture a view as a PNG image.
pub const CAPTURE_VIEW: &str = "perception.capture_view";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[PREVIEW, ANALYZE, CAPTURE_VIEW];

/// Whether `id` is one of the perception commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// The longest preview, in seconds: a longer range is cut to its first
/// 30 seconds.
pub const PREVIEW_SECONDS: f64 = 30.0;

/// The longest range `perception.analyze` renders, in seconds: ten
/// minutes. A longer one is cut.
pub const ANALYZE_SECONDS: f64 = 600.0;

/// How many previews are kept, newest first; an older one is forgotten.
pub const PREVIEWS_KEPT: usize = 4;

/// Where a preview is read as a resource: this, then its ID and `.wav`.
pub const PREVIEW_URI: &str = "beatlayer://preview/";

/// The most transients listed; more are counted.
pub const TRANSIENTS_LISTED: usize = 500;

/// The most short-term loudness readings listed, spread over the audio.
pub const SHORT_TERM_LISTED: usize = 60;

/// The URI a preview is read at.
#[must_use]
pub fn preview_uri(id: u64) -> String {
    format!("{PREVIEW_URI}{id}.wav")
}

/// The preview a URI names, if it names one.
#[must_use]
pub fn preview_of_uri(uri: &str) -> Option<u64> {
    uri.strip_prefix(PREVIEW_URI)?
        .strip_suffix(".wav")?
        .parse()
        .ok()
}

fn sixteen() -> u8 {
    16
}

/// What to render: a range of the set, of the whole mix or one track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RangeArgs {
    /// Where the range starts.
    pub start: Option<Position>,
    /// How long it is, in ticks.
    pub length: Option<Ticks>,
    /// The one track to hear.
    pub track: Option<TrackId>,
}

/// Arguments of `perception.preview`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewArgs {
    /// Where the range starts: ticks from 1.1.1, or bar.beat.sixteenth
    /// text such as `"9.1.1"`. Left out, the start of the Arrangement's
    /// time selection, or 1.1.1 without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// How long the range is, in ticks (705,600 to the quarter note, so a
    /// 4/4 bar is 2,822,400). Left out, to the end of the time selection,
    /// or without one to the end of the set's last clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<Ticks>,
    /// One track to hear on its own, as it sounds soloed: through its
    /// group tracks and the Main track, with the returns its sends reach
    /// while Solo in Place is on. Left out, the whole mix: the Main
    /// output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The preview's sample rate in hertz, from 8,000 to 192,000. Left
    /// out, the engine's rate. A lower rate makes a smaller file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 8000, max = 192_000))]
    pub sample_rate: Option<u32>,
    /// Bits per sample: 16 (the default, with triangular dither), 24, or
    /// 32 (floating point).
    #[serde(default = "sixteen")]
    #[schemars(extend("enum" = [16, 24, 32]))]
    pub bit_depth: u8,
}

impl PreviewArgs {
    /// The range asked for.
    #[must_use]
    pub fn range(&self) -> RangeArgs {
        RangeArgs {
            start: self.start.clone(),
            length: self.length,
            track: self.track,
        }
    }
}

impl AnalyzeArgs {
    /// The range asked for, when there's no preview.
    #[must_use]
    pub fn range(&self) -> RangeArgs {
        RangeArgs {
            start: self.start.clone(),
            length: self.length,
            track: self.track,
        }
    }
}

/// A track, by ID and name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrackRef {
    /// Its ID.
    pub id: TrackId,
    /// Its name.
    pub name: String,
}

/// Where rendered audio came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rendered {
    /// Where the range started, in ticks from 1.1.1.
    pub start: Ticks,
    /// Where it started, as bar.beat.sixteenth.
    pub start_position: String,
    /// Where it ended, in ticks: the first position not rendered.
    pub end: Ticks,
    /// Where it ended, as bar.beat.sixteenth.
    pub end_position: String,
    /// Where the range came from.
    pub range: RangeSource,
    /// Whether the range was cut to the longest render.
    pub truncated: bool,
    /// The one track heard, or null for the whole mix.
    pub track: Option<TrackRef>,
    /// The song's tempo where the range starts, in BPM, as the set says.
    pub set_tempo_bpm: f64,
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels in each frame.
    pub channels: u16,
    /// Frames rendered.
    pub frames: u64,
    /// How long it is, in seconds.
    pub seconds: f64,
    /// Media that clips name and that couldn't be found, by content hash:
    /// those clips are silent.
    pub missing_media: Vec<String>,
    /// Anything else worth knowing, such as a track whose activator is off.
    pub notes: Vec<String>,
}

/// What `perception.preview` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PreviewResult {
    /// The preview's ID, for `perception.analyze`.
    pub id: u64,
    /// The resource the WAV file is read at, such as
    /// `beatlayer://preview/1.wav`.
    pub uri: String,
    /// The file's media type: `audio/wav`.
    pub mime_type: String,
    /// Bits per sample.
    pub bit_depth: u8,
    /// The file's size in bytes.
    pub bytes: u64,
    /// The largest sample magnitude, in dBFS, or null for silence.
    pub peak_dbfs: Option<f64>,
    /// What was rendered.
    pub rendered: Rendered,
    /// How long the render took, in seconds.
    pub took_seconds: f64,
}

/// One kind of analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    /// Integrated, momentary, and short-term loudness, loudness range, and
    /// true peak.
    Loudness,
    /// Sample peak and RMS level.
    Levels,
    /// The level in each octave band.
    Spectrum,
    /// The tempo the audio suggests.
    Tempo,
    /// The key the audio suggests.
    Key,
    /// When notes and hits start.
    Transients,
}

impl Measure {
    /// Every measure, in the order results list them.
    pub const ALL: [Self; 6] = [
        Self::Loudness,
        Self::Levels,
        Self::Spectrum,
        Self::Tempo,
        Self::Key,
        Self::Transients,
    ];
}

/// Arguments of `perception.analyze`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnalyzeArgs {
    /// A preview to analyse, by the ID `perception.preview` gave. Left
    /// out, the range below is rendered for the analysis (up to ten
    /// minutes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<u64>,
    /// Without `preview`: where the range starts, as ticks from 1.1.1 or
    /// bar.beat.sixteenth text such as `"9.1.1"`. Left out, the start of
    /// the Arrangement's time selection, or 1.1.1 without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// How long the range is, in ticks (705,600 to the quarter note, so a
    /// 4/4 bar is 2,822,400). Left out, to the end of the time selection,
    /// or without one to the end of the set's last clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<Ticks>,
    /// One track to hear on its own, as it sounds soloed: through its
    /// group tracks and the Main track, with the returns its sends reach
    /// while Solo in Place is on. Left out, the whole mix: the Main
    /// output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// What to measure. Left out, everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<Vec<Measure>>,
}

/// One short-term loudness reading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShortTerm {
    /// When its 3-second window ends, in seconds from the start.
    pub at_seconds: f64,
    /// Its loudness in LUFS, or null for silence.
    pub lufs: Option<f64>,
}

/// Loudness, as EBU R 128 reports it (ITU-R BS.1770-4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LoudnessResult {
    /// Integrated loudness, gated, in LUFS: null for silence or under
    /// 0.4 seconds.
    pub integrated_lufs: Option<f64>,
    /// The loudest 0.4-second window, in LUFS.
    pub momentary_max_lufs: Option<f64>,
    /// The loudest 3-second window, in LUFS: null under 3 seconds.
    pub short_term_max_lufs: Option<f64>,
    /// Short-term loudness through the audio, at most 60 readings spread
    /// evenly.
    pub short_term: Vec<ShortTerm>,
    /// Loudness range, in LU (EBU Tech 3342): null under about 3 seconds.
    pub range_lu: Option<f64>,
    /// True peak, in dBTP (4 times oversampled at 48 kHz): null for
    /// silence.
    pub true_peak_dbtp: Option<f64>,
}

/// One channel's levels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelLevels {
    /// Its largest sample magnitude, in dBFS, or null for silence.
    pub sample_peak_dbfs: Option<f64>,
    /// Its RMS level, in dBFS (a full-scale sine reads −3.01), or null for
    /// silence.
    pub rms_dbfs: Option<f64>,
}

/// Sample peak and RMS level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LevelsResult {
    /// The largest sample magnitude in any channel, in dBFS.
    pub sample_peak_dbfs: Option<f64>,
    /// The RMS level of every channel together, in dBFS.
    pub rms_dbfs: Option<f64>,
    /// The sample peak less the RMS level, in dB.
    pub crest_factor_db: Option<f64>,
    /// Each channel's levels: left, then right.
    pub channels: Vec<ChannelLevels>,
}

/// One octave band.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BandResult {
    /// Its nominal centre, in hertz.
    pub centre_hz: f64,
    /// Its lower edge, in hertz.
    pub low_hz: f64,
    /// Its upper edge, in hertz.
    pub high_hz: f64,
    /// The level of what falls in it, in dBFS as the RMS level reads (a
    /// full-scale sine reads −3.01 in its band), or null for nothing.
    pub level_db: Option<f64>,
}

/// A coarse spectrum.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpectrumResult {
    /// The octave bands, 31.5 Hz to 16 kHz, lowest first; bands above the
    /// Nyquist frequency are left out.
    pub bands: Vec<BandResult>,
}

/// The tempo the audio suggests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TempoResult {
    /// Beats per minute, from 40 to 240, or null if the audio is shorter
    /// than 3 seconds or has no steady pulse. A tempo far from 120 may read
    /// as half or double.
    pub bpm: Option<f64>,
    /// How strongly the onsets repeat at that tempo, from 0 to 1.
    pub confidence: Option<f64>,
}

/// The key the audio suggests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KeyResult {
    /// The key, such as `C major` or `F# minor`, or null if nothing pitched
    /// was heard.
    pub key: Option<String>,
    /// How well the audio's pitches fit it: Pearson's r with its key
    /// profile, from −1 to 1.
    pub correlation: Option<f64>,
    /// The next best key, often the relative major or minor.
    pub runner_up: Option<String>,
    /// How well that fits.
    pub runner_up_correlation: Option<f64>,
}

/// When notes and hits start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TransientsResult {
    /// How many there are.
    pub count: usize,
    /// When each starts, in seconds from the start of the audio, at most
    /// 500.
    pub times_seconds: Vec<f64>,
    /// Where each falls in the song, as bar.beat.sixteenth (rounded down
    /// to the sixteenth), when the audio is a range of the set.
    pub positions: Vec<String>,
    /// Whether there were more than are listed.
    pub truncated: bool,
}

/// What `perception.analyze` returns. A measure not asked for is null.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AnalyzeResult {
    /// The preview analysed, or null for a range rendered for it.
    pub preview: Option<u64>,
    /// What was analysed.
    pub rendered: Rendered,
    /// The measurements in a few lines of text.
    pub summary: String,
    /// Loudness and true peak.
    pub loudness: Option<LoudnessResult>,
    /// Sample peak and RMS.
    pub levels: Option<LevelsResult>,
    /// The octave-band spectrum.
    pub spectrum: Option<SpectrumResult>,
    /// The tempo.
    pub tempo: Option<TempoResult>,
    /// The key.
    pub key: Option<KeyResult>,
    /// The transients.
    pub transients: Option<TransientsResult>,
}

/// A view to capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum View {
    /// The whole main window.
    Window,
    /// The Arrangement View, with its mixer if it shows.
    Arrangement,
    /// The Session View.
    Session,
    /// The mixer: the Arrangement's mixer section, or the Session View.
    Mixer,
    /// The browser.
    Browser,
    /// The detail area at the bottom: the Clip View or Device View, with
    /// the Info View.
    Detail,
    /// The Settings window.
    Settings,
}

impl View {
    /// The view's ID, as the arguments name it.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Arrangement => "arrangement",
            Self::Session => "session",
            Self::Mixer => "mixer",
            Self::Browser => "browser",
            Self::Detail => "detail",
            Self::Settings => "settings",
        }
    }
}

/// The default and the limits of a capture's longer side, in pixels.
pub const CAPTURE_SIZE: (u32, u32, u32) = (1568, 256, 4096);

/// Arguments of `perception.capture_view`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureArgs {
    /// The view to capture. It must be on screen: the main window shows
    /// the Arrangement or the Session View, and the browser, the mixer, and
    /// the Settings window only while they're open.
    pub view: View,
    /// The longest side of the image, in pixels, from 256 to 4,096: a
    /// larger capture is scaled down to fit. Left out, 1,568.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 256, max = 4096))]
    pub max_size: Option<u32>,
}

/// What `perception.capture_view` returns, beside the image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CaptureResult {
    /// The view captured.
    pub view: View,
    /// The image's width in pixels.
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
    /// Its media type: `image/png`.
    pub mime_type: String,
    /// The PNG file's size in bytes.
    pub bytes: u64,
    /// Image pixels to each of the window's points: under the screen's own
    /// where the capture was scaled down.
    pub pixels_per_point: f64,
}

/// Adds the perception commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(PREVIEW, "Preview Audio")
            .description(
                "Renders a short audio preview of a range of the set, offline, through the \
                 engine that exports, and keeps it as a WAV file for you to listen to: the \
                 agent API's `preview` tool returns it as audio, and `uri` reads it as a \
                 resource. The range is `start` and `length` if given, otherwise the \
                 Arrangement's time selection, otherwise the whole set; it plays straight \
                 through, without the loop, and is cut to its first 30 seconds. `track` \
                 previews one track as it sounds soloed; left out, the whole mix. 16-bit \
                 stereo at 48 kHz is about 190 KB a second: a lower `sample_rate` makes it \
                 smaller. Plug-in devices render through the plug-ins the engine plays, lent \
                 to the render, so while the transport plays a set with a CLAP plug-in the \
                 preview is refused (run `transport.stop` first); otherwise rendering doesn't \
                 stop or disturb playback. The last 4 previews are kept, for \
                 `perception.analyze`. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<PreviewArgs>()
            .result::<PreviewResult>(),
    )?;
    registry.register(
        CommandSpec::new(ANALYZE, "Analyze Audio")
            .description(
                "Measures audio and reports numbers with units: loudness (integrated, \
                 momentary and short-term LUFS, loudness range in LU, true peak in dBTP, as \
                 ITU-R BS.1770-4 and EBU R 128 define them), sample peak and RMS in dBFS, \
                 the level in each octave band from 31.5 Hz to 16 kHz, the tempo in BPM, the \
                 key, and when transients start. Give `preview` to measure a preview, or a \
                 range (`start`, `length`, and `track`, as for `perception.preview`) to \
                 render one for the analysis, up to ten minutes. `measure` picks what to \
                 measure; left out, everything. Tempo and key are estimates from the audio, \
                 not the set's settings. A range renders as `perception.preview` does, \
                 refused likewise while the transport plays a set with a CLAP plug-in. \
                 Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<AnalyzeArgs>()
            .result::<AnalyzeResult>(),
    )?;
    registry.register(
        CommandSpec::new(CAPTURE_VIEW, "Capture View")
            .description(
                "Captures one of the app's views as a PNG image, as the person sees it, for a \
                 model that reads images: the whole `window`, the `arrangement` or `session` \
                 view, the `mixer`, the `browser`, the `detail` area (Clip or Device View), or \
                 the `settings` window. The view must be on screen. The image is scaled to fit \
                 `max_size` pixels. Only the agent API's `capture_view` tool returns the \
                 image, and only while the app's window is open: headless (`bl-cli mcp`) \
                 there's no window to capture. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<CaptureArgs>()
            .result::<CaptureResult>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        // Spec: agent-perception R1.
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
        for id in IDS {
            let spec = registry.get(id).unwrap();
            assert_eq!(spec.permission, Some(Permission::Read), "{id}");
            assert!(spec.bindings.is_empty(), "{id} is the agent API's own");
        }
        let preview = registry.get(PREVIEW).unwrap().args.as_ref().unwrap();
        for ok in [
            json!({}),
            json!({ "start": "2.1.1", "length": 2_822_400, "track": 3 }),
            json!({ "sample_rate": 22_050, "bit_depth": 24 }),
        ] {
            assert!(bl_commands::validate(preview, &ok).is_ok(), "{ok}");
        }
        for bad in [
            json!({ "bit_depth": 8 }),
            json!({ "sample_rate": 4_000 }),
            json!({ "seconds": 3 }),
        ] {
            assert!(bl_commands::validate(preview, &bad).is_err(), "{bad}");
        }
        let analyze = registry.get(ANALYZE).unwrap().args.as_ref().unwrap();
        assert!(
            bl_commands::validate(analyze, &json!({ "preview": 1, "measure": ["key"] })).is_ok()
        );
        assert!(bl_commands::validate(analyze, &json!({ "measure": ["mood"] })).is_err());
        let capture = registry.get(CAPTURE_VIEW).unwrap().args.as_ref().unwrap();
        assert!(bl_commands::validate(capture, &json!({ "view": "mixer" })).is_ok());
        assert!(bl_commands::validate(capture, &json!({ "view": "piano_roll" })).is_err());
        assert!(
            bl_commands::validate(capture, &json!({ "view": "window", "max_size": 64 })).is_err()
        );
    }

    #[test]
    fn previews_have_uris() {
        assert_eq!(preview_uri(7), "beatlayer://preview/7.wav");
        assert_eq!(preview_of_uri("beatlayer://preview/7.wav"), Some(7));
        assert_eq!(preview_of_uri("beatlayer://guide"), None);
        assert_eq!(preview_of_uri("beatlayer://preview/x.wav"), None);
    }
}
