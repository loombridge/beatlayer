//! Operator, part 1 (WP-8.12; the Operator spec, R11 to R22): the
//! oscillators on the FM core, their envelopes and the pitch envelope with
//! loop modes and slopes, Time, the voice section's Voices, Transpose and
//! per-note bend, the banks, the factory presets and the shell's pages.
//!
//! Every block runs as a real-time section under the allocation trap. The
//! envelopes are read off a naive square wave (Square D at a fixed
//! frequency), whose every sample is the envelope's level: `|x|`.

use core::f64::consts::TAU;
use std::time::Instant;

use crate::common::{self, CHANNELS, RATE};
use bl_core::time::{Bpm, Frames, TempoMap, TempoSegment, Ticks};
use bl_devices::operator::{self, *};
use bl_devices::operator_envelope::{self as envelope, Curve, Loop, Shape};
use bl_engine::{Block, Node, NodeEvent, NoteBuffer, NoteEvent, NoteKind, TransportInfo};
use bl_model::device_layout::{PageStyle, Placed};
use bl_model::param::db_to_gain;
use bl_model::{DeviceRole, ParamId};
use lspk_voice::unison::pan_gains;

const HZ: f64 = 48_000.0;
const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

/// Key 57: 220 Hz exactly.
const A3: u8 = 57;
const F0: f64 = 220.0;

/// Square D: the naive digital square, ±1 whatever the key.
const SQUARE_D: f64 = 18.0;

fn on(offset: u32, key: u8, velocity: f32) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::On,
        channel: 0,
        key,
        velocity,
        note_id: None,
    }
}

fn off(offset: u32, key: u8) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::Off,
        channel: 0,
        key,
        velocity: 0.0,
        note_id: None,
    }
}

/// A new device's parameters but for Antialias off, Tone at 100 % and
/// Volume at 0 dB, so a test hears the oscillators' own sound; `extra`
/// overrides them.
fn patch(extra: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut params = vec![(ANTIALIAS, 0.0), (TONE, 100.0), (VOLUME, 1.0)];
    params.extend_from_slice(extra);
    params
}

/// A patch whose one carrier, A, is the naive square at a fixed 100 Hz, so
/// `|x|` is the envelope times A's level: set A's envelope in `extra`.
fn square(extra: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut params = vec![
        (osc(0, OSC_WAVE), SQUARE_D),
        (osc(0, OSC_FIXED), 1.0),
        (osc(0, OSC_FREQ), 100.0),
        (INTERPOLATION, 0.0),
    ];
    params.extend_from_slice(extra);
    patch(&params)
}

fn make_node(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device = common::device(&operator_type(), params);
    common::node(&common::registry(), &device, max_frames)
}

fn steady(bpm: f64) -> TempoMap {
    TempoMap::constant(Bpm::new(bpm).unwrap())
}

/// What a run plays and where.
struct Run {
    params: Vec<(ParamId, f64)>,
    /// The partials drawn on the device, whose user waves' tables it is sent.
    partials: Vec<bl_model::Partials>,
    notes: Vec<NoteEvent>,
    events: Vec<NodeEvent>,
    tempo: TempoMap,
    playing: bool,
    /// The song's position at the run's first frame.
    from: i64,
}

fn run(params: Vec<(ParamId, f64)>, notes: Vec<NoteEvent>) -> Run {
    Run {
        params,
        partials: Vec::new(),
        notes,
        events: Vec::new(),
        tempo: steady(120.0),
        playing: false,
        from: 0,
    }
}

impl Run {
    fn playing(mut self, tempo: TempoMap) -> Self {
        self.tempo = tempo;
        self.playing = true;
        self
    }

    fn events(mut self, events: Vec<NodeEvent>) -> Self {
        self.events = events;
        self
    }

    fn partials(mut self, osc: u8, levels: Vec<f32>) -> Self {
        self.partials.push(bl_model::Partials { osc, levels });
        self
    }

    /// `frames` frames in blocks of `block`, left and right.
    fn render(&self, frames: usize, block: usize) -> [Vec<f32>; 2] {
        // The tables the device's partials and Repeat build, sent as the
        // compiler sends them.
        let mut device = common::device(&operator_type(), &self.params);
        device.partials.clone_from(&self.partials);
        let mut node = common::node(&common::registry(), &device, block);
        let collector = bl_engine::Collector::new();
        for item in operator::device_data(&device, &common::config(block), &|_| None).items {
            node.receive_data(item(&collector.handle()));
        }
        let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
        let mut at = 0;
        while at < frames {
            let len = block.min(frames - at);
            let inside = |offset: u32| (offset as usize) >= at && (offset as usize) < at + len;
            let notes: Vec<NoteEvent> = self
                .notes
                .iter()
                .filter(|n| inside(n.offset))
                .map(|n| NoteEvent {
                    offset: n.offset - at as u32,
                    ..*n
                })
                .collect();
            let events: Vec<NodeEvent> = self
                .events
                .iter()
                .filter(|e| inside(e.offset))
                .map(|e| NodeEvent {
                    offset: e.offset - at as u32,
                    ..*e
                })
                .collect();
            let mut data = vec![0.0f32; CHANNELS * len];
            let mut sent = NoteBuffer::with_capacity(64);
            {
                let transport = TransportInfo {
                    playing: self.playing,
                    position: Frames(self.from + at as i64),
                    ..TransportInfo::default()
                };
                let mut block = Block::new(&mut data, CHANNELS, len, len, &events, transport, RATE)
                    .with_tempo(&self.tempo)
                    .with_notes(&notes, &mut sent);
                bl_rt::rt_test!({ node.process(&mut block) });
            }
            out[0].extend_from_slice(&data[..len]);
            out[1].extend_from_slice(&data[len..]);
            at += len;
        }
        out
    }

    /// The left channel, in one block as long as the run.
    fn left(&self, frames: usize) -> Vec<f32> {
        let [left, _] = self.render(frames, frames.min(4096));
        left
    }
}

fn frequency(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// The amplitude of the sine at `hz` in `signal`: a Hann-windowed DFT over
/// the whole slice.
fn amplitude_at(signal: &[f32], hz: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im, mut weights) = (0.0f64, 0.0f64, 0.0f64);
    for (i, &x) in signal.iter().enumerate() {
        let w = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
        let phase = TAU * hz * i as f64 / HZ;
        re += f64::from(x) * w * phase.cos();
        im -= f64::from(x) * w * phase.sin();
        weights += w;
    }
    2.0 * re.hypot(im) / weights
}

fn db(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1e-12).log10()
}

fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / signal.len().max(1) as f64).sqrt()
}

/// The level A's envelope has at each frame: `|x|` of the square.
fn levels(signal: &[f32]) -> Vec<f32> {
    signal.iter().map(|x| x.abs()).collect()
}

fn level(op: usize, amplitude: f64) -> (ParamId, f64) {
    (osc(op, OSC_LEVEL), amplitude)
}

fn env(control: u32, value: f64) -> (ParamId, f64) {
    (osc(0, control), value)
}

/// The share of an exponential fall left after `p` of its time, as the
/// envelope's law gives it.
fn fall(p: f64) -> f64 {
    let end = (-8.0f64).exp();
    ((-8.0 * p).exp() - end) / (1.0 - end)
}

// ---- R11: the device, the oscillators and the spectra ----

// Spec: operator R11, R19
#[test]
fn operator_is_an_instrument_with_every_control_a_parameter() {
    let kind = operator_type();
    assert_eq!(kind.role, DeviceRole::Instrument);
    kind.description.check().expect("a valid description");
    let params = &kind.description.params;
    assert!(params.len() > 100, "{} parameters", params.len());
    for op in 0..4 {
        for control in OSC_ON..=ENV_TIME_VEL {
            assert!(
                params.iter().any(|p| p.id == osc(op, control)),
                "oscillator {op}'s control {control}"
            );
        }
    }
    for base in [FILTER_ENV, PITCH_ENV, LFO_ENV] {
        for control in SHAPED_INITIAL..=SHAPED_TIME_VEL {
            assert!(params.iter().any(|p| p.id == shaped(base, control)));
        }
    }
    // Every parameter is automatable and has a description.
    assert!(
        params
            .iter()
            .all(|p| p.automatable && !p.description.is_empty())
    );
    // The per-note bend is a menu option, on by default.
    assert_eq!(kind.description.menu_options, [NOTE_PB]);
    assert_eq!(kind.description.param(NOTE_PB).unwrap().default, 1.0);
    // Time's and Tone's defaults, Voices' (R15, R9, R17).
    let default = |id| kind.description.param(id).unwrap().default;
    assert_eq!(
        (default(TIME), default(TONE), default(VOICES)),
        (0.0, 70.0, 6.0)
    );
}

// Spec: operator R11 (a new device's first oscillator sounds at its level)
#[test]
fn a_new_operator_plays_oscillator_a_as_a_sine_at_the_notes_pitch() {
    let signal = run(patch(&[]), vec![on(0, A3, 0.8)]).left(48_000);
    let heard = &signal[4_800..];
    let at = amplitude_at(heard, F0);
    assert!((at - 1.0).abs() < 0.01, "{at}");
    assert!(db(amplitude_at(heard, 2.0 * F0)) < -60.0);
}

// Spec: operator R1, R11 (an algorithm and four oscillators' spectra)
#[test]
fn an_algorithm_and_its_oscillators_play_the_bessel_spectrum() {
    // Algorithm 1, B into A: B at ratio 2.5 and amplitude 0.25 bends A by
    // an index of 1, so A's partials sit at 220 Hz and 220 ± 550k Hz with
    // the Bessel amplitudes J0(1), J1(1), J2(1) and so on.
    let params = patch(&[
        (osc(1, OSC_COARSE), 2.0),
        (osc(1, OSC_FINE), 500.0),
        level(1, 0.25),
    ]);
    let signal = run(params, vec![on(0, A3, 0.8)]).left(48_000);
    let heard = &signal[4_800..];
    let bessel = |n: i32, x: f64| {
        let steps = 2_000;
        let h = std::f64::consts::PI / f64::from(steps);
        let f = |t: f64| (f64::from(n) * t - x * t.sin()).cos();
        let mut sum = f(0.0) + f(std::f64::consts::PI);
        for i in 1..steps {
            sum += f(f64::from(i) * h) * if i % 2 == 1 { 4.0 } else { 2.0 };
        }
        sum * h / 3.0 / std::f64::consts::PI
    };
    for (n, hz) in [(0, 220.0), (1, 770.0), (1, 330.0), (2, 1320.0), (2, 880.0)] {
        let expected = bessel(n, 1.0).abs();
        let got = amplitude_at(heard, hz);
        assert!(
            (db(got) - db(expected)).abs() < 0.5,
            "sideband {n} at {hz} Hz: {got} against {expected}"
        );
    }
}

