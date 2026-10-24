//! MPE MIDI out of a track's MIDI To (WP-6.17b; the MPE spec, R17, R19, R21,
//! and `docs/specs/external-devices.md` R3): a clip's two overlapping notes,
//! each with Pitch, Slide and Pressure envelopes, leave for a port laid out
//! on the member channels of a zone, message by message, on their frames in
//! any block size. Every block runs as a real-time section under the
//! allocation trap.

use crate::common;
use std::time::{Duration, Instant};

use bl_core::midi::{MidiOutputEvent, PortId};
use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::{NoInput, TransportCommand};
use bl_model::{
    AutomationTarget, Breakpoint, Clip, ClipData, Command, EnvelopeKind, MidiData, MidiTo, MpeMode,
    MpeSettings, MpeZone, Note, TrackKind,
};
use common::{Rig, track};
use rtrb::{Consumer, RingBuffer};

const BEAT: Ticks = Ticks(TICKS_PER_QUARTER);

/// At 120 BPM and 48 kHz a quarter note is 24,000 frames.
const BEAT_FRAMES: i64 = 24_000;

const BLOCKS: [usize; 3] = [1, 64, 4096];

/// A message and the song frame it leaves on.
type Sent = (i64, Vec<u8>);

fn point(at: Ticks, value: f64) -> Breakpoint {
    Breakpoint {
        at,
        value,
        bend: 0.0,
    }
}

/// Two overlapping notes: A, key 60, from the start for a beat and a half,
/// bending up an octave over its first half beat with a full slide and a
/// full pressure that drops to none as the bend arrives; and B, key 64, from
/// half a beat in for a beat, bent down an octave with no slide and half
/// pressure.
fn clip() -> MidiData {
    let half = BEAT / 2;
    let mut a = Note::new(60, Ticks::ZERO, BEAT + half, 100);
    a.pitch = vec![point(Ticks::ZERO, 0.0), point(half, 12.0)];
    a.slide = vec![point(Ticks::ZERO, 127.0)];
    a.pressure = vec![
        point(Ticks::ZERO, 127.0),
        point(half - Ticks(1), 127.0),
        point(half, 0.0),
    ];
    let mut b = Note::new(64, half, BEAT, 90);
    b.pitch = vec![point(Ticks::ZERO, -12.0)];
    b.slide = vec![point(Ticks::ZERO, 0.0)];
    b.pressure = vec![point(Ticks::ZERO, 63.5)];
    MidiData::new([a, b])
}

/// Plays the clip to `Synth`, with MPE settings `mpe`, in blocks of `block`
/// frames; `setup` may add to the rig first. Returns every message sent
/// within `frames` frames, with the song frame each is heard at.
fn play(
    block: usize,
    mpe: MpeSettings,
    frames: i64,
    setup: impl Fn(&mut Rig, bl_model::ClipId),
) -> Vec<Sent> {
    let mut rig = Rig::with_config(block, |config| config.output_latency = 0);
    let (producer, mut output): (_, Consumer<MidiOutputEvent>) = RingBuffer::new(1 << 16);
    assert!(rig.processor.set_midi_output(producer).is_none());
    let keys = track(&mut rig.editor, TrackKind::Midi, None);
    let id = rig.editor.new_id();
    rig.apply(Command::CreateClip {
        track: keys,
        clip: Clip::new(id, Ticks::ZERO, BEAT * 4, ClipData::Midi(clip())),
    });
    setup(&mut rig, id);
    rig.apply(Command::SetMidiOutput {
        track: keys,
        to: Some(MidiTo::Port {
            name: "Synth".to_owned(),
            channel: 3,
            mpe: Some(mpe),
        }),
    });
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    let start = Instant::now();
    let mut sent = Vec::new();
    let mut played = 0i64;
    while played < frames {
        let n = (frames - played).min(block as i64) as usize;
        let now = start + Duration::from_nanos((played as u64) * 1_000_000_000 / 48_000);
        let processor = &mut rig.processor;
        bl_rt::rt_test!({
            processor.process_at(n, &NoInput, now);
        });
        played += n as i64;
        while let Ok(event) = output.pop() {
            assert_eq!(event.port, PortId::from_name("Synth"));
            let nanos = event.time.duration_since(start).as_nanos() as i64;
            let frame = (nanos * 48_000 + 500_000_000) / 1_000_000_000;
            sent.push((frame, event.message.bytes().to_vec()));
        }
    }
    sent
}

