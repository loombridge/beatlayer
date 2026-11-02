//! Sections of the song, the arranger track's objects (WP-6.45).
//!
//! A section is a named, coloured span of time. It has a stable ID, a start
//! in ticks from 1.1.1, a length longer than zero, a name and a swatch of
//! the palette, and no two sections overlap. Spec: arranger-track R3.

use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::colour::{TRACK_COLOURS, check_swatch};
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::SectionId;

/// A section of the song.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SongSection {
    /// The section's ID.
    pub id: SectionId,
    /// Where it starts: 0 (1.1.1) or later.
    pub start: Ticks,
    /// How long it is: more than 0.
    pub length: Ticks,
    /// Its name, which may be empty.
    pub name: String,
    /// Its colour, a swatch from 0 to 69 (the themes spec, R14).
    pub colour: u8,
}

impl SongSection {
    /// Where it ends.
    #[must_use]
    pub fn end(&self) -> Ticks {
        self.start + self.length
    }
}

/// The name a new section gets: "Section" and the next whole number, one
/// more than the highest any section is named with (R5).
#[must_use]
pub fn next_section_name(sections: &[SongSection]) -> String {
    let highest = sections
        .iter()
        .filter_map(|section| section.name.strip_prefix("Section ")?.parse::<u64>().ok())
        .max();
    format!("Section {}", highest.map_or(1, |number| number + 1))
}

/// The colour a new section takes: the swatch after the newest section's,
/// in the palette's top row (R5).
#[must_use]
pub fn next_section_colour(sections: &[SongSection]) -> u8 {
    sections
        .iter()
        .max_by_key(|section| section.id)
        .map_or(0, |section| (section.colour + 1) % TRACK_COLOURS)
}

/// Checks the sections a saved set holds (R3).
pub(crate) fn check_sections(sections: &[SongSection]) -> Result<(), ModelError> {
    let mut ends = Ticks::ZERO;
    let mut sorted: Vec<&SongSection> = sections.iter().collect();
    sorted.sort_by_key(|section| section.start);
    for section in sorted {
        check_span(section.start, section.length)?;
        check_swatch("section colour", section.colour)?;
        if section.start < ends {
            return Err(ModelError::ChangeInUse {
                what: "section",
                at: section.start,
            });
        }
        ends = section.end();
    }
    Ok(())
}

fn check_span(start: Ticks, length: Ticks) -> Result<(), ModelError> {
    if start < Ticks::ZERO {
        return Err(ModelError::NegativePosition(start));
    }
    if length <= Ticks::ZERO {
        return Err(ModelError::InvalidInput("a section must be longer than 0"));
    }
    start
        .checked_add(length)
        .ok_or(ModelError::PositionOverflow)?;
    Ok(())
}

impl Document {
    /// The song's sections, in time order.
    #[must_use]
    pub fn sections(&self) -> &[SongSection] {
        &self.settings.sections
    }

    /// The section with this ID.
    #[must_use]
    pub fn section(&self, id: SectionId) -> Option<&SongSection> {
        self.settings.sections.iter().find(|s| s.id == id)
    }

    /// The start of a section other than `except` that overlaps
    /// `start..start + length`.
    fn section_overlap(
        &self,
        start: Ticks,
        length: Ticks,
        except: Option<SectionId>,
    ) -> Option<Ticks> {
        self.settings
            .sections
            .iter()
            .filter(|other| Some(other.id) != except)
            .find(|other| other.start < start + length && other.end() > start)
            .map(|other| other.start)
    }

    /// Adds a section, naming it where it has no name (R5).
    pub(crate) fn add_section(&mut self, mut section: SongSection) -> Result<(), ModelError> {
        check_span(section.start, section.length)?;
        check_swatch("section colour", section.colour)?;
        self.check_fresh(section.id.0)?;
        if let Some(at) = self.section_overlap(section.start, section.length, None) {
            return Err(ModelError::ChangeInUse {
                what: "section",
                at,
            });
        }
        if section.name.is_empty() {
            section.name = next_section_name(&self.settings.sections);
        }
        self.settings.sections.push(section);
        self.settings.sections.sort_by_key(|s| s.start);
        Ok(())
    }

    /// Removes a section and nothing else (R11).
    pub(crate) fn remove_section(&mut self, id: SectionId) -> Result<(), ModelError> {
        let at = self
            .settings
            .sections
            .iter()
            .position(|s| s.id == id)
            .ok_or(ModelError::NoSuchSection(id))?;
        self.settings.sections.remove(at);
        Ok(())
    }

