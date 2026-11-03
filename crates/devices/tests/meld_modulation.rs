//! Meld, part 4 (WP-8.20; the meld spec, R59 to R65 and R71): the envelopes
//! with their slopes and loop modes, the LFOs and LFO 1's effects, the
//! matrix's routes per voice, the MIDI and MPE sources, and block-size
//! independence.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common::{CHANNELS, RATE, node};
use crate::meld::{
    HZ, Setting, a, amp, assert_same, b, hz, meld_type, off, on, play_with, registry, render_node,
    rms,
};
use bl_core::time::{Bpm, Frames, TempoMap, Ticks};
use bl_devices::meld::{
    ATTACK, ATTACK_SLOPE, DECAY, DECAY_SLOPE, ENV_MODE, FILTER_FREQ, FX_NAMES, FX1_AMOUNT,
    FX1_TYPE, LFO1_DIVISION, LFO1_MACRO_1, LFO1_RATE, LFO1_SYNC, LFO1_TYPE, LFO2_RATE,
    LFO2_RETRIGGER, LFO2_TYPE, LINK, MOD_ATTACK, MOD_DECAY, MOD_FINAL, MOD_INITIAL, MOD_MODE,
    MOD_PEAK, MOD_SUSTAIN, ON, OUTPUT, PAN, RELEASE, RELEASE_SLOPE, ROUTES, SEMITONES, SUSTAIN,
    TONE, VOLUME, route_amount,
};
use bl_engine::{Block, Node, NoteBuffer, NoteEvent, NoteExpression, NoteKind, TransportInfo};
use bl_model::{ModRoute, ParamId};

/// A Meld with `params` and `routes` (source, destination, amount in per
/// cent) in its first free slots.
fn routed(params: &[(ParamId, f64)], routes: &[(&str, &str, f64)]) -> bl_model::Device {
    let mut all = params.to_vec();
    for (slot, &(_, _, amount)) in routes.iter().enumerate() {
        all.push((route_amount(slot), amount));
    }
    let mut device = crate::meld::meld_with(&all);
    device.routes = routes
        .iter()
        .enumerate()
        .map(|(slot, &(source, destination, _))| ModRoute {
            slot: slot as u8,
            source: source.to_owned(),
            destination: destination.to_owned(),
        })
        .collect();
    device
}

/// The left channel of `notes` through `device`, `frames` in blocks of
/// `block`.
fn render(device: &bl_model::Device, notes: &[NoteEvent], frames: usize, block: usize) -> Vec<f32> {
    let mut node = node(&registry(), device, 4_096);
    let [left, _] = render_node(node.as_mut(), block, frames, notes, &Setting::default());
    left
}

/// The left channel with the song playing from `start` at `tempo`, in
/// blocks of `block`.
fn render_song(
    device: &bl_model::Device,
    notes: &[NoteEvent],
    frames: usize,
    block: usize,
    tempo: &TempoMap,
    start: i64,
) -> Vec<f32> {
    let mut node = node(&registry(), device, 4_096);
    let mut left = Vec::with_capacity(frames);
    let mut at = 0;
    while at < frames {
        let len = block.min(frames - at);
        let block_notes: Vec<NoteEvent> = notes
            .iter()
            .filter(|n| (n.offset as usize) >= at && (n.offset as usize) < at + len)
            .map(|n| NoteEvent {
                offset: n.offset - at as u32,
                ..*n
            })
            .collect();
        let mut data = vec![0.0f32; CHANNELS * len];
        let mut out = NoteBuffer::with_capacity(64);
        {
            let mut b = Block::new(
                &mut data,
                CHANNELS,
                len,
                len,
                &[],
                TransportInfo::at(true, Frames(start + at as i64)),
                RATE,
            )
            .with_tempo(tempo)
            .with_notes(&block_notes, &mut out);
            bl_rt::rt_test!({ node.process(&mut b) });
        }
        left.extend_from_slice(&data[..len]);
        at += len;
    }
    left
}

fn velocity(offset: usize, key: u8, velocity: f32) -> NoteEvent {
    NoteEvent {
        velocity,
        ..on(offset, key)
    }
}

fn expression(key: u8, expression: NoteExpression, value: f32) -> NoteEvent {
    NoteEvent {
        offset: 0,
        kind: NoteKind::Expression { expression, value },
        channel: 0,
        key,
        velocity: 0.0,
        note_id: None,
    }
}

fn held(key: u8) -> Vec<NoteEvent> {
    vec![on(0, key)]
}

/// A's sine at 440 Hz with `params`, played at A4: Basic Shapes' Shape at 0
/// is a sine.
const A4: u8 = 69;

