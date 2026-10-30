//! MIDI Timecode in (`docs/specs/midi-timecode.md`, R2 to R4): reading the
//! eight quarter-frame pieces of a time in the order they come, and what a
//! time is as a position in the Arrangement.
//!
//! [`ClockIn`](crate::clock_in::ClockIn) hands each quarter frame from an
//! input to [`Assembler::push`], and follows what it answers. Nothing here
//! allocates, locks or blocks.

use bl_core::time::TempoMap;
use bl_core::timecode::{FrameRate, Timecode};

/// What a quarter frame did to the time being read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    /// A piece of a time that isn't whole yet, or one that doesn't follow
    /// the one before and starts the wait for the next first piece.
    Part,
    /// The eighth piece in order: the whole time its pieces spell, and the
    /// rate they name, as it was when the first piece was sent.
    Whole(Timecode, FrameRate),
    /// A piece that comes before the one that came: the host runs the time
    /// backwards.
    Reverse,
}

/// Reads quarter frames: eight pieces in order, 0 to 7, make a time.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Assembler {
    pieces: [u8; 8],
    /// The piece that came last.
    last: Option<u8>,
    /// Whether the pieces held began with piece 0 and came in order.
    reading: bool,
}

impl Assembler {
    pub(crate) const fn new() -> Self {
        Self {
            pieces: [0; 8],
            last: None,
            reading: false,
        }
    }

    /// Forgets the pieces held.
    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    /// Takes a quarter frame's data byte: its piece number in bits 4 to 6
    /// and its four bits of the time in bits 0 to 3.
    pub(crate) fn push(&mut self, data: u8) -> Piece {
        let (piece, nibble) = ((data >> 4) & 7, data & 15);
        let previous = self.last.replace(piece);
        if previous.is_some_and(|before| piece == (before + 7) % 8) {
            self.reading = false;
            return Piece::Reverse;
        }
        if piece == 0 {
            self.pieces[0] = nibble;
            self.reading = true;
            return Piece::Part;
        }
        if !self.reading || previous != Some(piece - 1) {
            self.reading = false;
            return Piece::Part;
        }
        self.pieces[usize::from(piece)] = nibble;
        if piece < 7 {
            return Piece::Part;
        }
        self.reading = false;
        let p = &self.pieces;
        Piece::Whole(
            Timecode {
                frames: p[0] | (p[1] & 1) << 4,
                seconds: p[2] | (p[3] & 3) << 4,
                minutes: p[4] | (p[5] & 3) << 4,
                hours: p[6] | (p[7] & 1) << 4,
            },
            FrameRate::from_code(p[7] >> 1),
        )
    }
}

/// The time a full-frame message's bytes spell (hours with the rate code in
/// bits 5 and 6, minutes, seconds, frames) and the rate it names.
pub(crate) fn full_frame(data: [u8; 4]) -> (Timecode, FrameRate) {
    (
        Timecode {
            hours: data[0] & 31,
            minutes: data[1],
            seconds: data[2],
            frames: data[3],
        },
        FrameRate::from_code(data[0] >> 5),
    )
}

/// The song position, in ticks, of `seconds` of timecode when the Arrangement
/// starts at `offset` seconds (R4): a time before the offset is 1.1.1.
pub(crate) fn song_ticks(tempo: &TempoMap, seconds: f64, offset: f64) -> f64 {
    tempo.seconds_to_ticks_exact((seconds - offset).max(0.0))
}

