//! The warp settings a new clip gets (WP-4.9, the warping spec R31 to R36):
//! short and long samples, what Auto decides, and the maps auto-warp makes.
//! Clips made from a file by a drop, Enter or an import go through
//! [`new_clip`].

use bl_analysis::cache::{Analysis, AnalysisCache};
use bl_core::time::{TempoMap, Ticks};
use bl_media::ContentHash as MediaHash;
use bl_model::{
    AudioData, ContentHash, Document, EVEN_LOOP_BARS, FileBeats, WarpMarker, loop_bpm, nearest_bars,
};
use bl_shell_spec::settings::{LoopWarpShortSamples, WarpPrefs};

use crate::browser::arrange::length_at;

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

/// The clip a file of `seconds` becomes when it is added at `at` (R33): the
/// warp settings its preferences ask for, waiting for the file's analysis
/// where they need it (R36), or a plain unwarped clip that plays as long as
/// the file does at the set's tempo.
#[must_use]
pub fn new_clip(
    media: ContentHash,
    seconds: f64,
    doc: &Document,
    at: Ticks,
    warping: Warping<'_>,
) -> NewClip {
    let plain = || NewClip {
        audio: AudioData::new(media),
        length: length_at(doc, at, seconds),
    };
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
    let Ok(map) = audio.warp_map() else {
        return plain();
    };
    let end = audio.end.unwrap_or_else(|| map.time_to_beat(seconds));
    NewClip {
        length: Ticks(((end - audio.start).round() as i64).max(1)),
        audio,
    }
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
pub fn warp_recorded(audio: AudioData, tempo: &TempoMap, start: Ticks, end: Ticks) -> AudioData {
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
    audio.warped_by(markers, bpm).unwrap_or(audio)
}
