//! Simulated Launchkeys for tests (`docs/specs/supported-controllers.md`,
//! R5): virtual ports on the test MIDI system with the controller's port
//! names, which answer the device inquiry as it does, apply every message
//! the surface documents to a model of the lights, the display and the
//! modes, and send a pad's, button's, pot's, fader's or encoder's messages
//! when a test presses or moves it.
//!
//! Call [`Launchkey::pump`] after the host has handled a tick or a message
//! to bring the model up to date with what the surface sent.

use std::collections::BTreeMap;
use std::time::Instant;

use bl_midi_io::{Direction, TestBackend};

use super::{DRUM, Gen, Layer, Light, MAKER, PAD_TOP};

/// How big the controller is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    /// The 25-key regular model: pads and pots, no faders.
    K25,
    /// The 49-key model, with faders.
    K49,
    /// The 61-key model, with faders.
    K61,
    /// The 88-key model, with the MK3's other header.
    K88,
    /// The MK4 Mini: another header, no faders.
    Mini,
}

impl Size {
    const fn keys(self) -> u8 {
        match self {
            Self::K25 | Self::Mini => 25,
            Self::K49 => 49,
            Self::K61 => 61,
            Self::K88 => 88,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Led {
    fixed: u8,
    flash: Option<u8>,
    pulse: Option<u8>,
}

/// What a field of the MK4's screen holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    /// The display's configuration byte.
    pub config: u8,
    /// Its text fields.
    pub fields: Vec<String>,
}

/// A simulated Launchkey.
pub struct Launchkey {
    backend: TestBackend,
    family: Gen,
    size: Size,
    seen: usize,
    daw_mode: bool,
    /// The pad, pot and fader modes the controller shows.
    modes: [u8; 3],
    leds: BTreeMap<(Layer, u8), Led>,
    drum_claimed: bool,
    scale: (bool, u8, u8),
    /// The MK3's default display rows, and the names and values of its
    /// controls.
    rows: [String; 2],
    names: BTreeMap<u8, String>,
    values: BTreeMap<u8, String>,
    cleared: bool,
    /// The MK4's displays, by target.
    screens: BTreeMap<u8, Screen>,
    messages: Vec<Vec<u8>>,
    lit_when_left: Option<usize>,
}

impl Launchkey {
    fn new(backend: &TestBackend, family: Gen, size: Size) -> Self {
        Self {
            backend: backend.clone(),
            family,
            size,
            seen: 0,
            daw_mode: false,
            modes: [0; 3],
            leds: BTreeMap::new(),
            drum_claimed: false,
            scale: (false, 0, 0),
            rows: [String::new(), String::new()],
            names: BTreeMap::new(),
            values: BTreeMap::new(),
            cleared: false,
            screens: BTreeMap::new(),
            messages: Vec::new(),
            lit_when_left: None,
        }
    }

    /// A Launchkey MK3 on `backend`; call [`Launchkey::plug_in`] to connect
    /// it.
    #[must_use]
    pub fn mk3(backend: &TestBackend, size: Size) -> Self {
        Self::new(backend, Gen::Mk3, size)
    }

    /// A Launchkey MK4.
    #[must_use]
    pub fn mk4(backend: &TestBackend, size: Size) -> Self {
        Self::new(backend, Gen::Mk4, size)
    }

    /// The name of the controller's DAW ports, which are the surface's.
    #[must_use]
    pub fn daw_port(&self) -> String {
        match (self.family, self.size) {
            (Gen::Mk3, size) => format!("Launchkey MK3 {} LKMK3 DAW", size.keys()),
            (Gen::Mk4, Size::Mini) => "Launchkey Mini MK4 25 DAW".to_owned(),
            (Gen::Mk4, size) => format!("Launchkey MK4 {} DAW", size.keys()),
        }
    }

    /// The name of its MIDI ports, which stay ordinary MIDI ports.
    #[must_use]
    pub fn midi_port(&self) -> String {
        self.daw_port()
            .replace("LKMK3 DAW", "LKMK3 MIDI")
            .replace("DAW", "MIDI")
    }

    /// Plugs the controller in: its ports appear.
    pub fn plug_in(&self) {
        for name in [self.daw_port(), self.midi_port()] {
            self.backend.add_port(Direction::Input, &name);
            self.backend.add_port(Direction::Output, &name);
        }
    }

    // What the surface sent ----------------------------------------------

    /// Applies what the surface has sent to the DAW output since the last
    /// call, and answers what it asks.
    pub fn pump(&mut self) {
        let sent = self.backend.sent(&self.daw_port());
        for bytes in sent.iter().skip(self.seen) {
            self.messages.push(bytes.clone());
            self.apply(bytes);
        }
        self.seen = sent.len();
    }

    fn header(&self) -> [u8; 6] {
        let id = match (self.family, self.size) {
            (Gen::Mk3, Size::K88) => 0x12,
            (Gen::Mk3, _) => 0x0F,
            (Gen::Mk4, Size::Mini) => 0x13,
            (Gen::Mk4, _) => 0x14,
        };
        [0xF0, 0x00, 0x20, 0x29, 0x02, id]
    }

    fn reply(&self, bytes: &[u8]) {
        self.backend.send(&self.daw_port(), Instant::now(), bytes);
    }

    /// The channel of the mode reports: 16 on the MK3, 7 on the MK4.
    const fn mode_status(&self) -> u8 {
        match self.family {
            Gen::Mk3 => 0xBF,
            Gen::Mk4 => 0xB6,
        }
    }

    const fn mode_numbers(&self) -> [u8; 3] {
        match self.family {
            Gen::Mk3 => [0x03, 0x09, 0x0A],
            Gen::Mk4 => [0x1D, 0x1E, 0x1F],
        }
    }

    fn apply(&mut self, bytes: &[u8]) {
        let Some(&status) = bytes.first() else {
            return;
        };
        if status == 0xF0 {
            if bytes == [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7] {
                // The device inquiry's reply (application).
                let mut reply = vec![0xF0, 0x7E, 0x00, 0x06, 0x02];
                reply.extend(MAKER);
                reply.extend([0x36, 0x01, 0x00, 0x00, 1, 2, 3, 4, 0xF7]);
                self.reply(&reply);
            } else if bytes.len() > 7 && bytes[..6] == self.header() {
                self.sysex(&bytes[6..bytes.len() - 1]);
            }
            return;
        }
        let (kind, channel) = (status & 0xF0, status & 0x0F);
        let (a, b) = (
            bytes.get(1).copied().unwrap_or(0),
            bytes.get(2).copied().unwrap_or(0),
        );
        if status == 0x9F && a == 0x0C {
            self.daw_mode(b > 0);
            return;
        }
        let numbers = self.mode_numbers();
        if status == self.mode_status()
            && let Some(at) = numbers.iter().position(|n| *n == a)
        {
            self.modes[at] = b;
            return;
        }
        if self.family == Gen::Mk4 && status == 0xB6 {
            match a {
                0x54 => self.drum_claimed = b > 0,
                0x4A => self.scale.0 = b > 0,
                0x3D => self.scale.1 = b,
                0x3E => self.scale.2 = b,
                _ => {}
            }
            return;
        }
        let light = match (kind, channel) {
            (0x90 | 0x80, 0..=2) => Some((Layer::Pad, channel)),
            (0x90 | 0x80, 9..=11) => Some((Layer::Drum, channel - 9)),
            (0xB0, 0..=2) => Some((Layer::Button, channel)),
            _ => None,
        };
        if let Some((layer, mode)) = light {
            let c = if kind == 0x80 { 0 } else { b };
            let led = self.leds.entry((layer, a)).or_default();
            match mode {
                0 => {
                    *led = Led {
                        fixed: c,
                        flash: None,
                        pulse: None,
                    }
                }
                1 => led.flash = Some(c),
                _ => led.pulse = Some(c),
            }
        }
    }

    fn daw_mode(&mut self, on: bool) {
        if !on {
            self.lit_when_left = Some(self.lit());
        }
        self.daw_mode = on;
        self.leds.clear();
        self.rows = [String::new(), String::new()];
        self.names.clear();
        self.values.clear();
        self.screens.clear();
        self.drum_claimed = false;
        self.modes = [0; 3];
    }

    fn sysex(&mut self, body: &[u8]) {
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
        match (self.family, body) {
            (Gen::Mk3, [0x04, row, text_bytes @ ..]) => {
                if let Some(at) = self.rows.get_mut(usize::from(*row)) {
                    *at = text(text_bytes);
                    self.cleared = false;
                }
            }
            (Gen::Mk3, [0x06]) => {
                self.rows = [String::new(), String::new()];
                self.cleared = true;
            }
            (Gen::Mk3, [0x07, index, rest @ ..]) => {
                self.names.insert(*index, text(rest));
            }
            (Gen::Mk3, [0x08, index, rest @ ..]) => {
                self.values.insert(*index, text(rest));
            }
            (Gen::Mk4, [0x04, target, config]) => {
                self.screens.entry(*target).or_default().config = *config;
            }
            (Gen::Mk4, [0x06, target, field, rest @ ..]) => {
                let screen = self.screens.entry(*target).or_default();
                let field = usize::from(*field);
                if screen.fields.len() <= field {
                    screen.fields.resize(field + 1, String::new());
                }
                screen.fields[field] = text(rest);
            }
            _ => {}
        }
    }

    // What it shows -------------------------------------------------------

    /// How a pad or button is lit, if it is.
    #[must_use]
    pub fn light(&self, layer: Layer, number: u8) -> Option<Light> {
        let led = self.leds.get(&(layer, number))?;
        if let Some(pulse) = led.pulse.filter(|c| *c != 0) {
            Some(Light::Pulse(pulse))
        } else if let Some(flash) = led.flash.filter(|c| *c != 0) {
            Some(Light::Flash {
                a: led.fixed,
                b: flash,
            })
        } else {
            (led.fixed != 0).then_some(Light::Static(led.fixed))
        }
    }

    /// A Session pad's light, at a column and a row from the top.
    #[must_use]
    pub fn pad(&self, column: u8, row: u8) -> Option<Light> {
        self.light(Layer::Pad, PAD_TOP + 0x10 * row + column)
    }

    /// A drum pad's light, by its light index (36 to 51).
    #[must_use]
    pub fn drum_pad(&self, index: u8) -> Option<Light> {
        self.light(Layer::Drum, index)
    }

    /// A button's light, by its controller number.
    #[must_use]
    pub fn button(&self, cc: u8) -> Option<Light> {
        self.light(Layer::Button, cc)
    }

    /// How many pads and buttons are lit.
    #[must_use]
    pub fn lit(&self) -> usize {
        self.leds
            .keys()
            .filter(|(layer, number)| self.light(*layer, *number).is_some())
            .count()
    }

    /// How many were still lit when the controller was sent back to
    /// Standalone, or `None` while it hasn't been.
    #[must_use]
    pub const fn lit_when_left(&self) -> Option<usize> {
        self.lit_when_left
    }

    /// Whether the controller is in DAW mode.
    #[must_use]
    pub const fn daw_mode_on(&self) -> bool {
        self.daw_mode
    }

    /// The pad mode, pot (or encoder) mode and fader mode the controller
    /// shows, as its reports number them.
    #[must_use]
    pub const fn modes(&self) -> [u8; 3] {
        self.modes
    }

    /// Whether the DAW has the MK4's drum pads.
    #[must_use]
    pub const fn drum_claimed(&self) -> bool {
        self.drum_claimed
    }

    /// The scale the MK4 was last sent: whether it is on, the root and the
    /// mode's index.
    #[must_use]
    pub const fn scale(&self) -> (bool, u8, u8) {
        self.scale
    }

    /// A row of the MK3's default display.
    #[must_use]
    pub fn row(&self, row: usize) -> &str {
        &self.rows[row]
    }

    /// Whether the MK3's default display was given back.
    #[must_use]
    pub const fn display_cleared(&self) -> bool {
        self.cleared
    }

    /// The MK3's name for a control (38h to 3Fh pots, 50h to 58h faders).
    #[must_use]
    pub fn name(&self, index: u8) -> &str {
        self.names.get(&index).map_or("", String::as_str)
    }

    /// The MK3's value text for a control.
    #[must_use]
    pub fn value(&self, index: u8) -> &str {
        self.values.get(&index).map_or("", String::as_str)
    }

    /// One of the MK4's displays, by target.
    #[must_use]
    pub fn screen(&self, target: u8) -> Screen {
        self.screens.get(&target).cloned().unwrap_or_default()
    }

    /// Every message the surface has sent it, as [`Launchkey::pump`] saw
    /// them.
    #[must_use]
    pub fn messages(&self) -> &[Vec<u8>] {
        &self.messages
    }

    // What it sends ---------------------------------------------------------

    fn send(&self, bytes: &[u8]) {
        assert!(
            self.backend.send(&self.daw_port(), Instant::now(), bytes),
            "the surface's input isn't open"
        );
    }

    /// Presses the Session pad at a column and a row from the top.
    pub fn pad_down(&self, column: u8, row: u8) {
        self.send(&[0x90, PAD_TOP + 0x10 * row + column, 100]);
    }

    /// Releases it.
    pub fn pad_up(&self, column: u8, row: u8) {
        self.send(&[0x90, PAD_TOP + 0x10 * row + column, 0]);
    }

    /// Presses a Device Select pad (0 to 15) of the MK3.
    pub fn select_pad_down(&self, index: u8) {
        let note = if index < 8 { 0x40 } else { 0x50 } + index % 8;
        self.send(&[0x90, note, 100]);
    }

    /// Presses a drum pad of the MK4, by its light index (36 to 51), as a
    /// note on channel 10.
    pub fn drum_down(&self, index: u8) {
        assert!(DRUM.contains(&index));
        self.send(&[0x99, index, 100]);
    }

    /// Releases it.
    pub fn drum_up(&self, index: u8) {
        self.send(&[0x99, index, 0]);
    }

    /// The channel a button reports on: the MK3's > and Stop/Solo/Mute use
    /// channel 1, everything else channel 16.
    const fn button_status(&self, cc: u8) -> u8 {
        match (self.family, cc) {
            (Gen::Mk3, 0x68 | 0x69) => 0xB0,
            _ => 0xBF,
        }
    }

    /// Presses a button by its controller number.
    pub fn button_down(&self, cc: u8) {
        self.send(&[self.button_status(cc), cc, 127]);
    }

    /// Releases a button.
    pub fn button_up(&self, cc: u8) {
        self.send(&[self.button_status(cc), cc, 0]);
    }

    /// Turns pot (or encoder) `index`, from 0, to `value`.
    pub fn pot_to(&self, index: u8, value: u8) {
        self.send(&[0xBF, 0x15 + index, value]);
    }

    /// Moves fader `index`, from 0 (the ninth is the Main track's), to
    /// `value`.
    pub fn fader_to(&self, index: u8, value: u8) {
        let first = match self.family {
            Gen::Mk3 => 0x35,
            Gen::Mk4 => 0x05,
        };
        self.send(&[0xBF, first + index, value]);
    }

    /// Presses fader button `index`, from 0; the ninth is the switch.
    pub fn fader_button(&self, index: u8) {
        self.button_down(0x25 + index);
        self.button_up(0x25 + index);
    }

    /// Turns Transport encoder `index` by `steps`, positive clockwise (the
    /// MK4's relative controllers 55h to 5Ch).
    pub fn encoder_turn(&self, index: u8, steps: i8) {
        self.send(&[0xBF, 0x55 + index, (64 + steps) as u8]);
    }

    /// The person changes a mode: the controller shows it and reports it.
    fn report(&mut self, which: usize, value: u8) {
        self.modes[which] = value;
        let number = self.mode_numbers()[which];
        self.send(&[self.mode_status(), number, value]);
    }

    /// The person picks a pad mode.
    pub fn set_pad_mode(&mut self, value: u8) {
        self.report(0, value);
    }

    /// The person picks a pot or encoder mode.
    pub fn set_pot_mode(&mut self, value: u8) {
        self.report(1, value);
    }

    /// The person picks a fader mode.
    pub fn set_fader_mode(&mut self, value: u8) {
        self.report(2, value);
    }

    /// The person holds Device Select: the pads become the Device Select
    /// pads, which the controller reports, and the button is pressed.
    pub fn hold_device_select(&mut self) {
        self.set_pad_mode(9);
        self.button_down(0x33);
    }

    /// The person lets go of Device Select.
    pub fn release_device_select(&mut self) {
        self.button_up(0x33);
        self.set_pad_mode(2);
    }
}
