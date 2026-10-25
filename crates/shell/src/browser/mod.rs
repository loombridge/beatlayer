//! The browser without a window (WP-1.10): the sidebar's labels and places,
//! the content pane's rows, the search, the selection, the preview, and the
//! commands that change them. The UI (`bl-ui`) draws from it; people,
//! agents, and tests change it through the commands in
//! `bl_shell_spec::browser`, which [`BrowserView::run`] carries out.
//!
//! - [`listing`]: which files a folder shows, and in what order.
//! - [`index`]: the background index that search reads.
//! - [`db`]: the browser's database on disk: the index, the tags, and the
//!   items that carry them.
//! - [`tags`]: the tag groups, the tags in them, and the items that carry
//!   them.
//! - [`filter`]: the Filter View's groups and the tags selected in them.
//! - [`labels`]: saved labels, and the Library labels' order, names and
//!   icons.
//! - [`columns`]: the content pane's columns and sort.
//! - [`history`]: the steps Browse Back and Browse Forward go through.
//! - [`places`]: the User Library's place and starting folders.
//! - [`preview`]: decoding previews in the background.
//! - [`arrange`]: putting files into the Arrangement.
//! - [`drop`]: where a drag from the browser lands in the Arrangement.
//!
//! The behaviour is in the browser spec.

pub mod arrange;
pub mod collections;
pub mod columns;
pub mod db;
pub mod drop;
pub mod filter;
pub mod hash;
pub mod history;
pub mod index;
pub mod labels;
pub mod listing;
pub mod places;
pub mod preview;
pub mod tags;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use bl_core::time::Ticks;
use bl_media::MediaStore;
use bl_model::{Editor, TrackId};
pub use bl_shell_spec::browser::Part;
use bl_shell_spec::browser::{
    self as ids, AddTagArgs, AddTagGroupArgs, AddToArrangementArgs, BrowserState,
    ClearCollectionsArgs, ClearFiltersArgs, ClipFileKind, ColumnsState, DeleteTagArgs, FilterArgs,
    FocusArgs, FolderArgs, IndexState, Item, ItemKind, ItemTag, Label, LabelInfo,
    MAX_PREVIEW_VOLUME, MoveLabelArgs, MoveTagArgs, OpenFolderArgs, OpenPlaceArgs, PreviewArgs,
    PreviewState, QuickTag, RemoveLabelArgs, RenameFileArgs, RenameLabelArgs, RenameTagArgs,
    RevealArgs, SaveClipFileArgs, SaveClipFileResult, ScrubPreviewArgs, SearchArgs, Section,
    SelectArgs, SetCollectionArgs, SetColumnsArgs, SetFilterGroupsArgs, SetFilterViewArgs,
    SetLabelIconArgs, SetLabelShownArgs, SetPreviewArgs, SetRawArgs, ShowFileArgs,
    ShowQuickTagsArgs, ShowTagEditorArgs, StateArgs, TagEditorRow, TagEditorRowKind, TagGroupInfo,
    TagInfo, TagItemsArgs, TagSource, TagState, TaggedItem, TagsArgs, TagsState,
};
use bl_shell_spec::commands::{BROWSER_WIDTH, clamp_browser_width};
use bl_shell_spec::settings::CollectFiles;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::arrange::{AddError, MediaFile};
use self::collections::CollectionStore;
use self::columns::DetailsCache;
use self::db::{Db, FileOp, FileRow};
use self::filter::FilterPrefs;
use self::hash::{Hashed, Hasher};
use self::history::{History, Step};
use self::index::{Index, Scope, Snapshot};
use self::labels::{LabelPrefs, SavedLabel};
use self::listing::{Entry, file_kind};
use self::preview::{Loader, MediaSamples};
use self::tags::{Source, TagStore};
use crate::audio::AudioHost;
use crate::clip_files;
use crate::set_exchange::{self, Opened};
use bl_plugin_host::scan::ScannedPlugin;

/// Wakes the app's event loop when background work (indexing, decoding a
/// preview) has news. The host sets what it does; until then it does
/// nothing.
#[derive(Clone, Default)]
pub struct Waker(Arc<RwLock<Option<Wake>>>);

/// What waking the app does.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

impl Waker {
    /// Makes waking call `wake`.
    pub fn set(&self, wake: Wake) {
        if let Ok(mut slot) = self.0.write() {
            *slot = Some(wake);
        }
    }

    /// Wakes the app, if it asked to be.
    pub fn wake(&self) {
        let wake = self.0.read().ok().and_then(|slot| slot.clone());
        if let Some(wake) = wake {
            wake();
        }
    }
}

impl std::fmt::Debug for Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Waker")
    }
}

/// What the browser keeps between launches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserPrefs {
    /// Where the User Library is, if not in its default place.
    pub user_library: Option<PathBuf>,
    /// The folders added to Places, in order.
    pub folders: Vec<PathBuf>,
    /// The Preview switch.
    pub preview: bool,
    /// The preview volume, a linear gain.
    pub preview_volume: f64,
    /// The Raw switch in the preview strip (WP-4.18; spec: browser R18).
    pub raw: bool,
    /// Whether the Tag Editor is open beside the content pane (spec:
    /// browser-tags R10).
    pub tag_editor: bool,
    /// Whether Quick Tags shows at the content pane's bottom (spec:
    /// browser-tags R13). On in a new install.
    pub quick_tags: bool,
    /// The Filter View and the groups each label shows (spec: browser-tags
    /// R18, R21).
    pub filters: FilterPrefs,
    /// The Library labels' order, visibility, names and icons (spec:
    /// browser-tags R25, R26).
    pub labels: LabelPrefs,
    /// The content pane's columns, sort and file extensions (spec:
    /// browser-tags R27, R28).
    pub columns: ColumnsState,
    /// Whether the browser runs down to the Status Bar (Full-Height
    /// Browser; spec: browser R26).
    pub full_height: bool,
    /// The browser's width in points at 100%, as its right edge was last
    /// dragged (spec: browser R29).
    pub width: f32,
}

impl Default for BrowserPrefs {
    fn default() -> Self {
        Self {
            user_library: None,
            folders: Vec::new(),
            preview: true,
            preview_volume: 1.0,
            raw: false,
            tag_editor: false,
            quick_tags: true,
            filters: FilterPrefs::default(),
            labels: LabelPrefs::default(),
            columns: ColumnsState::default(),
            full_height: false,
            width: BROWSER_WIDTH,
        }
    }
}

/// Why a browser command didn't run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserError {
    /// The arguments don't fit.
    InvalidArgs(String),
    /// It can't run now, for the reason given.
    Unavailable(String),
    /// It ran and failed.
    Failed(String),
}

/// What the rest of the shell needs to do after a browser command.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The command's result.
    pub value: Value,
    /// Files added to the set, for its media table.
    pub media: Vec<MediaFile>,
    /// Whether the set changed.
    pub document_changed: bool,
    /// Whether the browser's preferences changed.
    pub prefs_changed: bool,
    /// Whether the browser should show, if it's hidden.
    pub show_browser: bool,
    /// Whether the keyboard focus moves to the browser.
    pub focus: bool,
    /// What the Status Bar says, if anything.
    pub message: Option<String>,
    /// Files and folders to show in the system's file manager (spec: browser
    /// R25).
    pub file_manager: Vec<PathBuf>,
}

/// What a browser command works with besides the browser.
pub struct Context<'a> {
    /// The set.
    pub editor: &'a mut Editor,
    /// The audio device, for previews.
    pub audio: &'a mut AudioHost,
    /// The media store, where added files are registered.
    pub media: &'a MediaStore,
    /// The Arrangement's insert marker, where loaded files go.
    pub insert_marker: Ticks,
    /// The Arrangement's first selected track, where loaded files go.
    pub selected_track: Option<TrackId>,
    /// Whether Create Fades on Clip Edges is on, so new clips get 4 ms fades
    /// (the audio clips spec, R7).
    pub edge_fades: bool,
    /// What a new clip's warp settings are made from (the warping spec,
    /// R33).
    pub warping: crate::auto_warp::Warping<'a>,
    /// Collect Files on Export: what saving a clip does with its audio
    /// (file management R7).
    pub collect_files: bl_shell_spec::settings::CollectFiles,
    /// Folders to search for the audio a clip file names when it isn't where
    /// the file says: the set's project folder, the User Library and the
    /// places (clip files R12).
    pub search: &'a [PathBuf],
    /// The set's tempo, in BPM, at the playhead: what a MIDI clip file's
    /// preview is rendered at (clip files R16).
    pub set_bpm: f64,
    /// The colour settings, for the tracks and clips added (the themes spec,
    /// R19, R20).
    pub look: &'a bl_shell_spec::settings::ThemeLook,
}

/// What [`BrowserView::poll`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Polled {
    /// Whether anything the browser shows changed.
    pub changed: bool,
    /// A preview that couldn't be decoded, for the Status Bar.
    pub error: Option<String>,
}

/// A decoded preview waiting for analysis (WP-4.18; spec: browser R21).
#[derive(Clone, Debug)]
struct PendingPreview {
    path: PathBuf,
    hash: bl_media::ContentHash,
    data: bl_media::SampleData,
    seconds: f64,
}

/// A preview the audio thread was given.
#[derive(Clone, Debug)]
struct Playing {
    generation: u64,
    path: PathBuf,
    /// Whether the audio thread has reported playing it.
    started: bool,
    warp_map: Option<bl_model::WarpMap>,
    length_seconds: f64,
}

/// The content pane's rows.
#[derive(Debug)]
enum Rows {
    /// A folder listing, or the places under All.
    Items(Vec<Item>),
    /// Items from the index: search results, or every sample.
    Hits {
        snapshot: Arc<Snapshot>,
        hits: Vec<u32>,
    },
}

/// The browser's state. See the [module docs](self).
#[derive(Debug)]
pub struct BrowserView {
    user_library: Option<PathBuf>,
    project: Option<PathBuf>,
    folders: Vec<PathBuf>,
    label: Label,
    search: String,
    open: BTreeSet<PathBuf>,
    listings: HashMap<PathBuf, Vec<Entry>>,
    rows: Rows,
    note: Option<String>,
    selected: Vec<PathBuf>,
    cursor: Option<usize>,
    index: Index,
    indexing: bool,
    loader: Loader,
    preview_on: bool,
    raw: bool,
    preview_volume: f64,
    loading: Option<(u64, PathBuf)>,
    pending_warp: Option<PendingPreview>,
    playing: Option<Playing>,
    /// Why the file selected last doesn't preview (clip files R13, R16).
    preview_note: Option<String>,
    preview_rate: u32,
    /// The waveform overview of the file previewed last, for the preview
    /// strip.
    overview: Option<(PathBuf, Arc<[[f32; 2]]>)>,
    focus: Part,
    focus_requests: u64,
    waker: Waker,
    /// The plug-ins the scanner found (WP-2.9), already filtered by the
    /// hidden and format preferences; the Plug-Ins label lists them.
    plugins: Vec<ScannedPlugin>,
    /// The tag groups, the tags in them, and the items that carry them
    /// (spec: browser-tags R6 to R9).
    tags: TagStore,
    /// The browser's database on disk, when there is one (spec:
    /// browser-tags R1).
    db: Option<Db>,
    /// The index as the database holds it, so only the files that changed
    /// are written again.
    saved_files: HashMap<String, FileRow>,
    /// The files the database holds, before the walk brings them up to
    /// date.
    db_files: Vec<FileRow>,
    /// The content hashes of the files that carry a tag, so a file moved
    /// outside Beatlayer keeps them (spec: browser-tags R5).
    tagged_files: HashMap<String, TaggedFile>,
    /// The files whose hash has been asked for and hasn't come back yet.
    pending_hashes: HashSet<String>,
    /// The thread that hashes files.
    hasher: Hasher,
    /// Whether the Tag Editor is open.
    tag_editor: bool,
    /// Whether Quick Tags shows.
    quick_tags: bool,
    /// What the Status Bar says, if anything.
    message: Option<String>,
    /// The file whose name is open for editing after `clip.save_file` saved
    /// it (clip files R4).
    renaming: Option<PathBuf>,
    /// The tags selected in the Filter View (spec: browser-tags R20).
    filter: BTreeSet<i64>,
    /// The Filter View and the groups each label shows.
    filter_prefs: FilterPrefs,
    /// The collections each item is in (spec: browser-tags R35).
    collections: CollectionStore,
    /// The saved labels (spec: browser-tags R24).
    saved: Vec<SavedLabel>,
    /// The Library labels' order, visibility, names and icons.
    label_prefs: LabelPrefs,
    /// The label whose name is open for editing.
    renaming_label: Option<Label>,
    /// The content pane's columns, sort and file extensions.
    columns: ColumnsState,
    /// The steps Browse Back and Browse Forward go through (spec:
    /// browser-tags R29).
    history: History,
    /// A number that goes up when what the labels hold may have changed,
    /// so what was worked out about them is worked out again.
    generation: u64,
    /// The tags some item under a label carries, worked out for one
    /// generation and label.
    available: Mutex<Option<AvailableTags>>,
    /// What was read from disk about files for the columns.
    details: Mutex<DetailsCache>,
    /// Whether the browser runs down to the Status Bar.
    full_height: bool,
    /// The browser's width in points at 100%.
    width: f32,
    /// The sets opened in the content pane, by their files (set exchange R1).
    sets: HashMap<PathBuf, SetState>,
    /// Sets read on background threads send what they read here.
    set_reads: (Sender<SetRead>, Receiver<SetRead>),
}

/// The tags some item under a label carries, with the generation and label
/// they were worked out for.
type AvailableTags = ((u64, String), BTreeSet<i64>);

/// A set opened in the content pane (set exchange R1).
#[derive(Clone, Debug)]
enum SetState {
    /// Being read on a background thread.
    Reading,
    /// Read: its rows are listed.
    Ready(Arc<Opened>),
    /// Refused: why.
    Refused(String),
}

/// A set read on a background thread: its file, and what it held or why not.
type SetRead = (PathBuf, Result<Arc<Opened>, String>);

/// A tagged file's size and content hash, so the tags pass to the same
/// content found at a new path (spec: browser-tags R5).
#[derive(Clone, Debug, PartialEq, Eq)]
struct TaggedFile {
    /// Its length in bytes.
    size: u64,
    /// Its BLAKE3 hash, as hex.
    hash: String,
}

/// A device the browser lists under a Library label (spec: racks R43,
/// modulation R12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserDevice {
    /// The row's ID, in place of a path: a native device's type ID, or
    /// `rack:` and the rack kind.
    pub id: String,
    /// The name the row shows.
    pub name: String,
    /// What it is.
    pub kind: bl_model::DeviceKind,
    /// Where it goes in a chain.
    pub role: bl_model::DeviceRole,
    /// Whether it has a sidechain input.
    pub sidechain: bool,
    /// Whether it has a MIDI sidechain input.
    pub midi_sidechain: bool,
}

