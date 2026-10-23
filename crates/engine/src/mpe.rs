//! Reading MPE from an input port (WP-6.17; the MPE spec, R10, R11 and R13).
//!
//! A port whose MPE switch is on is read as MPE: each of its messages becomes
//! the [`MpeRole`]s it plays on a track, as the spec lays out. The table of
//! ports, each port's zones, its member channels' latest values and the notes
//! held are made when the engine's MIDI input is made, so reading a message
//! allocates nothing; a port past the table's room is read as an ordinary
//! one.

use bl_core::midi::{MidiMessage, PortId};
use bl_model::Expression;

/// The most MPE ports read at once.
const PORTS: usize = 8;

/// The most notes one port holds at once.
const HELD: usize = 128;

/// The most roles one message gives: a note-on and its three starting
/// values, or an expression for each note held.
const ROLES: usize = 140;

/// A member channel's per-note bend range before a pitch bend sensitivity
/// changes it, in semitones (R10).
const BEND_RANGE: f32 = 48.0;

/// What one message of an MPE port is, on a track's input.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum MpeRole {
    /// A message as a port that isn't MPE sends it: a note, or a channel
    /// message (a master channel's, or any other controller's).
    #[default]
    Plain,
    /// A note-on or note-off on a member channel: the note has this ID.
    Note {
        /// The note's own ID.
        id: u32,
    },
    /// A per-note expression of a held note: a value of its envelope, in the
    /// envelope's own units (semitones, or 0 to 127). The key says which
    /// note, with the ID.
    Expression {
        /// The note's own ID.
        id: u32,
        /// The note's key.
        key: u8,
        /// Which expression.
        expression: Expression,
        /// Its value.
        value: f32,
    },
}

/// A member channel: the values it last received, if it has received any.
#[derive(Clone, Copy, Debug, Default)]
struct Member {
    /// The pitch bend, from −1 to 1.
    bend: Option<f32>,
    /// The slide (controller 74), 0 to 127.
    slide: Option<f32>,
    /// The channel pressure, 0 to 127.
    pressure: Option<f32>,
}

/// A registered parameter number a channel selected, as controllers 101 and
/// 100 send it.
#[derive(Clone, Copy, Debug)]
struct Parameter {
    msb: u8,
    lsb: u8,
}

const NULL: Parameter = Parameter { msb: 127, lsb: 127 };

/// A note held on a member channel.
#[derive(Clone, Copy, Debug)]
struct Held {
    id: u32,
    channel: u8,
    key: u8,
}

/// One port read as MPE: its zones, its channels and its held notes.
#[derive(Debug)]
struct Port {
    port: PortId,
    /// The lower zone's member channels, from channel 2 up (0 for no zone).
    lower: u8,
    /// The upper zone's member channels, from channel 15 down (0 for none).
    upper: u8,
    /// The per-note bend range of each zone, in semitones.
    range: [f32; 2],
    members: [Member; 16],
    parameter: [Parameter; 16],
    held: Vec<Held>,
}

/// Which part of its zones a channel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    /// A member channel of the zone (0 lower, 1 upper).
    Member(usize),
    /// A zone's master channel.
    Master,
    /// Neither: an ordinary channel.
    Free,
}

impl Port {
    /// A port with room for every note it holds, made off the audio thread.
    fn new() -> Self {
        Self {
            held: Vec::with_capacity(HELD),
            ..Self::fresh()
        }
    }

    /// Makes the port `port`, a lower zone of 15 member channels with
    /// nothing held. Allocates nothing.
    fn take(&mut self, port: PortId) {
        let mut held = std::mem::take(&mut self.held);
        held.clear();
        *self = Self {
            port,
            held,
            ..Self::fresh()
        };
    }

    /// The state of a new port, with no room made for held notes.
    fn fresh() -> Self {
        Self {
            port: PortId(0),
            lower: 15,
            upper: 0,
            range: [BEND_RANGE; 2],
            members: [Member::default(); 16],
            parameter: [NULL; 16],
            held: Vec::new(),
        }
    }

