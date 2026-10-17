//! The browser without a window (WP-1.10): the sidebar's labels and places,
//! the content pane's rows, the search, the selection, the preview, and the
//! commands that change them. The UI (`bl-ui`) draws from it; people,
//! agents, and tests change it through the commands in
//! `bl_shell_spec::browser`, which [`BrowserView::run`] carries out.
//!
//! - [`listing`]: which files a folder shows, and in what order.
//! - [`index`]: the background index that search reads.
//! - [`places`]: the User Library's place and starting folders.
//! - [`preview`]: decoding previews in the background.
//! - [`arrange`]: putting files into the Arrangement.
//! - [`drop`]: where a drag from the browser lands in the Arrangement.
//!
//! The behaviour is in the browser spec.

pub mod arrange;
pub mod drop;
pub mod index;
pub mod listing;
pub mod places;
pub mod preview;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use bl_core::time::Ticks;
use bl_media::MediaStore;
use bl_model::{Editor, TrackId};
pub use bl_shell_spec::browser::Part;
use bl_shell_spec::browser::{
    self as ids, AddToArrangementArgs, BrowserState, FocusArgs, FolderArgs, IndexState, Item,
    ItemKind, Label, LabelInfo, MAX_PREVIEW_VOLUME, OpenFolderArgs, OpenPlaceArgs, PreviewArgs,
    PreviewState, SearchArgs, Section, SelectArgs, SetPreviewArgs, StateArgs,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::arrange::{AddError, MediaFile};
use self::index::{Index, Scope, Snapshot};
use self::listing::{Entry, file_kind};
use self::preview::{Loader, MediaSamples};
use crate::audio::AudioHost;

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
}

impl Default for BrowserPrefs {
    fn default() -> Self {
        Self {
            user_library: None,
            folders: Vec::new(),
            preview: true,
            preview_volume: 1.0,
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
}

/// What [`BrowserView::poll`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Polled {
    /// Whether anything the browser shows changed.
    pub changed: bool,
    /// A preview that couldn't be decoded, for the Status Bar.
    pub error: Option<String>,
}

/// A preview the audio thread was given.
#[derive(Clone, Debug)]
struct Playing {
    generation: u64,
    path: PathBuf,
    /// Whether the audio thread has reported playing it.
    started: bool,
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
    preview_volume: f64,
    loading: Option<(u64, PathBuf)>,
    playing: Option<Playing>,
    preview_rate: u32,
    /// The waveform overview of the file previewed last, for the preview
    /// strip.
    overview: Option<(PathBuf, Arc<[[f32; 2]]>)>,
    focus: Part,
    focus_requests: u64,
    waker: Waker,
    /// The plug-ins the scanner found (WP-2.9), already filtered by the
    /// hidden and format preferences; the Plug-Ins label lists them.
    plugins: Vec<bl_plugin_host::scan::ScannedPlugin>,
}

/// Beatlayer's modulator devices, as the Modulators label lists them: each
/// with its type ID and name.
fn modulator_devices() -> Vec<(String, String)> {
    bl_devices::device_types()
        .into_iter()
        .filter(|t| bl_model::modulator::is_modulator_kind(&t.kind))
        .filter_map(|t| match t.kind {
            bl_model::DeviceKind::Native { device_type } => Some((device_type, t.name)),
            bl_model::DeviceKind::Plugin { .. } => None,
        })
        .collect()
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
            preview_volume: prefs.preview_volume.clamp(0.0, MAX_PREVIEW_VOLUME),
            loading: None,
            playing: None,
            preview_rate: 48_000,
            overview: None,
            focus: Part::Content,
            focus_requests: 0,
            waker,
            plugins: Vec::new(),
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
    pub fn set_plugins(&mut self, plugins: Vec<bl_plugin_host::scan::ScannedPlugin>) {
        self.plugins = plugins;
        if self.label == Label::PlugIns {
            self.rebuild();
        }
    }

    /// The preferences to keep.
    #[must_use]
    pub fn prefs(&self, user_library: Option<PathBuf>) -> BrowserPrefs {
        BrowserPrefs {
            user_library,
            folders: self.folders.clone(),
            preview: self.preview_on,
            preview_volume: self.preview_volume,
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

    /// The sidebar's Library labels and places, in order (spec: browser R2).
    #[must_use]
    pub fn labels(&self) -> Vec<LabelInfo> {
        let mut labels: Vec<LabelInfo> = Label::LIBRARY
            .into_iter()
            .map(|label| LabelInfo {
                available: label.pending().is_none(),
                name: label.name(),
                section: Section::Library,
                path: None,
                label,
            })
            .collect();
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
                label,
            });
        }
        labels
    }

    /// The places changed: index them again and list the label afresh.
    fn places_changed(&mut self) {
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
        self.rebuild();
    }

    /// The scope a label searches, and whether only audio files count; or
    /// `None` for a Library label without content yet.
    fn scope(&self, label: &Label) -> Option<(Scope, bool)> {
        match label {
            Label::All => Some((Scope::All, false)),
            Label::Samples => Some((Scope::All, true)),
            place if place.is_place() => {
                let path = self.place_path(place)?;
                let index = self
                    .index
                    .snapshot()
                    .roots()
                    .iter()
                    .position(|root| *root == path)?;
                Some((Scope::Place(u16::try_from(index).ok()?), false))
            }
            _ => None,
        }
    }

