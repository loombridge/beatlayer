//! A VST3 plug-in's node: what runs its component on the audio thread.
//!
//! The node is made on the compiler thread without the plug-in: the main
//! thread activates the component (`Vst3Host::idle`) and hands the active
//! component over through a one-slot queue, and the node hands it back the
//! same way when the host asks (to deactivate it) or when the node is
//! dropped. Until one arrives the node passes its input through; after the
//! plug-in fails a call it outputs silence. Every buffer, parameter queue
//! and event list is made with the node, so nothing on the audio thread
//! allocates, locks, or blocks (the real-time rules); the plug-in's own
//! code is its own.

use std::cell::{Cell, UnsafeCell};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

use bl_core::midi::MidiMessage;
use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::mpe_layout::{MpeLayout, PlainNotes};
use bl_engine::{
    AutomationList, Block, Node, NodeData, NodeEvent, NoteEvent, NoteExpression, NoteKind,
    ParamValue, PluginMpeData, PluginTuningData, TunedNotes,
};
use bl_model::{ParamId, PluginMpe};
use rtrb::{Consumer, Producer, RingBuffer};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComRef, ComWrapper};

use super::ParamMap;
use crate::drive::Driver;

/// The most channels the node feeds across a plug-in's buses; `activate`
/// leaves the buses past it inactive.
pub(crate) const MAX_CHANNELS: usize = 8;
/// The most buses a plug-in's arrangement is taken from each way.
pub(crate) const MAX_BUSES: usize = 8;
/// How many parameters one block's changes hold.
const MAX_QUEUES: usize = 64;
/// How many points one parameter's changes hold in a block.
const MAX_POINTS: usize = 64;
/// How many events a block's lists hold each way.
const MAX_EVENTS: usize = 1024;
/// How many parameter changes made while the plug-in isn't running are
/// kept for it; past that they're dropped and counted.
const PENDING_CAPACITY: usize = 512;
/// How many values the node reports to the main thread at a time: what it
/// applied to its component, so the controller's are the device's.
const APPLIED_CAPACITY: usize = 256;

/// The semitones a note's bend of 1 is (the MPE spec, R6).
const BEND_SEMITONES: f64 = 48.0;

/// The MIDI controller numbers VST3 gives a channel's pressure and pitch
/// bend beside the 128 control changes, and how many there are
/// (`ControllerNumbers`).
const AFTER_TOUCH: usize = 128;
const PITCH_BEND: usize = 129;
pub(crate) const CONTROLLERS: usize = 130;

/// The note expression types a plug-in's controller takes (the VST3 hosting
/// spec, R20): whether it implements `INoteExpressionController` at all, and
/// the types its `INoteExpressionPhysicalUIMapping` gives a key's Y movement
/// and its pressure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NoteExpressions {
    pub(crate) controller: bool,
    pub(crate) slide: Option<u32>,
    pub(crate) pressure: Option<u32>,
}

/// How many notes the plug-in sent that the node remembers the key of, so
/// a note expression it sends names it (R21).
const SENT_NOTES: usize = 128;

/// What a node and the main thread both see.
#[derive(Debug, Default)]
pub(crate) struct Status {
    /// The latency the plug-in reported when it was last activated.
    pub(crate) latency: AtomicU32,
    /// The host wants the active component back.
    pub(crate) hand_back: AtomicBool,
    /// The plug-in failed a call on the audio thread.
    pub(crate) failed: AtomicBool,
    /// The host says the device is silent while the node has no plug-in:
    /// its plug-in failed elsewhere, in an offline render (WP-4.21).
    pub(crate) silent: AtomicBool,
    /// Changes dropped because a queue was full.
    pub(crate) dropped: AtomicU64,
}

/// The main thread's ends of a node's queues.
pub(crate) struct HostEnds {
    pub(crate) to_node: Producer<Active>,
    pub(crate) from_node: Consumer<Active>,
    /// The values the node applied to its component, which the main thread
    /// gives the controller so its parameters are the device's (R5).
    pub(crate) applied: Consumer<(u32, f64)>,
    pub(crate) status: Arc<Status>,
    /// The generation of the component handed to this node and not yet
    /// given back, so the host can tell the node was dropped holding it.
    pub(crate) handed: Option<u64>,
}

/// The plug-in as the main thread activated it for a node: the component
/// and its audio processor, its bus layout, and what the node needs to
/// turn the block's plain parameter values into the normalized values a
/// process block carries.
pub(crate) struct Active {
    /// Which handover this is, so the host knows whether a component the
    /// node gives back is the one it handed it.
    pub(crate) generation: u64,
    /// Its audio processor.
    pub(crate) processor: ComPtr<IAudioProcessor>,
    /// The latency it reported when it was activated, in frames.
    pub(crate) latency: u32,
    /// How many audio buses it has each way.
    pub(crate) inputs: usize,
    pub(crate) outputs: usize,
    /// Each bus's channels, in the order `getBusInfo` reports them.
    pub(crate) in_channels: [usize; MAX_BUSES],
    pub(crate) out_channels: [usize; MAX_BUSES],
    /// Its parameters, for the plain-to-normalized conversion.
    pub(crate) params: Vec<ParamMap>,
    /// The parameter its controller maps each channel's MIDI controller to
    /// (`IMidiMapping`), at `channel * CONTROLLERS + controller`.
    pub(crate) midi: Vec<Option<u32>>,
    /// The note expression types its controller takes.
    pub(crate) expressions: NoteExpressions,
    /// The device's plain values, sent to it at the first frame of its
    /// next block.
    pub(crate) pending: Vec<(u32, f64)>,
    /// Its parameters' automation and modulation, played into it (the
    /// modulation spec, R10).
    pub(crate) drive: Driver,
}

/// A parameter change point in a block: `offset` frames into it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    offset: i32,
    value: f64,
}

impl Point {
    const ZERO: Self = Self {
        offset: 0,
        value: 0.0,
    };
}

