//! CLAP hosting (WP-2.6; the CLAP hosting spec).
//!
//! A [`ClapHost`] lives on the main thread, which is CLAP's main thread
//! for every plug-in it hosts: it loads plug-in files ([`ClapHost::load`],
//! until the scanner of WP-2.9 finds them), keeps one plug-in instance per
//! device, saves and loads their state, opens and closes their editors (in
//! a host window of Beatlayer's own with the plug-in's view embedded,
//! where the platform builds one, or in a floating window of the plug-in's
//! own; WP-2.10), and activates and deactivates them. Its [`ClapFactory`]
//! goes into the engine's [`Registry`](bl_engine::Registry): the compiler
//! thread asks it for a device's node, which it makes at once without the
//! plug-in, and the main thread activates the plug-in for that node the
//! next time it runs [`ClapHost::idle`] (at once, if the compiler runs on
//! the main thread, as [`Offline`](bl_engine::Offline) does). The node
//! passes its input through until the plug-in arrives.
//!
//! An instance outlives its node: a device moved, switched off, or deleted
//! and brought back with undo keeps its plug-in and the plug-in's state,
//! and a new engine (a new audio device or rate) deactivates the plug-in
//! and activates it again for its own node. The parameter changes a
//! plug-in makes itself, from its editor, come back through
//! [`ClapHost::take_param_changes`], each gesture as one change.
//!
//! A plug-in that offers the plug-in kit's agent-commands extension
//! (`com.loombridge.agent-commands`, which every kit plug-in does) lists
//! its commands when its instance starts ([`ClapHost::commands`]), and
//! [`ClapHost::run_command`] runs one on the main thread, returning its
//! result and the parameters it changed, so the shell can serve them to
//! agents as device commands and record each run as one undo step.
//!
//! Offline renders (exports and agents' previews) borrow the same
//! instances ([`ClapHost::lend`], WP-2.6b), so a plug-in device renders as
//! it plays: each instance comes back from the engine's node, is
//! reactivated for the render's rate and block size, and when the [`Loan`]
//! ends goes back to the engine with the state it had.
//!
//! Plug-ins run in Beatlayer's own process until Phase 10's sandbox, so a
//! plug-in that crashes takes the app with it. A plug-in that fails a
//! call (it can't be loaded, refuses to activate, refuses a state, or
//! returns an error from processing) is contained: see the spec's rules.

mod describe;
mod events;
mod handlers;
mod node;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::CString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use bl_engine::{DeviceType, Node, NodeConfig, NodeFactory};
use bl_model::{
    Device, DeviceDescription, DeviceId, DeviceKind, DeviceRole, ParamId, PluginFormat, PluginState,
};
use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, PluginGui};
use clack_extensions::latency::PluginLatency;
use clack_extensions::params::PluginParams;
use clack_extensions::state::PluginState as StateExtension;
use clack_host::prelude::*;
use lspk_plugin::agent_commands::PluginAgentCommands;

use crate::drive::Driver;
use crate::window::{EditorWindow, WindowEvent};

pub use bl_commands::CommandSpec;
pub use clack_host::entry::EntryDescriptor;
pub use node::ClapNode;

use handlers::{Host, MainThread, ProcessorData, Shared};
use node::{HostEnds, MAX_CHANNELS, ParamOut};

/// Why a CLAP host call failed. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClapError {
    /// The file isn't a CLAP plug-in Beatlayer can load, and why.
    Load(String),
    /// No plug-in of that ID is known.
    UnknownPlugin(String),
    /// The device has no running plug-in: it isn't a CLAP device, or its
    /// plug-in couldn't be loaded.
    NoInstance(DeviceId),
    /// The plug-in refused, and what it refused.
    Refused(String),
    /// It was called off the main thread.
    WrongThread,
    /// The plug-ins are lent to another offline render already.
    Lent,
}

impl fmt::Display for ClapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(why) => write!(f, "couldn't load the plug-in: {why}"),
            Self::UnknownPlugin(id) => write!(f, "no CLAP plug-in {id} is loaded"),
            Self::NoInstance(device) => write!(f, "{device} has no running CLAP plug-in"),
            Self::Refused(what) => write!(f, "the plug-in refused {what}"),
            Self::WrongThread => f.write_str("CLAP's main-thread calls run on the main thread"),
            Self::Lent => f.write_str("the plug-ins are lent to another render"),
        }
    }
}

impl std::error::Error for ClapError {}

/// A plug-in Beatlayer can make devices of, and the file it's in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClapType {
    /// The plug-in file or bundle.
    pub path: PathBuf,
    /// Its device type: kind, name, role, and its parameters.
    pub device_type: DeviceType,
    /// Its vendor, if it gives one (the scanner's list, WP-2.9).
    pub vendor: Option<String>,
    /// Its version, if it gives one (the scanner's list, WP-2.9).
    pub version: Option<String>,
}

/// A parameter change a plug-in made itself, such as from its editor: one
/// for each gesture, with its final value, or one for each change outside a
/// gesture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamChange {
    /// The device.
    pub device: DeviceId,
    /// The parameter.
    pub param: ParamId,
    /// Its new value.
    pub value: f64,
}

/// What running a plug-in's command did.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandRun {
    /// The command's result.
    pub result: serde_json::Value,
    /// The parameters whose values it changed, with their new values as
    /// the plug-in reports them, for the set.
    pub changes: Vec<(ParamId, f64)>,
}

/// What a device's plug-in is doing, for `plugin.info`.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginStatus {
    /// The plug-in's CLAP ID.
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
    /// What went wrong with it, if anything: it couldn't be loaded,
    /// refused to activate, or failed while processing.
    pub failure: Option<String>,
    /// Whether it has an editor that opens in a window of its own.
    pub has_editor: bool,
    /// Whether its editor is open.
    pub editor_open: bool,
    /// Whether its open editor is hidden: View ▸ Plug-In Windows or
    /// Auto-Hide Plug-In Windows hid it. Hidden is not closed.
    pub editor_hidden: bool,
    /// The display's scale the host gave its embedded editor
    /// (`set_scale`); `None` while it floats in a window of its own, or
    /// has no editor (WP-2.10).
    pub editor_scale: Option<f64>,
    /// Its host window's client size, in physical pixels; `None` while its
    /// editor floats in a window of its own, or isn't open (WP-2.10).
    pub editor_size: Option<GuiSize>,
    /// Each parameter's value, as the plug-in reports it.
    pub values: Vec<(ParamId, f64)>,
}

/// Things only the owner thread may touch: the plug-in instances, which
/// CLAP binds to the main thread.
struct MainOnly<T> {
    owner: ThreadId,
    value: T,
}

// SAFETY: the value is reached only through `get`, which checks it's on
// the thread that made it; the instances inside never leave that thread.
unsafe impl<T> Send for MainOnly<T> {}