/// The devices a label lists: under Modulators, Beatlayer's modulator
/// devices; under Drums, Instruments, Audio Effects and MIDI Effects, an
/// empty rack of the label's kind and the native devices of its role.
fn label_devices(label: &Label) -> Vec<BrowserDevice> {
    use bl_model::{DeviceKind, DeviceRole, RackKind};
    let (rack, role) = match label {
        Label::Modulators => (None, None),
        Label::Drums => (Some(RackKind::Drum), None),
        Label::Instruments => (Some(RackKind::Instrument), Some(DeviceRole::Instrument)),
        Label::AudioEffects => (Some(RackKind::AudioEffect), Some(DeviceRole::AudioEffect)),
        Label::MidiEffects => (Some(RackKind::MidiEffect), Some(DeviceRole::MidiEffect)),
        _ => return Vec::new(),
    };
    let mut devices: Vec<BrowserDevice> = rack
        .map(|kind| BrowserDevice {
            id: format!("rack:{}", rack_id(kind)),
            name: kind.name().to_owned(),
            kind: DeviceKind::Rack { kind },
            role: kind.role(),
            sidechain: false,
            midi_sidechain: false,
        })
        .into_iter()
        .collect();
    devices.extend(bl_devices::device_types().into_iter().filter_map(|t| {
        let modulator = bl_model::modulator::is_modulator_kind(&t.kind);
        let listed = if *label == Label::Modulators {
            modulator
        } else {
            !modulator && Some(t.role) == role
        };
        match t.kind {
            DeviceKind::Native { device_type } if listed => Some(BrowserDevice {
                id: device_type.clone(),
                name: t.name,
                kind: DeviceKind::Native { device_type },
                // A modulator goes in a chain as an audio effect does.
                role: if modulator {
                    DeviceRole::AudioEffect
                } else {
                    t.role
                },
                sidechain: t.sidechain,
                midi_sidechain: t.midi_sidechain,
            }),
            _ => None,
        }
    }));
    devices
}

/// What a clip file's row says it holds: nothing for any other row.
fn clip_kind_of(kind: ItemKind, path: &Path) -> Option<bl_shell_spec::browser::ClipFileKind> {
    (kind == ItemKind::Clip)
        .then(|| listing::clip_kind(path))
        .flatten()
}

fn rack_id(kind: bl_model::RackKind) -> &'static str {
    match kind {
        bl_model::RackKind::Instrument => "instrument",
        bl_model::RackKind::AudioEffect => "audio_effect",
        bl_model::RackKind::MidiEffect => "midi_effect",
        bl_model::RackKind::Drum => "drum",
    }
}

/// The device a row ID names, under any label that lists devices.
#[must_use]
pub fn device_by_id(id: &str) -> Option<BrowserDevice> {
    [
        Label::Modulators,
        Label::Drums,
        Label::Instruments,
        Label::AudioEffects,
        Label::MidiEffects,
    ]
    .iter()
    .flat_map(label_devices)
    .find(|device| device.id == id)
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, BrowserError> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| BrowserError::InvalidArgs(e.to_string()))
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn absolute(path: &str) -> Result<PathBuf, BrowserError> {
    if path.is_empty() {
        return Err(BrowserError::InvalidArgs("the path is empty".to_owned()));
    }
    std::path::absolute(path).map_err(|e| BrowserError::InvalidArgs(format!("{path}: {e}")))
}

/// The start of a Plug-Ins vendor folder's key.
pub const PLUGIN_FOLDER: &str = "plug-ins:/";

/// A plug-in's key in the Plug-Ins label: its file, `#`, and its
/// identifier, so two plug-ins in one file are told apart.
#[must_use]
pub fn plugin_key(plugin: &ScannedPlugin) -> String {
    format!("{}#{}", plugin.path.to_string_lossy(), plugin.plugin_id)
}

/// A vendor folder's key in the Plug-Ins label.
fn vendor_key(vendor: &str) -> String {
    format!("{PLUGIN_FOLDER}{vendor}")
}

