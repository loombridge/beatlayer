//! Commands in this area that have no key and no menu item, and how each is reached.

use crate::accessibility::reachability::setups::*;
use crate::accessibility::reachability::*;
use egui::Key;

pub(crate) fn entries() -> Vec<(&'static str, Path)> {
    use Path::Exception;
    use Reason::{Equivalent, Gap, Live, Query};
    vec![
        // The browser's Filter View, labels and columns (browser-tags R18
        // to R28).
        ("browser.add_label", Exception(FILTERS)),
        ("browser.clear_filters", Exception(FILTERS)),
        ("browser.filter", Exception(FILTERS)),
        ("browser.set_filter_groups", Exception(FILTERS)),
        ("browser.move_label", Exception(LABELS)),
        ("browser.remove_label", Exception(LABELS)),
        ("browser.reset_label_order", Exception(LABELS)),
        ("browser.set_label_icon", Exception(LABELS)),
        ("browser.set_label_shown", Exception(LABELS)),
        ("browser.set_columns", Exception(COLUMNS)),
        // The browser's tags (browser-tags R10 to R16).
        ("browser.add_tag", Exception(TAG_EDITOR)),
        ("browser.add_tag_group", Exception(TAG_EDITOR)),
        ("browser.delete_tag", Exception(TAG_EDITOR)),
        ("browser.move_tag", Exception(TAG_EDITOR)),
        ("browser.rename_tag", Exception(TAG_EDITOR)),
        ("browser.reset_tag_order", Exception(TAG_EDITOR)),
        ("browser.tag_items", Exception(TAG_EDITOR)),
        ("browser.show_quick_tags", Exception(QUICK_TAGS)),
        // Typing in the browser's search field (browser R5).
        (
            "browser.search",
            main(nothing, browser_search, "Search Field", Key::K),
        ),
        ("browser.state", Exception(Query)),
        ("browser.tags", Exception(Query)),
        // The File Manager (WP-6.63): its own rows and buttons reach
        // these, and its views open from the File and View menus.
        ("file_manager.state", Exception(Query)),
        (
            "file_manager.replace_file",
            Exception(Equivalent("the File Manager's rows (WP-6.63)")),
        ),
        (
            "file_manager.hot_swap",
            Exception(Equivalent("the File Manager's rows (WP-6.63)")),
        ),
        (
            "file_manager.edit_file",
            Exception(Equivalent("the File Manager's rows (WP-6.63)")),
        ),
        (
            "file_manager.delete_unused",
            Exception(Equivalent("the File Manager's rows (WP-6.63)")),
        ),
        (
            "file_manager.set_search_folder",
            Exception(Equivalent("the File Manager's rows (WP-6.63)")),
        ),
        (
            "browser.add_to_arrangement",
            Exception(Equivalent(
                "a drag; Enter adds the selected files (`browser.load_selected`)",
            )),
        ),
        (
            "browser.stop_preview",
            Exception(Equivalent(
                "selecting a folder, or switching Options ▸ Browser File Preview off, stops a \
                 preview",
            )),
        ),
        (
            "browser.scrub_preview",
            Exception(Live(
                "clicking in the browser's preview waveform is the pointer's way; Live has no \
                 keyboard shortcut for scrubbing a preview",
            )),
        ),
        (
            "browser.set_raw",
            Exception(Live(
                "clicking the Raw button in the browser's preview strip; Live has no keyboard \
                 shortcut for the Raw switch",
            )),
        ),
        (
            "browser.show_file",
            Exception(Equivalent(
                "the Sample Editor's context menu, opened from the keyboard with Shift+F10 \
                 (Ctrl+Option+Shift+M on macOS): its Show in Browser item",
            )),
        ),
        (
            "browser.reveal",
            Exception(Equivalent(
                "the browser item's context menu, opened from the keyboard with Shift+F10 \
                 (Ctrl+Option+Shift+M on macOS): its Show in Finder item",
            )),
        ),
        // The search field (WP-6.42): Tab reaches its scope buttons and Show
        // Only Matches, and Enter presses them; the text, the arrows, Return
        // and Esc act on it from the keyboard.
        (
            "search.set_scope",
            main(search_commands, nothing, "Tracks", Key::Enter),
        ),
        (
            "search.set_only_matches",
            main(search_tracks, nothing, "Show Only Matches", Key::Enter),
        ),
        (
            "search.set_text",
            Exception(Equivalent("typing in the search field")),
        ),
        (
            "search.next",
            Exception(Equivalent(
                "Down in the search field's Commands scope, Return in its Tracks scope",
            )),
        ),
        (
            "search.previous",
            Exception(Equivalent(
                "Up in the search field's Commands scope, Shift-Return in its Tracks scope",
            )),
        ),
        (
            "search.close",
            Exception(Equivalent("Esc in the search field")),
        ),
        ("search.state", Exception(Query)),
        ("search.commands", Exception(Query)),
        ("search.tracks", Exception(Query)),
        (
            "set.import",
            Exception(Equivalent(
                "Enter on a track or a Devices row of a set opened in the browser \
                 (`browser.load_selected`)",
            )),
        ),
        (
            "set.export_tracks",
            Exception(Gap(
                "a drag of track title bars into the browser has no keyboard form yet (set \
                 exchange R11)",
            )),
        ),
        (
            "set.export_clips",
            Exception(Gap(
                "a drag of Session clips into the browser has no keyboard form yet (set \
                 exchange R12)",
            )),
        ),
        (
            "browser.rename_file",
            Exception(Equivalent(
                "typing in the saved clip's name field in the browser's list: Enter renames \
                 the file, Esc keeps its name (clip files R4)",
            )),
        ),
    ]
}
