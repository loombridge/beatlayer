//! `bl-cli transcribe-bench`: how accurately Convert Melody and Convert
//! Harmony find notes (the audio-to-MIDI spec, R23 and R24).
//!
//! Seeded phrases with known notes are played through Beatlayer's own
//! instruments, Drift and Simpler playing generated waveforms, in three
//! variants: clean, with a generated room, and with noise 20 dB below the
//! signal. Each is transcribed and scored by mir_eval's note definitions
//! (Raffel et al., 2014): an estimated note matches a reference note of
//! the same pitch (within 50 cents; here whole keys) whose onset is within
//! 50 ms, each note matching at most one. The table's F-measure counts
//! onsets alone, as the targets do; the last column also asks for the
//! offset within the larger of 50 ms and a fifth of the reference's length,
//! and is reported without a target. Counts are pooled over the phrases of a
//! row.
//!
//! `BL_TRANSCRIBE_CORPUS` names a folder of an open labelled set whose
//! `<name>.wav` (or any other file the media crate decodes) has its notes
//! in `<name>.txt`: a line each of onset and offset in seconds and the MIDI
//! note number, separated by commas, tabs or spaces. Both kinds run on it,
//! with no target.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use bl_analysis::Audio;
use bl_analysis::transcribe::onnx::BasicPitch;
use bl_analysis::transcribe::{self, Kind};
use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::MediaMap;
use bl_model::{Clip, ClipData, Command, ContentHash, Editor, MidiData, Note, TrackKind};

use crate::Outcome;
use crate::render::{Cue, Settings, interleave, render_with};

/// Onset tolerance, in seconds (mir_eval's default).
const ONSET_TOLERANCE: f64 = 0.05;

/// The rate phrases are rendered at.
const RATE: SampleRate = SampleRate::HZ_48000;

/// The tempo of every phrase, so a sixteenth is 125 ms.
const SIXTEENTH: f64 = 0.125;

/// Noise's level against the signal's, in dB.
const NOISE_DB: f64 = -20.0;

/// How long the generated room rings, in seconds, to 60 dB down.
const ROOM_RT60: f64 = 0.7;

/// The onset F-measure each kind has to reach in each variant (R23).
const TARGETS: [(Kind, f64); 2] = [(Kind::Melody, 0.90), (Kind::Harmony, 0.80)];

/// The bench's arguments.
#[derive(Debug, clap::Args)]
pub struct BenchArgs {
    /// ONNX Runtime's shared library, from the Packages manager's
    /// `onnx-runtime` package. Defaults to `BL_ONNX_RUNTIME`.
    #[arg(long, env = "BL_ONNX_RUNTIME")]
    pub runtime: PathBuf,

    /// basic-pitch's model, from the `pitch-model` package. Defaults to
    /// `BL_PITCH_MODEL`.
    #[arg(long, env = "BL_PITCH_MODEL")]
    pub model: PathBuf,

    /// Phrases rendered for each kind and variant.
    #[arg(long, default_value_t = 12)]
    pub phrases: usize,

    /// The seed every phrase's own derives from.
    #[arg(long, default_value_t = 1)]
    pub seed: u64,
}

/// A note of a phrase, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reference {
    start: f64,
    end: f64,
    pitch: u8,
}

/// How a phrase is played.
#[derive(Clone, Copy, Debug)]
enum Instrument {
    /// Drift, with the oscillator's wave (an index in its list).
    Drift(f64),
    /// Simpler playing a generated sustained tone.
    Voice,
    /// Simpler playing a generated struck tone.
    Piano,
}

/// What is done to the render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Variant {
    Clean,
    Room,
    Noise,
}

impl Variant {
    const ALL: [Self; 3] = [Self::Clean, Self::Room, Self::Noise];

    fn name(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Room => "room",
            Self::Noise => "noise",
        }
    }
}

/// A small seeded generator (xorshift64*): the phrases are the same on
/// every computer.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11
    }

    /// 0 up to `n`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// -1 to 1.
    fn signed(&mut self) -> f32 {
        (self.next() as f64 / (1u64 << 52) as f64 * 2.0 - 1.0) as f32
    }
}

