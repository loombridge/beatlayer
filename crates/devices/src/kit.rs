//! Plug-ins built on the plug-in kit, compiled into Beatlayer and run as
//! native devices (the device framework spec, R21 to R25).
//!
//! A kit plug-in is a [`Plugin`] (its main-thread half) and a
//! [`Processor`] (its audio-thread half). Compiled in, it needs no plug-in
//! boundary: its parameters' descriptors are already Beatlayer's (both are
//! `lspk-params`'), its processor runs inside a [`NativeNode`] like any
//! native device, and its commands join Beatlayer's registry as device
//! commands.
//!
//! A [`KitHost`] lives on the main thread, as the CLAP host does, and keeps
//! one instance of the plug-in for each of its devices: it runs the
//! device's commands, keeps its own state, and makes the processor. Its
//! [`KitFactory`] goes into the engine's registry, where the compiler
//! makes each device's node at once without the plug-in; the main thread
//! activates the device's instance for the node the next time it runs
//! [`KitHost::idle`] (at once, when the compiler runs on the main thread)
//! and hands the processor over through a one-slot queue. Until it
//! arrives, an effect passes its input through and an instrument is
//! silent. An instance outlives its nodes: chain edits, new engines and
//! undoing a delete keep it and its state.
//!
//! The set is the truth for parameter values. The engine sends each change
//! to the node at its frame, where the processor gets it as a parameter
//! event on that frame. A change the plug-in makes itself (a command, its
//! editor) goes back to the set: [`KitHost::run_command`] returns the
//! changes a command made, for the shell to apply in the command's undo
//! step, and [`KitHost::take_param_changes`] the rest, one per gesture.
//! The engine then brings each to the node as it brings any other change.

use std::any::Any;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::ThreadId;

use bl_commands::{CommandSpec, Registry as Commands};
use bl_engine::{DeviceType, Node, NodeConfig, NodeEvent, NodeFactory, NoteKind, ParamValue};
use bl_model::{
    Device, DeviceDescription, DeviceId, DeviceKind, DeviceRole, Document, ParamId, PluginState,
};
use lspk_plugin::host::Edit;
use lspk_plugin::{
    Buffers, Event, EventSink, Events, Host, HostBackend, MAX_CHANNELS, Note, OutputEvent,
    ParamValues, Plugin, ProcessConfig, ProcessContext, Processor,
};
use rtrb::{Consumer, Producer, RingBuffer};
use serde_json::Value;

use crate::framework::{Audio, NativeDevice, NativeNode, ParamBank};

/// How many notes a stretch passes to a processor; more are dropped.
const NOTES: usize = 512;

/// A parameter change a kit plug-in made itself, outside a command (from
/// its editor, say): one for each gesture, with its final value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamChange {
    /// The device.
    pub device: DeviceId,
    /// The parameter.
    pub param: ParamId,
    /// Its new value, in its own units.
    pub value: f64,
}

/// What running a kit plug-in's command did.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandRun {
    /// The command's result.
    pub result: Value,
    /// The parameters it changed, each with its final value, for the set.
    pub changes: Vec<(ParamId, f64)>,
}

/// The kit plug-ins compiled into Beatlayer, and an instance for each of
/// their devices, on the thread that made it (CLAP's main thread, the
/// shell's).
///
/// Its methods do nothing (or return `None`) on other threads.
/// [`KitHost::idle`] must run on its thread often, as the shell's poll
/// does, for new nodes to get their processors.
#[derive(Clone)]
pub struct KitHost {
    inner: Arc<Inner>,
}

impl fmt::Debug for KitHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KitHost").finish_non_exhaustive()
    }
}

impl Default for KitHost {
    fn default() -> Self {
        Self::new()
    }
}

