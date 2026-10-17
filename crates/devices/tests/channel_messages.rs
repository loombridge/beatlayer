//! Channel messages between devices (WP-5.44; the device framework spec,
//! R39): a control change, pitch bend or channel pressure one device sends
//! reaches the next on its frame, in order with the notes, through a MIDI
//! effect and a switched-off device, and a block of hundreds allocates
//! nothing. Every block is a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_devices::lift::{LIFT, PITCH};
use bl_devices::{Audio, NativeDevice, ParamBank, add};
use bl_engine::{DeviceType, NoteEvent, NoteKind};
use bl_model::{Command, DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};
use common::chain::{BEAT_FRAMES, Chain, note};

const SIZES: [usize; 4] = [1, 100, 512, 4096];

const SENDER: &str = "test.channel_messages.sender";
const COUNT: ParamId = ParamId(0);

/// After each note-on it forwards, a test MIDI effect sends `Count` channel
/// messages on the same frame: a control change, a pitch bend, a channel
/// pressure, and again.
struct Sender;

impl NativeDevice for Sender {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let count = params.value(params.index(COUNT).unwrap()) as usize;
        for note in audio.notes() {
            audio.send(note);
            if note.kind == NoteKind::On {
                for i in 0..count {
                    audio.send(NoteEvent {
                        kind: message(i),
                        ..note
                    });
                }
            }
        }
    }
}

/// The `i`th message the sender sends.
fn message(i: usize) -> NoteKind {
    let value = (i % 128) as f32 / 127.0;
    match i % 3 {
        0 => NoteKind::Control {
            controller: (i % 128) as u8,
            value,
        },
        1 => NoteKind::PitchBend { value },
        _ => NoteKind::ChannelPressure { value },
    }
}

fn register(registry: &mut bl_engine::Registry) {
    let device_type = DeviceType::native(SENDER, "Channel Message Sender", DeviceRole::MidiEffect)
        .with_description(DeviceDescription::new(vec![
            ParamDescriptor::integer(COUNT, "Count", (0, 1000), 3, ParamUnit::None)
                .describe("How many messages follow each note-on."),
        ]));
    add(registry, device_type, |_, _| Sender);
}

fn chain(size: usize) -> Chain {
    Chain::with_registry(size.max(512), register)
}

fn kinds(chain: &Chain) -> Vec<(i64, NoteKind)> {
    chain.heard().iter().map(|h| (h.0, h.1)).collect()
}

// Spec: device-framework R39. A message a device sends reaches the next
// device on its frame, behind the note it followed; a MIDI effect that
// doesn't change it forwards it.
#[test]
fn a_message_one_device_sends_reaches_the_next_on_its_frame() {
    for size in SIZES {
        let mut chain = chain(size);
        chain.effect(SENDER, &[]);
        chain.effect(LIFT, &[(PITCH, 7.0)]);
        chain.clip(4, [note(60, 0, 1), note(64, 2, 1)]);
        chain.sync();
        chain.play();
        chain.run(4 * BEAT_FRAMES as usize, size);
        let heard = kinds(&chain);
        // A note-on, then the three messages sent behind it.
        let on = |frame| {
            let mut kinds = vec![(frame, NoteKind::On)];
            kinds.extend((0..3).map(|i| (frame, message(i))));
            kinds
        };
        let mut expected = on(0);
        expected.push((BEAT_FRAMES, NoteKind::Off));
        expected.extend(on(2 * BEAT_FRAMES));
        expected.push((3 * BEAT_FRAMES, NoteKind::Off));
        assert_eq!(heard, expected, "block size {size}");
    }
}

// Spec: device-framework R39, device-chains R6. A device switched off passes
// the messages it receives to the next unchanged, as it does its notes.
#[test]
fn a_switched_off_device_passes_messages_through() {
    for size in SIZES {
        let mut chain = chain(size);
        chain.effect(SENDER, &[]);
        let lift = chain.effect(LIFT, &[(PITCH, 7.0)]);
        chain.clip(2, [note(60, 0, 1)]);
        chain.sync();
        chain.apply(Command::SetDeviceEnabled {
            device: lift,
            enabled: false,
        });
        chain.play();
        chain.run(2 * BEAT_FRAMES as usize, size);
        let heard = chain.heard();
        let messages: Vec<_> = heard.iter().filter(|h| h.1.is_message()).collect();
        assert_eq!(messages.len(), 3, "block size {size}: {heard:?}");
        assert!(messages.iter().all(|h| h.0 == 0), "{heard:?}");
        assert_eq!(heard[0].2, 60, "the note isn't lifted while Lift is off");
    }
}

// Spec: device-framework R30, R39. A block of several hundred messages goes
// through a chain without allocating, in the order they were sent.
#[test]
fn several_hundred_messages_in_one_block_allocate_nothing_and_keep_their_order() {
    for size in SIZES {
        let mut chain = chain(size);
        chain.effect(SENDER, &[(COUNT, 600.0)]);
        chain.effect(LIFT, &[(PITCH, 0.0)]);
        chain.clip(2, [note(60, 0, 1)]);
        chain.sync();
        chain.play();
        chain.run(2 * BEAT_FRAMES as usize, size);
        let heard = kinds(&chain);
        let sent: Vec<_> = heard.iter().filter(|h| h.1.is_message()).collect();
        assert_eq!(sent.len(), 600, "block size {size}");
        for (i, &&(frame, kind)) in sent.iter().enumerate() {
            assert_eq!((frame, kind), (0, message(i)), "message {i}, size {size}");
        }
    }
}