/// A note of a phrase on the sixteenth grid.
#[derive(Clone, Copy)]
struct Hit {
    pitch: u8,
    start: u32,
    length: u32,
    velocity: u8,
}

/// A monophonic melody of 24 notes: a walk over C3 to C6 in notes of two to
/// eight sixteenths, with rests, and a rest before a repeated pitch.
fn melody(rng: &mut Rng) -> Vec<Hit> {
    let mut hits = Vec::new();
    let (mut at, mut pitch) = (0u32, 60i64);
    for _ in 0..24 {
        let step = rng.below(11) as i64 - 5;
        let next = (pitch + step).clamp(48, 84);
        let length = [2, 3, 4, 6, 8][rng.below(5) as usize];
        let rest = if next == pitch || rng.below(10) < 3 {
            1 + rng.below(2) as u32
        } else {
            0
        };
        hits.push(Hit {
            pitch: next as u8,
            start: at,
            length,
            velocity: 60 + rng.below(68) as u8,
        });
        at += length + rest;
        pitch = next;
    }
    hits
}

/// Chords of three to five notes, four or eight sixteenths each with a
/// sixteenth's rest, from triads and sevenths on roots from C2 to B3. Odd
/// phrases are piano-like voicings: the bass an octave below the chord, and
/// a melody of two to four sixteenths' notes above it.
fn harmony(rng: &mut Rng, voicing: bool) -> Vec<Hit> {
    const CHORDS: [&[u8]; 5] = [
        &[0, 4, 7],
        &[0, 3, 7],
        &[0, 4, 7, 10],
        &[0, 3, 7, 10],
        &[0, 4, 7, 11],
    ];
    let mut hits = Vec::new();
    let mut at = 0u32;
    for _ in 0..8 {
        let root = 36 + rng.below(24) as u8;
        let chord = CHORDS[rng.below(5) as usize];
        let length = [4, 8][rng.below(2) as usize];
        let velocity = 70 + rng.below(50) as u8;
        if voicing {
            hits.push(Hit {
                pitch: root - 12,
                start: at,
                length: length - 1,
                velocity,
            });
            for &interval in &chord[1..] {
                hits.push(Hit {
                    pitch: root + 12 + interval,
                    start: at,
                    length: length - 1,
                    velocity,
                });
            }
            let mut top = at;
            while top + 2 < at + length - 1 {
                let long = 2 + rng.below(3) as u32;
                let long = long.min(at + length - 1 - top);
                hits.push(Hit {
                    pitch: root + 24 + [0, 2, 4, 7, 9][rng.below(5) as usize],
                    start: top,
                    length: long,
                    velocity: (velocity + 10).min(127),
                });
                top += long + 1;
            }
        } else {
            for &interval in chord {
                hits.push(Hit {
                    pitch: root + 12 + interval,
                    start: at,
                    length: length - 1,
                    velocity,
                });
            }
        }
        at += length;
    }
    hits
}

/// One cycle's worth of the tone `sample` in `harmonics`: a mono 44.1 kHz
/// file of 6 seconds at C4, which the sampler plays at C3's pitch (its
/// reference) and repitches.
fn tone(instrument: Instrument) -> Vec<f32> {
    let rate = 44_100.0;
    let f0 = 261.625_565_3;
    (0..6 * 44_100)
        .map(|i| {
            let t = f64::from(i) / rate;
            let mut sum = 0.0;
            for n in 1..=20 {
                let n = f64::from(n);
                let (amplitude, tau) = match instrument {
                    Instrument::Piano => (1.0 / n, 2.5 / (1.0 + 0.5 * (n - 1.0))),
                    _ => (1.0 / n.powf(1.2), 8.0),
                };
                let stretch = if matches!(instrument, Instrument::Piano) {
                    (1.0 + 0.0004 * n * n).sqrt()
                } else {
                    1.0
                };
                let hz = n * f0 * stretch;
                if hz < 15_000.0 {
                    sum += amplitude * (-t / tau).exp() * (std::f64::consts::TAU * hz * t).sin();
                }
            }
            let attack = match instrument {
                Instrument::Piano => 1.0 - (-t / 0.003).exp(),
                _ => 1.0 - (-t / 0.04).exp(),
            };
            (0.3 * sum * attack) as f32
        })
        .collect()
}

