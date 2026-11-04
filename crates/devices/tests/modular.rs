//! Modular (WP-9.36; the modular device spec, R1 to R30): patches built by
//! command, rendered under the allocation trap at several block sizes.

use std::f64::consts::TAU;

use bl_core::time::{Bpm, Frames, TempoMap};
use bl_engine::{
    Block, Collector, Node, NodeData, NodeEvent, NoteBuffer, NoteEvent, TransportInfo,
};
use bl_model::modular::{self, BlockId, Cable, Patch};
use bl_model::{Command, Device, DeviceId, Editor, ParamId, TrackKind};

use crate::common::{self, CHANNELS, RATE};

const HZ: f64 = 48_000.0;

/// A Modular of `device_type` whose patch is built by commands, from the
/// patch the type starts with (`empty` false) or from no blocks at all.
struct Build {
    editor: Editor,
    device: DeviceId,
}

impl Build {
    fn new(device_type: &str, empty: bool) -> Self {
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
        let device = editor.new_id();
        let registry = common::registry();
        let ty = registry
            .device_types()
            .into_iter()
            .find(|t| matches!(&t.kind, bl_model::DeviceKind::Native { device_type: d } if d == device_type))
            .expect("the device type is registered");
        let mut made = ty.device(device);
        if empty {
            made.patch = Some(Patch::default());
        }
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: made,
            })
            .unwrap();
        Self { editor, device }
    }

    fn run(&mut self, command: Command) -> &mut Self {
        self.editor.apply(command).unwrap();
        self
    }

    fn add(&mut self, kind: &str, id: u32) -> &mut Self {
        let device = self.device;
        self.run(Command::ModularAddBlock {
            device,
            block_type: kind.to_owned(),
            id: Some(BlockId(id)),
            x: 0.0,
            y: 0.0,
        })
    }

    fn wire(&mut self, from: u32, from_port: &str, to: u32, to_port: &str) -> &mut Self {
        let device = self.device;
        self.run(Command::ModularConnect {
            device,
            from: BlockId(from),
            from_port: from_port.to_owned(),
            to: BlockId(to),
            to_port: to_port.to_owned(),
        })
    }

    fn set(&mut self, block: u32, setting: &str, value: f64) -> &mut Self {
        let device = self.device;
        self.run(Command::ModularSet {
            device,
            block: BlockId(block),
            setting: setting.to_owned(),
            value,
        })
    }

    fn expose(&mut self, block: u32, setting: &str) -> &mut Self {
        let device = self.device;
        self.run(Command::ModularExpose {
            device,
            block: BlockId(block),
            setting: setting.to_owned(),
            exposed: true,
        })
    }

    fn param(&mut self, param: ParamId, value: f64) -> &mut Self {
        let device = self.device;
        self.run(Command::SetDeviceParam {
            device,
            param,
            value,
        })
    }

    fn device(&self) -> Device {
        self.editor.document().device(self.device).unwrap().clone()
    }
}

/// A node being rendered a block at a time.
struct Rig {
    node: Box<dyn Node>,
    collector: Collector,
    block: usize,
    done: i64,
    tempo: TempoMap,
}

impl Rig {
    fn new(device: &Device, block: usize) -> Self {
        Self {
            node: common::node(&common::registry(), device, block),
            collector: Collector::new(),
            block,
            done: 0,
            tempo: TempoMap::constant(Bpm::DEFAULT),
        }
    }

    /// Hands the node the schedule `device`'s patch compiles to, as the
    /// engine does on the audio thread: the whole delivery is a real-time
    /// section.
    fn swap(&mut self, device: &Device) {
        let data = bl_devices::modular::device_data(device, &common::config(self.block), &|_| None);
        for item in data.items {
            let data: NodeData = item(&self.collector.handle());
            bl_rt::rt_test!({ self.node.receive_data(data) });
        }
    }

