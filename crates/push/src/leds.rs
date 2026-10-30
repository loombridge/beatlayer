//! The LED state model, the palette allocator and the animations
//! (`docs/specs/push-2.md`, R13 to R15).

use crate::controls::{BUTTONS, LedKind};
use crate::sysex;

/// The white LEDs' levels (R13): off, dim (white index 16) and full (127).
pub mod white {
    /// The LED is off.
    pub const OFF: u8 = 0;
    /// The button does something now.
    pub const DIM: u8 = 16;
    /// The button's function is active, or it is held.
    pub const FULL: u8 = 127;
}

/// The palette's fixed entries and their interface manual defaults: white,
/// light grey, dark grey, blue, green and red (R14).
pub mod fixed {
    /// Black, the LED off.
    pub const BLACK: u8 = 0;
    /// White.
    pub const WHITE: u8 = 122;
    /// Light grey.
    pub const LIGHT_GREY: u8 = 123;
    /// Dark grey.
    pub const DARK_GREY: u8 = 124;
    /// Blue.
    pub const BLUE: u8 = 125;
    /// Green.
    pub const GREEN: u8 = 126;
    /// Red.
    pub const RED: u8 = 127;
}

/// The channel that pulses over a quarter note (R15).
pub const PULSE: u8 = 9;
/// The channel that blinks over an eighth note (R15).
pub const BLINK: u8 = 13;

/// An LED's colour and its animation (R13).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Led {
    /// The colour index it shows, or starts an animation from.
    pub colour: u8,
    /// The colour and channel of an animation.
    pub animation: Option<(u8, u8)>,
}

impl Led {
    /// A steady colour.
    #[must_use]
    pub const fn steady(colour: u8) -> Self {
        Self {
            colour,
            animation: None,
        }
    }

    /// Pulsing between `from` and `to` over a quarter note.
    #[must_use]
    pub const fn pulsing(from: u8, to: u8) -> Self {
        Self {
            colour: from,
            animation: Some((to, PULSE)),
        }
    }

    /// Blinking between `from` and `to` over an eighth note.
    #[must_use]
    pub const fn blinking(from: u8, to: u8) -> Self {
        Self {
            colour: from,
            animation: Some((to, BLINK)),
        }
    }

    /// A white LED for a button that is `on`, `available` or neither (R13).
    #[must_use]
    pub const fn level(available: bool, on: bool) -> Self {
        Self::steady(match (available, on) {
            (_, true) => white::FULL,
            (true, false) => white::DIM,
            (false, false) => white::OFF,
        })
    }

    /// A white LED for a button that switches something on or off: it pulses
    /// while it is on (R13).
    #[must_use]
    pub const fn switch(on: bool) -> Self {
        if on {
            Self::pulsing(white::DIM, white::FULL)
        } else {
            Self::steady(white::DIM)
        }
    }
}

/// An LED's place: a pad by its note, a button by its controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Addr {
    /// A pad.
    Pad(u8),
    /// A button.
    Button(u8),
}

impl Addr {
    const fn slot(self) -> usize {
        match self {
            Self::Pad(note) => note as usize,
            Self::Button(number) => 128 + number as usize,
        }
    }

    fn every() -> impl Iterator<Item = Self> {
        (36..100u8)
            .map(Self::Pad)
            .chain(BUTTONS.iter().map(|b| Self::Button(b.number)))
    }

    fn message(self, channel: u8, colour: u8) -> Vec<u8> {
        match self {
            Self::Pad(note) => vec![0x90 | channel, note, colour],
            Self::Button(number) => vec![0xB0 | channel, number, colour],
        }
    }
}

/// What every LED should show, and what Push 2 has been told (R13).
pub struct Leds {
    wanted: Vec<Led>,
    sent: Vec<Option<Led>>,
    strip_wanted: [u8; 31],
    strip_sent: Option<[u8; 31]>,
}

impl Default for Leds {
    fn default() -> Self {
        Self {
            wanted: vec![Led::default(); 256],
            sent: vec![None; 256],
            strip_wanted: [0; 31],
            strip_sent: None,
        }
    }
}

impl Leds {
    /// Sets what `addr` should show.
    pub fn set(&mut self, addr: Addr, led: Led) {
        self.wanted[addr.slot()] = led;
    }

    /// What `addr` should show.
    #[must_use]
    pub fn get(&self, addr: Addr) -> Led {
        self.wanted[addr.slot()]
    }

    /// Sets the touch strip's 31 LEDs, levels 0 to 7, the bottom one first
    /// (R11).
    pub fn set_strip(&mut self, levels: [u8; 31]) {
        self.strip_wanted = levels;
    }

