//! Auto Shift (WP-8.46; the Auto Shift spec): the input section and its
//! latencies, the Quantizer's correction, the MIDI tab's voices, the LFO, the
//! pitch and vibrato sections, the display, and renders that repeat at every
//! block size. The voice is made here: an impulse train through three formant
//! resonators. Each block runs as a real-time section under the allocation
//! trap.

use std::f64::consts::{PI, TAU};

use crate::common;
use bl_core::id::Id;
use bl_core::time::SampleRate;
use bl_devices::auto_shift::{
    self, ATTACK, AUTO_SHIFT, BEND_RANGE, DEGREES, DRY_WET, FINE, FORMANT_FOLLOW, FORMANT_SHIFT,
    GLIDE, INPUT_GAIN, KEY_C, LFO_ATTACK, LFO_DELAY, LFO_DIVISION, LFO_PITCH, LFO_RATE, LFO_RESET,
    LFO_SYNC, LFO_VOLUME, LFO_WAVE, LIVE_MODE, MIDI_ON, MONO_POLY, NATURAL_VIBRATO, NOTE_LATCH,
    PAN_DEPTH, PAN_SOURCE, PITCH_DEPTH, PITCH_RANGE, PITCH_SHIFT, PITCH_SOURCE, RELEASE, ROOT,
    SCALE, SMOOTH, SMOOTHING_TIME, STRENGTH, USE_SCALE, VIBRATO_AMOUNT, VIBRATO_FADE, VIBRATO_RATE,
    VOICE_COUNT, VOLUME_DEPTH, VOLUME_SOURCE, auto_shift_type,
};
use bl_dsp::Random;
use bl_dsp::pitch::{Config, Tracker};
use bl_engine::scales::ScaleRuns;
use bl_engine::{
    Block, Node, NodeConfig, NodeEvent, NoteBuffer, NoteEvent, NoteKind, TransportInfo,
};
use bl_model::{DeviceId, ParamId, Scale, ScaleName};

/// The block sizes every render must agree at (R19).
const BLOCKS: [usize; 4] = [1, 64, 512, 4_096];

const RATE: SampleRate = SampleRate::HZ_48000;
const HZ: f64 = 48_000.0;

fn id(n: u64) -> DeviceId {
    DeviceId(Id::new(n).expect("an ID"))
}

fn cents(hz: f64, from: f64) -> f64 {
    1200.0 * (hz / from).log2()
}

/// The frequency of key `key`, A3 = 440 Hz at 69.
fn hz(key: f64) -> f64 {
    440.0 * ((key - 69.0) / 12.0).exp2()
}

/// A render of the device: its node and what it was given.
struct Rig {
    node: Box<dyn Node>,
}

impl Rig {
    fn new(params: &[(ParamId, f64)]) -> Self {
        Self::with_id(1, params)
    }

    fn with_id(n: u64, params: &[(ParamId, f64)]) -> Self {
        let kind = auto_shift_type();
        let mut device = kind.device(id(n));
        for (param, value) in params {
            device.params.insert(*param, *value);
        }
        let config = NodeConfig {
            round_trip_latency: 0,
            sample_rate: RATE,
            max_frames: 4_096,
            channels: 2,
        };
        Self {
            node: common::registry()
                .create(&device, &config)
                .expect("the registry makes Auto Shift"),
        }
    }

    /// Runs `input` (both channels) in blocks of `block` frames, with
    /// `events` and `notes` (frames from the start), `scale_at` giving the
    /// scale at a frame. Returns each channel.
    fn render(
        &mut self,
        input: &[f32],
        block: usize,
        events: &[(usize, ParamId, f64)],
        notes: &[NoteEvent],
        scale_at: impl Fn(usize) -> Scale,
    ) -> Vec<Vec<f32>> {
        let mut out = vec![Vec::new(); 2];
        for start in (0..input.len()).step_by(block) {
            let end = (start + block).min(input.len());
            let len = end - start;
            let here: Vec<NodeEvent> = events
                .iter()
                .filter(|e| e.0 >= start && e.0 < end)
                .map(|&(at, param, value)| common::set((at - start) as u32, param, value))
                .collect();
            let heard: Vec<NoteEvent> = notes
                .iter()
                .filter(|n| (n.offset as usize) >= start && (n.offset as usize) < end)
                .map(|n| NoteEvent {
                    offset: n.offset - start as u32,
                    ..*n
                })
                .collect();
            let mut runs = ScaleRuns::of(scale_at(start));
            for offset in 1..len {
                runs.push(offset as u32, scale_at(start + offset));
            }
            let mut data: Vec<f32> = (0..2).flat_map(|_| input[start..end].to_vec()).collect();
            let mut sent = NoteBuffer::with_capacity(8);
            {
                let mut block = Block::new(
                    &mut data,
                    2,
                    len,
                    len,
                    &here,
                    TransportInfo::default(),
                    RATE,
                )
                .with_scale(runs)
                .with_notes(&heard, &mut sent);
                bl_rt::rt_test!({ self.node.process(&mut block) });
            }
            for (channel, values) in out.iter_mut().zip(data.chunks(len)) {
                channel.extend_from_slice(values);
            }
        }
        out
    }

    /// `input` with nothing else, in blocks of 512; the left channel.
    fn play(&mut self, input: &[f32]) -> Vec<f32> {
        self.render(input, 512, &[], &[], |_| Scale::default())
            .swap_remove(0)
    }

    fn latency(&self) -> usize {
        self.node.latency() as usize
    }
}

fn on(offset: usize, key: u8) -> NoteEvent {
    NoteEvent {
        offset: offset as u32,
        kind: NoteKind::On,
        channel: 0,
        key,
        velocity: 0.8,
        note_id: None,
    }
}

fn off(offset: usize, key: u8) -> NoteEvent {
    NoteEvent {
        kind: NoteKind::Off,
        ..on(offset, key)
    }
}

/// The first three formants of the vowel A, their bandwidths and levels.
const A: [f64; 3] = [700.0, 1220.0, 2600.0];
const BANDWIDTH: [f64; 3] = [120.0, 150.0, 200.0];
const LEVELS: [f64; 3] = [1.0, 0.8, 0.7];

/// A two-pole resonator at `hz`, `bandwidth` wide, with unit gain at its peak.
struct Resonator {
    a1: f64,
    a2: f64,
    gain: f64,
    y1: f64,
    y2: f64,
}