/// The vendor a plug-in is filed under, if it names one.
fn vendor_of(plugin: &ScannedPlugin) -> Option<&str> {
    plugin
        .vendor
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

impl BrowserView {
    /// A browser with these preferences, showing the All label, waking
    /// `waker` when background work has news.
    #[must_use]
    pub fn new(prefs: &BrowserPrefs, waker: Waker) -> Self {
        let mut view = Self {
            user_library: None,
            project: None,
            folders: prefs.folders.clone(),
            label: Label::All,
            search: String::new(),
            open: BTreeSet::new(),
            listings: HashMap::new(),
            rows: Rows::Items(Vec::new()),
            note: None,
            selected: Vec::new(),
            cursor: None,
            index: Index::new(waker.clone()),
            indexing: false,
            loader: Loader::new(waker.clone()),
            preview_on: prefs.preview,
            raw: prefs.raw,
            preview_volume: prefs.preview_volume.clamp(0.0, MAX_PREVIEW_VOLUME),
            loading: None,
            pending_warp: None,
            playing: None,
            preview_note: None,
            preview_rate: 48_000,
            overview: None,
            focus: Part::Content,
            focus_requests: 0,
            waker,
            plugins: Vec::new(),
            tags: TagStore::new(),
            db: None,
            saved_files: HashMap::new(),
            db_files: Vec::new(),
            tagged_files: HashMap::new(),
            pending_hashes: HashSet::new(),
            hasher: Hasher::new(),
            tag_editor: prefs.tag_editor,
            quick_tags: prefs.quick_tags,
            message: None,
            renaming: None,
            filter: BTreeSet::new(),
            filter_prefs: prefs.filters.clone(),
            collections: CollectionStore::default(),
            saved: Vec::new(),
            label_prefs: prefs.labels.clone(),
            renaming_label: None,
            columns: prefs.columns.clone(),
            history: History::new(Step {
                label: Label::All,
                search: String::new(),
                tags: BTreeSet::new(),
                open: BTreeSet::new(),
                selected: Vec::new(),
            }),
            generation: 0,
            available: Mutex::new(None),
            details: Mutex::new(DetailsCache::default()),
            full_height: prefs.full_height,
            width: clamp_browser_width(prefs.width),
            sets: HashMap::new(),
            set_reads: channel(),
        };
        view.user_library.clone_from(&prefs.user_library);
        view.places_changed();
        view
    }

    /// The waker background work uses.
    #[must_use]
    pub fn waker(&self) -> &Waker {
        &self.waker
    }

    /// Sets the plug-ins the Plug-Ins label lists (WP-2.9). The shell
    /// passes what the scanner found, already filtered by the hidden and
    /// format preferences.
    pub fn set_plugins(&mut self, plugins: Vec<ScannedPlugin>) {
        self.plugins = plugins;
        self.generation += 1;
        // Beatlayer gives each plug-in's vendor a tag in the Creator group
        // (spec: browser-tags R7).
        let mut ops = Vec::new();
        if let Some(creator) = self.tags.group_id("Creator") {
            let mut vendors: Vec<String> = self
                .plugins
                .iter()
                .filter_map(vendor_of)
                .map(str::to_owned)
                .collect();
            vendors.sort_by_key(|v| v.to_lowercase());
            vendors.dedup();
            for vendor in vendors {
                if !self.tags.find(&format!("Creator|{vendor}")).is_empty() {
                    continue;
                }
                if let Ok((_, tag_ops)) = self.tags.add_builtin_tag(&vendor, creator) {
                    ops.extend(tag_ops);
                }
            }
        }
        self.save_tags(ops);
        if self.label == Label::PlugIns {
            self.rebuild();
        }
    }

    /// Opens the browser's database (spec: browser-tags R1): the index of
    /// the places' files, the tags, and the items that carry them. What it
    /// holds is there from the first frame; the background walk brings it
    /// up to date afterwards.
    pub fn open_database(&mut self, path: PathBuf) {
        let db = Db::open(path);
        let loaded = db.load();
        if let Some(damaged) = db.damaged() {
            self.message = Some(damaged.to_owned());
        }
        // The file is open before anything is written to it.
        self.db = Some(db);
        // The tags the file holds, and the ones Beatlayer gives.
        let ops = self.tags.load(loaded.groups, loaded.tags, loaded.item_tags);
        // The files the file holds, so search finds them from the first
        // frame.
        self.db_files = loaded.files.clone();
        // The saved labels the file holds (spec: browser-tags R24).
        self.saved = loaded.saved_labels.clone();
        // The collections the file holds (spec: browser-tags R35).
        self.collections.load(loaded.item_collections);
        // The files that carried a tag keep their hash, so a file moved
        // outside Beatlayer finds them again (spec: browser-tags R5).
        for row in &loaded.files {
            if let Some(hash) = &row.hash {
                self.tagged_files.insert(
                    row.path.clone(),
                    TaggedFile {
                        size: row.size,
                        hash: hash.clone(),
                    },
                );
            }
        }
        self.saved_files = loaded
            .files
            .iter()
            .map(|row| (row.path.clone(), row.clone()))
            .collect();
        self.adopt_index();
        self.save_tags(ops);
    }

    /// Takes the index the database holds as the browser's, so search finds
    /// what was indexed before from the first frame (spec: browser-tags
    /// R1). The background walk replaces it when it finishes.
    fn adopt_index(&mut self) {
        if self.db_files.is_empty() {
            return;
        }
        let roots: Vec<PathBuf> = self.roots().into_iter().map(|(_, path)| path).collect();
        if roots.is_empty() {
            return;
        }
        let snapshot = Snapshot::from_rows(roots, &self.db_files);
        self.index.adopt(snapshot);
    }

    /// Writes everything that's waiting for the database, and waits for it,
    /// for tests and for the app's last moments.
    pub fn flush_database(&self) {
        if let Some(db) = &self.db {
            db.flush();
        }
    }

    /// The content hash of a file that carries a tag, once it's known
    /// (spec: browser-tags R5).
    #[must_use]
    pub fn tagged_file_hash(&self, key: &str) -> Option<&str> {
        self.tagged_files
            .get(key)
            .map(|tagged| tagged.hash.as_str())
            .filter(|hash| !hash.is_empty())
    }

    /// The message the Status Bar shows, if the browser has one (spec:
    /// browser-tags R3).
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Takes the message the Status Bar should show.
    pub fn take_message(&mut self) -> Option<String> {
        self.message.take()
    }

    /// Reads the tags the database holds and adds the ones this store
    /// doesn't know, so a second Beatlayer on the same configuration folder
    /// is known (spec: browser-tags R3).
    fn reload_tags(&mut self) {
        let Some(db) = &self.db else {
            return;
        };
        let (groups, tags, _) = db.load_tags();
        self.tags.merge(groups, tags);
        self.generation += 1;
    }

    /// Writes the changes to the tags to the database.
    fn save_tags(&mut self, ops: Vec<db::TagOp>) {
        if ops.is_empty() {
            return;
        }
        self.generation += 1;
        if let Some(db) = &self.db {
            db.save_tags(ops);
        }
    }

    /// The key an item's tags are kept under (spec: browser-tags R5): a
    /// file or folder by its path, a plug-in by its format and identifier,
    /// and a device or rack by its row's ID.
    fn tag_key(&self, row: &str) -> String {
        if let Some(plugin) = self.plugin(Path::new(row)) {
            return plugin.key();
        }
        row.to_owned()
    }

    /// The selected items' keys.
    fn selected_keys(&self) -> Vec<String> {
        self.selected
            .iter()
            .map(|path| self.tag_key(&path.to_string_lossy()))
            .collect()
    }

    /// The tag Beatlayer gives a plug-in's format.
    fn format_tag(format: bl_model::PluginFormat) -> &'static str {
        match format {
            bl_model::PluginFormat::Clap => "CLAP",
            bl_model::PluginFormat::Vst3 => "VST3",
            bl_model::PluginFormat::AudioUnit => "AUv2",
            bl_model::PluginFormat::Lv2 => "LV2",
        }
    }

    /// The tag Beatlayer gives what goes in a chain (spec: browser-tags
    /// R7).
    fn role_tag(role: bl_model::DeviceRole) -> &'static str {
        match role {
            bl_model::DeviceRole::Instrument => "Instrument",
            bl_model::DeviceRole::AudioEffect => "Audio Effect",
            bl_model::DeviceRole::MidiEffect => "MIDI Effect",
        }
    }

    /// The tags Beatlayer gives an item, from what it knows of it (spec:
    /// browser-tags R7).
    fn builtin_tags(&self, key: &str) -> Vec<i64> {
        let mut names: Vec<String> = Vec::new();
        if let Some(plugin) = self.plugins.iter().find(|p| p.key() == key) {
            names.push("Content|Plug-In".to_owned());
            names.push(format!("Format|{}", Self::format_tag(plugin.format)));
            if let Some(vendor) = vendor_of(plugin) {
                names.push(format!("Creator|{vendor}"));
            }
            names.push(format!("Devices|{}", Self::role_tag(plugin.role)));
        } else if let Some(device) = device_by_id(key) {
            names.push("Content|Device".to_owned());
            names.push(format!("Devices|{}", Self::role_tag(device.role)));
        } else {
            let path = Path::new(key);
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                return Vec::new();
            };
            match file_kind(&name) {
                Some(ItemKind::Audio) => names.push("Content|Sample".to_owned()),
                Some(ItemKind::Clip) => match listing::clip_kind(path) {
                    Some(bl_shell_spec::browser::ClipFileKind::Audio) => {
                        names.push("Content|Audio Clip".to_owned());
                    }
                    Some(bl_shell_spec::browser::ClipFileKind::Midi) => {
                        names.push("Content|MIDI Clip".to_owned());
                    }
                    None => {}
                },
                Some(ItemKind::Preset) => names.push("Content|Preset".to_owned()),
                Some(ItemKind::Set) => {
                    if self.is_backup_set(path) {
                        names.push("Content|Backup Set".to_owned());
                    } else {
                        names.push("Content|Set".to_owned());
                    }
                }
                _ => {}
            }
        }
        names
            .iter()
            .filter_map(|name| self.tags.find(name).first().copied())
            .collect()
    }

    /// Whether a set file is in a project's Backup folder, which makes it a
    /// backup set and not a set (spec: browser-tags R7).
    fn is_backup_set(&self, path: &Path) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        if parent.file_name().map(|n| n.to_string_lossy()) != Some("Backup".into()) {
            return false;
        }
        let Some(grandparent) = parent.parent() else {
            return false;
        };
        self.roots()
            .iter()
            .any(|(_, root)| root.as_path() == grandparent)
    }

    /// The tags an item carries, and where each comes from (spec:
    /// browser-tags R9 and R16).
    fn carried_tags(&self, key: &str) -> Vec<(i64, Source)> {
        let mut carried: Vec<(i64, Source)> = Vec::new();
        let push = |id: i64, source: Source, carried: &mut Vec<(i64, Source)>| {
            if !carried.iter().any(|(other, _)| *other == id) {
                carried.push((id, source));
            }
        };
        for id in self.tags.item_tags(key) {
            push(id, Source::Own, &mut carried);
        }
        // A folder's tags count for what's inside it, at any depth.
        let path = Path::new(key);
        let mut folder = path.parent();
        let mut depth = 0;
        while let Some(dir) = folder {
            for id in self.tags.item_tags(&dir.to_string_lossy()) {
                push(id, Source::Folder, &mut carried);
            }
            folder = dir.parent();
            depth += 1;
            if depth > 32 {
                break;
            }
        }
        for id in self.builtin_tags(key) {
            push(id, Source::Beatlayer, &mut carried);
        }
        carried
    }

    /// Whether an item carries every one of these tags (spec: browser-tags
    /// R15).
    fn carries_all(&self, key: &str, tags: &[i64]) -> bool {
        if tags.is_empty() {
            return true;
        }
        let carried = self.carried_tags(key);
        tags.iter()
            .all(|tag| carried.iter().any(|(id, _)| id == tag))
    }

    /// How a tag sits on the selection (spec: browser-tags R11).
    fn tag_state(&self, tag: i64, keys: &[String]) -> TagState {
        if keys.is_empty() {
            return TagState::None;
        }
        let mut own = 0;
        let mut fixed = 0;
        for key in keys {
            match self
                .carried_tags(key)
                .iter()
                .find(|(id, _)| *id == tag)
                .map(|(_, source)| *source)
            {
                Some(Source::Own) => own += 1,
                Some(_) => fixed += 1,
                None => {}
            }
        }
        if own + fixed == 0 {
            TagState::None
        } else if own + fixed == keys.len() {
            if fixed > 0 {
                TagState::Fixed
            } else {
                TagState::All
            }
        } else {
            TagState::Some
        }
    }

    /// Whether the Tag Editor is open beside the content pane (spec:
    /// browser-tags R10).
    #[must_use]
    pub fn tag_editor(&self) -> bool {
        self.tag_editor
    }

    /// Whether Quick Tags shows at the content pane's bottom (spec:
    /// browser-tags R13).
    #[must_use]
    pub fn quick_tags_shown(&self) -> bool {
        self.quick_tags
    }

    /// The tag groups' names, in order.
    #[must_use]
    pub fn tag_groups(&self) -> Vec<String> {
        self.tags
            .groups()
            .into_iter()
            .map(|group| group.name.clone())
            .collect()
    }

    /// The tags' full names, in order.
    #[must_use]
    pub fn tag_names(&self) -> Vec<String> {
        self.tags
            .all_tags()
            .into_iter()
            .map(|tag| self.tags.full_name(tag.id))
            .collect()
    }

    /// The tags and tag groups, and what the named items carry (spec:
    /// browser-tags R16).
    fn tags_state(&self, items: &[String]) -> TagsState {
        let groups = self
            .tags
            .groups()
            .into_iter()
            .map(|group| TagGroupInfo {
                name: group.name.clone(),
                builtin: group.builtin,
                tags: self
                    .tags
                    .tags_in(group.id)
                    .into_iter()
                    .map(|tag| self.tag_info(tag.id))
                    .collect(),
            })
            .collect();
        let items = items
            .iter()
            .map(|item| {
                let key = self.tag_key(item);
                TaggedItem {
                    item: key.clone(),
                    tags: self
                        .carried_tags(&key)
                        .into_iter()
                        .map(|(id, source)| ItemTag {
                            tag: self.tags.full_name(id),
                            source: match source {
                                Source::Own => TagSource::Own,
                                Source::Folder => TagSource::Folder,
                                Source::Beatlayer => TagSource::Beatlayer,
                            },
                        })
                        .collect(),
                }
            })
            .collect();
        TagsState { groups, items }
    }

    /// A tag, as `browser.tags` reports it.
    fn tag_info(&self, id: i64) -> TagInfo {
        let Some(tag) = self.tags.tag(id) else {
            return TagInfo {
                name: String::new(),
                full_name: String::new(),
                builtin: false,
                position: 0,
                subtags: Vec::new(),
            };
        };
        TagInfo {
            name: tag.name.clone(),
            full_name: self.tags.full_name(id),
            builtin: tag.builtin,
            position: tag.position,
            subtags: self
                .tags
                .subtags(id)
                .into_iter()
                .map(|sub| self.tag_info(sub.id))
                .collect(),
        }
    }

    /// The Tag Editor's rows, in the order it shows them (spec:
    /// browser-tags R11 and R12). The list holds every row; which are on
    /// screen is the UI's business, so it builds only those.
    pub fn tag_editor_rows(&self) -> Vec<TagEditorRow> {
        let keys = self.selected_keys();
        let mut rows = Vec::new();
        for group in self.tags.groups() {
            let mut marked = false;
            for tag in self.tags.tags_in(group.id) {
                if self.tag_state(tag.id, &keys) != TagState::None {
                    marked = true;
                }
                for sub in self.tags.subtags(tag.id) {
                    if self.tag_state(sub.id, &keys) != TagState::None {
                        marked = true;
                    }
                }
            }
            rows.push(TagEditorRow {
                kind: TagEditorRowKind::Group,
                name: group.name.clone(),
                full_name: None,
                depth: 0,
                state: TagState::None,
                builtin: group.builtin,
                has_subtags: !self.tags.tags_in(group.id).is_empty(),
                marked,
            });
            for tag in self.tags.tags_in(group.id) {
                self.tag_editor_row(&mut rows, tag.id, 1, &keys);
            }
            rows.push(TagEditorRow {
                kind: TagEditorRowKind::AddTag,
                name: "Add Tag…".to_owned(),
                full_name: None,
                depth: 1,
                state: TagState::None,
                builtin: false,
                has_subtags: false,
                marked: false,
            });
        }
        rows.push(TagEditorRow {
            kind: TagEditorRowKind::AddGroup,
            name: "Add Group…".to_owned(),
            full_name: None,
            depth: 0,
            state: TagState::None,
            builtin: false,
            has_subtags: false,
            marked: false,
        });
        rows
    }

    /// A tag's row in the Tag Editor, and its subtags' rows.
    fn tag_editor_row(&self, rows: &mut Vec<TagEditorRow>, id: i64, depth: u16, keys: &[String]) {
        let Some(tag) = self.tags.tag(id) else {
            return;
        };
        let subtags = self.tags.subtags(id);
        let state = self.tag_state(id, keys);
        let marked = state != TagState::None
            || subtags
                .iter()
                .any(|sub| self.tag_state(sub.id, keys) != TagState::None);
        rows.push(TagEditorRow {
            kind: TagEditorRowKind::Tag,
            name: tag.name.clone(),
            full_name: Some(self.tags.full_name(id)),
            depth,
            state,
            builtin: tag.builtin,
            has_subtags: !subtags.is_empty(),
            marked,
        });
        for sub in subtags {
            self.tag_editor_row(rows, sub.id, depth + 1, keys);
        }
    }

    /// Quick Tags' rows: the tags the selected items carry (spec:
    /// browser-tags R13).
    pub fn quick_tags(&self) -> Vec<QuickTag> {
        let keys = self.selected_keys();
        let mut tags: Vec<(i64, bool, bool)> = Vec::new();
        for key in &keys {
            for (id, source) in self.carried_tags(key) {
                match tags.iter_mut().find(|(other, _, _)| *other == id) {
                    Some((_, all, removable)) => {
                        *all = *all && source == Source::Own;
                        *removable = *removable && source == Source::Own;
                    }
                    None => tags.push((id, source == Source::Own, source == Source::Own)),
                }
            }
        }
        let mut tags: Vec<QuickTag> = tags
            .into_iter()
            .filter_map(|(id, all, removable)| {
                let tag = self.tags.tag(id)?;
                let group = self.tags.group(tag.group)?;
                Some(QuickTag {
                    name: tag.name.clone(),
                    full_name: self.tags.full_name(id),
                    group: group.name.clone(),
                    all,
                    removable,
                })
            })
            .collect();
        // In alphabetical order, as Live shows them.
        tags.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.full_name.cmp(&b.full_name))
        });
        tags
    }

    /// The tags whose names hold what's typed, by full name, those searched
    /// recently first (spec: browser-tags R14 and R15).
    pub fn tag_suggestions(&self, text: &str) -> Vec<String> {
        let text = text.trim().to_lowercase();
        let recent = self.tags.recent();
        let mut found: Vec<(usize, usize, String)> = Vec::new();
        for (index, tag) in self.tags.all_tags().into_iter().enumerate() {
            let full = self.tags.full_name(tag.id);
            if !text.is_empty() && !full.to_lowercase().contains(&text) {
                continue;
            }
            let rank = recent
                .iter()
                .position(|&id| id == tag.id)
                .unwrap_or(recent.len());
            found.push((rank, index, full));
        }
        found.sort_by_key(|(rank, index, _)| (*rank, *index));
        found.into_iter().map(|(_, _, full)| full).collect()
    }

    /// The tags the search field's `#` terms name, and the terms that name
    /// no tag (spec: browser-tags R15).
    fn search_terms(&self) -> (Vec<String>, Vec<i64>, Vec<String>) {
        let mut words = Vec::new();
        let mut tags = Vec::new();
        let mut missing = Vec::new();
        for word in self.search.split_whitespace() {
            let Some(name) = word.strip_prefix('#') else {
                words.push(word.to_lowercase());
                continue;
            };
            let found = self.tags.find(name);
            if found.is_empty() {
                missing.push(name.to_owned());
                words.push(name.to_lowercase());
            } else {
                for tag in found {
                    if !tags.contains(&tag) {
                        tags.push(tag);
                    }
                }
            }
        }
        (words, tags, missing)
    }

    /// What the Status Bar says about the search field's `#` terms (spec:
    /// browser-tags R15).
    #[must_use]
    pub fn search_message(&self) -> Option<String> {
        let (_, _, missing) = self.search_terms();
        let first = missing.first()?;
        Some(format!(
            "No tag is called \u{201c}{first}\u{201d}. It's been searched as text."
        ))
    }

    /// The keys of the items that carry every one of these tags, folders'
    /// tags reaching what's inside them (spec: browser-tags R9 and R15).
    /// Several tags must all be carried: the sets are intersected.
    fn items_with_tags(&self, tags: &[i64]) -> HashSet<String> {
        let mut all: Option<HashSet<String>> = None;
        for tag in tags {
            let carried = self.items_with_tag(*tag);
            all = Some(match all {
                Some(previous) => previous.intersection(&carried).cloned().collect(),
                None => carried,
            });
        }
        all.unwrap_or_default()
    }

    /// The keys of the items that carry a tag, a folder's tag reaching
    /// what's inside it, at any depth (spec: browser-tags R9).
    fn items_with_tag(&self, tag: i64) -> HashSet<String> {
        let mut keys: HashSet<String> = self.tags.items_with_tag(tag).into_iter().collect();
        if keys.is_empty() {
            return keys;
        }
        let snapshot = self.index.snapshot();
        for index in 0..snapshot.len() as u32 {
            let path = snapshot.path(index);
            let mut folder = path.parent();
            while let Some(dir) = folder {
                if keys.contains(&dir.to_string_lossy().into_owned()) {
                    keys.insert(path.to_string_lossy().into_owned());
                    break;
                }
                folder = dir.parent();
            }
        }
        keys
    }

    /// The rows the search finds (spec: browser R9 and browser-tags R15).
    fn search_hits(
        &self,
        snapshot: &Arc<Snapshot>,
        scope: Scope,
        only: Option<ItemKind>,
    ) -> Vec<u32> {
        let (words, tags, _) = self.search_terms();
        let hits = snapshot.search(&words.join(" "), scope, only);
        if tags.is_empty() {
            return hits;
        }
        let keys = self.items_with_tags(&tags);
        hits.into_iter()
            .filter(|&hit| keys.contains(&snapshot.path(hit).to_string_lossy().into_owned()))
            .collect()
    }

    /// The preferences to keep.
    #[must_use]
    pub fn prefs(&self, user_library: Option<PathBuf>) -> BrowserPrefs {
        BrowserPrefs {
            user_library,
            folders: self.folders.clone(),
            preview: self.preview_on,
            preview_volume: self.preview_volume,
            raw: self.raw,
            tag_editor: self.tag_editor,
            quick_tags: self.quick_tags,
            filters: self.filter_prefs.clone(),
            labels: self.label_prefs.clone(),
            columns: self.columns.clone(),
            full_height: self.full_height,
            width: self.width,
        }
    }

    /// The User Library, if there is one.
    #[must_use]
    pub fn user_library(&self) -> Option<&Path> {
        self.user_library.as_deref()
    }

    /// Uses `path` as the User Library.
    pub fn set_user_library(&mut self, path: Option<PathBuf>) {
        if self.user_library != path {
            self.user_library = path;
            self.places_changed();
        }
    }

    /// The open set's project folder, if it has one.
    #[must_use]
    pub fn project(&self) -> Option<&Path> {
        self.project.as_deref()
    }

    /// Records the open set's project folder (spec: browser R4).
    pub fn set_project(&mut self, folder: Option<PathBuf>) {
        if self.project != folder {
            self.project = folder;
            self.places_changed();
        }
    }

    /// The folders added to Places, in order.
    #[must_use]
    pub fn folders(&self) -> &[PathBuf] {
        &self.folders
    }

    /// The chosen label.
    #[must_use]
    pub fn label(&self) -> &Label {
        &self.label
    }

    /// The search field's text.
    #[must_use]
    pub fn search_text(&self) -> &str {
        &self.search
    }

    /// The part of the browser with the keyboard focus.
    #[must_use]
    pub fn focus(&self) -> Part {
        self.focus
    }

    /// A number that goes up whenever a command moves the keyboard focus to
    /// a part of the browser, so the UI moves it there.
    #[must_use]
    pub fn focus_requests(&self) -> u64 {
        self.focus_requests
    }

    /// Records which part of the browser has the keyboard focus, as a click
    /// there does.
    pub fn set_focus(&mut self, part: Part) {
        self.focus = part;
    }

    /// Whether the Preview switch is on.
    #[must_use]
    pub fn preview_on(&self) -> bool {
        self.preview_on
    }

    /// The places' roots, in the sidebar's order, each once: the User
    /// Library, Current Project, then the added folders.
    #[must_use]
    pub fn roots(&self) -> Vec<(Label, PathBuf)> {
        let mut roots: Vec<(Label, PathBuf)> = Vec::new();
        let mut push = |label: Label, path: &Path| {
            if !roots.iter().any(|(_, p)| p == path) {
                roots.push((label, path.to_path_buf()));
            }
        };
        if let Some(path) = &self.user_library {
            push(Label::UserLibrary, path);
        }
        if let Some(path) = &self.project {
            push(Label::CurrentProject, path);
        }
        for folder in &self.folders {
            push(Label::Folder(folder.to_string_lossy().into_owned()), folder);
        }
        roots
    }

    /// The User Library's `Templates` folder, where set templates are saved
    /// (WP-2.25; the browser spec, R2).
    fn templates_folder(&self) -> Option<PathBuf> {
        self.user_library
            .as_ref()
            .map(|library| library.join("Templates"))
    }

    /// A place's folder.
    fn place_path(&self, label: &Label) -> Option<PathBuf> {
        match label {
            Label::UserLibrary => self.user_library.clone(),
            Label::CurrentProject => self.project.clone(),
            Label::Folder(path) => Some(PathBuf::from(path)),
            _ => None,
        }
    }

    /// The name a label has in the sidebar: the person's, a saved label's
    /// own, or the label's.
    fn label_name(&self, label: &Label) -> String {
        match label {
            Label::Saved(id) => self
                .saved
                .iter()
                .find(|saved| saved.id == *id)
                .map_or_else(|| label.name(), |saved| saved.name.clone()),
            other => self
                .label_prefs
                .name(other)
                .map_or_else(|| other.name(), str::to_owned),
        }
    }

    /// The sidebar's Library labels, saved labels and places, in order
    /// (spec: browser R2, browser-tags R24 to R26).
    #[must_use]
    pub fn labels(&self) -> Vec<LabelInfo> {
        // Spec: browser-tags R34. The collections come first, as the
        // sidebar lists them.
        let mut labels: Vec<LabelInfo> = (1..=7)
            .map(|number| {
                let label = Label::Collection(number);
                LabelInfo {
                    available: true,
                    name: self.label_name(&label),
                    section: Section::Collections,
                    path: None,
                    icon: None,
                    shown: self.label_prefs.shown(&label),
                    label,
                }
            })
            .collect();
        labels.extend(
            self.label_prefs
                .ordered(&self.saved)
                .into_iter()
                .map(|label| LabelInfo {
                    available: label.pending().is_none(),
                    name: self.label_name(&label),
                    section: Section::Library,
                    path: None,
                    icon: self.label_icon(&label),
                    shown: self.label_prefs.shown(&label),
                    label,
                }),
        );
        let mut places = vec![Label::UserLibrary, Label::CurrentProject];
        places.extend(
            self.folders
                .iter()
                .map(|f| Label::Folder(f.to_string_lossy().into_owned())),
        );
        for label in places {
            let path = self.place_path(&label);
            labels.push(LabelInfo {
                available: path.as_deref().is_some_and(Path::is_dir),
                name: label.name(),
                section: Section::Places,
                path: path.map(|p| p.to_string_lossy().into_owned()),
                icon: self.label_icon(&label),
                shown: true,
                label,
            });
        }
        labels
    }

    /// The icon a label has: a saved label's own, or the person's.
    fn label_icon(&self, label: &Label) -> Option<String> {
        match label {
            Label::Saved(id) => self
                .saved
                .iter()
                .find(|saved| saved.id == *id)
                .and_then(|saved| saved.icon.clone()),
            other => self.label_prefs.icon(other).map(str::to_owned),
        }
    }

    /// Files were written or renamed in the places: index them again and
    /// list the label afresh.
    fn files_changed(&mut self) {
        self.generation += 1;
        self.forget_details();
        self.index.refresh();
        self.indexing = self.index.indexing();
        self.listings.clear();
        self.rebuild();
    }

    /// The places changed: index them again and list the label afresh.
    fn places_changed(&mut self) {
        self.generation += 1;
        self.forget_details();
        let roots = self.roots().into_iter().map(|(_, path)| path).collect();
        self.index.set_roots(roots);
        self.indexing = self.index.indexing();
        if let Label::Folder(path) = &self.label
            && !self
                .folders
                .iter()
                .any(|f| f.to_string_lossy() == path.as_str())
        {
            self.label = Label::All;
        }
        self.listings.clear();
        // The database's index is what's shown until the walk brings it up
        // to date (spec: browser-tags R1).
        self.adopt_index();
        self.rebuild();
    }

    /// The scope a label searches, and whether only audio files count; or
    /// `None` for a Library label without content yet.
    fn scope(&self, label: &Label) -> Option<(Scope, Option<ItemKind>)> {
        match label {
            Label::All => Some((Scope::All, None)),
            Label::Samples => Some((Scope::All, Some(ItemKind::Audio))),
            Label::Clips => Some((Scope::All, Some(ItemKind::Clip))),
            place if place.is_place() => {
                let path = self.place_path(place)?;
                let index = self
                    .index
                    .snapshot()
                    .roots()
                    .iter()
                    .position(|root| *root == path)?;
                Some((Scope::Place(u16::try_from(index).ok()?), None))
            }
            _ => None,
        }
    }

    /// A folder's listing, read now if it isn't known.
    fn listing(&mut self, folder: &Path) -> Option<Vec<Entry>> {
        let mut entries = if let Some(entries) = self.listings.get(folder) {
            entries.clone()
        } else {
            let entries = listing::list(folder).ok()?;
            self.listings.insert(folder.to_path_buf(), entries.clone());
            entries
        };
        // Spec: browser-tags R28. The column the list is sorted by.
        self.sort_entries(&mut entries);
        Some(entries)
    }

    /// Adds a folder's items, and its open folders' items under them.
    fn list_into(&mut self, folder: &Path, depth: u16, out: &mut Vec<Item>) {
        let Some(entries) = self.listing(folder) else {
            return;
        };
        self.push_entries(entries, depth, out);
    }

    /// Adds entries' items, and the open folders' items under them.
    fn push_entries(&mut self, entries: Vec<Entry>, depth: u16, out: &mut Vec<Item>) {
        for entry in entries {
            let openable = matches!(entry.kind, ItemKind::Folder | ItemKind::Set);
            let open = openable && self.open.contains(&entry.path);
            out.push(Item {
                path: entry.path.to_string_lossy().into_owned(),
                name: entry.name.clone(),
                kind: entry.kind,
                depth,
                open,
                folder: None,
                format: None,
                clip_kind: entry.clip,
                details: None,
                collections: Vec::new(),
            });
            if open && entry.kind == ItemKind::Set {
                self.set_rows(&entry.path, depth.saturating_add(1), out);
            } else if open {
                self.list_into(&entry.path, depth.saturating_add(1), out);
            }
        }
    }

    /// The rows of an opened set (set exchange R1): its tracks, or a line
    /// saying why it can't be read or that it is being read.
    fn set_rows(&self, set: &Path, depth: u16, out: &mut Vec<Item>) {
        let note = match self.sets.get(set) {
            Some(SetState::Ready(opened)) => {
                opened.rows(depth, &self.open, out);
                return;
            }
            Some(SetState::Refused(why)) => why.clone(),
            _ => "Reading the set\u{2026}".to_owned(),
        };
        out.push(Item {
            path: format!("{}#note", set.to_string_lossy()),
            name: note,
            kind: ItemKind::Notice,
            depth,
            open: false,
            folder: None,
            format: None,
            clip_kind: None,
            details: None,
            collections: Vec::new(),
        });
    }

    /// Whether `key` is a row of a set that is open in the content pane.
    fn is_set_row(&self, key: &str) -> bool {
        key.rsplit_once('#')
            .is_some_and(|(set, _)| self.sets.contains_key(Path::new(set)))
    }

    /// Starts reading the set at `path` on a background thread, to list its
    /// rows when it is read (set exchange R1).
    fn read_set(&mut self, path: PathBuf) {
        let (reads, waker) = (self.set_reads.0.clone(), self.waker.clone());
        let file = path.clone();
        let started = std::thread::Builder::new()
            .name("bl-browser-set".to_owned())
            .spawn(move || {
                let read = set_exchange::open(&file);
                let _ = reads.send((file, read));
                waker.wake();
            });
        let state = match started {
            Ok(_) => SetState::Reading,
            Err(error) => SetState::Refused(format!("couldn't read the set: {error}")),
        };
        self.sets.insert(path, state);
    }

    /// Takes what a background read of a set found, if the set is still open.
    fn adopt_set(&mut self, (path, read): SetRead) -> bool {
        if !self.sets.contains_key(&path) {
            return false;
        }
        let state = match read {
            Ok(opened) => SetState::Ready(opened),
            Err(why) => SetState::Refused(why),
        };
        self.sets.insert(path, state);
        true
    }

    /// Forgets the sets and the set rows that aren't inside an open
    /// folder any more.
    fn close_sets(&mut self) {
        let open = self.open.clone();
        self.sets.retain(|set, _| open.contains(set));
        // A folder with a `#` in its name isn't a row of a set.
        self.open.retain(|p| {
            set_exchange::parse_key(&p.to_string_lossy()).is_none_or(|(set, _)| open.contains(&set))
        });
    }

    /// Waits until every set being read has been, for tests and tools.
    pub fn wait_for_sets(&mut self) {
        while self
            .sets
            .values()
            .any(|state| matches!(state, SetState::Reading))
        {
            let Ok(read) = self.set_reads.1.recv() else {
                break;
            };
            self.adopt_set(read);
        }
        self.rebuild();
    }

    /// Shows a file the shell wrote in the content pane, selected, with its
    /// name open for editing (clip files R4; set exchange R13), if it lies in
    /// one of the places. Returns whether it was shown.
    pub fn file_saved(&mut self, path: &Path, audio: &AudioHost) -> bool {
        self.files_changed();
        let shown = self.reveal(path, audio).is_ok();
        self.renaming = shown.then(|| path.to_path_buf());
        shown
    }

    /// Works out the content pane's rows again, keeping the selection.
    fn rebuild(&mut self) {
        self.note = None;
        // A saved label lists what its words and tags find under the label
        // it was made under (spec: browser-tags R24).
        let label = self.base_label();
        let (_, tags, _) = self.search_terms();
        // The selected filter tags in the groups the label shows (spec:
        // browser-tags R20).
        let selection = self.applied_selection();
        let builtin = if selection.is_empty() {
            filter::BuiltinIds::default()
        } else {
            self.builtin_ids()
        };
        if let Label::Collection(number) = label {
            // Spec: browser-tags R36. A collection lists the items in it.
            let items = self.collection_rows(number, &tags, &selection, &builtin);
            self.rows = Rows::Items(items);
            self.cursor = self.selected.last().and_then(|path| self.position_of(path));
            return;
        }
        if !label_devices(&label).is_empty() {
            // Spec: modulation R12, racks R43. These labels list devices.
            let query = self.search.trim().to_ascii_lowercase();
            let items: Vec<Item> = label_devices(&label)
                .into_iter()
                .filter(|d| query.is_empty() || d.name.to_ascii_lowercase().contains(&query))
                .filter(|d| self.carries_all(&d.id, &tags))
                .map(|d| Item {
                    path: d.id,
                    name: d.name,
                    kind: ItemKind::Device,
                    depth: 0,
                    open: false,
                    folder: None,
                    format: None,
                    clip_kind: None,
                    details: None,
                    collections: Vec::new(),
                })
                .filter(|item| self.item_passes(&selection, &builtin, item))
                .collect();
            if items.is_empty() {
                self.note = Some(format!(
                    "Nothing matches \u{201c}{}\u{201d}.",
                    self.search.trim()
                ));
            }
            self.rows = Rows::Items(items);
            self.cursor = self.selected.last().and_then(|path| self.position_of(path));
            return;
        }
        if label == Label::PlugIns {
            let items = self.plugin_rows();
            let items = items
                .into_iter()
                .filter(|item| {
                    tags.is_empty() || self.carries_all(&self.tag_key(&item.path), &tags)
                })
                .filter(|item| self.item_passes(&selection, &builtin, item))
                .collect();
            self.rows = Rows::Items(items);
            self.cursor = self.selected.last().and_then(|path| self.position_of(path));
            return;
        }
        if !self.search.trim().is_empty()
            || !selection.is_empty()
            || matches!(label, Label::Samples | Label::Clips)
        {
            match self.scope(&label) {
                Some((scope, only)) => {
                    let snapshot = Arc::clone(self.index.snapshot());
                    let mut hits = self.search_hits(&snapshot, scope, only);
                    if !selection.is_empty() {
                        hits.retain(|&hit| {
                            let ids = self.file_ids(
                                &snapshot.path(hit),
                                snapshot.entry(hit).kind,
                                &builtin,
                            );
                            selection.matches(&ids)
                        });
                    }
                    self.sort_hits(&snapshot, &mut hits);
                    if hits.is_empty() {
                        self.note = Some(if self.index.indexing() {
                            "Indexing the places\u{2026}".to_owned()
                        } else if self.search.trim().is_empty() && !selection.is_empty() {
                            "Nothing carries the selected tags.".to_owned()
                        } else if self.search.trim().is_empty() && label == Label::Clips {
                            "No clip files in the places yet. Drag a clip here to save it."
                                .to_owned()
                        } else if self.search.trim().is_empty() {
                            "No audio files in the places yet. Add samples to the User Library, \
                             or add a folder to Places."
                                .to_owned()
                        } else {
                            format!("Nothing matches \u{201c}{}\u{201d}.", self.search.trim())
                        });
                    }
                    self.rows = Rows::Hits { snapshot, hits };
                }
                None if label == Label::Templates => {
                    // Spec: file-management R5. Templates aren't in the
                    // index; the folder is listed and filtered by name.
                    let mut items = Vec::new();
                    if let Some(folder) = self.templates_folder() {
                        self.list_into(&folder, 0, &mut items);
                    }
                    let needle = self.search.trim().to_lowercase();
                    items.retain(|item| item.name.to_lowercase().contains(&needle));
                    items.retain(|item| self.item_passes(&selection, &builtin, item));
                    if items.is_empty() {
                        self.note = Some(format!("Nothing matches \u{201c}{needle}\u{201d}."));
                    }
                    self.rows = Rows::Items(items);
                }
                None => {
                    self.rows = Rows::Items(Vec::new());
                    self.note = Some(label.pending().map_or_else(
                        || "This place hasn't been indexed yet.".to_owned(),
                        str::to_owned,
                    ));
                }
            }
        } else {
            let mut items = Vec::new();
            match &label {
                Label::All => {
                    for (place, path) in self.roots() {
                        let open = self.open.contains(&path);
                        items.push(Item {
                            path: path.to_string_lossy().into_owned(),
                            name: place.name(),
                            kind: ItemKind::Folder,
                            depth: 0,
                            open,
                            folder: None,
                            format: None,
                            clip_kind: None,
                            details: None,
                            collections: Vec::new(),
                        });
                        if open {
                            self.list_into(&path, 1, &mut items);
                        }
                    }
                    if items.is_empty() {
                        self.note = Some(
                            "No places yet: add a folder to Places, or save the set.".to_owned(),
                        );
                    }
                }
                place if place.is_place() => match self.place_path(place) {
                    Some(path) if path.is_dir() => {
                        self.list_into(&path, 0, &mut items);
                        if items.is_empty() {
                            self.note = Some("This folder holds no audio files.".to_owned());
                        }
                    }
                    Some(path) => {
                        self.note = Some(format!(
                            "This folder can't be found: {}",
                            path.to_string_lossy()
                        ));
                    }
                    None if *place == Label::CurrentProject => {
                        self.note = Some(
                            "This set hasn't been saved, so it has no project folder yet."
                                .to_owned(),
                        );
                    }
                    None => {
                        self.note = Some("The User Library hasn't been set up.".to_owned());
                    }
                },
                Label::Templates => match self.templates_folder() {
                    Some(path) if path.is_dir() => {
                        self.list_into(&path, 0, &mut items);
                        if items.is_empty() {
                            self.note = Some(
                                "No templates yet: save one with File ▸ Save Beatlayer Set As \
                                 Template...."
                                    .to_owned(),
                            );
                        }
                    }
                    _ => {
                        self.note = Some(
                            "The User Library hasn't been set up, so there's no Templates folder."
                                .to_owned(),
                        );
                    }
                },
                other => {
                    self.note = other.pending().map(str::to_owned);
                }
            }
            self.rows = Rows::Items(items);
        }
        self.cursor = self.selected.last().and_then(|path| self.position_of(path));
    }

    /// The Plug-Ins label's rows (spec: plugin-scanning R7 and R11): a
    /// folder per vendor, in name order, each holding its plug-ins, one row
    /// per format, as Live 12 lays them out; plug-ins that name no vendor
    /// follow the folders. A search lists the plug-ins whose names match,
    /// each naming its vendor.
    fn plugin_rows(&mut self) -> Vec<Item> {
        let query = self.search.trim().to_lowercase();
        let row = |p: &ScannedPlugin, depth: u16, folder: Option<String>| Item {
            path: plugin_key(p),
            name: p.name.clone(),
            kind: ItemKind::Plugin,
            depth,
            open: false,
            folder,
            format: Some(p.format),
            clip_kind: None,
            details: None,
            collections: Vec::new(),
        };
        let mut items = Vec::new();
        if !query.is_empty() || !self.applied_selection().is_empty() {
            items.extend(
                self.plugins
                    .iter()
                    .filter(|p| p.name.to_lowercase().contains(&query))
                    .map(|p| row(p, 0, p.vendor.clone())),
            );
            if items.is_empty() {
                self.note = Some(format!(
                    "Nothing matches \u{201c}{}\u{201d}.",
                    self.search.trim()
                ));
            }
            return items;
        }
        let mut vendors: Vec<&str> = self.plugins.iter().filter_map(|p| vendor_of(p)).collect();
        vendors.sort_by_key(|v| v.to_lowercase());
        vendors.dedup();
        for vendor in vendors {
            let key = vendor_key(vendor);
            let open = self.open.contains(Path::new(&key));
            items.push(Item {
                path: key,
                name: vendor.to_owned(),
                kind: ItemKind::Folder,
                depth: 0,
                open,
                folder: None,
                format: None,
                clip_kind: None,
                details: None,
                collections: Vec::new(),
            });
            if open {
                items.extend(
                    self.plugins
                        .iter()
                        .filter(|p| vendor_of(p) == Some(vendor))
                        .map(|p| row(p, 1, None)),
                );
            }
        }
        items.extend(
            self.plugins
                .iter()
                .filter(|p| vendor_of(p).is_none())
                .map(|p| row(p, 0, None)),
        );
        if items.is_empty() {
            self.note = Some(
                "No plug-ins found. Rescan, or switch on a plug-in format, in Settings \u{25b8} \
                 Plug-Ins."
                    .to_owned(),
            );
        }
        items
    }

    /// The plug-in a Plug-Ins row's key names, if the label lists it.
    #[must_use]
    pub fn plugin(&self, key: &Path) -> Option<&ScannedPlugin> {
        let key = key.to_string_lossy();
        self.plugins.iter().find(|p| plugin_key(p) == key)
    }

    /// The selected plug-in, when the selection is one plug-in: what Enter
    /// or a double-click loads.
    #[must_use]
    pub fn selected_plugin(&self) -> Option<&ScannedPlugin> {
        match self.selected.as_slice() {
            [one] => self.plugin(one),
            _ => None,
        }
    }

    /// Whether `key` names a Plug-Ins row: a vendor folder or a plug-in.
    fn is_plugin_row(&self, key: &str) -> bool {
        key.strip_prefix(PLUGIN_FOLDER)
            .is_some_and(|vendor| self.plugins.iter().any(|p| vendor_of(p) == Some(vendor)))
            || self.plugin(Path::new(key)).is_some()
    }

    /// How many rows the content pane has.
    #[must_use]
    pub fn row_count(&self) -> usize {
        match &self.rows {
            Rows::Items(items) => items.len(),
            Rows::Hits { hits, .. } => hits.len(),
        }
    }

    /// The row at `index`.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<Item> {
        let mut item = match &self.rows {
            Rows::Items(items) => items.get(index).cloned()?,
            Rows::Hits { snapshot, hits } => {
                let hit = *hits.get(index)?;
                let entry = snapshot.entry(hit);
                Item {
                    path: snapshot.path(hit).to_string_lossy().into_owned(),
                    name: entry.name.to_string(),
                    kind: entry.kind,
                    depth: 0,
                    open: false,
                    folder: Some(snapshot.folder_label(hit)),
                    format: None,
                    clip_kind: clip_kind_of(entry.kind, &snapshot.path(hit)),
                    details: None,
                    collections: Vec::new(),
                }
            }
        };
        // Spec: browser-tags R27. What the shown columns say about it.
        item.details = self.details_for(&item);
        // Spec: browser-tags R37. The collections it is in.
        item.collections = if item.kind == ItemKind::Plugin {
            self.collections.of(&self.tag_key(&item.path))
        } else {
            self.collections.of(&item.path)
        };
        Some(item)
    }

    /// Where an item is in the content pane.
    #[must_use]
    pub fn position_of(&self, path: &Path) -> Option<usize> {
        match &self.rows {
            Rows::Items(items) => items.iter().position(|item| Path::new(&item.path) == path),
            Rows::Hits { snapshot, hits } => snapshot.position_of(hits, path),
        }
    }

    /// What the content pane says when it has no rows.
    #[must_use]
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// The selected items, in the order they were selected.
    #[must_use]
    pub fn selected(&self) -> &[PathBuf] {
        &self.selected
    }

    /// Whether an item is selected.
    #[must_use]
    pub fn is_selected(&self, path: &Path) -> bool {
        self.selected.iter().any(|p| p == path)
    }

    /// The row of the item selected last, which the arrow keys move from.
    #[must_use]
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// How far indexing has got.
    #[must_use]
    pub fn index_state(&self) -> IndexState {
        let snapshot = self.index.snapshot();
        IndexState {
            files: snapshot.files,
            folders: snapshot.folder_count,
            indexing: self.indexing,
        }
    }

    /// Waits for indexing to finish, up to `timeout`, for tests and tools.
    /// Returns whether it finished.
    pub fn wait_for_index(&mut self, timeout: Duration) -> bool {
        let done = self.index.wait(timeout);
        self.indexing = self.index.indexing();
        self.rebuild();
        done
    }

    /// The preview: the switch, the volume, and what plays, from what the
    /// audio thread last reported.
    #[must_use]
    pub fn preview_state(&self, audio: &AudioHost) -> PreviewState {
        let status = audio.preview_status();
        let rate = f64::from(self.preview_rate.max(1));
        // Given to the audio thread and not yet finished.
        let playing = self
            .playing
            .as_ref()
            .filter(|p| status.playing == Some(p.generation) || status.started < p.generation);
        let (position, length) = match playing {
            Some(p) if status.playing == Some(p.generation) => (status.position, status.length),
            _ => (0, 0),
        };
        PreviewState {
            on: self.preview_on,
            raw: self.raw,
            volume: self.preview_volume,
            loading: self
                .loading
                .as_ref()
                .map(|(_, path)| path.to_string_lossy().into_owned())
                .or_else(|| {
                    self.pending_warp
                        .as_ref()
                        .map(|p| p.path.to_string_lossy().into_owned())
                }),
            playing: playing.map(|p| p.path.to_string_lossy().into_owned()),
            note: self.preview_note.clone(),
            position_seconds: position as f64 / rate,
            length_seconds: length as f64 / rate,
        }
    }

    /// The browser's whole state, with rows `offset` to `offset + limit`.
    #[must_use]
    pub fn state(&self, audio: &AudioHost, offset: usize, limit: usize) -> BrowserState {
        let total = self.row_count();
        let first = offset.min(total);
        let end = first.saturating_add(limit).min(total);
        BrowserState {
            label: self.label.clone(),
            labels: self.labels(),
            search: self.search.clone(),
            rows: (first..end).filter_map(|i| self.row(i)).collect(),
            first_row: first,
            total_rows: total,
            note: self.note.clone(),
            selected: self
                .selected
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            preview: self.preview_state(audio),
            index: self.index_state(),
            renaming: self
                .renaming
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            tag_editor: self.tag_editor,
            quick_tags: self.quick_tags,
            filter_view: self.filter_view(),
            saved_labels: self.saved_labels(),
            renaming_label: self.renaming_label.clone(),
            columns: self.columns.clone(),
            can_back: self.can_back(),
            can_forward: self.can_forward(),
        }
    }

    /// Whether the browser runs down to the Status Bar (Full-Height
    /// Browser; spec: browser R26).
    #[must_use]
    pub fn full_height(&self) -> bool {
        self.full_height
    }

    /// Runs the browser down to the Status Bar, or back.
    pub fn set_full_height(&mut self, on: bool) {
        self.full_height = on;
    }

    /// The browser's width in points at 100%, as it was set: a window too
    /// narrow for it shows the browser narrower (spec: browser R29).
    #[must_use]
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Sets the browser's width, brought within its narrowest and widest.
    pub fn set_width(&mut self, points: f32) {
        self.width = clamp_browser_width(points);
    }

    /// Takes background work's news: a newer index, a decoded preview, a
    /// preview that finished. The host calls it every frame and when woken.
    pub fn poll(
        &mut self,
        audio: &mut AudioHost,
        media: &bl_media::MediaStore,
        warping: crate::auto_warp::Warping<'_>,
        set_bpm: f64,
    ) -> Polled {
        let mut polled = Polled::default();
        let mut news = false;
        while let Ok(read) = self.set_reads.1.try_recv() {
            news |= self.adopt_set(read);
        }
        if news {
            self.rebuild();
            polled.changed = true;
        }
        if self.index.poll() {
            self.generation += 1;
            if matches!(self.rows, Rows::Hits { .. }) {
                self.rebuild();
            }
            polled.changed = true;
        }
        let indexing = self.index.indexing();
        if indexing != self.indexing {
            self.indexing = indexing;
            self.index.poll();
            self.rebuild();
            polled.changed = true;
            if !indexing {
                // The walk is done: the database catches up with what
                // changed on disk.
                self.save_index();
            }
        }
        for hashed in self.hasher.poll() {
            self.hash_ready(hashed);
            polled.changed = true;
        }
        for loaded in self.loader.poll() {
            if self.loading.as_ref().map(|(id, _)| *id) != Some(loaded.id) {
                continue;
            }
            self.loading = None;
            polled.changed = true;
            if loaded.result.is_ok() {
                self.overview = Some((loaded.path.clone(), loaded.overview.into()));
            }
            match loaded.result {
                Ok(data) => {
                    let seconds = data.frames() as f64 / f64::from(data.sample_rate().max(1));
                    if let Some(clip) = loaded.clip {
                        self.play_clip(audio, loaded.path, data, clip, set_bpm, &mut polled);
                    } else if self.raw {
                        match audio.play_preview(Box::new(MediaSamples(data))) {
                            Ok(generation) => {
                                self.playing = Some(Playing {
                                    generation,
                                    path: loaded.path,
                                    started: false,
                                    warp_map: None,
                                    length_seconds: seconds,
                                });
                            }
                            Err(error) => polled.error = Some(format!("Couldn't preview: {error}")),
                        }
                    } else {
                        // The analysis cache reads files through the media
                        // store, so a browsed file joins it first.
                        let hash = media.import(&loaded.path);
                        let warp =
                            hash.as_ref()
                                .map_or(crate::auto_warp::PreviewWarp::Unwarped, |hash| {
                                    crate::auto_warp::preview_warp(
                                        *hash,
                                        &loaded.path,
                                        seconds,
                                        set_bpm,
                                        warping,
                                    )
                                });
                        match (warp, hash) {
                            (crate::auto_warp::PreviewWarp::Pending, Ok(hash)) => {
                                self.pending_warp = Some(PendingPreview {
                                    path: loaded.path,
                                    hash,
                                    data,
                                    seconds,
                                });
                            }
                            (crate::auto_warp::PreviewWarp::Warped(audio_data), Ok(hash)) => {
                                self.start_warped_preview(
                                    audio,
                                    PendingPreview {
                                        path: loaded.path,
                                        hash,
                                        data,
                                        seconds,
                                    },
                                    audio_data,
                                    set_bpm,
                                    &mut polled,
                                );
                            }
                            _ => match audio.play_preview(Box::new(MediaSamples(data))) {
                                Ok(generation) => {
                                    self.playing = Some(Playing {
                                        generation,
                                        path: loaded.path,
                                        started: false,
                                        warp_map: None,
                                        length_seconds: seconds,
                                    });
                                }
                                Err(error) => {
                                    polled.error = Some(format!("Couldn't preview: {error}"))
                                }
                            },
                        }
                    }
                }
                Err(error) => {
                    polled.error = Some(format!(
                        "Couldn't preview {}: {error}",
                        loaded.path.to_string_lossy()
                    ));
                    self.preview_note = Some(error);
                }
            }
        }
        if let Some(pending) = self.pending_warp.take() {
            match crate::auto_warp::preview_warp(
                pending.hash,
                &pending.path,
                pending.seconds,
                set_bpm,
                warping,
            ) {
                crate::auto_warp::PreviewWarp::Pending => {
                    self.pending_warp = Some(pending);
                }
                crate::auto_warp::PreviewWarp::Warped(audio_data) => {
                    polled.changed = true;
                    self.start_warped_preview(audio, pending, audio_data, set_bpm, &mut polled);
                }
                crate::auto_warp::PreviewWarp::Unwarped => {
                    polled.changed = true;
                    match audio.play_preview(Box::new(MediaSamples(pending.data))) {
                        Ok(generation) => {
                            self.playing = Some(Playing {
                                generation,
                                path: pending.path,
                                started: false,
                                warp_map: None,
                                length_seconds: pending.seconds,
                            });
                        }
                        Err(error) => polled.error = Some(format!("Couldn't preview: {error}")),
                    }
                }
            }
        }
        if let Some(playing) = &mut self.playing {
            let status = audio.preview_status();
            if status.playing == Some(playing.generation) {
                if !playing.started {
                    playing.started = true;
                    polled.changed = true;
                }
            } else if status.started >= playing.generation || !audio.is_running() {
                // It reached its end, or the device closed.
                self.playing = None;
                polled.changed = true;
            }
        }
        polled
    }

    /// Previews an audio clip file through its clip's own settings (clip
    /// files R13): warped and looping at the set's tempo, or, with Raw on or
    /// for an unwarped clip, once from the clip's start at the audio's own
    /// speed.
    fn play_clip(
        &mut self,
        audio: &mut AudioHost,
        path: PathBuf,
        data: bl_media::SampleData,
        clip: Box<bl_model::AudioData>,
        set_bpm: f64,
        polled: &mut Polled,
    ) {
        let seconds = data.frames() as f64 / f64::from(data.sample_rate().max(1));
        if self.raw || !clip.warp {
            let data = preview::trim(&data, clip.start, clip.end);
            let length_seconds = data.frames() as f64 / f64::from(data.sample_rate().max(1));
            match audio.play_preview(Box::new(MediaSamples(data))) {
                Ok(generation) => {
                    self.playing = Some(Playing {
                        generation,
                        path,
                        started: false,
                        warp_map: None,
                        length_seconds,
                    });
                }
                Err(error) => polled.error = Some(format!("Couldn't preview: {error}")),
            }
            return;
        }
        let hash = bl_media::ContentHash::from_bytes(clip.media.0);
        let pending = PendingPreview {
            path,
            hash,
            data,
            seconds,
        };
        self.start_warped_preview(audio, pending, clip, set_bpm, polled);
    }

    fn start_warped_preview(
        &mut self,
        audio: &mut AudioHost,
        pending: PendingPreview,
        audio_data: Box<bl_model::AudioData>,
        set_bpm: f64,
        polled: &mut Polled,
    ) {
        let map = match audio_data.warp_map() {
            Ok(m) => m,
            Err(e) => {
                polled.error = Some(format!("Couldn't warp preview: {e}"));
                return;
            }
        };
        let start = audio_data.start;
        let end = audio_data
            .end
            .unwrap_or_else(|| map.time_to_beat(pending.seconds));
        let mut loop_start = audio_data.loop_start;
        let mut loop_end = audio_data.loop_end.unwrap_or(end);
        if loop_end <= loop_start {
            loop_start = start;
            loop_end = end;
        }
        let sample_rate = pending.data.sample_rate() as f64;
        let seconds = pending.seconds;
        let path = pending.path;
        let warped = bl_engine::preview::WarpedPreview {
            samples: Box::new(MediaSamples(pending.data)),
            map: map.clone(),
            start,
            end,
            loop_start,
            loop_end,
            sample_rate,
            tempo: set_bpm,
        };
        match audio.play_warped_preview(Box::new(warped)) {
            Ok(generation) => {
                self.playing = Some(Playing {
                    generation,
                    path,
                    started: false,
                    warp_map: Some(map),
                    length_seconds: seconds,
                });
            }
            Err(error) => polled.error = Some(format!("Couldn't preview: {error}")),
        }
    }

    /// The file previewed last and its waveform overview, for the preview
    /// strip: the lowest and highest sample of equal stretches of it.
    #[must_use]
    pub fn preview_overview(&self) -> Option<(&Path, &[[f32; 2]])> {
        self.overview
            .as_ref()
            .map(|(path, overview)| (path.as_path(), &overview[..]))
    }

    /// Whether a preview is loading or playing, so the UI redraws its
    /// progress.
    #[must_use]
    pub fn preview_active(&self) -> bool {
        self.loading.is_some() || self.pending_warp.is_some() || self.playing.is_some()
    }

    /// Renames the file at `path` to `name`, keeping its extension; returns
    /// its new path.
    fn renamed(&self, path: &Path, name: &str) -> Result<PathBuf, BrowserError> {
        if !path.is_file() {
            return Err(BrowserError::InvalidArgs(format!(
                "{} isn't a file",
                path.to_string_lossy()
            )));
        }
        let name = bl_project::preset::file_safe(name);
        let new = match path.extension() {
            Some(extension) => {
                path.with_file_name(format!("{name}.{}", extension.to_string_lossy()))
            }
            None => path.with_file_name(&name),
        };
        if new == path {
            return Ok(new);
        }
        if new.exists() {
            return Err(BrowserError::InvalidArgs(format!(
                "{} is already there",
                new.to_string_lossy()
            )));
        }
        std::fs::rename(path, &new)
            .map_err(|e| BrowserError::Failed(format!("couldn't rename the file: {e}")))?;
        Ok(new)
    }

    fn start_preview(
        &mut self,
        path: &Path,
        audio: &mut AudioHost,
        search: &[PathBuf],
        bpm: f64,
    ) -> Result<(), BrowserError> {
        let kind = listing::path_kind(path);
        // A Session clip of an opened set previews as a clip file does.
        let row = set_exchange::parse_key(&path.to_string_lossy()).is_some();
        if !matches!(kind, Some(ItemKind::Audio | ItemKind::Clip)) || !(row || path.is_file()) {
            return Err(BrowserError::InvalidArgs(format!(
                "{} isn't an audio file or clip file Beatlayer opens",
                path.to_string_lossy()
            )));
        }
        let rate = audio.sample_rate().ok_or_else(|| {
            BrowserError::Unavailable(
                "the audio device isn't running; switch the audio engine on to preview".to_owned(),
            )
        })?;
        audio.stop_preview();
        self.playing = None;
        self.pending_warp = None;
        self.preview_note = None;
        let id = if kind == Some(ItemKind::Clip) {
            self.loader.request_clip(path, rate, search.to_vec(), bpm)
        } else {
            self.loader.request(path, rate)
        }
        .map_err(BrowserError::Failed)?;
        self.loading = Some((id, path.to_path_buf()));
        self.preview_rate = rate;
        Ok(())
    }

    fn stop_preview(&mut self, audio: &AudioHost) {
        audio.stop_preview();
        self.loader.cancel();
        self.preview_note = None;
        self.loading = None;
        self.pending_warp = None;
        self.playing = None;
    }

    /// The device selected under a label that lists devices.
    #[must_use]
    pub fn selected_device(&self) -> Option<BrowserDevice> {
        device_by_id(&self.selected.last()?.to_string_lossy())
    }

    /// Whether no audio file is selected.
    #[must_use]
    pub fn selected_audio_is_empty(&self) -> bool {
        self.selected_audio().is_empty()
    }

    /// The SFZ instrument selected, if one file is and it is that.
    #[must_use]
    pub fn selected_sfz(&self) -> Option<PathBuf> {
        match self.selected.as_slice() {
            [one]
                if one
                    .file_name()
                    .and_then(|n| file_kind(&n.to_string_lossy()))
                    == Some(ItemKind::Sfz) =>
            {
                Some(one.clone())
            }
            _ => None,
        }
    }

    /// The Stacks chord bank selected, if one file is and it is that.
    #[must_use]
    pub fn selected_chord_bank(&self) -> Option<PathBuf> {
        match self.selected.as_slice() {
            [one]
                if one
                    .file_name()
                    .and_then(|n| file_kind(&n.to_string_lossy()))
                    == Some(ItemKind::ChordBank) =>
            {
                Some(one.clone())
            }
            _ => None,
        }
    }

    /// The selected audio files, in the content pane's order.
    pub(crate) fn selected_audio(&self) -> Vec<PathBuf> {
        let mut files: Vec<(Option<usize>, PathBuf)> = self
            .selected
            .iter()
            .filter(|p| {
                p.file_name().and_then(|n| file_kind(&n.to_string_lossy())) == Some(ItemKind::Audio)
            })
            .map(|p| (self.position_of(p), p.clone()))
            .collect();
        files.sort_by_key(|(position, _)| *position);
        files.into_iter().map(|(_, path)| path).collect()
    }

    /// The file whose name is open for editing after a clip was saved (clip
    /// files R4).
    #[must_use]
    pub fn renaming(&self) -> Option<&Path> {
        self.renaming.as_deref()
    }

    /// The tracks, or one Devices row, selected in a set opened in the content
    /// pane, with the set (set exchange R4, R8): what Enter adds to the open
    /// set. `None` unless every selected item is such a row of one set.
    #[must_use]
    pub fn selected_set_rows(&self) -> Option<(PathBuf, Vec<set_exchange::Row>)> {
        let mut set = None;
        let mut rows = Vec::new();
        for path in &self.selected {
            let (found, row) = set_exchange::parse_key(&path.to_string_lossy())?;
            if set.get_or_insert_with(|| found.clone()) != &found
                || matches!(row, set_exchange::Row::Clip(_))
            {
                return None;
            }
            rows.push((self.position_of(path), row));
        }
        rows.sort_by_key(|(position, _)| *position);
        let rows: Vec<set_exchange::Row> = rows.into_iter().map(|(_, row)| row).collect();
        let several_devices = rows.len() > 1
            && rows
                .iter()
                .any(|r| matches!(r, set_exchange::Row::Devices(_)));
        (!rows.is_empty() && !several_devices).then(|| (set.unwrap_or_default(), rows))
    }

    /// The selected clip files, in the content pane's order.
    pub(crate) fn selected_clip_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<(Option<usize>, PathBuf)> = self
            .selected
            .iter()
            .filter(|p| listing::path_kind(p) == Some(ItemKind::Clip))
            .map(|p| (self.position_of(p), p.clone()))
            .collect();
        files.sort_by_key(|(position, _)| *position);
        files.into_iter().map(|(_, path)| path).collect()
    }

    /// The folders searched for a clip file's audio that isn't where the
    /// file says: the places (the User Library, the Current Project and the
    /// folders added), clip files R12.
    #[must_use]
    pub fn search_folders(&self) -> Vec<PathBuf> {
        self.roots().into_iter().map(|(_, path)| path).collect()
    }

    /// Whether a person can run browser command `id` with `args` now.
    #[must_use]
    pub fn can_run(&self, id: &str, args: &Value) -> bool {
        match id {
            ids::LOAD_SELECTED => {
                self.selected_set_rows().is_some()
                    || !self.selected_audio().is_empty()
                    || !self.selected_clip_files().is_empty()
                    || self.selected_plugin().is_some()
                    || self.selected_device().is_some()
                    || self.selected_sfz().is_some()
                    || self.selected_chord_bank().is_some()
            }
            ids::PREVIEW => {
                args.get("path").is_some()
                    || self.selected_audio().len() == 1
                    || self.selected_clip_files().len() == 1
            }
            ids::RENAME_LABEL => {
                let label = args
                    .get("label")
                    .and_then(|label| serde_json::from_value::<Label>(label.clone()).ok())
                    .unwrap_or_else(|| self.label.clone());
                label.is_library() || label.is_collection()
            }
            ids::SET_COLLECTION | ids::CLEAR_COLLECTIONS => {
                let items: Option<Vec<String>> = args
                    .get("items")
                    .and_then(|items| serde_json::from_value(items.clone()).ok());
                self.can_collect(items.as_deref())
            }
            ids::REVEAL => {
                let paths: Option<Vec<String>> = args
                    .get("paths")
                    .and_then(|paths| serde_json::from_value(paths.clone()).ok());
                self.can_collect(paths.as_deref())
            }
            ids::BACK => self.can_back(),
            ids::FORWARD => self.can_forward(),
            ids::ADD_LABEL => self.results_bar(),
            ids::REMOVE_FOLDER => args
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| self.folders.iter().any(|f| f.to_string_lossy() == path)),
            _ => true,
        }
    }

    /// Runs browser command `id` with `args`, as `bl-shell`'s `Shell::run`
    /// does.
    ///
    /// # Errors
    ///
    /// [`BrowserError`] if the arguments don't fit, it can't run now, or it
    /// fails; nothing has changed then.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per command keeps each one's behaviour in sight"
    )]
    pub fn run(
        &mut self,
        id: &str,
        args: Value,
        ctx: Context<'_>,
    ) -> Result<Outcome, BrowserError> {
        let mut outcome = Outcome::default();
        let state = |view: &Self, audio: &AudioHost| {
            to_value(&view.state(audio, 0, ids::DEFAULT_ROW_LIMIT))
        };
        // The open folders and the selection belong to the step the browser
        // is at until the command moves it on (spec: browser-tags R29).
        let walks = matches!(id, ids::BACK | ids::FORWARD);
        if !walks {
            self.keep_step();
        }
        match id {
            ids::STATE => {
                let StateArgs { offset, limit } = parse(args)?;
                outcome.value = to_value(&self.state(
                    ctx.audio,
                    offset.unwrap_or(0),
                    limit.unwrap_or(ids::DEFAULT_ROW_LIMIT),
                ));
            }
            ids::SEARCH => {
                let SearchArgs { query, label } = parse(args)?;
                if let Some(label) = label {
                    self.check_label(&label)?;
                    self.label = label;
                }
                self.search = query;
                // The tags a `#` term names come first among the next
                // suggestions, most recent first (spec: browser-tags R15).
                let (_, searched, _) = self.search_terms();
                for tag in searched {
                    self.tags.note_searched(tag);
                }
                self.rebuild();
                // A `#` term that names no tag is searched as text, and
                // the Status Bar says so (spec: browser-tags R15).
                outcome.message = self.search_message();
                outcome.value = state(self, ctx.audio);
            }
            ids::FOCUS_SEARCH => {
                parse::<bl_commands::NoArgs>(args)?;
                self.label = Label::All;
                // Spec: browser-tags R22. The tags selected come off under
                // All; the search field's text is selected, not cleared.
                self.filter.clear();
                self.focus = Part::Search;
                self.focus_requests += 1;
                self.rebuild();
                outcome.show_browser = true;
                outcome.focus = true;
            }
            ids::FOCUS_BROWSER => {
                let FocusArgs { part } = parse(args)?;
                self.focus = part.unwrap_or(Part::Content);
                self.focus_requests += 1;
                outcome.show_browser = true;
                outcome.focus = true;
            }
            ids::OPEN_PLACE => {
                let OpenPlaceArgs { label } = parse(args)?;
                self.check_label(&label)?;
                if let Label::Saved(id) = label {
                    // Spec: browser-tags R24. Its words and tags are the
                    // search and the selected tags now.
                    self.choose_saved(id)?;
                } else {
                    self.label = label;
                    self.search.clear();
                }
                // A place is read afresh when it's chosen.
                if let Some(path) = self.place_path(&self.label) {
                    self.listings.remove(&path);
                }
                self.rebuild();
                outcome.value = state(self, ctx.audio);
            }
            ids::OPEN_FOLDER => {
                let OpenFolderArgs {
                    path,
                    open,
                    keep_others,
                } = parse(args)?;
                let path = if path.starts_with(PLUGIN_FOLDER) && self.is_plugin_row(&path) {
                    PathBuf::from(path)
                } else if self.is_set_row(&path) {
                    // Spec: set-exchange R1. A track of an opened set opens
                    // to list its Session clips or its tracks, and closes
                    // again; the other rows stay as they are.
                    let row = PathBuf::from(path);
                    if open.unwrap_or(!self.open.contains(&row)) {
                        self.open.insert(row);
                    } else {
                        self.open.remove(&row);
                    }
                    self.rebuild();
                    outcome.value = state(self, ctx.audio);
                    return Ok(outcome);
                } else {
                    let path = absolute(&path)?;
                    let is_set = path.is_file()
                        && path
                            .file_name()
                            .and_then(|n| file_kind(&n.to_string_lossy()))
                            == Some(ItemKind::Set);
                    if !path.is_dir() && !is_set {
                        return Err(BrowserError::InvalidArgs(format!(
                            "{} isn't a folder or a set",
                            path.to_string_lossy()
                        )));
                    }
                    path
                };
                let opening = open.unwrap_or(!self.open.contains(&path));
                if opening {
                    if !keep_others {
                        // Spec: browser R6. Only the folders it's inside stay.
                        self.open.retain(|p| path.starts_with(p));
                    }
                    self.listings.remove(&path);
                    // Spec: set-exchange R1. A set is read in the background.
                    if path.is_file() {
                        self.read_set(path.clone());
                    }
                    self.open.insert(path);
                } else {
                    self.open.retain(|p| !p.starts_with(&path));
                    self.open.remove(&path);
                }
                self.close_sets();
                self.rebuild();
                outcome.value = state(self, ctx.audio);
            }
            ids::SELECT => {
                let SelectArgs { paths, add } = parse(args)?;
                // A modulator under the Modulators label is named by its
                // type ID, and a Plug-Ins row by its key (plugin-scanning
                // R11); neither is a path.
                let is_device = |p: &str| device_by_id(p).is_some();
                let paths = paths
                    .iter()
                    .map(|p| {
                        if is_device(p) || self.is_plugin_row(p) || self.is_set_row(p) {
                            Ok(PathBuf::from(p))
                        } else {
                            absolute(p)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(missing) = paths.iter().find(|p| {
                    let name = p.to_string_lossy();
                    !p.exists()
                        && !is_device(&name)
                        && !self.is_plugin_row(&name)
                        && !self.is_set_row(&name)
                }) {
                    return Err(BrowserError::InvalidArgs(format!(
                        "{} doesn't exist",
                        missing.to_string_lossy()
                    )));
                }
                if !add {
                    self.selected.clear();
                }
                for path in paths {
                    if !self.selected.contains(&path) {
                        self.selected.push(path);
                    }
                }
                self.cursor = self.selected.last().and_then(|path| self.position_of(path));
                // Spec: browser R11. One audio file previews while the switch
                // is on; anything else stops the preview.
                let audio_files = self.selected_audio();
                let clip_files = self.selected_clip_files();
                if self.preview_on
                    && self.selected.len() == 1
                    && (audio_files.len() == 1 || clip_files.len() == 1)
                {
                    // Without a running device there's nothing to hear; the
                    // selection stands either way.
                    if ctx.audio.sample_rate().is_some() {
                        let _ = self.start_preview(
                            audio_files
                                .first()
                                .or(clip_files.first())
                                .expect("one file"),
                            ctx.audio,
                            ctx.search,
                            ctx.set_bpm,
                        );
                    }
                } else if self.preview_active() {
                    self.stop_preview(ctx.audio);
                }
                outcome.value = state(self, ctx.audio);
            }
            ids::SHOW_FILE => {
                // Spec: sample-editor R2. Show in Browser: the place the file
                // lies in, opened down to it, with the file selected.
                let ShowFileArgs { path, clip } = parse(args)?;
                let path = match (path, clip) {
                    (Some(path), None) => absolute(&path)?,
                    (None, Some(clip)) => {
                        let media = match ctx.editor.document().clip(clip).map(|c| &c.data) {
                            Some(bl_model::ClipData::Audio(audio)) => audio.media,
                            Some(_) => {
                                return Err(BrowserError::InvalidArgs(
                                    "the clip isn't an audio clip".to_owned(),
                                ));
                            }
                            None => {
                                return Err(BrowserError::InvalidArgs(format!(
                                    "no clip has the ID {}",
                                    clip.0
                                )));
                            }
                        };
                        ctx.media
                            .entry(bl_media::ContentHash::from_bytes(media.0))
                            .map(|entry| entry.path)
                            .ok_or_else(|| {
                                BrowserError::Unavailable(
                                    "the media store doesn't know the clip's file".to_owned(),
                                )
                            })?
                    }
                    _ => {
                        return Err(BrowserError::InvalidArgs(
                            "give `path` or `clip`, not both".to_owned(),
                        ));
                    }
                };
                if !path.exists() {
                    return Err(BrowserError::InvalidArgs(format!(
                        "{} isn't a file or folder",
                        path.to_string_lossy()
                    )));
                }
                self.reveal(&path, ctx.audio)?;
                outcome.show_browser = true;
                outcome.value = state(self, ctx.audio);
            }
            ids::ADD_FOLDER => {
                let FolderArgs { path } = parse(args)?;
                let path = absolute(&path)?;
                if !path.is_dir() {
                    return Err(BrowserError::InvalidArgs(format!(
                        "{} isn't a folder",
                        path.to_string_lossy()
                    )));
                }
                if !self.folders.contains(&path) {
                    self.folders.push(path.clone());
                    outcome.prefs_changed = true;
                }
                self.label = Label::Folder(path.to_string_lossy().into_owned());
                self.search.clear();
                self.places_changed();
                outcome.show_browser = true;
                outcome.value = state(self, ctx.audio);
            }
            ids::REMOVE_FOLDER => {
                let FolderArgs { path } = parse(args)?;
                let before = self.folders.len();
                self.folders
                    .retain(|f| f.to_string_lossy() != path.as_str());
                if self.folders.len() == before {
                    return Err(BrowserError::InvalidArgs(format!(
                        "{path} isn't a folder added to Places"
                    )));
                }
                outcome.prefs_changed = true;
                self.places_changed();
                outcome.value = state(self, ctx.audio);
            }
            ids::PREVIEW => {
                let PreviewArgs { path } = parse(args)?;
                let path = match path {
                    Some(path) => absolute(&path)?,
                    None => {
                        let files = self.selected_audio();
                        let clips = self.selected_clip_files();
                        match (files.as_slice(), clips.as_slice()) {
                            ([one], []) | ([], [one]) => one.clone(),
                            _ => {
                                return Err(BrowserError::Unavailable(
                                    "select one audio file or clip file to preview".to_owned(),
                                ));
                            }
                        }
                    }
                };
                self.start_preview(&path, ctx.audio, ctx.search, ctx.set_bpm)?;
                outcome.value = to_value(&self.preview_state(ctx.audio));
            }
            ids::STOP_PREVIEW => {
                parse::<bl_commands::NoArgs>(args)?;
                self.stop_preview(ctx.audio);
                outcome.value = to_value(&self.preview_state(ctx.audio));
            }
            ids::SET_PREVIEW => {
                let SetPreviewArgs { on, volume } = parse(args)?;
                let on = match (on, volume) {
                    (Some(on), _) => on,
                    (None, None) => !self.preview_on,
                    (None, Some(_)) => self.preview_on,
                };
                if let Some(volume) = volume {
                    if !volume.is_finite() {
                        return Err(BrowserError::InvalidArgs(
                            "`volume` must be a number".to_owned(),
                        ));
                    }
                    self.preview_volume = volume.clamp(0.0, MAX_PREVIEW_VOLUME);
                    ctx.audio.set_preview_gain(self.preview_volume as f32);
                }
                if self.preview_on && !on {
                    self.stop_preview(ctx.audio);
                }
                self.preview_on = on;
                outcome.prefs_changed = true;
                outcome.value = to_value(&self.preview_state(ctx.audio));
            }
            ids::SET_RAW => {
                let SetRawArgs { raw } = parse(args)?;
                let raw = raw.unwrap_or(!self.raw);
                self.raw = raw;
                outcome.prefs_changed = true;
                outcome.value = to_value(&self.preview_state(ctx.audio));
            }
            ids::SCRUB_PREVIEW => {
                let ScrubPreviewArgs { position, fraction } = parse(args)?;
                if !self.raw
                    && let Some(playing) = &self.playing
                    && let Some(map) = &playing.warp_map
                {
                    let seconds = match (position, fraction) {
                        (Some(p), _) => p,
                        (None, Some(f)) => f.clamp(0.0, 1.0) * playing.length_seconds,
                        (None, None) => {
                            return Err(BrowserError::InvalidArgs(
                                "scrub needs position or fraction".to_owned(),
                            ));
                        }
                    };
                    let beat = map.time_to_beat(seconds.max(0.0));
                    ctx.audio
                        .scrub_preview(beat)
                        .map_err(BrowserError::Failed)?;
                }
                outcome.value = to_value(&self.preview_state(ctx.audio));
            }
            ids::LOAD_SELECTED => {
                parse::<bl_commands::NoArgs>(args)?;
                let clips = self.selected_clip_files();
                let files = if clips.is_empty() {
                    self.selected_audio()
                } else {
                    clips
                };
                if files.is_empty() {
                    return Err(BrowserError::Unavailable(
                        "select an audio file or a clip file in the browser first".to_owned(),
                    ));
                }
                // Spec: browser R14, clip files R10. The selected track if it
                // is a track of the files' kind, otherwise a new one.
                let wanted = match listing::clip_kind(&files[0]) {
                    _ if listing::path_kind(&files[0]) != Some(ItemKind::Clip) => {
                        bl_model::TrackKind::Audio
                    }
                    Some(bl_shell_spec::browser::ClipFileKind::Midi) => bl_model::TrackKind::Midi,
                    _ => bl_model::TrackKind::Audio,
                };
                let track = ctx.selected_track.filter(|track| {
                    ctx.editor
                        .document()
                        .track(*track)
                        .is_some_and(|t| t.kind() == wanted)
                });
                let args = AddToArrangementArgs {
                    paths: files
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect(),
                    track,
                    at: Some(ctx.insert_marker),
                    spread: false,
                    lane: None,
                };
                self.add(&args, ctx, &mut outcome)?;
            }
            ids::ADD_TO_ARRANGEMENT => {
                let args: AddToArrangementArgs = parse(args)?;
                self.add(&args, ctx, &mut outcome)?;
            }
            ids::SAVE_CLIP_FILE => {
                let SaveClipFileArgs {
                    clip,
                    folder,
                    name,
                    copy,
                } = parse(args)?;
                let library = self.user_library.clone();
                let folder = match folder {
                    Some(folder) => absolute(&folder)?,
                    None => library
                        .clone()
                        .ok_or_else(|| {
                            BrowserError::Unavailable(
                                "the User Library hasn't been set up".to_owned(),
                            )
                        })?
                        .join("Clips"),
                };
                if folder.exists() && !folder.is_dir() {
                    return Err(BrowserError::InvalidArgs(format!(
                        "{} isn't a folder",
                        folder.to_string_lossy()
                    )));
                }
                let doc = ctx.editor.document();
                let copy = match (ctx.collect_files, copy) {
                    (CollectFiles::Always, _) => true,
                    (CollectFiles::Never, _) => false,
                    (CollectFiles::Ask, Some(copy)) => copy,
                    (CollectFiles::Ask, None) => {
                        if clip_files::uses_outside(
                            doc,
                            ctx.media,
                            clip,
                            &folder,
                            library.as_deref(),
                        ) {
                            return Err(BrowserError::Unavailable(
                                "Collect Files on Export is set to Ask: ask the person whether to \
                                 copy the audio files, then run the command again with `copy` \
                                 set to true or false"
                                    .to_owned(),
                            ));
                        }
                        false
                    }
                };
                let saved = clip_files::save(
                    doc,
                    ctx.media,
                    clip,
                    &folder,
                    name.as_deref(),
                    copy,
                    library.as_deref(),
                )
                .map_err(BrowserError::Failed)?;
                // Its row is selected with the name open for editing, if the
                // folder is in a place the browser shows (clip files R4).
                let shown = self.file_saved(&saved.path, ctx.audio);
                outcome.show_browser = shown;
                outcome.message = Some(format!("Saved clip file {}.", saved.name));
                outcome.value = to_value(&SaveClipFileResult {
                    path: saved.path.to_string_lossy().into_owned(),
                    name: saved.name,
                    kind: match saved.kind {
                        bl_project::clip_file::ClipKind::Audio => ClipFileKind::Audio,
                        bl_project::clip_file::ClipKind::Midi => ClipFileKind::Midi,
                    },
                    copied: saved.copied,
                });
            }
            ids::RENAME_FILE => {
                let RenameFileArgs { path, name } = parse(args)?;
                let target = match path {
                    Some(path) => absolute(&path)?,
                    None => self.renaming.clone().ok_or_else(|| {
                        BrowserError::Unavailable("no file's name is open for editing".to_owned())
                    })?,
                };
                if self.renaming.as_ref() == Some(&target) {
                    self.renaming = None;
                }
                if let Some(name) = name {
                    let new = self.renamed(&target, &name)?;
                    self.files_changed();
                    let _ = self.reveal(&new, ctx.audio);
                }
                outcome.value = state(self, ctx.audio);
            }
            ids::TAGS => {
                let TagsArgs { items } = parse(args)?;
                let items = items.unwrap_or_default();
                outcome.value = to_value(&self.tags_state(&items));
            }
            ids::TAG_ITEMS => {
                self.reload_tags();
                let TagItemsArgs {
                    items,
                    tags,
                    attach,
                } = parse(args)?;
                let attach = attach.unwrap_or(true);
                let mut wanted: Vec<i64> = Vec::new();
                for name in &tags {
                    let found = self.tags.find(name);
                    if found.is_empty() {
                        return Err(BrowserError::InvalidArgs(format!(
                            "there's no tag called {name}"
                        )));
                    }
                    wanted.extend(found);
                }
                // Beatlayer's own tags and a folder's can't be taken off
                // (spec: browser-tags R11). Every item is checked before
                // any is changed, so a refused command changes nothing.
                if !attach {
                    for item in &items {
                        let key = self.tag_key(item);
                        for &tag in &wanted {
                            let fixed = self
                                .carried_tags(&key)
                                .iter()
                                .any(|(id, source)| *id == tag && *source != Source::Own);
                            if fixed {
                                return Err(BrowserError::Unavailable(format!(
                                    "{} can't be taken off {}: Beatlayer gives it, or it comes \
                                     from a folder",
                                    self.tags.full_name(tag),
                                    key
                                )));
                            }
                        }
                    }
                }
                let mut ops = Vec::new();
                for item in &items {
                    let key = self.tag_key(item);
                    let mut carried = self.tags.item_tags(&key);
                    for &tag in &wanted {
                        if attach {
                            if !carried.contains(&tag) {
                                carried.push(tag);
                            }
                        } else {
                            carried.retain(|&id| id != tag);
                        }
                    }
                    carried.sort_unstable();
                    ops.extend(self.tags.set_item_tags(&key, carried));
                }
                self.save_tags(ops);
                self.hash_tagged_files(&items);
                outcome.value = to_value(&self.tags_state(&items));
                self.rebuild();
            }
            ids::ADD_TAG => {
                self.reload_tags();
                let AddTagArgs {
                    name,
                    group,
                    parent,
                } = parse(args)?;
                let parent_id = match &parent {
                    Some(parent) => Some(self.tag_id(parent)?),
                    None => None,
                };
                let group_id = match &group {
                    Some(group) => self.tags.group_id(group).ok_or_else(|| {
                        BrowserError::InvalidArgs(format!("there's no group called {group}"))
                    })?,
                    None => match parent_id {
                        Some(parent) => {
                            self.tags.tag(parent).map(|tag| tag.group).ok_or_else(|| {
                                BrowserError::InvalidArgs("there's no such tag".to_owned())
                            })?
                        }
                        None => self
                            .tags
                            .groups()
                            .first()
                            .map(|group| group.id)
                            .ok_or_else(|| {
                                BrowserError::Unavailable("there are no tag groups".to_owned())
                            })?,
                    },
                };
                let (_, ops) = self
                    .tags
                    .add_tag(&name, group_id, parent_id)
                    .map_err(BrowserError::InvalidArgs)?;
                self.save_tags(ops);
                outcome.value = to_value(&self.tags_state(&[]));
            }
            ids::ADD_TAG_GROUP => {
                self.reload_tags();
                let AddTagGroupArgs { name } = parse(args)?;
                let (_, ops) = self
                    .tags
                    .add_group(&name)
                    .map_err(BrowserError::InvalidArgs)?;
                self.save_tags(ops);
                outcome.value = to_value(&self.tags_state(&[]));
            }
            ids::RENAME_TAG => {
                self.reload_tags();
                let RenameTagArgs { tag, group, name } = parse(args)?;
                let ops = match (tag, group) {
                    (Some(tag), None) => {
                        let id = self.tag_id(&tag)?;
                        self.tags
                            .rename_tag(id, &name)
                            .map_err(BrowserError::InvalidArgs)?
                    }
                    (None, Some(group)) => {
                        let id = self.tags.group_id(&group).ok_or_else(|| {
                            BrowserError::InvalidArgs(format!("there's no group called {group}"))
                        })?;
                        self.tags
                            .rename_group(id, &name)
                            .map_err(BrowserError::InvalidArgs)?
                    }
                    _ => {
                        return Err(BrowserError::InvalidArgs(
                            "give `tag` or `group`, not both".to_owned(),
                        ));
                    }
                };
                self.save_tags(ops);
                outcome.value = to_value(&self.tags_state(&[]));
            }
            ids::MOVE_TAG => {
                self.reload_tags();
                let MoveTagArgs {
                    tag,
                    group,
                    position,
                } = parse(args)?;
                let ops = match (tag, group) {
                    (Some(tag), None) => {
                        let id = self.tag_id(&tag)?;
                        self.tags
                            .move_tag(id, position)
                            .map_err(BrowserError::InvalidArgs)?
                    }
                    (None, Some(group)) => {
                        let id = self.tags.group_id(&group).ok_or_else(|| {
                            BrowserError::InvalidArgs(format!("there's no group called {group}"))
                        })?;
                        self.tags
                            .move_group(id, position)
                            .map_err(BrowserError::InvalidArgs)?
                    }
                    _ => {
                        return Err(BrowserError::InvalidArgs(
                            "give `tag` or `group`, not both".to_owned(),
                        ));
                    }
                };
                self.save_tags(ops);
                outcome.value = to_value(&self.tags_state(&[]));
            }
            ids::RESET_TAG_ORDER => {
                self.reload_tags();
                parse::<bl_commands::NoArgs>(args)?;
                let ops = self.tags.reset_order();
                self.save_tags(ops);
                outcome.value = to_value(&self.tags_state(&[]));
            }
            ids::DELETE_TAG => {
                self.reload_tags();
                let DeleteTagArgs {
                    tag,
                    group,
                    confirm,
                } = parse(args)?;
                // Spec: browser-tags R8. A saved label that uses what is
                // deleted is named first, and then leaves it out.
                let doomed = self.doomed_tags(tag.as_deref(), group.as_deref())?;
                let using = self.labels_using(&doomed);
                if !using.is_empty() && !confirm {
                    return Err(BrowserError::Unavailable(format!(
                        "the saved labels {} use it. Ask the person, then run the command again \
                         with `confirm` true: the labels then leave it out",
                        using.join(", ")
                    )));
                }
                let ops = match (tag, group) {
                    (Some(tag), None) => {
                        let id = self.tag_id(&tag)?;
                        self.tags
                            .delete_tag(id)
                            .map_err(BrowserError::InvalidArgs)?
                    }
                    (None, Some(group)) => {
                        let id = self.tags.group_id(&group).ok_or_else(|| {
                            BrowserError::InvalidArgs(format!("there's no group called {group}"))
                        })?;
                        self.tags
                            .delete_group(id)
                            .map_err(BrowserError::InvalidArgs)?
                    }
                    _ => unreachable!("checked above"),
                };
                self.save_tags(ops);
                self.forget_gone_tags();
                outcome.value = to_value(&self.tags_state(&[]));
                self.rebuild();
            }
            ids::SHOW_TAG_EDITOR => {
                let ShowTagEditorArgs { show } = parse(args)?;
                self.tag_editor = show.unwrap_or(!self.tag_editor);
                outcome.prefs_changed = true;
                outcome.value = json!({ "tag_editor": self.tag_editor });
            }
            ids::SHOW_QUICK_TAGS => {
                let ShowQuickTagsArgs { show } = parse(args)?;
                self.quick_tags = show.unwrap_or(!self.quick_tags);
                outcome.prefs_changed = true;
                outcome.value = json!({ "quick_tags": self.quick_tags });
            }
            ids::SET_FILTER_VIEW => {
                let SetFilterViewArgs { show } = parse(args)?;
                self.set_filter_view(show);
                outcome.prefs_changed = true;
                outcome.value = to_value(&self.filter_view());
            }
            ids::SET_FILTER_GROUPS => {
                let args: SetFilterGroupsArgs = parse(args)?;
                self.set_filter_groups(args)?;
                self.rebuild();
                outcome.prefs_changed = true;
                outcome.value = to_value(&self.filter_view());
            }
            ids::FILTER => {
                let FilterArgs { tags, add } = parse(args)?;
                self.select_filter_tags(&tags, add)?;
                self.rebuild();
                outcome.value = state(self, ctx.audio);
            }
            ids::CLEAR_FILTERS => {
                let ClearFiltersArgs { words, tags } = parse(args)?;
                if words.unwrap_or(true) {
                    self.search.clear();
                }
                if tags.unwrap_or(true) {
                    self.filter.clear();
                }
                self.rebuild();
                outcome.value = state(self, ctx.audio);
            }
            ids::ADD_LABEL => {
                parse::<bl_commands::NoArgs>(args)?;
                self.add_label()?;
                outcome.value = state(self, ctx.audio);
            }
            ids::RENAME_LABEL => {
                let args: RenameLabelArgs = parse(args)?;
                outcome.prefs_changed = self.rename_label(args)?;
                outcome.value = state(self, ctx.audio);
            }
            ids::SET_LABEL_ICON => {
                let args: SetLabelIconArgs = parse(args)?;
                outcome.prefs_changed = self.set_label_icon(args)?;
                outcome.value = state(self, ctx.audio);
            }
            ids::SET_LABEL_SHOWN => {
                let args: SetLabelShownArgs = parse(args)?;
                self.set_label_shown(args)?;
                outcome.prefs_changed = true;
                outcome.value = state(self, ctx.audio);
            }
            ids::MOVE_LABEL => {
                let args: MoveLabelArgs = parse(args)?;
                self.move_label(args)?;
                outcome.prefs_changed = true;
                outcome.value = state(self, ctx.audio);
            }
            ids::RESET_LABEL_ORDER => {
                parse::<bl_commands::NoArgs>(args)?;
                self.reset_label_order();
                outcome.prefs_changed = true;
                outcome.value = state(self, ctx.audio);
            }
            ids::REMOVE_LABEL => {
                let args: RemoveLabelArgs = parse(args)?;
                self.remove_label(args)?;
                outcome.prefs_changed = true;
                outcome.value = state(self, ctx.audio);
            }
            ids::SET_COLLECTION => {
                let args: SetCollectionArgs = parse(args)?;
                outcome.prefs_changed = self.set_collection(args)?;
                outcome.value = state(self, ctx.audio);
            }
            ids::CLEAR_COLLECTIONS => {
                let args: ClearCollectionsArgs = parse(args)?;
                self.clear_collections(args)?;
                outcome.value = state(self, ctx.audio);
            }
            ids::REVEAL => {
                let args: RevealArgs = parse(args)?;
                outcome.file_manager = self.reveal_targets(args)?;
                outcome.value = state(self, ctx.audio);
            }
            ids::SET_COLUMNS => {
                let args: SetColumnsArgs = parse(args)?;
                self.set_columns(args)?;
                self.rebuild();
                outcome.prefs_changed = true;
                outcome.value = state(self, ctx.audio);
            }
            ids::BACK | ids::FORWARD => {
                parse::<bl_commands::NoArgs>(args)?;
                self.go_step(id == ids::BACK)?;
                outcome.value = state(self, ctx.audio);
            }
            _ => return Err(BrowserError::InvalidArgs(format!("no command is `{id}`"))),
        }
        if !walks {
            self.record_step();
        }
        Ok(outcome)
    }

    /// The tags deleting a tag (and its subtags) or a group (and its tags)
    /// deletes.
    fn doomed_tags(
        &self,
        tag: Option<&str>,
        group: Option<&str>,
    ) -> Result<Vec<i64>, BrowserError> {
        match (tag, group) {
            (Some(tag), None) => {
                let id = self.tag_id(tag)?;
                let mut doomed = vec![id];
                doomed.extend(self.tags.subtags(id).iter().map(|sub| sub.id));
                Ok(doomed)
            }
            (None, Some(group)) => {
                let id = self.tags.group_id(group).ok_or_else(|| {
                    BrowserError::InvalidArgs(format!("there's no group called {group}"))
                })?;
                Ok(self
                    .tags
                    .all_tags()
                    .iter()
                    .filter(|t| t.group == id)
                    .map(|t| t.id)
                    .collect())
            }
            _ => Err(BrowserError::InvalidArgs(
                "give `tag` or `group`, not both".to_owned(),
            )),
        }
    }

    /// The names of the saved labels that use a tag or a group's tags,
    /// which deleting it asks about first (spec: browser-tags R8).
    #[must_use]
    pub fn labels_using_tag(&self, tag: Option<&str>, group: Option<&str>) -> Vec<String> {
        self.doomed_tags(tag, group)
            .map(|doomed| self.labels_using(&doomed))
            .unwrap_or_default()
    }

    /// The tag a name names: its full name, or its own name.
    fn tag_id(&self, name: &str) -> Result<i64, BrowserError> {
        self.tags
            .find(name)
            .first()
            .copied()
            .ok_or_else(|| BrowserError::InvalidArgs(format!("there's no tag called {name}")))
    }

    /// Keeps the size and content hash of a file that carries a tag, so
    /// its tags pass to the same content found at a new path (spec:
    /// browser-tags R5).
    fn hash_tagged_files(&mut self, items: &[String]) {
        for item in items {
            let key = self.tag_key(item);
            let path = PathBuf::from(&key);
            if !path.is_file() {
                continue;
            }
            let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            let known = self
                .tagged_files
                .get(&key)
                .is_some_and(|tagged| tagged.size == size && !tagged.hash.is_empty());
            if !known {
                self.tagged_files.insert(
                    key.clone(),
                    TaggedFile {
                        size,
                        hash: self
                            .tagged_files
                            .get(&key)
                            .map(|tagged| tagged.hash.clone())
                            .unwrap_or_default(),
                    },
                );
            }
            if self.pending_hashes.insert(key.clone())
                && self
                    .tagged_files
                    .get(&key)
                    .is_some_and(|tagged| tagged.hash.is_empty())
            {
                self.hasher.request(&path);
            }
        }
    }

    /// A file's hash has come back: keep it, and give a tagged file that
    /// moved outside Beatlayer its tags (spec: browser-tags R5).
    fn hash_ready(&mut self, hashed: Hashed) {
        let path = hashed.path.to_string_lossy().into_owned();
        self.pending_hashes.remove(&path);
        let Ok(hash) = hashed.hash else {
            return;
        };
        if let Some(tagged) = self.tagged_files.get_mut(&path)
            && tagged.hash.is_empty()
        {
            tagged.hash = hash.clone();
            if let Some(db) = &self.db {
                db.save_files(vec![FileOp::Hash {
                    path: path.clone(),
                    hash: hash.clone(),
                }]);
            }
        }
        self.rehome(&path, &hash);
    }

    /// The tags of a file that moved outside Beatlayer pass to the file
    /// with the same content (spec: browser-tags R5).
    fn rehome(&mut self, path: &str, hash: &str) {
        let orphans: Vec<String> = self
            .tagged_files
            .iter()
            .filter(|(old, tagged)| {
                tagged.hash == hash && *old != path && !self.saved_files.contains_key(*old)
            })
            .map(|(old, _)| old.clone())
            .collect();
        for old in orphans {
            let tags = self.tags.item_tags(&old);
            let collected = self.collections.of(&old);
            if tags.is_empty() && collected.is_empty() {
                continue;
            }
            if !tags.is_empty() {
                let mut ops = self.tags.set_item_tags(path, tags);
                ops.extend(self.tags.set_item_tags(&old, Vec::new()));
                self.save_tags(ops);
            }
            // Spec: browser-tags R5. Collections pass with the tags.
            if !collected.is_empty() {
                let mut kept: BTreeSet<u8> = self.collections.of(path).into_iter().collect();
                kept.extend(collected);
                let ops = [
                    self.collections.set(path, kept),
                    self.collections.set(&old, BTreeSet::new()),
                ];
                self.save_collections(ops.into_iter().flatten().collect());
            }
            if let Some(tagged) = self.tagged_files.remove(&old) {
                self.tagged_files.insert(path.to_owned(), tagged);
            }
        }
    }

    /// Writes the index to the database, the files that changed only
    /// (spec: browser-tags R1).
    fn save_index(&mut self) {
        let Some(db) = &self.db else {
            return;
        };
        let rows = self.index.snapshot().file_rows();
        let mut ops = Vec::new();
        let mut saved = HashMap::with_capacity(rows.len());
        for row in rows {
            let path = row.path.clone();
            saved.insert(path.clone(), row.clone());
            let unchanged = self.saved_files.get(&path).is_some_and(|old| {
                old.size == row.size
                    && old.modified == row.modified
                    && old.kind == row.kind
                    && old.place == row.place
            });
            if !unchanged {
                ops.push(FileOp::Upsert(row));
            }
        }
        for path in self.saved_files.keys() {
            if !saved.contains_key(path) {
                ops.push(FileOp::Delete(path.clone()));
            }
        }
        self.saved_files = saved;
        db.save_files(ops);
        // A tagged file that isn't there any more: its tags pass to the
        // same content, if it turns up (spec: browser-tags R5).
        let orphans: Vec<(String, u64)> = self
            .tagged_files
            .iter()
            .filter(|(old, tagged)| !tagged.hash.is_empty() && !self.saved_files.contains_key(*old))
            .map(|(old, tagged)| (old.clone(), tagged.size))
            .collect();
        for (_, size) in orphans {
            for index in self.index.snapshot().with_size(size) {
                let path = self.index.snapshot().path(index);
                if self
                    .pending_hashes
                    .insert(path.to_string_lossy().into_owned())
                {
                    self.hasher.request(&path);
                }
            }
        }
    }

    /// Shows `path` in the place it lies in, opened down to it, and selects
    /// it (Show in Browser; a saved clip file's new row).
    fn reveal(&mut self, path: &Path, audio: &AudioHost) -> Result<(), BrowserError> {
        let Some((label, root)) = self
            .roots()
            .into_iter()
            .find(|(_, root)| path.starts_with(root))
        else {
            return Err(BrowserError::Unavailable(format!(
                "{} lies in none of the browser's places: add its folder with \
                 `browser.add_folder` first",
                path.to_string_lossy()
            )));
        };
        self.label = label;
        self.search.clear();
        self.filter.clear();
        for folder in path
            .ancestors()
            .skip(1)
            .take_while(|folder| *folder != root && folder.starts_with(&root))
        {
            self.listings.remove(folder);
            self.open.insert(folder.to_path_buf());
        }
        self.rebuild();
        self.selected = vec![path.to_path_buf()];
        self.cursor = self.position_of(path);
        if self.preview_active() {
            self.stop_preview(audio);
        }
        Ok(())
    }

    fn add(
        &mut self,
        args: &AddToArrangementArgs,
        ctx: Context<'_>,
        outcome: &mut Outcome,
    ) -> Result<(), BrowserError> {
        let clip_files = args.paths.iter().any(|p| arrange::is_clip_file(p));
        let look = ctx.look;
        // Spec: themes R19, R20. The new tracks and clips take their colours
        // in the step that makes them.
        let (result, media) = ctx
            .editor
            .transact(arrange::UNDO_LABEL, |editor| {
                let (result, media) = if clip_files {
                    let (result, media, missing) = arrange::add_clip_files(
                        editor,
                        ctx.media,
                        ctx.insert_marker,
                        ctx.search,
                        args,
                    )?;
                    outcome.message = arrange::missing_message(missing);
                    (result, media)
                } else {
                    arrange::add_files(
                        editor,
                        ctx.media,
                        ctx.insert_marker,
                        ctx.edge_fades,
                        ctx.warping,
                        args,
                    )?
                };
                let clips: Vec<_> = result.clips.iter().map(|clip| clip.clip).collect();
                crate::colours::colour_new(editor, look, &result.created_tracks, &clips)
                    .map_err(|error| AddError::Failed(error.to_string()))?;
                Ok((result, media))
            })
            .map_err(|error| match error {
                AddError::InvalidArgs(message) => BrowserError::InvalidArgs(message),
                AddError::Failed(message) => BrowserError::Failed(message),
            })?;
        outcome.value = to_value(&result);
        outcome.media = media;
        outcome.document_changed = true;
        Ok(())
    }

    fn check_label(&self, label: &Label) -> Result<(), BrowserError> {
        if let Label::Collection(number) = label
            && !label.is_collection()
        {
            return Err(BrowserError::InvalidArgs(format!(
                "{number} isn't a collection: they are 1 (Favorites) to 7 (Gray)"
            )));
        }
        if let Label::Saved(id) = label
            && !self.saved.iter().any(|saved| saved.id == *id)
        {
            return Err(BrowserError::InvalidArgs(format!(
                "no saved label has the ID {id}"
            )));
        }
        if let Label::Folder(path) = label
            && !self
                .folders
                .iter()
                .any(|f| f.to_string_lossy() == path.as_str())
        {
            return Err(BrowserError::InvalidArgs(format!(
                "{path} isn't a folder in Places; add it with `browser.add_folder` first"
            )));
        }
        Ok(())
    }
}