    /// The touch strip's LEDs as they should be.
    #[must_use]
    pub const fn strip(&self) -> &[u8; 31] {
        &self.strip_wanted
    }

    /// Makes everything be sent again, as after leaving User Mode (R13).
    pub fn forget(&mut self) {
        self.sent.fill(None);
        self.strip_sent = None;
    }

    /// Turns everything off.
    pub fn clear(&mut self) {
        self.wanted.fill(Led::default());
        self.strip_wanted = [0; 31];
    }

    /// The palette indices the LEDs show, for the allocator not to reuse.
    pub fn colours(&self) -> impl Iterator<Item = u8> + '_ {
        Addr::every().flat_map(|addr| {
            let led = self.wanted[addr.slot()];
            [Some(led.colour), led.animation.map(|(to, _)| to)]
        })
        .flatten()
    }

    /// The messages that bring Push 2 to what the LEDs should show: only
    /// the LEDs that changed (R13). With `strip` the touch strip's LEDs go
    /// too, as the host controls them.
    pub fn flush(&mut self, strip: bool) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for addr in Addr::every() {
            let slot = addr.slot();
            let want = self.wanted[slot];
            if self.sent[slot] == Some(want) {
                continue;
            }
            out.push(addr.message(0, want.colour));
            if let Some((to, channel)) = want.animation {
                out.push(addr.message(channel, to));
            }
            self.sent[slot] = Some(want);
        }
        if strip && self.strip_sent != Some(self.strip_wanted) {
            out.push(sysex::touch_strip_leds(&self.strip_wanted));
            self.strip_sent = Some(self.strip_wanted);
        }
        out
    }

    /// Whether `addr` is a white LED.
    #[must_use]
    pub fn is_white(addr: Addr) -> bool {
        match addr {
            Addr::Pad(_) => false,
            Addr::Button(number) => BUTTONS
                .iter()
                .any(|b| b.number == number && b.led == LedKind::White),
        }
    }
}

/// The palette: which colour each index holds on Push 2, and which index to
/// give a colour (R14).
pub struct Palette {
    entries: [Option<[u8; 3]>; 128],
    shown: [u64; 128],
    clock: u64,
    writes: Vec<u8>,
}

impl Default for Palette {
    fn default() -> Self {
        let mut entries = [None; 128];
        for (index, rgb) in [
            (fixed::BLACK, [0, 0, 0]),
            (fixed::WHITE, [204, 204, 204]),
            (fixed::LIGHT_GREY, [64, 64, 64]),
            (fixed::DARK_GREY, [20, 20, 20]),
            (fixed::BLUE, [0, 0, 255]),
            (fixed::GREEN, [0, 255, 0]),
            (fixed::RED, [255, 0, 0]),
        ] {
            entries[usize::from(index)] = Some(rgb);
        }
        Self {
            entries,
            shown: [0; 128],
            clock: 0,
            writes: Vec::new(),
        }
    }
}

impl Palette {
    /// Whether `index` is one the allocator never rewrites: 0, the white
    /// LEDs' dim level 16, and 122 to 127 (R14).
    const fn is_fixed(index: u8) -> bool {
        index == 0 || index == white::DIM || index >= fixed::WHITE
    }

    /// The index that shows `rgb`: the one that already holds it, a free
    /// one, or the one shown least recently, preferring an index no LED
    /// uses (`in_use`). A colour that needs a new entry queues its write
    /// (R14).
    pub fn index(&mut self, rgb: [u8; 3], in_use: &dyn Fn(u8) -> bool) -> u8 {
        self.clock += 1;
        if let Some(index) = (0..128u8).find(|&i| self.entries[usize::from(i)] == Some(rgb)) {
            self.shown[usize::from(index)] = self.clock;
            return index;
        }
        let free = (1..122u8).find(|&i| !Self::is_fixed(i) && self.entries[usize::from(i)].is_none());
        let index = free.unwrap_or_else(|| {
            let candidates = || (1..122u8).filter(|&i| !Self::is_fixed(i));
            let by_age = |&i: &u8| self.shown[usize::from(i)];
            candidates()
                .filter(|&i| !in_use(i))
                .min_by_key(by_age)
                .or_else(|| candidates().min_by_key(by_age))
                .unwrap_or(1)
        });
        self.entries[usize::from(index)] = Some(rgb);
        self.shown[usize::from(index)] = self.clock;
        self.writes.push(index);
        index
    }

    /// The colour an index holds, if it is known.
    #[must_use]
    pub fn colour(&self, index: u8) -> Option<[u8; 3]> {
        self.entries.get(usize::from(index)).copied().flatten()
    }

