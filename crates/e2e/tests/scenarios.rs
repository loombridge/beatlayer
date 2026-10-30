//! Whole workflows an agent does, end to end, with assertions on what they
//! sound like: a remix, a beat, a Session performance, a recording and a
//! mix. Each is one test, built through the agent API as an agent builds a
//! set, played on the hand-paced null device or exported offline, and each
//! ends with the set saved and reopened unchanged, and, where it is one
//! session of edits, undone step by step back to where it started.
//!
//! A step that finds Beatlayer at fault goes through the known-failures
//! ratchet (suite `scenarios`), keyed `<scenario>/<step>`, its property
//! what the step promises, so a scenario runs on past a listed bug and
//! fails on a new one.

mod beat;
mod mix;
mod recording;
mod remix;
mod session;

use bl_core::time::{Bpm, TempoMap};
use bl_e2e::audio::amplitude_db;
use bl_e2e::{Audio, Driver, Host, document_diff};
use bl_model::Document;
use serde_json::{Value, json};

/// The device's rate, which every Host runs at.
const RATE: f64 = 48_000.0;

/// A song tempo map of one tempo, as `song.set_tempo` takes it.
fn tempo(bpm: f64) -> Value {
    let bpm = Bpm::new(bpm).unwrap_or_else(|error| panic!("{bpm} BPM: {error}"));
    serde_json::to_value(TempoMap::constant(bpm)).expect("a tempo map serialises")
}

/// The frame `seconds` into a render at the device's rate.
fn frame(seconds: f64) -> usize {
    (seconds * RATE).round() as usize
}

/// The RMS level of the mono mix of `audio` from frame `from` to `to`, in
/// dBFS: -inf for silence or an empty span.
fn rms_db(audio: &Audio, from: usize, to: usize) -> f64 {
    let to = to.min(audio.frames());
    if from >= to {
        return f64::NEG_INFINITY;
    }
    let mono = audio.mono();
    let sum: f64 = mono[from..to].iter().map(|&s| f64::from(s).powi(2)).sum();
    amplitude_db((sum / (to - from) as f64).sqrt())
}

/// How strongly a sine of `hz` sounds in the mono mix of `audio` from frame
/// `from` to `to`: its peak amplitude in dBFS, through a Hann-windowed
/// Goertzel filter, so a full-scale sine reads about 0 dB and a tone a few
/// hertz away reads far lower.
fn tone_db(audio: &Audio, hz: f64, from: usize, to: usize) -> f64 {
    let to = to.min(audio.frames());
    if from + 2 >= to {
        return f64::NEG_INFINITY;
    }
    let mono = audio.mono();
    let n = to - from;
    let coeff = 2.0 * (std::f64::consts::TAU * hz / f64::from(audio.rate)).cos();
    let (mut s1, mut s2, mut weights) = (0.0f64, 0.0f64, 0.0f64);
    for (i, &sample) in mono[from..to].iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (n - 1) as f64).cos();
        weights += w;
        let s0 = f64::from(sample) * w + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0);
    amplitude_db(2.0 * power.sqrt() / weights)
}

/// The sharpest rise in level within `search` frames of `at`: the most the
/// 2 ms after a frame are louder than the 6 ms that end a quarter of a
/// millisecond before it, in dB, and that frame's distance from `at`. A
/// drum hit landing there rises well above zero, a few frames off.
fn onset(audio: &Audio, at: usize, search: usize) -> (f64, isize) {
    let mono = audio.mono();
    let level = |from: usize, to: usize| {
        let to = to.min(mono.len());
        if from >= to {
            return -120.0;
        }
        let sum: f64 = mono[from..to].iter().map(|&s| f64::from(s).powi(2)).sum();
        amplitude_db((sum / (to - from) as f64).sqrt()).max(-120.0)
    };
    let mut best = (f64::NEG_INFINITY, 0);
    for frame in (at.saturating_sub(search)..=at + search).step_by(12) {
        let rise =
            level(frame, frame + 96) - level(frame.saturating_sub(300), frame.saturating_sub(12));
        if rise > best.0 {
            best = (rise, frame as isize - at as isize);
        }
    }
    best
}

/// The level of `audio` every 10 ms, as linear RMS of its mono mix: its
/// rhythm, for [`correlation`].
fn envelope(audio: &Audio) -> Vec<f64> {
    audio
        .mono()
        .chunks(480)
        .map(|chunk| {
            (chunk.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / chunk.len() as f64).sqrt()
        })
        .collect()
}

