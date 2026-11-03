//! Analog, part 2 (WP-8.15; the Analog spec, R18 to R25 and R29): the two
//! LFOs, vibrato, unison, glide, the keyboard, voices and priority, and the
//! MPE section, rendered as real-time sections under the allocation trap.

use bl_engine::{NoteEvent, NoteExpression, NoteKind, Registry};
use bl_model::param::db_to_gain;
use bl_model::{DeviceKind, ParamId};

use crate::common::synth::{self, BLOCKS, HZ, frequency, mod_wheel, off, on, rms, wheel};
use crate::common::{device, node};
use bl_devices::analog::{
    self, ANALOG, DESTINATIONS, DIVISIONS, GLIDE_MODES, LFO_WAVES, MPE, amp, env, filter, lfo,
    noise, osc,
};

fn registry() -> Registry {
    synth::registry()
}

fn analog_type() -> bl_engine::DeviceType {
    bl_devices::device_types()
        .into_iter()
        .find(|t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == ANALOG))
        .expect("Analog is registered")
}

fn choice(list: &[&str], name: &str) -> f64 {
    list.iter().position(|n| *n == name).unwrap() as f64
}

/// A device with a sine from Oscillator 1 into Filter 1, wide open, and
/// Amplifier 1 full at once and held, panned centre; the rest as a new device.
fn analog(params: &[(ParamId, f64)]) -> bl_model::Device {
    let mut all = vec![
        (osc::id(0, osc::LEVEL), 0.5),
        (osc::id(0, osc::BALANCE), 0.0),
        (osc::id(0, osc::SHAPE), 0.0),
        (env::id(2, env::ATTACK), 0.0),
        (env::id(2, env::DECAY), 0.0),
        (env::id(2, env::SUSTAIN), 100.0),
    ];
    all.extend_from_slice(params);
    device(&analog_type(), &all)
}

/// `notes` played by Analog with `params` for `frames`, in blocks of
/// `block`: the left channel.
fn play(params: &[(ParamId, f64)], frames: usize, block: usize, notes: &[NoteEvent]) -> Vec<f32> {
    let mut node = node(&registry(), &analog(params), block.max(1));
    synth::render(node.as_mut(), block, frames, notes)
        .into_iter()
        .next()
        .unwrap()
}

/// Spec: analog R2. The same, in one block.
fn render(params: &[(ParamId, f64)], frames: usize, notes: &[NoteEvent]) -> Vec<f32> {
    play(params, frames, frames, notes)
}

/// The frequency of `signal` from `from` to `to`, from its positive-going
/// zero crossings.
fn pitch(signal: &[f32], from: usize, to: usize) -> f64 {
    let mut crossings = Vec::new();
    for n in from.max(1)..to {
        let (a, b) = (signal[n - 1], signal[n]);
        if a < 0.0 && b >= 0.0 {
            crossings.push(n as f64 - 1.0 + f64::from(-a / (b - a)));
        }
    }
    assert!(crossings.len() > 2, "{} crossings", crossings.len());
    (crossings.len() - 1) as f64 * HZ / (crossings[crossings.len() - 1] - crossings[0])
}

