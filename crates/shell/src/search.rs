//! The search field's matching and ranking (WP-6.42, IMP-16 and IMP-18):
//! which commands a few typed letters find and in what order (command
//! palette R6), and which tracks they find (R9). The rows themselves are
//! built from the menus and the keymap by [`Shell::search_commands`]; the
//! field's state is the shell's.
//!
//! [`Shell::search_commands`]: crate::Shell::search_commands

use std::collections::BTreeSet;

use bl_model::{Document, TrackId};
use bl_shell_spec::search::{CommandRow, SearchScope, TrackRow};

/// A command a menu item, a context-menu item or a shortcut runs, before a
/// text picks among them (R4 and R5).
#[derive(Clone, Debug)]
pub struct Candidate {
    /// The row the list shows.
    pub row: CommandRow,
    /// The command's description, which a word may also match (R6).
    pub description: String,
}

/// What the search field holds between frames: view state, never saved
/// with the set and never an undo step (R3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Field {
    /// Whether the field is open.
    pub open: bool,
    /// Its scope.
    pub scope: SearchScope,
    /// What was typed.
    pub text: String,
    /// The highlighted row of the Commands scope.
    pub highlight: usize,
    /// Show Only Matches (R12).
    pub only_matches: bool,
    /// The rows the text finds in the Commands scope, in list order.
    pub commands: Vec<CommandRow>,
    /// The tracks the text matches in the Tracks scope, in display order.
    pub tracks: Vec<TrackRow>,
}

impl Field {
    /// Whether the Tracks scope has text that matches a track: while it
    /// does, the matches are outlined (R10).
    #[must_use]
    pub fn outlines(&self) -> bool {
        self.open && self.scope == SearchScope::Tracks && !self.tracks.is_empty()
    }
}

/// The words of a text that a typed word may start: runs of letters and
/// digits, in lower case.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether every typed word starts a word of `of`.
fn starts_all(typed: &[String], of: &[String]) -> bool {
    typed
        .iter()
        .all(|word| of.iter().any(|w| w.starts_with(word.as_str())))
}

/// The rows `text` finds among `candidates`, in the order the list shows
/// them (R6 and R7): those that can run before those that can't; within
/// each, those whose label matches every word, those whose label starts with
/// the first word first; then those matched through their menu; then those
/// matched through the description; each group in the label's alphabetical
/// order. Nothing typed finds nothing.
#[must_use]
pub fn rank_commands(candidates: Vec<Candidate>, text: &str) -> Vec<CommandRow> {
    let typed = words(text);
    if typed.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<((bool, u8, String, String), CommandRow)> = Vec::new();
    for Candidate { row, description } in candidates {
        let label_words = words(&row.label);
        let in_label = starts_all(&typed, &label_words);
        let tier = if in_label {
            u8::from(!row.label.to_lowercase().starts_with(&typed[0]))
        } else {
            let both: Vec<String> = label_words.into_iter().chain(words(&row.menu)).collect();
            if starts_all(&typed, &both) {
                2
            } else {
                let description = description.to_lowercase();
                let reached = typed.iter().all(|word| {
                    both.iter().any(|w| w.starts_with(word.as_str()))
                        || description.contains(word.as_str())
                });
                if !reached {
                    continue;
                }
                3
            }
        };
        let key = (
            !row.available,
            tier,
            row.label.to_lowercase(),
            row.menu.to_lowercase(),
        );
        found.push((key, row));
    }
    found.sort_by(|(a, row_a), (b, row_b)| {
        a.cmp(b)
            .then_with(|| row_a.command.cmp(&row_b.command))
            .then_with(|| row_a.args.len().cmp(&row_b.args.len()))
    });
    found.into_iter().map(|(_, row)| row).collect()
}

