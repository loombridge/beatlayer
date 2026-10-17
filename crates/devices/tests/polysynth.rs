//! Cascade, the native polysynth (WP-2.17, part 1; the polysynth spec): the
//! oscillators' aliasing, the low-pass filters against their prototypes,
//! the envelopes' timing, polyphony and stealing, and the voice budget.
//!
//! Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::time::Instant;

use bl_engine::{
    Block, DeviceType, Node, NoteBuffer, NoteEvent, NoteExpression, NoteKind, Registry,
    TransportInfo,
};
use bl_model::DeviceKind;
use bl_model::ParamId;
use bl_model::param::db_to_gain;
use common::{CHANNELS, RATE, device, node};
use realfft::RealFftPlanner;

use bl_devices::polysynth::{
    FILTER, FILTER_TYPE, FREQ, HP, KEY, LFO_AMOUNT, LFO_DIVISIONS, LFO_MODE, LFO_MODES, LFO_RATIO,
    LFO_RETRIGGER, LFO_SHAPE, LFO_SHAPES, LFO_SYNC, MOD_DESTINATIONS, MOD_SOURCES, MOD1_AMOUNT,
    MOD1_DEST, MOD1_SOURCE, MOD2_AMOUNT, MOD2_DEST, MOD2_SOURCE, NOISE, NOISE_GAIN, NOTE_PB,
    OSC1_GAIN, OSC1_SHAPE, OSC1_WAVE, OSC2, OSC2_DETUNE, PB_RANGE, PITCH_MOD1_AMOUNT,
    PITCH_MOD1_SOURCE, POLYSYNTH, RES, RETRIGGER, VOLUME,
};

/// The sample rate the tests run at, in hertz.
const HZ: f64 = 48_000.0;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

fn cascade() -> DeviceType {
    bl_devices::device_types()
        .into_iter()
        .find(|device_type| {
            matches!(&device_type.kind, DeviceKind::Native { device_type } if device_type == POLYSYNTH)
        })
        .expect("Cascade is registered")
}

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

/// Runs one block of `frames` through `node`, playing `notes`, and returns
/// the left channel.
fn play(node: &mut dyn Node, frames: usize, notes: &[NoteEvent]) -> Vec<f32> {
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut out = NoteBuffer::with_capacity(64);
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            &[],
            TransportInfo::default(),
            RATE,
        )
        .with_notes(notes, &mut out);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data[..frames].to_vec()
}

/// Plays `notes` for `frames`, in blocks of `block`, and returns the whole
/// left channel.
fn play_blocks(node: &mut dyn Node, frames: usize, block: usize, notes: &[NoteEvent]) -> Vec<f32> {
    let mut signal = Vec::with_capacity(frames);
    let mut at = 0;
    while at < frames {
        let len = block.min(frames - at);
        let block_notes: Vec<NoteEvent> = notes
            .iter()
            .filter(|note| (note.offset as usize) >= at && (note.offset as usize) < at + len)
            .map(|note| NoteEvent {
                offset: note.offset - at as u32,
                ..*note
            })
            .collect();
        signal.extend(play(node, len, &block_notes));
        at += len;
    }
    signal
}

/// The note key whose frequency is nearest `hz`.
fn key_for(hz: f64) -> u8 {
    (69.0 + 12.0 * (hz / 440.0).log2())
        .round()
        .clamp(0.0, 127.0) as u8
}

