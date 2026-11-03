//! Analog, part 1 (WP-8.14; the Analog spec, R1 to R17): the oscillators and
//! the noise generator, the filters, Drive, the routing, the envelopes'
//! sound, and the panel's pages.
//!
//! Every block runs as a real-time section under the allocation trap; the
//! envelopes' own laws are unit tests in the module, on its test clock.

use crate::common;
use std::f64::consts::{PI, TAU};

use bl_engine::{
    Block, DeviceType, Node, NoteBuffer, NoteEvent, NoteKind, Registry, TransportInfo,
};
use bl_model::param::db_to_gain;
use bl_model::{DeviceKind, ParamId};
use common::{CHANNELS, RATE, device, node};
use realfft::RealFftPlanner;

use bl_devices::analog::{
    self, ANALOG, DRIVES, LOOPS, MODES, SHAPES, SUB_SYNC, amp, env, filter, noise, osc,
};

/// The sample rate the tests run at, in hertz.
const HZ: f64 = 48_000.0;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

fn analog_type() -> DeviceType {
    bl_devices::device_types()
        .into_iter()
        .find(|t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == ANALOG))
        .expect("Analog is registered")
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
/// the left and right channels.
fn play(node: &mut dyn Node, frames: usize, notes: &[NoteEvent]) -> [Vec<f32>; 2] {
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
    [data[..frames].to_vec(), data[frames..].to_vec()]
}

/// Plays `notes` for `frames` in blocks of `block`.
fn play_blocks(
    node: &mut dyn Node,
    frames: usize,
    block: usize,
    notes: &[NoteEvent],
) -> [Vec<f32>; 2] {
    let mut signal = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut at = 0;
    while at < frames {
        let len = block.min(frames - at);
        let here: Vec<NoteEvent> = notes
            .iter()
            .filter(|n| (n.offset as usize) >= at && (n.offset as usize) < at + len)
            .map(|n| NoteEvent {
                offset: n.offset - at as u32,
                ..*n
            })
            .collect();
        let [l, r] = play(node, len, &here);
        signal[0].extend(l);
        signal[1].extend(r);
        at += len;
    }
    signal
}

/// The frequency of `key` with A4 at 440 Hz.
fn frequency(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// The note key whose frequency is nearest `hz`.
fn key_for(hz: f64) -> u8 {
    (69.0 + 12.0 * (hz / 440.0).log2())
        .round()
        .clamp(0.0, 127.0) as u8
}

fn rms(signal: &[f32]) -> f64 {
    (signal
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        / signal.len() as f64)
        .sqrt()
}

/// The amplitude of `hz` in `signal`, by a Hann-windowed DFT.
fn amplitude_at(signal: &[f32], hz: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &x) in signal.iter().enumerate() {
        let w = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
        let phase = TAU * hz * i as f64 / HZ;
        re += f64::from(x) * w * phase.cos();
        im -= f64::from(x) * w * phase.sin();
    }
    2.0 * re.hypot(im) / n
}

/// The loudest frequency in `signal` between `low` and `high`.
fn peak_frequency(signal: &[f32], low: f64, high: f64) -> f64 {
    const N: usize = 16_384;
    let frame = N.min(signal.len());
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(frame);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    for (n, (slot, &x)) in input.iter_mut().zip(&signal[..frame]).enumerate() {
        let w = 0.5 - 0.5 * (TAU * n as f64 / frame as f64).cos();
        *slot = f64::from(x) * w;
    }
    fft.process(&mut input, &mut spectrum).unwrap();
    let bin = HZ / frame as f64;
    let mut best = (0.0, 0.0);
    for (k, value) in spectrum.iter().enumerate() {
        let hz = k as f64 * bin;
        if (low..high).contains(&hz) && value.norm_sqr() > best.1 {
            best = (hz, value.norm_sqr());
        }
    }
    best.0
}

/// The energy outside a note's harmonics over the energy in them, in dB.
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
            let x = TAU * i as f64 / (N - 1) as f64;
            A.iter()
                .enumerate()
                .map(|(k, a)| a * (k as f64 * x).cos() * if k % 2 == 0 { 1.0 } else { -1.0 })
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
            let lowest = ((hz - margin) / fundamental).ceil().max(1.0);
            let highest = ((hz + margin) / fundamental).floor();
            if lowest <= highest && lowest * fundamental < HZ / 2.0 {
                harmonic += value.norm_sqr();
            } else {
                other += value.norm_sqr();
            }
        }
    }
    10.0 * (other / harmonic).log10()
}

/// The averaged power spectrum of `signal`, in 8,192-frame Hann windows
/// overlapping by half: one value for each of the 4,097 bins.
fn power_spectrum(signal: &[f32]) -> Vec<f64> {
    const N: usize = 8_192;
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(N);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let mut sum = vec![0.0; N / 2 + 1];
    let mut count = 0.0;
    for start in (0..=signal.len() - N).step_by(N / 2) {
        for (n, (slot, &x)) in input.iter_mut().zip(&signal[start..start + N]).enumerate() {
            let w = 0.5 - 0.5 * (TAU * n as f64 / N as f64).cos();
            *slot = f64::from(x) * w;
        }
        fft.process(&mut input, &mut spectrum).unwrap();
        for (total, value) in sum.iter_mut().zip(&spectrum) {
            *total += value.norm_sqr();
        }
        count += 1.0;
    }
    sum.iter().map(|s| s / count).collect()
}