impl Resonator {
    fn new(hz: f64, bandwidth: f64) -> Self {
        let r = (-PI * bandwidth / HZ).exp();
        let theta = TAU * hz / HZ;
        let (a1, a2) = (2.0 * r * theta.cos(), -r * r);
        let (sin, cos) = theta.sin_cos();
        let (sin2, cos2) = (2.0 * theta).sin_cos();
        Self {
            a1,
            a2,
            gain: (1.0 - a1 * cos - a2 * cos2).hypot(a1 * sin + a2 * sin2),
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.gain * x + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// One period of the Rosenberg glottal pulse's derivative at `phase` (0 to
/// 1): the flow rises over the first 40 % and falls over the next 16 %.
fn glottal(phase: f64) -> f64 {
    const OPEN: f64 = 0.4;
    const CLOSE: f64 = 0.16;
    if phase < OPEN {
        0.5 * PI / OPEN * (PI * phase / OPEN).sin()
    } else if phase < OPEN + CLOSE {
        -PI / (2.0 * CLOSE) * (PI * (phase - OPEN) / (2.0 * CLOSE)).sin()
    } else {
        0.0
    }
}

/// The Fourier series of [`glottal`], harmonic 1 first, as cosine and sine
/// coefficients: no more than 100 harmonics, so the pulse train never
/// aliases.
fn glottal_series() -> &'static [(f64, f64)] {
    static SERIES: std::sync::OnceLock<Vec<(f64, f64)>> = std::sync::OnceLock::new();
    SERIES.get_or_init(|| {
        const STEPS: usize = 8192;
        (1..=100)
            .map(|n| {
                let (mut c, mut s) = (0.0, 0.0);
                for step in 0..STEPS {
                    let phase = (step as f64 + 0.5) / STEPS as f64;
                    let (sin, cos) = (TAU * f64::from(n) * phase).sin_cos();
                    c += glottal(phase) * cos;
                    s += glottal(phase) * sin;
                }
                (2.0 * c / STEPS as f64, 2.0 * s / STEPS as f64)
            })
            .collect()
    })
}

/// A voice whose fundamental at sample `i` is `f0(i)` hertz, singing the
/// vowel A with its formants `scale` times as high, at −12 dBFS RMS: the
/// glottal source through three resonators.
fn voice(f0: impl Fn(usize) -> f64, scale: f64, len: usize) -> Vec<f32> {
    let series = glottal_series();
    let mut filters = [0, 1, 2].map(|k| Resonator::new(A[k] * scale, BANDWIDTH[k] * scale));
    let mut phase = 0.0f64;
    let out: Vec<f64> = (0..len)
        .map(|i| {
            let harmonics = ((0.4 * HZ / f0(i)) as usize).min(series.len());
            let source: f64 = series[..harmonics]
                .iter()
                .enumerate()
                .map(|(n, (c, s))| {
                    let (sin, cos) = (TAU * (n + 1) as f64 * phase).sin_cos();
                    c * cos + s * sin
                })
                .sum();
            phase = (phase + f0(i) / HZ).fract();
            filters
                .iter_mut()
                .zip(LEVELS)
                .map(|(f, level)| level * f.run(source))
                .sum()
        })
        .collect();
    let rms = (out.iter().map(|x| x * x).sum::<f64>() / len as f64).sqrt();
    out.iter().map(|x| (x * 0.25 / rms) as f32).collect()
}

fn noise(seed: u64, len: usize) -> Vec<f32> {
    let mut random = Random::seeded(seed);
    (0..len)
        .map(|_| random.next_bipolar() as f32 * 0.3)
        .collect()
}

/// The pitch the tracker reads at each of its measurements of `x`: the sample
/// the window was centred on, and the frequency, for the voiced ones.
fn pitches(x: &[f32]) -> Vec<(usize, f64)> {
    let config = Config::for_range(HZ, 40.0, 2_000.0);
    let window = config.window;
    let mut tracker = Tracker::new(&config);
    let mut found = Vec::new();
    for &sample in x {
        tracker.feed(sample);
        while let Some(reading) = tracker.pop() {
            if let Some(p) = reading.filter(|p| p.voiced) {
                found.push((p.ended as usize - window / 2, p.frequency));
            }
        }
    }
    found
}

/// The median pitch of `x` between samples `from` and `to`.
fn pitch_between(x: &[f32], from: usize, to: usize) -> f64 {
    let mut heard: Vec<f64> = pitches(x)
        .into_iter()
        .filter(|(at, _)| (from..to).contains(at))
        .map(|(_, f)| f)
        .collect();
    assert!(!heard.is_empty(), "no pitch heard between {from} and {to}");
    heard.sort_by(f64::total_cmp);
    heard[heard.len() / 2]
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

/// The amplitude of `x` at `at` hertz through a Hann window.
fn amplitude(x: &[f32], at: f64) -> f64 {
    let w = TAU * at / HZ;
    let n = x.len() as f64;
    let (mut re, mut im, mut sum) = (0.0, 0.0, 0.0);
    for (i, sample) in x.iter().enumerate() {
        let window = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
        let angle = w * i as f64;
        re += f64::from(*sample) * window * angle.cos();
        im -= f64::from(*sample) * window * angle.sin();
        sum += window;
    }
    2.0 * re.hypot(im) / sum
}

/// A sine of `at` hertz and amplitude `level`.
fn sine(at: f64, level: f32, len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| (TAU * at * i as f64 / HZ).sin() as f32 * level)
        .collect()
}

/// How far `note` is from the nearest note of C major, in cents.
fn from_c_major(note: f64) -> f64 {
    let pitch_class = note.rem_euclid(12.0);
    [0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0, 12.0]
        .iter()
        .map(|step| (pitch_class - step).abs() * 100.0)
        .fold(f64::MAX, f64::min)
}

fn note_of(hz: f64) -> f64 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

// Spec: auto-shift R1. The device is an audio effect with a MIDI sidechain
// that has a tapping point, Live's name and the parameters the spec lists.
#[test]
fn auto_shift_is_an_audio_effect_with_a_midi_sidechain_and_the_manual_s_names() {
    let kind = auto_shift_type();
    assert_eq!(AUTO_SHIFT, "beatlayer.auto_shift");
    assert_eq!(kind.name, "Auto Shift");
    let device = kind.device(id(1));
    assert!(
        device.midi_sidechain.is_some_and(|s| s.tap.is_some()),
        "a MIDI sidechain with a tapping point"
    );
    let names: Vec<&str> = kind
        .description
        .params
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    for wanted in [
        "Use Current Scale",
        "Live Mode",
        "MIDI On",
        "Pitch Range",
        "Input Gain",
        "Correction Strength",
        "Smooth",
        "Smoothing Time",
        "Root",
        "Scale",
        "Pitch Shift",
        "Pitch Shift Fine",
        "Formant Shift",
        "Formant Follow",
        "Vibrato Amount",
        "Vibrato Rate",
        "Vibrato Fade In",
        "Natural Vibrato",
        "Dry/Wet",
        "Mono/Poly",
        "Voice Count",
        "Glide",
        "Attack Time",
        "Release Time",
        "Note Latch",
        "Pitch Bend Range",
        "LFO Reset",
        "LFO Delay",
        "LFO Attack",
        "LFO Waveform",
        "LFO Rate",
    ] {
        assert!(names.contains(&wanted), "no parameter {wanted:?}");
    }
    // The device starts in Mid, at C major, with the correction full.
    assert_eq!(device.params.get(&PITCH_RANGE).copied().unwrap_or(1.0), 1.0);
}

// Spec: auto-shift R3. The latency is fixed by Pitch Range and Live Mode, the
// Bass range the longest and Live Mode half, and the node reports it.
#[test]
fn the_latency_is_fixed_by_pitch_range_and_live_mode() {
    let latency =
        |range: f64, live: f64| Rig::new(&[(PITCH_RANGE, range), (LIVE_MODE, live)]).latency();
    let (high, mid, bass) = (latency(0.0, 0.0), latency(1.0, 0.0), latency(2.0, 0.0));
    assert!(high < mid && mid < bass, "{high} {mid} {bass}");
    for (range, full) in [(0.0, high), (1.0, mid), (2.0, bass)] {
        let live = latency(range, 1.0);
        assert!(
            live.abs_diff(full / 2) <= 1,
            "Live Mode halves {full} to {live}"
        );
    }
    // Spec: a period of the range's lowest pitch (a twentieth below its
    // low end) three and a quarter times, and four samples.
    for (n, (low, _)) in auto_shift::RANGE_HERTZ.iter().enumerate() {
        let longest = (HZ / (low * 0.95)).ceil();
        let wanted = (13.0 * longest / 4.0).ceil() + 4.0;
        assert_eq!(latency(n as f64, 0.0) as f64, wanted, "range {n}");
    }
}

// Spec: auto-shift R3. A change of Pitch Range or Live Mode changes the
// latency the node reports.
#[test]
fn a_change_of_range_or_live_mode_changes_the_reported_latency() {
    let mut rig = Rig::new(&[]);
    let mid = rig.latency();
    rig.render(&[0.0; 512], 512, &[(0, PITCH_RANGE, 2.0)], &[], |_| {
        Scale::default()
    });
    let bass = rig.latency();
    assert!(bass > mid);
    rig.render(&[0.0; 512], 512, &[(0, LIVE_MODE, 1.0)], &[], |_| {
        Scale::default()
    });
    assert!(rig.latency().abs_diff(bass / 2) <= 1);
}

// Spec: auto-shift R3, R15. The dry path is delayed by the latency, on every
// range with Live Mode off and on: at Dry/Wet 0 an impulse comes out that
// many frames later, exactly.
#[test]
fn the_dry_path_is_aligned_with_the_latency() {
    for range in [0.0, 1.0, 2.0] {
        for live in [0.0, 1.0] {
            let mut rig = Rig::new(&[(PITCH_RANGE, range), (LIVE_MODE, live), (DRY_WET, 0.0)]);
            let latency = rig.latency();
            let mut input = vec![0.0f32; latency + 2_000];
            input[100] = 1.0;
            let out = rig.play(&input);
            let at = out.iter().position(|s| *s != 0.0).expect("an impulse");
            assert_eq!(at, 100 + latency, "range {range} live {live}");
            assert_eq!(out[at], 1.0);
        }
    }
}

// Spec: auto-shift R4. A frame the tracker calls unvoiced passes through
// without a correction: noise comes out as it went in, a latency later.
#[test]
fn unvoiced_frames_pass_uncorrected() {
    let mut rig = Rig::new(&[(STRENGTH, 100.0), (PITCH_SHIFT, 5.0)]);
    let latency = rig.latency();
    let input = noise(5, 30_000);
    let out = rig.play(&input);
    for i in latency..input.len() {
        assert!(
            (out[i] - input[i - latency]).abs() < 1e-6,
            "sample {i}: {} against {}",
            out[i],
            input[i - latency]
        );
    }
}

// Spec: auto-shift R5, R6. A synthetic voice gliding between scale notes is
// held within 5 cents of the nearest scale note at full strength.
#[test]
fn a_voice_gliding_between_scale_notes_is_held_within_5_cents_of_the_nearest() {
    // C2 to D2 over a second, after a second at C2 and before a second at
    // D2, in C major (the new device's scale).
    let (c, d) = (hz(48.0), hz(50.0));
    let f0 = |i: usize| {
        let t = i as f64 / HZ;
        let x = ((t - 1.0) / 1.0).clamp(0.0, 1.0);
        c * (d / c).powf(x)
    };
    let input = voice(f0, 1.0, 3 * 48_000);
    let mut rig = Rig::new(&[(SMOOTH, 0.0), (STRENGTH, 100.0)]);
    let latency = rig.latency();
    let out = rig.play(&input);
    let heard = pitches(&out);
    let settled: Vec<&(usize, f64)> = heard
        .iter()
        .filter(|(at, _)| *at > latency + 12_000)
        .collect();
    assert!(settled.len() > 100, "the corrected voice is heard");
    for (at, f) in settled {
        let note = note_of(*f);
        assert!(
            from_c_major(note) < 5.0,
            "{at}: {note:.2} is {:.1} cents from the scale",
            from_c_major(note)
        );
    }
    // Held at C before the glide and at D after it.
    let before = pitch_between(&out, latency + 30_000, latency + 45_000);
    let after = pitch_between(&out, latency + 100_000, latency + 140_000);
    assert!(
        cents(before, c).abs() < 5.0,
        "{before:.2} Hz, wanted {c:.2}"
    );
    assert!(cents(after, d).abs() < 5.0, "{after:.2} Hz, wanted {d:.2}");
}

/// A voice at `note` (a key, a fraction of one included) for `len` samples.
fn held(note: f64, len: usize) -> Vec<f32> {
    voice(|_| hz(note), 1.0, len)
}

/// The output pitch of a steady voice at `note`, after `params`, in keys.
fn corrected(note: f64, params: &[(ParamId, f64)]) -> f64 {
    let mut rig = Rig::new(params);
    let latency = rig.latency();
    let out = rig.play(&held(note, 60_000));
    note_of(pitch_between(&out, latency + 30_000, 60_000))
}

// Spec: auto-shift R6. Correction Strength is the share of the distance to
// the target the correction covers: none at 0 %, half at 50 %, all at 100 %.
#[test]
fn correction_strength_is_the_share_of_the_distance_covered() {
    let off = [(SMOOTH, 0.0)];
    for (strength, wanted) in [(0.0, 48.6), (50.0, 48.3), (100.0, 48.0)] {
        let note = corrected(48.6, &[off[0], (STRENGTH, strength)]);
        assert!(
            (note - wanted).abs() < 0.05,
            "at {strength} %: {note:.3}, wanted {wanted}"
        );
    }
}

// Spec: auto-shift R6. With Smooth on, the correction moves to a new target
// by a one-pole glide of Smoothing Time: 63 % of the way in one time
// constant, all of it in a few.
#[test]
fn smoothing_time_is_the_one_pole_glide_to_a_new_note() {
    // A quick slide, as a singer changes note without a break.
    let step = 48_000;
    let f0 = |i: usize| {
        let x = ((i as f64 - step as f64) / 1_440.0).clamp(0.0, 1.0);
        hz(48.0 + 2.0 * x)
    };
    let input = voice(f0, 1.0, 3 * 48_000);
    let mut rig = Rig::new(&[(SMOOTH, 1.0), (SMOOTHING_TIME, 100.0), (STRENGTH, 100.0)]);
    let latency = rig.latency();
    let out = rig.play(&input);
    let track: Vec<(f64, f64)> = pitches(&out)
        .into_iter()
        .map(|(at, f)| ((at as f64 - latency as f64 - step as f64) / HZ, note_of(f)))
        .collect();
    let near = |t: f64| {
        track
            .iter()
            .min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()))
            .unwrap()
            .1
    };
    for (t, wanted) in [(0.1, 49.26), (0.2, 49.73), (0.5, 49.99)] {
        let heard = near(t);
        assert!(
            (heard - wanted).abs() < 0.3,
            "{t} s after the step: {heard:.2}, wanted {wanted}"
        );
    }
    assert!(near(0.5) > near(0.1), "it keeps rising");
    // Smooth off: the voice is on the new note as soon as it is heard.
    let mut rig = Rig::new(&[(SMOOTH, 0.0), (SMOOTHING_TIME, 100.0)]);
    let out = rig.play(&input);
    let heard = note_of(pitch_between(
        &out,
        latency + step + 6_000,
        latency + step + 20_000,
    ));
    assert!((heard - 50.0).abs() < 0.05, "{heard:.3}");
}