    /// `frames` frames with `notes` (offsets from this call's first frame),
    /// parameter `events` and the effect's `input`/`sidechain`, in blocks of
    /// the rig's size; returns left and right.
    fn render(
        &mut self,
        frames: usize,
        notes: &[NoteEvent],
        events: &[NodeEvent],
        input: Option<&[f32]>,
        sidechain: Option<&[f32]>,
    ) -> [Vec<f32>; 2] {
        let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
        let mut at = 0;
        while at < frames {
            let len = self.block.min(frames - at);
            let inside = |offset: u32| (offset as usize) >= at && (offset as usize) < at + len;
            let notes: Vec<NoteEvent> = notes
                .iter()
                .filter(|n| inside(n.offset))
                .map(|n| NoteEvent {
                    offset: n.offset - at as u32,
                    ..*n
                })
                .collect();
            let events: Vec<NodeEvent> = events
                .iter()
                .filter(|e| inside(e.offset))
                .map(|e| NodeEvent {
                    offset: e.offset - at as u32,
                    ..*e
                })
                .collect();
            let mut data = vec![0.0f32; CHANNELS * len];
            if let Some(input) = input {
                for c in 0..CHANNELS {
                    data[c * len..(c + 1) * len].copy_from_slice(&input[at..at + len]);
                }
            }
            let mut side = Vec::new();
            if let Some(sidechain) = sidechain {
                for _ in 0..CHANNELS {
                    side.extend_from_slice(&sidechain[at..at + len]);
                }
            }
            let mut sent = NoteBuffer::with_capacity(64);
            {
                let transport = TransportInfo {
                    playing: true,
                    position: Frames(self.done + at as i64),
                    ..TransportInfo::default()
                };
                let mut block = Block::new(&mut data, CHANNELS, len, len, &events, transport, RATE)
                    .with_tempo(&self.tempo)
                    .with_notes(&notes, &mut sent);
                if sidechain.is_some() {
                    block = block.with_sidechain(&side);
                }
                bl_rt::rt_test!({ self.node.process(&mut block) });
            }
            out[0].extend_from_slice(&data[..len]);
            out[1].extend_from_slice(&data[len..]);
            at += len;
        }
        self.done += frames as i64;
        out
    }

    fn play(&mut self, frames: usize, notes: &[NoteEvent]) -> Vec<f32> {
        let [left, _] = self.render(frames, notes, &[], None, None);
        left
    }
}

fn on(offset: u32, key: u8, id: u32) -> NoteEvent {
    common::note_on(offset, key, 1.0, Some(id))
}

fn off(offset: u32, key: u8, id: u32) -> NoteEvent {
    common::note_off(offset, &on(0, key, id), 0.0)
}

fn hz(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// The amplitude of the sine at `frequency` in `signal` (a Hann-windowed
/// DFT over the whole slice).
fn amplitude_at(signal: &[f32], frequency: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im, mut weights) = (0.0f64, 0.0f64, 0.0f64);
    for (i, &x) in signal.iter().enumerate() {
        let w = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
        let phase = TAU * frequency * i as f64 / HZ;
        re += w * f64::from(x) * phase.cos();
        im += w * f64::from(x) * phase.sin();
        weights += w;
    }
    2.0 * (re * re + im * im).sqrt() / weights
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |m, x| m.max(x.abs()))
}

/// A one-voice sine instrument: Note In to an oscillator to Audio Out.
fn sine(wave: f64) -> Build {
    let mut b = Build::new(modular::INSTRUMENT, true);
    b.add("note_in", 1)
        .add("oscillator", 2)
        .add("audio_out", 3)
        .wire(1, "pitch", 2, "pitch")
        .wire(2, "out", 3, "left")
        .wire(2, "out", 3, "right")
        .set(2, "wave", wave);
    b
}

// Spec: modular-device R1, R13
#[test]
fn a_new_instrument_plays_the_note_it_is_sent_at_its_pitch() {
    let mut rig = Rig::new(&Build::new(modular::INSTRUMENT, false).device(), 512);
    let signal = rig.play(24_000, &[on(0, 69, 1)]);
    assert!(peak(&signal) > 0.5, "the default patch makes a sound");
    // A saw at A3 (key 69 is A4 here): its fundamental is the note's pitch.
    assert!(amplitude_at(&signal[4800..], hz(69)) > 0.3);
    let c = Rig::new(&sine(0.0).device(), 512).play(24_000, &[on(0, 60, 1)]);
    assert!(
        (amplitude_at(&c[2400..], 261.625_565) - 1.0).abs() < 0.02,
        "key 60 is C3 at 0 octaves"
    );
}

