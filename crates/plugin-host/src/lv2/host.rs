//! The LV2 host: devices whose plug-ins are LV2 plug-ins (the LV2 hosting
//! spec), on the model of the CLAP and VST3 hosts.
//!
//! A [`Lv2Host`] lives on the main thread. It finds plug-ins in a lilv
//! [`World`](super::World), keeps one entry per device, and makes an
//! [`Instance`](super::Instance) for each node the compiler thread made,
//! handing it over through the node's queue. Its [`Lv2Factory`] goes into the
//! engine's [`Registry`](bl_engine::Registry).
//!
//! An instance has its sample rate fixed when it is made, and LV2 lets its
//! state be saved while it runs, so the host never takes an instance back
//! from a node: a state loaded, a new node, and an offline render's copy are
//! each a new instance, started from the state the device's plug-in has.
//! The instance a node held goes back to the main thread, which frees it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::ThreadId;

use bl_engine::{DeviceType, Node, NodeConfig, NodeFactory};
use bl_model::{Device, DeviceId, DeviceKind, DeviceRole, ParamId, PluginFormat, PluginState};

use super::describe::{self, Parameter};
use super::node::{Active, HostEnds, MAX_CHANNELS};
use super::{Lv2Error, Lv2Node, PluginInfo, PortKind, StateHandle, World};
use crate::drive::Driver;
use crate::window::GuiSize;

/// A plug-in Beatlayer can make devices of, and the bundle it's in.
#[derive(Clone, Debug, PartialEq)]
pub struct Lv2Type {
    /// The bundle folder.
    pub path: PathBuf,
    /// Its device type: kind, name, role, and its parameters.
    pub device_type: DeviceType,
    /// Its author, if it names one.
    pub vendor: Option<String>,
}

/// What a device's plug-in is doing, for `plugin.info`.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginStatus {
    /// The plug-in's URI.
    pub plugin_id: String,
    /// The bundle it was loaded from.
    pub path: PathBuf,
    /// Whether it's active: an instance is with a node.
    pub active: bool,
    /// The settings it's active for: the engine's rate and block size, or
    /// an offline render's while one has it.
    pub active_for: Option<NodeConfig>,
    /// The latency it reported in its last block, in frames.
    pub latency: u32,
    /// What went wrong with it, if anything.
    pub failure: Option<String>,
    /// Whether it has an editor.
    pub has_editor: bool,
    /// Whether its editor is open.
    pub editor_open: bool,
    /// Whether its open editor is hidden.
    pub editor_hidden: bool,
    /// The size of its editor's host window, while it is open in one.
    pub editor_size: Option<GuiSize>,
    /// Each parameter's value, in plain units.
    pub values: Vec<(ParamId, f64)>,
}

/// A parameter change a plug-in's editor made: one for each gesture, with
/// its final value, or one for each change outside a gesture (spec:
/// clap-hosting R6, which the LV2 spec mirrors).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamChange {
    /// The device.
    pub device: DeviceId,
    /// The parameter.
    pub param: ParamId,
    /// Its new value, in plain units.
    pub value: f64,
}

/// Things only the owner thread may touch.
struct MainOnly<T> {
    owner: ThreadId,
    value: T,
}

impl<T> MainOnly<T> {
    fn get(&mut self) -> Result<&mut T, Lv2Error> {
        if std::thread::current().id() == self.owner {
            Ok(&mut self.value)
        } else {
            Err(Lv2Error::WrongThread)
        }
    }
}

/// A node the compiler thread made, waiting for its plug-in.
struct Request {
    device: Device,
    config: NodeConfig,
    ends: HostEnds,
}

/// What the host and its factory share.
struct Inner {
    owner: ThreadId,
    /// The lilv library to load instead of the system's, for a test.
    library: Mutex<Option<PathBuf>>,
    world: Mutex<Option<World>>,
    requests: Mutex<Vec<Request>>,
    types: Mutex<Vec<Lv2Type>>,
    main: Mutex<MainOnly<Main>>,
    /// The latency each device's plug-in last reported, for a node made
    /// after an engine restart.
    latencies: Mutex<BTreeMap<DeviceId, u32>>,
}

