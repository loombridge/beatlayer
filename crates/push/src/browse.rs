//! Browse Mode: the browser's lists in columns across the display, Load,
//! Preview and Add Track (`docs/specs/push-2-tracks.md`, R5 to R14).
//!
//! The lists come from the read model's catalogue and are fetched when the
//! catalogue's revision changes or the person moves, never waited for
//! (R13). What the mode shows is surface state: it changes no set and
//! makes no undo step until Load (R2).

use bl_control::browse::{Browse as Lists, Entry, Load};
use bl_control::{Cx, ReadModel};
use bl_model::{DeviceId, DeviceRole, TrackKind};
use serde_json::{Value, json};

use crate::content::{Content, Ctx, Enter, Gesture, Paint, Turn};
use crate::controls::{self, cc};
use crate::encoder::{Counting, LIST_STEPS};
use crate::screen::{Label, List};

/// The gesture key of the commands one Load runs, so they are one undo
/// step (R1, R12).
const LOAD_GESTURE: u32 = 0x5100;
/// The colour of the mode's labels.
const WHITE: [u8; 3] = [230, 230, 230];
/// The kinds Add Track offers, with the role of what each can hold (R12).
const KINDS: [(&str, &str, Option<DeviceRole>); 3] = [
    ("midi", "MIDI", None),
    ("audio", "Audio", Some(DeviceRole::AudioEffect)),
    ("return", "Return", Some(DeviceRole::AudioEffect)),
];

/// What Load does (R5, R12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadKind {
    /// Adds to the selected track.
    Add,
    /// Replaces this device.
    Replace(DeviceId),
    /// Adds a track, with the device on it when one is chosen.
    AddTrack,
}

/// What `push.state` reads of Browse Mode (R14).
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    /// What Load does.
    pub load: LoadKind,
    /// The columns.
    pub columns: Vec<ColumnState>,
    /// The column in use.
    pub in_use: usize,
}

/// One column as `push.state` reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnState {
    /// Its entries' names.
    pub entries: Vec<String>,
    /// The selected entry.
    pub selected: Option<usize>,
}

#[derive(Default)]
struct Col {
    entries: Vec<Entry>,
    selected: Option<usize>,
}

impl Col {
    fn entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected?)
    }
}

/// What Load acts on, worked out from the selected track and device (R5).
#[derive(Clone, Copy, PartialEq, Eq)]
struct Target {
    replace: Option<DeviceId>,
    role: Option<DeviceRole>,
    /// The instrument at the top of the selected track's chain, which an
    /// instrument Add Device loads replaces (R25).
    instrument: Option<DeviceId>,
}

impl Target {
    /// What Load acts on: with `adding`, Add Device's target, which adds
    /// after the selected device whatever it is (R25).
    fn of(model: &ReadModel, adding: bool) -> Self {
        let doc = &model.document;
        let instrument = model
            .selection
            .tracks
            .first()
            .and_then(|&track| doc.track(track))
            .and_then(|track| track.devices().find(|d| d.role == DeviceRole::Instrument))
            .map(|d| d.id);
        let selected = model
            .selected_device
            .filter(|_| !adding)
            .and_then(|id| doc.device(id).map(|device| (id, device.role)));
        if let Some((id, role)) = selected {
            return Self {
                replace: Some(id),
                role: Some(role),
                instrument,
            };
        }
        // An audio, group or return track takes audio effects only; a MIDI
        // track, or no track, anything (R5).
        let midi = model
            .selection
            .tracks
            .first()
            .and_then(|&track| doc.track(track))
            .is_none_or(|track| track.kind() == TrackKind::Midi);
        Self {
            replace: None,
            role: (!midi).then_some(DeviceRole::AudioEffect),
            instrument,
        }
    }
}

/// Browse Mode's content.
pub struct Browse {
    add_track: bool,
    add_device: bool,
    target: Option<Target>,
    columns: Vec<Col>,
    in_use: usize,
    seen: Option<u64>,
    counts: [Counting; 8],
    previewing: bool,
}

impl Default for Browse {
    fn default() -> Self {
        Self::new()
    }
}