/// One parameter's queue of changes in a block: a COM object the plug-in
/// reads through `IParameterChanges`, and, for its output changes, writes
/// to. Made with the node and reused every block, so nothing on the audio
/// thread allocates.
struct HostQueue {
    id: Cell<u32>,
    points: UnsafeCell<[Point; MAX_POINTS]>,
    count: Cell<usize>,
}

// SAFETY: a node's queues are touched only from the node's own thread, and
// a plug-in's calls during a process block run on that thread; the
// wrapper's reference count is atomic.
unsafe impl Send for HostQueue {}
// SAFETY: as above.
unsafe impl Sync for HostQueue {}

impl Class for HostQueue {
    type Interfaces = (IParamValueQueue,);
}

impl HostQueue {
    fn new() -> Self {
        Self {
            id: Cell::new(0),
            points: UnsafeCell::new([Point::ZERO; MAX_POINTS]),
            count: Cell::new(0),
        }
    }

    fn reset(&self) {
        self.count.set(0);
    }

    fn id(&self) -> u32 {
        self.id.get()
    }

    fn set_id(&self, id: u32) {
        self.id.set(id);
    }

    fn point(&self, index: usize) -> Option<Point> {
        if index >= self.count.get() {
            return None;
        }
        // SAFETY: a single-threaded read of a point the queue holds.
        Some(unsafe { (*self.points.get())[index] })
    }

    /// Adds a point at the end; false when the queue is full.
    fn push(&self, offset: i32, value: f64) -> bool {
        let at = self.count.get();
        if at >= MAX_POINTS {
            return false;
        }
        // SAFETY: a single-threaded write of a point slot the queue owns.
        unsafe { (*self.points.get())[at] = Point { offset, value } };
        self.count.set(at + 1);
        true
    }
}

impl IParamValueQueueTrait for HostQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }

    unsafe fn getPointCount(&self) -> int32 {
        int32::try_from(self.count.get()).unwrap_or(int32::MAX)
    }

    unsafe fn getPoint(&self, index: int32, sample_offset: *mut int32, value: *mut f64) -> tresult {
        let Some(point) = self.point(usize::try_from(index).unwrap_or(usize::MAX)) else {
            return kInvalidArgument;
        };
        // SAFETY: the caller passes valid out pointers, as the interface
        // says it does.
        unsafe {
            *sample_offset = point.offset;
            *value = point.value;
        }
        kResultTrue
    }

    unsafe fn addPoint(&self, sample_offset: int32, value: f64, index: *mut int32) -> tresult {
        let at = self.count.get();
        if !self.push(sample_offset, value) {
            return kResultFalse;
        }
        if !index.is_null() {
            // SAFETY: the caller passes a valid index pointer or null, as
            // the interface says it does.
            unsafe { *index = int32::try_from(at).unwrap_or(int32::MAX) };
        }
        kResultOk
    }
}

/// A block's parameter changes: the queues the plug-in reads before
/// processing, and, for its own output changes, the queues it writes.
pub(crate) struct HostParams {
    queues: [ComWrapper<HostQueue>; MAX_QUEUES],
    used: Cell<usize>,
    dropped: Cell<u64>,
}

// SAFETY: as `HostQueue`, which it holds.
unsafe impl Send for HostParams {}
// SAFETY: as above.
unsafe impl Sync for HostParams {}

impl Class for HostParams {
    type Interfaces = (IParameterChanges,);
}

impl HostParams {
    fn new() -> Self {
        Self {
            queues: std::array::from_fn(|_| ComWrapper::new(HostQueue::new())),
            used: Cell::new(0),
            dropped: Cell::new(0),
        }
    }

    /// Empties the queues for a new block.
    fn reset(&self) {
        for queue in &self.queues {
            queue.reset();
        }
        self.used.set(0);
        self.dropped.set(0);
    }

    /// The number of changes dropped because a queue was full.
    fn dropped(&self) -> u64 {
        self.dropped.get()
    }

    /// Adds a change: to the parameter's queue if the block has one, else a
    /// new queue while there's room.
    fn push(&self, id: u32, offset: i32, value: f64) {
        let mut slot = None;
        for index in 0..self.used.get() {
            if self.queues[index].id() == id {
                slot = Some(index);
                break;
            }
        }
        let index = match slot {
            Some(index) => index,
            None => {
                if self.used.get() >= MAX_QUEUES {
                    self.dropped.set(self.dropped.get() + 1);
                    return;
                }
                let index = self.used.get();
                self.used.set(index + 1);
                self.queues[index].set_id(id);
                self.queues[index].reset();
                index
            }
        };
        if !self.queues[index].push(offset, value) {
            self.dropped.set(self.dropped.get() + 1);
        }
    }

    fn queue(&self, index: usize) -> Option<ComRef<'_, IParamValueQueue>> {
        self.queues.get(index)?.as_com_ref::<IParamValueQueue>()
    }
}

impl IParameterChangesTrait for HostParams {
    unsafe fn getParameterCount(&self) -> int32 {
        int32::try_from(self.used.get()).unwrap_or(int32::MAX)
    }

    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        let Some(queue) = self.queue(usize::try_from(index).unwrap_or(usize::MAX)) else {
            return ptr::null_mut();
        };
        queue.as_ptr()
    }

    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        if id.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: the caller passes a valid ID pointer, as the interface
        // says it does.
        let id = unsafe { *id };
        let mut slot = None;
        for at in 0..self.used.get() {
            if self.queues[at].id() == id {
                slot = Some(at);
                break;
            }
        }
        let at = match slot {
            Some(at) => at,
            None => {
                if self.used.get() >= MAX_QUEUES {
                    return ptr::null_mut();
                }
                let at = self.used.get();
                self.used.set(at + 1);
                self.queues[at].set_id(id);
                self.queues[at].reset();
                at
            }
        };
        if !index.is_null() {
            // SAFETY: the caller passes a valid index pointer or null.
            unsafe { *index = int32::try_from(at).unwrap_or(int32::MAX) };
        }
        self.queue(at)
            .map_or(ptr::null_mut(), |queue| queue.as_ptr())
    }
}

