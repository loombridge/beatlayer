//! Electric (WP-8.23; spec: electric): the hammer, the fork, the damper and
//! pickup, and the global section.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common::{self, CHANNELS};
use bl_core::time::SampleRate;
use bl_devices::electric::*;
use bl_engine::{Block, Node, NoteBuffer, NoteEvent, NoteKind, TransportInfo};
use bl_model::ParamId;

const RATE: SampleRate = common::RATE;
const HZ: f64 = 48_000.0;

/// A pickup that stays in its linear range, and no damper noise, so a test
/// sees the fork.
const LINEAR: [(ParamId, f64); 2] = [(INPUT, 0.02), (DAMPER_LEVEL, 0.0)];

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

fn node(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let device = common::device(&electric_type(), params);
    common::node(&common::registry(), &device, 4096)
}

/// Plays `notes` for `frames` frames in blocks of `block`, and returns the
/// left channel.
fn play_blocks(node: &mut dyn Node, frames: usize, block: usize, notes: &[NoteEvent]) -> Vec<f32> {
    let mut signal = Vec::with_capacity(frames);
    let mut at = 0;
    let mut out = NoteBuffer::with_capacity(64);
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
        let mut data = vec![0.0f32; CHANNELS * len];
        {
            let mut b = Block::new(
                &mut data,
                CHANNELS,
                len,
                len,
                &[],
                TransportInfo::default(),
                RATE,
            )
            .with_notes(&here, &mut out);
            bl_rt::rt_test!({ node.process(&mut b) });
        }
        signal.extend_from_slice(&data[..len]);
        at += len;
    }
    signal
}

/// A fresh device with `params`, playing `notes` for `frames` frames.
fn render(params: &[(ParamId, f64)], notes: &[NoteEvent], frames: usize) -> Vec<f32> {
    play_blocks(node(params).as_mut(), frames, 4096, notes)
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn hz_of(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// The Hann-windowed amplitude of `signal[from..from + len]` at `hz`.
fn magnitude(signal: &[f32], hz: f64, from: usize, len: usize) -> f64 {
    let (mut re, mut im, mut sum) = (0.0, 0.0, 0.0);
    for (i, s) in signal[from..from + len].iter().enumerate() {
        let window = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / len as f64).cos();
        let angle = std::f64::consts::TAU * hz * (from + i) as f64 / HZ;
        re += f64::from(*s) * window * angle.cos();
        im += f64::from(*s) * window * angle.sin();
        sum += window;
    }
    2.0 * re.hypot(im) / sum
}

/// The frequency near `hz` at which `signal[from..from + len]` is strongest,
/// scanned in steps of a tenth of a cent over three cents either way.
fn pitch_near(signal: &[f32], hz: f64, from: usize, len: usize) -> f64 {
    (-30..=30)
        .map(|step| hz * 2f64.powf(f64::from(step) / 12_000.0))
        .max_by(|a, b| {
            magnitude(signal, *a, from, len).total_cmp(&magnitude(signal, *b, from, len))
        })
        .unwrap()
}

fn cents(hz: f64, of: f64) -> f64 {
    1200.0 * (hz / of).log2()
}

/// How long `hz` takes to fall 60 dB in `signal`, from its level in the
/// windows of `len` frames centred on frames `a` and `b`.
fn t60(signal: &[f32], hz: f64, a: usize, b: usize, len: usize) -> f64 {
    let level = |centre: usize| magnitude(signal, hz, centre - len / 2, len);
    let fall = 20.0 * (level(a) / level(b)).log10();
    60.0 * (b - a) as f64 / HZ / fall
}

fn energy(signal: &[f32]) -> f64 {
    signal.iter().map(|s| f64::from(*s) * f64::from(*s)).sum()
}

// Spec: electric R1
#[test]
fn electric_is_an_instrument_with_the_three_sections_and_the_global_one() {
    use bl_model::DeviceRole;
    use bl_model::device_layout::{LayoutNode, PageStyle};
    let types = bl_devices::device_types();
    let electric = types
        .iter()
        .find(|t| t.name == "Electric")
        .expect("Electric is registered");
    assert_eq!(electric.role, DeviceRole::Instrument);
    let layout = &electric.description.layout;
    let pages = layout.pages();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].style, PageStyle::Shell);
    let names: Vec<&str> = pages[0].pages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Hammer", "Fork", "Damper/Pickup"]);
    // The global section sits beside the pages, always shown.
    let LayoutNode::Row(row) = &layout.root else {
        panic!("the body is a row");
    };
    assert!(matches!(row[0], LayoutNode::Pages(_)));
    assert!(matches!(row[1], LayoutNode::Band(_)));
}

