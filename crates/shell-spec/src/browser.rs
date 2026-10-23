//! The browser's commands in the command registry (WP-1.10; decisions D16
//! and D17): places, browsing, search, preview, and adding files to the
//! Arrangement.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them
//! by ID. Commands that read folders or files outside the project, or play
//! through the audio device, are in the external class, so an agent asks the
//! person first. The behaviour they follow is in the browser spec.

use bl_commands::Context::{
    Arrangement, Browser, BrowserSearch, BrowserSidebar, ClipView, Global, SampleEditor, Session,
};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{ClipId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `browser.state`: what the browser shows.
pub const STATE: &str = "browser.state";
/// `browser.search`: search the places by file name.
pub const SEARCH: &str = "browser.search";
/// `browser.focus_search`: go to the search field (Cmd-F).
pub const FOCUS_SEARCH: &str = "browser.focus_search";
/// `browser.open_place`: choose a label in the sidebar.
pub const OPEN_PLACE: &str = "browser.open_place";
/// `browser.open_folder`: open or close a folder in the content pane.
pub const OPEN_FOLDER: &str = "browser.open_folder";
/// `browser.select`: select items in the content pane.
pub const SELECT: &str = "browser.select";
/// `browser.show_file`: show an audio file in the browser (Show in Browser).
pub const SHOW_FILE: &str = "browser.show_file";
/// `browser.add_folder`: add a folder to Places.
pub const ADD_FOLDER: &str = "browser.add_folder";
/// `browser.remove_folder`: take an added folder out of Places.
pub const REMOVE_FOLDER: &str = "browser.remove_folder";
/// `browser.preview`: preview a file.
pub const PREVIEW: &str = "browser.preview";
/// `browser.stop_preview`: stop the preview.
pub const STOP_PREVIEW: &str = "browser.stop_preview";
/// `browser.set_preview`: the Preview switch and the preview volume.
pub const SET_PREVIEW: &str = "browser.set_preview";
/// `browser.set_raw`: the Raw switch in the preview strip.
pub const SET_RAW: &str = "browser.set_raw";
/// `browser.scrub_preview`: move the preview to a position.
pub const SCRUB_PREVIEW: &str = "browser.scrub_preview";
/// `browser.load_selected`: load the selected files onto the selected track
/// (Enter).
pub const LOAD_SELECTED: &str = "browser.load_selected";
/// `clip.save_file`: save a clip as a clip file in a browser folder.
pub const SAVE_CLIP_FILE: &str = "clip.save_file";
/// `browser.rename_file`: give the file whose name is open for editing its
/// new name, or stop editing it.
pub const RENAME_FILE: &str = "browser.rename_file";
/// `browser.add_to_arrangement`: put files on a track, or on a new track.
pub const ADD_TO_ARRANGEMENT: &str = "browser.add_to_arrangement";
/// `browser.tags`: the tag groups and their tags, and what the given items
/// carry.
pub const TAGS: &str = "browser.tags";
/// `browser.tag_items`: put tags on items, or take them off.
pub const TAG_ITEMS: &str = "browser.tag_items";
/// `browser.add_tag`: make a tag in a group, or a subtag under a tag.
pub const ADD_TAG: &str = "browser.add_tag";
/// `browser.add_tag_group`: make a tag group.
pub const ADD_TAG_GROUP: &str = "browser.add_tag_group";
/// `browser.rename_tag`: rename a tag or a group.
pub const RENAME_TAG: &str = "browser.rename_tag";
/// `browser.move_tag`: move a tag or a group.
pub const MOVE_TAG: &str = "browser.move_tag";
/// `browser.reset_tag_order`: put every group and tag back in its first
/// order.
pub const RESET_TAG_ORDER: &str = "browser.reset_tag_order";
/// `browser.delete_tag`: delete a tag or a group.
pub const DELETE_TAG: &str = "browser.delete_tag";
/// `browser.show_tag_editor`: show or hide the Tag Editor.
pub const SHOW_TAG_EDITOR: &str = "browser.show_tag_editor";
/// `browser.show_quick_tags`: show or hide Quick Tags.
pub const SHOW_QUICK_TAGS: &str = "browser.show_quick_tags";
/// `focus.browser`: move the keyboard focus to the browser (Option-5).
pub const FOCUS_BROWSER: &str = "focus.browser";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    STATE,
    SEARCH,
    FOCUS_SEARCH,
    OPEN_PLACE,
    OPEN_FOLDER,
    SELECT,
    SHOW_FILE,
    ADD_FOLDER,
    REMOVE_FOLDER,
    PREVIEW,
    STOP_PREVIEW,
    SET_PREVIEW,
    SET_RAW,
    SCRUB_PREVIEW,
    LOAD_SELECTED,
    ADD_TO_ARRANGEMENT,
    SAVE_CLIP_FILE,
    RENAME_FILE,
    TAGS,
    TAG_ITEMS,
    ADD_TAG,
    ADD_TAG_GROUP,
    RENAME_TAG,
    MOVE_TAG,
    RESET_TAG_ORDER,
    DELETE_TAG,
    SHOW_TAG_EDITOR,
    SHOW_QUICK_TAGS,
    FOCUS_BROWSER,
];

/// Whether `id` is one of the browser's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// How many rows `browser.state` returns when the arguments don't say.
pub const DEFAULT_ROW_LIMIT: usize = 100;