impl<T> MainOnly<T> {
    fn get(&mut self) -> Result<&mut T, ClapError> {
        if std::thread::current().id() == self.owner {
            Ok(&mut self.value)
        } else {
            Err(ClapError::WrongThread)
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
    info: HostInfo,
    requests: Mutex<Vec<Request>>,
    types: Mutex<Vec<ClapType>>,
    main: Mutex<MainOnly<Main>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The main thread's plug-ins.
#[derive(Default)]
struct Main {
    entries: HashMap<PathBuf, PluginEntry>,
    instances: BTreeMap<DeviceId, Instance>,
    /// Devices whose plug-in couldn't be made, and why.
    missing: BTreeMap<DeviceId, (String, PathBuf, String)>,
    /// Plug-ins let go of while a node still had them, waiting for their
    /// processors to come back so they can be deactivated and destroyed on
    /// the main thread.
    closing: Vec<(DeviceId, Instance)>,
    changes: Vec<ParamChange>,
    /// Counts instances started and let go of.
    generation: u64,
    /// An offline render has the plug-ins ([`ClapHost::lend`]).
    lending: bool,
    /// Devices the render couldn't borrow a plug-in for, and why.
    loan_problems: Vec<(DeviceId, String)>,
}

/// One device's plug-in.
struct Instance {
    plugin_id: String,
    path: PathBuf,
    instance: PluginInstance<Host>,
    /// The node it runs for, and nodes it ran for that haven't given its
    /// processor back yet.
    ends: Option<HostEnds>,
    retired: Vec<HostEnds>,
    config: Option<NodeConfig>,
    /// A node has its processor.
    out: bool,
    /// Activation failed for this node; it isn't tried again until the
    /// next node or state.
    refused: Option<String>,
    failure: Option<String>,
    /// Its editor, while it's open in a host window of Beatlayer's own
    /// (WP-2.10).
    editor: Option<Editor>,
    /// Its open editor is hidden: View ▸ Plug-In Windows or Auto-Hide
    /// Plug-In Windows. Hidden is not closed.
    editor_hidden: bool,
    /// The visibility last asked for of its open editor; while the
    /// window's actual visibility differs from it, the plug-in refused
    /// the ask, and it's asked again only when the wanted visibility
    /// changes.
    editor_wanted: bool,
    /// It failed while processing: its device is silent, and it isn't
    /// activated again, live or offline, until the next node or state.
    silenced: bool,
    /// The settings it's active for.
    active_for: Option<NodeConfig>,
    /// The offline render that has it.
    lent: Option<Lent>,
    latency: u32,
    editor_open: bool,
    gestures: BTreeSet<u32>,
    held: BTreeMap<u32, f64>,
    /// Its commands, through the agent-commands extension, as it listed
    /// them when it started; `None` without the extension.
    commands: Option<Vec<CommandSpec>>,
    /// Values its commands set, which the set already holds: the gestures
    /// it sends for them are dropped when they arrive, rather than made
    /// undo steps of their own (oldest first, at most [`ECHOES`]).
    echoes: Vec<(u32, f64)>,
}

/// An editor embedded in a host window of Beatlayer's own (WP-2.10): the
/// window, and the display's scale the plug-in was told with `set_scale`.
/// Dropping it closes the window, after the plug-in's view is destroyed.
struct Editor {
    window: EditorWindow,
    scale: f64,
}

/// How many command changes a plug-in's gestures are matched against.
const ECHOES: usize = 256;

/// A plug-in lent to an offline render: the render's node, and what to
/// give back to the engine afterwards.
struct Lent {
    ends: HostEnds,
    /// The plug-in's state before the render, which it gets back.
    saved: Option<Vec<u8>>,
    /// Why the engine's node had it refused, if it had.
    refused: Option<String>,
}

/// How long a render waits for the engine's node to hand a plug-in back:
/// a node does at its next block, a few milliseconds away.
const HAND_BACK_WAIT: Duration = Duration::from_secs(2);

/// The CLAP host: CLAP's main thread for every plug-in, on the thread that
/// made it.
///
/// Its methods run only on that thread and return [`ClapError::WrongThread`]
/// elsewhere. [`ClapHost::idle`] must run there often (each frame, as the
/// shell's poll does) for new nodes to get their plug-ins.
#[derive(Clone)]
pub struct ClapHost {
    inner: Arc<Inner>,
}

impl Default for ClapHost {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ClapHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClapHost").finish_non_exhaustive()
    }
}

impl ClapHost {
    /// A host with no plug-ins, whose main thread is this one.
    #[must_use]
    pub fn new() -> Self {
        let owner = std::thread::current().id();
        let info = HostInfo::new(
            "Beatlayer",
            "Beatlayer",
            "https://github.com/loombridge/beatlayer",
            env!("CARGO_PKG_VERSION"),
        )
        .unwrap_or_else(|_| unreachable!("the host's names have no NUL"));
        Self {
            inner: Arc::new(Inner {
                owner,
                info,
                requests: Mutex::new(Vec::new()),
                types: Mutex::new(Vec::new()),
                main: Mutex::new(MainOnly {
                    owner,
                    value: Main::default(),
                }),
            }),
        }
    }

    /// The factory for the engine's registry, which makes a node for each
    /// CLAP device.
    #[must_use]
    pub fn factory(&self) -> ClapFactory {
        ClapFactory {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Loads the CLAP file or bundle at `path` and returns the plug-ins in
    /// it as device types, which devices can then be made of.
    ///
    /// # Errors
    ///
    /// [`ClapError::Load`] if it isn't a CLAP plug-in or offers none.
    pub fn load(&self, path: &Path) -> Result<Vec<ClapType>, ClapError> {
        let mut guard = lock(&self.inner.main);
        let main = guard.get()?;
        let entry = entry(main, path)?;
        let factory = entry
            .get_plugin_factory()
            .ok_or_else(|| ClapError::Load("it offers no plug-ins".to_owned()))?;
        let mut found = Vec::new();
        for descriptor in factory.plugin_descriptors() {
            let Some(id) = descriptor.id() else { continue };
            let Ok(id) = id.to_str() else { continue };
            let name = descriptor
                .name()
                .and_then(|n| n.to_str().ok())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or(id)
                .to_owned();
            let role = describe::role(descriptor.features());
            let vendor = descriptor
                .vendor()
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned);
            let version = descriptor
                .version()
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned);
            let mut probe = new_instance(&entry, id, &self.inner.info)?;
            let params = describe::params(&mut probe.plugin_handle());
            let outputs = describe::outputs(&mut probe.plugin_handle());
            let note_output = describe::note_output(&mut probe.plugin_handle());
            drop(probe);
            found.push(ClapType {
                path: path.to_owned(),
                device_type: DeviceType {
                    kind: kind(id),
                    name,
                    role,
                    description: DeviceDescription::new(params),
                    sidechain: false,
                    external: false,
                    midi_sidechain: false,
                    note_output,
                    outputs,
                },
                vendor,
                version,
            });
        }
        drop(guard);
        if found.is_empty() {
            return Err(ClapError::Load("it offers no plug-ins".to_owned()));
        }
        let mut types = lock(&self.inner.types);
        for found in &found {
            types.retain(|t| t.device_type.kind != found.device_type.kind);
            types.push(found.clone());
        }
        Ok(found)
    }

    /// Makes a plug-in compiled into this binary loadable as if it were
    /// the file at `path`: `plugin.load` with that path, and sets naming it,
    /// find this entry. For tests, and plug-ins built in.
    ///
    /// # Safety
    ///
    /// `entry` must be a CLAP entry that follows the CLAP specification, as
    /// loading a plug-in file trusts its entry to.
    ///
    /// # Errors
    ///
    /// [`ClapError::Load`] if the entry won't start, and
    /// [`ClapError::WrongThread`] off the main thread.
    pub unsafe fn add_entry(
        &self,
        path: &Path,
        entry: &'static EntryDescriptor,
    ) -> Result<(), ClapError> {
        let mut guard = lock(&self.inner.main);
        let main = guard.get()?;
        let name = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| ClapError::Load("the path holds a NUL character".to_owned()))?;
        // SAFETY: the caller vouches for the entry.
        let entry = unsafe { PluginEntry::load_from_raw(entry, &name) }
            .map_err(|e| ClapError::Load(e.to_string()))?;
        main.entries.insert(path.to_owned(), entry);
        Ok(())
    }

    /// The device's plug-in's own commands, by their IDs within the
    /// plug-in, if it offers the agent-commands extension and is running.
    #[must_use]
    pub fn commands(&self, device: DeviceId) -> Option<Vec<CommandSpec>> {
        let mut guard = lock(&self.inner.main);
        guard.get().ok()?.instances.get(&device)?.commands.clone()
    }

    /// A number that changes whenever a plug-in instance starts or is let
    /// go of, so the shell knows when to look for new commands.
    #[must_use]
    pub fn generation(&self) -> u64 {
        let mut guard = lock(&self.inner.main);
        guard.get().map_or(0, |main| main.generation)
    }

    /// Runs the device's plug-in's command `id` with `args` on the main
    /// thread, through the agent-commands extension, and returns its result
    /// and the parameters it changed (read from the plug-in before and
    /// after). The plug-in also sends the changes as gestures, which
    /// [`ClapHost::take_param_changes`] then finds already in the set.
    ///
    /// # Errors
    ///
    /// [`ClapError::NoInstance`] without a running plug-in,
    /// [`ClapError::Refused`] if it has no commands or the command failed,
    /// saying why.
    pub fn run_command(
        &self,
        device: DeviceId,
        id: &str,
        args: &serde_json::Value,
    ) -> Result<CommandRun, ClapError> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        let mut plugin = instance.instance.plugin_handle();
        let commands = plugin
            .get_extension::<PluginAgentCommands>()
            .ok_or_else(|| ClapError::Refused("commands: it offers none".to_owned()))?;
        let before = values(&mut plugin);
        let result = commands
            .run(&mut plugin, id, args)
            .map_err(ClapError::Refused)?;
        let after = values(&mut plugin);
        let changes: Vec<(ParamId, f64)> = after
            .into_iter()
            .filter(|(param, value)| {
                before
                    .iter()
                    .find(|(p, _)| p == param)
                    .is_none_or(|(_, old)| old != value)
            })
            .collect();
        // While it's inactive, nothing processes the gestures it queued;
        // a flush takes them (the changes are in the set already).
        if let Some(mut inactive) = instance.instance.inactive_plugin_handle()
            && let Some(params) = inactive.get_extension::<PluginParams>()
        {
            params.flush(
                &mut inactive,
                &InputEvents::empty(),
                &mut OutputEvents::void(),
            );
        }
        instance
            .echoes
            .extend(changes.iter().map(|(param, value)| (param.0, *value)));
        let over = instance.echoes.len().saturating_sub(ECHOES);
        instance.echoes.drain(..over);
        Ok(CommandRun { result, changes })
    }