/// The mean power of `spectrum` between `low` and `high` hertz.
fn band_power(spectrum: &[f64], low: f64, high: f64) -> f64 {
    let bin = HZ / ((spectrum.len() - 1) * 2) as f64;
    let (a, b) = ((low / bin) as usize, (high / bin) as usize);
    spectrum[a..b].iter().sum::<f64>() / (b - a) as f64
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

/// A device with the parameters the tests share: Oscillator 1 alone, all
/// to Filter 1, a sine at 0 dB; Filter 1 on and wide open, Filter 2 off;
/// both amplifiers on at 0 dB, Amplifier 1 left and Amplifier 2 right; both
/// amplifier envelopes full at once and held.
fn device_with(params: &[(ParamId, f64)]) -> bl_model::Device {
    let mut all = vec![
        (osc::id(0, osc::LEVEL), 1.0),
        (osc::id(0, osc::BALANCE), 0.0),
        (osc::id(0, osc::SHAPE), 0.0),
        (amp::id(0, amp::PAN), -100.0),
        (amp::id(1, amp::PAN), 100.0),
        (env::id(2, env::ATTACK), 0.0),
        (env::id(2, env::DECAY), 0.0),
        (env::id(2, env::SUSTAIN), 100.0),
        (env::id(3, env::ATTACK), 0.0),
        (env::id(3, env::DECAY), 0.0),
        (env::id(3, env::SUSTAIN), 100.0),
    ];
    all.extend_from_slice(params);
    device(&analog_type(), &all)
}

fn choice(list: &[&str], name: &str) -> f64 {
    list.iter().position(|n| *n == name).unwrap() as f64
}

/// A node of `device_with(params)`, played `notes` for `frames`.
fn render(params: &[(ParamId, f64)], frames: usize, notes: &[NoteEvent]) -> [Vec<f32>; 2] {
    let mut node = node(&registry(), &device_with(params), frames);
    play(node.as_mut(), frames, notes)
}

// Spec: analog R1, R2
#[test]
fn analog_is_listed_under_instruments_and_a_note_plays_at_its_pitch() {
    let ty = analog_type();
    assert_eq!(ty.name, "Analog");
    assert_eq!(ty.role, bl_model::DeviceRole::Instrument);
    for (shape, wave) in [("Sine", 0), ("Saw", 1), ("Rectangle", 2)] {
        let [left, _] = render(
            &[(osc::id(0, osc::SHAPE), choice(&SHAPES, shape))],
            24_000,
            &[on(0, 69, 1.0)],
        );
        let measured = peak_frequency(&left[8_192..], 300.0, 600.0);
        assert!(
            (measured - 440.0).abs() < 1.5,
            "{shape} played {measured} Hz ({wave})"
        );
    }
}

// Spec: analog R2, R4
#[test]
fn octave_semi_detune_and_key_move_the_pitch_as_specified() {
    let at = |params: &[(ParamId, f64)], key: u8| {
        let [left, _] = render(params, 32_768, &[on(0, key, 1.0)]);
        peak_frequency(&left[8_192..], 100.0, 4_000.0)
    };
    let a4 = 440.0;
    let octave = at(&[(osc::id(0, osc::OCTAVE), 1.0)], 69);
    assert!((octave - 2.0 * a4).abs() < 2.0, "octave {octave}");
    let semi = at(&[(osc::id(0, osc::SEMI), -12.0)], 69);
    assert!((semi - a4 / 2.0).abs() < 2.0, "semi {semi}");
    let detune = at(&[(osc::id(0, osc::DETUNE), 100.0)], 69);
    assert!(
        (detune - a4 * 2f64.powf(1.0 / 12.0)).abs() < 2.0,
        "detune {detune}"
    );
    // Key at 0 % holds C3 whatever the key; at 200 % two semitones a key.
    let held = at(&[(osc::id(0, osc::KEY), 0.0)], 72);
    assert!((held - frequency(60)).abs() < 2.0, "key 0 % {held}");
    let wide = at(&[(osc::id(0, osc::KEY), 200.0)], 62);
    assert!((wide - frequency(64)).abs() < 2.0, "key 200 % {wide}");
}

// Spec: analog R2, R4: the pitch envelope glides in a straight line in semitones.
#[test]
fn the_pitch_envelope_starts_at_its_initial_pitch_and_glides_to_the_note() {
    let [left, _] = render(
        &[
            (osc::id(0, osc::ENV_INITIAL), 12.0),
            (osc::id(0, osc::ENV_TIME), 500.0),
        ],
        48_000,
        &[on(0, 57, 1.0)],
    );
    let early = peak_frequency(&left[1_000..9_192], 100.0, 2_000.0);
    let late = peak_frequency(&left[40_000..], 100.0, 2_000.0);
    assert!(early > 300.0, "an octave up at the start: {early} Hz");
    assert!(
        (late - frequency(57)).abs() < 3.0,
        "the note's pitch at the end: {late} Hz"
    );
}

// Spec: analog R3
#[test]
fn saw_and_rectangle_fold_back_less_than_minus_80_db_at_every_key() {
    for shape in ["Saw", "Rectangle"] {
        for pulse in [50.0, 10.0] {
            for key in [127u8, 60] {
                let [left, _] = render(
                    &[
                        (osc::id(0, osc::SHAPE), choice(&SHAPES, shape)),
                        (osc::id(0, osc::LEVEL), db_to_gain(-6.0)),
                        (osc::id(0, osc::PULSE_WIDTH), pulse),
                    ],
                    1 << 15,
                    &[on(0, key, 1.0)],
                );
                let worst = aliasing_db(&left[4_096..], frequency(key));
                assert!(
                    worst < -80.0,
                    "{shape} {pulse} % at key {key}: {worst:.1} dB"
                );
            }
        }
    }
}

// Spec: analog R3
#[test]
fn pulse_width_sets_the_rectangles_duty_cycle() {
    for width in [10.0, 25.0, 50.0, 75.0, 90.0] {
        let [left, _] = render(
            &[
                (osc::id(0, osc::SHAPE), choice(&SHAPES, "Rectangle")),
                (osc::id(0, osc::PULSE_WIDTH), width),
            ],
            24_000,
            &[on(0, 36, 1.0)],
        );
        let steady = &left[8_000..];
        let high = steady.iter().filter(|&&x| x > 0.0).count() as f64 / steady.len() as f64;
        assert!(
            (high * 100.0 - width).abs() < 1.5,
            "{width} % played {:.1} %",
            high * 100.0
        );
    }
}

// Spec: analog R5
#[test]
fn the_sub_adds_a_square_an_octave_down_and_sync_raises_the_synced_pitch() {
    let amount = |mode: &str, amount: f64| {
        render(
            &[
                (osc::id(0, osc::SHAPE), choice(&SHAPES, "Saw")),
                (osc::id(0, osc::SUB_SYNC), choice(&SUB_SYNC, mode)),
                (osc::id(0, osc::SUB_SYNC_AMOUNT), amount),
            ],
            32_768,
            &[on(0, 57, 1.0)],
        )[0]
        .clone()
    };
    let f = frequency(57);
    let plain = amount("Off", 100.0);
    let sub = amount("Sub", 100.0);
    assert!(amplitude_at(&plain[8_192..], f / 2.0) < 0.005);
    assert!(
        amplitude_at(&sub[8_192..], f / 2.0) > 0.3,
        "the sub sounds an octave down"
    );
    let half = amount("Sub", 50.0);
    let ratio = amplitude_at(&half[8_192..], f / 2.0) / amplitude_at(&sub[8_192..], f / 2.0);
    assert!((ratio - 0.5).abs() < 0.05, "the sub's level: {ratio}");
    // Sync: the synced oscillator's pitch is the note's raised by the ratio,
    // 1 at 0 % and 8 at 100 %; between, the cycle keeps the note's pitch.
    let unsynced = amount("Sync", 0.0);
    assert!(
        (amplitude_at(&unsynced[8_192..], f) - amplitude_at(&plain[8_192..], f)).abs() < 0.01,
        "a ratio of 1 is the plain oscillator"
    );
    let eight = amount("Sync", 100.0);
    let at_eight = peak_frequency(&eight[8_192..], 100.0, 6_000.0);
    assert!(
        (at_eight - 8.0 * f).abs() < 4.0,
        "ratio 8 at {at_eight} Hz, not {} Hz",
        8.0 * f
    );
    let between = amount("Sync", 50.0);
    let third = amplitude_at(&between[8_192..], 3.0 * f);
    assert!(
        third > amplitude_at(&between[8_192..], 2.0 * f)
            && third > amplitude_at(&between[8_192..], 4.0 * f),
        "a ratio of 2.8 peaks at the third harmonic of the note's pitch"
    );
}

// Spec: analog R6
#[test]
fn the_noise_falls_at_six_decibels_an_octave_above_its_color() {
    let spectrum_of = |color: f64| {
        let [left, _] = render(
            &[
                (osc::id(0, osc::ON), 0.0),
                (noise::id(noise::ON), 1.0),
                (noise::id(noise::BALANCE), 0.0),
                (noise::id(noise::COLOR), color),
            ],
            1 << 17,
            &[on(0, 60, 1.0)],
        );
        power_spectrum(&left[4_096..])
    };
    let open = spectrum_of(20_000.0);
    let low = spectrum_of(1_000.0);
    // Flat white: the same power in two bands.
    let flat = db(band_power(&open, 3_500.0, 4_500.0) / band_power(&open, 400.0, 600.0));
    assert!(flat.abs() < 1.5, "white noise isn't flat: {flat:.1} dB");
    // A one-pole low-pass at 1 kHz: 4 kHz is 1 / (1 + 16) down, 500 Hz 1 / 1.25.
    let colour = db(band_power(&low, 3_500.0, 4_500.0) / band_power(&low, 400.0, 600.0));
    let expected = db((1.0 + 0.25) / (1.0 + 16.0));
    assert!(
        (colour - expected).abs() < 1.5,
        "{colour:.1} dB, expected {expected:.1} dB"
    );
    // Each voice has noise of its own.
}

/// The kit's filter prototypes at a 1 kHz cutoff: the gain at `hz` for
/// `mode`, from the bilinear-warped frequency ratio `s`.
fn prototype(mode: &str, hz: f64) -> f64 {
    let s = (PI * hz / HZ).tan() / (PI * 1_000.0 / HZ).tan();
    let second = |k: f64| ((1.0 - s * s).powi(2) + (k * s).powi(2)).sqrt();
    match mode {
        "LP12" => 1.0 / second(2f64.sqrt()),
        "LP24" => 1.0 / (second(1.847_759) * second(0.765_366_9)),
        "HP12" => s * s / second(2f64.sqrt()),
        "HP24" => s.powi(4) / (second(1.847_759) * second(0.765_366_9)),
        "BP12" => s / second(1.0),
        "BP24" => (s / second(1.0)).powi(2),
        "Notch12" => (1.0 - s * s).abs() / second(2.0),
        "Notch24" => ((1.0 - s * s).abs() / second(2.0)).powi(2),
        _ => unreachable!(),
    }
}

// Spec: analog R7
#[test]
fn each_filter_mode_matches_its_prototype_within_point_two_db() {
    let registry = registry();
    for mode in [
        "LP12", "LP24", "BP12", "BP24", "Notch12", "Notch24", "HP12", "HP24",
    ] {
        let mut worst = (0.0f64, 0.0f64);
        for target in [200.0, 400.0, 700.0, 1_400.0, 2_000.0, 4_000.0, 8_000.0] {
            let key = key_for(target);
            let hz = frequency(key);
            let notes = [on(0, key, 1.0)];
            let mut filtered = node(
                &registry,
                &device_with(&[
                    (filter::id(0, filter::MODE), choice(&MODES, mode)),
                    (filter::id(0, filter::FREQ), 1_000.0),
                ]),
                24_000,
            );
            let mut plain = node(
                &registry,
                &device_with(&[(filter::id(0, filter::ON), 0.0)]),
                24_000,
            );
            let a = play(filtered.as_mut(), 24_000, &notes)[0].clone();
            let b = play(plain.as_mut(), 24_000, &notes)[0].clone();
            let measured = 20.0 * (rms(&a[8_192..16_384]) / rms(&b[8_192..16_384])).log10();
            let expected = 20.0 * prototype(mode, hz).log10();
            if (measured - expected).abs() > worst.0 {
                worst = ((measured - expected).abs(), hz);
            }
        }
        assert!(
            worst.0 <= 0.2,
            "{mode}: off by {:.3} dB at {:.0} Hz",
            worst.0,
            worst.1
        );
    }
}

// Spec: analog R8
#[test]
fn the_formant_modes_peak_at_the_vowels_of_peterson_and_barney() {
    const VOWELS: [(&str, f64, f64, f64); 5] = [
        ("A", 0.0, 730.0, 1_090.0),
        ("E", 25.0, 530.0, 1_840.0),
        ("I", 50.0, 270.0, 2_290.0),
        ("O", 75.0, 570.0, 840.0),
        ("U", 100.0, 300.0, 870.0),
    ];
    let spectrum_of = |mode: &str, reso: f64, freq: f64| {
        let [left, _] = render(
            &[
                (osc::id(0, osc::ON), 0.0),
                (noise::id(noise::ON), 1.0),
                (noise::id(noise::BALANCE), 0.0),
                (filter::id(0, filter::MODE), choice(&MODES, mode)),
                (filter::id(0, filter::FREQ), freq),
                (filter::id(0, filter::RESO), reso),
            ],
            1 << 17,
            &[on(0, 60, 1.0)],
        );
        power_spectrum(&left[4_096..])
    };
    let peak_near = |spectrum: &[f64], hz: f64| {
        let bin = HZ / ((spectrum.len() - 1) * 2) as f64;
        let (a, b) = (((hz * 0.8) / bin) as usize, ((hz * 1.25) / bin) as usize);
        let (k, _) = spectrum[a..b]
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(y.1))
            .unwrap();
        (a + k) as f64 * bin
    };
    for mode in ["Formant12", "Formant24"] {
        for (vowel, reso, f1, f2) in VOWELS {
            let spectrum = spectrum_of(mode, reso, 1_000.0);
            for formant in [f1, f2] {
                let at = peak_near(&spectrum, formant);
                assert!(
                    (at - formant).abs() < 0.06 * formant,
                    "{mode} {vowel}: peak at {at:.0} Hz, the table has {formant} Hz"
                );
            }
        }
        // Freq transposes both formants together: 2 kHz doubles them.
        let spectrum = spectrum_of(mode, 50.0, 2_000.0);
        for formant in [540.0, 4_580.0] {
            let at = peak_near(&spectrum, formant);
            assert!(
                (at - formant).abs() < 0.06 * formant,
                "{mode} I at 2 kHz: {at:.0} Hz"
            );
        }
    }
}

