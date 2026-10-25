//! The meters between the devices of a chain (WP-6.69,
//! `docs/specs/midi-meters.md` R1 to R7): every device boundary publishes
//! what passed it, MIDI before the instrument and audio after it, at any
//! block size, with every block a real-time section under the allocation
//! trap.

use crate::common;

use std::time::{Duration, Instant};

use bl_core::clock::TestClock;
use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
use bl_engine::testing::{self, SEMITONES, VALUE};
use bl_engine::{ChainMeter, MeterOwner, MeterSite, NoInput, PointReading, TransportCommand};
use bl_model::{ChainId, Command, Device, DeviceId, DeviceKind, DeviceRole, Monitor};
use bl_model::{RackKind, TrackId, TrackKind};
use common::{Rig, in_chain, rack, track};
use rtrb::{Producer, RingBuffer};

/// The block sizes every test runs at.
const SIZES: [usize; 3] = [1, 64, 512];

/// The frame every size reaches whole blocks at, plus a few frames into one.
const AT: u64 = 8192 + 7;

/// A rig playing from frame 0, with a MIDI queue and exact block times.
struct Play {
    rig: Rig,
    queue: Producer<MidiInputEvent>,
    start: Instant,
    size: usize,
    blocks: u64,
}

impl Play {
    fn new(size: usize) -> Self {
        let mut rig = Rig::new(size);
        let (queue, consumer) = RingBuffer::new(4096);
        rig.processor.set_midi_input(consumer);
        Self {
            rig,
            queue,
            start: TestClock::new().now(),
            size,
            blocks: 0,
        }
    }

    /// A MIDI track that monitors its input, holding a transposing MIDI
    /// effect, a DC instrument of 0.5 and a through audio effect.
    fn chain(&mut self) -> (TrackId, [DeviceId; 3]) {
        let id = track(&mut self.rig.editor, TrackKind::Midi, None);
        let effect = self.insert(id, testing::TRANSPOSE, DeviceRole::MidiEffect);
        let source = self.insert(id, testing::DC, DeviceRole::Instrument);
        let through = self.insert(id, testing::THROUGH, DeviceRole::AudioEffect);
        self.rig
            .editor
            .apply(Command::SetDeviceParam {
                device: source,
                param: VALUE,
                value: 0.5,
            })
            .unwrap();
        self.rig
            .editor
            .apply(Command::SetDeviceParam {
                device: effect,
                param: SEMITONES,
                value: 0.0,
            })
            .unwrap();
        self.rig.apply(Command::SetMonitor {
            track: id,
            monitor: Monitor::In,
        });
        (id, [effect, source, through])
    }

    fn insert(&mut self, track: TrackId, device_type: &str, role: DeviceRole) -> DeviceId {
        let id = self.rig.editor.new_id();
        let device = Device::new(
            id,
            "",
            DeviceKind::Native {
                device_type: device_type.to_owned(),
            },
        )
        .with_role(role);
        let index = self
            .rig
            .editor
            .document()
            .track(track)
            .unwrap()
            .devices()
            .len();
        self.rig
            .editor
            .apply(Command::InsertDevice {
                track,
                index,
                device,
            })
            .unwrap();
        id
    }

    fn play(&mut self) {
        self.rig.sender.transport(TransportCommand::Play);
        self.block();
    }

    fn block_time(&self, n: u64) -> Instant {
        let frames = n * self.size as u64;
        self.start + Duration::from_nanos(frames * 1_000_000_000 / 48_000)
    }

    fn block(&mut self) {
        let now = self.block_time(self.blocks);
        let processor = &mut self.rig.processor;
        bl_rt::rt_test!({
            processor.process_at(self.size, &NoInput, now);
        });
        self.blocks += 1;
    }

    /// Processes blocks until the one that holds `frame` has run.
    fn run_to(&mut self, frame: u64) {
        while self.blocks * (self.size as u64) <= frame {
            self.block();
        }
    }

    fn send(&mut self, frame: u64, bytes: &[u8]) {
        let size = self.size as u64;
        assert!(frame >= size, "frame {frame} is in the first block");
        let time = self.block_time(frame / size - 1)
            + Duration::from_nanos((frame % size) * 1_000_000_000 / 48_000 + 100);
        self.queue
            .push(MidiInputEvent {
                source: MidiSource::ComputerKeyboard,
                time,
                message: MidiMessage::from_bytes(bytes).unwrap(),
            })
            .unwrap();
    }

    fn meter(&mut self, owner: MeterOwner, at: MeterSite) -> ChainMeter {
        *self
            .rig
            .monitor
            .state()
            .chain_meter(owner, at)
            .unwrap_or_else(|| panic!("no meter at {owner:?} {at:?}"))
    }

    /// The MIDI meter at `at`: its velocity, channel and expression levels.
    fn midi(&mut self, owner: MeterOwner, at: MeterSite) -> [f32; 3] {
        match self.meter(owner, at).reading {
            PointReading::Midi {
                velocity,
                channel,
                expression,
            } => [velocity, channel, expression],
            other => panic!("{at:?} is not a MIDI meter: {other:?}"),
        }
    }

    /// The level meter at `at`.
    fn audio(&mut self, owner: MeterOwner, at: MeterSite) -> [f32; 2] {
        match self.meter(owner, at).reading {
            PointReading::Audio { peak } => peak,
            other => panic!("{at:?} is not a level meter: {other:?}"),
        }
    }
}

