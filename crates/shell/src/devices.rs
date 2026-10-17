//! The Device View's state and commands (WP-2.4; the device chains spec):
//! which device is selected, which are folded, the device clipboard, and
//! hot-swapping, and the `device.*` commands that run on the shell (adding
//! a device as a person does, cut, copy, paste, duplicate, and presets);
//! and where floating devices' windows sit (WP-2.5b, IMP-14), which is
//! saved with the set but never an undo step.
//!
//! None of this is part of the set: selection, folding, the clipboard, and
//! hot-swapping are the view's, so they're never undo steps. The edits the
//! commands make go through the document's own commands, each command one
//! undo step named as Live 12.4.6 names it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bl_engine::DeviceType;
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Command, Device, DeviceDescription, DeviceId, DeviceKind, DeviceLayout, DeviceRole,
    DeviceWindows, Document, Editor, LayerId, MultisamplerView, MultisamplerViews, ParamId,
    TrackId, TrackKind,
};
use bl_project::preset::{self, Preset};
use bl_shell_spec::arrangement as view_ids;
use bl_shell_spec::devices::{
    self as ids, AddDeviceArgs, DeviceAdded, DeviceArgs, DeviceFrame, DevicePanel,
    DeviceParameters, DeviceViewState, FloatingWindow, FloatingWindows, FoldDeviceArgs, HotSwap,
    HotSwapArgs, LoadPresetArgs, ParameterState, PasteDeviceArgs, PresetResult, SavePresetArgs,
    SelectDeviceArgs, SetWindowArgs, ShowSidechainArgs, SidechainSection,
};
use bl_shell_spec::multisampler::MultisamplerState;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::arrangement::ArrangementView;

/// What the Device View shows besides the set: the selected device, the
/// folded devices, the clipboard, and hot-swapping; and the device types
/// the host offers, whose descriptions the generic UI draws (WP-2.5).
#[derive(Clone, Debug, Default)]
pub struct DeviceView {
    selected: Option<DeviceId>,
    folded: BTreeSet<DeviceId>,
    clipboard: Option<Device>,
    hot_swap: Option<DeviceId>,
    /// Devices whose sidechain section is shown (WP-2.19b).
    sidechains_shown: BTreeSet<DeviceId>,
    types: Vec<DeviceType>,
    /// Where floating devices' windows sit (WP-2.5b). A frame outlives
    /// its device's docking and deletion, so undoing either puts the
    /// window back where it was; a save keeps only those of devices the
    /// set has.
    windows: DeviceWindows,
    /// Which parameter each knob of a plug-in device's panel shows
    /// (WP-2.10, D19), by `format/plugin_id`: absent entries keep the
    /// plug-in's own parameter at that place. Synced from the preferences.
    plugin_panels: std::collections::BTreeMap<String, std::collections::BTreeMap<usize, ParamId>>,
    /// What each multisampler's views show (WP-5.13): saved with the set,
    /// never an undo step.
    multisamplers: MultisamplerViews,
    /// Each multisampler's selected layers, in the layer list (R11).
    selections: std::collections::BTreeMap<DeviceId, Vec<LayerId>>,
    /// With Auto Select on, the layers each held key plays (R7): while a
    /// note is held they are the selection.
    held: std::collections::BTreeMap<DeviceId, std::collections::BTreeMap<u8, Vec<LayerId>>>,
}

/// Why a device command didn't run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceError {
    /// The arguments don't fit.
    InvalidArgs(String),
    /// It can't run now, and why.
    Unavailable(String),
    /// It ran and failed, and why; nothing changed.
    Failed(String),
}

/// What a device command did, for the shell to carry out.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// The command's result.
    pub value: Value,
    /// Whether the set changed.
    pub document_changed: bool,
    /// A line for the Status Bar.
    pub message: Option<String>,
    /// Folders for the browser to open, outermost first, with the User
    /// Library's label chosen: hot-swapping's way to the device's presets.
    pub browse: Option<Vec<PathBuf>>,
}

/// What a device command needs from the rest of the shell.
pub struct Context<'a> {
    /// The set.
    pub editor: &'a mut Editor,
    /// The Arrangement View, whose first selected track is the one the
    /// Device View shows.
    pub arrangement: &'a mut ArrangementView,
    /// The User Library, where presets go.
    pub user_library: Option<&'a Path>,
    /// The interface zoom, as a factor: a floating window's smallest size
    /// in points on the screen is its size at 100% times this.
    pub zoom: f32,
    /// Whether Auto-Hide Plug-In Windows is on.
    pub auto_hide: bool,
    /// Whether View ▸ Plug-In Windows shows the windows.
    pub windows_shown: bool,
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, DeviceError> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| DeviceError::InvalidArgs(e.to_string()))
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// The track the Device View shows: the first selected track.
#[must_use]
pub fn shown_track(arrangement: &ArrangementView, doc: &Document) -> Option<TrackId> {
    arrangement
        .selected_tracks()
        .iter()
        .copied()
        .find(|track| doc.track(*track).is_some())
}

