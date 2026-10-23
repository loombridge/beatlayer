//! Native instruments under a tuning (WP-6.9; the tunings spec, R12, R13
//! and R18; the instrument specs' tuning rules): Drift, Simpler, Sampler and
//! Drum Sampler play each key at the pitch the tuning gives it, within a
//! tenth of a cent, the same at any block size; a tuning that changes
//! reaches the next notes and leaves sounding ones where they are; Impulse
//! doesn't follow it. Every block runs as a real-time section under the
//! allocation trap.

use crate::common;
use std::sync::Arc;

use bl_devices::multisampler::multisampler_type;
use bl_devices::polysynth::{
    FILTER, FREQ, HP, KEY, NOISE, NOTE_PB, OSC1_WAVE, OSC2, PB_RANGE, POLYSYNTH, RES, RETRIGGER,
    VOLUME,
};
use bl_devices::sampler::{LOOP, sampler_type};
use bl_engine::{
    Block, Collector, DeviceLayers, KeyPitches, LayerAudio, LayerSample, Node, NoteBuffer,
    NoteEvent, NoteExpression, NoteKind, Registry, SlotSample, TransportInfo,
};
use bl_model::{ContentHash, DeviceKind, Layer, LayerId, SustainMode, Tuning, pitch_of};
use common::{CHANNELS, RATE, device, node};

const HZ: f64 = 48_000.0;

/// The block sizes every render is checked at.
const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

/// C3 of twelve equal steps at A3 = 440 Hz, the pitch a sample recorded at
/// key 60 sounds at.
fn c3() -> f64 {
    440.0 * 2f64.powf(-9.0 / 12.0)
}

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

/// 19 equal steps to the octave, with the default reference and range.
fn nineteen() -> Tuning {
    let cents = (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect();
    Tuning::new("19 equal".into(), String::new(), cents).unwrap()
}

/// A note for [`Rig::render`]: the frame, what it does, its key.
type Played = (usize, NoteKind, u8);

fn on(frame: usize, key: u8) -> Played {
    (frame, NoteKind::On, key)
}

fn bend(frame: usize, key: u8, value: f32) -> Played {
    (
        frame,
        NoteKind::Expression {
            expression: NoteExpression::Bend,
            value,
        },
        key,
    )
}

/// A node, the collector that wraps its data, and the block size it runs at.
struct Rig {
    node: Box<dyn Node>,
    collector: Collector,
    block: usize,
    frame: usize,
}

impl Rig {
    fn new(node: Box<dyn Node>, block: usize) -> Self {
        Self {
            node,
            collector: Collector::new(),
            block,
            frame: 0,
        }
    }

    fn tune(&mut self, tuning: Option<&Tuning>) {
        self.node
            .receive_data(self.collector.handle().data(KeyPitches::of(tuning)));
    }

    /// Plays `frames` frames from where the last call stopped, `notes` at
    /// their frames from this call's start, and returns the left channel.
    fn render(&mut self, frames: usize, notes: &[Played]) -> Vec<f32> {
        self.render_with(frames, notes, &[])
    }

    /// As [`render`](Self::render), with a tuning sent at the start of the
    /// block each of `tunings` falls in.
    fn render_with(
        &mut self,
        frames: usize,
        notes: &[Played],
        tunings: &[(usize, Option<Tuning>)],
    ) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames);
        let mut start = 0;
        while start < frames {
            let len = self.block.min(frames - start);
            for (at, tuning) in tunings {
                if *at >= start && *at < start + len {
                    self.tune(tuning.as_ref());
                }
            }
            let block_notes: Vec<NoteEvent> = notes
                .iter()
                .filter(|n| n.0 >= start && n.0 < start + len)
                .map(|&(at, kind, key)| NoteEvent {
                    offset: (at - start) as u32,
                    kind,
                    channel: 0,
                    key,
                    velocity: 1.0,
                    note_id: None,
                })
                .collect();
            let mut data = vec![0.0f32; CHANNELS * len];
            let mut sink = NoteBuffer::with_capacity(block_notes.len().max(1));
            {
                let mut block = Block::new(
                    &mut data,
                    CHANNELS,
                    len,
                    len,
                    &[],
                    TransportInfo::default(),
                    RATE,
                )
                .with_notes(&block_notes, &mut sink);
                bl_rt::rt_test!({ self.node.process(&mut block) });
            }
            out.extend_from_slice(&data[..len]);
            start += len;
        }
        self.frame += frames;
        out
    }
}

