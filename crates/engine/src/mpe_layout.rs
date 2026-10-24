//! MPE MIDI: a track's notes and per-note expressions laid out on the member
//! channels of a zone, as the MPE specification does (the MPE spec, R17 and
//! R21).
//!
//! One layout serves every place MPE MIDI leaves Beatlayer: a CLAP plug-in's
//! MIDI port, a VST3 plug-in's MIDI mapping, an Audio Unit's MIDI block, and
//! a MIDI track's or External Instrument's MIDI To. It takes a block's notes
//! in order and hands each message, with the frame it belongs on, to a
//! caller's sink; the caller writes it into its own event list, which was
//! made with the node or route. The channel table is a fixed array, so
//! nothing here allocates.

use bl_core::midi::MidiMessage;
use bl_model::{MpeMode, MpeSettings, MpeZone};

use crate::node::{NoteEvent, NoteExpression, NoteKind};

/// The per-note bend range every member channel is set to, in semitones
/// (the MPE spec, R17).
pub const BEND_RANGE: u8 = 48;

/// The pitch bend value of a note's bend, from −1 to 1, over 14 bits.
fn bend14(value: f32) -> u16 {
    let value = value.clamp(-1.0, 1.0);
    let raw = if value >= 0.0 {
        8192 + (value * 8191.0).round() as i32
    } else {
        8192 + (value * 8192.0).round() as i32
    };
    raw.clamp(0, 16383) as u16
}

/// A value from 0 to 1 over 7 bits.
fn seven(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 127.0).round() as u8
}

/// A controller change on `channel`.
fn control(channel: u8, controller: u8, value: u8) -> Option<MidiMessage> {
    MidiMessage::from_bytes(&[0xB0 | channel, controller, value])
}

/// A note held on a member channel.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Held {
    key: u8,
    /// The channel the note came in on, which tells two notes apart when
    /// neither has an ID.
    input: u8,
    id: Option<u32>,
}

/// What a member channel holds and last received.
#[derive(Clone, Copy, Debug)]
struct Channel {
    held: Option<Held>,
    /// When its newest note started and when its last note ended, counted in
    /// note events; 0 for never.
    started: u64,
    ended: u64,
    /// The values last sent on the channel; `None` for none sent yet.
    bend: Option<u16>,
    slide: Option<u8>,
    pressure: Option<u8>,
}

impl Channel {
    const NEW: Self = Self {
        held: None,
        started: 0,
        ended: 0,
        bend: None,
        slide: None,
        pressure: None,
    };
}

/// A track's notes laid out as MPE MIDI.
#[derive(Clone, Debug)]
pub struct MpeLayout {
    settings: MpeSettings,
    channels: [Channel; 16],
    clock: u64,
    /// Whether the opening messages are still to be sent.
    opening: bool,
}

impl MpeLayout {
    /// A layout for `settings`, whose opening messages are sent first.
    #[must_use]
    pub fn new(settings: MpeSettings) -> Self {
        Self {
            settings,
            channels: [Channel::NEW; 16],
            clock: 0,
            opening: true,
        }
    }

    /// The settings it lays out for.
    #[must_use]
    pub fn settings(&self) -> MpeSettings {
        self.settings
    }

    /// Changes to `settings`: notes held end where they started, and the
    /// opening messages go out again. Nothing when they are the same.
    pub fn configure(&mut self, settings: MpeSettings, sink: &mut impl FnMut(u32, MidiMessage)) {
        if settings == self.settings {
            return;
        }
        self.release_all(0, sink);
        *self = Self {
            settings,
            channels: [Channel::NEW; 16],
            clock: self.clock,
            opening: true,
        };
    }

    /// Asks for the opening messages again, as when a port opens or the
    /// plug-in is activated.
    pub fn reopen(&mut self) {
        self.opening = true;
        self.channels = [Channel::NEW; 16];
    }

