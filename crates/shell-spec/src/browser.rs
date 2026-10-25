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
use bl_model::{ClipId, TakeLaneId, TrackId};
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
/// `browser.show_file`: show a file or folder in the browser (Show in Browser,
/// Show in Places).
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
/// `browser.set_filter_view`: show or hide the Filter View (Cmd-Option-G).
pub const SET_FILTER_VIEW: &str = "browser.set_filter_view";
/// `browser.set_filter_groups`: which filter groups a label shows.
pub const SET_FILTER_GROUPS: &str = "browser.set_filter_groups";
/// `browser.filter`: select filter tags.
pub const FILTER: &str = "browser.filter";
/// `browser.clear_filters`: clear the selected tags and the search words.
pub const CLEAR_FILTERS: &str = "browser.clear_filters";
/// `browser.add_label`: keep what the browser shows as a saved label.
pub const ADD_LABEL: &str = "browser.add_label";
/// `browser.rename_label`: rename a label, or open its name for editing.
pub const RENAME_LABEL: &str = "browser.rename_label";
/// `browser.set_label_icon`: give a label an icon, or take it away.
pub const SET_LABEL_ICON: &str = "browser.set_label_icon";
/// `browser.set_label_shown`: show or hide a Library label in the sidebar.
pub const SET_LABEL_SHOWN: &str = "browser.set_label_shown";
/// `browser.move_label`: move a Library or saved label within its section.
pub const MOVE_LABEL: &str = "browser.move_label";
/// `browser.reset_label_order`: put the Library labels back in their order.
pub const RESET_LABEL_ORDER: &str = "browser.reset_label_order";
/// `browser.remove_label`: delete a saved label.
pub const REMOVE_LABEL: &str = "browser.remove_label";
/// `browser.set_columns`: the content pane's columns, sort and file
/// extensions.
pub const SET_COLUMNS: &str = "browser.set_columns";
/// `browser.set_collection`: put items in a collection, or take them out.
pub const SET_COLLECTION: &str = "browser.set_collection";
/// `browser.clear_collections`: take items out of every collection.
pub const CLEAR_COLLECTIONS: &str = "browser.clear_collections";
/// `browser.reveal`: show items in the system's file manager.
pub const REVEAL: &str = "browser.reveal";
/// `browser.back`: go back one step in the browser's history.
pub const BACK: &str = "browser.back";
/// `browser.forward`: go forward one step in the browser's history.
pub const FORWARD: &str = "browser.forward";
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
    SET_FILTER_VIEW,
    SET_FILTER_GROUPS,
    FILTER,
    CLEAR_FILTERS,
    ADD_LABEL,
    RENAME_LABEL,
    SET_LABEL_ICON,
    SET_LABEL_SHOWN,
    MOVE_LABEL,
    RESET_LABEL_ORDER,
    REMOVE_LABEL,
    SET_COLUMNS,
    SET_COLLECTION,
    CLEAR_COLLECTIONS,
    REVEAL,
    BACK,
    FORWARD,
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
                   `{\"folder\": \"/its/path\"}`, or a saved label, as `{\"saved\": 3}`, or a \
                   collection, 1 (Favorites) to 7 (Gray), as `{\"collection\": 3}`."
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
    /// A saved label, by its ID (spec: browser-tags R24).
    Saved(i64),
    /// A collection, by its number: 1 is Favorites, 7 Gray (spec:
    /// browser-tags R34).
    Collection(u8),
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
            Self::Saved(_) => "Saved Label".to_owned(),
            Self::Collection(number) => COLLECTIONS
                .get(usize::from(*number).wrapping_sub(1))
                .map_or_else(|| "Collection".to_owned(), |name| (*name).to_owned()),
        }
    }

    /// The key the browser keeps the label's settings under: the order of
    /// the labels, the ones hidden, the names and icons, and the filter
    /// groups each shows.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Sounds => "sounds".to_owned(),
            Self::Drums => "drums".to_owned(),
            Self::Instruments => "instruments".to_owned(),
            Self::AudioEffects => "audio_effects".to_owned(),
            Self::MidiEffects => "midi_effects".to_owned(),
            Self::Modulators => "modulators".to_owned(),
            Self::PlugIns => "plug_ins".to_owned(),
            Self::Clips => "clips".to_owned(),
            Self::Samples => "samples".to_owned(),
            Self::Grooves => "grooves".to_owned(),
            Self::Tunings => "tunings".to_owned(),
            Self::Templates => "templates".to_owned(),
            Self::UserLibrary => "user_library".to_owned(),
            Self::CurrentProject => "current_project".to_owned(),
            Self::Folder(path) => format!("folder:{path}"),
            Self::Saved(id) => format!("saved:{id}"),
            Self::Collection(number) => format!("collection:{number}"),
        }
    }

    /// Whether it's in the Library section: a Library label or a saved
    /// label.
    #[must_use]
    pub const fn is_library(&self) -> bool {
        !matches!(
            self,
            Self::UserLibrary | Self::CurrentProject | Self::Folder(_) | Self::Collection(_)
        )
    }

    /// Whether it's one of the seven collections.
    #[must_use]
    pub const fn is_collection(&self) -> bool {
        matches!(self, Self::Collection(number) if *number >= 1 && *number <= 7)
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
    /// The seven collections (spec: browser-tags R34).
    Collections,
    /// The Library labels.
    Library,
    /// Places: folders on disk.
    Places,
}