/// The highest preview volume: +6 dB as a linear gain.
pub const MAX_PREVIEW_VOLUME: f64 = 1.995_262_314_968_879_5;

/// A label in the browser's sidebar: a Library label or a place.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A label in the browser's sidebar: `all`, a Library category (`sounds`, \
                   `drums`, `instruments`, `audio_effects`, `midi_effects`, `modulators`, \
                   `plug_ins`, `clips`, `samples`, `grooves`, `tunings`, `templates`), a place \
                   (`user_library`, `current_project`), or a folder added to Places, as \
                   `{\"folder\": \"/its/path\"}`."
)]
pub enum Label {
    /// Everything: the places as folders, searched together.
    All,
    /// Instrument presets.
    Sounds,
    /// Drum kits.
    Drums,
    /// Instruments.
    Instruments,
    /// Audio effects.
    AudioEffects,
    /// MIDI effects.
    MidiEffects,
    /// Modulators.
    Modulators,
    /// Plug-ins.
    PlugIns,
    /// Clip files.
    Clips,
    /// Every audio file in the places.
    Samples,
    /// Grooves.
    Grooves,
    /// Tunings.
    Tunings,
    /// Set templates.
    Templates,
    /// The User Library folder.
    UserLibrary,
    /// The open set's project folder.
    CurrentProject,
    /// A folder the person added to Places, by its path.
    Folder(String),
}

impl Label {
    /// The Library labels, in the sidebar's order (Max for Live is left
    /// out, D21).
    pub const LIBRARY: [Self; 13] = [
        Self::All,
        Self::Sounds,
        Self::Drums,
        Self::Instruments,
        Self::AudioEffects,
        Self::MidiEffects,
        Self::Modulators,
        Self::PlugIns,
        Self::Clips,
        Self::Samples,
        Self::Grooves,
        Self::Tunings,
        Self::Templates,
    ];

    /// The label's name in the sidebar, in Live's words; an added folder's
    /// is the folder's name.
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            Self::All => "All".to_owned(),
            Self::Sounds => "Sounds".to_owned(),
            Self::Drums => "Drums".to_owned(),
            Self::Instruments => "Instruments".to_owned(),
            Self::AudioEffects => "Audio Effects".to_owned(),
            Self::MidiEffects => "MIDI Effects".to_owned(),
            Self::Modulators => "Modulators".to_owned(),
            Self::PlugIns => "Plug-Ins".to_owned(),
            Self::Clips => "Clips".to_owned(),
            Self::Samples => "Samples".to_owned(),
            Self::Grooves => "Grooves".to_owned(),
            Self::Tunings => "Tunings".to_owned(),
            Self::Templates => "Templates".to_owned(),
            Self::UserLibrary => "User Library".to_owned(),
            Self::CurrentProject => "Current Project".to_owned(),
            Self::Folder(path) => std::path::Path::new(path)
                .file_name()
                .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned()),
        }
    }

    /// For a Library label Beatlayer has no content for yet, what will fill
    /// it and when.
    #[must_use]
    pub const fn pending(&self) -> Option<&'static str> {
        match self {
            Self::Sounds => Some(
                "Instrument presets appear here once Beatlayer has instruments (milestone M2); \
                 its own sound library comes in milestone M9.",
            ),
            Self::Grooves => Some("Grooves arrive with the Groove Pool in milestone M6."),
            Self::Tunings => Some("Tunings arrive in milestone M6."),
            _ => None,
        }
    }

    /// Whether it's a place (a folder on disk) rather than a Library label.
    #[must_use]
    pub const fn is_place(&self) -> bool {
        matches!(
            self,
            Self::UserLibrary | Self::CurrentProject | Self::Folder(_)
        )
    }
}

/// A sidebar section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    /// Collections (M6).
    Collections,
    /// The Library labels.
    Library,
    /// Places: folders on disk.
    Places,
}

/// A label as the sidebar lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LabelInfo {
    /// The label.
    pub label: Label,
    /// Its name.
    pub name: String,
    /// Its section.
    pub section: Section,
    /// For a place, its folder; `null` for Current Project while the set
    /// has never been saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether its folder can be read now (a place) or it has content (a
    /// Library label). A missing folder is listed greyed, as in Live.
    pub available: bool,
}

/// What an item in the content pane is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A folder.
    Folder,
    /// An audio file Beatlayer can open.
    Audio,
    /// An SFZ multisample instrument, which imports as a multisampler
    /// (WP-5.14).
    Sfz,
    /// A Beatlayer set.
    Set,
    /// A Beatlayer device preset, which loads into the device being
    /// hot-swapped (WP-2.4).
    Preset,
    /// A plug-in the scanner found (WP-2.9).
    Plugin,
    /// One of Beatlayer's own devices, listed under a device label (the
    /// Modulators label, WP-5.10); its path is the device type's ID.
    Device,
    /// A clip file (WP-6.34); the row's `clip_kind` says whether it holds an
    /// audio clip or a MIDI clip.
    Clip,
    /// A track of a set opened in the browser (WP-6.35); its path is the
    /// set's, `#t` and the track's ID.
    Track,
    /// A group track of an opened set, holding its tracks.
    Group,
    /// A return track of an opened set.
    Return,
    /// The Main track of an opened set.
    Main,
    /// A Session clip of an opened set (`#c` and the clip's ID): it behaves as
    /// a clip file does, and the row's `clip_kind` says whether it holds an
    /// audio clip or a MIDI clip.
    SessionClip,
    /// A track's chain of devices in an opened set (`#d` and the track's ID).
    Devices,
    /// A line in place of an opened set's rows, saying why it can't be read or
    /// that it is being read. Offers nothing to drag.
    Notice,
}