/// The tracks `text` finds, in display order (R9): those with the text,
/// ignoring case, anywhere in their name or in the name or own name of a
/// device on them at any depth, plug-ins included. Nothing typed finds
/// nothing.
#[must_use]
pub fn find_tracks(doc: &Document, text: &str) -> Vec<TrackRow> {
    if text.is_empty() {
        return Vec::new();
    }
    let typed = text.to_lowercase();
    let has = |name: &str| name.to_lowercase().contains(&typed);
    doc.tracks()
        .filter_map(|track| {
            let by_name = has(track.name());
            let device = track.devices_deep().find_map(|device| {
                [device.name.as_str(), device.own_name()]
                    .into_iter()
                    .find(|name| has(name))
            });
            (by_name || device.is_some()).then(|| TrackRow {
                track: track.id(),
                name: track.name().to_owned(),
                device: device.map(str::to_owned),
                by_name,
            })
        })
        .collect()
}

/// The tracks Show Only Matches leaves showing (R12): the matches and the
/// groups that hold them. Main always shows, so it isn't in the set.
#[must_use]
pub fn keep_set(doc: &Document, matches: &[TrackRow]) -> BTreeSet<TrackId> {
    let mut keep = BTreeSet::new();
    for row in matches {
        let mut at = Some(row.track);
        while let Some(track) = at {
            if !keep.insert(track) {
                break;
            }
            at = doc.parent(track);
        }
    }
    keep
}

/// The next (or with `backwards` the previous) of `matches` after `from` in
/// display order, wrapping from the last to the first; the first (the last,
/// backwards) when `from` is `None` or is the only match (R11).
#[must_use]
pub fn step(
    doc: &Document,
    matches: &[TrackRow],
    from: Option<TrackId>,
    backwards: bool,
) -> Option<TrackId> {
    let order = doc.track_order();
    let place = |track: TrackId| order.iter().position(|t| *t == track).unwrap_or(usize::MAX);
    let from = from.map(place);
    let mut ahead = matches.iter().map(|row| row.track);
    if backwards {
        let before = from.and_then(|from| {
            matches
                .iter()
                .map(|row| row.track)
                .rev()
                .find(|t| place(*t) < from)
        });
        before.or(ahead.next_back())
    } else {
        let after = from.and_then(|from| ahead.clone().find(|t| place(*t) > from));
        after.or(ahead.next())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    fn row(label: &str, menu: &str, available: bool, description: &str) -> Candidate {
        Candidate {
            row: CommandRow {
                command: label.to_lowercase().replace(' ', "."),
                args: Map::new(),
                label: label.to_owned(),
                menu: menu.to_owned(),
                keys: Vec::new(),
                available,
            },
            description: description.to_owned(),
        }
    }

    fn labels(rows: &[CommandRow]) -> Vec<&str> {
        rows.iter().map(|row| row.label.as_str()).collect()
    }

    // Spec: command-palette R6.
    #[test]
    fn labels_come_before_menus_and_menus_before_descriptions() {
        let candidates = vec![
            row("Show Duplicate Notes", "View", true, ""),
            row("Zoom", "Duplicate Menu", true, ""),
            row(
                "Paste",
                "Edit",
                true,
                "Pastes, or duplicates a pasted clip.",
            ),
            row("Duplicate Track Without Clips", "Edit", true, ""),
            row("Duplicate", "Edit", true, ""),
        ];
        let rows = rank_commands(candidates, "Dup");
        assert_eq!(
            labels(&rows),
            [
                "Duplicate",
                "Duplicate Track Without Clips",
                "Show Duplicate Notes",
                "Zoom",
                "Paste"
            ]
        );
    }

    // Spec: command-palette R6.
    #[test]
    fn every_word_must_match_and_nothing_typed_lists_nothing() {
        let candidates = vec![
            row("Insert MIDI Track", "Create", true, ""),
            row("Insert Audio Track", "Create", true, "Adds an audio track."),
        ];
        assert_eq!(
            labels(&rank_commands(candidates.clone(), "ins mid")),
            ["Insert MIDI Track"]
        );
        assert!(rank_commands(candidates.clone(), "ins xyz").is_empty());
        assert!(rank_commands(candidates, "  ").is_empty());
    }

    // Spec: command-palette R7.
    #[test]
    fn rows_that_cant_run_come_last() {
        let candidates = vec![
            row("Undo", "Edit", false, ""),
            row("Undo History", "View", true, ""),
        ];
        assert_eq!(
            labels(&rank_commands(candidates, "undo")),
            ["Undo History", "Undo"]
        );
    }
}