/// A block's events: the notes the plug-in reads on their frames, and the
/// ones it sends out on theirs.
pub(crate) struct HostEvents {
    events: UnsafeCell<[Event; MAX_EVENTS]>,
    count: Cell<usize>,
    dropped: Cell<u64>,
}

// SAFETY: as `HostQueue`: a node's event lists are touched only from its
// own thread and the plug-in's calls during a process block, and the
// wrapper's reference count is atomic.
unsafe impl Send for HostEvents {}
// SAFETY: as above.
unsafe impl Sync for HostEvents {}

impl Class for HostEvents {
    type Interfaces = (IEventList,);
}

impl HostEvents {
    fn new() -> Self {
        Self {
            events: UnsafeCell::new(std::array::from_fn(|_| {
                // SAFETY: an all-zero Event is a valid one; the list fills
                // its fields before it's read.
                unsafe { std::mem::zeroed() }
            })),
            count: Cell::new(0),
            dropped: Cell::new(0),
        }
    }

    fn reset(&self) {
        self.count.set(0);
    }

    fn len(&self) -> usize {
        self.count.get()
    }

    fn event(&self, index: usize) -> Option<Event> {
        if index >= self.count.get() {
            return None;
        }
        // SAFETY: a single-threaded read of an event the list holds.
        Some(unsafe { (*self.events.get())[index] })
    }

    fn push(&self, event: Event) -> bool {
        let at = self.count.get();
        if at >= MAX_EVENTS {
            self.dropped.set(self.dropped.get() + 1);
            return false;
        }
        // SAFETY: a single-threaded write of an event slot the list owns.
        unsafe { (*self.events.get())[at] = event };
        self.count.set(at + 1);
        true
    }

    /// Adds a note the block carries, on its frame, with its expressions
    /// sent as `expressions` and `mpe` say (the MPE spec, R15, R16).
    fn push_note(&self, note: &NoteEvent, mpe: bool, expressions: &NoteExpressions) -> bool {
        let mut event: Event = Event {
            busIndex: 0,
            sampleOffset: i32::try_from(note.offset).unwrap_or(i32::MAX),
            ppqPosition: 0.0,
            flags: 0,
            r#type: 0,
            __field0: Event__type0 {
                noteOn: NoteOnEvent {
                    channel: 0,
                    pitch: 0,
                    tuning: 0.0,
                    velocity: 0.0,
                    length: 0,
                    noteId: -1,
                },
            },
        };
        let channel = i16::from(note.channel.min(15));
        let pitch = i16::from(note.key.min(127));
        let note_id = note
            .note_id
            .map_or(-1, |id| i32::try_from(id).unwrap_or(-1));
        match note.kind {
            // Spec: vst3-hosting R19, R20. Pressure is polyphonic pressure
            // unless MPE Mode is on and the controller maps it to a note
            // expression; pitch and slide are note expressions, with MPE
            // Mode on.
            NoteKind::Expression { expression, value } => {
                let value = f64::from(value);
                let expression_type = match (mpe, expression) {
                    (true, NoteExpression::Bend) => Some((
                        NoteExpressionTypeIDs_::kTuningTypeID,
                        (0.5 + value * BEND_SEMITONES / 240.0).clamp(0.0, 1.0),
                    )),
                    (true, NoteExpression::Slide) => {
                        expressions.slide.map(|t| (t, value.clamp(0.0, 1.0)))
                    }
                    (true, NoteExpression::Pressure) => {
                        expressions.pressure.map(|t| (t, value.clamp(0.0, 1.0)))
                    }
                    _ => None,
                };
                if let Some((type_id, value)) = expression_type {
                    if note_id < 0 {
                        return false;
                    }
                    event.r#type = Event_::EventTypes_::kNoteExpressionValueEvent as u16;
                    event.__field0 = Event__type0 {
                        noteExpressionValue: NoteExpressionValueEvent {
                            typeId: type_id,
                            noteId: note_id,
                            value,
                        },
                    };
                } else if expression == NoteExpression::Pressure {
                    event.r#type = Event_::EventTypes_::kPolyPressureEvent as u16;
                    event.__field0 = Event__type0 {
                        polyPressure: PolyPressureEvent {
                            channel,
                            pitch,
                            pressure: value.clamp(0.0, 1.0) as f32,
                            noteId: note_id,
                        },
                    };
                } else {
                    return false;
                }
            }
            // Channel messages go in as parameter changes.
            NoteKind::Control { .. }
            | NoteKind::PitchBend { .. }
            | NoteKind::ChannelPressure { .. } => return false,
            NoteKind::On => {
                event.r#type = Event_::EventTypes_::kNoteOnEvent as u16;
                event.__field0 = Event__type0 {
                    noteOn: NoteOnEvent {
                        channel,
                        pitch,
                        tuning: 0.0,
                        velocity: note.velocity.clamp(0.0, 1.0),
                        length: 0,
                        noteId: note_id,
                    },
                };
            }
            NoteKind::Off | NoteKind::Choke => {
                event.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
                event.__field0 = Event__type0 {
                    noteOff: NoteOffEvent {
                        channel,
                        pitch,
                        velocity: if note.kind == NoteKind::Choke {
                            0.0
                        } else {
                            note.velocity.clamp(0.0, 1.0)
                        },
                        noteId: note_id,
                        tuning: 0.0,
                    },
                };
            }
        }
        self.push(event)
    }
}

