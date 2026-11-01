//! Where notes are on the pads of a melodic layout, and what the touch strip
//! shows of the range (`docs/specs/push-2-notes.md`, R32 to R34 and R39 to
//! R43).

use bl_model::{PadDirection, PadLayout, Scale};

use super::grid::{Melodic, Role};

/// C1, the note the bottom-left pad plays with Fixed on (R33).
const C1: i32 = 36;

/// The note the bottom-left pad plays with the pads `octave` octaves up from
/// where they start (R33): the scale's root in octave 1, or with Fixed on C1,
/// or the scale note nearest it, the higher on a tie.
#[must_use]
pub fn base(scale: Scale, pads: PadLayout, octave: i32) -> i32 {
    let c = C1 + 12 * octave;
    if !pads.fixed {
        return c + i32::from(scale.root);
    }
    if !pads.in_key || !(0..128).contains(&c) {
        return c;
    }
    (0..12)
        .flat_map(|d| [c + d, c - d])
        .find(|&key| u8::try_from(key).is_ok_and(|key| key < 128 && scale.holds(key)))
        .unwrap_or(c)
}

/// Whether the pads can move an octave up or down: the bottom-left pad stays
/// within notes 0 to 127 (R33).
#[must_use]
pub fn can_octave(scale: Scale, pads: PadLayout, octave: i32, up: bool) -> bool {
    (0..128).contains(&base(scale, pads, octave + if up { 1 } else { -1 }))
}

/// The note of the pad in row `row` from the bottom and column `column` in
/// 64 Notes, if it has one (R32, R33).
#[must_use]
pub fn pad_note(scale: Scale, pads: PadLayout, octave: i32, row: u8, column: u8) -> Option<u8> {
    let interval = pads.spacing.row(pads.in_key);
    let (along, up) = match pads.direction {
        PadDirection::Vertical => (1, interval),
        PadDirection::Horizontal => (interval, 1),
    };
    let steps = i32::from(column) * along + i32::from(row) * up;
    let from = u8::try_from(base(scale, pads, octave))
        .ok()
        .filter(|key| *key < 128)?;
    away(scale, pads.in_key, from, steps)
}

/// The note `by` degrees In Key, or semitones Chromatic, from `from`.
#[must_use]
pub fn away(scale: Scale, in_key: bool, from: u8, by: i32) -> Option<u8> {
    if in_key {
        scale.move_by(from, by)
    } else {
        u8::try_from(i32::from(from) + by)
            .ok()
            .filter(|key| *key < 128)
    }
}

/// Where the Melodic Sequencer's lowest row and the 32 notes start for a new
/// track: the scale's root in octave 1 (R40, R43).
#[must_use]
pub fn default_low(scale: Scale) -> u8 {
    (C1 + i32::from(scale.root)) as u8
}

/// The lowest row or note after Octave Up or Down moves it an octave, or one
/// scale note with Shift held (R41, R43); none where it can't move.
#[must_use]
pub fn shifted_low(scale: Scale, in_key: bool, low: u8, up: bool, one_note: bool) -> Option<u8> {
    let by = if up { 1 } else { -1 };
    let by = if one_note { by } else { by * 12 };
    if one_note {
        away(scale, in_key, low, by)
    } else {
        u8::try_from(i32::from(low) + by)
            .ok()
            .filter(|key| *key < 128)
    }
}

/// What the layout's pads are, in `layout`, with the loop length pads showing
/// or not (R39 to R44).
#[derive(Clone, Copy)]
pub struct View {
    /// The layout.
    pub layout: Melodic,
    /// The set's current scale.
    pub scale: Scale,
    /// The set's pad layout options.
    pub pads: PadLayout,
    /// 64 Notes' shift in octaves.
    pub octave: i32,
    /// The Melodic Sequencer's lowest row, and where its 32 notes start.
    pub low: u8,
    /// Whether the loop length pads show.
    pub loops: bool,
    /// How many step pads a row of a step area uses.
    pub per_row: u8,
}

