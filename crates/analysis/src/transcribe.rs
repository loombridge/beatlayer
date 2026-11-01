//! Pitch transcription (IMP-06; the audio-to-MIDI spec, R13 to R19): the
//! notes of a region of audio, for Convert Harmony and Convert Melody.
//!
//! [`transcribe`] resamples the audio to 22,050 Hz mono and runs the pitch
//! network over windows of two seconds, each overlapping the next by about
//! a quarter, and joins the windows' onset and note posteriorgrams back with
//! triangular weights, as stem separation joins its windows. The network is
//! behind [`Network`], so the tests run the arithmetic on a stand-in;
//! [`onnx`] runs the real one, Spotify's basic-pitch, through ONNX Runtime.
//! [`notes`] turns posteriorgrams into notes: the note step of basic-pitch
//! (Bittner et al., 2022), then Melody's one-at-a-time rule (R14), then
//! IMP-06's clean-up (R15) and the velocities (R16).
//!
//! Everything here runs on whatever thread calls it, never an audio thread:
//! it allocates freely.
//!
//! The note step is ported from basic-pitch's `output_to_notes_polyphonic`
//! (<https://github.com/spotify/basic-pitch>, Copyright 2022 Spotify AB,
//! Apache License 2.0), without its inferred onsets and its "melodia trick",
//! which would start notes where the onset posteriorgram has no peak.

#[cfg(feature = "stems")]
pub mod onnx;

use bl_media::{SampleData, convert_sample_rate};

use crate::Audio;

/// The rate the network works at, in frames per second.
pub const RATE: u32 = 22_050;

/// Samples between the network's frames.
const HOP: usize = 256;

/// The network's window, in samples: two seconds less one hop.
pub const WINDOW: usize = 2 * RATE as usize - HOP;

/// Frames the network gives for a window.
pub const FRAMES: usize = 172;

/// The piano keys the network covers, from A0 (MIDI note 21) to C8.
pub const KEYS: usize = 88;

/// MIDI note number of the lowest key.
const LOWEST: u8 = 21;

/// Frames from one window's start to the next's: windows overlap by 43 of
/// their 172 frames, a quarter.
const STRIDE: usize = 129;

/// Frames of silence put before the audio, which the first window's own
/// edge would otherwise spoil.
const LEAD: usize = 15;

/// A key's onset activity must peak at least this high to start a note.
const ONSET_THRESHOLD: f32 = 0.5;

/// A key's note activity is "on" at or above this.
const NOTE_THRESHOLD: f32 = 0.3;

/// A note ends when its key's activity stays below [`NOTE_THRESHOLD`] for
/// this many frames (R13).
const ENERGY_TOLERANCE: usize = 11;

/// Notes of Melody starting this many frames apart or fewer are one attack.
const TOGETHER: usize = 2;

/// Notes of one pitch closer than this, in seconds, are one note (R15).
const MERGE_GAP: f64 = 0.030;

/// Notes shorter than this, in seconds, are dropped (R15).
const MIN_LENGTH: f64 = 0.040;

/// An onset snaps to the grid from this far or less, in seconds (R15).
const SNAP_WITHIN: f64 = 0.020;

/// Only an onset this confident snaps (R15).
const SNAP_CONFIDENCE: f32 = 0.8;

/// What the network gives for one window: onset and note posteriorgrams,
/// [`FRAMES`] frames of [`KEYS`] keys, one frame after another.
#[derive(Clone, Debug, PartialEq)]
pub struct Frames {
    /// How likely each key starts a note in each frame.
    pub onset: Vec<f32>,
    /// How likely each key sounds in each frame.
    pub note: Vec<f32>,
}

/// A pitch network.
pub trait Network {
    /// Runs one window of [`WINDOW`] samples at [`RATE`].
    ///
    /// # Errors
    ///
    /// A message if the network fails.
    fn run(&mut self, window: &[f32]) -> Result<Frames, String>;
}

/// What a run finds notes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Polyphonic audio: every note found is kept (R14).
    Harmony,
    /// Monophonic audio: one note at a time (R14).
    Melody,
}

/// A line of the sixteenth-note grid, in seconds of the audio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// Seconds between lines: a sixteenth note.
    pub period: f64,
    /// Where a line is.
    pub origin: f64,
}

impl Grid {
    fn nearest(self, time: f64) -> f64 {
        self.origin + ((time - self.origin) / self.period).round() * self.period
    }
}