/// What a clip file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClipFileKind {
    /// An audio clip.
    Audio,
    /// A MIDI clip.
    Midi,
}

/// One row of the content pane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Item {
    /// Its path on disk; in the Plug-Ins label, the row's key instead: a
    /// vendor folder's is `plug-ins:/` and the vendor, a plug-in's is its
    /// file, `#`, and its identifier (the plug-in scanning spec, R11).
    pub path: String,
    /// Its name, as the Name column shows it.
    pub name: String,
    /// What it is.
    pub kind: ItemKind,
    /// How deep it's indented: 0 at the top of the list.
    pub depth: u16,
    /// For a folder, whether it's open.
    #[serde(default)]
    pub open: bool,
    /// In search results, the folder it's in, relative to its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// For a plug-in, its format, which the row's badge shows (the plug-in
    /// scanning spec, R11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<bl_model::PluginFormat>,
    /// For a clip file, what it holds; `null` when its first bytes don't say
    /// (a damaged file), which placing refuses (clip files R6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_kind: Option<ClipFileKind>,
}

/// The preview: the switch, the volume, and what plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PreviewState {
    /// Whether the Preview switch is on: selecting an audio file previews
    /// it.
    pub on: bool,
    /// Whether the Raw switch is on: previews play once at their original speed.
    pub raw: bool,
    /// The preview volume as a linear gain: 0 is silence, 1 unity (0 dB),
    /// 2 about +6 dB.
    pub volume: f64,
    /// The file being decoded for a preview, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loading: Option<String>,
    /// The file playing, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playing: Option<String>,
    /// How far it has played, in seconds.
    pub position_seconds: f64,
    /// How long it is, in seconds.
    pub length_seconds: f64,
}

/// The index of the places' files, built in the background.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IndexState {
    /// Files indexed so far.
    pub files: usize,
    /// Folders indexed so far.
    pub folders: usize,
    /// Whether indexing is still going on.
    pub indexing: bool,
}

/// What the browser shows: most browser commands return it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserState {
    /// The chosen sidebar label.
    pub label: Label,
    /// The sidebar's Library labels and places, in order.
    pub labels: Vec<LabelInfo>,
    /// The search field's text; empty when not searching.
    pub search: String,
    /// Rows of the content pane, from `first_row`: the label's contents,
    /// with open folders' contents under them, or the search results.
    pub rows: Vec<Item>,
    /// The index of the first row returned.
    pub first_row: usize,
    /// How many rows the content pane has in all.
    pub total_rows: usize,
    /// What the content pane says when it has no rows, such as which
    /// package fills an empty Library label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The selected items' paths.
    pub selected: Vec<String>,
    /// The file whose name is open for editing, after `clip.save_file` saved
    /// it: `browser.rename_file` gives it its new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renaming: Option<String>,
    /// The preview.
    pub preview: PreviewState,
    /// The index.
    pub index: IndexState,
    /// Whether the Tag Editor is open beside the content pane.
    pub tag_editor: bool,
    /// Whether Quick Tags shows at the content pane's bottom.
    pub quick_tags: bool,
}

/// How a tag sits on the selection (spec: browser-tags R11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TagState {
    /// No selected item carries it.
    None,
    /// Every selected item carries it.
    All,
    /// Some of them carry it.
    Some,
    /// It can't be taken off: Beatlayer gave it, or it comes from a folder.
    Fixed,
}

/// Where a tag on an item comes from (spec: browser-tags R16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TagSource {
    /// The item carries it in its own right.
    Own,
    /// A folder it lies in carries it.
    Folder,
    /// Beatlayer gave it, from what it knows of the item.
    Beatlayer,
}

/// A tag, as `browser.tags` reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TagInfo {
    /// Its own name, without the group's.
    pub name: String,
    /// Its full name: the group's name, its own, and any subtag's, joined
    /// by `|`, as `Format|CLAP`.
    pub full_name: String,
    /// Whether Beatlayer gave it: it can't be renamed or deleted.
    pub builtin: bool,
    /// Where it sits in its group.
    pub position: i32,
    /// Its subtags, one level deep.
    pub subtags: Vec<TagInfo>,
}

/// A tag group, as `browser.tags` reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TagGroupInfo {
    /// Its name.
    pub name: String,
    /// Whether Beatlayer gave it: it can't be renamed or deleted.
    pub builtin: bool,
    /// Its tags, in order.
    pub tags: Vec<TagInfo>,
}

/// A tag on an item, and where it comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ItemTag {
    /// The tag's full name.
    pub tag: String,
    /// Where it comes from.
    pub source: TagSource,
}

/// An item and the tags it carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaggedItem {
    /// The item's key: a file or folder by its path, a plug-in by its
    /// format and identifier, a device or rack by its row's ID.
    pub item: String,
    /// The tags it carries.
    pub tags: Vec<ItemTag>,
}

/// What `browser.tags` reports: the tag groups and their tags, and, for the
/// items named, what each carries.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TagsState {
    /// The tag groups, in order, with their tags.
    pub groups: Vec<TagGroupInfo>,
    /// The items named, with the tags each carries.
    pub items: Vec<TaggedItem>,
}

