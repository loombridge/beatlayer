//! The Arrangement's context menus, as data (WP-1.8b): the menu a clip
//! opens and the menu a time selection opens, in Live's order from the
//! menu reference (C-CLIP-ARR, C-TIME), each item tied to its
//! shortcut as the menu bar's are ([`Action::Shortcut`]), so it runs the
//! command the keymap gives that shortcut in the Arrangement, or shows
//! disabled until a package binds it.
//!
//! The grid's Adaptive Grid and Fixed Grid rows are rows of choices rather
//! than single items: they carry `arrangement.set_grid`, and the UI lays
//! out a choice for each setting.

use crate::arrangement::SET_GRID;
use crate::menus::{Action, MenuItem, Row, SEPARATOR, items};

use Action::{Command as Cmd, Pending, Shortcut as Key};

/// The rows that hold the zoom-adaptive grid's settings.
pub const ADAPTIVE_GRID_ROWS: [&str; 2] = ["C-CLIP-ARR-020", "C-TIME-013"];

/// The rows that hold the fixed grid's spacings.
pub const FIXED_GRID_ROWS: [&str; 2] = ["C-CLIP-ARR-021", "C-TIME-014"];

const CLIP: &[Row] = &[
    ("C-CLIP-ARR-001", "Cut", Key(&["S-EDIT-001"])),
    ("C-CLIP-ARR-002", "Copy", Key(&["S-EDIT-002"])),
    ("C-CLIP-ARR-003", "Duplicate", Key(&["S-EDIT-004"])),
    ("C-CLIP-ARR-004", "Delete", Key(&["S-EDIT-005"])),
    SEPARATOR,
    (
        "C-CLIP-ARR-005",
        "Zoom to Time Selection",
        Key(&["S-ARR-030"]),
    ),
    (
        "C-CLIP-ARR-006",
        "Zoom Back from Time Selection",
        Key(&["S-ARR-031"]),
    ),
    SEPARATOR,
    ("C-CLIP-ARR-007", "Rename", Key(&["S-EDIT-008"])),
    ("C-CLIP-ARR-008", "Edit Info Text", Pending(None)),
    ("C-CLIP-ARR-009", "Extract Groove(s)", Pending(None)),
    ("C-CLIP-ARR-010", "Deactivate Clip", Key(&["S-ARR-027"])),
    ("C-CLIP-ARR-011", "Export MIDI Clip...", Pending(None)),
    SEPARATOR,
    ("C-CLIP-ARR-012", "Loop Selection", Key(&["S-ARR-011"])),
    SEPARATOR,
    ("C-CLIP-ARR-013", "Freeze Track", Key(&["S-TRACK-017"])),
    ("C-CLIP-ARR-014", "Bounce Track in Place", Pending(None)),
    (
        "C-CLIP-ARR-015",
        "Bounce to New Track",
        Key(&["S-BOUNCE-001"]),
    ),
    (
        "C-CLIP-ARR-016",
        "Paste Bounced Audio",
        Key(&["S-BOUNCE-002"]),
    ),
    SEPARATOR,
    ("C-CLIP-ARR-017", "Crop Clip", Key(&["S-ARR-003"])),
    ("C-CLIP-ARR-018", "Reverse Clip(s)", Key(&["S-ARR-029"])),
    SEPARATOR,
    ("C-CLIP-ARR-019", "Snap to Grid", Key(&["S-GRID-005"])),
    ("C-CLIP-ARR-020", "Adaptive Grid", Cmd(SET_GRID)),
    ("C-CLIP-ARR-021", "Fixed Grid", Cmd(SET_GRID)),
    SEPARATOR,
    ("C-CLIP-ARR-022", "Narrow Grid", Key(&["S-GRID-002"])),
    ("C-CLIP-ARR-023", "Widen Grid", Key(&["S-GRID-003"])),
    SEPARATOR,
    ("C-CLIP-ARR-024", "Triplet Grid", Key(&["S-GRID-004"])),
];

