//! A simulated Push 2 for tests (`docs/specs/push-2.md`, R36).
//!
//! It stands in for the device on the test MIDI backend: the virtual Live and
//! User ports with the macOS names, a responder that answers Device Inquiry,
//! Request Statistics, each Get command and Set MIDI Mode, a model of every
//! LED that applies what it is sent, and a frame sink in place of the USB
//! display. A test presses and releases pads, turns and touches encoders,
//! moves the touch strip, presses buttons and footswitches, and reads LEDs,
//! sysex and frames, with no device and no window.

use bl_core::clock::TestClock;
use bl_midi_io::{Direction, TestBackend};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::controls;
use crate::display::{Claim, Sink};
use crate::frame;
use crate::leds::Led;
use crate::sysex::{self, MidiMode};

/// The Live port's name on macOS.
pub const LIVE: &str = "Ableton Push 2 Live Port";
/// The User port's name on macOS.
pub const USER: &str = "Ableton Push 2 User Port";

/// The identity reply of the interface manual's example: firmware 1.0, build
/// 47.
const IDENTITY: [u8; 23] = [
    0xF0, 0x7E, 0x01, 0x06, 0x02, 0x00, 0x21, 0x1D, 0x67, 0x32, 0x02, 0x00, 0x01, 0x00, 0x2F, 0x00,
    0x73, 0x4D, 0x1F, 0x08, 0x00, 0x01, 0xF7,
];

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The frames the simulated display received, as RGB.
#[derive(Clone, Default)]
pub struct Frames {
    shared: Arc<Mutex<FrameState>>,
}

#[derive(Default)]
struct FrameState {
    frames: Vec<Vec<u8>>,
    bad: usize,
    /// How many claims fail before one works.
    failing: u32,
    claims: u32,
}

impl Frames {
    /// Every frame received, oldest first.
    #[must_use]
    pub fn all(&self) -> Vec<Vec<u8>> {
        lock(&self.shared).frames.clone()
    }

    /// How many frames were received.
    #[must_use]
    pub fn count(&self) -> usize {
        lock(&self.shared).frames.len()
    }

    /// The latest frame.
    #[must_use]
    pub fn latest(&self) -> Option<Vec<u8>> {
        lock(&self.shared).frames.last().cloned()
    }

    /// How many frames the simulated display couldn't decode.
    #[must_use]
    pub fn bad(&self) -> usize {
        lock(&self.shared).bad
    }

    /// How many times the display was claimed.
    #[must_use]
    pub fn claims(&self) -> u32 {
        lock(&self.shared).claims
    }

    /// Makes the next `n` claims fail, as a display another program holds
    /// does.
    pub fn fail_claims(&self, n: u32) {
        lock(&self.shared).failing = n;
    }

    /// The frame sink, in place of the USB display.
    #[must_use]
    pub fn claim(&self) -> Claim {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            let mut state = lock(&shared);
            state.claims += 1;
            if state.failing > 0 {
                state.failing -= 1;
                return Err("the display interface is claimed by another program".to_owned());
            }
            Ok(Box::new(FrameSink(Arc::clone(&shared))) as Box<dyn Sink>)
        })
    }
}

struct FrameSink(Arc<Mutex<FrameState>>);

impl Sink for FrameSink {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut state = lock(&self.0);
        match frame::decode(bytes) {
            Some(rgb) => state.frames.push(rgb),
            None => state.bad += 1,
        }
        Ok(())
    }
}

/// What the device holds.
pub struct Device {
    /// The MIDI mode.
    pub mode: MidiMode,
    leds: Vec<Led>,
    /// The palette: each entry's colour and white value.
    pub palette: Vec<([u8; 3], u8)>,
    /// The LED brightness, 0 to 127.
    pub led_brightness: u8,
    /// The display brightness, 0 to 255.
    pub display_brightness: u8,
    /// The touch strip's configuration flags.
    pub strip_config: u8,
    /// The touch strip's 31 LEDs.
    pub strip_leds: [u8; 31],
    /// The aftertouch mode: 0 channel pressure, 1 polyphonic key pressure.
    pub aftertouch: u8,
    /// The pad velocity curve.
    pub curve: [u8; 128],
    /// The aftertouch thresholds.
    pub thresholds: (u16, u16),
}

impl Default for Device {
    fn default() -> Self {
        let mut palette = vec![([0, 0, 0], 0u8); 128];
        palette[122] = ([204, 204, 204], 128);
        palette[123] = ([64, 64, 64], 84);
        palette[124] = ([20, 20, 20], 32);
        palette[125] = ([0, 0, 255], 0);
        palette[126] = ([0, 255, 0], 0);
        palette[127] = ([255, 0, 0], 128);
        palette[16] = ([0, 0, 0], 32);
        Self {
            mode: MidiMode::Live,
            leds: vec![Led::default(); 256],
            palette,
            led_brightness: 127,
            display_brightness: 255,
            strip_config: 0x68,
            strip_leds: [0; 31],
            aftertouch: 0,
            curve: [0; 128],
            thresholds: (0, 0),
        }
    }
}