    /// The plug-ins loaded so far, as device types.
    #[must_use]
    pub fn types(&self) -> Vec<ClapType> {
        lock(&self.inner.types).clone()
    }

    /// Does the main thread's work: gives new nodes their plug-ins,
    /// deactivates plug-ins their nodes gave back, restarts plug-ins that
    /// asked, calls back plug-ins that asked, and collects the parameter
    /// changes plug-ins made. Returns whether anything a window shows may
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
        for (&device, instance) in &mut main.instances {
            changed |= pump(device, instance, &mut changes);
        }
        let mut discarded = Vec::new();
        for (device, instance) in &mut main.closing {
            pump(*device, instance, &mut discarded);
        }
        main.closing.retain(|(_, instance)| instance.out);
        changed |= !changes.is_empty();
        main.changes.extend(changes);
        changed
    }

    /// The parameter changes plug-ins made themselves since the last call:
    /// each gesture once, with its final value.
    pub fn take_param_changes(&self) -> Vec<ParamChange> {
        let mut guard = lock(&self.inner.main);
        guard
            .get()
            .map(|main| std::mem::take(&mut main.changes))
            .unwrap_or_default()
    }

    /// The device's plug-in's own state, with the file it came from, as the
    /// set saves it.
    ///
    /// # Errors
    ///
    /// [`ClapError::NoInstance`] if the device has no plug-in running, and
    /// [`ClapError::Refused`] if the plug-in won't save.
    pub fn save_state(&self, device: DeviceId) -> Result<PluginState, ClapError> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        let mut data = Vec::new();
        let plugin = instance.instance.plugin_handle();
        if let Some(state) = plugin.get_extension::<StateExtension>() {
            state
                .save(&plugin, &mut data)
                .map_err(|_| ClapError::Refused("to save its state".to_owned()))?;
        }
        Ok(PluginState {
            path: instance.path.to_string_lossy().into_owned(),
            data,
        })
    }

    /// Gives the device's plug-in a state it saved before. The plug-in
    /// keeps the state it had if it refuses.
    ///
    /// # Errors
    ///
    /// [`ClapError::NoInstance`] if the device has no plug-in running, and
    /// [`ClapError::Refused`] if the plug-in refuses the state.
    pub fn load_state(&self, device: DeviceId, data: &[u8]) -> Result<(), ClapError> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        load_state(instance, data)?;
        // A state that changes its latency, say, asks for a restart; a
        // plug-in that refused to activate, or failed, gets another try.
        instance.refused = None;
        instance.silenced = false;
        Ok(())
    }

    /// Opens the device's plug-in's editor: in a host window of
    /// Beatlayer's own, with the plug-in's view embedded into it, where the
    /// platform builds one and the plug-in supports it (WP-2.10); in a
    /// floating window of the plug-in's own otherwise, titled `title`.
    ///
    /// # Errors
    ///
    /// [`ClapError::NoInstance`] without a plug-in, and
    /// [`ClapError::Refused`] if it has no editor, or it won't open.
    pub fn open_editor(&self, device: DeviceId, title: &str) -> Result<(), ClapError> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        if instance.editor_open {
            return Ok(());
        }
        if open_embedded(instance, title)? {
            return Ok(());
        }
        let mut plugin = instance.instance.plugin_handle();
        let (gui, configuration) = floating_editor(&mut plugin)
            .ok_or_else(|| ClapError::Refused("to open its editor in a window".to_owned()))?;
        gui.create(&plugin, configuration)
            .map_err(|_| ClapError::Refused("to open its editor".to_owned()))?;
        if let Ok(title) = CString::new(title) {
            gui.suggest_title(&plugin, &title);
        }
        if gui.show(&plugin).is_err() {
            gui.destroy(&plugin);
            return Err(ClapError::Refused("to show its editor".to_owned()));
        }
        instance.editor_open = true;
        instance.editor_hidden = false;
        instance.editor_wanted = true;
        Ok(())
    }

    /// Closes the device's plug-in's editor, if it's open.
    ///
    /// # Errors
    ///
    /// [`ClapError::NoInstance`] without a plug-in.
    pub fn close_editor(&self, device: DeviceId) -> Result<(), ClapError> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        close_editor(instance);
        Ok(())
    }

    /// Shows or hides the device's plug-in's editor window without closing
    /// it: View ▸ Plug-In Windows and Auto-Hide Plug-In Windows hide and
    /// show a plug-in's window as they do a floating device's, and a
    /// hidden window stays open. Does nothing while its editor is closed.
    /// While the plug-in refuses, it's asked once, and asked again only
    /// when the wanted visibility changes, not on every frame.
    ///
    /// # Errors
    ///
    /// [`ClapError::NoInstance`] without a plug-in, [`ClapError::Refused`]
    /// if it won't show or hide its editor, and [`ClapError::WrongThread`]
    /// off the main thread.
    pub fn set_editor_visible(&self, device: DeviceId, shown: bool) -> Result<(), ClapError> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        if !instance.editor_open {
            return Ok(());
        }
        let refused = if shown {
            ClapError::Refused("to show its editor".to_owned())
        } else {
            ClapError::Refused("to hide its editor".to_owned())
        };
        if instance.editor_wanted != shown && instance.editor_hidden != !shown {
            // The wanted visibility changed, and the window isn't there
            // yet: ask the plug-in.
            let plugin = instance.instance.plugin_handle();
            let Some(gui) = plugin.get_extension::<PluginGui>() else {
                instance.editor_wanted = shown;
                return Ok(());
            };
            let ok = if shown {
                gui.show(&plugin).is_ok()
            } else {
                gui.hide(&plugin).is_ok()
            };
            instance.editor_wanted = shown;
            if !ok {
                return Err(refused);
            }
            instance.editor_hidden = !shown;
            // The host window hides and shows with the plug-in's view.
            {
                if let Some(editor) = &instance.editor {
                    editor.window.set_visible(shown);
                }
            }
            return Ok(());
        }
        // Unchanged since the last ask: the window is either already
        // there, or the plug-in refused it and it's asked again only when
        // the wanted visibility changes.
        instance.editor_wanted = shown;
        if instance.editor_hidden == !shown {
            Ok(())
        } else {
            Err(refused)
        }
    }

    /// Resizes the device's editor window, as dragging its edge does: the
    /// size the plug-in adjusts to is asked of it (`adjust_size`), told to
    /// it (`set_size`), and the host window follows. The shell gives it the
    /// frame a window reopens with.
    ///
    /// # Errors
    ///
    /// [`ClapError::NoInstance`] without a plug-in, [`ClapError::WrongThread`]
    /// off the main thread, and [`ClapError::Refused`] if its editor isn't
    /// open in a host window, or the plug-in won't take the size.
    pub fn resize_editor(&self, device: DeviceId, size: GuiSize) -> Result<(), ClapError> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        if !instance.editor_open || instance.editor.is_none() {
            return Err(ClapError::Refused(
                "to resize an editor that isn't in a host window".to_owned(),
            ));
        }
        let plugin = instance.instance.plugin_handle();
        let Some(gui) = plugin.get_extension::<PluginGui>() else {
            return Err(ClapError::Refused("to resize its editor".to_owned()));
        };
        if !gui.can_resize(&plugin) {
            return Err(ClapError::Refused("to resize its editor".to_owned()));
        }
        let working = gui.adjust_size(&plugin, size).unwrap_or(size);
        gui.set_size(&plugin, working)
            .map_err(|_| ClapError::Refused("to resize its editor".to_owned()))?;
        if let Some(editor) = &instance.editor {
            editor.window.resize(working);
        }
        Ok(())
    }

    /// The devices whose plug-in's editor window is open, hidden or shown.
    #[must_use]
    pub fn open_editors(&self) -> Vec<DeviceId> {
        let mut guard = lock(&self.inner.main);
        guard
            .get()
            .map(|main| {
                main.instances
                    .iter()
                    .filter(|(_, i)| i.editor_open)
                    .map(|(device, _)| *device)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// What the device's plug-in is doing, or `None` if the device has no
    /// CLAP plug-in, running or failed.
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
                editor_scale: None,
                editor_size: None,
                values: Vec::new(),
            });
        }
        let instance = main.instances.get_mut(&device)?;
        let mut plugin = instance.instance.plugin_handle();
        let has_editor = has_editor(&mut plugin);
        let values = plugin
            .get_extension::<PluginParams>()
            .map(|params| {
                describe::params(&mut plugin)
                    .iter()
                    .filter_map(|p| {
                        let id = ClapId::from_raw(p.id.0)?;
                        Some((p.id, params.get_value(&plugin, id)?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let editor_scale = instance.editor.as_ref().map(|editor| editor.scale);
        let editor_size = instance.editor.as_ref().map(|editor| editor.window.size());
        Some(PluginStatus {
            plugin_id: instance.plugin_id.clone(),
            path: instance.path.clone(),
            active: instance.out,
            active_for: instance.active_for.filter(|_| instance.out),
            latency: instance.latency,
            failure: instance
                .failure
                .clone()
                .or_else(|| instance.refused.clone()),
            has_editor,
            editor_open: instance.editor_open,
            editor_hidden: instance.editor_hidden,
            editor_scale,
            editor_size,
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
                main.generation += 1;
                close(main, device, instance);
            }
        }
    }

    /// Lets go of every plug-in, as when another set opens.
    pub fn clear(&self) {
        self.retain(&BTreeSet::new());
    }

    /// Lends the plug-ins to an offline render (WP-2.6b): an export or a
    /// preview, rendering on this thread. The loan's
    /// [`factory`](Loan::factory) goes into the render's registry; for
    /// each CLAP device the render makes a node for, it takes the plug-in
    /// back from the engine's node (waiting for that node's next block),
    /// deactivates it, sets it to the rendered set's parameter values, and
    /// activates it for the render's rate and block size. A device with no
    /// plug-in yet gets one, from its saved state. Meanwhile the engine's
    /// node passes its input through.
    ///
    /// When the loan is dropped, after the render's engine, every plug-in
    /// goes back to the engine with the state it had before, activated for
    /// the engine's rate and block size again; one that failed while
    /// rendering stays silent in the engine too. Parameter changes a
    /// plug-in makes while rendering are dropped: a render changes nothing
    /// in the set.
    ///
    /// # Errors
    ///
    /// [`ClapError::WrongThread`] off the main thread, and
    /// [`ClapError::Lent`] while another loan is out.
    pub fn lend(&self) -> Result<Loan, ClapError> {
        let mut guard = lock(&self.inner.main);
        let main = guard.get()?;
        if main.lending {
            return Err(ClapError::Lent);
        }
        main.lending = true;
        main.loan_problems.clear();
        Ok(Loan {
            inner: Arc::clone(&self.inner),
            _main_thread: std::marker::PhantomData,
        })
    }
}

/// The host's plug-ins, lent to an offline render ([`ClapHost::lend`]).
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
    /// The factory for the render's registry: it makes each CLAP device's
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
            end_loan(main);
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
            format: PluginFormat::Clap,
            ..
        } = &device.kind
        else {
            return None;
        };
        let (node, ends) = ClapNode::new(
            config,
            extra_outputs(&self.inner, &device.kind),
            device.role == DeviceRole::MidiEffect,
        );
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

    fn outputs(&self, kind: &DeviceKind) -> Vec<String> {
        lock(&self.inner.types)
            .iter()
            .find(|t| t.device_type.kind == *kind)
            .map_or_else(Vec::new, |t| t.device_type.outputs.clone())
    }
}

/// The CLAP plug-ins' factory for the engine's registry. It makes each CLAP
/// device's node at once; the host gives the node its plug-in.
pub struct ClapFactory {
    inner: Arc<Inner>,
}

impl NodeFactory for ClapFactory {
    fn create(&self, device: &Device, config: &NodeConfig) -> Option<Box<dyn Node>> {
        let DeviceKind::Plugin {
            format: PluginFormat::Clap,
            ..
        } = &device.kind
        else {
            return None;
        };
        let (node, ends) = ClapNode::new(
            config,
            extra_outputs(&self.inner, &device.kind),
            device.role == DeviceRole::MidiEffect,
        );
        lock(&self.inner.requests).push(Request {
            device: device.clone(),
            config: *config,
            ends,
        });
        if std::thread::current().id() == self.inner.owner {
            ClapHost {
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

    fn outputs(&self, kind: &DeviceKind) -> Vec<String> {
        lock(&self.inner.types)
            .iter()
            .find(|t| t.device_type.kind == *kind)
            .map_or_else(Vec::new, |t| t.device_type.outputs.clone())
    }
}

/// How many audio output ports the plug-in of `kind` has past its main one
/// (WP-5.45): 0 for one the host hasn't met, so its node feeds only the main
/// port.
fn extra_outputs(inner: &Inner, kind: &DeviceKind) -> usize {
    lock(&inner.types)
        .iter()
        .find(|t| t.device_type.kind == *kind)
        .map_or(0, |t| t.device_type.outputs.len())
}

/// A CLAP plug-in's device kind.
fn kind(id: &str) -> DeviceKind {
    DeviceKind::Plugin {
        format: PluginFormat::Clap,
        plugin_id: id.to_owned(),
    }
}

/// The plug-in file at `path`, loaded once.
fn entry(main: &mut Main, path: &Path) -> Result<PluginEntry, ClapError> {
    if let Some(entry) = main.entries.get(path) {
        return Ok(entry.clone());
    }
    // SAFETY: loading a plug-in runs its code in this process, which is
    // what hosting it in process means (the architecture, "Plug-in
    // hosting"); clack checks the entry point and the CLAP version.
    let entry = unsafe { PluginEntry::load(path) }.map_err(|e| ClapError::Load(e.to_string()))?;
    main.entries.insert(path.to_owned(), entry.clone());
    Ok(entry)
}

fn new_instance(
    entry: &PluginEntry,
    id: &str,
    info: &HostInfo,
) -> Result<PluginInstance<Host>, ClapError> {
    let id = CString::new(id).map_err(|_| ClapError::UnknownPlugin(id.to_owned()))?;
    PluginInstance::<Host>::new(
        |()| Shared::default(),
        |_| MainThread::default(),
        entry,
        &id,
        info,
    )
    .map_err(|e| ClapError::Load(e.to_string()))
}

fn instance(main: &mut Main, device: DeviceId) -> Result<&mut Instance, ClapError> {
    main.instances
        .get_mut(&device)
        .ok_or(ClapError::NoInstance(device))
}

fn load_state(instance: &mut Instance, data: &[u8]) -> Result<(), ClapError> {
    let plugin = instance.instance.plugin_handle();
    let state = plugin
        .get_extension::<StateExtension>()
        .ok_or_else(|| ClapError::Refused("a state: it keeps none".to_owned()))?;
    state
        .load(&plugin, &mut &data[..])
        .map_err(|_| ClapError::Refused("the state".to_owned()))
}

/// Each parameter's value, as the plug-in reports it.
fn values(plugin: &mut PluginMainThreadHandle<'_>) -> Vec<(ParamId, f64)> {
    let Some(params) = plugin.get_extension::<PluginParams>() else {
        return Vec::new();
    };
    describe::params(plugin)
        .iter()
        .filter_map(|p| {
            let id = ClapId::from_raw(p.id.0)?;
            Some((p.id, params.get_value(plugin, id)?))
        })
        .collect()
}

/// The plug-in's commands, if it offers the agent-commands extension.
fn list_commands(instance: &mut PluginInstance<Host>) -> Option<Vec<CommandSpec>> {
    let mut plugin = instance.plugin_handle();
    let commands = plugin.get_extension::<PluginAgentCommands>()?;
    commands.list(&mut plugin).ok()
}

/// The plug-in's editor, if it opens in a window of its own on this
/// platform.
fn floating_editor(
    plugin: &mut PluginMainThreadHandle<'_>,
) -> Option<(PluginGui, GuiConfiguration<'static>)> {
    let gui = plugin.get_extension::<PluginGui>()?;
    let configuration = GuiConfiguration {
        api_type: GuiApiType::default_for_current_platform()?,
        is_floating: true,
    };
    gui.is_api_supported(plugin, configuration)
        .then_some((gui, configuration))
}

/// Whether the plug-in's editor can open: embedded in a host window of
/// Beatlayer's own (WP-2.10), where the platform builds one and the
/// plug-in supports it, or in a floating window of its own.
fn has_editor(plugin: &mut PluginMainThreadHandle<'_>) -> bool {
    embedded_editor(plugin).is_some() || floating_editor(plugin).is_some()
}

/// The plug-in's editor embedded in a host window of Beatlayer's own, where
/// the platform builds one and the plug-in supports the API (WP-2.10).
fn embedded_editor(
    plugin: &PluginMainThreadHandle<'_>,
) -> Option<(PluginGui, GuiConfiguration<'static>)> {
    let gui = plugin.get_extension::<PluginGui>()?;
    let configuration = GuiConfiguration {
        api_type: GuiApiType::default_for_current_platform()?,
        is_floating: false,
    };
    gui.is_api_supported(plugin, configuration)
        .then_some((gui, configuration))
}

/// Opens `instance`'s plug-in's editor embedded in a host window of
/// Beatlayer's own (WP-2.10): `Ok(true)` once it's open, `Ok(false)` if
/// this plug-in wants a floating window of its own instead (the caller
/// opens that).
///
/// CLAP's order for an embedded editor: `create`, the display's scale
/// (`set_scale`), the size (`get_size`, which the window is made with),
/// the parent (`set_parent`), then `show`.
fn open_embedded(instance: &mut Instance, title: &str) -> Result<bool, ClapError> {
    let mut plugin = instance.instance.plugin_handle();
    // Where no host window can be made on this thread (AppKit makes them
    // only on the main thread), a plug-in that also floats opens floating.
    if !EditorWindow::can_open() && floating_editor(&mut plugin).is_some() {
        return Ok(false);
    }
    let Some((gui, configuration)) = embedded_editor(&plugin) else {
        return Ok(false);
    };
    gui.create(&plugin, configuration)
        .map_err(|_| ClapError::Refused("to open its editor".to_owned()))?;
    let Some(size) = gui.get_size(&plugin) else {
        gui.destroy(&plugin);
        return Err(ClapError::Refused("to size its editor".to_owned()));
    };
    let resizable = gui.can_resize(&plugin);
    let window = match EditorWindow::open(title, size, resizable) {
        Ok(window) => window,
        Err(why) => {
            gui.destroy(&plugin);
            return Err(ClapError::Refused(format!(
                "to open its editor in a window: {why}"
            )));
        }
    };
    // The display's scale, for a plug-in that doesn't read it itself
    // (CLAP's `set_scale`). A refusal isn't fatal: the win32 and x11 APIs
    // measure in physical pixels besides. CLAP's `cocoa` API measures in
    // points, and a host never calls `set_scale` with it.
    let scale = window.scale();
    if configuration.api_type != GuiApiType::COCOA {
        let _ = gui.set_scale(&plugin, scale);
    }
    // SAFETY: the window outlives the plug-in's GUI: the GUI is destroyed
    // with the editor, before the window is dropped (`close_editor`).
    if unsafe { gui.set_parent(&plugin, window.handle()) }.is_err() {
        gui.destroy(&plugin);
        return Err(ClapError::Refused(
            "to embed its editor in a window".to_owned(),
        ));
    }
    if gui.show(&plugin).is_err() {
        gui.destroy(&plugin);
        return Err(ClapError::Refused("to show its editor".to_owned()));
    }
    window.set_visible(true);
    instance.editor = Some(Editor { window, scale });
    instance.editor_open = true;
    instance.editor_hidden = false;
    instance.editor_wanted = true;
    Ok(true)
}

/// Tells the plug-in its editor's window is now `size` physical pixels, as
/// CLAP asks for an embedded editor (`adjust_size`, then `set_size`); a
/// size the plug-in adjusts goes back to the window.
fn tell_editor_size(instance: &mut Instance, size: GuiSize) {
    let plugin = instance.instance.plugin_handle();
    let Some(gui) = plugin.get_extension::<PluginGui>() else {
        return;
    };
    let working = gui.adjust_size(&plugin, size).unwrap_or(size);
    let _ = gui.set_size(&plugin, working);
    if working != size
        && let Some(editor) = &instance.editor
    {
        editor.window.resize(working);
    }
}

/// Tells the plug-in its editor's display scale changed, and remembers it
/// (`set_scale`).
fn tell_editor_scale(instance: &mut Instance, scale: f64) {
    let plugin = instance.instance.plugin_handle();
    if let Some(gui) = plugin.get_extension::<PluginGui>() {
        let _ = gui.set_scale(&plugin, scale);
    }
    if let Some(editor) = &mut instance.editor {
        editor.scale = scale;
    }
}

/// What the person did to the device's editor window since the last look
/// (WP-2.10): a resize or a move to a monitor of another DPI reaches the
/// plug-in, and a window the person closed closes the editor.
fn pump_editor(instance: &mut Instance) -> bool {
    let mut changed = false;
    loop {
        let event = {
            if !instance.editor_open {
                break;
            }
            let Some(editor) = &instance.editor else {
                break;
            };
            editor.window.poll()
        };
        let Some(event) = event else {
            break;
        };
        match event {
            WindowEvent::Resized(size) => {
                tell_editor_size(instance, size);
                changed = true;
            }
            WindowEvent::ScaleChanged(scale) => {
                tell_editor_scale(instance, scale);
                changed = true;
            }
            WindowEvent::Closed => {
                close_editor(instance);
                changed = true;
                break;
            }
        }
    }
    changed
}

/// A size the plug-in itself asked its editor's window to become (CLAP's
/// `request_resize`, WP-2.10): the host window takes it, as the plug-in's
/// own view already did. A plug-in that takes no new size is told the size
/// the window has instead (`set_size`), which reverts its resize.
fn resize_for_plugin(instance: &mut Instance, size: GuiSize) {
    let plugin = instance.instance.plugin_handle();
    let Some(gui) = plugin.get_extension::<PluginGui>() else {
        return;
    };
    let Some(editor) = &instance.editor else {
        // A floating editor resizes its own window.
        return;
    };
    if gui.can_resize(&plugin) {
        editor.window.resize(size);
    } else {
        let _ = gui.set_size(&plugin, editor.window.size());
    }
}

fn close_editor(instance: &mut Instance) {
    if !instance.editor_open {
        return;
    }
    let plugin = instance.instance.plugin_handle();
    if let Some(gui) = plugin.get_extension::<PluginGui>() {
        let _ = gui.hide(&plugin);
        gui.destroy(&plugin);
    }
    // The window closes with the editor, after the plug-in's view is
    // destroyed.
    {
        instance.editor = None;
    }
    instance.editor_open = false;
    instance.editor_hidden = false;
    instance.editor_wanted = true;
}

/// Lets go of a plug-in: closes its editor and asks its node for its
/// processor, keeping it until that's back.
fn close(main: &mut Main, device: DeviceId, mut instance: Instance) {
    close_editor(&mut instance);
    for ends in instance.ends.iter().chain(&instance.retired) {
        ends.status.hand_back.store(true, Relaxed);
    }
    instance.refused = Some("let go of".to_owned());
    let mut discarded = Vec::new();
    pump(device, &mut instance, &mut discarded);
    if instance.out {
        main.closing.push((device, instance));
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
    let made = path
        .clone()
        .ok_or_else(|| ClapError::UnknownPlugin(plugin_id.clone()))
        .and_then(|path| {
            let entry = entry(main, &path)?;
            new_instance(&entry, plugin_id, &inner.info).map(|i| (i, path))
        });
    let (plugin, path) = match made {
        Ok(made) => made,
        Err(error) => {
            // Spec: clap-hosting R12. The device stays, passing its input
            // through, and keeps its saved state for when the plug-in is
            // found.
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
    main.missing.remove(&device.id);
    let mut instance = Instance {
        plugin_id: plugin_id.clone(),
        path,
        instance: plugin,
        ends: None,
        retired: Vec::new(),
        config: None,
        out: false,
        refused: None,
        failure: None,
        silenced: false,
        active_for: None,
        lent: None,
        latency: 0,
        editor_open: false,
        editor_hidden: false,
        editor_wanted: true,
        editor: None,
        gestures: BTreeSet::new(),
        held: BTreeMap::new(),
        commands: None,
        echoes: Vec::new(),
    };
    instance.commands = list_commands(&mut instance.instance);
    // Spec: clap-hosting R9, R11. The saved state first, then the set's
    // parameter values over it.
    if let Some(saved) = device.plugin.as_ref().filter(|p| !p.data.is_empty())
        && let Err(error) = load_state(&mut instance, &saved.data)
    {
        instance.failure = Some(format!("{error}; it starts from its defaults"));
    }
    set_values(&mut instance, device);
    main.instances.insert(device.id, instance);
    main.generation += 1;
    true
}

/// Lends the device's plug-in to an offline render's node (WP-2.6b): takes
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
        // Spec: clap-hosting R12, as in the engine.
        return Ok(());
    }
    let Main {
        instances, changes, ..
    } = main;
    let Some(instance) = instances.get_mut(&device.id) else {
        return Ok(());
    };
    if instance.lent.is_some() {
        return Err("its plug-in is lent to this render already".to_owned());
    }
    // Spec: offline-plugin-renders R4. Its state, to give back.
    let saved = save(instance);
    // Spec: offline-plugin-renders R2. The engine's node hands it back at
    // its next block.
    if instance.out {
        for ends in instance.ends.iter().chain(&instance.retired) {
            ends.status.hand_back.store(true, Relaxed);
        }
        let until = Instant::now() + HAND_BACK_WAIT;
        loop {
            take_back(device.id, instance, changes);
            if !instance.out {
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
    if let Ok((processor, latency)) = activate(&mut instance.instance, &config) {
        lent.ends.status.latency.store(latency, Relaxed);
        if lent.ends.to_node.push(processor).is_ok() {
            instance.out = true;
            instance.active_for = Some(config);
        }
    }
    // Spec: clap-hosting R12. One that won't activate for the render
    // leaves its node passing its input through.
    instance.lent = Some(lent);
    Ok(())
}

/// Ends a loan: every lent plug-in goes back to the engine's node with the
/// state it had, or stays silent if it failed while rendering.
fn end_loan(main: &mut Main) {
    if !main.lending {
        return;
    }
    main.lending = false;
    main.loan_problems.clear();
    let Main {
        instances, changes, ..
    } = main;
    for (&device, instance) in instances.iter_mut() {
        let Some(lent) = &instance.lent else { continue };
        lent.ends.status.hand_back.store(true, Relaxed);
        // The render's engine is gone, and its node gave the processor
        // back as it went.
        take_back(device, instance, changes);
        let Some(lent) = instance.lent.take() else {
            continue;
        };
        let failed = lent.ends.status.failed.load(Relaxed);
        if instance.out {
            // A render's node still holds it: it's deactivated once that
            // node gives it back.
            instance.retired.push(lent.ends);
        }
        // Spec: offline-plugin-renders R4. The state it had.
        if let Some(saved) = &lent.saved {
            let _ = load_state(instance, saved);
        }
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
        pump(device, instance, changes);
    }
}

/// The plug-in's state, if it keeps one and will save it.
fn save(instance: &mut Instance) -> Option<Vec<u8>> {
    let plugin = instance.instance.plugin_handle();
    let state = plugin.get_extension::<StateExtension>()?;
    let mut data = Vec::new();
    state.save(&plugin, &mut data).ok()?;
    Some(data)
}

/// Sets the plug-in's parameters to the device's values, while it's
/// inactive.
fn set_values(instance: &mut Instance, device: &Device) {
    if device.params.is_empty() {
        return;
    }
    let Some(mut plugin) = instance.instance.inactive_plugin_handle() else {
        return;
    };
    let Some(params) = plugin.get_extension::<PluginParams>() else {
        return;
    };
    let mut events = EventBuffer::with_capacity(device.params.len());
    for (param, value) in &device.params {
        if let Some(id) = ClapId::from_raw(param.0) {
            events.push(&clack_host::events::event_types::ParamValueEvent::new(
                0,
                id,
                Pckn::match_all(),
                *value,
            ));
        }
    }
    params.flush(
        &mut plugin,
        &InputEvents::from_buffer(&events),
        &mut OutputEvents::void(),
    );
}

/// Takes back the processors nodes handed back and deactivates the
/// plug-in, and collects the parameter changes the plug-in made in the
/// engine's nodes. Returns whether anything changed.
fn take_back(device: DeviceId, instance: &mut Instance, changes: &mut Vec<ParamChange>) -> bool {
    let mut changed = false;
    let mut returned = Vec::new();
    for ends in instance.ends.iter_mut().chain(instance.retired.iter_mut()) {
        while let Ok(processor) = ends.from_node.pop() {
            returned.push(processor);
        }
        while let Ok(change) = ends.params.pop() {
            take_change(
                device,
                (&mut instance.gestures, &mut instance.held),
                &mut instance.echoes,
                change,
                changes,
            );
        }
    }
    if let Some(lent) = &mut instance.lent {
        while let Ok(processor) = lent.ends.from_node.pop() {
            returned.push(processor);
        }
        // Spec: offline-plugin-renders R5. A render changes nothing in the
        // set.
        while lent.ends.params.pop().is_ok() {}
    }
    for processor in returned {
        let stopped = processor.into_stopped();
        if stopped.matches(&instance.instance) {
            instance.instance.deactivate(stopped);
        }
        instance.out = false;
        changed = true;
    }
    // A node dropped with the processor still in its queue drops it there.
    if instance.out && !instance.instance.is_active() {
        instance.out = false;
    }
    if instance.out && instance.instance.try_deactivate().is_ok() {
        instance.out = false;
    }
    if !instance.out {
        instance.active_for = None;
    }
    changed
}

/// One plug-in's main-thread work. Returns whether anything changed.
fn pump(device: DeviceId, instance: &mut Instance, changes: &mut Vec<ParamChange>) -> bool {
    // Processors handed back are deactivated.
    let mut changed = take_back(device, instance, changes);
    instance
        .retired
        .retain(|ends| !(ends.from_node.is_abandoned() && ends.from_node.is_empty()));

    // A restart, or a new latency, while it's out: ask for it back.
    let restart = instance
        .instance
        .access_shared_handler(|s| s.restart.swap(false, Relaxed))
        | instance
            .instance
            .access_handler(|m| m.latency_changed.replace(false));
    if restart
        && instance.out
        && instance.lent.is_none()
        && let Some(ends) = &instance.ends
    {
        ends.status.hand_back.store(true, Relaxed);
    }
    if let Some(ends) = &instance.ends
        && ends.status.failed.load(Relaxed)
        && !instance.silenced
    {
        instance.silenced = true;
        instance.failure = Some("it failed while processing; the device is silent".to_owned());
        changed = true;
    }

    // Activate it for its node, unless a render has it.
    if !instance.out
        && instance.refused.is_none()
        && !instance.silenced
        && instance.lent.is_none()
        && let (Some(ends), Some(config)) = (&mut instance.ends, instance.config)
        && !ends.to_node.is_abandoned()
        && !instance.instance.is_active()
    {
        changed = true;
        match activate(&mut instance.instance, &config) {
            Ok((processor, latency)) => {
                instance.latency = latency;
                ends.status.latency.store(latency, Relaxed);
                if ends.to_node.push(processor).is_ok() {
                    instance.out = true;
                    instance.active_for = Some(config);
                }
            }
            Err(why) => instance.refused = Some(why),
        }
    }

    if instance
        .instance
        .access_shared_handler(|s| s.callback.swap(false, Relaxed))
    {
        instance.instance.call_on_main_thread_callback();
    }
    if instance
        .instance
        .access_shared_handler(|s| s.editor_closed.swap(false, Relaxed))
    {
        close_editor(instance);
        changed = true;
    }
    // WP-2.10: what the person did to its editor's host window.
    {
        changed |= pump_editor(instance);
    }
    // WP-2.10: a size the plug-in itself asked its editor's host window
    // for (CLAP's `request_resize`).
    if let Some((width, height)) = instance
        .instance
        .access_shared_handler(|shared| shared.take_resize_request())
    {
        resize_for_plugin(instance, GuiSize { width, height });
    }
    changed
}

/// Spec: clap-hosting R6. A change inside a gesture waits for its end. A
/// change a command made (spec: clap-hosting R19), which the set already
/// holds, is dropped.
fn take_change(
    device: DeviceId,
    (gestures, held): (&mut BTreeSet<u32>, &mut BTreeMap<u32, f64>),
    echoes: &mut Vec<(u32, f64)>,
    change: ParamOut,
    changes: &mut Vec<ParamChange>,
) {
    let mut emit = |id: u32, value: f64| {
        if let Some(at) = echoes.iter().position(|echo| *echo == (id, value)) {
            echoes.remove(at);
            return;
        }
        let param = ParamId(id);
        changes.retain(|c| !(c.device == device && c.param == param));
        changes.push(ParamChange {
            device,
            param,
            value,
        });
    };
    match change {
        ParamOut::Begin(id) => {
            gestures.insert(id);
        }
        ParamOut::Value(id, value) => {
            if gestures.contains(&id) {
                held.insert(id, value);
            } else {
                emit(id, value);
            }
        }
        ParamOut::End(id) => {
            gestures.remove(&id);
            if let Some(value) = held.remove(&id) {
                emit(id, value);
            }
        }
    }
}

/// Activates a plug-in for a node's settings. Returns its processor and
/// its latency, or why it refused.
fn activate(
    instance: &mut PluginInstance<Host>,
    config: &NodeConfig,
) -> Result<(node::Processor, u32), String> {
    let mut plugin = instance.plugin_handle();
    let ports = plugin.get_extension::<PluginAudioPorts>();
    let mut buffer = AudioPortInfoBuffer::default();
    let mut channels = |is_input: bool| {
        ports
            .and_then(|ports| {
                (ports.count(&plugin, is_input) > 0)
                    .then(|| ports.get(&plugin, 0, is_input, &mut buffer))
                    .flatten()
                    .map(|info| info.channel_count as usize)
            })
            .unwrap_or(0)
            .min(MAX_CHANNELS)
    };
    let (inputs, outputs) = (channels(true), channels(false));
    let params = plugin.get_extension::<PluginParams>();
    let descriptors = describe::params(&mut plugin);
    let mut defaults: Vec<(u32, f64)> = descriptors.iter().map(|p| (p.id.0, p.default)).collect();
    defaults.sort_by_key(|(id, _)| *id);
    // Each control starts where the plug-in is: the set's values and its
    // saved state are on it before it's activated (clap-hosting R11).
    let modulatable = describe::modulatable(&mut plugin);
    let drive = Driver::new(descriptors.into_iter().map(|p| {
        let value = params
            .zip(ClapId::from_raw(p.id.0))
            .and_then(|(params, id)| params.get_value(&plugin, id))
            .unwrap_or(p.default);
        (p.clone(), modulatable.contains(&p.id.0), value)
    }));
    let max_frames = u32::try_from(config.max_frames.max(1)).unwrap_or(u32::MAX);
    let processor = instance
        .activate(
            move |_, _| ProcessorData {
                inputs,
                outputs,
                defaults,
                params,
                drive,
            },
            PluginAudioConfiguration {
                sample_rate: f64::from(config.sample_rate.hz()),
                min_frames_count: 1,
                max_frames_count: max_frames,
            },
        )
        .map_err(|e| format!("it refused to activate: {e}"))?;
    let plugin = instance.plugin_handle();
    let latency = plugin
        .get_extension::<PluginLatency>()
        .map_or(0, |latency| latency.get(&plugin));
    Ok((processor.into(), latency))
}
