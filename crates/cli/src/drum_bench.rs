//! Drums in `bl-cli transcribe-bench`, and the training set of the drum
//! classifier (the audio-to-MIDI spec, R21, R23 and R24).
//!
//! A break is two bars of sixteenths with a kick, a snare and a hi-hat
//! pattern, each hit on a note of its own track's Drum Sampler, which plays a
//! generated sound (a kit) whose pitch, length and level are drawn from the
//! break's seed. The bench renders breaks in three variants (clean, with a
//! room, with noise), converts each with `bl_analysis::drums` and scores each
//! class by onsets within 50 ms. The same generator, from seeds the bench
//! never uses, makes the classifier's training set: [`training_set`] writes
//! each onset's features and the classes that sounded there, which
//! `tools/drum-classifier/train.py` fits.

use std::f64::consts::TAU;
use std::sync::Arc;

use bl_analysis::Audio;
use bl_analysis::drums::{self, FEATURES, PADS};
use bl_analysis::transcribe::Grid;
use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_devices::drum_sampler as strike;
use bl_engine::MediaMap;
use bl_model::{Clip, ClipData, Command, Editor, MidiData, Note, TrackKind};

use crate::Outcome;
use crate::render::{Cue, Settings, interleave, render_with};
use crate::transcribe_bench::{
    Instrument, RATE, Reference, Rng, SIXTEENTH, Tally, Variant, apply, harmony, media, noisy,
    play, room, row,
};

/// The onset F-measure each class has to reach in each variant (R23).
const TARGET: f64 = 0.90;

/// The class names, in the order of [`PADS`].
const NAMES: [&str; 3] = ["Kick", "Snare", "Hi-hat"];

/// Sixteenths in a break.
const STEPS: u32 = 32;

/// A hit within this many seconds of an onset is the onset's (training
/// labels).
const LABEL_WITHIN: f64 = 0.03;

/// How far below the drums the backing of R24's mixed row is, in dB.
const BACKING_DB: f64 = -6.0;

/// The training breaks' seeds start at the training seed times this, far
/// from the bench's, which start at its seed times 1,000.
const TRAINING_STRIDE: u64 = 1_000_000_000;

/// A hit of a break: class, sixteenth, velocity.
#[derive(Clone, Copy)]
struct Strike {
    class: usize,
    step: u32,
    velocity: u8,
}

/// The sound a track's Drum Sampler plays, and how it plays it.
struct Sound {
    sample: Vec<f32>,
    transpose: f64,
    hold: f64,
    decay: f64,
}

/// A uniform draw from `lo` to `hi`.
fn between(rng: &mut Rng, lo: f64, hi: f64) -> f64 {
    lo + (hi - lo) * (rng.signed() as f64 * 0.5 + 0.5)
}

/// A generated drum sound of `class` at 44.1 kHz, peaking at a drawn level.
fn sound(class: usize, rng: &mut Rng) -> Sound {
    let rate = 44_100.0;
    let (seconds, hold, decay) = match class {
        0 => (0.45, between(rng, 40.0, 200.0), between(rng, 30.0, 150.0)),
        1 => (0.30, between(rng, 30.0, 150.0), between(rng, 20.0, 120.0)),
        _ => (0.15, between(rng, 5.0, 40.0), between(rng, 15.0, 80.0)),
    };
    let (f_end, glide, tau_body, click) = (
        between(rng, 42.0, 70.0),
        between(rng, 0.015, 0.045),
        between(rng, 0.07, 0.17),
        between(rng, 0.15, 0.6),
    );
    let (tone_hz, tone, tau_noise, tilt) = (
        between(rng, 150.0, 260.0),
        between(rng, 0.2, 0.5),
        between(rng, 0.03, 0.09),
        between(rng, 0.0, 0.5),
    );
    let (tau_hat, ring) = (between(rng, 0.006, 0.02), rng.below(2) == 0);
    let (sweep, phase0) = (between(rng, 70.0, 160.0), between(rng, 0.0, TAU));
    let mut phase = 0.0;
    let mut previous = 0.0;
    let mut sample: Vec<f32> = (0..(seconds * rate) as usize)
        .map(|n| {
            let t = n as f64 / rate;
            let white = f64::from(rng.signed());
            let out = match class {
                0 => {
                    phase += TAU * (f_end + sweep * (-t / glide).exp()) / rate;
                    0.8 * (phase + phase0).sin() * (-t / tau_body).exp()
                        + click * white * (-t / 0.001).exp()
                }
                1 => {
                    let noise = white - tilt * previous;
                    previous = white;
                    0.5 * noise * (-t / tau_noise).exp()
                        + tone * (TAU * tone_hz * t).sin() * (-t / (tau_noise * 0.8)).exp()
                        + 0.5 * white * (-t / 0.001).exp()
                }
                _ => {
                    let noise = white - previous;
                    previous = white;
                    let metal = if ring {
                        0.3 * ((TAU * 7_300.0 * t).sin() + (TAU * 9_100.0 * t).sin())
                    } else {
                        0.0
                    };
                    (0.35 * noise + metal) * (-t / tau_hat).exp() + 0.5 * white * (-t / 0.001).exp()
                }
            };
            out as f32
        })
        .collect();
    let peak = sample.iter().fold(0.0f32, |a, s| a.max(s.abs())).max(1e-6);
    let level = between(rng, 0.6, 0.95) as f32;
    sample.iter_mut().for_each(|s| *s *= level / peak);
    Sound {
        sample,
        transpose: (between(rng, -4.0, 4.0)).round(),
        hold,
        decay,
    }
}

