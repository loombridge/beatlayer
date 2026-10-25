//! CV Instrument (WP-7.17; the CV tools spec, R10 to R15, R18, R19): the
//! voltages it writes for a phrase, read off the CV buffers a program would
//! give it, each block a real-time section under the allocation trap.

use crate::common;
use bl_core::time::Frames;
use bl_devices::cv_instrument::{
    self, BEND_RANGE, FINE, GATE_LEVEL, GLIDE, NOTE_PRIORITY, RETRIGGER, TRANSPOSE, mod_params,
};
use bl_devices::{CV_INSTRUMENT, CvCommand, cv_instrument_type};
use bl_engine::{
    Block, Collector, KeyPitches, Node, NoteBuffer, NoteEvent, NoteKind, Registry, TransportInfo,
};
use bl_model::{CvCalibration, ParamId, Tuning};
use common::{CHANNELS, RATE, device, node};

/// The block sizes every render is checked at.
const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

/// A note for [`Rig::render`]: the frame, what it does, its key, its
/// velocity.
#[derive(Clone, Copy)]
struct Played(usize, NoteKind, u8, f32);

fn on(frame: usize, key: u8) -> Played {
    Played(frame, NoteKind::On, key, 1.0)
}

fn off(frame: usize, key: u8) -> Played {
    Played(frame, NoteKind::Off, key, 0.0)
}

/// What the four outputs wrote over a render, in sample values.
struct Written {
    pitch: Vec<f32>,
    gate: Vec<f32>,
    mod1: Vec<f32>,
    mod2: Vec<f32>,
}

/// CV Instrument's node, the collector that wraps its data, and the block
/// size it runs at.
struct Rig {
    node: Box<dyn Node>,
    reader: bl_engine::DisplayReader,
    collector: Collector,
    block: usize,
    playing: bool,
    position: i64,
}

impl Rig {
    fn new(params: &[(ParamId, f64)], block: usize) -> Self {
        Self::with(params, block, None)
    }

    fn with(params: &[(ParamId, f64)], block: usize, calibration: Option<CvCalibration>) -> Self {
        let mut device = device(&cv_instrument_type(), params);
        if let Some(external) = device.external.as_mut() {
            external.cv_calibration = calibration;
        }
        let mut node = node(&registry(), &device, 4096);
        Self {
            reader: node.display().expect("the device has a display"),
            node,
            collector: Collector::new(),
            block,
            playing: true,
            position: 0,
        }
    }

    fn send<T: std::any::Any + Send + Sync>(&mut self, data: T) {
        self.node.receive_data(self.collector.handle().data(data));
    }

    fn tune(&mut self, tuning: Option<&Tuning>) {
        self.send(KeyPitches::of(tuning));
    }

    /// Plays `frames` frames from where the last call stopped, `notes` at
    /// their frames from this call's start.
    fn render(&mut self, frames: usize, notes: &[Played]) -> Written {
        let mut written = Written {
            pitch: Vec::new(),
            gate: Vec::new(),
            mod1: Vec::new(),
            mod2: Vec::new(),
        };
        let mut start = 0;
        while start < frames {
            let len = self.block.min(frames - start);
            let block_notes: Vec<NoteEvent> = notes
                .iter()
                .filter(|n| n.0 >= start && n.0 < start + len)
                .map(|&Played(at, kind, key, velocity)| NoteEvent {
                    offset: (at - start) as u32,
                    kind,
                    channel: 0,
                    key,
                    velocity,
                    note_id: None,
                })
                .collect();
            self.block(len, &block_notes, None, &mut written);
            start += len;
        }
        written
    }