/// The seven collections' names, in the sidebar's order (spec: browser R2,
/// browser-tags R34).
pub const COLLECTIONS: [&str; 7] = [
    "Favorites",
    "Orange",
    "Yellow",
    "Green",
    "Blue",
    "Purple",
    "Gray",
];

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
    /// The icon the person gave it, from [`LABEL_ICONS`] (spec:
    /// browser-tags R25).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Whether the sidebar shows it: the person can hide Library labels and
    /// saved labels (spec: browser-tags R26).
    #[serde(default = "shown_by_default")]
    pub shown: bool,
}

fn shown_by_default() -> bool {
    true
}

/// The icons a label can be given: Beatlayer's own set (spec: browser-tags
/// R25).
pub const LABEL_ICONS: [&str; 10] = [
    "star", "heart", "circle", "square", "triangle", "diamond", "bolt", "note", "wave", "cross",
];

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
    /// A Stacks chord bank (`.blstacks`), which loads into Stacks (WP-6.14b).
    ChordBank,
    /// A groove file (`.blgroove`), which loads into the Groove Pool, or
    /// replaces the groove being hot-swapped (WP-6.20; the grooves spec R32,
    /// R33).
    Groove,
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
    /// What the optional columns show for it, left out while none shows
    /// (spec: browser-tags R27).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ItemDetails>,
    /// The collections it is in, by their numbers (1 is Favorites), in
    /// order; left out while it is in none (spec: browser-tags R35, R40).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<u8>,
}

/// What an item's optional columns show (spec: browser-tags R27); a value
/// the item doesn't have, such as a folder's length, is left out.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ItemDetails {
    /// When it changed, in seconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<i64>,
    /// Its size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// What it is, in words: `Folder`, `Audio File`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// For an audio file, how long it plays, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_seconds: Option<f64>,
    /// For an audio file, its sample rate in hertz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// For an audio file, its number of channels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
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
    /// Why the file selected last doesn't preview, if it doesn't: a MIDI clip
    /// file whose chain has no instrument, or a clip file whose audio is
    /// missing. The preview strip shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
    /// The Filter View: whether it shows, its groups under the chosen label,
    /// and the tags selected (spec: browser-tags R18 to R22).
    pub filter_view: FilterViewState,
    /// The saved labels, in the order they were made (spec: browser-tags
    /// R24).
    #[serde(default)]
    pub saved_labels: Vec<SavedLabelInfo>,
    /// The label whose name is open for editing, after `browser.add_label`
    /// or `browser.rename_label` without a name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renaming_label: Option<Label>,
    /// The content pane's columns and sort (spec: browser-tags R27, R28).
    pub columns: ColumnsState,
    /// Whether Browse Back can go back a step (spec: browser-tags R30).
    pub can_back: bool,
    /// Whether Browse Forward can go forward a step.
    pub can_forward: bool,
}

/// A column of the content pane (spec: browser-tags R27). Name always shows,
/// first.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    /// The item's name.
    Name,
    /// When it changed.
    DateModified,
    /// Its size.
    Size,
    /// What it is.
    Kind,
    /// An audio file's length.
    Length,
    /// An audio file's sample rate.
    SampleRate,
    /// An audio file's channels.
    Channels,
}

impl Column {
    /// The columns the Content Options menu lists, in its order.
    pub const OPTIONAL: [Self; 6] = [
        Self::DateModified,
        Self::Size,
        Self::Kind,
        Self::Length,
        Self::SampleRate,
        Self::Channels,
    ];

