//! The context menus, as data: the menu a clip opens and the menu a time
//! selection opens in the Arrangement (WP-1.8b), the menu a device's
//! title bar and Show Options button open (WP-2.4), and the menu a track's
//! title bar opens (WP-4.22), in Live's order from
//! the menu reference (C-CLIP-ARR, C-TIME, C-DEVICE, C-TRACK), each item tied to its
//! shortcut as the menu bar's are ([`Action::Shortcut`]), so it runs the
//! command the keymap gives that shortcut in the Arrangement, or shows
//! disabled until a package binds it.
//!
//! The grid's Adaptive Grid and Fixed Grid rows are rows of choices rather
//! than single items: they carry `arrangement.set_grid`, and the UI lays
//! out a choice for each setting.

use crate::arrangement::SET_GRID;
use crate::devices::{SAVE_DEFAULT_PRESET, SET_FLOATING};
use crate::menus::{Action, IMP_12, MenuItem, Row, SEPARATOR, items};
use crate::tracks::DUPLICATE_WITHOUT_CLIPS;

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
    ("C-DEVICE-007", "Group", Pending(Some("S-DEV-001"))),
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

const TRACK: &[Row] = &[
    ("C-TRACK-001", "Cut", Key(&["S-EDIT-001"])),
    ("C-TRACK-002", "Copy", Key(&["S-EDIT-002"])),
    ("C-TRACK-003", "Duplicate", Key(&["S-EDIT-004"])),
    // IMP-12 places it straight after Duplicate.
    (
        IMP_12,
        "Duplicate Track Without Clips",
        Cmd(DUPLICATE_WITHOUT_CLIPS),
    ),
    ("C-TRACK-004", "Delete", Key(&["S-TRACK-018"])),
    SEPARATOR,
    ("C-TRACK-005", "Rename", Key(&["S-TRACK-004"])),
    ("C-TRACK-006", "Edit Info Text", Pending(None)),
    SEPARATOR,
    ("C-TRACK-007", "Freeze Track", Pending(Some("S-TRACK-017"))),
    ("C-TRACK-008", "Bounce Track in Place", Pending(None)),
    SEPARATOR,
    ("C-TRACK-009", "Insert Audio Track", Key(&["S-TRACK-001"])),
    ("C-TRACK-010", "Insert MIDI Track", Key(&["S-TRACK-002"])),
    ("C-TRACK-011", "Insert Return Track", Key(&["S-TRACK-003"])),
    ("C-TRACK-012", "Group Tracks", Key(&["S-TRACK-006"])),
    SEPARATOR,
    ("C-TRACK-013", "Select Track Content", Pending(None)),
    SEPARATOR,
    ("C-TRACK-014", "Save as Default MIDI Track", Pending(None)),
    SEPARATOR,
    ("C-TRACK-015", "Colour palette", Pending(None)),
    SEPARATOR,
    ("C-TRACK-016", "Assign Track Color to Clips", Pending(None)),
];

/// What makes a track's menu differ from the base one, C-TRACK-001 to
/// C-TRACK-016 (C-TRACK-017 to C-TRACK-020).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackMenu {
    /// The track is a group (C-TRACK-017).
    pub group: bool,
    /// Several tracks are selected (C-TRACK-018).
    pub several: bool,
    /// The menu opens in the Arrangement (C-TRACK-019).
    pub arrangement: bool,
    /// The Arrangement has a time selection on the track (C-TRACK-020).
    pub time_selection: bool,
}