// Spec: auto-shift R5, R7, R8. The scale is Root and Scale's, the included
// notes' keys leave notes out or bring them in, and the Quantizer's Pitch
// Shift moves each target degrees along the scale.
#[test]
fn the_root_and_scale_the_included_notes_and_the_degrees_choose_the_target() {
    let near = |note: f64, params: &[(ParamId, f64)]| {
        let mut all = vec![(SMOOTH, 0.0)];
        all.extend_from_slice(params);
        corrected(note, &all)
    };
    // C major: C# (49) is out, so 48.4 goes down to C.
    assert!((near(48.4, &[]) - 48.0).abs() < 0.05);
    // Key C# brings it in.
    assert!((near(48.8, &[(ParamId(KEY_C.0 + 1), 1.0)]) - 49.0).abs() < 0.05);
    // Key D leaves D (50) out: 50.2 goes to C or E, whichever is nearer.
    let without_d = near(50.7, &[(ParamId(KEY_C.0 + 2), 1.0)]);
    assert!((without_d - 52.0).abs() < 0.05, "{without_d:.3}");
    // D major: C# (49) and D (50) are in, C (48) isn't.
    let d_major = near(48.2, &[(ROOT, 2.0), (SCALE, 0.0)]);
    assert!((d_major - 49.0).abs() < 0.05, "{d_major:.3}");
    // A minor pentatonic: A, C, D, E and G.
    let names: Vec<&str> = ScaleName::ALL.iter().map(|n| n.label()).collect();
    let pentatonic = names
        .iter()
        .position(|n| n.contains("Minor Pentatonic"))
        .unwrap();
    let a_minor = near(50.4, &[(ROOT, 9.0), (SCALE, pentatonic as f64)]);
    assert!((a_minor - 50.0).abs() < 0.05, "{a_minor:.3}");
    let a_minor = near(51.2, &[(ROOT, 9.0), (SCALE, pentatonic as f64)]);
    assert!((a_minor - 52.0).abs() < 0.05, "{a_minor:.3}");
    // Degrees along C major: +2 from C is E, -1 is B.
    assert!((near(48.1, &[(DEGREES, 2.0)]) - 52.0).abs() < 0.05);
    assert!((near(48.1, &[(DEGREES, -1.0)]) - 47.0).abs() < 0.05);
    assert!((near(48.1, &[(DEGREES, 7.0)]) - 60.0).abs() < 0.05);
}

