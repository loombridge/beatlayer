//! Drum Buss, the native drum processor (WP-5.22b; the drum-bus spec): Trim,
//! Comp, the three distortions with Drive, Crunch, Damp, Transients, the
//! Boom resonator with Freq, Force To Note and Decay, Boom Audition, the Bass
//! Meter, Dry/Wet and Output, and renders that don't depend on the block
//! size.

bl_rt::install_trap!();

mod common;

use bl_devices::drum_bus::{
    BOOM, BOOM_AUDITION, COMP, CRUNCH, DAMP, DECAY, DRIVE, DRUM_BUSS, DRY_WET, FORCE_TO_NOTE, FREQ,
    OUTPUT, TRANSIENTS, TRIM, TYPE, drum_buss_type, nearest_note_hz,
};
use bl_engine::{DeviceType, Node, Registry};
use bl_model::ParamId;
use bl_model::param::db_to_gain;
use common::{RATE, device, dft_at, node, noise, run};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::drum_bus::register(&mut registry);
    registry
}

/// A Drum Buss of `params`, up to `max_frames` at a time.
fn buss(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = drum_buss_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// A sine of `hz` and `amplitude`, `frames` long.
fn sine_of(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (amplitude * (std::f64::consts::TAU * hz * i as f64 / RATE.as_f64()).sin()) as f32)
        .collect()
}

/// The RMS of `samples`.
fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

/// The magnitude of `samples` at `hz`.
fn magnitude(samples: &[f32], hz: f64) -> f64 {
    let (re, im) = dft_at(samples, hz);
    re.hypot(im)
}

/// A sine's harmonic distortion: its harmonics 2 to 10 against its
/// fundamental, read from the last 4,800 of `samples`.
fn harmonic_distortion(samples: &[f32], hz: f64) -> f64 {
    let tail = &samples[samples.len() - 4_800..];
    let fundamental = magnitude(tail, hz);
    let harmonics: f64 = (2..=10)
        .map(|k| magnitude(tail, hz * f64::from(k)).powi(2))
        .sum();
    harmonics.sqrt() / fundamental
}

/// The gain, as output over input RMS, of a sine of `hz` at a small
/// amplitude, once it has settled.
fn small_signal_gain(params: &[(ParamId, f64)], hz: f64) -> f64 {
    let frames = 14_400;
    let input = sine_of(hz, 1e-5, frames);
    let out = run(buss(params, frames).as_mut(), &input, &[]);
    let settled = |s: &[f32]| rms(&s[s.len() - 4_800..]);
    settled(&out[0]) / settled(&input)
}

/// The frequency the device rings at, near `around`, from an impulse's tail.
fn ring_frequency(params: &[(ParamId, f64)], around: f64) -> f64 {
    let frames = 48_000;
    let mut impulse = vec![0.0f32; frames];
    impulse[0] = 1.0;
    let out = run(buss(params, frames).as_mut(), &impulse, &[]);
    let tail = &out[0][2_000..];
    let (from, to) = (around * 0.9, around * 1.1);
    let steps = 4_000;
    let mut best = (from, 0.0);
    for k in 0..=steps {
        let hz = from + (to - from) * k as f64 / steps as f64;
        let value = magnitude(tail, hz);
        if value > best.1 {
            best = (hz, value);
        }
    }
    best.0
}

/// The resonator's ring-down time constant, from an impulse's tail.
fn ring_down_seconds(params: &[(ParamId, f64)]) -> f64 {
    let frames = 48_000;
    let mut impulse = vec![0.0f32; frames];
    impulse[0] = 1.0;
    let out = run(buss(params, frames).as_mut(), &impulse, &[]);
    let window = 2_000;
    let space = 9_600;
    let at = |start: usize| rms(&out[0][start..start + window]);
    let ratio = at(2_400 + space) / at(2_400);
    -(space as f64) / (RATE.as_f64() * ratio.ln())
}

// Spec: drum-bus R1
#[test]
fn the_drum_bus_is_described_and_laid_out() {
    let device_type = drum_buss_type();
    device_type.description.check().unwrap();
    assert_eq!(
        device_type.kind,
        bl_model::DeviceKind::Native {
            device_type: DRUM_BUSS.to_owned()
        }
    );
    assert_eq!(device_type.description.params.len(), 14);
    assert!(
        bl_devices::device_types()
            .iter()
            .any(|t| t.name == "Drum Buss")
    );
    assert_eq!(buss(&[], 64).latency(), 0, "no latency");
}