/// The rows the other states add after row `after`. menus.md doesn't
/// record where Live puts them: they sit by the items they go with.
fn track_extras(after: &str, state: TrackMenu) -> Vec<Row> {
    let mut rows = Vec::new();
    match after {
        "C-TRACK-008" => {
            if state.group {
                rows.push(("C-TRACK-017c", "Bounce Group to New Track", Pending(None)));
            }
            if state.time_selection {
                rows.push(("C-TRACK-020", "Crop Clip", Pending(Some("S-ARR-003"))));
            }
        }
        "C-TRACK-012" => {
            if state.group {
                rows.push(("C-TRACK-017a", "Ungroup Tracks", Key(&["S-TRACK-007"])));
            }
            if state.several {
                rows.push(("C-TRACK-018a", "Link Tracks", Pending(None)));
                rows.push(("C-TRACK-018b", "Unlink Track(s)", Pending(None)));
            }
            if state.arrangement {
                rows.push(SEPARATOR);
                rows.push(("C-TRACK-019a", "Show Take Lanes", Pending(None)));
                rows.push((
                    "C-TRACK-019b",
                    "Insert Take Lane",
                    Pending(Some("S-COMP-004")),
                ));
                rows.push(("C-TRACK-019c", "Delete All Take Lanes", Pending(None)));
            }
        }
        "C-TRACK-016" if state.group => rows.push((
            "C-TRACK-017b",
            "Assign Track Color to Grouped Tracks and Clips",
            Pending(None),
        )),
        _ => {}
    }
    rows
}

/// The menu a track's title bar opens, by a right-click in the Arrangement's
/// headers, the Session View, or a mixer (C-TRACK), with the items the
/// track's `state` adds.
#[must_use]
pub fn track(state: TrackMenu) -> Vec<MenuItem> {
    let mut rows = Vec::with_capacity(TRACK.len() + 8);
    for &row in TRACK {
        rows.push(row);
        rows.extend(track_extras(row.0, state));
    }
    items(&rows)
}

/// The package or milestone that builds a context-menu item shown disabled
/// for now, for the item to name.
#[must_use]
pub fn pending_package(row: &str) -> Option<&'static str> {
    match row {
        "C-DEVICE-005" | "C-CLIP-ARR-008" | "C-TRACK-006" => Some("WP-2.24 (the Info View)"),
        "C-TRACK-007" => Some("WP-4.14 (Freeze Track)"),
        "C-TRACK-008" | "C-TRACK-017c" => Some("WP-4.15 (Bounce Track in Place and Bounce Group)"),
        "C-TRACK-020" => Some("WP-4.13 (Crop Clip)"),
        "C-TRACK-018a" | "C-TRACK-018b" => Some("Phase 6 (linked-track editing)"),
        "C-TRACK-019a" | "C-TRACK-019b" | "C-TRACK-019c" => {
            Some("Phase 6 (take lanes and comping)")
        }
        "C-TRACK-015" | "C-TRACK-016" | "C-TRACK-017b" => {
            Some("a package not planned yet (track colours)")
        }
        "C-TRACK-013" | "C-TRACK-014" => Some("a package not planned yet"),
        "C-DEVICE-007" => Some("milestone M5 (racks)"),
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

    const EVERY_STATE: TrackMenu = TrackMenu {
        group: true,
        several: true,
        arrangement: true,
        time_selection: true,
    };

    #[test]
    fn the_device_and_track_menus_pending_items_name_their_package() {
        for item in device().into_iter().chain(track(EVERY_STATE)) {
            if matches!(item.action, Pending(_)) {
                assert!(pending_package(item.row).is_some(), "{}", item.row);
            }
        }
    }

    #[test]
    fn the_track_menus_other_states_add_the_items_menus_md_lists() {
        let base = track(TrackMenu::default());
        let all = track(EVERY_STATE);
        let added: Vec<_> = all
            .iter()
            .filter(|i| !i.is_separator() && !base.iter().any(|b| b.row == i.row))
            .collect();
        assert_eq!(added.len(), 9);
        let Some(text) = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/ui-parity/menus.md"),
        )
        .ok() else {
            return;
        };
        for item in added {
            let id = &item.row[..11];
            let line = text
                .lines()
                .find(|l| l.starts_with(&format!("| {id} |")))
                .unwrap_or_else(|| panic!("{id} isn't in menus.md"));
            assert!(line.contains(item.label), "{}: {line}", item.label);
        }
    }

    #[test]
    fn the_menus_follow_lives_rows_and_separators_in_order() {
        for (menu, heading) in [
            (arrangement_clip(), "(C-CLIP-ARR)"),
            (arrangement_time(), "(C-TIME)"),
            (device(), "(C-DEVICE)"),
            (track(TrackMenu::default()), "(C-TRACK)"),
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