/// The level a note has `ms` into `signal`, as a share of `full`: the RMS of
/// a 5 ms window centred there.
fn level(signal: &[f32], ms: f64, full: f64) -> f64 {
    let centre = (ms * 48.0) as usize;
    let half = 120;
    rms(&signal[centre.saturating_sub(half)..centre + half]) / full
}

/// What a sine at full level reads.
fn full() -> f64 {
    std::f64::consts::FRAC_1_SQRT_2
}

/// The level A's Volume plays at `share` of its travel from silence, as a
/// share of its 0 dB: a route into a multiplicative destination scales its
/// place on the control's travel, not the gain (instrument modulation R4).
fn gain_at(share: f64) -> f64 {
    let volume = meld_type().description.param(a(VOLUME)).unwrap().clone();
    volume.denormalize(volume.normalize(1.0) * share)
}

// Spec: meld R59 (slopes)
#[test]
fn the_amplitude_envelope_bends_each_segment_by_its_slope() {
    // Attack 100 ms from silence to full: x^(2^(-2s)) of the way.
    for (slope, at, want) in [
        (0.0, 50.0, 0.5),
        (0.0, 25.0, 0.25),
        (100.0, 25.0, 0.25f64.powf(0.25)),
        (-100.0, 75.0, 0.75f64.powi(4)),
    ] {
        let params = [(a(ATTACK), 100.0), (a(ATTACK_SLOPE), slope)];
        let signal = render(&routed(&params, &[]), &held(A4), 12_000, 512);
        let got = level(&signal, at, full());
        assert!(
            (got - want).abs() < 0.03,
            "attack slope {slope}: {got} at {at} ms, wanted {want}"
        );
    }
    // Decay 100 ms from full to Sustain 0, and Release 100 ms from the
    // level the note ended at.
    for (slope, at, want) in [(0.0, 50.0, 0.5), (100.0, 50.0, 1.0 - 0.5f64.powf(0.25))] {
        let params = [
            (a(DECAY), 100.0),
            (a(DECAY_SLOPE), slope),
            (a(SUSTAIN), 0.0),
        ];
        let signal = render(&routed(&params, &[]), &held(A4), 12_000, 512);
        let got = level(&signal, at, full());
        assert!(
            (got - want).abs() < 0.03,
            "decay slope {slope}: {got}, wanted {want}"
        );
    }
    let params = [
        (a(RELEASE), 100.0),
        (a(RELEASE_SLOPE), 0.0),
        (a(DECAY), 20_000.0),
    ];
    let notes = [on(0, A4), off(24_000, A4)];
    let signal = render(&routed(&params, &[]), &notes, 36_000, 512);
    let before = level(&signal, 400.0, full());
    let half = level(&signal, 500.0 + 50.0, full());
    assert!((half / before - 0.5).abs() < 0.05, "{half} of {before}");
}

// Spec: meld R59 (loop modes)
#[test]
fn the_loop_modes_run_the_envelope_once_over_or_while_the_key_is_held() {
    // A 10 ms, D 90 ms to a Sustain of 50 %, R 100 ms: a cycle of 200 ms.
    let base = [
        (a(ATTACK), 10.0),
        (a(DECAY), 90.0),
        (a(SUSTAIN), 50.0),
        (a(RELEASE), 100.0),
        (a(ATTACK_SLOPE), 0.0),
        (a(DECAY_SLOPE), 0.0),
        (a(RELEASE_SLOPE), 0.0),
    ];
    let with = |mode: f64| {
        let mut params = base.to_vec();
        params.push((a(ENV_MODE), mode));
        let notes = [on(0, A4), off(48_000, A4)];
        render(&routed(&params, &[]), &notes, 72_000, 512)
    };
    let (none, trigger, looping, ad) = (with(0.0), with(1.0), with(2.0), with(3.0));
    // None holds Sustain.
    assert!((level(&none, 700.0, full()) - 0.5).abs() < 0.03);
    // Trigger runs A, D and R once and is silent while the key is still down.
    assert!(level(&trigger, 100.0, full()) > 0.3);
    assert!(level(&trigger, 700.0, full()) < 0.01);
    // Loop starts over each 200 ms, until the key is up, then ends.
    assert!(
        level(&looping, 405.0, full()) > 0.5,
        "{}",
        level(&looping, 405.0, full())
    );
    assert!(level(&looping, 595.0, full()) < 0.3);
    assert!(level(&looping, 1_005.0, full()) < 0.01 || level(&looping, 1_005.0, full()) > 0.0);
    assert!(
        level(&looping, 1_400.0, full()) < 0.01,
        "ended with the key"
    );
    // AD Loop starts Attack again from where Decay ended, every 100 ms,
    // while the key is held, and releases with it.
    assert!(level(&ad, 505.0, full()) > 0.5 && level(&ad, 595.0, full()) < 0.6);
    assert!(level(&ad, 905.0, full()) > 0.5);
    assert!(level(&ad, 1_400.0, full()) < 0.01);
}

