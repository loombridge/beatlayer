//! Wavetable, part 1 (WP-8.9; the wavetable spec, R11 to R24): its
//! oscillators, their effects, the sub oscillator, the filters and their
//! routing, the amplitude envelope, the voices and unison's amount.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common;
use std::time::Instant;

use bl_devices::wavetable::{
    AMP_ATTACK, AMP_RELEASE, AMP_SUSTAIN, EFFECT, EFFECTS, FILTER_FREQ, FILTER_ON, FILTER_TYPE,
    FM_AMOUNT, FM_TUNE, FOLD, GAIN, GLIDE, OSC_ON, PAGES, POLY_VOICES, POSITION, PW, ROUTING, SEMI,
    SUB_GAIN, SUB_OCTAVE, SUB_ON, SUB_TONE, SYNC, TABS, TRANSPOSE, UNISON_AMOUNT, UNISON_VOICES,
    VOICE_MODE, VOLUME, WARP, WAVETABLE, filter, osc,
};
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

// Spec: wavetable R20 (the same at any block size)
#[test]
fn a_phrase_renders_the_same_at_block_sizes_1_64_512_and_4096() {
    let phrase = [
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
    let registry = registry();
    let at = |block: usize| {
        let mut node = node(&registry, &wavetable_with(&params), 4_096);
        play_blocks(node.as_mut(), 40_000, block, &phrase, &[])
    };
    let reference = at(4_096);
    assert!(rms(&reference[0]) > 0.01, "the phrase sounds");
    for block in BLOCKS {
        let signal = at(block);
        for (channel, want) in signal.iter().zip(&reference) {
            let worst = channel
                .iter()
                .zip(want)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 1e-3, "block size {block}: {worst}");
        }
    }
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
    // A device with no choice sends nothing and plays its default.
    let first = compiler.sync(editor.document());
    assert!(
        first
            .data
            .iter()
            .all(|(key, _)| *key != NodeKey::Device(id))
    );
    assert!(
        second(node.as_mut()) < -50.0,
        "the default is a sine at Position 0"
    );
    // A choice made later is built and sent as data to the node that is there.
    choose(&mut editor, "Saw to Square");
    let update = compiler.sync(editor.document());
    let sent: Vec<_> = update
        .data
        .into_iter()
        .filter(|(key, _)| *key == NodeKey::Device(id))
        .collect();
    assert_eq!(sent.len(), 1, "one table, for Oscillator 1");
    for (_, data) in sent {
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
    assert_eq!(
        first
            .data
            .iter()
            .filter(|(key, _)| *key == NodeKey::Device(id))
            .count(),
        1,
        "a new node is sent its table"
    );
}