// Spec: operator R11 (four oscillators on the core)
#[test]
fn four_carriers_each_sound_at_their_own_ratio_and_a_quarter_of_their_level() {
    let params = patch(&[
        (ALGORITHM, 11.0),
        level(0, 0.5),
        level(1, 0.5),
        level(2, 0.5),
        level(3, 0.5),
        (osc(1, OSC_COARSE), 2.0),
        (osc(2, OSC_COARSE), 3.0),
        (osc(3, OSC_COARSE), 4.0),
    ]);
    let signal = run(params, vec![on(0, A3, 0.8)]).left(48_000);
    let heard = &signal[4_800..];
    for k in 1..=4 {
        let at = amplitude_at(heard, F0 * f64::from(k));
        assert!((at - 0.125).abs() < 0.002, "partial {k}: {at}");
    }
}

// Spec: operator R11 (Level < Vel's law is polysynth R15's)
#[test]
fn level_vel_scales_the_level_by_one_minus_a_plus_a_times_velocity() {
    for amount in [0.0, 0.5, 1.0] {
        for velocity in [1.0, 0.5, 0.1] {
            let params = square(&[(osc(0, OSC_LEVEL_VEL), amount * 100.0)]);
            let signal = run(params, vec![on(0, A3, velocity)]).left(4_800);
            let got = f64::from(signal[4_000].abs());
            let expected = 1.0 - amount + amount * f64::from(velocity);
            assert!(
                (got - expected).abs() < 1e-4,
                "amount {amount}, velocity {velocity}: {got} against {expected}"
            );
        }
    }
}

// Spec: operator R11 (Level < Key: up to 6 dB an octave from C3)
#[test]
fn level_key_moves_the_level_by_six_decibels_an_octave_from_c3() {
    for (amount, key, expected_db) in [
        (100.0, 72, 6.0206),
        (100.0, 48, -6.0206),
        (-100.0, 72, -6.0206),
        (50.0, 84, 6.0206),
        (100.0, 60, 0.0),
    ] {
        let params = square(&[(osc(0, OSC_LEVEL_KEY), amount), level(0, 0.25)]);
        let signal = run(params, vec![on(0, key, 0.8)]).left(4_800);
        let got = db(f64::from(signal[4_000].abs()) / 0.25);
        assert!(
            (got - expected_db).abs() < 0.01,
            "amount {amount}, key {key}: {got} dB against {expected_db}"
        );
    }
}

// ---- R12, R13: the envelopes' stages and slopes ----

// Spec: operator R12 (Attack straight in amplitude)
#[test]
fn an_oscillators_attack_is_straight_in_amplitude_from_initial_to_peak() {
    let params = square(&[
        env(ENV_INITIAL, 0.2),
        env(ENV_PEAK, 0.9),
        env(ENV_SUSTAIN, 0.9),
        env(ENV_ATTACK, 100.0),
    ]);
    let signal = levels(&run(params, vec![on(0, A3, 0.8)]).left(9_600));
    for n in [0usize, 479, 1_200, 2_399, 3_600, 4_799] {
        let expected = 0.2 + 0.7 * (n as f64 + 1.0) / 4_800.0;
        assert!(
            (f64::from(signal[n]) - expected).abs() < 1e-4,
            "frame {n}: {} against {expected}",
            signal[n]
        );
    }
    assert!((signal[6_000] - 0.9).abs() < 1e-5, "holds its peak");
}

// Spec: operator R12 (Decay exponential to Sustain)
#[test]
fn decay_is_exponential_from_peak_to_sustain_and_sustain_holds() {
    let params = square(&[
        env(ENV_ATTACK, 0.0),
        env(ENV_PEAK, 1.0),
        env(ENV_SUSTAIN, 0.25),
        env(ENV_DECAY, 200.0),
    ]);
    let signal = levels(&run(params, vec![on(0, A3, 0.8)]).left(19_200));
    // The attack takes one frame; the decay's 9,600 frames follow.
    for p in [0.05, 0.125, 0.25, 0.5, 0.9] {
        let n = (p * 9_600.0) as usize + 1;
        let expected = 0.25 + 0.75 * fall(f64::from(n as u32 - 1) / 9_600.0 + 1.0 / 9_600.0);
        assert!(
            (f64::from(signal[n]) - expected).abs() < 2e-3,
            "at {p} of the decay: {} against {expected}",
            signal[n]
        );
    }
    for later in [12_000, 15_000, 19_000] {
        assert!((signal[later] - 0.25).abs() < 1e-5, "sustain at {later}");
    }
}

// Spec: operator R12 (Release exponential to silence; the voice ends)
#[test]
fn release_falls_exponentially_to_silence_and_the_voice_ends() {
    let params = square(&[
        env(ENV_ATTACK, 0.0),
        env(ENV_SUSTAIN, 0.5),
        env(ENV_DECAY, 0.0),
        env(ENV_RELEASE, 100.0),
    ]);
    let notes = vec![on(0, A3, 0.8), off(4_800, A3)];
    let signal = levels(&run(params, notes).left(19_200));
    assert!((signal[4_799] - 0.5).abs() < 1e-5);
    for p in [0.1, 0.25, 0.5, 0.75] {
        let n = 4_800 + (p * 4_800.0) as usize;
        let expected = 0.5 * fall(f64::from(n as u32 - 4_800) / 4_800.0 + 1.0 / 4_800.0);
        assert!(
            (f64::from(signal[n]) - expected).abs() < 2e-3,
            "at {p} of the release: {} against {expected}",
            signal[n]
        );
    }
    assert_eq!(signal[9_700..].iter().fold(0.0f32, |a, b| a.max(*b)), 0.0);
}

// Spec: operator R12 (a voice ends when every carrier's envelope is silent)
#[test]
fn a_voice_lasts_until_its_slowest_carrier_has_released() {
    // Algorithm 11: A releases in 50 ms, B in 400 ms; the note ends at 0.
    let params = patch(&[
        (ALGORITHM, 11.0),
        level(1, 1.0),
        env(ENV_RELEASE, 50.0),
        (osc(1, ENV_RELEASE), 400.0),
        (osc(1, OSC_COARSE), 2.0),
    ]);
    let notes = vec![on(0, A3, 0.8), off(9_600, A3)];
    let signal = run(params, notes).left(48_000);
    let tail = |from: usize, to: usize| rms(&signal[from..to]);
    assert!(tail(9_600 + 4_800, 9_600 + 9_600) > 0.01, "B still rings");
    assert!(tail(9_600 + 24_000, 48_000) < 1e-3, "and then it is silent");
}

fn shaped_envelope(slopes: [f32; 3]) -> Shape {
    Shape {
        initial: -0.5,
        peak: 0.75,
        sustain: 0.25,
        end: -0.25,
        attack: 0.1,
        decay: 0.2,
        release: 0.3,
        curve: Curve::Slopes(slopes),
        mode: Loop::None,
        loop_secs: 0.1,
        repeat: 1.0,
    }
}

/// Steps `state` by `ticks` of 1 ms; the levels.
fn step(state: &mut envelope::State, shape: &Shape, ticks: usize) -> Vec<f32> {
    (0..ticks).map(|_| state.step(shape, 0.001, 0.0)).collect()
}

// Spec: operator R13
#[test]
fn the_shaped_envelopes_segments_bow_by_x_to_the_two_to_the_minus_two_s() {
    for slope in [-1.0f32, -0.5, 0.0, 0.5, 1.0] {
        let shape = shaped_envelope([slope; 3]);
        let bow = |p: f64| p.powf(2f64.powf(-2.0 * f64::from(slope)));
        let mut state = envelope::State::DONE;
        state.start(&shape, 0.0, None);
        // Attack: 100 ticks from Initial −0.5 to Peak 0.75.
        let attack = step(&mut state, &shape, 100);
        for (k, level) in attack.iter().enumerate() {
            let expected = -0.5 + 1.25 * bow((k + 1) as f64 / 100.0);
            assert!(
                (f64::from(*level) - expected).abs() < 1e-5,
                "slope {slope}, attack {k}: {level} against {expected}"
            );
        }
        // Decay: 200 ticks from Peak to Sustain.
        let decay = step(&mut state, &shape, 200);
        for (k, level) in decay.iter().enumerate() {
            let expected = 0.75 - 0.5 * bow((k + 1) as f64 / 200.0);
            assert!(
                (f64::from(*level) - expected).abs() < 1e-5,
                "slope {slope}, decay {k}: {level} against {expected}"
            );
        }
        assert_eq!(step(&mut state, &shape, 1), [0.25], "sustain");
        state.release(&shape);
        // Release: 300 ticks from Sustain to End −0.25.
        let release = step(&mut state, &shape, 300);
        for (k, level) in release.iter().enumerate() {
            let expected = 0.25 - 0.5 * bow((k + 1) as f64 / 300.0);
            assert!(
                (f64::from(*level) - expected).abs() < 1e-5,
                "slope {slope}, release {k}: {level} against {expected}"
            );
        }
        assert!(state.is_done());
        assert_eq!(step(&mut state, &shape, 5), [-0.25; 5], "holds End");
    }
}

// Spec: operator R13 (positive slopes start fast and end slow)
#[test]
fn a_positive_slope_starts_fast_and_a_negative_one_starts_slow() {
    let first_half = |slope: f32| {
        let shape = shaped_envelope([slope, 0.0, 0.0]);
        let mut state = envelope::State::DONE;
        state.start(&shape, 0.0, None);
        let at = step(&mut state, &shape, 50)[49];
        (f64::from(at) + 0.5) / 1.25
    };
    assert!(first_half(0.8) > 0.55, "{}", first_half(0.8));
    assert!((first_half(0.0) - 0.5).abs() < 1e-5);
    assert!(first_half(-0.8) < 0.45, "{}", first_half(-0.8));
}