/// The WAV file's bytes of `samples`, and its hash as the engine's media
/// map keys it.
fn media(samples: &[f32], media: &mut MediaMap) -> Result<ContentHash, String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut cursor, spec).map_err(|e| e.to_string())?;
    for &s in samples {
        writer.write_sample(s).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    let bytes = cursor.into_inner();
    let hash = ContentHash(*bl_media::ContentHash::of_bytes(&bytes).as_bytes());
    let decoded = bl_media::Decoder::from_reader(std::io::Cursor::new(bytes), Some("wav"))
        .and_then(bl_media::Decoder::decode_to_end)
        .map_err(|e| e.to_string())?;
    media.insert(hash, Arc::new(decoded));
    Ok(hash)
}

fn apply(editor: &mut Editor, command: Command) -> Result<(), String> {
    editor.apply(command).map(drop).map_err(|e| e.to_string())
}

/// `hits` played by `instrument` through the engine: a mono mix at [`RATE`].
fn play(hits: &[Hit], instrument: Instrument) -> Result<Vec<f32>, String> {
    let mut editor = Editor::new();
    let mut files = MediaMap::new();
    let tempo = TempoMap::constant(Bpm::new(120.0).map_err(|e| e.to_string())?);
    apply(&mut editor, Command::SetTempo { tempo })?;
    let track = editor.new_id();
    let create = Command::CreateTrack {
        id: track,
        kind: TrackKind::Midi,
        name: "Phrase".to_owned(),
        parent: None,
        index: 0,
    };
    apply(&mut editor, create)?;
    let id = editor.new_id();
    let (device_type, params) = match instrument {
        Instrument::Drift(wave) => (
            bl_devices::polysynth::polysynth_type(),
            vec![(bl_devices::polysynth::OSC1_WAVE, wave)],
        ),
        Instrument::Voice | Instrument::Piano => (bl_devices::sampler::sampler_type(), vec![]),
    };
    let mut device = device_type.device(id);
    for (param, value) in params {
        device.params.insert(param, value);
    }
    let insert = Command::InsertDevice {
        track,
        index: 0,
        device,
    };
    apply(&mut editor, insert)?;
    if matches!(instrument, Instrument::Voice | Instrument::Piano) {
        let hash = media(&tone(instrument), &mut files)?;
        let sample = Command::SetDeviceSample {
            device: id,
            media: Some(hash),
            slot: 0,
        };
        apply(&mut editor, sample)?;
    }
    let sixteenth = Ticks::QUARTER.0 / 4;
    let notes: Vec<Note> = hits
        .iter()
        .map(|h| {
            Note::new(
                h.pitch,
                Ticks(i64::from(h.start) * sixteenth),
                Ticks(i64::from(h.length) * sixteenth),
                h.velocity,
            )
        })
        .collect();
    let end = hits.iter().map(|h| h.start + h.length).max().unwrap_or(0);
    let clip = editor.new_id();
    let create = Command::CreateClip {
        track,
        clip: Clip::new(
            clip,
            Ticks::ZERO,
            Ticks(i64::from(end + 4) * sixteenth),
            ClipData::Midi(MidiData::new(notes)),
        ),
    };
    apply(&mut editor, create)?;
    let seconds = f64::from(end) * SIXTEENTH + 2.0;
    let settings = Settings {
        sample_rate: RATE,
        block: 512,
        frames: RATE.seconds_to_frames(seconds).get() as u64,
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

/// `signal` with a generated room: a sparse tail of 1,500 taps a second,
/// decaying 60 dB in [`ROOM_RT60`] seconds, a third as loud as the signal.
fn room(signal: &[f32], rng: &mut Rng) -> Vec<f32> {
    let rate = f64::from(RATE.hz());
    let length = (ROOM_RT60 * rate) as usize;
    let taps: Vec<(usize, f32)> = (0..(ROOM_RT60 * 1_500.0) as usize)
        .map(|_| {
            let at = rng.below(length as u64) as usize;
            let decay = (-6.908 * at as f64 / length as f64).exp() as f32;
            (at, rng.signed().signum() * decay)
        })
        .collect();
    let energy = taps.iter().map(|(_, a)| a * a).sum::<f32>().sqrt();
    let mut out = signal.to_vec();
    out.resize(signal.len() + length, 0.0);
    for &(at, a) in &taps {
        let gain = a / energy / 3.0;
        for (o, &s) in out[at..].iter_mut().zip(signal) {
            *o += gain * s;
        }
    }
    out.truncate(signal.len());
    out
}

/// `signal` with white noise at [`NOISE_DB`] against its RMS.
fn noisy(signal: &[f32], rng: &mut Rng) -> Vec<f32> {
    let rms = (signal.iter().map(|s| s * s).sum::<f32>() / signal.len() as f32).sqrt();
    // Uniform noise of amplitude `a` has an RMS of a / sqrt(3).
    let amplitude = rms * 10f32.powf(NOISE_DB as f32 / 20.0) * 3f32.sqrt();
    signal
        .iter()
        .map(|s| s + amplitude * rng.signed())
        .collect()
}

/// How many of `estimated` match `reference`, by onset within
/// [`ONSET_TOLERANCE`] and the same pitch, and how many of those also have
/// their offset within the larger of 50 ms and a fifth of the reference's
/// length. Each note matches at most one; within a pitch, taking notes in
/// order of onset, first-come matching is the most there can be.
fn match_notes(reference: &[Reference], estimated: &[Reference]) -> (usize, usize) {
    let (mut onsets, mut with_offsets) = (0, 0);
    let mut pitches: Vec<u8> = reference.iter().map(|n| n.pitch).collect();
    pitches.sort_unstable();
    pitches.dedup();
    for pitch in pitches {
        let of = |notes: &[Reference]| {
            let mut v: Vec<Reference> =
                notes.iter().filter(|n| n.pitch == pitch).copied().collect();
            v.sort_by(|a, b| a.start.total_cmp(&b.start));
            v
        };
        let (refs, ests) = (of(reference), of(estimated));
        let mut taken = vec![false; ests.len()];
        for r in &refs {
            let found = ests
                .iter()
                .enumerate()
                .find(|(i, e)| !taken[*i] && (e.start - r.start).abs() <= ONSET_TOLERANCE);
            if let Some((i, e)) = found {
                taken[i] = true;
                onsets += 1;
                let tolerance = (0.2 * (r.end - r.start)).max(0.05);
                if (e.end - r.end).abs() <= tolerance {
                    with_offsets += 1;
                }
            }
        }
    }
    (onsets, with_offsets)
}

/// Counts pooled over phrases.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Tally {
    phrases: usize,
    reference: usize,
    estimated: usize,
    onsets: usize,
    with_offsets: usize,
}

impl Tally {
    fn add(&mut self, reference: &[Reference], estimated: &[Reference]) {
        let (onsets, with_offsets) = match_notes(reference, estimated);
        self.phrases += 1;
        self.reference += reference.len();
        self.estimated += estimated.len();
        self.onsets += onsets;
        self.with_offsets += with_offsets;
    }

    fn precision(&self) -> f64 {
        ratio(self.onsets, self.estimated)
    }

    fn recall(&self) -> f64 {
        ratio(self.onsets, self.reference)
    }

    fn f_measure(&self) -> f64 {
        f_of(self.onsets, self.estimated, self.reference)
    }

    fn f_with_offsets(&self) -> f64 {
        f_of(self.with_offsets, self.estimated, self.reference)
    }
}

fn ratio(n: usize, of: usize) -> f64 {
    if of == 0 { 0.0 } else { n as f64 / of as f64 }
}

fn f_of(matched: usize, estimated: usize, reference: usize) -> f64 {
    let (p, r) = (ratio(matched, estimated), ratio(matched, reference));
    if p + r == 0.0 {
        0.0
    } else {
        2.0 * p * r / (p + r)
    }
}

/// The notes `audio` has by `kind`.
fn transcribe_notes(
    network: &mut BasicPitch,
    samples: &[f32],
    channels: usize,
    rate: u32,
    kind: Kind,
) -> Result<Vec<Reference>, String> {
    let audio = Audio::new(samples, channels, rate)?;
    let grid = transcribe::Grid {
        period: SIXTEENTH,
        origin: 0.0,
    };
    let notes = transcribe::transcribe(&audio, network, kind, Some(grid), &mut |_| true)
        .map_err(|e| e.to_string())?;
    Ok(notes
        .iter()
        .map(|n| Reference {
            start: n.start,
            end: n.end,
            pitch: n.pitch,
        })
        .collect())
}

/// The phrases of one kind: a phrase's notes and how each is played.
fn phrases(kind: Kind, count: usize, seed: u64) -> Vec<(Vec<Hit>, Instrument)> {
    (0..count)
        .map(|i| {
            let mut rng = Rng::new(seed * 1_000 + i as u64);
            match kind {
                Kind::Melody => {
                    let instrument = match i % 3 {
                        0 => Instrument::Voice,
                        1 => Instrument::Drift(3.0),
                        _ => Instrument::Drift(1.0),
                    };
                    (melody(&mut rng), instrument)
                }
                Kind::Harmony => {
                    let instrument = if i % 2 == 0 {
                        Instrument::Piano
                    } else {
                        Instrument::Drift(2.0)
                    };
                    (harmony(&mut rng, i % 4 >= 2), instrument)
                }
            }
        })
        .collect()
}

fn name(kind: Kind) -> &'static str {
    match kind {
        Kind::Melody => "Melody",
        Kind::Harmony => "Harmony",
    }
}

