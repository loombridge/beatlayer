//! Audio to MIDI's background work (IMP-06): the transcription of a clip's
//! region, on a thread of its own. The shell starts it and lands its notes
//! (`Shell::run` for `audio_to_midi.*`); the behaviour is in the audio to
//! MIDI spec, and the analysis in [`bl_analysis::transcribe`] and
//! [`bl_analysis::drums`].
//!
//! Harmony and Melody need the packages of the `audio-to-midi` feature in
//! [`crate::packages`]'s manifest: ONNX Runtime's shared library and the
//! pitch model. Drums needs none.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use bl_analysis::Audio;
use bl_analysis::transcribe::{self, Error, Kind as Pitched, Note};
use bl_media::{ContentHash, MediaStore};
use bl_shell_spec::audio_to_midi::ConvertKind;

use crate::stems::{Ended, Progress, Worker};

/// The feature's ID in the manifest.
pub const FEATURE: &str = "audio-to-midi";

/// An onset this close to a transient the person placed moves to it, and a
/// held note crosses one by more than this to be split there (R8), in
/// seconds.
const HINT: f64 = 0.030;

/// Where the notes come from.
pub enum Backend {
    /// ONNX Runtime's library and the pitch model, from the Packages
    /// manager: Harmony and Melody.
    Network {
        /// ONNX Runtime's library.
        runtime: PathBuf,
        /// The model.
        model: PathBuf,
    },
    /// Drums' own classifier, which needs no package.
    Classifier,
    /// A stand-in for tests: these notes, in seconds from the region's
    /// start, whatever the audio holds.
    Notes(Vec<Note>),
}

/// What a conversion works on.
pub struct Job {
    /// Where the notes come from.
    pub backend: Backend,
    /// The set's media, which decodes the file.
    pub media: Arc<MediaStore>,
    /// The clip's file.
    pub source: ContentHash,
    /// The region of the file converted, in seconds (R4).
    pub region: (f64, f64),
    /// What to find.
    pub kind: ConvertKind,
    /// A sixteenth note at the set's tempo, in seconds: a drum note's
    /// length (R22).
    pub sixteenth: f64,
    /// Where the person's transient markers are, in seconds from the
    /// region's start (R8).
    pub hints: Vec<f64>,
}

/// Starts converting on a thread of its own. The notes it gives are in
/// seconds from the region's start, in start order.
///
/// # Errors
///
/// A message if the thread can't start.
pub fn convert(job: Job) -> Result<Worker<Vec<Note>>, String> {
    Worker::start("bl-shell-audio-to-midi", move |progress| {
        match run(&job, progress) {
            Ok(notes) => Ended::Done(notes),
            Err(Error::Cancelled) => Ended::Cancelled,
            Err(Error::Failed(error)) => Ended::Failed(error),
        }
    })
}

fn run(job: &Job, progress: &Progress) -> Result<Vec<Note>, Error> {
    progress.total.store(1000, Ordering::Relaxed);
    let mut report = |fraction: f32| {
        progress
            .done
            .store((f64::from(fraction) * 1000.0) as u64, Ordering::Relaxed);
        !progress.cancelled()
    };
    progress.set_stage("Reading the audio");
    let (samples, channels, rate) = region_samples(job)?;
    if progress.cancelled() {
        return Err(Error::Cancelled);
    }
    let mut notes = match (&job.backend, job.kind) {
        (Backend::Notes(notes), _) => notes.clone(),
        (Backend::Classifier | Backend::Network { .. }, ConvertKind::Drums) => {
            progress.set_stage("Finding the hits");
            let audio = Audio::new(&samples, channels, rate).map_err(Error::Failed)?;
            bl_analysis::drums::drums(&audio, job.sixteenth, None, &mut report)?
        }
        (Backend::Network { runtime, model }, kind) => {
            progress.set_stage("Loading the model");
            let mut network = bl_analysis::transcribe::onnx::BasicPitch::load(runtime, model)
                .map_err(Error::Failed)?;
            progress.set_stage("Finding the notes");
            let audio = Audio::new(&samples, channels, rate).map_err(Error::Failed)?;
            let pitched = match kind {
                ConvertKind::Melody => Pitched::Melody,
                _ => Pitched::Harmony,
            };
            transcribe::transcribe(&audio, &mut network, pitched, None, &mut report)?
        }
        (Backend::Classifier, _) => {
            return Err(Error::Failed(
                "Harmony and Melody need the pitch model".to_owned(),
            ));
        }
    };
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    let notes = with_hints(notes, &job.hints, job.kind != ConvertKind::Drums);
    progress.done.store(1000, Ordering::Relaxed);
    Ok(notes)
}