/// Where a new device with `role` goes in `track`'s chain when no index is
/// given: straight after `after` if that keeps the chain in order, or else
/// at the end of its role's place (the device chains spec, R12).
#[must_use]
pub fn default_index(
    doc: &Document,
    track: TrackId,
    role: DeviceRole,
    after: Option<DeviceId>,
) -> usize {
    let Some(track) = doc.track(track) else {
        return 0;
    };
    let roles: Vec<DeviceRole> = track.devices().map(|d| d.role).collect();
    let fits = |index: usize| {
        roles[..index].iter().all(|r| *r <= role) && roles[index..].iter().all(|r| *r >= role)
    };
    if let Some(index) = after.and_then(|d| track.device_position(d)).map(|i| i + 1)
        && fits(index)
    {
        return index;
    }
    roles.iter().take_while(|r| **r <= role).count()
}

impl DeviceView {
    /// Nothing selected, folded, copied, or hot-swapped.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The selected device.
    #[must_use]
    pub fn selected(&self) -> Option<DeviceId> {
        self.selected
    }

    /// Selects `device` without changing which track the Device View
    /// shows, as bringing its floating window to the front does (WP-2.5b).
    pub fn select_only(&mut self, device: DeviceId) {
        self.selected = Some(device);
    }

    /// Whether a device is folded.
    #[must_use]
    pub fn is_folded(&self, device: DeviceId) -> bool {
        self.folded.contains(&device)
    }

    /// The device being hot-swapped.
    #[must_use]
    pub fn hot_swap(&self) -> Option<DeviceId> {
        self.hot_swap
    }

    /// Whether the clipboard holds a device.
    #[must_use]
    pub fn can_paste(&self) -> bool {
        self.clipboard.is_some()
    }

    /// Sets the device types the host's engine makes, with their
    /// descriptions (WP-2.5).
    pub fn set_device_types(&mut self, types: Vec<DeviceType>) {
        self.types = types;
    }

    /// The description of devices of `kind`, if their type has one.
    #[must_use]
    pub fn description(&self, kind: &DeviceKind) -> Option<&DeviceDescription> {
        self.types
            .iter()
            .find(|t| t.kind == *kind)
            .map(|t| &t.description)
            .filter(|d| !d.params.is_empty())
    }

    /// What the generic UI draws for `device`, if its type is described.
    /// A plug-in device gets the plug-in panel (WP-2.10, D19): up to
    /// [`PANEL_KNOBS`](bl_shell_spec::plugins::PANEL_KNOBS) knobs, mapped to
    /// the plug-in's first parameters or to the preferences' remap, with
    /// the rest of the body an open-editor target.
    fn panel(&self, device: &Device) -> Option<DevicePanel> {
        let description = self.description(&device.kind)?;
        if let Some(key) = Self::plugin_key(&device.kind) {
            return Some(self.plugin_panel(description, &device.params, &key));
        }
        Some(DevicePanel {
            values: description
                .params
                .iter()
                .map(|p| description.value(&device.params, p.id).unwrap_or(p.default))
                .collect(),
            description: description.clone(),
            plugin: false,
        })
    }