    /// The column's name in the header row and the menu.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::DateModified => "Date Modified",
            Self::Size => "Size",
            Self::Kind => "Kind",
            Self::Length => "Length",
            Self::SampleRate => "Sample Rate",
            Self::Channels => "Channels",
        }
    }
}

/// The content pane's columns, sort and file extensions (spec: browser-tags
/// R27, R28).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColumnsState {
    /// The optional columns shown, in their order, after Name.
    pub shown: Vec<Column>,
    /// The column the list is sorted by.
    pub sort: Column,
    /// Whether the order is reversed.
    pub descending: bool,
    /// Whether names show their extensions.
    pub file_extensions: bool,
}

impl Default for ColumnsState {
    fn default() -> Self {
        Self {
            shown: Vec::new(),
            sort: Column::Name,
            descending: false,
            file_extensions: true,
        }
    }
}

/// A tag button in the Filter View.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilterTag {
    /// What the button says: the tag's name, a subtag's with its tag's.
    pub name: String,
    /// The tag's full name.
    pub full_name: String,
    /// Whether the tag is selected.
    pub selected: bool,
}

/// A filter group as the Filter View shows it: its tags that some item
/// under the label carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilterGroup {
    /// The group's name.
    pub name: String,
    /// Its tag buttons, in the Tag Editor's order.
    pub tags: Vec<FilterTag>,
}

/// A group the chosen label can show, as the Filter View Menu lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilterGroupChoice {
    /// The group's name.
    pub name: String,
    /// Whether the label shows it.
    pub shown: bool,
}

/// The Filter View (spec: browser-tags R18 to R22).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilterViewState {
    /// Whether the Filter View shows.
    pub shown: bool,
    /// The groups it shows under the chosen label that have tags to show.
    pub groups: Vec<FilterGroup>,
    /// Every group the chosen label can show, with whether it shows.
    pub menu: Vec<FilterGroupChoice>,
    /// The selected tags' full names.
    pub selected: Vec<String>,
}

/// A saved label (spec: browser-tags R24).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SavedLabelInfo {
    /// The label, `{"saved": id}`.
    pub label: Label,
    /// Its name.
    pub name: String,
    /// The icon it was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The label it was made under.
    pub base: Label,
    /// The search words it keeps.
    pub search: String,
    /// The tags it keeps, by their full names.
    pub tags: Vec<String>,
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

/// Arguments of `browser.set_collection`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetCollectionArgs {
    /// The collection: 1 is Favorites, 2 Orange, 3 Yellow, 4 Green, 5 Blue,
    /// 6 Purple, 7 Gray.
    pub collection: u8,
    /// The items, by their keys as `browser.tag_items` names them. Left out,
    /// the selected items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<String>>,
    /// Put them in the collection (true) or take them out (false). Left
    /// out, as the keys 1 to 7 do: if every item is in it already they all
    /// come out, otherwise they all go in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<bool>,
}

/// Arguments of `browser.clear_collections`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearCollectionsArgs {
    /// The items, by their keys as `browser.tag_items` names them. Left out,
    /// the selected items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<String>>,
}

/// Arguments of `browser.reveal`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevealArgs {
    /// The files and folders to show, by path. Left out, the selected items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
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
    /// Delete although saved labels use the tag or the group's tags: they
    /// then leave it out. Otherwise the command is refused, naming them
    /// (spec: browser-tags R8).
    #[serde(default)]
    pub confirm: bool,
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

/// Arguments of `browser.set_filter_view`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetFilterViewArgs {
    /// Whether the Filter View shows. Left out, it toggles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<bool>,
}

/// Arguments of `browser.set_filter_groups`. Give `group`, `show_all` or
/// `reset`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetFilterGroupsArgs {
    /// The label whose groups change. Left out, the chosen label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
    /// The group to show or hide, by its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Show (true) or hide (false) `group`. Left out, true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<bool>,
    /// Show every group.
    #[serde(default)]
    pub show_all: bool,
    /// Put the label's groups back to its default ones.
    #[serde(default)]
    pub reset: bool,
}

/// Arguments of `browser.filter`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilterArgs {
    /// The tags, by their full names or their own names.
    pub tags: Vec<String>,
    /// Add the tags to their groups' selections, or take a selected one out,
    /// as Cmd-click (Ctrl-click) does. Otherwise each tag is the only one
    /// selected in its group, and a tag that is already the only one is
    /// cleared.
    #[serde(default)]
    pub add: bool,
}

/// Arguments of `browser.clear_filters`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearFiltersArgs {
    /// Clear the search words. Left out, true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<bool>,
    /// Clear the selected tags. Left out, true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<bool>,
}