/// Runs a scenario in every block size, which must send the same, and
/// returns that.
fn in_every_block_size(
    mpe: MpeSettings,
    frames: i64,
    setup: impl Fn(&mut Rig, bl_model::ClipId) + Copy,
) -> Vec<Sent> {
    let first = play(BLOCKS[0], mpe, frames, setup);
    for block in &BLOCKS[1..] {
        // Not assert_eq: a mismatch would print thousands of messages.
        assert!(
            play(*block, mpe, frames, setup) == first,
            "block size {block} sends other than block size {}",
            BLOCKS[0]
        );
    }
    first
}

fn nothing(_: &mut Rig, _: bl_model::ClipId) {}

fn at(sent: &[Sent], frame: i64) -> Vec<&[u8]> {
    sent.iter()
        .filter(|(f, _)| *f == frame)
        .map(|(_, m)| m.as_slice())
        .collect()
}

// Spec: mpe R17, R19
#[test]
fn the_lower_zone_opens_first_and_each_note_takes_a_member_channel_with_its_starting_values() {
    let sent = in_every_block_size(MpeSettings::default(), 2 * BEAT_FRAMES, nothing);

    // At the first frame, before any note: the configuration message on
    // channel 1 (15 note channels), then a sensitivity of 48 semitones on
    // channels 2 to 16.
    let opening = at(&sent, 0);
    assert_eq!(
        opening[..5],
        [
            &[0xB0, 101, 0][..],
            &[0xB0, 100, 6],
            &[0xB0, 6, 15],
            &[0xB0, 101, 127],
            &[0xB0, 100, 127],
        ]
    );
    for (n, channel) in (1u8..16).enumerate() {
        let from = 5 + n * 6;
        assert_eq!(
            opening[from..from + 6],
            [
                &[0xB0 | channel, 101, 0][..],
                &[0xB0 | channel, 100, 0],
                &[0xB0 | channel, 6, 48],
                &[0xB0 | channel, 38, 0],
                &[0xB0 | channel, 101, 127],
                &[0xB0 | channel, 100, 127],
            ],
            "channel {}",
            channel + 1
        );
    }

    // Then A, on channel 2: its starting bend (the centre), slide and
    // pressure, each before its note-on.
    assert_eq!(
        opening[95..],
        [
            &[0xE1, 0, 64][..],
            &[0xB1, 74, 127],
            &[0xD1, 127],
            &[0x91, 60, 100],
        ]
    );
    // Its bend follows as a stream of 14-bit values at least every 5 ms,
    // rising to +12 of 48 semitones, 10,240, by the half beat.
    let bends: Vec<&Sent> = sent
        .iter()
        .filter(|(frame, m)| m[0] == 0xE1 && *frame > 0)
        .collect();
    let value = |m: &[u8]| i32::from(m[1]) | i32::from(m[2]) << 7;
    assert!(bends.windows(2).all(|w| value(&w[0].1) < value(&w[1].1)));
    assert!(bends.windows(2).all(|w| w[1].0 - w[0].0 <= 240));
    assert_eq!(
        bends.last().map(|(f, m)| (*f, value(m))),
        Some((BEAT_FRAMES / 2, 10_240))
    );
    // At the half beat its pressure drops to none and B starts on channel 3:
    // down an octave (6,144), no slide, half pressure, then its note-on.
    let half = at(&sent, BEAT_FRAMES / 2);
    assert!(half.contains(&&[0xD1, 0][..]));
    let from = half.iter().position(|m| m[0] == 0xE2).unwrap();
    assert_eq!(
        half[from..from + 4],
        [
            &[0xE2, 0, 48][..],
            &[0xB2, 74, 0],
            &[0xD2, 64],
            &[0x92, 64, 90],
        ]
    );
    // Both notes end on their own channels, with their release velocity,
    // and nothing is sent after.
    let end = at(&sent, BEAT_FRAMES * 3 / 2);
    assert_eq!(end.len(), 2);
    assert!(end.contains(&&[0x81, 60, 64][..]));
    assert!(end.contains(&&[0x82, 64, 64][..]));
    assert!(sent.iter().all(|(frame, _)| *frame <= BEAT_FRAMES * 3 / 2));
}

