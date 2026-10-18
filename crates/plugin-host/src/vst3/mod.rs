//! VST3 hosting (WP-2.7; the VST3 hosting spec): loading a `.vst3`
//! module, its factory and each audio class as a device type, the component
//! and its controller connected, and processing block by block with each
//! parameter change on its sample offset and each note in the event list on
//! its frame. The node passes its input through until the plug-in is
//! active. Saving and restoring the component's and the controller's state
//! with the set, latency and restarts, and the changes a plug-in's own
//! editor makes, are part 2's (as clap-hosting R6, R8 to R12 does for
//! CLAP).
//!
//! A [`Vst3Host`] lives on the main thread, like the CLAP host: it loads
//! modules, keeps one component and controller per device, and activates
//! them for a node the compiler thread made. Its [`Vst3Factory`] goes into
//! the engine's [`Registry`](bl_engine::Registry).
//!
//! Offline renders (exports and agents' previews) borrow the same
//! instances ([`Vst3Host::lend`], WP-4.21), as [`ClapHost::lend`] does for
//! CLAP: each instance comes back from the engine's node, is reactivated
//! for the render's rate and block size, and when the [`Loan`] ends goes
//! back to the engine with the state it had.
//!
//! [`ClapHost::lend`]: crate::clap::ClapHost::lend

mod describe;
mod handlers;
mod node;
mod stream;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{c_char, c_void};
use std::fmt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use bl_engine::{DeviceType, Node, NodeConfig, NodeFactory};
use bl_model::{
    Device, DeviceDescription, DeviceId, DeviceKind, ParamId, PluginFormat, PluginState,
};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper};

use crate::drive::Driver;
use handlers::{ComponentHandler, Gestures, HostApplication};
use node::{HostEnds, MAX_BUSES};

pub use node::Vst3Node;

/// Why a VST3 host call failed. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Vst3Error {
    /// The file isn't a VST3 plug-in Beatlayer can load, and why.
    Load(String),
    /// No plug-in of that class ID is known.
    UnknownPlugin(String),
    /// The device has no running plug-in: it isn't a VST3 device, or its
    /// plug-in couldn't be loaded.
    NoInstance(DeviceId),
    /// The plug-in refused, and what it refused.
    Refused(String),
    /// It was called off the main thread.
    WrongThread,
    /// The plug-ins are lent to another offline render already.
    Lent,
}

impl fmt::Display for Vst3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(why) => write!(f, "couldn't load the plug-in: {why}"),
            Self::UnknownPlugin(id) => write!(f, "no VST3 plug-in {id} is loaded"),
            Self::NoInstance(device) => write!(f, "{device} has no running VST3 plug-in"),
            Self::Refused(what) => write!(f, "the plug-in refused {what}"),
            Self::WrongThread => f.write_str("VST3's main-thread calls run on the main thread"),
            Self::Lent => f.write_str("the plug-ins are lent to another render"),
        }
    }
}

impl std::error::Error for Vst3Error {}

/// A plug-in Beatlayer can make devices of, and the file it's in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vst3Type {
    /// The plug-in bundle or module.
    pub path: PathBuf,
    /// Its device type: kind, name, role, and its parameters.
    pub device_type: DeviceType,
    /// Its vendor, if it gives one (the scanner's list, WP-2.9).
    pub vendor: Option<String>,
    /// Its version, if it gives one (the scanner's list, WP-2.9).
    pub version: Option<String>,
}

/// What a device's plug-in is doing, for `plugin.info`.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginStatus {
    /// The plug-in's class ID.
    pub plugin_id: String,
    /// The file it was loaded from.
    pub path: PathBuf,
    /// Whether it's active: activated for a node and processing.
    pub active: bool,
    /// The settings it's active for: the engine's rate and block size, or
    /// an offline render's while one has it.
    pub active_for: Option<NodeConfig>,
    /// The latency it reported when last activated, in frames.
    pub latency: u32,
    /// What went wrong with it, if anything: it couldn't be loaded, or
    /// refused to activate.
    pub failure: Option<String>,
    /// Whether it has an editor. Part 1 hosts no editors.
    pub has_editor: bool,
    /// Whether its editor is open.
    pub editor_open: bool,
    /// Whether its open editor is hidden.
    pub editor_hidden: bool,
    /// Each parameter's value, as the controller reports it, in plain
    /// units.
    pub values: Vec<(ParamId, f64)>,
}

/// One of a plug-in's parameters, as the node needs it to turn the
/// device's plain values into the normalized values a process block
/// carries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ParamMap {
    pub(crate) id: u32,
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) default: f64,
    pub(crate) steps: i32,
}

impl ParamMap {
    /// The normalized value of `plain`, over the parameter's plain range
    /// and steps.
    pub(crate) fn normalized(&self, plain: f64) -> f64 {
        let span = self.max - self.min;
        if !span.is_finite() || span <= 0.0 {
            return 0.0;
        }
        let value = ((plain - self.min) / span).clamp(0.0, 1.0);
        if self.steps > 0 {
            let steps = f64::from(self.steps);
            (value * steps).round() / steps
        } else {
            value
        }
    }
}

/// A parameter change a plug-in's controller made itself, such as from its
/// editor: one for each gesture, with its final value, or one for each
/// change outside a gesture (spec: clap-hosting R6, which the VST3 spec
/// mirrors).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamChange {
    /// The device.
    pub device: DeviceId,
    /// The parameter.
    pub param: ParamId,
    /// Its new value, in plain units.
    pub value: f64,
}

/// What a saved state starts with: the component's state and the
/// controller's, each with its length.
const STATE_MAGIC: &[u8; 4] = b"BLTV";

/// The state bytes a VST3 host saves: the component's state and the
/// controller's, each length-prefixed, little-endian, after [`STATE_MAGIC`].
/// Tests and tools use it to hand a plug-in a state, as
/// [`Vst3Host::load_state`] takes one.
#[must_use]
pub fn plugin_state(component: &[u8], controller: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(12 + component.len() + controller.len());
    data.extend_from_slice(STATE_MAGIC);
    for chunk in [component, controller] {
        data.extend_from_slice(&u32::try_from(chunk.len()).unwrap_or(u32::MAX).to_le_bytes());
        data.extend_from_slice(chunk);
    }
    data
}

/// Splits [`plugin_state`]'s bytes back into the component's state and the
/// controller's.
fn split_state(data: &[u8]) -> Option<(&[u8], &[u8])> {
    if data.get(..4) != Some(STATE_MAGIC.as_slice()) {
        return None;
    }
    let length = |at: usize| -> Option<usize> {
        let bytes: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        usize::try_from(u32::from_le_bytes(bytes)).ok()
    };
    let component_len = length(4)?;
    let component = data.get(8..8 + component_len)?;
    let controller_at = 8 + component_len;
    let controller_len = length(controller_at)?;
    let controller = data.get(controller_at + 4..controller_at + 4 + controller_len)?;
    Some((component, controller))
}

/// Things only the owner thread may touch: the plug-in instances, which
/// VST3 hosts on the main thread.
struct MainOnly<T> {
    owner: ThreadId,
    value: T,
}

// SAFETY: the value is reached only through `get`, which checks it's on
// the thread that made it; the instances inside never leave that thread.
unsafe impl<T> Send for MainOnly<T> {}