fn slot_of_button(number: u8) -> usize {
    128 + usize::from(number)
}

/// The simulated Push 2.
pub struct Simulated {
    backend: TestBackend,
    clock: TestClock,
    consumed: usize,
    /// What the device holds.
    pub device: Device,
    sent: Vec<Vec<u8>>,
    frames: Frames,
    /// Sysex the surface sent that its allowlist doesn't take.
    pub violations: Vec<Vec<u8>>,
    /// Whether the device stays silent, answering nothing.
    pub silent: bool,
    /// Whether the device answers a Device Inquiry as some other device.
    pub impostor: bool,
}

impl Simulated {
    /// A Push 2 on `backend`, not plugged in yet; its events are stamped from
    /// `clock`.
    #[must_use]
    pub fn new(backend: &TestBackend, clock: &TestClock) -> Self {
        Self {
            backend: backend.clone(),
            clock: clock.clone(),
            consumed: 0,
            device: Device::default(),
            sent: Vec::new(),
            frames: Frames::default(),
            violations: Vec::new(),
            silent: false,
            impostor: false,
        }
    }

    /// Plugs the device in: both ports appear.
    pub fn plug_in(&mut self) {
        for name in [LIVE, USER] {
            self.backend.add_port(Direction::Input, name);
            self.backend.add_port(Direction::Output, name);
        }
    }

    /// Unplugs it: both ports go.
    pub fn unplug(&mut self) {
        for name in [LIVE, USER] {
            self.backend.remove_port(Direction::Input, name);
            self.backend.remove_port(Direction::Output, name);
        }
        self.consumed = 0;
    }

    /// Forgets what the device was told, as a power cycle does.
    pub fn power_cycle(&mut self) {
        self.device = Device::default();
        self.sent.clear();
        self.consumed = 0;
    }

    /// The frame sink and the frames it got.
    #[must_use]
    pub fn frames(&self) -> &Frames {
        &self.frames
    }

    /// Everything the surface sent the Live port, in order, whole.
    #[must_use]
    pub fn sent(&self) -> &[Vec<u8>] {
        &self.sent
    }

    /// The messages sent with a time, which is how MIDI clock goes: the
    /// moment each was stamped for, and its bytes.
    #[must_use]
    pub fn stamped(&self) -> Vec<(std::time::Instant, Vec<u8>)> {
        self.backend.sent_at(LIVE)
    }

    /// The sysex messages the surface sent, in order.
    #[must_use]
    pub fn sysex(&self) -> Vec<Vec<u8>> {
        self.sent
            .iter()
            .filter(|m| sysex::is_sysex(m))
            .cloned()
            .collect()
    }

    /// Forgets what was sent so far, so a test reads what follows.
    pub fn forget_sent(&mut self) {
        self.sent.clear();
    }

    /// What an LED shows: a button by its name, a pad as `pad_<row>_<column>`.
    #[must_use]
    pub fn led(&self, name: &str) -> Option<Led> {
        let slot = if let Some(rest) = name.strip_prefix("pad_") {
            let (row, column) = rest.split_once('_')?;
            usize::from(controls::pad_note(row.parse().ok()?, column.parse().ok()?))
        } else {
            slot_of_button(controls::button_named(name)?.number)
        };
        Some(self.device.leds[slot])
    }

    /// The RGB colour an LED shows now, through the palette.
    #[must_use]
    pub fn colour(&self, name: &str) -> Option<[u8; 3]> {
        let led = self.led(name)?;
        Some(self.device.palette[usize::from(led.colour)].0)
    }

    /// Takes what the surface sent the Live port since the last call: applies
    /// each message to the device and answers the commands that have a
    /// reply. Returns how many messages there were.
    pub fn pump(&mut self) -> usize {
        let sent = self.backend.sent(LIVE);
        let new = sent.get(self.consumed..).unwrap_or_default().to_vec();
        self.consumed = sent.len();
        for bytes in &new {
            if sysex::is_sysex(bytes) && !sysex::allowed(bytes) {
                self.violations.push(bytes.clone());
            }
            self.receive(bytes);
        }
        // The clock and its Start and Stop are read with `stamped`.
        self.sent
            .extend(new.iter().filter(|m| m[0] < 0xF8).cloned());
        new.len()
    }