/// The gain of Filter 2's path, in dB, for a note `key`'s sine against the
/// same with the filters off.
fn left_right(params: &[(ParamId, f64)], key: u8) -> [f64; 2] {
    let [l, r] = render(params, 24_000, &[on(0, key, 1.0)]);
    [rms(&l[8_192..16_384]), rms(&r[8_192..16_384])]
}

// Spec: analog R9, R12: serial and parallel routing reach the output as the
// manual's diagram shows.
#[test]
fn sources_reach_their_filters_and_to_f2_passes_filter_1_into_filter_2() {
    let lowpass = [
        (filter::id(0, filter::MODE), choice(&MODES, "LP24")),
        (filter::id(0, filter::FREQ), 110.0),
        (filter::id(1, filter::ON), 1.0),
    ];
    let params = |extra: &[(ParamId, f64)]| {
        let mut all = lowpass.to_vec();
        all.extend_from_slice(extra);
        all
    };
    // Parallel: Oscillator 1 to Filter 1 only reaches Amplifier 1 (left).
    let [left, right] = left_right(&params(&[]), 69);
    assert!(left > 1e-4, "Amplifier 1 has the oscillator");
    assert!(right < 1e-6, "Filter 2 has nothing: {right}");
    // Oscillator 1 to Filter 2 only reaches Amplifier 2 (right), unfiltered by Filter 1.
    let [left, right] = left_right(&params(&[(osc::id(0, osc::BALANCE), 100.0)]), 69);
    assert!(
        left < 1e-6 && right > 0.1,
        "Filter 2 has the oscillator: {left} {right}"
    );
    // Serial: To F2 at 100 % sends Filter 1's output to Filter 2.
    let serial = left_right(&params(&[(filter::id(0, filter::EXTRA), 100.0)]), 69);
    assert!(
        (serial[1] / serial[0] - 1.0).abs() < 0.02,
        "Filter 2 passes Filter 1: {serial:?}"
    );
    let half = left_right(&params(&[(filter::id(0, filter::EXTRA), 50.0)]), 69);
    assert!(
        (half[1] / half[0] - 0.5).abs() < 0.02,
        "To F2 at 50 %: {half:?}"
    );
    // With Filter 2 off, its amplifier takes its sources unfiltered.
    let [_, right] = left_right(
        &[
            (osc::id(0, osc::BALANCE), 100.0),
            (filter::id(0, filter::MODE), choice(&MODES, "LP24")),
        ],
        69,
    );
    assert!(
        right > 0.1,
        "an unfiltered share reaches Amplifier 2: {right}"
    );
}