#[cfg(test)]
mod tests {
    use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, TempoSegment, Ticks};

    use super::*;

    /// The eight quarter-frame data bytes of `time` at `rate`'s code.
    fn pieces(time: Timecode, code: u8) -> [u8; 8] {
        let nibbles = [
            time.frames & 15,
            time.frames >> 4,
            time.seconds & 15,
            time.seconds >> 4,
            time.minutes & 15,
            time.minutes >> 4,
            time.hours & 15,
            (time.hours >> 4) | code << 1,
        ];
        core::array::from_fn(|i| (i as u8) << 4 | nibbles[i])
    }

    // Spec: midi-timecode R2, R3
    #[test]
    fn eight_pieces_in_order_spell_a_time_and_its_rate() {
        let time = Timecode {
            hours: 17,
            minutes: 42,
            seconds: 59,
            frames: 23,
        };
        let mut reader = Assembler::new();
        let data = pieces(time, 1);
        for &byte in &data[..7] {
            assert_eq!(reader.push(byte), Piece::Part);
        }
        assert_eq!(reader.push(data[7]), Piece::Whole(time, FrameRate::Fps25));
        // The next eight make another.
        for &byte in &data[..7] {
            assert_eq!(reader.push(byte), Piece::Part);
        }
        assert_eq!(reader.push(data[7]), Piece::Whole(time, FrameRate::Fps25));
    }

    // Spec: midi-timecode R2, R6
    #[test]
    fn a_stream_joined_part_way_waits_for_a_first_piece() {
        let time = Timecode {
            hours: 0,
            minutes: 0,
            seconds: 10,
            frames: 0,
        };
        let data = pieces(time, 1);
        let mut reader = Assembler::new();
        for &byte in &data[3..] {
            assert_eq!(reader.push(byte), Piece::Part);
        }
        for &byte in &data[..7] {
            assert_eq!(reader.push(byte), Piece::Part);
        }
        assert_eq!(reader.push(data[7]), Piece::Whole(time, FrameRate::Fps25));
        // A piece that goes missing spoils the time.
        for &byte in &data[..4] {
            reader.push(byte);
        }
        for &byte in &data[5..7] {
            assert_eq!(reader.push(byte), Piece::Part);
        }
        assert_eq!(reader.push(data[7]), Piece::Part);
    }

    // Spec: midi-timecode R8
    #[test]
    fn pieces_in_reverse_order_are_the_time_running_backwards() {
        let data = pieces(Timecode::ZERO, 0);
        let mut reader = Assembler::new();
        reader.push(data[4]);
        assert_eq!(reader.push(data[3]), Piece::Reverse);
        assert_eq!(reader.push(data[2]), Piece::Reverse);
        reader.push(data[0]);
        assert_eq!(reader.push(data[7]), Piece::Reverse);
    }

    // Spec: midi-timecode R2
    #[test]
    fn a_full_frame_spells_its_time_and_rate() {
        assert_eq!(
            full_frame([2 << 5 | 1, 2, 3, 4]),
            (
                Timecode {
                    hours: 1,
                    minutes: 2,
                    seconds: 3,
                    frames: 4,
                },
                FrameRate::Fps2997Drop
            )
        );
    }

    // Spec: midi-timecode R4
    #[test]
    fn a_time_less_the_offset_is_a_position_through_the_tempo_map() {
        // 120 BPM for the first four quarter notes (2 s), then 60 BPM.
        let tempo = TempoMap::new([
            TempoSegment::Constant {
                start: Ticks::ZERO,
                bpm: Bpm::new(120.0).unwrap(),
            },
            TempoSegment::Constant {
                start: Ticks(4 * TICKS_PER_QUARTER),
                bpm: Bpm::new(60.0).unwrap(),
            },
        ])
        .unwrap();
        let quarter = TICKS_PER_QUARTER as f64;
        // Ten seconds on, the Arrangement starting at ten seconds: 1.1.1.
        assert_eq!(song_ticks(&tempo, 10.0, 10.0), 0.0);
        // Before the offset: 1.1.1 too.
        assert_eq!(song_ticks(&tempo, 9.0, 10.0), 0.0);
        // 1.5 s after the offset: three quarter notes at 120 BPM.
        let ticks = song_ticks(&tempo, 11.5, 10.0);
        assert!((ticks - 3.0 * quarter).abs() < 1.0, "{ticks}");
        // 3 s after: the 2 s of 120 BPM, then one second of 60 BPM.
        let ticks = song_ticks(&tempo, 13.0, 10.0);
        assert!((ticks - 5.0 * quarter).abs() < 1.0, "{ticks}");
    }
}
