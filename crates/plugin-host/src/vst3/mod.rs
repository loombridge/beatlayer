//! VST3 hosting, part 1 (WP-2.7; the VST3 hosting spec): loading a `.vst3`
//! module, its factory and each audio class as a device type, the component
//! and its controller connected, and processing block by block with each
//! parameter change on its sample offset and each note in the event list on
//! its frame. The node passes its input through until the plug-in is
//! active. State, latency changes and the editor are the next part's.
//!
//! A [`Vst3Host`] lives on the main thread, like the CLAP host: it loads
//! modules, keeps one component and controller per device, and activates
//! them for a node the compiler thread made. Its [`Vst3Factory`] goes into
//! the engine's [`Registry`](bl_engine::Registry).

mod describe;
mod handlers;
mod node;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{c_char, c_void};
use std::fmt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::ThreadId;

use bl_engine::{DeviceType, Node, NodeConfig, NodeFactory};
use bl_model::{Device, DeviceDescription, DeviceId, DeviceKind, ParamId, PluginFormat};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper};

use handlers::{ComponentHandler, HostApplication};
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
}

impl fmt::Display for Vst3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(why) => write!(f, "couldn't load the plug-in: {why}"),
            Self::UnknownPlugin(id) => write!(f, "no VST3 plug-in {id} is loaded"),
            Self::NoInstance(device) => write!(f, "{device} has no running VST3 plug-in"),
            Self::Refused(what) => write!(f, "the plug-in refused {what}"),
            Self::WrongThread => f.write_str("VST3's main-thread calls run on the main thread"),
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
    /// The settings it's active for: the engine's rate and block size.
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
    /// The component handler their controllers report to.
    handler: ComWrapper<ComponentHandler>,
}