fn cents(hz: f64, of: f64) -> f64 {
    1200.0 * (hz / of).log2()
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

fn same(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

/// A note-on with its own ID, so a per-note expression can name it.
fn on_id(frame: usize, key: u8, id: u32) -> NoteEvent {
    NoteEvent {
        note_id: Some(id),
        ..on(frame, key)
    }
}

fn expression(frame: usize, key: u8, id: u32, which: NoteExpression, value: f32) -> NoteEvent {
    NoteEvent {
        offset: frame as u32,
        kind: NoteKind::Expression {
            expression: which,
            value,
        },
        channel: 0,
        key,
        velocity: 0.0,
        note_id: Some(id),
    }
}

/// An LFO 1 setting: on, a rectangle at `hz` restarting at 0°, moving
/// Amplifier 1's level by `amount`.
fn level_lfo(wave: &str, hz: f64, amount: f64) -> Vec<(ParamId, f64)> {
    vec![
        (lfo::id(0, lfo::ON), 1.0),
        (lfo::id(0, lfo::WAVE), choice(&LFO_WAVES, wave)),
        (lfo::id(0, lfo::RATE), hz),
        (lfo::id(0, lfo::RETRIG), 1.0),
        (amp::id(0, amp::LEVEL_LFO), amount),
    ]
}

// Spec: analog R18. LFO 1 is the LFO of Oscillator 1's, Filter 1's and
// Amplifier 1's controls and LFO 2 of the second side's.
#[test]
fn each_lfo_moves_the_controls_of_its_own_side() {
    let notes = [on(0, 57)];
    let plain = render(&[], 9_600, &notes);
    for control in [
        (osc::id(0, osc::PITCH_MOD), 100.0),
        (filter::id(0, filter::FREQ_LFO), 100.0),
        (amp::id(0, amp::LEVEL_LFO), 100.0),
    ] {
        let first = render(
            &[
                (lfo::id(0, lfo::ON), 1.0),
                (lfo::id(0, lfo::RATE), 8.0),
                control,
            ],
            9_600,
            &notes,
        );
        let second = render(
            &[
                (lfo::id(1, lfo::ON), 1.0),
                (lfo::id(1, lfo::RATE), 8.0),
                control,
            ],
            9_600,
            &notes,
        );
        let off = render(&[control], 9_600, &notes);
        assert!(!same(&first, &plain), "LFO 1 moves {control:?}");
        assert!(same(&second, &off), "LFO 2 doesn't reach {control:?}");
    }
    // The second side: Oscillator 2 into Filter 2 and Amplifier 2.
    let side = |on_lfo: usize, control: (ParamId, f64)| {
        render(
            &[
                (osc::id(0, osc::ON), 0.0),
                (osc::id(1, osc::ON), 1.0),
                (osc::id(1, osc::LEVEL), 0.5),
                (osc::id(1, osc::SHAPE), 0.0),
                (osc::id(1, osc::BALANCE), 100.0),
                (filter::id(1, filter::ON), 1.0),
                (amp::id(1, amp::ON), 1.0),
                (env::id(3, env::ATTACK), 0.0),
                (env::id(3, env::DECAY), 0.0),
                (env::id(3, env::SUSTAIN), 100.0),
                (amp::id(0, amp::ON), 0.0),
                (lfo::id(on_lfo, lfo::ON), 1.0),
                (lfo::id(on_lfo, lfo::RATE), 8.0),
                control,
            ],
            9_600,
            &notes,
        )
    };
    for control in [
        (osc::id(1, osc::PITCH_MOD), 100.0),
        (filter::id(1, filter::FREQ_LFO), 100.0),
        (amp::id(1, amp::LEVEL_LFO), 100.0),
    ] {
        let second = side(1, control);
        let first = side(0, control);
        let none = render(
            &[
                (osc::id(0, osc::ON), 0.0),
                (osc::id(1, osc::ON), 1.0),
                (osc::id(1, osc::LEVEL), 0.5),
                (osc::id(1, osc::SHAPE), 0.0),
                (osc::id(1, osc::BALANCE), 100.0),
                (filter::id(1, filter::ON), 1.0),
                (amp::id(1, amp::ON), 1.0),
                (env::id(3, env::ATTACK), 0.0),
                (env::id(3, env::DECAY), 0.0),
                (env::id(3, env::SUSTAIN), 100.0),
                (amp::id(0, amp::ON), 0.0),
                control,
            ],
            9_600,
            &notes,
        );
        assert!(!same(&second, &none), "LFO 2 moves {control:?}");
        assert!(same(&first, &none), "LFO 1 doesn't reach {control:?}");
    }
}

// Spec: analog R18. Rate is in hertz, restarting at Offset with Retrig, and
// Width is a rectangle's duty cycle.
#[test]
fn a_rectangle_lfo_switches_the_level_at_its_rate_and_width() {
    // A 5 Hz rectangle, 25 % wide: 12 dB up for 2,400 frames, down for 7,200.
    let mut params = level_lfo("Rectangle", 5.0, 100.0);
    params.push((lfo::id(0, lfo::WIDTH), 25.0));
    let signal = render(&params, 19_200, &[on(0, 57)]);
    let level = |from: usize, to: usize| db(rms(&signal[from..to]));
    let plain = render(&[], 19_200, &[on(0, 57)]);
    let base = db(rms(&plain[2_000..4_000]));
    assert!(
        (level(200, 2_200) - base - 12.0).abs() < 1.0,
        "up for the first quarter"
    );
    assert!(
        (level(2_800, 9_400) - base + 12.0).abs() < 1.0,
        "down for the rest"
    );
    assert!(
        (level(9_800, 11_800) - base - 12.0).abs() < 1.0,
        "up again in the next cycle"
    );
}

// Spec: analog R18. Every wave moves its destination, each its own way.
#[test]
fn every_wave_moves_its_destination() {
    let signals: Vec<Vec<f32>> = LFO_WAVES
        .iter()
        .map(|wave| render(&level_lfo(wave, 6.0, 100.0), 14_400, &[on(0, 57)]))
        .collect();
    let plain = render(&[], 14_400, &[on(0, 57)]);
    for (n, a) in signals.iter().enumerate() {
        assert!(!same(a, &plain), "{} moves the level", LFO_WAVES[n]);
        for (m, b) in signals.iter().enumerate().skip(n + 1) {
            assert!(!same(a, b), "{} and {} differ", LFO_WAVES[n], LFO_WAVES[m]);
        }
    }
}

// Spec: analog R18. Synced, the rate is a division of the tempo.
#[test]
fn a_synced_lfo_runs_a_division_of_the_tempo() {
    let mut synced = level_lfo("Triangle", 0.5, 100.0);
    synced.extend([
        (lfo::id(0, lfo::SYNC), 1.0),
        (lfo::id(0, lfo::DIVISION), choice(&DIVISIONS, "1/8")),
    ]);
    // At 120 beats a minute a 1/8 is a quarter of a second: 4 Hz.
    let a = render(&synced, 14_400, &[on(0, 57)]);
    let b = render(&level_lfo("Triangle", 4.0, 100.0), 14_400, &[on(0, 57)]);
    let worst = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "off by {worst}");
    let slower = render(
        &[
            (lfo::id(0, lfo::SYNC), 1.0),
            (lfo::id(0, lfo::DIVISION), choice(&DIVISIONS, "1/4")),
            (lfo::id(0, lfo::ON), 1.0),
            (amp::id(0, amp::LEVEL_LFO), 100.0),
        ],
        14_400,
        &[on(0, 57)],
    );
    assert!(!same(&slower, &a), "another division is another rate");
}

// Spec: analog R18. Delay holds the LFO at 0 and Attack fades it in; Offset
// is where a retriggered LFO starts.
#[test]
fn delay_and_attack_shape_the_lfo_and_offset_sets_its_start() {
    let plain = render(&[], 48_000, &[on(0, 57)]);
    let mut delayed = level_lfo("Sine", 4.0, 100.0);
    delayed.extend([
        (lfo::id(0, lfo::DELAY), 500.0),
        (lfo::id(0, lfo::ATTACK), 500.0),
    ]);
    let signal = render(&delayed, 48_000, &[on(0, 57)]);
    assert!(
        same(&signal[..24_000], &plain[..24_000]),
        "nothing moves during the delay"
    );
    let depth = |signal: &[f32], from: usize| {
        let window = &signal[from..from + 12_000];
        let (hi, lo) = window
            .iter()
            .fold((f32::MIN, f32::MAX), |(hi, lo), &x| (hi.max(x), lo.min(x)));
        f64::from(hi - lo)
    };
    let early = depth(&signal, 24_000);
    let late = depth(&signal, 36_000);
    assert!(
        late > 1.5 * early,
        "the attack brings it in: {early} then {late}"
    );
    // A quarter cycle in at the start, the sine is at its peak: +12 dB.
    let mut offset = level_lfo("Sine", 1.0, 100.0);
    offset.push((lfo::id(0, lfo::OFFSET), 90.0));
    let start = render(&offset, 4_800, &[on(0, 57)]);
    let gain = db(rms(&start[0..200]) / rms(&plain[0..200]));
    assert!((gain - 12.0).abs() < 0.5, "starts at the peak: {gain} dB");
}

// Spec: analog R18, R29. The random waves follow the device's seed.
#[test]
fn the_random_lfos_repeat_run_after_run_and_note_after_note_differ() {
    let mut params = level_lfo("S&H", 20.0, 100.0);
    params.push((osc::id(0, osc::PITCH_MOD), 30.0));
    let notes = [on(0, 57), off(9_000, 57), on(12_000, 57)];
    let a = render(&params, 24_000, &notes);
    let b = render(&params, 24_000, &notes);
    assert!(same(&a, &b), "the same seed, the same render");
    let first = &a[0..9_000];
    let second = &a[12_000..21_000];
    assert!(
        first.iter().zip(second).any(|(x, y)| (x - y).abs() > 1e-3),
        "the next note draws its own values"
    );
}

// Spec: analog R19. Vibrato moves the pitch by a sine up to a semitone, after
// its Delay and fading in over its Attack.
#[test]
fn vibrato_moves_the_pitch_a_semitone_after_its_delay_and_attack() {
    let params = [
        (analog::VIB_ON, 1.0),
        (analog::VIB_AMOUNT, 100.0),
        (analog::VIB_RATE, 4.0),
        (analog::VIB_DELAY, 500.0),
        (analog::VIB_ATTACK, 500.0),
    ];
    let signal = render(&params, 96_000, &[on(0, 69)]);
    let a4 = frequency(69);
    // During the delay the pitch holds.
    assert!(
        cents(pitch(&signal, 2_400, 22_000), a4).abs() < 3.0,
        "steady in the delay"
    );
    // Once faded in, the pitch swings to a semitone either way.
    let swings: Vec<f64> = (0..24)
        .map(|n| {
            cents(
                pitch(&signal, 60_000 + n * 1_000, 60_000 + n * 1_000 + 1_800),
                a4,
            )
        })
        .collect();
    let (high, low) = swings
        .iter()
        .fold((f64::MIN, f64::MAX), |(hi, lo), &c| (hi.max(c), lo.min(c)));
    assert!((high - 100.0).abs() < 12.0, "up {high} cents");
    assert!((low + 100.0).abs() < 12.0, "down {low} cents");
    // During the attack the swing is part-way.
    let early: Vec<f64> = (0..8)
        .map(|n| {
            cents(
                pitch(&signal, 26_000 + n * 800, 26_000 + n * 800 + 1_800),
                a4,
            )
            .abs()
        })
        .collect();
    let part = early.iter().copied().fold(0.0, f64::max);
    assert!(
        part > 5.0 && part < 60.0,
        "a part of the swing: {part} cents"
    );
}

// Spec: analog R19. Amt < MW adds the mod wheel's share to the amount.
#[test]
fn the_mod_wheel_adds_to_the_vibratos_amount() {
    let params = [
        (analog::VIB_ON, 1.0),
        (analog::VIB_RATE, 4.0),
        (analog::VIB_MOD_WHEEL, 100.0),
    ];
    let swing = |notes: &[NoteEvent]| {
        let signal = render(&params, 48_000, notes);
        (0..16)
            .map(|n| {
                cents(
                    pitch(&signal, 12_000 + n * 1_000, 12_000 + n * 1_000 + 1_800),
                    frequency(69),
                )
                .abs()
            })
            .fold(0.0, f64::max)
    };
    assert!(swing(&[on(0, 69)]) < 3.0, "the wheel down: no vibrato");
    let wheel_up = swing(&[mod_wheel(0, 1.0), on(0, 69)]);
    assert!(
        (wheel_up - 100.0).abs() < 12.0,
        "the wheel up: {wheel_up} cents"
    );
}

// Spec: analog R19, R29. Error varies each voice's rate and amount from the
// device's seed.
#[test]
fn error_varies_each_notes_vibrato_and_a_render_repeats() {
    let params = [
        (analog::VIB_ON, 1.0),
        (analog::VIB_AMOUNT, 60.0),
        (analog::VIB_RATE, 5.0),
        (analog::VIB_ERROR, 100.0),
    ];
    let notes = [on(0, 69), on(0, 69), off(24_000, 69), on(30_000, 69)];
    let a = render(&params, 60_000, &notes);
    let b = render(&params, 60_000, &notes);
    assert!(same(&a, &b), "the same seed, the same render");
    let flat = render(
        &[
            (analog::VIB_ON, 1.0),
            (analog::VIB_AMOUNT, 60.0),
            (analog::VIB_RATE, 5.0),
        ],
        60_000,
        &notes,
    );
    assert!(!same(&a, &flat), "Error changes the vibrato");
}

// Spec: analog R19. Volume is the device's output.
#[test]
fn volume_sets_the_output_down_to_silence() {
    let loud = render(&[], 4_800, &[on(0, 57)]);
    let quiet = render(&[(analog::VOLUME, db_to_gain(-6.0))], 4_800, &[on(0, 57)]);
    let silent = render(&[(analog::VOLUME, 0.0)], 4_800, &[on(0, 57)]);
    assert!((db(rms(&quiet[1_000..]) / rms(&loud[1_000..])) + 6.0).abs() < 0.1);
    assert!(rms(&silent) == 0.0, "−∞ dB is silence");
}

// Spec: analog R20. Unison stacks copies spread evenly over Detune, keeping
// the voice's level, and a Delay starts them one after another.
#[test]
fn unison_stacks_detuned_copies_at_the_voices_level() {
    let one = render(&[], 32_768, &[on(0, 69)]);
    let level = rms(&one[8_192..]);
    for (voices, spread) in [
        ("2", [-50.0, 50.0].as_slice()),
        ("4", &[-50.0, -50.0 / 3.0, 50.0 / 3.0, 50.0]),
    ] {
        let signal = render(
            &[
                (analog::UNI_ON, 1.0),
                (analog::UNI_VOICES, choice(&analog::UNISON_COPIES, voices)),
                (analog::UNI_DETUNE, 100.0),
            ],
            32_768,
            &[on(0, 69)],
        );
        let copies = voices.parse::<f64>().unwrap();
        for cents in spread {
            let hz = frequency(69) * 2f64.powf(cents / 1_200.0);
            let heard = synth::amplitude_at(&signal[8_192..], hz);
            let each = synth::amplitude_at(&one[8_192..], frequency(69)) / copies.sqrt();
            assert!(
                (heard / each - 1.0).abs() < 0.12,
                "{voices} copies: {heard} at {cents} cents against {each}"
            );
        }
        let kept = db(rms(&signal[8_192..]) / level);
        assert!(
            kept.abs() < 1.5,
            "{voices} copies keep the level: {kept} dB"
        );
    }
}

// Spec: analog R20. Delay starts copy i a share i/(n−1) of it after the first.
#[test]
fn unisons_delay_starts_the_last_copy_twenty_milliseconds_after_the_first() {
    let signal = render(
        &[
            (analog::UNI_ON, 1.0),
            (analog::UNI_DETUNE, 100.0),
            (analog::UNI_DELAY, 100.0),
        ],
        8_000,
        &[on(0, 69)],
    );
    let before = rms(&signal[200..900]);
    // The two copies, 25 Hz apart, beat every 1,920 frames: two beats.
    let after = rms(&signal[1_920..5_760]);
    assert!(
        (db(after / before) - 3.0).abs() < 1.2,
        "one copy for the first 960 frames, two after: {} dB",
        db(after / before)
    );
}

// Spec: analog R20, instrument voices R13. A voice and its copies count as one
// voice.
#[test]
fn a_voice_with_unison_counts_as_one_voice() {
    let params = [
        (analog::VOICE_COUNT, 2.0),
        (analog::UNI_ON, 1.0),
        (analog::UNI_VOICES, 1.0),
        (analog::UNI_DETUNE, 0.0),
    ];
    let signal = render(&params, 16_384, &[on(0, 57), on(0, 64)]);
    let both = &signal[4_096..];
    assert!(
        synth::amplitude_at(both, frequency(57)) > 0.05,
        "the first sounds"
    );
    assert!(
        synth::amplitude_at(both, frequency(64)) > 0.05,
        "the second sounds"
    );
}

// Spec: analog R21. Glide runs from the pitch before over Time, Const taking
// the same time for every interval and Prop scaling it with the interval.
#[test]
fn glide_slides_between_notes_in_const_or_prop_time() {
    let glide = |mode: &str, second: u8, extra: &[(ParamId, f64)]| {
        // One voice plays Mono: the second note takes it over, gliding.
        let mut params = vec![
            (analog::VOICE_COUNT, 1.0),
            (analog::GLIDE_ON, 1.0),
            (analog::GLIDE_TIME, 400.0),
            (analog::GLIDE_MODE, choice(&GLIDE_MODES, mode)),
        ];
        params.extend_from_slice(extra);
        render(&params, 40_000, &[on(0, 57), on(16_000, second)])
    };
    let wide = glide("Const", 69, &[]);
    let hz = |signal: &[f32], at: usize| pitch(signal, at, at + 1_000);
    assert!(
        (hz(&wide, 16_200) / frequency(57) - 1.0).abs() < 0.12,
        "starts from the first note"
    );
    assert!(
        (hz(&wide, 16_000 + 19_800) / frequency(69) - 1.0).abs() < 0.02,
        "arrives after Time"
    );
    let mid = hz(&wide, 16_000 + 9_000);
    assert!(
        mid > frequency(57) * 1.2 && mid < frequency(69) * 0.9,
        "on its way: {mid}"
    );
    // Prop: a fifth takes 7/12 of the time an octave does.
    let fifth = glide("Prop", 64, &[]);
    let octave = glide("Prop", 69, &[]);
    assert!(
        (hz(&fifth, 16_000 + 12_000) / frequency(64) - 1.0).abs() < 0.02,
        "a fifth has arrived"
    );
    let early = hz(&octave, 16_000 + 12_000);
    assert!(
        early < frequency(69) * 0.95,
        "the octave is still on its way: {early}"
    );
    assert!(
        (hz(&octave, 16_000 + 19_800) / frequency(69) - 1.0).abs() < 0.02,
        "and arrives after Time"
    );
    // Glide off, or at 0 ms, starts a note at its pitch.
    let off_glide = render(
        &[(analog::VOICE_COUNT, 1.0), (analog::GLIDE_TIME, 400.0)],
        40_000,
        &[on(0, 57), on(16_000, 69)],
    );
    assert!((hz(&off_glide, 16_200) / frequency(69) - 1.0).abs() < 0.02);
}

// Spec: analog R21. Legato glides only into a note played over a held one.
#[test]
fn glide_legato_glides_only_into_a_held_key() {
    let params = [
        (analog::VOICE_COUNT, 1.0),
        (analog::GLIDE_ON, 1.0),
        (analog::GLIDE_TIME, 400.0),
        (analog::GLIDE_LEGATO, 1.0),
    ];
    let hz = |signal: &[f32], at: usize| pitch(signal, at, at + 1_000);
    let released = render(
        &params,
        40_000,
        &[on(0, 57), off(8_000, 57), on(16_000, 69)],
    );
    assert!(
        (hz(&released, 16_400) / frequency(69) - 1.0).abs() < 0.02,
        "a note after the key was let go starts at its pitch"
    );
    let held = render(&params, 40_000, &[on(0, 57), on(16_000, 69)]);
    assert!(
        hz(&held, 16_400) < frequency(69) * 0.9,
        "a note over a held key glides"
    );
}

// Spec: analog R23. The keyboard's Octave, Semi and Detune transpose every
// note, and Stretch widens the octaves from C3.
#[test]
fn the_keyboard_transposes_and_stretches_the_notes() {
    let at = |params: &[(ParamId, f64)], key: u8| {
        let signal = render(params, 32_768, &[on(0, key)]);
        pitch(&signal, 8_192, 32_768)
    };
    let a4 = frequency(69);
    let moved = at(
        &[
            (analog::OCTAVE, 1.0),
            (analog::SEMI, -5.0),
            (analog::DETUNE, 50.0),
        ],
        69,
    );
    let expected = a4 * 2f64.powf((12.0 - 5.0 + 0.5) / 12.0);
    assert!(
        (moved / expected - 1.0).abs() < 0.003,
        "{moved} against {expected}"
    );
    // Stretch 100 %: C3 stays and each octave above is 12 cents wider.
    assert!((at(&[(analog::STRETCH, 100.0)], 60) / frequency(60) - 1.0).abs() < 0.002);
    let wide = at(&[(analog::STRETCH, 100.0)], 84);
    assert!(
        (cents(wide, frequency(84)) - 24.0).abs() < 4.0,
        "two octaves up: {} cents",
        cents(wide, frequency(84))
    );
}

// Spec: analog R23. The pitch wheel moves every note by PB Range.
#[test]
fn pb_range_sets_how_far_the_wheel_bends() {
    for range in [2.0, 12.0] {
        let signal = render(
            &[(analog::PB_RANGE, range)],
            32_768,
            &[on(0, 57), wheel(12_000, 1.0)],
        );
        let bent = pitch(&signal, 20_000, 30_000);
        let expected = frequency(57) * 2f64.powf(range / 12.0);
        assert!(
            (bent / expected - 1.0).abs() < 0.01,
            "range {range}: {bent} against {expected}"
        );
    }
}

// Spec: analog R23. Error detunes each note by up to half a semitone at 100 %.
#[test]
fn the_keyboards_error_detunes_notes_from_the_seed() {
    let render_error = |value: f64| render(&[(analog::ERROR, value)], 16_384, &[on(0, 57)]);
    assert!(
        same(&render_error(100.0), &render_error(100.0)),
        "repeatable"
    );
    let off_pitch = pitch(&render_error(100.0), 4_000, 16_000);
    let by = cents(off_pitch, frequency(57));
    assert!(by.abs() <= 50.5 && by.abs() > 0.1, "off by {by} cents");
    assert!(
        cents(pitch(&render_error(0.0), 4_000, 16_000), frequency(57)).abs() < 1.0,
        "no Error, no detune"
    );
}

// Spec: analog R23. Voices is the most notes that sound, and Priority says
// which a new note takes.
#[test]
fn voices_and_priority_decide_which_notes_sound() {
    let sounds =
        |signal: &[f32], key: u8| synth::amplitude_at(&signal[8_192..], frequency(key)) > 0.03;
    let play_three = |voices: f64, priority: &str, keys: [u8; 3]| {
        render(
            &[
                (analog::VOICE_COUNT, voices),
                (analog::PRIORITY, choice(&PRIORITY_NAMES, priority)),
            ],
            24_576,
            &[on(0, keys[0]), on(1_000, keys[1]), on(2_000, keys[2])],
        )
    };
    // Last: the new note takes the oldest voice.
    let last = play_three(2.0, "Last", [57, 64, 69]);
    assert!(
        !sounds(&last, 57) && sounds(&last, 64) && sounds(&last, 69),
        "Last"
    );
    // High keeps the highest notes: a lower new note isn't played.
    let high = play_three(2.0, "High", [57, 64, 50]);
    assert!(
        sounds(&high, 57) && sounds(&high, 64) && !sounds(&high, 50),
        "High"
    );
    // Low keeps the lowest.
    let low = play_three(2.0, "Low", [57, 64, 71]);
    assert!(
        sounds(&low, 57) && sounds(&low, 64) && !sounds(&low, 71),
        "Low"
    );
    // One voice plays Mono: the last note alone.
    let mono = play_three(1.0, "Last", [57, 64, 69]);
    assert!(
        !sounds(&mono, 57) && !sounds(&mono, 64) && sounds(&mono, 69),
        "Mono"
    );
}

const PRIORITY_NAMES: [&str; 3] = ["High", "Low", "Last"];

/// The value a destination chooser takes to name `name`.
fn destination(name: &str) -> f64 {
    1.0 + DESTINATIONS.iter().position(|d| *d == name).unwrap() as f64
}

fn pressure(frame: usize, key: u8, value: f32) -> NoteEvent {
    expression(frame, key, 0, NoteExpression::Pressure, value)
}

fn slide(frame: usize, key: u8, value: f32) -> NoteEvent {
    expression(frame, key, 0, NoteExpression::Slide, value)
}

// Spec: analog R24. Pressure and Slide move the destinations their choosers
// name, by their amounts, while MPE is on.
#[test]
fn pressure_and_slide_move_their_destinations_while_mpe_is_on() {
    let routed = |slot: usize, name: &str, amount: f64, on_switch: f64, notes: &[NoteEvent]| {
        render(
            &[
                (analog::MPE_ON, on_switch),
                (MPE[slot].0, destination(name)),
                (MPE[slot].1, amount),
            ],
            9_600,
            notes,
        )
    };
    let plain = render(&[], 9_600, &[on(0, 57)]);
    for (slot, source) in [
        (0, pressure(1, 57, 1.0)),
        (1, pressure(1, 57, 1.0)),
        (2, slide(1, 57, 1.0)),
        (3, slide(1, 57, 1.0)),
    ] {
        let moved = routed(slot, "Osc 1 Level", -100.0, 1.0, &[on(0, 57), source]);
        assert!(
            rms(&moved[2_000..]) < 0.1 * rms(&plain[2_000..]),
            "slot {slot} takes the level away"
        );
        assert!(
            same(
                &routed(slot, "Osc 1 Level", -100.0, 0.0, &[on(0, 57), source]),
                &plain
            ),
            "with MPE off, nothing moves"
        );
        assert!(
            same(
                &routed(slot, "Osc 1 Level", 0.0, 1.0, &[on(0, 57), source]),
                &plain
            ),
            "at 0 %, nothing moves"
        );
    }
    assert!(
        same(&routed(0, "Osc 1 Level", -100.0, 1.0, &[on(0, 57)]), &plain),
        "with no pressure, nothing moves"
    );
    // Every destination is reached: each moves the render.
    for (n, name) in DESTINATIONS.iter().enumerate() {
        let rich = |notes: &[NoteEvent]| {
            render(
                &[
                    (analog::MPE_ON, 1.0),
                    (MPE[0].0, destination(name)),
                    (MPE[0].1, -100.0),
                    (osc::id(0, osc::SHAPE), 2.0),
                    (osc::id(1, osc::ON), 1.0),
                    (osc::id(1, osc::BALANCE), 100.0),
                    (osc::id(1, osc::SHAPE), 2.0),
                    (noise::id(noise::ON), 1.0),
                    (noise::id(noise::LEVEL), 0.3),
                    (filter::id(0, filter::FREQ), 2_000.0),
                    (filter::id(0, filter::RESO), 50.0),
                    (filter::id(1, filter::ON), 1.0),
                    (filter::id(1, filter::FREQ), 2_000.0),
                    (filter::id(1, filter::RESO), 50.0),
                    (amp::id(1, amp::ON), 1.0),
                    (env::id(3, env::ATTACK), 0.0),
                    (env::id(3, env::DECAY), 0.0),
                    (env::id(3, env::SUSTAIN), 100.0),
                    (lfo::id(0, lfo::ON), 1.0),
                    (lfo::id(1, lfo::ON), 1.0),
                    (osc::id(0, osc::PITCH_MOD), 50.0),
                    (osc::id(1, osc::PITCH_MOD), 50.0),
                    (analog::VIB_ON, 1.0),
                    (analog::VIB_AMOUNT, 50.0),
                ],
                9_600,
                notes,
            )
        };
        let moved = rich(&[on(0, 57), pressure(1, 57, 1.0)]);
        assert!(
            !same(&moved, &rich(&[on(0, 57)])),
            "destination {n}, {name}, moves the sound"
        );
    }
}

// Spec: analog R24. Note PB bends one note alone, over its range.
#[test]
fn note_pb_bends_one_note_alone_over_its_range() {
    let notes = [
        on_id(0, 57, 1),
        on_id(0, 69, 2),
        expression(12_000, 57, 1, NoteExpression::Bend, 0.5),
    ];
    let signal = render(
        &[(analog::MPE_ON, 1.0), (analog::NOTE_PB, 12.0)],
        32_768,
        &notes,
    );
    let after = &signal[20_000..30_000];
    let bent = frequency(57) * 2f64.powf(6.0 / 12.0);
    assert!(
        synth::amplitude_at(after, bent) > 5.0 * synth::amplitude_at(after, frequency(57)),
        "the first bent six semitones"
    );
    assert!(
        synth::amplitude_at(after, frequency(69)) > 0.05,
        "the second didn't move"
    );
    // With MPE off a per-note bend does nothing.
    let flat = render(&[(analog::NOTE_PB, 12.0)], 32_768, &notes);
    let still = &flat[20_000..30_000];
    assert!(synth::amplitude_at(still, frequency(57)) > 5.0 * synth::amplitude_at(still, bent));
    // The range sets the bend: 24 semitones at half is a octave.
    let wide = render(
        &[(analog::MPE_ON, 1.0), (analog::NOTE_PB, 24.0)],
        32_768,
        &notes,
    );
    let octave = frequency(57) * 2.0;
    assert!(synth::amplitude_at(&wide[20_000..30_000], octave) > 0.05);
}

// Spec: analog R24. The activity lights show what the newest note plays.
#[test]
fn the_activity_lights_show_the_values_for_the_newest_note() {
    let mut node = node(
        &registry(),
        &analog(&[
            (analog::MPE_ON, 1.0),
            (MPE[0].0, destination("Osc 1 Level")),
            (MPE[0].1, -50.0),
        ]),
        4_096,
    );
    let mut seen = Vec::new();
    node.matrix_played(&mut |param, base, played| seen.push((param, base, played)));
    assert!(seen.is_empty(), "with no voice nothing is published");
    synth::render(
        node.as_mut(),
        4_096,
        4_096,
        &[on(0, 57), pressure(1, 57, 1.0)],
    );
    node.matrix_played(&mut |param, base, played| seen.push((param, base, played)));
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].0, osc::id(0, osc::LEVEL));
    assert!(seen[0].2 < 0.5 * seen[0].1, "the lowered level: {seen:?}");
}

