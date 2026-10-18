//! Perception (WP-1.16): audio previews and their analysis, for the agent
//! API. The behaviour it follows is in the agent perception spec.
//!
//! - [`preview`] renders a range of the set, the whole mix or one track
//!   soloed, through the export's offline render ([`crate::export::render`],
//!   the engine code that plays the set on a device), cut to
//!   [`PREVIEW_SECONDS`], and keeps it in [`Previews`]: the last
//!   [`PREVIEWS_KEPT`], as samples, made into a WAV file when it's read.
//! - [`analyze`] measures a preview, or a range rendered for it, with
//!   `bl-analysis`: loudness and true peak, levels, the octave-band
//!   spectrum, tempo, key, and transients.
//!
//! In the app, perception calls without CLAP, VST3 or Audio Unit devices
//! defer rendering to a worker thread in the app host (WP-2.28), while this
//! module provides the synchronous render primitives they execute.
//! Sets with such plug-in devices render synchronously on the calling thread
//! while the transport is stopped (the offline plug-in renders spec, R8),
//! because plug-in instances are main-thread objects borrowed from the engine.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use bl_analysis::{Audio, amplitude_db, key, levels, loudness, onsets, power_db, spectrum, tempo};
use bl_core::time::{Frames, SampleRate, Ticks};
use bl_engine::MediaLibrary;
use bl_media::{BitDepth, Dither};
use bl_model::{Command, Document};
use bl_shell_spec::perception::{
    ANALYZE, ANALYZE_SECONDS, AnalyzeArgs, AnalyzeResult, BandResult, ChannelLevels, KeyResult,
    LevelsResult, LoudnessResult, Measure, PREVIEW, PREVIEW_SECONDS, PREVIEWS_KEPT, PreviewArgs,
    PreviewResult, RangeArgs, Rendered, SHORT_TERM_LISTED, ShortTerm, SpectrumResult,
    TRANSIENTS_LISTED, TempoResult, TrackRef, TransientsResult, preview_uri,
};

use crate::ShellError;
use crate::export::{self, ExportError, Hosts, Length, Render};

/// A preview kept for the agent API: the rendered samples and what they
/// are.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    /// Its ID.
    pub id: u64,
    /// Interleaved samples, at `rendered.sample_rate`.
    pub samples: Vec<f32>,
    /// The WAV file's bits per sample.
    pub bit_depth: u8,
    /// What was rendered.
    pub rendered: Rendered,
    /// The set as it was rendered, whose tempo and time signatures place
    /// the preview's transients in the song. Versions of a set share
    /// almost everything, so keeping one costs little.
    pub document: Document,
}

/// Encodes interleaved audio samples as a WAV byte buffer (WP-2.28).
///
/// # Errors
///
/// A message if the bit depth is unsupported or WAV encoding fails.
pub fn encode_wav(
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
    bit_depth: u8,
) -> Result<Vec<u8>, String> {
    let depth =
        BitDepth::from_bits(u32::from(bit_depth)).ok_or_else(|| format!("{bit_depth} bits"))?;
    bl_media::wav_bytes(
        samples,
        channels,
        sample_rate,
        depth,
        if depth.is_integer() {
            Dither::Triangular
        } else {
            Dither::None
        },
    )
    .map_err(|error| error.to_string())
}

impl Preview {
    /// The preview as a WAV file: 16 bits with triangular dither, 24 bits,
    /// or 32-bit float, as it was asked for. The same preview makes the
    /// same bytes every time.
    ///
    /// # Errors
    ///
    /// A message if it can't be encoded.
    pub fn wav(&self) -> Result<Vec<u8>, String> {
        encode_wav(
            &self.samples,
            self.rendered.channels,
            self.rendered.sample_rate,
            self.bit_depth,
        )
    }

    /// What `perception.preview` returns for this preview with pre-encoded WAV bytes (WP-2.28).
    #[must_use]
    pub fn result_with_bytes(
        &self,
        bytes: u64,
        peak_dbfs: Option<f64>,
        took_seconds: f64,
    ) -> PreviewResult {
        PreviewResult {
            id: self.id,
            uri: preview_uri(self.id),
            mime_type: "audio/wav".to_owned(),
            bit_depth: self.bit_depth,
            bytes,
            peak_dbfs,
            rendered: self.rendered.clone(),
            took_seconds,
        }
    }