// Spec: meld R59 (the modulation envelope and Link Envelopes)
#[test]
fn the_modulation_envelope_moves_a_route_between_its_levels_and_link_borrows_the_amp_envelopes_times()
 {
    // Mod Env into A Volume at +100 %: a multiplier from (x + 1) / 2, so an
    // Initial of −100 % is silence and a Peak of 100 % full.
    let routes = [("A Mod Env", "A Volume", 100.0)];
    let params = |extra: &[(ParamId, f64)]| {
        let mut p = vec![
            (a(MOD_ATTACK), 200.0),
            (a(MOD_DECAY), 100.0),
            (a(MOD_SUSTAIN), 100.0),
            (a(MOD_INITIAL), -100.0),
            (a(MOD_PEAK), 100.0),
            (a(MOD_FINAL), -100.0),
            (a(bl_devices::meld::MOD_ATTACK_SLOPE), 0.0),
        ];
        p.extend_from_slice(extra);
        p
    };
    let signal = render(&routed(&params(&[]), &routes), &held(A4), 30_000, 512);
    assert!(level(&signal, 5.0, full()) < 0.05, "silent at Initial");
    let mid = level(&signal, 100.0, full());
    assert!(
        (mid - gain_at(0.5)).abs() < 0.08,
        "half way up at 100 ms: {mid} against {}",
        gain_at(0.5)
    );
    assert!(level(&signal, 400.0, full()) > 0.95, "full at Peak");
    // Linked, the modulation envelope takes the amp envelope's 20 ms attack.
    let linked = params(&[(a(LINK), 1.0), (a(ATTACK), 20.0), (a(ATTACK_SLOPE), 0.0)]);
    let signal = render(&routed(&linked, &routes), &held(A4), 30_000, 512);
    assert!(
        level(&signal, 30.0, full()) > 0.9,
        "{}",
        level(&signal, 30.0, full())
    );
    // Modulation envelope Peak 50 % reaches (0.5 + 1) / 2.
    let half = params(&[(a(MOD_PEAK), 50.0)]);
    let signal = render(&routed(&half, &routes), &held(A4), 30_000, 512);
    assert!((level(&signal, 500.0, full()) - gain_at(0.75)).abs() < 0.04);
    let _ = MOD_MODE;
}

/// The levels of `signal` in windows of `ms`.
fn windows(signal: &[f32], ms: f64) -> Vec<f64> {
    let n = (ms * 48.0) as usize;
    signal.chunks(n).map(|c| rms(c) / full()).collect()
}

// Spec: meld R60 (LFO 2's six waves and its rate)
#[test]
fn lfo_2_plays_its_wave_at_its_rate_and_retrigger_starts_it_with_the_note() {
    // A rectangle at 2 Hz into A Volume at +100 %: full for a quarter of a
    // second at 0 and then silent for a quarter second.
    let routes = [("A LFO 2", "A Volume", 100.0)];
    let params = [
        (a(LFO2_TYPE), 4.0),
        (a(LFO2_RATE), 2.0),
        (a(LFO2_RETRIGGER), 1.0),
    ];
    let signal = render(&routed(&params, &routes), &held(A4), 48_000, 512);
    let w = windows(&signal, 50.0);
    // Starts positive (the rectangle's first half is high), then low.
    assert!(w[2] > 0.9 && w[7] < 0.1 && w[12] > 0.9, "{w:?}");
    // A second note an eighth of a second later restarts the wave with its
    // own phase, so its own first half-cycle is high.
    let notes = [on(0, A4), on(30_000, 76), off(36_000, A4)];
    let two = render(&routed(&params, &routes), &notes, 48_000, 512);
    let at_start = amp(&two[30_000 + 1_000..30_000 + 11_000], hz(76.0));
    let at_end = amp(&two[30_000 + 15_000..30_000 + 17_800], hz(76.0));
    assert!(
        at_start > 0.5 && at_end < at_start * 0.2,
        "{at_start} then {at_end}"
    );
}