/// The Pearson correlation of two sequences over their common length: 1
/// when one is the other scaled, about 0 when unrelated.
fn correlation(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    if n < 2 {
        return 0.0;
    }
    let mean = |x: &[f64]| x[..n].iter().sum::<f64>() / n as f64;
    let (ma, mb) = (mean(a), mean(b));
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (x, y) = (a[i] - ma, b[i] - mb);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    ab / (aa * bb).sqrt().max(f64::MIN_POSITIVE)
}

/// The level above about 1.5 kHz relative to the whole, from frame `from`
/// to `to`, in dB: four one-pole high-passes in a row, 24 dB an octave
/// below the corner. A low-pass filter closing lowers it steeply.
fn high_band_db(audio: &Audio, from: usize, to: usize) -> f64 {
    let to = to.min(audio.frames());
    let mut signal: Vec<f64> = audio.mono()[..to].iter().map(|&s| f64::from(s)).collect();
    let k = (-std::f64::consts::TAU * 1_500.0 / f64::from(audio.rate)).exp();
    for _ in 0..4 {
        let (mut previous_in, mut previous_out) = (0.0, 0.0);
        for sample in &mut signal {
            let out = k * (previous_out + *sample - previous_in);
            previous_in = *sample;
            previous_out = out;
            *sample = out;
        }
    }
    let whole = rms_db(audio, from, to);
    let sum: f64 = signal[from..to].iter().map(|s| s * s).sum();
    amplitude_db((sum / (to - from).max(1) as f64).sqrt()) - whole
}

/// `Ok` if `holds`, else `Err` with `detail`: a step's check, for the
/// ratchet.
fn ensure(holds: bool, detail: impl FnOnce() -> String) -> Result<(), String> {
    if holds { Ok(()) } else { Err(detail()) }
}

/// Undoes step by step, through `edit.undo`, until the document is `start`,
/// and gives how many steps that took; `Err` if undo runs out first, with
/// where the document still differs.
fn undo_to(host: &mut Host, start: &Document) -> Result<usize, String> {
    for steps in 0..2_000 {
        if host.document() == start {
            return Ok(steps);
        }
        if let Err(error) = host.run("edit.undo", json!({})) {
            let diff = document_diff(start, host.document(), 8);
            return Err(format!(
                "undo ran out after {steps} steps ({error}) and the document still differs \
                 from where the scenario started:\n{}",
                diff.join("\n")
            ));
        }
    }
    Err("two thousand undo steps never reached the start".to_owned())
}

/// Redoes `steps` steps through `edit.redo`, and checks the document is
/// `end` again.
fn redo_to(host: &mut Host, steps: usize, end: &Document) -> Result<(), String> {
    for step in 0..steps {
        host.run("edit.redo", json!({}))
            .map_err(|error| format!("redo {step} of {steps} failed: {error}"))?;
    }
    ensure(host.document() == end, || {
        format!(
            "redo of {steps} steps didn't bring the edits back:\n{}",
            document_diff(end, host.document(), 8).join("\n")
        )
    })
}

/// Saves the set into the host's folder, reopens it, and checks the
/// reopened document is the one saved.
fn save_and_reopen(host: &mut Host) -> Result<(), String> {
    let dir = host.dir().join("saved");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    host.save_to(&dir);
    let saved = host.document().clone();
    host.reopen()?;
    host.settle();
    let reopened = host.document().clone();
    host.check();
    ensure(saved == reopened, || {
        format!(
            "the reopened set differs from the saved one:\n{}",
            document_diff(&saved, &reopened, 8).join("\n")
        )
    })
}

/// A copy of the drum fixture `name` in the host's own folder, as a path
/// string: commands that write beside a file they're handed (a warped
/// clip's settings, a recording) never touch the repository's fixtures.
fn drum_copy(host: &Host, name: &str) -> String {
    let to = host.dir().join("fixtures").join(name);
    std::fs::create_dir_all(to.parent().expect("a folder")).expect("the fixtures folder");
    std::fs::copy(bl_e2e::fixture::drums(name), &to)
        .unwrap_or_else(|error| panic!("copying {name}: {error}"));
    to.to_string_lossy().into_owned()
}

/// The ID `$new` name `name` got in a `run` reply.
fn new_id(reply: &Value, name: &str) -> u64 {
    reply["ids"][name]
        .as_u64()
        .unwrap_or_else(|| panic!("no ID for `{name}` in {reply}"))
}

/// The ID of the default set's track called `name`.
fn track_named(driver: &mut impl Driver, name: &str) -> u64 {
    let read = driver.query(json!({ "path": format!("track \"{name}\"") }));
    read["id"]
        .as_u64()
        .unwrap_or_else(|| panic!("no track `{name}`: {read}"))
}
