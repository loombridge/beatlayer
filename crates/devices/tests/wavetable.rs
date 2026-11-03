//! Wavetable, part 1 (WP-8.9; the wavetable spec, R11 to R24): its
//! oscillators, their effects, the sub oscillator, the filters and their
//! routing, the amplitude envelope, the voices and unison's amount.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common;
use std::time::Instant;

use bl_core::time::{Bpm, Frames, TempoMap, TempoSegment, Ticks};
use bl_devices::wavetable::{
    AMP_ATTACK, AMP_RELEASE, AMP_SUSTAIN, EFFECT, EFFECTS, ENV_ATTACK, ENV_ATTACK_SLOPE, ENV_DECAY,
    ENV_FINAL, ENV_INITIAL, ENV_MODE, ENV_PEAK, ENV_RELEASE, ENV_SUSTAIN, FILTER_FREQ, FILTER_ON,
    FILTER_TYPE, FM_AMOUNT, FM_TUNE, FOLD, GAIN, GLIDE, HI_QUALITY, LFO_AMOUNT, LFO_ATTACK,
    LFO_DIVISION, LFO_OFFSET, LFO_RATE, LFO_RETRIGGER, LFO_SHAPE, LFO_SYNC, LFO_WAVE, MATRIX_TIME,
    OSC_ON, PAGES, POLY_VOICES, POSITION, PW, ROUTING, SEMI, SLOTS, SUB_GAIN, SUB_OCTAVE, SUB_ON,
    SUB_TONE, SYNC, TABS, TRANSPOSE, UNISON_AMOUNT, UNISON_MODE, UNISON_VOICES, VOICE_MODE, VOLUME,
    WARP, WAVETABLE, envelope, filter, lfo, osc, slot_amount,
};
use bl_devices::wavetable_mod::{DIVISIONS, LFO_WAVES, LOOP_MODES, UNISON_MODES};
use bl_devices::wavetables::{OscTable, Source};
use bl_engine::{
    Block, Collector, DeviceType, Node, NoteBuffer, NoteEvent, NoteKind, Registry, TransportInfo,
};
use bl_model::device_layout::LayoutNode;
use bl_model::{DeviceKind, DeviceRole, ParamId};
use common::{CHANNELS, RATE, device, node, set};
use realfft::RealFftPlanner;

/// The sample rate the tests run at, in hertz.
const HZ: f64 = 48_000.0;

/// The block sizes a render must be the same at (R20).
const BLOCKS: [usize; 4] = [1, 64, 512, 4_096];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

fn wavetable() -> DeviceType {
    bl_devices::device_types()
        .into_iter()
        .find(|t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == WAVETABLE))
        .expect("Wavetable is registered")
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

/// Runs one block of `frames` through `node`, playing `notes` and applying
/// `events`, and returns both channels.
fn play(
    node: &mut dyn Node,
    frames: usize,
    notes: &[NoteEvent],
    events: &[bl_engine::NodeEvent],
) -> Vec<Vec<f32>> {
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut out = NoteBuffer::with_capacity(64);
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            events,
            TransportInfo::default(),
            RATE,
        )
        .with_notes(notes, &mut out);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

/// Plays `notes` and `events` for `frames`, `block` frames at a time, and
/// returns both channels whole.
fn play_blocks(
    node: &mut dyn Node,
    frames: usize,
    block: usize,
    notes: &[NoteEvent],
    events: &[bl_engine::NodeEvent],
) -> Vec<Vec<f32>> {
    let mut signal: Vec<Vec<f32>> = (0..CHANNELS).map(|_| Vec::with_capacity(frames)).collect();
    let mut at = 0;
    while at < frames {
        let len = block.min(frames - at);
        let within = |offset: u32| (offset as usize) >= at && (offset as usize) < at + len;
        let block_notes: Vec<NoteEvent> = notes
            .iter()
            .filter(|n| within(n.offset))
            .map(|n| NoteEvent {
                offset: n.offset - at as u32,
                ..*n
            })
            .collect();
        let block_events: Vec<bl_engine::NodeEvent> = events
            .iter()
            .filter(|e| within(e.offset))
            .map(|e| bl_engine::NodeEvent {
                offset: e.offset - at as u32,
                ..*e
            })
            .collect();
        for (all, part) in signal
            .iter_mut()
            .zip(play(node, len, &block_notes, &block_events))
        {
            all.extend(part);
        }
        at += len;
    }
    signal
}

/// A Wavetable with `params` over the shared defaults: Oscillator 1 alone,
/// no filter, Volume at 0 dB.
fn wavetable_with(params: &[(ParamId, f64)]) -> bl_model::Device {
    let mut all = vec![
        (filter(1, FILTER_ON), 0.0),
        (VOLUME, 1.0),
        (bl_devices::wavetable::AMP_ATTACK, 1.0),
    ];
    all.extend_from_slice(params);
    device(&wavetable(), &all)
}

/// The left channel of `notes` played for `frames` through a Wavetable with
/// `params`.
fn render(params: &[(ParamId, f64)], notes: &[NoteEvent], frames: usize) -> Vec<f32> {
    let registry = registry();
    let mut node = node(&registry, &wavetable_with(params), 512);
    play_blocks(node.as_mut(), frames, 512, notes, &[]).remove(0)
}

/// A table of the factory list, sent to oscillator `n` of `node`.
fn give(node: &mut dyn Node, n: u8, category: &str, name: &str) {
    let collector = Collector::new();
    let table = Source::Factory {
        category: category.into(),
        name: name.into(),
    }
    .build()
    .expect("a factory table");
    node.receive_data(collector.handle().data(OscTable {
        oscillator: n,
        table,
    }));
    // The collector frees what the node drops, off the test's section.
    std::mem::forget(collector);
}

/// The frequency of `key` with A4 at 440 Hz.
fn frequency(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// The amplitude of `hz` in `signal`, by a Hann-windowed DFT.
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

/// The root mean square of `signal`.
fn rms(signal: &[f32]) -> f64 {
    (signal
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        / signal.len() as f64)
        .sqrt()
}

/// `a` in decibels over `b`.
fn db(a: f64, b: f64) -> f64 {
    20.0 * (a / b.max(1e-12)).log10()
}

/// The part of `signal` after the first 0.25 s: a note's steady state.
fn steady(signal: &[f32]) -> &[f32] {
    &signal[HZ as usize / 4..]
}

fn choice(list: &[&str], name: &str) -> f64 {
    list.iter().position(|&n| n == name).expect("in the list") as f64
}

/// The note the amplitude tests hold, from frame 0.
fn held(key: u8) -> Vec<NoteEvent> {
    vec![on(0, key, 1.0)]
}

// Spec: wavetable R11, R22
#[test]
fn it_is_listed_as_an_instrument_with_the_four_tabs() {
    let device_type = wavetable();
    assert_eq!(device_type.role, DeviceRole::Instrument);
    device_type.description.check().unwrap();
    let LayoutNode::Pages(tabs) = &device_type.description.layout.root else {
        panic!("the body is a pages node");
    };
    assert_eq!(tabs.name, TABS);
    let names: Vec<&str> = tabs.pages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, PAGES);
}

// Spec: wavetable R12, R22 (every control is a parameter, each on one page)
#[test]
fn every_control_is_a_described_parameter() {
    let description = wavetable().description;
    for id in [
        osc(1, OSC_ON),
        osc(1, POSITION),
        osc(2, OSC_ON),
        osc(2, SEMI),
        SUB_ON,
        SUB_GAIN,
        SUB_TONE,
        SUB_OCTAVE,
        filter(1, FILTER_ON),
        filter(2, FILTER_TYPE),
        ROUTING,
        AMP_ATTACK,
        AMP_SUSTAIN,
        VOICE_MODE,
        POLY_VOICES,
        GLIDE,
        TRANSPOSE,
        VOLUME,
        UNISON_VOICES,
        UNISON_AMOUNT,
    ] {
        assert!(description.param(id).is_some(), "{id:?}");
    }
    // A new device starts as R12 says: Oscillator 1 on, 2 off, Position 0.
    let defaults = wavetable_with(&[]);
    let value = |id| description.value(&defaults.params, id).unwrap();
    assert_eq!(value(osc(1, OSC_ON)), 1.0);
    assert_eq!(description.param(osc(2, OSC_ON)).unwrap().default, 0.0);
    assert_eq!(description.param(osc(1, POSITION)).unwrap().default, 0.0);
    assert_eq!(value(SUB_ON), 0.0);
}

// Spec: wavetable R11, R12 (a note plays at the note's pitch)
#[test]
fn a_note_plays_at_the_notes_pitch() {
    for key in [45u8, 57, 69] {
        let signal = render(&[], &held(key), 48_000);
        let peak = peak_frequency(steady(&signal), 20.0, 2_000.0);
        assert!(
            (peak - frequency(key)).abs() < 3.0,
            "key {key}: peak at {peak} Hz"
        );
        assert!(rms(steady(&signal)) > 0.01, "key {key} sounds");
    }
}