/// Arguments of `browser.rename_label`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameLabelArgs {
    /// The label to rename. Left out, the chosen label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
    /// The new name; empty gives a Library label its own name back. Left
    /// out, the label's name opens for editing, or, if it's open already,
    /// editing ends and the name is kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Arguments of `browser.set_label_icon`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLabelIconArgs {
    /// The label to give an icon.
    pub label: Label,
    /// One of [`LABEL_ICONS`]. Left out (or `null`), Reset Icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Arguments of `browser.set_label_shown`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLabelShownArgs {
    /// The Library label, saved label or collection.
    pub label: Label,
    /// Whether the sidebar shows it.
    pub shown: bool,
}

/// Arguments of `browser.move_label`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveLabelArgs {
    /// The Library label or saved label to move.
    pub label: Label,
    /// Where it goes in the Library section: 0 is the first place, which
    /// All keeps, so 1 is the first place anything else can take.
    pub position: usize,
}

/// Arguments of `browser.remove_label`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveLabelArgs {
    /// The saved label to delete.
    pub label: Label,
}

/// Arguments of `browser.set_columns`. Fields left out keep their values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetColumnsArgs {
    /// The optional columns shown, in their order after Name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<Vec<Column>>,
    /// The column to sort by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Column>,
    /// Reverse the order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descending: Option<bool>,
    /// Show file extensions in names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_extensions: Option<bool>,
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
    /// The Groove Pool under the list (the grooves spec R24): its keys are its
    /// own, so no browser key applies.
    GroovePool,
}

impl Part {
    /// The shortcut context this part gives key presses: the global one for
    /// the Groove Pool, whose keys the pool handles itself.
    #[must_use]
    pub const fn context(self) -> bl_commands::Context {
        match self {
            Self::Sidebar => BrowserSidebar,
            Self::Content => Browser,
            Self::Search => BrowserSearch,
            Self::GroovePool => Global,
        }
    }
}

/// Arguments of `focus.browser`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FocusArgs {
    /// The part to focus: `sidebar`, `content` (the list), `search`, or
    /// `groove_pool`. Left out, the list.
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
    /// The file or folder to show, by path. Give this or `clip`.
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
    /// A take lane of `track` to put them on in place of its main lane (the
    /// comping spec, R36): the clips go one after another on that lane, as
    /// takes. With `spread`, the first goes on the lane and each of the others
    /// on the take lane at the same place among the take lanes of the next
    /// track below, in turn, a lane being added where a track has none there.
    /// `track` may be left out; given, it must be the lane's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<TakeLaneId>,
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
    /// The take lane it is on, if it is on one rather than a main lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<TakeLaneId>,
}