const NOTE_ON: [u8; 3] = [0x90, 60, 100];
const CONTROLLER: [u8; 3] = [0xB0, 1, 64];
const PRESSURE: [u8; 3] = [0xA0, 60, 127];

// Spec: midi-meters R6, R7
#[test]
fn the_points_before_the_instrument_publish_what_passed_and_those_after_it_the_audio() {
    for size in SIZES {
        let mut play = Play::new(size);
        let (id, [effect, source, through]) = play.chain();
        let owner = MeterOwner::Track(id);
        play.play();
        // A block with nothing in it publishes nothing lit.
        for at in [effect, source] {
            assert_eq!(play.midi(owner, MeterSite::Before(at)), [0.0; 3]);
        }
        play.send(AT, &NOTE_ON);
        play.send(AT, &CONTROLLER);
        play.send(AT, &PRESSURE);
        play.run_to(AT);
        let velocity = 100.0 / 127.0;
        for at in [effect, source] {
            let lit = play.midi(owner, MeterSite::Before(at));
            assert!((lit[0] - velocity).abs() < 1e-6, "size {size}: {lit:?}");
            assert_eq!(lit[1..], [1.0, 1.0], "size {size}");
        }
        for at in [MeterSite::Before(through), MeterSite::End] {
            assert_eq!(play.audio(owner, at), [0.5; 2], "size {size}");
        }
    }
}

// Spec: midi-meters R3, R4, R5
#[test]
fn a_lit_meter_falls_to_dark_over_300_ms_and_note_offs_light_nothing() {
    for size in SIZES {
        let mut play = Play::new(size);
        let (id, [effect, ..]) = play.chain();
        let owner = MeterOwner::Track(id);
        let at = MeterSite::Before(effect);
        play.play();
        play.send(4096 + 3, &[0x80, 60, 64]);
        play.send(AT, &NOTE_ON);
        play.run_to(4096 + 3);
        assert_eq!(play.midi(owner, at), [0.0; 3], "size {size}: a note-off");
        play.run_to(AT);
        let lit = play.midi(owner, at)[0];
        assert!(lit > 0.78, "size {size}: {lit}");
        // 150 ms on, the meter has fallen about half way.
        play.run_to(AT + 7200 + size as u64);
        let half = play.midi(owner, at);
        assert!(half[0] > 0.2 && half[0] < 0.5, "size {size}: {half:?}");
        // 300 ms on, it's dark.
        play.run_to(AT + 14400 + size as u64);
        assert_eq!(play.midi(owner, at), [0.0; 3], "size {size}");
    }
}

// Spec: midi-meters R1
#[test]
fn a_midi_track_with_no_instrument_has_a_midi_meter_at_its_end_until_one_is_added() {
    let mut play = Play::new(64);
    let id = track(&mut play.rig.editor, TrackKind::Midi, None);
    play.rig.apply(Command::SetMonitor {
        track: id,
        monitor: Monitor::In,
    });
    let owner = MeterOwner::Track(id);
    play.play();
    play.send(AT, &NOTE_ON);
    play.run_to(AT);
    assert!(play.midi(owner, MeterSite::End)[0] > 0.78);
    let source = play.insert(id, testing::DC, DeviceRole::Instrument);
    play.rig.sync();
    play.block();
    assert_eq!(play.audio(owner, MeterSite::End), [0.0; 2]);
    assert_eq!(play.midi(owner, MeterSite::Before(source)), [0.0; 3]);
}

// Spec: midi-meters R2
#[test]
fn an_instrument_racks_chains_have_their_own_meters() {
    let mut play = Play::new(64);
    let id = track(&mut play.rig.editor, TrackKind::Midi, None);
    let (rack, chains) = rack(&mut play.rig.editor, id, RackKind::Instrument, 1);
    let chain: ChainId = chains[0];
    let effect = in_chain(
        &mut play.rig.editor,
        chain,
        testing::TRANSPOSE,
        DeviceRole::MidiEffect,
        &[(SEMITONES, 0.0)],
    );
    let source = in_chain(
        &mut play.rig.editor,
        chain,
        testing::DC,
        DeviceRole::Instrument,
        &[(VALUE, 0.5)],
    );
    play.rig.apply(Command::SetMonitor {
        track: id,
        monitor: Monitor::In,
    });
    play.play();
    play.send(AT, &NOTE_ON);
    play.run_to(AT);
    let track_owner = MeterOwner::Track(id);
    assert!(play.midi(track_owner, MeterSite::Before(rack))[0] > 0.78);
    assert_eq!(play.audio(track_owner, MeterSite::End), [0.5; 2]);
    let inside = MeterOwner::Chain(chain);
    assert!(play.midi(inside, MeterSite::Before(effect))[0] > 0.78);
    assert!(play.midi(inside, MeterSite::Before(source))[0] > 0.78);
    assert_eq!(play.audio(inside, MeterSite::End), [0.5; 2]);
}

// Spec: midi-meters R2
#[test]
fn an_audio_tracks_chain_has_level_meters_only() {
    let mut play = Play::new(64);
    let id = track(&mut play.rig.editor, TrackKind::Audio, None);
    let source = play.insert(id, testing::DC, DeviceRole::AudioEffect);
    play.rig
        .editor
        .apply(Command::SetDeviceParam {
            device: source,
            param: VALUE,
            value: 0.25,
        })
        .unwrap();
    play.rig.sync();
    play.play();
    let owner = MeterOwner::Track(id);
    assert_eq!(play.audio(owner, MeterSite::Before(source)), [0.0; 2]);
    assert_eq!(play.audio(owner, MeterSite::End), [0.25; 2]);
}