// Spec: drum-bus R2
#[test]
fn trim_and_output_reach_silence() {
    let input = noise(2_048);
    let silenced = run(buss(&[(TRIM, 0.0)], 2_048).as_mut(), &input, &[]);
    assert!(silenced[0].iter().all(|v| *v == 0.0), "Trim at silence");
    let silenced = run(buss(&[(OUTPUT, 0.0)], 2_048).as_mut(), &input, &[]);
    assert!(silenced[0].iter().all(|v| *v == 0.0), "Output at silence");
}

// Spec: drum-bus R3
#[test]
fn comp_reduces_the_levels_of_a_loud_hit() {
    let frames = 24_000;
    // The compressor's fixed curve takes more from a louder hit than a
    // quieter one, so the two levels' ratio narrows.
    let level = |comp: f64, amplitude: f64| {
        let input = sine_of(120.0, amplitude, frames);
        let out = run(buss(&[(COMP, comp)], frames).as_mut(), &input, &[]);
        rms(&out[0][frames - 9_600..])
    };
    let ratio_off = level(0.0, 0.8) / level(0.0, 0.2);
    let ratio_on = level(1.0, 0.8) / level(1.0, 0.2);
    assert!(
        ratio_on < 0.75 * ratio_off,
        "the compressor narrows the levels: {ratio_on} against {ratio_off}"
    );
}

// Spec: drum-bus R4, R5
#[test]
fn the_three_distortions_distort_more_from_the_first_to_the_third() {
    let frames = 24_000;
    // A tenth of full scale, driven 24 dB, so the shapes saturate together.
    let input = sine_of(1_000.0, 0.1, frames);
    let distortion: Vec<f64> = [0, 1, 2]
        .iter()
        .map(|kind| {
            let out = run(
                buss(&[(DRIVE, 100.0), (TYPE, f64::from(*kind))], frames).as_mut(),
                &input,
                &[],
            );
            harmonic_distortion(&out[0], 1_000.0)
        })
        .collect();
    let [soft, medium, hard] = [distortion[0], distortion[1], distortion[2]];
    assert!(soft < medium, "Soft {soft} against Medium {medium}");
    assert!(medium < hard, "Medium {medium} against Hard {hard}");
}

// Spec: drum-bus R6
#[test]
fn crunch_adds_harmonics_to_the_mid_highs_only() {
    let frames = 24_000;
    let mid = sine_of(1_000.0, 0.3, frames);
    let clean = run(buss(&[], frames).as_mut(), &mid, &[]);
    let crunched = run(buss(&[(CRUNCH, 100.0)], frames).as_mut(), &mid, &[]);
    assert!(
        harmonic_distortion(&crunched[0], 1_000.0) > 4.0 * harmonic_distortion(&clean[0], 1_000.0),
        "Crunch adds harmonics to the mid-highs"
    );
    // A tone below the crossover is left alone.
    let low = sine_of(80.0, 0.3, frames);
    let clean = run(buss(&[], frames).as_mut(), &low, &[]);
    let crunched = run(buss(&[(CRUNCH, 100.0)], frames).as_mut(), &low, &[]);
    let tail = |s: &[f32]| rms(&s[s.len() - 9_600..]);
    assert!(
        (20.0 * (tail(&crunched[0]) / tail(&clean[0])).log10()).abs() < 0.5,
        "the low band is untouched"
    );
}

// Spec: drum-bus R7
#[test]
fn damps_corner_is_where_the_spec_puts_it() {
    for damp in [25.0, 50.0, 100.0] {
        let params = [(DAMP, damp)];
        let corner = 20_000.0 * (200.0f64 / 20_000.0).powf(damp / 100.0);
        // Where the response falls 3 dB below its low-frequency gain.
        let steps = 160;
        let from = corner / 8.0;
        let to = (corner * 8.0).min(RATE.as_f64() * 0.45);
        let at = |k: usize| from * (to / from).powf(k as f64 / steps as f64);
        let gains: Vec<f64> = (0..=steps)
            .map(|k| small_signal_gain(&params, at(k)))
            .collect();
        let level = gains[0] / 2f64.sqrt();
        let first = (1..=steps)
            .find(|k| gains[*k] < level && gains[*k - 1] >= level)
            .expect("the corner is on the sweep");
        let t = (level - gains[first - 1]) / (gains[first] - gains[first - 1]);
        let found = (at(first - 1).ln() + t * (at(first).ln() - at(first - 1).ln())).exp();
        assert!(
            (found / corner - 1.0).abs() < 0.02,
            "Damp {damp}: corner {found} against {corner}"
        );
    }
}