    /// What `perception.preview` returns for this preview.
    ///
    /// # Errors
    ///
    /// A message if the WAV file couldn't be encoded.
    pub fn result(
        &self,
        peak_dbfs: Option<f64>,
        took_seconds: f64,
    ) -> Result<PreviewResult, String> {
        let bytes = self.wav()?.len() as u64;
        Ok(self.result_with_bytes(bytes, peak_dbfs, took_seconds))
    }
}

/// The last previews made, newest last.
#[derive(Clone, Debug, Default)]
pub struct Previews {
    kept: VecDeque<Preview>,
    last_id: u64,
}

impl Previews {
    /// The preview with this ID, if it's still kept.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Preview> {
        self.kept.iter().find(|preview| preview.id == id)
    }

    /// The IDs of the previews kept, oldest first.
    pub fn ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.kept.iter().map(|preview| preview.id)
    }

    pub fn add(
        &mut self,
        samples: Vec<f32>,
        bit_depth: u8,
        rendered: Rendered,
        document: Document,
    ) -> &Preview {
        self.last_id += 1;
        while self.kept.len() >= PREVIEWS_KEPT {
            self.kept.pop_front();
        }
        self.kept.push_back(Preview {
            id: self.last_id,
            samples,
            bit_depth,
            rendered,
            document,
        });
        self.kept.back().expect("one was just added")
    }
}

/// What a render needs from the shell: the set, its media, its plug-ins,
/// the Arrangement's time selection, the engine's rate, and whether the
/// transport plays.
pub struct Source<'a> {
    /// The set.
    pub document: &'a Document,
    /// Its clips' audio.
    pub media: Arc<dyn MediaLibrary>,
    /// The Arrangement's time selection, if there is one.
    pub selection: Option<(Ticks, Ticks)>,
    /// The engine's rate.
    pub engine_rate: u32,
    /// The plug-ins the engine plays, which the render borrows.
    pub plugins: Hosts<'a>,
    /// Whether the transport plays.
    pub playing: bool,
    /// A cancellation flag observed during offline rendering (WP-2.28).
    pub cancel: Option<&'a AtomicBool>,
}

fn invalid(command: &str, message: impl Into<String>) -> ShellError {
    ShellError::InvalidArgs {
        command: command.to_owned(),
        message: message.into(),
    }
}

fn failed(command: &str, message: impl Into<String>) -> ShellError {
    ShellError::Failed {
        command: command.to_owned(),
        message: message.into(),
    }
}

/// Rounds to `places` decimal places, for results people and models read.
fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

fn round_opt(value: Option<f64>, places: i32) -> Option<f64> {
    value.map(|v| round(v, places))
}

/// The set to render: as it is, or with `track` soloed alone (Spec:
/// agent-perception R3). Returns it, the track, and notes for the result.
fn document_for(
    command: &str,
    document: &Document,
    track: Option<bl_model::TrackId>,
) -> Result<(Document, Option<TrackRef>, Vec<String>), ShellError> {
    let Some(id) = track else {
        return Ok((document.clone(), None, Vec::new()));
    };
    if id == document.main_id() {
        return Err(invalid(
            command,
            "the Main track is the whole mix: leave `track` out to hear it",
        ));
    }
    let found = document
        .track(id)
        .ok_or_else(|| invalid(command, format!("no track has the ID {}", id.0)))?;
    let mut notes = Vec::new();
    if found.mixer().mute {
        notes.push(format!(
            "the track \u{201c}{}\u{201d} has its activator off, so it's silent",
            found.name()
        ));
    }
    let mut soloed = document.clone();
    soloed
        .apply(&Command::SoloExclusive { track: id })
        .map_err(|error| invalid(command, error.to_string()))?;
    Ok((
        soloed,
        Some(TrackRef {
            id,
            name: found.name().to_owned(),
        }),
        notes,
    ))
}

