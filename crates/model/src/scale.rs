//! The scales (WP-6.7): the scale list, and the arithmetic that moves a key
//! by scale degrees.
//!
//! A scale is a root, one of the twelve pitch classes C to B, and a name
//! from the scale list ([`ScaleName`]). Its notes are the root and the
//! steps above it the list gives, in every octave. The list, its order and
//! its steps are Beatlayer's own (the scales spec, R1).
//!
//! [`Scale`] is what a clip and the set carry: the Scale Mode switch, the
//! root and the name. A scale with its switch off is the chromatic scale,
//! whose degree is a semitone, which is what a device or a tool reads where
//! no scale is active (the scales spec, R2).
//!
//! The arithmetic is plain functions over a scale, so the devices
//! (WP-6.8), the note editor (WP-6.7b) and the MIDI Tools (WP-6.12 to
//! WP-6.16) can all use it without a UI crate.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The chromatic scale's steps: every semitone, so a degree of it is a
/// semitone (the scales spec, R2).
pub const CHROMATIC: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

/// The names of the scales in the scale list, in the order the chooser
/// shows them (the scales spec, R1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScaleName {
    /// The major scale.
    Major,
    /// The natural minor scale.
    Minor,
    /// The Dorian mode.
    Dorian,
    /// The Mixolydian mode.
    Mixolydian,
    /// The Lydian mode.
    Lydian,
    /// The Phrygian mode.
    Phrygian,
    /// The Locrian mode.
    Locrian,
    /// The harmonic minor scale.
    HarmonicMinor,
    /// The melodic minor scale.
    MelodicMinor,
    /// The harmonic major scale.
    HarmonicMajor,
    /// The major pentatonic scale.
    MajorPentatonic,
    /// The minor pentatonic scale.
    MinorPentatonic,
    /// The minor blues scale.
    MinorBlues,
    /// The whole tone scale.
    WholeTone,
    /// The whole-half diminished scale.
    WholeHalfDiminished,
    /// The half-whole diminished scale.
    HalfWholeDiminished,
}

impl ScaleName {
    /// Every scale, in the list's order (the scales spec, R1).
    pub const ALL: [Self; 16] = [
        Self::Major,
        Self::Minor,
        Self::Dorian,
        Self::Mixolydian,
        Self::Lydian,
        Self::Phrygian,
        Self::Locrian,
        Self::HarmonicMinor,
        Self::MelodicMinor,
        Self::HarmonicMajor,
        Self::MajorPentatonic,
        Self::MinorPentatonic,
        Self::MinorBlues,
        Self::WholeTone,
        Self::WholeHalfDiminished,
        Self::HalfWholeDiminished,
    ];

    /// The scale's steps above the root, in semitones (the scales spec, R1).
    #[must_use]
    pub const fn steps(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Self::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Self::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Self::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Self::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            Self::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Self::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            Self::HarmonicMajor => &[0, 2, 4, 5, 7, 8, 11],
            Self::MajorPentatonic => &[0, 2, 4, 7, 9],
            Self::MinorPentatonic => &[0, 3, 5, 7, 10],
            Self::MinorBlues => &[0, 3, 5, 6, 7, 10],
            Self::WholeTone => &[0, 2, 4, 6, 8, 10],
            Self::WholeHalfDiminished => &[0, 2, 3, 5, 6, 8, 9, 11],
            Self::HalfWholeDiminished => &[0, 1, 3, 4, 6, 7, 9, 10],
        }
    }

    /// The scale's name as the chooser shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Major => "Major",
            Self::Minor => "Minor",
            Self::Dorian => "Dorian",
            Self::Mixolydian => "Mixolydian",
            Self::Lydian => "Lydian",
            Self::Phrygian => "Phrygian",
            Self::Locrian => "Locrian",
            Self::HarmonicMinor => "Harmonic Minor",
            Self::MelodicMinor => "Melodic Minor",
            Self::HarmonicMajor => "Harmonic Major",
            Self::MajorPentatonic => "Major Pentatonic",
            Self::MinorPentatonic => "Minor Pentatonic",
            Self::MinorBlues => "Minor Blues",
            Self::WholeTone => "Whole Tone",
            Self::WholeHalfDiminished => "Whole-Half Diminished",
            Self::HalfWholeDiminished => "Half-Whole Diminished",
        }
    }

    /// The scale's name as a set file writes it: its ID, in snake case.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
            Self::Dorian => "dorian",
            Self::Mixolydian => "mixolydian",
            Self::Lydian => "lydian",
            Self::Phrygian => "phrygian",
            Self::Locrian => "locrian",
            Self::HarmonicMinor => "harmonic_minor",
            Self::MelodicMinor => "melodic_minor",
            Self::HarmonicMajor => "harmonic_major",
            Self::MajorPentatonic => "major_pentatonic",
            Self::MinorPentatonic => "minor_pentatonic",
            Self::MinorBlues => "minor_blues",
            Self::WholeTone => "whole_tone",
            Self::WholeHalfDiminished => "whole_half_diminished",
            Self::HalfWholeDiminished => "half_whole_diminished",
        }
    }

    /// The scale the name holds, if the list holds it: the name as the
    /// chooser shows it, its ID, or either in any case, with spaces,
    /// hyphens and underscores alike (the scales spec, R5).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        let wanted = normalise(name);
        Self::ALL
            .into_iter()
            .find(|scale| normalise(scale.id()) == wanted || normalise(scale.label()) == wanted)
    }
}

