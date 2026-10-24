//! The menu bar, as data: every menu, item, and separator of
//! the menu reference in Live's order (D16), each item tied to the
//! command it runs.
//!
//! An item reaches its command in one of two ways: directly
//! ([`Action::Command`]), for the shell's own commands and items without a
//! shortcut, or through its shortcut ([`Action::Shortcut`]), as menus.md
//! records it, in which case the keymap names the command for the focused
//! context. Items whose command isn't built yet are kept in place and shown
//! disabled ([`Action::Pending`]). The app turns this into native menus on
//! macOS and Windows and draws it on Linux.
//!
//! Labels are Live's words, with Beatlayer's name where Live names its own
//! product ("New Beatlayer Set", "About Beatlayer", "Read the Beatlayer
//! Manual..."); items for Ableton's services and other products (its
//! website, account, store, pack service, and Max for Live) are left out
//! ([`Action::Omitted`]). Decision D21 records the rule.

mod app;
mod create;
mod edit;
mod file;
mod help;
mod navigate;
mod options;
mod playback;
mod view;

use app::APP;
use create::CREATE;
use edit::EDIT;
use file::FILE;
use help::HELP;
use navigate::NAVIGATE;
use options::OPTIONS;
use playback::PLAYBACK;
use view::VIEW;

/// The platform a menu bar is for: macOS has an application menu; Windows
/// and Linux put Settings in Options, Quit in File, and About in Help.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuPlatform {
    /// macOS.
    Mac,
    /// Windows and Linux.
    Other,
}

impl MenuPlatform {
    /// The platform this build runs on.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Other
        }
    }
}

/// An item the operating system supplies on macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemItem {
    /// The Services submenu.
    Services,
    /// Hide Others, which handles its own key (S-FILE-010).
    HideOthers,
    /// Show All.
    ShowAll,
    /// Emoji & Symbols, which macOS adds to the Edit menu by itself, with
    /// its key (S-MENU-006).
    AddedByMacOs,
}

/// What a menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Runs a command.
    Command(&'static str),
    /// Runs whatever the first of these shortcuts runs in the focused
    /// context; disabled if none runs anything there.
    Shortcut(&'static [&'static str]),
    /// Runs a command with these arguments (JSON), one of a radio group of
    /// items that run the same command: the one the command's state
    /// matches is checked.
    Choice(&'static str, &'static str),
    /// Its command isn't built yet: shown, disabled. It may name the
    /// shortcut menus.md records for it, which the item displays.
    Pending(Option<&'static str>),
    /// A submenu: the rows after it whose IDs extend its own (`M-EDIT-013a`
    /// after `M-EDIT-013`).
    Submenu,
    /// Supplied by the operating system.
    System(SystemItem),
    /// Left out: the row names an Ableton service or another Ableton
    /// product (D21). The reason says what Beatlayer does instead.
    Omitted(&'static str),
    /// A line between groups of items, where Live draws one.
    Separator,
}

/// The row of Beatlayer's own Assistant item in the View menu (D24): Live
/// has no assistant, so it is no row of menus.md.
pub const D24: &str = "D24";

/// The row of Beatlayer's own Duplicate Track Without Clips (IMP-12),
/// next to Duplicate: an improvement, so not a row of menus.md, which
/// records Live's menus.
pub const IMP_12: &str = "IMP-12";

/// The row of Beatlayer's own Return Tracks Scroll with Tracks (IMP-01),
/// straight after Return Tracks in the Mixer Controls submenu and in the
/// Arrangement Track Controls submenu: an improvement, so not one of
/// menus.md's Live rows, though the reference names it IMP-01.
pub const IMP_01: &str = "IMP-01";

/// The row of Beatlayer's own Devices section switch (IMP-25), straight
/// after Performance Impact in the Mixer Controls submenu: an improvement,
/// so not one of menus.md's Live rows, though the reference names it
/// IMP-25.
pub const IMP_25: &str = "IMP-25";

/// The row of Beatlayer's own Close All Plug-in Windows (IMP-25), after
/// Plug-In Windows: an improvement, so not a row of menus.md.
pub const IMP_25_CLOSE_WINDOWS: &str = "IMP-25-close-windows";

/// The row of Beatlayer's own Manage Plug-Ins (IMP-29), after Manage
/// Files: an improvement, so not a row of menus.md.
pub const IMP_29: &str = "IMP-29";

/// The row of Beatlayer's own Save History (IMP-05), after Open Beatlayer
/// Set Backups: an improvement, so not one of menus.md's Live rows.
pub const IMP_05: &str = "IMP-05";

/// The rows of Beatlayer's own Search Commands… (IMP-16) at the top of Help,
/// with the separator after it, and Find Track (IMP-18) after Next Neighbor:
/// improvements, so not rows of menus.md, which records Live's menus.
pub const IMP_16: &str = "IMP-16";
/// See [`IMP_16`].
pub const IMP_16_SEPARATOR: &str = "IMP-16-separator";
/// See [`IMP_16`].
pub const IMP_18: &str = "IMP-18";

/// The rows of the Create menu's Sample Editor items, which menus.md's note
/// under the Create menu names but gives no rows (WP-4.10).
pub const SAMPLE_WARP: &str = "WP-4.10-warp";
/// See [`SAMPLE_WARP`].
pub const SAMPLE_INSERT: &str = "WP-4.10-insert";
/// See [`SAMPLE_WARP`].
pub const SAMPLE_DELETE: &str = "WP-4.10-delete";

/// Whether a row is one of Live's, in menus.md, rather than one of
/// Beatlayer's improvements.
#[must_use]
pub fn is_lives(row: &str) -> bool {
    row.is_empty() || row.starts_with("M-")
}

/// One row of menus.md, or of an improvement, as Beatlayer shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// The row's ID in menus.md, such as `M-FILE-009`; empty for a
    /// separator.
    pub row: &'static str,
    /// The label Beatlayer shows.
    pub label: &'static str,
    /// What choosing it does.
    pub action: Action,
    /// A submenu's items.
    pub items: Vec<MenuItem>,
}

