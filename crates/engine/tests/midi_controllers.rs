//! MIDI controller data in clips (WP-2.29): a MIDI clip's controller
//! envelopes and its notes' pressure reach the track's devices as channel
//! messages and polyphonic pressure, on their exact frames, in any block
//! size. Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::{NoteExpression, NoteKind, TransportCommand, testing};
use bl_model::{
    AutomationTarget, Breakpoint, Clip, ClipData, ClipId, Command, EnvelopeKind, MidiData, Note,
    TrackId, TrackKind,
};
use common::{Rig, device, track};

const BEAT: Ticks = Ticks(TICKS_PER_QUARTER);
const BAR: Ticks = Ticks(TICKS_PER_QUARTER * 4);

/// At 120 BPM and 48 kHz a quarter note is 24,000 frames.
const BEAT_FRAMES: i64 = 24_000;

/// The block sizes every scenario runs in.
const BLOCKS: [usize; 4] = [1, 100, 512, 4096];

fn cc(number: u8) -> AutomationTarget {
    AutomationTarget::MidiCc { number }
}

/// What a test instrument received: the song frame, what it was and the key.
type Heard = (i64, NoteKind, u8);

/// A rig with a MIDI track holding a test instrument and one 4-bar clip of
/// `notes`, on which `setup` may put envelopes; plays `frames` frames in
/// blocks of `block` and returns what the instrument heard. `stop_at` stops
/// the transport after that many frames and plays on for as many again.
fn run(
    block: usize,
    midi: MidiData,
    setup: impl Fn(&mut Rig, TrackId, ClipId),
    frames: usize,
    stop_at: Option<usize>,
) -> Vec<Heard> {
    let mut rig = Rig::new(block);
    let trk = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, trk, testing::INSTRUMENT, &[]);
    let clip = rig.editor.new_id();
    rig.apply(Command::CreateClip {
        track: trk,
        clip: Clip::new(clip, Ticks::ZERO, BAR * 4, ClipData::Midi(midi)),
    });
    setup(&mut rig, trk, clip);
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    let play = |rig: &mut Rig, frames: usize| {
        let mut left = frames;
        while left > 0 {
            let n = left.min(block);
            rig.processor.process(n);
            left -= n;
        }
    };
    match stop_at {
        Some(at) => {
            play(&mut rig, at);
            rig.sender.transport(TransportCommand::Stop);
            play(&mut rig, at);
        }
        None => play(&mut rig, frames),
    }
    rig.probe
        .recorded_notes()
        .into_iter()
        .map(|heard| (heard.frame, heard.event.kind, heard.event.key))
        .collect()
}

/// Runs a scenario in every block size, which must hear the same, and
/// returns that.
fn in_every_block_size(
    midi: &MidiData,
    setup: impl Fn(&mut Rig, TrackId, ClipId) + Copy,
    frames: usize,
    stop_at: Option<usize>,
) -> Vec<Heard> {
    let first = run(BLOCKS[0], midi.clone(), setup, frames, stop_at);
    for block in &BLOCKS[1..] {
        let heard = run(*block, midi.clone(), setup, frames, stop_at);
        assert_eq!(heard, first, "block size {block}");
    }
    first
}

fn set(rig: &mut Rig, clip: ClipId, target: AutomationTarget, at: Ticks, value: f64) {
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        clip,
        kind: EnvelopeKind::Modulation,
        target,
        at,
        value,
    });
}

fn control(controller: u8, level: u8) -> NoteKind {
    NoteKind::Control {
        controller,
        value: f32::from(level) / 127.0,
    }
}