/// A break: a kick, snare and hi-hat pattern with its own style.
fn pattern(rng: &mut Rng) -> Vec<Strike> {
    let mut strikes = Vec::new();
    let chance = |rng: &mut Rng, percent: u64| rng.below(100) < percent;
    // The hi-hat's style: eighths, sixteenths, offbeats or quarters.
    let style = [0, 0, 0, 0, 1, 1, 1, 2, 2, 3, 3][rng.below(11) as usize];
    for step in 0..STEPS {
        let beat = step % 4 == 0;
        let backbeat = step % 8 == 4;
        let velocity = |rng: &mut Rng, lo: u64, hi: u64| (lo + rng.below(hi - lo + 1)) as u8;
        if (step % 16 == 0 && chance(rng, 90))
            || (!beat && chance(rng, 14))
            || (step % 8 == 0 && chance(rng, 40))
            || (backbeat && chance(rng, 12))
        {
            strikes.push(Strike {
                class: 0,
                step,
                velocity: velocity(rng, 80, 127),
            });
        }
        if backbeat && chance(rng, 90) {
            strikes.push(Strike {
                class: 1,
                step,
                velocity: velocity(rng, 70, 127),
            });
        } else if !backbeat && chance(rng, 7) {
            strikes.push(Strike {
                class: 1,
                step,
                velocity: velocity(rng, 30, 60),
            });
        }
        let hat = match style {
            0 => step % 2 == 0,
            1 => true,
            2 => step % 4 == 2,
            _ => step % 4 == 0,
        };
        if hat && chance(rng, if style == 1 { 85 } else { 95 }) {
            strikes.push(Strike {
                class: 2,
                step,
                velocity: velocity(rng, 60, 110),
            });
        }
    }
    strikes
}

/// A break's reference notes of `class`.
fn reference(strikes: &[Strike], class: usize) -> Vec<Reference> {
    strikes
        .iter()
        .filter(|s| s.class == class)
        .map(|s| Reference {
            start: f64::from(s.step) * SIXTEENTH,
            end: f64::from(s.step + 1) * SIXTEENTH,
            pitch: PADS[class],
        })
        .collect()
}