impl Browse {
    /// Browse Mode before it is entered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            add_track: false,
            add_device: false,
            target: None,
            columns: Vec::new(),
            in_use: 0,
            seen: None,
            counts: Default::default(),
            previewing: false,
        }
    }

    /// The role of what the lists offer.
    fn role(&self) -> Option<DeviceRole> {
        if self.add_track {
            let kind = self.columns.first().and_then(Col::entry)?;
            KINDS.iter().find(|k| k.0 == kind.key).and_then(|k| k.2)
        } else {
            self.target.and_then(|t| t.role)
        }
    }

    /// What `above` holds: the entries of the column after it, or the
    /// first column when there is nothing above (R6).
    fn list(&self, lists: &Lists, above: Option<&Entry>) -> Vec<Entry> {
        let role = self.role();
        match above {
            None if self.add_track => KINDS
                .iter()
                .map(|(key, name, _)| Entry {
                    key: (*key).to_owned(),
                    name: (*name).to_owned(),
                    load: None,
                    folder: true,
                })
                .collect(),
            None => lists.catalogue.labels(role),
            Some(entry) if self.add_track && KINDS.iter().any(|k| k.0 == entry.key) => {
                lists.catalogue.labels(role)
            }
            Some(entry) if entry.folder => lists.catalogue.children(&entry.key, role),
            Some(_) => Vec::new(),
        }
    }

    /// Fetches every column again, keeping the selections that are still
    /// there.
    fn rebuild(&mut self, lists: &Lists) {
        let keys: Vec<Option<String>> = self
            .columns
            .iter()
            .map(|c| c.entry().map(|e| e.key.clone()))
            .collect();
        let mut columns: Vec<Col> = Vec::new();
        let mut above: Option<Entry> = None;
        for depth in 0..keys.len().max(1) + 1 {
            // The role of the later columns depends on the first's choice.
            self.columns = std::mem::take(&mut columns);
            let entries = self.list(lists, above.as_ref());
            columns = std::mem::take(&mut self.columns);
            let selected = match keys.get(depth) {
                Some(Some(key)) => entries.iter().position(|e| e.key == *key),
                Some(None) => None,
                None => (depth == 0 && !entries.is_empty()).then_some(0),
            };
            let next = selected
                .and_then(|at| entries.get(at))
                .filter(|e| e.folder)
                .cloned();
            columns.push(Col { entries, selected });
            match next {
                Some(entry) => above = Some(entry),
                None => break,
            }
        }
        self.in_use = self.in_use.min(columns.len() - 1);
        self.columns = columns;
    }

    /// Selects entry `at` of column `depth`: the columns right of it are
    /// fetched again (R7).
    fn select(&mut self, lists: &Lists, depth: usize, at: usize, ctx: &mut Ctx<'_>) {
        let Some(col) = self.columns.get_mut(depth) else {
            return;
        };
        col.selected = Some(at);
        self.columns.truncate(depth + 1);
        self.in_use = depth;
        let entry = self.columns[depth].entry().cloned();
        if let Some(entry) = entry {
            if entry.folder {
                let entries = self.list(lists, Some(&entry));
                self.columns.push(Col {
                    entries,
                    selected: None,
                });
            }
            self.preview(lists, &entry, ctx.cx);
        }
    }

    /// Previews a preset when Preview is on, and stops what was playing
    /// when the selection is anything else (R10).
    fn preview(&mut self, lists: &Lists, entry: &Entry, cx: &mut Cx) {
        match &entry.load {
            Some(Load::Preset { path, .. }) if lists.preview => {
                cx.run("browser.preview", json!({ "path": path }));
                self.previewing = true;
            }
            _ if self.previewing => {
                cx.run("browser.stop_preview", json!({}));
                self.previewing = false;
            }
            _ => {}
        }
    }

    /// Moves the selection of column `depth` by `by` entries (R7).
    fn step(&mut self, lists: &Lists, depth: usize, by: i32, ctx: &mut Ctx<'_>) {
        let Some(col) = self.columns.get(depth) else {
            return;
        };
        if col.entries.is_empty() {
            return;
        }
        let last = col.entries.len() - 1;
        // Nothing selected counts as the place before the first entry.
        let base = col
            .selected
            .map_or(-1, |at| isize::try_from(at).unwrap_or(0));
        let at =
            (base + isize::try_from(by).unwrap_or(0)).clamp(0, isize::try_from(last).unwrap_or(0));
        let at = usize::try_from(at).unwrap_or(0);
        if col.selected != Some(at) {
            self.select(lists, depth, at, ctx);
        } else {
            self.in_use = depth;
        }
    }

    /// Makes the column to the right the one in use, selecting its first
    /// entry (R7, R8).
    fn open(&mut self, lists: &Lists, ctx: &mut Ctx<'_>) {
        let next = self.in_use + 1;
        if self
            .columns
            .get(next)
            .is_some_and(|c| !c.entries.is_empty())
        {
            let at = self.columns[next].selected.unwrap_or(0);
            self.select(lists, next, at, ctx);
        }
    }

    /// The first column the display shows: the column in use and the one
    /// after it always show (R6).
    fn first(&self) -> usize {
        (self.in_use + 2).saturating_sub(8)
    }

    /// The entry Load loads, if one is chosen (R11).
    fn chosen(&self) -> Option<&Entry> {
        self.columns.get(self.in_use).and_then(Col::entry)
    }

    /// The instrument the chosen entry replaces, when Add Device is on an
    /// instrument and the track has one (R25).
    fn replaced_instrument(&self) -> Option<DeviceId> {
        let instrument = self.target.and_then(|t| t.instrument)?;
        let chosen = self.chosen()?.load.as_ref()?;
        (self.add_device && chosen.role() == Some(DeviceRole::Instrument)).then_some(instrument)
    }

    fn load_kind(&self) -> LoadKind {
        match (self.add_track, self.target.and_then(|t| t.replace)) {
            (true, _) => LoadKind::AddTrack,
            (false, Some(device)) => LoadKind::Replace(device),
            (false, None) => self
                .replaced_instrument()
                .map_or(LoadKind::Add, LoadKind::Replace),
        }
    }

    /// The commands loading `load` runs onto what Load acts on (R5, R9).
    fn commands(&self, load: &Load) -> (String, Value) {
        let replace = if self.add_track {
            None
        } else {
            self.target
                .and_then(|t| t.replace)
                .or_else(|| self.replaced_instrument())
        };
        match (load, replace) {
            (Load::Device { new_device, .. }, Some(device)) => (
                "device.replace".to_owned(),
                json!({ "device": device, "new_device": new_device }),
            ),
            (Load::Device { new_device, .. }, None) => {
                ("device.add".to_owned(), json!({ "device": new_device }))
            }
            (Load::Preset { path, .. }, Some(device)) => (
                "device.replace".to_owned(),
                json!({ "device": device, "preset": path }),
            ),
            (Load::Preset { path, .. }, None) => {
                ("device.add_preset".to_owned(), json!({ "path": path }))
            }
            // A plug-in is added, as the window's browser adds it.
            (Load::Plugin { path, plugin, .. }, _) => (
                "plugin.load".to_owned(),
                json!({ "path": path, "plugin": plugin }),
            ),
        }
    }

    /// Load (R11, R12).
    fn load(&mut self, ctx: &mut Ctx<'_>) {
        let Some(entry) = self.chosen().cloned() else {
            return;
        };
        if self.add_track {
            let kind = self
                .columns
                .first()
                .and_then(Col::entry)
                .map(|e| e.key.clone());
            let Some(kind) = kind else {
                return;
            };
            match &entry.load {
                None if entry.key == kind => ctx.cx.run("track.create", json!({ "kind": kind })),
                Some(load) => {
                    let (id, args) = self.commands(load);
                    ctx.cx
                        .run_held(LOAD_GESTURE, "track.create", json!({ "kind": kind }));
                    ctx.cx.run_released(LOAD_GESTURE, &id, args);
                }
                None => return,
            }
            // The new track is selected: the next Load acts on it.
            self.add_track = false;
            self.columns.remove(0);
            self.in_use = self.in_use.saturating_sub(1);
            self.target = None;
            return;
        }
        if let Some(load) = &entry.load {
            let (id, args) = self.commands(load);
            ctx.cx.run(&id, args);
        }
    }

    fn lists(ctx: &Ctx<'_>) -> Option<Lists> {
        ctx.model.and_then(|m| m.browse.clone())
    }
}

