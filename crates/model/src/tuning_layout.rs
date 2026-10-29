//! A MIDI track's MIDI Controller Layout (WP-6.9b; the tunings spec, R22
//! and R23): which key of the set's tuning each key of an incoming keyboard
//! plays.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ModelError;

/// How many incoming keys a custom layout maps: the MIDI keys.
pub const LAYOUT_KEYS: usize = 128;

/// Which mapping a layout uses (the tunings spec, R22).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    /// Each key plays the same key of the tuning.
    #[default]
    AllKeys,
    /// The black keys play the tuning's keys from key 60 up, C#3 first.
    BlackKeysOnly,
    /// The white keys play the tuning's keys from key 60 up, C3 first.
    WhiteKeysOnly,
    /// Each key plays the tuning's key nearest its twelve-step pitch.
    ClosestInPitch,
    /// Each key plays the key the custom table gives it, or none.
    Custom,
}

/// A track's MIDI Controller Layout and its custom table (R22, R23).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    description = "A MIDI track's MIDI Controller Layout: `layout` (`all_keys`, \
                   `black_keys_only`, `white_keys_only`, `closest_in_pitch` or `custom`) and \
                   `keys`, the custom table: for each of the 128 incoming keys the key of the \
                   tuning it plays, or null for none (left out while each key plays itself). \
                   The table is kept while another layout is chosen."
)]
pub struct TuningLayout {
    /// The mapping in use.
    #[serde(default)]
    pub layout: LayoutKind,
    /// The custom table, 128 entries, or empty while each key plays itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<Option<u8>>,
}

impl TuningLayout {
    /// Whether it is a new track's: all keys, with the new custom table.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether a table fits: 128 entries, none past key 127.
    ///
    /// # Errors
    /// [`ModelError::InvalidInput`] if it doesn't.
    pub fn check_keys(keys: &[Option<u8>]) -> Result<(), ModelError> {
        if keys.len() != LAYOUT_KEYS || keys.iter().flatten().any(|&key| key > 127) {
            return Err(ModelError::InvalidInput(
                "a custom layout has 128 entries, each a key from 0 to 127 or null",
            ));
        }
        Ok(())
    }

    /// The key of the tuning the custom table gives incoming key `key`.
    #[must_use]
    pub fn custom(&self, key: u8) -> Option<u8> {
        self.keys.get(usize::from(key)).map_or(Some(key), |&to| to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: tunings R23
    #[test]
    fn a_new_layout_is_all_keys_and_a_table_with_no_entries_maps_each_key_to_itself() {
        let layout = TuningLayout::default();
        assert!(layout.is_default());
        assert_eq!(layout.custom(61), Some(61));
        let mut keys = vec![None; LAYOUT_KEYS];
        keys[61] = Some(70);
        let custom = TuningLayout {
            layout: LayoutKind::Custom,
            keys,
        };
        assert_eq!(custom.custom(61), Some(70));
        assert_eq!(custom.custom(62), None);
    }

    // Spec: tunings R23
    #[test]
    fn a_table_must_hold_128_keys_from_0_to_127() {
        assert!(TuningLayout::check_keys(&vec![None; 128]).is_ok());
        assert!(TuningLayout::check_keys(&vec![None; 127]).is_err());
        let mut keys = vec![None; 128];
        keys[0] = Some(128);
        assert!(TuningLayout::check_keys(&keys).is_err());
    }
}