// Spec: auto-shift R5, R18. With Use Current Scale on, the target is the
// clip's scale on the frame it changes; a held note keeps its target until
// the next note begins.
#[test]
fn use_current_scale_follows_the_clip_and_a_held_note_keeps_its_target() {
    let change = 70_000;
    let d_major = Scale::new(2, ScaleName::Major);
    let scale = |at: usize| {
        if at < change {
            Scale::new(0, ScaleName::Major)
        } else {
            d_major
        }
    };
    // C held across the change, then a gap, then C again.
    let mut input = held(48.0, 110_000);
    input.extend(std::iter::repeat_n(0.0, 12_000));
    input.extend(held(48.0, 60_000));
    let mut rig = Rig::new(&[(USE_SCALE, 1.0), (SMOOTH, 0.0)]);
    let latency = rig.latency();
    let out = rig.render(&input, 512, &[], &[], scale).swap_remove(0);
    // C is in C major, and stays through the change in D major.
    let first = note_of(pitch_between(&out, latency + 20_000, latency + 60_000));
    let held_on = note_of(pitch_between(
        &out,
        latency + change + 6_000,
        latency + 105_000,
    ));
    assert!((first - 48.0).abs() < 0.05, "{first:.3}");
    assert!(
        (held_on - 48.0).abs() < 0.05,
        "held across the change: {held_on:.3}"
    );
    // The next note is corrected to D major: C isn't in it; B (47) is the
    // lower of the two nearest.
    let from = 110_000 + 12_000 + latency + 25_000;
    let next = note_of(pitch_between(&out, from, from + 25_000));
    assert!((next - 47.0).abs() < 0.05, "the new note: {next:.3}");
}

// Spec: auto-shift R9. The notes of the MIDI source set the pitch, each on
// its frame: before the note the voice passes as it is, exactly; after it the
// voice is at the note's pitch.
#[test]
fn a_midi_note_sets_the_pitch_on_its_frame() {
    let params = [(MIDI_ON, 1.0), (ATTACK, 0.0), (RELEASE, 0.0)];
    let input = held(48.0, 120_000);
    let mut rig = Rig::new(&params);
    let latency = rig.latency();
    let (start, end) = (40_000, 80_000);
    let out = rig
        .render(&input, 512, &[], &[on(start, 55), off(end, 55)], |_| {
            Scale::default()
        })
        .swap_remove(0);
    for i in latency..start + latency {
        assert_eq!(out[i], input[i - latency], "sample {i}");
    }
    let differs = (start + latency..start + latency + 600).any(|i| out[i] != input[i - latency]);
    assert!(differs, "the voice changes on the note's frame");
    let heard = pitch_between(&out, start + latency + 8_000, end + latency - 2_000);
    assert!(
        cents(heard, hz(55.0)).abs() < 5.0,
        "{heard:.2} Hz, wanted {:.2}",
        hz(55.0)
    );
    // After the note-off the release is 0: the dry voice again.
    let after = pitch_between(&out, end + latency + 8_000, 119_000);
    assert!(cents(after, hz(48.0)).abs() < 5.0);
}

// Spec: auto-shift R9. Mono glides to the next note over Glide when the
// notes overlap, and goes back to the one still held when the newest ends.
#[test]
fn mono_glides_between_overlapping_notes_and_returns_to_the_held_one() {
    let params = [
        (MIDI_ON, 1.0),
        (ATTACK, 0.0),
        (RELEASE, 0.0),
        (GLIDE, 400.0),
    ];
    let input = held(48.0, 160_000);
    let mut rig = Rig::new(&params);
    let latency = rig.latency();
    let notes = [
        on(10_000, 48),
        on(50_000, 60),
        off(100_000, 60),
        off(130_000, 48),
    ];
    let out = rig
        .render(&input, 512, &[], &notes, |_| Scale::default())
        .swap_remove(0);
    let track: Vec<(usize, f64)> = pitches(&out);
    let at = |frame: usize| {
        let (_, f) = track
            .iter()
            .min_by_key(|(a, _)| a.abs_diff(frame + latency))
            .unwrap();
        note_of(*f)
    };
    // Halfway through the 400 ms glide the pitch is halfway up the octave.
    assert!(
        (at(50_000 + 9_600) - 54.0).abs() < 1.0,
        "{:.2}",
        at(50_000 + 9_600)
    );
    assert!((at(50_000 + 30_000) - 60.0).abs() < 0.1);
    // The newest note ends: back to C3, gliding.
    assert!((at(125_000) - 48.0).abs() < 0.1, "{:.2}", at(125_000));
}

/// A sine at 196 Hz heard as a voice, and the amplitudes at the pitches of
/// `keys` in `out` between `from` and `to`.
fn chord(
    params: &[(ParamId, f64)],
    notes: &[NoteEvent],
    keys: &[f64],
    from: usize,
    to: usize,
) -> Vec<f64> {
    let mut rig = Rig::new(params);
    let out = rig
        .render(&sine(hz(55.0), 0.5, 120_000), 512, &[], notes, |_| {
            Scale::default()
        })
        .swap_remove(0);
    keys.iter()
        .map(|&k| amplitude(&out[from..to], hz(k)))
        .collect()
}

// Spec: auto-shift R9. Poly makes a copy of the voice for each held note,
// and a note past Voice Count takes the oldest voice's place.
#[test]
fn poly_voices_come_from_the_pool_and_the_oldest_is_stolen() {
    let base = [
        (MIDI_ON, 1.0),
        (MONO_POLY, 1.0),
        (ATTACK, 0.0),
        (RELEASE, 0.0),
    ];
    let notes = |count: usize| -> Vec<NoteEvent> {
        // Five notes, one a quarter second.
        [57u8, 60, 62, 64, 67][..count]
            .iter()
            .enumerate()
            .map(|(n, key)| on(30_000 + n * 3_000, *key))
            .collect()
    };
    let keys = [57.0, 60.0, 62.0, 64.0, 67.0];
    let heard = |voices: f64, count: usize| {
        let mut params = base.to_vec();
        params.push((VOICE_COUNT, voices));
        chord(&params, &notes(count), &keys, 80_000, 110_000)
    };
    // Four voices (index 1) and four notes: all four sound.
    let four = heard(1.0, 4);
    assert!(four[..4].iter().all(|a| *a > 0.1), "{four:?}");
    assert!(four[4] < 0.04, "the fifth key isn't played: {four:?}");
    // A fifth note takes the first's voice.
    let five = heard(1.0, 5);
    assert!(five[0] < 0.04, "the oldest voice was taken: {five:?}");
    assert!(five[1..].iter().all(|a| *a > 0.1), "{five:?}");
    // Two voices (index 0): only the newest two of four.
    let two = heard(0.0, 4);
    assert!(
        two[2] > 0.1 && two[3] > 0.1 && two[0] < 0.04 && two[1] < 0.04,
        "{two:?}"
    );
}