// SAFETY: the worlds and instances inside are used under the rules above:
// the main thread's data only on the main thread, and a plug-in on the one
// thread that holds it.
unsafe impl Send for Inner {}
// SAFETY: as above.
unsafe impl Sync for Inner {}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The main thread's plug-ins.
pub(super) struct Main {
    /// The open editors, declared first so they are cleaned up while their
    /// plug-ins are still there.
    pub(super) editors: super::ui::Editors,
    pub(super) entries: BTreeMap<DeviceId, Entry>,
    /// Devices whose plug-in couldn't be found or made, and why.
    missing: BTreeMap<DeviceId, (String, PathBuf, String)>,
    /// Instances nodes gave back, freed on the next look.
    garbage: Vec<Active>,
    /// The changes the plug-ins' editors made, for the set's undo steps.
    pub(super) changes: Vec<ParamChange>,
    /// The devices whose plug-in reported a new latency.
    latency_changes: Vec<(DeviceId, u32)>,
    /// An offline render has the plug-ins ([`Lv2Host::lend`]).
    lending: bool,
    /// Devices the render couldn't make a plug-in for, and why.
    loan_problems: Vec<(DeviceId, String)>,
}

/// One device's plug-in.
pub(super) struct Entry {
    pub(super) plugin_id: String,
    path: PathBuf,
    pub(super) info: PluginInfo,
    pub(super) params: Vec<Parameter>,
    /// The state of the instance last handed to a node.
    pub(super) handle: Option<StateHandle>,
    /// The state the next instance starts from: the set's saved state, or
    /// one that was loaded.
    restore: Option<String>,
    /// The set's parameter values for the device, in plain units, applied
    /// over the state when an instance is made.
    values: Vec<(u32, f64)>,
    /// The node it runs for, and nodes it ran for that haven't given their
    /// instance back yet.
    pub(super) ends: Option<HostEnds>,
    retired: Vec<HostEnds>,
    pub(super) config: Option<NodeConfig>,
    /// An instance is with the current node.
    handed: bool,
    /// Making an instance failed; it isn't tried again until the next node.
    refused: Option<String>,
    /// It failed while processing: its device is silent.
    silenced: bool,
    failure: Option<String>,
    latency: u32,
    /// The offline render that has a copy.
    lent: Option<Lent>,
}

/// A copy of the plug-in running in an offline render.
struct Lent {
    ends: HostEnds,
    config: NodeConfig,
}

/// The LV2 host: LV2's main thread for every plug-in, on the thread that made
/// it.
///
/// Its methods run only on that thread and return [`Lv2Error::WrongThread`]
/// elsewhere. [`Lv2Host::idle`] must run there often (each frame, as the
/// shell's poll does) for new nodes to get their plug-ins.
#[derive(Clone)]
pub struct Lv2Host {
    inner: Arc<Inner>,
}

impl Default for Lv2Host {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Lv2Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lv2Host").finish_non_exhaustive()
    }
}