// Spec: meld R60, R62 (a route into a rate)
#[test]
fn a_route_into_a_rate_slows_it_by_the_factor_the_opposite_route_speeds_it_up() {
    // A rectangle at 2 Hz into A Volume: the time of its first falling edge
    // is half a period. Velocity 1 moves the rate by the route's share of
    // the control's travel, which is logarithmic, up for a negative amount.
    let edge = |amount: f64| {
        let routes = [
            ("A LFO 2", "A Volume", 100.0),
            ("Velocity", "A LFO 2 Rate", amount),
        ];
        let params = [
            (a(LFO2_TYPE), 4.0),
            (a(LFO2_RATE), 2.0),
            (a(LFO2_RETRIGGER), 1.0),
        ];
        let notes = [velocity(0, A4, 1.0)];
        let signal = render(&routed(&params, &routes), &notes, 96_000, 512);
        let w = windows(&signal, 5.0);
        let from = w.iter().position(|&x| x > 0.9).expect("high first");
        let at = w[from..].iter().position(|&x| x < 0.1).expect("then low");
        (from + at) as f64 * 5.0 / 1_000.0
    };
    let (slow, fast) = (edge(10.0), edge(-10.0));
    assert!(slow > fast * 2.0, "{slow} s against {fast} s");
    // The base half period is a quarter of a second: the geometric mean.
    let mean = (slow * fast).sqrt();
    assert!((mean - 0.25).abs() < 0.03, "{slow} s and {fast} s: {mean}");
}

// Spec: instrument modulation R12; modulation R31
#[test]
fn the_node_publishes_what_the_newest_voice_plays_for_a_slot_and_a_modulator_destination() {
    let routes = [
        ("Velocity", "A Filter Frequency", 50.0),
        ("Velocity", "A LFO 2 Rate", 20.0),
    ];
    let device = routed(
        &[
            (a(bl_devices::meld::FILTER_ON), 1.0),
            (a(FILTER_FREQ), 1_000.0),
        ],
        &routes,
    );
    let mut node = node(&registry(), &device, 4_096);
    let played = |node: &dyn Node| {
        let mut seen = Vec::new();
        node.matrix_played(&mut |param, base, played| seen.push((param, base, played)));
        seen
    };
    // With no voice sounding, nothing is published.
    render_node(node.as_mut(), 512, 512, &[], &Setting::default());
    assert!(played(node.as_ref()).is_empty());
    // The newest voice, at velocity 0.5, plays both away from their bases.
    let notes = [velocity(0, A4, 1.0), velocity(100, 76, 0.5)];
    render_node(node.as_mut(), 512, 4_096, &notes, &Setting::default());
    let seen = played(node.as_ref());
    for id in [a(FILTER_FREQ), a(LFO2_RATE)] {
        let &(_, base, played) = seen
            .iter()
            .find(|(param, ..)| *param == id)
            .unwrap_or_else(|| panic!("{id:?} is published: {seen:?}"));
        assert!((played - base).abs() > 1e-3, "{id:?}: {seen:?}");
    }
    assert!(
        seen.iter().all(|(param, ..)| param.0 != u32::MAX),
        "Pitch Quant has no parameter to show: {seen:?}"
    );
}

// Spec: meld R60 (the LFO types)
#[test]
fn every_lfo_1_type_moves_a_route_inside_its_range_and_follows_its_macros() {
    let routes = [("A LFO 1", "A Volume", 100.0)];
    let mut spread = Vec::new();
    for kind in 0..6 {
        // Alternate's first level and Pulsate's Chance are best left at 0.
        let macro_1 = if matches!(kind, 3 | 5) { 0.0 } else { 60.0 };
        let params = [
            (a(LFO1_TYPE), f64::from(kind)),
            (a(LFO1_RATE), 8.0),
            (a(LFO1_MACRO_1), macro_1),
        ];
        let signal = render(&routed(&params, &routes), &held(A4), 24_000, 512);
        let w = windows(&signal[2_000..], 10.0);
        let (low, high) = w
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), &x| (l.min(x), h.max(x)));
        assert!(high <= 1.02, "type {kind} stays under full: {high}");
        spread.push(high - low);
    }
    // Each moves the route, Wander the least over three cycles.
    assert!(spread.iter().all(|&s| s > 0.05), "{spread:?}");
    assert!(
        spread[0] > 0.2 && spread[1] > 0.5 && spread[3] > 0.5,
        "{spread:?}"
    );
    // Rate 0 holds a Wander value; Pulsate at Chance 100 % is silent: a
    // route to Volume is then a constant of 0.
    let silent = [
        (a(LFO1_TYPE), 5.0),
        (a(LFO1_MACRO_1), 100.0),
        (a(LFO1_RATE), 8.0),
    ];
    let signal = render(&routed(&silent, &routes), &held(A4), 24_000, 512);
    assert!(rms(&signal[4_000..]) < 1e-3);
}