/// `strikes` played by a Drum Sampler of `kit` on each class's own track:
/// a mono mix at [`RATE`].
fn render(strikes: &[Strike], kit: &[Sound; 3]) -> Result<Vec<f32>, String> {
    let mut editor = Editor::new();
    let mut files = MediaMap::new();
    let tempo = TempoMap::constant(Bpm::new(120.0).map_err(|e| e.to_string())?);
    apply(&mut editor, Command::SetTempo { tempo })?;
    let sixteenth = Ticks::QUARTER.0 / 4;
    for (class, sound) in kit.iter().enumerate() {
        let notes: Vec<Note> = strikes
            .iter()
            .filter(|s| s.class == class)
            .map(|s| {
                Note::new(
                    60,
                    Ticks(i64::from(s.step) * sixteenth),
                    Ticks(sixteenth),
                    s.velocity,
                )
            })
            .collect();
        if notes.is_empty() {
            continue;
        }
        let track = editor.new_id();
        let create = Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: NAMES[class].to_owned(),
            parent: None,
            index: class,
        };
        apply(&mut editor, create)?;
        let id = editor.new_id();
        let mut device = strike::drum_sampler_type().device(id);
        for (param, value) in [
            (strike::HOLD_INF, 0.0),
            (strike::HOLD, sound.hold),
            (strike::DECAY, sound.decay),
            (strike::TRANSPOSE, sound.transpose),
        ] {
            device.params.insert(param, value);
        }
        apply(
            &mut editor,
            Command::InsertDevice {
                track,
                index: 0,
                device,
            },
        )?;
        let hash = media(&sound.sample, &mut files)?;
        let sample = Command::SetDeviceSample {
            device: id,
            media: Some(hash),
            slot: 0,
        };
        apply(&mut editor, sample)?;
        let clip = editor.new_id();
        let create = Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(i64::from(STEPS + 4) * sixteenth),
                ClipData::Midi(MidiData::new(notes)),
            ),
        };
        apply(&mut editor, create)?;
    }
    let settings = Settings {
        sample_rate: RATE,
        block: 512,
        frames: RATE
            .seconds_to_frames(f64::from(STEPS) * SIXTEENTH + 1.5)
            .get() as u64,
    };
    let mut samples = Vec::new();
    let summary = render_with(
        editor.document(),
        Arc::new(files),
        &[],
        &[Cue::PLAY],
        &settings,
        |output| interleave(output, &mut samples),
    );
    if summary.non_finite > 0 || summary.peak == 0.0 {
        return Err("the render is silent or has samples that aren't finite".to_owned());
    }
    let channels = summary.channels;
    Ok(samples
        .chunks_exact(channels)
        .map(|frame| (frame[0] + frame[1]) * 0.5)
        .collect())
}

/// A break from `seed`, rendered clean.
fn make(seed: u64) -> Result<(Vec<Strike>, Vec<f32>), String> {
    let mut rng = Rng::new(seed);
    let kit = [sound(0, &mut rng), sound(1, &mut rng), sound(2, &mut rng)];
    let strikes = pattern(&mut rng);
    let audio = render(&strikes, &kit)?;
    Ok((strikes, audio))
}

/// `clean` as `variant` makes it, with the generator of break `i`.
fn vary(clean: Vec<f32>, variant: Variant, seed: u64, i: usize) -> Vec<f32> {
    let mut rng = Rng::new(seed * 7_919 + i as u64);
    match variant {
        Variant::Clean => clean,
        Variant::Room => room(&clean, &mut rng),
        Variant::Noise => noisy(&clean, &mut rng),
    }
}

/// The drum notes of `samples`, as references.
fn transcribe(samples: &[f32]) -> Result<Vec<Reference>, String> {
    let audio = Audio::new(samples, 1, RATE.hz())?;
    let grid = Grid {
        period: SIXTEENTH,
        origin: 0.0,
    };
    let notes =
        drums::drums(&audio, SIXTEENTH, Some(grid), &mut |_| true).map_err(|e| e.to_string())?;
    Ok(notes
        .iter()
        .map(|n| Reference {
            start: n.start,
            end: n.end,
            pitch: n.pitch,
        })
        .collect())
}

/// Scores `found` against `strikes` into each class's tally.
fn score(tallies: &mut [Tally; 3], strikes: &[Strike], found: &[Reference]) {
    for (class, tally) in tallies.iter_mut().enumerate() {
        let estimated: Vec<Reference> = found
            .iter()
            .filter(|n| n.pitch == PADS[class])
            .copied()
            .collect();
        tally.add(&reference(strikes, class), &estimated);
    }
}

fn rms(signal: &[f32]) -> f32 {
    (signal.iter().map(|s| s * s).sum::<f32>() / signal.len() as f32).sqrt()
}

/// Runs the Drums rows of the bench: each variant against its target, then
/// the mixed breaks without one (R24). Returns whether every target was met
/// and a repeat run gave the same notes.
pub(crate) fn bench(phrases: usize, seed: u64) -> Result<bool, String> {
    let mut passed = true;
    let mut identical = true;
    let breaks = (0..phrases)
        .map(|i| make(seed * 1_000 + i as u64))
        .collect::<Result<Vec<_>, _>>()?;
    for variant in Variant::ALL {
        let mut tallies: [Tally; 3] = Default::default();
        for (i, (strikes, clean)) in breaks.iter().enumerate() {
            let heard = vary(clean.clone(), variant, seed, i);
            let found = transcribe(&heard)?;
            // R17: the first break of each row is converted twice.
            if i == 0 {
                identical &= found == transcribe(&heard)?;
            }
            score(&mut tallies, strikes, &found);
        }
        for (name, tally) in NAMES.iter().zip(&tallies) {
            passed &= row(name, variant.name(), tally, Some(TARGET));
        }
    }
    let mut tallies: [Tally; 3] = Default::default();
    for (i, (strikes, clean)) in breaks.iter().enumerate() {
        let mut rng = Rng::new(seed * 104_729 + i as u64);
        let backing = play(&harmony(&mut rng, true), Instrument::Drift(2.0))?;
        let gain = 10f32.powf(BACKING_DB as f32 / 20.0) * rms(clean) / rms(&backing);
        let mix: Vec<f32> = clean
            .iter()
            .zip(backing.iter().chain(std::iter::repeat(&0.0)))
            .map(|(d, b)| d + gain * b)
            .collect();
        score(&mut tallies, strikes, &transcribe(&mix)?);
    }
    for (name, tally) in NAMES.iter().zip(&tallies) {
        row(name, "mix", tally, None);
    }
    println!(
        "a repeat run gives {} drum notes",
        if identical { "identical" } else { "DIFFERENT" }
    );
    Ok(passed && identical)
}

