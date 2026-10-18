//! The host objects a VST3 plug-in calls: the host application it's given
//! at initialization, and the component handler its controller reports
//! parameter gestures and restart asks to.
//!
//! Nothing here allocates, locks, or blocks: a plug-in's editor, or its
//! audio thread, may call these at any time, and the main thread takes what
//! they record in [`Vst3Host::idle`](super::Vst3Host::idle) (the real-time
//! rules).

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

use vst3::Class;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;

/// The host, as `IHostApplication`: its name, and nothing it can create.
pub(crate) struct HostApplication;

impl Class for HostApplication {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the caller passes a valid name buffer, as the interface
        // says it does.
        let buffer = unsafe { &mut *name };
        buffer.fill(0);
        for (dst, src) in buffer.iter_mut().zip("Beatlayer".encode_utf16()) {
            *dst = src;
        }
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        _cid: *mut TUID,
        _iid: *mut TUID,
        _obj: *mut *mut c_void,
    ) -> tresult {
        kNotImplemented
    }
}

/// The parameter a gesture slot holds no value for.
const FREE: u32 = u32::MAX;
/// How many parameters can have a gesture in flight at once; a gesture for
/// one past that is dropped, as a full event list's is.
const MAX_GESTURES: usize = 64;

/// One parameter's gesture: a begin, the performed values, and an end
/// (spec: clap-hosting R6, which the VST3 spec mirrors). A change outside a
/// gesture finishes at once.
struct GestureSlot {
    /// The parameter this slot holds, [`FREE`] when it holds none.
    id: AtomicU32,
    /// The last performed value, as `f64` bits, normalized.
    value: AtomicU64,
    /// A gesture is under way.
    held: AtomicBool,
    /// A value arrived since the gesture began, or since it was taken.
    fresh: AtomicBool,
    /// A finished change waits to be taken.
    ready: AtomicBool,
}

impl Default for GestureSlot {
    fn default() -> Self {
        Self {
            id: AtomicU32::new(FREE),
            value: AtomicU64::new(0),
            held: AtomicBool::new(false),
            fresh: AtomicBool::new(false),
            ready: AtomicBool::new(false),
        }
    }
}

/// What a plug-in's controller reports its own changes to: a slot per
/// parameter with a gesture in flight, recycled as each one is taken.
pub(crate) struct Gestures {
    slots: [GestureSlot; MAX_GESTURES],
}

impl Default for Gestures {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| GestureSlot::default()),
        }
    }
}

impl Gestures {
    /// The slot holding `id`, taking a free one if there is none.
    fn slot(&self, id: u32) -> Option<&GestureSlot> {
        let mut free = None;
        for slot in &self.slots {
            if slot.id.load(Relaxed) == id {
                return Some(slot);
            }
            if free.is_none() && slot.id.load(Relaxed) == FREE {
                free = Some(slot);
            }
        }
        let slot = free?;
        slot.id
            .compare_exchange(FREE, id, Relaxed, Relaxed)
            .is_ok()
            .then_some(slot)
    }

    /// A gesture began.
    pub(crate) fn begin(&self, id: u32) {
        if let Some(slot) = self.slot(id) {
            slot.held.store(true, Relaxed);
        }
    }

    /// A value inside the gesture, or one outside it, which finishes at
    /// once.
    pub(crate) fn perform(&self, id: u32, value: f64) {
        let Some(slot) = self.slot(id) else {
            return;
        };
        slot.value.store(value.to_bits(), Relaxed);
        slot.fresh.store(true, Relaxed);
        if !slot.held.load(Relaxed) {
            slot.ready.store(true, Relaxed);
        }
    }

    /// A gesture ended: its value, the last one performed, waits to be
    /// taken.
    pub(crate) fn end(&self, id: u32) {
        let Some(slot) = self.slot(id) else {
            return;
        };
        slot.held.store(false, Relaxed);
        if slot.fresh.swap(false, Relaxed) {
            slot.ready.store(true, Relaxed);
        }
    }

    /// Takes the finished changes: each gesture once, with its last value.
    /// Each slot is freed for the next gesture.
    pub(crate) fn take(&self, mut emit: impl FnMut(u32, f64)) {
        for slot in &self.slots {
            if !slot.ready.swap(false, Relaxed) {
                continue;
            }
            let id = slot.id.swap(FREE, Relaxed);
            if id == FREE {
                continue;
            }
            emit(id, f64::from_bits(slot.value.load(Relaxed)));
        }
    }
}

/// The component handler: the changes a plug-in's controller makes itself,
/// such as from its own editor, and its ask for a restart.
pub(crate) struct ComponentHandler {
    gestures: Arc<Gestures>,
    restart: Arc<AtomicU32>,
}

impl ComponentHandler {
    pub(crate) fn new(gestures: Arc<Gestures>, restart: Arc<AtomicU32>) -> Self {
        Self { gestures, restart }
    }
}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, id: ParamID) -> tresult {
        self.gestures.begin(id);
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value_normalized: ParamValue) -> tresult {
        self.gestures.perform(id, value_normalized);
        kResultOk
    }

    unsafe fn endEdit(&self, id: ParamID) -> tresult {
        self.gestures.end(id);
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        // A restart ask from any thread: the flags wait for the main
        // thread, which deactivates and activates the plug-in again.
        self.restart
            .fetch_or(u32::from_ne_bytes(flags.to_ne_bytes()), Relaxed);
        kResultOk
    }
}