    /// How many channels notes can go on.
    fn members(&self) -> usize {
        match self.settings.mode {
            MpeMode::Zone => usize::from(self.settings.channels.min(15)),
            MpeMode::MultiChannel => {
                usize::from(self.settings.last.saturating_sub(self.settings.first)) + 1
            }
        }
    }

    /// The channel (0 to 15) of the `n`th note channel, in the order the
    /// zone counts them from its master.
    fn member(&self, n: usize) -> u8 {
        let n = n as u8;
        match self.settings.mode {
            MpeMode::Zone => match self.settings.zone {
                MpeZone::Lower => 1 + n,
                MpeZone::Upper => 14 - n,
            },
            MpeMode::MultiChannel => self.settings.first - 1 + n,
        }
    }

    /// The channel the track's own messages go on, or every channel of the
    /// range in multi-channel mode: `(first, last)`, 0 to 15.
    fn global(&self) -> (u8, u8) {
        match self.settings.mode {
            MpeMode::Zone => {
                let master = match self.settings.zone {
                    MpeZone::Lower => 0,
                    MpeZone::Upper => 15,
                };
                (master, master)
            }
            MpeMode::MultiChannel => (self.settings.first - 1, self.settings.last - 1),
        }
    }

    /// Sends the messages that open the zone, once: its MPE Configuration
    /// Message on the master channel and a bend sensitivity of 48 semitones
    /// on each note channel (R17); in multi-channel mode only the
    /// sensitivities. Call before laying a block out.
    pub fn open(&mut self, sink: &mut impl FnMut(u32, MidiMessage)) {
        if !std::mem::take(&mut self.opening) {
            return;
        }
        let mut send = |message: Option<MidiMessage>| {
            if let Some(message) = message {
                sink(0, message);
            }
        };
        // A registered parameter number, its value, and the null number so
        // later data entry changes nothing.
        let mut parameter = |channel: u8, lsb: u8, value: u8, fine: bool| {
            send(control(channel, 101, 0));
            send(control(channel, 100, lsb));
            send(control(channel, 6, value));
            if fine {
                send(control(channel, 38, 0));
            }
            send(control(channel, 101, 127));
            send(control(channel, 100, 127));
        };
        if self.settings.mode == MpeMode::Zone {
            parameter(self.global().0, 6, self.settings.channels.min(15), false);
        }
        for n in 0..self.members() {
            parameter(self.member(n), 0, BEND_RANGE, true);
        }
    }

    /// The member channel index of the held note `note` names.
    fn find(&self, note: &NoteEvent) -> Option<usize> {
        (0..self.members())
            .map(|n| usize::from(self.member(n)))
            .filter(|&c| {
                self.channels[c]
                    .held
                    .is_some_and(|held| match note.note_id {
                        Some(id) => held.id == Some(id) && held.key == note.key,
                        None => held.key == note.key && held.input == note.channel,
                    })
            })
            .max_by_key(|&c| self.channels[c].started)
    }

    /// The channel a new note goes on (R17): of those holding no note, the
    /// one whose last note ended longest ago; when every one holds a note,
    /// the one whose newest note started earliest.
    fn allocate(&self) -> usize {
        let mut free: Option<usize> = None;
        let mut oldest: Option<usize> = None;
        for n in 0..self.members() {
            let c = usize::from(self.member(n));
            let channel = &self.channels[c];
            if channel.held.is_none() {
                if free.is_none_or(|f| channel.ended < self.channels[f].ended) {
                    free = Some(c);
                }
            } else if oldest.is_none_or(|o| channel.started < self.channels[o].started) {
                oldest = Some(c);
            }
        }
        free.or(oldest).unwrap_or(usize::from(self.member(0)))
    }