/// Drift playing a sine from Oscillator 1 alone.
fn drift(block: usize) -> Rig {
    let device_type = bl_devices::device_types()
        .into_iter()
        .find(|t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == POLYSYNTH))
        .expect("Drift is registered");
    let params = [
        (OSC1_WAVE, 0.0),
        (OSC2, 0.0),
        (NOISE, 0.0),
        (FILTER, 0.0),
        (FREQ, 20_000.0),
        (HP, 20.0),
        (RES, 0.0),
        (KEY, 0.0),
        (RETRIGGER, 1.0),
        (VOLUME, 1.0),
        (NOTE_PB, 1.0),
        (PB_RANGE, 12.0),
    ];
    Rig::new(
        node(&registry(), &device(&device_type, &params), 4096),
        block,
    )
}

/// A sine of `hz`, `frames` long, one channel.
fn sine(frames: usize, hz: f64) -> Arc<bl_media::SampleData> {
    let data = (0..frames)
        .map(|i| (core::f64::consts::TAU * hz * i as f64 / HZ).sin() as f32)
        .collect();
    Arc::new(bl_media::SampleData::new(RATE.hz(), vec![data]).unwrap())
}

/// Simpler holding a C3 sine, in Classic mode.
fn simpler(block: usize) -> Rig {
    let mut rig = Rig::new(
        node(&registry(), &device(&sampler_type(), &[(LOOP, 1.0)]), 4096),
        block,
    );
    let data = rig
        .collector
        .handle()
        .data(SlotSample::new(0, sine(96_000, c3())));
    rig.node.receive_data(data);
    rig
}

/// Sampler holding a looping C3 sine, rooted on key 60.
fn sampler(block: usize) -> Rig {
    let mut rig = Rig::new(
        node(&registry(), &device(&multisampler_type(), &[]), 4096),
        block,
    );
    let sine = sine(4800, LOOPED_HZ);
    let mut layer = Layer::new(LayerId(1), "Sine", ContentHash([1; 32]), 4800, RATE.hz(), 1);
    layer.sustain = SustainMode::Loop;
    let layers = DeviceLayers::new(
        vec![LayerAudio {
            layer,
            sample: Some(LayerSample::Whole(sine)),
        }],
        false,
        false,
    );
    rig.node.receive_data(rig.collector.handle().data(layers));
    rig
}

/// Drum Sampler holding a C3 sine.
fn strike(block: usize) -> Rig {
    let mut rig = Rig::new(
        node(
            &registry(),
            &device(&bl_devices::drum_sampler::drum_sampler_type(), &[]),
            4096,
        ),
        block,
    );
    let data = rig
        .collector
        .handle()
        .data(SlotSample::new(0, sine(96_000, c3())));
    rig.node.receive_data(data);
    rig
}