/// Renders the range `range` asks for, at `sample_rate` (or the engine's),
/// cut to `longest` seconds. Returns the interleaved samples, what was
/// rendered, and how long it took.
fn render_range(
    command: &str,
    source: &Source<'_>,
    range: &RangeArgs,
    sample_rate: Option<u32>,
    longest: f64,
) -> Result<(Vec<f32>, Rendered, f64), ShellError> {
    let began = Instant::now();
    let (from, to, range_source) = export::range(
        source.document,
        range.start.as_ref(),
        range.length,
        source.selection,
    )
    .map_err(|message| invalid(command, message))?;
    let (document, track, mut notes) = document_for(command, source.document, range.track)?;
    // Spec: offline-plugin-renders R8. Borrowing the plug-ins would take
    // them from the song playing.
    let borrows = source
        .plugins
        .borrows_any(|format| crate::plugins::has_format(&document, format));
    if source.playing && borrows {
        return Err(ShellError::Unavailable {
            command: command.to_owned(),
            reason: "the set is playing, and rendering it would take its plug-ins from the \
                     engine playing it; stop the transport with `transport.stop`, then render"
                .to_owned(),
        });
    }
    let sample_rate = sample_rate.unwrap_or(source.engine_rate);
    // Spec: agent-perception R2. The range is cut to the longest render.
    let render_hz = sample_rate.max(source.engine_rate);
    let rate = SampleRate::new(render_hz)
        .ok_or_else(|| invalid(command, format!("a rate of {render_hz} Hz")))?;
    let settings = document.settings();
    let first = settings.tempo.ticks_to_frames(from, rate).get();
    let whole = settings.tempo.ticks_to_frames(to, rate).get() - first;
    let cap = (longest * f64::from(render_hz)).floor() as i64;
    let (length, end, truncated) = if whole > cap {
        let end = settings.tempo.frames_to_ticks(Frames(first + cap), rate);
        (Length::Frames(cap as u64), end, true)
    } else {
        (Length::To(to), to, false)
    };
    let spec = Render {
        start: from,
        length,
        sample_rate,
        engine_rate: source.engine_rate,
        block: export::BLOCK,
    };
    static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);
    let cancel = source.cancel.unwrap_or(&NEVER_CANCEL);
    let mut samples = Vec::new();
    let rendered = export::render(
        &document,
        Arc::clone(&source.media),
        source.plugins,
        &spec,
        longest + 1.0,
        cancel,
        &mut |_| {},
        &mut |block| {
            samples.extend_from_slice(block);
            Ok(())
        },
        None,
    )
    .map_err(|error| match error {
        ExportError::Invalid(message) => invalid(command, message),
        other => failed(command, other.to_string()),
    })?;
    if truncated {
        notes.push(format!(
            "the range was cut to its first {longest} seconds, the longest this command renders"
        ));
    }
    // Spec: external-devices R16, R27. A preview reaches no hardware: an
    // external device sends nothing and brings back silence.
    for device in document.external_devices() {
        notes.push(format!(
            "{device} sent nothing and played silence: a preview doesn't reach hardware"
        ));
    }
    let signatures = &settings.time_signatures;
    Ok((
        samples,
        Rendered {
            start: from,
            start_position: signatures.to_bbs(from).to_string(),
            end,
            end_position: signatures.to_bbs(end).to_string(),
            range: range_source,
            truncated,
            track,
            set_tempo_bpm: settings.tempo.bpm_at(from).get(),
            sample_rate,
            channels: rendered.channels,
            frames: rendered.frames,
            seconds: rendered.frames as f64 / f64::from(sample_rate),
            missing_media: rendered
                .missing_media
                .iter()
                .map(ToString::to_string)
                .collect(),
            notes,
        },
        began.elapsed().as_secs_f64(),
    ))
}

/// What rendering a preview produced (WP-1.16, WP-2.28).
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewRenderOutcome {
    /// The interleaved audio samples.
    pub samples: Vec<f32>,
    /// Metadata of what was rendered.
    pub rendered: Rendered,
    /// Peak amplitude in dBFS.
    pub peak_dbfs: Option<f64>,
    /// How many seconds the render took.
    pub took_seconds: f64,
}

