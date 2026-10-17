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
    Chain, ChainId, Command, Device, DeviceDescription, DeviceId, DeviceKind, DeviceLayout,
    DevicePlace, DeviceRole, DeviceWindows, Document, Editor, ParamId, RackKind, TrackId,
    TrackKind,
};
use bl_project::preset::{self, Preset};
use bl_shell_spec::arrangement as view_ids;
use bl_shell_spec::devices::{
    self as ids, AddDeviceArgs, ChainAdded, ChainArgs, DeviceAdded, DeviceArgs, DeviceFrame,
    DevicePanel, DeviceParameters, DeviceViewState, ExpandDeviceArgs, FloatingWindow,
    FloatingWindows, FoldDeviceArgs, GroupArgs, Grouped, HotSwap, HotSwapArgs, LoadPresetArgs,
    ParameterState, PasteChainArgs, PasteDeviceArgs, PresetResult, SavePresetArgs,
    SelectDeviceArgs, SetWindowArgs, ShowSidechainArgs, SidechainSection, Ungrouped,
};
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
    /// Devices whose expanded part is shown (WP-5.42).
    expanded: BTreeSet<DeviceId>,
    clipboard: Option<Device>,
    /// The chain cut or copied from a rack (WP-5.1).
    chain_clipboard: Option<Chain>,
    hot_swap: Option<DeviceId>,
    /// Devices whose sidechain section is shown (WP-2.19b).
    sidechains_shown: BTreeSet<DeviceId>,
    /// The view each device's display shows, where it has chosen one
    /// (WP-5.43).
    display_views: std::collections::BTreeMap<DeviceId, String>,
    /// The display data the engine's nodes last published (WP-5.43).
    displays: std::collections::BTreeMap<DeviceId, bl_engine::DisplayData>,
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

/// The display of a device whose type draws none.
fn empty_display(device: DeviceId) -> bl_model::DeviceDisplay {
    bl_model::DeviceDisplay {
        device,
        live: false,
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    }
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
    index_among(&roles, role, after.and_then(|d| track.device_position(d)))
}