// Spec: modular-device R5, R13
#[test]
fn velocity_gate_and_release_shape_the_note() {
    let mut b = Build::new(modular::INSTRUMENT, true);
    b.add("note_in", 1)
        .add("envelope", 2)
        .add("audio_out", 3)
        .wire(1, "gate", 2, "gate")
        .wire(2, "out", 3, "left")
        .set(2, "attack", 0.0)
        .set(2, "decay", 0.0)
        .set(2, "sustain", 1.0)
        .set(2, "release", 100.0);
    let mut rig = Rig::new(&b.device(), 64);
    let out = rig.play(24_000, &[on(100, 60, 1), off(12_000, 60, 1)]);
    assert_eq!(out[99], 0.0);
    assert!(
        (out[100] - 1.0).abs() < 1e-6,
        "the gate opens on the note's frame"
    );
    assert!((out[11_999] - 1.0).abs() < 1e-6);
    assert!(
        out[12_000 + 2400] > 0.4 && out[12_000 + 2400] < 0.6,
        "half way down a 100 ms release"
    );
    assert!(out[12_000 + 4900].abs() < 1e-6);
}

// Spec: modular-device R7, R20
#[test]
fn a_patch_renders_the_same_at_any_block_size_and_moving_a_block_changes_nothing() {
    let mut b = Build::new(modular::INSTRUMENT, true);
    b.add("note_in", 1)
        .add("oscillator", 2)
        .add("filter", 3)
        .add("envelope", 4)
        .add("amplifier", 5)
        .add("lfo", 6)
        .add("delay", 7)
        .add("mixer", 8)
        .add("audio_out", 9)
        .wire(1, "pitch", 2, "pitch")
        .wire(2, "out", 3, "in")
        .wire(6, "out", 3, "cutoff")
        .wire(1, "gate", 4, "gate")
        .wire(3, "out", 5, "in")
        .wire(4, "out", 5, "cv")
        .wire(5, "out", 8, "in1")
        .wire(5, "out", 7, "in")
        .wire(7, "out", 8, "in2")
        .wire(8, "out", 9, "left")
        .wire(8, "out", 9, "right")
        .set(2, "wave", 2.0)
        .set(3, "cutoff", 800.0)
        .set(3, "resonance", 0.5)
        .set(6, "rate", 3.0)
        .set(7, "time", 37.0)
        .set(8, "level2", 0.5);
    let notes = [
        on(10, 60, 1),
        on(2000, 64, 2),
        off(9000, 60, 1),
        on(10_000, 67, 3),
        off(20_000, 64, 2),
    ];
    let device = b.device();
    let reference = Rig::new(&device, 512).play(30_000, &notes);
    assert!(peak(&reference) > 0.1);
    for block in [1, 61, 4096] {
        assert_eq!(
            Rig::new(&device, block).play(30_000, &notes),
            reference,
            "block size {block}"
        );
    }
    // R20: another place for a block, and a block no cable reaches.
    let device_id = b.device;
    b.run(Command::ModularMoveBlock {
        device: device_id,
        block: BlockId(4),
        x: 9.0,
        y: 9.0,
    });
    b.add("oscillator", 20);
    assert_eq!(Rig::new(&b.device(), 61).play(30_000, &notes), reference);
}

// Spec: modular-device R4
#[test]
fn a_patch_edit_swaps_in_without_a_click_and_keeps_the_blocks_state() {
    let step = |signal: &[f32]| {
        signal
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    };
    let before = sine(0.0);
    let mut rig = Rig::new(&before.device(), 61);
    let mut signal = rig.play(2000, &[on(0, 69, 1)]);
    // The oscillator goes up a fifth, mid-note.
    let mut after = sine(0.0);
    after.set(2, "coarse", 7.0);
    rig.swap(&after.device());
    signal.extend(rig.play(2000, &[]));
    assert!(
        step(&signal) < 0.1,
        "a sine at 659 Hz steps by at most 0.087"
    );
    assert!(amplitude_at(&signal[2400 - 400 + 400..], 440.0 * 2f64.powf(7.0 / 12.0)) > 0.9);
    // A block of another ID starts from rest: its phase jumps. It is the
    // control: the check above would see that.
    let mut rig = Rig::new(&before.device(), 61);
    let mut jumped = rig.play(1999, &[on(0, 69, 1)]);
    let mut fresh = Build::new(modular::INSTRUMENT, true);
    fresh
        .add("note_in", 1)
        .add("oscillator", 12)
        .add("audio_out", 3)
        .wire(1, "pitch", 12, "pitch")
        .wire(12, "out", 3, "left")
        .set(12, "wave", 0.0);
    rig.swap(&fresh.device());
    jumped.extend(rig.play(500, &[]));
    assert!(
        step(&jumped) > 0.3,
        "a block that isn't kept restarts its phase"
    );
}