fn header() {
    println!(
        "{:<8} {:<8} {:>7} {:>6} {:>10} {:>7} {:>9} {:>11} {:>7}",
        "kind",
        "set",
        "phrases",
        "notes",
        "precision",
        "recall",
        "F-measure",
        "F, offsets",
        "target"
    );
}

fn row(kind: Kind, set: &str, tally: &Tally, target: Option<f64>) -> bool {
    let met = target.is_none_or(|t| tally.f_measure() >= t);
    println!(
        "{:<8} {:<8} {:>7} {:>6} {:>10.3} {:>7.3} {:>9.3} {:>11.3} {:>7}{}",
        name(kind),
        set,
        tally.phrases,
        tally.reference,
        tally.precision(),
        tally.recall(),
        tally.f_measure(),
        tally.f_with_offsets(),
        target.map_or_else(|| "none".to_owned(), |t| format!("{t:.2}")),
        if met { "" } else { "  FAIL" },
    );
    met
}

/// Reads a reference file of the corpus: onset, offset and MIDI note, a
/// line each.
fn read_notes(path: &Path) -> Result<Vec<Reference>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            let fields: Vec<f64> = line
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|f| !f.is_empty())
                .map(|f| {
                    f.parse()
                        .map_err(|_| format!("{}: `{line}`", path.display()))
                })
                .collect::<Result<_, _>>()?;
            match fields[..] {
                [start, end, pitch, ..] => Ok(Reference {
                    start,
                    end,
                    pitch: pitch.round().clamp(0.0, 127.0) as u8,
                }),
                _ => Err(format!(
                    "{}: `{line}` isn't onset, offset, note",
                    path.display()
                )),
            }
        })
        .collect()
}