// Spec: electric R1
#[test]
fn the_factory_presets_set_parameters_in_range_and_sound() {
    let description = electric_type().description;
    for (name, settings) in PRESETS {
        for (id, value) in settings {
            let param = description
                .param(*id)
                .expect("the preset's parameter exists");
            assert_eq!(param.clamp(*value), *value, "{name}: {}", param.name);
        }
        let s = render(settings, &[on(0, 48, 0.8), on(0, 64, 0.8)], 24_000);
        assert!(s.iter().all(|x| x.is_finite()), "{name}");
        assert!(peak(&s) > 0.01, "{name} sounds");
    }
}

// Spec: electric R2
#[test]
fn a_hammers_pulse_sums_to_one_and_is_5_ms_soft_and_a_fifth_of_a_ms_hard() {
    assert!((pulse_seconds(0.0) - 0.005).abs() < 1e-12);
    assert!((pulse_seconds(1.0) - 0.0002).abs() < 1e-12);
    for width in [10, 48, 240] {
        let sum: f32 = (0..width + 5).map(|n| pulse(n, width)).sum();
        assert!((sum - 1.0).abs() < 1e-4, "{width}: {sum}");
    }
}

// Spec: electric R2, R14
#[test]
fn force_sets_the_attacks_height_and_stiffness_its_brightness() {
    let attack = 2_400;
    let play = |extra: &[(ParamId, f64)]| {
        let mut params = LINEAR.to_vec();
        params.push((NOISE, 0.0));
        params.extend_from_slice(extra);
        render(&params, &[on(0, 48, 0.8)], 12_000)
    };
    // Twice the force makes the attack twice as high.
    let half = play(&[(FORCE, 30.0), (STIFFNESS, 50.0)]);
    let full = play(&[(FORCE, 60.0), (STIFFNESS, 50.0)]);
    let ratio = peak(&full[..attack]) / peak(&half[..attack]);
    assert!((ratio - 2.0).abs() < 0.05, "{ratio}");
    // A harder hammer is shorter, so the tine's second partial rings
    // louder against its fundamental.
    let brightness = |stiffness| {
        let s = play(&[(STIFFNESS, stiffness)]);
        let hz = hz_of(48);
        magnitude(&s, hz * 6.267, 0, 8_192) / magnitude(&s, hz, 0, 8_192)
    };
    let (soft, hard) = (brightness(0.0), brightness(100.0));
    assert!(hard > 10.0 * soft, "soft {soft}, hard {hard}");
}

// Spec: electric R2, R14
#[test]
fn velocity_moves_the_sound_only_through_the_amounts_and_otherwise_only_its_level() {
    let shape = |velocity, extra: &[(ParamId, f64)]| {
        let s = render(extra, &[on(0, 60, velocity)], 12_000);
        let top = peak(&s);
        s.iter().map(|x| x / top).collect::<Vec<f32>>()
    };
    let distance = |a: &[f32], b: &[f32]| {
        a.iter()
            .zip(b)
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()))
    };
    // At every amount 0, two velocities sound alike but for level, noise
    // and damper included.
    let (soft, loud) = (shape(0.3, &[]), shape(0.9, &[]));
    assert!(distance(&soft, &loud) < 1e-3, "{}", distance(&soft, &loud));
    let louder = render(&[], &[on(0, 60, 0.9)], 4_800);
    let softer = render(&[], &[on(0, 60, 0.3)], 4_800);
    assert!(peak(&louder) > 2.5 * peak(&softer));
    // With an amount, they sound different.
    let moved = [(FORCE_VEL, 100.0), (STIFFNESS_VEL, 100.0)];
    let (soft, loud) = (shape(0.3, &moved), shape(0.9, &moved));
    assert!(distance(&soft, &loud) > 0.05, "{}", distance(&soft, &loud));
}

