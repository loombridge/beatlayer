//! Simulated Launchpads for tests (`docs/specs/supported-controllers.md`,
//! R5): virtual ports on the test MIDI system with the controller's port
//! names, which answer the device inquiry as it does, apply every message
//! the surface documents to a model of the lights and faders, and send a
//! pad's, button's or fader's messages when a test presses or moves it.
//!
//! Call [`Launchpad::pump`] after the host has handled a tick or a message
//! to bring the model up to date with what the surface sent.

use std::collections::BTreeMap;
use std::time::Instant;

use bl_midi_io::{Direction, TestBackend};

use super::{FADER_CC, Layer, Light, Model, SCENE, light_bytes, pad_note};

/// How a grid fader is set up and where it stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fader {
    /// Whether its colour isn't 0, so it shows.
    pub shown: bool,
    /// Whether it is bipolar.
    pub bipolar: bool,
    /// Its controller number.
    pub cc: u8,
    /// Its palette colour.
    pub colour: u8,
    /// Its position, 0 to 127.
    pub position: u8,
}

/// A fader bank: its orientation and eight faders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bank {
    /// Whether the faders lie across, not up.
    pub horizontal: bool,
    /// The faders.
    pub faders: [Fader; 8],
}

#[derive(Clone, Copy, Debug, Default)]
struct Led {
    fixed: u8,
    flash: Option<u8>,
    pulse: Option<u8>,
}

/// A simulated Launchpad.
pub struct Launchpad {
    backend: TestBackend,
    model: Model,
    seen: usize,
    daw_mode: bool,
    layout: (u8, u8),
    drum_mode: u8,
    leds: BTreeMap<(Layer, u8), Led>,
    banks: [Bank; 4],
    messages: Vec<Vec<u8>>,
    lit_when_left: Option<usize>,
}

impl Launchpad {
    fn new(backend: &TestBackend, model: Model) -> Self {
        Self {
            backend: backend.clone(),
            model,
            seen: 0,
            daw_mode: false,
            layout: (1, 0),
            drum_mode: 0,
            leds: BTreeMap::new(),
            banks: [Bank::default(); 4],
            messages: Vec::new(),
            lit_when_left: None,
        }
    }

    /// A Launchpad X on `backend`; call [`Launchpad::plug_in`] to connect it.
    #[must_use]
    pub fn x(backend: &TestBackend) -> Self {
        Self::new(backend, Model::X)
    }

    /// A Launchpad Mini MK3.
    #[must_use]
    pub fn mini(backend: &TestBackend) -> Self {
        Self::new(backend, Model::Mini)
    }

    /// A Launchpad Pro MK3.
    #[must_use]
    pub fn pro(backend: &TestBackend) -> Self {
        Self::new(backend, Model::Pro)
    }