/// The rows whose items the shell fills in now (WP-6.63): the backups in
/// File ▸ Open Beatlayer Set Backups, which change as the set is saved.
pub const DYNAMIC_ROWS: &[&str] = &["M-FILE-008"];

/// A menu item the shell fills in now (WP-6.63): the backups in File ▸ Open
/// Beatlayer Set Backups, which change as the set is saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicItem {
    /// The item's label.
    pub label: String,
    /// The command it runs.
    pub id: &'static str,
    /// The arguments it runs with, as JSON.
    pub args: String,
}

impl MenuItem {
    /// The command this item runs directly, if it does.
    #[must_use]
    pub fn command(&self) -> Option<&'static str> {
        match self.action {
            Action::Command(id) | Action::Choice(id, _) => Some(id),
            _ => None,
        }
    }

    /// Whether the item appears in the menu. A separator appears unless
    /// [`shown`] drops it.
    #[must_use]
    pub fn is_shown(&self) -> bool {
        !matches!(
            self.action,
            Action::Omitted(_) | Action::System(SystemItem::AddedByMacOs)
        )
    }

    /// Whether the item is a separator.
    #[must_use]
    pub fn is_separator(&self) -> bool {
        self.action == Action::Separator
    }
}

/// The items a menu shows, in order: hidden items left out, and with them
/// any separator that would then open or close the menu or follow another.
#[must_use]
pub fn shown(items: &[MenuItem]) -> Vec<&MenuItem> {
    let mut out: Vec<&MenuItem> = Vec::with_capacity(items.len());
    for item in items.iter().filter(|item| item.is_shown()) {
        if item.is_separator() && out.last().is_none_or(|last| last.is_separator()) {
            continue;
        }
        out.push(item);
    }
    while out.last().is_some_and(|last| last.is_separator()) {
        out.pop();
    }
    out
}

/// A menu-bar menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    /// Its title, such as `File`.
    pub title: &'static str,
    /// The letter that opens it with Alt on Windows (S-MENU-001), if any.
    pub mnemonic: Option<char>,
    /// Its items in order, submenus nested, separators included.
    pub items: Vec<MenuItem>,
}

impl Menu {
    /// Every item, submenu items and separators included, in order.
    pub fn walk(&self) -> impl Iterator<Item = &MenuItem> {
        fn push<'a>(items: &'a [MenuItem], out: &mut Vec<&'a MenuItem>) {
            for item in items {
                out.push(item);
                push(&item.items, out);
            }
        }
        let mut out = Vec::new();
        push(&self.items, &mut out);
        out.into_iter()
    }
}

use Action::{Submenu, System};

pub(crate) type Row = (&'static str, &'static str, Action);

/// A separator row.
pub(crate) const SEPARATOR: Row = ("", "", Action::Separator);

/// Builds items from rows, nesting each submenu's rows under it.
pub(crate) fn items(rows: &[Row]) -> Vec<MenuItem> {
    let mut out: Vec<MenuItem> = Vec::new();
    let mut previous = "";
    for &(row, label, action) in rows {
        let item = MenuItem {
            row,
            label,
            action,
            items: Vec::new(),
        };
        // A row joins the submenu above it when its ID starts with the
        // submenu's, and so do IMP-01's and IMP-25's rows, which the
        // reference names with their improvements' IDs, when the row above
        // it was one of the submenu's.
        let joins = |submenu: &str| {
            row.starts_with(submenu)
                || ((row == IMP_01 || row == IMP_25) && previous.starts_with(submenu))
        };
        let parent = out
            .last_mut()
            .filter(|last| last.action == Submenu && !row.is_empty() && joins(last.row));
        match parent {
            Some(parent) => parent.items.push(item),
            None => out.push(item),
        }
        previous = row;
    }
    out
}