// Spec: electric R2
#[test]
fn the_noise_is_a_burst_by_its_level_decay_pitch_and_key() {
    let heard = |extra: &[(ParamId, f64)], key: u8| {
        let mut all = vec![(FORCE, 0.0), (DAMPER_LEVEL, 0.0)];
        all.extend_from_slice(extra);
        render(&all, &[on(0, key, 1.0)], 48_000)
    };
    // At 0 there is none; at 100 % it is a burst that falls over its Decay.
    assert!(heard(&[(NOISE, 0.0)], 60).iter().all(|s| *s == 0.0));
    let short = heard(&[(NOISE, 100.0), (NOISE_DECAY, 20.0)], 60);
    let long = heard(&[(NOISE, 100.0), (NOISE_DECAY, 500.0)], 60);
    assert!(energy(&short[..2_400]) > 1e-3);
    assert!(energy(&long[12_000..14_400]) > 20.0 * energy(&short[12_000..14_400]));
    // Pitch moves the band: a high one has more of its energy above 4 kHz.
    let above = |s: &[f32]| {
        let sp = common::spectrum(&s[..8_192]);
        let bin = |hz: f64| (hz * 8_192.0 / HZ) as usize;
        sp[bin(4_000.0)..].iter().map(|a| a * a).sum::<f64>()
            / sp.iter().map(|a| a * a).sum::<f64>()
    };
    let low = heard(&[(NOISE, 100.0), (NOISE_PITCH, 200.0)], 60);
    let high = heard(&[(NOISE, 100.0), (NOISE_PITCH, 8_000.0)], 60);
    assert!(above(&high) > 10.0 * above(&low));
    // Key 96 is 9 dB above C3 at 100 %.
    let flat = peak(&heard(&[(NOISE, 100.0)], 96));
    let keyed = peak(&heard(&[(NOISE, 100.0), (NOISE_KEY, 100.0)], 96));
    assert!((common::db(f64::from(keyed / flat)) - 9.0).abs() < 0.2);
}

// Spec: electric R3, R6
#[test]
fn the_fork_rings_at_its_pitch_within_a_cent_across_the_keyboard() {
    for key in [24, 36, 48, 60, 72, 84, 96] {
        let s = render(&LINEAR, &[on(0, key, 0.8)], 120_000);
        let hz = hz_of(key);
        let found = pitch_near(&s, hz, 9_600, 96_000);
        assert!(
            cents(found, hz).abs() < 1.0,
            "key {key}: {} cents",
            cents(found, hz)
        );
    }
}

// Spec: electric R10
#[test]
fn semi_detune_and_stretch_tune_the_fork() {
    // Cents above the key's own pitch at which the fork rings, scanning
    // around `expected` cents.
    let found = |params: &[(ParamId, f64)], key: u8, expected: f64| {
        let mut all = LINEAR.to_vec();
        all.extend_from_slice(params);
        let s = render(&all, &[on(0, key, 0.8)], 120_000);
        let centre = hz_of(key) * 2f64.powf(expected / 1200.0);
        cents(pitch_near(&s, centre, 9_600, 96_000), hz_of(key))
    };
    assert!((found(&[(DETUNE, 50.0)], 60, 50.0) - 50.0).abs() < 1.0);
    assert!((found(&[(STRETCH, 100.0)], 84, 24.0) - 24.0).abs() < 1.0);
    assert!((found(&[(STRETCH, 100.0)], 36, -24.0) + 24.0).abs() < 1.0);
    assert!((found(&[(SEMI, 12.0)], 60, 1200.0) - 1200.0).abs() < 1.0);
}

// Spec: electric R3, R6
#[test]
fn the_tine_decays_as_its_decay_says() {
    for key in [48, 72] {
        let s = render(
            &[
                LINEAR[0],
                LINEAR[1],
                (TINE_DECAY, 3_000.0),
                (TONE_DECAY, 100.0),
            ],
            &[on(0, key, 0.8)],
            96_000,
        );
        let t = t60(&s, hz_of(key), 28_800, 76_800, 14_400);
        assert!((t - 3.0).abs() < 0.15, "key {key}: {t} s");
    }
}

// Spec: electric R3, R7
#[test]
fn key_moves_the_tines_and_the_pickups_level_by_up_to_12_db_over_four_octaves() {
    // Key 96 is three quarters of four octaves above C3 (key 60): 9 dB.
    let level = |extra: &[(ParamId, f64)], key: u8| {
        let mut all = LINEAR.to_vec();
        all.push((NOISE, 0.0));
        all.extend_from_slice(extra);
        let s = render(&all, &[on(0, key, 0.8)], 24_000);
        magnitude(&s, hz_of(key), 2_400, 12_000)
    };
    for key_param in [TINE_KEY, OUTPUT_KEY] {
        let moved = level(&[(key_param, 100.0)], 96) / level(&[], 96);
        assert!(
            (common::db(moved) - 9.0).abs() < 0.2,
            "{key_param:?}: {}",
            common::db(moved)
        );
        // And C3 itself doesn't move.
        let centre = level(&[(key_param, 100.0)], 60) / level(&[], 60);
        assert!(
            (common::db(centre)).abs() < 0.1,
            "{key_param:?}: {}",
            common::db(centre)
        );
    }
}

