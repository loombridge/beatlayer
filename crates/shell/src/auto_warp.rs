//! The warp settings a new clip gets (WP-4.9, the warping spec R31 to R36):
//! short and long samples, what Auto decides, and the maps auto-warp makes.
//! Clips made from a file by a drop, Enter or an import go through
//! [`new_clip`].

use bl_analysis::cache::{Analysis, AnalysisCache};
use bl_core::time::{TempoMap, Ticks};
use bl_media::ContentHash as MediaHash;
use bl_model::{
    AudioData, ContentHash, Document, EVEN_LOOP_BARS, FileBeats, WarpMarker, WarpMode, loop_bpm,
    nearest_bars,
};
use bl_shell_spec::settings::{DefaultWarpMode, LoopWarpShortSamples, WarpPrefs};

use crate::browser::arrange::{MediaFile, length_at};
use crate::clip_defaults;

/// A sample of this many seconds or more is a long one (R31): Beatlayer's
/// own threshold.
pub const SHORT_SAMPLE_SECONDS: f64 = 60.0;

/// The least confidence that counts as a steady beat (R32).
const STEADY: f64 = 0.5;

/// How far a tempo may be from the one a loop's length gives, as a share of
/// it, for Auto to take the sample for a loop (R32).
const TEMPO_FIT: f64 = 0.02;

/// What a new clip's warp settings are made from.
#[derive(Clone, Copy)]
pub struct Warping<'a> {
    /// The preferences.
    pub prefs: WarpPrefs,
    /// Where files' analyses are made. Without one, new clips are left
    /// unwarped.
    pub analysis: Option<&'a AnalysisCache>,
}

/// A new clip's data and how long it lasts in the Arrangement.
pub struct NewClip {
    /// The clip's data.
    pub audio: AudioData,
    /// Its length in ticks.
    pub length: Ticks,
}

/// The warp mode Default Warp Mode names.
#[must_use]
pub fn warp_mode(setting: DefaultWarpMode) -> WarpMode {
    match setting {
        DefaultWarpMode::Beats => WarpMode::Beats,
        DefaultWarpMode::Tones => WarpMode::Tones,
        DefaultWarpMode::Texture => WarpMode::Texture,
        DefaultWarpMode::RePitch => WarpMode::RePitch,
        DefaultWarpMode::Complex => WarpMode::Complex,
        DefaultWarpMode::ComplexPro => WarpMode::ComplexPro,
    }
}

/// `audio` as a clip added at `at`: a warped one lasts from its Start to its
/// End marker, any other as long as the file plays at the set's tempo (R33).
fn sized(audio: AudioData, seconds: f64, doc: &Document, at: Ticks) -> NewClip {
    let length = match audio.warp.then(|| audio.warp_map().ok()).flatten() {
        Some(map) => {
            let end = audio.end.unwrap_or_else(|| map.time_to_beat(seconds));
            Ticks(((end - audio.start).round() as i64).max(1))
        }
        None => length_at(doc, at, seconds),
    };
    NewClip { audio, length }
}

/// The clip a file of `seconds` becomes when it is added at `at` (R33): the
/// settings saved for the file (R42) if it has any, else the warp settings
/// its preferences ask for, waiting for the file's analysis where they need
/// it (R36), or a plain unwarped clip that plays as long as the file does at
/// the set's tempo.
#[must_use]
pub fn new_clip(
    file: &MediaFile,
    seconds: f64,
    doc: &Document,
    at: Ticks,
    warping: Warping<'_>,
) -> NewClip {
    let media = file.hash;
    if let Some(audio) = clip_defaults::load(&file.path, media)
        .and_then(|defaults| AudioData::new(media).with_defaults(&defaults).ok())
    {
        return sized(audio, seconds, doc, at);
    }
    let plain = || sized(AudioData::new(media), seconds, doc, at);
    let short = seconds < SHORT_SAMPLE_SECONDS;
    let wanted = if short {
        warping.prefs.loop_warp_short_samples != LoopWarpShortSamples::UnwarpedOneShot
    } else {
        warping.prefs.auto_warp_long_samples
    };
    let Some(cache) = warping.analysis.filter(|_| wanted) else {
        return plain();
    };
    // shortcut: the add waits for the analysis, which blocks the caller for
    // as long as it takes; upgrade by landing the clip from `Shell::poll`
    // with its place drawn pending.
    let Ok(analysis) = cache.wait(MediaHash::from_bytes(media.0)) else {
        return plain();
    };
    let set_bpm = doc.settings().tempo.bpm_at(at).get();
    let warped = if short {
        warp_short(
            media,
            seconds,
            set_bpm,
            warping.prefs.loop_warp_short_samples,
            &analysis,
        )
    } else {
        AudioData::new(media)
            .auto_warped(&FileBeats {
                seconds,
                bpm: analysis.tempo.map(|tempo| tempo.bpm),
                beats: analysis.beats.clone(),
            })
            .ok()
    };
    let Some(audio) = warped else {
        return plain();
    };
    let audio = AudioData {
        warp_mode: warp_mode(warping.prefs.default_warp_mode),
        ..audio
    };
    sized(audio, seconds, doc, at)
}

