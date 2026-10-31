//! Learn for Chord, the chord MIDI effect (WP-5.30; the MIDI chord spec): while
//! a Chord device's Learn is on, the keys played set its shifts.

use bl_core::midi::MidiMessage;

/// The keys held down since a chord began, in the order they went down.
#[derive(Debug, Default)]
pub(crate) struct ChordLearn {
    held: Vec<u8>,
    /// The keys that went down since all were last up, in order: the chord
    /// being learned. A release leaves it as it is.
    chord: Vec<u8>,
    /// Messages heard while a gesture was open, learned once it ends.
    pub(crate) deferred: Vec<MidiMessage>,
    /// Whether the MIDI input is copying messages for it.
    pub(crate) tapping: bool,
}

impl ChordLearn {
    /// Reads the note messages in `heard` and returns the chord, in the order
    /// its keys went down, if a key was added to it; [`learn`] turns it into
    /// shifts. A chord begins with the first key down after all were up: that
    /// key is the root. Releasing keys changes nothing. Anything that isn't
    /// a note is skipped.
    ///
    /// [`learn`]: bl_devices::stack::learn
    pub(crate) fn listen_from(&mut self, heard: &[MidiMessage]) -> Option<Vec<u8>> {
        let mut added = false;
        for message in heard {
            if !(message.is_note_on() || message.is_note_off()) {
                continue;
            }
            let key = message.data1();
            if message.is_note_on() && message.data2() > 0 {
                if self.held.is_empty() {
                    self.chord.clear();
                }
                if !self.held.contains(&key) {
                    self.held.push(key);
                }
                if !self.chord.contains(&key) {
                    self.chord.push(key);
                    added = true;
                }
            } else {
                self.held.retain(|&k| k != key);
            }
        }
        added.then(|| self.chord.clone())
    }

    /// Forgets the chord, as when Learn goes off.
    pub(crate) fn clear(&mut self) {
        self.held.clear();
        self.chord.clear();
        self.deferred.clear();
    }
}
