//! Drum transcription (the audio-to-MIDI spec, R20 to R22): the generated
//! drum loop, which a different script made than the classifier's training
//! sounds, and the classifier's weights.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::common::{TempDir, drum_hits, drum_loop};
use bl_analysis::Audio;
use bl_analysis::drums::{PADS, drums};
use bl_analysis::transcribe::{Error, Grid};
use bl_media::decode_file;

const SIXTEENTH: f64 = 0.125;

/// The notes of the drum loop, with no grid.
fn notes() -> Vec<bl_analysis::transcribe::Note> {
    let dir = TempDir::new("drums-notes");
    let path = dir.path().join("drum-loop.wav");
    std::fs::write(&path, drum_loop()).unwrap();
    let data = decode_file(&path).unwrap();
    let samples: Vec<f32> = data.channels().next().unwrap().to_vec();
    let audio = Audio::new(&samples, 1, data.sample_rate()).unwrap();
    drums(&audio, SIXTEENTH, None, &mut |_| true).unwrap()
}

// Spec: audio-to-midi R20, R22. The loop (tests/fixtures/gen/gen_drums.py) has
// a kick on beats 1 and 3 and an eighth before beat 4, a snare on beats 2 and
// 4 and a hat between the beats: each is found on its pad, a sixteenth long.
// Its sounds are a third script's, not the classifier's training sounds, and
// a snare there also sounds a little like a hat to it, so one extra note is
// let through.
#[test]
fn the_drum_loops_hits_are_found_on_their_pads() {
    let hits = drum_hits();
    let want: [u8; 9] = [36, 42, 38, 42, 36, 42, 36, 38, 42];
    let notes = notes();
    for (hit, pad) in hits.iter().zip(want) {
        let note = notes
            .iter()
            .find(|n| n.pitch == pad && (n.start - hit).abs() < 0.001)
            .unwrap_or_else(|| panic!("no note {pad} at {hit}: {notes:?}"));
        assert!((note.end - note.start - SIXTEENTH).abs() < 1e-9);
        assert!((1..=127).contains(&note.velocity));
    }
    assert!(notes.len() <= hits.len() + 1, "{notes:?}");
    assert!(notes.iter().all(|n| PADS.contains(&n.pitch)));
}

// Spec: audio-to-midi R22. Onsets within 20 ms of a sixteenth's line that the
// classifier is sure of move onto it.
#[test]
fn a_confident_onset_near_a_line_moves_onto_it() {
    let dir = TempDir::new("drums-grid");
    let path = dir.path().join("drum-loop.wav");
    std::fs::write(&path, drum_loop()).unwrap();
    let data = decode_file(&path).unwrap();
    let samples: Vec<f32> = data.channels().next().unwrap().to_vec();
    let audio = Audio::new(&samples, 1, data.sample_rate()).unwrap();
    // A grid 10 ms late: every hit is within 20 ms of a line.
    let grid = Grid {
        period: SIXTEENTH,
        origin: 0.010,
    };
    let notes = drums(&audio, SIXTEENTH, Some(grid), &mut |_| true).unwrap();
    let moved = notes
        .iter()
        .filter(|n| ((n.start - 0.010) / SIXTEENTH).fract().abs() < 1e-9)
        .count();
    assert!(moved >= 6, "{moved} of {} moved: {notes:?}", notes.len());
    assert!(notes.iter().all(|n| n.confidence >= 0.5));
}

// Spec: audio-to-midi R20. Silence has no hits, and a progress callback that
// returns false stops the run.
#[test]
fn silence_has_no_hits_and_cancel_stops_the_run() {
    let silent = vec![0.0f32; 48_000];
    let audio = Audio::new(&silent, 1, 48_000).unwrap();
    assert!(
        drums(&audio, SIXTEENTH, None, &mut |_| true)
            .unwrap()
            .is_empty()
    );
    let dir = TempDir::new("drums-cancel");
    let path = dir.path().join("drum-loop.wav");
    std::fs::write(&path, drum_loop()).unwrap();
    let data = decode_file(&path).unwrap();
    let samples: Vec<f32> = data.channels().next().unwrap().to_vec();
    let audio = Audio::new(&samples, 1, data.sample_rate()).unwrap();
    let cancelled = drums(&audio, SIXTEENTH, None, &mut |_| false);
    // The loop has fewer than 32 onsets, so progress is never asked: the
    // run finishes. A longer one is asked, and stops.
    assert!(cancelled.is_ok());
    let long: Vec<f32> = (0..20).flat_map(|_| samples.iter().copied()).collect();
    let audio = Audio::new(&long, 1, data.sample_rate()).unwrap();
    assert_eq!(
        drums(&audio, SIXTEENTH, None, &mut |_| false),
        Err(Error::Cancelled)
    );
}

fn tool() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/drum-classifier")
}

// Spec: audio-to-midi R21. The weights built into the app are what the
// training script writes from its seed and the committed training set.
#[test]
fn the_script_reproduces_the_committed_weights() {
    let dir = TempDir::new("drums-train");
    let out = dir.path().join("weights.txt");
    let python = ["python3", "python"]
        .into_iter()
        .find(|name| {
            Command::new(name)
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
        })
        .expect("a Python 3 interpreter is needed to run the trainer");
    let status = Command::new(python)
        .arg("-I")
        .arg(tool().join("train.py"))
        .arg("--features")
        .arg(tool().join("training.txt"))
        .arg("--out")
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success(), "the trainer ran");
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        std::fs::read_to_string(tool().join("weights.txt")).unwrap(),
        "weights.txt is the script's output"
    );
}