/// A short sample's data under `setting` (R32, R34): warped as a loop of
/// whole bars with the loop on or off, or `None` to leave it unwarped.
fn warp_short(
    media: ContentHash,
    seconds: f64,
    set_bpm: f64,
    setting: LoopWarpShortSamples,
    analysis: &Analysis,
) -> Option<AudioData> {
    let steady = analysis.tempo.filter(|tempo| tempo.confidence >= STEADY);
    let looping = match setting {
        LoopWarpShortSamples::UnwarpedOneShot => unreachable!("an unwarped sample needs no map"),
        LoopWarpShortSamples::WarpedOneShot => false,
        LoopWarpShortSamples::WarpedLoop => true,
        LoopWarpShortSamples::Auto => {
            let fits = |bars: u32| {
                let wanted = loop_bpm(seconds, bars);
                steady.is_some_and(|tempo| (tempo.bpm - wanted).abs() <= TEMPO_FIT * wanted)
            };
            if !EVEN_LOOP_BARS.into_iter().any(fits) {
                return None;
            }
            true
        }
    };
    let bars = nearest_bars(
        &EVEN_LOOP_BARS,
        seconds,
        steady.map_or(set_bpm, |tempo| tempo.bpm),
    );
    AudioData::new(media)
        .warped_as_loop(0.0, seconds, bars, looping)
        .ok()
}

/// A recorded clip of the song's `start` to `end` warped so it plays where
/// it was recorded and at the speed it was played (R37): a marker at its
/// start and one at each tempo change inside it, and the tempo at its end
/// as the clip's. `audio` holds its Start and End markers and loop brace in
/// seconds in the file; they become ticks. A tempo ramp counts as a straight
/// segment between its ends.
#[must_use]
pub fn warp_recorded(
    audio: AudioData,
    tempo: &TempoMap,
    start: Ticks,
    end: Ticks,
    mode: WarpMode,
) -> AudioData {
    let from = tempo.ticks_to_seconds(start);
    let mut markers = vec![WarpMarker {
        time: audio.start,
        beat: Ticks::ZERO,
    }];
    markers.extend(
        tempo
            .segments()
            .map(bl_core::time::TempoSegment::start)
            .filter(|&at| start < at && at < end)
            .map(|at| WarpMarker {
                time: audio.start + tempo.ticks_to_seconds(at) - from,
                beat: at - start,
            }),
    );
    let bpm = tempo.bpm_at(end.saturating_sub(Ticks(1)).max(start)).get();
    audio
        .warped_by(markers, bpm)
        .map_or(audio, |warped| AudioData {
            warp_mode: mode,
            ..warped
        })
}

/// The warp decision for a preview (WP-4.18; spec: browser R19, R21).
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewWarp {
    /// Plays unwarped at its own tempo and doesn't loop.
    Unwarped,
    /// Warped audio settings to preview through, looping to the set's tempo.
    Warped(Box<AudioData>),
    /// Waiting for background analysis to finish.
    Pending,
}

/// What warp settings a preview of `path`, whose content is `media_hash`,
/// gets: saved defaults if present, otherwise the auto-warp settings for
/// short or long samples, or unwarped. Returns `Pending` if analysis is
/// needed and still running in the background. The file must be in the
/// analysis cache's media store, or its analysis fails.
#[must_use]
pub fn preview_warp(
    media_hash: MediaHash,
    path: &std::path::Path,
    seconds: f64,
    set_bpm: f64,
    warping: Warping<'_>,
) -> PreviewWarp {
    let media = ContentHash(*media_hash.as_bytes());
    if let Some(audio) = clip_defaults::load(path, media)
        .and_then(|defaults| AudioData::new(media).with_defaults(&defaults).ok())
    {
        if audio.warp && audio.warp_map().is_ok() {
            return PreviewWarp::Warped(Box::new(audio));
        }
        return PreviewWarp::Unwarped;
    }
    let short = seconds < SHORT_SAMPLE_SECONDS;
    let wanted = if short {
        warping.prefs.loop_warp_short_samples != LoopWarpShortSamples::UnwarpedOneShot
    } else {
        warping.prefs.auto_warp_long_samples
    };
    if !wanted {
        return PreviewWarp::Unwarped;
    }
    let Some(cache) = warping.analysis else {
        return PreviewWarp::Unwarped;
    };
    let analysis = match cache.request(media_hash) {
        bl_analysis::cache::AnalysisStatus::Pending => return PreviewWarp::Pending,
        bl_analysis::cache::AnalysisStatus::Ready(analysis) => analysis,
        bl_analysis::cache::AnalysisStatus::Failed(_) => return PreviewWarp::Unwarped,
    };
    let warped = if short {
        warp_short(
            media,
            seconds,
            set_bpm,
            warping.prefs.loop_warp_short_samples,
            &analysis,
        )
    } else {
        AudioData::new(media)
            .auto_warped(&FileBeats {
                seconds,
                bpm: analysis.tempo.map(|tempo| tempo.bpm),
                beats: analysis.beats.clone(),
            })
            .ok()
    };
    match warped {
        Some(audio) => {
            let audio = AudioData {
                warp_mode: warp_mode(warping.prefs.default_warp_mode),
                ..audio
            };
            PreviewWarp::Warped(Box::new(audio))
        }
        None => PreviewWarp::Unwarped,
    }
}
