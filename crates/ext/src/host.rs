//! The extension host (extensions R4 to R12): installed extensions in the
//! User Library's `Extensions` folder, their compiled code in the cache
//! folder, and components running under `wasmtime` with their memory
//! reserved up front and every call bounded by epoch interruption.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use sha2::{Digest, Sha256};
use wasmtime::component::{Component, Linker, Val};
use wasmtime::{
    Config, Engine, InstanceAllocationStrategy, PoolConcurrencyLimitError, PoolingAllocationConfig,
    Store, StoreLimits, StoreLimitsBuilder, Trap,
};

use crate::Error;
use crate::package::{ComponentSpec, MANIFEST, MAX_MEMORY_MIB, Manifest, Package};

/// The `wasmtime` the compiled code is for (R5). It is pinned in
/// `Cargo.toml`, and a test holds the two together.
const WASMTIME: &str = "50.0.1";
/// How often the epoch advances, so a call's limit is counted in milliseconds.
const TICK: Duration = Duration::from_millis(1);
/// How long making an instance may run, which can run the component's
/// start function. Not on the audio thread.
const SETUP: Duration = Duration::from_secs(1);
/// A component instance's memory is a slot of the pool, so this many
/// instances run at once, in all.
const DEFAULT_INSTANCES: u32 = 8;

/// How a [`Host`] is made.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// How many component instances run at once.
    pub instances: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            instances: DEFAULT_INSTANCES,
        }
    }
}

/// An installed extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extension {
    /// Its manifest.
    pub manifest: Manifest,
    /// Its folder in the User Library's `Extensions` folder.
    pub folder: PathBuf,
}

/// The extensions that failed, and why (R10 to R12). Shared with the
/// instances, which record into it from whatever thread called them.
#[derive(Default)]
struct Failures(Mutex<HashMap<String, String>>);