// Spec: analog R9: Follow uses Filter 1's modulated cutoff.
#[test]
fn follow_moves_filter_2s_cutoff_with_filter_1s_modulated_cutoff() {
    let gain_at = |follow: bool, key: u8| {
        // A sine of 1,047 Hz, fixed by Key at 0 %; Filter 1 is off, and tracks the key.
        let [_, right] = left_right(
            &[
                (osc::id(0, osc::KEY), 0.0),
                (osc::id(0, osc::OCTAVE), 2.0),
                (osc::id(0, osc::BALANCE), 100.0),
                (filter::id(0, filter::ON), 0.0),
                (filter::id(0, filter::FREQ), 500.0),
                (filter::id(0, filter::FREQ_KEY), 100.0),
                (filter::id(1, filter::ON), 1.0),
                (filter::id(1, filter::MODE), choice(&MODES, "LP24")),
                (filter::id(1, filter::FREQ), 632.455_5),
                (filter::id(1, filter::EXTRA), f64::from(follow)),
            ],
            key,
        );
        20.0 * right.log10()
    };
    // At offset 0, Filter 2's cutoff is Filter 1's: 500 Hz at C3, an octave up at C4.
    let low = gain_at(true, 60);
    let high = gain_at(true, 72);
    assert!(
        high - low > 20.0,
        "following opens the filter: {low:.1} dB to {high:.1} dB"
    );
    let fixed = gain_at(false, 60) - gain_at(false, 72);
    assert!(
        fixed.abs() < 0.1,
        "without Follow the cutoff stays: {fixed:.2} dB"
    );
}