impl IEventListTrait for HostEvents {
    unsafe fn getEventCount(&self) -> int32 {
        int32::try_from(self.count.get()).unwrap_or(int32::MAX)
    }

    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        let Some(event) = self.event(usize::try_from(index).unwrap_or(usize::MAX)) else {
            return kInvalidArgument;
        };
        if e.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the caller passes a valid out pointer, as the interface
        // says it does.
        unsafe { *e = event };
        kResultTrue
    }

    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        if e.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the caller passes a valid event, as the interface says it
        // does.
        let event = unsafe { *e };
        if self.push(event) {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

/// A VST3 plug-in's node.
pub struct Vst3Node {
    active: Option<Active>,
    from_host: Consumer<Active>,
    to_host: Producer<Active>,
    status: Arc<Status>,
    /// It failed while processing: it's never called again and the node is
    /// silent.
    dead: bool,
    /// The block's changes to the plug-in, and its own changes out.
    incoming: ComWrapper<HostParams>,
    outgoing: ComWrapper<HostParams>,
    /// The block's notes to the plug-in, and its own notes out.
    events_in: ComWrapper<HostEvents>,
    events_out: ComWrapper<HostEvents>,
    /// The values applied to the component, for the main thread to give the
    /// controller.
    applied: Producer<(u32, f64)>,
    /// The block's channels, in lanes of `max_frames`.
    ins: Vec<f32>,
    outs: Vec<f32>,
    in_ptrs: Vec<*mut f32>,
    out_ptrs: Vec<*mut f32>,
    in_buses: [AudioBusBuffers; MAX_BUSES],
    out_buses: [AudioBusBuffers; MAX_BUSES],
    context: ProcessContext,
    /// Changes kept for the plug-in while it isn't running.
    pending: Vec<(u32, ParamValue)>,
    /// The automation and clip modulation the compiler sent.
    automation: Option<NodeData>,
    /// The device sits in the MIDI-effect position (IMP-24): its notes go
    /// on, its audio output is dropped, and the audio it receives passes
    /// through.
    midi_effect: bool,
    /// Frames processed since the plug-in was activated.
    steady: u64,
    max_frames: usize,
    /// The device's MPE Mode and settings (the MPE spec, R14), as the
    /// compiler last sent them; `pending_mpe` waits for the next block.
    mpe: PluginMpe,
    pending_mpe: Option<PluginMpe>,
    /// MPE MIDI laid out for a controller without note expression (R17),
    /// made with the node.
    layout: MpeLayout,
    /// Retunes the block's notes while the plug-in plays the set's tuning
    /// (the tunings spec, R20).
    tuned: TunedNotes,
    /// The notes it was given as plain notes, which MPE MIDI laid out through
    /// the MIDI mapping takes over from.
    plain: PlainNotes,
    /// The notes the plug-in sent: ID, channel, key, in a ring, so its note
    /// expressions name them (R21).
    sent: [(i32, u8, u8); SENT_NOTES],
    sent_next: usize,
}

impl Vst3Node {
    /// A node for `config`, with the main thread's ends of its queues. The
    /// plug-in's `latency` is the last one it reported, if the host knows
    /// it: a node made after a restart reads it at once, so the compiler's
    /// delay compensation doesn't wait for the first block. With
    /// `midi_effect` the device is placed as a MIDI effect (IMP-24), so its
    /// audio output is dropped.
    pub(crate) fn new(
        config: &bl_engine::NodeConfig,
        latency: u32,
        midi_effect: bool,
    ) -> (Self, HostEnds) {
        let (to_node, from_host) = RingBuffer::new(1);
        let (to_host, from_node) = RingBuffer::new(2);
        let (applied, reported) = RingBuffer::new(APPLIED_CAPACITY);
        let status = Arc::new(Status::default());
        status.latency.store(latency, Relaxed);
        let max_frames = config.max_frames.max(1);
        let node = Self {
            active: None,
            from_host,
            to_host,
            status: Arc::clone(&status),
            dead: false,
            incoming: ComWrapper::new(HostParams::new()),
            outgoing: ComWrapper::new(HostParams::new()),
            events_in: ComWrapper::new(HostEvents::new()),
            events_out: ComWrapper::new(HostEvents::new()),
            applied,
            ins: vec![0.0; MAX_CHANNELS * max_frames],
            outs: vec![0.0; MAX_CHANNELS * max_frames],
            in_ptrs: vec![ptr::null_mut(); MAX_CHANNELS],
            out_ptrs: vec![ptr::null_mut(); MAX_CHANNELS],
            // SAFETY: an all-zero AudioBusBuffers is a valid one; each
            // block fills in what the plug-in reads.
            in_buses: std::array::from_fn(|_| unsafe { std::mem::zeroed() }),
            // SAFETY: as above.
            out_buses: std::array::from_fn(|_| unsafe { std::mem::zeroed() }),
            // SAFETY: an all-zero ProcessContext is a valid one; each block
            // fills in what the plug-in reads.
            context: unsafe { std::mem::zeroed() },
            pending: Vec::with_capacity(PENDING_CAPACITY),
            automation: None,
            midi_effect,
            steady: 0,
            max_frames,
            mpe: PluginMpe::default(),
            pending_mpe: None,
            layout: MpeLayout::new(PluginMpe::default().settings),
            tuned: TunedNotes::new(),
            plain: PlainNotes::default(),
            sent: [(-1, 0, 0); SENT_NOTES],
            sent_next: 0,
        };
        let ends = HostEnds {
            to_node,
            from_node,
            applied: reported,
            status,
            handed: None,
        };
        (node, ends)
    }

    /// Takes an active component the host sent, giving back any it
    /// replaces.
    fn receive(&mut self) {
        while let Ok(active) = self.from_host.pop() {
            for &(id, value) in &active.pending {
                self.keep(id, ParamValue::Set(value));
            }
            if let Some(old) = self.active.replace(active) {
                self.give_back(old);
            }
            self.dead = false;
            // Spec: mpe R17. A plug-in activated again is opened again.
            self.layout.reopen();
            if let Some(active) = self.active.as_ref() {
                self.status.latency.store(active.latency, Relaxed);
            }
        }
        if self.status.hand_back.swap(false, Relaxed)
            && let Some(active) = self.active.take()
        {
            self.give_back(active);
        }
    }

    /// Sends an active component to the main thread to deactivate.
    fn give_back(&mut self, active: Active) {
        // The queue has room for the component the node had and one it
        // replaced; if it's full the component is dropped here, and the
        // host deactivates the plug-in once it's gone.
        let _ = self.to_host.push(active);
    }

    /// Keeps a change for the plug-in when it next runs.
    fn keep(&mut self, id: u32, value: ParamValue) {
        if let Some(slot) = self.pending.iter_mut().find(|(param, _)| *param == id) {
            slot.1 = value;
        } else if self.pending.len() < self.pending.capacity() {
            self.pending.push((id, value));
        } else {
            self.status.dropped.fetch_add(1, Relaxed);
        }
    }

    fn fail(&mut self, block: &mut Block<'_>) {
        self.dead = true;
        self.status.failed.store(true, Relaxed);
        silence(block);
    }

    /// Fills the audio buses for the block from the node's buffers, and
    /// points them at the block's own lanes.
    fn fill_buses(
        &mut self,
        block: &Block<'_>,
        frames: usize,
        (inputs, outputs): (usize, usize),
        in_channels: &[usize; MAX_BUSES],
        out_channels: &[usize; MAX_BUSES],
    ) {
        let stride = self.max_frames;
        let mut at = 0usize;
        for (bus, channels) in in_channels.iter().enumerate().take(inputs) {
            let count = *channels;
            self.in_buses[bus] = AudioBusBuffers {
                numChannels: i32::try_from(count).unwrap_or(i32::MAX),
                silenceFlags: if count == 0 { 1 } else { 0 },
                __field0: AudioBusBuffers__type0 {
                    // SAFETY: the pointer is into `in_ptrs`, which outlives
                    // the call, at a slot for this bus.
                    channelBuffers32: unsafe { self.in_ptrs.as_mut_ptr().add(at) },
                },
            };
            for channel in 0..count {
                self.in_ptrs[at + channel] =
                    // SAFETY: `ins` holds a lane for every channel index.
                    unsafe { self.ins.as_mut_ptr().add((at + channel) * stride) };
            }
            at += count;
        }
        let block_channels = block.channels();
        let fed = in_channels.first().copied().unwrap_or(0).min(MAX_CHANNELS);
        for channel in 0..fed {
            if block_channels == 0 {
                break;
            }
            let from = block.channel(channel.min(block_channels - 1));
            self.ins[channel * stride..channel * stride + frames].copy_from_slice(from);
        }

        let mut at = 0usize;
        for (bus, channels) in out_channels.iter().enumerate().take(outputs) {
            let count = *channels;
            self.out_buses[bus] = AudioBusBuffers {
                numChannels: i32::try_from(count).unwrap_or(i32::MAX),
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    // SAFETY: as the input buses above.
                    channelBuffers32: unsafe { self.out_ptrs.as_mut_ptr().add(at) },
                },
            };
            for channel in 0..count {
                // SAFETY: `outs` holds a lane for every channel index.
                self.out_ptrs[at + channel] =
                    unsafe { self.outs.as_mut_ptr().add((at + channel) * stride) };
            }
            at += count;
        }
    }

    /// Applies the plug-in's own parameter changes to nothing yet: part 1
    /// takes them from the block and drops them (the next part wires them
    /// into the set as undo steps).
    fn drain_params(&mut self) {
        self.outgoing.reset();
    }
}