/// One row of the Tag Editor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TagEditorRow {
    /// What the row is.
    pub kind: TagEditorRowKind,
    /// Its name.
    pub name: String,
    /// Its full name, for a tag: the group's name, its own, and any
    /// subtag's, joined by `|`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    /// How deep it's indented: 0 for a group, 1 for a tag, 2 for a subtag.
    pub depth: u16,
    /// How the tag sits on the selection.
    pub state: TagState,
    /// Whether Beatlayer gave it, so it can't be renamed, deleted or
    /// taken off.
    pub builtin: bool,
    /// Whether it has subtags, so it shows an arrow.
    pub has_subtags: bool,
    /// Whether the selection carries one of its tags, for a folded row.
    pub marked: bool,
}

/// What a row of the Tag Editor is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TagEditorRowKind {
    /// A tag group.
    Group,
    /// A tag, or a subtag.
    Tag,
    /// Add Tag..., at the end of a group.
    AddTag,
    /// Add Group..., at the end of the list.
    AddGroup,
}

/// A tag in Quick Tags.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QuickTag {
    /// Its own name.
    pub name: String,
    /// Its full name: the group's name, its own, and any subtag's, joined
    /// by `|`.
    pub full_name: String,
    /// The group it's in.
    pub group: String,
    /// Whether every selected item carries it. Otherwise it shows an
    /// asterisk.
    pub all: bool,
    /// Whether it can be taken off the selected items: the person's own
    /// tags can, Beatlayer's and a folder's can't.
    pub removable: bool,
}

/// Arguments of `browser.tags`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TagsArgs {
    /// The items to report on, by their keys. Left out, none are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<String>>,
}

/// Arguments of `browser.tag_items`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TagItemsArgs {
    /// The items to tag, by their keys.
    pub items: Vec<String>,
    /// The tags, by their full names or their own names.
    pub tags: Vec<String>,
    /// Put the tags on the items (true) or take them off (false). Left out,
    /// they're put on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<bool>,
}

/// Arguments of `browser.add_tag`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddTagArgs {
    /// The tag's name.
    pub name: String,
    /// The group to make it in, by its name. Left out with `parent` left
    /// out too, the first group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The tag to make it under, by its full name or its own name. It
    /// becomes a subtag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// Arguments of `browser.add_tag_group`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddTagGroupArgs {
    /// The group's name.
    pub name: String,
}

/// Arguments of `browser.rename_tag`. Give `tag` or `group`, not both.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameTagArgs {
    /// The tag to rename, by its full name or its own name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The group to rename, by its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The new name.
    pub name: String,
}

/// Arguments of `browser.move_tag`. Give `tag` or `group`, not both.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveTagArgs {
    /// The tag to move, by its full name or its own name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The group to move, by its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Where it goes: 0 is the first place in its group or the list.
    pub position: i32,
}

/// Arguments of `browser.delete_tag`. Give `tag` or `group`, not both.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteTagArgs {
    /// The tag to delete, by its full name or its own name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The group to delete, by its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// Arguments of `browser.show_tag_editor`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowTagEditorArgs {
    /// Whether it shows. Left out, it toggles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<bool>,
}

/// Arguments of `browser.show_quick_tags`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowQuickTagsArgs {
    /// Whether it shows. Left out, it toggles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<bool>,
}

/// A part of the browser that takes the keyboard focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// The sidebar's labels.
    Sidebar,
    /// The content pane's list.
    #[default]
    Content,
    /// The search field.
    Search,
}

impl Part {
    /// The shortcut context this part gives key presses.
    #[must_use]
    pub const fn context(self) -> bl_commands::Context {
        match self {
            Self::Sidebar => BrowserSidebar,
            Self::Content => Browser,
            Self::Search => BrowserSearch,
        }
    }
}

/// Arguments of `focus.browser`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FocusArgs {
    /// The part to focus: `sidebar`, `content` (the list), or `search`.
    /// Left out, the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<Part>,
}

/// Arguments of `browser.state`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StateArgs {
    /// The first row to return. Left out, 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// How many rows to return, at most. Left out, 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Arguments of `browser.search`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// The words to find. Empty ends the search.
    pub query: String,
    /// Search under this label instead of the chosen one: `all` searches
    /// every place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
}

/// Arguments of `browser.open_place`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenPlaceArgs {
    /// The label to choose.
    pub label: Label,
}

/// Arguments of `browser.open_folder`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenFolderArgs {
    /// The folder, set file or row of an opened set, as a row of the content
    /// pane gives it.
    pub path: String,
    /// Open (true) or close (false). Left out, it toggles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    /// Keep the other open folders open, as holding Cmd (Ctrl) does.
    /// Otherwise opening a folder closes every open folder it isn't inside.
    #[serde(default)]
    pub keep_others: bool,
}

/// Arguments of `browser.show_file`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowFileArgs {
    /// The file to show, by path. Give this or `clip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// An audio clip whose file to show. Give this or `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
}

/// Arguments of `browser.select`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectArgs {
    /// The items to select, by path, as rows of the content pane give them.
    /// Empty clears the selection.
    pub paths: Vec<String>,
    /// Add them to the selection instead of replacing it.
    #[serde(default)]
    pub add: bool,
}

/// Arguments of `browser.add_folder` and `browser.remove_folder`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderArgs {
    /// The folder's path.
    pub path: String,
}