// Spec: analog R11
#[test]
fn drive_adds_odd_harmonics_in_the_symmetric_modes_and_even_ones_in_the_asymmetric() {
    let harmonics = |drive: &str| {
        let [left, _] = render(
            &[
                (osc::id(0, osc::LEVEL), 0.4),
                (filter::id(0, filter::DRIVE), choice(&DRIVES, drive)),
            ],
            32_768,
            &[on(0, 57, 1.0)],
        );
        let f = frequency(57);
        let steady = &left[8_192..];
        let fundamental = amplitude_at(steady, f);
        [2.0, 3.0].map(|n| amplitude_at(steady, n * f) / fundamental)
    };
    let clean = harmonics("Off");
    assert!(clean[1] < 1e-3, "no drive, no harmonics: {clean:?}");
    let mut previous = 0.0;
    for mode in ["Sym 1", "Sym 2", "Sym 3"] {
        let [even, odd] = harmonics(mode);
        assert!(
            even < 1e-3,
            "{mode}: only odd harmonics, the second at {even}"
        );
        assert!(odd > previous, "{mode} distorts more than the one before");
        previous = odd;
    }
    for mode in ["Asym 1", "Asym 2", "Asym 3"] {
        let [even, _] = harmonics(mode);
        assert!(even > 0.01, "{mode} adds the second harmonic: {even}");
    }
}