    /// Sends `value` on channel `c` unless the channel holds it already; with
    /// none, puts the channel back at rest (the bend centred, the slide and
    /// pressure 0) if it was moved off.
    fn put(
        &mut self,
        offset: u32,
        c: usize,
        which: NoteExpression,
        value: Option<u16>,
        sink: &mut impl FnMut(u32, MidiMessage),
    ) {
        let channel = c as u8;
        let (sent, rest) = match which {
            NoteExpression::Bend => (self.channels[c].bend, 8192),
            NoteExpression::Slide => (self.channels[c].slide.map(u16::from), 0),
            NoteExpression::Pressure => (self.channels[c].pressure.map(u16::from), 0),
        };
        let value = match value {
            Some(value) => value,
            // No value of its own: put the channel back at rest if it left
            // it.
            None => match sent {
                Some(sent) if sent != rest => rest,
                _ => return,
            },
        };
        if sent == Some(value) {
            return;
        }
        let message = match which {
            NoteExpression::Bend => {
                self.channels[c].bend = Some(value);
                MidiMessage::from_bytes(&[0xE0 | channel, (value & 0x7F) as u8, (value >> 7) as u8])
            }
            NoteExpression::Slide => {
                self.channels[c].slide = Some(value as u8);
                control(channel, 74, value as u8)
            }
            NoteExpression::Pressure => {
                self.channels[c].pressure = Some(value as u8);
                MidiMessage::from_bytes(&[0xD0 | channel, value as u8])
            }
        };
        if let Some(message) = message {
            sink(offset, message);
        }
    }

    /// Lays a block's `notes`, in order of frame, out: each message goes to
    /// `sink` with its frame.
    pub fn lay_out(&mut self, notes: &[NoteEvent], sink: &mut impl FnMut(u32, MidiMessage)) {
        for (at, note) in notes.iter().enumerate() {
            match note.kind {
                NoteKind::On => self.note_on(notes, at, sink),
                NoteKind::Off | NoteKind::Choke => {
                    let Some(c) = self.find(note) else { continue };
                    let velocity = seven(note.velocity);
                    if let Some(message) =
                        MidiMessage::from_bytes(&[0x80 | c as u8, note.key & 0x7F, velocity])
                    {
                        sink(note.offset, message);
                    }
                    self.clock += 1;
                    self.channels[c].held = None;
                    self.channels[c].ended = self.clock;
                }
                NoteKind::Expression { expression, value } => {
                    let Some(c) = self.find(note) else { continue };
                    let value = match expression {
                        NoteExpression::Bend => bend14(value),
                        _ => u16::from(seven(value)),
                    };
                    self.put(note.offset, c, expression, Some(value), sink);
                }
                NoteKind::Control { .. }
                | NoteKind::PitchBend { .. }
                | NoteKind::ChannelPressure { .. } => {
                    let (first, last) = self.global();
                    for channel in first..=last {
                        if let Some(message) = (NoteEvent { channel, ..*note }).message() {
                            sink(note.offset, message);
                        }
                    }
                }
            }
        }
    }

    fn note_on(&mut self, notes: &[NoteEvent], at: usize, sink: &mut impl FnMut(u32, MidiMessage)) {
        let note = &notes[at];
        let c = self.allocate();
        // A note on a channel that still holds one ends it first.
        if let Some(held) = self.channels[c].held.take() {
            if let Some(message) = MidiMessage::from_bytes(&[0x80 | c as u8, held.key, 0]) {
                sink(note.offset, message);
            }
            self.clock += 1;
            self.channels[c].ended = self.clock;
        }
        // The note's starting values are its expressions on this frame, which
        // follow it: the channel takes them before the note-on (R17).
        let (mut bend, mut slide, mut pressure) = (None, None, None);
        for next in &notes[at + 1..] {
            if next.offset != note.offset {
                break;
            }
            let same = match note.note_id {
                Some(id) => next.note_id == Some(id) && next.key == note.key,
                None => next.key == note.key && next.channel == note.channel,
            };
            if let (true, NoteKind::Expression { expression, value }) = (same, next.kind) {
                match expression {
                    NoteExpression::Bend => bend = Some(bend14(value)),
                    NoteExpression::Slide => slide = Some(u16::from(seven(value))),
                    NoteExpression::Pressure => pressure = Some(u16::from(seven(value))),
                }
            }
        }
        self.put(note.offset, c, NoteExpression::Bend, bend, sink);
        self.put(note.offset, c, NoteExpression::Slide, slide, sink);
        self.put(note.offset, c, NoteExpression::Pressure, pressure, sink);
        let velocity = seven(note.velocity).max(1);
        sink(
            note.offset,
            MidiMessage::note_on(c as u8, note.key, velocity),
        );
        self.clock += 1;
        self.channels[c].started = self.clock;
        self.channels[c].held = Some(Held {
            key: note.key & 0x7F,
            input: note.channel,
            id: note.note_id,
        });
    }