/// What `browser.add_to_arrangement` and `browser.load_selected` return.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddResult {
    /// Audio tracks made for the files, top to bottom.
    pub created_tracks: Vec<TrackId>,
    /// The clips placed, in the order of the files.
    pub clips: Vec<PlacedClip>,
    /// Take lanes added to tracks that had none at the place a file was
    /// dropped on (the comping spec, R36).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_lanes: Vec<TakeLaneId>,
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
                "Shows a file or folder in the browser, as Show in Browser does in the Sample \
                 Editor and Show in Places does in a browser item's context menu: shows the \
                 browser, chooses the place it lies in, opens the folders down to it and \
                 selects it. Name it by `path`, or an audio file by the `clip` that plays it. Refused if the file lies in none of the browser's places; \
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
                 file goes on a track of its own, new tracks below the target. With `lane`, a \
                 take lane of the track, they go on that lane as takes instead, and with \
                 `spread` the others go on the take lane at the same place of the tracks below, \
                 which must be audio tracks (a lane is added where one has none there). Each \
                 clip is named \
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
                 it back. When saved labels use the tag (or the group's tags) the command is \
                 refused, naming them, unless `confirm` is true; the labels then leave it \
                 out.",
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
        CommandSpec::new(SET_FILTER_VIEW, "Show Filter View")
            .description(
                "Shows or hides the Filter View, the tag groups at the top of the browser's \
                 content pane, as Cmd-Option-G (Ctrl-Alt-G) and View > Show Filter View do. It \
                 shows in a new install. Left out, `show` toggles. Kept for the next start. \
                 Read the Filter View in `browser.state`'s `filter_view`.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .binding("S-BRW-013")
            .args::<SetFilterViewArgs>()
            .result::<FilterViewState>(),
        CommandSpec::new(SET_FILTER_GROUPS, "Set Filter Groups")
            .description(
                "Chooses which filter groups a label shows in the Filter View (`label`, or the \
                 chosen one): `group` with `shown` true (the default) or false shows or hides \
                 that group; `show_all` shows every group; `reset` puts the label's groups \
                 back to the default ones. Give one of `group`, `show_all` and `reset`. Kept \
                 for the next start.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<SetFilterGroupsArgs>()
            .result::<FilterViewState>(),
        CommandSpec::new(FILTER, "Filter Browser")
            .description(
                "Selects filter tags, as clicking tag buttons in the Filter View does. Without \
                 `add`, each tag in `tags` becomes the only one selected in its group (a tag \
                 that is already the only one is cleared). With `add`, each tag is added to \
                 its group's selection, or taken out if it is in it already. The list then \
                 holds the items under the label that carry, for each group with a selection, \
                 at least one of its selected tags, and match every search word. Selected tags \
                 stay when another Library label is chosen. Changes nothing, but lists files \
                 outside the project.",
            )
            .permission(Permission::External)
            .contexts([Global, Browser])
            .args::<FilterArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(CLEAR_FILTERS, "Clear Browser Filters")
            .description(
                "Clears the selected filter tags and the search words (`words` and `tags`, both \
                 true if left out), as the Results bar's Clear button and Esc in the search \
                 field do. Changes nothing, but lists files outside the project.",
            )
            .permission(Permission::External)
            .contexts([Global, Browser, BrowserSearch])
            .args::<ClearFiltersArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(ADD_LABEL, "Add Label")
            .description(
                "Keeps what the browser shows as a saved label, as the Results bar's Add Label \
                 button does: the label it was made under, the search words and the selected \
                 tags. It goes at the end of the Library section, named after its words and \
                 tags, with its name open for editing (`browser.state`'s `renaming_label`); \
                 `browser.rename_label` finishes it. Choosing it lists what those words and \
                 tags find under that label now. Refused when there are no search words and \
                 no selected tags. Kept in the browser's database.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<NoArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(RENAME_LABEL, "Rename Label")
            .description(
                "Renames a saved label, a Library label or a collection, as Rename (Cmd-R) in its \
                 context menu does (`label`, or the chosen label). With `name`, the label has \
                 that name (empty gives a Library label or a collection its own name back). Without it, the name opens for editing, or, if it is open \
                 already, editing ends and the name is kept. A place's label (the User Library, \
                 Current Project, an added folder) takes its folder's name and isn't renamed. \
                 Kept for the next start.",
            )
            .permission(Permission::Edit)
            .contexts([BrowserSidebar])
            .binding("S-EDIT-008")
            .args::<RenameLabelArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(SET_LABEL_ICON, "Choose Label Icon")
            .description(
                "Gives a label one of Beatlayer's icons (`icon`: star, heart, circle, square, \
                 triangle, diamond, bolt, note, wave or cross), or takes it away (Reset Icon) \
                 when `icon` is left out. All, User Library and Current Project keep theirs. \
                 Kept for the next start.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser, BrowserSidebar])
            .args::<SetLabelIconArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(SET_LABEL_SHOWN, "Show Label in Sidebar")
            .description(
                "Shows or hides a Library label, a saved label or a collection in the sidebar, \
                 as the check boxes in the Library and Collections sections' Edit mode do. All \
                 can't be hidden. Hiding the chosen label chooses All. Kept for the next start.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser, BrowserSidebar])
            .args::<SetLabelShownArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(SET_COLLECTION, "Set Collection")
            .description(
                "Puts items in one of the seven collections (`collection`: 1 Favorites, 2 \
                 Orange, 3 Yellow, 4 Green, 5 Blue, 6 Purple, 7 Gray), or takes them out, as the \
                 keys 1 to 7 and the items of a browser item's context menu do. The items are \
                 named by their keys as `browser.tag_items` names them (`items`; left out, the \
                 selected items). With `member` true they go in, with false they come out; left \
                 out, they all come out if every one is in the collection already, otherwise \
                 they all go in. An item can be in several collections. A hidden collection \
                 comes back into the sidebar when an item goes in. Changes the browser's \
                 database, not the files, and isn't an undo step of the set.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<SetCollectionArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(CLEAR_COLLECTIONS, "Clear All Colors")
            .description(
                "Takes items out of every collection, as the key 0 and Clear All Colors in a \
                 browser item's context menu do. The items are named by their keys as \
                 `browser.tag_items` names them (`items`; left out, the selected items). \
                 Changes the browser's database, not the files.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<ClearCollectionsArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(REVEAL, "Show in Finder")
            .description(
                "Shows files and folders in the system's file manager (Finder on macOS, File \
                 Explorer on Windows, the default file manager on Linux), as Show in Finder in \
                 a browser item's context menu does. Name them by `paths`; left out, the \
                 selected items, which must be files or folders. Opens another application and \
                 changes nothing in the set.",
            )
            .permission(Permission::External)
            .contexts([Global, Browser])
            .args::<RevealArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(MOVE_LABEL, "Move Label")
            .description(
                "Moves a Library label or a saved label to `position` within the Library \
                 section, as dragging it does; All stays first, so 1 is the earliest place \
                 another label can take. Kept for the next start.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser, BrowserSidebar])
            .args::<MoveLabelArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(RESET_LABEL_ORDER, "Reset Label Order")
            .description(
                "Puts the Library labels back in their first order, the saved labels after \
                 them, as Reset Label Order in the Library heading's context menu does. Hidden \
                 labels stay hidden.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser, BrowserSidebar])
            .args::<NoArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(REMOVE_LABEL, "Remove Label")
            .description(
                "Deletes a saved label, as Remove in its context menu does. Nothing else is \
                 deleted: not the files, the tags or the search words it kept. It can't be \
                 brought back.",
            )
            .permission(Permission::Destructive)
            .contexts([Global, Browser, BrowserSidebar])
            .args::<RemoveLabelArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(SET_COLUMNS, "Set Browser Columns")
            .description(
                "Sets the content pane's columns: `shown`, the optional columns after Name \
                 (date_modified, size, kind, length, sample_rate, channels) in their order; \
                 `sort`, the column the list is sorted by, with `descending` to reverse it \
                 (folders stay first; ties go by name); and `file_extensions`, whether names \
                 show their extensions. Fields left out keep their values. Kept for the next \
                 start.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Browser])
            .args::<SetColumnsArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(BACK, "Browser History Back")
            .description(
                "Goes back one step in the browser's history, as Browse Back and Cmd-[ (Ctrl-[) \
                 do, showing that step's label, search words, selected tags, open folders and \
                 selection. A step is taken when the label changes, when the selected tags \
                 change, and when a search starts or its words change (changes in a row make \
                 one step); the history holds 100 steps and isn't kept between launches. \
                 Refused at the first step (`browser.state`'s `can_back`).",
            )
            .permission(Permission::External)
            .contexts([Global, Browser])
            .binding("S-BRW-011")
            .args::<NoArgs>()
            .result::<BrowserState>(),
        CommandSpec::new(FORWARD, "Browser History Forward")
            .description(
                "Goes forward one step in the browser's history, as Browse Forward and Cmd-] \
                 (Ctrl-]) do. Refused at the last step (`browser.state`'s `can_forward`).",
            )
            .permission(Permission::External)
            .contexts([Global, Browser])
            .binding("S-BRW-012")
            .args::<NoArgs>()
            .result::<BrowserState>(),
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
            REVEAL,
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
            SET_FILTER_VIEW,
            SET_FILTER_GROUPS,
            ADD_LABEL,
            RENAME_LABEL,
            SET_LABEL_ICON,
            SET_LABEL_SHOWN,
            MOVE_LABEL,
            RESET_LABEL_ORDER,
            SET_COLUMNS,
            SET_COLLECTION,
            CLEAR_COLLECTIONS,
        ] {
            assert_eq!(class(id), Permission::Edit, "{id}");
        }
        // Spec: browser-tags R32. These change what the browser lists.
        for id in [FILTER, CLEAR_FILTERS, BACK, FORWARD] {
            assert_eq!(class(id), Permission::External, "{id} lists files outside");
        }
        for id in [DELETE_TAG, REMOVE_LABEL] {
            assert_eq!(class(id), Permission::Destructive, "{id}");
        }
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
        // Spec: browser-tags R34.
        assert_eq!(
            serde_json::to_value(Label::Collection(3)).unwrap(),
            serde_json::json!({ "collection": 3 })
        );
        assert_eq!(
            COLLECTIONS.to_vec(),
            (1..=7)
                .map(|n| Label::Collection(n).name())
                .collect::<Vec<_>>()
        );
        assert!(Label::Collection(1).is_collection() && !Label::Collection(1).is_library());
        assert!(!Label::Collection(8).is_collection());
    }
}