/// Renders a preview range (WP-1.16, WP-2.28).
///
/// # Errors
///
/// [`ShellError`] for invalid arguments, unavailable devices, or render failures.
pub fn render_preview(
    source: &Source<'_>,
    args: &PreviewArgs,
) -> Result<PreviewRenderOutcome, ShellError> {
    if let Some(rate) = args.sample_rate
        && !(8_000..=192_000).contains(&rate)
    {
        return Err(invalid(
            PREVIEW,
            format!("a sample rate of {rate} Hz; previews take 8,000 to 192,000 Hz"),
        ));
    }
    if !matches!(args.bit_depth, 16 | 24 | 32) {
        return Err(invalid(
            PREVIEW,
            format!("{} bits; give 16, 24, or 32", args.bit_depth),
        ));
    }
    let (samples, rendered, took) = render_range(
        PREVIEW,
        source,
        &args.range(),
        args.sample_rate,
        PREVIEW_SECONDS,
    )?;
    let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    Ok(PreviewRenderOutcome {
        samples,
        rendered,
        peak_dbfs: round_opt(amplitude_db(f64::from(peak)), 2),
        took_seconds: round(took, 3),
    })
}

/// Runs `perception.preview`: renders the range and keeps it.
///
/// # Errors
///
/// [`ShellError`] for a range or track that can't be rendered, or a render
/// that failed.
pub fn preview(
    previews: &mut Previews,
    source: &Source<'_>,
    args: &PreviewArgs,
) -> Result<PreviewResult, ShellError> {
    let outcome = render_preview(source, args)?;
    let preview = previews.add(
        outcome.samples,
        args.bit_depth,
        outcome.rendered,
        source.document.clone(),
    );
    preview
        .result(outcome.peak_dbfs, outcome.took_seconds)
        .map_err(|message| failed(PREVIEW, message))
}

/// Runs `perception.analyze`: measures a preview or a range rendered for
/// it.
///
/// # Errors
///
/// [`ShellError`] for a preview no longer kept, a range or track that
/// can't be rendered, or a render that failed.
pub fn analyze(
    previews: &Previews,
    source: &Source<'_>,
    args: &AnalyzeArgs,
) -> Result<AnalyzeResult, ShellError> {
    let rendered_here;
    let (samples, rendered, preview, document): (&[f32], Rendered, Option<u64>, &Document) =
        match args.preview {
            Some(id) => {
                if args.start.is_some() || args.length.is_some() || args.track.is_some() {
                    return Err(invalid(
                        ANALYZE,
                        "give `preview`, or a range (`start`, `length`, `track`), not both",
                    ));
                }
                let kept = previews.get(id).ok_or_else(|| {
                    let kept: Vec<String> = previews.ids().map(|id| id.to_string()).collect();
                    invalid(
                        ANALYZE,
                        format!(
                            "no preview {id} is kept (the last {PREVIEWS_KEPT} are; kept now: {}); \
                         make a new one, or give a range",
                            if kept.is_empty() {
                                "none".to_owned()
                            } else {
                                kept.join(", ")
                            }
                        ),
                    )
                })?;
                (
                    &kept.samples,
                    kept.rendered.clone(),
                    Some(id),
                    &kept.document,
                )
            }
            None => {
                let (samples, rendered, _) =
                    render_range(ANALYZE, source, &args.range(), None, ANALYZE_SECONDS)?;
                rendered_here = samples;
                (&rendered_here, rendered, None, source.document)
            }
        };
    let audio = Audio::new(
        samples,
        usize::from(rendered.channels),
        rendered.sample_rate,
    )
    .map_err(|message| failed(ANALYZE, message))?;
    let wanted = args
        .measure
        .clone()
        .unwrap_or_else(|| Measure::ALL.to_vec());
    let wants = |measure: Measure| wanted.contains(&measure);

    let loudness = wants(Measure::Loudness).then(|| loudness_result(&audio));
    let levels = wants(Measure::Levels).then(|| levels_result(&audio));
    let spectrum = wants(Measure::Spectrum).then(|| SpectrumResult {
        bands: spectrum::octave_bands(&audio)
            .into_iter()
            .map(|band| BandResult {
                centre_hz: band.centre,
                low_hz: round(band.low, 1),
                high_hz: round(band.high, 1),
                level_db: round_opt(power_db(band.power), 2),
            })
            .collect(),
    });
    let (tempo, transients) = if wants(Measure::Tempo) || wants(Measure::Transients) {
        let mono = audio.mono();
        let function = onsets::onset_function(&mono, audio.rate());
        let times = onsets::detect(&mono, &function);
        let tempo = wants(Measure::Tempo).then(|| {
            let found = tempo::estimate(&function, &times);
            TempoResult {
                bpm: found.map(|t| round(t.bpm, 2)),
                confidence: found.map(|t| round(t.confidence, 3)),
            }
        });
        let transients =
            wants(Measure::Transients).then(|| transients_result(document, &rendered, &times));
        (tempo, transients)
    } else {
        (None, None)
    };
    let key = wants(Measure::Key).then(|| {
        let found = key::estimate(&audio);
        KeyResult {
            key: found.as_ref().map(|k| k.key.to_string()),
            correlation: found.as_ref().map(|k| round(k.correlation, 3)),
            runner_up: found.as_ref().map(|k| k.runner_up.to_string()),
            runner_up_correlation: found.as_ref().map(|k| round(k.runner_up_correlation, 3)),
        }
    });
    let mut result = AnalyzeResult {
        preview,
        rendered,
        summary: String::new(),
        loudness,
        levels,
        spectrum,
        tempo,
        key,
        transients,
    };
    result.summary = summary(&result);
    Ok(result)
}