/// A name with its spaces, hyphens and underscores alike, and in lower
/// case, so a name can be read in any of the forms the list holds it.
fn normalise(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// A scale: the Scale Mode switch, the root, and the name (the scales
/// spec, R3).
///
/// A new set's scale, and a new clip's, is off, at C, in Major, and a set
/// file leaves that out, so a set saved before the scales loads unchanged
/// (the scales spec, R8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, JsonSchema)]
#[schemars(
    description = "A scale: whether Scale Mode is on (`enabled`), the root note (0 is C, 11 \
                   is B), and the scale's `name`, from the scale list. A scale with `enabled` \
                   false is the chromatic scale, whose degree is a semitone."
)]
pub struct Scale {
    /// Whether Scale Mode is on: the scale's keys are the ones the rest of
    /// the set reads.
    pub enabled: bool,
    /// The root note: 0 (C) to 11 (B).
    pub root: u8,
    /// The scale's name, from the scale list.
    pub name: ScaleName,
}

impl Default for Scale {
    /// The scale a new set and a new clip have: off, at C, in Major.
    fn default() -> Self {
        Self {
            enabled: false,
            root: 0,
            name: ScaleName::Major,
        }
    }
}

impl Scale {
    /// The scale at `root` in `name`, with Scale Mode on.
    #[must_use]
    pub const fn new(root: u8, name: ScaleName) -> Self {
        Self {
            enabled: true,
            root,
            name,
        }
    }

    /// Whether this scale is the default one, so a set file leaves it out.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The command that sets this scale on `clips` (the scales spec, R5).
    #[must_use]
    pub fn command(&self, clips: Vec<crate::ids::ClipId>) -> crate::command::Command {
        crate::command::Command::SetClipScale {
            clips,
            enabled: Some(self.enabled),
            root: Some(self.root),
            name: Some(self.name.id().to_owned()),
        }
    }

    /// The root note's name, as Beatlayer names the twelve pitch classes.
    #[must_use]
    pub const fn root_name(root: u8) -> &'static str {
        const NAMES: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];
        NAMES[(root % 12) as usize]
    }

    /// The scale's steps above the root, in semitones: the twelve of the
    /// chromatic scale while Scale Mode is off (the scales spec, R2).
    #[must_use]
    pub fn steps(&self) -> &'static [u8] {
        if self.enabled {
            self.name.steps()
        } else {
            &CHROMATIC
        }
    }

    /// The scale's notes from key 0 to key 127, in order: its degrees (the
    /// scales spec, R2).
    #[must_use]
    pub fn notes(&self) -> Vec<u8> {
        let mut notes = Vec::new();
        for key in 0..=127u8 {
            if self.holds(key) {
                notes.push(key);
            }
        }
        notes
    }

    /// Whether the scale holds `key`.
    #[must_use]
    pub fn holds(&self, key: u8) -> bool {
        let above = key % 12;
        let root = self.root % 12;
        let step = (above + 12 - root) % 12;
        self.steps().contains(&step)
    }

    /// The key `degrees` degrees from `key` (the scales spec, R2): a key in
    /// the scale lands on the scale note that many degrees above or below
    /// it, and a key outside it moves by as many semitones as the scale
    /// note below it does, so it keeps its distance above that note.
    ///
    /// `None` when the move would leave the keyboard, which is what a
    /// device or a tool says what it does about.
    #[must_use]
    pub fn move_by(&self, key: u8, degrees: i32) -> Option<u8> {
        let notes = self.extended_notes();
        let below = notes
            .iter()
            .rposition(|note| *note <= i32::from(key))
            .expect("the extended scale reaches below the keyboard");
        let target = below.checked_add_signed(isize::try_from(degrees).ok()?)?;
        let moved = notes.get(target)?;
        let key = i32::from(key) + (moved - notes[below]);
        // A move that leaves the keyboard is off it (the scales spec, R2).
        if !(0..=127).contains(&key) {
            return None;
        }
        Some(key as u8)
    }

    /// The scale note nearest `key`: the one fewest semitones away, the
    /// lower of two equally near (the scales spec, R2).
    #[must_use]
    pub fn nearest(&self, key: u8) -> u8 {
        let key = i32::from(key);
        self.extended_notes()
            .into_iter()
            .filter(|note| (0..=127).contains(note))
            .min_by_key(|note| ((note - key).abs(), *note))
            .unwrap_or(0) as u8
    }

    /// The scale's notes from below the keyboard to above it, in order, so
    /// a move that leaves the scale's range still has a note to land on and
    /// can be refused for leaving the keyboard.
    fn extended_notes(&self) -> Vec<i32> {
        let steps = self.steps();
        let root = i32::from(self.root % 12);
        (-36i32..=156)
            .filter(|key| {
                let step = ((*key).rem_euclid(12) - root).rem_euclid(12);
                steps.contains(&(step as u8))
            })
            .collect()
    }
}