/// The frequency of `key` with A4 at 440 Hz.
fn frequency(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// The loudest frequency in `signal` between `low` and `high`, in hertz.
fn peak_frequency(signal: &[f32], low: f64, high: f64) -> f64 {
    const N: usize = 16_384;
    let frame = N.min(signal.len());
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(frame);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    for ((slot, &x), n) in input.iter_mut().zip(&signal[..frame]).zip(0..frame) {
        let w = 0.5 - 0.5 * (core::f64::consts::TAU * n as f64 / frame as f64).cos();
        *slot = f64::from(x) * w;
    }
    fft.process(&mut input, &mut spectrum).unwrap();
    let bin = HZ / frame as f64;
    let mut best = (0.0, 0.0);
    for (k, value) in spectrum.iter().enumerate() {
        let hz = k as f64 * bin;
        if (low..high).contains(&hz) {
            let energy = value.norm_sqr();
            if energy > best.1 {
                best = (hz, energy);
            }
        }
    }
    best.0
}

/// The energy outside a note's harmonics over the energy in them, in dB:
/// the aliasing level. A 7-term Blackman–Harris window, 8,192-frame
/// spectra overlapping by half, and 9 bins either side of each harmonic,
/// as the kit's own measurement does.
fn aliasing_db(signal: &[f32], fundamental: f64) -> f64 {
    const N: usize = 8_192;
    const A: [f64; 7] = [
        0.271_051_400_693_42,
        0.433_297_939_234_48,
        0.218_122_999_543_11,
        0.065_925_446_388_03,
        0.010_811_742_098_37,
        0.000_776_584_825_22,
        0.000_013_887_217_35,
    ];
    let window: Vec<f64> = (0..N)
        .map(|i| {
            let x = core::f64::consts::TAU * i as f64 / (N - 1) as f64;
            A.iter()
                .enumerate()
                .map(|(k, a)| {
                    let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                    sign * a * (k as f64 * x).cos()
                })
                .sum()
        })
        .collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(N);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let bin = HZ / N as f64;
    let margin = 9.0 * bin;
    let (mut harmonic, mut other) = (0.0, 0.0);
    for start in (0..=signal.len() - N).step_by(N / 2) {
        for ((slot, &x), &w) in input.iter_mut().zip(&signal[start..start + N]).zip(&window) {
            *slot = f64::from(x) * w;
        }
        fft.process(&mut input, &mut spectrum).unwrap();
        for (k, value) in spectrum.iter().enumerate() {
            let hz = k as f64 * bin;
            if hz < margin || hz >= HZ / 2.0 {
                continue;
            }
            let energy = value.norm_sqr();
            let lowest = ((hz - margin) / fundamental).ceil().max(1.0);
            let highest = ((hz + margin) / fundamental).floor();
            if lowest <= highest && lowest * fundamental < HZ / 2.0 {
                harmonic += energy;
            } else {
                other += energy;
            }
        }
    }
    10.0 * (other / harmonic).log10()
}

/// The root mean square of `signal`.
fn rms(signal: &[f32]) -> f64 {
    (signal
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        / signal.len() as f64)
        .sqrt()
}

/// The amplitude of `hz` in `signal`, by a Hann-windowed DFT over the
/// whole slice.
fn amplitude_at(signal: &[f32], hz: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &x) in signal.iter().enumerate() {
        let w = 0.5 - 0.5 * (core::f64::consts::TAU * i as f64 / n).cos();
        let phase = core::f64::consts::TAU * hz * i as f64 / HZ;
        re += f64::from(x) * w * phase.cos();
        im -= f64::from(x) * w * phase.sin();
    }
    2.0 * re.hypot(im) / n
}

/// `signal`'s envelope: the peak of every 128-frame window.
fn envelope(signal: &[f32]) -> Vec<f32> {
    signal
        .chunks(128)
        .map(|window| window.iter().fold(0.0f32, |peak, x| peak.max(x.abs())))
        .collect()
}

/// The first window of `signal` whose envelope reaches `fraction` of its
/// peak, in milliseconds; `None` if it never does.
fn rise_time_ms(signal: &[f32], fraction: f32) -> Option<f64> {
    let envelope = envelope(signal);
    let peak = envelope.iter().copied().fold(0.0f32, f32::max);
    let at = envelope.iter().position(|&x| x >= fraction * peak)?;
    Some(at as f64 * 128.0 / HZ * 1000.0)
}

/// When `signal`'s envelope last holds more than `fraction` of its peak,
/// in milliseconds: the end of a stage that falls to silence.
fn end_time_ms(signal: &[f32], fraction: f32) -> Option<f64> {
    let envelope = envelope(signal);
    let peak = envelope.iter().copied().fold(0.0f32, f32::max);
    let last = envelope.iter().rposition(|&x| x > fraction * peak)?;
    Some((last + 1) as f64 * 128.0 / HZ * 1000.0)
}

/// A device with the switches and values the tests share: Oscillator 1
/// alone, both filters wide open.
fn device_with(params: &[(ParamId, f64)]) -> bl_model::Device {
    let mut all = vec![
        (OSC2, 0.0),
        (NOISE, 0.0),
        (FILTER, 0.0),
        (FREQ, 20_000.0),
        (HP, 20.0),
        (RES, 0.0),
        (KEY, 0.0),
        (RETRIGGER, 1.0),
        (VOLUME, 1.0),
    ];
    all.extend_from_slice(params);
    device(&cascade(), &all)
}

// Spec: polysynth R1
#[test]
fn a_note_plays_at_the_notes_pitch() {
    let registry = registry();
    let mut node = node(&registry, &device_with(&[]), 4_096);
    let signal = play_blocks(node.as_mut(), 24_000, 4_096, &[on(0, 69, 1.0)]);
    let measured = peak_frequency(&signal[8_192..], 300.0, 600.0);
    assert!(
        (measured - 440.0).abs() < 1.0,
        "A4 played at {measured:.2} Hz"
    );
}

// Spec: polysynth R1 (the oscillators' aliasing)
#[test]
fn a_saw_at_the_top_key_aliases_below_minus_80_db() {
    let registry = registry();
    let params = [
        (OSC1_WAVE, 4.0),
        (OSC1_GAIN, db_to_gain(-6.0)),
        (OSC1_SHAPE, 0.0),
        (FILTER, 0.0),
    ];
    let mut node = node(&registry, &device_with(&params), 1 << 15);
    // The top key, where the band-limiting leaves the fundamental alone,
    // and a middle key, where a saw keeps many harmonics below Nyquist.
    for key in [127u8, 60] {
        let signal = play(node.as_mut(), 1 << 15, &[on(0, key, 1.0)]);
        // Skip the attack; measure the steady tone.
        let worst = aliasing_db(&signal[4_096..], frequency(key));
        println!("a saw at key {key}: aliasing {worst:.1} dB");
        assert!(worst < -80.0, "aliasing {worst:.1} dB at key {key}");
    }
}

