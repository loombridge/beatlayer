//! Audio Unit hosting on macOS (WP-2.8; the AU hosting spec).
//!
//! An [`AuHost`] lives on the main thread, which owns every unit it hosts:
//! it finds a unit by its component description, keeps one instance per
//! device, saves and restores their state, and activates and deactivates
//! them. Its [`AuFactory`] goes into the engine's registry: the compiler
//! thread asks it for a device's node, which it makes at once without the
//! unit, and the main thread activates the unit for that node the next time
//! it runs [`AuHost::idle`]. The node passes its input through until the
//! unit arrives.
//!
//! Offline renders (exports and agents' previews) borrow the same units
//! ([`AuHost::lend`], WP-4.21), as `ClapHost::lend` does for CLAP: each
//! unit comes back from the engine's node, is reactivated for the render's
//! rate and block size, and when the [`Loan`] ends goes back to the engine
//! with the state it had.
//!
//! Both Audio Units v2 and v3 are hosted through `AUAudioUnit`, the v3
//! host API, which bridges a v2 component (Apple's built-ins are v2) to it:
//! instantiation by component description, render blocks, the parameter
//! tree, `fullStateForDocument`, and latency are v3 names for v2's
//! `AudioComponentInstanceNew`, `AudioUnitRender`, parameter lists, class
//! info, and `kAudioUnitProperty_Latency`.
//!
//! A unit renders one segment at a time, the block split at its parameter
//! changes and notes (see the node's documentation); its state saves with
//! the set; its latency is reported for delay compensation (WP-2.11).
//! Units run in Beatlayer's own process until Phase 10's sandbox, so a unit
//! that crashes takes the app with it; a unit that fails a call is
//! contained: the song keeps playing, and its device says what went wrong.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::OnceLock;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use bl_engine::{DeviceType, Node, NodeConfig, NodeFactory};
use bl_model::param::Taper;
use bl_model::{
    Device, DeviceDescription, DeviceId, DeviceKind, DeviceRole, ParamDescriptor, ParamId,
    ParamKind, ParamUnit, PluginFormat, PluginState,
};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, msg_send};
use objc2_app_kit::{NSBackingStoreType, NSViewController, NSWindow, NSWindowStyleMask};
use objc2_audio_toolbox::{
    AUAudioUnit, AUAudioUnitBus, AUParameter, AUParameterAutomationEvent,
    AUParameterAutomationEventType, AUParameterObserverToken, AUParameterTree,
    AudioComponentDescription, AudioComponentFindNext, AudioComponentGetVersion,
    AudioComponentInstantiationOptions, AudioUnitParameterOptions, AudioUnitParameterUnit,
};
use objc2_avf_audio::AVAudioFormat;
use objc2_foundation::{
    NSArray, NSBundle, NSData, NSDate, NSDictionary, NSError, NSPropertyListFormat,
    NSPropertyListMutabilityOptions, NSPropertyListSerialization, NSRunLoop, NSString,
};

pub use crate::clap::{ParamChange, PluginStatus};
use crate::drive::Driver;
use crate::scan::ScannedPlugin;

use super::{AuError, description_id};
#[path = "node.rs"]
mod node;
use node::{AuNode, HostEnds, MAX_CHANNELS, Processor};

/// An Audio Unit Beatlayer can make devices of.
#[derive(Clone, Debug, PartialEq)]
pub struct AuType {
    /// Its device type: kind, name, role, and its parameters.
    pub device_type: DeviceType,
    /// Its vendor, if it gives one.
    pub vendor: Option<String>,
    /// Its version, if the system gives one.
    pub version: Option<String>,
}

/// Things only the thread that made the host may touch: the unit
/// instances, which live with the thread that made them.
struct MainOnly<T> {
    owner: ThreadId,
    value: T,
}

// SAFETY: the value is reached only through `get`, which checks it's on the
// thread that made it; the units inside never leave that thread.
unsafe impl<T> Send for MainOnly<T> {}

impl<T> MainOnly<T> {
    fn get(&mut self) -> Result<&mut T, AuError> {
        if std::thread::current().id() == self.owner {
            Ok(&mut self.value)
        } else {
            Err(AuError::WrongThread)
        }
    }
}

/// A node the compiler thread made, waiting for its unit.
struct Request {
    device: Device,
    config: NodeConfig,
    ends: HostEnds,
}