struct Inner {
    owner: ThreadId,
    kinds: Mutex<Vec<Arc<dyn Kind>>>,
    requests: Mutex<Vec<Request>>,
    main: Mutex<MainOnly<Main>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What only the owner thread may touch: the instances, which hold `Rc`s.
struct MainOnly<T> {
    owner: ThreadId,
    value: T,
}

// SAFETY: the value is reached only through `get`, which checks it's on the
// thread that made it; nothing inside leaves that thread.
unsafe impl<T> Send for MainOnly<T> {}

impl<T> MainOnly<T> {
    fn get(&mut self) -> Option<&mut T> {
        (std::thread::current().id() == self.owner).then_some(&mut self.value)
    }
}

/// A node the compiler made, waiting for its processor.
struct Request {
    device: DeviceId,
    kind: DeviceKind,
    /// The device as the compiler saw it, for an instance made here.
    values: Device,
    config: NodeConfig,
    ends: Box<dyn Any + Send>,
}

#[derive(Default)]
struct Main {
    instances: BTreeMap<DeviceId, Box<dyn Instance>>,
    /// Instances let go of while a node still had their processor.
    closing: Vec<Box<dyn Instance>>,
    changes: Vec<ParamChange>,
}

impl KitHost {
    /// A host with no plug-ins, whose main thread is this one.
    #[must_use]
    pub fn new() -> Self {
        let owner = std::thread::current().id();
        Self {
            inner: Arc::new(Inner {
                owner,
                kinds: Mutex::new(Vec::new()),
                requests: Mutex::new(Vec::new()),
                main: Mutex::new(MainOnly {
                    owner,
                    value: Main::default(),
                }),
            }),
        }
    }

    /// Adds kit plug-in `P` as a native device type: its kind is the
    /// plug-in's ID, its name the plug-in's, its role from its audio and
    /// note ports, and its description its parameters' descriptors with the
    /// generic layout. Returns the type.
    ///
    /// # Panics
    ///
    /// If its parameters break a rule (the device framework spec, R3), as
    /// a native device type that breaks one does.
    pub fn add<P: Plugin>(&self) -> DeviceType {
        let probe = P::new(Host::new(Box::new(Backend(Rc::default()))));
        let description = DeviceDescription::new(probe.params());
        if let Err(problem) = description.check() {
            panic!("{}: {problem}", P::INFO.name);
        }
        let info = P::INFO;
        let role = if info.audio.inputs == 0 && info.audio.outputs > 0 {
            DeviceRole::Instrument
        } else if info.audio.outputs == 0 && info.notes.output {
            DeviceRole::MidiEffect
        } else {
            DeviceRole::AudioEffect
        };
        let device_type =
            DeviceType::native(info.id, info.name, role).with_description(description);
        let mut kinds = lock(&self.inner.kinds);
        kinds.retain(|kind| kind.device_type().kind != device_type.kind);
        kinds.push(Arc::new(KindOf::<P> {
            device_type: device_type.clone(),
            _plugin: PhantomData,
        }));
        device_type
    }

    /// The kit plug-ins' device types.
    #[must_use]
    pub fn device_types(&self) -> Vec<DeviceType> {
        lock(&self.inner.kinds)
            .iter()
            .map(|kind| kind.device_type().clone())
            .collect()
    }

    /// Whether `kind` is one of the kit plug-ins'.
    #[must_use]
    pub fn handles(&self, kind: &DeviceKind) -> bool {
        self.kind(kind).is_some()
    }

    fn kind(&self, kind: &DeviceKind) -> Option<Arc<dyn Kind>> {
        lock(&self.inner.kinds)
            .iter()
            .find(|k| k.device_type().kind == *kind)
            .cloned()
    }