// Spec: drum-bus R8
#[test]
fn transients_raises_the_attack_to_sustain_ratio_above_a_hundred_hertz() {
    let frames = 28_800;
    // A drum hit: a 250 Hz tone decaying over about a fifth of a second.
    let hit: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f64 / RATE.as_f64();
            (0.8 * (-t / 0.05).exp() * (std::f64::consts::TAU * 250.0 * t).sin()) as f32
        })
        .collect();
    let attack = 0..240;
    let sustain = 2_400..7_200;
    let shape = |transients: f64| {
        let out = run(
            buss(&[(TRANSIENTS, transients)], frames).as_mut(),
            &hit,
            &[],
        );
        (rms(&out[0][attack.clone()]), rms(&out[0][sustain.clone()]))
    };
    let (attack_0, sustain_0) = shape(0.0);
    let (attack_pos, sustain_pos) = shape(100.0);
    let (_, sustain_neg) = shape(-100.0);
    assert!(
        attack_pos / sustain_pos > attack_0 / sustain_0,
        "positive raises the attack-to-sustain ratio"
    );
    assert!(
        sustain_neg < 0.95 * sustain_0,
        "negative lowers the sustain: {sustain_neg} against {sustain_0}"
    );
    // A tone below 100 Hz is left within half a dB.
    let low = sine_of(50.0, 0.3, frames);
    let clean = run(buss(&[], frames).as_mut(), &low, &[]);
    let shaped = run(buss(&[(TRANSIENTS, 100.0)], frames).as_mut(), &low, &[]);
    let tail = |s: &[f32]| rms(&s[s.len() - 9_600..]);
    assert!(
        (20.0 * (tail(&shaped[0]) / tail(&clean[0])).log10()).abs() < 0.5,
        "the band below 100 Hz is within half a dB"
    );
}

// Spec: drum-bus R9, R10
#[test]
fn booms_resonator_rings_at_freq_and_force_to_note_snaps_it() {
    let params = [(BOOM, 100.0), (FREQ, 80.0), (DECAY, 500.0)];
    let found = ring_frequency(&params, 80.0);
    assert!(
        (found / 80.0 - 1.0).abs() < 0.01,
        "Freq 80: rings at {found}"
    );
    // Force To Note snaps 200 Hz to the nearest equal-tempered note, G3.
    let snapped = [(BOOM, 100.0), (FREQ, 200.0), (FORCE_TO_NOTE, 1.0)];
    let want = nearest_note_hz(200.0);
    let found = ring_frequency(&snapped, want);
    assert!(
        (found / want - 1.0).abs() < 0.01,
        "Force To Note: rings at {found} against {want}"
    );
}

// Spec: drum-bus R11
#[test]
fn decay_sets_the_resonators_ring_down_time() {
    for decay_ms in [100.0, 300.0] {
        let params = [(BOOM, 100.0), (FREQ, 80.0), (DECAY, decay_ms)];
        let found = ring_down_seconds(&params);
        let want = decay_ms * 0.001;
        assert!(
            (found / want - 1.0).abs() < 0.10,
            "Decay {decay_ms} ms: ring-down {found} s against {want} s"
        );
    }
}

// Spec: drum-bus R12
#[test]
fn boom_audition_outputs_only_the_enhancer() {
    let frames = 24_000;
    let input = sine_of(1_000.0, 0.5, frames);
    let full = run(buss(&[(BOOM, 100.0)], frames).as_mut(), &input, &[]);
    let solo = run(
        buss(&[(BOOM, 100.0), (BOOM_AUDITION, 1.0)], frames).as_mut(),
        &input,
        &[],
    );
    let tail = |s: &[f32]| rms(&s[s.len() - 9_600..]);
    assert!(tail(&full[0]) > 0.1, "the signal passes with Boom");
    assert!(
        tail(&solo[0]) < 0.1 * tail(&full[0]),
        "Boom Audition plays only the enhancer: {} against {}",
        tail(&solo[0]),
        tail(&full[0])
    );
}