fn loudness_result(audio: &Audio<'_>) -> LoudnessResult {
    let measured = loudness::measure(audio);
    let series = &measured.short_term;
    let picked: Vec<&(f64, f64)> = if series.len() <= SHORT_TERM_LISTED {
        series.iter().collect()
    } else {
        (0..SHORT_TERM_LISTED)
            .map(|i| &series[i * (series.len() - 1) / (SHORT_TERM_LISTED - 1)])
            .collect()
    };
    LoudnessResult {
        integrated_lufs: round_opt(measured.integrated, 2),
        momentary_max_lufs: round_opt(measured.momentary_max, 2),
        short_term_max_lufs: round_opt(measured.short_term_max, 2),
        short_term: picked
            .into_iter()
            .map(|&(at, lufs)| ShortTerm {
                at_seconds: round(at, 1),
                lufs: lufs.is_finite().then(|| round(lufs, 2)),
            })
            .collect(),
        range_lu: round_opt(measured.range, 2),
        true_peak_dbtp: round_opt(measured.true_peak, 2),
    }
}

fn levels_result(audio: &Audio<'_>) -> LevelsResult {
    let measured = levels::measure(audio);
    let peak = amplitude_db(measured.peak_all);
    let rms = amplitude_db(measured.rms_all);
    LevelsResult {
        sample_peak_dbfs: round_opt(peak, 2),
        rms_dbfs: round_opt(rms, 2),
        crest_factor_db: peak.zip(rms).map(|(p, r)| round(p - r, 2)),
        channels: measured
            .peak
            .iter()
            .zip(&measured.rms)
            .map(|(&p, &r)| ChannelLevels {
                sample_peak_dbfs: round_opt(amplitude_db(p), 2),
                rms_dbfs: round_opt(amplitude_db(r), 2),
            })
            .collect(),
    }
}

/// The transients, in seconds and as positions in the song.
fn transients_result(document: &Document, rendered: &Rendered, times: &[f64]) -> TransientsResult {
    let listed = &times[..times.len().min(TRANSIENTS_LISTED)];
    let settings = document.settings();
    let positions = SampleRate::new(rendered.sample_rate).map_or_else(Vec::new, |rate| {
        let first = settings.tempo.ticks_to_frames(rendered.start, rate).get();
        listed
            .iter()
            .map(|&t| {
                let frame = first + (t * f64::from(rendered.sample_rate)).round() as i64;
                let ticks = settings.tempo.frames_to_ticks(Frames(frame), rate);
                settings.time_signatures.to_bbs(ticks).to_string()
            })
            .collect()
    });
    TransientsResult {
        count: times.len(),
        times_seconds: listed.iter().map(|&t| round(t, 4)).collect(),
        positions,
        truncated: times.len() > listed.len(),
    }
}