    /// The entries written since the last call, then Reapply Color Palette;
    /// nothing if none was (R14). An entry's white value is its colour's
    /// brightness.
    pub fn take_writes(&mut self) -> Vec<Vec<u8>> {
        if self.writes.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<Vec<u8>> = std::mem::take(&mut self.writes)
            .into_iter()
            .filter_map(|index| {
                let rgb = self.entries[usize::from(index)]?;
                Some(sysex::palette_entry(index, rgb, brightness(rgb)))
            })
            .collect();
        out.push(sysex::reapply_palette());
        out
    }

    /// Queues a write of every entry the allocator has given out, as after
    /// Push 2 was set to its own state.
    pub fn rewrite_all(&mut self) {
        self.writes = (1..122u8)
            .filter(|&i| !Self::is_fixed(i) && self.entries[usize::from(i)].is_some())
            .collect();
    }
}

/// A colour's brightness, 0 to 255, for the white LEDs.
fn brightness([r, g, b]: [u8; 3]) -> u8 {
    ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114 + 500) / 1000) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: push-2 R13. Only what changed is sent, and a pad is a note-on, a
    // button a controller.
    #[test]
    fn only_changed_leds_are_sent_and_all_again_after_forgetting() {
        let mut leds = Leds::default();
        let first = leds.flush(false);
        assert_eq!(first.len(), 64 + BUTTONS.len(), "every LED once at the start");
        assert!(leds.flush(false).is_empty());

        leds.set(Addr::Pad(36), Led::steady(fixed::RED));
        leds.set(Addr::Button(85), Led::steady(fixed::GREEN));
        assert_eq!(leds.flush(false), [vec![0x90, 36, 127], vec![0xB0, 85, 126]]);
        assert!(leds.flush(false).is_empty());

        leds.forget();
        assert_eq!(leds.flush(false).len(), first.len());
    }

    // Spec: push-2 R13, R15. An animation starts from the colour on channel 0
    // and goes to its second colour on the animation's channel.
    #[test]
    fn a_pulse_sends_both_colours() {
        let mut leds = Leds::default();
        leds.flush(false);
        leds.set(Addr::Button(9), Led::switch(true));
        assert_eq!(leds.flush(false), [vec![0xB0, 9, 16], vec![0xB9, 9, 127]]);
        leds.set(Addr::Button(9), Led::switch(false));
        assert_eq!(leds.flush(false), [vec![0xB0, 9, 16]]);
    }

    // Spec: push-2 R11. The strip's 31 LEDs go as one sysex only when the host
    // lights them and they changed.
    #[test]
    fn the_strip_goes_when_the_host_lights_it() {
        let mut leds = Leds::default();
        leds.flush(false);
        leds.set_strip([3; 31]);
        assert!(leds.flush(false).is_empty());
        let sent = leds.flush(true);
        assert_eq!(sent.len(), 1);
        assert!(sysex::allowed(&sent[0]));
        assert!(leds.flush(true).is_empty());
    }

    // Spec: push-2 R14. Index 0, 16 and 122 to 127 are never given out, and
    // the entry shown least recently goes first.
    #[test]
    fn the_allocator_keeps_the_fixed_entries_and_reuses_the_least_recently_shown() {
        let mut palette = Palette::default();
        let none = |_| false;
        assert_eq!(palette.index([0, 255, 0], &none), fixed::GREEN, "a colour that has an index keeps it");
        let mut given = Vec::new();
        for n in 0..120u8 {
            given.push(palette.index([n, 1, 2], &none));
        }
        assert!(given.iter().all(|&i| i != 0 && i != 16 && i < 122));
        assert_eq!(given.iter().collect::<std::collections::BTreeSet<_>>().len(), 120);
        // Show the first again: the second is now the oldest.
        assert_eq!(palette.index([0, 1, 2], &none), given[0]);
        let reused = palette.index([200, 1, 2], &none);
        assert_eq!(reused, given[1]);
        for fixed in [0, 122, 123, 124, 125, 126, 127] {
            assert!(palette.colour(fixed).is_some());
        }
        // An index an LED shows is kept while another can go.
        let kept = given[2];
        let next = palette.index([201, 1, 2], &|i| i == kept);
        assert_eq!(next, given[3]);
    }

    // Spec: push-2 R14. A new colour is written with its brightness as the
    // white value, then the palette is reapplied.
    #[test]
    fn a_new_colour_is_written_and_the_palette_reapplied() {
        let mut palette = Palette::default();
        let index = palette.index([255, 128, 0], &|_| false);
        let writes = palette.take_writes();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0], sysex::palette_entry(index, [255, 128, 0], 151));
        assert_eq!(writes[1], sysex::reapply_palette());
        assert!(palette.take_writes().is_empty());
        palette.index([255, 128, 0], &|_| false);
        assert!(palette.take_writes().is_empty(), "a colour already held is not written again");
    }
}