// ---- R14: the loop modes ----

fn loop_shape(mode: Loop) -> Shape {
    Shape {
        initial: 0.0,
        peak: 1.0,
        sustain: 0.5,
        end: 0.0,
        attack: 0.01,
        decay: 0.02,
        release: 0.05,
        curve: Curve::Slopes([0.0; 3]),
        mode,
        loop_secs: 0.03,
        repeat: 1.0,
    }
}

// Spec: operator R14 (Loop)
#[test]
fn loop_returns_to_initial_over_loop_time_and_attacks_again_while_held() {
    let shape = loop_shape(Loop::Loop);
    let mut state = envelope::State::DONE;
    state.start(&shape, 0.0, None);
    // Attack 10 ticks, Decay 20, the sustain frame, Back 30: a cycle of 61.
    let cycle = step(&mut state, &shape, 61 * 3);
    for k in 0..3 {
        let at = |n: usize| cycle[k * 61 + n];
        assert!((at(9) - 1.0).abs() < 1e-5, "peak of cycle {k}");
        assert!((at(29) - 0.5).abs() < 1e-5, "sustain of cycle {k}");
        assert!((at(30) - 0.5).abs() < 1e-5);
        assert!((at(45) - 0.25).abs() < 0.02, "halfway back: {}", at(45));
        assert!(at(59).abs() < 0.02, "back at Initial");
    }
    // Released, it stops looping and falls.
    state.release(&shape);
    let tail = step(&mut state, &shape, 60);
    assert!(state.is_done() && tail[59].abs() < 1e-6);
}

// Spec: operator R14 (Trigger)
#[test]
fn trigger_runs_attack_and_decay_and_then_releases_whatever_the_note_does() {
    let shape = loop_shape(Loop::Trigger);
    let mut state = envelope::State::DONE;
    state.start(&shape, 0.0, None);
    let levels = step(&mut state, &shape, 200);
    assert!((levels[9] - 1.0).abs() < 1e-5, "attack");
    assert!((levels[29] - 0.5).abs() < 1e-5, "decay");
    assert!(state.is_done(), "released itself, the note still held");
    assert!(levels[100].abs() < 0.5 && levels[199].abs() < 1e-6);
    // The note's end changes nothing.
    let mut state = envelope::State::DONE;
    state.start(&shape, 0.0, None);
    step(&mut state, &shape, 5);
    state.release(&shape);
    let levels = step(&mut state, &shape, 100);
    assert!(
        (levels[24] - 1.0).abs() < 1e-5 || levels[24] > 0.4,
        "{}",
        levels[24]
    );
}

// ---- R14 through the engine's transport: Beat and Sync on the song's grid ----

/// A square whose envelope is a burst: Peak at once, fading to silence in
/// 100 ms, restarting under `mode`.
fn bursts(mode: Loop, extra: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut params = vec![
        env(ENV_ATTACK, 0.0),
        env(ENV_SUSTAIN, 0.0),
        env(ENV_DECAY, 100.0),
        env(ENV_RELEASE, 10.0),
        env(ENV_LOOP, mode as usize as f64),
        // Repeat "1": one beat.
        env(ENV_REPEAT, 4.0),
    ];
    params.extend_from_slice(extra);
    square(&params)
}

/// The frames where a burst begins: the first loud frame after silence.
fn onsets(signal: &[f32]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut quiet = true;
    for (n, x) in signal.iter().enumerate() {
        if quiet && x.abs() > 0.5 {
            found.push(n);
            quiet = false;
        }
        if x.abs() < 0.01 {
            quiet = true;
        }
    }
    found
}

fn changing() -> TempoMap {
    TempoMap::new([
        TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: Bpm::new(120.0).unwrap(),
        },
        TempoSegment::Constant {
            start: Ticks::from_quarters(2),
            bpm: Bpm::new(90.0).unwrap(),
        },
    ])
    .unwrap()
}

fn near(found: &[usize], expected: &[usize], within: usize) {
    assert_eq!(
        found.len(),
        expected.len(),
        "{found:?} against {expected:?}"
    );
    for (f, e) in found.iter().zip(expected) {
        assert!(f.abs_diff(*e) <= within, "{found:?} against {expected:?}");
    }
}

/// The frame of the song's quarter note `quarters` (a fraction of one too)
/// at `tempo`.
fn quarter_frame(tempo: &TempoMap, quarters: f64) -> usize {
    let ticks = Ticks((quarters * Ticks::from_quarters(1).0 as f64) as i64);
    tempo.ticks_to_frames(ticks, RATE).0 as usize
}

// Spec: operator R14 (Beat: every Repeat after the note's start, unquantised)
#[test]
fn beat_starts_the_envelope_again_every_repeat_after_the_note_at_any_block_size() {
    for tempo in [steady(120.0), steady(75.0), changing()] {
        let frames = 5 * 24_000;
        let notes = vec![on(7_000, A3, 0.8)];
        let make = || run(bursts(Loop::Beat, &[]), notes.clone()).playing(tempo.clone());
        let reference = make().left(frames);
        // Each restart a beat after the note's start, in the song's beats.
        let start = 7_000;
        let song_beat_of_start = tempo
            .frames_to_ticks(Frames(start as i64), RATE)
            .as_quarters();
        let expected: Vec<usize> = std::iter::once(start)
            .chain((1..).map(|k| quarter_frame(&tempo, song_beat_of_start + f64::from(k))))
            .take_while(|f| *f < frames - 6_000)
            .collect();
        near(&onsets(&reference), &expected, 3);
        for block in BLOCKS {
            assert_eq!(
                make().render(frames, block)[0],
                reference,
                "block size {block}"
            );
        }
    }
}

// Spec: operator R14 (Sync: the first restart on the next sixteenth)
#[test]
fn sync_makes_its_first_restart_on_the_songs_next_sixteenth_at_any_block_size() {
    let tempo = steady(120.0);
    // A sixteenth is 6,000 frames and a beat 24,000: the note starts 3,000
    // frames into the sixteenth that ends at 6,000.
    let frames = 4 * 24_000;
    let notes = vec![on(3_000, A3, 0.8)];
    let make = || run(bursts(Loop::Sync, &[]), notes.clone()).playing(tempo.clone());
    let reference = make().left(frames);
    near(
        &onsets(&reference),
        &[3_000, 6_000, 30_000, 54_000, 78_000],
        3,
    );
    for block in BLOCKS {
        assert_eq!(
            make().render(frames, block)[0],
            reference,
            "block size {block}"
        );
    }
    // A note exactly on a sixteenth waits for the next one.
    let on_grid = run(bursts(Loop::Sync, &[]), vec![on(12_000, A3, 0.8)])
        .playing(tempo.clone())
        .left(frames);
    near(
        &onsets(&on_grid),
        &[12_000, 18_000, 42_000, 66_000, 90_000],
        3,
    );
    // A song position inside the sixteenth moves the grid with it.
    let mut shifted = run(bursts(Loop::Sync, &[]), vec![on(1_000, A3, 0.8)]).playing(tempo.clone());
    shifted.from = 2_000;
    near(
        &onsets(&shifted.left(frames)),
        &[1_000, 4_000, 28_000, 52_000, 76_000],
        3,
    );
}

// Spec: operator R14 (Sync with the song stopped acts as Beat)
#[test]
fn sync_with_the_transport_stopped_acts_as_beat() {
    let frames = 4 * 24_000;
    let notes = vec![on(3_000, A3, 0.8)];
    let stopped = run(bursts(Loop::Sync, &[]), notes.clone()).left(frames);
    near(&onsets(&stopped), &[3_000, 27_000, 51_000, 75_000], 3);
    let beat = run(bursts(Loop::Beat, &[]), notes).left(frames);
    assert_eq!(stopped, beat);
}

// Spec: operator R14 (a released note's envelope doesn't restart)
#[test]
fn a_released_envelope_does_not_restart_on_the_beat() {
    let notes = vec![on(0, A3, 0.8), off(10_000, A3)];
    let signal = run(bursts(Loop::Beat, &[]), notes).left(4 * 24_000);
    near(&onsets(&signal), &[0], 3);
}

// Spec: operator R14 (Trigger through the device)
#[test]
fn a_trigger_envelope_ends_the_sound_though_the_note_is_held() {
    let params = square(&[
        env(ENV_ATTACK, 5.0),
        env(ENV_DECAY, 20.0),
        env(ENV_SUSTAIN, 0.0),
        env(ENV_RELEASE, 20.0),
        env(ENV_LOOP, Loop::Trigger as usize as f64),
    ]);
    let signal = levels(&run(params, vec![on(0, A3, 0.8)]).left(48_000));
    assert!(signal[240] > 0.99, "the attack");
    assert!(
        signal[5_000..].iter().all(|x| *x == 0.0),
        "silent while held"
    );
}

// ---- R15: Time ----

/// The first frame at which the levels reach `target` or more.
fn reach(levels: &[f32], target: f32) -> usize {
    levels
        .iter()
        .position(|l| *l >= target)
        .expect("it gets there")
}

// Spec: operator R15 (Time scales times by 2^(4 Time) and no level)
#[test]
fn time_scales_every_envelope_time_by_two_to_the_four_time_and_no_level() {
    let base = |time: f64| {
        let params = square(&[
            (TIME, time),
            env(ENV_INITIAL, 0.0),
            env(ENV_ATTACK, 50.0),
            env(ENV_DECAY, 100.0),
            env(ENV_SUSTAIN, 0.3),
            env(ENV_RELEASE, 80.0),
        ]);
        levels(&run(params, vec![on(0, A3, 0.8), off(60_000, A3)]).left(120_000))
    };
    let flat = base(0.0);
    for (time, factor) in [(25.0, 2.0), (50.0, 4.0), (-25.0, 0.5), (100.0, 16.0)] {
        let scaled = base(time);
        // The attack's midpoint comes `factor` times later or earlier, the
        // peak and the sustain are where they were.
        let half = |l: &[f32]| reach(l, 0.5) as f64;
        let ratio = half(&scaled) / half(&flat);
        assert!(
            (ratio - factor).abs() < 0.02 * factor,
            "Time {time}: {ratio}"
        );
        let peak = scaled.iter().fold(0.0f32, |a, b| a.max(*b));
        assert!((peak - 1.0).abs() < 1e-4, "Time {time}: peak {peak}");
        if factor > 4.0 {
            // Too slow to settle before the note ends.
            continue;
        }
        assert!((scaled[59_000] - 0.3).abs() < 1e-3, "Time {time}: sustain");
        // The release, from the note's end.
        let tail = |l: &[f32]| 60_000 + l[60_000..].iter().position(|x| *x < 0.15).unwrap();
        let release = (tail(&scaled) - 60_000) as f64 / (tail(&flat) - 60_000) as f64;
        assert!(
            (release - factor).abs() < 0.03 * factor,
            "Time {time}: release {release}"
        );
    }
}

