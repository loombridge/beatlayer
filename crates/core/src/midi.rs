//! MIDI messages as they travel from a port to the engine (WP-2.2).
//!
//! These are the types the MIDI device layer (`bl-midi-io`) and the engine
//! (`bl-engine`) share, so neither depends on the other: a [`MidiMessage`]
//! (one short MIDI 1.0 message), the [`MidiSource`] it came from, and the
//! [`MidiInputEvent`] that carries both, with the time it arrived, across
//! the lock-free queue to the audio thread. Every type here is `Copy` and
//! small, so the queue holds them by value and nothing allocates.
//!
//! The rules are in `docs/specs/midi-io.md`.

use core::fmt;
use std::time::Instant;

/// One short MIDI 1.0 message: a status byte and up to two data bytes.
///
/// Channel messages (note on and off, pressure, control and program
/// changes, pitch bend) and the short system messages (clock, start, stop,
/// song position, and the like) fit. System exclusive doesn't: it has no
/// fixed length, and nothing that reaches a track needs it.
///
/// ```
/// use bl_core::midi::MidiMessage;
///
/// let on = MidiMessage::note_on(0, 60, 100);
/// assert_eq!(on.bytes(), &[0x90, 60, 100]);
/// assert_eq!(on.channel(), Some(0));
/// assert_eq!(MidiMessage::from_bytes(&[0x90, 60, 100]), Some(on));
/// assert_eq!(MidiMessage::from_bytes(&[0xF0, 1, 2, 0xF7]), None);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MidiMessage {
    bytes: [u8; 3],
    len: u8,
}

impl MidiMessage {
    /// The message in `bytes`, if they're one whole short message: a status
    /// byte, then exactly as many data bytes (each below 128) as it takes.
    /// Running status (a message without its status byte) isn't accepted;
    /// every MIDI system Beatlayer reads from adds it back.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let (&status, data) = bytes.split_first()?;
        let wanted = data_bytes(status)?;
        if data.len() != wanted || data.iter().any(|&b| b >= 0x80) {
            return None;
        }
        let mut out = [status, 0, 0];
        out[1..=wanted].copy_from_slice(data);
        Some(Self {
            bytes: out,
            len: 1 + wanted as u8,
        })
    }

    /// A note-on on `channel` (0 to 15) for `key` (0 to 127) at `velocity`
    /// (1 to 127). Values out of range are masked to 4 and 7 bits.
    #[must_use]
    pub const fn note_on(channel: u8, key: u8, velocity: u8) -> Self {
        Self {
            bytes: [0x90 | (channel & 0x0F), key & 0x7F, velocity & 0x7F],
            len: 3,
        }
    }

    /// A note-off on `channel` (0 to 15) for `key` (0 to 127), with a
    /// release velocity of 64, as most keyboards send.
    #[must_use]
    pub const fn note_off(channel: u8, key: u8) -> Self {
        Self {
            bytes: [0x80 | (channel & 0x0F), key & 0x7F, 64],
            len: 3,
        }
    }

    /// The message's bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// The status byte and two data bytes, with 0 for any the message
    /// lacks.
    #[must_use]
    pub const fn padded(&self) -> [u8; 3] {
        self.bytes
    }

    /// The status byte.
    #[must_use]
    pub const fn status(&self) -> u8 {
        self.bytes[0]
    }

    /// The channel, 0 to 15, of a channel message; `None` for a system
    /// message.
    #[must_use]
    pub const fn channel(&self) -> Option<u8> {
        if self.bytes[0] < 0xF0 {
            Some(self.bytes[0] & 0x0F)
        } else {
            None
        }
    }

    /// Whether it's a system message: clock, transport, song position, and
    /// the rest of the 0xF_ range.
    #[must_use]
    pub const fn is_system(&self) -> bool {
        self.bytes[0] >= 0xF0
    }

    /// Whether it's a note-on with a velocity above 0. (A note-on at
    /// velocity 0 is a note-off.)
    #[must_use]
    pub const fn is_note_on(&self) -> bool {
        self.bytes[0] & 0xF0 == 0x90 && self.bytes[2] > 0
    }

    /// Whether it ends a note: a note-off, or a note-on at velocity 0.
    #[must_use]
    pub const fn is_note_off(&self) -> bool {
        let kind = self.bytes[0] & 0xF0;
        kind == 0x80 || (kind == 0x90 && self.bytes[2] == 0)
    }

    /// The first data byte (a note's key, a controller's number), or 0.
    #[must_use]
    pub const fn data1(&self) -> u8 {
        self.bytes[1]
    }

    /// The second data byte (a note's velocity, a controller's value), or
    /// 0.
    #[must_use]
    pub const fn data2(&self) -> u8 {
        self.bytes[2]
    }
}