impl Lv2Host {
    /// A host with no plug-ins, whose main thread is this one. It loads
    /// the system's lilv when it first needs it.
    #[must_use]
    pub fn new() -> Self {
        let owner = std::thread::current().id();
        Self {
            inner: Arc::new(Inner {
                owner,
                library: Mutex::new(None),
                world: Mutex::new(None),
                requests: Mutex::new(Vec::new()),
                types: Mutex::new(Vec::new()),
                main: Mutex::new(MainOnly {
                    owner,
                    value: Main {
                        editors: super::ui::Editors::default(),
                        entries: BTreeMap::new(),
                        missing: BTreeMap::new(),
                        garbage: Vec::new(),
                        changes: Vec::new(),
                        latency_changes: Vec::new(),
                        lending: false,
                        loan_problems: Vec::new(),
                    },
                }),
                latencies: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    /// Uses the lilv library at `library` instead of the system's, as a test
    /// does off Linux. Call it before anything else.
    pub fn use_library(&self, library: impl Into<PathBuf>) {
        *lock(&self.inner.library) = Some(library.into());
        *lock(&self.inner.world) = None;
    }

    /// The factory for the engine's registry, which makes a node for each
    /// LV2 device.
    #[must_use]
    pub fn factory(&self) -> Lv2Factory {
        Lv2Factory {
            inner: Arc::clone(&self.inner),
        }
    }

    /// The world of plug-ins: the system's, found through `LV2_PATH` or the
    /// standard folders.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Unavailable`] where lilv isn't installed, and always off
    /// Linux.
    pub fn world(&self) -> Result<World, Lv2Error> {
        world(&self.inner)
    }

    /// Loads the LV2 bundle folder at `path` and returns its plug-ins as
    /// device types, which devices can then be made of.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Load`] if it isn't a bundle or offers no plug-ins, and
    /// [`Lv2Error::Unavailable`] without lilv.
    pub fn load(&self, path: &Path) -> Result<Vec<Lv2Type>, Lv2Error> {
        lock(&self.inner.main).get()?;
        if !path.is_dir() {
            return Err(Lv2Error::Load(format!(
                "{} isn't an LV2 bundle folder",
                path.display()
            )));
        }
        let world = world(&self.inner)?;
        world.load_bundle(path)?;
        let found: Vec<Lv2Type> = bundle_plugins(&world, path)
            .iter()
            .map(|info| Lv2Type::new(info, path))
            .collect();
        if found.is_empty() {
            return Err(Lv2Error::Load("it offers no plug-ins".to_owned()));
        }
        let mut types = lock(&self.inner.types);
        for found in &found {
            types.retain(|t| t.device_type.kind != found.device_type.kind);
            types.push(found.clone());
        }
        Ok(found)
    }

    /// The plug-ins loaded so far, as device types.
    #[must_use]
    pub fn types(&self) -> Vec<Lv2Type> {
        lock(&self.inner.types).clone()
    }

    /// Does the main thread's work: gives new nodes their plug-ins, frees
    /// instances nodes gave back, notes latencies the plug-ins report, and
    /// lets the editors do theirs. Returns whether anything a window shows
    /// may have changed.
    pub fn idle(&self) -> bool {
        let requests = std::mem::take(&mut *lock(&self.inner.requests));
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else { return false };
        let mut changed = !requests.is_empty();
        for request in requests {
            serve(main, request, &self.inner);
        }
        let mut latencies = lock(&self.inner.latencies);
        let Main {
            entries,
            garbage,
            latency_changes,
            ..
        } = &mut *main;
        for (&device, entry) in entries.iter_mut() {
            changed |= pump(
                device,
                entry,
                garbage,
                latency_changes,
                &mut latencies,
                &self.inner,
            );
        }
        garbage.clear();
        drop(latencies);
        changed |= super::ui::pump(main);
        changed
    }

    /// The parameter changes the plug-ins' editors made since the last call:
    /// each gesture once, with its final value, in plain units.
    pub fn take_param_changes(&self) -> Vec<ParamChange> {
        let mut guard = lock(&self.inner.main);
        guard
            .get()
            .map(|main| std::mem::take(&mut main.changes))
            .unwrap_or_default()
    }

    /// The devices whose plug-in reported a new latency since the last call,
    /// with the latency in frames: the engine recompiles with it, so delay
    /// compensation follows a plug-in that reports its own.
    pub fn take_latency_changes(&self) -> Vec<(DeviceId, u32)> {
        let mut guard = lock(&self.inner.main);
        guard
            .get()
            .map(|main| std::mem::take(&mut main.latency_changes))
            .unwrap_or_default()
    }

    /// The device's plug-in's own state, with the bundle it came from, as
    /// the set saves it: the text lilv writes.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::NoInstance`] if the device has no plug-in running, and
    /// [`Lv2Error::Failed`] if the plug-in won't save.
    pub fn save_state(&self, device: DeviceId) -> Result<PluginState, Lv2Error> {
        let mut guard = lock(&self.inner.main);
        let entry = entry(guard.get()?, device)?;
        let handle = entry.handle.as_ref().ok_or(Lv2Error::NoInstance(device))?;
        Ok(PluginState {
            path: entry.path.to_string_lossy().into_owned(),
            data: handle.save_state()?.into_bytes(),
        })
    }

    /// Gives the device's plug-in a state it saved before: a new instance
    /// starts from it and replaces the running one. The plug-in keeps the
    /// state it had if it refuses.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::NoInstance`] if the device has no plug-in, and
    /// [`Lv2Error::Failed`] if the plug-in refuses the state.
    pub fn load_state(&self, device: DeviceId, data: &[u8]) -> Result<(), Lv2Error> {
        let text = std::str::from_utf8(data).map_err(|_| Lv2Error::Failed("the state".into()))?;
        let mut guard = lock(&self.inner.main);
        let main = guard.get()?;
        let entry = entry(main, device)?;
        if entry.handed {
            // A state the plug-in refuses leaves the running instance as it
            // is.
            let config = entry.config.expect("handed with a config");
            let (active, handle) = made(&world(&self.inner)?, entry, &config, Some(text))?;
            if let Some(ends) = entry.ends.as_mut() {
                ends.to_node
                    .push(active)
                    .map_err(|_| Lv2Error::Failed("the node is busy".into()))?;
                entry.handle = Some(handle);
            }
        }
        entry.restore = Some(text.to_owned());
        entry.handle = entry.handle.take().filter(|_| entry.handed);
        entry.refused = None;
        entry.silenced = false;
        entry.failure = None;
        Ok(())
    }

    /// What the device's plug-in is doing, or `None` if the device has no
    /// LV2 plug-in, running or failed.
    #[must_use]
    pub fn status(&self, device: DeviceId) -> Option<PluginStatus> {
        let mut guard = lock(&self.inner.main);
        let main = guard.get().ok()?;
        if let Some((plugin_id, path, why)) = main.missing.get(&device) {
            return Some(PluginStatus {
                plugin_id: plugin_id.clone(),
                path: path.clone(),
                active: false,
                active_for: None,
                latency: 0,
                failure: Some(why.clone()),
                has_editor: false,
                editor_open: false,
                editor_hidden: false,
                editor_size: None,
                values: Vec::new(),
            });
        }
        let entry = main.entries.get(&device)?;
        let values: Vec<(ParamId, f64)> = entry
            .params
            .iter()
            .map(|param| {
                let port = param.map.port;
                let value = entry.handle.as_ref().map_or_else(
                    || param.descriptor.default,
                    |h| param.map.plain_value(h.control(port)),
                );
                (ParamId(port), value)
            })
            .collect();
        #[cfg(feature = "test-hooks")]
        crate::count_value_reads(values.len());
        let (editor_open, editor_hidden) = editor_state(main, device);
        let entry = main.entries.get(&device)?;
        Some(PluginStatus {
            plugin_id: entry.plugin_id.clone(),
            path: entry.path.clone(),
            active: entry.handed,
            active_for: entry
                .lent
                .as_ref()
                .map_or(entry.config, |lent| Some(lent.config))
                .filter(|_| entry.handed),
            latency: entry.latency,
            failure: entry.failure.clone().or_else(|| entry.refused.clone()),
            has_editor: has_editor(&entry.info),
            editor_open,
            editor_hidden,
            editor_size: super::ui::size(main, device),
            values,
        })
    }

    /// Opens the device's plug-in's editor in a host window titled `title`.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::NoInstance`] if the device has no plug-in, and
    /// [`Lv2Error::Failed`] if it has no editor, or it won't open.
    pub fn open_editor(&self, device: DeviceId, title: &str) -> Result<(), Lv2Error> {
        let mut guard = lock(&self.inner.main);
        open_editor(guard.get()?, device, title)
    }

    /// Closes the device's plug-in's editor, if it's open.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::WrongThread`] off the main thread.
    pub fn close_editor(&self, device: DeviceId) -> Result<(), Lv2Error> {
        let mut guard = lock(&self.inner.main);
        close_editor(guard.get()?, device);
        Ok(())
    }

    /// Shows or hides the device's editor window without closing it, as
    /// View ▸ Plug-In Windows does. Does nothing while its editor is closed.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::WrongThread`] off the main thread.
    pub fn set_editor_visible(&self, device: DeviceId, shown: bool) -> Result<(), Lv2Error> {
        let mut guard = lock(&self.inner.main);
        set_editor_visible(guard.get()?, device, shown);
        Ok(())
    }

    /// Makes the device's editor window `size` physical pixels, as dragging
    /// its edge does, and tells the editor.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::Failed`] if its editor isn't open in a host window.
    pub fn resize_editor(&self, device: DeviceId, size: GuiSize) -> Result<(), Lv2Error> {
        let mut guard = lock(&self.inner.main);
        super::ui::resize_window(guard.get()?, device, size)
    }

    /// The devices whose plug-in's editor window is open, hidden or shown.
    #[must_use]
    pub fn open_editors(&self) -> Vec<DeviceId> {
        let mut guard = lock(&self.inner.main);
        guard.get().map(open_editors).unwrap_or_default()
    }

    /// Opens the device's editor with no window, for a test where there is no
    /// display.
    #[cfg(test)]
    pub(super) fn open_editor_headless(&self, device: DeviceId) -> Result<(), Lv2Error> {
        let mut guard = lock(&self.inner.main);
        super::ui::open_with(guard.get()?, device, None)
    }

    /// Whether the device's plug-in has an editor, whether it's open, and
    /// whether it's hidden, or `None` if the device has no LV2 plug-in:
    /// what [`Lv2Host::status`] says of the editor, without reading the
    /// parameters.
    #[must_use]
    pub fn editor_state(&self, device: DeviceId) -> Option<(bool, bool, bool)> {
        let mut guard = lock(&self.inner.main);
        let main = guard.get().ok()?;
        if main.missing.contains_key(&device) {
            return Some((false, false, false));
        }
        let has_editor = has_editor(&main.entries.get(&device)?.info);
        let (open, hidden) = editor_state(main, device);
        Some((has_editor, open, hidden))
    }

    /// Lets go of every plug-in whose device isn't in `keep`, as when a set
    /// closes. Their nodes keep the instances they hold until they're dropped.
    pub fn retain(&self, keep: &BTreeSet<DeviceId>) {
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else { return };
        main.missing.retain(|device, _| keep.contains(device));
        let gone: Vec<DeviceId> = main
            .entries
            .keys()
            .filter(|d| !keep.contains(d))
            .copied()
            .collect();
        for device in gone {
            close_editor(main, device);
            main.entries.remove(&device);
        }
        lock(&self.inner.latencies).retain(|device, _| keep.contains(device));
    }

    /// Lets go of every plug-in, as when another set opens.
    pub fn clear(&self) {
        self.retain(&BTreeSet::new());
    }

    /// Lends the plug-ins to an offline render (WP-4.21), as
    /// [`ClapHost::lend`](crate::clap::ClapHost::lend) does for CLAP: an
    /// export or a preview, rendering on this thread. The loan's
    /// [`factory`](Loan::factory) goes into the render's registry; for each
    /// LV2 device the render makes a node for, it makes a copy of the
    /// device's plug-in at the render's rate and block size, started from the
    /// state the plug-in has and the rendered set's parameter values. The
    /// engine's instance isn't touched, so what the render does to the copy
    /// is dropped with it.
    ///
    /// # Errors
    ///
    /// [`Lv2Error::WrongThread`] off the main thread, and [`Lv2Error::Lent`]
    /// while another loan is out.
    pub fn lend(&self) -> Result<Loan, Lv2Error> {
        let mut guard = lock(&self.inner.main);
        let main = guard.get()?;
        if main.lending {
            return Err(Lv2Error::Lent);
        }
        main.lending = true;
        main.loan_problems.clear();
        Ok(Loan {
            inner: Arc::clone(&self.inner),
            _main_thread: std::marker::PhantomData,
        })
    }
}

/// The host's plug-ins, lent to an offline render ([`Lv2Host::lend`]).
/// Dropping it, on the main thread and after the render's engine, ends the
/// loan.
pub struct Loan {
    inner: Arc<Inner>,
    /// It ends on the main thread, which made it.
    _main_thread: std::marker::PhantomData<*const ()>,
}

impl fmt::Debug for Loan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Loan").finish_non_exhaustive()
    }
}

impl Loan {
    /// The factory for the render's registry: it makes each LV2 device's
    /// node with a copy of the device's plug-in.
    #[must_use]
    pub fn factory(&self) -> LoanFactory {
        LoanFactory {
            inner: Arc::clone(&self.inner),
        }
    }

    /// The devices whose plug-in the render couldn't copy, and why. Their
    /// nodes pass their input through.
    #[must_use]
    pub fn problems(&self) -> Vec<(DeviceId, String)> {
        let mut guard = lock(&self.inner.main);
        guard
            .get()
            .map(|main| main.loan_problems.clone())
            .unwrap_or_default()
    }
}

impl Drop for Loan {
    fn drop(&mut self) {
        let mut guard = lock(&self.inner.main);
        if let Ok(main) = guard.get() {
            end_loan(main, &self.inner);
        }
    }
}

/// A loan's factory for an offline render's registry ([`Loan::factory`]).
pub struct LoanFactory {
    inner: Arc<Inner>,
}

impl NodeFactory for LoanFactory {
    fn create(&self, device: &Device, config: &NodeConfig) -> Option<Box<dyn Node>> {
        let DeviceKind::Plugin {
            format: PluginFormat::Lv2,
            ..
        } = &device.kind
        else {
            return None;
        };
        let (node, ends) = Lv2Node::new(config, 0, device.role == DeviceRole::MidiEffect);
        let mut guard = lock(&self.inner.main);
        let problem = match guard.get() {
            Ok(main) if main.lending => borrow(main, device, *config, ends, &self.inner).err(),
            Ok(_) => Some("the plug-ins aren't lent to a render".to_owned()),
            Err(error) => Some(error.to_string()),
        };
        if let (Some(why), Ok(main)) = (problem, guard.get()) {
            main.loan_problems.push((device.id, why));
        }
        Some(Box::new(node))
    }

    fn device_types(&self) -> Vec<DeviceType> {
        lock(&self.inner.types)
            .iter()
            .map(|t| t.device_type.clone())
            .collect()
    }
}

/// The LV2 plug-ins' factory for the engine's registry. It makes each LV2
/// device's node at once; the host gives the node its plug-in.
pub struct Lv2Factory {
    inner: Arc<Inner>,
}

impl NodeFactory for Lv2Factory {
    fn create(&self, device: &Device, config: &NodeConfig) -> Option<Box<dyn Node>> {
        let DeviceKind::Plugin {
            format: PluginFormat::Lv2,
            ..
        } = &device.kind
        else {
            return None;
        };
        // A plug-in that already runs keeps its latency: the node reads it
        // at once, so a program compiled after an engine restart compensates
        // the same way as one compiled before it.
        let latency = lock(&self.inner.latencies).get(&device.id).copied();
        let (node, ends) = Lv2Node::new(
            config,
            latency.unwrap_or(0),
            device.role == DeviceRole::MidiEffect,
        );
        lock(&self.inner.requests).push(Request {
            device: device.clone(),
            config: *config,
            ends,
        });
        if std::thread::current().id() == self.inner.owner {
            Lv2Host {
                inner: Arc::clone(&self.inner),
            }
            .idle();
        }
        Some(Box::new(node))
    }

    fn device_types(&self) -> Vec<DeviceType> {
        lock(&self.inner.types)
            .iter()
            .map(|t| t.device_type.clone())
            .collect()
    }
}

fn world(inner: &Inner) -> Result<World, Lv2Error> {
    let mut guard = lock(&inner.world);
    if let Some(world) = &*guard {
        return Ok(world.clone());
    }
    let world = match lock(&inner.library).clone() {
        Some(library) => World::with_library(&library)?,
        None => World::new()?,
    };
    world.load_all();
    *guard = Some(world.clone());
    Ok(world)
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

impl Lv2Type {
    /// The device type of `info`, a plug-in in the bundle folder `path`.
    #[must_use]
    pub fn new(info: &PluginInfo, path: &Path) -> Self {
        let (device_type, _) = describe::device_type(info);
        Self {
            path: path.to_owned(),
            device_type,
            vendor: info.vendor.clone(),
        }
    }
}

/// The plug-ins `world` knows in the bundle folder `path`.
#[must_use]
pub fn bundle_plugins(world: &World, path: &Path) -> Vec<PluginInfo> {
    let wanted = canonical(path);
    world
        .plugins()
        .into_iter()
        .filter(|info| canonical(&info.bundle) == wanted)
        .collect()
}

/// Whether the plug-in has an editor Beatlayer can open: an X11 UI, on the
/// platforms that host X11 windows.
fn has_editor(info: &PluginInfo) -> bool {
    cfg!(all(unix, not(target_os = "macos"))) && info.ui.is_some()
}

fn editor_state(main: &Main, device: DeviceId) -> (bool, bool) {
    super::ui::state(main, device)
}

fn open_editor(main: &mut Main, device: DeviceId, title: &str) -> Result<(), Lv2Error> {
    super::ui::open(main, device, title)
}

fn close_editor(main: &mut Main, device: DeviceId) {
    super::ui::close(main, device);
}

fn set_editor_visible(main: &mut Main, device: DeviceId, shown: bool) {
    super::ui::set_visible(main, device, shown);
}

fn open_editors(main: &mut Main) -> Vec<DeviceId> {
    super::ui::open_devices(main)
}

/// The device's running plug-in, or why it has none.
fn entry(main: &mut Main, device: DeviceId) -> Result<&mut Entry, Lv2Error> {
    main.entries
        .get_mut(&device)
        .ok_or(Lv2Error::NoInstance(device))
}

/// Makes an instance of the entry's plug-in for `config`, from `state` if
/// there is one and the set's parameter values over it: the [`Active`] a
/// node runs, and the handle that saves its state.
fn made(
    world: &World,
    entry: &Entry,
    config: &NodeConfig,
    state: Option<&str>,
) -> Result<(Active, StateHandle), Lv2Error> {
    let mut instance = world.instantiate(
        &entry.plugin_id,
        f64::from(config.sample_rate.hz()),
        config.max_frames.max(1),
    )?;
    // Spec: lv2-hosting R9, R11 (as clap-hosting's R9, R11): the saved state
    // first, then the set's parameter values over it.
    if let Some(state) = state {
        instance.restore_state(state)?;
    }
    for &(port, plain) in &entry.values {
        if let Some(param) = entry.params.iter().find(|p| p.map.port == port) {
            instance.set_control(port, param.map.port_value(plain));
        }
    }
    let mut channel = 0;
    let mut audio_in = Vec::new();
    let mut audio_out = 0;
    for port in instance.ports() {
        match (port.kind, port.input) {
            (PortKind::Audio, true) => {
                audio_in.push((!port.sidechain && channel < MAX_CHANNELS).then(|| {
                    channel += 1;
                    channel - 1
                }));
            }
            (PortKind::Audio, false) => audio_out += 1,
            _ => {}
        }
    }
    let position = instance.ports().iter().any(|p| p.input && p.position);
    let drive = Driver::new(entry.params.iter().map(|p| {
        // Each control starts where the plug-in is: the set's values and
        // its saved state are on it.
        let plain = p.map.plain_value(instance.control(p.map.port));
        (p.descriptor.clone(), false, plain)
    }));
    let handle = instance.handle();
    Ok((
        Active {
            instance,
            audio_in,
            audio_out,
            position,
            params: entry.params.iter().map(|p| p.map).collect(),
            drive,
        },
        handle,
    ))
}

/// Gives a new node its plug-in, making the entry if the device has none.
fn serve(main: &mut Main, request: Request, inner: &Inner) {
    let Request {
        device,
        config,
        ends,
    } = request;
    if !ensure_entry(main, &device, inner) {
        return;
    }
    let Some(entry) = main.entries.get_mut(&device.id) else {
        return;
    };
    if let Some(old) = entry.ends.take() {
        entry.retired.push(old);
    }
    entry.ends = Some(ends);
    entry.config = Some(config);
    entry.handed = false;
    entry.refused = None;
    entry.silenced = false;
    entry.values = device
        .params
        .iter()
        .map(|(param, value)| (param.0, *value))
        .collect();
}

/// Makes the device's entry if it has none. Returns whether it has one; a
/// plug-in that can't be found is recorded as missing.
fn ensure_entry(main: &mut Main, device: &Device, inner: &Inner) -> bool {
    let DeviceKind::Plugin { plugin_id, .. } = &device.kind else {
        return false;
    };
    // A plug-in left from another set under the same ID is let go of.
    if main
        .entries
        .get(&device.id)
        .is_some_and(|entry| entry.plugin_id != *plugin_id)
    {
        main.entries.remove(&device.id);
    }
    if main.entries.contains_key(&device.id) {
        return true;
    }
    let saved = device
        .plugin
        .as_ref()
        .map(|p| PathBuf::from(&p.path))
        .filter(|p| !p.as_os_str().is_empty());
    let found = world(inner).and_then(|world| {
        let mut info = world.plugin(plugin_id);
        if info.is_none()
            && let Some(path) = saved.as_ref().filter(|p| p.is_dir())
        {
            world.load_bundle(path)?;
            info = world.plugin(plugin_id);
        }
        info.ok_or_else(|| Lv2Error::UnknownPlugin(plugin_id.clone()))
    });
    let info = match found {
        Ok(info) => info,
        Err(error) => {
            // Spec: clap-hosting R12, which the LV2 spec mirrors: the device
            // stays, passing its input through.
            main.missing.insert(
                device.id,
                (
                    plugin_id.clone(),
                    saved.unwrap_or_default(),
                    error.to_string(),
                ),
            );
            return false;
        }
    };
    main.missing.remove(&device.id);
    // A set that opens brings its plug-in's type, so the device shows its
    // parameters without a load.
    {
        let mut types = lock(&inner.types);
        if !types.iter().any(|t| t.device_type.kind == device.kind) {
            types.push(Lv2Type::new(&info, &info.bundle));
        }
    }
    let (_, params) = describe::device_type(&info);
    let restore = device
        .plugin
        .as_ref()
        .filter(|p| !p.data.is_empty())
        .and_then(|p| String::from_utf8(p.data.clone()).ok());
    main.entries.insert(
        device.id,
        Entry {
            plugin_id: plugin_id.clone(),
            path: info.bundle.clone(),
            info,
            params,
            handle: None,
            restore,
            values: Vec::new(),
            ends: None,
            retired: Vec::new(),
            config: None,
            handed: false,
            refused: None,
            silenced: false,
            failure: None,
            latency: 0,
            lent: None,
        },
    );
    true
}

/// Makes the render's copy of the device's plug-in (WP-4.21): a new
/// instance at the render's settings, from the plug-in's state and the
/// rendered set's parameter values. A plug-in that can't be made leaves the
/// node passing its input through.
fn borrow(
    main: &mut Main,
    device: &Device,
    config: NodeConfig,
    ends: HostEnds,
    inner: &Inner,
) -> Result<(), String> {
    if !ensure_entry(main, device, inner) {
        // Spec: clap-hosting R12, which the LV2 spec mirrors.
        return Ok(());
    }
    let Some(entry) = main.entries.get_mut(&device.id) else {
        return Ok(());
    };
    if entry.lent.is_some() {
        return Err("its plug-in is lent to this render already".to_owned());
    }
    // The render's values are the rendered set's, not the engine's.
    let engine_values = std::mem::replace(
        &mut entry.values,
        device
            .params
            .iter()
            .map(|(param, value)| (param.0, *value))
            .collect(),
    );
    let state = entry
        .handle
        .as_ref()
        .and_then(|handle| handle.save_state().ok())
        .or_else(|| entry.restore.clone());
    let copy = world(inner).and_then(|world| made(&world, entry, &config, state.as_deref()));
    entry.values = engine_values;
    let mut ends = ends;
    if let Ok((active, _)) = copy {
        ends.status
            .latency
            .store(active.instance.latency(), Relaxed);
        let _ = ends.to_node.push(active);
    }
    entry.lent = Some(Lent { ends, config });
    Ok(())
}

/// Ends a loan: the render's copies are freed once their nodes give them
/// back.
fn end_loan(main: &mut Main, _inner: &Inner) {
    if !main.lending {
        return;
    }
    main.lending = false;
    main.loan_problems.clear();
    let Main {
        entries, garbage, ..
    } = main;
    for entry in entries.values_mut() {
        let Some(mut lent) = entry.lent.take() else {
            continue;
        };
        while let Ok(active) = lent.ends.from_node.pop() {
            garbage.push(active);
        }
        if !lent.ends.from_node.is_abandoned() {
            // The render's node still holds its copy: it is freed when the
            // node gives it back.
            entry.retired.push(lent.ends);
        }
    }
    garbage.clear();
}

/// One entry's main-thread work: free what nodes gave back, make an
/// instance for its node when it has none, and report a latency that
/// changed. Returns whether anything changed.
fn pump(
    device: DeviceId,
    entry: &mut Entry,
    garbage: &mut Vec<Active>,
    latency_changes: &mut Vec<(DeviceId, u32)>,
    latencies: &mut BTreeMap<DeviceId, u32>,
    inner: &Inner,
) -> bool {
    let mut changed = false;
    for ends in entry.ends.iter_mut().chain(entry.retired.iter_mut()) {
        while let Ok(active) = ends.from_node.pop() {
            garbage.push(active);
        }
    }
    entry
        .retired
        .retain(|ends| !(ends.from_node.is_abandoned() && ends.from_node.is_empty()));
    if let Some(ends) = &entry.ends
        && ends.from_node.is_abandoned()
    {
        // Its node is gone.
        entry.handed = false;
    }
    if let Some(ends) = &entry.ends
        && ends.status.failed.load(Relaxed)
        && !entry.silenced
    {
        entry.silenced = true;
        entry.failure = Some("it failed while processing; the device is silent".to_owned());
        changed = true;
    }
    let ready = !entry.handed
        && entry.lent.is_none()
        && entry.refused.is_none()
        && !entry.silenced
        && entry.config.is_some()
        && entry
            .ends
            .as_ref()
            .is_some_and(|e| !e.to_node.is_abandoned());
    if ready {
        let config = entry.config.expect("the check above");
        // The state the plug-in has now, else the one it was given.
        let state = entry
            .handle
            .as_ref()
            .and_then(|handle| handle.save_state().ok())
            .or_else(|| entry.restore.clone());
        match world(inner).and_then(|world| made(&world, entry, &config, state.as_deref())) {
            Ok((active, handle)) => {
                // A state the plug-in refused isn't an error that stops it:
                // it starts from its defaults (R9).
                latencies.insert(device, active.instance.latency());
                if let Some(ends) = entry.ends.as_mut()
                    && ends.to_node.push(active).is_ok()
                {
                    entry.handle = Some(handle);
                    entry.handed = true;
                    changed = true;
                }
            }
            Err(error) => {
                match world(inner).and_then(|world| made(&world, entry, &config, None)) {
                    Ok((active, handle)) if state.is_some() => {
                        entry.failure = Some(format!("{error}; it starts from its defaults"));
                        if let Some(ends) = entry.ends.as_mut()
                            && ends.to_node.push(active).is_ok()
                        {
                            entry.handle = Some(handle);
                            entry.handed = true;
                        }
                    }
                    _ => entry.refused = Some(error.to_string()),
                }
                changed = true;
            }
        }
    }
    // The latency it reports after a block (R10).
    if entry.handed
        && let Some(handle) = &entry.handle
    {
        let now = handle.latency();
        if now != entry.latency {
            entry.latency = now;
            latencies.insert(device, now);
            latency_changes.push((device, now));
            changed = true;
        }
    }
    changed
}
