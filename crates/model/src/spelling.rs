//! How the note editor spells the keys (WP-6.7b; the scales spec, R24): one
//! choice for every editor, and the names it gives the piano ruler's rows
//! and the Root Note choosers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::scale::Scale;

/// How the keys are spelled (the scales spec, R24). The labels are
/// Beatlayer's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoteSpelling {
    /// C#, as the rows read today.
    #[default]
    Sharps,
    /// D♭.
    Flats,
    /// Both: C#/D♭.
    SharpsAndFlats,
    /// Flats when the clip's scale has a root of F, B♭, E♭, A♭, D♭ or G♭,
    /// and sharps otherwise.
    Auto,
    /// The key's MIDI note number.
    NoteNumbers,
}

impl NoteSpelling {
    /// Every choice, in the order the piano ruler's menu lists them.
    pub const ALL: [Self; 5] = [
        Self::Sharps,
        Self::Flats,
        Self::SharpsAndFlats,
        Self::Auto,
        Self::NoteNumbers,
    ];

    /// The choice as the menu shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sharps => "Sharps",
            Self::Flats => "Flats",
            Self::SharpsAndFlats => "Sharps and Flats",
            Self::Auto => "Auto",
            Self::NoteNumbers => "MIDI Note Numbers",
        }
    }

    /// The name the command takes.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Sharps => "sharps",
            Self::Flats => "flats",
            Self::SharpsAndFlats => "sharps_and_flats",
            Self::Auto => "auto",
            Self::NoteNumbers => "note_numbers",
        }
    }

    /// Auto settled: Flats where the scale is on and its root is one of the
    /// flat keys, Sharps otherwise; the other choices as they are.
    fn settled(self, scale: &Scale) -> Self {
        match self {
            Self::Auto if scale.enabled && matches!(scale.root, 1 | 3 | 5 | 6 | 8 | 10) => {
                Self::Flats
            }
            Self::Auto => Self::Sharps,
            other => other,
        }
    }

    /// The name of the pitch class `class` (0 is C) under this choice, for
    /// the Root Note choosers.
    #[must_use]
    pub fn class_name(self, class: u8, scale: &Scale) -> String {
        let class = usize::from(class % 12);
        match self.settled(scale) {
            Self::Flats => FLATS[class].to_owned(),
            Self::SharpsAndFlats if FLATS[class] != SHARPS[class] => {
                format!("{}/{}", SHARPS[class], FLATS[class])
            }
            Self::NoteNumbers => class.to_string(),
            _ => SHARPS[class].to_owned(),
        }
    }

    /// The name of `key` under this choice, with its octave as Live counts
    /// them (C3 is key 60). Where the scale is on and has seven notes, a key
    /// in it takes one letter a degree from the root's letter up, and the
    /// choice names only the keys outside it (the scales spec, R24).
    #[must_use]
    pub fn key_name(self, key: u8, scale: &Scale) -> String {
        let settled = self.settled(scale);
        if settled == Self::NoteNumbers {
            return key.to_string();
        }
        if let Some(name) = degree_name(settled, key, scale) {
            return name;
        }
        let octave = i32::from(key) / 12 - 2;
        format!("{}{octave}", settled.class_name(key % 12, scale))
    }
}

const SHARPS: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
const FLATS: [&str; 12] = [
    "C", "D♭", "D", "E♭", "E", "F", "G♭", "G", "A♭", "A", "B♭", "B",
];
/// The seven letters, from C, and the pitch class of each.
const LETTERS: [(char, i32); 7] = [
    ('C', 0),
    ('D', 2),
    ('E', 4),
    ('F', 5),
    ('G', 7),
    ('A', 9),
    ('B', 11),
];