/// How many data bytes follow a status byte, or `None` if it isn't the
/// status of a short message.
const fn data_bytes(status: u8) -> Option<usize> {
    match status {
        0x80..=0xBF | 0xE0..=0xEF => Some(2),
        0xC0..=0xDF => Some(1),
        // MIDI time code quarter frame, song select.
        0xF1 | 0xF3 => Some(1),
        // Song position pointer.
        0xF2 => Some(2),
        // Tune request and the real-time messages (clock, start, continue,
        // stop, active sensing, reset).
        0xF6 | 0xF8 | 0xFA..=0xFC | 0xFE | 0xFF => Some(0),
        // Data bytes, system exclusive and its end, and the undefined
        // statuses.
        _ => None,
    }
}

impl fmt::Debug for MidiMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MidiMessage(")?;
        for (i, byte) in self.bytes().iter().enumerate() {
            if i > 0 {
                write!(f, " ")?;
            }
            write!(f, "{byte:02X}")?;
        }
        write!(f, ")")
    }
}

/// A MIDI port's identity in the engine: a hash of its name.
///
/// The set names a track's input port by name, as Live does, so the same
/// set finds the same port on another computer or after a replug. The
/// device layer stamps each message with its port's ID and the compiler
/// works out the same ID from the name in the set, so the two never need to
/// share a table. The hash is 64-bit FNV-1a over the name's UTF-8 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortId(pub u64);

impl PortId {
    /// The ID of the port called `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in name.as_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
        }
        Self(hash)
    }
}

/// Where a message came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MidiSource {
    /// An input port, by its ID.
    Port(PortId),
    /// An input port whose MPE switch is on (`docs/specs/midi-io.md`, R27),
    /// by its ID: its messages are read as MPE.
    MpePort(PortId),
    /// The computer MIDI keyboard.
    ComputerKeyboard,
}

impl MidiSource {
    /// The port the message came from, MPE or not; `None` for the computer
    /// keyboard.
    #[must_use]
    pub const fn port(self) -> Option<PortId> {
        match self {
            Self::Port(port) | Self::MpePort(port) => Some(port),
            Self::ComputerKeyboard => None,
        }
    }
}

/// A message on its way from a MIDI input to the engine: where it came
/// from, when it arrived, and the message.
///
/// `time` is on the monotonic clock ([`Instant`]) the engine reads at the
/// start of each block, which is how the engine places the message on a
/// sample (`docs/specs/midi-io.md`, R13 to R16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiInputEvent {
    /// The port, or the computer keyboard.
    pub source: MidiSource,
    /// When it arrived.
    pub time: Instant,
    /// The message.
    pub message: MidiMessage,
}

/// A message on its way from the engine to a MIDI output port: the port, the
/// moment its frame is heard at the audio interface's output, and the
/// message (`docs/specs/external-devices.md`, R4, R5).
///
/// `time` is on the same monotonic clock ([`Instant`]) as input times. The
/// MIDI output thread hands the message to the MIDI system stamped with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiOutputEvent {
    /// The port.
    pub port: PortId,
    /// When the message is due at the port.
    pub time: Instant,
    /// The message.
    pub message: MidiMessage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_messages_parse_and_the_rest_are_refused() {
        for bytes in [
            &[0x90, 60, 100][..],
            &[0x8F, 0, 0],
            &[0xB3, 7, 127],
            &[0xC0, 5],
            &[0xD1, 64],
            &[0xE0, 0, 64],
            &[0xF8],
            &[0xFA],
            &[0xF2, 1, 2],
        ] {
            let message = MidiMessage::from_bytes(bytes).unwrap();
            assert_eq!(message.bytes(), bytes);
        }
        for bytes in [
            &[][..],
            &[60, 100],
            &[0x90, 60],
            &[0x90, 60, 100, 0],
            &[0x90, 60, 200],
            &[0xF0, 1, 0xF7],
            &[0xF7],
            &[0xF4],
            &[0xC0],
        ] {
            assert_eq!(MidiMessage::from_bytes(bytes), None, "{bytes:?}");
        }
    }

    #[test]
    fn channels_and_kinds() {
        let on = MidiMessage::note_on(3, 64, 90);
        assert_eq!(on.channel(), Some(3));
        assert!(on.is_note_on() && !on.is_note_off() && !on.is_system());
        assert_eq!((on.data1(), on.data2()), (64, 90));
        let off = MidiMessage::note_off(15, 64);
        assert_eq!(off.bytes(), &[0x8F, 64, 64]);
        assert!(off.is_note_off());
        assert!(MidiMessage::note_on(0, 1, 0).is_note_off());
        let clock = MidiMessage::from_bytes(&[0xF8]).unwrap();
        assert_eq!(clock.channel(), None);
        assert!(clock.is_system());
        assert_eq!(format!("{on:?}"), "MidiMessage(93 40 5A)");
    }

    #[test]
    fn port_ids_follow_the_name() {
        assert_eq!(
            PortId::from_name("IAC Bus 1"),
            PortId::from_name("IAC Bus 1")
        );
        assert_ne!(
            PortId::from_name("IAC Bus 1"),
            PortId::from_name("IAC Bus 2")
        );
        // FNV-1a's offset basis for the empty string.
        assert_eq!(PortId::from_name("").0, 0xcbf2_9ce4_8422_2325);
    }
}