const TIME: &[Row] = &[
    ("C-TIME-001", "Cut", Key(&["S-EDIT-001"])),
    ("C-TIME-002", "Copy", Key(&["S-EDIT-002"])),
    ("C-TIME-003", "Duplicate", Key(&["S-EDIT-004"])),
    SEPARATOR,
    ("C-TIME-004", "Zoom to Time Selection", Key(&["S-ARR-030"])),
    (
        "C-TIME-005",
        "Zoom Back from Time Selection",
        Key(&["S-ARR-031"]),
    ),
    SEPARATOR,
    ("C-TIME-006", "Insert Empty MIDI Clip", Key(&["S-ARR-037"])),
    SEPARATOR,
    ("C-TIME-007", "Loop Selection", Key(&["S-ARR-011"])),
    SEPARATOR,
    ("C-TIME-008", "Freeze Track", Key(&["S-TRACK-017"])),
    ("C-TIME-009", "Bounce Track in Place", Pending(None)),
    ("C-TIME-010", "Bounce to New Track", Key(&["S-BOUNCE-001"])),
    ("C-TIME-011", "Paste Bounced Audio", Key(&["S-BOUNCE-002"])),
    SEPARATOR,
    ("C-TIME-012", "Snap to Grid", Key(&["S-GRID-005"])),
    ("C-TIME-013", "Adaptive Grid", Cmd(SET_GRID)),
    ("C-TIME-014", "Fixed Grid", Cmd(SET_GRID)),
    SEPARATOR,
    ("C-TIME-015", "Narrow Grid", Key(&["S-GRID-002"])),
    ("C-TIME-016", "Widen Grid", Key(&["S-GRID-003"])),
    SEPARATOR,
    ("C-TIME-017", "Triplet Grid", Key(&["S-GRID-004"])),
];

/// The menu a clip in the Arrangement opens (C-CLIP-ARR).
#[must_use]
pub fn arrangement_clip() -> Vec<MenuItem> {
    items(CLIP)
}

/// The menu a time selection in the Arrangement opens (C-TIME).
#[must_use]
pub fn arrangement_time() -> Vec<MenuItem> {
    items(TIME)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The rows of one context-menu section of the menu reference, up to
    /// its "Other states" table: each row's ID and Live's label, or `None`
    /// for a separator. `None` in a checkout without the planning documents
    /// (they aren't published yet), where the test skips.
    fn reference(heading: &str) -> Option<Vec<Option<(String, String)>>> {
        let docs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        if !docs.exists() {
            eprintln!(
                "skipped: the UI parity reference isn't published yet, and this checkout doesn't have it"
            );
            return None;
        }
        let text = std::fs::read_to_string(docs.join("ui-parity").join("menus.md")).unwrap();
        let mut rows = Vec::new();
        let mut inside = false;
        for line in text.lines() {
            if line.starts_with("### ") {
                inside = line.contains(heading);
                continue;
            }
            if !inside {
                continue;
            }
            if line.starts_with("Other states") {
                break;
            }
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            if cells.len() > 3 && cells[1].starts_with("C-") {
                rows.push(Some((cells[1].to_owned(), cells[2].to_owned())));
            } else if cells.len() > 3 && cells[1].is_empty() && cells[2] == "—" {
                rows.push(None);
            }
        }
        Some(rows)
    }

    #[test]
    fn the_menus_follow_lives_rows_and_separators_in_order() {
        for (menu, heading) in [
            (arrangement_clip(), "(C-CLIP-ARR)"),
            (arrangement_time(), "(C-TIME)"),
        ] {
            let Some(theirs) = reference(heading) else {
                return;
            };
            assert!(!theirs.is_empty(), "{heading}");
            assert_eq!(menu.len(), theirs.len(), "{heading}");
            for (item, row) in menu.iter().zip(&theirs) {
                match row {
                    None => assert!(item.is_separator(), "{heading}: {}", item.row),
                    Some((id, label)) => {
                        assert_eq!(item.row, id);
                        // The grid rows are rows of choices, named for
                        // their kind.
                        let label = label.split(':').next().unwrap();
                        let label = label.trim_end_matches(" choices");
                        assert_eq!(item.label, label, "{id}");
                    }
                }
            }
        }
    }
}