/// Arguments of `browser.preview`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewArgs {
    /// The audio file to preview. Left out, the selected file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Arguments of `browser.set_preview`. Fields left out keep their values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPreviewArgs {
    /// The Preview switch. Left out with `volume` left out too, it toggles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// The preview volume as a linear gain, from 0 (silence) to 2 (about
    /// +6 dB); values outside are brought within it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
}

/// Arguments of `browser.set_raw`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRawArgs {
    /// The Raw switch. Left out, it toggles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<bool>,
}

/// Arguments of `browser.scrub_preview`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrubPreviewArgs {
    /// Where in the preview to scrub to, in seconds from its start. Give
    /// this or `fraction`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
    /// Where in the preview to scrub to, as a fraction from 0 (the start)
    /// to 1 (the end). Give this or `position`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
}

/// Arguments of `browser.add_to_arrangement`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddToArrangementArgs {
    /// The audio files, in the order they go down.
    pub paths: Vec<String>,
    /// The audio track to put them on. Left out (or `null`), a new audio
    /// track below the last track, as dropping in the drop area does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// Where the first clip starts. Left out, the insert marker. It isn't
    /// snapped: snap first if you want the grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Ticks>,
    /// Put each file on a track of its own (as dropping with Cmd or Ctrl
    /// held does), new tracks going below the target, instead of one after
    /// another on one track.
    #[serde(default)]
    pub spread: bool,
}

/// Arguments of `clip.save_file`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveClipFileArgs {
    /// The clip to save: one in the Arrangement or in a Session slot.
    pub clip: ClipId,
    /// The folder to save it in. Left out, the User Library's `Clips`
    /// folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The file's name, without the extension. Left out, the clip's name, or
    /// its track's when the clip has none. A name already taken in the
    /// folder gets " 2", " 3" and so on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Answers Collect Files on Export when it is set to Ask: `true` copies
    /// the audio files the clip and its track's devices play, `false` leaves
    /// the clip file naming them where they are. Under Always and Never it
    /// is ignored; under Ask, leaving it out refuses the command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<bool>,
}

/// What `clip.save_file` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SaveClipFileResult {
    /// The clip file's path.
    pub path: String,
    /// Its name, without the extension.
    pub name: String,
    /// Whether it holds an audio clip or a MIDI clip.
    pub kind: ClipFileKind,
    /// How many audio files were copied beside it.
    pub copied: usize,
}

/// Arguments of `browser.rename_file`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameFileArgs {
    /// The file to rename. Left out, the file whose name is open for
    /// editing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The new name, without the extension. Left out (or `null`), the name
    /// is kept and editing ends, as Esc does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A clip a browser command placed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlacedClip {
    /// The new clip.
    pub clip: ClipId,
    /// Its track.
    pub track: TrackId,
    /// Where it starts.
    pub start: Ticks,
    /// How long it is.
    pub length: Ticks,
    /// Its name: the file's, without the extension.
    pub name: String,
    /// The file it plays.
    pub path: String,
}

/// What `browser.add_to_arrangement` and `browser.load_selected` return.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddResult {
    /// Audio tracks made for the files, top to bottom.
    pub created_tracks: Vec<TrackId>,
    /// The clips placed, in the order of the files.
    pub clips: Vec<PlacedClip>,
}

