//! Simulated APC mini mk2 and APC40 mkII for tests
//! (`docs/specs/supported-controllers.md`, R5): virtual ports on the test MIDI
//! system with the controller's port names, which answer the device inquiry
//! and the introduction as it does, apply every note-on the surface sends to
//! a model of the lights, and send a pad's, button's, fader's or knob's
//! messages when a test presses or moves it.
//!
//! Call [`Apc::pump`] after the host has handled a tick or a message to bring
//! the model up to date with what the surface sent.

use std::collections::BTreeMap;
use std::time::Instant;

use bl_midi_io::{Direction, TestBackend};

/// How a light moves between its colour and another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// Fading in and out.
    Pulsing,
    /// Switching on and off.
    Blinking,
}

/// What a pad's light goes on to: the other colour it alternates with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Then {
    /// How it moves.
    pub motion: Motion,
    /// The palette entry it moves to; 0 is dark.
    pub colour: u8,
    /// The note value's divisor it keeps time at: 8 is an eighth note.
    pub rate: u8,
}

/// How an RGB pad is lit: a palette entry at a brightness, and perhaps a
/// motion to another entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pad {
    /// The palette entry.
    pub colour: u8,
    /// The brightness, in percent.
    pub percent: u8,
    /// What it alternates with, if it does.
    pub then: Option<Then>,
}

impl Pad {
    /// Solid at a brightness.
    #[must_use]
    pub const fn solid(colour: u8, percent: u8) -> Self {
        Self {
            colour,
            percent,
            then: None,
        }
    }

    /// Pulsing between a colour and dark.
    #[must_use]
    pub const fn pulsing(colour: u8, rate: u8) -> Self {
        Self::flashing(colour, Motion::Pulsing, 0, rate)
    }

    /// Blinking between a colour and dark.
    #[must_use]
    pub const fn blinking(colour: u8, rate: u8) -> Self {
        Self::flashing(colour, Motion::Blinking, 0, rate)
    }

