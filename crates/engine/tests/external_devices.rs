//! The external instrument in the engine (WP-5.34, `docs/specs/external-
//! devices.md` R9 to R13): its notes leave for its port on its channel at
//! the moment their frame would reach the output, without the compensation
//! the track's audio gets, and the sound it plays is the interface's inputs
//! it is set to. Every block runs as a real-time section under the
//! allocation trap.

use crate::common;
use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiOutputEvent, MidiSource, PortId};
use bl_engine::testing::{DELAY, DELAY_FRAMES, EXTERNAL};
use bl_engine::{DeviceInput, NoInput};
use bl_model::{
    Channel, Command, Device, DeviceKind, DeviceRole, External, HardwareLatency, Input, Monitor,
    TrackId, TrackKind,
};
use common::{Rig, device, track};
use rtrb::{Consumer, Producer, RingBuffer};

fn frames(frames: u64) -> Duration {
    Duration::from_nanos(frames * 1_000_000_000 / 48_000)
}

/// Puts the test external instrument on `track`, set as `external` says.
fn instrument(rig: &mut Rig, track: TrackId, external: External) {
    let id = rig.editor.new_id();
    let mut device = Device::new(
        id,
        "Hardware",
        DeviceKind::Native {
            device_type: EXTERNAL.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument)
    .with_external();
    device.external = Some(external);
    rig.editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
}

/// An interface whose input channel `n` (counted from 1) holds the constant
/// `n / 10`.
struct Steady(Vec<Vec<f32>>);

impl Steady {
    fn new(channels: usize, frames: usize) -> Self {
        Self(
            (0..channels)
                .map(|c| vec![(c + 1) as f32 / 10.0; frames])
                .collect(),
        )
    }
}

impl DeviceInput for Steady {
    fn channels(&self) -> usize {
        self.0.len()
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.0[index]
    }
}

// Spec: external-devices R11
#[test]
fn the_return_is_the_chosen_inputs() {
    for (audio_from, left, right) in [
        // A pair as left and right.
        (Channel::Pair(3), 0.3, 0.4),
        (Channel::Pair(1), 0.1, 0.2),
        // A single channel on both sides.
        (Channel::Mono(2), 0.2, 0.2),
        // A channel the interface lacks gives silence: the second of the
        // pair 4/5, a channel past the four.
        (Channel::Pair(4), 0.4, 0.0),
        (Channel::Mono(9), 0.0, 0.0),
    ] {
        let mut rig = Rig::with_config(64, |config| config.input_channels = 4);
        let keys = track(&mut rig.editor, TrackKind::Midi, None);
        instrument(
            &mut rig,
            keys,
            External {
                audio_from,
                ..External::default()
            },
        );
        rig.sync();
        let input = Steady::new(4, 64);
        for _ in 0..3 {
            rig.block_with_input(64, &input);
        }
        let output = rig.processor.output();
        assert_eq!(
            (output.channel(0)[63], output.channel(1)[63]),
            (left, right),
            "{audio_from:?}"
        );
    }
    // No audio device at all: silence.
    let mut rig = Rig::new(64);
    let keys = track(&mut rig.editor, TrackKind::Midi, None);
    instrument(&mut rig, keys, External::default());
    rig.sync();
    rig.block(64);
    assert_eq!(rig.processor.output().channel(0)[63], 0.0);
}

/// A rig with a MIDI queue each way, a keys track on the keyboard that
/// sends its notes to the external instrument, set to the port `Hardware`
/// on channel 2, and `slow`, a track through a device that adds frames.
struct Notes {
    rig: Rig,
    input: Producer<MidiInputEvent>,
    output: Consumer<MidiOutputEvent>,
    start: Instant,
    blocks: u64,
}

impl Notes {
    const BLOCK: usize = 100;
    const LATENCY: u32 = 300;

    fn new(slow: u32) -> Self {
        let mut rig = Rig::with_config(Self::BLOCK, |config| {
            config.output_latency = Self::LATENCY;
            config.round_trip_latency = 700;
        });
        let (input, consumer) = RingBuffer::new(256);
        rig.processor.set_midi_input(consumer);
        let (producer, output) = RingBuffer::new(256);
        rig.processor.set_midi_output(producer);
        let keys = track(&mut rig.editor, TrackKind::Midi, None);
        instrument(
            &mut rig,
            keys,
            External {
                port: Some("Hardware".to_owned()),
                channel: 2,
                hardware_latency: HardwareLatency::Samples { value: 50 },
                ..External::default()
            },
        );
        if slow > 0 {
            let other = track(&mut rig.editor, TrackKind::Audio, None);
            device(
                &mut rig.editor,
                other,
                DELAY,
                &[(DELAY_FRAMES, f64::from(slow))],
            );
        }
        rig.sync();
        rig.apply(Command::SetInput {
            track: keys,
            input: Input::ComputerKeyboard,
            channel: None,
        });
        rig.apply(Command::SetMonitor {
            track: keys,
            monitor: Monitor::In,
        });
        Self {
            rig,
            input,
            output,
            start: Instant::now(),
            blocks: 0,
        }
    }

    fn run(&mut self) {
        let now = self.start + frames(self.blocks * Self::BLOCK as u64);
        let processor = &mut self.rig.processor;
        bl_rt::rt_test!({
            processor.process_at(Self::BLOCK, &NoInput, now);
        });
        self.blocks += 1;
    }
}

// Spec: external-devices R10
#[test]
fn the_instruments_notes_leave_when_their_frame_reaches_the_output() {
    // With a track whose audio is held back 4,000 frames, and without: the
    // instrument's notes leave 300 frames after their block's frame, at the
    // output, and the track's compensation doesn't add to it.
    for slow in [0, 4000] {
        let mut notes = Notes::new(slow);
        notes.run();
        notes.run();
        let arrival = notes.start + frames(150);
        notes
            .input
            .push(MidiInputEvent {
                source: MidiSource::ComputerKeyboard,
                time: arrival,
                message: MidiMessage::note_on(0, 60, 100),
            })
            .unwrap();
        for _ in 0..3 {
            notes.run();
        }
        let event = notes.output.pop().expect("the note left");
        assert!(notes.output.pop().is_err());
        assert_eq!(event.port, PortId::from_name("Hardware"));
        assert_eq!(event.message.padded(), [0x91, 60, 100], "channel 2");
        // One block after it arrived, and the output latency after that.
        let expected = arrival + frames(Notes::BLOCK as u64 + u64::from(Notes::LATENCY));
        let error = event.time.max(expected) - event.time.min(expected);
        assert!(
            error <= frames(1) + Duration::from_micros(1),
            "compensation {slow}: {error:?}"
        );
    }
}

// Spec: external-devices R10, R6
#[test]
fn the_instrument_ends_its_notes_when_its_device_goes() {
    let mut notes = Notes::new(0);
    notes.run();
    notes.run();
    notes
        .input
        .push(MidiInputEvent {
            source: MidiSource::ComputerKeyboard,
            time: notes.start + frames(150),
            message: MidiMessage::note_on(0, 60, 100),
        })
        .unwrap();
    notes.run();
    notes.run();
    notes.run();
    assert_eq!(
        notes.output.pop().unwrap().message.padded(),
        [0x91, 60, 100]
    );
    let device = notes
        .rig
        .editor
        .document()
        .tracks()
        .flat_map(|t| t.devices_deep())
        .find(|d| d.external.is_some())
        .unwrap()
        .id;
    notes.rig.apply(Command::SetExternal {
        device,
        midi_to: Some(bl_model::ExternalPort::Port {
            name: "Elsewhere".to_owned(),
        }),
        channel: None,
        audio_from: None,
        audio_to: None,
        hardware_latency: None,
    });
    notes.run();
    notes.run();
    let off = notes.output.pop().expect("the note ends where it began");
    assert_eq!(off.port, PortId::from_name("Hardware"));
    assert_eq!(off.message.padded(), [0x81, 60, 64]);
    assert!(notes.output.pop().is_err());
}

// Spec: external-devices R6, device-framework R32
#[test]
fn the_instrument_ends_its_notes_when_it_is_switched_off() {
    let mut notes = Notes::new(0);
    notes.run();
    notes.run();
    notes
        .input
        .push(MidiInputEvent {
            source: MidiSource::ComputerKeyboard,
            time: notes.start + frames(150),
            message: MidiMessage::note_on(0, 60, 100),
        })
        .unwrap();
    notes.run();
    notes.run();
    notes.run();
    assert_eq!(
        notes.output.pop().unwrap().message.padded(),
        [0x91, 60, 100]
    );
    let device = notes
        .rig
        .editor
        .document()
        .tracks()
        .flat_map(|t| t.devices_deep())
        .find(|d| d.external.is_some())
        .unwrap()
        .id;
    notes.rig.apply(Command::SetDeviceEnabled {
        device,
        enabled: false,
    });
    notes.run();
    notes.run();
    let off = notes
        .output
        .pop()
        .expect("the note ends when the device goes off");
    assert_eq!(off.port, PortId::from_name("Hardware"));
    assert_eq!(off.message.padded(), [0x81, 60, 64]);
    notes.run();
    assert!(notes.output.pop().is_err(), "it is ended once");
}