// Spec: operator R15 (Time leaves Beat's Repeat alone)
#[test]
fn time_leaves_beats_repeat_alone() {
    let notes = vec![on(0, A3, 0.8)];
    let at = |time: f64| {
        run(bursts(Loop::Beat, &[(TIME, time)]), notes.clone())
            .playing(steady(120.0))
            .left(3 * 24_000)
    };
    for time in [-50.0, 0.0, 50.0] {
        near(&onsets(&at(time)), &[0, 24_000, 48_000], 3);
    }
}

// Spec: operator R15 (Time < Key)
#[test]
fn time_key_shortens_times_by_two_to_the_octaves_above_c3_times_the_amount() {
    let reach_at = |key: u8, amount: f64| {
        let params = square(&[
            (TIME_KEY, amount),
            env(ENV_INITIAL, 0.0),
            env(ENV_ATTACK, 100.0),
        ]);
        let signal = levels(&run(params, vec![on(0, key, 0.8)]).left(9_600));
        reach(&signal, 0.5) as f64
    };
    let c3 = reach_at(60, 100.0);
    assert!((reach_at(72, 100.0) / c3 - 0.5).abs() < 0.01);
    assert!((reach_at(48, 100.0) / c3 - 2.0).abs() < 0.02);
    assert!((reach_at(72, -100.0) / c3 - 2.0).abs() < 0.02);
    assert!((reach_at(72, 50.0) / c3 - 0.5f64.sqrt()).abs() < 0.01);
    assert!((reach_at(72, 0.0) / c3 - 1.0).abs() < 0.001);
}

// Spec: operator R15 (each envelope's Time < Vel)
#[test]
fn time_vel_scales_by_two_to_the_minus_two_amount_times_velocity_less_a_half() {
    let reach_at = |velocity: f32, amount: f64| {
        let params = square(&[
            env(ENV_TIME_VEL, amount),
            env(ENV_INITIAL, 0.0),
            env(ENV_ATTACK, 100.0),
        ]);
        let signal = levels(&run(params, vec![on(0, A3, velocity)]).left(19_200));
        reach(&signal, 0.5) as f64
    };
    let mid = reach_at(0.5, 100.0);
    assert!((reach_at(1.0, 100.0) / mid - 0.5).abs() < 0.01);
    assert!((reach_at(0.0, 100.0) / mid - 2.0).abs() < 0.02);
    assert!((reach_at(1.0, -100.0) / mid - 2.0).abs() < 0.02);
    assert!((reach_at(1.0, 0.0) / mid - 1.0).abs() < 0.001);
}

// ---- R9 Tone through Operator ----

/// The energy of `signal` above `hz`, as a share of all of it.
fn share_above(signal: &[f32], hz: f64) -> f64 {
    let spectrum = common::spectrum(signal);
    let all: f64 = spectrum.iter().map(|a| a * a).sum();
    let bin = |f: f64| (f * signal.len() as f64 / HZ) as usize;
    let high: f64 = spectrum[bin(hz)..].iter().map(|a| a * a).sum();
    high / all
}

// Spec: operator R9 (Tone darkens the waves)
#[test]
fn tone_darkens_the_waves() {
    let saw = |tone: f64| {
        let params = vec![
            (osc(0, OSC_WAVE), 9.0),
            (TONE, tone),
            (ANTIALIAS, 0.0),
            (VOLUME, 1.0),
        ];
        run(params, vec![on(0, 48, 0.8)]).left(48_000)[4_800..].to_vec()
    };
    // Tone 30 % is 70 % below the top: 2^2.8 times fewer partials, none
    // above about 3.4 kHz.
    let bright = share_above(&saw(100.0), 4_000.0);
    let dark = share_above(&saw(30.0), 4_000.0);
    assert!(
        bright > 0.01 && dark < bright / 50.0,
        "{dark} against {bright}"
    );
}

// ---- R16: the pitch envelope ----

/// The pitch envelope held at `level` per cent: flat, from the first frame.
fn flat_pitch(level: f64) -> Vec<(ParamId, f64)> {
    let control = |c| shaped(PITCH_ENV, c);
    vec![
        (PITCH_ON, 1.0),
        (control(SHAPED_INITIAL), level),
        (control(SHAPED_PEAK), level),
        (control(SHAPED_SUSTAIN), level),
        (control(SHAPED_END), level),
        (control(SHAPED_ATTACK), 0.0),
        (control(SHAPED_DECAY), 0.0),
    ]
}

// Spec: operator R16 (moves only the oscillators it is switched on for)
#[test]
fn the_pitch_envelope_moves_only_the_oscillators_switched_on_for() {
    let mut extra = vec![
        (ALGORITHM, 11.0),
        level(0, 0.5),
        level(1, 0.5),
        (ParamId(PITCH_OSC), 1.0),
        (ParamId(PITCH_OSC + 1), 0.0),
        (ParamId(PITCH_OSC + 2), 0.0),
        (ParamId(PITCH_OSC + 3), 0.0),
    ];
    // Level 100 % at Pitch Env 100 % is 48 semitones: A plays 16 times up.
    extra.extend(flat_pitch(100.0));
    let signal = run(patch(&extra), vec![on(0, A3, 0.8)]).left(48_000);
    let heard = &signal[4_800..];
    let (moved, kept, left_behind) = (
        amplitude_at(heard, 16.0 * F0),
        amplitude_at(heard, F0),
        amplitude_at(heard, F0 * 2.0),
    );
    assert!(
        (moved - 0.125).abs() < 0.002,
        "A went up four octaves: {moved}"
    );
    assert!((kept - 0.125).abs() < 0.002, "B stayed: {kept}");
    assert!(db(left_behind) < -60.0);
    // Switched off, it moves nothing.
    extra.retain(|(id, _)| *id != PITCH_ON);
    extra.push((PITCH_ON, 0.0));
    let signal = run(patch(&extra), vec![on(0, A3, 0.8)]).left(48_000);
    assert!(db(amplitude_at(&signal[4_800..], 16.0 * F0)) < -60.0);
}

// Spec: operator R16 (Pitch Env: 100 % follows the levels, -100 % inverts them)
#[test]
fn pitch_env_scales_and_inverts_the_envelopes_levels() {
    for (level, amount, ratio) in [
        (50.0, 100.0, 4.0),
        (100.0, -50.0, 0.25),
        (-100.0, 50.0, 0.25),
    ] {
        let mut extra = vec![(PITCH_AMOUNT, amount)];
        extra.extend(flat_pitch(level));
        let signal = run(patch(&extra), vec![on(0, A3, 0.8)]).left(48_000);
        let at = amplitude_at(&signal[4_800..], F0 * ratio);
        assert!(
            (at - 1.0).abs() < 0.01,
            "level {level}, Pitch Env {amount}: {at}"
        );
    }
}

// Spec: operator R16 (the pitch envelope's shape moves pitch over time)
#[test]
fn the_pitch_envelope_bends_pitch_along_its_decay() {
    // Peak 100 % falling to 0 over a second at Pitch Env 25 % (12 semitones):
    // an octave above at the start, back at the note's pitch at the end.
    let control = |c| shaped(PITCH_ENV, c);
    let extra = vec![
        (PITCH_ON, 1.0),
        (PITCH_AMOUNT, 25.0),
        (control(SHAPED_INITIAL), 100.0),
        (control(SHAPED_PEAK), 100.0),
        (control(SHAPED_SUSTAIN), 0.0),
        (control(SHAPED_ATTACK), 0.0),
        (control(SHAPED_DECAY), 1_000.0),
    ];
    let signal = run(patch(&extra), vec![on(0, A3, 0.8)]).left(96_000);
    let late = &signal[60_000..96_000];
    assert!(
        (amplitude_at(late, F0) - 1.0).abs() < 0.02,
        "settled at the note's pitch"
    );
    // Early on it is close to an octave up: the zero crossings are twice as
    // dense as at the end.
    let crossings = |s: &[f32]| s.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64;
    let early = crossings(&signal[2_400..7_200]) / 4_800.0;
    let late = crossings(&signal[84_000..96_000]) / 12_000.0;
    assert!(early > 1.7 * late, "{early} against {late}");
}

// Spec: operator R16 (Destination B: one more target with Amount B)
#[test]
fn destination_b_sends_the_pitch_envelope_to_one_more_target() {
    // Osc Volume B (destination 2): B is silent at Level 0 and the envelope
    // raises it by Amount B times its value.
    let mut extra = vec![
        (ALGORITHM, 11.0),
        level(0, 0.0),
        level(1, 0.0),
        (osc(1, OSC_COARSE), 2.0),
        (PITCH_DEST_B, 2.0),
        (PITCH_AMOUNT_B, 80.0),
        (PITCH_AMOUNT, 0.0),
    ];
    extra.extend(flat_pitch(100.0));
    let signal = run(patch(&extra), vec![on(0, A3, 0.8)]).left(48_000);
    let at = amplitude_at(&signal[4_800..], 2.0 * F0);
    // 0.8 of full level, divided among four carriers.
    assert!((at - 0.2).abs() < 0.004, "{at}");
    // Pitch Env at 0 % moved no pitch.
    assert!(db(amplitude_at(&signal[4_800..], 32.0 * F0)) < -60.0);
    // Osc Fixed Frequency (destination 8) moves a fixed oscillator.
    let mut extra = vec![
        (osc(0, OSC_FIXED), 1.0),
        (osc(0, OSC_FREQ), 200.0),
        (PITCH_DEST_B, 8.0),
        (PITCH_AMOUNT_B, 50.0),
        (PITCH_AMOUNT, 0.0),
    ];
    extra.extend(flat_pitch(100.0));
    let signal = run(patch(&extra), vec![on(0, A3, 0.8)]).left(48_000);
    // 50 % of 100 % is 24 semitones: 200 Hz to 800 Hz.
    assert!((amplitude_at(&signal[4_800..], 800.0) - 1.0).abs() < 0.01);
}