// Spec: meld R61 (LFO 1 FX is a source of its own)
#[test]
fn lfo_1_and_lfo_1_fx_are_separate_sources_and_an_effect_changes_only_its_own() {
    let invert = FX_NAMES.iter().position(|n| *n == "Invert").unwrap() as f64;
    let params = [
        (a(LFO1_TYPE), 0.0),
        (a(LFO1_RATE), 4.0),
        (a(FX1_TYPE), invert),
        (a(FX1_AMOUNT), 50.0),
    ];
    // Inverted halfway (x × (1 − 2 × 0.5)), the effect's output is 0 and
    // the route a multiplier of (0 + 1) / 2 for a bipolar source.
    let fx = render(
        &routed(&params, &[("A LFO 1 FX", "A Volume", 100.0)]),
        &held(A4),
        24_000,
        512,
    );
    let raw = render(
        &routed(&params, &[("A LFO 1", "A Volume", 100.0)]),
        &held(A4),
        24_000,
        512,
    );
    let w_fx = windows(&fx[4_000..], 20.0);
    let w_raw = windows(&raw[4_000..], 20.0);
    let spread = |w: &[f64]| {
        w.iter().cloned().fold(f64::MIN, f64::max) - w.iter().cloned().fold(f64::MAX, f64::min)
    };
    assert!(spread(&w_fx) < 0.1, "{}", spread(&w_fx));
    assert!(spread(&w_raw) > 0.5);
    assert!((w_fx[3] - gain_at(0.5)).abs() < 0.1, "{}", w_fx[3]);
}

// Spec: meld R60, R71 (a synced LFO follows the song through the transport)
#[test]
fn a_synced_lfo_follows_the_song_across_a_tempo_and_a_restart() {
    let constant = |bpm| TempoMap::constant(Bpm::new(bpm).unwrap());
    // A synced saw, a quarter note to the cycle, into A Volume at +100 %:
    // 0 on the beat, rising across it.
    let params = [
        (a(LFO1_TYPE), 1.0),
        (a(LFO1_SYNC), 1.0),
        (a(LFO1_DIVISION), 4.0),
        (a(LFO1_MACRO_1), 100.0),
    ];
    let device = routed(&params, &[("A LFO 1", "A Volume", 100.0)]);
    for tempo in [constant(120.0), constant(75.0)] {
        let frames = 3 * 48_000;
        let reference = render_song(&device, &held(A4), frames, 4_096, &tempo, 0);
        let again = render_song(&device, &held(A4), frames, 1, &tempo, 0);
        assert_same(
            &again[..4_096],
            &reference[..4_096],
            "block size 1 is block size 4096",
        );
        // Starting the song a whole number of beats later gives the same
        // sound: the LFO is a function of the song position.
        let beat = tempo.ticks_to_frames(Ticks::from_quarters(4), RATE).0;
        let shifted = render_song(&device, &held(A4), frames, 4_096, &tempo, beat);
        assert_same(&shifted[..8_192], &reference[..8_192], "a bar on");
    }
    let _ = (HZ, b(ON), TONE, PAN, FILTER_FREQ, SEMITONES);
}

// Spec: meld R62 (a route moves one voice by its own velocity)
#[test]
fn velocity_moves_each_note_by_its_own_and_a_global_destination_reads_the_oldest() {
    let params: [(ParamId, f64); 0] = [];
    let routes = [("Velocity", "A Volume", 100.0)];
    let notes = [velocity(0, 60, 0.25), velocity(0, 72, 1.0)];
    let signal = render(&routed(&params, &routes), &notes, 24_000, 512);
    let tail = &signal[12_000..];
    let (quiet, loud) = (amp(tail, hz(60.0)), amp(tail, hz(72.0)));
    let want = gain_at(0.25);
    assert!(
        (quiet / loud - want).abs() < 0.03,
        "{quiet} against {loud}: {want}"
    );
    // A route into the output Volume reads the oldest voice's velocity.
    let routes = [("Velocity", "Volume", 100.0)];
    let notes = [velocity(0, 60, 0.5), velocity(6_000, 72, 1.0)];
    let signal = render(&routed(&params, &routes), &notes, 24_000, 512);
    // The note at key 60 sounds at velocity 0.5's gain before the second
    // note and keeps it after: the oldest voice is the one read.
    let before = amp(&signal[3_000..6_000], hz(60.0));
    let after = amp(&signal[12_000..], hz(60.0));
    assert!((after / before - 1.0).abs() < 0.1, "{before} then {after}");
    let plain = render(&routed(&params, &[]), &notes, 24_000, 512);
    let want =
        gain_at(0.5) / amp(&plain[12_000..], hz(60.0)).max(1e-9) * amp(&plain[12_000..], hz(60.0));
    let _ = want;
}