    /// Ends every note held, at `offset`.
    pub fn release_all(&mut self, offset: u32, sink: &mut impl FnMut(u32, MidiMessage)) {
        for c in 0..16 {
            if let Some(held) = self.channels[c].held.take() {
                sink(offset, MidiMessage::note_off(c as u8, held.key));
                self.clock += 1;
                self.channels[c].ended = self.clock;
            }
        }
    }

    /// The notes held: `(channel, key)`, channel 0 to 15.
    pub fn held(&self) -> impl Iterator<Item = (u8, u8)> + '_ {
        self.channels
            .iter()
            .enumerate()
            .filter_map(|(c, channel)| Some((c as u8, channel.held?.key)))
    }

    /// The key held on each channel, `u8::MAX` for none.
    #[must_use]
    pub fn keys(&self) -> [u8; 16] {
        std::array::from_fn(|c| self.channels[c].held.map_or(u8::MAX, |held| held.key))
    }

    /// Whether any note is held.
    #[must_use]
    pub fn is_holding(&self) -> bool {
        self.channels.iter().any(|channel| channel.held.is_some())
    }
}

/// The notes a node was given as plain notes, with MPE Mode off, so that
/// switching MPE Mode on can end them: a layout knows only its own.
#[derive(Clone, Debug, Default)]
pub struct PlainNotes {
    /// Which keys are held, one bit each, per channel.
    held: [u128; 16],
}

impl PlainNotes {
    /// Follows `note`: a note-on holds its key, a note-off or choke lets it
    /// go.
    pub fn track(&mut self, note: &NoteEvent) {
        let bit = 1u128 << (note.key & 0x7F);
        let held = &mut self.held[usize::from(note.channel & 0x0F)];
        match note.kind {
            NoteKind::On => *held |= bit,
            NoteKind::Off | NoteKind::Choke => *held &= !bit,
            _ => {}
        }
    }

