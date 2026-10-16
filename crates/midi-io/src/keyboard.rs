//! The computer MIDI keyboard (`docs/specs/midi-io.md`, R17 to R23).

use bl_core::midi::MidiMessage;

/// The letters that play notes, from the lowest: the home row plays white
/// keys and the row above black keys, A being C (S-MAP-006).
pub const NOTE_LETTERS: [char; 16] = [
    'A', 'W', 'S', 'E', 'D', 'F', 'T', 'G', 'Y', 'H', 'U', 'J', 'K', 'O', 'L', 'P',
];

/// The velocities C and V step through (S-MAP-005).
pub const VELOCITIES: [u8; 8] = [1, 20, 40, 60, 80, 100, 120, 127];

/// The lowest octave: A plays C-2, note 0.
pub const MIN_OCTAVE: i8 = -5;

/// The highest octave: A plays C8, note 120.
pub const MAX_OCTAVE: i8 = 5;

/// The note A plays in octave 0: C3, in Live's numbering.
const BASE_NOTE: i16 = 60;

/// The velocity a fresh keyboard plays at.
const START_VELOCITY: usize = 5;

/// The MIDI channel its notes use: channel 1.
const CHANNEL: u8 = 0;

/// What a letter does while the keyboard is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyboardKey {
    /// Plays the note this many semitones above A's (0 to 15).
    Note(u8),
    /// Z: an octave down (S-MAP-004).
    OctaveDown,
    /// X: an octave up.
    OctaveUp,
    /// C: a lower velocity (S-MAP-005).
    VelocityDown,
    /// V: a higher velocity.
    VelocityUp,
}

impl KeyboardKey {
    /// What `letter` does, upper or lower case; `None` for a letter the
    /// keyboard doesn't use, which keeps its shortcut.
    #[must_use]
    pub fn from_letter(letter: char) -> Option<Self> {
        let letter = letter.to_ascii_uppercase();
        if let Some(semitone) = NOTE_LETTERS.iter().position(|&l| l == letter) {
            return Some(Self::Note(semitone as u8));
        }
        match letter {
            'Z' => Some(Self::OctaveDown),
            'X' => Some(Self::OctaveUp),
            'C' => Some(Self::VelocityDown),
            'V' => Some(Self::VelocityUp),
            _ => None,
        }
    }

    /// The letter for this key.
    #[must_use]
    pub fn letter(self) -> char {
        match self {
            Self::Note(semitone) => NOTE_LETTERS[usize::from(semitone).min(15)],
            Self::OctaveDown => 'Z',
            Self::OctaveUp => 'X',
            Self::VelocityDown => 'C',
            Self::VelocityUp => 'V',
        }
    }
}

/// The computer MIDI keyboard: whether it's on, its octave and velocity,
/// and the notes its keys hold.
///
/// ```
/// use bl_midi_io::ComputerKeyboard;
///
/// let mut keyboard = ComputerKeyboard::new();
/// keyboard.set_on(true);
/// let on = keyboard.press(0).unwrap(); // A
/// assert_eq!(on.bytes(), &[0x90, 60, 100]);
/// keyboard.shift_octave(1); // X, while A is held
/// assert_eq!(keyboard.release(0).unwrap().bytes(), &[0x80, 60, 64]);
/// assert_eq!(keyboard.press(0).unwrap().data1(), 72);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComputerKeyboard {
    on: bool,
    octave: i8,
    velocity: usize,
    /// The note each key holds, by semitone.
    held: [Option<u8>; 16],
}

impl Default for ComputerKeyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputerKeyboard {
    /// A keyboard that's off, with A on C3 and a velocity of 100.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            on: false,
            octave: 0,
            velocity: START_VELOCITY,
            held: [None; 16],
        }
    }

    /// Whether it's on.
    #[must_use]
    pub const fn is_on(&self) -> bool {
        self.on
    }

    /// Its octave: 0 puts A on C3, -5 on C-2, 5 on C8.
    #[must_use]
    pub const fn octave(&self) -> i8 {
        self.octave
    }

    /// The velocity its notes play at.
    #[must_use]
    pub const fn velocity(&self) -> u8 {
        VELOCITIES[self.velocity]
    }

    /// The note A plays.
    #[must_use]
    pub fn lowest_note(&self) -> u8 {
        (BASE_NOTE + 12 * i16::from(self.octave)) as u8
    }

    /// The notes its keys reach, as the Status Bar shows them: `C3 to D#4`.
    #[must_use]
    pub fn range_text(&self) -> String {
        let low = self.lowest_note();
        let high = low.saturating_add(15).min(127);
        format!("{} to {}", note_name(low), note_name(high))
    }

    /// Switches it on or off. Switching it off releases every note it
    /// holds: the note-offs to send are returned.
    pub fn set_on(&mut self, on: bool) -> Vec<MidiMessage> {
        self.on = on;
        if on {
            return Vec::new();
        }
        self.held
            .iter_mut()
            .filter_map(Option::take)
            .map(|note| MidiMessage::note_off(CHANNEL, note))
            .collect()
    }

    /// Moves it `step` octaves (Z is -1, X is 1), within C-2 to C8. Returns
    /// whether the octave changed. Held notes keep sounding until their keys
    /// are released.
    pub fn shift_octave(&mut self, step: i8) -> bool {
        let next = self
            .octave
            .saturating_add(step)
            .clamp(MIN_OCTAVE, MAX_OCTAVE);
        let changed = next != self.octave;
        self.octave = next;
        changed
    }

    /// Steps the velocity through [`VELOCITIES`] (C is -1, V is 1). Returns
    /// whether it changed.
    pub fn step_velocity(&mut self, step: i8) -> bool {
        let last = VELOCITIES.len() as isize - 1;
        let next = (self.velocity as isize + isize::from(step)).clamp(0, last) as usize;
        let changed = next != self.velocity;
        self.velocity = next;
        changed
    }

    /// A key at `semitone` (0 to 15) went down: the note-on to send, or
    /// `None` if the keyboard is off, the key already holds a note, or its
    /// note would pass 127.
    pub fn press(&mut self, semitone: u8) -> Option<MidiMessage> {
        let lowest = i16::from(self.lowest_note());
        let slot = self.held.get_mut(usize::from(semitone))?;
        if !self.on || slot.is_some() {
            return None;
        }
        let note = u8::try_from(lowest + i16::from(semitone))
            .ok()
            .filter(|&n| n <= 127)?;
        *slot = Some(note);
        Some(MidiMessage::note_on(CHANNEL, note, self.velocity()))
    }

    /// A key at `semitone` came up: the note-off for the note it started,
    /// whatever the octave is now, or `None` if it held none.
    pub fn release(&mut self, semitone: u8) -> Option<MidiMessage> {
        let note = self.held.get_mut(usize::from(semitone))?.take()?;
        Some(MidiMessage::note_off(CHANNEL, note))
    }
}