// ---- R17: Volume, Pan, Transpose, Voices and per-note bend ----

/// The sine's amplitude on each channel, the first second after 0.1 s.
fn sides(signal: &[Vec<f32>; 2]) -> (f64, f64) {
    (
        amplitude_at(&signal[0][4_800..48_000], F0),
        amplitude_at(&signal[1][4_800..48_000], F0),
    )
}

// Spec: operator R17 (Volume)
#[test]
fn volume_sets_the_output_level() {
    let half = run(
        patch(&[(VOLUME, db_to_gain(-6.0206))]),
        vec![on(0, A3, 0.8)],
    )
    .left(48_000);
    assert!((amplitude_at(&half[4_800..], F0) - 0.5).abs() < 0.005);
    let silent = run(patch(&[(VOLUME, 0.0)]), vec![on(0, A3, 0.8)]).left(48_000);
    assert_eq!(rms(&silent), 0.0);
}

// Spec: operator R17 (Pan)
#[test]
fn pan_places_the_sound_between_left_and_right() {
    for pan in [-100.0, -40.0, 0.0, 70.0, 100.0] {
        let signal = run(patch(&[(PAN, pan)]), vec![on(0, A3, 0.8)]).render(48_000, 4_096);
        let (left, right) = sides(&signal);
        let (l, r) = pan_gains((pan / 100.0) as f32);
        assert!(
            (left - f64::from(l)).abs() < 0.01 && (right - f64::from(r)).abs() < 0.01,
            "pan {pan}: {left}, {right} against {l}, {r}"
        );
    }
}

// Spec: operator R17 (Pan < Key: hard four octaves either side of C3)
#[test]
fn pan_key_pans_low_notes_left_and_high_notes_right() {
    for (key, pan) in [(60, 0.0), (72, 0.25), (108, 1.0), (12, -1.0), (36, -0.5)] {
        let params = patch(&[(PAN_KEY, 100.0)]);
        let signal = run(params, vec![on(0, key, 0.8)]).render(48_000, 4_096);
        let hz = frequency(key);
        let (left, right) = (
            amplitude_at(&signal[0][4_800..48_000], hz),
            amplitude_at(&signal[1][4_800..48_000], hz),
        );
        let (l, r) = pan_gains(pan);
        assert!(
            (left - f64::from(l)).abs() < 0.01 && (right - f64::from(r)).abs() < 0.01,
            "key {key}: {left}, {right} against {l}, {r}"
        );
    }
}

// Spec: operator R17 (Pan < Random: a seeded random share up to the amount)
#[test]
fn pan_random_pans_each_note_by_a_seeded_share_up_to_the_amount() {
    let notes: Vec<NoteEvent> = (0..6)
        .map(|i| on(i * 48_000, A3, 0.8))
        .chain((0..6).map(|i| off(i * 48_000 + 40_000, A3)))
        .collect();
    let render = || run(patch(&[(PAN_RANDOM, 50.0)]), notes.clone()).render(6 * 48_000, 4_096);
    let signal = render();
    assert_eq!(render(), signal, "seeded: the same every run");
    let mut pans = Vec::new();
    for i in 0..6 {
        let from = i * 48_000 + 4_800;
        let (left, right) = (
            amplitude_at(&signal[0][from..from + 24_000], F0),
            amplitude_at(&signal[1][from..from + 24_000], F0),
        );
        // pan_gains: (right² − left²) / 2 is sin(pan × π / 2).
        let pan = ((right * right - left * left) / 2.0).asin() * 2.0 / std::f64::consts::PI;
        assert!(pan.abs() <= 0.5 + 0.01, "note {i}: {pan}");
        pans.push(pan);
    }
    let spread = pans.iter().fold(0.0f64, |a, b| a.max(b.abs()));
    assert!(
        spread > 0.1,
        "the notes don't all sit in one place: {pans:?}"
    );
}

// Spec: operator R17 (Transpose moves notes already sounding)
#[test]
fn transpose_moves_every_note_those_sounding_included() {
    let at = |transpose: f64| {
        let signal = run(patch(&[(TRANSPOSE, transpose)]), vec![on(0, A3, 0.8)]).left(48_000);
        amplitude_at(&signal[4_800..], F0 * (transpose / 12.0).exp2())
    };
    for transpose in [-12.0, 0.0, 7.0, 12.0] {
        assert!((at(transpose) - 1.0).abs() < 0.01, "{transpose}");
    }
    // Changed during the note.
    let signal = run(patch(&[]), vec![on(0, A3, 0.8)])
        .events(vec![common::set(24_000, TRANSPOSE, 12.0)])
        .left(72_000);
    assert!((amplitude_at(&signal[4_800..20_000], F0) - 1.0).abs() < 0.02);
    assert!((amplitude_at(&signal[30_000..70_000], 2.0 * F0) - 1.0).abs() < 0.02);
}

// Spec: operator R17 (Voices at 1 plays Mono with Legato)
#[test]
fn voices_at_one_plays_mono_legato_without_restarting_the_envelope() {
    let slow = |voices: f64| {
        let params = square(&[
            (VOICES, voices),
            env(ENV_INITIAL, 0.0),
            env(ENV_ATTACK, 400.0),
        ]);
        // The second note comes in a quarter of the way up the attack.
        let notes = vec![on(0, 60, 0.8), on(4_800, 62, 0.8)];
        levels(&run(params, notes).left(14_400))
    };
    let mono = slow(1.0);
    assert!(
        mono.windows(2).all(|w| w[1] >= w[0] - 1e-6),
        "the attack carries on through the second note"
    );
    let poly = slow(6.0);
    assert!(
        poly[6_000] > mono[6_000] + 0.05,
        "poly: a second voice begins"
    );
}

// Spec: operator R17 (Voices' limit)
#[test]
fn voices_limits_how_many_notes_sound_at_once() {
    let chord: Vec<NoteEvent> = (0..4).map(|i| on(0, 48 + 7 * i, 0.8)).collect();
    let count = |voices: f64| {
        let signal = run(patch(&[(VOICES, voices)]), chord.clone()).left(48_000);
        (0..4)
            .filter(|i| amplitude_at(&signal[4_800..], frequency(48 + 7 * *i as u8)) > 0.1)
            .count()
    };
    assert_eq!(count(6.0), 4);
    assert_eq!(count(2.0), 2);
}

// Spec: operator R20 (Enable Per-Note Pitch Bend, on by default)
#[test]
fn a_per_note_bend_of_one_is_48_semitones_and_the_menu_option_switches_it_off() {
    let notes = vec![common::on(0, A3, 1), common::bend(0, A3, 1, 0.25)];
    // A quarter of 48 semitones is a minor tenth...12 semitones: an octave.
    let bent = run(patch(&[]), notes.clone()).left(48_000);
    assert!((amplitude_at(&bent[4_800..], 2.0 * F0) - 1.0).abs() < 0.02);
    let ignored = run(patch(&[(NOTE_PB, 0.0)]), notes).left(48_000);
    assert!((amplitude_at(&ignored[4_800..], F0) - 1.0).abs() < 0.02);
}

// Spec: operator R17 (PB Range: the wheel)
#[test]
fn the_pitch_wheel_moves_every_note_by_pb_range() {
    let wheel = NoteEvent {
        offset: 0,
        kind: NoteKind::PitchBend { value: 1.0 },
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    };
    for range in [2.0, 5.0, 12.0] {
        let signal = run(patch(&[(PB_RANGE, range)]), vec![wheel, on(0, A3, 0.8)]).left(48_000);
        let at = amplitude_at(&signal[4_800..], F0 * (range / 12.0).exp2());
        assert!((at - 1.0).abs() < 0.02, "range {range}: {at}");
    }
}

// ---- R21: the same at any block size, and the budget ----

fn phrase() -> Run {
    let mut params = patch(&[
        (ALGORITHM, 4.0),
        level(1, 0.3),
        level(2, 0.2),
        (osc(1, OSC_COARSE), 2.0),
        (osc(2, OSC_COARSE), 3.0),
        (osc(1, ENV_LOOP), Loop::Beat as usize as f64),
        (osc(1, ENV_REPEAT), 3.0),
        (osc(1, ENV_DECAY), 300.0),
        (osc(1, ENV_SUSTAIN), 0.1),
        (osc(0, OSC_LEVEL_VEL), 60.0),
        (osc(0, OSC_FEEDBACK), 30.0),
        (PAN_KEY, 40.0),
        (PAN_RANDOM, 30.0),
        (TIME, 20.0),
        (TIME_KEY, 30.0),
        (VOICES, 3.0),
    ]);
    params.extend(flat_pitch(40.0));
    let control = |c| shaped(PITCH_ENV, c);
    params.extend([
        (control(SHAPED_INITIAL), 100.0),
        (control(SHAPED_DECAY), 400.0),
        (control(SHAPED_SUSTAIN), 10.0),
        (control(SHAPED_LOOP), Loop::Sync as usize as f64),
        (control(SHAPED_REPEAT), 2.0),
        (PITCH_AMOUNT, 20.0),
        (ANTIALIAS, 1.0),
    ]);
    let mut notes = Vec::new();
    for (i, key) in [48u8, 55, 60, 64, 67, 72, 55, 48].into_iter().enumerate() {
        let at = 5_000 * i as u32;
        notes.push(on(at, key, 0.3 + 0.1 * i as f32));
        notes.push(off(at + 11_000, key));
    }
    notes.sort_by_key(|n| n.offset);
    run(params, notes).playing(changing())
}