/// Both kinds on the open labelled set in `folder`, with no target.
fn corpus(network: &mut BasicPitch, folder: &Path) -> Result<bool, String> {
    let mut pairs: Vec<(PathBuf, PathBuf)> = std::fs::read_dir(folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x != "txt") && p.with_extension("txt").exists())
        .map(|p| (p.clone(), p.with_extension("txt")))
        .collect();
    pairs.sort();
    if pairs.is_empty() {
        return Err(format!(
            "{} holds no audio file with a .txt of notes beside it",
            folder.display()
        ));
    }
    for kind in [Kind::Melody, Kind::Harmony] {
        let mut tally = Tally::default();
        for (audio, notes) in &pairs {
            let data = bl_media::Decoder::open(audio)
                .and_then(bl_media::Decoder::decode_to_end)
                .map_err(|e| format!("{}: {e}", audio.display()))?;
            let mut interleaved = Vec::with_capacity(data.frames() * data.channel_count());
            for frame in 0..data.frames() {
                interleaved.extend(data.channels().map(|c| c[frame]));
            }
            let found = transcribe_notes(
                network,
                &interleaved,
                data.channel_count(),
                data.sample_rate(),
                kind,
            )?;
            tally.add(&read_notes(notes)?, &found);
        }
        row(kind, "corpus", &tally, None);
    }
    Ok(true)
}