// SAFETY: the node's raw pointers point into its own buffers, which move
// with it, and it's used by one thread at a time: the compiler thread makes
// it, the audio thread runs it, and the main thread's ends are the queues.
unsafe impl Send for Vst3Node {}

/// Fills a block's parameter changes for the plug-in: the values
/// automation and modulation give the parameters they drive, on their
/// frames (the modulation spec, R10); the values kept while it wasn't
/// running at the first frame, then the block's changes on theirs, those of
/// driven parameters only moving their controls; as normalized values over
/// each parameter's plain range. Each value applied from the document is
/// kept for `applied`, so the main thread can give the controller the same
/// values (R5). Free of the node so it borrows only what it needs, and
/// allocates nothing.
fn fill_params(
    incoming: &ComWrapper<HostParams>,
    pending: &mut Vec<(u32, ParamValue)>,
    applied: &mut Producer<(u32, f64)>,
    active: &mut Active,
    list: Option<&AutomationList>,
    block: &Block<'_>,
    status: &Status,
) {
    incoming.reset();
    let Active { params, drive, .. } = active;
    let params: &[ParamMap] = params;
    let plain_of = |map: &ParamMap, value: ParamValue| match value {
        ParamValue::Set(value) => value,
        ParamValue::Default => map.default,
    };
    drive.prepare(list, block);
    for &(id, value) in pending.iter() {
        if let Some(map) = params.iter().find(|map| map.id == id) {
            drive.set_control(id, plain_of(map, value));
        }
    }
    for event in block.events() {
        if let Some(map) = params.iter().find(|map| map.id == event.param.0) {
            drive.set_control(event.param.0, plain_of(map, event.value));
        }
    }
    drive.play(list, block, |play| {
        if let Some(map) = params.iter().find(|map| map.id == play.id) {
            let offset = i32::try_from(play.offset).unwrap_or(i32::MAX);
            incoming.push(play.id, offset, map.normalized(play.played));
        }
    });
    for &(id, value) in pending.iter() {
        let Some(map) = params.iter().find(|map| map.id == id) else {
            continue;
        };
        if drive.is_driven(id) {
            continue;
        }
        let normalized = map.normalized(plain_of(map, value));
        incoming.push(id, 0, normalized);
        report(applied, status, id, normalized);
    }
    pending.clear();
    for event in block.events() {
        let Some(map) = params.iter().find(|map| map.id == event.param.0) else {
            continue;
        };
        if drive.is_driven(event.param.0) {
            continue;
        }
        let offset = i32::try_from(event.offset).unwrap_or(i32::MAX);
        let normalized = map.normalized(plain_of(map, event.value));
        incoming.push(event.param.0, offset, normalized);
        report(applied, status, event.param.0, normalized);
    }
    let dropped = incoming.dropped();
    if dropped > 0 {
        status.dropped.fetch_add(dropped, Relaxed);
    }
}