// Spec: midi-controller-data R3, R5
#[test]
fn a_step_reaches_the_device_on_its_frame_before_the_note_on_there() {
    let notes = MidiData::new([Note::new(60, BEAT * 2, BEAT, 100)]);
    // Steps are two breakpoints a tick apart: the value held, then the new
    // one.
    let heard = in_every_block_size(
        &notes,
        |rig, _, clip| {
            set(rig, clip, cc(1), BEAT, 10.0);
            set(rig, clip, cc(1), BEAT * 2 - Ticks(1), 10.0);
            set(rig, clip, cc(1), BEAT * 2, 90.0);
        },
        BEAT_FRAMES as usize * 4,
        None,
    );
    let messages: Vec<_> = heard
        .iter()
        .filter(|(_, kind, _)| kind.is_message())
        .collect();
    assert_eq!(
        messages
            .iter()
            .map(|(frame, kind, _)| (*frame, *kind))
            .collect::<Vec<_>>(),
        vec![
            (BEAT_FRAMES, control(1, 10)),
            (BEAT_FRAMES * 2, control(1, 90)),
        ]
    );
    let at_two: Vec<_> = heard
        .iter()
        .filter(|(frame, ..)| *frame == BEAT_FRAMES * 2)
        .map(|(_, kind, _)| *kind)
        .collect();
    assert_eq!(
        at_two.first(),
        Some(&control(1, 90)),
        "the step comes first on its frame"
    );
    assert!(at_two.contains(&NoteKind::On));
}

// Spec: midi-controller-data R3
#[test]
fn a_flat_stretch_sends_nothing() {
    let heard = in_every_block_size(
        &MidiData::new([]),
        |rig, _, clip| {
            set(rig, clip, cc(7), BEAT, 64.0);
            set(rig, clip, cc(7), BEAT * 2, 64.0);
            set(rig, clip, cc(7), BEAT * 3, 64.0);
        },
        BEAT_FRAMES as usize * 4,
        None,
    );
    assert_eq!(heard, vec![(BEAT_FRAMES, control(7, 64), 0)]);
}

// Spec: midi-controller-data R3
#[test]
fn a_slope_sends_a_stream_at_least_every_five_milliseconds() {
    let target = AutomationTarget::MidiPitchBend;
    let heard = in_every_block_size(
        &MidiData::new([]),
        move |rig, _, clip| {
            set(rig, clip, target, BEAT, -8192.0);
            set(rig, clip, target, BEAT * 5, 8191.0);
        },
        BEAT_FRAMES as usize * 6,
        None,
    );
    let bends: Vec<(i64, f32)> = heard
        .iter()
        .filter_map(|&(frame, kind, _)| match kind {
            NoteKind::PitchBend { value } => Some((frame, value)),
            _ => None,
        })
        .collect();
    assert_eq!(bends.first(), Some(&(BEAT_FRAMES, -1.0)));
    assert_eq!(bends.last(), Some(&(BEAT_FRAMES * 5, 1.0)));
    let five_ms = 240;
    for pair in bends.windows(2) {
        assert!(
            pair[1].0 - pair[0].0 <= five_ms,
            "{} to {} is more than 5 ms",
            pair[0].0,
            pair[1].0
        );
        assert!(pair[1].1 > pair[0].1, "each message is a new, higher value");
    }
    assert!(bends.len() > 400, "{} messages", bends.len());
}

// Spec: midi-controller-data R3, R4
#[test]
fn stopping_sends_no_controller_back_to_rest() {
    let heard = in_every_block_size(
        &MidiData::new([]),
        |rig, _, clip| {
            set(rig, clip, cc(1), Ticks::ZERO, 100.0);
            set(rig, clip, cc(1), BEAT * 2, 20.0);
        },
        0,
        Some(BEAT_FRAMES as usize),
    );
    // The slope plays until Stop at one beat and sends nothing after it, no
    // reset and no message for the rest of the stopped time.
    assert!(heard.len() > 1);
    assert!(heard.iter().all(|&(frame, ..)| frame < BEAT_FRAMES));
    assert!(
        heard
            .iter()
            .all(|&(_, kind, _)| matches!(kind, NoteKind::Control { value, .. } if value > 0.4)),
        "no controller goes back to rest"
    );
}

fn point(at: Ticks, value: f64) -> Breakpoint {
    Breakpoint {
        at,
        value,
        bend: 0.0,
    }
}