// Spec: analog R12: Level, Pan and the envelope as the level's shape.
#[test]
fn amplifiers_pan_and_scale_by_level() {
    let [left, right] = render(&[], 24_000, &[on(0, 69, 1.0)]);
    assert!(rms(&left[8_192..]) > 0.1 && rms(&right[8_192..]) < 1e-6);
    let level = |gain: f64| {
        rms(&render(&[(amp::id(0, amp::LEVEL), gain)], 24_000, &[on(0, 69, 1.0)])[0][8_192..])
    };
    assert!((level(0.5) / level(1.0) - 0.5).abs() < 0.01);
    assert_eq!(level(0.0), 0.0, "at -inf the amplifier is silent");
    let centred = render(&[(amp::id(0, amp::PAN), 0.0)], 24_000, &[on(0, 69, 1.0)]);
    let (l, r) = (rms(&centred[0][8_192..]), rms(&centred[1][8_192..]));
    assert!(
        (l - r).abs() < 1e-3,
        "centre pans both channels equally: {l} {r}"
    );
}

// Spec: analog R13: the attack and the release as the envelope's laws give them.
#[test]
fn the_amplifier_envelope_shapes_the_note() {
    let rise = |velocity: f32, attack_velocity: f64| {
        let [left, _] = render(
            &[
                (env::id(2, env::ATTACK), 1_600.0),
                (env::id(2, env::ATT_VEL), attack_velocity),
                (osc::id(0, osc::KEY), 0.0),
            ],
            96_000,
            &[on(0, 60, velocity)],
        );
        let peak = left.iter().fold(0.0f32, |p, x| p.max(x.abs()));
        left.chunks(64)
            .position(|w| w.iter().any(|x| x.abs() >= 0.5 * peak))
            .unwrap() as f64
            * 64.0
            / HZ
    };
    let slow = rise(1.0, 0.0);
    assert!(
        (slow - 0.8).abs() < 0.02,
        "half-way through a 1.6 s attack at {slow:.3} s"
    );
    let quick = rise(1.0, 100.0);
    assert!(
        (slow / quick - 16.0).abs() < 1.5,
        "velocity 127 is sixteen times faster: {}",
        slow / quick
    );
    // Release: straight to silence over its time.
    let [left, _] = render(
        &[(env::id(2, env::RELEASE), 100.0)],
        24_000,
        &[on(0, 69, 1.0), off(12_000, 69)],
    );
    assert!(rms(&left[11_000..12_000]) > 0.1);
    assert!(rms(&left[17_500..18_500]) < 0.02, "released after 100 ms");
    assert!(rms(&left[12_000..12_800]) > 0.05, "not at once");
}

// Spec: analog R13, R15
#[test]
fn every_loop_mode_is_listed_and_loops_an_amplifier_envelope_while_the_key_is_down() {
    let held = |loop_mode: &str| {
        let [left, _] = render(
            &[
                (env::id(2, env::ATTACK), 20.0),
                (env::id(2, env::DECAY), 20.0),
                (env::id(2, env::SUSTAIN), 0.0),
                (env::id(2, env::RELEASE), 20.0),
                (env::id(2, env::LOOP), choice(&LOOPS, loop_mode)),
                (osc::id(0, osc::KEY), 0.0),
            ],
            48_000,
            &[on(0, 60, 1.0)],
        );
        // The number of times the level rises through half in the second half.
        let level: Vec<f32> = left[24_000..]
            .chunks(48)
            .map(|w| w.iter().fold(0.0f32, |p, x| p.max(x.abs())))
            .collect();
        let peak = level.iter().copied().fold(0.0f32, f32::max);
        level
            .windows(2)
            .filter(|w| w[0] < 0.5 * peak && w[1] >= 0.5 * peak)
            .count()
    };
    assert_eq!(held("Off"), 0, "without a loop the note dies away");
    assert!(held("AD-R") >= 8, "AD-R repeats attack and decay");
    assert!(
        held("ADR-R") >= 8,
        "ADR-R repeats attack, decay and release"
    );
    assert_eq!(held("ADS-R"), 0, "ADS-R holds its sustain");
}