/// The drums of one file of the open labelled set, scored with no target:
/// `reference` is the notes of its `.txt`, of which the pads' are kept.
pub(crate) fn corpus_row(files: &[(Vec<f32>, usize, u32, Vec<Reference>)]) -> Result<(), String> {
    let mut tally = Tally::default();
    for (samples, channels, rate, notes) in files {
        let audio = Audio::new(samples, *channels, *rate)?;
        let found =
            drums::drums(&audio, SIXTEENTH, None, &mut |_| true).map_err(|e| e.to_string())?;
        let found: Vec<Reference> = found
            .iter()
            .map(|n| Reference {
                start: n.start,
                end: n.end,
                pitch: n.pitch,
            })
            .collect();
        let kept: Vec<Reference> = notes
            .iter()
            .filter(|n| PADS.contains(&n.pitch))
            .copied()
            .collect();
        tally.add(&kept, &found);
    }
    row("Drums", "corpus", &tally, None);
    Ok(())
}

/// The arguments of `bl-cli drum-training-set`.
#[derive(Debug, clap::Args)]
pub struct TrainingArgs {
    /// Breaks to render; each is converted clean, with a room and with noise.
    #[arg(long, default_value_t = 60)]
    pub breaks: usize,

    /// The seed the breaks' own derive from.
    #[arg(long, default_value_t = 1)]
    pub seed: u64,
}