// Spec: polysynth R2 (the low-pass filters' responses)
#[test]
fn the_low_pass_filters_match_their_prototypes_within_point_two_db() {
    let registry = registry();
    let cutoff = 1_000.0f64;
    let omega = |hz: f64| {
        (core::f64::consts::PI * hz / HZ).tan() / (core::f64::consts::PI * cutoff / HZ).tan()
    };
    // The kit's prototypes: a Butterworth second order for Type I, and a
    // fixed Butterworth section into a resonant one for Type II.
    let db = |gain: f64| 20.0 * gain.log10();
    let prototype = |kind: usize, hz: f64| {
        let s = omega(hz);
        let second = |k: f64| 1.0 / ((1.0 - s * s).powi(2) + (k * s).powi(2)).sqrt();
        if kind == 0 {
            second(2f64.sqrt())
        } else {
            // The kit's fourth-order pair: 2 sin(3π/8) then 2 sin(π/8).
            second(1.847_759) * second(0.765_366_9)
        }
    };
    for kind in 0..2 {
        let mut on_node = node(
            &registry,
            &device_with(&[
                (OSC1_WAVE, 0.0),
                (OSC1_GAIN, db_to_gain(-20.0)),
                (FILTER, 1.0),
                (FILTER_TYPE, kind as f64),
                (FREQ, cutoff),
                (HP, 20.0),
            ]),
            24_000,
        );
        let mut off_node = node(
            &registry,
            &device_with(&[
                (OSC1_WAVE, 0.0),
                (OSC1_GAIN, db_to_gain(-20.0)),
                (FILTER, 0.0),
            ]),
            24_000,
        );
        let mut worst = (0.0f64, 0.0f64);
        for target in [
            200.0, 400.0, 700.0, 1_000.0, 1_400.0, 2_000.0, 4_000.0, 8_000.0,
        ] {
            let key = key_for(target);
            let hz = frequency(key);
            let filtered = play(
                on_node.as_mut(),
                24_000,
                &[on(0, key, 1.0), off(20_000, key)],
            );
            let plain = play(
                off_node.as_mut(),
                24_000,
                &[on(0, key, 1.0), off(20_000, key)],
            );
            let measured = db(rms(&filtered[8_192..16_384]) / rms(&plain[8_192..16_384]));
            let expected = db(prototype(kind, hz));
            let dft = db(amplitude_at(&filtered[8_192..16_384], hz)
                / amplitude_at(&plain[8_192..16_384], hz));
            println!(
                "type {kind} at {hz:.0} Hz: {measured:.3} dB (dft {dft:.3} dB), prototype {expected:.3} dB"
            );
            if (measured - expected).abs() > worst.0 {
                worst = ((measured - expected).abs(), hz);
            }
        }
        assert!(
            worst.0 <= 0.2,
            "type {kind}: off by {:.3} dB at {:.0} Hz",
            worst.0,
            worst.1
        );
    }
}

// Spec: polysynth R3 (the envelopes' timing)
#[test]
fn the_envelopes_timing_is_within_five_percent() {
    let registry = registry();
    // The attack, straight, reaches full at its time.
    for attack_ms in [50.0, 200.0] {
        let mut device = device_with(&[(OSC1_WAVE, 0.0)]);
        device
            .params
            .insert(bl_devices::polysynth::ENV1_ATTACK, attack_ms);
        let mut node = node(&registry, &device, 24_000);
        let signal = play(node.as_mut(), 24_000, &[on(0, 96, 1.0)]);
        let measured = rise_time_ms(&signal, 0.99).expect("the attack reaches full");
        println!("attack {attack_ms} ms: {measured:.1} ms");
        assert!(
            (measured - attack_ms).abs() <= 0.05 * attack_ms,
            "attack {attack_ms} ms measured {measured:.1} ms"
        );
    }
    // The decay, from a fast attack to silence, ends at its time.
    let decay_ms = 200.0;
    let mut device = device_with(&[(OSC1_WAVE, 0.0)]);
    device
        .params
        .insert(bl_devices::polysynth::ENV1_ATTACK, 1.0);
    device
        .params
        .insert(bl_devices::polysynth::ENV1_DECAY, decay_ms);
    device
        .params
        .insert(bl_devices::polysynth::ENV1_SUSTAIN, 0.0);
    let mut node = node(&registry, &device, 24_000);
    let signal = play(node.as_mut(), 24_000, &[on(0, 96, 1.0)]);
    let measured = end_time_ms(&signal, 1e-5).expect("the decay reaches silence");
    println!("decay {decay_ms} ms ends: {measured:.1} ms");
    assert!(
        (measured - (decay_ms + 1.0)).abs() <= 0.05 * decay_ms,
        "decay {decay_ms} ms ended at {measured:.1} ms"
    );
}