/// An instrument under test: its name, how it makes a rig, and the frequency
/// it plays at key 60 with no tuning.
type Instrument = (&'static str, fn(usize) -> Rig, f64);

/// The instruments, by name, with the frequency each plays at key 60 when
/// the set has no tuning: Drift's and the samplers' C3, and the looping
/// Sampler's, whose sine is a whole number of cycles so its loop is clean.
fn instruments() -> [Instrument; 4] {
    [
        ("Drift", drift, c3()),
        ("Simpler", simpler, c3()),
        ("Sampler", sampler, LOOPED_HZ),
        ("Drum Sampler", strike, c3()),
    ]
}

/// The frequency of the Sampler's looping sine: 26 cycles in 4,800 frames.
const LOOPED_HZ: f64 = 260.0;

/// The frequency an instrument whose key 60 plays `base` Hz plays when the
/// tuning gives the key `hz`.
fn expected(base: f64, hz: f64) -> f64 {
    base * hz / c3()
}

/// The frequency of the sine in `signal[from..to]`, from the rising zero
/// crossings, which are placed between samples by linear interpolation.
fn measure(signal: &[f32], from: usize, to: usize) -> f64 {
    let mut first = None;
    let mut last = 0.0;
    let mut crossings = 0usize;
    for i in from..to - 1 {
        let (a, b) = (f64::from(signal[i]), f64::from(signal[i + 1]));
        if a <= 0.0 && b > 0.0 {
            let at = i as f64 + (-a) / (b - a);
            first.get_or_insert(at);
            last = at;
            crossings += 1;
        }
    }
    let first = first.expect("the signal crosses zero");
    (crossings - 1) as f64 * HZ / (last - first)
}

fn cents(measured: f64, wanted: f64) -> f64 {
    1200.0 * (measured / wanted).log2()
}

/// Plays one key under `tuning` and returns the signal, the key at frame 0.
fn play(make: fn(usize) -> Rig, block: usize, tuning: Option<&Tuning>, key: u8) -> Vec<f32> {
    let mut rig = make(block);
    rig.tune(tuning);
    rig.render(24_000, &[on(0, key)])
}

// Spec: tunings R12; polysynth R24; sampler R21; multisampler R74;
// one-shot drum sampler R25. Each native instrument plays keys 60, 61 and
// 79 under a 19-step tuning at the frequency R6 gives them, within a tenth
// of a cent, whatever the block size.
#[test]
fn keys_play_at_the_tunings_frequencies_at_any_block_size() {
    let tuning = nineteen();
    for (name, make, base) in instruments() {
        for key in [60u8, 61, 79] {
            let wanted = expected(base, tuning.key_frequency(key).unwrap());
            let mut first: Option<Vec<f32>> = None;
            for block in BLOCKS {
                let signal = play(make, block, Some(&tuning), key);
                let measured = measure(&signal, 6_000, 22_000);
                let off = cents(measured, wanted);
                assert!(
                    off.abs() < 0.1,
                    "{name} key {key} block {block}: {measured:.4} Hz, {off:+.4} cents"
                );
                match &first {
                    None => first = Some(signal),
                    Some(first) => {
                        let worst = first
                            .iter()
                            .zip(&signal)
                            .map(|(a, b)| (a - b).abs())
                            .fold(0.0, f32::max);
                        assert!(worst < 2e-4, "{name} key {key}: blocks differ by {worst}");
                    }
                }
            }
        }
    }
}

// Spec: tunings R1, R18. With no tuning, or a tuning removed, a key plays
// its twelve-step pitch.
#[test]
fn without_a_tuning_keys_play_as_before() {
    for (name, make, base) in instruments() {
        let signal = play(make, 512, None, 69);
        let measured = measure(&signal, 6_000, 22_000);
        let wanted = expected(base, 440.0);
        assert!(cents(measured, wanted).abs() < 0.1, "{name}: {measured}");
    }
}

// Spec: tunings R8. Keys above the range play nothing.
#[test]
fn keys_above_the_range_are_silent() {
    let tuning = nineteen();
    let short = tuning
        .with_range(tuning.key_note(0), tuning.key_note(64))
        .unwrap();
    for (name, make, _) in instruments() {
        let inside = play(make, 512, Some(&short), 64);
        let outside = play(make, 512, Some(&short), 65);
        assert!(inside.iter().any(|s| s.abs() > 0.1), "{name} inside");
        assert!(outside.iter().all(|s| *s == 0.0), "{name} outside");
    }
}

// Spec: tunings R18. A reference pitch that changes reaches the notes that
// start after it, on their frames, and a note already sounding keeps its
// pitch. A tuning removed does the same.
#[test]
fn a_changed_tuning_reaches_the_next_notes_and_leaves_sounding_ones() {
    let tuning = nineteen();
    let higher = tuning.with_reference(None, None, Some(271.626)).unwrap();
    for (name, make, base) in instruments() {
        for block in [64, 4096] {
            for (after, label) in [(Some(higher.clone()), "reference"), (None, "removed")] {
                let mut rig = make(block);
                rig.tune(Some(&tuning));
                // Key 60 starts at frame 0 and sounds throughout; key 61
                // starts after the change, at frame 24,000.
                let first = rig.render_with(
                    48_000,
                    &[on(0, 60), on(24_000, 61)],
                    &[(20_000, after.clone())],
                );
                let held_before = measure(&first, 6_000, 18_000);
                let wanted_60 = expected(base, tuning.key_frequency(60).unwrap());
                // Two voices sound after 24,000: only the first is
                // measured, over the stretch before the second starts.
                assert!(
                    cents(held_before, wanted_60).abs() < 0.1,
                    "{name} {label}: key 60 at {held_before}"
                );
                // The new note: alone in a fresh rig it plays at the new
                // tuning's pitch; here, with key 60 still sounding, mix of
                // two is not measured, so repeat with only the new note.
                let mut rig = make(block);
                rig.tune(Some(&tuning));
                let second = rig.render_with(48_000, &[on(24_000, 61)], &[(20_000, after.clone())]);
                let got = measure(&second, 30_000, 46_000);
                let wanted = expected(
                    base,
                    match &after {
                        Some(t) => t.key_frequency(61).unwrap(),
                        None => 440.0 * 2f64.powf((61.0 - 69.0) / 12.0),
                    },
                );
                assert!(
                    cents(got, wanted).abs() < 0.1,
                    "{name} {label}: key 61 at {got}, wanted {wanted}"
                );
            }
        }
    }
}

// Spec: tunings R18. The note that is sounding when the tuning changes
// keeps its pitch to its end.
#[test]
fn a_sounding_note_does_not_move_when_the_tuning_changes() {
    let tuning = nineteen();
    let higher = tuning.with_reference(None, None, Some(300.0)).unwrap();
    for (name, make, base) in instruments() {
        let mut rig = make(512);
        rig.tune(Some(&tuning));
        let signal = rig.render_with(40_000, &[on(0, 61)], &[(20_000, Some(higher.clone()))]);
        let wanted = expected(base, tuning.key_frequency(61).unwrap());
        let before = measure(&signal, 6_000, 19_000);
        let after = measure(&signal, 22_000, 38_000);
        assert!(cents(before, wanted).abs() < 0.1, "{name} before {before}");
        assert!(cents(after, wanted).abs() < 0.1, "{name} after {after}");
    }
}

// Spec: tunings R13; polysynth R25; one-shot drum sampler R25. Under a
// tuning a per-note bend moves a note by steps of the tuning.
#[test]
fn a_per_note_bend_moves_by_steps_of_the_tuning() {
    let tuning = nineteen();
    // A bend of 1/12 of the 12-step range is one step.
    for (name, make, range) in [
        ("Drift", drift as fn(usize) -> Rig, 12.0f32),
        ("Drum Sampler", strike as fn(usize) -> Rig, 48.0),
    ] {
        let mut rig = make(512);
        rig.tune(Some(&tuning));
        let signal = rig.render(48_000, &[on(0, 60), bend(24_000, 60, 1.0 / range * 1.0)]);
        let moved = measure(&signal, 30_000, 46_000);
        // Key 60 bent by one step plays key 61's pitch.
        let wanted = tuning.key_frequency(61).unwrap();
        assert!(
            cents(moved, wanted).abs() < 0.5,
            "{name}: bent to {moved}, wanted {wanted}"
        );
    }
}

// Spec: tunings R12. The tuned pitch is where the key was: a key's pitch in
// semitones agrees with the table.
#[test]
fn the_key_table_agrees_with_the_tuning() {
    let tuning = nineteen();
    let table = KeyPitches::of(Some(&tuning));
    for key in 0..128u8 {
        let wanted = pitch_of(tuning.key_frequency(key).unwrap());
        assert!((table.pitch(key).unwrap() - wanted).abs() < 1e-12);
    }
}

// Spec: tunings R8, R18. A note already sounding on a key that a new range
// leaves out still stops when it's let go.
#[test]
fn a_note_held_past_a_shortened_range_still_stops() {
    let tuning = nineteen();
    let short = tuning
        .with_range(tuning.key_note(0), tuning.key_note(64))
        .unwrap();
    for (name, make, _) in instruments() {
        let mut rig = make(512);
        rig.tune(Some(&tuning));
        let signal = rig.render_with(
            96_000,
            &[on(0, 70), (8_000, NoteKind::Off, 70)],
            &[(4_000, Some(short.clone()))],
        );
        assert!(
            signal[..4_000].iter().any(|s| s.abs() > 0.1),
            "{name} plays"
        );
        assert!(
            signal[90_000..].iter().all(|s| s.abs() < 1e-3),
            "{name} stops"
        );
    }
}
