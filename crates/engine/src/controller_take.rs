//! What a take or a capture makes of the controller messages it heard
//! (WP-2.29b): the stepped envelopes of the clip and the pressure of its
//! notes. Arithmetic on ticks, so the rules are tested without an engine.
//!
//! The rules are in the MIDI controller data spec, R10 to R15.

use bl_core::midi::MidiMessage;
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Breakpoint, Note, PressurePoint, envelope_value};

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

/// Gives each polyphonic pressure message in `moves` to the note its key
/// holds then: a pressure point at its offset from the note's start. A
/// message for a key with no note held is left out, and the later of two on
/// one tick wins (R11). Positions are in the same ticks as the notes'.
pub fn give_pressure(notes: &mut [Note], moves: &[(Ticks, MidiMessage)]) {
    for &(at, message) in moves.iter().filter(|(_, m)| m.status() & 0xF0 == 0xA0) {
        let held = notes
            .iter_mut()
            .filter(|note| {
                note.key == message.data1() && note.start <= at && at < note.start + note.length
            })
            .max_by_key(|note| note.start);
        let Some(note) = held else {
            continue;
        };
        let point = PressurePoint {
            at: at - note.start,
            value: message.data2(),
        };
        match note.pressure.last_mut() {
            Some(last) if last.at == point.at => *last = point,
            _ => note.pressure.push(point),
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

    // Spec: midi-controller-data R11. Pressure goes to the note its key
    // holds at that moment, and to none when no note is held.
    #[test]
    fn pressure_goes_to_the_note_held_on_its_key() {
        let mut notes = vec![
            Note::new(60, Ticks(0), Ticks(100), 100),
            Note::new(62, Ticks(0), Ticks(100), 100),
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
        assert_eq!(heard, [(Ticks(10), 112), (Ticks(50), 97)]);
        assert!(notes[1].pressure.is_empty());
    }
}