/// Adds the browser's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs() {
        registry.register(spec)?;
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "one spec per command, written out in full"
)]
fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(STATE, "Browser State")
            .description(
                "Reports what the browser shows: the sidebar's labels and places, the chosen \
                 label, the search text, the content pane's rows (folders and audio files, with \
                 open folders' contents under them, or the search results; `offset` and `limit` \
                 page through them, 100 at a time by default), the selection, the preview, and \
                 how far indexing has got. Changes nothing, but lists files outside the project.",
            )
            .permission(Permission::External)
            .contexts([Global, Browser, BrowserSidebar])
            .args::<StateArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(SEARCH, "Browser Search")
            .description(
                "Searches the browser by file name, as typing in its search field does: an item \
                 matches when every word in `query` appears in its name, ignoring case. Under \
                 `all` and `samples` it searches every place (User Library, Current Project, \
                 added folders; `samples` only audio files), under a place that place. Returns \
                 the browser's state with the results as its rows, in name order; an empty query \
                 ends the search. Changes nothing, but lists files outside the project.",
            )
            .permission(Permission::External)
            .contexts([Global, Browser, BrowserSearch])
            .args::<SearchArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(FOCUS_SEARCH, "Search in Browser")
            .description(
                "Shows the browser, chooses the All label, and puts the keyboard focus in the \
                 search field, as Cmd-F (Ctrl-F) does, so typing searches. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-BRW-006")
            .args::<NoArgs>()
            .result::<()>(),
        CommandSpec::new(OPEN_PLACE, "Open Browser Label")
            .description(
                "Chooses a label in the browser's sidebar and shows its contents: a place's \
                 folders and audio files, the places themselves under `all`, every audio file \
                 under `samples`. Ends any search. Up and Down in the sidebar do this. Changes \
                 nothing, but reads folders outside the project.",
            )
            .permission(Permission::External)
            .contexts([Browser, BrowserSidebar])
            .args::<OpenPlaceArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(OPEN_FOLDER, "Open Folder")
            .description(
                "Opens or closes a folder in the browser's content pane, listing its folders \
                 and audio files indented under it (in the Plug-Ins label, a vendor's folder \
                 and its plug-ins). A set file opens like a folder, read in the background: \
                 its tracks in its order, a group holding its tracks, then its return tracks \
                 and Main, each openable in turn (a track lists its Session clips by scene, and \
                 every track a Devices row); a set that can't be read opens to a line saying \
                 why. A row of an opened set is named by its `path`, the set's path, `#` and \
                 `t`, `d` or `c` with the ID of the track, the track whose chain it is, or the \
                 Session clip; `set.import` takes those IDs. Opening a folder closes the other \
                 open folders it isn't inside, unless `keep_others` (Cmd or Ctrl held). Right \
                 and Left do this on the selected folder. Changes nothing, but reads folders \
                 outside the project.",
            )
            .permission(Permission::External)
            .context(Browser)
            .args::<OpenFolderArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(SELECT, "Select in Browser")
            .description(
                "Selects items in the browser's content pane by path (or, in the Plug-Ins label, \
                 by the row's key), replacing the selection \
                 or, with `add`, adding to it, as clicking, Shift-clicking, and Up and Down do. \
                 While the Preview switch is on, selecting one audio file previews it through \
                 the audio device; selecting anything else stops the preview. Changes nothing in \
                 the set.",
            )
            .permission(Permission::External)
            .context(Browser)
            .args::<SelectArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(SHOW_FILE, "Show in Browser")
            .description(
                "Shows an audio file in the browser, as Show in Browser does in the Sample \
                 Editor: shows the browser, chooses the place the file lies in, opens the \
                 folders down to it and selects it. Name the file by `path`, or by the `clip` \
                 that plays it. Refused if the file lies in none of the browser's places; \
                 `browser.add_folder` adds one. Changes nothing in the set, but reads folders \
                 outside the project.",
            )
            .permission(Permission::External)
            .contexts([Browser, ClipView, SampleEditor])
            .args::<ShowFileArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(ADD_FOLDER, "Add Folder to Browser...")
            .description(
                "Adds the folder at `path` to the browser's Places, after Current Project, \
                 without moving or changing it, and indexes its files in the background so \
                 search finds them. Added folders are kept between launches; adding one twice \
                 changes nothing. A person choosing the menu item or the sidebar's Add \
                 Folder... picks the folder in a dialog.",
            )
            .permission(Permission::External)
            .contexts([Global, BrowserSidebar])
            .args::<FolderArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(REMOVE_FOLDER, "Remove from Sidebar")
            .description(
                "Takes a folder the person added out of the browser's Places, as its context \
                 menu's Remove from Sidebar does. The folder and its files stay on disk. The \
                 User Library and Current Project can't be removed.",
            )
            .permission(Permission::Edit)
            .context(BrowserSidebar)
            .args::<FolderArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(PREVIEW, "Preview")
            .description(
                "Previews an audio file (`path`, or the selected file): plays it once from its \
                 start through the main output, at the preview volume, on a voice of its own, \
                 replacing any preview, whether or not the Preview switch is on, as Shift-Enter \
                 and Right do. The file is decoded in the background, so it starts a moment \
                 later; `browser.state` reports it loading, then playing. Never changes the set. \
                 Plays through the audio device.",
            )
            .permission(Permission::External)
            .context(Browser)
            .args::<PreviewArgs>()
            .result::<PreviewState>(),
        CommandSpec::new(STOP_PREVIEW, "Stop Preview")
            .description("Stops the browser's preview, fading it out. Changes nothing else.")
            .permission(Permission::Read)
            .contexts([Global, Browser])
            .args::<NoArgs>()
            .result::<PreviewState>(),
        CommandSpec::new(SET_PREVIEW, "Browser File Preview")
            .description(
                "Sets the browser's Preview switch (`on`): while it's on, selecting an audio \
                 file previews it. Also sets the preview volume (`volume`, a linear gain from 0, \
                 silence, to 2, about +6 dB; 1 is unity), which is the Main track's Preview/Cue \
                 volume. With no arguments it toggles the switch. Switching it off stops a \
                 preview. Both are kept for the next start.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<SetPreviewArgs>()
            .result::<PreviewState>(),
        CommandSpec::new(SET_RAW, "Raw Preview")
            .description(
                "Sets the browser's Raw switch (`raw`): with Raw on, previews play once from \
                 the start at the file's original tempo; with Raw off, audio files preview at \
                 the set's tempo and loop. With no arguments it toggles the switch. Kept for the \
                 next start.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<SetRawArgs>()
            .result::<PreviewState>(),
        CommandSpec::new(SCRUB_PREVIEW, "Scrub Preview")
            .description(
                "Moves a playing warped preview to `position` (in seconds) or `fraction` (0 to 1). \
                 While the transport runs it moves on the next step of the global launch \
                 quantisation; while stopped it moves at once. With Raw on or on an unwarped \
                 preview, does nothing.",
            )
            .permission(Permission::External)
            .context(Browser)
            .args::<ScrubPreviewArgs>()
            .result::<PreviewState>(),
        CommandSpec::new(LOAD_SELECTED, "Load Selected Item")
            .description(
                "Loads the browser's selected audio files into the Arrangement, as Enter or a \
                 double-click does: onto the selected track at the insert marker if it's an \
                 audio track, otherwise onto a new audio track below the last one, one after \
                 another, as one undo step. A clip placed over part of another replaces that \
                 part, so when a clip on that track ends after the insert marker it's \
                 destructive as well as external. A selected preset file, or a device or \
                 rack listed under a label (Drums, Instruments, Audio Effects, MIDI Effects \
                 or Modulators), is added to the shown track as `device.add_preset` and \
                 `device.add` add it, and a chain preset to the chain list's rack \
                 (`chain.load_preset`). While a device is being hot-swapped \
                 (`device.hot_swap`), a selected preset or device replaces it instead \
                 (`device.replace`), as does a chain preset the chain armed by its Hot-Swap \
                 button (`chain.load_preset`); a selected audio file loads into it if the \
                 device plays samples (the sampler, or the eight-slot drum sampler, into its \
                 selected slot: `device.load_sample`). With one plug-in selected it loads \
                 that plug-in instead, as `plugin.load` does: onto the selected track (a new \
                 track when none is), an instrument onto a new MIDI track when the selected \
                 track holds audio, or in place of the device being hot-swapped. Reads the \
                 files, outside the project.",
            )
            .permission(Permission::External)
            .context(Browser)
            .binding("S-BRW-004")
            .args::<NoArgs>()
            .result::<AddResult>(),
        CommandSpec::new(ADD_TO_ARRANGEMENT, "Add Files to Arrangement")
            .description(
                "Puts audio files into the Arrangement as clips, as dragging them from the \
                 browser does: onto the audio track `track` starting at `at` (or the insert \
                 marker), one after another; or, with no `track`, onto a new audio track below \
                 the last track (the drop area), named after the first file. With `spread`, each \
                 file goes on a track of its own, new tracks below the target. Each clip is named \
                 after its file and lasts as long as the file plays at the song's tempo. A clip \
                 placed over part of another replaces that part: the other is shortened, split \
                 around it, or removed, so a track never plays two clips at once; when a clip on \
                 `track` ends after `at`, it's destructive as well as external. One undo step. \
                 Reads the files, outside the project, and registers them so the set keeps them.",
            )
            .permission(Permission::External)
            .contexts([Arrangement, Browser])
            .args::<AddToArrangementArgs>()
            .result::<AddResult>(),
        CommandSpec::new(SAVE_CLIP_FILE, "Save Clip as File")
            .description(
                "Saves a clip from the Arrangement or a Session slot as a clip file (`.blclip`) \
                 in `folder` (the User Library's `Clips` folder if left out), as dragging the \
                 clip onto a folder in the browser does. The file keeps the clip as the set \
                 saves it (its notes or its warp markers, envelopes and launch settings) and the \
                 devices of its track with their state. It is named `name`, the clip's name, or \
                 its track's when the clip has none, with \" 2\", \" 3\" added for a name already \
                 taken, and written whole or not at all. The audio the clip and the devices play \
                 is copied into the `Samples/Imported` folder of the project the file lands in or \
                 left where it is, as Collect Files on Export says (`settings.state`'s \
                 `library.collect_files_on_export`); under Ask, `copy` answers for the person and \
                 leaving it out refuses the command. The new row is selected with its name open \
                 for editing (`browser.state`'s `renaming`; `browser.rename_file` finishes it). \
                 Changes nothing in the set. Writes files outside the project.",
            )
            .permission(Permission::External)
            .contexts([Arrangement, Browser, Session])
            .args::<SaveClipFileArgs>()
            .result::<SaveClipFileResult>(),
        CommandSpec::new(RENAME_FILE, "Rename Browser File")
            .description(
                "Renames the file whose name is open for editing in the browser (or the file at \
                 `path`) to `name`, keeping its extension, as typing a name and pressing Enter \
                 does after a clip is saved. A name another file in the folder has is refused. \
                 Without a `name` the name is kept and editing ends, as Esc does. Renames a file \
                 outside the project.",
            )
            .permission(Permission::External)
            .context(Browser)
            .args::<RenameFileArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(TAGS, "Browser Tags")
            .description(
                "Reports the browser's tag groups and their tags, in order: Beatlayer's own \
                 groups (Content, Format, Creator and Devices) and the person's. With `items`, \
                 the keys of the items to report on, it also says which tags each carries and \
                 where each comes from: its own, a folder's, or Beatlayer's. Changes nothing, \
                 but lists files outside the project.",
            )
            .permission(Permission::External)
            .contexts([Global, Browser])
            .args::<TagsArgs>()
            .result::<TagsState>(),
        CommandSpec::new(TAG_ITEMS, "Tag Items")
            .description(
                "Puts tags on items in the browser, or takes them off (`attach` false puts them \
                 on). The items are named by their keys: a file or folder by its path, a \
                 plug-in by its format and identifier (`clap/org.example.synth`), a device or \
                 rack by its row's ID. The tags are named by their full names (`Format|CLAP`) \
                 or their own names. Beatlayer's own tags and a tag that comes from a folder \
                 can't be taken off. Changes the browser's database, not the files.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<TagItemsArgs>()
            .result::<TagsState>(),
        CommandSpec::new(ADD_TAG, "Add Browser Tag")
            .description(
                "Makes a tag in the browser: in the group `group`, or under the tag `parent`, \
                 which makes it a subtag. A subtag can't hold another subtag. Changes the \
                 browser's database.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<AddTagArgs>()
            .result::<TagsState>(),
        CommandSpec::new(ADD_TAG_GROUP, "Add Browser Tag Group")
            .description(
                "Makes a tag group in the browser, named `name`, at the end of the list. \
                 Changes the browser's database.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<AddTagGroupArgs>()
            .result::<TagsState>(),
        CommandSpec::new(RENAME_TAG, "Rename Browser Tag")
            .description(
                "Renames a tag or a group in the browser: `tag` for a tag, by its full name \
                 (`Format|CLAP`) or its own name, `group` for a group, by its name; `name` is \
                 the new name. Beatlayer's own tags and groups can't be renamed. Renaming a \
                 tag keeps it on every item that carries it. Changes the browser's database.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<RenameTagArgs>()
            .result::<TagsState>(),
        CommandSpec::new(MOVE_TAG, "Move Browser Tag")
            .description(
                "Moves a tag or a group in the browser to `position`: 0 is the first place in \
                 its group, or the first in the list for a group. Beatlayer's own tags and \
                 groups can't be moved. Changes the browser's database.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<MoveTagArgs>()
            .result::<TagsState>(),
        CommandSpec::new(RESET_TAG_ORDER, "Reset Browser Tag Order")
            .description(
                "Puts every tag group and tag in the browser back in the order it was made \
                 in, as Reset Order in the Tag Editor's context menu does. Changes the \
                 browser's database.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<NoArgs>()
            .result::<TagsState>(),
        CommandSpec::new(DELETE_TAG, "Delete Browser Tag")
            .description(
                "Deletes a tag or a group in the browser: `tag` for a tag, by its full name \
                 (`Format|CLAP`) or its own name, `group` for a group, by its name. Deleting a \
                 group deletes its tags. Beatlayer's own tags and groups can't be deleted. \
                 A deleted tag is gone from every item that carried it, and undo can't bring \
                 it back.",
            )
            .permission(Permission::Destructive)
            .contexts([Global, Browser])
            .args::<DeleteTagArgs>()
            .result::<TagsState>(),
        CommandSpec::new(SHOW_TAG_EDITOR, "Show Tag Editor")
            .description(
                "Shows the Tag Editor beside the browser's content pane, or hides it, as \
                 Cmd-Shift-E (Ctrl-Shift-E) and the Filter View Menu's Tag Editor item do. \
                 Whether it shows is kept for the next start.",
            )
            .permission(Permission::Read)
            .contexts([Global, Browser])
            .binding("S-BRW-014")
            .args::<ShowTagEditorArgs>()
            .result::<()>(),
        CommandSpec::new(SHOW_QUICK_TAGS, "Show Quick Tags")
            .description(
                "Shows Quick Tags at the browser's content pane's bottom, or hides it, as the \
                 Filter View Menu's Show Quick Tags item does. Quick Tags shows the selected \
                 items' tags and adds one by typing its name. Whether it shows is kept for \
                 the next start.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<ShowQuickTagsArgs>()
            .result::<()>(),
        CommandSpec::new(FOCUS_BROWSER, "Browser")
            .description(
                "Moves the keyboard focus to the browser (to `part`: the sidebar, the list, or \
                 the search field; the list if left out), showing the browser if it's hidden, so \
                 its keys apply: Up and Down move through the list, Left and Right open and close \
                 folders, Enter loads. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-FOCUS-006")
            .args::<FocusArgs>()
            .result::<()>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        // Spec: browser R16.
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        for id in IDS {
            assert!(registry.contains(id), "{id}");
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
        let class = |id| registry.get(id).unwrap().permission.unwrap();
        for id in [
            STATE,
            SEARCH,
            OPEN_PLACE,
            OPEN_FOLDER,
            SELECT,
            SHOW_FILE,
            ADD_FOLDER,
            PREVIEW,
            SCRUB_PREVIEW,
            LOAD_SELECTED,
            ADD_TO_ARRANGEMENT,
            SAVE_CLIP_FILE,
            RENAME_FILE,
        ] {
            assert_eq!(class(id), Permission::External, "{id} reads outside");
        }
        for id in [SET_PREVIEW, SET_RAW, REMOVE_FOLDER] {
            assert_eq!(class(id), Permission::Edit, "{id}");
        }
        for id in [
            TAG_ITEMS,
            ADD_TAG,
            ADD_TAG_GROUP,
            RENAME_TAG,
            MOVE_TAG,
            RESET_TAG_ORDER,
            SHOW_QUICK_TAGS,
        ] {
            assert_eq!(class(id), Permission::Edit, "{id}");
        }
        assert_eq!(class(DELETE_TAG), Permission::Destructive);
        assert_eq!(class(TAGS), Permission::External);
        assert_eq!(class(SHOW_TAG_EDITOR), Permission::Read);
        for id in [FOCUS_SEARCH, STOP_PREVIEW, FOCUS_BROWSER] {
            assert_eq!(class(id), Permission::Read, "{id}");
        }
    }

    #[test]
    fn labels_read_as_live_names_them_and_serialise_simply() {
        assert_eq!(Label::LIBRARY.map(|l| l.name())[7], "Plug-Ins");
        assert!(!Label::LIBRARY.iter().any(|l| l.name() == "Max for Live"));
        assert_eq!(
            serde_json::to_value(Label::AudioEffects).unwrap(),
            "audio_effects"
        );
        assert_eq!(
            serde_json::to_value(Label::Folder("/a/Loops".into())).unwrap(),
            serde_json::json!({ "folder": "/a/Loops" })
        );
        assert_eq!(Label::Folder("/a/Loops".into()).name(), "Loops");
        assert!(Label::Sounds.pending().is_some());
        assert!(Label::All.pending().is_none() && Label::Samples.pending().is_none());
        assert!(Label::Clips.pending().is_none(), "clip files are built");
    }
}