impl Failures {
    fn record(&self, id: &str, file: &str, reason: &str) {
        self.map()
            .insert(id.to_owned(), format!("{file}: {reason}"));
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A thread that advances the engine's epoch every [`TICK`], until dropped.
struct Ticker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Ticker {
    fn start(engine: &Engine) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new()
            .name("extension-epoch".into())
            .spawn({
                let (engine, stop) = (engine.clone(), stop.clone());
                move || {
                    while !stop.load(Ordering::Relaxed) {
                        thread::sleep(TICK);
                        engine.increment_epoch();
                    }
                }
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The extension host: the engine, the folders, and what has failed.
pub struct Host {
    engine: Engine,
    extensions: PathBuf,
    cache: PathBuf,
    failures: Arc<Failures>,
    compiled: AtomicU32,
    _ticker: Ticker,
}

impl Host {
    /// A host for the extensions in `extensions` (the User Library's
    /// `Extensions` folder, which needn't exist yet), keeping compiled code
    /// in `cache`.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if the engine or its thread can't start.
    pub fn new(extensions: PathBuf, cache: PathBuf, options: Options) -> Result<Self, Error> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        config.wasm_backtrace_max_frames(None);
        // R9: every instance's memory is reserved in the pool up front, so
        // making one allocates nothing in the host.
        let mut pool = PoolingAllocationConfig::new();
        pool.total_component_instances(options.instances)
            .total_core_instances(options.instances)
            .total_memories(options.instances)
            .total_tables(options.instances)
            .max_memory_size((MAX_MEMORY_MIB as usize) << 20);
        config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        let engine = Engine::new(&config).map_err(|e| Error::Io(format!("{e:#}")))?;
        let ticker = Ticker::start(&engine).map_err(|e| Error::Io(e.to_string()))?;
        Ok(Self {
            engine,
            extensions,
            cache,
            failures: Arc::default(),
            compiled: AtomicU32::new(0),
            _ticker: ticker,
        })
    }

    /// The installed extensions, by ID. A folder without a readable manifest
    /// isn't one.
    #[must_use]
    pub fn installed(&self) -> Vec<Extension> {
        let Ok(entries) = fs::read_dir(&self.extensions) else {
            return Vec::new();
        };
        let mut found: Vec<Extension> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let folder = entry.path();
                let manifest = fs::read_to_string(folder.join(MANIFEST)).ok()?;
                let manifest = Manifest::parse(&manifest).ok()?;
                (folder.file_name()?.to_str()? == manifest.id)
                    .then_some(Extension { manifest, folder })
            })
            .collect();
        found.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        found
    }

    /// The installed extension `id`.
    #[must_use]
    pub fn extension(&self, id: &str) -> Option<Extension> {
        self.installed().into_iter().find(|e| e.manifest.id == id)
    }

    /// Installs the package at `path` (R3, R4). The whole package is checked
    /// before anything is written; one that fails keeps nothing. A package
    /// with a later version of an installed extension replaces it.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] with the reason the package is refused, or
    /// [`Error::Io`] when a file can't be read or written.
    pub fn install(&self, path: &Path) -> Result<Extension, Error> {
        let file = fs::File::open(path).map_err(|e| io_error(path, &e))?;
        let package = Package::read(io::BufReader::new(file), &self.engine)?;
        let id = &package.manifest.id;
        let version = package.manifest.version;
        let old = self.extension(id);
        if let Some(old) = old.as_ref().filter(|o| o.manifest.version >= version) {
            return Err(Error::Refused(format!(
                "{id} {} is already installed, and the package holds {version}",
                old.manifest.version
            )));
        }
        let folder = self.extensions.join(id);
        let building = self.extensions.join(format!(".installing-{id}"));
        let replaced = self.extensions.join(format!(".replaced-{id}"));
        for leftover in [&building, &replaced] {
            remove_folder(leftover)?;
        }
        if let Err(error) = write_files(&building, &package) {
            let _ = remove_folder(&building);
            return Err(error);
        }
        if old.is_some() {
            fs::rename(&folder, &replaced).map_err(|e| io_error(&folder, &e))?;
        }
        if let Err(error) = fs::rename(&building, &folder) {
            if old.is_some() {
                let _ = fs::rename(&replaced, &folder);
            }
            return Err(io_error(&folder, &error));
        }
        if let Some(old) = &old {
            self.forget_code(&old.manifest, &replaced);
        }
        remove_folder(&replaced)?;
        // The compiled code is only a cache: if it can't be kept, the next
        // load compiles again.
        for (spec, code) in package.manifest.components.iter().zip(&package.compiled) {
            let _ = self.keep_code(&package.files[&spec.file], code);
        }
        self.failures.map().remove(id);
        Ok(Extension {
            manifest: package.manifest,
            folder,
        })
    }

    /// Removes the extension `id`, its folder and its compiled code (R6).
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] when none is installed, [`Error::Io`] when its
    /// folder can't be removed.
    pub fn remove(&self, id: &str) -> Result<(), Error> {
        let extension = self
            .extension(id)
            .ok_or_else(|| Error::Refused(format!("no extension {id} is installed")))?;
        self.forget_code(&extension.manifest, &extension.folder);
        remove_folder(&extension.folder)?;
        self.failures.map().remove(id);
        Ok(())
    }

    /// Why the extension `id` was switched off by a failure (R10 to R12),
    /// until it is switched on again or Beatlayer restarts.
    #[must_use]
    pub fn failure(&self, id: &str) -> Option<String> {
        self.failures.map().get(id).cloned()
    }

    /// Forgets the failure of `id`: it is being switched on again (R12).
    pub fn clear_failure(&self, id: &str) {
        self.failures.map().remove(id);
    }

    /// An instance of the component `file` of the extension `id`, made off
    /// the audio thread (R26). A failed extension has none (R12); one that
    /// can't be made fails the extension.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] when the extension or component isn't installed or
    /// has failed, [`Error::Failed`] when the instance can't be made.
    pub fn instantiate(&self, id: &str, file: &str) -> Result<Instance, Error> {
        if let Some(reason) = self.failure(id) {
            return Err(Error::Refused(format!(
                "{id} is off after a failure: {reason}"
            )));
        }
        let extension = self
            .extension(id)
            .ok_or_else(|| Error::Refused(format!("no extension {id} is installed")))?;
        let spec = extension
            .manifest
            .components
            .iter()
            .find(|c| c.file == file)
            .ok_or_else(|| Error::Refused(format!("{id} has no component {file}")))?
            .clone();
        let bytes =
            fs::read(extension.folder.join(file)).map_err(|e| io_error(&extension.folder, &e))?;
        let fail = |reason: String| {
            self.failures.record(id, file, &reason);
            Error::Failed(reason)
        };
        let component = self.component(&bytes).map_err(fail)?;
        let limits = StoreLimitsBuilder::new()
            .memory_size((spec.memory_mib as usize) << 20)
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(&self.engine, limits);
        store.limiter(|limits| limits);
        store.set_epoch_deadline(ticks(SETUP));
        let instance = Linker::new(&self.engine)
            .instantiate(&mut store, &component)
            .map_err(|e| {
                // A full pool is the host's limit, not the component's fault.
                if e.downcast_ref::<PoolConcurrencyLimitError>().is_some() {
                    Error::Refused(format!("{id} can't start: {e}"))
                } else {
                    fail(reason(&e, SETUP))
                }
            })?;
        Ok(Instance {
            id: id.to_owned(),
            spec,
            store,
            instance,
            failures: self.failures.clone(),
            off: false,
        })
    }

    /// How many components this host has compiled since it started, which
    /// the cache (R5) spares it.
    #[must_use]
    pub fn compiled(&self) -> u32 {
        self.compiled.load(Ordering::Relaxed)
    }

    /// The component in `bytes`, from the cache when its compiled code is
    /// there (R5), otherwise compiled and kept.
    fn component(&self, bytes: &[u8]) -> Result<Component, String> {
        let path = self.code_path(bytes);
        if path.is_file() {
            // SAFETY: the file is code this host wrote for this `wasmtime`
            // (`keep_code`), named by the hash of the component it came
            // from, in a folder only this program writes. Deserializing
            // checks its header, and a file that doesn't load is compiled
            // again below.
            if let Ok(component) = unsafe { Component::deserialize_file(&self.engine, &path) } {
                return Ok(component);
            }
        }
        let component = Component::new(&self.engine, bytes).map_err(|e| format!("{e:#}"))?;
        self.compiled.fetch_add(1, Ordering::Relaxed);
        if let Ok(code) = component.serialize() {
            let _ = self.keep_code(bytes, &code);
        }
        Ok(component)
    }

    /// Where the compiled code of the component in `bytes` is kept (R5):
    /// named by the component's hash and `wasmtime`'s version.
    fn code_path(&self, bytes: &[u8]) -> PathBuf {
        let mut hash = Sha256::new();
        hash.update(bytes);
        let hash: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        self.cache.join(format!("{hash}-wasmtime-{WASMTIME}.cwasm"))
    }

    fn keep_code(&self, bytes: &[u8], code: &[u8]) -> io::Result<()> {
        fs::create_dir_all(&self.cache)?;
        let path = self.code_path(bytes);
        let temp = path.with_extension("tmp");
        fs::write(&temp, code)?;
        fs::rename(&temp, path)
    }

    /// Deletes the compiled code of an extension's components, which are
    /// read from `folder`.
    fn forget_code(&self, manifest: &Manifest, folder: &Path) {
        for spec in &manifest.components {
            if let Ok(bytes) = fs::read(folder.join(&spec.file)) {
                let _ = fs::remove_file(self.code_path(&bytes));
            }
        }
    }
}

/// A running component instance. A call that traps or runs past its limit
/// stops the instance for good and marks its extension failed (R10, R11);
/// nothing else is touched.
pub struct Instance {
    id: String,
    spec: ComponentSpec,
    store: Store<StoreLimits>,
    instance: wasmtime::component::Instance,
    failures: Arc<Failures>,
    off: bool,
}

impl Instance {
    /// Calls `function` in the interface the component's kind and world name
    /// (`beatlayer:ext/midi-tool@0.1.0`), giving up after `limit` (R11).
    ///
    /// # Errors
    ///
    /// [`Error::Failed`] with the reason, when the component has no such
    /// function, traps, runs past `limit`, or has failed before.
    pub fn call(
        &mut self,
        function: &str,
        args: &[Val],
        limit: Duration,
    ) -> Result<Vec<Val>, Error> {
        if self.off {
            return Err(Error::Failed(
                "the instance was stopped by an earlier failure".into(),
            ));
        }
        let interface = self.spec.interface();
        let func = self
            .instance
            .get_export_index(&mut self.store, None, &interface)
            .and_then(|iface| {
                self.instance
                    .get_export_index(&mut self.store, Some(&iface), function)
            })
            .and_then(|index| self.instance.get_func(&mut self.store, index))
            .ok_or_else(|| Error::Failed(format!("{interface} has no function {function}")))?;
        let mut results = vec![Val::Bool(false); func.ty(&self.store).results().len()];
        self.store.set_epoch_deadline(ticks(limit));
        match func.call(&mut self.store, args, &mut results) {
            Ok(()) => Ok(results),
            Err(error) => {
                self.off = true;
                let reason = reason(&error, limit);
                self.failures.record(&self.id, &self.spec.file, &reason);
                Err(Error::Failed(reason))
            }
        }
    }