// Spec: wavetable R13 (Position chooses the frame)
#[test]
fn position_moves_through_the_tables_frames() {
    // Sine to Saw: a sine at 0 %, a saw at 100 %.
    let f0 = frequency(45);
    let at = |position: f64| {
        let signal = render(&[(osc(1, POSITION), position)], &held(45), 24_000);
        let s = steady(&signal);
        db(amplitude_at(s, 2.0 * f0), amplitude_at(s, f0))
    };
    assert!(
        at(0.0) < -50.0,
        "a sine has no second harmonic: {}",
        at(0.0)
    );
    assert!(
        (-9.0..=-3.0).contains(&at(100.0)),
        "a saw's second harmonic is half its first: {}",
        at(100.0)
    );
    assert!(at(50.0) > at(0.0) + 10.0, "halfway there is some");
}

// Spec: wavetable R13, R24 (Position at audio rate makes no click)
#[test]
fn position_swept_at_audio_rate_scans_the_frames_without_clicks() {
    let registry = registry();
    let mut device = wavetable_with(&[]);
    device.params.insert(osc(1, GAIN), 1.0);
    let mut node = node(&registry, &device, 512);
    // Sine Folds is smooth at every frame, so any step in the output comes
    // from the way Position is read. Position moves every 24 frames, a 2 kHz
    // staircase triangle between 20 % and 80 %.
    give(node.as_mut(), 1, "Shape Morphs", "Sine Folds");
    let frames = 24_000;
    let events: Vec<_> = (0..frames / 24)
        .map(|i| {
            let phase = (i % 24) as f64 / 24.0;
            let triangle = 1.0 - (2.0 * phase - 1.0).abs();
            set((i * 24) as u32, osc(1, POSITION), 20.0 + 60.0 * triangle)
        })
        .collect();
    let signal = play_blocks(node.as_mut(), frames, 512, &held(45), &events).remove(0);
    let tail = steady(&signal);
    let peak = tail.iter().fold(0.0f32, |p, x| p.max(x.abs()));
    assert!(peak > 0.05, "it sounds");
    let jump = tail
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        jump < 0.35 * peak,
        "the largest step is {jump} of a {peak} peak"
    );
}

// Spec: wavetable R14 (FM: the modulator sits at the tuning ratio)
#[test]
fn fms_sidebands_sit_at_its_tuning_ratio() {
    let f0 = frequency(45);
    let at = |tune: f64| {
        let params = [
            (osc(1, EFFECT), choice(&EFFECTS, "FM")),
            (osc(1, FM_AMOUNT), 25.0),
            (osc(1, FM_TUNE), tune),
        ];
        let signal = render(&params, &held(45), 48_000);
        let s = steady(&signal);
        let fundamental = amplitude_at(s, f0);
        (
            db(amplitude_at(s, 2.0 * f0), fundamental),
            db(amplitude_at(s, 3.0 * f0), fundamental),
        )
    };
    // An octave up (+50 %): sidebands at f0 ± 2k f0, so odd harmonics only.
    let (second, third) = at(50.0);
    assert!(third > -30.0, "the third harmonic is a sideband: {third}");
    assert!(
        second < -45.0,
        "no second harmonic an octave away: {second}"
    );
    // At the note's own frequency (0 %): sidebands at every harmonic.
    let (second, _) = at(0.0);
    assert!(second > -20.0, "the second is a sideband at 0 %: {second}");
    // Two octaves down (−100 %): sidebands every quarter of f0.
    let params = [
        (osc(1, EFFECT), choice(&EFFECTS, "FM")),
        (osc(1, FM_AMOUNT), 25.0),
        (osc(1, FM_TUNE), -100.0),
    ];
    let signal = render(&params, &held(45), 48_000);
    let s = steady(&signal);
    assert!(
        db(amplitude_at(s, 0.75 * f0), amplitude_at(s, f0)) > -30.0,
        "a sideband a quarter below"
    );
}

// Spec: wavetable R14 (Classic: PW narrows the cycle, Sync raises the ratio)
#[test]
fn classics_pw_narrows_the_cycle_and_sync_raises_the_ratio() {
    let f0 = frequency(45);
    let classic = choice(&EFFECTS, "Classic");
    // PW: the share of the cycle in which the wave moves falls from nearly
    // all to a tenth.
    let active = |pw: f64| {
        let params = [(osc(1, EFFECT), classic), (osc(1, PW), pw)];
        let signal = render(&params, &held(45), 24_000);
        let s = steady(&signal);
        let peak = s.iter().fold(0.0f32, |p, x| p.max(x.abs()));
        s.iter().filter(|x| x.abs() > 0.1 * peak).count() as f64 / s.len() as f64
    };
    let (wide, narrow) = (active(0.0), active(100.0));
    assert!(
        wide > 0.5,
        "an unshaped sine is above a tenth most of the time: {wide}"
    );
    assert!(
        narrow < 0.25,
        "a full PW leaves a tenth of the cycle: {narrow}"
    );
    // Sync: the wave's own ratio to the hidden oscillator rises to 8.
    let ratio = |sync: f64| {
        let params = [(osc(1, EFFECT), classic), (osc(1, SYNC), sync)];
        let signal = render(&params, &held(45), 48_000);
        peak_frequency(steady(&signal), 20.0, 2_000.0) / f0
    };
    assert!((ratio(0.0) - 1.0).abs() < 0.1, "no sync: {}", ratio(0.0));
    assert!(
        (7.0..9.0).contains(&ratio(100.0)),
        "full sync: {}",
        ratio(100.0)
    );
    let half = ratio(50.0);
    assert!((2.0..6.0).contains(&half), "half sync is between: {half}");
}

// Spec: wavetable R14 (Modern: Warp and Fold change the spectrum)
#[test]
fn moderns_warp_and_fold_change_the_spectrum() {
    let f0 = frequency(45);
    let modern = choice(&EFFECTS, "Modern");
    let high = |params: &[(ParamId, f64)]| {
        let mut all = vec![(osc(1, EFFECT), modern)];
        all.extend_from_slice(params);
        let signal = render(&all, &held(45), 24_000);
        let s = steady(&signal);
        (3..12).map(|h| amplitude_at(s, h as f64 * f0)).sum::<f64>() / amplitude_at(s, f0)
    };
    let plain = high(&[]);
    assert!(plain < 0.01, "a plain sine has no upper harmonics: {plain}");
    assert!(
        high(&[(osc(1, FOLD), 100.0)]) > plain + 0.3,
        "Fold adds harmonics"
    );
    assert!(
        high(&[(osc(1, WARP), 100.0)]) > plain + 0.05,
        "Warp adds harmonics"
    );
}

// Spec: wavetable R23
#[test]
fn only_fm_classic_and_modern_are_offered() {
    assert_eq!(EFFECTS, ["None", "FM", "Classic", "Modern"]);
    let description = wavetable().description;
    let effect = description.param(osc(1, EFFECT)).unwrap();
    assert_eq!(effect.max() as usize + 1, EFFECTS.len());
}

// Spec: wavetable R15 (the sub sits octaves down; Tone adds harmonics)
#[test]
fn the_sub_sits_an_octave_or_two_down_and_tone_adds_harmonics() {
    let f0 = frequency(57);
    let sub = |octave: f64, tone: f64| {
        let params = [
            (osc(1, OSC_ON), 0.0),
            (SUB_ON, 1.0),
            (SUB_OCTAVE, octave),
            (SUB_TONE, tone),
        ];
        render(&params, &held(57), 48_000)
    };
    for (octave, ratio) in [(-1.0, 0.5), (-2.0, 0.25)] {
        let signal = sub(octave, 0.0);
        let peak = peak_frequency(steady(&signal), 20.0, 1_000.0);
        assert!(
            (peak - f0 * ratio).abs() < 2.0,
            "octave {octave}: peak at {peak} Hz"
        );
    }
    let sine = sub(-1.0, 0.0);
    let square = sub(-1.0, 100.0);
    let third = |s: &[f32]| {
        db(
            amplitude_at(steady(s), 1.5 * f0),
            amplitude_at(steady(s), 0.5 * f0),
        )
    };
    assert!(third(&sine) < -50.0, "a sine has none: {}", third(&sine));
    assert!(
        (-12.0..-6.0).contains(&third(&square)),
        "a square's third harmonic is a third: {}",
        third(&square)
    );
}

/// The amplitudes at 440 and 1,760 Hz of two oscillators two octaves apart
/// through the filters `params` set, routed as `routing` says.
fn two_sines(routing: &str, params: &[(ParamId, f64)]) -> (f64, f64) {
    let mut all = vec![
        (osc(1, OSC_ON), 1.0),
        (osc(2, OSC_ON), 1.0),
        (osc(2, SEMI), 24.0),
        (ROUTING, choice(&bl_devices::wavetable::ROUTINGS, routing)),
    ];
    all.extend_from_slice(params);
    let signal = render(&all, &held(69), 24_000);
    let s = steady(&signal);
    (amplitude_at(s, 440.0), amplitude_at(s, 1_760.0))
}