    /// The name of the controller's DAW ports, which are the surface's.
    #[must_use]
    pub const fn daw_port(&self) -> &'static str {
        match self.model {
            Model::X => "LPX DAW",
            Model::Mini => "LPMiniMK3 DAW",
            Model::Pro => "LPProMK3 DAW",
        }
    }

    /// The name of its MIDI ports, which stay ordinary MIDI ports.
    #[must_use]
    pub const fn midi_port(&self) -> &'static str {
        match self.model {
            Model::X => "LPX MIDI",
            Model::Mini => "LPMiniMK3 MIDI",
            Model::Pro => "LPProMK3 MIDI",
        }
    }

    /// Plugs the controller in: its ports appear.
    pub fn plug_in(&self) {
        let mut names = vec![self.daw_port(), self.midi_port()];
        if self.model == Model::Pro {
            names.push("LPProMK3 DIN");
        }
        for name in names {
            self.backend.add_port(Direction::Input, name);
            self.backend.add_port(Direction::Output, name);
        }
    }

    /// Unplugs it.
    pub fn unplug(&self) {
        let mut names = vec![self.daw_port(), self.midi_port()];
        if self.model == Model::Pro {
            names.push("LPProMK3 DIN");
        }
        for name in names {
            self.backend.remove_port(Direction::Input, name);
            self.backend.remove_port(Direction::Output, name);
        }
    }

    // What the surface sent ----------------------------------------------

    /// Applies what the surface has sent to the DAW output since the last
    /// call, and answers what it asks.
    pub fn pump(&mut self) {
        let sent = self.backend.sent(self.daw_port());
        for bytes in sent.iter().skip(self.seen) {
            self.messages.push(bytes.clone());
            self.apply(bytes);
        }
        self.seen = sent.len();
    }

    fn header(&self) -> [u8; 6] {
        [0xF0, 0x00, 0x20, 0x29, 0x02, self.model.id()]
    }

    fn reply(&self, bytes: &[u8]) {
        self.backend.send(self.daw_port(), Instant::now(), bytes);
    }

    fn apply(&mut self, bytes: &[u8]) {
        let Some(&status) = bytes.first() else {
            return;
        };
        if status == 0xF0 {
            if bytes == [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7] {
                // The device inquiry's reply (application).
                self.reply(&[
                    0xF0, 0x7E, 0x00, 0x06, 0x02, 0x00, 0x20, 0x29, 0x13, 0x01, 0x00, 0x00, 1, 2,
                    3, 4, 0xF7,
                ]);
            } else if bytes.len() > 7 && bytes[..6] == self.header() {
                self.sysex(&bytes[6..bytes.len() - 1]);
            }
            return;
        }
        let (kind, channel) = (status & 0xF0, status & 0x0F);
        let (a, b) = (bytes.get(1).copied().unwrap_or(0), bytes.get(2).copied().unwrap_or(0));
        let layer = match (kind, channel) {
            (0x90 | 0x80, 0..=2) => Some((Layer::Pad, channel)),
            (0x90 | 0x80, 8..=10) => Some((Layer::Drum, channel - 8)),
            (0xB0, 0..=2) => Some((Layer::Button, channel)),
            _ => None,
        };
        if let Some((layer, mode)) = layer {
            let c = if kind == 0x80 { 0 } else { b };
            let led = self.leds.entry((layer, a)).or_default();
            match mode {
                0 => *led = Led { fixed: c, flash: None, pulse: None },
                1 => led.flash = Some(c),
                _ => led.pulse = Some(c),
            }
            return;
        }
        match (kind, channel) {
            (0xB0, 4) => {
                for bank in &mut self.banks {
                    for fader in bank.faders.iter_mut().filter(|f| f.cc == a && f.shown) {
                        fader.position = b;
                    }
                }
            }
            (0xB0, 5) => {
                for bank in &mut self.banks {
                    for fader in bank.faders.iter_mut().filter(|f| f.cc == a) {
                        fader.colour = b;
                        fader.shown = b != 0;
                    }
                }
            }
            _ => {}
        }
    }

    fn sysex(&mut self, body: &[u8]) {
        match body {
            // DAW mode, or Standalone, which clears the DAW's state.
            [0x10, mode] => {
                if *mode == 0 {
                    self.lit_when_left = Some(self.lit());
                }
                self.daw_mode = *mode == 1;
                self.leds.clear();
                self.banks = [Bank::default(); 4];
                self.drum_mode = 0;
                self.layout = if self.daw_mode { (0, 0) } else { (1, 0) };
            }
            [0x00, layout] => self.layout = (*layout, 0),
            [0x00, layout, page, 0x00] => {
                self.layout = (*layout, *page);
                self.report_layout();
            }
            [0x0F, mode] => self.drum_mode = *mode,
            [0x01, rest @ ..] => self.set_up_faders(rest),
            _ => {}
        }
    }

    fn set_up_faders(&mut self, rest: &[u8]) {
        let (bank, rest) = match self.model {
            Model::Pro => (usize::from(rest[0]).min(3), &rest[1..]),
            _ => (0, &rest[1..]),
        };
        let Some((&orientation, faders)) = rest.split_first() else {
            return;
        };
        let bank = &mut self.banks[bank];
        bank.horizontal = orientation == 1;
        for entry in faders.chunks_exact(4) {
            if let Some(fader) = bank.faders.get_mut(usize::from(entry[0])) {
                let position = fader.position;
                *fader = Fader {
                    shown: entry[3] != 0,
                    bipolar: entry[1] == 1,
                    cc: entry[2],
                    colour: entry[3],
                    position,
                };
            }
        }
    }

    /// The Pro tells the DAW each layout it shows (R29).
    fn report_layout(&self) {
        if self.model == Model::Pro {
            let mut bytes = self.header().to_vec();
            bytes.extend([0x00, self.layout.0, self.layout.1, 0x00, 0xF7]);
            self.reply(&bytes);
        }
    }

    // What it shows -------------------------------------------------------

    /// How a pad or button is lit, if it is. A Session pad is at a column
    /// and a row from the top.
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
        self.light(Layer::Pad, pad_note(column, row))
    }

    /// A drum layout pad's light.
    #[must_use]
    pub fn drum_pad(&self, column: u8, row: u8) -> Option<Light> {
        self.light(Layer::Drum, pad_note(column, row))
    }

    /// A button's light, by its controller number.
    #[must_use]
    pub fn button(&self, cc: u8) -> Option<Light> {
        self.light(Layer::Button, cc)
    }

    /// A scene button's light, from the top.
    #[must_use]
    pub fn scene_button(&self, row: usize) -> Option<Light> {
        self.button(SCENE[row])
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

    /// Whether nothing is lit.
    #[must_use]
    pub fn all_dark(&self) -> bool {
        self.lit() == 0
    }

    /// Whether the controller is in DAW mode.
    #[must_use]
    pub const fn daw_mode(&self) -> bool {
        self.daw_mode
    }

    /// The layout it shows, and its page.
    #[must_use]
    pub const fn layout(&self) -> (u8, u8) {
        self.layout
    }

    /// Its drum layout mode: 0 for its own Note mode, 1 for the DAW's drum
    /// layout.
    #[must_use]
    pub const fn drum_mode(&self) -> u8 {
        self.drum_mode
    }

    /// A fader bank: the X's one is 0; the Pro's are Volumes, Pans, Sends and
    /// Devices.
    #[must_use]
    pub const fn bank(&self, bank: usize) -> Bank {
        self.banks[bank]
    }

    /// How many clock pulses it has been sent, and when each was stamped
    /// for.
    #[must_use]
    pub fn clock(&self) -> Vec<Instant> {
        self.backend
            .sent_at(self.daw_port())
            .into_iter()
            .filter(|(_, bytes)| bytes == &[0xF8])
            .map(|(at, _)| at)
            .collect()
    }

    /// Every message the surface has sent it, as [`Launchpad::pump`] saw
    /// them.
    #[must_use]
    pub fn messages(&self) -> &[Vec<u8>] {
        &self.messages
    }

    /// The light messages that would show `light` on a pad, for a test that
    /// checks the bytes.
    #[must_use]
    pub fn bytes_for(layer: Layer, number: u8, light: Option<Light>) -> Vec<Vec<u8>> {
        light_bytes(layer, number, light)
    }

    // What it sends ---------------------------------------------------------

    fn send(&self, bytes: &[u8], at: Instant) {
        assert!(
            self.backend.send(self.daw_port(), at, bytes),
            "the surface's input isn't open"
        );
    }

    /// Presses the Session pad at a column and a row from the top.
    pub fn pad_down(&self, column: u8, row: u8) {
        self.send(&[0x90, pad_note(column, row), 100], Instant::now());
    }

    /// Releases it.
    pub fn pad_up(&self, column: u8, row: u8) {
        self.send(&[0x90, pad_note(column, row), 0], Instant::now());
    }

    /// Presses a drum layout pad (channel 9).
    pub fn drum_down(&self, column: u8, row: u8) {
        self.send(&[0x98, pad_note(column, row), 100], Instant::now());
    }

    /// Releases it.
    pub fn drum_up(&self, column: u8, row: u8) {
        self.send(&[0x98, pad_note(column, row), 0], Instant::now());
    }

    /// Presses a button by its controller number; the controller changes
    /// layout by itself for its mode buttons.
    pub fn button_down(&mut self, cc: u8) {
        self.button_down_at(cc, Instant::now());
    }

    /// Presses a button as if at `at`.
    pub fn button_down_at(&mut self, cc: u8, at: Instant) {
        match (self.model, cc) {
            (Model::X, 95) | (Model::Mini, 95) => self.layout = (0, 0),
            (Model::X, 96) => self.layout = (1, 0),
            (Model::X, 97) => self.layout = (4, 0),
            (Model::Pro, 93) => {
                self.layout = (0, 0);
                self.report_layout();
            }
            (Model::Pro, 94) => {
                self.layout = (4, 0);
                self.report_layout();
            }
            _ => {}
        }
        self.send(&[0xB0, cc, 127], at);
    }

    /// Releases a button.
    pub fn button_up(&self, cc: u8) {
        self.send(&[0xB0, cc, 0], Instant::now());
    }

    /// Releases a button as if at `at`.
    pub fn button_up_at(&self, cc: u8, at: Instant) {
        self.send(&[0xB0, cc, 0], at);
    }

    /// Moves fader `index` of the bank in view to `value`.
    pub fn fader_to(&mut self, index: usize, value: u8) {
        let bank = match self.model {
            Model::Pro if self.layout.0 == 1 => usize::from(self.layout.1).min(3),
            _ => 0,
        };
        let fader = &mut self.banks[bank].faders[index];
        fader.position = value;
        let cc = if fader.cc == 0 {
            FADER_CC + index as u8
        } else {
            fader.cc
        };
        self.send(&[0xB4, cc, value], Instant::now());
    }
}