    fn part(&self, channel: u8) -> Part {
        let channel = usize::from(channel);
        if channel == 0 && self.lower > 0 || channel == 15 && self.upper > 0 {
            Part::Master
        } else if (1..=usize::from(self.lower)).contains(&channel) {
            Part::Member(0)
        } else if self.upper > 0 && (15 - usize::from(self.upper)..=14).contains(&channel) {
            Part::Member(1)
        } else {
            Part::Free
        }
    }

    /// Sets a zone's member count from the MPE Configuration Message, the
    /// zone the other of the two shares channels with giving up the ones it
    /// would have in common.
    fn configure(&mut self, upper: bool, count: u8) {
        if upper {
            self.upper = count.min(14);
            if self.upper > 0 {
                self.lower = self.lower.min(14 - self.upper);
            }
        } else {
            self.lower = count.min(15);
            self.upper = self.upper.min(14u8.saturating_sub(self.lower));
        }
    }

    /// Reads a message, pushing what it is on a track's input into `roles`.
    /// An empty `roles` is a message taken in and not passed on.
    fn read(&mut self, message: &MidiMessage, next_id: &mut u32, roles: &mut Vec<MpeRole>) {
        let (Some(channel), status) = (message.channel(), message.status() & 0xF0) else {
            return;
        };
        let part = self.part(channel);
        if status == 0xB0 && matches!(message.data1(), 6 | 38 | 98..=101) {
            self.parameter_number(channel, message);
            return;
        }
        let Part::Member(zone) = part else {
            roles.push(MpeRole::Plain);
            return;
        };
        let member = &mut self.members[usize::from(channel)];
        match status {
            0x90 | 0x80 if message.is_note_on() => {
                if self.held.len() == self.held.capacity() {
                    return;
                }
                let id = *next_id;
                *next_id = next_id.wrapping_add(1) | 0x8000_0000;
                self.held.push(Held {
                    id,
                    channel,
                    key: message.data1(),
                });
                roles.push(MpeRole::Note { id });
                let key = message.data1();
                let range = self.range[zone];
                for (expression, value) in [
                    (Expression::Pitch, member.bend.map(|bend| bend * range)),
                    (Expression::Slide, member.slide),
                    (Expression::Pressure, member.pressure),
                ] {
                    if let Some(value) = value {
                        roles.push(MpeRole::Expression {
                            id,
                            key,
                            expression,
                            value,
                        });
                    }
                }
            }
            0x80 | 0x90 => {
                let at = self
                    .held
                    .iter()
                    .position(|held| held.channel == channel && held.key == message.data1());
                if let Some(at) = at {
                    let held = self.held.remove(at);
                    roles.push(MpeRole::Note { id: held.id });
                }
            }
            0xA0 => {
                for held in &self.held {
                    if held.channel == channel && held.key == message.data1() {
                        roles.push(MpeRole::Expression {
                            id: held.id,
                            key: held.key,
                            expression: Expression::Pressure,
                            value: f32::from(message.data2()),
                        });
                    }
                }
            }
            0xD0 => {
                let value = f32::from(message.data1());
                member.pressure = Some(value);
                self.each_held(channel, Expression::Pressure, value, roles);
            }
            0xE0 => {
                let raw = i32::from(message.data1()) | i32::from(message.data2()) << 7;
                let bend = if raw >= 8192 {
                    (raw - 8192) as f32 / 8191.0
                } else {
                    (raw - 8192) as f32 / 8192.0
                };
                member.bend = Some(bend);
                let semitones = bend * self.range[zone];
                self.each_held(channel, Expression::Pitch, semitones, roles);
            }
            0xB0 if message.data1() == 74 => {
                let value = f32::from(message.data2());
                member.slide = Some(value);
                self.each_held(channel, Expression::Slide, value, roles);
            }
            _ => roles.push(MpeRole::Plain),
        }
    }

    fn each_held(&self, channel: u8, expression: Expression, value: f32, roles: &mut Vec<MpeRole>) {
        for held in self.held.iter().filter(|held| held.channel == channel) {
            roles.push(MpeRole::Expression {
                id: held.id,
                key: held.key,
                expression,
                value,
            });
        }
    }