// Spec: electric R4, R6
#[test]
fn the_tone_bar_rings_on_for_its_own_decay_after_the_tine_has_died() {
    for key in [48, 72] {
        let s = render(
            &[
                LINEAR[0],
                LINEAR[1],
                (TINE_DECAY, 100.0),
                (TONE_DECAY, 3_000.0),
            ],
            &[on(0, key, 0.8)],
            96_000,
        );
        let t = t60(&s, hz_of(key), 28_800, 76_800, 14_400);
        assert!((t - 3.0).abs() < 0.15, "key {key}: {t} s");
    }
}

// Spec: electric R5, R6, R9
#[test]
fn the_fork_falls_60_db_within_release_after_the_key_is_let_go() {
    let params = [
        LINEAR[0],
        LINEAR[1],
        (TINE_DECAY, 20_000.0),
        (TONE_DECAY, 20_000.0),
        (RELEASE, 1_500.0),
    ];
    let notes = [on(0, 60, 0.8), off(48_000, 60)];
    let s = render(&params, &notes, 48_000 * 4);
    let hz = hz_of(60);
    let t = t60(&s, hz, 48_000 + 19_200, 48_000 + 57_600, 9_600);
    assert!((t - 1.5).abs() < 0.075, "{t} s");
    let before = magnitude(&s, hz, 48_000 - 9_600, 9_600);
    let after = magnitude(&s, hz, 48_000 + 72_000 - 4_800, 9_600);
    assert!(20.0 * (before / after).log10() > 57.0);
    // And the voice is gone for good soon after.
    assert!(peak(&s[48_000 * 3..]) < 1e-6);
}

// Spec: electric R7
#[test]
fn a_long_hammer_noise_fades_with_the_fork_on_release() {
    let params = [
        LINEAR[0],
        LINEAR[1],
        (NOISE, 100.0),
        (NOISE_DECAY, 1_000.0),
        (RELEASE, 10.0),
    ];
    let notes = [on(0, 60, 0.8), off(4_800, 60)];
    let s = render(&params, &notes, 48_000);
    // Release's tail is 18 ms; the voice itself lasts about 100 ms more.
    let held = peak(&s[2_400..4_800]);
    let late = peak(&s[4_800 + 2_000..4_800 + 4_000]);
    assert!(late < held * 1e-2, "{late} after release against {held}");
}

#[test]
fn symmetry_and_distance_shape_the_pickups_harmonics() {
    let hz = hz_of(60);
    let level = |params: &[(ParamId, f64)], harmonic: f64| {
        let mut all = vec![(TINE_DECAY, 20_000.0), (NOISE, 0.0), (INPUT, 4.0)];
        all.extend_from_slice(params);
        let s = render(&all, &[on(0, 60, 1.0)], 48_000);
        let f0 = magnitude(&s, hz, 4_800, 24_000);
        common::db(magnitude(&s, hz * harmonic, 4_800, 24_000) / f0)
    };
    for kind in [0.0, 1.0] {
        let kind = (PICKUP_TYPE, kind);
        // Straight in front only odd harmonics come out; away from it the
        // even ones rise.
        let straight = level(&[kind, (SYMMETRY, 50.0), (DISTANCE, 0.0)], 2.0);
        let away = level(&[kind, (SYMMETRY, 0.0), (DISTANCE, 0.0)], 2.0);
        assert!(straight < -50.0, "{kind:?}: {straight}");
        assert!(away > straight + 30.0, "{kind:?}: {away}");
        // Closer overdrives more.
        let near = level(&[kind, (SYMMETRY, 50.0), (DISTANCE, 0.0)], 3.0);
        let far = level(&[kind, (SYMMETRY, 50.0), (DISTANCE, 100.0)], 3.0);
        assert!(near > far + 10.0, "{kind:?}: near {near}, far {far}");
    }
}

// Spec: electric R7
#[test]
fn the_pickup_never_divides_by_zero_however_hard_it_is_driven() {
    for kind in [0.0, 1.0] {
        for symmetry in [0.0, 100.0] {
            let s = render(
                &[
                    (PICKUP_TYPE, kind),
                    (SYMMETRY, symmetry),
                    (DISTANCE, 0.0),
                    (INPUT, 4.0),
                    (FORCE, 100.0),
                    (FORCE_VEL, 100.0),
                ],
                &[on(0, 72, 1.0), on(0, 36, 1.0)],
                24_000,
            );
            assert!(s.iter().all(|x| x.is_finite() && x.abs() < 1e4));
        }
    }
}

// Spec: electric R8
#[test]
fn the_device_reports_the_oversamplers_latency() {
    assert_eq!(
        node(&[]).latency(),
        bl_dsp::Oversampler::new(2, 1, 32).latency()
    );
    assert_eq!(node(&[]).latency(), 48);
}