// Spec: auto-shift R10. Note Latch holds a voice's pitch after the note-off
// until the next note; without it the voice ends with the note.
#[test]
fn note_latch_holds_the_pitch_until_the_next_note() {
    let run = |latch: f64| {
        let params = [
            (MIDI_ON, 1.0),
            (ATTACK, 0.0),
            (RELEASE, 0.0),
            (NOTE_LATCH, latch),
        ];
        let notes = [on(20_000, 57), off(50_000, 57), on(100_000, 62)];
        chord(&params, &notes, &[55.0, 57.0, 62.0], 70_000, 95_000)
    };
    let latched = run(1.0);
    assert!(
        latched[1] > 0.1,
        "the latched note still sounds: {latched:?}"
    );
    let free = run(0.0);
    assert!(free[1] < 0.04, "{free:?}");
    assert!(
        free[0] > 0.1,
        "with no note the voice passes as it is: {free:?}"
    );
}

// Spec: auto-shift R10. A voice fades in over Attack Time and out over Release
// Time, and the pitch bend moves it by Pitch Bend Range.
#[test]
fn attack_release_and_the_pitch_bend_shape_a_midi_voice() {
    let params = [(MIDI_ON, 1.0), (ATTACK, 500.0), (RELEASE, 500.0)];
    let notes = [on(20_000, 62), off(70_000, 62)];
    let mut rig = Rig::new(&params);
    let out = rig
        .render(&sine(hz(55.0), 0.5, 160_000), 512, &[], &notes, |_| {
            Scale::default()
        })
        .swap_remove(0);
    let level = |from: usize| amplitude(&out[from..from + 2_000], hz(62.0));
    let latency = rig.latency();
    let early = level(20_000 + latency + 2_000);
    let late = level(20_000 + latency + 28_000);
    let fading = level(70_000 + latency + 12_000);
    assert!(early < 0.35 * late, "fade in: {early:.3} then {late:.3}");
    assert!(
        fading < 0.8 * late && fading > 0.02,
        "fade out: {fading:.3}"
    );
    // A bend of +1 at 5 semitones range puts the note 5 above.
    let bend = NoteEvent {
        kind: NoteKind::PitchBend { value: 1.0 },
        key: 0,
        ..on(30_000, 0)
    };
    let heard = chord(
        &[
            (MIDI_ON, 1.0),
            (ATTACK, 0.0),
            (RELEASE, 0.0),
            (BEND_RANGE, 5.0),
        ],
        &[on(20_000, 62), bend],
        &[62.0, 67.0],
        80_000,
        110_000,
    );
    assert!(heard[1] > 0.2 && heard[0] < 0.04, "{heard:?}");
}

// Spec: auto-shift R11. A target takes one source and a depth: Velocity to
// Volume scales the voice, Pitch Bend to Pitch moves it, a source to Pan puts
// it to one side.
#[test]
fn the_midi_tab_s_routings_move_pitch_volume_and_pan() {
    let base = [(MIDI_ON, 1.0), (ATTACK, 0.0), (RELEASE, 0.0)];
    let notes = [on(20_000, 62)];
    let level = |extra: &[(ParamId, f64)]| {
        let mut params = base.to_vec();
        params.extend_from_slice(extra);
        chord(&params, &notes, &[62.0], 80_000, 110_000)[0]
    };
    let plain = level(&[]);
    let quieter = level(&[(VOLUME_SOURCE, 0.0), (VOLUME_DEPTH, 100.0)]);
    // Velocity 0.8 at a depth of 100 %: the level is 0.8 of the plain one.
    assert!(
        (quieter / plain - 0.8).abs() < 0.05,
        "{quieter:.3} / {plain:.3}"
    );
    let flipped = level(&[(VOLUME_SOURCE, 0.0), (VOLUME_DEPTH, -100.0)]);
    assert!(
        (flipped / plain - 0.2).abs() < 0.05,
        "{flipped:.3} / {plain:.3}"
    );
    // Mod Wheel to Pitch at 50 %: 6 semitones above the note at a full wheel.
    let wheel = NoteEvent {
        kind: NoteKind::Control {
            controller: 1,
            value: 1.0,
        },
        key: 0,
        ..on(10_000, 0)
    };
    let mut params = base.to_vec();
    params.extend([(PITCH_SOURCE, 2.0), (PITCH_DEPTH, 50.0)]);
    let heard = chord(
        &params,
        &[wheel, on(20_000, 62)],
        &[62.0, 68.0],
        80_000,
        110_000,
    );
    assert!(heard[1] > 0.2 && heard[0] < 0.04, "{heard:?}");
    // Velocity to Pan at 100 %: 0.8 to the right.
    let mut rig = Rig::new(&[
        (MIDI_ON, 1.0),
        (ATTACK, 0.0),
        (RELEASE, 0.0),
        (PAN_SOURCE, 0.0),
        (PAN_DEPTH, 100.0),
    ]);
    let out = rig.render(&sine(hz(55.0), 0.5, 120_000), 512, &[], &notes, |_| {
        Scale::default()
    });
    let (left, right) = (rms(&out[0][80_000..110_000]), rms(&out[1][80_000..110_000]));
    assert!(right > 3.0 * left, "{left:.3} left, {right:.3} right");
}

/// The pitch track of `x` as (seconds, cents from the median), from the
/// tracker's measurements between `from` and `to` (samples).
fn cents_track(x: &[f32], from: usize, to: usize) -> Vec<(f64, f64)> {
    let heard: Vec<(usize, f64)> = pitches(x)
        .into_iter()
        .filter(|(at, _)| (from..to).contains(at))
        .collect();
    let mut sorted: Vec<f64> = heard.iter().map(|(_, f)| *f).collect();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    heard
        .iter()
        .map(|(at, f)| (*at as f64 / HZ, cents(*f, median)))
        .collect()
}

/// The span of `track` from its lowest to its highest, in cents.
fn span(track: &[(f64, f64)]) -> f64 {
    let low = track.iter().map(|t| t.1).fold(f64::MAX, f64::min);
    let high = track.iter().map(|t| t.1).fold(f64::MIN, f64::max);
    high - low
}

/// How many times per second `track` rises through its mid-line.
fn rate(track: &[(f64, f64)]) -> f64 {
    let mid = track.iter().map(|t| t.1).sum::<f64>() / track.len() as f64;
    let rises = track
        .windows(2)
        .filter(|w| w[0].1 < mid && w[1].1 >= mid)
        .count();
    rises as f64 / (track.last().unwrap().0 - track[0].0)
}

/// A steady voice, heard without correction (strength 0), with `params`.
fn uncorrected(params: &[(ParamId, f64)], len: usize) -> (Vec<f32>, usize) {
    let mut all = vec![(STRENGTH, 0.0)];
    all.extend_from_slice(params);
    let mut rig = Rig::new(&all);
    let latency = rig.latency();
    (rig.play(&held(48.5, len)), latency)
}

// Spec: auto-shift R14. The vibrato is a sine of Vibrato Amount from its
// lowest pitch to its highest, at Vibrato Rate.
#[test]
fn the_vibrato_has_its_depth_and_rate() {
    for (amount, speed) in [(100.0, 6.0), (60.0, 4.0), (200.0, 9.0)] {
        let (out, latency) = uncorrected(
            &[
                (VIBRATO_AMOUNT, amount),
                (VIBRATO_RATE, speed),
                (VIBRATO_FADE, 0.0),
            ],
            120_000,
        );
        let track = cents_track(&out, latency + 30_000, 120_000);
        let ptp = span(&track);
        assert!(
            (ptp - amount).abs() < 0.25 * amount,
            "{amount} cents at {speed} Hz: span {ptp:.0}"
        );
        let heard = rate(&track);
        assert!((heard / speed - 1.0).abs() < 0.08, "{speed} Hz: {heard:.2}");
    }
}