    /// The factory for the engine's registry, which makes each kit
    /// device's node.
    #[must_use]
    pub fn factory(&self) -> KitFactory {
        KitFactory {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Brings the instances in step with `document`: each kit device gets
    /// an instance if it has none (loading the state saved with the set),
    /// and each instance takes its device's parameter values.
    pub fn sync(&self, document: &Document) {
        let mut guard = lock(&self.inner.main);
        let Some(main) = guard.get() else { return };
        for track in document.tracks() {
            for device in track.devices_deep() {
                let Some(kind) = self.kind(&device.kind) else {
                    continue;
                };
                main.instances
                    .entry(device.id)
                    .or_insert_with(|| new_instance(kind.as_ref(), device))
                    .set_values(device);
            }
        }
    }

    /// Does the main thread's work: gives new nodes their processors,
    /// deactivates processors their nodes gave back, calls back plug-ins
    /// that asked, and collects the parameter changes plug-ins made
    /// themselves. Returns whether anything changed.
    pub fn idle(&self) -> bool {
        let requests = std::mem::take(&mut *lock(&self.inner.requests));
        let mut guard = lock(&self.inner.main);
        let Some(main) = guard.get() else {
            return false;
        };
        let mut changed = !requests.is_empty();
        for request in requests {
            let Some(kind) = self.kind(&request.kind) else {
                continue;
            };
            main.instances
                .entry(request.device)
                .or_insert_with(|| new_instance(kind.as_ref(), &request.values))
                .connect(request.ends, request.config);
        }
        let mut changes = Vec::new();
        for (&device, instance) in &mut main.instances {
            let mut made = Vec::new();
            changed |= instance.pump(&mut made);
            changes.extend(made.into_iter().map(|(param, value)| ParamChange {
                device,
                param,
                value,
            }));
        }
        for instance in &mut main.closing {
            instance.pump(&mut Vec::new());
        }
        main.closing.retain(|instance| instance.is_out());
        for change in changes {
            main.changes
                .retain(|c| !(c.device == change.device && c.param == change.param));
            main.changes.push(change);
        }
        changed
    }

    /// The parameter changes plug-ins made themselves since the last call,
    /// outside commands: each gesture once, with its final value.
    pub fn take_param_changes(&self) -> Vec<ParamChange> {
        let mut guard = lock(&self.inner.main);
        guard
            .get()
            .map(|main| std::mem::take(&mut main.changes))
            .unwrap_or_default()
    }

    /// The device's plug-in's own commands, by their IDs within the
    /// plug-in; `None` if the device has no instance.
    #[must_use]
    pub fn commands(&self, device: DeviceId) -> Option<Vec<CommandSpec>> {
        let mut guard = lock(&self.inner.main);
        Some(guard.get()?.instances.get(&device)?.commands())
    }

    /// Runs the device's plug-in's command `id` with `args`, checked against
    /// its schema, and returns its result and the parameters it changed.
    ///
    /// # Errors
    ///
    /// The device has no instance, or why the command didn't run.
    pub fn run_command(
        &self,
        device: DeviceId,
        id: &str,
        args: &Value,
    ) -> Result<CommandRun, String> {
        let mut guard = lock(&self.inner.main);
        let main = guard
            .get()
            .ok_or("kit plug-ins' commands run on the main thread")?;
        let instance = main
            .instances
            .get_mut(&device)
            .ok_or_else(|| format!("{device} has no kit plug-in"))?;
        let result = instance.run(id, args)?;
        let mut changes = Vec::new();
        instance.drain(&mut changes);
        Ok(CommandRun { result, changes })
    }

    /// The device's plug-in's own state, as the set saves it: every
    /// parameter's value and the plug-in's own bytes, in the kit's state
    /// layout. The set's parameter values win when it's loaded again.
    #[must_use]
    pub fn save_state(&self, device: DeviceId) -> Option<PluginState> {
        let mut guard = lock(&self.inner.main);
        let data = guard.get()?.instances.get(&device)?.save_state();
        Some(PluginState {
            path: String::new(),
            data,
        })
    }

    /// Whether the device's plug-in has a processor running in a node.
    #[must_use]
    pub fn is_active(&self, device: DeviceId) -> bool {
        let mut guard = lock(&self.inner.main);
        guard
            .get()
            .and_then(|main| main.instances.get(&device))
            .is_some_and(|instance| instance.is_out())
    }

    /// Lets go of every instance whose device isn't in `keep`, as when a
    /// set closes. A node still running keeps its processor until it gives
    /// it back.
    pub fn retain(&self, keep: &BTreeSet<DeviceId>) {
        let mut guard = lock(&self.inner.main);
        let Some(main) = guard.get() else { return };
        let gone: Vec<DeviceId> = main
            .instances
            .keys()
            .filter(|d| !keep.contains(d))
            .copied()
            .collect();
        for device in gone {
            if let Some(mut instance) = main.instances.remove(&device) {
                instance.release();
                instance.pump(&mut Vec::new());
                if instance.is_out() {
                    main.closing.push(instance);
                }
            }
        }
    }

    /// Lets go of every instance, as when another set opens.
    pub fn clear(&self) {
        self.retain(&BTreeSet::new());
    }
}

/// A new instance for `device`, with the state saved with the set and the
/// set's values.
fn new_instance(kind: &dyn Kind, device: &Device) -> Box<dyn Instance> {
    let mut instance = kind.instance();
    if let Some(saved) = device.plugin.as_ref().filter(|p| !p.data.is_empty()) {
        instance.load_state(&saved.data);
    }
    instance.set_values(device);
    instance
}

/// The kit plug-ins' factory for the engine's registry.
pub struct KitFactory {
    inner: Arc<Inner>,
}

impl NodeFactory for KitFactory {
    fn create(&self, device: &Device, config: &NodeConfig) -> Option<Box<dyn Node>> {
        let kind = lock(&self.inner.kinds)
            .iter()
            .find(|k| k.device_type().kind == device.kind)
            .cloned()?;
        let (node, ends) = kind.node(device, config);
        lock(&self.inner.requests).push(Request {
            device: device.id,
            kind: device.kind.clone(),
            values: device.clone(),
            config: *config,
            ends,
        });
        if std::thread::current().id() == self.inner.owner {
            KitHost {
                inner: Arc::clone(&self.inner),
            }
            .idle();
        }
        Some(node)
    }

    fn device_types(&self) -> Vec<DeviceType> {
        lock(&self.inner.kinds)
            .iter()
            .map(|kind| kind.device_type().clone())
            .collect()
    }
}

/// One kit plug-in type, for any thread.
trait Kind: Send + Sync {
    fn device_type(&self) -> &DeviceType;
    /// A node for `device`, and the main thread's ends of its queues.
    fn node(&self, device: &Device, config: &NodeConfig) -> (Box<dyn Node>, Box<dyn Any + Send>);
    /// A new instance, on the main thread.
    fn instance(&self) -> Box<dyn Instance>;
}

struct KindOf<P> {
    device_type: DeviceType,
    _plugin: PhantomData<fn() -> P>,
}

impl<P: Plugin> Kind for KindOf<P> {
    fn device_type(&self) -> &DeviceType {
        &self.device_type
    }

    fn node(&self, device: &Device, config: &NodeConfig) -> (Box<dyn Node>, Box<dyn Any + Send>) {
        let (to_node, from_main) = RingBuffer::new(1);
        let (to_main, from_node) = RingBuffer::new(2);
        let hand_back = Arc::new(AtomicBool::new(false));
        let kit = KitDevice::<P> {
            running: None,
            from_main,
            to_main,
            hand_back: Arc::clone(&hand_back),
            events: Vec::with_capacity(self.device_type.description.params.len() + NOTES),
            instrument: P::INFO.audio.inputs == 0,
            outputs: usize::from(P::INFO.audio.outputs).min(MAX_CHANNELS),
        };
        let node = NativeNode::new(kit, &self.device_type.description, device, config);
        let ends: Ends<P> = Ends {
            to_node,
            from_node,
            hand_back,
        };
        (Box::new(node), Box::new(ends))
    }

    fn instance(&self) -> Box<dyn Instance> {
        let flags = Rc::new(Flags::default());
        let host = Host::new(Box::new(Backend(Rc::clone(&flags))));
        let plugin = P::new(host.clone());
        let values = Arc::new(
            ParamValues::new(plugin.params())
                .expect("the descriptors passed the same checks when the type was added"),
        );
        let edits = host.connect(Arc::clone(&values));
        let mut commands = Commands::new();
        if plugin.register_commands(&mut commands).is_err() {
            commands = Commands::new();
        }
        Box::new(KitInstance {
            plugin,
            host,
            values,
            edits,
            flags,
            commands,
            ends: None,
            retired: Vec::new(),
            config: None,
            out: false,
            refused: None,
            gestures: BTreeSet::new(),
            held: BTreeMap::new(),
        })
    }
}

/// A processor on its way to a node, with the values it reads.
struct Running<P: Plugin> {
    processor: P::Processor,
    values: Arc<ParamValues>,
}

/// The main thread's ends of a node's queues.
struct Ends<P: Plugin> {
    to_node: Producer<Running<P>>,
    from_node: Consumer<Running<P>>,
    /// The node gives its processor back when this is set.
    hand_back: Arc<AtomicBool>,
}

/// What a plug-in asked of Beatlayer through its [`Host`].
#[derive(Default)]
struct Flags {
    restart: Cell<bool>,
    callback: Cell<bool>,
}

/// Beatlayer as a kit plug-in's host. Each request only sets a flag for
/// [`KitHost::idle`]; parameter edits are taken from the edit queue there
/// and after each command.
struct Backend(Rc<Flags>);

impl HostBackend for Backend {
    fn request_flush(&self) {}
    fn mark_dirty(&self) {
        // The set takes the plug-in's state each time it's saved.
    }
    fn request_restart(&self) {
        self.0.restart.set(true);
    }
    fn request_process(&self) {
        // Beatlayer calls every node every block.
    }
    fn request_callback(&self) {
        self.0.callback.set(true);
    }
    fn values_changed(&self) {}
    fn host_name(&self) -> Option<String> {
        Some("Beatlayer".to_owned())
    }
}

/// One device's instance, on the main thread.
trait Instance {
    fn commands(&self) -> Vec<CommandSpec>;
    fn run(&mut self, id: &str, args: &Value) -> Result<Value, String>;
    fn set_values(&self, device: &Device);
    fn load_state(&mut self, data: &[u8]);
    fn save_state(&self) -> Vec<u8>;
    fn connect(&mut self, ends: Box<dyn Any + Send>, config: NodeConfig);
    /// The main thread's work; adds finished parameter changes to
    /// `changes`. Returns whether anything changed.
    fn pump(&mut self, changes: &mut Vec<(ParamId, f64)>) -> bool;
    /// Takes the plug-in's waiting edits, adding finished changes.
    fn drain(&mut self, changes: &mut Vec<(ParamId, f64)>);
    /// Asks every node to give its processor back, and activates no more.
    fn release(&mut self);
    /// Whether a node has its processor.
    fn is_out(&self) -> bool;
}

struct KitInstance<P: Plugin> {
    plugin: P,
    host: Host,
    values: Arc<ParamValues>,
    edits: Consumer<Edit>,
    flags: Rc<Flags>,
    commands: Commands,
    /// The node it runs for, and nodes it ran for that haven't given its
    /// processor back yet.
    ends: Option<Ends<P>>,
    retired: Vec<Ends<P>>,
    config: Option<NodeConfig>,
    /// A node has its processor.
    out: bool,
    /// Why it couldn't activate for this node; not tried again until the
    /// next node.
    refused: Option<String>,
    gestures: BTreeSet<ParamId>,
    held: BTreeMap<ParamId, f64>,
}

impl<P: Plugin> Instance for KitInstance<P> {
    fn commands(&self) -> Vec<CommandSpec> {
        self.commands.iter().cloned().collect()
    }

    fn run(&mut self, id: &str, args: &Value) -> Result<Value, String> {
        let plugin = &mut self.plugin;
        lspk_plugin::agent_commands::run_checked(&self.commands, id, args, |id, args| {
            plugin
                .run_command(id, args)
                .map_err(|e| e.message().to_owned())
        })
    }

    fn set_values(&self, device: &Device) {
        for descriptor in self.values.descriptors() {
            let value = device
                .params
                .get(&descriptor.id)
                .copied()
                .unwrap_or(descriptor.default);
            self.values.set(descriptor.id, value);
        }
    }

    fn load_state(&mut self, data: &[u8]) {
        // The set's parameter values win over the state's.
        if let Ok(decoded) = lspk_plugin::state::decode(data) {
            let _ = self.plugin.load_state(decoded.own);
        }
    }

    fn save_state(&self) -> Vec<u8> {
        lspk_plugin::state::encode(&self.values, &self.plugin.save_state())
    }

    fn connect(&mut self, ends: Box<dyn Any + Send>, config: NodeConfig) {
        let Ok(ends) = ends.downcast::<Ends<P>>() else {
            return;
        };
        if let Some(old) = self.ends.take() {
            old.hand_back.store(true, Relaxed);
            self.retired.push(old);
        }
        self.ends = Some(*ends);
        self.config = Some(config);
        self.refused = None;
    }

    fn pump(&mut self, changes: &mut Vec<(ParamId, f64)>) -> bool {
        let mut changed = false;
        // Processors given back are deactivated here, on the main thread.
        let mut returned = Vec::new();
        for ends in self.ends.iter_mut().chain(self.retired.iter_mut()) {
            while let Ok(running) = ends.from_node.pop() {
                returned.push(running);
            }
        }
        for running in returned {
            self.plugin.deactivate(running.processor);
            self.out = false;
            changed = true;
        }
        let gone = |ends: &Ends<P>| ends.from_node.is_abandoned() && ends.from_node.is_empty();
        if self.out && self.ends.iter().chain(&self.retired).all(gone) {
            // Every node it ran for went without giving the processor back:
            // one that never took it leaves it in its queue, which drops it.
            self.out = false;
        }
        self.retired.retain(|ends| !gone(ends));

        if self.flags.restart.replace(false)
            && self.out
            && let Some(ends) = &self.ends
        {
            ends.hand_back.store(true, Relaxed);
        }

        if !self.out
            && self.refused.is_none()
            && let (Some(ends), Some(config)) = (&mut self.ends, self.config)
            && !ends.to_node.is_abandoned()
            && !ends.hand_back.load(Relaxed)
        {
            changed = true;
            let process = ProcessConfig::new(
                config.sample_rate.as_f64(),
                u32::try_from(config.max_frames.max(1)).unwrap_or(u32::MAX),
            );
            match self.plugin.activate(&process, &self.values) {
                Ok(processor) => {
                    let running = Running {
                        processor,
                        values: Arc::clone(&self.values),
                    };
                    match ends.to_node.push(running) {
                        Ok(()) => self.out = true,
                        Err(rtrb::PushError::Full(running)) => {
                            self.plugin.deactivate(running.processor);
                        }
                    }
                }
                Err(error) => self.refused = Some(error.message().to_owned()),
            }
        }

        if self.flags.callback.replace(false) {
            self.plugin.on_main_thread();
        }
        let before = changes.len();
        self.drain(changes);
        changed | (changes.len() > before)
    }

    fn drain(&mut self, changes: &mut Vec<(ParamId, f64)>) {
        let mut finish = |param: ParamId, value: f64| {
            changes.retain(|(p, _)| *p != param);
            changes.push((param, value));
        };
        while let Ok(edit) = self.edits.pop() {
            match edit {
                Edit::Begin(id) => {
                    self.gestures.insert(id);
                }
                Edit::Value { id, value, echo } => {
                    if !echo {
                        continue;
                    }
                    if self.gestures.contains(&id) {
                        self.held.insert(id, value);
                    } else {
                        finish(id, value);
                    }
                }
                Edit::End(id) => {
                    self.gestures.remove(&id);
                    if let Some(value) = self.held.remove(&id) {
                        finish(id, value);
                    }
                }
            }
        }
    }

    fn release(&mut self) {
        for ends in self.ends.iter().chain(&self.retired) {
            ends.hand_back.store(true, Relaxed);
        }
        self.refused = Some("let go of".to_owned());
    }

    fn is_out(&self) -> bool {
        self.out
    }
}

impl<P: Plugin> Drop for KitInstance<P> {
    fn drop(&mut self) {
        self.host.disconnect();
    }
}

/// A kit plug-in's processor as a native device.
struct KitDevice<P: Plugin> {
    running: Option<Running<P>>,
    from_main: Consumer<Running<P>>,
    to_main: Producer<Running<P>>,
    hand_back: Arc<AtomicBool>,
    /// A stretch's events for the processor, made with the node and never
    /// grown.
    events: Vec<Event>,
    instrument: bool,
    outputs: usize,
}

impl<P: Plugin> KitDevice<P> {
    /// Takes a processor the main thread sent, and gives it back when
    /// asked. Lock-free.
    fn exchange(&mut self) {
        if self.hand_back.load(Relaxed) {
            if let Some(running) = self.running.take()
                && let Err(rtrb::PushError::Full(running)) = self.to_main.push(running)
            {
                self.running = Some(running);
                return;
            }
            self.hand_back.store(false, Relaxed);
            return;
        }
        if self.running.is_none()
            && let Ok(running) = self.from_main.pop()
        {
            self.running = Some(running);
        }
    }

    /// The stretch's parameter changes as the processor's events at its
    /// first frame, recorded in the values it reads.
    fn push_changes(events: &mut Vec<Event>, values: &ParamValues, changes: &[NodeEvent]) {
        for change in changes {
            let Some(descriptor) = values.descriptor(change.param) else {
                continue;
            };
            let value = match change.value {
                ParamValue::Set(value) => descriptor.clamp(value),
                ParamValue::Default => descriptor.default,
            };
            values.set(descriptor.id, value);
            if events.len() < events.capacity() {
                events.push(Event::Param {
                    time: 0,
                    id: descriptor.id,
                    value,
                });
            }
        }
    }
}

/// What a processor sends the host: its own parameter changes and the
/// voices it ends, which Beatlayer doesn't take yet.
struct Unheard;

impl EventSink for Unheard {
    fn push(&mut self, _event: OutputEvent) -> bool {
        true
    }
}

impl<P: Plugin> NativeDevice for KitDevice<P> {
    fn process(&mut self, audio: &mut Audio<'_, '_>, _params: &ParamBank) {
        self.exchange();
        let Some(running) = self.running.as_mut() else {
            if self.instrument {
                audio.channels_mut().for_each(|channel| channel.fill(0.0));
            }
            return;
        };
        self.events.clear();
        Self::push_changes(&mut self.events, &running.values, audio.changes());
        for note in audio.notes() {
            if self.events.len() >= self.events.capacity() {
                break;
            }
            let time = note.offset;
            let which = Note {
                port: Some(0),
                channel: Some(note.channel),
                key: Some(note.key),
                id: note.note_id,
            };
            self.events.push(match note.kind {
                NoteKind::On => Event::NoteOn {
                    time,
                    note: which,
                    velocity: note.velocity,
                },
                NoteKind::Off => Event::NoteOff {
                    time,
                    note: which,
                    velocity: note.velocity,
                },
                NoteKind::Choke => Event::NoteChoke { time, note: which },
                // Per-note expressions wait for the kit host's MPE work:
                // the instruments that play them today are native.
                NoteKind::Expression { .. } => continue,
                // Spec: device-framework R39. A kit plug-in reads a channel
                // message as the MIDI event it is.
                NoteKind::Control { .. }
                | NoteKind::PitchBend { .. }
                | NoteKind::ChannelPressure { .. } => {
                    let Some(message) = note.message() else {
                        continue;
                    };
                    Event::Midi {
                        time,
                        port: 0,
                        data: message.padded(),
                    }
                }
            });
        }
        let frames = audio.frames();
        let mut channels = [std::ptr::null_mut(); MAX_CHANNELS];
        let mut count = 0;
        for channel in audio.channels_mut().take(self.outputs) {
            if self.instrument {
                channel.fill(0.0);
            }
            channels[count] = channel.as_mut_ptr();
            count += 1;
        }
        // SAFETY: each pointer is a distinct channel of the stretch, `frames`
        // long, and nothing else touches the audio until `process` returns.
        let mut buffers = unsafe { Buffers::from_raw(channels, count, frames) };
        let mut sink = Unheard;
        let mut cx = ProcessContext::new(&running.values, &mut sink);
        running
            .processor
            .process(&mut buffers, &Events::from_slice(&self.events), &mut cx);
    }

    fn flush(&mut self, events: &[NodeEvent], _params: &ParamBank) {
        self.exchange();
        let Some(running) = self.running.as_mut() else {
            return;
        };
        self.events.clear();
        Self::push_changes(&mut self.events, &running.values, events);
        let mut sink = Unheard;
        let mut cx = ProcessContext::new(&running.values, &mut sink);
        running
            .processor
            .flush(&Events::from_slice(&self.events), &mut cx);
    }
}

impl<P: Plugin> Drop for KitDevice<P> {
    fn drop(&mut self) {
        // On the drop thread: the processor goes back to the main thread,
        // which deactivates it.
        if let Some(running) = self.running.take() {
            let _ = self.to_main.push(running);
        }
    }
}

/// Adds the kit's example plug-ins, Example Gain and Example Sine, with the
/// `kit-examples` feature (off by default, so nothing user-facing ships);
/// without it, adds nothing. They prove the adapter until the first real
/// kit plug-in arrives.
pub fn add_examples(host: &KitHost) {
    #[cfg(feature = "kit-examples")]
    {
        host.add::<lspk_example_gain::Gain>();
        host.add::<lspk_example_sine::Sine>();
    }
    let _ = host;
}