/// What the host and its factory share.
struct Inner {
    owner: ThreadId,
    requests: Mutex<Vec<Request>>,
    types: Mutex<Vec<AuType>>,
    main: Mutex<MainOnly<Main>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The main thread's units.
#[derive(Default)]
struct Main {
    instances: BTreeMap<DeviceId, Instance>,
    /// Devices whose unit couldn't be made, and why.
    missing: BTreeMap<DeviceId, (String, String)>,
    /// Units let go of while a node still had their processor, waiting for
    /// it to come back so they can be deactivated and let go of.
    closing: Vec<(DeviceId, Instance)>,
    /// Counts instances started and let go of.
    generation: u64,
    /// An offline render has the units ([`AuHost::lend`]).
    lending: bool,
    /// Devices the render couldn't borrow a unit for, and why.
    loan_problems: Vec<(DeviceId, String)>,
}

/// One device's unit.
struct Instance {
    plugin_id: String,
    unit: Retained<AUAudioUnit>,
    /// Every parameter of the unit's tree, in order, with the value it had
    /// when the unit was made: a parameter's [`ParamId`] is its position
    /// here, and that value is its default for a `ParamValue::Default`
    /// event.
    params: Vec<(Retained<AUParameter>, f64)>,
    /// The parameters the device type describes, for `plugin.info`.
    visible: Vec<ParamId>,
    /// Their descriptors, for the node's automation and modulation (the
    /// modulation spec, R10).
    descriptors: Vec<ParamDescriptor>,
    /// The node it runs for, and nodes it ran for that haven't given their
    /// processor back yet.
    ends: Option<HostEnds>,
    retired: Vec<HostEnds>,
    config: Option<NodeConfig>,
    /// A node has its processor.
    out: bool,
    /// Activation failed for this node; it isn't tried again until the next
    /// node.
    refused: Option<String>,
    failure: Option<String>,
    /// It failed while processing: its device is silent, and it isn't
    /// activated again until the next node.
    silenced: bool,
    /// The settings it's active for.
    active_for: Option<NodeConfig>,
    latency: u32,
    /// The unit's own editor, while it's open (WP-2.8's second part).
    editor: Option<Editor>,
    /// Its open editor is hidden: View ▸ Plug-In Windows or Auto-Hide
    /// Plug-In Windows hid it. Hidden is not closed.
    editor_hidden: bool,
    /// Its editor was asked for and the unit's view controller hasn't come
    /// back yet.
    opening: bool,
    /// The completion block, kept until it arrives.
    pending: Option<Box<ViewControllerBlock>>,
    /// Why the editor couldn't be opened, if it couldn't.
    editor_failure: Option<String>,
    /// The offline render that has it.
    lent: Option<Lent>,
}

/// A unit lent to an offline render: the render's node, and what to give
/// back to the engine afterwards.
struct Lent {
    ends: HostEnds,
    /// The unit's state before the render, which it gets back.
    saved: Option<Vec<u8>>,
    /// Why the engine's node had it refused, if it had.
    refused: Option<String>,
}

/// How long a render waits for the engine's node to hand a unit back: a
/// node does at its next block, a few milliseconds away.
const HAND_BACK_WAIT: Duration = Duration::from_secs(2);

/// How many parameter events one editor's observer holds for the main
/// thread before more are dropped. The buffer is reserved when the editor
/// opens, and the observer takes it with `try_lock`, so a callback the unit
/// makes on the audio thread neither allocates nor blocks.
const EDITOR_EVENTS: usize = 4096;

/// A parameter event a unit's editor made, before the main thread turns it
/// into an undo step.
#[derive(Clone, Copy)]
struct RawEvent {
    /// The unit's 64-bit parameter address.
    address: u64,
    /// The value then.
    value: f64,
    /// Where in a gesture it sits.
    kind: EventKind,
}

/// What part of a gesture a parameter event is.
#[derive(Clone, Copy)]
enum EventKind {
    /// A gesture began (`AUParameterAutomationEventTypeTouch`).
    Begin,
    /// A value within (or outside) a gesture.
    Value,
    /// A gesture ended (`AUParameterAutomationEventTypeRelease`).
    End,
}

/// The block a unit's view controller arrives through: Apple calls it on the
/// main thread with the controller, or null when the unit has none.
type ViewControllerBlock = RcBlock<dyn Fn(*mut NSViewController)>;

/// A unit's open editor: its window, and the observer that brings the
/// changes the unit's own controls make back into the set as one undo step
/// per gesture (R6).
struct Editor {
    /// Its window, holding the unit's view controller.
    window: Retained<NSWindow>,
    /// The unit's parameter tree, and the token of the observer added to
    /// it, so both can be let go of when the editor closes.
    tree: Retained<AUParameterTree>,
    token: AUParameterObserverToken,
    /// The events the observer saw, from whatever thread it ran on; the
    /// main thread drains them.
    events: Arc<Mutex<Vec<RawEvent>>>,
    /// The observer block, kept for as long as the token lives.
    _observer: RcBlock<dyn Fn(isize, NonNull<AUParameterAutomationEvent>)>,
    /// The gesture state, on the main thread only.
    gestures: Gestures,
}

/// Turns a unit editor's parameter events into changes: one per gesture,
/// with the value it ended on, or one for a value outside a gesture (spec:
/// clap-hosting R6).
#[derive(Default)]
struct Gestures {
    /// The parameter addresses whose gesture is open.
    open: BTreeSet<u64>,
    /// The value each open gesture last held.
    held: BTreeMap<u64, f64>,
}

impl Gestures {
    /// Feeds one event, returning the parameter and value to record, if the
    /// event completes a change.
    fn feed(&mut self, event: RawEvent) -> Option<(u64, f64)> {
        match event.kind {
            EventKind::Begin => {
                self.open.insert(event.address);
                None
            }
            EventKind::Value => {
                if self.open.contains(&event.address) {
                    self.held.insert(event.address, event.value);
                    None
                } else {
                    Some((event.address, event.value))
                }
            }
            EventKind::End => {
                self.open.remove(&event.address);
                self.held.remove(&event.address).map(|v| (event.address, v))
            }
        }
    }
}

/// How long a unit keeps living after it's let go of, before its last
/// reference is released.
///
/// Apple's `AUAudioUnitV2Bridge` rebuilds its parameter tree from an
/// asynchronous CoreAudio event, and a unit deallocated while that event is
/// in flight crashes inside AudioToolbox (recorded on 15 October 2026:
/// SIGSEGV in `-[AUAudioUnitV2Bridge _createParameterTree]_block_invoke`,
/// from `AUEventListener::MessagesAreAvailable`). Holding the unit for a
/// moment lets the event run against a live unit first. It costs one
/// reference and a quarter of a second of one unit's memory.
const RELEASE_GRACE: Duration = Duration::from_millis(250);

/// Holds a unit's last reference until the release thread lets it go.
struct Held(*mut AUAudioUnit);

// SAFETY: the pointer is one +1 reference, dropped exactly once, by the
// release thread: Objective-C's release is thread-agnostic, and no other
// thread touches the unit by then (the host has let go of it).
unsafe impl Send for Held {}

impl Held {
    fn release(self) {
        // SAFETY: it came from `Retained::into_raw`, and this is the one
        // release of that reference.
        drop(unsafe { Retained::from_raw(self.0) });
    }
}

/// Units waiting out [`RELEASE_GRACE`] before their last reference is
/// released, with when each was let go of.
static RELEASES: Mutex<Vec<(Instant, Held)>> = Mutex::new(Vec::new());

/// Lets go of `unit` after [`RELEASE_GRACE`], on a thread of its own: the
/// event that races a release runs on a queue of CoreAudio's, so no caller
/// thread may wait for it. The first release starts the thread; it lives
/// for the process.
fn release_later(unit: Retained<AUAudioUnit>) {
    lock(&RELEASES).push((Instant::now(), Held(Retained::into_raw(unit))));
    static RELEASING: OnceLock<()> = OnceLock::new();
    RELEASING.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("beatlayer-au-release".to_owned())
            .spawn(|| {
                loop {
                    std::thread::sleep(RELEASE_GRACE);
                    let now = Instant::now();
                    let due: Vec<Held> = {
                        let mut held = lock(&RELEASES);
                        let (due, later) =
                            std::mem::take(&mut *held).into_iter().partition(|(at, _)| {
                                now.saturating_duration_since(*at) >= RELEASE_GRACE
                            });
                        *held = later;
                        due.into_iter().map(|(_, held)| held).collect()
                    };
                    for unit in due {
                        unit.release();
                    }
                }
            });
    });
}

/// The Audio Unit host: it owns every unit it hosts, on the thread that
/// made it.
///
/// Its methods run only on that thread and return [`AuError::WrongThread`]
/// elsewhere. [`AuHost::idle`] must run there often (each frame, as the
/// shell's poll does) for new nodes to get their units.
#[derive(Clone)]
pub struct AuHost {
    inner: Arc<Inner>,
}

impl Default for AuHost {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AuHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuHost").finish_non_exhaustive()
    }
}