// Spec: wavetable R16 (each routing reaches the output as S3 says)
#[test]
fn each_filter_routing_reaches_the_output_as_the_spec_says() {
    // Low-pass 24 at 600 Hz, and a high-pass 24 at 5 kHz.
    let lp = choice(&bl_devices::sampler::FILTER_TYPE_CHOICES, "Low-pass 24");
    let hp = choice(&bl_devices::sampler::FILTER_TYPE_CHOICES, "High-pass 24");
    let filter1 = |on: f64| {
        [
            (filter(1, FILTER_ON), on),
            (filter(1, FILTER_TYPE), lp),
            (filter(1, FILTER_FREQ), 600.0),
        ]
    };
    let filter2 = |on: f64, kind: f64, hz: f64| {
        [
            (filter(2, FILTER_ON), on),
            (filter(2, FILTER_TYPE), kind),
            (filter(2, FILTER_FREQ), hz),
        ]
    };
    // Serial: both oscillators through Filter 1, so 1,760 Hz is gone.
    let mut serial = filter1(1.0).to_vec();
    serial.extend(filter2(0.0, lp, 20_000.0));
    let (low, high) = two_sines("Serial", &serial);
    assert!(db(high, low) < -25.0, "serial: {} dB", db(high, low));
    // Serial with Filter 1 off and Filter 2 low-pass: the same, one filter on.
    let mut serial_two = filter1(0.0).to_vec();
    serial_two.extend(filter2(1.0, lp, 600.0));
    let (low, high) = two_sines("Serial", &serial_two);
    assert!(
        db(high, low) < -25.0,
        "serial through Filter 2: {} dB",
        db(high, low)
    );
    // Parallel with Filter 2 off: Oscillator 2 passes through unfiltered.
    let mut parallel = filter1(1.0).to_vec();
    parallel.extend(filter2(0.0, lp, 20_000.0));
    let (low, high) = two_sines("Parallel", &parallel);
    assert!(
        db(high, low).abs() < 3.0,
        "parallel keeps both: {} dB",
        db(high, low)
    );
    // Split with a high-pass on Oscillator 2: it is cut and 1 stays.
    let mut split = filter1(1.0).to_vec();
    split.extend(filter2(1.0, hp, 5_000.0));
    let (low, high) = two_sines("Split", &split);
    assert!(low > 0.1, "Oscillator 1 sounds through Filter 1: {low}");
    assert!(
        db(high, low) < -25.0,
        "Oscillator 2 is cut by Filter 2: {} dB",
        db(high, low)
    );
    // Split with Filter 2 off keeps Oscillator 2.
    let mut split_off = filter1(1.0).to_vec();
    split_off.extend(filter2(0.0, hp, 5_000.0));
    let (low, high) = two_sines("Split", &split_off);
    assert!(
        db(high, low).abs() < 3.0,
        "split with a filter off: {} dB",
        db(high, low)
    );
}

// Spec: wavetable R15, R16 (the sub enters the filters)
#[test]
fn the_sub_goes_through_the_filters() {
    let params = |routing: &str, freq: f64| {
        let mut all = vec![
            (osc(1, OSC_ON), 0.0),
            (SUB_ON, 1.0),
            (filter(1, FILTER_ON), 1.0),
            (filter(1, FILTER_TYPE), 1.0),
            (filter(1, FILTER_FREQ), freq),
            (filter(2, FILTER_ON), 1.0),
            (filter(2, FILTER_TYPE), 1.0),
            (filter(2, FILTER_FREQ), freq),
            (ROUTING, choice(&bl_devices::wavetable::ROUTINGS, routing)),
        ];
        all.extend_from_slice(&[(SUB_TONE, 100.0)]);
        render(&all, &held(57), 24_000)
    };
    let f0 = frequency(57) / 2.0;
    for routing in ["Serial", "Parallel", "Split"] {
        let open = params(routing, 20_000.0);
        let shut = params(routing, 50.0);
        assert!(rms(steady(&open)) > 0.05, "{routing}: the sub sounds");
        assert!(
            rms(steady(&shut)) < 0.1 * rms(steady(&open)),
            "{routing}: Filter 1 at 50 Hz cuts a {f0} Hz square's harmonics"
        );
    }
}

// Spec: wavetable R17 (the amplitude envelope; a voice ends in silence)
#[test]
fn the_amp_envelope_shapes_each_voice_and_it_ends_when_silent() {
    let params = [
        (AMP_ATTACK, 200.0),
        (AMP_RELEASE, 100.0),
        (AMP_SUSTAIN, 100.0),
    ];
    let signal = render(&params, &[on(0, 57, 1.0), off(24_000, 57)], 48_000);
    let window = |from: usize| rms(&signal[from..from + 480]);
    assert!(window(0) < 0.3 * window(12_000), "the attack takes 200 ms");
    assert!(window(20_000) > 0.05, "it sustains");
    assert!(
        window(24_000 + 12_000) < 1e-4,
        "it is silent after the release"
    );
    // A slope bends a segment: a positive attack slope is quick at the start.
    let rise = |slope: f64| {
        let params = [
            (AMP_ATTACK, 200.0),
            (bl_devices::wavetable::AMP_ATTACK_SLOPE, slope),
        ];
        let signal = render(&params, &held(57), 12_000);
        rms(&signal[2_400..2_880])
    };
    assert!(rise(1.0) > 1.5 * rise(0.0), "positive starts fast");
    assert!(rise(-1.0) < 0.6 * rise(0.0), "negative starts slow");
}

// Spec: wavetable R18 (Poly Voices; Mono; Glide; Transpose and Volume)
#[test]
fn poly_plays_polyvoices_notes_mono_glides_and_transpose_and_volume_act() {
    // Poly Voices: with 2, a third note takes a voice and leaves two sounding.
    let keys = [45u8, 52, 57];
    let chord: Vec<NoteEvent> = keys.iter().map(|&k| on(0, k, 1.0)).collect();
    let signal = render(&[(POLY_VOICES, 2.0)], &chord, 24_000);
    let s = steady(&signal);
    let sounding = keys
        .iter()
        .filter(|&&k| amplitude_at(s, frequency(k)) > 0.02)
        .count();
    assert_eq!(sounding, 2, "two voices of three notes");
    // Mono plays one note: the last.
    let mono = render(&[(VOICE_MODE, 1.0)], &chord, 24_000);
    let s = steady(&mono);
    assert!(amplitude_at(s, frequency(57)) > 0.1);
    assert!(amplitude_at(s, frequency(45)) < 0.01, "one note sounds");
    // Glide: a mono legato note slides, so just after the second note the
    // pitch is still near the first.
    let notes = [on(0, 57, 1.0), on(12_000, 69, 1.0)];
    let glide = render(&[(VOICE_MODE, 1.0), (GLIDE, 1_000.0)], &notes, 24_000);
    let early = peak_frequency(&glide[12_000..12_000 + 4_096], 100.0, 1_000.0);
    let late = peak_frequency(&glide[24_000 - 4_096..], 100.0, 1_000.0);
    assert!(
        early < 0.8 * frequency(69),
        "still sliding up at {early} Hz"
    );
    assert!(late > early, "and rising: {late} Hz");
    // Transpose moves the pitch; Volume scales the level.
    let up = render(&[(TRANSPOSE, 12.0)], &held(45), 24_000);
    let peak = peak_frequency(steady(&up), 20.0, 2_000.0);
    assert!(
        (peak - 2.0 * frequency(45)).abs() < 3.0,
        "an octave up: {peak}"
    );
    let loud = rms(steady(&render(&[(VOLUME, 1.0)], &held(45), 24_000)));
    let quiet = rms(steady(&render(&[(VOLUME, 0.5)], &held(45), 24_000)));
    assert!((quiet / loud - 0.5).abs() < 0.02, "Volume halves the level");
}

// Spec: wavetable R19 (Unison: Voices and Amount on Classic's law)
#[test]
fn unison_stacks_copies_that_detune_with_amount() {
    let f0 = frequency(57);
    let on_off = |amount: f64| {
        let params = [(UNISON_VOICES, 4.0), (UNISON_AMOUNT, amount)];
        let signal = render(&params, &held(57), 48_000);
        let s = steady(&signal);
        (rms(s), amplitude_at(s, f0 * 2f64.powf(-25.0 / 1200.0)))
    };
    let (level0, outer0) = on_off(0.0);
    let (level1, outer1) = on_off(100.0);
    // At 0 % one copy plays at the level of a plain note.
    let plain = rms(steady(&render(&[], &held(57), 48_000)));
    assert!((level0 / plain - 1.0).abs() < 0.02, "no unison at 0 %");
    // At 100 % the outer copies sit 25 cents either way; the left channel
    // holds the first copy, 25 cents down.
    assert!(
        outer1 > 4.0 * outer0.max(1e-4),
        "a copy at −25 cents: {outer1} vs {outer0}"
    );
    // The copies' sum keeps the level of one voice, within a few dB.
    assert!(
        db(level1, level0).abs() < 4.0,
        "level {} dB",
        db(level1, level0)
    );
}