// Spec: auto-shift R14. The vibrato fades in over Vibrato Fade In from the
// start of the voice; Natural Vibrato wanders the rate and depth by no more
// than 10 %, the same every run.
#[test]
fn the_vibrato_fades_in_and_wanders_when_natural() {
    let (out, latency) = uncorrected(
        &[
            (VIBRATO_AMOUNT, 100.0),
            (VIBRATO_FADE, 1_500.0),
            (VIBRATO_RATE, 6.0),
        ],
        200_000,
    );
    let early = span(&cents_track(&out, latency + 2_000, latency + 14_000));
    let late = span(&cents_track(&out, latency + 100_000, 200_000));
    assert!(early < 0.5 * late, "{early:.0} then {late:.0} cents");
    // Natural: the rate stays within 10 % of 6 Hz (and is not exactly it).
    let (a, latency) = uncorrected(
        &[
            (VIBRATO_AMOUNT, 100.0),
            (VIBRATO_FADE, 0.0),
            (NATURAL_VIBRATO, 1.0),
        ],
        200_000,
    );
    let natural = rate(&cents_track(&a, latency + 30_000, 200_000));
    assert!((natural / 5.5 - 1.0).abs() < 0.12, "{natural:.2} Hz");
    let (b, _) = uncorrected(
        &[
            (VIBRATO_AMOUNT, 100.0),
            (VIBRATO_FADE, 0.0),
            (NATURAL_VIBRATO, 1.0),
        ],
        200_000,
    );
    assert_eq!(a, b, "the same device renders the same twice");
}

/// A voice that sings for `on` frames, is quiet for `gap`, and sings again.
fn two_onsets(on: usize, gap: usize, note: f64) -> Vec<f32> {
    let mut x = held(note, on);
    x.extend(std::iter::repeat_n(0.0, gap));
    x.extend(held(note, on));
    x
}

// Spec: auto-shift R12. The LFO moves the pitch by its depth at its rate,
// free in milliseconds or synced to the tempo.
#[test]
fn the_lfo_moves_the_pitch_at_its_rate_free_or_synced() {
    let run = |params: &[(ParamId, f64)]| {
        let (out, latency) = uncorrected(params, 160_000);
        cents_track(&out, latency + 30_000, 160_000)
    };
    // 50 % of 12 semitones is 6, a sine from −600 to +600 cents.
    let free = run(&[(LFO_PITCH, 50.0), (LFO_RATE, 400.0)]);
    assert!((span(&free) - 1_200.0).abs() < 250.0, "{:.0}", span(&free));
    assert!(
        (rate(&free) / 2.5 - 1.0).abs() < 0.08,
        "{:.2} Hz",
        rate(&free)
    );
    // Synced to 1/4 at the default 120 beats a minute: a cycle in half a second.
    let quarter = auto_shift::division_names()
        .iter()
        .position(|n| *n == "1/4")
        .unwrap();
    let synced = run(&[
        (LFO_PITCH, 50.0),
        (LFO_SYNC, 1.0),
        (LFO_DIVISION, quarter as f64),
    ]);
    assert!(
        (rate(&synced) / 2.0 - 1.0).abs() < 0.08,
        "{:.2} Hz",
        rate(&synced)
    );
    // A negative depth turns it over. A saw up rises through each cycle,
    // which starts every second on the output's frame 0.
    let latency = Rig::new(&[]).latency() as f64 / HZ;
    let slope = |depth: f64| {
        let (out, _) = uncorrected(
            &[(LFO_PITCH, depth), (LFO_RATE, 1_000.0), (LFO_WAVE, 4.0)],
            200_000,
        );
        let points: Vec<(f64, f64)> = cents_track(&out, 40_000, 200_000)
            .into_iter()
            .map(|(t, c)| ((t - latency).rem_euclid(1.0), c))
            .filter(|(phase, _)| (0.05..0.9).contains(phase))
            .collect();
        let n = points.len() as f64;
        let (mx, my) = (
            points.iter().map(|p| p.0).sum::<f64>() / n,
            points.iter().map(|p| p.1).sum::<f64>() / n,
        );
        let (mut xy, mut xx, mut yy) = (0.0, 0.0, 0.0);
        for (x, y) in points {
            xy += (x - mx) * (y - my);
            xx += (x - mx).powi(2);
            yy += (y - my).powi(2);
        }
        xy / (xx * yy).sqrt()
    };
    assert!(slope(10.0) > 0.8, "{:.2}", slope(10.0));
    assert!(slope(-10.0) < -0.8, "{:.2}", slope(-10.0));
}

// Spec: auto-shift R12. LFO Delay holds the LFO at zero and LFO Attack
// fades it in, from the start and after each reset.
#[test]
fn the_lfo_s_delay_and_attack_hold_it_at_zero_and_fade_it_in() {
    let (out, latency) = uncorrected(
        &[
            (LFO_PITCH, 50.0),
            (LFO_RATE, 200.0),
            (LFO_DELAY, 800.0),
            (LFO_ATTACK, 800.0),
        ],
        200_000,
    );
    let at = |from: f64, to: f64| {
        span(&cents_track(
            &out,
            latency + (from * HZ) as usize,
            latency + (to * HZ) as usize,
        ))
    };
    let held_still = at(0.2, 0.7);
    let rising = at(0.9, 1.3);
    let full = at(2.5, 3.5);
    assert!(held_still < 60.0, "during the delay: {held_still:.0} cents");
    assert!(
        rising > held_still && rising < full,
        "{held_still:.0} {rising:.0} {full:.0}"
    );
    assert!(full > 900.0, "{full:.0}");
}

// Spec: auto-shift R12. LFO Reset restarts the LFO at each onset the tracker
// reports, with its lamp lit on that frame.
#[test]
fn lfo_reset_restarts_the_lfo_at_each_onset() {
    // A 1.2 s cycle (a sine, from zero rising) and two onsets 1.7 s apart:
    // the second falls a third of a cycle later in it.
    let input = two_onsets(48_000, 33_600, 48.5);
    let at_onset = |reset: f64| {
        let mut rig = Rig::new(&[
            (STRENGTH, 0.0),
            (LFO_PITCH, 50.0),
            (LFO_RATE, 1_200.0),
            (LFO_RESET, reset),
        ]);
        let latency = rig.latency();
        let out = rig.play(&input);
        // The pitch 150 ms into each voice, in cents from the voice's own.
        let note = |from: usize| {
            let f = pitch_between(&out, from + latency + 6_000, from + latency + 9_600);
            cents(f, hz(48.5))
        };
        (note(0), note(48_000 + 33_600))
    };
    let (first, second) = at_onset(1.0);
    assert!(
        (first - second).abs() < 120.0,
        "with Reset the two onsets start alike: {first:.0} and {second:.0}"
    );
    let (first, second) = at_onset(0.0);
    assert!(
        (first - second).abs() > 400.0,
        "without it they differ: {first:.0} and {second:.0}"
    );
}

// Spec: auto-shift R12. The LFO's Volume depth moves the level.
#[test]
fn the_lfo_s_volume_depth_tremolos_the_voice() {
    let level = |depth: f64| {
        let mut rig = Rig::new(&[(STRENGTH, 0.0), (LFO_VOLUME, depth), (LFO_RATE, 400.0)]);
        let out = rig.play(&held(48.5, 100_000));
        let windows: Vec<f64> = out[30_000..90_000].chunks(2_400).map(rms).collect();
        let high = windows.iter().copied().fold(f64::MIN, f64::max);
        let low = windows.iter().copied().fold(f64::MAX, f64::min);
        low / high
    };
    assert!(level(0.0) > 0.9);
    assert!(level(100.0) < 0.5, "{:.2}", level(100.0));
}