/// LFOs of every kind, vibrato with Error, unison with Detune and Delay, and
/// the keyboard's Error: everything random in part 2.
const RANDOM_PARTS: [(ParamId, f64); 16] = [
    (lfo::id(0, lfo::ON), 1.0),
    (lfo::id(0, lfo::WAVE), 4.0),
    (lfo::id(0, lfo::RATE), 7.0),
    (filter::id(0, filter::FREQ), 3_000.0),
    (filter::id(0, filter::FREQ_LFO), 40.0),
    (lfo::id(1, lfo::ON), 1.0),
    (lfo::id(1, lfo::WAVE), 3.0),
    (amp::id(0, amp::LEVEL_LFO), 30.0),
    (analog::VIB_ON, 1.0),
    (analog::VIB_AMOUNT, 30.0),
    (analog::VIB_ERROR, 80.0),
    (analog::UNI_ON, 1.0),
    (analog::UNI_VOICES, 1.0),
    (analog::UNI_DETUNE, 60.0),
    (analog::UNI_DELAY, 70.0),
    (analog::ERROR, 40.0),
];

fn phrase() -> [NoteEvent; 5] {
    [
        on(0, 48),
        on(3_000, 55),
        off(9_000, 48),
        on(11_000, 60),
        off(14_000, 55),
    ]
}