impl Content for Browse {
    fn name(&self) -> &str {
        "Browse"
    }

    fn enter(&mut self, why: Enter, ctx: &mut Ctx<'_>) {
        self.add_track = why == Enter::AddTrack;
        self.add_device = why == Enter::AddDevice;
        self.target = ctx.model.map(|model| Target::of(model, self.add_device));
        self.columns.clear();
        self.in_use = 0;
        self.counts.iter_mut().for_each(Counting::reset);
        match Self::lists(ctx) {
            Some(lists) => {
                self.seen = Some(lists.revision);
                self.rebuild(&lists);
            }
            None => self.seen = None,
        }
    }

    fn button(&mut self, button: u8, gesture: Gesture, ctx: &mut Ctx<'_>) {
        if gesture != Gesture::Press {
            return;
        }
        match button {
            cc::LEFT => self.in_use = self.in_use.saturating_sub(1),
            cc::RIGHT => {
                if let Some(lists) = Self::lists(ctx) {
                    self.open(&lists, ctx);
                }
            }
            cc::UP | cc::DOWN => {
                if let Some(lists) = Self::lists(ctx) {
                    let by = if button == cc::UP { -1 } else { 1 };
                    self.step(&lists, self.in_use, by, ctx);
                }
            }
            _ => match controls::upper_column(button) {
                // The seventh goes up the hierarchy and the eighth down (R8).
                Some(6) => self.in_use = self.in_use.saturating_sub(1),
                Some(7) => {
                    if let Some(lists) = Self::lists(ctx) {
                        self.open(&lists, ctx);
                    }
                }
                _ => match controls::lower_column(button) {
                    Some(6) => ctx.cx.run("browser.set_preview", json!({})),
                    Some(7) => self.load(ctx),
                    _ => {}
                },
            },
        }
    }