    /// Ends every note held, at `offset`.
    pub fn release_all(&mut self, offset: u32, sink: &mut impl FnMut(u32, MidiMessage)) {
        for (channel, held) in self.held.iter_mut().enumerate() {
            while *held != 0 {
                let key = held.trailing_zeros() as u8;
                *held &= *held - 1;
                sink(offset, MidiMessage::note_off(channel as u8, key));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(offset: u32, key: u8, id: u32) -> NoteEvent {
        NoteEvent {
            offset,
            kind: NoteKind::On,
            channel: 0,
            key,
            velocity: 100.0 / 127.0,
            note_id: Some(id),
        }
    }

    fn off(offset: u32, key: u8, id: u32) -> NoteEvent {
        NoteEvent {
            kind: NoteKind::Off,
            ..on(offset, key, id)
        }
    }

    fn expression(offset: u32, key: u8, id: u32, which: NoteExpression, value: f32) -> NoteEvent {
        NoteEvent {
            kind: NoteKind::Expression {
                expression: which,
                value,
            },
            ..on(offset, key, id)
        }
    }

    fn lay(layout: &mut MpeLayout, notes: &[NoteEvent]) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        let mut sink = |offset, message: MidiMessage| out.push((offset, message.bytes().to_vec()));
        layout.open(&mut sink);
        layout.lay_out(notes, &mut sink);
        out
    }

    // Spec: mpe R17
    #[test]
    fn a_zone_opens_with_its_configuration_and_a_sensitivity_of_48() {
        let mut layout = MpeLayout::new(MpeSettings {
            channels: 2,
            ..MpeSettings::default()
        });
        let sent = lay(&mut layout, &[]);
        let bytes: Vec<&[u8]> = sent.iter().map(|(_, b)| b.as_slice()).collect();
        assert_eq!(
            bytes,
            [
                // The configuration message on the master channel, 1.
                &[0xB0, 101, 0][..],
                &[0xB0, 100, 6],
                &[0xB0, 6, 2],
                &[0xB0, 101, 127],
                &[0xB0, 100, 127],
                // Channels 2 and 3: registered parameter 0, 48 semitones.
                &[0xB1, 101, 0],
                &[0xB1, 100, 0],
                &[0xB1, 6, 48],
                &[0xB1, 38, 0],
                &[0xB1, 101, 127],
                &[0xB1, 100, 127],
                &[0xB2, 101, 0],
                &[0xB2, 100, 0],
                &[0xB2, 6, 48],
                &[0xB2, 38, 0],
                &[0xB2, 101, 127],
                &[0xB2, 100, 127],
            ]
        );
        assert!(sent.iter().all(|(offset, _)| *offset == 0));
        // Once only.
        assert!(lay(&mut layout, &[]).is_empty());
        // The upper zone's master is channel 16 and its notes run down.
        let mut upper = MpeLayout::new(MpeSettings {
            zone: MpeZone::Upper,
            channels: 1,
            ..MpeSettings::default()
        });
        let sent = lay(&mut upper, &[]);
        assert_eq!(sent[0].1, [0xBF, 101, 0]);
        assert_eq!(sent[5].1, [0xBE, 101, 0]);
        assert_eq!(sent.len(), 11);
    }

    // Spec: mpe R17
    #[test]
    fn a_note_takes_the_channel_free_longest_and_starts_with_its_values() {
        let mut layout = MpeLayout::new(MpeSettings {
            channels: 3,
            ..MpeSettings::default()
        });
        lay(&mut layout, &[]);
        let sent = lay(
            &mut layout,
            &[
                on(0, 60, 1),
                expression(0, 60, 1, NoteExpression::Bend, 0.25),
                expression(0, 60, 1, NoteExpression::Slide, 1.0),
                expression(0, 60, 1, NoteExpression::Pressure, 0.5),
                on(10, 64, 2),
                off(20, 60, 1),
                on(30, 67, 3),
                off(40, 64, 2),
                off(50, 67, 3),
            ],
        );
        assert_eq!(
            sent,
            [
                // Channel 2: bend (+12 of 48 is 10,240), slide, pressure,
                // then the note-on.
                (0, vec![0xE1, 0, 80]),
                (0, vec![0xB1, 74, 127]),
                (0, vec![0xD1, 64]),
                (0, vec![0x91, 60, 100]),
                // The second note: channel 3, with nothing to set.
                (10, vec![0x92, 64, 100]),
                (20, vec![0x81, 60, 100]),
                // The third: channel 4, never used, ahead of channel 2 which
                // ended last.
                (30, vec![0x93, 67, 100]),
                (40, vec![0x82, 64, 100]),
                (50, vec![0x83, 67, 100]),
            ]
        );
    }

    // Spec: mpe R17
    #[test]
    fn a_channel_a_note_left_is_put_back_at_rest_before_the_next_note() {
        let mut layout = MpeLayout::new(MpeSettings {
            channels: 1,
            ..MpeSettings::default()
        });
        lay(&mut layout, &[]);
        let sent = lay(
            &mut layout,
            &[
                on(0, 60, 1),
                expression(5, 60, 1, NoteExpression::Bend, -1.0),
                expression(6, 60, 1, NoteExpression::Bend, -1.0),
                off(8, 60, 1),
                on(9, 62, 2),
            ],
        );
        assert_eq!(
            sent,
            [
                (0, vec![0x91, 60, 100]),
                // Once, though it came twice.
                (5, vec![0xE1, 0, 0]),
                (8, vec![0x81, 60, 100]),
                (9, vec![0xE1, 0, 64]),
                (9, vec![0x91, 62, 100]),
            ]
        );
    }

    // Spec: mpe R17
    #[test]
    fn with_every_channel_held_a_note_takes_the_one_started_earliest() {
        let mut layout = MpeLayout::new(MpeSettings {
            channels: 2,
            ..MpeSettings::default()
        });
        lay(&mut layout, &[]);
        let sent = lay(&mut layout, &[on(0, 60, 1), on(1, 62, 2), on(2, 64, 3)]);
        assert_eq!(
            sent,
            [
                (0, vec![0x91, 60, 100]),
                (1, vec![0x92, 62, 100]),
                // Channel 2's note is the oldest: it ends, and the new note
                // takes its place.
                (2, vec![0x81, 60, 0]),
                (2, vec![0x91, 64, 100]),
            ]
        );
    }

    // Spec: mpe R17
    #[test]
    fn the_tracks_channel_messages_go_on_the_master_or_every_channel_of_a_range() {
        let message = NoteEvent {
            offset: 3,
            kind: NoteKind::Control {
                controller: 1,
                value: 1.0,
            },
            channel: 4,
            key: 0,
            velocity: 0.0,
            note_id: None,
        };
        let mut zone = MpeLayout::new(MpeSettings::default());
        lay(&mut zone, &[]);
        assert_eq!(lay(&mut zone, &[message]), [(3, vec![0xB0, 1, 127])]);
        let mut upper = MpeLayout::new(MpeSettings {
            zone: MpeZone::Upper,
            ..MpeSettings::default()
        });
        lay(&mut upper, &[]);
        assert_eq!(lay(&mut upper, &[message]), [(3, vec![0xBF, 1, 127])]);
        let mut multi = MpeLayout::new(MpeSettings {
            mode: MpeMode::MultiChannel,
            first: 3,
            last: 5,
            ..MpeSettings::default()
        });
        let sent = lay(&mut multi, &[]);
        // No configuration message; a sensitivity on each channel of the
        // range.
        assert_eq!(sent.len(), 18);
        assert_eq!(sent[0].1, [0xB2, 101, 0]);
        assert_eq!(
            lay(&mut multi, &[message]),
            [
                (3, vec![0xB2, 1, 127]),
                (3, vec![0xB3, 1, 127]),
                (3, vec![0xB4, 1, 127])
            ]
        );
        // Its notes take the range's channels.
        assert_eq!(lay(&mut multi, &[on(0, 60, 1)]), [(0, vec![0x92, 60, 100])]);
    }

    // Spec: mpe R17
    #[test]
    fn plain_notes_held_end_when_the_layout_takes_over() {
        let mut plain = PlainNotes::default();
        let mut note = on(0, 60, 1);
        note.channel = 3;
        plain.track(&note);
        plain.track(&on(0, 64, 2));
        plain.track(&off(5, 64, 2));
        let mut sent = Vec::new();
        plain.release_all(7, &mut |offset, message: MidiMessage| {
            sent.push((offset, message.bytes().to_vec()));
        });
        assert_eq!(sent, [(7, vec![0x83, 60, 64])]);
        plain.release_all(7, &mut |_, _| panic!("nothing is held now"));
    }

    // Spec: mpe R17
    #[test]
    fn new_settings_end_the_notes_held_and_open_again() {
        let mut layout = MpeLayout::new(MpeSettings::default());
        lay(&mut layout, &[on(0, 60, 1)]);
        assert_eq!(layout.held().collect::<Vec<_>>(), [(1, 60)]);
        let mut sent = Vec::new();
        layout.configure(
            MpeSettings {
                channels: 4,
                ..MpeSettings::default()
            },
            &mut |offset, message: MidiMessage| sent.push((offset, message.bytes().to_vec())),
        );
        assert_eq!(sent, [(0, vec![0x81, 60, 64])]);
        assert!(!layout.is_holding());
        let again = lay(&mut layout, &[]);
        assert_eq!(again[2].1, [0xB0, 6, 4]);
    }
}