/// The parameter and normalized value a channel message sets in the plug-in,
/// if its controller maps the message's controller to one.
fn mapped(map: &[Option<u32>], note: &NoteEvent) -> Option<(u32, f64)> {
    let message = note.message()?;
    let (controller, value) = match message.status() & 0xF0 {
        0xB0 => (
            usize::from(message.data1()),
            f64::from(message.data2()) / 127.0,
        ),
        0xD0 => (AFTER_TOUCH, f64::from(message.data1()) / 127.0),
        _ => {
            let raw = i32::from(message.data1()) | i32::from(message.data2()) << 7;
            (PITCH_BEND, f64::from(raw) / 16383.0)
        }
    };
    let at = usize::from(note.channel & 0x0F) * CONTROLLERS + controller;
    Some((map.get(at).copied().flatten()?, value))
}

/// Reports an applied value to the main thread; past the queue's room it's
/// dropped and counted.
fn report(applied: &mut Producer<(u32, f64)>, status: &Status, id: u32, value: f64) {
    if applied.push((id, value)).is_err() {
        status.dropped.fetch_add(1, Relaxed);
    }
}

fn silence(block: &mut Block<'_>) {
    for channel in block.channels_mut() {
        channel.fill(0.0);
    }
}

impl Node for Vst3Node {
    fn process(&mut self, block: &mut Block<'_>) {
        self.receive();
        let frames = block.frames();
        if self.active.is_none() {
            // Not running: keep the changes for it, and pass the input
            // through (or stay silent after a failure).
            for event in block.events() {
                self.keep(event.param.0, event.value);
            }
            if self.dead || self.status.silent.load(Relaxed) {
                silence(block);
            }
            return;
        }
        if self.dead {
            for event in block.events() {
                self.keep(event.param.0, event.value);
            }
            silence(block);
            return;
        }
        let (inputs, outputs, in_channels, out_channels) = {
            let active = self.active.as_ref().expect("the check above");
            (
                active.inputs,
                active.outputs,
                active.in_channels,
                active.out_channels,
            )
        };

        // The changes and the notes the plug-in reads, on their frames.
        {
            let active = self.active.as_mut().expect("the check above");
            let list = self
                .automation
                .as_ref()
                .and_then(|data| data.downcast_ref::<AutomationList>());
            fill_params(
                &self.incoming,
                &mut self.pending,
                &mut self.applied,
                active,
                list,
                block,
                &self.status,
            );
        }
        self.events_in.reset();
        let active = self.active.as_ref().expect("the check above");
        let (map, expressions) = (&active.midi, active.expressions);
        take_mpe(
            &mut self.pending_mpe,
            &mut self.mpe,
            (&mut self.layout, &mut self.plain),
            (&self.events_in, &self.incoming),
            (map, !expressions.controller),
        );
        // Spec: tunings R20.
        let notes = self.tuned.retune(block.notes());
        if self.mpe.on && !expressions.controller {
            // Spec: vst3-hosting R20. A controller without note expression
            // gets MPE MIDI: each note on its member channel, and that
            // channel's pitch bend, controller 74 and pressure through its
            // MIDI mapping.
            let (events, incoming) = (&self.events_in, &self.incoming);
            let mut sink = |offset: u32, message: MidiMessage| {
                lay_in(events, incoming, map, offset, message);
            };
            self.layout.open(&mut sink);
            self.layout.lay_out(notes, &mut sink);
        } else {
            for note in notes {
                if note.kind.is_message() {
                    // Spec: vst3-hosting R18. A plug-in reads a channel
                    // message as the parameter its controller maps it to.
                    if let Some((id, value)) = mapped(map, note) {
                        let offset = i32::try_from(note.offset).unwrap_or(i32::MAX);
                        // shortcut: appended after the block's own changes, so a message earlier than an automation point of the same parameter is out of order; sort the queue if a plug-in maps an automated parameter.
                        self.incoming.push(id, offset, value);
                    }
                } else {
                    self.plain.track(note);
                    self.events_in.push_note(note, self.mpe.on, &expressions);
                }
            }
        }
        self.events_out.reset();
        self.outgoing.reset();
        self.fill_buses(
            block,
            frames,
            (inputs, outputs),
            &in_channels,
            &out_channels,
        );
        fill_context(&mut self.context, self.steady, block);

        let active = self.active.as_ref().expect("the check above");
        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            numSamples: i32::try_from(frames).unwrap_or(i32::MAX),
            numInputs: i32::try_from(inputs).unwrap_or(i32::MAX),
            numOutputs: i32::try_from(outputs).unwrap_or(i32::MAX),
            inputs: self.in_buses.as_mut_ptr(),
            outputs: self.out_buses.as_mut_ptr(),
            inputParameterChanges: self
                .incoming
                .as_com_ref::<IParameterChanges>()
                .map_or(ptr::null_mut(), |changes| changes.as_ptr()),
            outputParameterChanges: self
                .outgoing
                .as_com_ref::<IParameterChanges>()
                .map_or(ptr::null_mut(), |changes| changes.as_ptr()),
            inputEvents: self
                .events_in
                .as_com_ref::<IEventList>()
                .map_or(ptr::null_mut(), |events| events.as_ptr()),
            outputEvents: self
                .events_out
                .as_com_ref::<IEventList>()
                .map_or(ptr::null_mut(), |events| events.as_ptr()),
            processContext: &raw mut self.context,
        };
        let result = {
            // Spec: ara-hosting R7. An ARA plug-in's audio readers must not
            // be reached from inside this call.
            let _rendering = crate::ara::Rendering::enter();
            // SAFETY: the plug-in is active and gets a valid ProcessData
            // with its buses, changes and events, which live through the
            // call.
            unsafe { active.processor.process(&raw mut data) }
        };
        self.steady = self.steady.wrapping_add(frames as u64);
        if result != kResultOk {
            self.events_out.reset();
            self.drain_params();
            self.fail(block);
            return;
        }