impl Main {
    fn new() -> Self {
        Self {
            instances: BTreeMap::new(),
            closing: Vec::new(),
            missing: BTreeMap::new(),
            modules: HashMap::new(),
            host: ComWrapper::new(HostApplication),
            handler: ComWrapper::new(ComponentHandler),
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
}

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
    /// deactivates plug-ins their nodes gave back, and lets go of closed
    /// ones once their nodes are done with them. Returns whether anything
    /// a window shows may have changed.
    pub fn idle(&self) -> bool {
        let requests = std::mem::take(&mut *lock(&self.inner.requests));
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else { return false };
        let mut changed = !requests.is_empty();
        for request in requests {
            serve(main, request, &self.inner);
        }
        for instance in main.instances.values_mut() {
            changed |= pump(instance);
        }
        for (_, instance) in &mut main.closing {
            pump(instance);
            if instance.handed.is_none() {
                teardown(instance);
            }
        }
        main.closing
            .retain(|(_, instance)| instance.handed.is_some());
        changed
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
            active_for: instance.config.filter(|_| instance.handed.is_some()),
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
    }

    /// Lets go of every plug-in, as when another set opens.
    pub fn clear(&self) {
        self.retain(&BTreeSet::new());
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
        let (node, ends) = Vst3Node::new(config);
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
    let mut library =
        unsafe { Library::new(&binary) }.map_err(|e| Vst3Error::Load(e.to_string()))?;
    #[cfg(target_os = "windows")]
    {
        // SAFETY: the module exports it or it doesn't; that's all the VST3
        // module entry is.
        let init = unsafe { library.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") }
            .ok()
            .map(|symbol| *symbol);
        if let Some(init) = init {
            let _ = unsafe { init() };
        }
    }
    #[cfg(unix)]
    {
        // SAFETY: the module exports it or it doesn't; the entry point gets
        // the module's own handle, which stays loaded.
        let entry = unsafe {
            library.get::<unsafe extern "system" fn(*mut c_void) -> bool>(b"ModuleEntry\0")
        }
        .ok()
        .map(|symbol| *symbol);
        if let Some(entry) = entry {
            library = library_handle(library, |handle| {
                // SAFETY: the module exported the entry point, which gets
                // the module's own handle.
                let _ = unsafe { entry(handle) };
            });
        }
    }
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

#[cfg(windows)]
fn library_handle<R>(library: Library, use_handle: impl FnOnce(*mut c_void) -> R) -> Library {
    let raw = library.into_raw();
    let handle = raw as *mut c_void;
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
/// initialized, the controller given the handler, and the two connected.
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
    if let Some(handler) = main.handler.as_com_ref::<IComponentHandler>() {
        // SAFETY: the handler is the host's own object, which outlives the
        // controller.
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

/// Activates an instance for a node: the sample size, the bus
/// arrangements, processing setup, activation and processing start, in the
/// VST3 order, then an [`Active`](node::Active) for the node.
fn activate(instance: &mut Instance, config: &NodeConfig) -> Result<node::Active, String> {
    let processor = &instance.processor;
    // SAFETY: the processor is a live object and these are its own calls,
    // from the main thread.
    if unsafe { processor.canProcessSampleSize(SymbolicSampleSizes_::kSample32 as i32) }
        != kResultOk
    {
        return Err("it doesn't process 32-bit audio".to_owned());
    }
    let mut inputs = [SpeakerArr::kStereo];
    let mut outputs = [SpeakerArr::kStereo];
    // The main stereo arrangement is asked for; a plug-in that refuses it
    // keeps its own, and its channel counts are read below either way.
    // SAFETY: as above.
    unsafe { processor.setBusArrangements(inputs.as_mut_ptr(), 1, outputs.as_mut_ptr(), 1) };
    let setup = ProcessSetup {
        processMode: ProcessModes_::kRealtime as i32,
        symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
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
    let inputs_count =
        unsafe { component.getBusCount(MediaTypes_::kAudio as i32, BusDirections_::kInput as i32) }
            .clamp(0, MAX_BUSES as i32);
    // SAFETY: as above.
    let outputs_count = unsafe {
        component.getBusCount(MediaTypes_::kAudio as i32, BusDirections_::kOutput as i32)
    }
    .clamp(0, MAX_BUSES as i32);
    let mut in_channels = [0usize; MAX_BUSES];
    let mut out_channels = [0usize; MAX_BUSES];
    for bus in 0..inputs_count {
        // SAFETY: an all-zero bus info is valid; the component fills it.
        let mut info: BusInfo = unsafe { std::mem::zeroed() };
        // SAFETY: the component reports its buses by index.
        if unsafe {
            component.getBusInfo(
                MediaTypes_::kAudio as i32,
                BusDirections_::kInput as i32,
                bus,
                &mut info,
            )
        } == kResultOk
        {
            in_channels[bus as usize] = usize::try_from(info.channelCount.max(0)).unwrap_or(0);
        }
        // SAFETY: as above.
        unsafe {
            component.activateBus(
                MediaTypes_::kAudio as i32,
                BusDirections_::kInput as i32,
                bus,
                1,
            )
        };
    }
    for bus in 0..outputs_count {
        // SAFETY: an all-zero bus info is valid; the component fills it.
        let mut info: BusInfo = unsafe { std::mem::zeroed() };
        // SAFETY: the component reports its buses by index.
        if unsafe {
            component.getBusInfo(
                MediaTypes_::kAudio as i32,
                BusDirections_::kOutput as i32,
                bus,
                &mut info,
            )
        } == kResultOk
        {
            out_channels[bus as usize] = usize::try_from(info.channelCount.max(0)).unwrap_or(0);
        }
        // SAFETY: as above.
        unsafe {
            component.activateBus(
                MediaTypes_::kAudio as i32,
                BusDirections_::kOutput as i32,
                bus,
                1,
            )
        };
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
        pending: std::mem::take(&mut instance.pending),
    })
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
    for ends in instance.ends.iter_mut().chain(instance.retired.iter_mut()) {
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
/// activate what waits for its node. Returns whether anything changed.
fn pump(instance: &mut Instance) -> bool {
    let mut changed = take_back(instance);
    instance
        .retired
        .retain(|ends| !(ends.from_node.is_abandoned() && ends.from_node.is_empty()));

    if let Some(ends) = &instance.ends
        && ends.status.failed.load(Relaxed)
        && !instance.silenced
    {
        instance.silenced = true;
        instance.failure = Some("it failed while processing; the device is silent".to_owned());
        changed = true;
    }

    let ready = instance.handed.is_none()
        && instance.refused.is_none()
        && !instance.silenced
        && instance.config.is_some();
    if ready {
        let config = instance.config.expect("the check above");
        match activate(instance, &config) {
            Ok(active) => {
                let generation = active.generation;
                instance.latency = active.latency;
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