// Spec: polysynth R4 (polyphony and stealing)
#[test]
fn sixteen_voices_play_and_a_seventeenth_steals_the_oldest() {
    let registry = registry();
    let mut node = node(&registry, &device_with(&[(OSC1_WAVE, 0.0)]), 4_096);
    let chord: Vec<NoteEvent> = (0..16).map(|i| on(0, 48 + i * 3, 0.8)).collect();
    let signal = play(node.as_mut(), 4_096, &chord);
    let level = rms(&signal[1_024..]);
    assert!(level > 0.01, "the chord sounds, {level:.4}");
    // A seventeenth note steals the oldest voice and the chord carries on.
    let mut extra = chord.clone();
    extra.push(on(2_048, 100, 0.8));
    let signal = play(node.as_mut(), 4_096, &extra);
    let level = rms(&signal[3_000..]);
    assert!(level > 0.01, "the chord carries on, {level:.4}");
}

// Spec: polysynth R4 (a released note is silent)
#[test]
fn a_released_note_falls_silent() {
    let registry = registry();
    let mut node = node(&registry, &device_with(&[(OSC1_WAVE, 0.0)]), 4_096);
    let signal = play_blocks(
        node.as_mut(),
        96_000,
        4_096,
        &[on(0, 60, 1.0), off(24_000, 60)],
    );
    let tail = rms(&signal[72_000..]);
    assert!(tail < 1e-6, "the tail is silent, {tail:.9}");
}

// Spec: polysynth R5 (the Shape knob)
#[test]
fn the_shape_knob_changes_the_wave() {
    let registry = registry();
    let mut plain = node(
        &registry,
        &device_with(&[(OSC1_WAVE, 4.0), (OSC1_SHAPE, 0.0)]),
        4_096,
    );
    let mut shaped = node(
        &registry,
        &device_with(&[(OSC1_WAVE, 4.0), (OSC1_SHAPE, 100.0)]),
        4_096,
    );
    let a = play(plain.as_mut(), 4_096, &[on(0, 60, 1.0)]);
    let b = play(shaped.as_mut(), 4_096, &[on(0, 60, 1.0)]);
    let difference = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(difference > 0.05, "the shapes differ by {difference:.3}");
}

// Spec: polysynth R6 (the retrigger switch)
#[test]
fn retrigger_restarts_the_oscillators_at_the_same_phase() {
    let registry = registry();
    for (retrigger, restarts) in [(1.0, true), (0.0, false)] {
        // One note, released quickly so its voice is free again, then the
        // same note: with retrigger on, the second note's first samples
        // are the first note's, from the same phase; off, they aren't.
        let mut device = device_with(&[(OSC1_WAVE, 4.0), (RETRIGGER, retrigger)]);
        device
            .params
            .insert(bl_devices::polysynth::ENV1_RELEASE, 1.0);
        let mut node = node(&registry, &device, 4_096);
        let signal = play(
            node.as_mut(),
            4_096,
            &[on(0, 60, 1.0), off(256, 60), on(1_024, 60, 1.0)],
        );
        let same = signal[..256] == signal[1_024..1_280];
        assert_eq!(
            same,
            restarts,
            "retrigger {retrigger}: the phase {} restart",
            if same { "didn't" } else { "did" }
        );
    }
}

// Spec: polysynth R12 (the saturation points follow the gains)
#[test]
fn louder_sources_drive_the_filters_saturators() {
    let registry = registry();
    let key = key_for(392.0);
    let hz = frequency(key);
    let mut levels = Vec::new();
    for db in [-20.0, 12.0] {
        let mut node = node(
            &registry,
            &device_with(&[
                (OSC1_WAVE, 0.0),
                (OSC1_GAIN, db_to_gain(db)),
                (FILTER, 1.0),
                (FREQ, 20_000.0),
            ]),
            24_000,
        );
        let signal = play(node.as_mut(), 24_000, &[on(0, key, 1.0), off(20_000, key)]);
        let window = &signal[8_192..16_384];
        let fundamental = amplitude_at(window, hz);
        let harmonic = amplitude_at(window, 2.0 * hz) + amplitude_at(window, 3.0 * hz);
        let ratio = harmonic / fundamental;
        println!(
            "gain {db} dB: harmonics {:.3} % of the fundamental",
            ratio * 100.0
        );
        levels.push(ratio);
    }
    assert!(
        levels[1] > 4.0 * levels[0] && levels[1] > 0.01,
        "the hotter gain distorts more: {:?}",
        levels
    );
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

// Spec: polysynth R6 (the voice budget)
#[test]
fn sixteen_voices_render_faster_than_the_device_budget() {
    let registry = registry();
    let mut node = node(
        &registry,
        &device_with(&[(OSC1_WAVE, 4.0), (FILTER, 1.0)]),
        128,
    );
    let seconds = 10;
    let frames = seconds * HZ as usize;
    let chord: Vec<NoteEvent> = (0..16).map(|i| on(0, 36 + i * 4, 0.8)).collect();
    let start = Instant::now();
    let signal = play_blocks(node.as_mut(), frames, 128, &chord);
    let elapsed = start.elapsed().as_secs_f64();
    let realtime = seconds as f64 / elapsed;
    println!("16 voices: {seconds} s of audio in {elapsed:.3} s, {realtime:.0}x real time (debug)");
    assert!(rms(&signal) > 0.01, "the chord sounds");
    if full_depth() {
        // A debug build on a loaded machine; a release build is many times
        // faster. 16 voices mustn't eat more than a quarter of the time
        // they play.
        assert!(
            realtime >= 4.0,
            "16 voices ran at {realtime:.1}x real time in a debug build"
        );
    }
}

// ---- WP-2.17, part 2: the LFO, the modulation routes and MPE ----

/// The index of `name` in a chooser's list, as a parameter value.
fn choice(list: &[&str], name: &str) -> f64 {
    list.iter()
        .position(|entry| *entry == name)
        .unwrap_or_else(|| panic!("{name} is one of {list:?}")) as f64
}

/// A note-on with its own note ID, so per-note expressions can name it.
fn on_id(offset: u32, key: u8, velocity: f32, id: u32) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::On,
        channel: 0,
        key,
        velocity,
        note_id: Some(id),
    }
}