/// The measurements in a few lines of text.
fn summary(result: &AnalyzeResult) -> String {
    let db = |v: Option<f64>, unit: &str| {
        v.map_or_else(|| "none (silence)".to_owned(), |v| format!("{v:.1} {unit}"))
    };
    let mut lines = vec![format!(
        "{:.2} s at {} Hz{}, from {} to {}.",
        result.rendered.seconds,
        result.rendered.sample_rate,
        result
            .rendered
            .track
            .as_ref()
            .map_or_else(String::new, |t| format!(
                ", track \u{201c}{}\u{201d} soloed",
                t.name
            )),
        result.rendered.start_position,
        result.rendered.end_position,
    )];
    if let Some(l) = &result.loudness {
        lines.push(format!(
            "Loudness: integrated {}, short-term max {}, range {}, true peak {}.",
            db(l.integrated_lufs, "LUFS"),
            db(l.short_term_max_lufs, "LUFS"),
            l.range_lu
                .map_or_else(|| "none".to_owned(), |v| format!("{v:.1} LU")),
            db(l.true_peak_dbtp, "dBTP"),
        ));
    }
    if let Some(l) = &result.levels {
        lines.push(format!(
            "Levels: sample peak {}, RMS {}.",
            db(l.sample_peak_dbfs, "dBFS"),
            db(l.rms_dbfs, "dBFS"),
        ));
    }
    if let Some(s) = &result.spectrum
        && let Some(loudest) = s
            .bands
            .iter()
            .filter(|b| b.level_db.is_some())
            .max_by(|a, b| {
                a.level_db
                    .unwrap_or(f64::MIN)
                    .total_cmp(&b.level_db.unwrap_or(f64::MIN))
            })
    {
        lines.push(format!(
            "Spectrum: loudest octave band {} Hz at {}.",
            loudest.centre_hz,
            db(loudest.level_db, "dB"),
        ));
    }
    if let Some(t) = &result.tempo {
        lines.push(match (t.bpm, t.confidence) {
            (Some(bpm), Some(c)) => format!("Tempo: {bpm:.2} BPM (confidence {c:.2})."),
            _ => "Tempo: no steady pulse found.".to_owned(),
        });
    }
    if let Some(k) = &result.key {
        lines.push(match (&k.key, k.correlation, &k.runner_up) {
            (Some(key), Some(r), Some(next)) => {
                format!("Key: {key} (r {r:.2}; next {next}).")
            }
            _ => "Key: nothing pitched heard.".to_owned(),
        });
    }
    if let Some(t) = &result.transients {
        lines.push(match t.times_seconds.first() {
            Some(first) => format!(
                "Transients: {}, the first at {first:.3} s{}.",
                t.count,
                t.positions
                    .first()
                    .map_or_else(String::new, |p| format!(" ({p})"))
            ),
            None => "Transients: none.".to_owned(),
        });
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_previews_are_kept() {
        let mut previews = Previews::default();
        let rendered = Rendered {
            start: Ticks::ZERO,
            start_position: "1.1.1".to_owned(),
            end: Ticks::ZERO,
            end_position: "1.1.1".to_owned(),
            range: bl_shell_spec::export::RangeSource::Arguments,
            truncated: false,
            track: None,
            set_tempo_bpm: 120.0,
            sample_rate: 48_000,
            channels: 2,
            frames: 1,
            seconds: 1.0 / 48_000.0,
            missing_media: Vec::new(),
            notes: Vec::new(),
        };
        for _ in 0..PREVIEWS_KEPT + 2 {
            previews.add(
                vec![0.0, 0.0],
                16,
                rendered.clone(),
                bl_model::Editor::new().document().clone(),
            );
        }
        assert_eq!(previews.ids().collect::<Vec<_>>(), vec![3, 4, 5, 6]);
        assert!(previews.get(2).is_none());
        let wav = previews.get(6).unwrap().wav().unwrap();
        assert_eq!(&wav[..4], b"RIFF");
    }
}