// Spec: modular-device R5
#[test]
fn an_instrument_plays_up_to_its_voices_and_a_new_note_takes_the_oldest_with_a_fade() {
    let mut b = sine(0.0);
    b.param(modular::VOICES, 2.0);
    let mut rig = Rig::new(&b.device(), 61);
    let notes = [on(0, 60, 1), on(100, 67, 2), on(24_000, 72, 3)];
    let signal = rig.play(48_000, &notes);
    // Two voices before the third note: both pitches sound.
    let before = &signal[8000..24_000];
    assert!(amplitude_at(before, hz(60)) > 0.4 && amplitude_at(before, hz(67)) > 0.4);
    // The third takes the oldest, which was the first: C is gone, G and the
    // new note stay.
    let after = &signal[30_000..48_000];
    assert!(
        amplitude_at(after, hz(60)) < 0.02,
        "the oldest voice is gone"
    );
    assert!(amplitude_at(after, hz(67)) > 0.4 && amplitude_at(after, hz(72)) > 0.4);
    // The oldest fades over 5 ms rather than cutting off.
    let fade = &signal[24_000..24_000 + 260];
    let step = fade
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(step < 0.2, "no click when a voice is stolen: {step}");
    // Four voices, more notes than that: the oldest of the four goes.
    let mut b = sine(0.0);
    b.param(modular::VOICES, 4.0);
    let mut rig = Rig::new(&b.device(), 512);
    let many: Vec<_> = (0..6u32)
        .map(|i| on(i * 100, 60 + i as u8, i + 1))
        .collect();
    let out = rig.play(12_000, &many);
    assert!(peak(&out) <= 4.2, "at most four voices sound");
}

// Spec: modular-device R6
#[test]
fn a_patch_parameter_moves_its_setting_and_automation_reaches_it() {
    let mut b = sine(0.0);
    b.expose(2, "coarse");
    let device = b.device();
    let slot = device.patch.as_ref().unwrap().params[0].slot;
    let param = modular::slot_param(slot);
    // Its value is where the setting sits in its range: 0 st is the middle.
    assert!((device.params[&param] - 0.5).abs() < 1e-9);
    let mut rig = Rig::new(&device, 128);
    let notes = [on(0, 60, 1)];
    let up = common::set(24_000, param, 0.5 + 12.0 / 96.0);
    let [signal, _] = rig.render(48_000, &notes, &[up], None, None);
    assert!(amplitude_at(&signal[4000..20_000], 261.625_565) > 0.9);
    assert!(
        amplitude_at(&signal[30_000..46_000], 2.0 * 261.625_565) > 0.9,
        "an octave up"
    );
}