// Spec: operator R21 (the same at any block size, Beat and Sync included)
#[test]
fn a_phrase_renders_the_same_at_block_sizes_1_to_4096() {
    let frames = 60_000;
    let reference = phrase().render(frames, 4_096);
    assert!(rms(&reference[0]) > 0.01 && rms(&reference[1]) > 0.01);
    for block in BLOCKS {
        assert_eq!(
            phrase().render(frames, block),
            reference,
            "block size {block}"
        );
    }
}

// Spec: operator R21 (Antialias reports its latency)
#[test]
fn antialias_is_the_cores_two_times_oversampling_and_reports_its_latency() {
    let node = make_node(&[(ANTIALIAS, 1.0)], 512);
    let off = make_node(&[(ANTIALIAS, 0.0)], 512);
    assert!(node.latency() > 0 && off.latency() == 0);
}

/// Whether the tests run at full depth: `BL_TEST_DEPTH=full`, as on pushes
/// to main, manual CI runs, and the nightly run. The voice budget is judged
/// only then, so a loaded machine can't fail it.
fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Ok("quick") | Err(_) => false,
        Ok(other) => {
            panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full, or leave it unset")
        }
    }
}

// Spec: operator R21 (sixteen voices with four operators inside the budget)
#[test]
fn sixteen_voices_of_four_operators_render_faster_than_the_device_budget() {
    let params = patch(&[
        (ALGORITHM, 4.0),
        level(1, 0.4),
        level(2, 0.4),
        level(3, 0.4),
        (osc(1, OSC_COARSE), 2.0),
        (osc(2, OSC_COARSE), 3.0),
        (osc(3, OSC_COARSE), 5.0),
        (ANTIALIAS, 1.0),
        (VOICES, 16.0),
    ]);
    let seconds = 5;
    let frames = seconds * HZ as usize;
    let chord: Vec<NoteEvent> = (0..16).map(|i| on(0, 36 + i * 3, 0.8)).collect();
    let play = run(params, chord);
    let start = Instant::now();
    let [left, _] = play.render(frames, 128);
    let elapsed = start.elapsed().as_secs_f64();
    let realtime = seconds as f64 / elapsed;
    println!("16 voices: {seconds} s of audio in {elapsed:.3} s, {realtime:.0}x real time (debug)");
    assert!(rms(&left) > 0.01, "the chord sounds");
    if full_depth() && crate::common::budget::judged() {
        // A debug build on a loaded machine; 16 voices mustn't eat more than
        // a quarter of the time they play.
        assert!(realtime >= 4.0, "16 voices ran at {realtime:.1}x real time");
    }
}

// ---- R22: the factory presets ----

// Spec: operator R22
#[test]
fn every_factory_preset_sets_parameters_in_range_and_plays() {
    let kind = operator_type();
    let all = operator::presets();
    assert!(all.len() >= 4, "a few presets");
    let mut names: Vec<&str> = all.iter().map(|p| p.name).collect();
    names.dedup();
    assert_eq!(names.len(), all.len(), "each has a name of its own");
    for preset in all {
        for (id, value) in &preset.values {
            let param = kind.description.param(*id).unwrap_or_else(|| {
                panic!(
                    "{} sets parameter {}, which Operator hasn't",
                    preset.name, id.0
                )
            });
            assert_eq!(
                param.clamp(*value),
                *value,
                "{}: {}",
                preset.name,
                param.name
            );
        }
        let mut params = patch(&[]);
        params.extend(preset.values.iter().copied());
        let notes = vec![on(0, 48, 0.8), off(30_000, 48)];
        let signal = run(params, notes).left(48_000);
        assert!(
            rms(&signal[2_000..20_000]) > 0.005,
            "{} is silent",
            preset.name
        );
        assert!(
            signal.iter().all(|x| x.is_finite() && x.abs() < 4.0),
            "{}",
            preset.name
        );
    }
}

// ---- R18: the shell's pages ----

fn shell_placed(show: Option<&str>) -> (Placed, Vec<String>) {
    let kind = operator_type();
    let layout = kind.description.layout.clone();
    let choice: bl_model::device_layout::PageChoice = show
        .map(|name| [("Sections".to_owned(), name.to_owned())].into())
        .unwrap_or_default();
    let layout = layout.with_pages(&choice);
    let placed = layout.place(layout.docked_size());
    let names = layout.pages()[0]
        .pages
        .iter()
        .map(|p| p.name.clone())
        .collect();
    (placed, names)
}

// Spec: operator R18 (a shell of sections in the manual's order)
#[test]
fn the_body_is_a_shell_of_sections_and_each_section_fills_the_display() {
    let kind = operator_type();
    let layout = &kind.description.layout;
    let nodes = layout.pages();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].name, "Sections");
    assert_eq!(nodes[0].style, PageStyle::Shell);
    let (_, names) = shell_placed(None);
    assert_eq!(
        names,
        [
            "Global", "Osc A", "Osc B", "Osc C", "Osc D", "LFO", "Filter", "Pitch"
        ]
    );
    // Every parameter's control is placed once across the pages, and the
    // menu option has none (the description's check says so too).
    kind.description.check().expect("every control placed once");
    let controls_of = |placed: &Placed| -> Vec<u32> {
        let mut found: Vec<u32> = placed.controls.iter().map(|c| c.param.0).collect();
        found.sort_unstable();
        found
    };
    // The sections' controls show whichever page does; the display holds
    // the shown page's further controls.
    let (global, _) = shell_placed(Some("Global"));
    let (a, _) = shell_placed(Some("Osc A"));
    let (d, _) = shell_placed(Some("Osc D"));
    let (pitch, _) = shell_placed(Some("Pitch"));
    for placed in [&global, &a, &d, &pitch] {
        let ids = controls_of(placed);
        for section in [
            ALGORITHM.0,
            osc(0, OSC_WAVE).0,
            osc(3, OSC_COARSE).0,
            PITCH_ON.0,
        ] {
            assert!(ids.contains(&section), "section control {section}");
        }
    }
    assert!(controls_of(&global).contains(&TONE.0));
    assert!(!controls_of(&global).contains(&osc(0, OSC_FINE).0));
    assert!(controls_of(&a).contains(&osc(0, OSC_FINE).0));
    assert!(controls_of(&a).contains(&osc(0, ENV_ATTACK).0));
    assert!(!controls_of(&a).contains(&osc(3, OSC_FINE).0));
    assert!(controls_of(&d).contains(&osc(3, OSC_FINE).0));
    assert!(controls_of(&pitch).contains(&shaped(PITCH_ENV, SHAPED_ATTACK).0));
    assert!(controls_of(&pitch).contains(&PITCH_DEST_B.0));
    // The device keeps its size whichever section shows.
    let size = layout.docked_size();
    for name in [
        "Global", "Osc A", "Osc B", "Osc C", "Osc D", "LFO", "Filter", "Pitch",
    ] {
        let shown = layout.with_pages(&[("Sections".to_owned(), name.to_owned())].into());
        assert_eq!(shown.docked_size(), size, "{name}");
    }
}

// ---- Part 2 (WP-8.13): the LFO, the filter and shaper, the global targets,
// Glide and Spread ----

/// A patch whose carrier A is the naive square at a fixed 100 Hz, and whose
/// LFO is a square at 8 Hz (Range Hi, Rate 0) at full Amount, sending
/// itself to A's level: `|x|` is 1 while the LFO is high (the first 3,000
/// frames of each 6,000) and `1 - Amount A` while it is low.
fn tremolo(extra: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut params = vec![
        (LFO_WAVE, 1.0),
        (LFO_RANGE, 1.0),
        (LFO_RATE, 0.0),
        (LFO_AMOUNT, 100.0),
        (ParamId(LFO_OSC), 1.0),
    ];
    params.extend_from_slice(extra);
    square(&params)
}

/// `|x|` of `signal` at `frame`, averaged over 40 frames.
fn at_frame(signal: &[f32], frame: usize) -> f64 {
    signal[frame..frame + 40]
        .iter()
        .map(|x| f64::from(x.abs()))
        .sum::<f64>()
        / 40.0
}

// Spec: operator R23 (the LFO's waves, range and Amount, to the oscillators' levels)
#[test]
fn the_lfo_swings_an_oscillators_level_between_its_whole_and_none() {
    let signal = run(tremolo(&[]), vec![on(0, A3, 0.8)]).left(24_000);
    assert!((at_frame(&signal, 1_000) - 1.0).abs() < 0.01);
    assert!(at_frame(&signal, 4_500) < 0.01);
    assert!((at_frame(&signal, 7_000) - 1.0).abs() < 0.01);
    // Amount A halves the swing; a negative one swings the other way.
    let half = run(tremolo(&[(LFO_AMOUNT_A, 50.0)]), vec![on(0, A3, 0.8)]).left(24_000);
    assert!((at_frame(&half, 4_500) - 0.5).abs() < 0.01);
    let flipped = run(tremolo(&[(LFO_AMOUNT_A, -100.0)]), vec![on(0, A3, 0.8)]).left(24_000);
    assert!(at_frame(&flipped, 1_000) < 0.01);
    assert!((at_frame(&flipped, 4_500) - 1.0).abs() < 0.01);
    // Amount scales the depth, and a new device's 0 % moves nothing.
    let less = run(tremolo(&[(LFO_AMOUNT, 25.0)]), vec![on(0, A3, 0.8)]).left(24_000);
    assert!((at_frame(&less, 4_500) - 0.75).abs() < 0.01);
    let none = run(tremolo(&[(LFO_AMOUNT, 0.0)]), vec![on(0, A3, 0.8)]).left(24_000);
    assert!((at_frame(&none, 4_500) - 1.0).abs() < 0.01);
}

// Spec: operator R23 (Amt < Vel; its envelope scales it)
#[test]
fn amt_vel_and_the_lfos_envelope_scale_its_depth() {
    // Velocity 0.5 at 100 % leaves half the Amount.
    let soft = run(tremolo(&[(LFO_AMT_VEL, 100.0)]), vec![on(0, A3, 0.5)]).left(24_000);
    assert!((at_frame(&soft, 4_500) - 0.5).abs() < 0.01);
    let loud = run(tremolo(&[(LFO_AMT_VEL, 100.0)]), vec![on(0, A3, 1.0)]).left(24_000);
    assert!(at_frame(&loud, 4_500) < 0.01);
    // An envelope held at 50 % halves it.
    let envelope = |level| {
        [SHAPED_INITIAL, SHAPED_PEAK, SHAPED_SUSTAIN, SHAPED_END]
            .map(|c| (shaped(LFO_ENV, c), level))
    };
    let half = run(tremolo(&envelope(50.0)), vec![on(0, A3, 0.8)]).left(24_000);
    assert!((at_frame(&half, 4_500) - 0.5).abs() < 0.01);
}