impl AuHost {
    /// A host with no units, whose main thread is this one.
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
                    value: Main::default(),
                }),
            }),
        }
    }

    /// The factory for the engine's registry, which makes a node for each
    /// Audio Unit device.
    #[must_use]
    pub fn factory(&self) -> AuFactory {
        AuFactory {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Finds the Audio Unit with the given component description — its
    /// type, subtype, and manufacturer, four characters each — and returns
    /// it as a device type, which devices can then be made of.
    ///
    /// # Errors
    ///
    /// [`AuError::Load`] if the fields aren't four characters, no installed
    /// unit has that description, it isn't an instrument, effect, or MIDI
    /// effect, or it won't start; [`AuError::WrongThread`] off the main
    /// thread.
    pub fn load(
        &self,
        component_type: &str,
        subtype: &str,
        manufacturer: &str,
    ) -> Result<AuType, AuError> {
        let mut guard = lock(&self.inner.main);
        guard.get()?;
        let id = description_id(component_type, subtype, manufacturer).ok_or_else(|| {
            AuError::Load("its type, subtype, and manufacturer are four characters each".to_owned())
        })?;
        let description = description_of(&id)
            .ok_or_else(|| AuError::Load(format!("{id} isn't a component description")))?;
        let role = role(description.componentType).ok_or_else(|| {
            AuError::Load(format!("{id} isn't an instrument, effect, or MIDI effect"))
        })?;
        // SAFETY: the description is a plain value the system understands;
        // finding a component runs no third-party code.
        let component = unsafe {
            AudioComponentFindNext(std::ptr::null_mut(), std::ptr::NonNull::from(&description))
        };
        if component.is_null() {
            return Err(AuError::Load(format!("no Audio Unit {id} is installed")));
        }
        let mut version = 0u32;
        // SAFETY: the component was just found, and the out pointer is a
        // live u32.
        let has_version =
            unsafe { AudioComponentGetVersion(component, std::ptr::NonNull::from(&mut version)) }
                == 0;
        let unit = instantiate(&description).map_err(AuError::Load)?;
        // SAFETY: reading the unit's own name and vendor is a property read
        // on the thread that made it.
        let name = unsafe { unit.audioUnitName() }
            .map(|name| name.to_string())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| id.clone());
        // SAFETY: as above; the unit was just made, on this thread.
        let vendor = unsafe { unit.manufacturerName() }
            .map(|vendor| vendor.to_string())
            .filter(|vendor| !vendor.trim().is_empty());
        let (descriptors, _, _) = describe(&unit);
        let found = AuType {
            device_type: DeviceType {
                kind: kind(&id),
                name,
                role,
                description: DeviceDescription::new(descriptors),
                sidechain: false,
                external: false,
            },
            vendor,
            version: has_version.then(|| version_text(version)),
        };
        drop(unit);
        let mut types = lock(&self.inner.types);
        types.retain(|t| t.device_type.kind != found.device_type.kind);
        types.push(found.clone());
        Ok(found)
    }

    /// The Audio Units loaded so far, as device types.
    #[must_use]
    pub fn types(&self) -> Vec<AuType> {
        lock(&self.inner.types).clone()
    }

    /// Does the main thread's work: gives new nodes their units,
    /// deactivates units their nodes gave back, and records what failed.
    /// Returns whether anything a window shows may have changed.
    pub fn idle(&self) -> bool {
        // A unit's view controller arrives on this thread's run loop; turn
        // it, without the lock held, so `finish_editor` can take the lock
        // when it arrives.
        let opening = lock(&self.inner.main)
            .get()
            .is_ok_and(|main| main.instances.values().any(|instance| instance.opening));
        if opening {
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
        }
        let requests = std::mem::take(&mut *lock(&self.inner.requests));
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else { return false };
        let mut changed = !requests.is_empty() || opening;
        for request in requests {
            serve(main, request);
        }
        for instance in main.instances.values_mut() {
            changed |= pump(instance);
        }
        for (_, instance) in &mut main.closing {
            pump(instance);
        }
        main.closing.retain(|(_, instance)| instance.out);
        changed
    }

    /// The device's unit's own state, as the set saves it.
    ///
    /// # Errors
    ///
    /// [`AuError::NoInstance`] if the device has no unit running, and
    /// [`AuError::Refused`] if the unit won't save.
    pub fn save_state(&self, device: DeviceId) -> Result<PluginState, AuError> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        let data = state_of(&instance.unit)
            .ok_or_else(|| AuError::Refused("to save its state".to_owned()))?;
        Ok(PluginState {
            path: String::new(),
            data,
        })
    }

    /// What the device's unit is doing, or `None` if the device has no
    /// Audio Unit, running or failed.
    #[must_use]
    pub fn status(&self, device: DeviceId) -> Option<PluginStatus> {
        let mut guard = lock(&self.inner.main);
        let main = guard.get().ok()?;
        if let Some((plugin_id, why)) = main.missing.get(&device) {
            return Some(PluginStatus {
                plugin_id: plugin_id.clone(),
                path: PathBuf::new(),
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
        let instance = main.instances.get(&device)?;
        let values: Vec<(ParamId, f64)> = instance
            .visible
            .iter()
            .filter_map(|param| {
                let value = instance.unit_value(param.0)?;
                Some((*param, value))
            })
            .collect();
        // SAFETY: reading a unit property is a read on the thread that owns
        // the unit.
        let provides_interface = unsafe { instance.unit.providesUserInterface() };
        let has_editor =
            instance.editor.is_some() || (instance.editor_failure.is_none() && provides_interface);
        Some(PluginStatus {
            plugin_id: instance.plugin_id.clone(),
            path: PathBuf::new(),
            active: instance.out,
            active_for: instance.active_for.filter(|_| instance.out),
            latency: instance.latency,
            failure: instance
                .failure
                .clone()
                .or_else(|| instance.refused.clone()),
            has_editor,
            editor_open: instance.editor.is_some(),
            editor_hidden: instance.editor_hidden,
            editor_scale: None,
            editor_size: None,
            values,
        })
    }

    /// Opens the device's unit's own editor in a window of its own, titled
    /// `title`, as a CLAP plug-in's editor opens (spec: clap-hosting R13,
    /// au-hosting's second part). Apple makes a unit's view controller on
    /// this thread and delivers it on the main thread's run loop, so the
    /// window is made when it arrives, in [`AuHost::idle`]; asking again
    /// while it's on its way does nothing.
    ///
    /// # Errors
    ///
    /// [`AuError::NoInstance`] without a unit, and [`AuError::WrongThread`]
    /// off the thread that made the host.
    pub fn open_editor(&self, device: DeviceId, title: &str) -> Result<(), AuError> {
        // The unit is cloned out and the lock dropped before the unit is
        // asked: Apple may call the completion synchronously, and
        // `finish_editor` takes the lock.
        let unit = {
            let mut guard = lock(&self.inner.main);
            let instance = instance(guard.get()?, device)?;
            if instance.editor.is_some() || instance.opening {
                return Ok(());
            }
            instance.opening = true;
            instance.editor_failure = None;
            instance.unit.clone()
        };
        let inner = Arc::clone(&self.inner);
        let title = title.to_owned();
        let block = RcBlock::new(move |controller: *mut NSViewController| {
            finish_editor(&inner, device, &title, controller);
        });
        // SAFETY: the unit is alive for the call; its completion runs on the
        // main thread, where `finish_editor` takes the lock.
        let _: () =
            unsafe { msg_send![&unit, requestViewControllerWithCompletionHandler: &*block] };
        // Keep the block alive in case the completion is still on its way.
        let mut guard = lock(&self.inner.main);
        if let Ok(main) = guard.get()
            && let Some(instance) = main.instances.get_mut(&device)
            && instance.opening
        {
            instance.pending = Some(Box::new(block));
        }
        Ok(())
    }

    /// Closes the device's unit's own editor, if it's open.
    ///
    /// # Errors
    ///
    /// [`AuError::NoInstance`] without a unit.
    pub fn close_editor(&self, device: DeviceId) -> Result<(), AuError> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        // A view controller still on its way is dropped when it arrives:
        // `finish_editor` leaves a closed editor closed.
        instance.opening = false;
        instance.pending = None;
        instance.editor_failure = None;
        if let Some(editor) = instance.editor.take() {
            if let Some(ends) = &instance.ends {
                ends.status.observer.store(std::ptr::null_mut(), Relaxed);
            }
            // SAFETY: the unit's tree is alive; removing an observer blocks
            // until in-flight callbacks finish, and ours never blocks.
            unsafe { editor.tree.removeParameterObserver(editor.token) };
            editor.window.orderOut(None);
            // The window was made with releasedWhenClosed false, so this
            // only orders it out; our own reference drops with the editor.
            editor.window.close();
        }
        instance.editor_hidden = false;
        Ok(())
    }

    /// Shows or hides the device's unit's editor window without closing it:
    /// View ▸ Plug-In Windows and Auto-Hide Plug-In Windows hide and show
    /// it as they do a CLAP editor's. Does nothing while its editor is
    /// closed.
    ///
    /// # Errors
    ///
    /// [`AuError::NoInstance`] without a unit, and [`AuError::WrongThread`]
    /// off the main thread.
    pub fn set_editor_visible(&self, device: DeviceId, shown: bool) -> Result<(), AuError> {
        let mut guard = lock(&self.inner.main);
        let instance = instance(guard.get()?, device)?;
        let hidden = match &instance.editor {
            Some(editor) => {
                if editor.window.isVisible() != shown {
                    if shown {
                        editor.window.makeKeyAndOrderFront(None);
                    } else {
                        editor.window.orderOut(None);
                    }
                }
                !shown
            }
            None => return Ok(()),
        };
        instance.editor_hidden = hidden;
        Ok(())
    }

    /// The devices whose unit's editor window is open, hidden or shown.
    #[must_use]
    pub fn open_editors(&self) -> Vec<DeviceId> {
        let mut guard = lock(&self.inner.main);
        guard
            .get()
            .map(|main| {
                main.instances
                    .iter()
                    .filter(|(_, instance)| instance.editor.is_some())
                    .map(|(device, _)| *device)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The parameter changes the units made themselves since the last call,
    /// such as from their editors, each gesture as one change with its final
    /// value (spec: clap-hosting R6).
    #[must_use]
    pub fn take_param_changes(&self) -> Vec<ParamChange> {
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else {
            return Vec::new();
        };
        let mut changes = Vec::new();
        for (device, instance) in &mut main.instances {
            pump_editor_changes(*device, instance, &mut changes);
        }
        changes
    }

    /// Lets go of every unit whose device isn't in `keep`, as when a set
    /// closes. Their nodes must be gone first; any still running keeps its
    /// unit until its processor comes back.
    pub fn retain(&self, keep: &BTreeSet<DeviceId>) {
        let mut guard = lock(&self.inner.main);
        let Ok(main) = guard.get() else { return };
        main.missing.retain(|device, _| keep.contains(device));
        let gone: Vec<DeviceId> = main
            .instances
            .keys()
            .filter(|device| !keep.contains(device))
            .copied()
            .collect();
        for device in gone {
            if let Some(instance) = main.instances.remove(&device) {
                main.generation += 1;
                close(main, device, instance);
            }
        }
    }

    /// Lets go of every unit, as when another set opens.
    pub fn clear(&self) {
        self.retain(&BTreeSet::new());
    }

    /// Counts instances started and let go of, so the shell knows when to
    /// look for new commands.
    #[must_use]
    pub fn generation(&self) -> u64 {
        let mut guard = lock(&self.inner.main);
        guard.get().map_or(0, |main| main.generation)
    }

    /// Lends the units to an offline render (WP-4.21), as `ClapHost::lend`
    /// does for CLAP: an export or a preview, rendering on this thread.
    /// The loan's [`factory`](Loan::factory) goes into the render's
    /// registry; for each Audio Unit device the render makes a node for, it
    /// takes the unit back from the engine's node (waiting for that node's
    /// next block), deactivates it, sets it to the rendered set's parameter
    /// values, and activates it for the render's rate and block size. A
    /// device with no unit yet gets one, from its saved state. Meanwhile
    /// the engine's node passes its input through.
    ///
    /// When the loan is dropped, after the render's engine, every unit goes
    /// back to the engine with the state it had before, activated for the
    /// engine's rate and block size again; one that failed while rendering
    /// stays silent in the engine too.
    ///
    /// # Errors
    ///
    /// [`AuError::WrongThread`] off the host's thread, and
    /// [`AuError::Lent`] while another loan is out.
    pub fn lend(&self) -> Result<Loan, AuError> {
        let mut guard = lock(&self.inner.main);
        let main = guard.get()?;
        if main.lending {
            return Err(AuError::Lent);
        }
        main.lending = true;
        main.loan_problems.clear();
        Ok(Loan {
            inner: Arc::clone(&self.inner),
            _main_thread: std::marker::PhantomData,
        })
    }
}

/// The host's units, lent to an offline render ([`AuHost::lend`]).
/// Dropping it, on the main thread and after the render's engine, gives
/// every unit back to the engine.
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
    /// The factory for the render's registry: it makes each Audio Unit
    /// device's node with the device's own unit, borrowed from the engine.
    #[must_use]
    pub fn factory(&self) -> LoanFactory {
        LoanFactory {
            inner: Arc::clone(&self.inner),
        }
    }

    /// The devices whose unit the render couldn't borrow, and why: the
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
            format: PluginFormat::AudioUnit,
            ..
        } = &device.kind
        else {
            return None;
        };
        let (node, ends) = AuNode::new(config);
        let mut guard = lock(&self.inner.main);
        let problem = match guard.get() {
            Ok(main) if main.lending => borrow(main, device, *config, ends).err(),
            Ok(_) => Some("the units aren't lent to a render".to_owned()),
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

impl Drop for Instance {
    fn drop(&mut self) {
        // See `release_later`: a unit is kept a moment past its last use,
        // so CoreAudio's own event finishes before the unit is released.
        release_later(self.unit.clone());
    }
}

impl Instance {
    /// A parameter's value, as the unit reports it.
    fn unit_value(&self, param: u32) -> Option<f64> {
        let (parameter, _) = self.params.get(param as usize)?;
        // SAFETY: reading a value is a property read on the thread that
        // owns the unit, and the parameter belongs to its tree.
        Some(f64::from(unsafe { parameter.value() }))
    }
}

/// The Audio Units' factory for the engine's registry. It makes each Audio
/// Unit device's node at once; the host gives the node its unit.
pub struct AuFactory {
    inner: Arc<Inner>,
}

impl NodeFactory for AuFactory {
    fn create(&self, device: &Device, config: &NodeConfig) -> Option<Box<dyn Node>> {
        let DeviceKind::Plugin {
            format: PluginFormat::AudioUnit,
            ..
        } = &device.kind
        else {
            return None;
        };
        let (node, ends) = AuNode::new(config);
        lock(&self.inner.requests).push(Request {
            device: device.clone(),
            config: *config,
            ends,
        });
        if std::thread::current().id() == self.inner.owner {
            AuHost {
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

/// The component description a `plugin_id` names, if it names one.
fn description_of(id: &str) -> Option<AudioComponentDescription> {
    let (component_type, subtype, manufacturer) = super::description_fields(id)?;
    Some(AudioComponentDescription {
        componentType: component_type,
        componentSubType: subtype,
        componentManufacturer: manufacturer,
        componentFlags: 0,
        componentFlagsMask: 0,
    })
}

/// Describes the Audio Units the `.component` bundle at `path` declares,
/// for the scanner's child process (WP-2.8's second part): each unit is
/// loaded by its component description so a bundle that crashes or hangs is
/// caught by the scanner's timeout, not Beatlayer.
///
/// # Errors
///
/// [`AuError::Load`] if the bundle isn't one or its `AudioComponents` list
/// can't be read, or a unit it declares won't start.
pub(super) fn scan_file(path: &Path) -> Result<Vec<ScannedPlugin>, AuError> {
    let host = AuHost::new();
    let mut found = Vec::new();
    for (kind, subtype, manufacturer) in component_descriptions(path)? {
        if role_text(&kind).is_none() {
            continue;
        }
        let unit = host.load(&kind, &subtype, &manufacturer)?;
        let plugin_id = description_id(&kind, &subtype, &manufacturer).ok_or_else(|| {
            AuError::Load("its type, subtype, and manufacturer are four characters each".to_owned())
        })?;
        found.push(ScannedPlugin {
            format: PluginFormat::AudioUnit,
            path: path.to_path_buf(),
            plugin_id,
            name: unit.device_type.name,
            vendor: unit.vendor,
            version: unit.version,
            role: unit.device_type.role,
        });
    }
    Ok(found)
}

/// The component descriptions an Audio Unit bundle declares, from its
/// `Info.plist`'s `AudioComponents` list: its type, subtype, and
/// manufacturer, four characters each. Reading the list doesn't load the
/// unit's code.
fn component_descriptions(path: &Path) -> Result<Vec<(String, String, String)>, AuError> {
    let path = path.to_string_lossy().into_owned();
    let bundle = NSBundle::bundleWithPath(&NSString::from_str(&path))
        .ok_or_else(|| AuError::Load("it isn't an Audio Unit bundle".to_owned()))?;
    let Some(list) = bundle.objectForInfoDictionaryKey(&NSString::from_str("AudioComponents"))
    else {
        return Err(AuError::Load("it declares no Audio Components".to_owned()));
    };
    // SAFETY: an Audio Unit's Info.plist holds AudioComponents as an array
    // of dictionaries of strings.
    let list: &NSArray = list
        .downcast_ref()
        .ok_or_else(|| AuError::Load("its Audio Components list isn't an array".to_owned()))?;
    let field = |entry: &AnyObject, key: &str| -> Option<String> {
        let key = NSString::from_str(key);
        // SAFETY: the entry is a dictionary and the key a string; a
        // value that isn't a string is ignored.
        let value: *mut AnyObject = unsafe { msg_send![entry, valueForKey: &*key] };
        if value.is_null() {
            return None;
        }
        // SAFETY: the value is a live object from the dictionary.
        let value = unsafe { Retained::retain(value) }?;
        // SAFETY: the fields are strings.
        let string = value.downcast_ref::<NSString>()?;
        Some(string.to_string())
    };
    let mut descriptions = Vec::new();
    for index in 0..list.count() {
        let entry = list.objectAtIndex(index);
        let (Some(kind), Some(subtype), Some(manufacturer)) = (
            field(&entry, "type"),
            field(&entry, "subtype"),
            field(&entry, "manufacturer"),
        ) else {
            continue;
        };
        descriptions.push((kind, subtype, manufacturer));
    }
    Ok(descriptions)
}

/// What an automation event is: the start or end of a gesture, or a value
/// within one (or on its own).
fn event_kind(kind: AUParameterAutomationEventType) -> EventKind {
    if kind.0 == AUParameterAutomationEventType::Touch.0 {
        EventKind::Begin
    } else if kind.0 == AUParameterAutomationEventType::Release.0 {
        EventKind::End
    } else {
        EventKind::Value
    }
}

/// Completes an editor's opening, on the main thread: makes the window and
/// the parameter observer for the view controller that arrived, or records
/// why there is none. Does nothing if the editor was closed meanwhile.
fn finish_editor(
    inner: &Arc<Inner>,
    device: DeviceId,
    title: &str,
    controller: *mut NSViewController,
) {
    let mut guard = lock(&inner.main);
    let Ok(main) = guard.get() else { return };
    let Some(instance) = main.instances.get_mut(&device) else {
        return;
    };
    if !instance.opening {
        return;
    }
    instance.opening = false;
    instance.pending = None;
    // SAFETY: reading the parameter tree is a property read on the thread
    // that owns the unit.
    let Some(tree) = (unsafe { instance.unit.parameterTree() }) else {
        instance.editor_failure = Some("the unit has no parameter tree".to_owned());
        return;
    };
    if controller.is_null() {
        instance.editor_failure = Some("the unit has no editor".to_owned());
        return;
    }
    // SAFETY: the completion runs on the main thread with a view controller
    // the unit still owns; take our own reference.
    let Some(controller) = (unsafe { Retained::retain(controller) }) else {
        instance.editor_failure = Some("the unit has no editor".to_owned());
        return;
    };
    let window = make_window(&controller, title);
    let events: Arc<Mutex<Vec<RawEvent>>> = Arc::new(Mutex::new(Vec::with_capacity(EDITOR_EVENTS)));
    let sink = Arc::clone(&events);
    let observer = RcBlock::new(
        move |count: isize, list: NonNull<AUParameterAutomationEvent>| {
            // Never block or allocate here: this may run on the audio
            // thread. Events past the buffer's room are dropped.
            let Ok(mut out) = sink.try_lock() else { return };
            let count = usize::try_from(count).unwrap_or(0);
            // SAFETY: the unit lends the callback `count` events for the
            // call; it holds at least that many.
            let events = unsafe { std::slice::from_raw_parts(list.as_ptr(), count) };
            for event in events {
                if out.len() >= EDITOR_EVENTS {
                    break;
                }
                out.push(RawEvent {
                    address: event.address,
                    value: f64::from(event.value),
                    kind: event_kind(event.eventType),
                });
            }
        },
    );
    // SAFETY: the tree belongs to the unit, alive on this thread; the block
    // is kept alive by the editor below.
    let token =
        unsafe { tree.tokenByAddingParameterAutomationObserver(RcBlock::as_ptr(&observer)) };
    instance.editor = Some(Editor {
        window,
        tree,
        token,
        events,
        _observer: observer,
        gestures: Gestures::default(),
    });
    instance.editor_hidden = false;
    instance.editor_failure = None;
    // The host's own parameter changes must not come back as the unit's:
    // the node passes this token to setValue:originator:.
    if let Some(ends) = &instance.ends {
        ends.status.observer.store(token, Relaxed);
    }
}

/// A window of its own holding `controller`, titled `title`, shown and made
/// key; the unit's own controls are in it.
fn make_window(controller: &NSViewController, title: &str) -> Retained<NSWindow> {
    let mut size = controller.view().frame().size;
    if size.width <= 0.0 || size.height <= 0.0 {
        size = objc2_foundation::NSSize {
            width: 480.0,
            height: 360.0,
        };
    }
    let rect = objc2_foundation::NSRect {
        origin: objc2_foundation::NSPoint { x: 0.0, y: 0.0 },
        size,
    };
    let style =
        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
    let mtm = MainThreadMarker::new().expect("a unit's editor opens on the main thread");
    // SAFETY: the window is made on the main thread, as AppKit requires.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the window is released when we let our own reference go, not
    // when it's closed, so `close_editor` may close and then drop it.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str(title));
    window.setContentViewController(Some(controller));
    window.center();
    window.makeKeyAndOrderFront(None);
    window
}

/// Turns the events an editor's observer saw into the set's changes, one
/// per gesture with its final value (spec: clap-hosting R6).
fn pump_editor_changes(device: DeviceId, instance: &mut Instance, changes: &mut Vec<ParamChange>) {
    let Some(editor) = &mut instance.editor else {
        return;
    };
    let events: Vec<RawEvent> = match editor.events.try_lock() {
        Ok(mut events) => std::mem::take(&mut *events),
        Err(_) => return,
    };
    if events.is_empty() {
        return;
    }
    // Every parameter's address, to find the position a change names.
    let positions: BTreeMap<u64, u32> = instance
        .params
        .iter()
        .enumerate()
        .filter_map(|(index, (parameter, _))| {
            let param = u32::try_from(index).ok()?;
            // SAFETY: reading a parameter's address is a property read on
            // the thread that owns the unit.
            Some((unsafe { parameter.address() }, param))
        })
        .collect();
    for event in events {
        let Some((address, value)) = editor.gestures.feed(event) else {
            continue;
        };
        let Some(param) = positions.get(&address).copied() else {
            continue;
        };
        emit_change(device, param, value, changes);
    }
}

/// Records one change, replacing an earlier one for the same parameter so a
/// gesture ends as a single change.
fn emit_change(device: DeviceId, param: u32, value: f64, changes: &mut Vec<ParamChange>) {
    let param = ParamId(param);
    changes.retain(|change| !(change.device == device && change.param == param));
    changes.push(ParamChange {
        device,
        param,
        value,
    });
}

/// A unit's device kind.
fn kind(id: &str) -> DeviceKind {
    DeviceKind::Plugin {
        format: PluginFormat::AudioUnit,
        plugin_id: id.to_owned(),
    }
}

/// Where a unit goes in a chain, from its component type: an instrument, an
/// audio effect, or a MIDI effect. `None` for anything else the system
/// registers (format converters, codecs, and the like).
fn role(component_type: u32) -> Option<DeviceRole> {
    role_text(&super::field(component_type))
}

/// Where a unit goes in a chain, from its component type's four characters.
fn role_text(component_type: &str) -> Option<DeviceRole> {
    match component_type {
        "aufx" => Some(DeviceRole::AudioEffect),
        "aumu" | "augn" => Some(DeviceRole::Instrument),
        "aumi" => Some(DeviceRole::MidiEffect),
        _ => None,
    }
}

/// A version as the system reports it: 0xMMMMmmDD as `M.m.D`.
fn version_text(version: u32) -> String {
    format!(
        "{}.{}.{}",
        version >> 16,
        (version >> 8) & 0xff,
        version & 0xff
    )
}

/// Makes the unit with `description`, in this process, as hosting an
/// in-process plug-in means (the architecture, "Plug-in hosting"). The
/// unit's code runs as it starts, so only units a person chose are made.
fn instantiate(description: &AudioComponentDescription) -> Result<Retained<AUAudioUnit>, String> {
    // SAFETY: the description is one Beatlayer built from an id; the
    // initializer either makes the unit or says why. It runs on the
    // thread that made the host.
    unsafe {
        AUAudioUnit::initWithComponentDescription_options_error(
            AUAudioUnit::alloc(),
            *description,
            AudioComponentInstantiationOptions::LoadInProcess,
        )
    }
    .map_err(|error| error.localizedDescription().to_string())
}

/// What a unit's parameter tree gives: the descriptors for the parameters a
/// person can set, and every parameter of the tree in order with the value
/// it had when the unit was made (its default).
type Described = (
    Vec<ParamDescriptor>,
    Vec<(Retained<AUParameter>, f64)>,
    Vec<ParamId>,
);

/// The unit's parameters: every one the tree holds, in order, as
/// descriptors for the ones a person can set, and every parameter itself so
/// a change can be set on it by its [`ParamId`].
fn describe(unit: &AUAudioUnit) -> Described {
    let mut params = Vec::new();
    let mut descriptors = Vec::new();
    let mut visible = Vec::new();
    // SAFETY: reading the parameter tree is a property read on the thread
    // that made the unit.
    let Some(tree) = (unsafe { unit.parameterTree() }) else {
        return (descriptors, params, visible);
    };
    // SAFETY: the tree belongs to the unit, alive for the borrow.
    let all = unsafe { tree.allParameters() };
    for (index, parameter) in all.iter().enumerate() {
        // SAFETY: reading a value is a property read on the thread that
        // made the unit, before anything sets it: its value now is its
        // default.
        let default = f64::from(unsafe { parameter.value() });
        params.push((parameter.clone(), default));
        let Ok(id) = u32::try_from(index) else {
            continue;
        };
        // SAFETY: reading a parameter's properties is a host read on the
        // thread that owns the unit.
        let flags = unsafe { parameter.flags() };
        if !flags.contains(AudioUnitParameterOptions::Flag_IsReadable)
            || !flags.contains(AudioUnitParameterOptions::Flag_IsWritable)
        {
            continue;
        }
        // SAFETY: reading a parameter's range and value is a host read on
        // the thread that owns the unit.
        let (min, max, value) = unsafe {
            (
                f64::from(parameter.minValue()),
                f64::from(parameter.maxValue()),
                f64::from(parameter.value()),
            )
        };
        if !(min.is_finite() && max.is_finite() && min < max) {
            continue;
        }
        // SAFETY: as above: its own thread, its own parameter.
        let name = unsafe { parameter.displayName() }.to_string();
        let name = name.trim();
        let name = if name.is_empty() {
            format!("Parameter {}", index + 1)
        } else {
            name.to_owned()
        };
        let default = if value.is_finite() {
            value.clamp(min, max)
        } else {
            min
        };
        // SAFETY: as above: its own thread, its own parameter.
        let unit_kind = unsafe { parameter.unit() };
        #[expect(clippy::cast_possible_truncation, reason = "whole numbers in range")]
        let kind = if unit_kind == AudioUnitParameterUnit::Indexed
            && min.fract() == 0.0
            && max.fract() == 0.0
            && min.abs() < 1e15
            && max.abs() < 1e15
        {
            ParamKind::Integer {
                min: min as i64,
                max: max as i64,
                unit: ParamUnit::None,
            }
        } else {
            ParamKind::Continuous {
                min,
                max,
                unit: unit_of(unit_kind),
                taper: taper_of(flags, min),
                decimals: None,
            }
        };
        let default = if matches!(kind, ParamKind::Integer { .. }) {
            default.round()
        } else {
            default
        };
        let descriptor = ParamDescriptor {
            id: ParamId(id),
            name,
            description: String::new(),
            kind,
            default,
            automatable: true,
        };
        if descriptor.check().is_ok() {
            descriptors.push(descriptor);
            visible.push(ParamId(id));
        }
    }
    (descriptors, params, visible)
}

/// What a parameter measures, where Beatlayer has a unit for it.
fn unit_of(unit: AudioUnitParameterUnit) -> ParamUnit {
    if unit == AudioUnitParameterUnit::Hertz {
        ParamUnit::Hertz
    } else if unit == AudioUnitParameterUnit::Decibels {
        ParamUnit::Decibels
    } else if unit == AudioUnitParameterUnit::Percent {
        ParamUnit::Percent
    } else if unit == AudioUnitParameterUnit::Seconds {
        ParamUnit::Seconds
    } else if unit == AudioUnitParameterUnit::Milliseconds {
        ParamUnit::Milliseconds
    } else if unit == AudioUnitParameterUnit::Cents || unit == AudioUnitParameterUnit::AbsoluteCents
    {
        ParamUnit::Cents
    } else if unit == AudioUnitParameterUnit::RelativeSemiTones {
        ParamUnit::Semitones
    } else if unit == AudioUnitParameterUnit::Ratio {
        ParamUnit::Ratio
    } else {
        ParamUnit::None
    }
}

/// How a continuous parameter's control travel maps onto its range: evenly
/// in the logarithm when the unit says so and its range allows it.
fn taper_of(flags: AudioUnitParameterOptions, min: f64) -> Taper {
    if flags.contains(AudioUnitParameterOptions::Flag_DisplayLogarithmic) && min > 0.0 {
        Taper::Logarithmic
    } else {
        Taper::Linear
    }
}

/// Sees any object as `AnyObject`: the same pointer, for the Foundation
/// calls that take one.
fn any_object<T: objc2::Message + ?Sized>(object: &T) -> &AnyObject {
    // SAFETY: every Objective-C object is an `AnyObject` underneath, with
    // the same layout; the borrow outlives the returned reference.
    unsafe { &*std::ptr::from_ref(object).cast::<AnyObject>() }
}

/// The unit's state as a binary property list, if it will save it.
fn state_of(unit: &AUAudioUnit) -> Option<Vec<u8>> {
    // SAFETY: reading the state is a property read on the main thread,
    // where the unit was made.
    let state = unsafe { unit.fullStateForDocument() }?;
    plist(&state)
}

/// Writes a unit's state as a binary property list.
fn plist(state: &NSDictionary<NSString, AnyObject>) -> Option<Vec<u8>> {
    // SAFETY: the dictionary comes from the unit; writing it as a property
    // list is a Foundation call on the main thread.
    let data = unsafe {
        NSPropertyListSerialization::dataWithPropertyList_format_options_error(
            any_object(state),
            NSPropertyListFormat::BinaryFormat_v1_0,
            0,
        )
    }
    .ok()?;
    let length = data.length();
    let mut bytes = vec![0u8; length];
    if !bytes.is_empty() {
        // SAFETY: the buffer has room for the data's own length, and the
        // NSData copies its bytes into it.
        unsafe {
            data.getBytes_length(NonNull::from(bytes.as_mut_slice()).cast(), length);
        }
    }
    Some(bytes)
}

/// Gives the unit a state it saved before. The unit keeps the state it had
/// if it refuses.
fn load_state(unit: &AUAudioUnit, data: &[u8]) -> Result<(), String> {
    // SAFETY: the bytes are the set's copy of a property list; the NSData
    // copies them.
    let data =
        unsafe { NSData::initWithBytes_length(NSData::alloc(), data.as_ptr().cast(), data.len()) };
    let refused = || "its saved state isn't a property list".to_owned();
    // SAFETY: parsing the set's own bytes is a Foundation call; a format
    // pointer of null asks for no format back.
    let plist = unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &data,
            NSPropertyListMutabilityOptions(0),
            std::ptr::null_mut(),
        )
    }
    .map_err(|_| refused())?;
    let Some(state) = plist.downcast_ref::<NSDictionary>() else {
        return Err(refused());
    };
    // SAFETY: setting the state is a property write on the thread that
    // made the unit, before it's activated (or between render calls); the
    // dictionary is the set's own state.
    unsafe {
        let _: () = msg_send![unit, setFullStateForDocument: state];
    }
    Ok(())
}

/// Sets the unit's parameters to the device's values.
fn set_values(instance: &Instance, device: &Device) {
    for (param, value) in &device.params {
        let Some((parameter, _)) = instance.params.get(param.0 as usize) else {
            continue;
        };
        // SAFETY: setting a value is a host call on the thread that owns
        // the unit, outside a render call.
        unsafe { parameter.setValue(*value as f32) };
    }
}

/// Gives a new node its unit, making the unit if the device has none.
fn serve(main: &mut Main, request: Request) {
    let Request {
        device,
        config,
        ends,
    } = request;
    if !ensure_instance(main, &device) {
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

/// Lends the device's unit to an offline render's node (WP-4.21): takes it
/// back from the engine's node, and activates it for the render's settings
/// with the rendered set's parameter values. A unit that can't be made or
/// won't activate leaves the node passing its input through; one that
/// failed while processing leaves it silent.
fn borrow(
    main: &mut Main,
    device: &Device,
    config: NodeConfig,
    ends: HostEnds,
) -> Result<(), String> {
    if !ensure_instance(main, device) {
        // Spec: au-hosting R12, as in the engine.
        return Ok(());
    }
    let Some(instance) = main.instances.get_mut(&device.id) else {
        return Ok(());
    };
    if instance.lent.is_some() {
        return Err("its unit is lent to this render already".to_owned());
    }
    // Spec: offline-plugin-renders R4. Its state, to give back.
    let saved = state_of(&instance.unit);
    // Spec: offline-plugin-renders R2. The engine's node hands it back at
    // its next block.
    if instance.out {
        for ends in instance.ends.iter().chain(&instance.retired) {
            ends.status.hand_back.store(true, Relaxed);
        }
        let until = Instant::now() + HAND_BACK_WAIT;
        loop {
            take_back(instance);
            if !instance.out {
                break;
            }
            if Instant::now() >= until {
                return Err(
                    "the engine playing the set didn't hand its unit back in time".to_owned(),
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
    // render's rate and block size. An open editor's observer is the
    // originator, so it doesn't report the values as the unit's own.
    let observer = instance.ends.as_ref().map_or(std::ptr::null_mut(), |ends| {
        ends.status.observer.load(Relaxed)
    });
    lent.ends.status.observer.store(observer, Relaxed);
    set_values(instance, device);
    // Spec: au-hosting R12. One that won't activate for the render leaves
    // its node passing its input through.
    if let Ok((processor, latency)) = activate(
        &instance.unit,
        &config,
        &instance.params,
        &instance.descriptors,
    ) {
        lent.ends.status.latency.store(latency, Relaxed);
        if lent.ends.to_node.push(processor).is_ok() {
            instance.out = true;
            instance.active_for = Some(config);
        }
    }
    instance.lent = Some(lent);
    Ok(())
}

/// Ends a loan: every lent unit goes back to the engine's node with the
/// state it had, or stays silent if it failed while rendering.
fn end_loan(main: &mut Main) {
    if !main.lending {
        return;
    }
    main.lending = false;
    main.loan_problems.clear();
    for instance in main.instances.values_mut() {
        let Some(lent) = &instance.lent else { continue };
        lent.ends.status.hand_back.store(true, Relaxed);
        // The render's engine is gone, and its node gave the processor
        // back as it went.
        take_back(instance);
        let Some(lent) = instance.lent.take() else {
            continue;
        };
        let failed = lent.ends.status.failed.load(Relaxed);
        if instance.out {
            // A render's node still holds it: it's deactivated once that
            // node gives it back.
            instance.retired.push(lent.ends);
        }
        // Spec: offline-plugin-renders R4. The state it had; what the
        // render did to its parameters is dropped, the editor's too.
        if let Some(saved) = &lent.saved {
            let _ = load_state(&instance.unit, saved);
        }
        if let Some(editor) = &instance.editor
            && let Ok(mut events) = editor.events.try_lock()
        {
            events.clear();
        }
        instance.refused = lent.refused;
        if failed {
            // Spec: offline-plugin-renders R6, au-hosting R12.
            instance.silenced = true;
            instance.failure = Some(
                "it failed while processing in an offline render; the device is silent".to_owned(),
            );
            if let Some(ends) = &instance.ends {
                ends.status.silent.store(true, Relaxed);
            }
        }
        pump(instance);
    }
}

/// Makes the device's unit if it has none. Returns whether it has one; one
/// that can't be made is recorded as missing.
fn ensure_instance(main: &mut Main, device: &Device) -> bool {
    let DeviceKind::Plugin {
        format: PluginFormat::AudioUnit,
        plugin_id,
    } = &device.kind
    else {
        return false;
    };
    // A unit left from another set under the same ID is let go of.
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
    let made = description_of(plugin_id)
        .ok_or_else(|| AuError::Load(format!("{plugin_id} isn't a component description")))
        .and_then(|description| instantiate(&description).map_err(AuError::Load));
    let unit = match made {
        Ok(unit) => unit,
        Err(error) => {
            // Spec: au-hosting R12. The device stays, passing its input
            // through, and keeps its saved state for when the unit is
            // found.
            main.missing
                .insert(device.id, (plugin_id.clone(), error.to_string()));
            return false;
        }
    };
    main.missing.remove(&device.id);
    let (descriptors, params, visible) = describe(&unit);
    let mut instance = Instance {
        plugin_id: plugin_id.clone(),
        unit,
        params,
        visible,
        descriptors,
        ends: None,
        retired: Vec::new(),
        config: None,
        out: false,
        refused: None,
        failure: None,
        silenced: false,
        active_for: None,
        latency: 0,
        editor: None,
        editor_hidden: false,
        opening: false,
        pending: None,
        editor_failure: None,
        lent: None,
    };
    // Spec: au-hosting R9, R11. The saved state first, then the set's
    // parameter values over it.
    if let Some(saved) = device.plugin.as_ref().filter(|p| !p.data.is_empty())
        && let Err(error) = load_state(&instance.unit, &saved.data)
    {
        instance.failure = Some(format!("{error}; it starts from its defaults"));
    }
    set_values(&instance, device);
    main.instances.insert(device.id, instance);
    main.generation += 1;
    true
}

/// One unit's main-thread work. Returns whether anything changed.
fn pump(instance: &mut Instance) -> bool {
    let mut changed = take_back(instance);
    instance
        .retired
        .retain(|ends| !(ends.from_node.is_abandoned() && ends.from_node.is_empty()));
    if let Some(ends) = &instance.ends
        && ends.status.failed.load(Relaxed)
        && !instance.silenced
    {
        // Spec: au-hosting R12. A unit that fails while processing is never
        // called again; its device is silent.
        instance.silenced = true;
        instance.failure = Some("it failed while processing; the device is silent".to_owned());
        changed = true;
    }
    // Activate it for its node.
    if !instance.out
        && instance.refused.is_none()
        && !instance.silenced
        && instance.lent.is_none()
        && let (Some(ends), Some(config)) = (&mut instance.ends, instance.config)
        && !ends.to_node.is_abandoned()
    {
        changed = true;
        match activate(
            &instance.unit,
            &config,
            &instance.params,
            &instance.descriptors,
        ) {
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
    changed
}

/// Takes back the processors nodes handed back and deactivates the unit.
fn take_back(instance: &mut Instance) -> bool {
    let mut returned = false;
    let lent = instance.lent.iter_mut().map(|lent| &mut lent.ends);
    for ends in instance
        .ends
        .iter_mut()
        .chain(instance.retired.iter_mut())
        .chain(lent)
    {
        while ends.from_node.pop().is_ok() {
            returned = true;
        }
    }
    if returned {
        deactivate(&instance.unit);
        instance.out = false;
    }
    // A node dropped with the processor still in its queue drops it there;
    // the unit's render resources are gone with it.
    if instance.out && !allocated(&instance.unit) {
        instance.out = false;
    }
    if !instance.out {
        instance.active_for = None;
    }
    returned
}

/// Lets go of a unit: asks its nodes for their processors, keeping it until
/// they're back.
fn close(main: &mut Main, device: DeviceId, mut instance: Instance) {
    for ends in instance.ends.iter().chain(&instance.retired) {
        ends.status.hand_back.store(true, Relaxed);
    }
    instance.refused = Some("let go of".to_owned());
    pump(&mut instance);
    if instance.out {
        main.closing.push((device, instance));
    }
}

fn instance(main: &mut Main, device: DeviceId) -> Result<&mut Instance, AuError> {
    main.instances
        .get_mut(&device)
        .ok_or(AuError::NoInstance(device))
}

/// Stops a unit rendering: its render resources are deallocated, and the
/// audio thread may not call it again until it's activated once more.
fn deactivate(unit: &AUAudioUnit) {
    // SAFETY: deallocating render resources stops the unit rendering; it is
    // called after the processor's render call returned, or before the unit
    // ever rendered.
    unsafe {
        if unit.renderResourcesAllocated() {
            unit.deallocateRenderResources();
        }
    }
}

/// Whether a unit's render resources are allocated: it's active.
fn allocated(unit: &AUAudioUnit) -> bool {
    // SAFETY: reading whether resources are allocated is a property read on
    // the unit's own thread.
    unsafe { unit.renderResourcesAllocated() }
}

/// Activates a unit for a node's settings: its input and output buses take
/// the engine's rate and the channels they offer (up to [`MAX_CHANNELS`]),
/// the maximum block size is the engine's, and its render resources are
/// allocated. Returns its processor and its latency, or why it refused.
fn activate(
    unit: &Retained<AUAudioUnit>,
    config: &NodeConfig,
    params: &[(Retained<AUParameter>, f64)],
    descriptors: &[ParamDescriptor],
) -> Result<(Processor, u32), String> {
    deactivate(unit);
    let rate = f64::from(config.sample_rate.hz());
    let input_channels = configure_bus(unit, true, rate)?;
    let output_channels = configure_bus(unit, false, rate)?;
    let max_frames = u32::try_from(config.max_frames.max(1)).unwrap_or(u32::MAX);
    // SAFETY: the maximum frame count is sent as the `NSUInteger` the
    // runtime's method takes (the header's `AUAudioFrameCount` is the v2
    // name); the blocks are cached before activation, as Apple's
    // documentation asks.
    let (render, midi) = unsafe {
        let _: () = msg_send![unit, setMaximumFramesToRender: max_frames as usize];
        (unit.renderBlock(), unit.scheduleMIDIEventBlock())
    };
    if render.is_null() {
        return Err("it gives no render block".to_owned());
    }
    // SAFETY: the unit is inactive (just deallocated, or never activated);
    // activation either succeeds or says why.
    if let Err(error) = unsafe { unit.allocateRenderResourcesAndReturnError() } {
        return Err(format!(
            "it refused to activate: {}",
            error.localizedDescription()
        ));
    }
    // SAFETY: reading the latency is a property read on the unit's own
    // thread, after activation, when it's known.
    let seconds = unsafe { unit.latency() };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=u32::MAX"
    )]
    let latency = (seconds * rate).round().clamp(0.0, f64::from(u32::MAX)) as u32;
    // Each control starts where the unit is: the set's values and its saved
    // state are on it before it's activated.
    let drive = Driver::new(descriptors.iter().map(|descriptor| {
        let value =
            params
                .get(descriptor.id.0 as usize)
                .map_or(descriptor.default, |(parameter, _)| {
                    // SAFETY: reading a value is a property read on the thread that
                    // made the unit.
                    f64::from(unsafe { parameter.value() })
                });
        (descriptor.clone(), false, value)
    }));
    let processor = Processor::new(
        unit.clone(),
        config,
        params.to_vec(),
        drive,
        render,
        midi,
        (input_channels, output_channels),
    );
    Ok((processor, latency))
}

/// Sets up one of a unit's main buses: the engine's rate, the channel count
/// the bus offers, enabled. Returns the channel count, 0 for a bus the unit
/// doesn't have (an instrument's input).
fn configure_bus(unit: &AUAudioUnit, is_input: bool, rate: f64) -> Result<usize, String> {
    // SAFETY: reading the unit's busses is a property read on the thread
    // that made it.
    let busses = unsafe {
        if is_input {
            unit.inputBusses()
        } else {
            unit.outputBusses()
        }
    };
    // SAFETY: the array belongs to the unit, alive for the borrow.
    if unsafe { busses.count() } == 0 {
        return Ok(0);
    }
    // SAFETY: the index is inside the array's count.
    let bus = unsafe { busses.objectAtIndexedSubscript(0) };
    let channels = bus_channels(&bus)?;
    let channels = channels.min(u32::try_from(MAX_CHANNELS).unwrap_or(u32::MAX));
    if channels == 0 {
        return Err("its bus takes no channels".to_owned());
    }
    // SAFETY: making a standard deinterleaved float format is a Foundation
    // call; `None` only when the channel count is out of range.
    let wanted = unsafe {
        AVAudioFormat::initStandardFormatWithSampleRate_channels(
            AVAudioFormat::alloc(),
            rate,
            channels,
        )
    };
    let Some(wanted) = wanted else {
        return Err(format!(
            "it won't take {channels} channels at {} Hz",
            rate.round()
        ));
    };
    let mut error: *mut NSError = std::ptr::null_mut();
    // SAFETY: the bus takes a format with its own channel count; the
    // out-parameter is a live NSError pointer, null until the call sets it.
    let ok: bool = unsafe { msg_send![&*bus, setFormat: &*wanted, error: &mut error] };
    if !ok {
        let why = if error.is_null() {
            "an unknown format".to_owned()
        } else {
            // SAFETY: the call set the pointer to a live NSError.
            unsafe { (*error).localizedDescription() }.to_string()
        };
        return Err(format!("it refused the engine's format: {why}"));
    }
    // SAFETY: enabling a bus is a property write on the unit's own thread.
    unsafe { bus.setEnabled(true) };
    Ok(channels as usize)
}

/// The channel count a bus's current format takes.
fn bus_channels(bus: &AUAudioUnitBus) -> Result<u32, String> {
    // `format` has no binding in the version this crate pins, so it's sent
    // by selector.
    // SAFETY: the selector `format` returns an AVAudioFormat for a bus that
    // has one, on the bus's own thread.
    let format: Retained<AVAudioFormat> = unsafe { msg_send![bus, format] };
    // SAFETY: reading the format's channel count is a property read.
    Ok(unsafe { format.channelCount() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_units_role_comes_from_its_type() {
        assert_eq!(role(0x6175_6678), Some(DeviceRole::AudioEffect));
        assert_eq!(role(0x6175_6d75), Some(DeviceRole::Instrument));
        assert_eq!(role(0x6175_676e), Some(DeviceRole::Instrument));
        assert_eq!(role(0x6175_6d69), Some(DeviceRole::MidiEffect));
        assert_eq!(role(0x6175_6663), None);
        assert_eq!(role(0), None);
    }

    #[test]
    fn a_version_reads_as_major_minor_dot() {
        assert_eq!(version_text(0x0001_0203), "1.2.3");
        assert_eq!(version_text(0x0000_0000), "0.0.0");
        assert_eq!(version_text(0xFFFF_FFFF), "65535.255.255");
    }

    #[test]
    fn a_description_no_unit_has_is_refused() {
        let host = AuHost::new();
        let refused = host.load("aufx", "zzzz", "appl");
        assert!(matches!(refused, Err(AuError::Load(_))), "{refused:?}");
        let refused = host.load("aufc", "conv", "appl");
        assert!(matches!(refused, Err(AuError::Load(_))), "{refused:?}");
        let refused = host.load("auf", "dely", "appl");
        assert!(matches!(refused, Err(AuError::Load(_))), "{refused:?}");
    }

    /// Spec: clap-hosting R6. A gesture makes one change, with the value it
    /// ended on; a value outside a gesture is a change of its own.
    #[test]
    fn a_gesture_makes_one_change_with_its_final_value() {
        let event = |kind, value| RawEvent {
            address: 7,
            value,
            kind,
        };
        let mut gestures = Gestures::default();
        assert_eq!(gestures.feed(event(EventKind::Begin, 1.0)), None);
        assert_eq!(gestures.feed(event(EventKind::Value, 2.0)), None);
        assert_eq!(gestures.feed(event(EventKind::Value, 3.0)), None);
        assert_eq!(
            gestures.feed(event(EventKind::End, 3.0)),
            Some((7, 3.0)),
            "one change, on the value it ended with"
        );
        assert_eq!(
            gestures.feed(event(EventKind::Value, 9.0)),
            Some((7, 9.0)),
            "outside a gesture, a value is a change of its own"
        );
        // A gesture that ends without a value changes nothing.
        assert_eq!(gestures.feed(event(EventKind::Begin, 0.0)), None);
        assert_eq!(gestures.feed(event(EventKind::End, 0.0)), None);
    }
}