// Spec: analog R29, R17. Every random value comes from the device's seed, so
// LFOs, vibrato and unison render the same run after run and at any block
// size, one frame included.
#[test]
fn lfos_vibrato_and_unison_render_the_same_run_after_run_and_at_any_block_size() {
    let notes = phrase();
    let reference = play(&RANDOM_PARTS, 18_000, 4_096, &notes);
    assert!(rms(&reference) > 0.01, "it plays");
    assert!(
        same(&reference, &play(&RANDOM_PARTS, 18_000, 4_096, &notes)),
        "run after run"
    );
    for block in BLOCKS {
        assert!(
            same(&play(&RANDOM_PARTS, 18_000, block, &notes), &reference),
            "block {block}"
        );
    }
}

// Spec: analog R17, R21, R29. A pitch that an LFO or glide moves renders the
// same at 64, 512 and 4,096 frames a block; the kit places a mipmap change
// by block, so a block of one frame differs (the limit R17 gives).
#[test]
fn a_pitch_an_lfo_and_glide_move_renders_the_same_at_blocks_of_64_and_more() {
    let mut params = RANDOM_PARTS.to_vec();
    params.extend([
        (osc::id(0, osc::PITCH_MOD), 8.0),
        (analog::GLIDE_ON, 1.0),
        (analog::GLIDE_TIME, 120.0),
    ]);
    let notes = phrase();
    let reference = play(&params, 18_000, 4_096, &notes);
    assert!(
        same(&reference, &play(&params, 18_000, 4_096, &notes)),
        "run after run"
    );
    for block in [64, 512] {
        assert!(
            same(&play(&params, 18_000, block, &notes), &reference),
            "block {block}"
        );
    }
}

