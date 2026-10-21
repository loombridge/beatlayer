//! Locators: the markers in the Arrangement's scrub area (WP-6.24).
//!
//! A locator marks a place in the song to play from. It has a stable ID, a
//! position in ticks from 1.1.1, a name, and info text, and no two share a
//! position. The song start time is a locator or the loop brace's start.

use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::LocatorId;

/// A locator: a place in the song to play from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Locator {
    /// The locator's ID.
    pub id: LocatorId,
    /// Where it sits: 0 (1.1.1) or later.
    pub position: Ticks,
    /// Its name, which may be empty.
    pub name: String,
    /// Its info text, which the Info View shows while the pointer is over
    /// it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub info_text: String,
}

impl Locator {
    /// A locator at `position`, named `name`.
    #[must_use]
    pub fn new(id: LocatorId, position: Ticks, name: String) -> Self {
        Self {
            id,
            position,
            name,
            info_text: String::new(),
        }
    }
}

/// Where the song starts: a locator, or the loop brace's start. It follows
/// the locator or the brace as they move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SongStart {
    /// A locator: the song starts there.
    Locator(LocatorId),
    /// The loop brace's start: the song starts there.
    LoopBrace,
}

/// The name a new locator gets: the next whole number, one more than the
/// highest any locator is named with, and "1" in a set with none.
#[must_use]
pub fn next_locator_name(locators: &[Locator]) -> String {
    let highest = locators
        .iter()
        .filter_map(|locator| locator.name.parse::<u64>().ok())
        .max();
    format!("{}", highest.map_or(1, |number| number + 1))
}

#[cfg(test)]
mod tests {
    use bl_core::time::Ticks;

    use crate::command::Command;
    use crate::document::Document;
    use crate::ids::LocatorId;
    use crate::locator::{Locator, SongStart};

    use super::next_locator_name;

    /// A document with its Main track and the default scenes.
    fn song() -> Document {
        let mut ids = bl_core::id::IdAllocator::new();
        let main = crate::ids::TrackId(ids.allocate());
        let scenes: Vec<_> = (0..8)
            .map(|_| crate::ids::SceneId(ids.allocate()))
            .collect();
        Document::new(main, scenes)
    }

    #[test]
    fn a_locator_is_part_of_the_set_and_saved_when_there_is_one() {
        let mut doc = song();
        let saved = serde_json::to_value(&doc).unwrap();
        assert!(
            saved["settings"].get("locators").is_none(),
            "a set with no locators doesn't write them"
        );
        assert!(saved["settings"].get("song_start").is_none());
        let id = LocatorId(bl_core::id::Id::new(100).unwrap());
        doc.apply(&Command::AddLocator {
            locator: Locator::new(id, Ticks::WHOLE, "1".to_owned()),
        })
        .unwrap();
        doc.apply(&Command::SetSongStart {
            start: Some(SongStart::Locator(id)),
        })
        .unwrap();
        let saved = serde_json::to_value(&doc).unwrap();
        assert_eq!(saved["settings"]["locators"][0]["position"], 2_822_400);
        assert_eq!(saved["settings"]["song_start"]["locator"], 100);
        let loaded: Document = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded, doc);
        assert_eq!(loaded.song_start_ticks(), Some(Ticks::WHOLE));
    }

    #[test]
    fn a_set_saved_before_locators_loads_with_none() {
        let doc = song();
        let saved = serde_json::to_value(&doc).unwrap();
        let loaded: Document = serde_json::from_value(saved).unwrap();
        assert!(loaded.locators().is_empty());
        assert!(loaded.song_start_ticks().is_none());
    }

    #[test]
    fn two_locators_at_one_position_dont_load() {
        let mut doc = song();
        let id = LocatorId(bl_core::id::Id::new(100).unwrap());
        doc.apply(&Command::AddLocator {
            locator: Locator::new(id, Ticks::WHOLE, "1".to_owned()),
        })
        .unwrap();
        let mut saved = serde_json::to_value(&doc).unwrap();
        saved["settings"]["locators"].as_array_mut().unwrap().push(
            serde_json::json!({ "id": 101, "position": 2_822_400, "name": "2" }),
        );
        assert!(serde_json::from_value::<Document>(saved).is_err());
    }

    #[test]
    fn a_locator_before_the_start_doesnt_load() {
        let mut doc = song();
        let id = LocatorId(bl_core::id::Id::new(100).unwrap());
        assert!(doc
            .apply(&Command::AddLocator {
                locator: Locator::new(id, Ticks(-1), "1".to_owned()),
            })
            .is_err());
        // The command is refused, so the set never holds one.
        assert!(doc.locators().is_empty());
    }

    #[test]
    fn a_new_locator_is_named_with_the_next_whole_number() {
        let mut doc = song();
        let id = LocatorId(bl_core::id::Id::new(100).unwrap());
        doc.apply(&Command::AddLocator {
            locator: Locator::new(id, Ticks::WHOLE, String::new()),
        })
        .unwrap();
        assert_eq!(doc.locators()[0].name, "1");
        let second = LocatorId(bl_core::id::Id::new(101).unwrap());
        doc.apply(&Command::AddLocator {
            locator: Locator::new(second, Ticks::WHOLE * 2, String::new()),
        })
        .unwrap();
        assert_eq!(doc.locators()[1].name, "2");
        assert_eq!(next_locator_name(&[]), "1");
    }
}