/// A per-note expression for the note `id` (MPE).
fn expression(offset: u32, key: u8, id: u32, expression: NoteExpression, value: f32) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::Expression { expression, value },
        channel: 0,
        key,
        velocity: 0.0,
        note_id: Some(id),
    }
}

/// A channel message on channel 0 (WP-5.44).
fn message(offset: u32, kind: NoteKind) -> NoteEvent {
    NoteEvent {
        offset,
        kind,
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    }
}

/// The RMS of `signal`'s first difference: a crude high-frequency energy.
fn difference_rms(signal: &[f32]) -> f64 {
    let energy: f64 = signal
        .windows(2)
        .map(|pair| {
            let difference = f64::from(pair[1] - pair[0]);
            difference * difference
        })
        .sum();
    (energy / (signal.len().saturating_sub(1).max(1)) as f64).sqrt()
}

/// `signal`'s brightness one value every 10 ms, from its first difference.
fn brightness(signal: &[f32]) -> Vec<f64> {
    signal
        .chunks(480)
        .filter(|window| window.len() == 480)
        .map(difference_rms)
        .collect()
}

/// The strongest frequency in `series`, sampled at `rate` hertz, between
/// `low` and `high`: a slow DFT in 0.05 Hz steps.
fn series_peak(series: &[f64], rate: f64, low: f64, high: f64) -> f64 {
    let mean = series.iter().sum::<f64>() / series.len() as f64;
    let mut best = (0.0f64, 0.0f64);
    let mut hz = low;
    while hz <= high {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, value) in series.iter().enumerate() {
            let phase = core::f64::consts::TAU * hz * i as f64 / rate;
            re += (value - mean) * phase.cos();
            im -= (value - mean) * phase.sin();
        }
        let power = re * re + im * im;
        if power > best.1 {
            best = (hz, power);
        }
        hz += 0.05;
    }
    best.0
}

// Spec: polysynth R19, R20 (an LFO-to-cutoff route at the synced rate)
#[test]
fn an_lfo_to_cutoff_route_moves_the_cutoff_at_the_synced_rate() {
    let registry = registry();
    let node_for = |division: &str| -> Box<dyn Node> {
        let params = [
            (OSC1_WAVE, 4.0),
            (OSC1_GAIN, db_to_gain(-6.0)),
            (FILTER, 1.0),
            (FREQ, 400.0),
            (HP, 20.0),
            (RES, 0.0),
            (KEY, 0.0),
            (LFO_MODE, choice(&LFO_MODES, "Sync")),
            (LFO_SYNC, choice(&LFO_DIVISIONS, division)),
            (LFO_SHAPE, choice(&LFO_SHAPES, "Sine")),
            (LFO_RETRIGGER, 1.0),
            (LFO_AMOUNT, 100.0),
            (MOD1_SOURCE, choice(&MOD_SOURCES, "LFO")),
            (MOD1_DEST, choice(&MOD_DESTINATIONS, "LP Frequency")),
            (MOD1_AMOUNT, 30.0),
        ];
        node(&registry, &device_with(&params), 480)
    };
    // 120 BPM throughout: a quarter note is 0.5 seconds, a half note one
    // second, so the LFO runs at 2 and 1 hertz.
    let mut measured = Vec::new();
    for (division, expected) in [("1/4", 2.0), ("1/2", 1.0)] {
        let mut node = node_for(division);
        let signal = play_blocks(node.as_mut(), 96_000, 480, &[on(0, 40, 1.0)]);
        // Skip the attack's first 0.1 s.
        let series = brightness(&signal[4_800..]);
        let peak = series_peak(&series, HZ / 480.0, 0.5, 5.0);
        println!("the {division} LFO moved the cutoff at {peak:.2} Hz");
        assert!(
            (peak - expected).abs() < 0.15,
            "the {division} LFO ran at {peak:.2} Hz, wanted {expected}"
        );
        measured.push(peak);
    }
    let ratio = measured[0] / measured[1];
    assert!(
        (ratio - 2.0).abs() < 0.2,
        "the quarter note ran {ratio:.2} times the half note"
    );
}

