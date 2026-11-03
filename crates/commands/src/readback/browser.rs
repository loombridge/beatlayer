//! The Browser and the File Manager.

use super::Object::{Clip, Track};
use super::{BROWSER, Readback, nothing, query, read};

/// Why the browser's tag commands read back through nothing a read command
/// gives.
const TAGS: &str = "the browser's tags are read with `browser.tags`, which is `external` because \
                    it lists files outside the project";
/// Why the browser's collection commands read back through nothing a read
/// command gives.
const COLLECTIONS: &str = "the browser's collections are read with `browser.state`, whose rows \
                           name the collections each item is in, whose labels list the nested \
                           collections under their parents, and which is `external` because it \
                           lists files outside the project";
/// Why the File Manager's commands read back through nothing a read command
/// gives.
const FILE_MANAGER: &str = "the File Manager's state is read with `file_manager.state`, which \
                             is `external` because it lists files outside the project";
/// The readback of this area's commands, one line each, so a command's entry is easy to find.
#[rustfmt::skip]
pub(super) const ROWS: &[(&str, Readback)] = &[
    ("browser.add_collection", nothing(COLLECTIONS)),
    ("browser.add_folder", nothing(BROWSER)),
    ("browser.add_label", nothing(BROWSER)),
    ("browser.set_analysis_paused", read("browser.analysis_state", "paused")),
    ("browser.add_tag", nothing(TAGS)),
    ("browser.add_tag_group", nothing(TAGS)),
    ("browser.add_to_arrangement", query(Track, "clips")),
    ("browser.clear_collections", nothing(COLLECTIONS)),
    ("browser.clear_filters", nothing(BROWSER)),
    ("browser.back", nothing(BROWSER)),
    ("browser.delete_collection", nothing(COLLECTIONS)),
    ("browser.delete_tag", nothing(TAGS)),
    ("browser.forward", nothing(BROWSER)),
    ("browser.filter", nothing(BROWSER)),
    ("browser.load_selected", query(Track, "clips")),
    ("browser.move_collection", nothing(COLLECTIONS)),
    ("browser.move_label", nothing(BROWSER)),
    ("browser.move_tag", nothing(TAGS)),
    ("browser.open_folder", nothing(BROWSER)),
    ("browser.open_place", nothing(BROWSER)),
    ("browser.preview", nothing(BROWSER)),
    ("browser.remove_folder", nothing(BROWSER)),
    ("browser.remove_label", nothing(BROWSER)),
    ("browser.rename_file", nothing(BROWSER)),
    ("browser.rename_label", nothing(BROWSER)),
    ("browser.rename_tag", nothing(TAGS)),
    (
        "browser.reveal",
        nothing(
            "opens the system's file manager, another application; neither the browser nor the \
             set changes",
        ),
    ),
    ("browser.reset_label_order", nothing(BROWSER)),
    ("browser.reset_tag_order", nothing(TAGS)),
    ("browser.scrub_preview", nothing(BROWSER)),
    ("browser.search", nothing(BROWSER)),
    ("browser.select", nothing(BROWSER)),
    ("browser.set_collection", nothing(COLLECTIONS)),
    ("browser.set_label_shown", nothing(BROWSER)),
    ("browser.set_label_icon", nothing(BROWSER)),
    ("browser.set_filter_view", nothing(BROWSER)),
    ("browser.set_include_auto_tags", nothing(BROWSER)),
    ("browser.set_filter_groups", nothing(BROWSER)),
    ("browser.set_columns", nothing(BROWSER)),
    ("browser.set_preview", nothing(BROWSER)),
    ("browser.set_raw", nothing(BROWSER)),
    ("browser.show_file", nothing(BROWSER)),
    ("browser.show_quick_tags", nothing(BROWSER)),
    ("browser.show_similar", nothing(BROWSER)),
    ("browser.tag_items", nothing(TAGS)),
    (
        "browser.tags",
        nothing(
            "a query of its own: it reads the tags, and is `external` for the files it lists",
        ),
    ),
    (
        "browser.state",
        nothing(
            "a query of its own: it reads the Browser, and is `external` for the files it lists",
        ),
    ),
    (
        "file_manager.delete_unused",
        nothing("moves files outside the set to the trash; the set doesn't change"),
    ),
    ("file_manager.edit_file", query(Clip, "data.audio.media")),
    ("file_manager.hot_swap", nothing(FILE_MANAGER)),
    ("file_manager.replace_file", query(Clip, "data.audio.media")),
    (
        "file_manager.state",
        nothing(
            "a query of its own: it reads the File Manager, and is `external` for the files it \
             lists",
        ),
    ),
];