impl Serialize for Scale {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut fields = serializer.serialize_struct("Scale", 3)?;
        if self.enabled {
            fields.serialize_field("enabled", &true)?;
        }
        if self.root != 0 {
            fields.serialize_field("root", &self.root)?;
        }
        if self.name != ScaleName::Major {
            fields.serialize_field("name", self.name.id())?;
        }
        fields.end()
    }
}

impl<'de> Deserialize<'de> for Scale {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The saved form, read with the name as a string, so a name the
        /// list doesn't hold can be read and replaced (the scales spec, R8).
        #[derive(Deserialize)]
        struct Saved {
            #[serde(default)]
            enabled: bool,
            #[serde(default)]
            root: u8,
            #[serde(default)]
            name: Option<String>,
        }
        let saved = Saved::deserialize(deserializer)?;
        Ok(Self {
            enabled: saved.enabled,
            root: saved.root.min(11),
            name: saved
                .name
                .as_deref()
                .and_then(ScaleName::parse)
                .unwrap_or(ScaleName::Major),
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn the_list_holds_sixteen_scales_in_order() {
        // Spec: scales R1
        assert_eq!(ScaleName::ALL.len(), 16);
        assert_eq!(ScaleName::ALL[0], ScaleName::Major);
        assert_eq!(ScaleName::ALL[15], ScaleName::HalfWholeDiminished);
        for scale in ScaleName::ALL {
            assert_eq!(
                scale.steps().first(),
                Some(&0),
                "{scale:?} starts at the root"
            );
            assert!(
                scale.steps().iter().all(|step| *step < 12),
                "{scale:?} has a step in an octave"
            );
        }
    }

    #[test]
    fn a_scale_holds_its_own_notes() {
        // Spec: scales R1, R2
        let major = Scale::new(0, ScaleName::Major);
        let notes = major.notes();
        assert_eq!(
            notes.len(),
            75,
            "ten octaves of seven notes and the five of the last"
        );
        assert_eq!(&notes[..7], &[0, 2, 4, 5, 7, 9, 11]);
        assert_eq!(&notes[7..14], &[12, 14, 16, 17, 19, 21, 23]);
        assert!(major.holds(60), "C3 is in C Major");
        assert!(!major.holds(61), "C#3 is not");
        assert!(
            Scale::new(2, ScaleName::Dorian).holds(62),
            "D3 is in D Dorian"
        );
    }

    #[test]
    fn a_scale_off_is_the_chromatic_one() {
        // Spec: scales R2
        let off = Scale::default();
        assert_eq!(off.steps(), &CHROMATIC);
        assert_eq!(off.notes().len(), 128);
        assert_eq!(off.move_by(60, 1), Some(61));
        assert_eq!(off.move_by(60, -12), Some(48));
    }

    #[test]
    fn a_key_in_the_scale_moves_by_degrees() {
        // Spec: scales R2
        let major = Scale::new(0, ScaleName::Major);
        assert_eq!(major.move_by(60, 1), Some(62), "C3 up a degree is D3");
        assert_eq!(major.move_by(60, -1), Some(59), "C3 down a degree is B2");
        assert_eq!(major.move_by(60, 7), Some(72), "a degree up an octave");
        assert_eq!(major.move_by(62, 2), Some(65), "D3 up two degrees is F3");
    }

    #[test]
    fn a_key_outside_the_scale_keeps_its_distance() {
        // Spec: scales R2
        let major = Scale::new(0, ScaleName::Major);
        // C#3 is a semitone above C3, which moves up a degree to D3, so
        // C#3 moves up two semitones to D#3.
        assert_eq!(major.move_by(61, 1), Some(63));
        // A key that isn't in the scale and has no scale note below it on
        // the keyboard keeps its distance above the one below it: C is a
        // semitone above the B below it, which moves up a degree to C#,
        // two semitones up, so C moves to D.
        let b_major = Scale::new(11, ScaleName::Major);
        assert_eq!(b_major.move_by(0, 1), Some(2));
    }

    #[test]
    fn a_move_that_leaves_the_keyboard_is_refused() {
        // Spec: scales R2
        let major = Scale::new(0, ScaleName::Major);
        assert_eq!(major.move_by(0, -1), None);
        assert_eq!(major.move_by(127, 1), None);
    }

    #[test]
    fn the_nearest_note_is_the_lower_of_two() {
        // Spec: scales R2
        let major = Scale::new(0, ScaleName::Major);
        // E3 is a semitone from D3 and two from F3.
        assert_eq!(major.nearest(64), 64);
        assert_eq!(major.nearest(63), 62, "C#3 is nearer D3 than B2");
        assert_eq!(major.nearest(61), 60, "C#3 is nearer C3 than D3");
    }

    #[test]
    fn a_scale_is_saved_only_when_it_differs() {
        // Spec: scales R8
        let off = Scale::default();
        assert_eq!(serde_json::to_value(off).unwrap(), serde_json::json!({}));
        let on = Scale::new(2, ScaleName::Dorian);
        assert_eq!(
            serde_json::to_value(on).unwrap(),
            serde_json::json!({ "enabled": true, "root": 2, "name": "dorian" })
        );
        // A name the list doesn't hold loads as Major, keeping the root and
        // the switch.
        let saved: Scale = serde_json::from_value(serde_json::json!({
            "enabled": true, "root": 5, "name": "bogus"
        }))
        .unwrap();
        assert_eq!(
            saved,
            Scale {
                enabled: true,
                root: 5,
                name: ScaleName::Major
            }
        );
    }

    #[test]
    fn a_name_is_read_in_any_of_its_forms() {
        // Spec: scales R5
        assert_eq!(ScaleName::parse("major"), Some(ScaleName::Major));
        assert_eq!(ScaleName::parse("Major"), Some(ScaleName::Major));
        assert_eq!(
            ScaleName::parse("Major Pentatonic"),
            Some(ScaleName::MajorPentatonic)
        );
        assert_eq!(
            ScaleName::parse("major_pentatonic"),
            Some(ScaleName::MajorPentatonic)
        );
        assert_eq!(ScaleName::parse("bogus"), None);
    }

    proptest! {
        /// Moving a key of the scale by some degrees and back again lands
        /// on a key in the scale (the scales spec, R2).
        #[test]
        fn moving_by_degrees_and_back_keeps_the_key_in_the_scale(
            name_index in 0usize..16,
            root in 0u8..12,
            key in 0u8..128,
            degrees in -12i32..12,
        ) {
            let scale = Scale::new(root, ScaleName::ALL[name_index]);
            // A key the scale holds: an out-of-scale key keeps its distance
            // above the scale note below it, so it comes back to itself.
            prop_assume!(scale.holds(key));
            let Some(moved) = scale.move_by(key, degrees) else {
                return Ok(());
            };
            let Some(back) = scale.move_by(moved, -degrees) else {
                return Ok(());
            };
            prop_assert!(scale.holds(back));
        }

        /// The chromatic scale moves by semitones (the scales spec, R2).
        #[test]
        fn the_chromatic_scale_moves_by_semitones(
            key in 0u8..128,
            degrees in -12i32..12,
        ) {
            let scale = Scale::default();
            if let Some(moved) = scale.move_by(key, degrees) {
                prop_assert_eq!(i32::from(moved), i32::from(key) + degrees);
            }
        }

        /// The nearest note of a scale is in the scale, and a tie goes to
        /// the lower note (the scales spec, R2).
        #[test]
        fn the_nearest_note_is_in_the_scale(
            name_index in 0usize..16,
            root in 0u8..12,
            key in 0u8..128,
        ) {
            let scale = Scale::new(root, ScaleName::ALL[name_index]);
            let nearest = scale.nearest(key);
            prop_assert!(scale.holds(nearest));
            let lower = scale.notes().into_iter().rev().find(|n| *n <= key);
            let upper = scale.notes().into_iter().find(|n| *n >= key);
            if let (Some(lower), Some(upper)) = (lower, upper) {
                let below = key - lower;
                let above = upper - key;
                if below == above {
                    prop_assert_eq!(nearest, lower);
                }
            }
        }
    }
}