// Spec: polysynth R21 (MPE's per-note pitch bend)
#[test]
fn mpe_pitch_bend_bends_one_note_only() {
    let registry = registry();
    let params = [
        (OSC1_WAVE, 0.0),
        (OSC1_GAIN, db_to_gain(-6.0)),
        (PB_RANGE, 12.0),
        (NOTE_PB, 1.0),
    ];
    let mut node = node(&registry, &device_with(&params), 8_192);
    // Two notes, C4 and C5, and a half-way bend for the first one's ID.
    let notes = [
        on_id(0, 60, 1.0, 1),
        on_id(0, 72, 1.0, 2),
        expression(48_000, 60, 1, NoteExpression::Bend, 0.5),
    ];
    let signal = play_blocks(node.as_mut(), 96_000, 8_192, &notes);
    let before = &signal[9_600..38_400];
    let after = &signal[57_600..86_400];
    let (c4, c4_bent, c5, c5_bent) = (261.63, 369.99, 523.25, 739.99);
    println!(
        "before: C4 {:.4}, bent C4 {:.4}, C5 {:.4}; after: C4 {:.4}, bent C4 {:.4}, C5 {:.4}, bent C5 {:.4}",
        amplitude_at(before, c4),
        amplitude_at(before, c4_bent),
        amplitude_at(before, c5),
        amplitude_at(after, c4),
        amplitude_at(after, c4_bent),
        amplitude_at(after, c5),
        amplitude_at(after, c5_bent)
    );
    assert!(amplitude_at(before, c4) > 0.05, "C4 sounds");
    assert!(amplitude_at(before, c5) > 0.05, "C5 sounds");
    assert!(
        amplitude_at(before, c4_bent) < 0.1 * amplitude_at(before, c4),
        "C4 hasn't bent yet"
    );
    // The bend moved the first note up six semitones, 12 × 0.5.
    assert!(
        amplitude_at(after, c4_bent) > 10.0 * amplitude_at(after, c4),
        "C4 bent to {c4_bent:.2} Hz"
    );
    // The second note didn't move.
    assert!(
        amplitude_at(after, c5) > 0.05,
        "C5 still sounds at its own pitch"
    );
    assert!(
        amplitude_at(after, c5_bent) < 0.1 * amplitude_at(after, c5),
        "C5 didn't bend"
    );
}

// Spec: polysynth R22, device-framework R39. The pitch wheel moves every
// note on its frame, by the PB Range.
#[test]
fn the_pitch_wheel_bends_every_note_by_the_pb_range() {
    let registry = registry();
    let params = [
        (OSC1_WAVE, 0.0),
        (OSC1_GAIN, db_to_gain(-6.0)),
        (PB_RANGE, 12.0),
    ];
    let mut node = node(&registry, &device_with(&params), 8_192);
    // Two notes, C4 and C5, a half-way bend up at 1 s and a full bend down
    // at 2 s.
    let notes = [
        on(0, 60, 1.0),
        on(0, 72, 1.0),
        message(48_000, NoteKind::PitchBend { value: 0.5 }),
        message(96_000, NoteKind::PitchBend { value: -1.0 }),
    ];
    let signal = play_blocks(node.as_mut(), 144_000, 8_192, &notes);
    let (before, up, down) = (
        &signal[9_600..38_400],
        &signal[57_600..86_400],
        &signal[105_600..134_400],
    );
    let strongest = |part: &[f32], hz: f64| amplitude_at(part, hz);
    let (c4, c5) = (261.63, 523.25);
    // Six semitones up from each, then an octave down from each.
    let (c4_up, c5_up) = (369.99, 739.99);
    let (c4_down, c5_down) = (130.81, 261.63);
    assert!(strongest(before, c4) > 0.05 && strongest(before, c5) > 0.05);
    assert!(strongest(before, c4_up) < 0.1 * strongest(before, c4));
    assert!(strongest(up, c4_up) > 10.0 * strongest(up, c4), "C4 up");
    assert!(strongest(up, c5_up) > 10.0 * strongest(up, c5), "C5 up");
    assert!(
        strongest(down, c4_down) > 10.0 * strongest(down, c4_up),
        "C4 down"
    );
    assert!(
        strongest(down, c5_down) > 10.0 * strongest(down, c5_up),
        "C5 down"
    );
}