/// The region's samples, interleaved, with their channel count and rate.
fn region_samples(job: &Job) -> Result<(Vec<f32>, usize, u32), Error> {
    let failed = |message: String| Error::Failed(message);
    let audio = job
        .media
        .load(job.source)
        .map_err(|e| failed(format!("couldn't read the clip's file: {e}")))?;
    let rate = audio.sample_rate();
    let frames = audio.frames();
    let at = |seconds: f64| ((seconds * f64::from(rate)).round().max(0.0) as usize).min(frames);
    let (from, to) = (at(job.region.0), at(job.region.1));
    if to <= from {
        return Err(failed("the clip plays no audio to convert".to_owned()));
    }
    let channels = audio.channel_count().max(1);
    let mut samples = Vec::with_capacity((to - from) * channels);
    for frame in from..to {
        for channel in 0..channels {
            samples.push(audio.channel(channel).map_or(0.0, |c| c[frame]));
        }
    }
    Ok((samples, channels, rate))
}

/// `notes` with the person's transient markers applied (R8): an onset within
/// 30 ms of one of `hints` moves to it, and, when `split` is set (Harmony
/// and Melody), a note that crosses one by more than 30 ms on both sides is
/// split there into two of its pitch and velocity. No two notes of a pitch
/// overlap afterwards: the earlier ends where the later starts.
#[must_use]
pub fn with_hints(notes: Vec<Note>, hints: &[f64], split: bool) -> Vec<Note> {
    if hints.is_empty() {
        return notes;
    }
    let mut out = Vec::with_capacity(notes.len());
    for mut note in notes {
        let near = hints
            .iter()
            .copied()
            .filter(|hint| (hint - note.start).abs() <= HINT && *hint < note.end)
            .min_by(|a, b| (a - note.start).abs().total_cmp(&(b - note.start).abs()));
        if let Some(hint) = near {
            note.start = hint;
        }
        let mut cuts: Vec<f64> = if split {
            hints
                .iter()
                .copied()
                .filter(|hint| *hint - note.start > HINT && note.end - *hint > HINT)
                .collect()
        } else {
            Vec::new()
        };
        cuts.sort_by(f64::total_cmp);
        let mut start = note.start;
        for cut in cuts {
            out.push(Note {
                start,
                end: cut,
                ..note
            });
            start = cut;
        }
        out.push(Note { start, ..note });
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    // A pitch's notes follow one another: the earlier ends where the later
    // starts, and is dropped if that leaves it nothing.
    for i in 0..out.len() {
        if let Some(next) = out[i + 1..].iter().find(|n| n.pitch == out[i].pitch) {
            out[i].end = out[i].end.min(next.start);
        }
    }
    out.retain(|note| note.end > note.start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: f64, end: f64, pitch: u8) -> Note {
        Note {
            start,
            end,
            pitch,
            velocity: 100,
            confidence: 0.9,
        }
    }

    // Spec: audio-to-midi R8. An onset within 30 ms of a marker the person
    // placed moves to it; one further off stays.
    #[test]
    fn an_onset_within_30_ms_of_a_marker_moves_to_it() {
        let notes = vec![note(1.010, 1.5, 60), note(2.050, 2.5, 62)];
        let out = with_hints(notes, &[1.0, 2.0], true);
        assert_eq!(out[0].start, 1.0);
        assert_eq!(out[1].start, 2.050);
    }

    // Spec: audio-to-midi R8. A held note that crosses a marker by more
    // than 30 ms is split there, in Melody and Harmony; a drum hit is not.
    #[test]
    fn a_held_note_crossing_a_marker_is_split_there() {
        let held = vec![note(1.0, 2.0, 60)];
        let out = with_hints(held.clone(), &[1.5], true);
        assert_eq!(
            out.iter().map(|n| (n.start, n.end)).collect::<Vec<_>>(),
            [(1.0, 1.5), (1.5, 2.0)]
        );
        assert!(out.iter().all(|n| n.pitch == 60 && n.velocity == 100));
        // A marker 20 ms inside the note only moves the onset.
        assert_eq!(with_hints(held.clone(), &[1.02], true).len(), 1);
        // A drum note is never split.
        assert_eq!(with_hints(held, &[1.5], false).len(), 1);
    }

    // Spec: audio-to-midi R8. No hints leave the notes alone, and a moved
    // onset never overlaps the note before it.
    #[test]
    fn hints_change_nothing_without_markers_and_never_overlap() {
        let notes = vec![note(1.0, 2.0, 60)];
        assert_eq!(with_hints(notes.clone(), &[], true), notes);
        let notes = vec![note(1.0, 1.2, 60), note(1.21, 1.6, 60)];
        let out = with_hints(notes, &[1.0, 1.19], true);
        assert!(out.windows(2).all(|pair| pair[0].end <= pair[1].start));
        assert_eq!(out[1].start, 1.19);
    }
}