    fn receive(&mut self, bytes: &[u8]) {
        if sysex::is_sysex(bytes) {
            self.command(bytes);
            return;
        }
        // In User mode the device ignores the Live port, except for the
        // User button's LED (the interface manual's "MIDI Mode").
        let (status, a, b) = (
            bytes[0],
            bytes.get(1).copied().unwrap_or(0),
            bytes.get(2).copied().unwrap_or(0),
        );
        let channel = status & 0x0F;
        let slot = match status & 0xF0 {
            0x90 if controls::pad_of(a).is_some() => Some(usize::from(a)),
            0xB0 if controls::button(a).is_some() => Some(slot_of_button(a)),
            _ => None,
        };
        let Some(slot) = slot else {
            return;
        };
        if self.device.mode == MidiMode::User && slot != slot_of_button(controls::cc::USER) {
            return;
        }
        let led = &mut self.device.leds[slot];
        if channel == 0 {
            *led = Led::steady(b);
        } else {
            led.animation = Some((b, channel));
        }
    }

    fn reply(&self, bytes: &[u8]) {
        let at = self.clock.now();
        self.backend.send(LIVE, at, bytes);
    }

    fn silent_effects(&mut self, _bytes: &[u8]) {}

    fn command(&mut self, bytes: &[u8]) {
        if self.silent {
            // The commands still take effect; the replies are lost.
            self.silent_effects(bytes);
            return;
        }
        if bytes == sysex::DEVICE_INQUIRY {
            let mut identity = IDENTITY;
            if self.impostor {
                identity[8] = 0x10;
            }
            self.reply(&identity);
            return;
        }
        let Some(body) = bytes.strip_prefix(&[0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01]) else {
            return;
        };
        let Some((&id, rest)) = body.split_first() else {
            return;
        };
        let args = rest.strip_suffix(&[0xF7]).unwrap_or(rest);
        let (backend, clock) = (self.backend.clone(), self.clock.clone());
        let reply = |bytes: &[u8]| {
            backend.send(LIVE, clock.now(), bytes);
        };
        let device = &mut self.device;
        let answer = |id: u8, args: &[u8]| sysex::command(id, args);
        match id {
            sysex::id::SET_PALETTE_ENTRY if args.len() == 9 => {
                let value = |n: usize| args[n] | args[n + 1] << 7;
                device.palette[usize::from(args[0])] = ([value(1), value(3), value(5)], value(7));
            }
            sysex::id::SET_LED_BRIGHTNESS => {
                device.led_brightness = args.first().copied().unwrap_or(0)
            }
            sysex::id::SET_DISPLAY_BRIGHTNESS => {
                device.display_brightness =
                    args.first().copied().unwrap_or(0) | args.get(1).copied().unwrap_or(0) << 7;
            }
            sysex::id::SET_MIDI_MODE => {
                let mode = args.first().copied().unwrap_or(0);
                device.mode = if mode == 1 {
                    MidiMode::User
                } else {
                    MidiMode::Live
                };
                reply(&answer(sysex::id::SET_MIDI_MODE, &[mode]));
            }
            sysex::id::SET_TOUCH_STRIP_CONFIG => {
                device.strip_config = args.first().copied().unwrap_or(0)
            }
            sysex::id::SET_TOUCH_STRIP_LEDS if args.len() == 16 => {
                for n in 0..31 {
                    device.strip_leds[n] = args[n / 2] >> (3 * (n % 2)) & 7;
                }
            }
            sysex::id::REQUEST_STATISTICS => {
                reply(&answer(
                    sysex::id::REQUEST_STATISTICS,
                    &[1, 0, 0x3F, 7, 0, 0, 0],
                ));
            }
            sysex::id::SET_PAD_PARAMETERS if args.len() == 8 => {
                let value = |n: usize| u16::from(args[n]) | u16::from(args[n + 1]) << 7;
                device.thresholds = (value(4), value(6));
            }
            sysex::id::SET_AFTERTOUCH_MODE => {
                device.aftertouch = args.first().copied().unwrap_or(0)
            }
            sysex::id::SET_VELOCITY_CURVE if args.len() == 17 => {
                let start = usize::from(args[0]);
                device.curve[start..start + 16].copy_from_slice(&args[1..]);
            }
            // The Get commands, answered from what the device holds.
            0x04 => {
                let index = usize::from(args.first().copied().unwrap_or(0) & 0x7F);
                let (rgb, white) = device.palette[index];
                let mut entry = vec![index as u8];
                for value in [rgb[0], rgb[1], rgb[2], white] {
                    entry.extend([value & 0x7F, value >> 7]);
                }
                reply(&answer(0x04, &entry));
            }
            0x07 => reply(&answer(0x07, &[device.led_brightness])),
            0x09 => {
                let value = device.display_brightness;
                reply(&answer(0x09, &[value & 0x7F, value >> 7]));
            }
            0x18 => reply(&answer(0x18, &[device.strip_config])),
            0x1F => reply(&answer(0x1F, &[device.aftertouch])),
            0x21 => {
                let index = args.first().copied().unwrap_or(0) & 0x7F;
                reply(&answer(0x21, &[index, device.curve[usize::from(index)]]));
            }
            _ => {}
        }
    }