// Spec: electric R9
#[test]
fn the_damper_sounds_at_the_start_the_end_or_both_by_att_rel() {
    let heard = |att_rel: f64, level: f64| {
        let s = render(
            &[
                (FORCE, 0.0),
                (NOISE, 0.0),
                (DAMPER_LEVEL, level),
                (DAMPER_TONE, 100.0),
                (DAMPER_ATT_REL, att_rel),
            ],
            &[on(0, 60, 1.0), off(24_000, 60)],
            48_000,
        );
        (energy(&s[..4_800]), energy(&s[24_000..28_800]))
    };
    let (start, end) = heard(-100.0, 1.0);
    assert!(start > 1e-3 && end < start * 1e-3, "{start} {end}");
    let (start, end) = heard(100.0, 1.0);
    assert!(end > 1e-3 && start < end * 1e-3, "{start} {end}");
    let (start, end) = heard(0.0, 1.0);
    assert!(start > 1e-3 && end > 1e-3, "{start} {end}");
    let (start, end) = heard(0.0, 0.0);
    assert!(start == 0.0 && end == 0.0);
}

// Spec: electric R10
#[test]
fn volume_scales_the_output_and_voices_limits_the_notes() {
    let full = render(&LINEAR, &[on(0, 60, 0.8)], 9_600);
    let half = render(
        &[LINEAR[0], LINEAR[1], (VOLUME, 0.5)],
        &[on(0, 60, 0.8)],
        9_600,
    );
    assert!(
        full.iter()
            .zip(&half)
            .all(|(f, h)| (f * 0.5 - h).abs() < 1e-6)
    );
    assert!(
        render(&[(VOLUME, 0.0)], &[on(0, 60, 0.8)], 9_600)
            .iter()
            .all(|s| *s == 0.0)
    );
    // One voice: the second note takes the first's place.
    let one = render(
        &[
            LINEAR[0],
            LINEAR[1],
            (VOICES, 1.0),
            (TINE_DECAY, 20_000.0),
            (TONE_DECAY, 20_000.0),
        ],
        &[on(0, 60, 0.8), on(4_800, 67, 0.8)],
        48_000,
    );
    let (first, second) = (
        magnitude(&one, hz_of(60), 24_000, 12_000),
        magnitude(&one, hz_of(67), 24_000, 12_000),
    );
    assert!(second > 100.0 * first, "{first} {second}");
}

// Spec: electric R10
#[test]
fn pitch_bend_sets_the_wheels_range_and_zero_means_no_bend() {
    let bend = |params: &[(ParamId, f64)], expected: f64| {
        let mut all = LINEAR.to_vec();
        all.extend_from_slice(params);
        let wheel = NoteEvent {
            offset: 0,
            kind: NoteKind::PitchBend { value: 1.0 },
            channel: 0,
            key: 0,
            velocity: 0.0,
            note_id: None,
        };
        let s = render(&all, &[wheel, on(0, 60, 0.8)], 48_000);
        let centre = hz_of(60) * 2f64.powf(expected / 1200.0);
        cents(pitch_near(&s, centre, 9_600, 24_000), hz_of(60))
    };
    assert!(bend(&[(PB_RANGE, 0.0)], 0.0).abs() < 1.0);
    assert!((bend(&[(PB_RANGE, 2.0)], 200.0) - 200.0).abs() < 1.0);
    assert!((bend(&[(PB_RANGE, 12.0)], 1200.0) - 1200.0).abs() < 1.0);
}

// Spec: electric R13
#[test]
fn the_render_is_the_same_at_every_block_size_and_run_after_run() {
    let notes = [
        on(0, 60, 0.8),
        on(3_001, 64, 0.5),
        off(20_000, 60),
        off(26_000, 64),
        on(30_017, 72, 1.0),
        off(33_000, 72),
    ];
    let play = |block| play_blocks(node(&[]).as_mut(), 60_000, block, &notes);
    let reference = play(4_096);
    assert!(peak(&reference) > 0.05);
    for block in [1, 64, 512, 4_096] {
        assert!(play(block) == reference, "block size {block}");
    }
}

// Spec: electric R12, R13 (the device budget is `benches/electric.rs`)
#[test]
fn sixteen_voices_render_with_no_allocation() {
    let mut node = node(&[(VOICES, 16.0)]);
    let chord: Vec<NoteEvent> = (0..16).map(|i| on(0, 36 + i * 3, 0.8)).collect();
    let heard = play_blocks(node.as_mut(), 48_000, 128, &chord);
    assert!(heard.iter().all(|s| s.is_finite()) && peak(&heard) > 0.05);
}