/// The row of `rows` with this ID, as an item.
fn take(rows: &[Row], id: &str) -> MenuItem {
    let &(row, label, action) = rows
        .iter()
        .find(|(row, ..)| *row == id)
        .expect("the row exists");
    MenuItem {
        row,
        label,
        action,
        items: Vec::new(),
    }
}

/// The menu bar for a platform, in Live's order: on macOS the application
/// menu first; on Windows and Linux, Settings at the end of Options, Quit
/// at the end of File, and About at the end of Help, where Live's manual
/// puts the first two (their exact place on Windows, and the separators
/// there, are still to observe).
#[must_use]
pub fn menu_bar(platform: MenuPlatform) -> Vec<Menu> {
    let menu = |title, mnemonic, rows: &[Row]| Menu {
        title,
        mnemonic,
        items: items(rows),
    };
    let mut menus = vec![
        menu("File", Some('F'), FILE),
        menu("Edit", Some('E'), EDIT),
        menu("Create", Some('C'), CREATE),
        menu("Playback", Some('P'), PLAYBACK),
        menu("View", Some('V'), VIEW),
        menu("Navigate", Some('N'), NAVIGATE),
        menu("Options", Some('O'), OPTIONS),
        menu("Help", Some('H'), HELP),
    ];
    match platform {
        MenuPlatform::Mac => menus.insert(0, menu("Beatlayer", None, APP)),
        MenuPlatform::Other => {
            menus[0].items.push(take(APP, "M-APP-007"));
            menus[6].items.push(take(APP, "M-APP-002"));
            menus[7].items.push(take(APP, "M-APP-001"));
            for menu in &mut menus {
                menu.items
                    .retain(|item| !matches!(item.action, System(SystemItem::AddedByMacOs)));
            }
        }
    }
    menus
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::commands::{APP_ABOUT, APP_QUIT, APP_SETTINGS};

    /// A menus.md row: its ID and Live's label, or `None` for a separator.
    type Reference = Option<(String, String)>;

    /// Each menu-bar section of menus.md, by heading, with its rows and
    /// separators in order; `None` in a checkout without the planning
    /// documents (they aren't published yet), where the test skips.
    fn reference_menus() -> Option<Vec<(String, Vec<Reference>)>> {
        let docs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        if !docs.exists() {
            eprintln!(
                "skipped: the UI parity reference isn't published yet, and this checkout doesn't have it"
            );
            return None;
        }
        let text = std::fs::read_to_string(docs.join("ui-parity/menus.md")).unwrap();
        let mut menus: Vec<(String, Vec<Reference>)> = Vec::new();
        for line in text.lines() {
            if let Some(heading) = line.strip_prefix("## ") {
                // The menu bar's sections end where the Control Bar's and the
                // context menus' begin.
                if heading.starts_with("Control Bar menus") || heading.starts_with("Context menus")
                {
                    break;
                }
                menus.push((heading.to_owned(), Vec::new()));
                continue;
            }
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            let Some((_, rows)) = menus.last_mut() else {
                continue;
            };
            if cells.len() > 3 && cells[1].starts_with("M-") {
                let label = cells[2].trim_end_matches(" ▸").to_owned();
                rows.push(Some((cells[1].to_owned(), label)));
            } else if cells.len() > 3 && cells[1].is_empty() && cells[2] == "—" {
                rows.push(None);
            }
        }
        menus.retain(|(_, rows)| !rows.is_empty());
        Some(menus)
    }

    #[test]
    fn every_menu_row_and_separator_is_in_the_model_in_order() {
        // Spec: app-shell R15.
        let Some(reference) = reference_menus() else {
            return;
        };
        let bar = menu_bar(MenuPlatform::Mac);
        assert_eq!(bar.len(), reference.len(), "nine menus");
        assert!(
            reference[0].0.starts_with("Live"),
            "the application menu comes first on macOS"
        );
        let mut separators = 0;
        for (menu, (heading, rows)) in bar.iter().zip(&reference) {
            let ours: Vec<&str> = menu
                .walk()
                .filter(|item| is_lives(item.row))
                .map(|item| if item.is_separator() { "—" } else { item.row })
                .collect();
            let theirs: Vec<&str> = rows
                .iter()
                .map(|row| row.as_ref().map_or("—", |(id, _)| id.as_str()))
                .collect();
            assert_eq!(
                ours, theirs,
                "menu {} against menus.md's {heading}",
                menu.title
            );
            separators += ours.iter().filter(|row| **row == "—").count();
        }
        assert_eq!(separators, 65, "README: 65 separators in the menu bar");
    }

    #[test]
    fn labels_are_lives_with_beatlayers_name_and_no_ableton_services() {
        // Spec: app-shell R15.
        // D21: Beatlayer's name where Live names its own product; items for
        // Ableton's services and other products left out; everything else
        // in Live's words.
        let Some(menus) = reference_menus() else {
            return;
        };
        let reference: Vec<(String, String)> = menus
            .into_iter()
            .flat_map(|(_, rows)| rows)
            .flatten()
            .collect();
        let bar = menu_bar(MenuPlatform::Mac);
        let items: Vec<&MenuItem> = bar
            .iter()
            .flat_map(Menu::walk)
            .filter(|i| !i.is_separator() && is_lives(i.row))
            .collect();
        assert_eq!(items.len(), reference.len());
        let mut omitted = Vec::new();
        for (item, (id, live)) in items.iter().zip(&reference) {
            assert_eq!(item.row, id);
            if let Action::Omitted(reason) = item.action {
                assert!(reason.contains("D21"), "{id}: {reason}");
                omitted.push(item.row);
                continue;
            }
            let expected = live
                .split(' ')
                .map(|word| if word == "Live" { "Beatlayer" } else { word })
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(item.label, expected, "{id}");
            for word in ["Live", "Ableton", "ableton.com", "Max for"] {
                assert!(!item.label.contains(word), "{id}: {}", item.label);
            }
        }
        assert_eq!(
            omitted,
            [
                "M-VIEW-006",
                "M-VIEW-030",
                "M-OPT-028",
                "M-HELP-002",
                "M-HELP-005",
                "M-HELP-008",
                "M-HELP-009"
            ]
        );
        for (id, live) in &reference {
            if live.contains("Max for Live") || live.contains("ableton.com") {
                assert!(omitted.contains(&id.as_str()), "{id}");
            }
        }
    }

    #[test]
    fn submenus_nest_their_rows() {
        let bar = menu_bar(MenuPlatform::Mac);
        let edit = bar.iter().find(|m| m.title == "Edit").unwrap();
        let markers = edit.items.iter().find(|i| i.row == "M-EDIT-013").unwrap();
        let rows: Vec<&str> = markers.items.iter().map(|i| i.row).collect();
        assert_eq!(
            rows,
            ["M-EDIT-013a", "M-EDIT-013b", "M-EDIT-013c", "M-EDIT-013d"]
        );
        assert!(edit.items.iter().all(|i| i.row != "M-EDIT-013a"));
    }

    #[test]
    fn hidden_items_take_their_spare_separators_with_them() {
        // Spec: app-shell R15.
        for platform in [MenuPlatform::Mac, MenuPlatform::Other] {
            for menu in menu_bar(platform) {
                let items = shown(&menu.items);
                assert!(!items.first().unwrap().is_separator(), "{}", menu.title);
                assert!(!items.last().unwrap().is_separator(), "{}", menu.title);
                for pair in items.windows(2) {
                    assert!(
                        !(pair[0].is_separator() && pair[1].is_separator()),
                        "{}",
                        menu.title
                    );
                }
                assert!(items.iter().all(|item| item.is_shown()));
            }
        }
        // Help loses three of Live's items and keeps its groups.
        let bar = menu_bar(MenuPlatform::Mac);
        let help = bar.iter().find(|m| m.title == "Help").unwrap();
        let rows: Vec<&str> = shown(&help.items)
            .iter()
            .map(|i| if i.is_separator() { "—" } else { i.row })
            .collect();
        assert!(!rows.contains(&"M-HELP-005"));
        assert_eq!(rows.last(), Some(&"M-HELP-010"));
    }

    #[test]
    fn other_platforms_move_the_application_menu_items() {
        let bar = menu_bar(MenuPlatform::Other);
        assert_eq!(bar[0].title, "File");
        assert_eq!(bar[0].items.last().unwrap().command(), Some(APP_QUIT));
        assert_eq!(bar[6].items.last().unwrap().command(), Some(APP_SETTINGS));
        assert_eq!(bar[7].items.last().unwrap().command(), Some(APP_ABOUT));
        assert!(bar.iter().all(|m| m.walk().all(|i| i.row != "M-APP-003")));
        assert!(bar.iter().all(|m| m.walk().all(|i| i.row != "M-EDIT-040")));
    }
}