    // ----- what a person does -----

    /// The port the device sends a control's message on: the Live port in
    /// Live mode, the User port in User mode.
    fn port(&self) -> &'static str {
        match self.device.mode {
            MidiMode::Live => LIVE,
            MidiMode::User => USER,
        }
    }

    fn send(&self, port: &str, bytes: &[u8]) -> bool {
        self.backend.send(port, self.clock.now(), bytes)
    }

    /// Presses the pad in row `row` from the bottom and column `column`
    /// from the left.
    pub fn press_pad(&self, row: u8, column: u8, velocity: u8) -> bool {
        self.send(
            self.port(),
            &[0x90, controls::pad_note(row, column), velocity],
        )
    }

    /// Releases a pad.
    pub fn release_pad(&self, row: u8, column: u8) -> bool {
        self.send(self.port(), &[0x80, controls::pad_note(row, column), 0])
    }

    /// Polyphonic key pressure on a pad.
    pub fn pressure(&self, row: u8, column: u8, value: u8) -> bool {
        self.send(self.port(), &[0xA0, controls::pad_note(row, column), value])
    }

    /// Channel pressure.
    pub fn channel_pressure(&self, value: u8) -> bool {
        self.send(self.port(), &[0xD0, value])
    }

    /// Turns encoder `index` (0 to 7, the master 8, tempo 9, swing 10) by
    /// `steps`, positive clockwise, as the relative value it sends.
    pub fn turn(&self, index: u8, steps: i32) -> bool {
        let value = if steps >= 0 {
            steps.min(63) as u8
        } else {
            (128 + steps.max(-64)) as u8
        };
        self.send(
            self.port(),
            &[
                0xB0,
                controls::ENCODERS[usize::from(index)].controller,
                value,
            ],
        )
    }

    /// A finger comes on encoder `index`.
    pub fn touch(&self, index: u8) -> bool {
        self.send(
            self.port(),
            &[0x90, controls::ENCODERS[usize::from(index)].touch, 127],
        )
    }

    /// The finger leaves encoder `index`.
    pub fn release(&self, index: u8) -> bool {
        self.send(
            self.port(),
            &[0x90, controls::ENCODERS[usize::from(index)].touch, 0],
        )
    }

    /// A finger comes on the touch strip.
    pub fn strip_touch(&self) -> bool {
        self.send(self.port(), &[0x90, controls::STRIP_TOUCH, 127])
    }

    /// The finger leaves the touch strip.
    pub fn strip_release(&self) -> bool {
        self.send(self.port(), &[0x90, controls::STRIP_TOUCH, 0])
    }

    /// Moves a finger on the touch strip to `position`, 0 at the bottom to 1
    /// at the top, as pitch bend or as controller 1, as its configuration
    /// says.
    pub fn strip_move(&self, position: f32) -> bool {
        let position = position.clamp(0.0, 1.0);
        if self.device.strip_config & 0b100 == 0 {
            let value = ((position * 255.0).round() as u16) << 6;
            self.send(
                self.port(),
                &[0xE0, (value & 0x7F) as u8, (value >> 7) as u8],
            )
        } else {
            self.send(self.port(), &[0xB0, 1, (position * 127.0).round() as u8])
        }
    }

    /// Presses a button, by its name.
    pub fn press(&self, name: &str) -> bool {
        self.button(name, 127)
    }

    /// Releases a button.
    pub fn let_go(&self, name: &str) -> bool {
        self.button(name, 0)
    }

    fn button(&self, name: &str, value: u8) -> bool {
        let Some(button) = controls::button_named(name) else {
            return false;
        };
        let message = [0xB0, button.number, value];
        // The User button is sent on both ports, in both modes.
        if button.number == controls::cc::USER {
            let live = self.send(LIVE, &message);
            let user = self.send(USER, &message);
            return live || user;
        }
        self.send(self.port(), &message)
    }

    /// Footswitch `number` (1 or 2) goes down or up.
    pub fn footswitch(&self, number: u8, down: bool) -> bool {
        let controller = if number == 1 {
            controls::cc::FOOTSWITCH_1
        } else {
            controls::cc::FOOTSWITCH_2
        };
        self.send(self.port(), &[0xB0, controller, if down { 127 } else { 0 }])
    }
}