impl<T> MainOnly<T> {
    fn get(&mut self) -> Result<&mut T, Vst3Error> {
        if std::thread::current().id() == self.owner {
            Ok(&mut self.value)
        } else {
            Err(Vst3Error::WrongThread)
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
    requests: Mutex<Vec<Request>>,
    types: Mutex<Vec<Vst3Type>>,
    main: Mutex<MainOnly<Main>>,
    /// The latency each device's plug-in last reported. The main thread
    /// writes it, and the compiler thread reads it when it makes a node for
    /// a device whose plug-in already runs (after an engine restart, say),
    /// so the node starts with the latency the plug-in has.
    latencies: Mutex<BTreeMap<DeviceId, u32>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The main thread's plug-ins.
struct Main {
    /// One component and controller per device. This map is declared before
    /// `modules` so the plug-ins go before the modules they came from.
    instances: BTreeMap<DeviceId, Instance>,
    /// Instances let go of while a node still had their component, waiting
    /// for it to come back so they can be deactivated and terminated.
    closing: Vec<(DeviceId, Instance)>,
    /// Devices whose plug-in couldn't be made, and why.
    missing: BTreeMap<DeviceId, (String, PathBuf, String)>,
    /// The modules loaded so far, by path. Declared after the instances so
    /// a module outlives every plug-in made from it.
    modules: HashMap<PathBuf, Rc<Module>>,
    /// The host application the plug-ins are initialized with.
    host: ComWrapper<HostApplication>,
    /// The changes the plug-ins' controllers made themselves since the last
    /// call, for the set's undo steps.
    changes: Vec<ParamChange>,
    /// The devices whose plug-in reported a new latency, for the engine's
    /// delay compensation.
    latency_changes: Vec<(DeviceId, u32)>,
    /// An offline render has the plug-ins ([`Vst3Host::lend`]).
    lending: bool,
    /// Devices the render couldn't borrow a plug-in for, and why.
    loan_problems: Vec<(DeviceId, String)>,
}

impl Main {
    fn new() -> Self {
        Self {
            instances: BTreeMap::new(),
            closing: Vec::new(),
            missing: BTreeMap::new(),
            modules: HashMap::new(),
            host: ComWrapper::new(HostApplication),
            changes: Vec::new(),
            latency_changes: Vec::new(),
            lending: false,
            loan_problems: Vec::new(),
        }
    }
}

/// A loaded module: its library, and the factory it gives.
struct Module {
    factory: ComPtr<IPluginFactory>,
    factory2: Option<ComPtr<IPluginFactory2>>,
    /// The module's exit entry point, if the platform's module interface
    /// has one, called before the library is unloaded.
    exit: Option<unsafe extern "system" fn() -> bool>,
    /// On macOS, the bundle the module was loaded from, released after
    /// `bundleExit`.
    #[cfg(target_os = "macos")]
    bundle: *mut c_void,
    /// The module's own library, kept loaded for the factory's lifetime
    /// and dropped (unloaded) after the factory is released.
    #[expect(dead_code, reason = "held for its drop: it unloads the module")]
    library: Library,
}

impl Drop for Module {
    fn drop(&mut self) {
        // The module's exit entry point, if it has one, runs before the
        // library is unloaded; the factory's release runs after that, with
        // the library still loaded.
        if let Some(exit) = self.exit {
            // SAFETY: the module exported it, and it's still loaded.
            unsafe { exit() };
        }
        #[cfg(target_os = "macos")]
        // SAFETY: the reference came from `CFBundleCreate` and is released
        // once, after the module's own bundle exit.
        unsafe {
            cf_release(self.bundle);
        }
    }
}

#[cfg(unix)]
type Library = libloading::os::unix::Library;
#[cfg(windows)]
type Library = libloading::os::windows::Library;

/// One device's plug-in: its component and controller, and the node it
/// runs for.
struct Instance {
    plugin_id: String,
    path: PathBuf,
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: ComPtr<IEditController>,
    /// Whether the component and controller are connected.
    connected: bool,
    /// Whether its controller is a class of its own. A single-component
    /// effect is its own controller: the component is initialized and
    /// terminated once, as VST3's own host does.
    separate: bool,
    /// The flags it asked a restart with, shared with its handler.
    restart: Arc<AtomicU32>,
    /// The gestures its controller made, shared with its handler.
    gestures: Arc<Gestures>,
    /// Its parameters.
    params: Vec<describe::Parameter>,
    /// The values to send it at the first frame of its next block,
    /// normalized.
    pending: Vec<(u32, f64)>,
    /// The node it runs for, and nodes it ran for that haven't given their
    /// component back yet.
    ends: Option<HostEnds>,
    retired: Vec<HostEnds>,
    config: Option<NodeConfig>,
    /// The handover a node has, if it has one.
    handed: Option<u64>,
    /// Counts the handovers.
    next_generation: u64,
    /// Activation failed for this node; it isn't tried again until the
    /// next node.
    refused: Option<String>,
    /// It failed while processing: its device is silent, and it isn't
    /// activated again until the next node.
    silenced: bool,
    failure: Option<String>,
    latency: u32,
    /// The offline render that has it.
    lent: Option<Lent>,
    /// The host's component handler, which the controller reports its own
    /// changes and restart asks to. Declared last, so it outlives both the
    /// controller that holds it and the component it is set through.
    #[expect(
        dead_code,
        reason = "held for its lifetime: the controller keeps a pointer to it"
    )]
    handler: ComWrapper<ComponentHandler>,
}

/// A plug-in lent to an offline render: the render's node, and what to
/// give back to the engine afterwards.
struct Lent {
    ends: HostEnds,
    /// The rate and block size it is activated for in the render.
    config: NodeConfig,
    /// The plug-in's state before the render, which it gets back.
    saved: Option<Vec<u8>>,
    /// Why the engine's node had it refused, if it had.
    refused: Option<String>,
}

/// How long a render waits for the engine's node to hand a plug-in back:
/// a node does at its next block, a few milliseconds away.
const HAND_BACK_WAIT: Duration = Duration::from_secs(2);

/// The VST3 host: VST3's main thread for every plug-in, on the thread that
/// made it.
///
/// Its methods run only on that thread and return [`Vst3Error::WrongThread`]
/// elsewhere. [`Vst3Host::idle`] must run there often (each frame, as the
/// shell's poll does) for new nodes to get their plug-ins.
#[derive(Clone)]
pub struct Vst3Host {
    inner: Arc<Inner>,
}

impl Default for Vst3Host {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Vst3Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vst3Host").finish_non_exhaustive()
    }
}

