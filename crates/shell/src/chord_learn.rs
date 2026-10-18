//! Learn for Chord, the chord MIDI effect (WP-5.30; the MIDI chord spec): while
//! a Chord device's Learn is on, the keys played set its shifts.

use bl_core::midi::MidiMessage;
use bl_devices::stack::learn;

/// The keys held down since a chord began, in the order they went down.
#[derive(Debug, Default)]
pub(crate) struct ChordLearn {
    held: Vec<u8>,
    /// Whether the MIDI input is copying notes for it.
    pub(crate) tapping: bool,
    /// Scratch for the messages read from the MIDI threads.
    pub(crate) heard: Vec<MidiMessage>,
}

impl ChordLearn {
    /// Takes what `heard` holds, and returns the shifts to set if the chord
    /// changed. A chord begins with the first key down after all were up:
    /// that key is the root.
    pub(crate) fn listen(&mut self) -> Option<[i32; 6]> {
        let before = self.held.clone();
        for message in self.heard.drain(..) {
            let key = message.data1();
            if message.is_note_on() {
                if !self.held.contains(&key) {
                    self.held.push(key);
                }
            } else {
                self.held.retain(|&k| k != key);
            }
        }
        (self.held != before && !self.held.is_empty()).then(|| learn(&self.held))
    }

    /// Forgets the chord, as when Learn goes off.
    pub(crate) fn clear(&mut self) {
        self.held.clear();
        self.heard.clear();
    }
}