        // The audio: the plug-in's main output bus back into the block. A
        // plug-in placed as a MIDI effect passes the audio it receives on,
        // and its own output is dropped (IMP-24).
        if !self.midi_effect {
            let channels = block.channels();
            let outs = out_channels.first().copied().unwrap_or(0).min(MAX_CHANNELS);
            let stride = self.max_frames;
            for channel in 0..channels {
                let out = block.channel_mut(channel);
                if outs == 0 {
                    out.fill(0.0);
                } else {
                    let from = channel.min(outs - 1) * stride;
                    out.copy_from_slice(&self.outs[from..from + frames]);
                }
            }
        }

        // Its notes back into the block on their frames, and, with MPE Mode
        // on, its note expressions as the expressions of those notes
        // (R21).
        let expressions = self.active.as_ref().map(|active| active.expressions);
        for index in 0..self.events_out.len() {
            let Some(event) = self.events_out.event(index) else {
                break;
            };
            let Some(note) = note_out(&event) else {
                if self.mpe.on
                    && let Some(expressions) = expressions
                    && let Some(note) = expression_out(&event, &expressions, &self.sent)
                {
                    block.send_note(note);
                }
                continue;
            };
            if note.kind == NoteKind::On
                && let Some(id) = note.note_id
            {
                // A plug-in may reuse an ended note's ID: its slot is the
                // new note's.
                let id = id as i32;
                let slot = self.sent.iter().position(|(sent, ..)| *sent == id);
                self.sent[slot.unwrap_or(self.sent_next)] = (id, note.channel, note.key);
                if slot.is_none() {
                    self.sent_next = (self.sent_next + 1) % SENT_NOTES;
                }
            }
            block.send_note(note);
        }
        self.drain_params();
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        self.receive();
        // VST3 has no flush call: the changes wait for the plug-in's next
        // block, at its first frame.
        for event in events {
            self.keep(event.param.0, event.value);
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        if data.downcast_ref::<AutomationList>().is_some() {
            self.automation = Some(data);
        } else if let Some(PluginMpeData(mpe)) = data.downcast_ref() {
            self.pending_mpe = Some(*mpe);
        } else if let Some(PluginTuningData(keys)) = data.downcast_ref() {
            self.tuned.set(*keys);
        }
    }

    fn modulated(&self, report: &mut dyn FnMut(ParamId, f64, f64)) {
        if let Some(active) = &self.active {
            active.drive.modulated(report);
        }
    }

    fn latency(&self) -> u32 {
        self.status.latency.load(Relaxed)
    }
}

/// Takes the MPE Mode and settings the compiler sent, at the start of a
/// block: the notes MPE MIDI laid out for the old ones ends, and the zone
/// opens again.
fn take_mpe(
    pending: &mut Option<PluginMpe>,
    mpe: &mut PluginMpe,
    (layout, plain): (&mut MpeLayout, &mut PlainNotes),
    (events, incoming): (&ComWrapper<HostEvents>, &ComWrapper<HostParams>),
    (map, through_mapping): (&[Option<u32>], bool),
) {
    let Some(new) = pending.take() else {
        return;
    };
    let mut sink = |offset: u32, message: MidiMessage| {
        lay_in(events, incoming, map, offset, message);
    };
    if mpe.on != new.on {
        layout.release_all(0, &mut sink);
        // The layout knows none of the notes given as plain ones.
        if new.on && through_mapping {
            plain.release_all(0, &mut sink);
        }
    }
    layout.configure(new.settings, &mut sink);
    *mpe = new;
}

/// Hands a message of MPE MIDI to the plug-in: a note as a note event on its
/// channel, anything else as the parameter the controller maps it to
/// (R20).
fn lay_in(
    events: &ComWrapper<HostEvents>,
    incoming: &ComWrapper<HostParams>,
    map: &[Option<u32>],
    offset: u32,
    message: MidiMessage,
) {
    let Some(note) = NoteEvent::from_midi(offset, &message) else {
        return;
    };
    if matches!(note.kind, NoteKind::On | NoteKind::Off) {
        events.push_note(&note, false, &NoteExpressions::default());
    } else if let Some((id, value)) = mapped(map, &note) {
        incoming.push(id, i32::try_from(offset).unwrap_or(i32::MAX), value);
    }
}

/// The per-note expression a note expression value event out is, if the
/// plug-in sent the note it names and its type is one Beatlayer takes:
/// tuning as a bend of its semitones over 48, and the types its physical
/// mapping gives Y movement and pressure as slide and pressure (R21).
fn expression_out(
    event: &Event,
    expressions: &NoteExpressions,
    sent: &[(i32, u8, u8); SENT_NOTES],
) -> Option<NoteEvent> {
    if event.r#type != Event_::EventTypes_::kNoteExpressionValueEvent as u16 {
        return None;
    }
    // SAFETY: the event's type says which union member it holds.
    let value = unsafe { event.__field0.noteExpressionValue };
    let (expression, scaled) = if value.typeId == NoteExpressionTypeIDs_::kTuningTypeID {
        (
            NoteExpression::Bend,
            ((value.value - 0.5) * 240.0 / BEND_SEMITONES).clamp(-1.0, 1.0),
        )
    } else if Some(value.typeId) == expressions.slide {
        (NoteExpression::Slide, value.value.clamp(0.0, 1.0))
    } else if Some(value.typeId) == expressions.pressure {
        (NoteExpression::Pressure, value.value.clamp(0.0, 1.0))
    } else {
        return None;
    };
    let &(id, channel, key) = sent
        .iter()
        .find(|(id, ..)| *id == value.noteId && *id >= 0)?;
    Some(NoteEvent {
        offset: u32::try_from(event.sampleOffset.max(0)).unwrap_or(0),
        kind: NoteKind::Expression {
            expression,
            value: scaled as f32,
        },
        channel,
        key,
        velocity: 0.0,
        note_id: Some(id as u32),
    })
}

