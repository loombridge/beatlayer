//! What a take or a capture makes of the controller messages it heard
//! (WP-2.29b): the stepped envelopes of the clip and the pressure of its
//! notes, and (WP-6.17) the Pitch, Slide and Pressure envelopes of the notes
//! an MPE port played. Arithmetic on ticks, so the rules are tested without an
//! engine.
//!
//! The rules are in the MIDI controller data spec, R10 to R15, and the MPE
//! spec, R12.

use bl_core::midi::MidiMessage;
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Breakpoint, Expression, Note, envelope_value};

/// Whether a recorded message is one a clip keeps besides notes: a control
/// change for one of the controllers a clip can hold, pitch bend, channel
/// pressure or polyphonic pressure (R10).
#[must_use]
pub fn is_controller(message: &MidiMessage) -> bool {
    match message.status() & 0xF0 {
        0xA0 | 0xD0 | 0xE0 => true,
        0xB0 => AutomationTarget::is_clip_controller(message.data1()),
        _ => false,
    }
}

/// The envelope a message moves and the value it moves it to, or `None` for
/// a message that is not a control change, pitch bend or channel pressure.
/// The channel is dropped: the clip has none (R10).
#[must_use]
pub fn envelope_move(message: &MidiMessage) -> Option<(AutomationTarget, f64)> {
    match message.status() & 0xF0 {
        0xB0 if AutomationTarget::is_clip_controller(message.data1()) => Some((
            AutomationTarget::MidiCc {
                number: message.data1(),
            },
            f64::from(message.data2()),
        )),
        0xD0 => Some((
            AutomationTarget::MidiChannelPressure,
            f64::from(message.data1()),
        )),
        0xE0 => Some((
            AutomationTarget::MidiPitchBend,
            f64::from((i32::from(message.data2()) << 7 | i32::from(message.data1())) - 8192),
        )),
        _ => None,
    }
}

/// The value an envelope holds before its first message: 0 for every
/// controller, which is pitch bend's centre (R11).
pub const REST: f64 = 0.0;

/// The breakpoints that step an envelope through `moves` (positions and
/// values, in order): each move holds the value before it up to the tick
/// before, then steps. Two moves on one tick leave the later (R11). `before`
/// is the value held before the first move.
#[must_use]
pub fn stepped(moves: &[(Ticks, f64)], before: f64) -> Vec<(Ticks, f64)> {
    let mut points: Vec<(Ticks, f64)> = Vec::with_capacity(moves.len() * 2);
    let mut held = before;
    for &(at, value) in moves {
        match points.last_mut() {
            Some(last) if last.0 == at => last.1 = value,
            last => {
                if at > Ticks::ZERO && last.is_none_or(|last| last.0 < at - Ticks(1)) {
                    points.push((at - Ticks(1), held));
                }
                points.push((at, value));
            }
        }
        held = value;
    }
    points
}

/// The breakpoints that add `moves` (positions and values, in order) to an
/// envelope holding `points`, removing none (R15): each move steps from the
/// value the envelope holds there and holds until the next breakpoint, old
/// or new, so the old envelope steps back at its next breakpoint. Two moves
/// on one tick leave the later.
#[must_use]
pub fn added(points: &[Breakpoint], moves: &[(Ticks, f64)]) -> Vec<(Ticks, f64)> {
    fn put(all: &mut Vec<Breakpoint>, set: &mut Vec<(Ticks, f64)>, at: Ticks, value: f64) {
        match all.binary_search_by_key(&at, |point| point.at) {
            Ok(index) => all[index].value = value,
            Err(index) => all.insert(
                index,
                Breakpoint {
                    at,
                    value,
                    bend: 0.0,
                },
            ),
        }
        set.retain(|point| point.0 != at);
        set.push((at, value));
    }
    let mut moves = moves.to_vec();
    moves.dedup_by(|later, earlier| {
        let same = later.0 == earlier.0;
        if same {
            earlier.1 = later.1;
        }
        same
    });
    let mut all = points.to_vec();
    let mut set = Vec::with_capacity(moves.len() * 3);
    for (index, &(at, value)) in moves.iter().enumerate() {
        let before = at - Ticks(1);
        if at > Ticks::ZERO && all.binary_search_by_key(&before, |point| point.at).is_err() {
            let held = envelope_value(&all, before.0 as f64).unwrap_or(REST);
            put(&mut all, &mut set, before, held);
        }
        put(&mut all, &mut set, at, value);
        let next_old = all.iter().map(|point| point.at).find(|next| *next > at);
        let next_move = moves.get(index + 1).map(|next| next.0);
        let until = match (next_old, next_move) {
            (Some(old), Some(new)) => Some(old.min(new)),
            (old, new) => old.or(new),
        };
        if let Some(until) = until
            && until - Ticks(1) > at
        {
            put(&mut all, &mut set, until - Ticks(1), value);
        }
    }
    set.sort_by_key(|point| point.0);
    set
}