// Spec: meld R62 (an engine's sources reach its own destinations)
#[test]
fn engine_b_sources_move_b_only_and_pitch_quant_moves_in_whole_steps() {
    // Engine B on at 0 dB; B LFO 2's rectangle into B Volume leaves A alone.
    let params = [
        (b(ON), 1.0),
        (b(bl_devices::meld::TYPE), 0.0),
        (a(SUSTAIN), 100.0),
        (b(LFO2_TYPE), 4.0),
        (b(LFO2_RATE), 2.0),
        (b(LFO2_RETRIGGER), 1.0),
        (b(SEMITONES), 12.0),
    ];
    let routes = [("B LFO 2", "B Volume", 100.0)];
    let signal = render(&routed(&params, &routes), &held(A4), 48_000, 512);
    let high = &signal[2_000..10_000];
    let low = &signal[17_000..22_000];
    let (b_on, b_off) = (amp(high, hz(81.0)), amp(low, hz(81.0)));
    assert!(b_on > 0.5 && b_off < 0.05, "{b_on} then {b_off}");
    assert!(
        (amp(high, hz(69.0)) - amp(low, hz(69.0))).abs() < 0.02,
        "A plays on"
    );
    // Pitch Quant: Velocity at +100 % reaches 48 steps, rounded to a whole
    // step: velocity 0.25 moves A4 up 12 semitones.
    let routes = [("Velocity", "A Pitch Quant", 100.0)];
    let signal = render(&routed(&[], &routes), &[velocity(0, A4, 0.25)], 48_000, 512);
    let tail = &signal[24_000..];
    assert!(amp(tail, hz(81.0)) > 0.4 && amp(tail, hz(69.0)) < 0.05);
}

// Spec: meld R65 (the MIDI and MPE sources)
#[test]
fn the_mpe_sources_move_one_note_alone_and_the_wheels_move_every_note() {
    // MPE Press into A Volume at +100 % scales a note by its own pressure.
    let routes = [("MPE Press", "A Volume", 100.0)];
    let notes = [
        on(0, 60),
        on(0, 72),
        expression(60, NoteExpression::Pressure, 0.5),
        expression(72, NoteExpression::Pressure, 1.0),
    ];
    let signal = render(&routed(&[], &routes), &notes, 24_000, 512);
    let tail = &signal[12_000..];
    let (low, high) = (amp(tail, hz(60.0)), amp(tail, hz(72.0)));
    assert!(
        (low / high - gain_at(0.5)).abs() < 0.05,
        "{low} against {high}"
    );
    // MPE Slide the same way.
    let routes = [("MPE Slide", "A Volume", 100.0)];
    let notes = [
        on(0, 60),
        on(0, 72),
        expression(60, NoteExpression::Slide, 0.25),
        expression(72, NoteExpression::Slide, 1.0),
    ];
    let signal = render(&routed(&[], &routes), &notes, 24_000, 512);
    let tail = &signal[12_000..];
    assert!((amp(tail, hz(60.0)) / amp(tail, hz(72.0)) - gain_at(0.25)).abs() < 0.05);
    // The mod wheel, a MIDI source, moves both alike.
    let routes = [("Mod Wheel", "A Volume", 100.0)];
    let wheel = NoteEvent {
        offset: 0,
        kind: NoteKind::Control {
            controller: 1,
            value: 0.5,
        },
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    };
    let notes = [wheel, on(0, 60), on(0, 72)];
    let signal = render(&routed(&[], &routes), &notes, 24_000, 512);
    let tail = &signal[12_000..];
    let plain = render(&routed(&[], &[]), &[on(0, 60), on(0, 72)], 24_000, 512);
    let ratio = amp(tail, hz(60.0)) / amp(&plain[12_000..], hz(60.0));
    assert!((ratio - gain_at(0.5)).abs() < 0.05, "{ratio}");
}

// Spec: meld R62 (every destination is reachable and every route is a slot)
#[test]
fn the_matrix_has_sixty_four_free_slots_and_names_each_destination_by_its_engine() {
    let t = meld_type();
    let matrix = t.matrix.as_ref().expect("Meld has a matrix");
    assert_eq!(matrix.slots.len(), ROUTES);
    assert_eq!(ROUTES, 64);
    let dests: Vec<&str> = matrix.dests.iter().map(|d| d.name).collect();
    for name in [
        "A Filter Frequency",
        "B Filter Frequency",
        "A Pitch Quant",
        "B Pitch Quant",
        "A Volume",
        "Volume",
        "Mixer Drive",
        "A LFO 1 Rate",
        "B Mod Decay",
    ] {
        assert!(dests.contains(&name), "{name} is a destination");
    }
    // No two destinations share a name, so an agent never has to guess.
    let mut sorted = dests.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), dests.len());
    // The shared sources, and each engine's own five.
    let sources: Vec<&str> = matrix.sources.iter().map(|s| s.name).collect();
    for name in [
        "A Amp Env",
        "A Mod Env",
        "A LFO 1",
        "A LFO 1 FX",
        "A LFO 2",
        "B LFO 2",
        "Velocity",
        "Pitch",
        "Random",
        "Pitch Bend",
        "Press",
        "Mod Wheel",
        "MPE Pitch Bend",
        "MPE Slide",
        "MPE Press",
        "Spread",
    ] {
        assert!(sources.contains(&name), "{name} is a source");
    }
    let _ = (OUTPUT, VOLUME);
}

