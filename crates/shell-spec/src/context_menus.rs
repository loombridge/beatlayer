//! The context menus, as data: the menu a clip opens and the menu a time
//! selection opens in the Arrangement (WP-1.8b), and the menu a device's
//! title bar and Show Options button open (WP-2.4), in Live's order from
//! the menu reference (C-CLIP-ARR, C-TIME, C-DEVICE), each item tied to its
//! shortcut as the menu bar's are ([`Action::Shortcut`]), so it runs the
//! command the keymap gives that shortcut in the Arrangement, or shows
//! disabled until a package binds it.
//!
//! The grid's Adaptive Grid and Fixed Grid rows are rows of choices rather
//! than single items: they carry `arrangement.set_grid`, and the UI lays
//! out a choice for each setting.

use crate::arrangement::SET_GRID;
use crate::devices::{SAVE_DEFAULT_PRESET, SET_FLOATING};
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

/// The row of Beatlayer's own Float (IMP-14, WP-2.5b), after Fold: an
/// improvement, so not a row of menus.md, which records Live's menus. It's
/// ticked while the selected device floats, and choosing it again docks it.
pub const IMP_14: &str = "IMP-14";

/// Whether a context menu's row is one of Live's, in menus.md, rather than
/// one of Beatlayer's improvements.
#[must_use]
pub fn is_lives(row: &str) -> bool {
    row.is_empty() || row.starts_with("C-")
}

const DEVICE: &[Row] = &[
    ("C-DEVICE-001", "Cut", Key(&["S-EDIT-001"])),
    ("C-DEVICE-002", "Copy", Key(&["S-EDIT-002"])),
    ("C-DEVICE-003", "Duplicate", Key(&["S-EDIT-004"])),
    SEPARATOR,
    ("C-DEVICE-004", "Rename", Key(&["S-EDIT-008"])),
    ("C-DEVICE-005", "Edit Info Text", Pending(None)),
    SEPARATOR,
    ("C-DEVICE-006", "Delete", Key(&["S-EDIT-005"])),
    ("C-DEVICE-007", "Group", Key(&["S-DEV-001"])),
    ("C-DEVICE-008", "Fold", Key(&["S-DEV-014"])),
    (IMP_14, "Float", Cmd(SET_FLOATING)),
    (
        "C-DEVICE-009",
        "Show Preset Name",
        Cmd("device.set_show_preset_name"),
    ),
    SEPARATOR,
    (
        "C-DEVICE-010",
        "Compare: Switch to B",
        Pending(Some("S-DEV-011")),
    ),
    ("C-DEVICE-011", "Compare: Copy A to B", Pending(None)),
    SEPARATOR,
    (
        "C-DEVICE-012",
        "Save as Default Preset",
        Cmd(SAVE_DEFAULT_PRESET),
    ),
];

/// The menu a device's title bar opens, by a right-click or its Show
/// Options button (C-DEVICE). A device's own items, which Live lists after
/// these, follow them in the menu: the device's switches that its type marks
/// as menu options ([`DeviceFrame::menu_options`](crate::devices::DeviceFrame::menu_options),
/// C-DEVICE-015).
#[must_use]
pub fn device() -> Vec<MenuItem> {
    items(DEVICE)
}

/// The package or milestone that builds a context-menu item shown disabled
/// for now, for the item to name.
#[must_use]
pub fn pending_package(row: &str) -> Option<&'static str> {
    match row {
        "C-DEVICE-005" | "C-CLIP-ARR-008" => Some("WP-2.24 (the Info View)"),
        "C-DEVICE-010" | "C-DEVICE-011" => Some("milestone M5 (A/B compare)"),
        _ => None,
    }
}

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
    fn the_device_menus_pending_items_name_their_package() {
        for item in device() {
            if matches!(item.action, Pending(_)) {
                assert!(pending_package(item.row).is_some(), "{}", item.row);
            }
        }
    }

    #[test]
    fn the_menus_follow_lives_rows_and_separators_in_order() {
        for (menu, heading) in [
            (arrangement_clip(), "(C-CLIP-ARR)"),
            (arrangement_time(), "(C-TIME)"),
            (device(), "(C-DEVICE)"),
        ] {
            let Some(theirs) = reference(heading) else {
                return;
            };
            assert!(!theirs.is_empty(), "{heading}");
            let menu: Vec<_> = menu.into_iter().filter(|i| is_lives(i.row)).collect();
            assert_eq!(menu.len(), theirs.len(), "{heading}");
            for (item, row) in menu.iter().zip(&theirs) {
                match row {
                    None => assert!(item.is_separator(), "{heading}: {}", item.row),
                    Some((id, label)) => {
                        assert_eq!(item.row, id);
                        // The grid rows are rows of choices, named for
                        // their kind.
                        let grid = ADAPTIVE_GRID_ROWS.contains(&id.as_str())
                            || FIXED_GRID_ROWS.contains(&id.as_str());
                        let label = if grid {
                            label.split(':').next().unwrap()
                        } else {
                            label
                        };
                        let label = label.trim_end_matches(" choices");
                        assert_eq!(item.label, label, "{id}");
                    }
                }
            }
        }
    }
}