    /// Follows the registered parameter number controllers (101, 100) and
    /// their data entry (6): the MPE Configuration Message (parameter 6) on
    /// a master channel sets a zone's member count, and a pitch bend
    /// sensitivity (parameter 0) on a member channel sets its zone's range.
    fn parameter_number(&mut self, channel: u8, message: &MidiMessage) {
        let at = usize::from(channel);
        let selected = self.parameter[at];
        match message.data1() {
            101 => self.parameter[at].msb = message.data2(),
            100 => self.parameter[at].lsb = message.data2(),
            // A non-registered number takes the entries that follow it away
            // from the registered one.
            98 | 99 => self.parameter[at] = NULL,
            6 => match (selected.msb, selected.lsb) {
                // Spec: mpe R10. Channel 1 or 16 says how many member
                // channels its zone has.
                (0, 6) if channel == 0 || channel == 15 => {
                    self.configure(channel == 15, message.data2());
                }
                (0, 0) => {
                    if let Part::Member(zone) = self.part(channel) {
                        self.range[zone] = f32::from(message.data2());
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// The MPE ports in use, and what reading a message gives.
#[derive(Debug)]
pub(crate) struct MpeReader {
    /// Every slot made up front; the first `used` have a port.
    ports: Vec<Port>,
    used: usize,
    next_id: u32,
    roles: Vec<MpeRole>,
}

impl MpeReader {
    /// A reader with room for every port, made off the audio thread.
    pub(crate) fn new() -> Self {
        Self {
            ports: (0..PORTS).map(|_| Port::new()).collect(),
            used: 0,
            next_id: 0x8000_0000,
            roles: Vec::with_capacity(ROLES),
        }
    }

    /// What `message`, from the MPE port `port`, is on a track's input. A
    /// port past the table's room is read as an ordinary one.
    pub(crate) fn read(&mut self, port: PortId, message: &MidiMessage) -> &[MpeRole] {
        self.roles.clear();
        let used = self.used;
        let at = match self.ports[..used].iter().position(|p| p.port == port) {
            Some(at) => at,
            None if used < self.ports.len() => {
                self.ports[used].take(port);
                self.used += 1;
                used
            }
            None => {
                self.roles.push(MpeRole::Plain);
                return &self.roles;
            }
        };
        self.ports[at].read(message, &mut self.next_id, &mut self.roles);
        &self.roles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(bytes: &[u8]) -> MidiMessage {
        MidiMessage::from_bytes(bytes).unwrap()
    }

    fn roles(reader: &mut MpeReader, message: &[u8]) -> Vec<MpeRole> {
        reader.read(PortId(1), &bytes(message)).to_vec()
    }

    // Spec: mpe R10. A port is one lower zone until a configuration message
    // says otherwise, and its pitch bend sensitivity is 48 semitones until
    // changed.
    #[test]
    fn a_port_starts_as_a_lower_zone_with_a_range_of_48_semitones() {
        let mut reader = MpeReader::new();
        let on = roles(&mut reader, &[0x91, 60, 100]);
        let [MpeRole::Note { id }] = on[..] else {
            panic!("{on:?}");
        };
        // Full bend up is the whole range.
        let bend = roles(&mut reader, &[0xE1, 0x7F, 0x7F]);
        assert_eq!(
            bend,
            [MpeRole::Expression {
                id,
                key: 60,
                expression: Expression::Pitch,
                value: 48.0
            }]
        );
        // The master channel's messages are channel messages.
        assert_eq!(roles(&mut reader, &[0xB0, 1, 5]), [MpeRole::Plain]);
        assert_eq!(roles(&mut reader, &[0xE0, 0, 0x40]), [MpeRole::Plain]);
    }

    // Spec: mpe R10. An upper zone's configuration message takes channels
    // from the lower zone, and a sensitivity on a member channel sets its
    // zone's range.
    #[test]
    fn configuration_messages_set_the_zones_and_the_range() {
        let mut reader = MpeReader::new();
        // Registered parameter 6 on channel 16, five member channels.
        for message in [[0xBF, 101, 0], [0xBF, 100, 6], [0xBF, 6, 5]] {
            assert_eq!(roles(&mut reader, &message), [], "taken in, not passed on");
        }
        // Channels 11 to 15 are the upper zone's members now; channel 16 its
        // master.
        let on = roles(&mut reader, &[0x9E, 64, 90]);
        let [MpeRole::Note { id }] = on[..] else {
            panic!("{on:?}");
        };
        assert_eq!(roles(&mut reader, &[0xBF, 1, 9]), [MpeRole::Plain]);
        // A bend sensitivity of 24 on a member channel of the upper zone.
        for message in [[0xBE, 101, 0], [0xBE, 100, 0], [0xBE, 6, 24]] {
            assert_eq!(roles(&mut reader, &message), []);
        }
        assert_eq!(
            roles(&mut reader, &[0xEE, 0x7F, 0x7F]),
            [MpeRole::Expression {
                id,
                key: 64,
                expression: Expression::Pitch,
                value: 24.0
            }]
        );
        // The lower zone kept its range and lost the channels the upper took.
        let low = roles(&mut reader, &[0x92, 60, 100]);
        assert!(matches!(low[..], [MpeRole::Note { .. }]), "{low:?}");
        assert_eq!(roles(&mut reader, &[0x9C, 61, 100]).len(), 1);
        // A count of 0 removes the zone: channel 16 is an ordinary one.
        for message in [[0xBF, 101, 0], [0xBF, 100, 6], [0xBF, 6, 0]] {
            roles(&mut reader, &message);
        }
        assert_eq!(roles(&mut reader, &[0xBF, 1, 9]), [MpeRole::Plain]);
    }

    // Spec: mpe R11. Values received before a note-on are its starting
    // values; slide and pressure follow the held note; polyphonic pressure
    // goes to the note on its key; a message for a channel with no note held
    // changes only the channel's values.
    #[test]
    fn a_note_starts_with_the_values_its_channel_received_and_follows_its_messages() {
        let mut reader = MpeReader::new();
        assert_eq!(roles(&mut reader, &[0xE2, 0, 0x40]), [], "no note held");
        assert_eq!(roles(&mut reader, &[0xB2, 74, 100]), []);
        let on = roles(&mut reader, &[0x92, 62, 80]);
        let [MpeRole::Note { id }, a, b] = on[..] else {
            panic!("{on:?}");
        };
        assert!(matches!(
            a,
            MpeRole::Expression {
                expression: Expression::Pitch,
                value,
                ..
            } if value == 0.0
        ));
        assert_eq!(
            b,
            MpeRole::Expression {
                id,
                key: 62,
                expression: Expression::Slide,
                value: 100.0
            }
        );
        assert_eq!(
            roles(&mut reader, &[0xD2, 33]),
            [MpeRole::Expression {
                id,
                key: 62,
                expression: Expression::Pressure,
                value: 33.0
            }]
        );
        assert_eq!(
            roles(&mut reader, &[0xA2, 62, 77]),
            [MpeRole::Expression {
                id,
                key: 62,
                expression: Expression::Pressure,
                value: 77.0
            }]
        );
        assert_eq!(
            roles(&mut reader, &[0xA2, 63, 77]),
            [],
            "no note on that key"
        );
        // A second note on the same channel takes the channel's messages too.
        let second = roles(&mut reader, &[0x92, 64, 80]);
        let [MpeRole::Note { id: other }, ..] = second[..] else {
            panic!("{second:?}");
        };
        assert_ne!(id, other, "each note has an ID of its own");
        assert_eq!(roles(&mut reader, &[0xB2, 74, 5]).len(), 2);
        // Note-offs end the notes, each with its own ID; other controllers on
        // a member channel are channel messages.
        assert_eq!(roles(&mut reader, &[0x82, 62, 64]), [MpeRole::Note { id }]);
        assert_eq!(roles(&mut reader, &[0xB2, 1, 5]), [MpeRole::Plain]);
        assert_eq!(
            roles(&mut reader, &[0x92, 64, 0]),
            [MpeRole::Note { id: other }]
        );
    }
}