/// The levels, in decibels, of the harmonics of a voice of `f0` hertz in the
/// last 8,192 samples of `x`, from the first up to 4.5 kHz.
fn harmonic_levels(x: &[f32], f0: f64) -> Vec<f64> {
    let frame = &x[x.len() - 8_192..];
    (1..=(4_500.0 / f0) as usize)
        .map(|k| {
            let at = k as f64 * f0;
            let loudest = (-3..=3)
                .map(|n| amplitude(frame, at * (1.0 + 0.0005 * f64::from(n))))
                .fold(0.0, f64::max);
            20.0 * loudest.max(1e-9).log10()
        })
        .collect()
}

/// How far `out`'s spectral envelope is stretched along the frequency axis
/// from the same vowel sung at the same pitch, `f0` hertz: the factor that
/// moves the plain vowel's harmonic levels nearest to `out`'s.
fn envelope_stretch(out: &[f32], f0: f64) -> f64 {
    let heard = harmonic_levels(out, f0);
    let wanted = harmonic_levels(&voice(|_| f0, 1.0, 20_000), f0);
    let first = (200.0 / f0).ceil() as usize;
    let last = (3_500.0 / f0) as usize;
    let at = |h: f64| {
        let h = h.clamp(1.0, wanted.len() as f64);
        let (lo, fraction) = (h.floor() as usize, h.fract());
        let hi = (lo + 1).min(wanted.len());
        wanted[lo - 1] * (1.0 - fraction) + wanted[hi - 1] * fraction
    };
    let cost = |s: f64| {
        let diffs: Vec<f64> = (first..=last)
            .map(|k| heard[k - 1] - at(k as f64 / s))
            .collect();
        let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
        diffs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / diffs.len() as f64
    };
    (0..=1_300)
        .map(|i| 0.7 + 0.001 * f64::from(i))
        .min_by(|a, b| cost(*a).total_cmp(&cost(*b)))
        .unwrap()
}

// Spec: auto-shift R13. Pitch Shift and its Fine move every output pitch,
// and with Formant Follow at 0 the first three formants stay within 5 %.
#[test]
fn pitch_shift_moves_the_pitch_and_leaves_the_formants() {
    let f0 = 140.0;
    let input = voice(|_| f0, 1.0, 60_000);
    for (semitones, fine) in [(5.0, 0.0), (-7.0, 0.0), (12.0, 0.0), (3.0, 30.0)] {
        let mut rig = Rig::new(&[(STRENGTH, 0.0), (PITCH_SHIFT, semitones), (FINE, fine)]);
        let latency = rig.latency();
        let out = rig.play(&input);
        let wanted = f0 * ((semitones + fine / 100.0) / 12.0).exp2();
        let heard = pitch_between(&out, latency + 30_000, 60_000);
        assert!(
            cents(heard, wanted).abs() < 5.0,
            "{semitones:+} st {fine:+} ct: {heard:.2} Hz, wanted {wanted:.2}"
        );
        let stretch = envelope_stretch(&out, heard);
        assert!(
            (stretch - 1.0).abs() < 0.05,
            "{semitones:+} st: the formants moved by {stretch:.3}"
        );
    }
}

// Spec: auto-shift R13. Formant Shift moves the formants apart from the
// pitch, and Formant Follow moves them with the pitch's shift.
#[test]
fn formant_shift_and_formant_follow_move_the_formants() {
    let f0 = 140.0;
    let input = voice(|_| f0, 1.0, 60_000);
    let run = |params: &[(ParamId, f64)]| {
        let mut all = vec![(STRENGTH, 0.0)];
        all.extend_from_slice(params);
        let mut rig = Rig::new(&all);
        let latency = rig.latency();
        let out = rig.play(&input);
        (pitch_between(&out, latency + 30_000, 60_000), out)
    };
    // Formant Shift +50 %: half an octave up (2^0.5), the pitch where it was.
    let (heard, out) = run(&[(FORMANT_SHIFT, 50.0)]);
    assert!(cents(heard, f0).abs() < 5.0, "{heard:.2}");
    let stretch = envelope_stretch(&out, heard);
    assert!((stretch / 2f64.sqrt() - 1.0).abs() < 0.05, "{stretch:.3}");
    // Down by a quarter octave.
    let (heard, out) = run(&[(FORMANT_SHIFT, -25.0)]);
    let stretch = envelope_stretch(&out, heard);
    assert!(
        (stretch / 2f64.powf(-0.25) - 1.0).abs() < 0.05,
        "{stretch:.3}"
    );
    // Formant Follow 100 % with a shift of 7 semitones: they move by 7.
    let (heard, out) = run(&[(PITCH_SHIFT, 7.0), (FORMANT_FOLLOW, 100.0)]);
    assert!(cents(heard, f0 * 2f64.powf(7.0 / 12.0)).abs() < 5.0);
    let stretch = envelope_stretch(&out, heard);
    assert!(
        (stretch / 2f64.powf(7.0 / 12.0) - 1.0).abs() < 0.05,
        "{stretch:.3}"
    );
    // And half of it at 50 %.
    let (heard, out) = run(&[(PITCH_SHIFT, 7.0), (FORMANT_FOLLOW, 50.0)]);
    let stretch = envelope_stretch(&out, heard);
    assert!(
        (stretch / 2f64.powf(3.5 / 12.0) - 1.0).abs() < 0.05,
        "{stretch:.3}"
    );
}

// Spec: auto-shift R15. Dry/Wet balances the delayed dry voice against the
// corrected one: at 50 % the corrected voice doubles the dry one.
#[test]
fn dry_wet_doubles_the_voice_at_50_percent() {
    // A little under C#: C is the nearest note of C major.
    let input = sine(hz(48.9), 0.5, 100_000);
    let at = |wet: f64| {
        let mut rig = Rig::new(&[(SMOOTH, 0.0), (DRY_WET, wet)]);
        let out = rig.play(&input);
        let tail = &out[50_000..90_000];
        (amplitude(tail, hz(48.9)), amplitude(tail, hz(48.0)))
    };
    let (dry, corrected) = at(0.0);
    assert!(dry > 0.45 && corrected < 0.02, "{dry:.3} {corrected:.3}");
    let (dry, corrected) = at(100.0);
    assert!(dry < 0.02 && corrected > 0.4, "{dry:.3} {corrected:.3}");
    let (dry, corrected) = at(50.0);
    assert!(
        (dry - 0.25).abs() < 0.04 && (corrected - 0.25).abs() < 0.05,
        "{dry:.3} {corrected:.3}"
    );
}