    /// A plug-in device's panel (WP-2.10, D19): the plug-in's first
    /// parameters as knobs, in the preferences' remap where one is set.
    fn plugin_panel(
        &self,
        description: &DeviceDescription,
        values: &bl_model::OrdMap<ParamId, f64>,
        key: &str,
    ) -> DevicePanel {
        let remap = self.plugin_panels.get(key);
        let mapped = |index: usize| -> Option<&bl_model::ParamDescriptor> {
            remap
                .and_then(|m| m.get(&index))
                .and_then(|id| description.param(*id))
                .or_else(|| description.params.get(index))
        };
        let selected: Vec<bl_model::ParamDescriptor> = (0..bl_shell_spec::plugins::PANEL_KNOBS)
            .filter_map(mapped)
            .cloned()
            .collect();
        let controls: Vec<(ParamId, Widget)> =
            selected.iter().map(|p| (p.id, Widget::Knob)).collect();
        let layout = DeviceLayout::new(LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &controls,
        )));
        DevicePanel {
            values: selected
                .iter()
                .map(|p| description.value(values, p.id).unwrap_or(p.default))
                .collect(),
            description: DeviceDescription {
                params: selected,
                layout,
            },
            plugin: true,
        }
    }

    /// `device`'s parameters as `device.parameters` reports them.
    fn parameters(&self, device: &Device) -> DeviceParameters {
        let params = self
            .description(&device.kind)
            .map(|description| {
                description
                    .params
                    .iter()
                    .map(|p| {
                        let value = description.value(&device.params, p.id).unwrap_or(p.default);
                        ParameterState {
                            descriptor: p.clone(),
                            value,
                            text: p.format(value),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        DeviceParameters {
            device: device.id,
            params,
        }
    }

    /// `params` as a device of `kind` takes them: clamped to its parameters'
    /// ranges, unknown ones left out, if its type is described.
    fn sanitize(
        &self,
        kind: &DeviceKind,
        params: bl_model::OrdMap<bl_model::ParamId, f64>,
    ) -> bl_model::OrdMap<bl_model::ParamId, f64> {
        match self.description(kind) {
            Some(description) => description.sanitize(&params),
            None => params,
        }
    }

    /// Forgets devices the set no longer has.
    pub fn forget_missing(&mut self, doc: &Document) {
        let exists = |d: &DeviceId| doc.device(*d).is_some();
        self.selected = self.selected.filter(exists);
        self.hot_swap = self.hot_swap.filter(exists);
        self.folded.retain(exists);
        self.sidechains_shown.retain(exists);
        self.selections.retain(|device, _| exists(device));
        self.held.retain(|device, _| exists(device));
    }

    /// Whether a device's sidechain section is shown.
    #[must_use]
    pub fn is_sidechain_shown(&self, device: DeviceId) -> bool {
        self.sidechains_shown.contains(&device)
    }

    /// What the Device View shows for `track`.
    #[must_use]
    pub fn state(&self, doc: &Document, track: Option<TrackId>) -> DeviceViewState {
        let shown = track.and_then(|id| doc.track(id));
        let devices: Vec<DeviceFrame> = shown
            .map(|t| t.devices().map(|d| self.frame(doc, d)).collect())
            .unwrap_or_default();
        DeviceViewState {
            track: shown.map(bl_model::Track::id),
            track_kind: shown.map(bl_model::Track::kind),
            selected: devices.iter().find(|d| d.selected).map(|d| d.id),
            devices,
            hot_swap: self.hot_swap.and_then(|device| {
                doc.device(device).map(|d| HotSwap {
                    device,
                    label: format!("Swapping {}", d.role.label()),
                })
            }),
            can_paste: self.clipboard.is_some(),
        }
    }

    /// Where floating devices' windows sit, for the next save.
    #[must_use]
    pub fn windows(&self) -> &DeviceWindows {
        &self.windows
    }

    /// Takes the window frames an opened set saved, or none for a new set.
    pub fn set_windows(&mut self, windows: DeviceWindows) {
        self.windows = windows;
    }

    /// Where a device's window sits, if one has been placed: a floating
    /// device's (WP-2.5b) or a plug-in device's editor window (WP-2.10).
    #[must_use]
    pub fn window_frame(&self, device: DeviceId) -> Option<bl_model::WindowFrame> {
        self.windows.get(&device).copied()
    }

    /// Records where a plug-in device's editor window sits (`plugin.set_window`,
    /// WP-2.10): the same frame store as a floating device's, so a save
    /// keeps it and it's never an undo step.
    pub fn set_editor_frame(&mut self, device: DeviceId, frame: bl_model::WindowFrame) {
        self.windows.insert(device, frame);
    }

    /// The parameter mapping a plug-in device's panel knobs use, from the
    /// preferences: absent entries keep the plug-in's own parameter at that
    /// place (WP-2.10, D19).
    pub fn set_plugin_panels(
        &mut self,
        panels: std::collections::BTreeMap<String, std::collections::BTreeMap<usize, ParamId>>,
    ) {
        self.plugin_panels = panels;
    }

    /// The preferences key of a plug-in device, `format/plugin_id`
    /// (WP-2.10, D19), or `None` for a device that isn't a plug-in.
    #[must_use]
    pub fn plugin_key(kind: &DeviceKind) -> Option<String> {
        match kind {
            DeviceKind::Plugin { format, plugin_id } => Some(format!(
                "{}/{}",
                bl_plugin_host::scan::format_name(*format),
                plugin_id
            )),
            _ => None,
        }
    }

    /// Every floating device's window, on any track, in the order of the
    /// tracks and their chains (`device.windows`). None shows while
    /// `all_shown` is off (View ▸ Plug-In Windows); with `auto_hide` on,
    /// only the windows of `shown`'s devices show (Auto-Hide Plug-In
    /// Windows; the device chains spec, R31 and R33).
    #[must_use]
    pub fn floating_windows(
        &self,
        doc: &Document,
        shown: Option<TrackId>,
        auto_hide: bool,
        all_shown: bool,
    ) -> FloatingWindows {
        let windows = doc
            .track_order()
            .into_iter()
            .filter_map(|id| doc.track(id))
            .flat_map(|track| track.devices().map(move |d| (track.id(), d)))
            .filter(|(_, d)| d.floating)
            .map(|(track, d)| {
                let mut view = self.frame(doc, d);
                // The window shows the device whole, whatever its strip.
                view.folded = false;
                let (min_width, min_height) = ids::window_min_size(&view);
                let (grows_across, grows_down) = ids::window_grows(&view);
                FloatingWindow {
                    device: d.id,
                    track,
                    title: format!("{}/{}", d.title(), crate::routing::track_label(doc, track)),
                    frame: self.windows.get(&d.id).copied(),
                    min_width,
                    min_height,
                    grows_across,
                    grows_down,
                    view,
                    shown: all_shown && (!auto_hide || shown == Some(track)),
                }
            })
            .collect();
        FloatingWindows { windows }
    }

    /// Whether `device` is a multisampler (WP-5.13).
    #[must_use]
    pub fn is_multisampler(device: &Device) -> bool {
        matches!(
            &device.kind,
            DeviceKind::Native { device_type } if device_type == bl_devices::multisampler::MULTISAMPLER
        )
    }

    /// What a multisampler's views show: a new device's until a command
    /// changes them.
    #[must_use]
    pub fn multisampler_view(&self, device: DeviceId) -> MultisamplerView {
        self.multisamplers.get(&device).copied().unwrap_or_default()
    }

    /// Changes what a multisampler's views show.
    pub fn set_multisampler_view(&mut self, device: DeviceId, view: MultisamplerView) {
        if view == MultisamplerView::default() {
            self.multisamplers.remove(&device);
        } else {
            self.multisamplers.insert(device, view);
        }
    }

    /// Every multisampler's views, for the next save.
    #[must_use]
    pub fn multisampler_views(&self) -> &MultisamplerViews {
        &self.multisamplers
    }

    /// Takes the views an opened set saved, or none for a new set.
    pub fn set_multisampler_views(&mut self, views: MultisamplerViews) {
        self.multisamplers = views;
        self.selections.clear();
        self.held.clear();
    }

    /// A multisampler's selected layers that are still in it, in list
    /// order: the layers of the notes held while Auto Select is on, else the
    /// ones picked in the list.
    #[must_use]
    pub fn layer_selection(&self, doc: &Document, device: DeviceId) -> Vec<LayerId> {
        doc.device(device)
            .map_or_else(Vec::new, |d| self.layer_selection_of(d))
    }

    /// Picks a multisampler's layers in the list.
    pub fn select_layers(&mut self, device: DeviceId, layers: Vec<LayerId>) {
        self.selections.insert(device, layers);
    }

    /// A key went down or up on the computer keyboard: with Auto Select on,
    /// a multisampler on `track` selects the layers the note plays for as
    /// long as it's held (R7). Plays nothing in the set.
    pub fn note(&mut self, doc: &Document, track: TrackId, key: u8, velocity: u8, down: bool) {
        let Some(track) = doc.track(track) else {
            return;
        };
        for device in track.devices().filter(|d| Self::is_multisampler(d)) {
            let held = self.held.entry(device.id).or_default();
            if !down {
                held.remove(&key);
            } else if self
                .multisamplers
                .get(&device.id)
                .is_some_and(|v| v.auto_select)
            {
                let selector = device
                    .params
                    .get(&bl_devices::multisampler::SELECTOR)
                    .map_or(0, |v| *v as u8);
                held.insert(key, device.layers.matching(key, velocity, selector));
            }
        }
    }

    /// A multisampler's views and selection, as `device_view.state` reports
    /// them.
    fn multisampler_state(&self, d: &Device) -> Option<MultisamplerState> {
        Self::is_multisampler(d).then(|| MultisamplerState {
            view: self.multisampler_view(d.id),
            selection: self.layer_selection_of(d),
            layers: d.layers.list.len(),
            hot_swap: None,
        })
    }

    /// As [`layer_selection`](Self::layer_selection), for a device already
    /// in hand.
    fn layer_selection_of(&self, d: &Device) -> Vec<LayerId> {
        let held: std::collections::BTreeSet<LayerId> = self
            .held
            .get(&d.id)
            .into_iter()
            .flat_map(|keys| keys.values().flatten().copied())
            .collect();
        let picked = self.selections.get(&d.id);
        d.layers
            .list
            .iter()
            .map(|l| l.id)
            .filter(|id| {
                if held.is_empty() {
                    picked.is_some_and(|p| p.contains(id))
                } else {
                    held.contains(id)
                }
            })
            .collect()
    }

    /// One device as the Device View, or its floating window, shows it.
    fn frame(&self, doc: &Document, d: &Device) -> DeviceFrame {
        DeviceFrame {
            id: d.id,
            title: d.title().to_owned(),
            name: d.own_name().to_owned(),
            kind: d.kind.clone(),
            role: d.role,
            enabled: d.enabled,
            takes_sample: matches!(
                &d.kind,
                DeviceKind::Native { device_type } if device_type == bl_devices::sampler::SAMPLER
            ),
            show_preset_name: d.show_preset_name,
            folded: self.folded.contains(&d.id),
            floating: d.floating,
            selected: self.selected == Some(d.id),
            hot_swap: self.hot_swap == Some(d.id),
            multisampler: self.multisampler_state(d),
            panel: self.panel(d),
            sidechain: d.sidechain.as_ref().map(|sidechain| {
                let (source, tap) = match &sidechain.input {
                    bl_model::Input::Track { track, tap } => {
                        (crate::routing::track_label(doc, *track), Some(*tap))
                    }
                    _ => ("No Input".to_owned(), None),
                };
                SidechainSection {
                    shown: self.sidechains_shown.contains(&d.id),
                    enabled: sidechain.enabled,
                    source,
                    tap,
                    mix: sidechain.mix,
                    gain: sidechain.gain,
                    listen: sidechain.listen,
                }
            }),
        }
    }

    /// The device a command acts on: `device`, or the selected one.
    fn target(&self, doc: &Document, device: Option<DeviceId>) -> Result<DeviceId, DeviceError> {
        let device = device
            .or(self.selected)
            .ok_or_else(|| DeviceError::Unavailable("select a device first".to_owned()))?;
        if doc.device(device).is_none() {
            return Err(DeviceError::InvalidArgs(format!("no {device}")));
        }
        Ok(device)
    }

    /// Whether a person can run `id` with `args` now.
    #[must_use]
    pub fn can_run(&self, id: &str, args: &Value, doc: &Document, track: Option<TrackId>) -> bool {
        let named = args
            .get("device")
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        let has_target = || self.target(doc, named).is_ok();
        match id {
            ids::CUT | ids::COPY | ids::SAVE_PRESET | ids::SAVE_DEFAULT_PRESET | ids::FOLD => {
                has_target()
            }
            ids::SHOW_SIDECHAIN => self
                .target(doc, named)
                .is_ok_and(|device| doc.device(device).is_some_and(|d| d.sidechain.is_some())),
            ids::DUPLICATE => self.target(doc, named).is_ok_and(|device| {
                doc.device(device)
                    .is_some_and(|d| d.role != DeviceRole::Instrument)
            }),
            ids::HOT_SWAP => has_target() || (args.get("on") == Some(&Value::Bool(false))),
            ids::PASTE => self.clipboard.as_ref().is_some_and(|device| {
                track
                    .and_then(|t| doc.track(t))
                    .is_some_and(|t| device.role.fits(t.kind()))
            }),
            ids::LOAD_PRESET => has_target() && args.get("path").is_some(),
            ids::PARAMETERS => has_target(),
            ids::SET_WINDOW => named
                .and_then(|device| doc.device(device))
                .is_some_and(|d| d.floating),
            _ => true,
        }
    }

    /// Runs Device View command `id`.
    ///
    /// # Errors
    ///
    /// [`DeviceError`] saying why it didn't run; nothing has changed then.
    pub fn run(&mut self, id: &str, args: Value, ctx: Context<'_>) -> Result<Outcome, DeviceError> {
        let mut outcome = Outcome::default();
        let Context {
            editor,
            arrangement,
            user_library,
            zoom,
            auto_hide,
            windows_shown,
        } = ctx;
        let track = shown_track(arrangement, editor.document());
        let windows = |view: &Self, editor: &Editor| {
            view.floating_windows(editor.document(), track, auto_hide, windows_shown)
        };
        let state = |view: &Self, editor: &Editor, arrangement: &ArrangementView| {
            to_value(&view.state(
                editor.document(),
                shown_track(arrangement, editor.document()),
            ))
        };
        match id {
            ids::STATE => {
                parse::<bl_commands::NoArgs>(args)?;
            }
            ids::WINDOWS => {
                parse::<bl_commands::NoArgs>(args)?;
                outcome.value = to_value(&windows(self, editor));
                return Ok(outcome);
            }
            ids::SET_WINDOW => {
                let SetWindowArgs { device, mut frame } = parse(args)?;
                if !frame.is_valid() {
                    return Err(DeviceError::InvalidArgs(
                        "a window's position and size must be finite, and its size above 0"
                            .to_owned(),
                    ));
                }
                let open = windows(self, editor);
                let window = open
                    .windows
                    .iter()
                    .find(|w| w.device == device)
                    .ok_or_else(|| DeviceError::Unavailable(format!("{device} isn't floating")))?;
                // IMP-14: the docked size is the smallest, and a device
                // without a display that grows stays at it.
                let zoom = f64::from(zoom);
                let (least_w, least_h) = (
                    f64::from(window.min_width) * zoom,
                    f64::from(window.min_height) * zoom,
                );
                frame.width = if window.grows_across {
                    frame.width.max(least_w)
                } else {
                    least_w
                };
                frame.height = if window.grows_down {
                    frame.height.max(least_h)
                } else {
                    least_h
                };
                self.windows.insert(device, frame);
                outcome.value = to_value(&windows(self, editor));
                return Ok(outcome);
            }
            ids::SELECT => {
                let SelectDeviceArgs { device } = parse(args)?;
                if let Some(device) = device {
                    let owner = editor
                        .document()
                        .device_track(device)
                        .ok_or_else(|| DeviceError::InvalidArgs(format!("no {device}")))?;
                    if track != Some(owner) {
                        select_track(arrangement, editor.document(), owner)?;
                    }
                }
                self.selected = device;
            }
            ids::FOLD => {
                let FoldDeviceArgs { device, folded } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let folded = folded.unwrap_or(!self.folded.contains(&device));
                if folded {
                    self.folded.insert(device);
                } else {
                    self.folded.remove(&device);
                }
            }
            ids::ADD => {
                let args: AddDeviceArgs = parse(args)?;
                let added = self.add(&args, editor, arrangement, user_library, track)?;
                if let Some(name) = &added.default_preset {
                    // Spec: device-chains R13, as Live's "Using Default
                    // Preset Auto Filter.adv."
                    outcome.message = Some(format!(
                        "Using Default Preset {name}.{}.",
                        preset::PRESET_EXTENSION
                    ));
                }
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::SHOW_SIDECHAIN => {
                let ShowSidechainArgs { device, shown } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                if editor
                    .document()
                    .device(device)
                    .is_none_or(|d| d.sidechain.is_none())
                {
                    return Err(DeviceError::Unavailable(format!(
                        "{device} has no sidechain input"
                    )));
                }
                let shown = shown.unwrap_or(!self.sidechains_shown.contains(&device));
                if shown {
                    self.sidechains_shown.insert(device);
                } else {
                    self.sidechains_shown.remove(&device);
                }
            }
            ids::CUT => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let copy = editor.document().device(device).cloned();
                editor
                    .apply(Command::RemoveDevice { device })
                    .map_err(|e| DeviceError::Failed(e.to_string()))?;
                self.clipboard = copy;
                outcome.document_changed = true;
            }
            ids::COPY => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                self.clipboard = editor.document().device(device).cloned();
            }
            ids::PASTE => {
                let PasteDeviceArgs { track: to, before } = parse(args)?;
                let copy = self
                    .clipboard
                    .clone()
                    .ok_or_else(|| DeviceError::Unavailable("nothing to paste".to_owned()))?;
                let to = to
                    .or(track)
                    .ok_or_else(|| DeviceError::Unavailable("select a track first".to_owned()))?;
                let doc = editor.document();
                let chain = doc
                    .track(to)
                    .ok_or_else(|| DeviceError::InvalidArgs(format!("no {to}")))?;
                // Spec: device-chains R15. Before the selected device, as
                // Live pastes.
                let before = before.or(self
                    .selected
                    .filter(|d| chain.device_position(*d).is_some()));
                let index = match before {
                    Some(device) => chain.device_position(device).ok_or_else(|| {
                        DeviceError::InvalidArgs(format!("{device} isn't on {to}"))
                    })?,
                    None => chain.devices().len(),
                };
                let added = self.insert_copy(editor, &copy, to, index, "Copy Device")?;
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::DUPLICATE => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let doc = editor.document();
                let copy = doc.device(device).cloned().expect("the target exists");
                let owner = doc.device_track(device).expect("the target has a track");
                let index = doc
                    .track(owner)
                    .and_then(|t| t.device_position(device))
                    .map_or(0, |i| i + 1);
                let added = self.insert_copy(editor, &copy, owner, index, "Duplicate Device")?;
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::SAVE_PRESET => {
                let SavePresetArgs { device, path } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let current = editor.document().device(device).expect("the target exists");
                let path = match path {
                    Some(path) => preset_path(&path)?,
                    None => {
                        let library = user_library.ok_or_else(no_library)?;
                        preset::unused_path(
                            &preset::presets_folder(library, current),
                            current.title(),
                        )
                    }
                };
                Preset::of(current)
                    .save(&path)
                    .map_err(|e| DeviceError::Failed(e.to_string()))?;
                outcome.message = Some(format!("Saved preset {}.", path.display()));
                outcome.value = to_value(&preset_result(device, &path));
                return Ok(outcome);
            }
            ids::SAVE_DEFAULT_PRESET => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let library = user_library.ok_or_else(no_library)?;
                let current = editor.document().device(device).expect("the target exists");
                let path = preset::default_preset_path(library, current);
                Preset::of(current)
                    .save(&path)
                    .map_err(|e| DeviceError::Failed(e.to_string()))?;
                outcome.message = Some(format!(
                    "Saved the default preset for {}: {}.",
                    current.own_name(),
                    path.display()
                ));
                outcome.value = to_value(&preset_result(device, &path));
                return Ok(outcome);
            }
            ids::PARAMETERS => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let current = editor.document().device(device).expect("the target exists");
                outcome.value = to_value(&self.parameters(current));
                return Ok(outcome);
            }
            ids::LOAD_PRESET => {
                let LoadPresetArgs { device, path } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let path = preset_path(&path)?;
                load_preset(self, editor, device, &path)?;
                outcome.document_changed = true;
                outcome.value = to_value(&preset_result(device, &path));
                return Ok(outcome);
            }
            ids::HOT_SWAP => {
                let HotSwapArgs { device, on } = parse(args)?;
                let on = on.unwrap_or(
                    self.hot_swap.is_none() || device.is_some_and(|d| Some(d) != self.hot_swap),
                );
                if on {
                    let device = self.target(editor.document(), device)?;
                    self.hot_swap = Some(device);
                    if let Some(library) = user_library {
                        let current = editor.document().device(device).expect("the target exists");
                        outcome.browse = Some(presets_route(library, current));
                    }
                } else {
                    self.hot_swap = None;
                }
            }
            _ => {
                return Err(DeviceError::InvalidArgs(format!(
                    "`{id}` isn't a Device View command"
                )));
            }
        }
        outcome.value = state(self, editor, arrangement);
        Ok(outcome)
    }

    /// Adds a device as a person does (`device.add`).
    fn add(
        &mut self,
        args: &AddDeviceArgs,
        editor: &mut Editor,
        arrangement: &mut ArrangementView,
        user_library: Option<&Path>,
        shown: Option<TrackId>,
    ) -> Result<DeviceAdded, DeviceError> {
        let role = args.device.role;
        let target = args
            .track
            .or(shown)
            .ok_or_else(|| DeviceError::Unavailable("select a track first".to_owned()))?;
        let kind = editor
            .document()
            .track(target)
            .map(bl_model::Track::kind)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {target}")))?;
        // Spec: device-chains R12. An instrument or MIDI effect for a
        // track that can't hold it goes on a new MIDI track at the end.
        let new_track = !role.fits(kind);
        let id: DeviceId = editor.new_id();
        let mut device =
            Device::new(id, args.device.name.clone(), args.device.kind.clone()).with_role(role);
        if args.device.sidechain {
            device = device.with_sidechain();
        }
        let default = user_library
            .map(|library| preset::default_preset_path(library, &device))
            .filter(|path| path.is_file())
            .and_then(|path| {
                Preset::load(&path)
                    .ok()
                    .filter(|p| p.fits(&device))
                    .map(|p| (path, p))
            });
        let label = if new_track {
            "Insert Device and Track"
        } else {
            "Insert Device"
        };
        let track_id: TrackId = if new_track { editor.new_id() } else { target };
        let index = match (new_track, args.index) {
            (true, _) => 0,
            (false, Some(index)) => index,
            (false, None) => default_index(editor.document(), target, role, self.selected),
        };
        let result = editor.transact(label, |editor| {
            if new_track {
                let top = editor.document().top_level().len();
                editor.apply(Command::CreateTrack {
                    id: track_id,
                    kind: TrackKind::Midi,
                    name: format!("{} {}", top + 1, args.device.name),
                    parent: None,
                    index: top,
                })?;
            }
            editor.apply(Command::InsertDevice {
                track: track_id,
                index,
                device,
            })?;
            if let Some((path, preset)) = &default {
                editor.apply(Command::SetDeviceState {
                    device: id,
                    params: self.sanitize(&args.device.kind, preset.params.clone()),
                    name: preset::preset_name(path),
                })?;
            }
            Ok::<_, bl_model::EditError>(())
        });
        result.map_err(|e| DeviceError::Failed(e.to_string()))?;
        if new_track {
            select_track(arrangement, editor.document(), track_id)?;
        }
        self.selected = Some(id);
        Ok(DeviceAdded {
            device: id,
            track: track_id,
            new_track,
            default_preset: default.map(|(path, _)| preset::preset_name(&path)),
        })
    }

    /// Inserts a copy of `device` with a new ID, selected, as one undo step
    /// named `label`.
    fn insert_copy(
        &mut self,
        editor: &mut Editor,
        device: &Device,
        track: TrackId,
        index: usize,
        label: &'static str,
    ) -> Result<DeviceAdded, DeviceError> {
        let id: DeviceId = editor.new_id();
        let mut copy = device.clone();
        copy.id = id;
        // A copy goes in the chain, docked, whether or not the original
        // floats (WP-2.5b).
        copy.floating = false;
        editor
            .transact(label, |editor| {
                editor.apply(Command::InsertDevice {
                    track,
                    index,
                    device: copy,
                })
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.selected = Some(id);
        Ok(DeviceAdded {
            device: id,
            track,
            new_track: false,
            default_preset: None,
        })
    }
}

/// Selects one track in the Arrangement, which the Device View then shows.
fn select_track(
    arrangement: &mut ArrangementView,
    doc: &Document,
    track: TrackId,
) -> Result<(), DeviceError> {
    arrangement
        .run(view_ids::SELECT_TRACKS, json!({ "tracks": [track] }), doc)
        .map(|_| ())
        .map_err(|_| DeviceError::Failed(format!("couldn't select {track}")))
}

/// Loads a preset file into a device as one undo step (`device.load_preset`,
/// and hot-swapping). A described device takes its values clamped to its
/// parameters' ranges, without values for parameters it doesn't have (a
/// preset from another version of the device, say: the device framework
/// spec, R7).
fn load_preset(
    view: &DeviceView,
    editor: &mut Editor,
    device: DeviceId,
    path: &Path,
) -> Result<(), DeviceError> {
    let loaded = Preset::load(path).map_err(|e| DeviceError::Failed(e.to_string()))?;
    let current = editor
        .document()
        .device(device)
        .ok_or_else(|| DeviceError::InvalidArgs(format!("no {device}")))?;
    loaded
        .check_fits(current)
        .map_err(|e| DeviceError::Failed(e.to_string()))?;
    editor
        .apply(Command::SetDeviceState {
            device,
            params: view.sanitize(&loaded.kind, loaded.params),
            name: preset::preset_name(path),
        })
        .map(|_| ())
        .map_err(|e| DeviceError::Failed(e.to_string()))
}

fn no_library() -> DeviceError {
    DeviceError::Unavailable("there's no User Library to save presets in".to_owned())
}

/// A preset path from a command's argument: absolute, ending `.blpreset`.
fn preset_path(path: &str) -> Result<PathBuf, DeviceError> {
    let path = PathBuf::from(path);
    if path.as_os_str().is_empty() || !preset::is_preset(&path) {
        return Err(DeviceError::InvalidArgs(format!(
            "a preset file's name ends .{}",
            preset::PRESET_EXTENSION
        )));
    }
    std::path::absolute(&path).map_err(|e| DeviceError::InvalidArgs(e.to_string()))
}

fn preset_result(device: DeviceId, path: &Path) -> PresetResult {
    PresetResult {
        device,
        path: path.to_string_lossy().into_owned(),
        name: preset::preset_name(path),
    }
}

/// The folders from the User Library down to `device`'s presets, as far as
/// they exist: hot-swapping opens them in the browser.
fn presets_route(library: &Path, device: &Device) -> Vec<PathBuf> {
    let target = preset::presets_folder(library, device);
    let mut route = Vec::new();
    let mut folder = library.to_path_buf();
    if let Ok(rest) = target.strip_prefix(library) {
        for part in rest {
            folder.push(part);
            if !folder.is_dir() {
                break;
            }
            route.push(folder.clone());
        }
    }
    route
}

/// Loads the browser's selected preset into the device being hot-swapped,
/// for the shell's `browser.load_selected` (the device chains spec, R19).
///
/// # Errors
///
/// [`DeviceError`] if nothing is being hot-swapped or the preset doesn't
/// load.
pub fn load_hot_swapped(
    view: &DeviceView,
    editor: &mut Editor,
    path: &Path,
) -> Result<PresetResult, DeviceError> {
    let device = view
        .hot_swap
        .ok_or_else(|| DeviceError::Unavailable("no device is being hot-swapped".to_owned()))?;
    load_preset(view, editor, device, path)?;
    Ok(preset_result(device, path))
}