/// A phrase that uses what part 1 has: two notes overlapping and a third,
/// Classic on Oscillator 1, FM on Oscillator 2, the sub, unison, both
/// filters in parallel, rendered `block` frames at a time.
fn phrase(block: usize) -> Vec<Vec<f32>> {
    let notes = [
        on(0, 45, 1.0),
        on(3_000, 52, 0.8),
        off(20_000, 45),
        on(22_000, 64, 0.6),
        off(30_000, 52),
        off(34_000, 64),
    ];
    let params = [
        (osc(1, EFFECT), choice(&EFFECTS, "Classic")),
        (osc(1, PW), 40.0),
        (osc(1, SYNC), 30.0),
        (osc(2, OSC_ON), 1.0),
        (osc(2, EFFECT), choice(&EFFECTS, "FM")),
        (osc(2, FM_AMOUNT), 20.0),
        (osc(2, FM_TUNE), 30.0),
        (osc(2, SEMI), 7.0),
        (SUB_ON, 1.0),
        (SUB_TONE, 40.0),
        (UNISON_VOICES, 3.0),
        (UNISON_AMOUNT, 60.0),
        (filter(1, FILTER_ON), 1.0),
        (filter(1, FILTER_FREQ), 3_000.0),
        (filter(2, FILTER_ON), 1.0),
        (filter(2, FILTER_FREQ), 5_000.0),
        (ROUTING, 1.0),
    ];
    let mut node = node(&registry(), &wavetable_with(&params), 4_096);
    play_blocks(node.as_mut(), 40_000, block, &notes, &[])
}

// Spec: wavetable R20 (the same at any block size)
#[test]
fn a_phrase_renders_the_same_at_block_sizes_1_64_512_and_4096() {
    let reference = phrase(4_096);
    assert!(rms(&reference[0]) > 0.01, "the phrase sounds");
    for block in BLOCKS {
        for (channel, want) in phrase(block).iter().zip(&reference) {
            let worst = channel
                .iter()
                .zip(want)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 1e-3, "block size {block}: {worst}");
        }
    }
}

/// FNV-1a over the samples' bits: what a golden render compares.
fn digest(signal: &[f32]) -> u64 {
    signal.iter().fold(0xcbf2_9ce4_8422_2325, |hash, sample| {
        (hash ^ u64::from(sample.to_bits())).wrapping_mul(0x0100_0000_01b3)
    })
}

// Spec: wavetable R20 (a golden render, and run after run)
#[test]
fn the_phrase_renders_to_its_golden_digest_run_after_run() {
    let signal = phrase(512);
    assert_eq!(phrase(512), signal, "run after run");
    let both: Vec<f32> = signal.concat();
    // A vector unit of another architecture rounds some samples
    // differently, so only the one the digest was taken on compares it. The
    // digest changed with part 2: Hi-Quality, off in this phrase, renders the
    // effects at the lighter oversampling of 2 (R31) where part 1 used 4.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    assert_eq!(digest(&both), 0x6f2a_c131_52e7_acbd);
    let _ = digest(&both);
}

/// Whether the tests run at full depth.
fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Ok("quick") | Err(_) => false,
        Ok(other) => {
            panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full, or leave it unset")
        }
    }
}

// Spec: wavetable R20 (sixteen voices of eight copies on both oscillators)
#[test]
fn sixteen_voices_with_eight_unison_copies_render_inside_the_device_budget() {
    let params = [
        (osc(2, OSC_ON), 1.0),
        (osc(1, EFFECT), choice(&EFFECTS, "Classic")),
        (osc(1, PW), 30.0),
        (osc(1, SYNC), 20.0),
        (osc(2, EFFECT), choice(&EFFECTS, "Modern")),
        (osc(2, FOLD), 30.0),
        (POLY_VOICES, 16.0),
        (UNISON_VOICES, 8.0),
        (UNISON_AMOUNT, 50.0),
        (SUB_ON, 1.0),
        (filter(1, FILTER_ON), 1.0),
        (filter(1, FILTER_FREQ), 4_000.0),
        (filter(2, FILTER_ON), 1.0),
        (filter(2, FILTER_FREQ), 6_000.0),
    ];
    let registry = registry();
    let mut node = node(&registry, &wavetable_with(&params), 128);
    let seconds = 4;
    let chord: Vec<NoteEvent> = (0..16).map(|i| on(0, 36 + i * 4, 0.8)).collect();
    let start = Instant::now();
    let signal = play_blocks(node.as_mut(), seconds * HZ as usize, 128, &chord, &[]);
    let elapsed = start.elapsed().as_secs_f64();
    let realtime = seconds as f64 / elapsed;
    println!("16 voices of 8 copies: {seconds} s in {elapsed:.3} s, {realtime:.1}x real time");
    assert!(rms(&signal[0]) > 0.01, "the chord sounds");
    if full_depth() {
        // A debug build on a loaded machine, as Drift's budget is judged.
        assert!(
            realtime >= 1.0,
            "ran at {realtime:.2}x real time in a debug build"
        );
    }
}

// Spec: wavetable R23, R24 (the table in use follows the data the node is sent)
#[test]
fn a_table_sent_to_the_node_replaces_an_oscillators_table() {
    let registry = registry();
    let mut node = node(&registry, &wavetable_with(&[]), 512);
    let f0 = frequency(45);
    let second = |signal: &[f32]| {
        db(
            amplitude_at(steady(signal), 2.0 * f0),
            amplitude_at(steady(signal), f0),
        )
    };
    let before = play_blocks(node.as_mut(), 24_000, 512, &held(45), &[]).remove(0);
    assert!(
        second(&before) < -50.0,
        "the default table starts as a sine"
    );
    // Saw to Square starts as a saw, which has a second harmonic.
    give(node.as_mut(), 1, "Shape Morphs", "Saw to Square");
    let after = play_blocks(
        node.as_mut(),
        24_000,
        512,
        &[off(0, 45), on(1, 45, 1.0)],
        &[],
    )
    .remove(0);
    assert!(
        second(&after) > -30.0,
        "the new table has a second harmonic: {}",
        second(&after)
    );
}

// Spec: wavetable R3, R8, R10 (the compiler builds the model's table off the
// audio thread and sends it; a changed choice is sent again, with no new node)
#[test]
fn the_compiler_sends_the_tables_the_model_names() {
    use bl_engine::{Compiler, EngineConfig, NodeKey};
    use bl_model::{Command, DeviceId, Editor, TrackKind, WavetableChoice};

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let id: DeviceId = editor.new_id();
    let mut model = wavetable().device(id);
    model.params.insert(filter(1, FILTER_ON), 0.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: model,
        })
        .unwrap();
    let choose = |editor: &mut Editor, name: &str| {
        editor
            .apply(Command::SetDeviceWavetable {
                device: id,
                oscillator: 1,
                choice: WavetableChoice::Factory {
                    category: "Shape Morphs".into(),
                    name: name.into(),
                },
                media: None,
            })
            .unwrap();
    };
    let collector = Collector::new();
    let mut compiler = Compiler::new(EngineConfig::new(RATE, 512), registry(), collector.handle());
    let mut node = node(&registry(), editor.document().device(id).unwrap(), 512);
    let f0 = frequency(45);
    let second = |node: &mut dyn Node| {
        let signal = play_blocks(node, 24_000, 512, &[off(0, 45), on(1, 45, 1.0)], &[]).remove(0);
        db(
            amplitude_at(steady(&signal), 2.0 * f0),
            amplitude_at(steady(&signal), f0),
        )
    };
    let sent = |update: bl_engine::Update| -> Vec<_> {
        update
            .data
            .into_iter()
            .filter(|(key, _)| *key == NodeKey::Device(id))
            .collect()
    };
    // A device with no choice is sent the default on both oscillators.
    assert_eq!(sent(compiler.sync(editor.document())).len(), 2);
    assert!(
        second(node.as_mut()) < -50.0,
        "the default is a sine at Position 0"
    );
    // A choice made later is built and sent as data to the node that is there.
    choose(&mut editor, "Saw to Square");
    let update = sent(compiler.sync(editor.document()));
    assert_eq!(update.len(), 2, "Oscillator 1's table and 2's default");
    for (_, data) in update {
        node.receive_data(data);
    }
    assert!(
        second(node.as_mut()) > -9.0,
        "the node plays a saw: {}",
        second(node.as_mut())
    );
    // A node made after the choice is sent the table with its first compile.
    let mut fresh = Compiler::new(EngineConfig::new(RATE, 512), registry(), collector.handle());
    let first = fresh.sync(editor.document());
    assert_eq!(sent(first).len(), 2, "a new node is sent its tables");
    // Undoing the first choice sends the default back.
    editor.undo().unwrap();
    for (_, data) in sent(compiler.sync(editor.document())) {
        node.receive_data(data);
    }
    assert!(
        second(node.as_mut()) < -50.0,
        "undone, the node plays the default again"
    );
}