// Spec: auto-shift R16, R17. The display reads the detected pitch, the
// corrected one, the correction, the latency and whether the frame is voiced,
// headless, and draws two traces against the scale's keys.
#[test]
fn the_display_reads_the_detected_and_corrected_pitch() {
    let kind = auto_shift_type();
    let mut device = kind.device(id(1));
    device.params.insert(SMOOTH, 0.0);
    // D# is brought into the scale.
    device.params.insert(ParamId(KEY_C.0 + 3), 1.0);
    let config = NodeConfig {
        round_trip_latency: 0,
        sample_rate: RATE,
        max_frames: 512,
        channels: 2,
    };
    let mut node = common::registry().create(&device, &config).unwrap();
    let mut reader = node.display().expect("the device has a display");
    let input = held(48.6, 96_000);
    for part in input.chunks(512) {
        let mut data: Vec<f32> = (0..2).flat_map(|_| part.to_vec()).collect();
        let mut sent = NoteBuffer::with_capacity(8);
        let mut block = Block::new(
            &mut data,
            2,
            part.len(),
            part.len(),
            &[],
            TransportInfo::default(),
            RATE,
        )
        .with_notes(&[], &mut sent);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    let data = reader.read().values.clone();
    let display = auto_shift::display(id(1), &kind.description, &device.params, Some(&data));
    assert!(display.live);
    let reading = |name: &str| display.reading(name).unwrap_or_else(|| panic!("no {name}"));
    assert_eq!(reading("Voiced").value, 1.0);
    assert!(
        (f64::from(reading("Input Pitch").value) - 48.6).abs() < 0.05,
        "{}",
        reading("Input Pitch").text
    );
    assert_eq!(reading("Input Pitch").text, "C#2 -40 ct");
    assert!((f64::from(reading("Target Pitch").value) - 48.0).abs() < 0.05);
    assert_eq!(reading("Target Pitch").text, "C2 +0 ct");
    assert!(
        (f64::from(reading("Correction").value) + 60.0).abs() < 6.0,
        "{}",
        reading("Correction").text
    );
    let latency = Rig::new(&[]).latency() as f64 / HZ * 1_000.0;
    assert!((f64::from(reading("Latency").value) - latency).abs() < 0.01);
    // Mid is the range, and 130 Hz is in High, Mid and Bass alike.
    assert_eq!(reading("Mid Range").value, 1.0);
    // Two traces over four seconds, the newest at the right.
    let (detected, corrected) = (
        display.series(auto_shift::DETECTED).unwrap(),
        display.series(auto_shift::CORRECTED).unwrap(),
    );
    assert_eq!(detected.values.len(), auto_shift::COLUMNS);
    assert!((f64::from(*detected.values.last().unwrap()) - 48.6).abs() < 0.05);
    assert!((f64::from(*corrected.values.last().unwrap()) - 48.0).abs() < 0.05);
    assert_eq!(detected.values[0], 23.0, "no pitch before the voice");
    // The ruler marks the scale: C is in C major, C# isn't (key 60 is C3).
    assert!(display.markers.iter().any(|m| m.name == "Scale C3"));
    assert!(!display.markers.iter().any(|m| m.name == "Scale C#3"));
    assert!(
        display.markers.iter().any(|m| m.name == "Scale D#3"),
        "the included notes"
    );
    // Without an engine it holds what the parameters give.
    let idle = auto_shift::display(id(1), &kind.description, &device.params, None);
    assert!(!idle.live && idle.readings.is_empty());
    assert!(idle.markers.iter().any(|m| m.name == "Scale G3"));
}

// Spec: auto-shift R19. A render is bit-identical at every block size and run
// after run, with the LFO's random wave, Natural Vibrato, MIDI notes, a
// scale that changes and parameters that move.
#[test]
fn renders_are_identical_at_block_sizes_1_64_512_and_4096() {
    let input = two_onsets(30_000, 9_000, 48.4);
    let params = [
        (USE_SCALE, 1.0),
        (MIDI_ON, 0.0),
        (LFO_PITCH, 10.0),
        (LFO_WAVE, 7.0),
        (LFO_RATE, 300.0),
        (LFO_RESET, 1.0),
        (VIBRATO_AMOUNT, 80.0),
        (NATURAL_VIBRATO, 1.0),
    ];
    let events = [
        (20_000, STRENGTH, 60.0),
        (45_000, MIDI_ON, 1.0),
        (70_000, SMOOTH, 0.0),
    ];
    let notes = [on(48_000, 55), off(60_000, 55), on(62_000, 52)];
    let scale = |at: usize| {
        if at < 40_000 {
            Scale::new(0, ScaleName::Major)
        } else {
            Scale::new(2, ScaleName::Minor)
        }
    };
    let render = |block: usize, id: u64| {
        Rig::with_id(id, &params).render(&input, block, &events, &notes, scale)
    };
    let first = render(1, 1);
    for block in BLOCKS {
        assert_eq!(render(block, 1), first, "block of {block}");
    }
    assert_ne!(
        render(512, 2),
        first,
        "another device's random wave differs"
    );
}

// Spec: auto-shift R23. A set with the device saves and reopens with every
// parameter, the included notes among them, and its MIDI source.
#[test]
fn a_set_with_auto_shift_saves_and_reopens_with_every_parameter_and_its_midi_source() {
    use bl_model::{Command, Document, Editor, Tap, TrackKind};
    let mut editor = Editor::new();
    let mut track = |kind| {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        id
    };
    let (audio, keys) = (track(TrackKind::Audio), track(TrackKind::Midi));
    let kind = auto_shift_type();
    let device_id = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track: audio,
            index: 0,
            device: kind.device(device_id),
        })
        .unwrap();
    let mut moved = Vec::new();
    for p in &kind.description.params {
        let value = p.clamp(p.min() + 0.3 * (p.max() - p.min()));
        editor
            .apply(Command::SetDeviceParam {
                device: device_id,
                param: p.id,
                value,
            })
            .unwrap();
        moved.push((p.id, value));
    }
    editor
        .apply(Command::SetMidiSidechainSource {
            device: device_id,
            track: Some(keys),
            tap: Some(Tap::PostMixer),
        })
        .unwrap();
    let saved = serde_json::to_value(editor.document()).unwrap();
    let reopened: Document = serde_json::from_value(saved).unwrap();
    assert_eq!(&reopened, editor.document());
    let device = reopened.device(device_id).unwrap();
    for (param, value) in moved {
        assert_eq!(device.params.get(&param), Some(&value), "{param:?}");
    }
    let sidechain = device.midi_sidechain.as_ref().unwrap();
    assert_eq!(
        (sidechain.source, sidechain.tap),
        (Some(keys), Some(Tap::PostMixer))
    );
}

// Spec: auto-shift R3. With Live Mode on the correction still lands on a
// voice well inside its range, a latency that is half as long.
#[test]
fn live_mode_halves_the_latency_and_still_corrects_a_voice_in_its_range() {
    let params = [(LIVE_MODE, 1.0), (SMOOTH, 0.0), (PITCH_RANGE, 0.0)];
    let mut rig = Rig::new(&params);
    let latency = rig.latency();
    let full = Rig::new(&[(PITCH_RANGE, 0.0)]).latency();
    assert!(latency.abs_diff(full / 2) <= 1);
    // 196 Hz, G3 in the High range, a little sharp.
    let out = rig.play(&held(55.4, 60_000));
    let heard = note_of(pitch_between(&out, latency + 30_000, 60_000));
    assert!((heard - 55.0).abs() < 0.1, "{heard:.3}");
}

// Spec: auto-shift R9. A voice can be moved two octaves either way.
#[test]
fn a_midi_note_two_octaves_from_the_voice_is_reached() {
    // A voice at G (key 55): two octaves up is key 79, two down is key 31.
    for key in [79u8, 31] {
        let mut rig = Rig::new(&[(MIDI_ON, 1.0), (ATTACK, 0.0), (RELEASE, 0.0)]);
        let latency = rig.latency();
        let out = rig
            .render(&held(55.0, 120_000), 512, &[], &[on(20_000, key)], |_| {
                Scale::default()
            })
            .swap_remove(0);
        let heard = pitch_between(&out, latency + 50_000, 110_000);
        assert!(
            cents(heard, hz(f64::from(key))).abs() < 10.0,
            "key {key}: {heard:.2} Hz, wanted {:.2}",
            hz(f64::from(key))
        );
    }
}

// Spec: auto-shift R2, R21. Input Gain sets the level into the tracker and the
// shifter, from silence up, and leaves the dry signal alone.
#[test]
fn input_gain_sets_the_level_into_the_shifter_and_not_the_dry_signal() {
    let input = held(48.6, 60_000);
    let level = |wet: f64, gain: f64| {
        let mut rig = Rig::new(&[(STRENGTH, 0.0), (DRY_WET, wet), (INPUT_GAIN, gain)]);
        rms(&rig.play(&input)[30_000..60_000])
    };
    let unity = level(100.0, 1.0);
    assert!((level(100.0, 0.5) / unity - 0.5).abs() < 0.05);
    assert!(level(100.0, 0.0) < 1e-6, "silence into the shifter");
    // The dry path doesn't take it.
    assert!((level(0.0, 0.5) / level(0.0, 1.0) - 1.0).abs() < 1e-6);
    let kind = auto_shift_type();
    let gain = kind.description.param(INPUT_GAIN).unwrap();
    assert!(gain.is_gain() && gain.max() > 15.0, "up to +24 dB");
}