// Spec: analog R7, R9 to R12, R17: the renders are the same at any block size.
#[test]
fn a_phrase_renders_the_same_at_block_sizes_1_64_512_and_4096() {
    let params = [
        (osc::id(0, osc::SHAPE), choice(&SHAPES, "Rectangle")),
        (osc::id(0, osc::SUB_SYNC), choice(&SUB_SYNC, "Sub")),
        (osc::id(0, osc::SUB_SYNC_AMOUNT), 60.0),
        (osc::id(1, osc::ON), 1.0),
        (osc::id(1, osc::LEVEL), 0.5),
        (osc::id(1, osc::BALANCE), 100.0),
        (osc::id(1, osc::SUB_SYNC), choice(&SUB_SYNC, "Sync")),
        (osc::id(1, osc::SUB_SYNC_AMOUNT), 40.0),
        (noise::id(noise::ON), 1.0),
        (noise::id(noise::LEVEL), 0.2),
        (filter::id(0, filter::MODE), choice(&MODES, "LP24")),
        (filter::id(0, filter::FREQ), 1_500.0),
        (filter::id(0, filter::RESO), 60.0),
        (filter::id(0, filter::DRIVE), choice(&DRIVES, "Asym 2")),
        (filter::id(0, filter::FREQ_ENV), 50.0),
        (filter::id(0, filter::RES_ENV), 40.0),
        (filter::id(0, filter::EXTRA), 50.0),
        (filter::id(1, filter::ON), 1.0),
        (filter::id(1, filter::MODE), choice(&MODES, "Formant24")),
        (filter::id(1, filter::FREQ), 1_000.0),
        (filter::id(1, filter::RESO), 30.0),
        (env::id(0, env::ATTACK), 30.0),
        (env::id(0, env::DECAY), 200.0),
        (env::id(0, env::SUSTAIN), 30.0),
        (env::id(2, env::ATTACK), 5.0),
        (env::id(2, env::DECAY), 300.0),
        (env::id(2, env::SUSTAIN), 60.0),
        (env::id(2, env::SLOPE), 1.0),
        (env::id(3, env::LOOP), choice(&LOOPS, "ADR-R")),
        (env::id(3, env::ATTACK), 30.0),
        (env::id(3, env::DECAY), 40.0),
        (env::id(3, env::RELEASE), 40.0),
    ];
    let notes = [
        on(0, 48, 0.9),
        on(2_000, 55, 0.6),
        off(9_000, 48),
        on(10_001, 72, 1.0),
        off(15_000, 55),
        off(16_000, 72),
    ];
    let run = |block: usize| {
        let mut node = node(&registry(), &device_with(&params), 4_096);
        play_blocks(node.as_mut(), 20_000, block, &notes)
    };
    let reference = run(4_096);
    assert!(rms(&reference[0]) > 0.001, "the phrase is audible");
    for block in [1, 64, 512] {
        let rendered = run(block);
        for (channel, (a, b)) in reference.iter().zip(&rendered).enumerate() {
            let worst = a
                .iter()
                .zip(b)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max);
            assert!(
                worst == 0.0,
                "block {block}, channel {channel}: differs by {worst}"
            );
        }
    }
}

// Spec: analog R16: every section's page places its controls inside the
// body without overlap, and each parameter exactly once across the pages.
#[test]
fn the_shell_places_every_page_without_overlap_and_every_control_once() {
    use bl_model::device_layout::{PageChoice, Rect};
    let ty = analog_type();
    let layout = ty.description.layout.clone();
    layout.check(&ty.description.params).unwrap();
    let docked = layout.docked_size();
    assert!(docked.height <= bl_model::device_layout::BODY_HEIGHT + 0.5);
    let overlap = |a: &Rect, b: &Rect| {
        a.x < b.right() - 1e-3
            && b.x < a.right() - 1e-3
            && a.y < b.bottom() - 1e-3
            && b.y < a.bottom() - 1e-3
    };
    assert_eq!(analog::SECTIONS.len(), 8);
    for page in analog::SECTIONS {
        let shown = layout.with_pages(&PageChoice::from([(
            analog::PAGES.to_owned(),
            page.to_owned(),
        )]));
        let placed = shown.place(docked);
        assert_eq!(
            shown.docked_size(),
            docked,
            "{page}: the device keeps its size"
        );
        let whole = Rect::new(0.0, 0.0, docked.width, docked.height);
        for (i, a) in placed.controls.iter().enumerate() {
            assert!(whole.contains(&a.rect), "{page}: {a:?} is outside the body");
            for b in &placed.controls[i + 1..] {
                assert!(!overlap(&a.rect, &b.rect), "{page}: {a:?} overlaps {b:?}");
            }
        }
        assert_eq!(placed.pages[0].names, analog::SECTIONS);
    }
    let first = layout.place(docked);
    assert_eq!(first.pages[0].shown, 0, "a new device shows Oscillator 1");
}

