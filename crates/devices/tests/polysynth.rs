//! Cascade, the native polysynth (WP-2.17, part 1; the polysynth spec): the
//! oscillators' aliasing, the low-pass filters against their prototypes,
//! the envelopes' timing, polyphony and stealing, and the voice budget.
//!
//! Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::time::Instant;

use bl_engine::{
    Block, DeviceType, Node, NoteBuffer, NoteEvent, NoteKind, Registry, TransportInfo,
};
use bl_model::DeviceKind;
use bl_model::ParamId;
use bl_model::param::db_to_gain;
use common::{CHANNELS, RATE, device, node};
use realfft::RealFftPlanner;

use bl_devices::polysynth::{
    FILTER, FILTER_TYPE, FREQ, HP, KEY, NOISE, OSC1_GAIN, OSC1_SHAPE, OSC1_WAVE, OSC2, POLYSYNTH,
    RES, RETRIGGER, VOLUME,
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