    fn encoder(&mut self, column: u8, turn: Turn, ctx: &mut Ctx<'_>) {
        let counts = &mut self.counts[usize::from(column)];
        match turn {
            Turn::Touch => counts.reset(),
            Turn::Release => {}
            Turn::Steps(steps) => {
                let by = counts.take(steps, LIST_STEPS);
                if by != 0
                    && let Some(lists) = Self::lists(ctx)
                {
                    let depth = self.first() + usize::from(column);
                    self.step(&lists, depth, by, ctx);
                }
            }
        }
    }

    fn paint(&mut self, paint: &mut Paint<'_>) {
        let target = Target::of(paint.model, self.add_device);
        let Some(lists) = &paint.model.browse else {
            return;
        };
        // What Load acts on follows the window (R3); the lists are fetched
        // again when it changes what is offered or the catalogue changes.
        let offered = self.target.map(|t| t.role) != Some(target.role);
        if !self.add_track {
            self.target = Some(target);
        }
        if self.seen != Some(lists.revision) || (offered && !self.add_track) {
            self.seen = Some(lists.revision);
            self.rebuild(lists);
        }
        let first = self.first();
        for slot in 0..8 {
            if let Some(col) = self.columns.get(first + slot) {
                paint.screen.lists[slot] = Some(List {
                    rows: col
                        .entries
                        .iter()
                        .map(|e| (e.name.clone(), e.folder))
                        .collect(),
                    selected: col.selected,
                    in_use: first + slot == self.in_use,
                });
            }
        }
        // A kind chosen in Add Track's first column loads an empty track (R12).
        let loadable = self
            .chosen()
            .is_some_and(|e| e.load.is_some() || (self.add_track && self.in_use == 0));
        paint.screen.upper[6] = Label::new("Up", WHITE, false);
        paint.screen.upper[7] = Label::new("Down", WHITE, false);
        paint.screen.lower[6] = Label::new("Preview", WHITE, lists.preview);
        if loadable {
            paint.screen.lower[7] = Label::new("Load", WHITE, false);
        }
    }

    fn claims_lower(&self) -> bool {
        true
    }

    fn browse_state(&self) -> Option<State> {
        Some(State {
            load: self.load_kind(),
            columns: self
                .columns
                .iter()
                .map(|c| ColumnState {
                    entries: c.entries.iter().map(|e| e.name.clone()).collect(),
                    selected: c.selected,
                })
                .collect(),
            in_use: self.in_use,
        })
    }
}