/// The note an event list holds, if it's one Beatlayer takes.
fn note_out(event: &Event) -> Option<NoteEvent> {
    let offset = u32::try_from(event.sampleOffset.max(0)).unwrap_or(0);
    // The bindings type the event types by the platform's default, while
    // the event's own field is `uint16`; comparing them there works the
    // same on every platform.
    // SAFETY: the event's type says which union member it holds.
    unsafe {
        match event.r#type {
            x if x == Event_::EventTypes_::kNoteOnEvent as u16 => {
                let note = event.__field0.noteOn;
                Some(NoteEvent {
                    offset,
                    kind: NoteKind::On,
                    channel: u8::try_from(note.channel.clamp(0, 15)).unwrap_or(0),
                    key: u8::try_from(note.pitch.clamp(0, 127)).unwrap_or(0),
                    velocity: note.velocity.clamp(0.0, 1.0),
                    note_id: (note.noteId >= 0).then_some(note.noteId as u32),
                })
            }
            x if x == Event_::EventTypes_::kNoteOffEvent as u16 => {
                let note = event.__field0.noteOff;
                Some(NoteEvent {
                    offset,
                    kind: NoteKind::Off,
                    channel: u8::try_from(note.channel.clamp(0, 15)).unwrap_or(0),
                    key: u8::try_from(note.pitch.clamp(0, 127)).unwrap_or(0),
                    velocity: note.velocity.clamp(0.0, 1.0),
                    note_id: (note.noteId >= 0).then_some(note.noteId as u32),
                })
            }
            _ => None,
        }
    }
}

impl Drop for Vst3Node {
    fn drop(&mut self) {
        // The drop thread drops nodes: the active component goes back to
        // the main thread to be deactivated there.
        if let Some(active) = self.active.take() {
            self.give_back(active);
        }
    }
}

/// Fills the process context from the block's transport and the song's
/// time signature at its first frame.
fn fill_context(context: &mut ProcessContext, steady: u64, block: &Block<'_>) {
    let info = block.transport();
    let rate = block.sample_rate();
    let tempo = block.tempo();
    let ticks = tempo.frames_to_ticks(info.position, rate);
    let quarters = |ticks: Ticks| ticks.0 as f64 / TICKS_PER_QUARTER as f64;
    let signatures = block.signatures();
    let signature = signatures.signature_at(ticks);
    let (_, into_bar) = signatures.bar_at(ticks);
    use ProcessContext_::StatesAndFlags_ as Flags;
    // The bindings' default enum type is signed on Windows and unsigned
    // elsewhere; the field is always `uint32`.
    context.state = (Flags::kTempoValid
        | Flags::kTimeSigValid
        | Flags::kProjectTimeMusicValid
        | Flags::kBarPositionValid
        | Flags::kContTimeValid
        | if info.playing { Flags::kPlaying } else { 0 }) as uint32;
    context.sampleRate = f64::from(rate.hz());
    context.projectTimeSamples = info.position.0;
    context.continousTimeSamples = i64::try_from(steady).unwrap_or(i64::MAX);
    context.projectTimeMusic = quarters(ticks);
    context.barPositionMusic = quarters(ticks - into_bar);
    context.tempo = tempo.bpm_at(ticks).get();
    context.timeSigNumerator = i32::from(signature.numerator());
    context.timeSigDenominator = i32::from(signature.denominator());
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_core::time::{Frames, SampleRate, TimeSignature, TimeSignatureMap};
    use bl_engine::TransportInfo;

    #[test]
    fn a_dropped_change_is_counted_in_its_own_block_only() {
        let params = HostParams::new();
        for id in 0..=MAX_QUEUES as u32 {
            params.push(id, 0, 0.5);
        }
        assert_eq!(params.dropped(), 1);
        params.reset();
        assert_eq!(params.dropped(), 0);
    }

    #[test]
    fn the_process_context_carries_the_songs_time_signature_and_bar_start() {
        let signatures = TimeSignatureMap::constant(TimeSignature::new(3, 4).unwrap());
        let mut data = [0.0f32; 8];
        // At 120 bpm and 48 kHz four quarters are 96 000 frames: one quarter
        // into the second bar of 3/4.
        let transport = TransportInfo::at(true, Frames(96_000));
        let block = Block::new(&mut data, 1, 8, 8, &[], transport, SampleRate::HZ_48000)
            .with_signatures(&signatures);
        // SAFETY: an all-zero process context is a valid one.
        let mut context: ProcessContext = unsafe { std::mem::zeroed() };
        fill_context(&mut context, 0, &block);
        assert_eq!(context.timeSigNumerator, 3);
        assert_eq!(context.timeSigDenominator, 4);
        assert_eq!(context.projectTimeMusic, 4.0);
        assert_eq!(context.barPositionMusic, 3.0);
        let valid = ProcessContext_::StatesAndFlags_::kBarPositionValid as uint32;
        assert_ne!(context.state & valid, 0);
    }

    #[test]
    fn a_stepped_value_is_its_step_over_the_step_count() {
        // The SDK's stepped range parameter maps normalized values to
        // min(steps, normalized * (steps + 1)), which isn't linear.
        let map = ParamMap::new(0, (0.0, 3.0), 0.0, 3, |normalized| {
            (normalized * 4.0).min(3.0)
        });
        for step in 0..=3 {
            assert_eq!(map.normalized(f64::from(step)), f64::from(step) / 3.0);
        }
    }
}