/// Runs `bl-cli transcribe-bench`.
///
/// # Errors
///
/// A message if the runtime or the model won't load, or a phrase can't be
/// rendered. The exit code is a failure if a target isn't met.
pub fn run(args: &BenchArgs) -> Outcome {
    let mut network = BasicPitch::load(&args.runtime, &args.model)?;
    header();
    let mut passed = true;
    let mut identical = true;
    for (kind, target) in TARGETS {
        let set = phrases(kind, args.phrases, args.seed);
        for variant in Variant::ALL {
            let mut tally = Tally::default();
            for (i, (hits, instrument)) in set.iter().enumerate() {
                let clean = play(hits, *instrument)?;
                let mut rng = Rng::new(args.seed * 7_919 + i as u64);
                let heard = match variant {
                    Variant::Clean => clean,
                    Variant::Room => room(&clean, &mut rng),
                    Variant::Noise => noisy(&clean, &mut rng),
                };
                let reference: Vec<Reference> = hits
                    .iter()
                    .map(|h| Reference {
                        start: f64::from(h.start) * SIXTEENTH,
                        end: f64::from(h.start + h.length) * SIXTEENTH,
                        pitch: h.pitch,
                    })
                    .collect();
                let found = transcribe_notes(&mut network, &heard, 1, RATE.hz(), kind)?;
                // R17: the first phrase of each row is transcribed twice.
                if i == 0 {
                    identical &=
                        found == transcribe_notes(&mut network, &heard, 1, RATE.hz(), kind)?;
                }
                tally.add(&reference, &found);
            }
            passed &= row(kind, variant.name(), &tally, Some(target));
        }
    }
    println!(
        "a repeat run gives {} notes",
        if identical { "identical" } else { "DIFFERENT" }
    );
    passed &= identical;
    if let Some(folder) = std::env::var_os("BL_TRANSCRIBE_CORPUS") {
        corpus(&mut network, Path::new(&folder))?;
    }
    Ok(if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: f64, end: f64, pitch: u8) -> Reference {
        Reference { start, end, pitch }
    }

    // Spec: audio-to-midi R23. A note matches by pitch and an onset within
    // 50 ms, once.
    #[test]
    fn notes_match_by_pitch_and_onset_within_50_ms_once_each() {
        let reference = [note(0.0, 1.0, 60), note(1.0, 2.0, 60), note(2.0, 3.0, 62)];
        let estimated = [note(0.04, 1.0, 60), note(0.06, 1.0, 60), note(2.0, 3.0, 61)];
        assert_eq!(match_notes(&reference, &estimated), (1, 1));
        let mut tally = Tally::default();
        tally.add(&reference, &estimated);
        assert!((tally.precision() - 1.0 / 3.0).abs() < 1e-9);
        assert!((tally.recall() - 1.0 / 3.0).abs() < 1e-9);
        assert!((tally.f_measure() - 1.0 / 3.0).abs() < 1e-9);
    }

    // Spec: audio-to-midi R23. Offsets are reported, not scored: a note
    // with the wrong offset still counts in the onset F-measure.
    #[test]
    fn offsets_are_reported_but_the_onset_score_ignores_them() {
        let reference = [note(0.0, 1.0, 60)];
        assert_eq!(match_notes(&reference, &[note(0.0, 1.15, 60)]), (1, 1));
        assert_eq!(match_notes(&reference, &[note(0.0, 1.5, 60)]), (1, 0));
    }

    // Spec: audio-to-midi R23. The same seed gives the same phrase and the
    // same render.
    #[test]
    fn a_seed_gives_the_same_phrase_and_the_same_audio() {
        let make = || {
            let (hits, instrument) = phrases(Kind::Melody, 1, 5).remove(0);
            (
                hits.iter().map(|h| (h.pitch, h.start)).collect::<Vec<_>>(),
                play(&hits, instrument),
            )
        };
        let (a, audio_a) = make();
        let (b, audio_b) = make();
        assert_eq!(a, b);
        assert_eq!(audio_a, audio_b);
        assert!(audio_a.unwrap().iter().any(|&s| s.abs() > 0.01));
    }
}