impl Vst3Host {
    /// A host with no plug-ins, whose main thread is this one.
    #[must_use]
    pub fn new() -> Self {
        let owner = std::thread::current().id();
        Self {
            inner: Arc::new(Inner {
                owner,
                requests: Mutex::new(Vec::new()),
                types: Mutex::new(Vec::new()),
                main: Mutex::new(MainOnly {
                    owner,
                    value: Main::new(),
                }),
                latencies: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    /// The factory for the engine's registry, which makes a node for each
    /// VST3 device.
    #[must_use]
    pub fn factory(&self) -> Vst3Factory {
        Vst3Factory {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Loads the VST3 bundle or module at `path` and returns its audio
    /// classes as device types, which devices can then be made of.
    ///
    /// # Errors
    ///
    /// [`Vst3Error::Load`] if it isn't a VST3 plug-in or offers no audio
    /// classes.
    pub fn load(&self, path: &Path) -> Result<Vec<Vst3Type>, Vst3Error> {
        let mut guard = lock(&self.inner.main);
        let main = guard.get()?;
        let module = module(main, path)?;
        let mut found = Vec::new();
        // SAFETY: the factory reports its own class count.
        let count = unsafe { module.factory.countClasses() }.max(0);
        for index in 0..count {
            // SAFETY: an all-zero class info is valid; the factory fills
            // its fields.
            let mut info: PClassInfo = unsafe { std::mem::zeroed() };
            // SAFETY: the factory reports its classes by index.
            if unsafe { module.factory.getClassInfo(index, &mut info) } != kResultOk {
                continue;
            }
            if cstring(&info.category) != "Audio Module Class" {
                continue;
            }
            let cid = info.cid;
            let mut subcategories = String::new();
            let mut vendor = None;
            let mut version = None;
            if let Some(factory2) = &module.factory2 {
                // SAFETY: as above.
                let mut info2: PClassInfo2 = unsafe { std::mem::zeroed() };
                // SAFETY: as the class info above.
                if unsafe { factory2.getClassInfo2(index, &mut info2) } == kResultOk {
                    subcategories = cstring(&info2.subCategories);
                    vendor = Some(cstring(&info2.vendor)).filter(|v| !v.is_empty());
                    version = Some(cstring(&info2.version)).filter(|v| !v.is_empty());
                }
            }
            let name = cstring(&info.name);
            let name = if name.is_empty() {
                format_fuid(&cid)
            } else {
                name
            };
            // A probe instance reads the class's parameters; it's made and
            // terminated like any other, so the factory's own code sees a
            // real lifecycle.
            let mut probe = new_instance(main, &module, &cid)?;
            let params: Vec<_> = probe.params.iter().map(|p| p.descriptor.clone()).collect();
            teardown(&mut probe);
            drop(probe);
            found.push(Vst3Type {
                path: path.to_owned(),
                device_type: DeviceType {
                    kind: DeviceKind::Plugin {
                        format: PluginFormat::Vst3,
                        plugin_id: format_fuid(&cid),
                    },
                    name,
                    role: describe::role(&subcategories),
                    description: DeviceDescription::new(params),
                    sidechain: false,
                    midi_sidechain: false,
                },
                vendor,
                version,
            });
        }
        drop(guard);
        if found.is_empty() {
            return Err(Vst3Error::Load("it offers no audio classes".to_owned()));
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
    pub fn types(&self) -> Vec<Vst3Type> {
        lock(&self.inner.types).clone()
    }

    /// Does the main thread's work: gives new nodes their plug-ins,
    /// deactivates plug-ins their nodes gave back, restarts plug-ins that
    /// asked, tells their controllers what their nodes applied, and
    /// collects both the changes the controllers made themselves and the
    /// latencies they reported. Returns whether anything a window shows may
    /// have changed.
    pub fn idle(&self) -> bool {
        let requests = std::mem::take(&mut *lock(&self.inner.requests));
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else { return false };
        let mut changed = !requests.is_empty();
        for request in requests {
            serve(main, request, &self.inner);
        }
        let mut changes = Vec::new();
        let mut latency_changes = Vec::new();
        let mut latencies = lock(&self.inner.latencies);
        for (&device, instance) in &mut main.instances {
            changed |= pump(
                device,
                instance,
                &mut changes,
                &mut latency_changes,
                &mut latencies,
            );
        }
        for (device, instance) in &mut main.closing {
            changed |= pump(
                *device,
                instance,
                &mut changes,
                &mut latency_changes,
                &mut latencies,
            );
            if instance.handed.is_none() {
                teardown(instance);
            }
        }
        main.closing
            .retain(|(_, instance)| instance.handed.is_some());
        changed |= !changes.is_empty() || !latency_changes.is_empty();
        main.changes.extend(changes);
        main.latency_changes.extend(latency_changes);
        changed
    }

    /// The parameter changes the plug-ins' controllers made themselves
    /// since the last call: each gesture once, with its final value, in
    /// plain units.
    pub fn take_param_changes(&self) -> Vec<ParamChange> {
        let mut guard = lock(&self.inner.main);
        guard
            .get()
            .map(|main| std::mem::take(&mut main.changes))
            .unwrap_or_default()
    }

    /// The devices whose plug-in reported a new latency when it was last
    /// activated since the last call, with the latency in frames: the
    /// engine recompiles with it, so delay compensation follows a plug-in
    /// that changed its own.
    pub fn take_latency_changes(&self) -> Vec<(DeviceId, u32)> {
        let mut guard = lock(&self.inner.main);
        guard
            .get()
            .map(|main| std::mem::take(&mut main.latency_changes))
            .unwrap_or_default()
    }

    /// The device's plug-in's own state, with the file it came from, as the
    /// set saves it: the component's state and the controller's, together.
    ///
    /// # Errors
    ///
    /// [`Vst3Error::NoInstance`] if the device has no plug-in running, and
    /// [`Vst3Error::Refused`] if the plug-in won't save.
    pub fn save_state(&self, device: DeviceId) -> Result<PluginState, Vst3Error> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        let (component, controller) = saved_state(instance)?;
        Ok(PluginState {
            path: instance.path.to_string_lossy().into_owned(),
            data: plugin_state(&component, &controller),
        })
    }

    /// Gives the device's plug-in a state it saved before: the component's
    /// state, the controller's, and the values the component's mean for the
    /// controller in between. The plug-in keeps the state it had if it
    /// refuses.
    ///
    /// # Errors
    ///
    /// [`Vst3Error::NoInstance`] if the device has no plug-in running, and
    /// [`Vst3Error::Refused`] if the plug-in refuses the state, naming
    /// which part.
    pub fn load_state(&self, device: DeviceId, data: &[u8]) -> Result<(), Vst3Error> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        load_state(instance, data)?;
        // A state that changes its latency, say, asks for a restart; a
        // plug-in that refused to activate, or failed, gets another try.
        instance.refused = None;
        instance.silenced = false;
        Ok(())
    }

    /// What the device's plug-in is doing, or `None` if the device has no
    /// VST3 plug-in, running or failed.
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
                values: Vec::new(),
            });
        }
        let instance = main.instances.get(&device)?;
        let values = instance
            .params
            .iter()
            .map(|param| {
                let id = param.map.id;
                // SAFETY: the controller reports its own parameter's value.
                let normalized = unsafe { instance.controller.getParamNormalized(id) };
                // SAFETY: as above.
                let plain = unsafe { instance.controller.normalizedParamToPlain(id, normalized) };
                (ParamId(id), plain)
            })
            .collect();
        Some(PluginStatus {
            plugin_id: instance.plugin_id.clone(),
            path: instance.path.clone(),
            active: instance.handed.is_some(),
            active_for: instance
                .lent
                .as_ref()
                .map_or(instance.config, |lent| Some(lent.config))
                .filter(|_| instance.handed.is_some()),
            latency: instance.latency,
            failure: instance
                .failure
                .clone()
                .or_else(|| instance.refused.clone()),
            has_editor: false,
            editor_open: false,
            editor_hidden: false,
            values,
        })
    }

    /// Lets go of every plug-in whose device isn't in `keep`, as when a set
    /// closes. Their nodes must be gone first; any still running keeps its
    /// plug-in until it's dropped.
    pub fn retain(&self, keep: &BTreeSet<DeviceId>) {
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else { return };
        main.missing.retain(|device, _| keep.contains(device));
        let gone: Vec<DeviceId> = main
            .instances
            .keys()
            .filter(|d| !keep.contains(d))
            .copied()
            .collect();
        for device in gone {
            if let Some(instance) = main.instances.remove(&device) {
                close(main, device, instance);
            }
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
    /// [`factory`](Loan::factory) goes into the render's registry; for
    /// each VST3 device the render makes a node for, it takes the plug-in
    /// back from the engine's node (waiting for that node's next block),
    /// deactivates it, sets it to the rendered set's parameter values, and
    /// activates it for the render's rate and block size. A device with no
    /// plug-in yet gets one, from its saved state. Meanwhile the engine's
    /// node passes its input through.
    ///
    /// When the loan is dropped, after the render's engine, every plug-in
    /// goes back to the engine with the state it had before, activated for
    /// the engine's rate and block size again; one that failed while
    /// rendering stays silent in the engine too.
    ///
    /// # Errors
    ///
    /// [`Vst3Error::WrongThread`] off the main thread, and
    /// [`Vst3Error::Lent`] while another loan is out.
    pub fn lend(&self) -> Result<Loan, Vst3Error> {
        let mut guard = lock(&self.inner.main);
        let main = guard.get()?;
        if main.lending {
            return Err(Vst3Error::Lent);
        }
        main.lending = true;
        main.loan_problems.clear();
        Ok(Loan {
            inner: Arc::clone(&self.inner),
            _main_thread: std::marker::PhantomData,
        })
    }
}

/// The host's plug-ins, lent to an offline render ([`Vst3Host::lend`]).
/// Dropping it, on the main thread and after the render's engine, gives
/// every plug-in back to the engine.
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
    /// The factory for the render's registry: it makes each VST3 device's
    /// node with the device's own plug-in, borrowed from the engine.
    #[must_use]
    pub fn factory(&self) -> LoanFactory {
        LoanFactory {
            inner: Arc::clone(&self.inner),
        }
    }

    /// The devices whose plug-in the render couldn't borrow, and why: the
    /// engine's node didn't hand it back in time. Their nodes pass their
    /// input through, so the render isn't what the engine plays.
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
            format: PluginFormat::Vst3,
            ..
        } = &device.kind
        else {
            return None;
        };
        let (node, ends) = Vst3Node::new(config, 0);
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

/// The VST3 plug-ins' factory for the engine's registry. It makes each VST3
/// device's node at once; the host gives the node its active component.
pub struct Vst3Factory {
    inner: Arc<Inner>,
}

impl NodeFactory for Vst3Factory {
    fn create(&self, device: &Device, config: &NodeConfig) -> Option<Box<dyn Node>> {
        let DeviceKind::Plugin {
            format: PluginFormat::Vst3,
            ..
        } = &device.kind
        else {
            return None;
        };
        // A plug-in that already runs keeps its latency: the node reads it
        // at once, so a program compiled after an engine restart compensates
        // the same way as one compiled before it.
        let latency = lock(&self.inner.latencies).get(&device.id).copied();
        let (node, ends) = Vst3Node::new(config, latency.unwrap_or(0));
        lock(&self.inner.requests).push(Request {
            device: device.clone(),
            config: *config,
            ends,
        });
        if std::thread::current().id() == self.inner.owner {
            Vst3Host {
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

/// The module at `path`, loaded once.
fn module(main: &mut Main, path: &Path) -> Result<Rc<Module>, Vst3Error> {
    if let Some(module) = main.modules.get(path) {
        return Ok(Rc::clone(module));
    }
    let loaded = Rc::new(load_module(path)?);
    main.modules.insert(path.to_owned(), Rc::clone(&loaded));
    Ok(loaded)
}

/// Loads the module and gets its factory, calling the module's entry
/// points where the platform has them. On macOS the host loads the bundle's
/// binary itself, so the bundle's entry point, which wants a `CFBundleRef`,
/// isn't called.
fn load_module(path: &Path) -> Result<Module, Vst3Error> {
    let binary = module_binary(path)?;
    // SAFETY: loading a plug-in runs its code in this process, which is
    // what hosting it in process means (the architecture, "Plug-in
    // hosting"); the module's own code is its own.
    let library = unsafe { Library::new(&binary) }.map_err(|e| Vst3Error::Load(e.to_string()))?;
    #[cfg(target_os = "windows")]
    {
        // SAFETY: the module exports it or it doesn't; that's all the VST3
        // module entry is.
        let init = unsafe { library.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") }
            .ok()
            .map(|symbol| *symbol);
        if let Some(init) = init {
            // SAFETY: the module exported it, and it's still loaded.
            let _ = unsafe { init() };
        }
    }
    // On platforms whose module entry point wants the module's own handle,
    // the library is asked for it and put back.
    #[cfg(unix)]
    let library = {
        // SAFETY: the module exports it or it doesn't; the entry point gets
        // the module's own handle, which stays loaded.
        let entry = unsafe {
            library.get::<unsafe extern "system" fn(*mut c_void) -> bool>(b"ModuleEntry\0")
        }
        .ok()
        .map(|symbol| *symbol);
        match entry {
            Some(entry) => library_handle(library, |handle| {
                // SAFETY: the module exported the entry point, which gets
                // the module's own handle.
                let _ = unsafe { entry(handle) };
            }),
            None => library,
        }
    };
    #[cfg(target_os = "macos")]
    let bundle = {
        // The bundle's entry point gets the `CFBundleRef` of the `.vst3`
        // bundle the module came from, as the VST3 specification has it.
        let bundle = bundle_ref(path);
        // SAFETY: the module exports it or it doesn't.
        let entry = unsafe {
            library.get::<unsafe extern "system" fn(*mut c_void) -> bool>(b"bundleEntry\0")
        }
        .ok()
        .map(|symbol| *symbol);
        if let Some(entry) = entry {
            // SAFETY: the entry point gets the bundle, which is alive.
            let _ = unsafe { entry(bundle) };
        }
        bundle
    };
    // SAFETY: the module's mandatory entry point, which the VST3
    // specification has it export.
    let factory = unsafe {
        let get = library
            .get::<unsafe extern "system" fn() -> *mut IPluginFactory>(b"GetPluginFactory\0")
            .map_err(|e| Vst3Error::Load(format!("it has no plug-in factory: {e}")))?;
        get()
    };
    if factory.is_null() {
        return Err(Vst3Error::Load("its factory is empty".to_owned()));
    }
    // SAFETY: a non-null factory, whose reference count the host takes.
    let factory = unsafe { ComPtr::from_raw_unchecked(factory) };
    let factory2 = factory.cast::<IPluginFactory2>();
    let exit = module_exit(&library);
    Ok(Module {
        factory,
        factory2,
        exit,
        #[cfg(target_os = "macos")]
        bundle,
        library,
    })
}

/// The library, reconstructed after being asked for its raw handle on
/// platforms whose module entry point wants one.
#[cfg(unix)]
fn library_handle<R>(library: Library, use_handle: impl FnOnce(*mut c_void) -> R) -> Library {
    let raw = library.into_raw();
    let handle = raw.cast::<c_void>();
    let result = use_handle(handle);
    let _ = result;
    // SAFETY: the raw handle came from `into_raw` just above.
    unsafe { Library::from_raw(raw) }
}

/// The module's exit entry point, if it has one: `ExitDll` on Windows,
/// `ModuleExit` on Linux, and `bundleExit` on macOS.
fn module_exit(library: &Library) -> Option<unsafe extern "system" fn() -> bool> {
    #[cfg(target_os = "windows")]
    let name: &[u8] = b"ExitDll\0";
    #[cfg(all(unix, not(target_os = "macos")))]
    let name: &[u8] = b"ModuleExit\0";
    #[cfg(target_os = "macos")]
    let name: &[u8] = b"bundleExit\0";
    // SAFETY: the symbol lookup is the module's own export or nothing.
    unsafe { library.get::<unsafe extern "system" fn() -> bool>(name) }
        .ok()
        .map(|symbol| *symbol)
}

/// The `CFBundleRef` of the `.vst3` bundle a module came from, as null if
/// none was found or it can't be made.
#[cfg(target_os = "macos")]
fn bundle_ref(path: &Path) -> *mut c_void {
    // The bundle itself, or the `.vst3` folder on the way to the file the
    // loader was given.
    let bundle = if path.is_dir() {
        path
    } else {
        match path
            .ancestors()
            .find(|ancestor| ancestor.to_string_lossy().ends_with(".vst3"))
        {
            Some(ancestor) => ancestor,
            None => return ptr::null_mut(),
        }
    };
    // SAFETY: the path's bytes are read as a CFURL, and both the URL and
    // the bundle it makes are released here or in `Module::drop`.
    unsafe {
        let bytes = bundle.as_os_str().as_encoded_bytes();
        let url = CFURLCreateFromFileSystemRepresentation(
            ptr::null(),
            bytes.as_ptr(),
            isize::try_from(bytes.len()).unwrap_or(0),
            1,
        );
        if url.is_null() {
            return ptr::null_mut();
        }
        let bundle = CFBundleCreate(ptr::null(), url);
        cf_release(url);
        bundle
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: *const c_void,
        buffer: *const u8,
        length: isize,
        is_directory: u8,
    ) -> *mut c_void;
    fn CFBundleCreate(allocator: *const c_void, url: *const c_void) -> *mut c_void;
    fn CFRelease(reference: *const c_void);
}

/// Releases a Core Foundation reference, if it isn't null.
#[cfg(target_os = "macos")]
unsafe fn cf_release(reference: *mut c_void) {
    if !reference.is_null() {
        // SAFETY: the caller hands over a reference it owns, once.
        unsafe { CFRelease(reference) };
    }
}

/// The binary inside a `.vst3` bundle, or the file given.
fn module_binary(path: &Path) -> Result<PathBuf, Vst3Error> {
    if path.is_file() {
        return Ok(path.to_owned());
    }
    if !path.is_dir() {
        return Err(Vst3Error::Load(format!("{} isn't there", path.display())));
    }
    let folder = if cfg!(target_os = "macos") {
        "MacOS"
    } else if cfg!(target_os = "windows") {
        if cfg!(target_arch = "aarch64") {
            "arm64-win"
        } else {
            "x86_64-win"
        }
    } else if cfg!(target_arch = "aarch64") {
        "arm64-linux"
    } else {
        "x86_64-linux"
    };
    let contents = path.join("Contents").join(folder);
    let mut found: Vec<PathBuf> = std::fs::read_dir(&contents)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|entry| entry.is_file())
        .collect();
    found.sort();
    found.into_iter().next().ok_or_else(|| {
        Vst3Error::Load(format!(
            "{} has no module under Contents/{folder}",
            path.display()
        ))
    })
}

/// Makes the class's instance: a component and its controller, both
/// initialized, the controller given its own component handler, and the
/// two connected.
fn new_instance(main: &Main, module: &Module, cid: &TUID) -> Result<Instance, Vst3Error> {
    let component = create_instance::<IComponent>(module, cid)?;
    let host = main
        .host
        .as_com_ref::<FUnknown>()
        .map_or(ptr::null_mut(), |host| host.as_ptr());
    // SAFETY: the component is a live VST3 object and gets the host's
    // application, which outlives it.
    if unsafe { component.initialize(host) } != kResultOk {
        return Err(Vst3Error::Refused("to initialize".to_owned()));
    }
    let controller_cid = {
        let mut cid = [0 as c_char; 16];
        // SAFETY: the component reports the controller class it wants, or
        // says it has none.
        if unsafe { component.getControllerClassId(&mut cid) } == kResultOk {
            Some(cid)
        } else {
            None
        }
    };
    let separate = controller_cid.is_some();
    let controller = match controller_cid {
        Some(cid) => create_instance::<IEditController>(module, &cid)?,
        // A single-component effect is its own controller.
        None => component
            .cast::<IEditController>()
            .ok_or_else(|| Vst3Error::Refused("to say what its controller is".to_owned()))?,
    };
    if separate {
        // SAFETY: the controller is a live object of its own and gets the
        // host's application, which outlives it. A single-component
        // effect's controller is its component, already initialized once,
        // so it isn't initialized again.
        if unsafe { controller.initialize(host) } != kResultOk {
            return Err(Vst3Error::Refused(
                "to initialize its controller".to_owned(),
            ));
        }
    }
    // Each instance has a component handler of its own, so the gestures
    // and restart asks it records belong to this device (spec:
    // clap-hosting R6, R10, which the VST3 spec mirrors).
    let gestures = Arc::new(Gestures::default());
    let restart = Arc::new(AtomicU32::new(0));
    let handler = ComWrapper::new(ComponentHandler::new(
        Arc::clone(&gestures),
        Arc::clone(&restart),
    ));
    if let Some(handler) = handler.as_com_ref::<IComponentHandler>() {
        // SAFETY: the handler is this instance's own object, which outlives
        // the controller it is set on.
        unsafe { controller.setComponentHandler(handler.as_ptr()) };
    }
    // A single-component effect is its own controller, so there is nothing
    // to connect: an object connected to itself isn't the two ends VST3's
    // connection points are for.
    let connected = separate
        && match (
            component.cast::<IConnectionPoint>(),
            controller.cast::<IConnectionPoint>(),
        ) {
            (Some(component_side), Some(controller_side)) => {
                // SAFETY: both are live objects of the interface.
                (unsafe { component_side.connect(controller_side.as_ptr()) }) == kResultOk
            }
            _ => false,
        };
    let params = describe::parameters(&controller);
    let processor = component
        .cast::<IAudioProcessor>()
        .ok_or_else(|| Vst3Error::Refused("to be an audio processor".to_owned()))?;
    Ok(Instance {
        plugin_id: format_fuid(cid),
        path: PathBuf::new(),
        component,
        processor,
        controller,
        connected,
        separate,
        restart,
        gestures,
        params,
        pending: Vec::new(),
        ends: None,
        retired: Vec::new(),
        config: None,
        handed: None,
        next_generation: 0,
        refused: None,
        silenced: false,
        failure: None,
        latency: 0,
        lent: None,
        handler,
    })
}

/// Asks a module's factory for a class as one of its interfaces.
fn create_instance<T: vst3::Interface>(
    module: &Module,
    cid: &TUID,
) -> Result<ComPtr<T>, Vst3Error> {
    let mut object: *mut c_void = ptr::null_mut();
    // SAFETY: the factory is loaded, and the class ID, interface ID and out
    // pointer are valid; the factory's own code is its own.
    let result = unsafe {
        module
            .factory
            .createInstance(cid.as_ptr(), T::IID.as_ptr().cast::<c_char>(), &mut object)
    };
    if result != kResultOk || object.is_null() {
        return Err(Vst3Error::Refused("to make an instance".to_owned()));
    }
    // SAFETY: a non-null object of the asked interface, whose reference
    // count the host takes.
    Ok(unsafe { ComPtr::from_raw_unchecked(object as *mut T) })
}

/// Disconnects and terminates an instance's component and controller, on
/// the main thread.
fn teardown(instance: &mut Instance) {
    if instance.connected {
        if let (Some(component_side), Some(controller_side)) = (
            instance.component.cast::<IConnectionPoint>(),
            instance.controller.cast::<IConnectionPoint>(),
        ) {
            // SAFETY: both are live objects of the interface.
            unsafe { component_side.disconnect(controller_side.as_ptr()) };
        }
        instance.connected = false;
    }
    // SAFETY: the controller and component are live objects, and their
    // termination runs on the main thread, as VST3 asks. A single-component
    // effect is its own controller, so it terminates once, as its
    // component.
    unsafe {
        instance.component.terminate();
        if instance.separate {
            instance.controller.terminate();
        }
    }
}

/// The device's running plug-in, or why it has none.
fn instance(main: &mut Main, device: DeviceId) -> Result<&mut Instance, Vst3Error> {
    main.instances
        .get_mut(&device)
        .ok_or(Vst3Error::NoInstance(device))
}

/// Saves the component's state and the controller's, each on the main
/// thread, as [`Vst3Host::save_state`] reports them.
fn saved_state(instance: &Instance) -> Result<(Vec<u8>, Vec<u8>), Vst3Error> {
    let component = stream::save(|state| {
        // SAFETY: the component is a live object and writes into the host's
        // stream, which lives through the call.
        unsafe { instance.component.getState(state) }
    })?;
    // A single-component effect is its own controller, so its state is its
    // component's, saved once.
    let controller = if instance.separate {
        stream::save(|state| {
            // SAFETY: the controller is a live object and writes into the
            // host's stream, which lives through the call.
            unsafe { instance.controller.getState(state) }
        })?
    } else {
        Vec::new()
    };
    Ok((component, controller))
}

/// Gives the plug-in a saved state: the component's, what it means for the
/// controller, then the controller's own (spec: vst3-hosting R9, R11, as
/// clap-hosting's R9, R11).
fn load_state(instance: &mut Instance, data: &[u8]) -> Result<(), Vst3Error> {
    let (component, controller) =
        split_state(data).ok_or_else(|| Vst3Error::Refused("the state".to_owned()))?;
    let result = stream::load(component, |state| {
        // SAFETY: the component is a live object and reads the host's
        // stream, which lives through the call.
        unsafe { instance.component.setState(state) }
    });
    if result != kResultOk {
        return Err(Vst3Error::Refused("the component's state".to_owned()));
    }
    if instance.separate {
        // The controller's values follow the component's, as VST3's own
        // host asks. A controller that can't take it keeps its values: the
        // set's own parameter values are applied over them next (R11).
        stream::load(component, |state| {
            // SAFETY: the controller is a live object and reads the host's
            // stream, which lives through the call.
            unsafe { instance.controller.setComponentState(state) }
        });
        if !controller.is_empty() {
            let result = stream::load(controller, |state| {
                // SAFETY: as above.
                unsafe { instance.controller.setState(state) }
            });
            if result != kResultOk {
                return Err(Vst3Error::Refused("the controller's state".to_owned()));
            }
        }
    }
    Ok(())
}

/// Gives a new node its plug-in, making the plug-in if the device has none.
fn serve(main: &mut Main, request: Request, inner: &Inner) {
    let Request {
        device,
        config,
        ends,
    } = request;
    if !ensure_instance(main, &device, inner) {
        return;
    }
    let Some(instance) = main.instances.get_mut(&device.id) else {
        return;
    };
    if let Some(old) = instance.ends.take() {
        old.status.hand_back.store(true, Relaxed);
        instance.retired.push(old);
    }
    instance.ends = Some(ends);
    instance.config = Some(config);
    instance.refused = None;
    instance.silenced = false;
}

/// Lends the device's plug-in to an offline render's node (WP-4.21): takes
/// it back from the engine's node, and activates it for the render's
/// settings with the rendered set's parameter values. A plug-in that can't
/// be made or won't activate leaves the node passing its input through;
/// one that failed while processing leaves it silent.
fn borrow(
    main: &mut Main,
    device: &Device,
    config: NodeConfig,
    ends: HostEnds,
    inner: &Inner,
) -> Result<(), String> {
    if !ensure_instance(main, device, inner) {
        // Spec: clap-hosting R12, which the VST3 spec mirrors.
        return Ok(());
    }
    let Some(instance) = main.instances.get_mut(&device.id) else {
        return Ok(());
    };
    if instance.lent.is_some() {
        return Err("its plug-in is lent to this render already".to_owned());
    }
    // Spec: offline-plugin-renders R4. Its state, to give back.
    let saved = saved_state(instance)
        .ok()
        .map(|(component, controller)| plugin_state(&component, &controller));
    // Spec: offline-plugin-renders R2. The engine's node hands it back at
    // its next block.
    if instance.handed.is_some() {
        for ends in instance.ends.iter().chain(&instance.retired) {
            ends.status.hand_back.store(true, Relaxed);
        }
        let until = Instant::now() + HAND_BACK_WAIT;
        loop {
            take_back(instance);
            if instance.handed.is_none() {
                break;
            }
            if Instant::now() >= until {
                return Err(
                    "the engine playing the set didn't hand its plug-in back in time".to_owned(),
                );
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let mut lent = Lent {
        ends,
        config,
        saved,
        refused: instance.refused.take(),
    };
    if instance.silenced {
        // Spec: offline-plugin-renders R6. Failed, it stays silent.
        lent.ends.status.silent.store(true, Relaxed);
        instance.lent = Some(lent);
        return Ok(());
    }
    // Spec: offline-plugin-renders R3. The rendered set's values, and the
    // render's rate and block size.
    set_values(instance, device);
    // Spec: clap-hosting R12, which the VST3 spec mirrors. One that won't
    // activate for the render leaves its node passing its input through.
    if let Ok(active) = activate(instance, &config) {
        lent.ends.status.latency.store(active.latency, Relaxed);
        let generation = active.generation;
        if lent.ends.to_node.push(active).is_ok() {
            instance.handed = Some(generation);
        }
    }
    instance.lent = Some(lent);
    Ok(())
}

/// Ends a loan: every lent plug-in goes back to the engine's node with the
/// state it had, or stays silent if it failed while rendering.
fn end_loan(main: &mut Main, inner: &Inner) {
    if !main.lending {
        return;
    }
    main.lending = false;
    main.loan_problems.clear();
    let Main {
        instances,
        changes,
        latency_changes,
        ..
    } = main;
    let mut latencies = lock(&inner.latencies);
    for (&device, instance) in instances {
        let Some(lent) = &instance.lent else { continue };
        lent.ends.status.hand_back.store(true, Relaxed);
        // The render's engine is gone, and its node gave the component
        // back as it went; one it never ran leaves its queue with it.
        take_back(instance);
        if instance.handed.is_some()
            && instance
                .lent
                .as_ref()
                .is_some_and(|lent| lent.ends.to_node.is_abandoned())
        {
            deactivate(instance);
            instance.handed = None;
        }
        let Some(lent) = instance.lent.take() else {
            continue;
        };
        let failed = lent.ends.status.failed.load(Relaxed);
        if instance.handed.is_some() {
            // A render's node still holds it: it's deactivated once that
            // node gives it back.
            instance.retired.push(lent.ends);
        }
        // Spec: offline-plugin-renders R4. The state it had; what the
        // render did to its parameters is dropped.
        if let Some(saved) = &lent.saved {
            let _ = load_state(instance, saved);
        }
        instance.gestures.take(|_, _| {});
        instance.refused = lent.refused;
        if failed {
            // Spec: offline-plugin-renders R6, clap-hosting R12.
            instance.silenced = true;
            instance.failure = Some(
                "it failed while processing in an offline render; the device is silent".to_owned(),
            );
            if let Some(ends) = &instance.ends {
                ends.status.silent.store(true, Relaxed);
            }
        }
        pump(device, instance, changes, latency_changes, &mut latencies);
    }
}

/// Makes the device's plug-in if it has none. Returns whether it has one;
/// one that can't be made is recorded as missing.
fn ensure_instance(main: &mut Main, device: &Device, inner: &Inner) -> bool {
    let DeviceKind::Plugin { plugin_id, .. } = &device.kind else {
        return false;
    };
    // A plug-in left from another set under the same ID is let go of.
    if main
        .instances
        .get(&device.id)
        .is_some_and(|instance| instance.plugin_id != *plugin_id)
        && let Some(old) = main.instances.remove(&device.id)
    {
        close(main, device.id, old);
    }
    if main.instances.contains_key(&device.id) {
        return true;
    }
    let path = device
        .plugin
        .as_ref()
        .map(|p| PathBuf::from(&p.path))
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| {
            lock(&inner.types)
                .iter()
                .find(|t| t.device_type.kind == device.kind)
                .map(|t| t.path.clone())
        });
    let made = parse_fuid(plugin_id)
        .ok_or_else(|| Vst3Error::UnknownPlugin(plugin_id.clone()))
        .and_then(|cid| {
            let path = path
                .clone()
                .ok_or_else(|| Vst3Error::UnknownPlugin(plugin_id.clone()))?;
            let module = module(main, &path)?;
            new_instance(main, &module, &cid).map(|instance| (instance, path))
        });
    let (mut instance, path) = match made {
        Ok(made) => made,
        Err(error) => {
            // Spec: clap-hosting R12, which the VST3 spec mirrors: the
            // device stays, passing its input through.
            main.missing.insert(
                device.id,
                (
                    plugin_id.clone(),
                    path.unwrap_or_default(),
                    error.to_string(),
                ),
            );
            return false;
        }
    };
    instance.path = path;
    main.missing.remove(&device.id);
    // Spec: vst3-hosting R9, R11 (as clap-hosting's R9, R11): the saved
    // state first, then the set's parameter values over it.
    if let Some(saved) = device.plugin.as_ref().filter(|p| !p.data.is_empty())
        && let Err(error) = load_state(&mut instance, &saved.data)
    {
        instance.failure = Some(format!("{error}; it starts from its defaults"));
    }
    set_values(&mut instance, device);
    main.instances.insert(device.id, instance);
    true
}

/// Sets the controller's values to the device's plain ones, while it's
/// inactive, and keeps them for the plug-in's first block.
fn set_values(instance: &mut Instance, device: &Device) {
    instance.pending.clear();
    for (param, value) in &device.params {
        let id = param.0;
        if !instance.params.iter().any(|p| p.map.id == id) {
            continue;
        }
        // SAFETY: the controller maps its own parameter's value, by ID.
        let normalized = unsafe { instance.controller.plainParamToNormalized(id, *value) };
        let normalized = if normalized.is_finite() {
            normalized.clamp(0.0, 1.0)
        } else {
            0.0
        };
        // SAFETY: as above.
        if unsafe { instance.controller.setParamNormalized(id, normalized) } != kResultOk {
            continue;
        }
        instance.pending.push((id, *value));
    }
}

/// A VST3 enum value as the `int32` a VST3 call takes.
///
/// The bindings type the C enums by the platform's default (`int` on
/// Windows, `unsigned int` elsewhere), so the same conversion is needed on
/// one platform and rejected as useless on the other. The values involved
/// are small constants; nothing is lost.
fn int32_of(value: impl TryInto<i32>) -> i32 {
    value.try_into().unwrap_or_default()
}

/// Activates an instance for a node: the sample size, the bus
/// arrangements, processing setup, activation and processing start, in the
/// VST3 order, then an [`Active`](node::Active) for the node.
fn activate(instance: &mut Instance, config: &NodeConfig) -> Result<node::Active, String> {
    let processor = &instance.processor;
    let audio = int32_of(MediaTypes_::kAudio);
    let input = int32_of(BusDirections_::kInput);
    let output = int32_of(BusDirections_::kOutput);
    let samples32 = int32_of(SymbolicSampleSizes_::kSample32);
    let realtime = int32_of(ProcessModes_::kRealtime);
    // SAFETY: the processor is a live object and these are its own calls,
    // from the main thread.
    if unsafe { processor.canProcessSampleSize(samples32) } != kResultOk {
        return Err("it doesn't process 32-bit audio".to_owned());
    }
    let mut inputs = [SpeakerArr::kStereo];
    let mut outputs = [SpeakerArr::kStereo];
    // The main stereo arrangement is asked for; a plug-in that refuses it
    // keeps its own, and its channel counts are read below either way.
    // SAFETY: as above.
    unsafe { processor.setBusArrangements(inputs.as_mut_ptr(), 1, outputs.as_mut_ptr(), 1) };
    let setup = ProcessSetup {
        processMode: realtime,
        symbolicSampleSize: samples32,
        maxSamplesPerBlock: i32::try_from(config.max_frames.max(1)).unwrap_or(i32::MAX),
        sampleRate: f64::from(config.sample_rate.hz()),
    };
    let mut setup = setup;
    // SAFETY: as above.
    if unsafe { processor.setupProcessing(&raw mut setup) } != kResultOk {
        return Err("it refused to set up processing".to_owned());
    }
    let component = &instance.component;
    // SAFETY: as above.
    let inputs_count = unsafe { component.getBusCount(audio, input) }.clamp(0, MAX_BUSES as i32);
    // SAFETY: as above.
    let outputs_count = unsafe { component.getBusCount(audio, output) }.clamp(0, MAX_BUSES as i32);
    let mut in_channels = [0usize; MAX_BUSES];
    let mut out_channels = [0usize; MAX_BUSES];
    for bus in 0..inputs_count {
        // SAFETY: an all-zero bus info is valid; the component fills it.
        let mut info: BusInfo = unsafe { std::mem::zeroed() };
        // SAFETY: the component reports its buses by index.
        if unsafe { component.getBusInfo(audio, input, bus, &mut info) } == kResultOk {
            in_channels[bus as usize] = usize::try_from(info.channelCount.max(0)).unwrap_or(0);
        }
        // SAFETY: as above.
        unsafe { component.activateBus(audio, input, bus, 1) };
    }
    for bus in 0..outputs_count {
        // SAFETY: an all-zero bus info is valid; the component fills it.
        let mut info: BusInfo = unsafe { std::mem::zeroed() };
        // SAFETY: the component reports its buses by index.
        if unsafe { component.getBusInfo(audio, output, bus, &mut info) } == kResultOk {
            out_channels[bus as usize] = usize::try_from(info.channelCount.max(0)).unwrap_or(0);
        }
        // SAFETY: as above.
        unsafe { component.activateBus(audio, output, bus, 1) };
    }
    // SAFETY: as above.
    if unsafe { component.setActive(1) } != kResultOk {
        return Err("it refused to activate".to_owned());
    }
    // SAFETY: as above.
    if unsafe { processor.setProcessing(1) } != kResultOk {
        // SAFETY: as above.
        unsafe { component.setActive(0) };
        return Err("it refused to start processing".to_owned());
    }
    // SAFETY: as above.
    let latency = unsafe { processor.getLatencySamples() };
    let generation = instance.next_generation;
    instance.next_generation += 1;
    Ok(node::Active {
        generation,
        processor: instance.processor.clone(),
        latency,
        inputs: inputs_count as usize,
        outputs: outputs_count as usize,
        in_channels,
        out_channels,
        params: instance.params.iter().map(|p| p.map).collect(),
        midi: midi_map(&instance.controller),
        pending: std::mem::take(&mut instance.pending),
        drive: Driver::new(instance.params.iter().map(|p| {
            // Each control starts where the controller is: the set's values
            // and the plug-in's saved state are on it.
            // SAFETY: the controller maps its own parameter's value, by ID.
            let plain = unsafe {
                instance.controller.normalizedParamToPlain(
                    p.map.id,
                    instance.controller.getParamNormalized(p.map.id),
                )
            };
            let control = if plain.is_finite() {
                plain.clamp(p.map.min, p.map.max)
            } else {
                p.map.default
            };
            (p.descriptor.clone(), false, control)
        })),
    })
}

/// What the controller's `IMidiMapping` maps each channel's MIDI controllers
/// to: the parameter, at `channel * CONTROLLERS + controller`. Every
/// assignment is asked once, here on the main thread, so the audio thread
/// only looks it up. A controller without the interface maps nothing.
fn midi_map(controller: &ComPtr<IEditController>) -> Vec<Option<u32>> {
    let mut map = vec![None; 16 * node::CONTROLLERS];
    let Some(mapping) = controller.cast::<IMidiMapping>() else {
        return map;
    };
    for (at, slot) in map.iter_mut().enumerate() {
        let (channel, number) = (at / node::CONTROLLERS, at % node::CONTROLLERS);
        let mut id = 0;
        // SAFETY: the controller answers for any channel and controller
        // number, from the main thread, writing the ID where it's pointed.
        let result = unsafe {
            mapping.getMidiControllerAssignment(0, channel as i16, number as i16, &raw mut id)
        };
        if result == kResultOk {
            *slot = Some(id);
        }
    }
    map
}

/// Deactivates an instance, as VST3 asks: stop processing, then leave the
/// active state.
fn deactivate(instance: &mut Instance) {
    // SAFETY: the processor and component are live objects, and this runs
    // on the main thread.
    unsafe {
        instance.processor.setProcessing(0);
        instance.component.setActive(0);
    }
}

/// Takes back the components nodes handed back. Returns whether anything
/// changed.
fn take_back(instance: &mut Instance) -> bool {
    let mut returned = Vec::new();
    let lent = instance.lent.iter_mut().map(|lent| &mut lent.ends);
    for ends in instance
        .ends
        .iter_mut()
        .chain(instance.retired.iter_mut())
        .chain(lent)
    {
        while let Ok(active) = ends.from_node.pop() {
            returned.push(active);
        }
    }
    let mut changed = false;
    for active in returned {
        if instance.handed == Some(active.generation) {
            instance.handed = None;
            deactivate(instance);
            changed = true;
        }
    }
    changed
}

/// One plug-in's main-thread work: deactivate what nodes gave back,
/// restart it when it asked, tell its controller what its node applied,
/// collect the changes its controller made itself, activate what waits for
/// its node, and report a latency that changed. Returns whether anything
/// changed.
fn pump(
    device: DeviceId,
    instance: &mut Instance,
    changes: &mut Vec<ParamChange>,
    latency_changes: &mut Vec<(DeviceId, u32)>,
    latencies: &mut BTreeMap<DeviceId, u32>,
) -> bool {
    let mut changed = take_back(instance);
    instance
        .retired
        .retain(|ends| !(ends.from_node.is_abandoned() && ends.from_node.is_empty()));

    // A restart it asked for — its latency changed, say: ask its node for
    // the component; the next pump activates it again, and the node passes
    // its input through for the blocks between (R10).
    if instance.lent.is_none()
        && instance.restart.swap(0, Relaxed) != 0
        && instance.handed.is_some()
        && let Some(ends) = &instance.ends
    {
        ends.status.hand_back.store(true, Relaxed);
        changed = true;
    }

    // The values its node applied, so the controller's parameters are the
    // device's (R5), and the changes its controller made itself, for the
    // set (R6).
    changed |= sync_controller(instance);
    if instance.lent.is_none() {
        changed |= take_gestures(device, instance, changes);
    }

    if let Some(ends) = &instance.ends
        && ends.status.failed.load(Relaxed)
        && !instance.silenced
    {
        instance.silenced = true;
        instance.failure = Some("it failed while processing; the device is silent".to_owned());
        changed = true;
    }

    let ready = instance.handed.is_none()
        && instance.lent.is_none()
        && instance.refused.is_none()
        && !instance.silenced
        && instance.config.is_some();
    if ready {
        let config = instance.config.expect("the check above");
        match activate(instance, &config) {
            Ok(active) => {
                let generation = active.generation;
                // The latency it reports now: the engine recompiles with it,
                // so delay compensation follows a plug-in that changed its
                // own (R10, delay-compensation R6), and a node made later
                // reads it at once.
                latencies.insert(device, active.latency);
                if instance.latency != active.latency {
                    latency_changes.push((device, active.latency));
                }
                instance.latency = active.latency;
                if let Some(ends) = &instance.ends {
                    // The node reports it from now on, before its next block
                    // takes the component (R10).
                    ends.status.latency.store(active.latency, Relaxed);
                }
                let pushed = instance.ends.as_mut().is_some_and(|ends| {
                    !ends.to_node.is_abandoned() && ends.to_node.push(active).is_ok()
                });
                if pushed {
                    instance.handed = Some(generation);
                    changed = true;
                }
            }
            Err(why) => {
                instance.refused = Some(why);
                changed = true;
            }
        }
    }
    changed
}

/// Tells the controller the values its node applied to the component, so
/// its parameters are the device's (R5). Returns whether anything changed.
fn sync_controller(instance: &mut Instance) -> bool {
    let mut changed = false;
    // Spec: offline-plugin-renders R4. What a render applied is dropped.
    if let Some(lent) = &mut instance.lent {
        while lent.ends.applied.pop().is_ok() {}
    }
    for ends in instance.ends.iter_mut().chain(instance.retired.iter_mut()) {
        while let Ok((id, normalized)) = ends.applied.pop() {
            // SAFETY: the controller takes its own parameter's value, by
            // ID, on the main thread.
            if unsafe { instance.controller.setParamNormalized(id, normalized) } == kResultOk {
                changed = true;
            }
        }
    }
    changed
}

/// Takes the changes the plug-in's controller made itself, such as from its
/// editor: each gesture once, with its final value, in plain units (R6).
/// Returns whether there were any.
fn take_gestures(device: DeviceId, instance: &Instance, changes: &mut Vec<ParamChange>) -> bool {
    let gestures = Arc::clone(&instance.gestures);
    let before = changes.len();
    gestures.take(|id, normalized| {
        // SAFETY: the controller maps its own parameter's value, by ID, on
        // the main thread.
        let plain = unsafe { instance.controller.normalizedParamToPlain(id, normalized) };
        changes.push(ParamChange {
            device,
            param: ParamId(id),
            value: plain,
        });
    });
    changes.len() > before
}

/// Lets go of a plug-in: asks its node for the component, keeping the
/// instance until that's back.
fn close(main: &mut Main, device: DeviceId, mut instance: Instance) {
    for ends in instance.ends.iter().chain(&instance.retired) {
        ends.status.hand_back.store(true, Relaxed);
    }
    instance.refused = Some("let go of".to_owned());
    take_back(&mut instance);
    if instance.handed.is_some() {
        main.closing.push((device, instance));
    } else {
        teardown(&mut instance);
    }
}

/// The 16 bytes of a class ID as the host writes them: uppercase hex with
/// dashes, as the VST3 SDK's `FUID::toString` does.
#[must_use]
pub fn format_fuid(tuid: &TUID) -> String {
    let bytes: Vec<u8> = tuid.iter().map(|b| *b as u8).collect();
    let hex = |range: &[u8]| range.iter().map(|b| format!("{b:02X}")).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        hex(&bytes[0..4]),
        hex(&bytes[4..6]),
        hex(&bytes[6..8]),
        hex(&bytes[8..10]),
        hex(&bytes[10..16])
    )
}

/// The class ID a device's kind names.
fn parse_fuid(text: &str) -> Option<TUID> {
    let digits: String = text.chars().filter(|c| *c != '-').collect();
    if digits.len() != 32 {
        return None;
    }
    let mut out = [0 as c_char; 16];
    for (index, slot) in out.iter_mut().enumerate() {
        let byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16).ok()?;
        *slot = byte as c_char;
    }
    Some(out)
}

/// A NUL-terminated `char8` array as a string.
fn cstring<const N: usize>(buffer: &[c_char; N]) -> String {
    let bytes: Vec<u8> = buffer
        .iter()
        .map(|c| *c as u8)
        .take_while(|b| *b != 0)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_owned()
}