/// The envelope that steps through `moves` (offsets from a note's start, in
/// order, and values), as a recorded controller steps (MPE R12): a breakpoint
/// at the note's start holding `start`, if there is one, and one at each
/// move's offset, the value before it held up to the tick before; the later
/// of two moves on one tick wins, and a move at or past `length`, which is
/// after the note-off, isn't kept.
#[must_use]
pub fn step_envelope(start: Option<f64>, moves: &[(Ticks, f64)], length: Ticks) -> Vec<Breakpoint> {
    let point = |at: Ticks, value: f64| Breakpoint {
        at,
        value,
        bend: 0.0,
    };
    let mut points: Vec<Breakpoint> = Vec::with_capacity(moves.len() * 2 + 1);
    let mut held = start;
    if let Some(value) = start {
        points.push(point(Ticks::ZERO, value));
    }
    for &(at, value) in moves {
        if at >= length {
            break;
        }
        let at = at.max(Ticks::ZERO);
        match points.last_mut() {
            Some(last) if last.at == at => last.value = value,
            last => {
                if let (Some(held), Some(last)) = (held, last)
                    && at > last.at + Ticks(1)
                {
                    points.push(point(at - Ticks(1), held));
                }
                points.push(point(at, value));
            }
        }
        held = Some(value);
    }
    points
}

/// Gives each polyphonic pressure message in `moves` to the note its key
/// holds then, as the note's Pressure envelope stepped through the messages
/// (R11; MPE R2): a message for a key with no note held is left out, and the
/// later of two on one tick wins. Positions are in the same ticks as the
/// notes'.
pub fn give_pressure(notes: &mut [Note], moves: &[(Ticks, MidiMessage)]) {
    let mut heard: Vec<(usize, Ticks, f64)> = Vec::new();
    for &(at, message) in moves.iter().filter(|(_, m)| m.status() & 0xF0 == 0xA0) {
        let held = notes
            .iter()
            .enumerate()
            .filter(|(_, note)| {
                note.key == message.data1() && note.start <= at && at < note.start + note.length
            })
            .max_by_key(|(_, note)| note.start);
        if let Some((index, note)) = held {
            heard.push((index, at - note.start, f64::from(message.data2())));
        }
    }
    for (index, note) in notes.iter_mut().enumerate() {
        let moves: Vec<(Ticks, f64)> = heard
            .iter()
            .filter(|(held, ..)| *held == index)
            .map(|&(_, at, value)| (at, value))
            .collect();
        if !moves.is_empty() {
            note.pressure = step_envelope(None, &moves, note.length);
        }
    }
}

/// One expression an MPE port sent for a note it was playing: the note's ID,
/// which expression, its value in the envelope's own units (semitones, or 0
/// to 127), and where, in the same ticks as the notes' starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heard {
    /// The note's ID, as the MPE input gave it.
    pub id: u32,
    /// Which envelope.
    pub expression: Expression,
    /// The value.
    pub value: f64,
    /// Where it was heard.
    pub at: Ticks,
}