// Spec: analog R4, R17. A moving pitch renders the same at 64, 512 and 4,096
// frames a block; the kit places a mipmap level's change by block until it
// places its pitch ramps by frame, so a block of one frame differs.
#[test]
fn a_gliding_pitch_renders_the_same_at_block_sizes_64_512_and_4096() {
    let params = [
        (osc::id(0, osc::SHAPE), choice(&SHAPES, "Rectangle")),
        (osc::id(0, osc::ENV_INITIAL), 7.0),
        (osc::id(0, osc::ENV_TIME), 150.0),
    ];
    let notes = [
        on(0, 48, 0.9),
        on(2_000, 55, 0.6),
        off(9_000, 48),
        off(12_000, 55),
    ];
    let run = |block: usize| {
        let mut node = node(&registry(), &device_with(&params), 4_096);
        play_blocks(node.as_mut(), 16_000, block, &notes)
    };
    let reference = run(4_096);
    for block in [64, 512] {
        assert!(run(block) == reference, "block {block} differs");
    }
}

// Spec: analog R10: the filter's envelope moves the cutoff by up to six octaves.
#[test]
fn freq_mod_env_moves_the_cutoff_by_the_envelopes_level() {
    // A sine two octaves above C3, 1,047 Hz. The cutoff is 125 Hz;
    // the filter envelope held at 50 % lifts it by three octaves, to 1 kHz,
    // where a 24 dB low-pass is 3 dB down.
    let gain = |env_amount: f64| {
        let params = |on: f64| {
            vec![
                (osc::id(0, osc::KEY), 0.0),
                (osc::id(0, osc::OCTAVE), 2.0),
                (osc::id(0, osc::SEMI), 0.0),
                (osc::id(0, osc::DETUNE), 0.0),
                (filter::id(0, filter::ON), on),
                (filter::id(0, filter::MODE), choice(&MODES, "LP24")),
                (filter::id(0, filter::FREQ), 125.0),
                (filter::id(0, filter::FREQ_ENV), env_amount),
                (env::id(0, env::ATTACK), 0.0),
                (env::id(0, env::DECAY), 0.0),
                (env::id(0, env::SUSTAIN), 50.0),
            ]
        };
        let filtered = render(&params(1.0), 24_000, &[on(0, 60, 1.0)])[0].clone();
        let plain = render(&params(0.0), 24_000, &[on(0, 60, 1.0)])[0].clone();
        20.0 * (rms(&filtered[8_192..16_384]) / rms(&plain[8_192..16_384])).log10()
    };
    let lifted = gain(100.0);
    let expected = 20.0 * prototype("LP24", frequency(84)).log10();
    assert!(
        (lifted - expected).abs() < 1.0,
        "{lifted:.2} dB, a 24 dB low-pass at its cutoff is {expected:.2} dB"
    );
    assert!(
        gain(0.0) < -30.0,
        "without the envelope the cutoff stays at 125 Hz"
    );
}

// Spec: analog R17: sixteen voices play and a seventeenth steals one.
#[test]
fn sixteen_voices_play_and_a_seventeenth_steals_the_oldest() {
    let chord: Vec<NoteEvent> = (0..17).map(|i| on(i * 64, 36 + i as u8 * 3, 0.8)).collect();
    let mut node = node(
        &registry(),
        &device_with(&[
            (osc::id(0, osc::SHAPE), choice(&SHAPES, "Saw")),
            (filter::id(0, filter::MODE), choice(&MODES, "LP24")),
            (filter::id(0, filter::RESO), 50.0),
            (filter::id(0, filter::FREQ), 2_000.0),
        ]),
        4_096,
    );
    let signal = play_blocks(node.as_mut(), 24_000, 512, &chord);
    assert!(
        signal[0].iter().all(|x| x.is_finite()),
        "no sample is lost to a number"
    );
    assert!(rms(&signal[0][20_000..]) > 0.05, "the chord sounds");
}

// Spec: analog R5, R17: Sync works in an engine whose blocks are a single frame.
#[test]
fn sync_renders_in_a_node_whose_blocks_are_one_frame() {
    let params = [
        (osc::id(0, osc::SUB_SYNC), choice(&SUB_SYNC, "Sync")),
        (osc::id(0, osc::SUB_SYNC_AMOUNT), 50.0),
    ];
    let mut small = node(&registry(), &device_with(&params), 1);
    let signal = play_blocks(small.as_mut(), 4_000, 1, &[on(0, 57, 1.0)]);
    let mut large = node(&registry(), &device_with(&params), 4_096);
    let reference = play_blocks(large.as_mut(), 4_000, 4_096, &[on(0, 57, 1.0)]);
    assert!(rms(&signal[0]) > 0.1);
    assert!(signal == reference, "the node's block size changes nothing");
}