// Spec: polysynth R21, device-framework R39. A change of the mod wheel
// (controller 1) moves the Modwheel source, from the frame it lands on.
#[test]
fn the_mod_wheel_moves_the_modwheel_source() {
    let registry = registry();
    let params = [
        (OSC1_WAVE, 0.0),
        (OSC1_GAIN, db_to_gain(-6.0)),
        (MOD1_SOURCE, choice(&MOD_SOURCES, "Modwheel")),
        (MOD1_DEST, choice(&MOD_DESTINATIONS, "Main Volume")),
        (MOD1_AMOUNT, 100.0),
    ];
    let mut node = node(&registry, &device_with(&params), 24_000);
    let wheel = NoteKind::Control {
        controller: 1,
        value: 1.0,
    };
    let notes = [on(0, 60, 1.0), message(12_000, wheel)];
    let signal = play_blocks(node.as_mut(), 24_000, 24_000, &notes);
    let before = rms(&signal[4_800..11_000]);
    let after = rms(&signal[14_000..23_000]);
    println!("mod wheel: {before:.5} then {after:.5}");
    assert!(after > 4.0 * before, "the mod wheel raised the level");
}

// Spec: polysynth R21 (MPE's pressure and slide as sources)
#[test]
fn mpe_pressure_and_slide_are_modulation_sources() {
    let registry = registry();
    let node_for = |source: &str, dest: &str, amount: f64, extra: &[(ParamId, f64)]| {
        let mut params = vec![
            (OSC1_WAVE, 0.0),
            (OSC1_GAIN, db_to_gain(-6.0)),
            (MOD1_SOURCE, choice(&MOD_SOURCES, source)),
            (MOD1_DEST, choice(&MOD_DESTINATIONS, dest)),
            (MOD1_AMOUNT, amount),
        ];
        params.extend_from_slice(extra);
        node(&registry, &device_with(&params), 24_000)
    };
    // Pressure moves the Main Volume: the note rises when it arrives.
    let mut node = node_for("Pressure", "Main Volume", 100.0, &[]);
    let notes = [
        on_id(0, 60, 1.0, 1),
        expression(12_000, 60, 1, NoteExpression::Pressure, 1.0),
    ];
    let signal = play_blocks(node.as_mut(), 24_000, 24_000, &notes);
    let before = rms(&signal[4_800..11_000]);
    let after = rms(&signal[14_000..23_000]);
    println!("pressure: {before:.5} then {after:.5}");
    assert!(after > 4.0 * before, "pressure raised the level");
    // Slide moves the low-pass cutoff: the note brightens.
    let mut node = node_for(
        "Slide",
        "LP Frequency",
        50.0,
        &[(FILTER, 1.0), (FREQ, 300.0), (OSC1_WAVE, 4.0)],
    );
    let notes = [
        on_id(0, 40, 1.0, 1),
        expression(12_000, 40, 1, NoteExpression::Slide, 1.0),
    ];
    let signal = play_blocks(node.as_mut(), 24_000, 24_000, &notes);
    let before = difference_rms(&signal[4_800..11_000]);
    let after = difference_rms(&signal[14_000..23_000]);
    println!("slide: {before:.5} then {after:.5}");
    assert!(after > 2.0 * before, "slide raised the cutoff");
}

// Spec: polysynth R20 (every Mod destination moves the sound)
#[test]
fn every_modulation_destination_moves_the_sound() {
    let registry = registry();
    let lfo = choice(&MOD_SOURCES, "LFO");
    let lp_freq = choice(&MOD_DESTINATIONS, "LP Frequency");
    let cases: Vec<(&str, Vec<(ParamId, f64)>)> = vec![
        ("Osc 1 Gain", vec![]),
        ("Osc 1 Shape", vec![(OSC1_WAVE, 4.0)]),
        ("Osc 2 Gain", vec![(OSC2, 1.0)]),
        ("Osc 2 Detune", vec![(OSC2, 1.0), (OSC2_DETUNE, 7.0)]),
        (
            "Noise Gain",
            vec![(NOISE, 1.0), (NOISE_GAIN, db_to_gain(-12.0))],
        ),
        ("LP Frequency", vec![(FILTER, 1.0), (FREQ, 500.0)]),
        (
            "LP Resonance",
            vec![(FILTER, 1.0), (FREQ, 500.0), (RES, 40.0)],
        ),
        ("HP Frequency", vec![(FILTER, 1.0), (HP, 400.0)]),
        (
            "LFO Rate",
            vec![
                (FILTER, 1.0),
                (FREQ, 500.0),
                (MOD2_SOURCE, lfo),
                (MOD2_DEST, lp_freq),
                (MOD2_AMOUNT, 50.0),
            ],
        ),
        ("Main Volume", vec![(VOLUME, 0.5)]),
    ];
    for (name, extra) in cases {
        let mut base = vec![(OSC1_WAVE, 0.0), (OSC1_GAIN, db_to_gain(-6.0))];
        base.extend(extra.iter().copied());
        let mut plain = node(&registry, &device_with(&base), 8_192);
        let mut moved_params = base.clone();
        moved_params.extend([
            (MOD1_SOURCE, choice(&MOD_SOURCES, "Env 1")),
            (MOD1_DEST, choice(&MOD_DESTINATIONS, name)),
            (MOD1_AMOUNT, 50.0),
        ]);
        let mut moved = node(&registry, &device_with(&moved_params), 8_192);
        let a = play(plain.as_mut(), 8_192, &[on(0, 60, 1.0)]);
        let b = play(moved.as_mut(), 8_192, &[on(0, 60, 1.0)]);
        let difference = a
            .iter()
            .zip(&b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max);
        assert!(
            difference > 1e-4,
            "the {name} route changed nothing ({difference})"
        );
    }
}