/// A note's name in Live's numbering, where note 60 is C3: `C-2` for 0,
/// `F#3` for 66, `G8` for 127.
#[must_use]
pub fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = i16::from(note / 12) - 2;
    format!("{}{octave}", NAMES[usize::from(note % 12)])
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: midi-io R18
    #[test]
    fn the_letters_play_live_layout() {
        let mut keyboard = ComputerKeyboard::new();
        keyboard.set_on(true);
        let played: Vec<u8> = "AWSEDFTGYHUJKOLP"
            .chars()
            .map(|letter| {
                let KeyboardKey::Note(semitone) = KeyboardKey::from_letter(letter).unwrap() else {
                    panic!("{letter} plays a note");
                };
                keyboard.press(semitone).unwrap().data1()
            })
            .collect();
        assert_eq!(played, (60..76).collect::<Vec<_>>());
        assert_eq!(KeyboardKey::from_letter('z'), Some(KeyboardKey::OctaveDown));
        assert_eq!(KeyboardKey::from_letter('X'), Some(KeyboardKey::OctaveUp));
        assert_eq!(
            KeyboardKey::from_letter('c'),
            Some(KeyboardKey::VelocityDown)
        );
        assert_eq!(KeyboardKey::from_letter('V'), Some(KeyboardKey::VelocityUp));
        for other in ['M', 'B', 'Q', 'R', 'I', 'N', '1'] {
            assert_eq!(KeyboardKey::from_letter(other), None, "{other}");
        }
        for key in [
            KeyboardKey::Note(7),
            KeyboardKey::OctaveUp,
            KeyboardKey::VelocityDown,
        ] {
            assert_eq!(KeyboardKey::from_letter(key.letter()), Some(key));
        }
    }

    // Spec: midi-io R19
    #[test]
    fn octaves_run_from_c_minus_2_to_c8() {
        let mut keyboard = ComputerKeyboard::new();
        keyboard.set_on(true);
        assert_eq!(keyboard.range_text(), "C3 to D#4");
        while keyboard.shift_octave(-1) {}
        assert_eq!(keyboard.octave(), MIN_OCTAVE);
        assert_eq!(keyboard.press(0).unwrap().data1(), 0);
        assert_eq!(keyboard.range_text(), "C-2 to D#-1");
        while keyboard.shift_octave(1) {}
        assert_eq!(keyboard.octave(), MAX_OCTAVE);
        assert_eq!(keyboard.lowest_note(), 120);
        // G8 (127) plays; the keys above it don't.
        assert_eq!(keyboard.press(7).unwrap().data1(), 127);
        assert_eq!(keyboard.press(8), None);
        assert_eq!(keyboard.range_text(), "C8 to G8");
    }

    // Spec: midi-io R20
    #[test]
    fn velocity_steps_from_100() {
        let mut keyboard = ComputerKeyboard::new();
        assert_eq!(keyboard.velocity(), 100);
        let mut seen = vec![keyboard.velocity()];
        while keyboard.step_velocity(1) {
            seen.push(keyboard.velocity());
        }
        assert_eq!(seen, vec![100, 120, 127]);
        while keyboard.step_velocity(-1) {}
        assert_eq!(keyboard.velocity(), 1);
        keyboard.set_on(true);
        assert_eq!(keyboard.press(0).unwrap().data2(), 1);
    }

    // Spec: midi-io R21
    #[test]
    fn keys_hold_their_notes_and_off_releases_them() {
        let mut keyboard = ComputerKeyboard::new();
        assert_eq!(keyboard.press(0), None, "off plays nothing");
        keyboard.set_on(true);
        assert_eq!(keyboard.press(4), Some(MidiMessage::note_on(0, 64, 100)));
        assert_eq!(keyboard.press(4), None, "a held key doesn't repeat");
        keyboard.shift_octave(-1);
        assert_eq!(keyboard.press(0), Some(MidiMessage::note_on(0, 48, 100)));
        // Each release ends the note its key started.
        assert_eq!(keyboard.release(4), Some(MidiMessage::note_off(0, 64)));
        assert_eq!(keyboard.release(4), None);
        // Switching off ends the rest.
        assert_eq!(keyboard.set_on(false), vec![MidiMessage::note_off(0, 48)]);
        assert_eq!(keyboard.release(0), None);
        assert_eq!(keyboard.press(16), None);
    }

    #[test]
    fn notes_are_named_as_live_names_them() {
        assert_eq!(note_name(60), "C3");
        assert_eq!(note_name(0), "C-2");
        assert_eq!(note_name(66), "F#3");
        assert_eq!(note_name(127), "G8");
    }
}