/// Where a device with `role` goes among a chain's devices, whose roles are
/// `roles`: straight after the one at `after` if that keeps the chain in
/// order, or else at the end of its role's place.
fn index_among(roles: &[DeviceRole], role: DeviceRole, after: Option<usize>) -> usize {
    let fits = |index: usize| {
        roles[..index].iter().all(|r| *r <= role) && roles[index..].iter().all(|r| *r >= role)
    };
    if let Some(index) = after.map(|i| i + 1)
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
                views: Vec::new(),
                menu_options: Vec::new(),
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
                            menu_option: description.menu_options.contains(&p.id),
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
        self.expanded.retain(exists);
        self.sidechains_shown.retain(exists);
        self.display_views.retain(|d, _| exists(d));
        self.displays.retain(|d, _| exists(d));
    }

    /// Whether `device`'s type has an expanded part.
    fn expandable(&self, device: &Device) -> bool {
        self.description(&device.kind)
            .is_some_and(|d| d.layout.has_expanded())
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
            expandable: self.expandable(d),
            expanded: self.expandable(d) && self.expanded.contains(&d.id),
            floating: d.floating,
            selected: self.selected == Some(d.id),
            hot_swap: self.hot_swap == Some(d.id),
            panel: self.panel(d),
            sidechain: d.sidechain.as_ref().map(|sidechain| {
                let (source, tap) = match &sidechain.input {
                    bl_model::Input::Track { track, tap, .. } => {
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
            display_view: self.display_view(d),
        }
    }

    /// The view `device`'s display shows: the one chosen, or the first its
    /// description offers; none for a display with one view.
    fn display_view(&self, device: &Device) -> Option<String> {
        let views = &self.description(&device.kind)?.views;
        self.display_views
            .get(&device.id)
            .filter(|view| views.contains(view))
            .or_else(|| views.first())
            .cloned()
    }

    /// The display data the engine published, to fill from.
    pub fn displays_mut(
        &mut self,
    ) -> &mut std::collections::BTreeMap<DeviceId, bl_engine::DisplayData> {
        &mut self.displays
    }

    /// What `device`'s display shows now: its type's display from the
    /// device's values and the newest data its node published.
    #[must_use]
    pub fn display(&self, device: &Device) -> bl_model::DeviceDisplay {
        let DeviceKind::Native { device_type } = &device.kind else {
            return empty_display(device.id);
        };
        let data = self.displays.get(&device.id).map(|d| d.values.as_slice());
        let mut display = self
            .description(&device.kind)
            .and_then(|description| {
                bl_devices::display(device_type, device.id, description, &device.params, data)
            })
            .unwrap_or_else(|| empty_display(device.id));
        display.view = self.display_view(device);
        display
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
            ids::EXPAND => self
                .target(doc, named)
                .is_ok_and(|device| doc.device(device).is_some_and(|d| self.expandable(d))),
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
            ids::GROUP => {
                let devices: Vec<DeviceId> = args
                    .get("devices")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                args.get("chains").is_some()
                    || (devices.is_empty()
                        && self.selected.is_some_and(|d| doc.device(d).is_some()))
                    || devices.iter().any(|d| doc.device(*d).is_some())
            }
            ids::UNGROUP => self.target(doc, named).is_ok_and(|device| {
                doc.device(device)
                    .is_some_and(|d| d.rack_kind().is_some() && d.chains.len() <= 1)
            }),
            ids::CHAIN_PASTE => self.chain_clipboard.is_some(),
            ids::LOAD_PRESET => has_target() && args.get("path").is_some(),
            ids::PARAMETERS | ids::DISPLAY | ids::SET_DISPLAY_VIEW => has_target(),
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
            ids::EXPAND => {
                let ExpandDeviceArgs { device, expanded } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                if editor
                    .document()
                    .device(device)
                    .is_none_or(|d| !self.expandable(d))
                {
                    return Err(DeviceError::Unavailable(format!(
                        "{device} has no expanded part"
                    )));
                }
                let expanded = expanded.unwrap_or(!self.expanded.contains(&device));
                if expanded {
                    self.expanded.insert(device);
                } else {
                    self.expanded.remove(&device);
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
                let PasteDeviceArgs {
                    track: to,
                    chain,
                    before,
                } = parse(args)?;
                let copy = self
                    .clipboard
                    .clone()
                    .ok_or_else(|| DeviceError::Unavailable("nothing to paste".to_owned()))?;
                let doc = editor.document();
                let place = match (chain, before) {
                    (Some(chain), _) => {
                        let track = doc
                            .chain_track(chain)
                            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
                        // Spec: device-chains R15. Before the selected
                        // device, as Live pastes, if it's in this chain.
                        let at = before
                            .or(self.selected)
                            .and_then(|d| doc.device_place(d))
                            .filter(|p| p.chain == Some(chain));
                        DevicePlace {
                            track,
                            chain: Some(chain),
                            index: at.map_or_else(
                                || doc.chain(chain).map_or(0, |c| c.devices.len()),
                                |p| p.index,
                            ),
                        }
                    }
                    (None, Some(device)) => {
                        let place = doc
                            .device_place(device)
                            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {device}")))?;
                        if let Some(to) = to.filter(|to| *to != place.track) {
                            return Err(DeviceError::InvalidArgs(format!(
                                "{device} isn't on {to}"
                            )));
                        }
                        place
                    }
                    (None, None) => {
                        let to = to.or(track).ok_or_else(|| {
                            DeviceError::Unavailable("select a track first".to_owned())
                        })?;
                        let chain = doc
                            .track(to)
                            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {to}")))?;
                        match self
                            .selected
                            .and_then(|d| doc.device_place(d))
                            .filter(|p| p.track == to)
                        {
                            Some(place) => place,
                            None => DevicePlace {
                                track: to,
                                chain: None,
                                index: chain.devices().len(),
                            },
                        }
                    }
                };
                let added = self.insert_copy(editor, &copy, place, "Copy Device")?;
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::DUPLICATE => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let doc = editor.document();
                let copy = doc.device(device).cloned().expect("the target exists");
                let mut place = doc.device_place(device).expect("the target has a place");
                place.index += 1;
                let added = self.insert_copy(editor, &copy, place, "Duplicate Device")?;
                outcome.document_changed = true;
                outcome.value = to_value(&added);
                return Ok(outcome);
            }
            ids::GROUP => {
                let args: GroupArgs = parse(args)?;
                let grouped = self.group(editor, args)?;
                outcome.document_changed = true;
                outcome.value = to_value(&grouped);
                return Ok(outcome);
            }
            ids::UNGROUP => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let ungrouped = self.ungroup(editor, device)?;
                outcome.document_changed = true;
                outcome.value = to_value(&ungrouped);
                return Ok(outcome);
            }
            ids::CHAIN_COPY | ids::CHAIN_CUT => {
                let ChainArgs { chain } = parse(args)?;
                let copy = editor
                    .document()
                    .chain(chain)
                    .cloned()
                    .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
                if id == ids::CHAIN_CUT {
                    editor
                        .apply(Command::RemoveChain { chain })
                        .map_err(|e| DeviceError::Failed(e.to_string()))?;
                    outcome.document_changed = true;
                }
                self.chain_clipboard = Some(copy);
            }
            ids::CHAIN_PASTE => {
                let PasteChainArgs { rack, index } = parse(args)?;
                let copy = self
                    .chain_clipboard
                    .clone()
                    .ok_or_else(|| DeviceError::Unavailable("no chain to paste".to_owned()))?;
                let at = index.unwrap_or_else(|| {
                    editor
                        .document()
                        .device(rack)
                        .map_or(0, |device| device.chains.len())
                });
                outcome.value = to_value(&self.insert_chain_copy(editor, &copy, rack, at)?);
                outcome.document_changed = true;
                return Ok(outcome);
            }
            ids::CHAIN_DUPLICATE => {
                let ChainArgs { chain } = parse(args)?;
                let doc = editor.document();
                let copy = doc
                    .chain(chain)
                    .cloned()
                    .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
                let (rack, index) = doc.chain_place(chain).expect("the chain has a rack");
                outcome.value =
                    to_value(&self.insert_chain_copy(editor, &copy, rack, index + 1)?);
                outcome.document_changed = true;
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
            ids::DISPLAY => {
                let DeviceArgs { device } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let current = editor.document().device(device).expect("the target exists");
                outcome.value = to_value(&self.display(current));
                return Ok(outcome);
            }
            ids::SET_DISPLAY_VIEW => {
                let ids::SetDisplayViewArgs { device, view } = parse(args)?;
                let device = self.target(editor.document(), device)?;
                let current = editor.document().device(device).expect("the target exists");
                let offered = self
                    .description(&current.kind)
                    .map_or(&[][..], |d| d.views.as_slice());
                if !offered.contains(&view) {
                    return Err(DeviceError::InvalidArgs(format!(
                        "{device}'s display has no view {view:?}; it offers {offered:?}"
                    )));
                }
                self.display_views.insert(device, view);
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
        if let Some(chain) = args.chain {
            return self.add_to_chain(args, chain, editor, user_library);
        }
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
        let device = new_device(id, args);
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

    /// Adds a device to a rack's chain (`device.add` with `chain`), after
    /// the selected device if it's in the chain, with its default preset.
    fn add_to_chain(
        &mut self,
        args: &AddDeviceArgs,
        chain: ChainId,
        editor: &mut Editor,
        user_library: Option<&Path>,
    ) -> Result<DeviceAdded, DeviceError> {
        let doc = editor.document();
        let track = doc
            .chain_track(chain)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
        let roles: Vec<DeviceRole> = doc
            .chain(chain)
            .map(|c| c.devices.iter().map(|d| d.role).collect())
            .unwrap_or_default();
        let after = self
            .selected
            .and_then(|d| doc.device_place(d))
            .filter(|p| p.chain == Some(chain))
            .map(|p| p.index);
        let index = args
            .index
            .unwrap_or_else(|| index_among(&roles, args.device.role, after));
        let id: DeviceId = editor.new_id();
        let device = new_device(id, args);
        let default = user_library
            .map(|library| preset::default_preset_path(library, &device))
            .filter(|path| path.is_file())
            .and_then(|path| {
                Preset::load(&path)
                    .ok()
                    .filter(|p| p.fits(&device))
                    .map(|p| (path, p))
            });
        editor
            .transact("Insert Device", |editor| {
                editor.apply(Command::InsertChainDevice {
                    chain,
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
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.selected = Some(id);
        Ok(DeviceAdded {
            device: id,
            track,
            new_track: false,
            default_preset: default.map(|(path, _)| preset::preset_name(&path)),
        })
    }

    /// Inserts a copy of `device` with new IDs for it and everything in it,
    /// selected, at `place` as one undo step named `label`.
    fn insert_copy(
        &mut self,
        editor: &mut Editor,
        device: &Device,
        place: DevicePlace,
        label: &'static str,
    ) -> Result<DeviceAdded, DeviceError> {
        let mut copy = device.clone();
        // A copy goes in the chain, docked, whether or not the original
        // floats (WP-5.1, WP-2.5b), and so does everything inside a rack.
        copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
        let id = copy.id;
        editor
            .transact(label, |editor| match place.chain {
                None => editor.apply(Command::InsertDevice {
                    track: place.track,
                    index: place.index,
                    device: copy,
                }),
                Some(chain) => editor.apply(Command::InsertChainDevice {
                    chain,
                    index: place.index,
                    device: copy,
                }),
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.selected = Some(id);
        Ok(DeviceAdded {
            device: id,
            track: place.track,
            new_track: false,
            default_preset: None,
        })
    }

    /// Inserts a copy of `chain` with new IDs at `index` in `rack`'s list
    /// of chains, as one undo step named after Insert Chain.
    fn insert_chain_copy(
        &mut self,
        editor: &mut Editor,
        chain: &Chain,
        rack: DeviceId,
        index: usize,
    ) -> Result<ChainAdded, DeviceError> {
        let mut copy = chain.clone();
        copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
        let id = copy.id;
        editor
            .apply(Command::AddChain {
                rack,
                index,
                chain: copy,
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        Ok(ChainAdded { chain: id, rack })
    }

    /// Group (Cmd-G): wraps devices of one chain, or chains of one rack, in
    /// a new rack (the racks spec, R9, R10).
    fn group(&mut self, editor: &mut Editor, args: GroupArgs) -> Result<Grouped, DeviceError> {
        let doc = editor.document();
        let chains = args.chains.unwrap_or_default();
        let devices = args.devices.unwrap_or_default();
        if !chains.is_empty() {
            if !devices.is_empty() {
                return Err(DeviceError::InvalidArgs(
                    "group devices or chains, not both".to_owned(),
                ));
            }
            return self.group_chains(editor, &chains);
        }
        let devices = if devices.is_empty() {
            vec![
                self.selected
                    .ok_or_else(|| DeviceError::Unavailable("select a device first".to_owned()))?,
            ]
        } else {
            devices
        };
        let mut placed = Vec::with_capacity(devices.len());
        for &id in &devices {
            let device = doc
                .device(id)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {id}")))?;
            let place = doc
                .device_place(id)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {id}")))?;
            placed.push((place.index, device.role, device.title().to_owned()));
        }
        placed.sort();
        let kind = RackKind::holding(placed.iter().map(|(_, role, _)| *role));
        let rack: DeviceId = editor.new_id();
        let chain: ChainId = editor.new_id();
        let first = placed[0].2.clone();
        editor
            .apply(Command::GroupDevices {
                devices,
                rack: Device::rack(rack, kind).with_chain(Chain::new(chain, first)),
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.selected = Some(rack);
        Ok(Grouped { rack, chain })
    }

    /// Group with several chains of one rack selected (R10): a new chain in
    /// the first one's place holds a new rack of the same kind, and they
    /// become its chains.
    fn group_chains(
        &mut self,
        editor: &mut Editor,
        chains: &[ChainId],
    ) -> Result<Grouped, DeviceError> {
        let doc = editor.document();
        let mut placed = Vec::with_capacity(chains.len());
        for &chain in chains {
            let (rack, index) = doc
                .chain_place(chain)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
            placed.push((index, chain, rack));
        }
        placed.sort();
        let rack = placed[0].2;
        if placed.iter().any(|&(.., r)| r != rack) {
            return Err(DeviceError::Unavailable(
                "the chains to group must be in one rack".to_owned(),
            ));
        }
        let kind = doc
            .device(rack)
            .and_then(Device::rack_kind)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("{rack} isn't a rack")))?;
        let (inner, outer): (DeviceId, ChainId) = (editor.new_id(), editor.new_id());
        editor
            .transact("Group", |editor| {
                editor.apply(Command::AddChain {
                    rack,
                    index: placed[0].0,
                    chain: Chain::new(outer, "Chain").with_devices(vec![Device::rack(inner, kind)]),
                })?;
                for &(_, chain, _) in &placed {
                    let end = editor
                        .document()
                        .device(inner)
                        .map_or(0, |d| d.chains.len());
                    editor.apply(Command::MoveChain {
                        chain,
                        rack: inner,
                        index: end,
                    })?;
                }
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.selected = Some(inner);
        Ok(Grouped {
            rack: inner,
            chain: outer,
        })
    }

    /// Ungroup (Cmd-Shift-G): takes a rack of one chain, or none, apart
    /// (R11), selecting the first device that took its place.
    fn ungroup(&mut self, editor: &mut Editor, rack: DeviceId) -> Result<Ungrouped, DeviceError> {
        let current = editor
            .document()
            .device(rack)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {rack}")))?;
        if current.rack_kind().is_none() {
            return Err(DeviceError::Unavailable(format!("{rack} isn't a rack")));
        }
        if current.chains.len() > 1 {
            return Err(DeviceError::Unavailable(
                "a rack of several chains can't be ungrouped".to_owned(),
            ));
        }
        let devices: Vec<DeviceId> = current
            .chains
            .first()
            .map(|chain| chain.devices.iter().map(|d| d.id).collect())
            .unwrap_or_default();
        editor
            .apply(Command::UngroupDevice { device: rack })
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.selected = devices.first().copied();
        Ok(Ungrouped { devices })
    }
}

/// A new device as `device.add` makes it: a rack of its kind, or a device of
/// the type and role the arguments name.
fn new_device(id: DeviceId, args: &AddDeviceArgs) -> Device {
    if let DeviceKind::Rack { kind } = args.device.kind {
        return Device::rack(id, kind);
    }
    let mut device = Device::new(id, args.device.name.clone(), args.device.kind.clone())
        .with_role(args.device.role);
    if args.device.sidechain {
        device = device.with_sidechain();
    }
    device
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