// Spec: midi-controller-data R3; mpe R2, R5. A set saved with pressure points
// holds each as a step, and the pressure plays what it played.
#[test]
fn per_note_pressure_arrives_as_polyphonic_pressure_on_the_note() {
    // As the version 7 set's points 0 → 127 and BEAT / 2 → 64 load.
    let note = Note {
        pressure: vec![
            point(Ticks::ZERO, 127.0),
            point(BEAT / 2 - Ticks(1), 127.0),
            point(BEAT / 2, 64.0),
        ],
        ..Note::new(62, BEAT, BEAT * 2, 100)
    };
    let heard = in_every_block_size(
        &MidiData::new([note]),
        |_, _, _| {},
        BEAT_FRAMES as usize * 4,
        None,
    );
    let pressure = |value: f32| NoteKind::Expression {
        expression: NoteExpression::Pressure,
        value,
    };
    assert_eq!(
        heard,
        vec![
            (BEAT_FRAMES, NoteKind::On, 62),
            (BEAT_FRAMES, pressure(1.0), 62),
            (BEAT_FRAMES * 3 / 2, pressure(64.0 / 127.0), 62),
            (BEAT_FRAMES * 3, NoteKind::Off, 62),
        ]
    );
}

fn expression(heard: &[Heard], which: NoteExpression) -> Vec<(i64, f32)> {
    heard
        .iter()
        .filter_map(|&(frame, kind, _)| match kind {
            NoteKind::Expression { expression, value } if expression == which => {
                Some((frame, value))
            }
            _ => None,
        })
        .collect()
}

/// A note of two beats from the second beat: Pitch rises 12 semitones over
/// its first beat and holds, Slide steps from 10 to 100 a half beat in, and
/// Pressure has one breakpoint.
fn bent() -> Note {
    Note {
        pitch: vec![point(Ticks::ZERO, 0.0), point(BEAT, 12.0)],
        slide: vec![
            point(Ticks::ZERO, 10.0),
            point(BEAT / 2 - Ticks(1), 10.0),
            point(BEAT / 2, 100.0),
        ],
        pressure: vec![point(BEAT, 50.0)],
        ..Note::new(60, BEAT, BEAT * 2, 100)
    }
}

// Spec: mpe R5, R6, R8. A clip note's three envelopes reach the device on
// their frames: each starting value just after the note-on, a step on its
// breakpoint's frame, a value at least every 5 ms along a slope, nothing
// from the note-off on; the same at every block size, under the allocation
// trap.
#[test]
fn a_clip_notes_pitch_slide_and_pressure_reach_the_device_on_their_frames() {
    let heard = in_every_block_size(
        &MidiData::new([bent()]),
        |_, _, _| {},
        BEAT_FRAMES as usize * 5,
        None,
    );
    let off = BEAT_FRAMES * 3;
    // The note-on comes first, and the starting values just after it, on its
    // frame.
    assert_eq!(heard[0], (BEAT_FRAMES, NoteKind::On, 60));
    let starting: Vec<_> = heard[1..4].iter().map(|h| (h.0, h.2)).collect();
    assert_eq!(
        starting,
        [(BEAT_FRAMES, 60), (BEAT_FRAMES, 60), (BEAT_FRAMES, 60)]
    );
    let bend = expression(&heard, NoteExpression::Bend);
    assert_eq!(bend[0], (BEAT_FRAMES, 0.0), "a bend of 0 semitones");
    // The slope streams on a grid at most 5 ms (240 frames) apart, each value
    // a share of 48 semitones.
    let slope: Vec<_> = bend[1..bend.len() - 1].to_vec();
    assert!(slope.len() >= 124, "{} messages", slope.len());
    for pair in slope.windows(2) {
        assert!(pair[1].0 - pair[0].0 <= 240, "{pair:?}");
        assert!(pair[1].1 > pair[0].1, "each message is a new, higher value");
    }
    for &(frame, value) in &slope {
        let semitones = 12.0 * (frame - BEAT_FRAMES) as f32 / BEAT_FRAMES as f32;
        assert!((value * 48.0 - semitones).abs() < 0.01, "{frame}: {value}");
    }
    // It ends on the breakpoint's frame at +12 semitones, a quarter of the
    // range, and nothing is sent after.
    assert_eq!(bend.last(), Some(&(BEAT_FRAMES * 2, 0.25)));
    // Slide steps once, on its frame.
    let slide = expression(&heard, NoteExpression::Slide);
    assert_eq!(
        slide,
        [
            (BEAT_FRAMES, 10.0 / 127.0),
            (BEAT_FRAMES + BEAT_FRAMES / 2, 100.0 / 127.0)
        ]
    );
    // Pressure has its one value to start with.
    assert_eq!(
        expression(&heard, NoteExpression::Pressure),
        [(BEAT_FRAMES, 50.0 / 127.0)]
    );
    // The note-off is the last thing, and every expression is before it.
    assert_eq!(heard.last(), Some(&(off, NoteKind::Off, 60)));
    assert!(
        heard
            .iter()
            .filter(|h| matches!(h.1, NoteKind::Expression { .. }))
            .all(|h| h.0 < off)
    );
}