/// Gives each note an MPE port played, named by its entry of `ids` (`None`
/// for a note from elsewhere), its Pitch, Slide and Pressure envelopes from
/// what `heard` says of it, each stepped as [`step_envelope`] does (MPE R12).
/// An expression the port sent nothing for leaves its envelope none.
pub fn give_expressions(notes: &mut [Note], ids: &[Option<u32>], heard: &[Heard]) {
    for (note, id) in notes.iter_mut().zip(ids) {
        let Some(id) = *id else {
            continue;
        };
        for expression in Expression::ALL {
            let (low, high) = expression.range();
            let mut moves: Vec<(Ticks, f64)> = heard
                .iter()
                .filter(|h| h.id == id && h.expression == expression && h.at >= note.start)
                .map(|h| (h.at - note.start, h.value.clamp(low, high)))
                .collect();
            moves.sort_by_key(|(at, _)| *at);
            if !moves.is_empty() {
                *note.envelope_mut(expression) = step_envelope(None, &moves, note.length);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ticks: i64, value: f64) -> (Ticks, f64) {
        (Ticks(ticks), value)
    }

    // Spec: midi-controller-data R11. Each move holds the value before it,
    // then steps; the later of two on one tick wins.
    #[test]
    fn moves_step_from_the_value_held_before() {
        let points = stepped(
            &[at(100, 10.0), at(100, 20.0), at(101, 30.0), at(300, 5.0)],
            0.0,
        );
        assert_eq!(
            points,
            [
                at(99, 0.0),
                at(100, 20.0),
                at(101, 30.0),
                at(299, 30.0),
                at(300, 5.0)
            ]
        );
        assert_eq!(stepped(&[at(0, 7.0)], 0.0), [at(0, 7.0)]);
        assert_eq!(stepped(&[], 0.0), []);
    }

    // Spec: midi-controller-data R15 (S2). An added value steps from the old
    // envelope and holds until its next breakpoint, where the old steps
    // resume: no ramp to or from it.
    #[test]
    fn added_moves_hold_until_the_next_breakpoint() {
        let old = |at, value| Breakpoint {
            at: Ticks(at),
            value,
            bend: 0.0,
        };
        let points = [old(0, 10.0), old(499, 10.0), old(500, 20.0)];
        assert_eq!(
            added(&points, &[at(200, 127.0), at(600, 127.0), at(700, 127.0)]),
            [
                at(199, 10.0),
                at(200, 127.0),
                at(498, 127.0),
                at(599, 20.0),
                at(600, 127.0),
                at(699, 127.0),
                at(700, 127.0)
            ]
        );
        assert_eq!(
            added(&[], &[at(100, 1.0), at(100, 2.0), at(200, 3.0)]),
            stepped(&[at(100, 2.0), at(200, 3.0)], REST),
            "with no old breakpoints, the same steps as a take"
        );
    }

    // Spec: midi-controller-data R10. Pitch bend is 14 bits around 8192,
    // channel pressure has one data byte, and the channel is dropped.
    #[test]
    fn messages_become_the_moves_of_their_envelopes_whatever_the_channel() {
        let bytes = |bytes: &[u8]| MidiMessage::from_bytes(bytes).unwrap();
        assert_eq!(
            envelope_move(&bytes(&[0xB3, 1, 99])),
            Some((AutomationTarget::MidiCc { number: 1 }, 99.0))
        );
        assert_eq!(
            envelope_move(&bytes(&[0xE5, 0x00, 0x40])),
            Some((AutomationTarget::MidiPitchBend, 0.0))
        );
        assert_eq!(
            envelope_move(&bytes(&[0xE0, 0x7F, 0x7F])),
            Some((AutomationTarget::MidiPitchBend, 8191.0))
        );
        assert_eq!(
            envelope_move(&bytes(&[0xE0, 0, 0])),
            Some((AutomationTarget::MidiPitchBend, -8192.0))
        );
        assert_eq!(
            envelope_move(&bytes(&[0xD2, 55])),
            Some((AutomationTarget::MidiChannelPressure, 55.0))
        );
        for left_out in [
            &[0xB0, 0, 1][..],
            &[0xB0, 32, 1],
            &[0xB0, 123, 0],
            &[0xA0, 60, 9],
        ] {
            assert_eq!(envelope_move(&bytes(left_out)), None, "{left_out:?}");
        }
        assert!(is_controller(&bytes(&[0xA0, 60, 9])));
        assert!(!is_controller(&bytes(&[0xB0, 123, 0])));
        assert!(!is_controller(&MidiMessage::note_on(0, 60, 100)));
    }

    // Spec: midi-controller-data R11; mpe R12. Pressure goes to the note its
    // key holds, stepped, the later of two on one tick winning.
    #[test]
    fn pressure_goes_to_the_note_held_on_its_key() {
        let mut notes = [
            Note::new(60, Ticks(0), Ticks(100), 100),
            Note::new(64, Ticks(0), Ticks(100), 100),
        ];
        let pressure = |key, value| MidiMessage::from_bytes(&[0xA0, key, value]).unwrap();
        give_pressure(
            &mut notes,
            &[
                (Ticks(10), pressure(60, 127)),
                (Ticks(10), pressure(60, 112)),
                (Ticks(50), pressure(60, 97)),
                (Ticks(60), pressure(64, 50)),
                (Ticks(100), pressure(60, 1)),
            ],
        );
        let heard: Vec<_> = notes[0].pressure.iter().map(|p| (p.at, p.value)).collect();
        assert_eq!(
            heard,
            [(Ticks(10), 112.0), (Ticks(49), 112.0), (Ticks(50), 97.0)],
            "the one at 100 is after the note-off"
        );
        let other: Vec<_> = notes[1].pressure.iter().map(|p| (p.at, p.value)).collect();
        assert_eq!(other, [(Ticks(60), 50.0)]);
        assert!(notes[0].expressions_fit());
    }

    // Spec: mpe R12. A starting value is a breakpoint at the note's start;
    // each move holds the value before it, then steps; the later of two on
    // a tick wins, and what is after the note-off isn't kept.
    #[test]
    fn expressions_step_from_the_starting_value() {
        let heard = |expression, at, value| Heard {
            id: 7,
            expression,
            value,
            at: Ticks(at),
        };
        let mut notes = [
            Note::new(60, Ticks(1000), Ticks(200), 100),
            Note::new(62, Ticks(1000), Ticks(200), 100),
        ];
        give_expressions(
            &mut notes,
            &[Some(7), None],
            &[
                heard(Expression::Pitch, 1000, 0.0),
                heard(Expression::Pitch, 1050, 2.0),
                heard(Expression::Pitch, 1050, 3.0),
                heard(Expression::Pitch, 1051, 4.0),
                heard(Expression::Pitch, 1200, 9.0),
                heard(Expression::Slide, 1100, 64.0),
                heard(Expression::Pressure, 900, 10.0),
            ],
        );
        let pitch: Vec<_> = notes[0]
            .pitch
            .iter()
            .map(|p| (p.at.get(), p.value))
            .collect();
        assert_eq!(pitch, [(0, 0.0), (49, 0.0), (50, 3.0), (51, 4.0)]);
        let slide: Vec<_> = notes[0]
            .slide
            .iter()
            .map(|p| (p.at.get(), p.value))
            .collect();
        assert_eq!(
            slide,
            [(100, 64.0)],
            "nothing before the first one, and no step"
        );
        assert!(notes[0].pressure.is_empty(), "heard before the note began");
        assert!(!notes[1].has_expression(), "a note from elsewhere has none");
        assert!(notes[0].expressions_fit());
    }
}