impl View {
    /// What the pad in row `row` from the bottom and column `column` from the
    /// left is.
    #[must_use]
    pub fn role(&self, row: u8, column: u8) -> Role {
        if self.loops && row == self.layout.loop_row() && column < super::grid::MELODIC_PAGES {
            return Role::Loop(column);
        }
        let row_key = |index: u8| away(self.scale, self.pads.in_key, self.low, i32::from(index));
        match self.layout {
            Melodic::Notes => pad_note(self.scale, self.pads, self.octave, row, column)
                .map_or(Role::Off, Role::Key),
            Melodic::Sequencer => {
                if column >= self.per_row {
                    return Role::Off;
                }
                row_key(row).map_or(Role::Off, |key| Role::Cell { key, step: column })
            }
            Melodic::SequencerNotes => {
                if row < 4 {
                    return row_key(row * 8 + column).map_or(Role::Off, Role::Pick);
                }
                if column < self.per_row {
                    Role::Step((row - 4) * self.per_row + column)
                } else {
                    Role::Off
                }
            }
        }
    }

    /// The lowest and highest note the pads, rows or 32 notes play (R41).
    #[must_use]
    pub fn range(&self) -> Option<(u8, u8)> {
        let keys = (0..8).flat_map(|row| (0..8).map(move |column| (row, column)));
        let keys = keys.filter_map(|(row, column)| match self.role(row, column) {
            Role::Key(key) | Role::Pick(key) | Role::Cell { key, .. } => Some(key),
            _ => None,
        });
        keys.fold(None, |range, key| match range {
            None => Some((key, key)),
            Some((low, high)) => Some((low.min(key), high.max(key))),
        })
    }
}

/// The name of note `key` as Beatlayer names them, C1 being 36.
#[must_use]
pub fn note_name(key: u8) -> String {
    format!("{}{}", Scale::root_name(key % 12), i32::from(key / 12) - 2)
}