// Spec: polysynth R19 (the Ratio time mode follows the note)
#[test]
fn the_lfo_ratio_mode_modulates_the_note_at_its_own_rate() {
    let registry = registry();
    let node_for = |amount: f64| -> Box<dyn Node> {
        let params = [
            (OSC1_WAVE, 0.0),
            (OSC1_GAIN, db_to_gain(-6.0)),
            (LFO_MODE, choice(&LFO_MODES, "Ratio")),
            (LFO_RATIO, 1.0),
            (LFO_AMOUNT, 100.0),
            (PITCH_MOD1_SOURCE, choice(&MOD_SOURCES, "LFO")),
            (PITCH_MOD1_AMOUNT, amount),
        ];
        node(&registry, &device_with(&params), 24_000)
    };
    // Key 57 is A3, 220 Hz; an LFO at the note's own rate bends its pitch
    // at audio rate, which adds sidebands at 440 Hz and up.
    let mut plain = node_for(0.0);
    let mut fm = node_for(50.0);
    let a = play(plain.as_mut(), 24_000, &[on(0, 57, 1.0)]);
    let b = play(fm.as_mut(), 24_000, &[on(0, 57, 1.0)]);
    // The pitch bends by 2^(sin), whose average is the note times I₀(ln 2):
    // its lines sit at 248 Hz and each 220 Hz above that, so they show the
    // LFO running at the note's own rate.
    let shifted = amplitude_at(&b[4_800..], 248.0);
    let sideband = amplitude_at(&b[4_800..], 468.0);
    let plain_shifted = amplitude_at(&a[4_800..], 248.0);
    let plain_sideband = amplitude_at(&a[4_800..], 468.0);
    println!(
        "ratio mode: {shifted:.4} at 248 Hz, {sideband:.4} at 468 Hz; \
         without the route {plain_shifted:.4} and {plain_sideband:.4}"
    );
    assert!(shifted > 0.1, "the FM tone's shifted carrier is there");
    assert!(
        sideband > 0.05,
        "the ratio LFO adds FM sidebands ({sideband:.4})"
    );
    assert!(
        plain_shifted < 0.1 * shifted && plain_sideband < 0.1 * sideband,
        "without the route there are no sidebands"
    );
}

// Spec: polysynth R19 (Retrigger and the free-running LFO)
#[test]
fn a_free_running_lfo_is_shared_and_retrigger_restarts_it() {
    let registry = registry();
    let node_for = |retrigger: f64| -> Box<dyn Node> {
        let params = [
            (OSC1_WAVE, 0.0),
            (OSC1_GAIN, db_to_gain(-6.0)),
            (LFO_MODE, choice(&LFO_MODES, "Sync")),
            (LFO_SYNC, choice(&LFO_DIVISIONS, "1/4")),
            (LFO_SHAPE, choice(&LFO_SHAPES, "Square")),
            (LFO_RETRIGGER, retrigger),
            (LFO_AMOUNT, 100.0),
            (MOD1_SOURCE, choice(&MOD_SOURCES, "LFO")),
            (MOD1_DEST, choice(&MOD_DESTINATIONS, "Main Volume")),
            (MOD1_AMOUNT, 100.0),
        ];
        node(&registry, &device_with(&params), 24_000)
    };
    // A square LFO on the volume, 2 Hz at 120 BPM: silent from 0.25 to
    // 0.5 s. A second note starts at 0.25 s, where a free-running LFO is
    // low and a restarted one is high.
    let notes = [on(0, 60, 1.0), on(12_000, 67, 1.0)];
    let window = 12_480..23_040;
    let mut free = node_for(0.0);
    let free_signal = play_blocks(free.as_mut(), 24_000, 24_000, &notes);
    let mut restarted = node_for(1.0);
    let restarted_signal = play_blocks(restarted.as_mut(), 24_000, 24_000, &notes);
    let free_level = rms(&free_signal[window.clone()]);
    let restarted_level = rms(&restarted_signal[window]);
    println!("free-running {free_level:.5}, restarted {restarted_level:.5}");
    assert!(
        free_level < 1e-5,
        "a free-running LFO keeps both notes quiet there ({free_level:.5})"
    );
    assert!(
        restarted_level > 1e-2,
        "Retrigger starts the second note at the top ({restarted_level:.5})"
    );
}