    /// Whether a failure has stopped this instance.
    #[must_use]
    pub fn is_off(&self) -> bool {
        self.off
    }
}

/// `limit` in epoch ticks, at least one.
fn ticks(limit: Duration) -> u64 {
    (limit.as_nanos().div_ceil(TICK.as_nanos()) as u64).max(1)
}

/// What went wrong, for the person to read.
fn reason(error: &wasmtime::Error, limit: Duration) -> String {
    match error.downcast_ref::<Trap>() {
        Some(Trap::Interrupt) => format!("ran past its time limit of {} ms", limit.as_millis()),
        Some(trap) => format!("trapped: {trap}"),
        None => format!("{error:#}"),
    }
}

fn write_files(folder: &Path, package: &Package) -> Result<(), Error> {
    for (name, bytes) in &package.files {
        let path = folder.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error(parent, &e))?;
        }
        fs::write(&path, bytes).map_err(|e| io_error(&path, &e))?;
    }
    Ok(())
}

/// Removes a folder an install or removal owns, if it is there.
fn remove_folder(folder: &Path) -> Result<(), Error> {
    match fs::remove_dir_all(folder) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(io_error(folder, &error)),
        _ => Ok(()),
    }
}

fn io_error(path: &Path, error: &io::Error) -> Error {
    Error::Io(format!("{}: {error}", path.display()))
}