// Spec: analog R17, R24. Parameters of part 2 are described, automatable and
// reach the device's banks.
#[test]
fn part_two_is_described_and_in_the_banks() {
    let ty = analog_type();
    for name in [
        "LFO 1",
        "LFO 2 Offset",
        "Volume",
        "Vibrato Amt < MW",
        "Unison Voices",
        "Glide Mode",
        "Keyboard Stretch",
        "Voices",
        "Priority",
        "MPE",
        "Pressure Dest 1",
        "Slide Amount 2",
        "Note PB",
    ] {
        assert!(
            ty.description.params.iter().any(|p| p.name == name),
            "{name} is a parameter"
        );
    }
    let matrix = ty.matrix.expect("Analog has a matrix");
    assert_eq!(matrix.slots.len(), 4);
    assert_eq!(matrix.sources.len(), 2);
    assert_eq!(matrix.dests.len(), 16);
}

// Spec: analog R24. Pressure moves the note it belongs to and no other.
#[test]
fn pressure_moves_one_note_alone() {
    let notes = [
        on_id(0, 57, 1),
        on_id(0, 69, 2),
        expression(2_000, 57, 1, NoteExpression::Pressure, 1.0),
    ];
    let signal = render(
        &[
            (analog::MPE_ON, 1.0),
            (MPE[0].0, destination("Osc 1 Level")),
            (MPE[0].1, -100.0),
        ],
        16_384,
        &notes,
    );
    let after = &signal[8_192..];
    assert!(
        synth::amplitude_at(after, frequency(57))
            < 0.05 * synth::amplitude_at(after, frequency(69)),
        "the pressed note lost its level"
    );
    assert!(
        synth::amplitude_at(after, frequency(69)) > 0.05,
        "the other kept it"
    );
}