// Spec: operator R23 (Rate < Key doubles the rate each octave at 100 %)
#[test]
fn rate_key_doubles_the_rate_each_octave_from_c3() {
    // C4 is an octave up: 16 Hz, so the LFO is high for 1,500 frames.
    let signal = run(tremolo(&[(LFO_RATE_KEY, 100.0)]), vec![on(0, 72, 0.8)]).left(12_000);
    assert!((at_frame(&signal, 700) - 1.0).abs() < 0.01);
    assert!(at_frame(&signal, 2_200) < 0.01);
    assert!((at_frame(&signal, 3_700) - 1.0).abs() < 0.01);
    // C3 is the centre: 8 Hz.
    let centre = run(tremolo(&[(LFO_RATE_KEY, 100.0)]), vec![on(0, 60, 0.8)]).left(12_000);
    assert!((at_frame(&centre, 2_200) - 1.0).abs() < 0.01);
}

// Spec: operator R23 (Sync: a division of the song's tempo, on the beat)
#[test]
fn sync_runs_the_lfo_in_a_division_of_the_tempo() {
    // Rate 55.6 is the sixth division, one beat a cycle.
    let sync = [(LFO_RANGE, 2.0), (LFO_RATE, 55.6)];
    for (bpm, cycle) in [(120.0, 24_000usize), (60.0, 48_000)] {
        let signal = run(tremolo(&sync), vec![on(0, A3, 0.8)])
            .playing(steady(bpm))
            .left(cycle * 2);
        assert!(
            (at_frame(&signal, cycle / 4) - 1.0).abs() < 0.01,
            "{bpm} bpm"
        );
        assert!(at_frame(&signal, cycle * 3 / 4) < 0.01, "{bpm} bpm");
        assert!(
            (at_frame(&signal, cycle + cycle / 4) - 1.0).abs() < 0.01,
            "{bpm} bpm"
        );
    }
}

// Spec: operator R23 (Retrigger restarts it per note; off, one free-running LFO)
#[test]
fn retrigger_restarts_the_lfo_with_each_note_and_off_one_free_runs() {
    let notes = vec![on(0, A3, 0.8), off(1_000, A3), on(4_500, A3, 0.8)];
    // 4,500 is in the low half of a cycle begun at 0.
    let retriggered = run(tremolo(&[]), notes.clone()).left(9_000);
    assert!((at_frame(&retriggered, 5_000) - 1.0).abs() < 0.01);
    let free = run(tremolo(&[(LFO_RETRIGGER, 0.0)]), notes).left(9_000);
    assert!(at_frame(&free, 5_000) < 0.01);
}

// Spec: operator R23 (the filter's cutoff, up to four octaves; Destination B)
#[test]
fn the_lfo_moves_the_filters_cutoff_and_one_more_destination() {
    // A saw through a low-pass at 440 Hz: the LFO's high half raises the
    // cutoff four octaves, its low half lowers it by as many.
    let saw = [
        (osc(0, OSC_WAVE), 9.0),
        (FILTER_ON, 1.0),
        (FILTER_FREQ, 440.0),
        (LFO_FILTER, 1.0),
        (ParamId(LFO_OSC), 0.0),
    ];
    let signal = run(tremolo(&saw), vec![on(0, A3, 0.8)]).left(12_000);
    let (high, low) = (rms(&signal[1_500..2_900]), rms(&signal[4_500..5_900]));
    assert!(high > 4.0 * low, "{high} {low}");
    // Destination B, Osc Volume B (2): B at Level 0 sounds while the LFO is
    // high, at the Amount B of the LFO.
    let b = [
        (ALGORITHM, 11.0),
        level(0, 0.0),
        level(1, 0.0),
        (osc(1, OSC_COARSE), 2.0),
        (ParamId(LFO_OSC), 0.0),
        (LFO_DEST_B, 2.0),
        (LFO_AMOUNT_B, 80.0),
    ];
    let signal = run(tremolo(&b), vec![on(0, A3, 0.8)]).left(12_000);
    let high = amplitude_at(&signal[600..2_900], 2.0 * F0);
    let low = amplitude_at(&signal[3_600..5_900], 2.0 * F0);
    // 0.8 of full level, divided among four carriers.
    assert!((high - 0.2).abs() < 0.01 && low < 0.005, "{high} {low}");
}

/// A saw (Saw 64) at A3 through the filter with `extra`, left channel.
fn filtered(extra: &[(ParamId, f64)], notes: Vec<NoteEvent>, frames: usize) -> Vec<f32> {
    let mut params = vec![(osc(0, OSC_WAVE), 9.0), (FILTER_ON, 1.0), (LFO_ON, 0.0)];
    params.extend_from_slice(extra);
    run(patch(&params), notes).left(frames)
}

// Spec: operator R24 (Freq < Key, Freq < Vel)
#[test]
fn the_cutoff_follows_the_key_and_the_velocity() {
    // C5 is two octaves above C3: at 100 % its cutoff rises 24 semitones.
    let rms_of = |extra: &[(ParamId, f64)], key, velocity| {
        let signal = filtered(extra, vec![on(0, key, velocity)], 24_000);
        rms(&signal[9_600..])
    };
    let fixed = rms_of(&[(FILTER_FREQ, 300.0)], 84, 0.8);
    let keyed = rms_of(&[(FILTER_FREQ, 300.0), (FREQ_KEY, 100.0)], 84, 0.8);
    assert!(keyed > 2.0 * fixed, "{fixed} {keyed}");
    let soft = rms_of(&[(FILTER_FREQ, 300.0), (FREQ_VEL, 100.0)], 84, 0.1);
    let hard = rms_of(&[(FILTER_FREQ, 300.0), (FREQ_VEL, 100.0)], 84, 1.0);
    assert!(hard > 2.0 * soft, "{soft} {hard}");
}

// Spec: operator R24 (Freq < Envelope and the filter's envelope)
#[test]
fn the_cutoff_follows_the_filters_envelope() {
    let env = |c, v| (shaped(FILTER_ENV, c), v);
    let signal = filtered(
        &[
            (FILTER_FREQ, 100.0),
            (FREQ_ENV, 100.0),
            env(SHAPED_INITIAL, 100.0),
            env(SHAPED_PEAK, 100.0),
            env(SHAPED_SUSTAIN, 0.0),
            env(SHAPED_DECAY, 300.0),
        ],
        vec![on(0, A3, 0.8)],
        48_000,
    );
    let (early, late) = (rms(&signal[480..4_800]), rms(&signal[38_400..]));
    assert!(early > 5.0 * late, "{early} {late}");
    // Without the amount the envelope moves nothing.
    let flat = filtered(
        &[
            (FILTER_FREQ, 100.0),
            env(SHAPED_INITIAL, 100.0),
            env(SHAPED_SUSTAIN, 0.0),
        ],
        vec![on(0, A3, 0.8)],
        48_000,
    );
    assert!(rms(&flat[480..4_800]) < 2.0 * rms(&flat[38_400..]));
}

// Spec: operator R24 (the shaper: curve, Drive, Dry/Wet; bypassed at 0 %)
#[test]
fn the_shaper_follows_the_filter_and_is_bypassed_at_zero_wet() {
    let sine = |extra: &[(ParamId, f64)]| {
        let mut params = vec![(LFO_ON, 0.0), (SHAPER_TYPE, 2.0), (SHAPER_DRIVE, 24.0)];
        params.extend_from_slice(extra);
        run(patch(&params), vec![on(0, A3, 0.1)]).left(24_000)
    };
    let clean = run(patch(&[(LFO_ON, 0.0)]), vec![on(0, A3, 0.1)]).left(24_000);
    // At 0 % the signal is the unshaped one, to the bit.
    assert_eq!(sine(&[(SHAPER_WET, 0.0)]), clean);
    // Hard clipping a sine adds its odd harmonics.
    let hard = sine(&[(SHAPER_WET, 100.0)]);
    assert!(db(amplitude_at(&hard[4_800..], 3.0 * F0)) > -20.0);
    assert!(db(amplitude_at(&clean[4_800..], 3.0 * F0)) < -60.0);
    // Off bypasses it whatever the wet.
    let mut params = vec![(LFO_ON, 0.0), (SHAPER_DRIVE, 24.0), (SHAPER_WET, 100.0)];
    params.push((SHAPER_TYPE, 0.0));
    assert_eq!(
        run(patch(&params), vec![on(0, A3, 0.1)]).left(24_000),
        clean
    );
}

/// Target `source`'s Destination A chooser and Amount A.
fn target(source: usize, dest: usize, amount: f64) -> [(ParamId, f64); 2] {
    [
        (target_dest(source, false), dest as f64),
        (target_amount(source, false), amount),
    ]
}

fn controller(controller: u8, value: f32) -> NoteEvent {
    NoteEvent {
        offset: 0,
        kind: NoteKind::Control { controller, value },
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    }
}