// Spec: modular-device R3, R28
#[test]
fn a_delay_delays_by_its_time_and_closes_a_feedback_loop() {
    let mut b = Build::new(modular::EFFECT, true);
    b.add("audio_in", 1)
        .add("delay", 2)
        .add("audio_out", 3)
        .wire(1, "left", 2, "in")
        .wire(2, "out", 3, "left")
        .set(2, "time", 10.0);
    let mut input = vec![0.0f32; 4800];
    input[100] = 1.0;
    let [out, _] = Rig::new(&b.device(), 61).render(4800, &[], &[], Some(&input), None);
    assert_eq!(
        out.iter().position(|x| *x != 0.0),
        Some(580),
        "10 ms is 480 frames"
    );
    // The smallest delay is one sample.
    b.set(2, "time", 0.0);
    let [out, _] = Rig::new(&b.device(), 61).render(4800, &[], &[], Some(&input), None);
    assert_eq!(out.iter().position(|x| *x != 0.0), Some(101));
    // The time input adds seconds.
    b.set(2, "time", 0.0)
        .add("arithmetic", 4)
        .wire(4, "out", 2, "time")
        .set(4, "op", 6.0)
        .set(4, "a", 0.0078125);
    let [out, _] = Rig::new(&b.device(), 61).render(4800, &[], &[], Some(&input), None);
    assert_eq!(
        out.iter().position(|x| *x != 0.0),
        Some(475),
        "1/128 s is 375 frames"
    );
    // A loop through the delay: each trip comes back at half the level.
    let mut b = Build::new(modular::EFFECT, true);
    b.add("audio_in", 1)
        .add("mixer", 2)
        .add("delay", 3)
        .add("amplifier", 4)
        .add("audio_out", 5)
        .wire(1, "left", 2, "in1")
        .wire(4, "out", 2, "in2")
        .wire(2, "out", 3, "in")
        .wire(3, "out", 4, "in")
        .wire(2, "out", 5, "left")
        .set(3, "time", 1.0)
        .set(4, "gain", -6.0206);
    let [out, _] = Rig::new(&b.device(), 61).render(4800, &[], &[], Some(&input), None);
    assert!(
        (out[100] - 1.0).abs() < 1e-6
            && (out[148] - 0.5).abs() < 1e-4
            && (out[196] - 0.25).abs() < 1e-4
    );
}

// Spec: modular-device R14
#[test]
fn an_effect_passes_audio_in_to_audio_out_and_is_silent_with_no_cable_to_it() {
    let input = common::noise(2048);
    let [out, _] = Rig::new(&Build::new(modular::EFFECT, false).device(), 512).render(
        2048,
        &[],
        &[],
        Some(&input),
        None,
    );
    assert_eq!(out, input);
    let mut b = Build::new(modular::EFFECT, false);
    let device = b.device;
    b.run(Command::ModularDisconnect {
        device,
        from: BlockId(1),
        from_port: "left".to_owned(),
        to: BlockId(2),
        to_port: "left".to_owned(),
    });
    let [out, right] = Rig::new(&b.device(), 512).render(2048, &[], &[], Some(&input), None);
    assert_eq!(peak(&out), 0.0);
    assert_eq!(right, input, "the right side still has its cable");
}

// Spec: modular-device R14, R29
#[test]
fn sidechain_in_gives_the_devices_sidechain() {
    let mut b = Build::new(modular::EFFECT, true);
    b.add("sidechain_in", 1)
        .add("audio_out", 2)
        .wire(1, "left", 2, "left");
    let side = common::noise(1024);
    let input = vec![0.0f32; 1024];
    let [out, _] = Rig::new(&b.device(), 256).render(1024, &[], &[], Some(&input), Some(&side));
    assert_eq!(out, side);
}

// Spec: modular-device R13, R29
#[test]
fn a_midi_effect_sends_on_the_notes_a_note_in_joins_to_a_note_out() {
    let device = Build::new(modular::MIDI_EFFECT, false).device();
    let mut node = common::node(&common::registry(), &device, 256);
    let sent = common::notes_through(node.as_mut(), &[on(5, 60, 1), off(100, 60, 1)], 256);
    assert_eq!(common::keys_on(&sent), vec![60]);
    assert_eq!(sent.len(), 2);
    let mut b = Build::new(modular::MIDI_EFFECT, false);
    let device_id = b.device;
    b.run(Command::ModularDisconnect {
        device: device_id,
        from: BlockId(1),
        from_port: "notes".to_owned(),
        to: BlockId(2),
        to_port: "notes".to_owned(),
    });
    let mut node = common::node(&common::registry(), &b.device(), 256);
    assert!(common::notes_through(node.as_mut(), &[on(5, 60, 1)], 256).is_empty());
}