// ---- Wavetable, part 2 (WP-8.10; R25 to R34) ----

/// A Wavetable with `params` and `routes` (source, destination, amount in
/// per cent) in its first free slots.
fn routed(params: &[(ParamId, f64)], routes: &[(&str, &str, f64)]) -> bl_model::Device {
    let mut all = params.to_vec();
    for (slot, &(_, _, amount)) in routes.iter().enumerate() {
        all.push((slot_amount(slot), amount));
    }
    let mut device = wavetable_with(&all);
    device.routes = routes
        .iter()
        .enumerate()
        .map(|(slot, &(source, destination, _))| bl_model::ModRoute {
            slot: slot as u8,
            source: source.to_owned(),
            destination: destination.to_owned(),
        })
        .collect();
    device
}

/// Both channels of `notes` played for `frames` in blocks of `block` through
/// `device`.
fn render_device(
    device: &bl_model::Device,
    notes: &[NoteEvent],
    frames: usize,
    block: usize,
) -> Vec<Vec<f32>> {
    let registry = registry();
    let mut node = node(&registry, device, 4_096);
    play_blocks(node.as_mut(), frames, block, notes, &[])
}

fn message(kind: NoteKind) -> NoteEvent {
    NoteEvent {
        offset: 0,
        kind,
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    }
}

fn expression(key: u8, expression: bl_engine::NoteExpression, value: f32) -> NoteEvent {
    NoteEvent {
        key,
        ..message(NoteKind::Expression { expression, value })
    }
}

/// The root mean square of each `window` of `signal`.
fn levels(signal: &[f32], window: usize) -> Vec<f64> {
    signal.chunks(window).map(rms).collect()
}

/// The routes' sources and every destination the manual names.
const SOURCES: [&str; 13] = [
    "Amp",
    "Env 2",
    "Env 3",
    "LFO 1",
    "LFO 2",
    "Velocity",
    "Note",
    "Pitch Bend",
    "Aftertouch",
    "Mod Wheel",
    "Random",
    "Pressure",
    "Slide",
];

// Spec: wavetable R25, R34
#[test]
fn the_matrix_has_every_source_and_destination_the_manual_names_and_64_free_slots() {
    use bl_model::matrix::{Kind, SlotKind};
    let device_type = wavetable();
    let matrix = device_type.matrix.as_ref().expect("Wavetable has a matrix");
    let sources: Vec<&str> = matrix.sources.iter().map(|s| s.name).collect();
    assert_eq!(sources, SOURCES);
    assert_eq!(matrix.slots.len(), SLOTS);
    assert!(matrix.slots.iter().all(|s| s.kind() == SlotKind::Free));

    // Every destination, and nothing else (R34).
    let mut expected: Vec<String> = Vec::new();
    for n in 1..=2 {
        for what in [
            "Position",
            "FM Amount",
            "FM Tune",
            "PW",
            "Sync",
            "Warp",
            "Fold",
            "Gain",
            "Pan",
            "Semi",
            "Detune",
        ] {
            expected.push(format!("Osc {n} {what}"));
        }
    }
    expected.extend(["Sub Gain".into(), "Sub Tone".into()]);
    for n in 1..=2 {
        for what in ["Freq", "Res", "Drive", "Morph"] {
            expected.push(format!("Filter {n} {what}"));
        }
    }
    let times = [
        "Attack",
        "Decay",
        "Sustain",
        "Release",
        "Attack Slope",
        "Decay Slope",
        "Release Slope",
    ];
    for what in times {
        expected.push(format!("Amp {what}"));
    }
    for n in 2..=3 {
        for what in times.into_iter().chain(["Initial", "Peak", "Final"]) {
            expected.push(format!("Env {n} {what}"));
        }
    }
    for n in 1..=2 {
        for what in ["Rate", "Shape", "Amount"] {
            expected.push(format!("LFO {n} {what}"));
        }
    }
    expected.extend(["Unison Amount", "Volume", "Matrix Time", "Matrix Amount"].map(String::from));
    let mut dests: Vec<String> = matrix.dests.iter().map(|d| d.name.to_owned()).collect();
    dests.sort();
    expected.sort();
    assert_eq!(dests, expected);
    for dest in &matrix.dests {
        let param = device_type.description.param(dest.param.unwrap()).unwrap();
        assert_eq!(param.name, dest.name);
    }

    // Which multiply (R25); Offset is no destination (R28).
    let mut multiplicative: Vec<&str> = matrix
        .dests
        .iter()
        .filter(|d| d.kind == Kind::Multiplicative)
        .map(|d| d.name)
        .collect();
    multiplicative.sort_unstable();
    assert_eq!(
        multiplicative,
        [
            "Amp Sustain",
            "Env 2 Initial",
            "Env 3 Initial",
            "LFO 1 Amount",
            "LFO 2 Amount",
            "Matrix Amount",
            "Unison Amount",
            "Volume",
        ]
    );
    assert!(dests.iter().all(|d| !d.contains("Offset")));
}

/// The share a route into Volume left of its level, from the level it gave:
/// a multiplicative route scales the control's travel, which Volume's fader
/// law (instrument modulation R4) turns into the level.
fn share(level: f64) -> f64 {
    let device_type = wavetable();
    let volume = device_type.description.param(VOLUME).unwrap();
    volume.normalize(level) / volume.normalize(1.0)
}

/// The level of a held note's steady state, unrouted, at `key`.
fn full_level(key: u8, params: &[(ParamId, f64)]) -> f64 {
    rms(steady(
        &render_device(&routed(params, &[]), &held(key), 48_000, 512)[0],
    ))
}

// Spec: wavetable R25, R26, R29
#[test]
fn a_route_from_every_source_reaches_its_destination() {
    // Volume at -100 %: the route's factor is 1 - s, s the source's 0 to 1
    // reading, so a source at its top silences the note.
    let key = 72;
    let params = [
        (lfo(1, LFO_OFFSET), 90.0),
        (lfo(1, LFO_RATE), 0.01),
        (lfo(2, LFO_OFFSET), 90.0),
        (lfo(2, LFO_RATE), 0.01),
    ];
    let full = full_level(key, &params);
    for source in SOURCES {
        let mut notes = Vec::new();
        match source {
            "Pitch Bend" => notes.push(message(NoteKind::PitchBend { value: 1.0 })),
            "Aftertouch" => notes.push(message(NoteKind::ChannelPressure { value: 1.0 })),
            "Mod Wheel" => notes.push(message(NoteKind::Control {
                controller: 1,
                value: 1.0,
            })),
            _ => {}
        }
        notes.push(on(0, key, 1.0));
        match source {
            "Pressure" => notes.push(expression(key, bl_engine::NoteExpression::Pressure, 1.0)),
            "Slide" => notes.push(expression(key, bl_engine::NoteExpression::Slide, 1.0)),
            _ => {}
        }
        let device = routed(&params, &[(source, "Volume", -100.0)]);
        let out = render_device(&device, &notes, 48_000, 512);
        let left = share(rms(steady(&out[0])) / full);
        match source {
            // Key 72 is 12 semitones above C3, a tenth of the source's span.
            "Note" => assert!((left - 0.4498).abs() < 0.03, "{source}: {left}"),
            "Random" => assert!(left < 0.999, "{source}: {left}"),
            _ => assert!(left < 0.03, "{source}: {left}"),
        }
    }
}

// Spec: wavetable R25, R29 (a voice's own sources move that voice alone)
#[test]
fn mpe_pressure_and_slide_and_velocity_move_one_note_alone() {
    use bl_engine::NoteExpression::{Pressure, Slide};
    // Into Pan at 100 %: a source at 1 sends the note hard right. Each note
    // has a channel of its own, as MPE gives them.
    let sides = |out: &[Vec<f32>], key: u8| {
        let hz = frequency(key);
        (
            amplitude_at(steady(&out[0]), hz),
            amplitude_at(steady(&out[1]), hz),
        )
    };
    for (source, press) in [
        ("Pressure", Some(Pressure)),
        ("Slide", Some(Slide)),
        ("Velocity", None),
    ] {
        // Slide rests at its middle, which moves a note a quarter of the
        // travel at 50 %.
        let amount = if source == "Pressure" { 100.0 } else { 50.0 };
        let device = routed(&[], &[(source, "Osc 1 Pan", amount)]);
        let other = NoteEvent {
            channel: 1,
            ..on(0, 79, if press.is_some() { 1.0 } else { 0.02 })
        };
        let mut notes = vec![on(0, 72, 1.0), other];
        if let Some(press) = press {
            notes.push(expression(72, press, 1.0));
        }
        let out = render_device(&device, &notes, 48_000, 512);
        let (moved_left, moved_right) = sides(&out, 72);
        let (other_left, other_right) = sides(&out, 79);
        assert!(
            moved_left < moved_right * 0.05,
            "{source}: {moved_left} {moved_right}"
        );
        match source {
            "Pressure" => assert!(
                (other_left / other_right - 1.0).abs() < 0.05,
                "the other note stayed centred ({other_left} {other_right})"
            ),
            // The other note keeps the resting slide or its small velocity:
            // part way right, not hard.
            _ => assert!(
                other_left > other_right * 0.2,
                "{source}: {other_left} {other_right}"
            ),
        }
    }
}