// Spec: mpe R5. A note that doesn't sound sends no expression.
#[test]
fn a_note_that_does_not_sound_sends_no_expression() {
    let silent = Note {
        probability: 0.0,
        ..bent()
    };
    let muted = Note {
        mute: true,
        key: 61,
        ..bent()
    };
    let heard = in_every_block_size(
        &MidiData::new([silent, muted]),
        |_, _, _| {},
        BEAT_FRAMES as usize * 5,
        None,
    );
    assert_eq!(heard, []);
}

// Spec: mpe R5. Stopping sends no more expression: the note ends.
#[test]
fn stopping_ends_a_notes_expressions_with_it() {
    let heard = in_every_block_size(
        &MidiData::new([bent()]),
        |_, _, _| {},
        0,
        Some(BEAT_FRAMES as usize * 2 + BEAT_FRAMES as usize / 2),
    );
    let stop = BEAT_FRAMES * 5 / 2;
    assert!(
        heard
            .iter()
            .filter(|h| matches!(h.1, NoteKind::Expression { .. }))
            .all(|h| h.0 < stop),
        "nothing after the stop"
    );
    assert!(heard.iter().any(|h| matches!(h.1, NoteKind::Off)));
}

// Spec: midi-controller-data R3
#[test]
fn an_unlinked_envelope_runs_once_from_the_clip_start_while_the_loop_repeats() {
    // A one-bar loop over a four-bar clip.
    let looping = MidiData {
        looping: true,
        loop_end: Some(BAR),
        ..MidiData::new([])
    };
    let heard = in_every_block_size(
        &looping,
        |rig, _, clip| {
            set(rig, clip, cc(1), BEAT, 5.0);
            set(rig, clip, cc(2), BEAT, 5.0);
            set(rig, clip, cc(2), BAR * 2, 5.0);
            rig.apply(Command::SetClipEnvelopeLinked {
                clip,
                kind: EnvelopeKind::Modulation,
                target: cc(2),
                linked: false,
            });
        },
        BEAT_FRAMES as usize * 12,
        None,
    );
    let frames_of = |controller: u8| -> Vec<i64> {
        heard
            .iter()
            .filter(|&&(_, kind, _)| matches!(kind, NoteKind::Control { controller: c, .. } if c == controller))
            .map(|&(frame, ..)| frame)
            .collect()
    };
    assert_eq!(
        frames_of(1),
        vec![BEAT_FRAMES, BEAT_FRAMES * 5, BEAT_FRAMES * 9,],
        "the linked envelope repeats with the loop"
    );
    assert_eq!(
        frames_of(2),
        vec![BEAT_FRAMES],
        "the unlinked one plays once"
    );
}