// Spec: modular-device R21
#[test]
fn an_oscillator_plays_its_four_waves_at_its_pitch_with_coarse_fine_pulse_width_sync_and_pm() {
    for (wave, rms) in [(0.0, 0.707), (1.0, 0.577), (2.0, 0.577), (3.0, 1.0)] {
        let out = Rig::new(&sine(wave).device(), 512).play(24_000, &[on(0, 69, 1)]);
        let body = &out[2400..];
        let level =
            (body.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / body.len() as f64).sqrt();
        assert!((level - rms).abs() < 0.03, "wave {wave}: {level}");
        let crossings = body
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count() as f64;
        assert!(
            (crossings / (body.len() as f64 / HZ) - 440.0).abs() < 2.0,
            "wave {wave}"
        );
    }
    // Coarse and Fine: 12 semitones and 1,200 cents are one octave each.
    for (setting, value) in [("coarse", 12.0), ("fine", 100.0)] {
        let mut b = sine(0.0);
        b.set(2, setting, value);
        let out = Rig::new(&b.device(), 512).play(24_000, &[on(0, 69, 1)]);
        assert!(
            amplitude_at(
                &out[2400..],
                440.0 * 2f64.powf(value / if setting == "coarse" { 12.0 } else { 1200.0 })
            ) > 0.95,
            "{setting}"
        );
    }
    // Pulse width: the share of the cycle the square is high.
    let mut b = sine(3.0);
    b.set(2, "pulse_width", 0.25);
    let out = Rig::new(&b.device(), 512).play(24_000, &[on(0, 69, 1)]);
    let high = out[2400..].iter().filter(|x| **x > 0.0).count() as f64 / (out.len() - 2400) as f64;
    assert!((high - 0.25).abs() < 0.01);
    // Phase modulation: a constant shifts the starting phase by half the
    // input in cycles.
    let mut b = Build::new(modular::INSTRUMENT, true);
    b.add("note_in", 1)
        .add("oscillator", 2)
        .add("arithmetic", 3)
        .add("audio_out", 4)
        .wire(1, "pitch", 2, "pitch")
        .wire(3, "out", 2, "pm")
        .wire(2, "out", 4, "left")
        .set(2, "wave", 0.0)
        .set(3, "op", 6.0)
        .set(3, "a", 0.5);
    let out = Rig::new(&b.device(), 512).play(100, &[on(0, 60, 1)]);
    assert!((out[0] - (TAU * 0.25).sin() as f32).abs() < 1e-5);
    // Band-limited: a saw high in the range keeps its energy at its
    // harmonics, not folded back between them.
    let mut b = sine(2.0);
    b.set(2, "coarse", 31.0);
    let out = Rig::new(&b.device(), 512).play(16_384 + 2400, &[on(0, 60, 1)]);
    let body = &out[2400..];
    let f0 = 261.625_565 * 2f64.powf(31.0 / 12.0);
    let spectrum = common::spectrum(body);
    let bin = HZ / body.len() as f64;
    let (mut total, mut aliased) = (0.0f64, 0.0f64);
    for (i, power) in spectrum.iter().enumerate().skip(2) {
        let hz = i as f64 * bin;
        total += power * power;
        let harmonic = (hz / f0).round().max(1.0) * f0;
        if (hz - harmonic).abs() > 4.0 * bin {
            aliased += power * power;
        }
    }
    assert!(
        10.0 * (aliased / total).log10() < -30.0,
        "{} dB",
        10.0 * (aliased / total).log10()
    );
}