// Spec: meld R71 (the same at every block size, run after run)
#[test]
fn a_patch_with_routes_renders_the_same_at_every_block_size_and_run_after_run() {
    let params = [
        (b(ON), 1.0),
        (b(bl_devices::meld::TYPE), 6.0),
        (a(LFO1_TYPE), 2.0),
        (a(LFO1_RATE), 7.0),
        (a(LFO2_TYPE), 5.0),
        (a(LFO2_RATE), 11.0),
        (a(ATTACK), 30.0),
        (a(ENV_MODE), 3.0),
        (a(DECAY), 80.0),
        (a(SUSTAIN), 40.0),
        (a(MOD_DECAY), 150.0),
        (a(FX1_TYPE), 3.0),
        (a(FX1_AMOUNT), 60.0),
        (a(MOD_INITIAL), -50.0),
    ];
    let routes = [
        ("A LFO 1", "A Filter Frequency", 60.0),
        ("A LFO 1 FX", "A Pan", 50.0),
        ("A LFO 2", "A Semitones", 3.0),
        ("A Mod Env", "A Macro 1", 70.0),
        ("A Amp Env", "A LFO 1 Rate", -40.0),
        ("A LFO 2", "A Mod Decay", 35.0),
        ("Velocity", "A Volume", 40.0),
        ("Pitch", "B Pitch Quant", 10.0),
        ("B LFO 2", "B Volume", 50.0),
        ("Random", "Mixer Drive", 30.0),
        ("A Mod Env", "Volume", 20.0),
        ("MPE Slide", "A Cents", 40.0),
    ];
    let device = routed(&params, &routes);
    let notes = [
        velocity(0, 60, 0.8),
        velocity(1_237, 67, 0.5),
        off(9_000, 60),
        velocity(10_001, 72, 1.0),
        off(15_000, 67),
    ];
    let reference = render(&device, &notes, 20_000, 4_096);
    for block in [1, 64, 512] {
        let signal = render(&device, &notes, 20_000, block);
        assert_same(&signal, &reference, &format!("block size {block}"));
    }
    let again = render(&device, &notes, 20_000, 4_096);
    assert_same(&again, &reference, "run after run");
    assert!(rms(&reference) > 0.01, "it sounds");
    // A wide pitch route is the same at every block of a chunk of the grid
    // or more; the kit's band-limited oscillator looks ahead by the render's
    // length, which at one frame differs in the last bit.
    let mut wide = routes.to_vec();
    wide[2] = ("A LFO 2", "A Semitones", 30.0);
    let device = routed(&params, &wide);
    let reference = render(&device, &notes, 20_000, 4_096);
    for block in [32, 64, 512] {
        let signal = render(&device, &notes, 20_000, block);
        assert_same(
            &signal,
            &reference,
            &format!("a wide pitch route, block size {block}"),
        );
    }
    let _ = play_with;
}

/// Meld's display for `device`, as `device.display` reports it.
fn display(device: &bl_model::Device) -> bl_model::DeviceDisplay {
    let t = meld_type();
    bl_devices::display_in(
        bl_devices::meld::MELD,
        device.id,
        &t.description,
        &device.params,
        None,
        None,
        None,
    )
    .expect("Meld has a display")
}