/// The name of `key` in a seven-note scale that is on and holds it: the
/// letter its degree gives, with the accidentals that reach its pitch.
/// `settled` names the root's letter: the flat or the sharp spelling of its
/// pitch class.
fn degree_name(settled: NoteSpelling, key: u8, scale: &Scale) -> Option<String> {
    let steps = scale.steps();
    if !scale.enabled || steps.len() != 7 {
        return None;
    }
    let above = (i32::from(key) - i32::from(scale.root)).rem_euclid(12);
    let degree = steps.iter().position(|step| i32::from(*step) == above)?;
    let root_name = if settled == NoteSpelling::Flats {
        FLATS[usize::from(scale.root)]
    } else {
        SHARPS[usize::from(scale.root)]
    };
    let root_letter = LETTERS
        .iter()
        .position(|(letter, _)| root_name.starts_with(*letter))?;
    let (letter, natural) = LETTERS[(root_letter + degree) % 7];
    // How many semitones the key is above its letter's natural, within a
    // double flat to a double sharp.
    let accidental = (i32::from(key % 12) - natural + 18).rem_euclid(12) - 6;
    let octave = (i32::from(key) - accidental).div_euclid(12) - 2;
    let mark = if accidental < 0 { "♭" } else { "#" };
    let marks = mark.repeat(accidental.unsigned_abs() as usize);
    Some(format!("{letter}{marks}{octave}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::ScaleName;

    #[test]
    fn sharps_read_as_the_rows_read_today() {
        // Spec: scales R24.
        let off = Scale::default();
        assert_eq!(NoteSpelling::Sharps.key_name(61, &off), "C#3");
        assert_eq!(NoteSpelling::Flats.key_name(61, &off), "D♭3");
        assert_eq!(NoteSpelling::SharpsAndFlats.key_name(61, &off), "C#/D♭3");
        assert_eq!(NoteSpelling::SharpsAndFlats.key_name(60, &off), "C3");
        assert_eq!(NoteSpelling::NoteNumbers.key_name(61, &off), "61");
    }

    #[test]
    fn auto_spells_flats_for_the_flat_roots_while_the_scale_is_on() {
        // Spec: scales R24. F Major reads B♭ and E reads as the scale's
        // letters; C Major and a scale that is off read with sharps.
        let f_major = Scale::new(5, ScaleName::Major);
        assert_eq!(NoteSpelling::Auto.key_name(70, &f_major), "B♭3");
        let c_major = Scale::new(0, ScaleName::Major);
        assert_eq!(NoteSpelling::Auto.key_name(70, &c_major), "A#3");
        let f_off = Scale {
            enabled: false,
            ..f_major
        };
        assert_eq!(NoteSpelling::Auto.key_name(70, &f_off), "A#3");
    }

    #[test]
    fn a_seven_note_scale_spells_a_letter_a_degree() {
        // Spec: scales R24. G♭ Major has C♭ (the B below C4), F# Major has E#
        // and D Major has F#; the choice names only the keys outside the
        // scale.
        let g_flat = Scale::new(6, ScaleName::Major);
        assert_eq!(NoteSpelling::Flats.key_name(71, &g_flat), "C♭4");
        let f_sharp = Scale::new(6, ScaleName::Major);
        assert_eq!(NoteSpelling::Sharps.key_name(65, &f_sharp), "E#3");
        let d_major = Scale::new(2, ScaleName::Major);
        assert_eq!(NoteSpelling::Flats.key_name(66, &d_major), "F#3");
        assert_eq!(NoteSpelling::Flats.key_name(63, &d_major), "E♭3");
        assert_eq!(NoteSpelling::Sharps.key_name(63, &d_major), "D#3");
        // A pentatonic scale has no letter a degree.
        let pentatonic = Scale::new(5, ScaleName::MajorPentatonic);
        assert_eq!(NoteSpelling::Sharps.key_name(70, &pentatonic), "A#3");
    }

    #[test]
    fn the_root_choosers_follow_the_choice() {
        let f_major = Scale::new(5, ScaleName::Major);
        assert_eq!(NoteSpelling::Auto.class_name(10, &f_major), "B♭");
        assert_eq!(NoteSpelling::Sharps.class_name(10, &f_major), "A#");
        assert_eq!(
            NoteSpelling::SharpsAndFlats.class_name(10, &f_major),
            "A#/B♭"
        );
        assert_eq!(NoteSpelling::NoteNumbers.class_name(10, &f_major), "10");
    }
}
