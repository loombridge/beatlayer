//! The browser's commands in the command registry (WP-1.10; decisions D16
//! and D17): places, browsing, search, preview, and adding files to the
//! Arrangement.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them
//! by ID. Commands that read folders or files outside the project, or play
//! through the audio device, are in the external class, so an agent asks the
//! person first. The behaviour they follow is in the browser spec.

use bl_commands::Context::{Arrangement, Browser, BrowserSearch, BrowserSidebar, Global};
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
/// `browser.load_selected`: load the selected files onto the selected track
/// (Enter).
pub const LOAD_SELECTED: &str = "browser.load_selected";
/// `browser.add_to_arrangement`: put files on a track, or on a new track.
pub const ADD_TO_ARRANGEMENT: &str = "browser.add_to_arrangement";
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
    ADD_FOLDER,
    REMOVE_FOLDER,
    PREVIEW,
    STOP_PREVIEW,
    SET_PREVIEW,
    LOAD_SELECTED,
    ADD_TO_ARRANGEMENT,
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
            Self::Drums => Some("Drum kits arrive with the drum racks in milestone M5."),
            Self::Instruments => {
                Some("Beatlayer's instruments arrive in milestone M2 (WP-2.16 and WP-2.17).")
            }
            Self::AudioEffects => {
                Some("Beatlayer's audio effects arrive in milestone M2 (WP-2.18a and WP-2.18b).")
            }
            Self::MidiEffects => Some("MIDI effects arrive in milestone M5."),
            Self::Clips => Some("Clip files arrive in milestone M6."),
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
}

/// The preview: the switch, the volume, and what plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PreviewState {
    /// Whether the Preview switch is on: selecting an audio file previews
    /// it.
    pub on: bool,
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
    /// The preview.
    pub preview: PreviewState,
    /// The index.
    pub index: IndexState,
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
    /// The folder, as a row of the content pane gives it.
    pub path: String,
    /// Open (true) or close (false). Left out, it toggles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    /// Keep the other open folders open, as holding Cmd (Ctrl) does.
    /// Otherwise opening a folder closes every open folder it isn't inside.
    #[serde(default)]
    pub keep_others: bool,
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
                 and its plug-ins). Opening a folder closes the other open \
                 folders it isn't inside, unless `keep_others` (Cmd or Ctrl held). Right and \
                 Left do this on the selected folder. Changes nothing, but reads folders outside \
                 the project.",
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
        CommandSpec::new(LOAD_SELECTED, "Load Selected Item")
            .description(
                "Loads the browser's selected audio files into the Arrangement, as Enter or a \
                 double-click does: onto the selected track at the insert marker if it's an \
                 audio track, otherwise onto a new audio track below the last one, one after \
                 another, as one undo step. A clip placed over part of another replaces that \
                 part, so when a clip on that track ends after the insert marker it's \
                 destructive as well as external. While a device is being hot-swapped \
                 (`device.hot_swap`), a selected preset loads into it instead, and so does a \
                 selected audio file, if the device plays samples (the sampler, or the \
                 eight-slot drum sampler, into its selected slot: `device.load_sample`). \
                 With one plug-in selected it loads that plug-in instead, as `plugin.load` \
                 does: onto the selected track (a new track when none is), an instrument onto a \
                 new MIDI track when the selected track holds audio, or in place of the device \
                 being hot-swapped. Reads the files, outside the project.",
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
            ADD_FOLDER,
            PREVIEW,
            LOAD_SELECTED,
            ADD_TO_ARRANGEMENT,
        ] {
            assert_eq!(class(id), Permission::External, "{id} reads outside");
        }
        for id in [SET_PREVIEW, REMOVE_FOLDER] {
            assert_eq!(class(id), Permission::Edit, "{id}");
        }
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
    }
}