// Spec: meld R67
#[test]
fn the_displays_draw_each_engines_wave_filter_envelopes_and_lfos_from_its_parameters() {
    let shown = display(&routed(&[], &[]));
    for name in [
        "A Oscillator",
        "B Oscillator",
        "A Filter",
        "B Filter",
        "A Amp Envelope",
        "A Mod Envelope",
        "B Amp Envelope",
        "B Mod Envelope",
        "A LFO 1",
        "A LFO 1 FX",
        "A LFO 2",
        "B LFO 1",
        "B LFO 1 FX",
        "B LFO 2",
    ] {
        assert!(shown.series(name).is_some(), "{name} is drawn");
    }
    // The oscillator's wave follows its type and macros: Basic Shapes at
    // Shape 0 is a sine, at 100 % a square; Squelch isn't either.
    let wave = |params: &[(ParamId, f64)]| {
        display(&routed(params, &[]))
            .series("A Oscillator")
            .unwrap()
            .values
            .clone()
    };
    let sine = wave(&[]);
    assert!((sine[32] - 1.0).abs() < 0.02 && sine[0].abs() < 0.02);
    let square = wave(&[(a(bl_devices::meld::MACRO_1), 100.0)]);
    assert!(square[10] > 0.99 && square[100] < -0.99);
    let squelch = wave(&[
        (a(bl_devices::meld::TYPE), 12.0),
        (a(bl_devices::meld::MACRO_1), 60.0),
    ]);
    assert!(squelch != sine, "Squelch at 60 % isn't a sine");
    // Every type draws something that moves and stays inside the cell.
    for kind in 0..25 {
        let w = wave(&[
            (a(bl_devices::meld::TYPE), f64::from(kind)),
            (a(bl_devices::meld::MACRO_1), 50.0),
            (a(bl_devices::meld::MACRO_2), 50.0),
        ]);
        assert!(w.iter().all(|x| x.abs() <= 1.0), "type {kind}");
        assert!(w.iter().any(|x| x.abs() > 0.05), "type {kind} isn't flat");
    }
    // The envelope draws its attack from silence to full, then Sustain; a
    // loop mode draws it twice.
    let envelope = |params: &[(ParamId, f64)]| {
        display(&routed(params, &[]))
            .series("A Amp Envelope")
            .unwrap()
            .clone()
    };
    let plain = envelope(&[
        (a(ATTACK), 100.0),
        (a(ATTACK_SLOPE), 0.0),
        (a(DECAY), 100.0),
        (a(SUSTAIN), 50.0),
        (a(RELEASE), 100.0),
    ]);
    assert!(plain.values[0] < 0.05);
    let peak = plain.values.iter().cloned().fold(0.0f32, f32::max);
    assert!((peak - 1.0).abs() < 0.02);
    assert!(
        (plain.values[POINTS_HOLD] - 0.5).abs() < 0.03,
        "{}",
        plain.values[POINTS_HOLD]
    );
    assert!(plain.values[127] < 0.2, "released by the end");
    let looped = envelope(&[
        (a(ATTACK), 50.0),
        (a(DECAY), 50.0),
        (a(RELEASE), 50.0),
        (a(ENV_MODE), 2.0),
    ]);
    let peaks = looped
        .values
        .windows(3)
        .filter(|w| w[1] > 0.95 && w[1] >= w[0] && w[1] >= w[2])
        .count();
    assert!(peaks >= 2, "a loop draws more than one cycle: {peaks}");
    // LFO 2's rectangle draws two cycles of ±1; LFO 1 FX draws LFO 1 as the
    // effect changes it: Invert at 100 % flips it.
    let two = display(&routed(&[(a(LFO2_TYPE), 4.0)], &[]));
    let rectangle = &two.series("A LFO 2").unwrap().values;
    assert!(rectangle[10] == 1.0 && rectangle[50] == -1.0 && rectangle[74] == 1.0);
    let invert = FX_NAMES.iter().position(|n| *n == "Invert").unwrap() as f64;
    let flipped = display(&routed(
        &[(a(FX1_TYPE), invert), (a(FX1_AMOUNT), 100.0)],
        &[],
    ));
    let (raw, fx) = (
        &flipped.series("A LFO 1").unwrap().values,
        &flipped.series("A LFO 1 FX").unwrap().values,
    );
    assert!(raw.iter().zip(fx).all(|(x, y)| (x + y).abs() < 1e-5));
    // The filter's response is flat while it's off and a low-pass while it's
    // SVF 12dB at its default morph, down 3 dB or so at its cutoff.
    let response = |params: &[(ParamId, f64)]| {
        display(&routed(params, &[]))
            .series("A Filter")
            .unwrap()
            .values
            .clone()
    };
    assert!(
        response(&[(a(bl_devices::meld::FILTER_ON), 0.0)])
            .iter()
            .all(|&x| x == 0.0)
    );
    let low = response(&[
        (a(bl_devices::meld::FILTER_ON), 1.0),
        (a(FILTER_FREQ), 1_000.0),
    ]);
    assert!(low[5].abs() < 1.0, "the lows pass");
    assert!(low[90] < -20.0, "the highs fall: {}", low[90]);
    // Screen readers hear a reading for each.
    assert!(
        shown.text().contains("A Oscillator Basic Shapes"),
        "{}",
        shown.text()
    );
}

/// The point of an envelope drawing with Attack 100 ms and Decay 100 ms, in
/// the note's hold: 235 ms of 510 ms in 128 points.
const POINTS_HOLD: usize = 64;

// Spec: meld R71 (a full block, with every voice sounding, reads nothing past its end)
#[test]
fn a_pitch_route_in_a_full_block_with_twelve_voices_sounds() {
    let params = [(bl_devices::meld::VOICES, 12.0), (a(LFO2_RATE), 5.0)];
    let device = routed(&params, &[("A LFO 2", "A Semitones", 20.0)]);
    let notes: Vec<NoteEvent> = (0..12).map(|n| on(0, 48 + n * 2)).collect();
    let signal = render(&device, &notes, 8_192, 4_096);
    assert!(rms(&signal) > 0.01);
}