/// The share of its level each 10 ms of `signal` keeps, given Volume's route.
fn shares(signal: &[f32], full: f64) -> Vec<f64> {
    levels(signal, 480)
        .into_iter()
        .map(|level| share(level / full))
        .collect()
}

// Spec: wavetable R27
#[test]
fn an_envelopes_slopes_and_stage_times_follow_the_law() {
    // Env 2 into Volume at +100 %: the route's factor is s = (x + 1) / 2, so
    // Initial -100 % to Peak 100 % reads as the share from 0 to 1.
    let full = full_level(60, &[]);
    let attack = |slope: f64| {
        let params = [
            (envelope(1, ENV_INITIAL), -100.0),
            (envelope(1, ENV_PEAK), 100.0),
            (envelope(1, ENV_FINAL), -100.0),
            (envelope(1, ENV_ATTACK), 200.0),
            (envelope(1, ENV_ATTACK_SLOPE), slope),
        ];
        let device = routed(&params, &[("Env 2", "Volume", 100.0)]);
        shares(&render_device(&device, &held(60), 48_000, 512)[0], full)
    };
    for (slope, expected) in [(0.0, 0.5), (1.0, 0.5f64.powf(0.25)), (-1.0, 0.5f64.powi(4))] {
        let curve = attack(slope);
        // 100 ms is index 10 (10 ms windows): halfway through the 200 ms attack.
        assert!(
            (curve[10] - expected).abs() < 0.08,
            "slope {slope}: {} against {expected}",
            curve[10]
        );
        assert!(curve[25] > 0.97, "slope {slope} reached the peak");
    }
}

// Spec: wavetable R27
#[test]
fn the_loop_modes_hold_run_once_or_cycle() {
    let full = full_level(60, &[]);
    let run = |mode: &str, amp: bool, notes: &[NoteEvent]| {
        let e = usize::from(!amp);
        let mut params = vec![
            (envelope(e, ENV_MODE), choice(&LOOP_MODES, mode)),
            (envelope(e, ENV_ATTACK), 100.0),
            (envelope(e, ENV_DECAY), 0.0),
            (envelope(e, ENV_SUSTAIN), 100.0),
            (envelope(e, ENV_RELEASE), 100.0),
        ];
        if e > 0 {
            params.extend([
                (envelope(e, ENV_INITIAL), -100.0),
                (envelope(e, ENV_PEAK), 100.0),
                (envelope(e, ENV_FINAL), -100.0),
            ]);
        }
        let device = if amp {
            routed(&params, &[])
        } else {
            routed(&params, &[("Env 2", "Volume", 100.0)])
        };
        shares(&render_device(&device, notes, 48_000, 512)[0], full)
    };
    // None holds its peak while the note lasts.
    let none = run("None", false, &held(60));
    assert!(none[40] > 0.95, "None holds Sustain");
    // Trigger runs Attack, Decay and Release once, whatever the note's length:
    // up at 100 ms, back to silence by 250 ms and for good.
    for notes in [held(60), vec![on(0, 60, 1.0), off(2_400, 60)]] {
        let trigger = run("Trigger", false, &notes);
        assert!(trigger[10] > 0.8, "rose: {trigger:?}");
        assert!(trigger[30] < 0.05, "fell");
        assert!(trigger[45] < 0.05, "stayed down");
    }
    // Loop cycles the whole envelope: peaks at 100 and 300 ms, a trough at 200.
    let looped = run("Loop", false, &held(60));
    assert!(looped[10] > 0.8 && looped[30] > 0.8, "{looped:?}");
    assert!(looped[20] < 0.1, "{looped:?}");
    // A note's end sends a looping envelope into Release from where it is.
    let ended = run("Loop", false, &[on(0, 60, 1.0), off(3_600, 60)]);
    assert!(ended[45] < 0.05, "released and stayed released: {ended:?}");
    // The Amp envelope in Trigger ends its voice after one run, the note held.
    let amp = levels(
        &render_device(
            &routed(
                &[
                    (envelope(0, ENV_MODE), choice(&LOOP_MODES, "Trigger")),
                    (envelope(0, ENV_ATTACK), 20.0),
                    (envelope(0, ENV_DECAY), 20.0),
                    (envelope(0, ENV_SUSTAIN), 50.0),
                    (envelope(0, ENV_RELEASE), 50.0),
                ],
                &[],
            ),
            &held(60),
            48_000,
            512,
        )[0],
        480,
    );
    assert!(amp[3] > 0.1 && amp[45] == 0.0, "{amp:?}");
}

/// The share an LFO 1 route into Volume at +100 % gives over each 10 ms, for
/// LFO 1 with `params` and a held note.
fn lfo_shares(params: &[(ParamId, f64)], notes: &[NoteEvent], frames: usize) -> Vec<f64> {
    let full = full_level(60, &[]);
    let device = routed(params, &[("LFO 1", "Volume", 100.0)]);
    shares(&render_device(&device, notes, frames, 512)[0], full)
}

/// How many times `shares` rises through one half.
fn rises(shares: &[f64]) -> usize {
    shares
        .windows(2)
        .filter(|w| w[0] < 0.5 && w[1] >= 0.5)
        .count()
}

// Spec: wavetable R28
#[test]
fn each_lfo_wave_runs_at_its_rate_and_amount_with_its_shape() {
    let wave = |name: &str| (lfo(1, LFO_WAVE), choice(&LFO_WAVES, name));
    // Five hertz over two seconds: ten cycles of every wave but Random.
    for name in ["Sine", "Saw", "Triangle", "Square"] {
        let curve = lfo_shares(&[wave(name), (lfo(1, LFO_RATE), 5.0)], &held(60), 96_000);
        assert!(
            (9..=10).contains(&rises(&curve)),
            "{name}: {}",
            rises(&curve)
        );
    }
    // Amount scales the swing: a sine at 50 % stays between a quarter and
    // three quarters.
    let half = lfo_shares(
        &[
            wave("Sine"),
            (lfo(1, LFO_RATE), 5.0),
            (lfo(1, LFO_AMOUNT), 50.0),
        ],
        &held(60),
        48_000,
    );
    let (low, high) = half
        .iter()
        .fold((1.0f64, 0.0f64), |(l, h), &x| (l.min(x), h.max(x)));
    assert!(
        (low - 0.25).abs() < 0.05 && (high - 0.75).abs() < 0.05,
        "{low} {high}"
    );
    // A square's Shape is its pulse width: 0.5 + 0.45 × Shape high.
    for (shape, duty) in [(-100.0, 0.05), (0.0, 0.5), (100.0, 0.95)] {
        let curve = lfo_shares(
            &[
                wave("Square"),
                (lfo(1, LFO_RATE), 2.0),
                (lfo(1, LFO_SHAPE), shape),
            ],
            &held(60),
            96_000,
        );
        let high = curve.iter().filter(|&&x| x > 0.5).count() as f64 / curve.len() as f64;
        assert!(
            (high - duty).abs() < 0.05,
            "shape {shape}: {high} against {duty}"
        );
    }
    // A saw's Shape bends its ramp: the middle of a cycle sits above half
    // with a positive Shape.
    let bend = |shape: f64| {
        let curve = lfo_shares(
            &[
                wave("Saw"),
                (lfo(1, LFO_RATE), 1.0),
                (lfo(1, LFO_SHAPE), shape),
            ],
            &held(60),
            48_000,
        );
        curve[50]
    };
    assert!(bend(100.0) > bend(0.0) + 0.1 && bend(-100.0) < bend(0.0) - 0.1);
    // Random holds a value for a cycle and draws another for the next.
    let random = lfo_shares(
        &[wave("Random"), (lfo(1, LFO_RATE), 2.0)],
        &held(60),
        96_000,
    );
    let cycle = |n: usize| &random[n * 50 + 5..n * 50 + 45];
    for n in 0..3 {
        let values = cycle(n);
        let spread = values
            .iter()
            .fold(0.0f64, |m, &x| m.max((x - values[0]).abs()));
        assert!(spread < 0.02, "cycle {n} holds: {spread}");
    }
    assert!((cycle(0)[0] - cycle(1)[0]).abs() > 0.01 || (cycle(1)[0] - cycle(2)[0]).abs() > 0.01);
}