// Spec: modular-device R22
#[test]
fn a_filter_cuts_above_its_cutoff_by_its_slope_and_follows_its_cutoff_input() {
    let level = |setup: &dyn Fn(&mut Build), hz: f64| {
        let mut b = Build::new(modular::EFFECT, true);
        b.add("audio_in", 1)
            .add("filter", 2)
            .add("audio_out", 3)
            .wire(1, "left", 2, "in")
            .wire(2, "out", 3, "left")
            .set(2, "cutoff", 1000.0)
            .set(2, "resonance", 0.0);
        setup(&mut b);
        let input = common::sine(hz, 1.0, 24_000);
        let [out, _] = Rig::new(&b.device(), 512).render(24_000, &[], &[], Some(&input), None);
        amplitude_at(&out[4800..], hz)
    };
    let nothing = |_: &mut Build| {};
    assert!(level(&nothing, 100.0) > 0.95);
    let twelve = level(&nothing, 8000.0);
    let twenty_four = level(
        &|b: &mut Build| {
            b.set(2, "slope", 1.0);
        },
        8000.0,
    );
    assert!(
        twelve < 0.03 && twenty_four < twelve * 0.2,
        "{twelve} {twenty_four}"
    );
    assert!(
        level(
            &|b: &mut Build| {
                b.set(2, "mode", 1.0);
            },
            100.0
        ) < 0.03,
        "high-pass"
    );
    assert!(
        level(
            &|b: &mut Build| {
                b.set(2, "mode", 1.0);
            },
            8000.0
        ) > 0.95
    );
    assert!(
        level(
            &|b: &mut Build| {
                b.set(2, "mode", 2.0);
            },
            1000.0
        ) > 0.5,
        "band-pass"
    );
    assert!(
        level(
            &|b: &mut Build| {
                b.set(2, "mode", 2.0);
            },
            100.0
        ) < 0.2
    );
    assert!(
        level(
            &|b: &mut Build| {
                b.set(2, "mode", 3.0);
            },
            1000.0
        ) < 0.1,
        "notch"
    );
    // Two octaves up on the cutoff input passes what 1 kHz stopped.
    let up = |b: &mut Build| {
        b.add("arithmetic", 4)
            .wire(4, "out", 2, "cutoff")
            .set(4, "op", 6.0)
            .set(4, "a", 3.0);
    };
    assert!(level(&nothing, 4000.0) < 0.1 && level(&up, 4000.0) > 0.8);
}

// Spec: modular-device R23
#[test]
fn an_envelope_attacks_decays_sustains_and_releases_from_where_it_is() {
    let mut b = Build::new(modular::INSTRUMENT, true);
    b.add("note_in", 1)
        .add("envelope", 2)
        .add("audio_out", 3)
        .wire(1, "gate", 2, "gate")
        .wire(2, "out", 3, "left")
        .set(2, "attack", 10.0)
        .set(2, "decay", 20.0)
        .set(2, "sustain", 0.5)
        .set(2, "release", 30.0);
    let out = Rig::new(&b.device(), 100).play(12_000, &[on(0, 60, 1), off(6000, 60, 1)]);
    assert!((out[240] - 0.5).abs() < 0.01, "half way up a 10 ms attack");
    assert!((out[480] - 1.0).abs() < 0.01 && (out[481] - 1.0).abs() < 0.01 && out[500] < 1.0);
    assert!((out[480 + 960] - 0.5).abs() < 0.01 && (out[3000] - 0.5).abs() < 1e-6);
    assert!(
        (out[6000 + 720] - 0.25).abs() < 0.01,
        "half way down a 30 ms release from 0.5"
    );
    assert!(out[6000 + 1500].abs() < 1e-6);
    // A note released mid-attack releases from where the attack had got.
    let out = Rig::new(&b.device(), 100).play(3000, &[on(0, 60, 1), off(240, 60, 1)]);
    assert!(out[239] > 0.4 && out[239] < 0.6 && out[240 + 720] < out[239] * 0.6);
    // A curve bends a segment: above 0 starts slowly.
    b.set(2, "attack_curve", 1.0);
    let out = Rig::new(&b.device(), 100).play(1000, &[on(0, 60, 1)]);
    assert!(
        out[240] < 0.2,
        "a convex attack is low half way: {}",
        out[240]
    );
}

// Spec: modular-device R24
#[test]
fn an_lfo_runs_at_its_rate_or_the_songs_tempo_and_resets_on_a_gate() {
    let mut b = Build::new(modular::EFFECT, true);
    b.add("lfo", 1)
        .add("audio_out", 2)
        .wire(1, "out", 2, "left")
        .set(1, "rate", 6.0);
    let [out, _] = Rig::new(&b.device(), 512).render(48_000, &[], &[], None, None);
    let crossings = |s: &[f32]| s.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    assert_eq!(crossings(&out), 6);
    b.set(1, "sync", 1.0).set(1, "division", 2.0);
    let [out, _] = Rig::new(&b.device(), 512).render(48_000, &[], &[], None, None);
    assert_eq!(crossings(&out), 2, "quarter notes at 120 BPM are 2 Hz");
    for wave in [1.0, 2.0, 3.0, 4.0] {
        b.set(1, "wave", wave);
        let [out, _] = Rig::new(&b.device(), 512).render(48_000, &[], &[], None, None);
        assert!(peak(&out) <= 1.0 && peak(&out) > 0.2, "wave {wave}");
    }
    // Random waves repeat from one render to the next.
    let a = Rig::new(&b.device(), 512).render(20_000, &[], &[], None, None);
    assert_eq!(
        a,
        Rig::new(&b.device(), 33).render(20_000, &[], &[], None, None)
    );
}