    /// Changes a section's name, colour or span (R7).
    pub(crate) fn edit_section(
        &mut self,
        id: SectionId,
        name: Option<&str>,
        colour: Option<u8>,
        start: Option<Ticks>,
        length: Option<Ticks>,
    ) -> Result<(), ModelError> {
        let now = self.section(id).ok_or(ModelError::NoSuchSection(id))?;
        let (start, length) = (start.unwrap_or(now.start), length.unwrap_or(now.length));
        check_span(start, length)?;
        if let Some(colour) = colour {
            check_swatch("section colour", colour)?;
        }
        if let Some(at) = self.section_overlap(start, length, Some(id)) {
            return Err(ModelError::ChangeInUse {
                what: "section",
                at,
            });
        }
        let section = self
            .settings
            .sections
            .iter_mut()
            .find(|s| s.id == id)
            .expect("just found the section");
        section.start = start;
        section.length = length;
        if let Some(name) = name {
            name.clone_into(&mut section.name);
        }
        if let Some(colour) = colour {
            section.colour = colour;
        }
        self.settings.sections.sort_by_key(|s| s.start);
        Ok(())
    }

    /// Opens `length` at `at` in the sections: those at or after `at` move
    /// later and one across it is lengthened (R13).
    pub(crate) fn insert_section_time(&mut self, at: Ticks, length: Ticks) {
        for section in &mut self.settings.sections {
            if section.start >= at {
                section.start += length;
            } else if section.end() > at {
                section.length += length;
            }
        }
    }

    /// Closes `start..end` in the sections: the part inside goes, a section
    /// wholly inside is deleted, and later ones move earlier (R13).
    pub(crate) fn delete_section_time(&mut self, start: Ticks, end: Ticks) {
        let length = end - start;
        self.settings.sections.retain_mut(|section| {
            let (from, to) = (section.start, section.end());
            if to <= start {
                return true;
            }
            if from >= end {
                section.start -= length;
                return true;
            }
            let remaining = section.length - (to.min(end) - from.max(start));
            section.start = from.min(start);
            section.length = remaining;
            remaining > Ticks::ZERO
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;

    fn song() -> Document {
        let mut ids = bl_core::id::IdAllocator::new();
        let main = crate::ids::TrackId(ids.allocate());
        let scenes: Vec<_> = (0..8)
            .map(|_| crate::ids::SceneId(ids.allocate()))
            .collect();
        Document::new(main, scenes)
    }

    fn section(id: u64, start: i64, length: i64) -> SongSection {
        SongSection {
            id: SectionId(bl_core::id::Id::new(id).unwrap()),
            start: Ticks(start),
            length: Ticks(length),
            name: String::new(),
            colour: 0,
        }
    }

    // Spec: arranger-track R3
    #[test]
    fn sections_are_saved_when_there_are_some_and_overlaps_dont_load() {
        let mut doc = song();
        let saved = serde_json::to_value(&doc).unwrap();
        assert!(saved["settings"].get("sections").is_none());
        doc.apply(&Command::AddSection {
            section: section(100, 0, 10),
        })
        .unwrap();
        doc.apply(&Command::AddSection {
            section: section(101, 10, 10),
        })
        .unwrap();
        assert!(
            doc.apply(&Command::AddSection {
                section: section(102, 5, 10)
            })
            .is_err(),
            "an overlapping section is refused"
        );
        assert_eq!(doc.sections()[1].name, "Section 2");
        let mut saved = serde_json::to_value(&doc).unwrap();
        let loaded: Document = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(loaded, doc);
        saved["settings"]["sections"][1]["start"] = 5.into();
        assert!(serde_json::from_value::<Document>(saved).is_err());
    }

    // Spec: arranger-track R13
    #[test]
    fn time_edits_lengthen_cut_and_move_sections() {
        let mut doc = song();
        doc.settings.sections = vec![section(100, 10, 10), section(101, 30, 10)];
        doc.insert_section_time(Ticks(15), Ticks(5));
        let spans = |doc: &Document| -> Vec<(i64, i64)> {
            doc.sections()
                .iter()
                .map(|s| (s.start.get(), s.length.get()))
                .collect()
        };
        assert_eq!(spans(&doc), [(10, 15), (35, 10)]);
        doc.delete_section_time(Ticks(12), Ticks(40));
        assert_eq!(spans(&doc), [(10, 2), (12, 5)]);
        doc.delete_section_time(Ticks(5), Ticks(30));
        assert!(doc.sections().is_empty());
    }
}