// Spec: drum-bus R13
#[test]
fn the_bass_meter_is_readable_headlessly() {
    let frames = 8_192;
    let input = sine_of(60.0, 0.5, frames);
    let mut node = buss(&[(BOOM, 100.0), (FREQ, 60.0), (DECAY, 500.0)], 4_096);
    let mut reader = node.display().expect("the device has a display");
    run(node.as_mut(), &input[..4_096], &[]);
    run(node.as_mut(), &input[4_096..], &[]);
    let data = reader.read();
    assert_eq!(data.values.len(), 1);
    assert!(
        data.values[0] > -60.0,
        "the meter reads {} dB",
        data.values[0]
    );

    // At Boom 0 the enhancer is silent and the meter sits on its floor.
    let mut node = buss(&[(BOOM, 0.0)], 4_096);
    let mut reader = node.display().expect("the device has a display");
    run(node.as_mut(), &input[..4_096], &[]);
    let data = reader.read();
    assert_eq!(data.values[0], -120.0, "at rest");

    let shown = bl_devices::display(
        DRUM_BUSS,
        bl_model::DeviceId(bl_core::id::Id::new(1).unwrap()),
        &drum_buss_type().description,
        &bl_model::OrdMap::new(),
        Some(&data.values),
    )
    .unwrap();
    assert_eq!(shown.reading("Bass Meter").unwrap().text, "-120.0 dB");
}

// Spec: drum-bus R14
#[test]
fn dry_wet_at_zero_is_the_input() {
    let input = noise(2_048);
    let out = run(buss(&[(DRY_WET, 0.0)], 2_048).as_mut(), &input, &[]);
    assert_eq!(out[0], input);
}

// Spec: drum-bus R15
#[test]
fn renders_are_bit_identical_at_any_block_size() {
    let input = noise(8_192);
    // Gain parameters take a linear amplitude factor, so a dB setting is
    // converted: `-3.0` alone would clamp to silence and the render below
    // would compare zeros.
    let params = [
        (TRIM, db_to_gain(-3.0)),
        (COMP, 1.0),
        (TYPE, 2.0),
        (DRIVE, 60.0),
        (CRUNCH, 40.0),
        (DAMP, 30.0),
        (TRANSIENTS, 50.0),
        (BOOM, 60.0),
        (FREQ, 70.0),
        (DECAY, 300.0),
        (DRY_WET, 80.0),
        (OUTPUT, db_to_gain(-2.0)),
    ];
    let render = |block: usize| -> Vec<f32> {
        let mut node = buss(&params, block);
        input
            .chunks(block)
            .flat_map(|chunk| run(node.as_mut(), chunk, &[])[0].clone())
            .collect()
    };
    let reference = render(4_096);
    assert!(
        reference.iter().any(|s| s.abs() > 1e-3),
        "the render isn't silence"
    );
    for block in [1, 64, 512] {
        assert!(render(block) == reference, "block size {block}");
    }
}

// Spec: drum-bus R1, R15
#[test]
fn a_set_with_drum_buss_saves_and_reopens() {
    use bl_model::{Command, Device, DeviceKind, Editor, TrackKind};

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: "Drums".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let mut dev = Device::new(
        editor.new_id(),
        "Drum Buss",
        DeviceKind::Native {
            device_type: DRUM_BUSS.to_owned(),
        },
    );
    for (param, value) in [
        (TRIM, -4.0),
        (COMP, 1.0),
        (TYPE, 2.0),
        (DRIVE, 70.0),
        (CRUNCH, 30.0),
        (DAMP, 20.0),
        (TRANSIENTS, -40.0),
        (BOOM, 80.0),
        (FREQ, 90.0),
        (FORCE_TO_NOTE, 1.0),
        (DECAY, 400.0),
        (BOOM_AUDITION, 1.0),
        (DRY_WET, 90.0),
        (OUTPUT, -6.0),
    ] {
        dev.params.insert(param, value);
    }
    let dev_id = dev.id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: dev,
        })
        .unwrap();

    let saved = editor.document().clone();
    let reopened = Editor::from_document(saved, bl_core::id::IdAllocator::new());
    let reopened_dev = reopened
        .document()
        .track(track)
        .unwrap()
        .devices()
        .find(|d| d.id == dev_id)
        .unwrap();
    assert_eq!(reopened_dev.params.get(&TRIM), Some(&-4.0));
    assert_eq!(reopened_dev.params.get(&TYPE), Some(&2.0));
    assert_eq!(reopened_dev.params.get(&DRIVE), Some(&70.0));
    assert_eq!(reopened_dev.params.get(&TRANSIENTS), Some(&-40.0));
    assert_eq!(reopened_dev.params.get(&FORCE_TO_NOTE), Some(&1.0));
    assert_eq!(reopened_dev.params.get(&BOOM_AUDITION), Some(&1.0));
    assert_eq!(reopened_dev.params.get(&OUTPUT), Some(&-6.0));
}