// Spec: modular-device R25, R26, R27
#[test]
fn an_amplifier_a_mixer_and_arithmetic_do_what_they_say() {
    let run = |b: &Build| Rig::new(&b.device(), 32).render(64, &[], &[], None, None)[0][63];
    let mut b = Build::new(modular::EFFECT, true);
    b.add("arithmetic", 1)
        .add("amplifier", 2)
        .add("audio_out", 3)
        .wire(1, "out", 2, "in")
        .wire(2, "out", 3, "left")
        .set(1, "op", 6.0)
        .set(1, "a", 0.5)
        .set(2, "gain", -6.0206);
    assert!(
        (run(&b) - 0.25).abs() < 1e-4,
        "gain in decibels; an open control input is 1"
    );
    b.add("arithmetic", 4)
        .wire(4, "out", 2, "cv")
        .set(4, "op", 6.0)
        .set(4, "a", 0.5);
    assert!((run(&b) - 0.125).abs() < 1e-4, "times a control input");
    // Mixer: four inputs, a level each.
    let mut m = Build::new(modular::EFFECT, true);
    m.add("mixer", 1)
        .add("audio_out", 2)
        .wire(1, "out", 2, "left");
    for (i, v) in [0.1, 0.2, 0.3, 0.4].into_iter().enumerate() {
        let id = 10 + i as u32;
        let port = format!("in{}", i + 1);
        m.add("arithmetic", id)
            .set(id, "op", 6.0)
            .set(id, "a", v)
            .wire(id, "out", 1, &port);
    }
    m.set(1, "level2", 0.5).set(1, "level4", 0.0);
    assert!((run(&m) - (0.1 + 0.1 + 0.3)).abs() < 1e-6);
    // Arithmetic: every operation.
    for (op, a, b_, want) in [
        (0.0, 0.5, 0.25, 0.75),
        (1.0, 0.5, 0.25, 0.25),
        (2.0, 0.5, 0.25, 0.125),
        (3.0, 0.5, 0.25, 0.25),
        (4.0, 0.5, 0.25, 0.5),
        (5.0, -0.5, 0.25, 0.5),
        (6.0, 0.5, 0.25, 0.5),
    ] {
        let mut c = Build::new(modular::EFFECT, true);
        c.add("arithmetic", 1)
            .add("audio_out", 2)
            .wire(1, "out", 2, "left")
            .set(1, "op", op)
            .set(1, "a", a)
            .set(1, "b", b_);
        assert!((run(&c) - want).abs() < 1e-6, "op {op}");
    }
}

// Spec: modular-device R10
#[test]
fn a_block_of_a_later_version_is_kept_and_silent() {
    let mut device = Build::new(modular::EFFECT, false).device();
    let patch = device.patch.as_mut().unwrap();
    patch.blocks.push(modular::Block {
        id: BlockId(7),
        kind: "from_the_future".to_owned(),
        x: 0.0,
        y: 0.0,
        settings: std::collections::BTreeMap::new(),
    });
    // It feeds Audio Out's right side in place of Audio In.
    patch
        .cables
        .retain(|c| !(c.to == BlockId(2) && c.to_port == "right"));
    patch
        .cables
        .push(Cable::of(BlockId(7), "anything", BlockId(2), "right"));
    assert!(patch.check().is_ok());
    let input = common::noise(512);
    let [left, right] = Rig::new(&device, 512).render(512, &[], &[], Some(&input), None);
    assert_eq!(left, input);
    assert_eq!(peak(&right), 0.0);
}

// Spec: modular-device R12
#[test]
fn modular_reports_no_latency_for_blocks_that_report_none() {
    let rig = Rig::new(&Build::new(modular::INSTRUMENT, false).device(), 64);
    assert_eq!(rig.node.latency(), 0);
}