// Spec: operator R25 (a global target moves its destination by its amount)
#[test]
fn each_global_target_moves_its_destination_by_its_amount() {
    // Velocity to Osc Volume B (2): B at Level 0 sounds at velocity times
    // the amount, over four carriers.
    let b = |velocity, amount| {
        let mut extra = vec![
            (ALGORITHM, 11.0),
            level(0, 0.0),
            level(1, 0.0),
            (osc(1, OSC_COARSE), 2.0),
        ];
        extra.extend(target(0, 2, amount));
        let signal = run(patch(&extra), vec![on(0, A3, velocity)]).left(48_000);
        amplitude_at(&signal[4_800..], 2.0 * F0)
    };
    assert!((b(0.8, 100.0) - 0.2).abs() < 0.004);
    assert!((b(0.8, 50.0) - 0.1).abs() < 0.004);
    assert!(b(0.8, 0.0) < 0.001);
    // Mod Wheel to Volume (19): a wheel at 0.5 with -100 % halves the output.
    let volume = |wheel, amount| {
        let signal = run(
            patch(&target(4, 19, amount)),
            vec![controller(1, wheel), on(0, A3, 0.8)],
        )
        .left(48_000);
        amplitude_at(&signal[4_800..], F0)
    };
    assert!((volume(0.5, -100.0) - 0.5).abs() < 0.01);
    assert!(volume(1.0, -100.0) < 0.001);
    assert!((volume(0.0, -100.0) - 1.0).abs() < 0.01);
    // Pitch Bend to Osc Fixed Frequency (8): 50 % of a full bend is 24
    // semitones, 200 Hz to 800 Hz.
    let mut extra = vec![(osc(0, OSC_FIXED), 1.0), (osc(0, OSC_FREQ), 200.0)];
    extra.extend(target(3, 8, 50.0));
    let bend = NoteEvent {
        offset: 0,
        kind: NoteKind::PitchBend { value: 1.0 },
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    };
    let signal = run(patch(&extra), vec![bend, on(0, A3, 0.8)]).left(48_000);
    assert!((amplitude_at(&signal[4_800..], 800.0) - 1.0).abs() < 0.02);
    // Key to Panorama (20): C5 is half of the key's span to the right.
    let signal = run(patch(&target(1, 20, 100.0)), vec![on(0, 84, 0.8)]).render(48_000, 4_096);
    let (left, right) = (rms(&signal[0][4_800..]), rms(&signal[1][4_800..]));
    assert!(right > 2.0 * left, "{left} {right}");
    // Aftertouch to Osc Crossfade A/C (5): at full pressure A fades out
    // and C stays.
    let pressure = NoteEvent {
        offset: 0,
        kind: NoteKind::ChannelPressure { value: 1.0 },
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    };
    let mut extra = vec![(ALGORITHM, 11.0), level(2, 1.0), (osc(2, OSC_COARSE), 3.0)];
    extra.extend(target(2, 5, 100.0));
    let signal = run(patch(&extra), vec![pressure, on(0, A3, 0.8)]).left(48_000);
    assert!(amplitude_at(&signal[4_800..], F0) < 0.001);
    assert!(amplitude_at(&signal[4_800..], 3.0 * F0) > 0.2);
}

// Spec: operator R25 (FM Drive raises every index)
#[test]
fn fm_drive_raises_the_index_of_every_modulation() {
    let pressure = NoteEvent {
        offset: 0,
        kind: NoteKind::ChannelPressure { value: 1.0 },
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    };
    let sideband = |amount| {
        let mut extra = vec![(ALGORITHM, 7.0), level(1, 0.15)];
        extra.extend(target(2, 9, amount));
        let signal = run(patch(&extra), vec![pressure, on(0, A3, 0.8)]).left(48_000);
        amplitude_at(&signal[4_800..], 3.0 * F0)
    };
    assert!(sideband(100.0) > 3.0 * sideband(0.0).max(1e-4));
}

/// The strongest frequency from `lo` to `hi` hertz in `signal`.
fn peak_hz(signal: &[f32], lo: f64, hi: f64) -> f64 {
    let mut best = (lo, 0.0);
    let mut hz = lo;
    while hz <= hi {
        let at = amplitude_at(signal, hz);
        if at > best.1 {
            best = (hz, at);
        }
        hz += 0.5;
    }
    best.0
}

// Spec: operator R26 (Glide: polyphonic portamento over Glide Time)
#[test]
fn glide_slides_each_note_from_the_last_over_glide_time() {
    let notes = vec![on(0, A3, 0.8), on(24_000, A3 + 7, 0.8)];
    let after = |glide: &[(ParamId, f64)]| {
        let mut extra = vec![(VOICES, 1.0)];
        extra.extend_from_slice(glide);
        let signal = run(patch(&extra), notes.clone()).left(24_000 + 48_000);
        (
            peak_hz(&signal[24_000 + 240..24_000 + 1_680], 200.0, 360.0),
            peak_hz(&signal[24_000 + 36_000..24_000 + 38_400], 200.0, 360.0),
        )
    };
    let glide = [(GLIDE_ON, 1.0), (GLIDE_TIME, 500.0)];
    let (early, late) = after(&glide);
    assert!(early < 235.0, "{early}");
    assert!((late - frequency(A3 + 7)).abs() < 2.0, "{late}");
    // Off, the note plays at its pitch at once.
    let (early, _) = after(&[]);
    assert!((early - frequency(A3 + 7)).abs() < 2.0, "{early}");
}

// Spec: operator R26 (Spread: two copies detuned and panned, in phase at the start)
#[test]
fn spread_plays_two_detuned_copies_left_and_right_that_start_in_phase() {
    let spread = run(patch(&[(SPREAD, 100.0)]), vec![on(0, A3, 0.8)]).render(96_000, 4_096);
    let (low, high) = (F0 * (-0.25f64 / 12.0).exp2(), F0 * (0.25f64 / 12.0).exp2());
    let [left, right] = &spread;
    // The lower copy is hard left, the higher hard right.
    assert!(amplitude_at(&left[9_600..], low) > 3.0 * amplitude_at(&left[9_600..], high));
    assert!(amplitude_at(&right[9_600..], high) > 3.0 * amplitude_at(&right[9_600..], low));
    // They start in the same phase, so the first frames are alike.
    let largest = left[..200].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(largest > 0.01);
    for i in 0..30 {
        assert!((left[i] - right[i]).abs() < 0.03 * largest, "frame {i}");
    }
    // Each copy is 3 dB down from the single note.
    let single = run(patch(&[]), vec![on(0, A3, 0.8)]).render(96_000, 4_096);
    let ratio = amplitude_at(&left[9_600..], low) / amplitude_at(&single[0][9_600..], F0);
    let pan = f64::from(pan_gains(0.0).0);
    assert!((ratio - 0.7079 / pan).abs() < 0.02, "{ratio} {pan}");
}

// Spec: operator R30 (every new parameter is the same at any block size)
#[test]
fn part_two_renders_the_same_at_block_sizes_1_to_4096() {
    let phrase = || {
        let mut params = patch(&[
            (ALGORITHM, 4.0),
            level(1, 0.3),
            (osc(1, OSC_COARSE), 2.0),
            (ANTIALIAS, 1.0),
            (LFO_WAVE, 6.0),
            (LFO_RANGE, 1.0),
            (LFO_RATE, 20.0),
            (LFO_AMOUNT, 70.0),
            (LFO_RATE_KEY, 50.0),
            (ParamId(LFO_OSC), 1.0),
            (LFO_FILTER, 1.0),
            (LFO_DEST_B, 20.0),
            (LFO_AMOUNT_B, 40.0),
            (FILTER_ON, 1.0),
            (FILTER_FREQ, 900.0),
            (FILTER_RES, 40.0),
            (FREQ_KEY, 60.0),
            (FREQ_ENV, 50.0),
            (SHAPER_TYPE, 1.0),
            (SHAPER_DRIVE, 12.0),
            (SHAPER_WET, 50.0),
            (SPREAD, 60.0),
            (GLIDE_ON, 1.0),
            (GLIDE_TIME, 120.0),
            (PAN_RANDOM, 30.0),
        ]);
        params.extend(target(0, 2, 30.0));
        params.extend(target(1, 10, 20.0));
        params.extend([(shaped(FILTER_ENV, SHAPED_DECAY), 200.0)]);
        let mut notes = Vec::new();
        for (i, key) in [48u8, 55, 60, 64, 67].into_iter().enumerate() {
            let at = 4_000 * i as u32;
            notes.push(on(at, key, 0.4 + 0.1 * i as f32));
            notes.push(off(at + 9_000, key));
        }
        notes.sort_by_key(|n| n.offset);
        run(params, notes)
    };
    let frames = 40_000;
    let reference = phrase().render(frames, 4_096);
    assert!(rms(&reference[0]) > 0.01 && rms(&reference[1]) > 0.01);
    // The filter's coefficients are updated in stretches that start where a
    // call does, so its output differs by rounding alone (under -90 dB).
    for block in BLOCKS {
        let got = phrase().render(frames, block);
        for (got, reference) in got.iter().zip(&reference) {
            let worst = got
                .iter()
                .zip(reference)
                .fold(0.0f32, |worst, (a, b)| worst.max((a - b).abs()));
            assert!(worst < 3e-5, "block {block}: {worst}");
        }
    }
}

// Spec: operator R7, R27 (a drawn wave's partials and Repeat build its table)
#[test]
fn a_drawn_waves_partials_and_repeat_build_the_table_the_device_plays() {
    let mut levels = vec![0.0f32; 16];
    levels[0] = 1.0;
    levels[2] = 0.5;
    let user = [(osc(0, OSC_WAVE), 22.0)];
    let heard = |extra: &[(ParamId, f64)]| {
        let mut params = user.to_vec();
        params.extend_from_slice(extra);
        let signal = run(patch(&params), vec![on(0, A3, 0.8)])
            .partials(0, levels.clone())
            .left(48_000);
        move |partial: f64| amplitude_at(&signal[4_800..], partial * F0)
    };
    let off = heard(&[]);
    assert!((off(1.0) - 1.0).abs() < 0.01 && (off(3.0) - 0.5).abs() < 0.01);
    assert!(off(2.0) < 0.001 && off(17.0) < 0.001);
    // Repeat at 50 % goes on with the pattern a copy fainter: partial 17 is
    // the first again, at half.
    let repeating = heard(&[(osc(0, OSC_REPEAT_ON), 1.0), (osc(0, OSC_REPEAT), 50.0)]);
    assert!((repeating(17.0) - 0.5).abs() < 0.01, "{}", repeating(17.0));
    assert!((repeating(19.0) - 0.25).abs() < 0.01);
    // A new user wave is the first partial alone.
    let new = run(patch(&user), vec![on(0, A3, 0.8)]).left(48_000);
    assert!((amplitude_at(&new[4_800..], F0) - 1.0).abs() < 0.01);
    assert!(amplitude_at(&new[4_800..], 3.0 * F0) < 0.001);
}