// Spec: wavetable R28
#[test]
fn an_lfo_starts_at_offset_and_fades_in_over_its_attack() {
    let sine = [(lfo(1, LFO_WAVE), 0.0), (lfo(1, LFO_RATE), 0.01)];
    let with = |extra: &[(ParamId, f64)]| {
        let mut params = sine.to_vec();
        params.extend_from_slice(extra);
        lfo_shares(&params, &held(60), 48_000)
    };
    // Retrigger on: each note starts at Offset, a quarter cycle on a peak.
    let top = with(&[(lfo(1, LFO_OFFSET), 90.0), (lfo(1, LFO_RETRIGGER), 1.0)]);
    let bottom = with(&[(lfo(1, LFO_OFFSET), 270.0), (lfo(1, LFO_RETRIGGER), 1.0)]);
    assert!(
        top[2] > 0.95 && bottom[2] < 0.05,
        "{} {}",
        top[2],
        bottom[2]
    );
    // Attack fades the swing in from the note's start: a share of
    // (1 + t / Attack) / 2 at the top.
    let faded = with(&[(lfo(1, LFO_OFFSET), 90.0), (lfo(1, LFO_ATTACK), 400.0)]);
    assert!((faded[10] - 0.625).abs() < 0.04, "{}", faded[10]);
    assert!(faded[45] > 0.97, "{}", faded[45]);
}

/// Both channels of `notes` for `frames` in blocks of `block`, the song
/// playing from `start` at `tempo`.
fn song(
    device: &bl_model::Device,
    notes: &[NoteEvent],
    frames: usize,
    block: usize,
    tempo: &TempoMap,
    start: i64,
) -> Vec<f32> {
    let registry = registry();
    let mut node = node(&registry, device, 4_096);
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

// Spec: wavetable R28 (the engine's transport, a tempo change, a restart)
#[test]
fn an_lfo_without_retrigger_follows_the_song_across_a_tempo_change_and_restarts() {
    let constant = |bpm| TempoMap::constant(Bpm::new(bpm).unwrap());
    let changing = TempoMap::new([
        TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: Bpm::new(120.0).unwrap(),
        },
        TempoSegment::Constant {
            start: Ticks::from_quarters(4),
            bpm: Bpm::new(90.0).unwrap(),
        },
    ])
    .unwrap();
    // A synced saw, a beat to the cycle, into Volume at +100 %: the share
    // is the cycle's phase, 0 on the beat and a half half a beat on.
    let params = [
        (lfo(1, LFO_WAVE), 1.0),
        (lfo(1, LFO_SYNC), 1.0),
        (
            lfo(1, LFO_DIVISION),
            choice(&DIVISIONS.map(|(n, _)| n), "1/4"),
        ),
    ];
    let device = routed(&params, &[("LFO 1", "Volume", 100.0)]);
    let full = full_level(60, &[]);
    for tempo in [constant(120.0), constant(75.0), changing] {
        let frames = 8 * 48_000;
        let reference = song(&device, &held(60), frames, 4_096, &tempo, 0);
        let share_at =
            |signal: &[f32], from: usize| share(rms(&signal[from + 24..from + 264]) / full);
        for beat in 1..7 {
            let at = tempo.ticks_to_frames(Ticks::from_quarters(beat), RATE).0 as usize;
            if at + 20_000 > frames {
                continue;
            }
            assert!(
                share_at(&reference, at) < 0.03,
                "beat {beat}: the cycle is at {}",
                share_at(&reference, at)
            );
            let half = tempo
                .ticks_to_frames(Ticks(Ticks::from_quarters(beat).0 + 352_800), RATE)
                .0 as usize;
            let phase = share_at(&reference, half);
            assert!((phase - 0.5).abs() < 0.03, "half a beat on: {phase}");
        }
        // The song restarted from two beats in sounds the same.
        let two = tempo.ticks_to_frames(Ticks::from_quarters(2), RATE).0;
        let again = song(&device, &held(60), 2 * 48_000, 4_096, &tempo, two);
        for k in 1..8 {
            let from = k * 9_600;
            let (a, b) = (
                share_at(&reference, two as usize + from),
                share_at(&again, from),
            );
            assert!((a - b).abs() < 0.02, "{k}: {a} against {b}");
        }
    }
    // The same at every block size, to the bit.
    let tempo = constant(120.0);
    let reference = song(&device, &held(60), 48_000, 4_096, &tempo, 0);
    for block in BLOCKS {
        assert_eq!(
            song(&device, &held(60), 48_000, block, &tempo, 0),
            reference,
            "block {block}"
        );
    }
    // A note that starts half a beat in meets the song's phase with Retrigger
    // off, and starts at Offset with it on.
    let late = [on(12_000, 60, 1.0)];
    let follow = song(&device, &late, 36_000, 4_096, &tempo, 0);
    let met = share(rms(&follow[12_048..12_288]) / full);
    assert!((met - 0.5).abs() < 0.05, "followed: {met}");
    let retrigger = routed(
        &[params.to_vec(), vec![(lfo(1, LFO_RETRIGGER), 1.0)]].concat(),
        &[("LFO 1", "Volume", 100.0)],
    );
    let own = song(&retrigger, &late, 36_000, 4_096, &tempo, 0);
    assert!(share(rms(&own[12_048..12_288]) / full) < 0.05);
}

/// A held note through four copies at Amount 100 % in unison mode `mode`.
fn unison_device(mode: &str) -> bl_model::Device {
    routed(
        &[
            (UNISON_MODE, choice(&UNISON_MODES, mode)),
            (UNISON_VOICES, 4.0),
            (UNISON_AMOUNT, 100.0),
        ],
        &[],
    )
}

/// The digests of each unison mode's render of key 72 held for 0.6 s; they
/// change only when a law does.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const UNISON_GOLDEN: [u64; 7] = [
    3_966_585_481_234_137_731,
    16_529_990_399_356_464_618,
    1_668_533_000_297_481_902,
    9_967_932_885_665_470_972,
    11_465_586_728_367_113_971,
    10_127_496_319_436_159_777,
    15_041_902_144_172_882_308,
];

// Spec: wavetable R30, R33
#[test]
fn each_unison_mode_renders_the_same_run_after_run_and_at_every_block_size() {
    let mut digests = Vec::new();
    for mode in UNISON_MODES {
        let device = unison_device(mode);
        let reference = render_device(&device, &held(72), 28_800, 512)[0].clone();
        assert_eq!(
            render_device(&device, &held(72), 28_800, 512)[0],
            reference,
            "{mode} again"
        );
        for block in BLOCKS {
            assert_eq!(
                render_device(&device, &held(72), 28_800, block)[0],
                reference,
                "{mode} at block {block}"
            );
        }
        digests.push(digest(&reference));
    }
    // A vector unit of another architecture rounds some samples differently,
    // so only the one the digests were taken on compares them.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    assert_eq!(digests, UNISON_GOLDEN, "{digests:?}");
    let _ = digests;
}

// Spec: wavetable R30
#[test]
fn each_unison_mode_spreads_its_copies_as_its_law_says() {
    let hz = frequency(96);
    let cents = |c: f64| hz * 2f64.powf(c / 1200.0);
    // The copies pan alternately, so each channel holds half of them: the
    // mix has all.
    let one = |mode: &str| {
        let out = render_device(&unison_device(mode), &held(96), 48_000, 512);
        out[0]
            .iter()
            .zip(&out[1])
            .map(|(l, r)| l + r)
            .collect::<Vec<f32>>()
    };
    let at = |signal: &[f32], c: f64| amplitude_at(steady(signal), cents(c));

    // None plays one copy, whatever Amount says.
    let none = one("None");
    let single = render_device(
        &routed(&[(UNISON_MODE, 1.0), (UNISON_AMOUNT, 0.0)], &[]),
        &held(96),
        48_000,
        512,
    );
    let single: Vec<f32> = single[0]
        .iter()
        .zip(&single[1])
        .map(|(l, r)| l + r)
        .collect();
    assert!(
        at(&none, 0.0) > 0.5 * at(&single, 0.0),
        "one copy at the note's pitch"
    );
    assert!(at(&none, 25.0) < 0.05 * at(&none, 0.0));

    // Classic: four copies at ±25 and ±8.3 cents; the places between them hold less.
    let classic = one("Classic");
    for edge in [-25.0, 25.0] {
        assert!(
            at(&classic, edge) > 3.0 * at(&classic, edge * 0.64),
            "Classic at {edge}"
        );
    }
    assert!(at(&classic, 0.0) < 0.3 * at(&classic, 25.0));

    // Phase Sync keeps Classic's detune with the copies starting together, so a
    // note's first 2 ms are louder than Classic's random phases give.
    let sync = one("Phase Sync");
    assert!(
        at(&sync, 25.0) > 3.0 * at(&sync, 16.0),
        "Phase Sync keeps the detune"
    );
    assert!(
        rms(&sync[..96]) > 1.2 * rms(&classic[..96]),
        "{} {}",
        rms(&sync[..96]),
        rms(&classic[..96])
    );

    // Position Spread detunes by up to ±5 cents, far less than Classic.
    let spread = one("Position Spread");
    assert!(at(&spread, 25.0) < 0.15 * at(&classic, 25.0));
    assert_ne!(digest(&spread), digest(&classic));

    // Shimmer and Noise stay within ±12 cents of the note.
    for mode in ["Shimmer", "Noise"] {
        let jitter = one(mode);
        let peak = peak_frequency(steady(&jitter), hz * 0.97, hz * 1.03);
        assert!((peak / hz - 1.0).abs() < 0.008, "{mode}: {peak}");
        assert_ne!(digest(&jitter), digest(&classic), "{mode}");
    }
    assert_ne!(digest(&one("Shimmer")), digest(&one("Noise")));

    // Random Note draws each note's copies again: the same key twice sounds
    // different, and a render still repeats.
    let device = unison_device("Random Note");
    let notes = [
        on(0, 72, 1.0),
        off(14_400, 72),
        on(48_000, 72, 1.0),
        off(62_400, 72),
    ];
    let out = render_device(&device, &notes, 96_000, 512)[0].clone();
    assert_ne!(digest(&out[2_000..14_000]), digest(&out[50_000..62_000]));
    assert_eq!(render_device(&device, &notes, 96_000, 512)[0], out);
}