/// A note found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    /// Seconds from the start of the audio.
    pub start: f64,
    /// Seconds from the start of the audio.
    pub end: f64,
    /// A MIDI note number, A0 to C8.
    pub pitch: u8,
    /// 1 to 127, from the onset's strength (R16).
    pub velocity: u8,
    /// How sure the network was of the onset, 0 to 1.
    pub confidence: f32,
}

/// Why a run stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The progress callback asked it to stop.
    Cancelled,
    /// The network failed, or the audio can't be transcribed.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("the transcription was cancelled"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// Onset and note posteriorgrams of a stretch of audio, `frames` frames of
/// [`KEYS`] keys, one frame after another, [`HOP`] samples at [`RATE`] apart.
#[derive(Clone, Debug, PartialEq)]
pub struct Posteriorgrams {
    /// How many frames there are.
    pub frames: usize,
    /// Onset activity.
    pub onset: Vec<f32>,
    /// Note activity.
    pub note: Vec<f32>,
}

/// The notes in `audio`: its mix of channels, resampled to [`RATE`], run
/// through `network` window by window, and turned into notes. `progress`
/// hears how far it is, from 0 to 1, after each window, and stops it by
/// returning `false`.
///
/// # Errors
///
/// [`Error::Cancelled`] if `progress` stopped it, or [`Error::Failed`] if
/// the audio can't be resampled or the network fails.
pub fn transcribe(
    audio: &Audio<'_>,
    network: &mut dyn Network,
    kind: Kind,
    grid: Option<Grid>,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Vec<Note>, Error> {
    let mono: Vec<f32> = audio.mono().iter().map(|&s| s as f32).collect();
    let failed = |e: bl_media::MediaError| Error::Failed(e.to_string());
    let data = SampleData::new(audio.rate(), vec![mono]).map_err(failed)?;
    let data = convert_sample_rate(&data, RATE).map_err(failed)?;
    let samples = data.channels().next().map_or(&[][..], |c| &c[..]);
    let grams = posteriorgrams(samples, network, progress)?;
    Ok(notes(&grams, kind, grid))
}

/// Runs `network` over `samples` at [`RATE`] and joins its windows.
fn posteriorgrams(
    samples: &[f32],
    network: &mut dyn Network,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Posteriorgrams, Error> {
    // The frames of the padded audio, and the frames of the original among
    // them: frame `g` is centred on sample `g * HOP` of the padded audio.
    let frames = samples.len() / HOP + 1;
    let padded = LEAD + frames;
    let windows = padded.saturating_sub(FRAMES).div_ceil(STRIDE) + 1;
    let mut onset = vec![0.0f32; padded * KEYS];
    let mut note = vec![0.0f32; padded * KEYS];
    let mut weights = vec![0.0f32; padded];
    let mut input = vec![0.0f32; WINDOW];
    let sample = |at: usize| {
        at.checked_sub(LEAD * HOP)
            .and_then(|at| samples.get(at))
            .copied()
            .unwrap_or(0.0)
    };
    for window in 0..windows {
        let first = window * STRIDE;
        for (i, slot) in input.iter_mut().enumerate() {
            *slot = sample(first * HOP + i);
        }
        let out = network.run(&input).map_err(Error::Failed)?;
        if out.onset.len() != FRAMES * KEYS || out.note.len() != FRAMES * KEYS {
            return Err(Error::Failed(format!(
                "the network gave {} and {} values for a window, not {}",
                out.onset.len(),
                out.note.len(),
                FRAMES * KEYS
            )));
        }
        for f in 0..FRAMES.min(padded - first) {
            // Triangular weights, never zero, so an edge frame no other
            // window covers still counts.
            let weight = (f + 1).min(FRAMES - f) as f32;
            weights[first + f] += weight;
            for k in 0..KEYS {
                onset[(first + f) * KEYS + k] += weight * out.onset[f * KEYS + k];
                note[(first + f) * KEYS + k] += weight * out.note[f * KEYS + k];
            }
        }
        if !progress((window + 1) as f32 / windows as f32) {
            return Err(Error::Cancelled);
        }
    }
    // R17: rounded to four decimal places, so the note step sees the same
    // numbers on every computer.
    let round = |x: f32| (x * 1e4).round() / 1e4;
    let join = |sums: &[f32]| -> Vec<f32> {
        sums[LEAD * KEYS..(LEAD + frames) * KEYS]
            .chunks_exact(KEYS)
            .zip(&weights[LEAD..])
            .flat_map(|(row, &w)| row.iter().map(move |&x| round(x / w)))
            .collect()
    };
    Ok(Posteriorgrams {
        frames,
        onset: join(&onset),
        note: join(&note),
    })
}

/// A note as the note step finds it, in frames, before the clean-up.
#[derive(Clone, Copy, Debug)]
struct Raw {
    start: usize,
    /// One past the last frame.
    end: usize,
    key: usize,
    strength: f32,
}

/// Seconds at a frame.
fn seconds(frame: usize) -> f64 {
    (frame * HOP) as f64 / f64::from(RATE)
}

/// The notes in `grams`: the note step, Melody's one-at-a-time rule for
/// [`Kind::Melody`], then the clean-up and the velocities, in start order.
#[must_use]
pub fn notes(grams: &Posteriorgrams, kind: Kind, grid: Option<Grid>) -> Vec<Note> {
    let mut raw = note_step(grams);
    if kind == Kind::Melody {
        raw = monophonic(&raw, grams);
    }
    let mut notes: Vec<Note> = raw
        .iter()
        .map(|n| Note {
            start: seconds(n.start),
            end: seconds(n.end),
            pitch: LOWEST + n.key as u8,
            velocity: 0,
            confidence: n.strength,
        })
        .collect();
    clean_up(&mut notes, grid);
    velocities(&mut notes);
    notes
}

/// R13: basic-pitch's note step. Onset peaks over the threshold, latest
/// first, each running until its key has been quiet for the tolerance; the
/// frames a note takes are used up, in its key and the two beside it, so an
/// earlier note stops where a later one starts.
fn note_step(grams: &Posteriorgrams) -> Vec<Raw> {
    let n = grams.frames;
    let mut remaining = grams.note.clone();
    let mut found = Vec::new();
    for start in (0..n.saturating_sub(1)).rev() {
        for key in 0..KEYS {
            let at = |f: usize| grams.onset[f * KEYS + key];
            let peak = at(start) >= ONSET_THRESHOLD
                && (start == 0 || at(start) > at(start - 1))
                && at(start) > at(start + 1);
            if !peak {
                continue;
            }
            let mut i = start + 1;
            let mut quiet = 0;
            while i < n - 1 && quiet < ENERGY_TOLERANCE {
                if remaining[i * KEYS + key] < NOTE_THRESHOLD {
                    quiet += 1;
                } else {
                    quiet = 0;
                }
                i += 1;
            }
            let end = i - quiet;
            for frame in start..end {
                for near in key.saturating_sub(1)..=(key + 1).min(KEYS - 1) {
                    remaining[frame * KEYS + near] = 0.0;
                }
            }
            found.push(Raw {
                start,
                end,
                key,
                strength: at(start),
            });
        }
    }
    found.sort_by_key(|r| (r.start, r.key));
    found
}

/// R14: of notes that start together, the one with the most activity over
/// its length is the melody's; a note ends where the next one starts, so a
/// sounding tail never hides a new note.
fn monophonic(raw: &[Raw], grams: &Posteriorgrams) -> Vec<Raw> {
    // Notes starting within a couple of frames of each other, on keys that
    // leak into one another, are one attack.
    let activity = |r: &Raw| {
        (r.start..r.end)
            .map(|f| grams.note[f * KEYS + r.key])
            .sum::<f32>()
    };
    let mut kept: Vec<Raw> = Vec::new();
    let mut attack = 0;
    for r in raw {
        match kept.last_mut() {
            Some(last) if r.start <= attack + TOGETHER => {
                if activity(r) > activity(last) {
                    *last = *r;
                }
            }
            _ => {
                attack = r.start;
                kept.push(*r);
            }
        }
    }
    for i in 1..kept.len() {
        kept[i - 1].end = kept[i - 1].end.min(kept[i].start);
    }
    kept
}

/// Orders notes by pitch, then start.
fn by_pitch(notes: &mut [Note]) {
    notes.sort_by(|a, b| a.pitch.cmp(&b.pitch).then(a.start.total_cmp(&b.start)));
}

/// R15: merge, drop, snap, trim overlaps, in that order.
fn clean_up(notes: &mut Vec<Note>, grid: Option<Grid>) {
    // Merge: per pitch, in time order, a note starting within the gap of
    // the last one's end joins it.
    by_pitch(notes);
    let mut merged: Vec<Note> = Vec::with_capacity(notes.len());
    for &note in notes.iter() {
        match merged.last_mut() {
            Some(last)
                if last.pitch == note.pitch
                    && note.start - last.end < MERGE_GAP
                    && note.confidence < SNAP_CONFIDENCE =>
            {
                last.end = last.end.max(note.end);
            }
            _ => merged.push(note),
        }
    }
    merged.retain(|n| n.end - n.start >= MIN_LENGTH);
    if let Some(grid) = grid {
        for note in &mut merged {
            let line = grid.nearest(note.start);
            if note.confidence >= SNAP_CONFIDENCE
                && (line - note.start).abs() <= SNAP_WITHIN
                && note.end - line >= MIN_LENGTH
            {
                note.start = line;
            }
        }
    }
    // Overlaps: per pitch, the earlier ends where the later starts. What
    // that or a snap leaves too short goes too.
    by_pitch(&mut merged);
    for i in 1..merged.len() {
        if merged[i - 1].pitch == merged[i].pitch {
            merged[i - 1].end = merged[i - 1].end.min(merged[i].start);
        }
    }
    merged.retain(|n| n.end - n.start >= MIN_LENGTH);
    merged.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    *notes = merged;
}

/// R16: 127 for the strongest onset, and 63 fewer for each factor of ten
/// weaker, never under 1.
fn velocities(notes: &mut [Note]) {
    let strongest = notes.iter().map(|n| n.confidence).fold(0.0f32, f32::max);
    for note in notes {
        let ratio = f64::from(note.confidence / strongest);
        note.velocity = (127.0 + 63.0 * ratio.log10()).round().clamp(1.0, 127.0) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Posteriorgrams of `frames` frames, empty.
    fn silence(frames: usize) -> Posteriorgrams {
        Posteriorgrams {
            frames,
            onset: vec![0.0; frames * KEYS],
            note: vec![0.0; frames * KEYS],
        }
    }

    /// A note on `key` from frame `start` for `length` frames, its onset
    /// `strength` at its first frame.
    fn put(g: &mut Posteriorgrams, key: usize, start: usize, length: usize, strength: f32) {
        g.onset[start * KEYS + key] = strength;
        for f in start..start + length {
            g.note[f * KEYS + key] = 0.9;
        }
    }

    fn pitches(notes: &[Note]) -> Vec<u8> {
        notes.iter().map(|n| n.pitch).collect()
    }

    // Spec: audio-to-midi R13. A note runs from its onset peak until its key
    // has been quiet for 11 frames, and ends at its last loud frame; one
    // under the onset threshold never starts.
    #[test]
    fn a_note_runs_from_its_onset_to_where_its_key_falls_quiet() {
        let mut g = silence(200);
        put(&mut g, 39, 10, 40, 0.9);
        put(&mut g, 50, 20, 40, 0.4);
        let found = note_step(&g);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].start, found[0].end, found[0].key), (10, 50, 39));
    }

    // Spec: audio-to-midi R13. The earlier of two onsets of one key ends
    // where the later starts.
    #[test]
    fn an_earlier_note_stops_where_a_later_one_of_its_key_starts() {
        let mut g = silence(200);
        put(&mut g, 39, 10, 80, 0.9);
        g.onset[60 * KEYS + 39] = 0.9;
        let found = note_step(&g);
        let ends: Vec<_> = found.iter().map(|r| (r.start, r.end)).collect();
        assert_eq!(ends, [(10, 60), (60, 90)]);
    }

    // Spec: audio-to-midi R14. Harmony keeps every note, at once.
    #[test]
    fn harmony_keeps_a_chord() {
        let mut g = silence(200);
        for key in [39, 43, 46] {
            put(&mut g, key, 10, 60, 0.9);
        }
        let found = notes(&g, Kind::Harmony, None);
        assert_eq!(pitches(&found), [60, 64, 67]);
    }

    // Spec: audio-to-midi R14. A later note takes over from its start, even
    // under a louder tail; the earlier one ends there.
    #[test]
    fn melody_ends_a_note_where_the_next_starts_whatever_the_tail_says() {
        let mut g = silence(200);
        put(&mut g, 39, 10, 60, 0.9);
        put(&mut g, 41, 40, 60, 0.9);
        for f in 40..70 {
            g.note[f * KEYS + 39] = 0.99;
        }
        let found = notes(&g, Kind::Melody, None);
        assert_eq!(pitches(&found), [60, 62]);
        assert!((found[0].end - found[1].start).abs() < 1e-9);
        assert!(found[0].end < seconds(70));
    }

    // Spec: audio-to-midi R14. Of notes that start together, the one with
    // the most activity over its length is the melody's: an overtone that
    // dies sooner is dropped.
    #[test]
    fn melody_keeps_the_louder_of_notes_that_start_together() {
        let mut g = silence(200);
        put(&mut g, 39, 10, 60, 0.9);
        put(&mut g, 51, 11, 6, 0.9);
        assert_eq!(pitches(&notes(&g, Kind::Harmony, None)), [60, 72]);
        assert_eq!(pitches(&notes(&g, Kind::Melody, None)), [60]);
    }

    // Spec: audio-to-midi R15. A fragment of one pitch starting within 30 ms
    // of the last one's end, its onset not confident, is one note with it; a
    // confident onset starts a note of its own, and so does a gap that wide.
    #[test]
    fn fragments_of_one_pitch_merge_across_a_gap_under_30_ms() {
        let mut close = silence(300);
        put(&mut close, 39, 10, 40, 0.9);
        put(&mut close, 39, 52, 40, 0.6);
        let found = notes(&close, Kind::Harmony, None);
        assert_eq!(found.len(), 1);
        assert!((found[0].end - seconds(92)).abs() < 1e-9);
        let mut confident = silence(300);
        put(&mut confident, 39, 10, 40, 0.9);
        put(&mut confident, 39, 52, 40, 0.9);
        assert_eq!(notes(&confident, Kind::Harmony, None).len(), 2);
        let mut apart = silence(300);
        put(&mut apart, 39, 10, 40, 0.9);
        put(&mut apart, 39, 56, 40, 0.6);
        assert_eq!(notes(&apart, Kind::Harmony, None).len(), 2);
    }

    // Spec: audio-to-midi R15. A note under 40 ms is dropped.
    #[test]
    fn a_note_under_40_ms_is_dropped() {
        let mut g = silence(200);
        put(&mut g, 39, 10, 3, 0.9);
        put(&mut g, 43, 10, 4, 0.9);
        let found = notes(&g, Kind::Harmony, None);
        assert_eq!(pitches(&found), [64]);
        assert!(found[0].end - found[0].start >= MIN_LENGTH);
    }

    // Spec: audio-to-midi R15. A confident onset within 20 ms of a
    // sixteenth moves onto it; a doubtful one, or one further away, stays.
    #[test]
    fn a_confident_onset_near_the_grid_snaps_to_it() {
        let grid = Grid {
            period: 0.125,
            origin: 0.0,
        };
        // Frame 11 is 127.9 ms: 3 ms from a line. Frame 13 is 150.9 ms:
        // 25 ms from one.
        let mut g = silence(200);
        put(&mut g, 39, 11, 40, 0.9);
        put(&mut g, 41, 13, 40, 0.9);
        put(&mut g, 43, 11, 40, 0.7);
        let found = notes(&g, Kind::Harmony, Some(grid));
        let start = |p| found.iter().find(|n| n.pitch == p).unwrap().start;
        assert!((start(60) - 0.125).abs() < 1e-9);
        assert!((start(62) - seconds(13)).abs() < 1e-9);
        assert!((start(64) - seconds(11)).abs() < 1e-9);
    }

    // Spec: audio-to-midi R15, and the brief's acceptance: no note shorter
    // than 40 ms, none overlapping another of its pitch, whatever the
    // posteriorgram holds.
    #[test]
    fn no_note_is_short_or_overlaps_one_of_its_pitch() {
        let mut g = silence(400);
        let mut seed = 7u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed >> 8
        };
        for _ in 0..120 {
            let key = (next() % 12) as usize + 30;
            let start = (next() % 360) as usize;
            let length = (next() % 40) as usize + 1;
            put(
                &mut g,
                key,
                start,
                length,
                0.5 + (next() % 50) as f32 / 100.0,
            );
        }
        let grid = Grid {
            period: 0.1,
            origin: 0.013,
        };
        for kind in [Kind::Harmony, Kind::Melody] {
            let found = notes(&g, kind, Some(grid));
            assert!(!found.is_empty());
            assert!(found.iter().all(|n| n.end - n.start >= MIN_LENGTH));
            for pitch in 51..=70 {
                let mut of: Vec<_> = found.iter().filter(|n| n.pitch == pitch).collect();
                of.sort_by(|a, b| a.start.total_cmp(&b.start));
                assert!(of.windows(2).all(|w| w[0].end <= w[1].start));
            }
        }
    }

    // Spec: audio-to-midi R16. The strongest onset is 127 and one a tenth
    // as strong is about 64.
    #[test]
    fn velocity_follows_the_onsets_strength_by_a_log_law() {
        let note = |confidence| Note {
            start: 0.0,
            end: 1.0,
            pitch: 60,
            velocity: 0,
            confidence,
        };
        let mut found = [note(1.0), note(0.1), note(0.5), note(0.001)];
        velocities(&mut found);
        let v: Vec<_> = found.iter().map(|n| n.velocity).collect();
        assert_eq!(v, [127, 64, 108, 1]);
    }

    /// A network that hears a steady tone as one key sounding from its
    /// first window: onset in the first frame, note activity throughout.
    struct Stand {
        runs: usize,
        key: usize,
    }

    impl Network for Stand {
        fn run(&mut self, window: &[f32]) -> Result<Frames, String> {
            assert_eq!(window.len(), WINDOW);
            let loud = window.iter().any(|&s| s != 0.0);
            let mut out = Frames {
                onset: vec![0.0; FRAMES * KEYS],
                note: vec![0.0; FRAMES * KEYS],
            };
            for f in 0..FRAMES {
                out.note[f * KEYS + self.key] = if loud { 0.9 } else { 0.0 };
            }
            if self.runs == 0 {
                out.onset[20 * KEYS + self.key] = 0.9;
            }
            self.runs += 1;
            Ok(out)
        }
    }

    fn tone(seconds: f64) -> Vec<f32> {
        (0..(seconds * 44_100.0) as usize)
            .map(|i| (i as f32 * 0.05).sin() * 0.5)
            .collect()
    }

    // Spec: audio-to-midi R13, R17. Windows join into one note, and the
    // same audio gives the same notes on a second run.
    #[test]
    fn windows_join_and_a_repeat_run_is_identical() {
        let samples = tone(7.0);
        let audio = Audio::new(&samples, 1, 44_100).unwrap();
        let run = || {
            let mut net = Stand { runs: 0, key: 39 };
            transcribe(&audio, &mut net, Kind::Melody, None, &mut |_| true).unwrap()
        };
        let first = run();
        assert_eq!(pitches(&first), [60]);
        // The onset is at frame 20 of the first window, 15 frames of lead
        // in: 5 frames, 58 ms, into the audio.
        assert!((first[0].start - seconds(5)).abs() < 1e-9);
        assert!(first[0].end > 6.5);
        assert_eq!(first, run());
    }

    // Spec: audio-to-midi R13. The windows cover the audio with a quarter
    // overlap: a 7 s clip takes 5.
    #[test]
    fn windows_overlap_by_a_quarter() {
        let samples = tone(7.0);
        let audio = Audio::new(&samples, 1, 44_100).unwrap();
        let mut net = Stand { runs: 0, key: 39 };
        let mut shares = Vec::new();
        transcribe(&audio, &mut net, Kind::Harmony, None, &mut |share| {
            shares.push(share);
            true
        })
        .unwrap();
        assert_eq!(net.runs, 5);
        assert_eq!(shares.last(), Some(&1.0));
    }

    // Spec: audio-to-midi R19 and the brief's acceptance: a cancelled run
    // returns nothing, and silence returns no notes.
    #[test]
    fn a_cancelled_run_returns_nothing_and_silence_finds_none() {
        let samples = tone(7.0);
        let audio = Audio::new(&samples, 1, 44_100).unwrap();
        let mut net = Stand { runs: 0, key: 39 };
        let stopped = transcribe(&audio, &mut net, Kind::Harmony, None, &mut |_| false);
        assert_eq!(stopped, Err(Error::Cancelled));
        assert_eq!(net.runs, 1);
        let quiet = vec![0.0f32; 44_100];
        let audio = Audio::new(&quiet, 1, 44_100).unwrap();
        let mut net = Stand { runs: 0, key: 39 };
        let found = transcribe(&audio, &mut net, Kind::Harmony, None, &mut |_| true);
        assert_eq!(found, Ok(Vec::new()));
    }
}