// Spec: mpe R17, R18
#[test]
fn the_upper_zone_has_its_master_on_channel_16_and_its_notes_run_down_from_15() {
    let upper = MpeSettings {
        zone: MpeZone::Upper,
        channels: 3,
        ..MpeSettings::default()
    };
    let sent = in_every_block_size(upper, BEAT_FRAMES, nothing);
    let opening = at(&sent, 0);
    assert_eq!(
        opening[..5],
        [
            &[0xBF, 101, 0][..],
            &[0xBF, 100, 6],
            &[0xBF, 6, 3],
            &[0xBF, 101, 127],
            &[0xBF, 100, 127],
        ]
    );
    // The sensitivities are on channels 15, 14 and 13.
    let channels: Vec<u8> = opening[5..]
        .iter()
        .filter(|m| m[1] == 6)
        .map(|m| m[0] & 0x0F)
        .collect();
    assert_eq!(channels, [14, 13, 12]);
    // A takes channel 15 (index 14).
    assert!(opening.contains(&&[0x9E, 60, 100][..]));
    assert!(opening.contains(&&[0xEE, 0, 64][..]));
}

// Spec: mpe R17, R18
#[test]
fn multi_channel_mode_sends_notes_on_its_range_and_no_configuration_message() {
    let multi = MpeSettings {
        mode: MpeMode::MultiChannel,
        first: 5,
        last: 7,
        ..MpeSettings::default()
    };
    let sent = in_every_block_size(multi, BEAT_FRAMES, nothing);
    let opening = at(&sent, 0);
    // No configuration message (registered parameter 6) anywhere.
    assert!(
        sent.iter()
            .all(|(_, m)| !(m[0] & 0xF0 == 0xB0 && m[1] == 100 && m[2] == 6))
    );
    // Sensitivities on channels 5, 6 and 7 only.
    let channels: Vec<u8> = opening
        .iter()
        .filter(|m| m[1] == 6)
        .map(|m| m[0] & 0x0F)
        .collect();
    assert_eq!(channels, [4, 5, 6]);
    // A takes the first channel of the range.
    assert!(opening.contains(&&[0x94, 60, 100][..]));
}

// Spec: mpe R17
#[test]
fn the_tracks_controllers_go_on_the_master_channel_or_every_channel_of_a_range() {
    let controller = |rig: &mut Rig, clip: bl_model::ClipId| {
        for (at, value) in [(BEAT, 10.0), (BEAT * 2 - Ticks(1), 10.0), (BEAT * 2, 90.0)] {
            rig.apply(Command::SetClipEnvelopeBreakpoint {
                clip,
                kind: EnvelopeKind::Modulation,
                target: AutomationTarget::MidiCc { number: 1 },
                at,
                value,
            });
        }
    };
    let cc = |sent: &[Sent], frame: i64| -> Vec<Vec<u8>> {
        at(sent, frame)
            .into_iter()
            .filter(|m| m[0] & 0xF0 == 0xB0 && m[1] == 1)
            .map(<[u8]>::to_vec)
            .collect()
    };
    let lower = in_every_block_size(MpeSettings::default(), 3 * BEAT_FRAMES, controller);
    assert_eq!(cc(&lower, BEAT_FRAMES), [vec![0xB0, 1, 10]]);
    assert_eq!(cc(&lower, 2 * BEAT_FRAMES), [vec![0xB0, 1, 90]]);
    let upper = MpeSettings {
        zone: MpeZone::Upper,
        ..MpeSettings::default()
    };
    let upper = in_every_block_size(upper, 3 * BEAT_FRAMES, controller);
    assert_eq!(cc(&upper, BEAT_FRAMES), [vec![0xBF, 1, 10]]);
    let multi = MpeSettings {
        mode: MpeMode::MultiChannel,
        first: 3,
        last: 5,
        ..MpeSettings::default()
    };
    let multi = in_every_block_size(multi, 3 * BEAT_FRAMES, controller);
    assert_eq!(
        cc(&multi, BEAT_FRAMES),
        [vec![0xB2, 1, 10], vec![0xB3, 1, 10], vec![0xB4, 1, 10]]
    );
}