/// The touch strip's 31 LEDs for a range of notes from `low` to `high`: the
/// LEDs that the range covers are bright, and those that cover notes the clip
/// has outside it dim (R41).
#[must_use]
pub fn strip_leds(low: u8, high: u8, clip_keys: &[u8]) -> [u8; 31] {
    let mut levels = [0; 31];
    for (i, level) in levels.iter_mut().enumerate() {
        let (from, to) = (i as u32 * 128 / 31, (i as u32 + 1) * 128 / 31);
        let covers = |key: u8| (from..to).contains(&u32::from(key));
        if from <= u32::from(high) && to > u32::from(low) {
            *level = 7;
        } else if clip_keys.iter().any(|&key| covers(key)) {
            *level = 2;
        }
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_model::{PadSpacing, ScaleName};

    // Spec: push-2-notes R32, R33. Every pad of every layout against the
    // formulas the spec gives, in C major, D♭ major and A minor, with Fixed on
    // and off.
    #[test]
    fn every_pad_follows_the_formulas_for_every_layout() {
        let scales = [
            Scale::new(0, ScaleName::Major),
            Scale::new(1, ScaleName::Major),
            Scale::new(9, ScaleName::Minor),
        ];
        for scale in scales {
            let degrees = scale.notes();
            let degree = |key: i32| degrees.iter().position(|&n| i32::from(n) == key).unwrap();
            for in_key in [true, false] {
                for spacing in PadSpacing::ALL {
                    for direction in [PadDirection::Vertical, PadDirection::Horizontal] {
                        for fixed in [false, true] {
                            let pads = PadLayout {
                                in_key,
                                fixed,
                                spacing,
                                direction,
                            };
                            // R33: the bottom-left pad.
                            let first = if fixed {
                                // All three keys hold C.
                                C1
                            } else {
                                C1 + i32::from(scale.root)
                            };
                            assert_eq!(base(scale, pads, 0), first);
                            let (right, up) = match (in_key, spacing) {
                                (true, PadSpacing::Fourths) => (1, 3),
                                (true, PadSpacing::Thirds) => (1, 2),
                                (false, PadSpacing::Fourths) => (1, 5),
                                (false, PadSpacing::Thirds) => (1, 4),
                                (_, PadSpacing::Sequent) => (1, 8),
                            };
                            let (right, up) = if direction == PadDirection::Vertical {
                                (right, up)
                            } else {
                                (up, right)
                            };
                            for row in 0..8u8 {
                                for column in 0..8u8 {
                                    let by = i32::from(row) * up + i32::from(column) * right;
                                    let want = if in_key {
                                        degrees.get(degree(first) + by as usize).copied()
                                    } else {
                                        u8::try_from(first + by).ok().filter(|k| *k < 128)
                                    };
                                    assert_eq!(
                                        pad_note(scale, pads, 0, row, column),
                                        want,
                                        "{scale:?} {pads:?} row {row} column {column}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Spec: push-2-notes R33. With Fixed on, a key without C puts the scale
    // note nearest C1 at the bottom left, the higher on a tie; Octave stops
    // at notes 0 and 127.
    #[test]
    fn fixed_finds_the_nearest_scale_note_and_octave_stops_at_the_ends() {
        let pads = PadLayout {
            fixed: true,
            ..PadLayout::default()
        };
        // D♭ major holds C, so Fixed puts C1 itself at the bottom left.
        let c_sharp = Scale::new(1, ScaleName::Major);
        assert_eq!(base(c_sharp, pads, 0), 36);
        // D major holds no C: C♯ (37) and B (35) are one away, the higher wins.
        let d = Scale::new(2, ScaleName::Major);
        assert!(!d.holds(36) && d.holds(35) && d.holds(37));
        assert_eq!(base(d, pads, 0), 37);
        let c = Scale::new(0, ScaleName::Major);
        let open = PadLayout::default();
        let top = (0..).find(|&n| !can_octave(c, open, n, true)).unwrap();
        assert_eq!(base(c, open, top), 36 + 12 * top);
        assert!(base(c, open, top + 1) > 127);
        let bottom = (0..).find(|&n| !can_octave(c, open, -n, false)).unwrap();
        assert!(base(c, open, -bottom - 1) < 0);
        assert!(base(c, open, -bottom) >= 0);
    }

    // Spec: push-2-notes R40, R41. The rows run one scale note apart In Key
    // and a semitone apart Chromatic, and the strip's LEDs show the range
    // brightly and the clip's notes outside it dimly.
    #[test]
    fn the_sequencers_rows_and_the_strips_leds() {
        let scale = Scale::new(0, ScaleName::Major);
        let view = |in_key| View {
            layout: Melodic::Sequencer,
            scale,
            pads: PadLayout {
                in_key,
                ..PadLayout::default()
            },
            octave: 0,
            low: 36,
            loops: false,
            per_row: 8,
        };
        let rows = |view: View| -> Vec<u8> {
            (0..8)
                .filter_map(|row| match view.role(row, 0) {
                    Role::Cell { key, .. } => Some(key),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(rows(view(true)), [36, 38, 40, 41, 43, 45, 47, 48]);
        assert_eq!(rows(view(false)), [36, 37, 38, 39, 40, 41, 42, 43]);
        let shown = View {
            loops: true,
            ..view(true)
        };
        assert_eq!(rows(shown).len(), 7);
        assert_eq!(shown.role(7, 3), Role::Loop(3));
        let leds = strip_leds(36, 48, &[10, 100]);
        assert!(leds.iter().any(|&l| l == 7));
        assert_eq!(leds[10 * 31 / 128], 2);
        assert_eq!(leds[100 * 31 / 128], 2);
        assert_eq!(leds[0], 0);
    }
}