// Spec: wavetable R31, instrument voices R19, R20
#[test]
fn hi_quality_is_a_menu_option_that_works_control_rate_routes_every_frame() {
    let device_type = wavetable();
    assert_eq!(device_type.description.menu_options, [HI_QUALITY]);
    assert_eq!(
        device_type.description.param(HI_QUALITY).unwrap().default,
        0.0
    );
    // LFO 1 into Filter 1's Frequency, a control-rate route: with Hi-Quality
    // off it moves on a grid of 32 frames, on it moves every frame.
    let render = |hi: f64, block: usize| {
        let device = routed(
            &[
                (HI_QUALITY, hi),
                (filter(1, FILTER_ON), 1.0),
                (filter(1, FILTER_FREQ), 800.0),
                (lfo(1, LFO_RATE), 40.0),
            ],
            &[("LFO 1", "Filter 1 Freq", 60.0)],
        );
        render_device(&device, &held(48), 9_600, block)[0].clone()
    };
    let (off, on) = (render(0.0, 512), render(1.0, 512));
    assert_ne!(
        off, on,
        "Hi-Quality changes how often the route is worked out"
    );
    for block in BLOCKS {
        assert_eq!(render(0.0, block), off, "off at block {block}");
        assert_eq!(render(1.0, block), on, "on at block {block}");
    }
}

// Spec: wavetable R25, R26, R33
#[test]
fn a_patch_with_routes_renders_the_same_at_every_block_size() {
    let device = routed(
        &[
            (osc(1, EFFECT), 3.0),
            (osc(1, WARP), 30.0),
            (osc(2, OSC_ON), 1.0),
            (osc(2, SEMI), 7.0),
            (SUB_ON, 1.0),
            (filter(1, FILTER_ON), 1.0),
            (filter(1, FILTER_FREQ), 1_500.0),
            (lfo(1, LFO_RATE), 7.0),
            (lfo(2, LFO_WAVE), 4.0),
            (lfo(2, LFO_RATE), 3.0),
            (envelope(1, ENV_ATTACK), 80.0),
            (envelope(1, ENV_DECAY), 300.0),
            (envelope(1, ENV_SUSTAIN), 40.0),
            (UNISON_MODE, choice(&UNISON_MODES, "Shimmer")),
            (UNISON_AMOUNT, 60.0),
            (MATRIX_TIME, 25.0),
        ],
        &[
            ("LFO 1", "Osc 1 Position", 35.0),
            ("Env 2", "Filter 1 Freq", 50.0),
            ("LFO 2", "Osc 2 Pan", 80.0),
            ("Velocity", "Osc 2 Gain", -30.0),
            ("Note", "Filter 1 Res", 40.0),
            ("LFO 1", "Osc 1 Detune", 20.0),
            ("Env 3", "Sub Tone", 70.0),
            ("Mod Wheel", "Unison Amount", 100.0),
            ("Amp", "Env 2 Release", 20.0),
            ("LFO 1", "LFO 2 Rate", 30.0),
            ("Random", "Matrix Time", 10.0),
        ],
    );
    let notes = [
        message(NoteKind::Control {
            controller: 1,
            value: 0.7,
        }),
        on(0, 48, 0.9),
        on(2_000, 60, 0.6),
        on(7_000, 67, 1.0),
        off(15_000, 60),
        off(24_000, 48),
        off(30_000, 67),
    ];
    let reference = render_device(&device, &notes, 48_000, 512)[0].clone();
    assert!(rms(&reference) > 0.01, "the patch sounds");
    for block in BLOCKS {
        assert_eq!(
            render_device(&device, &notes, 48_000, block)[0],
            reference,
            "block {block}"
        );
    }
    // As the unison digests: compared where they were taken.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    assert_eq!(
        digest(&reference),
        1_672_112_042_196_658_321,
        "{}",
        digest(&reference)
    );
}

/// The energy of `signal` away from the harmonics of `hz`, in decibels over
/// the energy at them.
fn fold_back_db(signal: &[f32], hz: f64) -> f64 {
    let n = 16_384.min(signal.len());
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    for (i, slot) in input.iter_mut().enumerate() {
        let w = 0.5 - 0.5 * (core::f64::consts::TAU * i as f64 / n as f64).cos();
        *slot = f64::from(signal[i]) * w;
    }
    fft.process(&mut input, &mut spectrum).unwrap();
    let bin = HZ / n as f64;
    let (mut away, mut at) = (0.0, 0.0);
    for (k, value) in spectrum.iter().enumerate() {
        let f = k as f64 * bin;
        let harmonic = (f / hz).round();
        if harmonic >= 1.0 && (f - harmonic * hz).abs() < 6.0 * bin {
            at += value.norm_sqr();
        } else {
            away += value.norm_sqr();
        }
    }
    10.0 * (away / at).log10()
}

// Spec: wavetable R31 (Hi-Quality renders the effects at the finest oversampling)
#[test]
fn hi_quality_lowers_the_fold_back_of_a_folded_table_at_high_pitches_by_12_db() {
    for key in [96, 108, 120, 127] {
        let render = |hi: f64| {
            let device = routed(
                &[
                    (HI_QUALITY, hi),
                    (osc(1, EFFECT), choice(&EFFECTS, "Modern")),
                    (osc(1, FOLD), 100.0),
                    (osc(1, POSITION), 100.0),
                ],
                &[],
            );
            render_device(&device, &held(key), 48_000, 512)[0].clone()
        };
        let (off, on) = (
            fold_back_db(steady(&render(0.0)), frequency(key)),
            fold_back_db(steady(&render(1.0)), frequency(key)),
        );
        assert!(on < off - 12.0, "key {key}: {off:.1} dB off, {on:.1} dB on");
    }
}

// Spec: wavetable R26 (Time, Amount, and the Note source on a filter's Frequency)
#[test]
fn the_matrixs_time_and_amount_act_and_note_makes_a_filter_follow_the_key() {
    // Time scales every modulator's rate by 2^(-2t): at -50 % a 5 Hz LFO runs
    // at 10, at +50 % at 2.5.
    let sine = [(lfo(1, LFO_WAVE), 0.0), (lfo(1, LFO_RATE), 5.0)];
    let rises_with = |time: f64| {
        let mut params = sine.to_vec();
        params.push((MATRIX_TIME, time));
        rises(&lfo_shares(&params, &held(60), 96_000))
    };
    assert!(
        (19..=20).contains(&rises_with(-50.0)),
        "{}",
        rises_with(-50.0)
    );
    assert!((9..=10).contains(&rises_with(0.0)), "{}", rises_with(0.0));
    assert!((4..=5).contains(&rises_with(50.0)), "{}", rises_with(50.0));

    // Amount scales the route: at 50 % an LFO into Volume at +100 % keeps the
    // share between a half and one.
    let mut params = sine.to_vec();
    params.push((bl_devices::wavetable::MATRIX_AMOUNT, 50.0));
    let curve = lfo_shares(&params, &held(60), 48_000);
    let low = curve.iter().fold(1.0f64, |m, &x| m.min(x));
    assert!((low - 0.5).abs() < 0.05, "{low}");

    // Note into Filter 1's Frequency at 100 %: the cutoff moves an octave per
    // octave of key from C3, so the filter follows the note exactly.
    let device = routed(
        &[
            (filter(1, FILTER_ON), 1.0),
            (filter(1, FILTER_FREQ), 1_000.0),
        ],
        &[("Note", "Filter 1 Freq", 100.0)],
    );
    let registry = registry();
    for (key, octaves) in [(60u8, 0.0f64), (72, 1.0), (84, 2.0), (48, -1.0)] {
        let mut node = node(&registry, &device, 4_096);
        play_blocks(node.as_mut(), 4_096, 512, &held(key), &[]);
        let mut played = None;
        node.matrix_played(&mut |id, _base, value| {
            if id == filter(1, FILTER_FREQ) {
                played = Some(value);
            }
        });
        let played = played.expect("the filter's Frequency was moved");
        let expected = 1_000.0 * 2f64.powf(octaves);
        assert!(
            (played / expected - 1.0).abs() < 0.02,
            "key {key}: {played} against {expected}"
        );
    }
}
