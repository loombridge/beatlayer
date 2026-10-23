//! Learn for Chord, the chord MIDI effect (WP-5.30; the MIDI chord spec): while
//! a Chord device's Learn is on, the keys played set its shifts.

use bl_core::midi::MidiMessage;

/// The keys held down since a chord began, in the order they went down.
#[derive(Debug, Default)]
pub(crate) struct ChordLearn {
    held: Vec<u8>,
    /// Whether the MIDI input is copying messages for it.
    pub(crate) tapping: bool,
}

impl ChordLearn {
    /// Reads the note messages in `heard` and returns the keys held, in the
    /// order they went down, if the chord changed; [`learn`] turns them into
    /// shifts. A chord begins with the first key down after all were up: that
    /// key is the root. Anything that isn't a note is skipped.
    ///
    /// [`learn`]: bl_devices::stack::learn
    pub(crate) fn listen_from(&mut self, heard: &[MidiMessage]) -> Option<Vec<u8>> {
        let before = self.held.clone();
        for message in heard {
            if !(message.is_note_on() || message.is_note_off()) {
                continue;
            }
            let key = message.data1();
            if message.is_note_on() && message.data2() > 0 {
                if !self.held.contains(&key) {
                    self.held.push(key);
                }
            } else {
                self.held.retain(|&k| k != key);
            }
        }
        (self.held != before && !self.held.is_empty()).then(|| self.held.clone())
    }

    /// Forgets the chord, as when Learn goes off.
    pub(crate) fn clear(&mut self) {
        self.held.clear();
    }
}