    /// Runs one block, with the interface's return `heard` if there is one.
    fn block(
        &mut self,
        len: usize,
        notes: &[NoteEvent],
        heard: Option<&[f32]>,
        written: &mut Written,
    ) {
        let pitch = CHANNELS * len;
        let mut data = vec![0.0f32; CHANNELS * len];
        let mut cv = vec![0.0f32; 4 * pitch];
        let mut sink = NoteBuffer::with_capacity(notes.len().max(1));
        {
            let transport = TransportInfo {
                playing: self.playing,
                position: Frames(self.position),
            };
            let mut block = Block::new(&mut data, CHANNELS, len, len, &[], transport, RATE)
                .with_notes(notes, &mut sink)
                .with_cv_out(&mut cv);
            if let Some(heard) = heard {
                block = block.with_external(heard);
            }
            let node = &mut self.node;
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        self.position += len as i64;
        for (output, out) in [
            &mut written.pitch,
            &mut written.gate,
            &mut written.mod1,
            &mut written.mod2,
        ]
        .into_iter()
        .enumerate()
        {
            out.extend_from_slice(&cv[output * pitch..output * pitch + len]);
        }
    }

    fn display(&mut self) -> Vec<f32> {
        self.reader.read().values.clone()
    }
}

/// 19 equal steps to the octave.
fn nineteen() -> Tuning {
    let cents = (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect();
    Tuning::new("19 equal".into(), String::new(), cents).unwrap()
}

// Spec: cv-tools R12
#[test]
fn key_60_is_zero_volts_and_72_is_one_volt_an_octave_up() {
    for block in BLOCKS {
        let mut rig = Rig::new(&[], block);
        let out = rig.render(2000, &[on(100, 60), off(500, 60), on(1000, 72)]);
        assert_eq!(out.pitch[200], 0.0, "key 60 at block size {block}");
        assert_eq!(out.pitch[1500], 0.1, "key 72 at block size {block}");
        // The pitch holds after the key is released.
        assert_eq!(out.pitch[700], 0.0);
    }
}

// Spec: cv-tools R11
#[test]
fn transpose_fine_bend_range_and_a_tuning_move_the_pitch() {
    // Transpose by an octave.
    let mut rig = Rig::new(&[(TRANSPOSE, 12.0)], 64);
    assert_eq!(rig.render(200, &[on(0, 60)]).pitch[100], 0.1);
    // Fine: 100 cents is a semitone, a twelfth of a volt.
    let mut rig = Rig::new(&[(FINE, 100.0)], 64);
    let pitch = rig.render(200, &[on(0, 60)]).pitch[100];
    assert!((pitch - 0.1 / 12.0).abs() < 1e-7, "{pitch}");
    // Pitch bend times Bend Range: a full bend up of 12 semitones.
    let mut rig = Rig::new(&[(BEND_RANGE, 12.0)], 64);
    let bend = Played(0, NoteKind::PitchBend { value: 1.0 }, 0, 0.0);
    let pitch = rig.render(200, &[bend, on(1, 60)]).pitch[100];
    assert!((pitch - 0.1).abs() < 1e-7, "{pitch}");
    // A loaded 19-step tuning: key 79 is an octave over key 60.
    let tuning = nineteen();
    let mut rig = Rig::new(&[], 64);
    rig.tune(Some(&tuning));
    let out = rig.render(600, &[on(0, 60), on(200, 79), on(400, 61)]);
    assert!(
        (out.pitch[300] - out.pitch[100] - 0.1).abs() < 1e-6,
        "{} over {}",
        out.pitch[300],
        out.pitch[100]
    );
    // One key up is 12 / 19 semitones.
    assert!(
        (out.pitch[500] - out.pitch[100] - 0.1 / 19.0).abs() < 1e-6,
        "{} over {}",
        out.pitch[500],
        out.pitch[100]
    );
}

// Spec: cv-tools R11
#[test]
fn glide_runs_a_straight_line_in_volts_over_its_time() {
    // 100 ms at 48 kHz is 4,800 frames.
    for block in BLOCKS {
        let mut rig = Rig::new(&[(GLIDE, 100.0)], block);
        let out = rig.render(8000, &[on(0, 60), on(1000, 72)]);
        assert_eq!(out.pitch[999], 0.0);
        assert_eq!(out.pitch[1000], 0.0, "the line starts where the pitch was");
        for k in [1200usize, 2000, 3400, 5000] {
            let want = 0.1 * (k - 1000) as f32 / 4800.0;
            assert!(
                (out.pitch[k] - want).abs() < 1e-6,
                "frame {k} at block size {block}: {} against {want}",
                out.pitch[k]
            );
        }
        assert!((out.pitch[5800] - 0.1).abs() < 1e-6, "it arrives");
        assert!((out.pitch[7999] - 0.1).abs() < 1e-6);
    }
}

// Spec: cv-tools R13
#[test]
fn gate_rises_and_falls_on_the_notes_frames_at_gate_level() {
    let mut rig = Rig::new(&[(GATE_LEVEL, 8.0)], 64);
    let out = rig.render(1000, &[on(100, 60), off(600, 60)]);
    assert_eq!(out.gate[99], 0.0);
    assert_eq!(out.gate[100], 0.8, "8 V is 0.8");
    assert_eq!(out.gate[599], 0.8);
    assert_eq!(out.gate[600], 0.0);
}

// Spec: cv-tools R13
#[test]
fn a_legato_note_takes_the_gate_down_for_a_millisecond_only_with_retrigger() {
    let phrase = [on(100, 60), on(500, 64), off(900, 60), off(1100, 64)];
    for block in BLOCKS {
        let mut rig = Rig::new(&[], block);
        let out = rig.render(1500, &phrase);
        assert_eq!(out.gate[499], 0.5);
        for frame in 500..548 {
            assert_eq!(out.gate[frame], 0.0, "frame {frame} at block size {block}");
        }
        assert_eq!(out.gate[548], 0.5, "back after 48 frames at 48 kHz");
        // The gate falls with the last key, not the first.
        assert_eq!(out.gate[1000], 0.5);
        assert_eq!(out.gate[1100], 0.0);

        let mut rig = Rig::new(&[(RETRIGGER, 0.0)], block);
        let out = rig.render(1500, &phrase);
        assert!(out.gate[100..1100].iter().all(|&g| g == 0.5), "legato");
    }
}

// Spec: cv-tools R11
#[test]
fn note_priority_picks_the_key_from_a_chord_and_on_release() {
    let chord = [
        on(100, 60),
        on(200, 67),
        on(300, 64),
        off(600, 64),
        off(800, 67),
    ];
    let at = |priority: f64| {
        let mut rig = Rig::new(&[(NOTE_PRIORITY, priority)], 64);
        let out = rig.render(1000, &chord);
        // The key sounding at frames 400, 700 and 900, as semitones over 60.
        [400, 700, 900].map(|f| (out.pitch[f] * 120.0).round() as i32)
    };
    // Last: the newest, then the one before it as keys are let go.
    assert_eq!(at(0.0), [4, 7, 0]);
    // Low: the lowest of those held.
    assert_eq!(at(1.0), [0, 0, 0]);
    // High: the highest.
    assert_eq!(at(2.0), [7, 7, 0]);
}

// Spec: cv-tools R13
#[test]
fn the_mod_outputs_follow_velocity_and_the_mod_wheel_between_min_and_max() {
    let [_, min1, max1] = mod_params(0);
    let [_, min2, max2] = mod_params(1);
    let mut rig = Rig::new(&[(min1, 1.0), (max1, 9.0), (min2, -5.0), (max2, 5.0)], 64);
    let wheel = |at: usize, value: f32| {
        Played(
            at,
            NoteKind::Control {
                controller: 1,
                value,
            },
            0,
            0.0,
        )
    };
    let out = rig.render(
        1000,
        &[
            // Velocity and the mod wheel before anything is played.
            Played(100, NoteKind::On, 60, 0.5),
            wheel(300, 0.25),
            wheel(500, 1.0),
        ],
    );
    // Mod 1 follows Velocity from 1 V to 9 V: half way is 5 V.
    assert_eq!(out.mod1[50], 0.1, "1 V before a note");
    assert!((out.mod1[200] - 0.5).abs() < 1e-6);
    // Mod 2 follows the wheel from -5 V to 5 V.
    assert!((out.mod2[200] + 0.5).abs() < 1e-6);
    assert!((out.mod2[400] + 0.25).abs() < 1e-6, "a quarter of the way");
    assert!((out.mod2[600] - 0.5).abs() < 1e-6);
}

// Spec: cv-tools R18
#[test]
fn the_gate_falls_with_the_transport_and_when_the_device_lets_go() {
    let mut rig = Rig::new(&[], 64);
    let held = rig.render(200, &[on(0, 60)]);
    assert_eq!(held.gate[199], 0.5);
    rig.playing = false;
    let stopped = rig.render(64, &[]);
    assert_eq!(stopped.gate[0], 0.0, "the first frame after the stop");

    let mut rig = Rig::new(&[], 64);
    rig.render(200, &[on(0, 60)]);
    let mut sink = NoteBuffer::with_capacity(4);
    rig.node.release(&mut sink);
    assert_eq!(rig.render(64, &[]).gate[0], 0.0);
}

// Spec: cv-tools R9
#[test]
fn the_display_gives_each_outputs_last_voltage() {
    let mut rig = Rig::new(&[(TRANSPOSE, 12.0)], 64);
    rig.render(200, &[on(0, 60)]);
    let shown = rig.display();
    assert_eq!(
        &shown[..4],
        &[1.0, 5.0, 5.0, 0.0][..],
        "V of Pitch, Gate, Mod 1 (velocity 1), Mod 2"
    );
    let display = bl_devices::display(
        CV_INSTRUMENT,
        bl_model::DeviceId(bl_core::id::Id::new(1).unwrap()),
        &cv_instrument_type().description,
        &Default::default(),
        Some(&shown),
    )
    .expect("CV Instrument has a display");
    assert_eq!(display.reading("Pitch").unwrap().text, "1.00 V");
    assert_eq!(display.reading("Gate").unwrap().text, "5.00 V");
}

/// An oscillator patched to Pitch: it sounds a sine whose frequency follows
/// the voltage written the block before.
struct Oscillator {
    phase: f64,
    volts: f64,
}

impl Oscillator {
    /// 0.97 octaves a volt from 164 Hz at -0.13 V, with a slight bow.
    fn hz(volts: f64) -> f64 {
        164.0 * (0.97 * (volts + 0.13) + 0.00005 * (volts * volts - 3.0 * volts)).exp2()
    }

    fn sound(&mut self, frames: usize) -> Vec<f32> {
        let hz = Self::hz(self.volts);
        (0..frames)
            .map(|_| {
                self.phase = (self.phase + hz / 48_000.0).fract();
                (self.phase * std::f64::consts::TAU).sin() as f32 * 0.5
            })
            .collect()
    }
}

// Spec: cv-tools R14
#[test]
fn a_calibration_steps_through_the_voltages_and_publishes_each_pitch() {
    let mut rig = Rig::new(&[], 64);
    rig.send(CvCommand::Calibrate);
    let mut oscillator = Oscillator {
        phase: 0.0,
        volts: -2.0,
    };
    let mut written = Written {
        pitch: Vec::new(),
        gate: Vec::new(),
        mod1: Vec::new(),
        mod2: Vec::new(),
    };
    // Eight steps of 250 ms, with a block to spare.
    let mut volts = Vec::new();
    for _ in 0..(8 * 12_000 / 64 + 2) {
        let heard = oscillator.sound(64);
        let heard = [heard.clone(), heard].concat();
        rig.block(64, &[], Some(&heard), &mut written);
        oscillator.volts = f64::from(*written.pitch.last().unwrap()) * 10.0;
        volts.push(oscillator.volts);
    }
    let shown = rig.display();
    assert_eq!(shown[cv_instrument::STATE_AT], cv_instrument::FINISHED);
    // Each step's pitch is what the oscillator sounded, where the detector
    // reads it: from 41 Hz to 2 kHz (above it, it mistakes the octave).
    for step in 0..8 {
        let want = Oscillator::hz(-2.0 + step as f64);
        let got = f64::from(shown[cv_instrument::PITCHES_AT + step]);
        if (41.0..=2000.0).contains(&want) {
            let cents = 1200.0 * (got / want).log2();
            assert!(cents.abs() < 1.0, "step {step}: {got} Hz against {want} Hz");
        }
    }
    // The gate was high throughout the first step.
    assert!(written.gate[..12_000].iter().all(|&g| g == 0.5));
    assert!((volts[10] + 2.0).abs() < 1e-6);
}

// Spec: cv-tools R12, R15
#[test]
fn a_calibrated_pitch_makes_the_oscillator_sound_the_keys_frequency() {
    let measured: Vec<(f64, f64)> = (-2..=3)
        .map(|v| (f64::from(v), Oscillator::hz(f64::from(v))))
        .collect();
    let calibration = CvCalibration::fit(&measured).unwrap();
    let mut rig = Rig::with(&[], 64, Some(calibration));
    for key in [36u8, 48, 60, 72, 84, 96] {
        let out = rig.render(200, &[on(0, key), off(150, key)]);
        let volts = f64::from(out.pitch[100]) * 10.0;
        let want = 440.0 * ((f64::from(key) - 69.0) / 12.0).exp2();
        let cents = 1200.0 * (Oscillator::hz(volts) / want).log2();
        assert!(cents.abs() < 1.0, "key {key}: {cents} cents off");
    }
}

// Spec: cv-tools R19
#[test]
fn a_notes_own_bend_slide_and_pressure_reach_the_pitch_and_the_mod_outputs() {
    use bl_devices::cv_instrument::BEND_RANGE;
    let [source1, _, _] = mod_params(0);
    let [source2, _, _] = mod_params(1);
    // Mod 1 follows Slide (the seventh choice), Mod 2 Pressure (the fourth).
    let mut rig = Rig::new(&[(BEND_RANGE, 12.0), (source1, 6.0), (source2, 3.0)], 64);
    let own = |at: usize, expression, value: f32| {
        Played(at, NoteKind::Expression { expression, value }, 60, 0.0)
    };
    let out = rig.render(
        1000,
        &[
            on(0, 60),
            own(100, bl_engine::NoteExpression::Bend, 1.0),
            own(300, bl_engine::NoteExpression::Slide, 0.5),
            own(500, bl_engine::NoteExpression::Pressure, 1.0),
        ],
    );
    assert_eq!(out.pitch[50], 0.0);
    // The note's own bend of a whole range up is an octave, as the channel's
    // bend would be.
    assert!((out.pitch[200] - 0.1).abs() < 1e-6, "{}", out.pitch[200]);
    // Slide and Pressure follow the sounding note's: Max is 5 V.
    assert!((out.mod1[400] - 0.25).abs() < 1e-6, "{}", out.mod1[400]);
    assert!((out.mod2[800] - 0.5).abs() < 1e-6, "{}", out.mod2[800]);
}