    /// A colour alternating with another by `motion`.
    #[must_use]
    pub const fn flashing(colour: u8, motion: Motion, other: u8, rate: u8) -> Self {
        Self {
            colour,
            percent: 100,
            then: Some(Then {
                motion,
                colour: other,
                rate,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Mini,
    Forty,
}

/// What a light was last sent: its first message, and the one that
/// alternates with it.
#[derive(Clone, Copy, Debug, Default)]
struct Led {
    primary: Option<(u8, u8)>,
    secondary: Option<(u8, u8)>,
}

/// A simulated APC mini mk2 or APC40 mkII.
pub struct Apc {
    backend: TestBackend,
    kind: Kind,
    seen: usize,
    leds: BTreeMap<(u8, u8), Led>,
    messages: Vec<Vec<u8>>,
    introduction: Option<Vec<u8>>,
    faders: [u8; 9],
    rings: BTreeMap<u8, u8>,
    lit_when_left: Option<usize>,
}

impl Apc {
    fn new(backend: &TestBackend, kind: Kind) -> Self {
        Self {
            backend: backend.clone(),
            kind,
            seen: 0,
            leds: BTreeMap::new(),
            messages: Vec::new(),
            introduction: None,
            faders: [0; 9],
            rings: BTreeMap::new(),
            lit_when_left: None,
        }
    }

    /// An APC mini mk2 on `backend`; call [`Apc::plug_in`] to connect it.
    #[must_use]
    pub fn mini(backend: &TestBackend) -> Self {
        Self::new(backend, Kind::Mini)
    }

    /// An APC40 mkII on `backend`.
    #[must_use]
    pub fn apc40(backend: &TestBackend) -> Self {
        Self::new(backend, Kind::Forty)
    }

    /// Where the faders stand before the surface asks, which the mini's
    /// introduction reply gives it.
    #[must_use]
    pub const fn with_faders(mut self, faders: [u8; 9]) -> Self {
        self.faders = faders;
        self
    }

    /// The name of the controller's surface port: the mini's first, and the
    /// APC40 mkII's only.
    #[must_use]
    pub const fn port(&self) -> &'static str {
        match self.kind {
            Kind::Mini => "APC mini mk2 Control",
            Kind::Forty => "APC40 mkII",
        }
    }

    /// The name of the mini's second port, which stays an ordinary MIDI port.
    #[must_use]
    pub const fn notes_port(&self) -> Option<&'static str> {
        match self.kind {
            Kind::Mini => Some("APC mini mk2 Notes"),
            Kind::Forty => None,
        }
    }

    /// Plugs the controller in: its ports appear.
    pub fn plug_in(&self) {
        for name in [Some(self.port()), self.notes_port()].into_iter().flatten() {
            self.backend.add_port(Direction::Input, name);
            self.backend.add_port(Direction::Output, name);
        }
    }

    // What the surface sent ----------------------------------------------

    /// Applies what the surface has sent to its output since the last call,
    /// and answers what it asks.
    pub fn pump(&mut self) {
        let sent = self.backend.sent(self.port());
        for bytes in sent.iter().skip(self.seen) {
            self.messages.push(bytes.clone());
            self.apply(bytes);
        }
        self.seen = sent.len();
    }

    fn reply(&self, bytes: &[u8]) {
        self.backend.send(self.port(), Instant::now(), bytes);
    }

    /// The product's byte in the system exclusive header.
    const fn product(&self) -> u8 {
        match self.kind {
            Kind::Mini => 0x4F,
            Kind::Forty => 0x29,
        }
    }

    fn apply(&mut self, bytes: &[u8]) {
        let Some(&status) = bytes.first() else {
            return;
        };
        if status == 0xF0 {
            self.sysex(bytes);
            return;
        }
        let (kind, channel) = (status & 0xF0, status & 0x0F);
        let (a, b) = (
            bytes.get(1).copied().unwrap_or(0),
            bytes.get(2).copied().unwrap_or(0),
        );
        match kind {
            0x90 | 0x80 => self.note(channel, a, if kind == 0x80 { 0 } else { b }),
            0xB0 => {
                self.rings.insert(a, b);
            }
            _ => {}
        }
    }

    fn sysex(&mut self, bytes: &[u8]) {
        if bytes == [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7] {
            // The device inquiry's reply: maker, product, and the rest zero.
            let mut reply = vec![
                0xF0,
                0x7E,
                0x00,
                0x06,
                0x02,
                0x47,
                self.product(),
                0x00,
                0x19,
            ];
            reply.extend([0; 26]);
            reply.push(0xF7);
            self.reply(&reply);
        } else if bytes.len() == 12 && bytes[..5] == [0xF0, 0x47, 0x7F, self.product(), 0x60] {
            self.introduction = Some(bytes.to_vec());
            if bytes[7] == 0x40 {
                self.lit_when_left = Some(self.lit());
            }
            let mut reply = vec![0xF0, 0x47, 0x7F, self.product(), 0x61, 0x00, 0x04];
            reply.extend(self.faders);
            reply.push(0xF7);
            self.reply(&reply);
        }
    }

    fn note(&mut self, channel: u8, note: u8, velocity: u8) {
        // The strip buttons of the APC40 are one per track, by channel.
        let strip = match (self.kind, note) {
            (Kind::Forty, 0x30..=0x34 | 0x42) => channel,
            _ => 0,
        };
        let led = self.leds.entry((note, strip)).or_default();
        let rgb = match self.kind {
            Kind::Mini => note < 0x40,
            Kind::Forty => note < 0x28 || (0x52..=0x56).contains(&note),
        };
        if velocity == 0 {
            // A note-off, or a velocity of 0, puts the light out.
            *led = Led::default();
        } else if rgb && self.kind == Kind::Forty && channel > 0 {
            led.secondary = Some((channel, velocity));
        } else if rgb && self.kind == Kind::Forty {
            *led = Led {
                primary: Some((channel, velocity)),
                secondary: None,
            };
        } else {
            led.primary = Some((channel, velocity));
        }
    }

    // What it shows ---------------------------------------------------------

    /// A pad's note: the mini's rise left to right and then up; the APC40's
    /// do the same over five rows.
    const fn pad_note(&self, column: u8, row: u8) -> u8 {
        match self.kind {
            Kind::Mini => (7 - row) * 8 + column,
            Kind::Forty => (4 - row) * 8 + column,
        }
    }

    /// The light at a note, by the track strip it belongs to.
    fn led(&self, note: u8, strip: u8) -> Option<Led> {
        self.leds
            .get(&(note, strip))
            .copied()
            .filter(|led| led.primary.is_some())
    }

    /// How the pad at a column and a row from the top is lit, if it is.
    #[must_use]
    pub fn pad(&self, column: u8, row: u8) -> Option<Pad> {
        self.rgb(self.pad_note(column, row))
    }

    /// How an RGB light is lit, by its note: a pad, or an APC40's scene
    /// button.
    #[must_use]
    pub fn rgb(&self, note: u8) -> Option<Pad> {
        let led = self.led(note, 0)?;
        let (channel, colour) = led.primary?;
        match self.kind {
            Kind::Mini => {
                const PERCENT: [u8; 7] = [10, 25, 50, 65, 75, 90, 100];
                Some(match channel {
                    0..=6 => Pad::solid(colour, PERCENT[usize::from(channel)]),
                    7..=10 => Pad::pulsing(colour, [16, 8, 4, 2][usize::from(channel - 7)]),
                    _ => Pad::blinking(colour, [24, 16, 8, 4, 2][usize::from(channel - 11)]),
                })
            }
            Kind::Forty => {
                let mut pad = Pad::solid(colour, 100);
                if let Some((channel, other)) = led.secondary {
                    // Channels 1 to 5 go once, 6 to 10 pulse, 11 to 15 blink.
                    let rate = [24, 16, 8, 4, 2][usize::from((channel - 1) % 5)];
                    let motion = if channel >= 11 {
                        Motion::Blinking
                    } else {
                        Motion::Pulsing
                    };
                    pad = Pad::flashing(colour, motion, other, rate);
                }
                Some(pad)
            }
        }
    }

    /// An APC40 scene button's light, from the top.
    #[must_use]
    pub fn scene_pad(&self, row: u8) -> Option<Pad> {
        self.rgb(0x52 + row)
    }

    /// A single-colour light's value, by its note and the channel of the
    /// track strip it is in (0 for the others): 1 for on, 2 for blinking (or
    /// the second colour), `None` for dark.
    #[must_use]
    pub fn single(&self, note: u8, strip: u8) -> Option<u8> {
        self.led(note, strip)?.primary.map(|(_, value)| value)
    }

    /// The mini's track button, from the left.
    #[must_use]
    pub fn track_button(&self, column: u8) -> Option<u8> {
        self.single(0x64 + column, 0)
    }

    /// The mini's scene button, from the top.
    #[must_use]
    pub fn scene_button(&self, row: u8) -> Option<u8> {
        self.single(0x70 + row, 0)
    }

    /// How many lights are on.
    #[must_use]
    pub fn lit(&self) -> usize {
        self.leds
            .keys()
            .filter(|(note, strip)| self.led(*note, *strip).is_some())
            .count()
    }

    /// Whether nothing is lit.
    #[must_use]
    pub fn all_dark(&self) -> bool {
        self.lit() == 0
    }

    /// How many lights were still on when the APC40 was sent back to Generic
    /// mode, or `None` while it hasn't been.
    #[must_use]
    pub const fn lit_when_left(&self) -> Option<usize> {
        self.lit_when_left
    }

    /// The introduction the surface sent, whole.
    #[must_use]
    pub fn introduction(&self) -> Option<&[u8]> {
        self.introduction.as_deref()
    }

    /// The mode the APC40's last introduction chose: 40h, 41h or 42h.
    #[must_use]
    pub fn mode(&self) -> Option<u8> {
        self.introduction.as_ref().map(|bytes| bytes[7])
    }

    /// What an APC40's knob ring last received as its type (controller 18h
    /// to 1Fh for the device knobs and 38h to 3Fh for the track knobs) and as
    /// its value (10h to 17h and 30h to 37h), by controller number.
    #[must_use]
    pub fn ring(&self, controller: u8) -> Option<u8> {
        self.rings.get(&controller).copied()
    }

    /// How many clock pulses it has been sent, and when each was stamped
    /// for.
    #[must_use]
    pub fn clock(&self) -> Vec<Instant> {
        self.backend
            .sent_at(self.port())
            .into_iter()
            .filter(|(_, bytes)| bytes == &[0xF8])
            .map(|(at, _)| at)
            .collect()
    }

    /// Every message the surface has sent it, as [`Apc::pump`] saw them.
    #[must_use]
    pub fn messages(&self) -> &[Vec<u8>] {
        &self.messages
    }

    // What it sends ---------------------------------------------------------

    fn send(&self, bytes: &[u8]) {
        assert!(
            self.backend.send(self.port(), Instant::now(), bytes),
            "the surface's input isn't open"
        );
    }

    /// A note-on on `channel`.
    pub fn note_on(&self, channel: u8, note: u8) {
        self.send(&[0x90 | channel, note, 127]);
    }

    /// A note-off on `channel`.
    pub fn note_off(&self, channel: u8, note: u8) {
        self.send(&[0x80 | channel, note, 127]);
    }

    /// A controller message on `channel`.
    pub fn controller(&self, channel: u8, controller: u8, value: u8) {
        self.send(&[0xB0 | channel, controller, value]);
    }

    /// Presses the pad at a column and a row from the top.
    pub fn pad_down(&self, column: u8, row: u8) {
        self.send(&[0x90, self.pad_note(column, row), 100]);
    }

    /// Releases it.
    pub fn pad_up(&self, column: u8, row: u8) {
        self.send(&[0x80, self.pad_note(column, row), 0]);
    }

    /// Presses a pad in the mini's Drum mode, which arrives on channel 10.
    pub fn drum_down(&self, column: u8, row: u8) {
        self.send(&[0x99, self.pad_note(column, row), 100]);
    }

    /// Releases it.
    pub fn drum_up(&self, column: u8, row: u8) {
        self.send(&[0x89, self.pad_note(column, row), 0]);
    }

    /// Presses a button of the mini, or one of the APC40's with no strip, by
    /// its note.
    pub fn button_down(&self, note: u8) {
        self.note_on(0, note);
    }

    /// Releases it.
    pub fn button_up(&self, note: u8) {
        self.note_off(0, note);
    }

    /// Moves the mini's fader `index` (the ninth is the master) to `value`.
    pub fn fader_to(&mut self, index: usize, value: u8) {
        self.faders[index] = value;
        self.controller(0, 0x30 + index as u8, value);
    }
}