/// Writes the classifier's training set to `out`: a line for each onset
/// found, whose first three numbers are 1 or 0 for a kick, a snare and a
/// hi-hat sounding there (a hit within 30 ms), then its [`FEATURES`]
/// features.
///
/// # Errors
///
/// A message if a break can't be rendered or `out` can't be written.
pub fn write_training_set(
    out: &mut impl std::io::Write,
    args: &TrainingArgs,
) -> Result<(), String> {
    let base = args.seed * TRAINING_STRIDE;
    for i in 0..args.breaks {
        let (strikes, clean) = make(base + i as u64)?;
        for variant in Variant::ALL {
            let heard = vary(clean.clone(), variant, base, i);
            let mono: Vec<f64> = heard.iter().map(|&s| f64::from(s)).collect();
            let onsets =
                drums::onsets(&mono, RATE.hz(), &mut |_| true).map_err(|e| e.to_string())?;
            for onset in onsets {
                let mut line: Vec<String> = (0..3)
                    .map(|class| {
                        let heard = reference(&strikes, class)
                            .iter()
                            .any(|r| (r.start - onset.time).abs() <= LABEL_WITHIN);
                        u8::from(heard).to_string()
                    })
                    .collect();
                line.extend(onset.features.iter().map(|f| format!("{f:.2}")));
                debug_assert_eq!(line.len(), 3 + FEATURES);
                writeln!(out, "{}", line.join(" ")).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// Runs `bl-cli drum-training-set`: [`write_training_set`] to standard
/// output.
///
/// # Errors
///
/// As [`write_training_set`].
pub fn training_set(args: &TrainingArgs) -> Outcome {
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    write_training_set(&mut out, args)?;
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: audio-to-midi R23. A seed gives the same break, kit and render.
    #[test]
    fn a_seed_gives_the_same_break_and_the_same_audio() {
        let (a, audio_a) = make(5).unwrap();
        let (b, audio_b) = make(5).unwrap();
        assert_eq!(
            a.iter()
                .map(|s| (s.class, s.step, s.velocity))
                .collect::<Vec<_>>(),
            b.iter()
                .map(|s| (s.class, s.step, s.velocity))
                .collect::<Vec<_>>()
        );
        assert_eq!(audio_a, audio_b);
        assert!((0..3).all(|c| a.iter().any(|s| s.class == c)));
        assert!(audio_a.iter().any(|&s| s.abs() > 0.01));
    }

    /// A kit from `seed` playing `strikes`, converted.
    fn convert(seed: u64, strikes: &[Strike]) -> Vec<Reference> {
        let mut rng = Rng::new(seed);
        let kit = [sound(0, &mut rng), sound(1, &mut rng), sound(2, &mut rng)];
        transcribe(&render(strikes, &kit).unwrap()).unwrap()
    }

    // Spec: audio-to-midi R20. Drums that land together, a kick with a
    // hi-hat, a kick with a snare, a snare with a hi-hat, all three, are
    // each found, in at least nine of ten across twelve kits.
    #[test]
    fn hits_that_land_together_are_each_found() {
        let steps: [(u32, &[usize]); 4] =
            [(0, &[0, 2]), (4, &[0, 1]), (8, &[1, 2]), (12, &[0, 1, 2])];
        let strikes: Vec<Strike> = steps
            .iter()
            .flat_map(|&(step, classes)| {
                classes.iter().map(move |&class| Strike {
                    class,
                    step,
                    velocity: 100,
                })
            })
            .collect();
        let (mut found, mut wanted) = (0, 0);
        for seed in 0..12 {
            let notes = convert(50_000 + seed, &strikes);
            for s in &strikes {
                let at = f64::from(s.step) * SIXTEENTH;
                wanted += 1;
                found += usize::from(
                    notes
                        .iter()
                        .any(|n| n.pitch == PADS[s.class] && (n.start - at).abs() < 0.05),
                );
            }
        }
        assert!(found * 10 >= wanted * 9, "{found} of {wanted}");
    }

    // Spec: audio-to-midi R22. Each note is a sixteenth long, on its pad;
    // a harder hit has a higher velocity, and the strongest is 127.
    #[test]
    fn notes_are_a_sixteenth_long_and_velocity_follows_the_hit() {
        let hit = |step, velocity| Strike {
            class: 0,
            step,
            velocity,
        };
        let strikes = [hit(0, 127), hit(8, 80), hit(16, 40)];
        let (mut firsts, mut seconds, mut thirds) = (0, 0, 0);
        for seed in 0..6 {
            let mut rng = Rng::new(60_000 + seed);
            let kit = [sound(0, &mut rng), sound(1, &mut rng), sound(2, &mut rng)];
            let audio = render(&strikes, &kit).unwrap();
            let audio = Audio::new(&audio, 1, RATE.hz()).unwrap();
            let notes = drums::drums(&audio, SIXTEENTH, None, &mut |_| true).unwrap();
            assert!(
                notes
                    .iter()
                    .all(|n| (n.end - n.start - SIXTEENTH).abs() < 1e-9)
            );
            let kicks: Vec<u8> = notes
                .iter()
                .filter(|n| n.pitch == PADS[0])
                .map(|n| n.velocity)
                .collect();
            assert_eq!(kicks.len(), 3, "kit {seed}: {notes:?}");
            assert_eq!(kicks[0], 127);
            firsts += usize::from(kicks[0] > kicks[1]);
            seconds += usize::from(kicks[1] > kicks[2]);
            thirds += usize::from(kicks[2] >= 1);
        }
        assert_eq!((firsts, seconds, thirds), (6, 6, 6));
    }

    // Spec: audio-to-midi R21. The set `bl-cli drum-training-set` writes
    // from the training seed starts as the committed training set does, to
    // a hundredth in each feature (the platforms' maths differs in the last
    // places).
    #[test]
    fn the_committed_training_set_is_what_the_generator_writes() {
        let committed = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tools/drum-classifier/training.txt"),
        )
        .unwrap();
        let mut written = Vec::new();
        write_training_set(&mut written, &TrainingArgs { breaks: 4, seed: 1 }).unwrap();
        let written = String::from_utf8(written).unwrap();
        assert!(written.lines().count() > 100);
        for (new, old) in written.lines().zip(committed.lines()) {
            let (new, old): (Vec<f64>, Vec<f64>) = (
                new.split(' ').map(|n| n.parse().unwrap()).collect(),
                old.split(' ').map(|n| n.parse().unwrap()).collect(),
            );
            assert!(
                new.iter().zip(&old).all(|(a, b)| (a - b).abs() <= 0.011),
                "{new:?} against {old:?}"
            );
        }
    }
}