    /// A folder's listing, read now if it isn't known.
    fn listing(&mut self, folder: &Path) -> Option<Vec<Entry>> {
        if let Some(entries) = self.listings.get(folder) {
            return Some(entries.clone());
        }
        let entries = listing::list(folder).ok()?;
        self.listings.insert(folder.to_path_buf(), entries.clone());
        Some(entries)
    }

    /// Adds a folder's items, and its open folders' items under them.
    fn list_into(&mut self, folder: &Path, depth: u16, out: &mut Vec<Item>) {
        let Some(entries) = self.listing(folder) else {
            return;
        };
        for entry in entries {
            let open = entry.kind == ItemKind::Folder && self.open.contains(&entry.path);
            out.push(Item {
                path: entry.path.to_string_lossy().into_owned(),
                name: entry.name.clone(),
                kind: entry.kind,
                depth,
                open,
                folder: None,
            });
            if open {
                self.list_into(&entry.path, depth.saturating_add(1), out);
            }
        }
    }

    /// Works out the content pane's rows again, keeping the selection.
    fn rebuild(&mut self) {
        self.note = None;
        let label = self.label.clone();
        if label == Label::Modulators {
            // Spec: modulation R12. The Modulators label lists Beatlayer's
            // modulator devices.
            let query = self.search.trim().to_ascii_lowercase();
            let items: Vec<Item> = modulator_devices()
                .into_iter()
                .filter(|(_, name)| query.is_empty() || name.to_ascii_lowercase().contains(&query))
                .map(|(id, name)| Item {
                    path: id,
                    name,
                    kind: ItemKind::Device,
                    depth: 0,
                    open: false,
                    folder: None,
                })
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
            // Spec: plugin-scanning R7. The Plug-Ins label lists what the
            // scanner found, hidden plug-ins already filtered out.
            let query = self.search.trim().to_ascii_lowercase();
            let items: Vec<Item> = self
                .plugins
                .iter()
                .filter(|p| query.is_empty() || p.name.to_ascii_lowercase().contains(&query))
                .map(|p| Item {
                    path: format!("{}#{}", p.path.to_string_lossy(), p.plugin_id),
                    name: p.name.clone(),
                    kind: ItemKind::Plugin,
                    depth: 0,
                    open: false,
                    folder: None,
                })
                .collect();
            if items.is_empty() {
                self.note = Some(if query.is_empty() {
                    "No plug-ins found yet. Rescan to look for the plug-ins on this machine."
                        .to_owned()
                } else {
                    format!("Nothing matches \u{201c}{}\u{201d}.", self.search.trim())
                });
            }
            self.rows = Rows::Items(items);
            self.cursor = self.selected.last().and_then(|path| self.position_of(path));
            return;
        }
        if !self.search.trim().is_empty() || label == Label::Samples {
            match self.scope(&label) {
                Some((scope, audio_only)) => {
                    let snapshot = Arc::clone(self.index.snapshot());
                    let hits = snapshot.search(&self.search, scope, audio_only);
                    if hits.is_empty() {
                        self.note = Some(if self.index.indexing() {
                            "Indexing the places\u{2026}".to_owned()
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
        match &self.rows {
            Rows::Items(items) => items.get(index).cloned(),
            Rows::Hits { snapshot, hits } => {
                let hit = *hits.get(index)?;
                let entry = snapshot.entry(hit);
                Some(Item {
                    path: snapshot.path(hit).to_string_lossy().into_owned(),
                    name: entry.name.to_string(),
                    kind: entry.kind,
                    depth: 0,
                    open: false,
                    folder: Some(snapshot.folder_label(hit)),
                })
            }
        }
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
            volume: self.preview_volume,
            loading: self
                .loading
                .as_ref()
                .map(|(_, path)| path.to_string_lossy().into_owned()),
            playing: playing.map(|p| p.path.to_string_lossy().into_owned()),
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
        }
    }

    /// Takes background work's news: a newer index, a decoded preview, a
    /// preview that finished. The host calls it every frame and when woken.
    pub fn poll(&mut self, audio: &mut AudioHost) -> Polled {
        let mut polled = Polled::default();
        if self.index.poll() {
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
                Ok(data) => match audio.play_preview(Box::new(MediaSamples(data))) {
                    Ok(generation) => {
                        self.playing = Some(Playing {
                            generation,
                            path: loaded.path,
                            started: false,
                        });
                    }
                    Err(error) => polled.error = Some(format!("Couldn't preview: {error}")),
                },
                Err(error) => {
                    polled.error = Some(format!(
                        "Couldn't preview {}: {error}",
                        loaded.path.to_string_lossy()
                    ));
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
        self.loading.is_some() || self.playing.is_some()
    }

    fn start_preview(&mut self, path: &Path, audio: &mut AudioHost) -> Result<(), BrowserError> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        if name.as_deref().and_then(file_kind) != Some(ItemKind::Audio) || !path.is_file() {
            return Err(BrowserError::InvalidArgs(format!(
                "{} isn't an audio file Beatlayer opens",
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
        let id = self
            .loader
            .request(path, rate)
            .map_err(BrowserError::Failed)?;
        self.loading = Some((id, path.to_path_buf()));
        self.preview_rate = rate;
        Ok(())
    }

    fn stop_preview(&mut self, audio: &AudioHost) {
        audio.stop_preview();
        self.loading = None;
        self.playing = None;
    }

    /// The modulator device selected under the Modulators label, as its type
    /// ID and name.
    #[must_use]
    pub fn selected_device(&self) -> Option<(String, String)> {
        let selected = self.selected.last()?.to_string_lossy().into_owned();
        modulator_devices()
            .into_iter()
            .find(|(id, _)| *id == selected)
    }

    /// Whether no audio file is selected.
    #[must_use]
    pub fn selected_audio_is_empty(&self) -> bool {
        self.selected_audio().is_empty()
    }

    /// The selected audio files, in the content pane's order.
    fn selected_audio(&self) -> Vec<PathBuf> {
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

    /// Whether a person can run browser command `id` with `args` now.
    #[must_use]
    pub fn can_run(&self, id: &str, args: &Value) -> bool {
        match id {
            ids::LOAD_SELECTED => {
                !self.selected_audio().is_empty() || self.selected_device().is_some()
            }
            ids::PREVIEW => args.get("path").is_some() || self.selected_audio().len() == 1,
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
                self.rebuild();
                outcome.value = state(self, ctx.audio);
            }
            ids::FOCUS_SEARCH => {
                parse::<bl_commands::NoArgs>(args)?;
                self.label = Label::All;
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
                self.label = label;
                self.search.clear();
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
                let path = absolute(&path)?;
                if !path.is_dir() {
                    return Err(BrowserError::InvalidArgs(format!(
                        "{} isn't a folder",
                        path.to_string_lossy()
                    )));
                }
                let opening = open.unwrap_or(!self.open.contains(&path));
                if opening {
                    if !keep_others {
                        // Spec: browser R6. Only the folders it's inside stay.
                        self.open.retain(|p| path.starts_with(p));
                    }
                    self.listings.remove(&path);
                    self.open.insert(path);
                } else {
                    self.open.retain(|p| !p.starts_with(&path));
                }
                self.rebuild();
                outcome.value = state(self, ctx.audio);
            }
            ids::SELECT => {
                let SelectArgs { paths, add } = parse(args)?;
                // A modulator under the Modulators label is named by its
                // type ID, which isn't a path.
                let is_device = |p: &str| modulator_devices().iter().any(|(id, _)| id == p);
                let paths = paths
                    .iter()
                    .map(|p| {
                        if is_device(p) {
                            Ok(PathBuf::from(p))
                        } else {
                            absolute(p)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(missing) = paths
                    .iter()
                    .find(|p| !p.exists() && !is_device(&p.to_string_lossy()))
                {
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
                if self.preview_on && self.selected.len() == 1 && audio_files.len() == 1 {
                    // Without a running device there's nothing to hear; the
                    // selection stands either way.
                    if ctx.audio.sample_rate().is_some() {
                        let _ = self.start_preview(&audio_files[0], ctx.audio);
                    }
                } else if self.preview_active() {
                    self.stop_preview(ctx.audio);
                }
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
                        match files.as_slice() {
                            [one] => one.clone(),
                            _ => {
                                return Err(BrowserError::Unavailable(
                                    "select one audio file to preview".to_owned(),
                                ));
                            }
                        }
                    }
                };
                self.start_preview(&path, ctx.audio)?;
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
            ids::LOAD_SELECTED => {
                parse::<bl_commands::NoArgs>(args)?;
                let files = self.selected_audio();
                if files.is_empty() {
                    return Err(BrowserError::Unavailable(
                        "select an audio file in the browser first".to_owned(),
                    ));
                }
                // Spec: browser R14. The selected track if it holds audio,
                // otherwise a new one.
                let track = ctx.selected_track.filter(|track| {
                    ctx.editor
                        .document()
                        .track(*track)
                        .is_some_and(|t| t.kind() == bl_model::TrackKind::Audio)
                });
                let args = AddToArrangementArgs {
                    paths: files
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect(),
                    track,
                    at: Some(ctx.insert_marker),
                    spread: false,
                };
                self.add(&args, ctx, &mut outcome)?;
            }
            ids::ADD_TO_ARRANGEMENT => {
                let args: AddToArrangementArgs = parse(args)?;
                self.add(&args, ctx, &mut outcome)?;
            }
            _ => return Err(BrowserError::InvalidArgs(format!("no command is `{id}`"))),
        }
        Ok(outcome)
    }

    fn add(
        &mut self,
        args: &AddToArrangementArgs,
        ctx: Context<'_>,
        outcome: &mut Outcome,
    ) -> Result<(), BrowserError> {
        let (result, media) = arrange::add_files(
            ctx.editor,
            ctx.media,
            ctx.insert_marker,
            ctx.edge_fades,
            args,
        )
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
