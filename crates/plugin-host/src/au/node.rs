//! An Audio Unit's node: what runs it on the audio thread.
//!
//! The node is made on the compiler thread without the unit: the main
//! thread activates the unit ([`AuHost::idle`](super::AuHost::idle)) and
//! hands its processor over through a one-slot queue ([`Processor`]), and
//! the node hands it back the same way when the host asks (to deactivate
//! it) or when the node is dropped. Until a processor arrives the node
//! passes its input through, as a device no factory makes does; after the
//! unit fails a call it outputs silence. Every buffer is made with the
//! node, so nothing on the audio thread allocates, locks, or blocks (the
//! real-time rules); the unit's own code is its own.
//!
//! The unit renders one segment at a time, the block split at its parameter
//! changes and notes (the engine's segment pattern): a parameter change is
//! set on the unit before the segment it starts, and a note is scheduled
//! for that segment's first frame. Setting the value at the segment is what
//! puts a unit's change on its exact sample: Apple's Audio Units ignore
//! scheduled parameter events (AUDelay's Dry/Wet Mix does), while their
//! MIDI events land on the frames they're scheduled for.

use std::cell::{Cell, UnsafeCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering::Relaxed};

use bl_core::midi::MidiMessage;
use bl_engine::mpe_layout::{MpeLayout, PlainNotes};
use bl_engine::{
    AutomationList, Block, Node, NodeConfig, NodeData, NodeEvent, NoteEvent, NoteExpression,
    NoteKind, ParamValue, PluginMpeData,
};
use bl_model::{ParamId, PluginMpe};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2_audio_toolbox::{
    AUAudioUnit, AUAudioUnitStatus, AUEventSampleTimeImmediate, AUParameter, AURenderBlock,
    AURenderPullInputBlock, AUScheduleMIDIEventBlock, AudioUnitRenderActionFlags,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp, AudioTimeStampFlags};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::drive::Driver;

/// The most channels a unit's bus has that Beatlayer feeds.
pub(crate) const MAX_CHANNELS: usize = 8;

/// How many parameter changes and notes one block holds; past that they're
/// dropped and counted (real-time rule 4).
const EVENT_CAPACITY: usize = 1024;

/// How many parameter changes made while the unit isn't running a node
/// keeps for it; past that they're dropped and counted.
const PENDING_CAPACITY: usize = 512;

/// How many MIDI output events one render holds before more are dropped
/// (real-time rule 4).
const MIDI_OUT_EVENTS: usize = 512;

/// The most bytes one MIDI output event carries; a longer one is truncated.
const MIDI_OUT_BYTES: usize = 8;

/// The unit's MIDI output block's signature, as [`AUMIDIOutputEventBlock`]
/// has it: the event's frame within the render call, the cable, the byte
/// count, and the bytes.
///
/// [`AUMIDIOutputEventBlock`]: objc2_audio_toolbox::AUMIDIOutputEventBlock
type MidiOutFn = dyn Fn(i64, u8, isize, NonNull<u8>) -> i32;

/// One MIDI event a unit sent out during a render.
#[derive(Clone, Copy)]
struct MidiOutEvent {
    /// Its frame within the render call, as the unit gave it.
    time: i64,
    /// Its bytes, the first `length` of them.
    bytes: [u8; MIDI_OUT_BYTES],
    length: usize,
}

/// The MIDI events a unit sent out during a render: written by the block
/// the unit calls on the audio thread, drained by the node after the render
/// call. Neither allocates nor locks.
struct MidiOut {
    events: UnsafeCell<[MidiOutEvent; MIDI_OUT_EVENTS]>,
    count: Cell<usize>,
    dropped: Cell<u64>,
}

impl Default for MidiOut {
    fn default() -> Self {
        Self {
            events: UnsafeCell::new(
                [MidiOutEvent {
                    time: 0,
                    bytes: [0; MIDI_OUT_BYTES],
                    length: 0,
                }; MIDI_OUT_EVENTS],
            ),
            count: Cell::new(0),
            dropped: Cell::new(0),
        }
    }
}

/// The host's end of a node's processor queue, the node's, and what they
/// both see.
pub(crate) struct HostEnds {
    pub(crate) to_node: Producer<Processor>,
    pub(crate) from_node: Consumer<Processor>,
    pub(crate) status: Arc<Status>,
}

/// What a node and the main thread both see.
#[derive(Debug, Default)]
pub(crate) struct Status {
    /// The latency the unit reported when it was last activated.
    pub(crate) latency: AtomicU32,
    /// The host wants the processor back.
    pub(crate) hand_back: AtomicBool,
    /// The unit failed a call on the audio thread.
    pub(crate) failed: AtomicBool,
    /// The host says the device is silent while the node has no unit: its
    /// unit failed elsewhere, in an offline render (WP-4.21).
    pub(crate) silent: AtomicBool,
    /// Events dropped because a list was full.
    pub(crate) dropped: AtomicU64,
    /// The editor's parameter-observer token, so the host's own parameter
    /// changes aren't reported back as the unit's own (WP-2.8's second
    /// part); null while no editor is open. Written on the main thread,
    /// read on the audio thread.
    pub(crate) observer: AtomicPtr<c_void>,
}

/// An Audio Unit and everything the audio thread renders it with, made on
/// the main thread and handed to the node through a one-slot queue.
pub(crate) struct Processor {
    /// The unit, deactivated by the host before it's let go of.
    unit: Retained<AUAudioUnit>,
    /// Its render block, cached while it's active, as Apple's documentation
    /// asks.
    render: AURenderBlock,
    /// Its MIDI scheduling block, if it takes notes (a music device or
    /// effect); null otherwise.
    midi: AUScheduleMIDIEventBlock,
    /// The block the unit calls with the MIDI it sends out (IMP-24), kept
    /// here so it outlives the render calls and drops before `midi_out`,
    /// which its closure points at; `None` while the unit names no MIDI
    /// output.
    _midi_out_block: Option<RcBlock<MidiOutFn>>,
    /// The events that block wrote, drained by the node after each render
    /// call.
    midi_out: Box<MidiOut>,
    /// Every parameter of the unit's tree, in order, with the value it had
    /// when the unit was made: a parameter change is one `setValue` on its
    /// own frame, and a `ParamValue::Default` event sets its captured
    /// value, never read from the unit on the audio thread.
    params: Vec<(Retained<AUParameter>, f64)>,
    /// Its parameters' automation and modulation, played into it (the
    /// modulation spec, R10).
    drive: Driver,
    /// The pull block the unit reads its input with: it points at [`pull`].
    ///
    /// [`pull`]: Self::pull
    pull_block: RcBlock<PullInput>,
    /// What the pull block reads: the block's input, planar, and which
    /// frames of it the next render call is for. Dropped after the block.
    pull: Box<Pull>,
    /// The unit's output, planar, filled by its render calls.
    output: Box<[f32]>,
    /// The output list handed to the unit, with room for its channels.
    list: Box<Output>,
    /// Channels the unit's input bus takes: 0 for an instrument.
    input_channels: usize,
    /// Channels its output bus gives.
    output_channels: usize,
    max_frames: usize,
}

/// The pull block's signature, as [`AURenderPullInputBlock`] has it.
type PullInput = dyn Fn(
    NonNull<AudioUnitRenderActionFlags>,
    NonNull<AudioTimeStamp>,
    u32,
    isize,
    NonNull<AudioBufferList>,
) -> AUAudioUnitStatus;

// SAFETY: the unit is activated for one node at a time and moves between the
// main thread and the audio thread through the node's queues, so only one
// thread reaches it at a time; the blocks and the buffers stay valid until
// the processor is dropped, after the unit stops rendering.
unsafe impl Send for Processor {}

/// What the unit's pull block reads.
struct Pull {
    /// Planar f32: `channels` rows of `max_frames`.
    data: Box<[f32]>,
    channels: usize,
    max_frames: usize,
    /// The frame within the block the next render call starts at.
    offset: usize,
}

/// An output list with room for [`MAX_CHANNELS`] non-interleaved buffers:
/// the ABI's `AudioBufferList` with extra buffers behind its one.
#[repr(C)]
struct Output {
    list: AudioBufferList,
    extra: [AudioBuffer; MAX_CHANNELS - 1],
}

impl Output {
    /// The buffers, as many as `count`, with the list's count set to match.
    fn buffers(&mut self, count: usize) -> &mut [AudioBuffer] {
        let count = count.min(MAX_CHANNELS);
        self.list.mNumberBuffers = u32::try_from(count).unwrap_or(0);
        // SAFETY: `list` and `extra` are laid out as an AudioBufferList with
        // room for MAX_CHANNELS buffers in one allocation (`#[repr(C)]`, so
        // `extra` follows the list's single buffer without a gap), and the
        // returned slice stops at MAX_CHANNELS.
        unsafe {
            let all = std::slice::from_raw_parts_mut(
                (&raw mut self.list.mBuffers).cast::<AudioBuffer>(),
                MAX_CHANNELS,
            );
            &mut all[..count]
        }
    }
}

/// An event the unit is given, at its frame within the block.
#[derive(Clone, Copy, Debug)]
enum In {
    /// A parameter's new value, already resolved.
    Param(u32, f64),
    /// A note.
    Note(NoteEvent),
    /// A MIDI message for the unit's MIDI block: a note's polyphonic
    /// pressure with MPE Mode off, or MPE MIDI with it on (the MPE spec,
    /// R15, R17).
    Midi(MidiMessage),
}

/// An Audio Unit's node.
pub struct AuNode {
    processor: Option<Processor>,
    from_host: Consumer<Processor>,
    to_host: Producer<Processor>,
    status: Arc<Status>,
    failed: bool,
    /// Changes kept for the unit while it isn't running a node.
    pending: Vec<(u32, ParamValue)>,
    /// The automation and clip modulation the compiler sent.
    automation: Option<NodeData>,
    /// The block's events, in order, kept with the node's room.
    events: Vec<(u32, In)>,
    /// The device sits in the MIDI-effect position (IMP-24): its notes go
    /// on, its audio output is dropped, and the audio it receives passes
    /// through.
    midi_effect: bool,
    steady: u64,
    /// The device's MPE Mode and settings (the MPE spec, R14), as the
    /// compiler last sent them; `pending_mpe` waits for the next block.
    mpe: PluginMpe,
    pending_mpe: Option<PluginMpe>,
    /// MPE MIDI laid out for the unit's MIDI block (R17), made with the
    /// node.
    layout: MpeLayout,
    /// The notes it was given as plain notes, which MPE Mode turned on ends.
    plain: PlainNotes,
}

impl AuNode {
    /// A node for `config`, with the main thread's ends of its queues. With
    /// `midi_effect` the device is placed as a MIDI effect (IMP-24), so its
    /// audio output is dropped.
    pub(crate) fn new(_config: &NodeConfig, midi_effect: bool) -> (Self, HostEnds) {
        let (to_node, from_host) = RingBuffer::new(1);
        let (to_host, from_node) = RingBuffer::new(2);
        let status = Arc::new(Status::default());
        let node = Self {
            processor: None,
            from_host,
            to_host,
            status: Arc::clone(&status),
            failed: false,
            pending: Vec::with_capacity(PENDING_CAPACITY),
            automation: None,
            events: Vec::with_capacity(EVENT_CAPACITY),
            midi_effect,
            steady: 0,
            mpe: PluginMpe::default(),
            pending_mpe: None,
            layout: MpeLayout::new(PluginMpe::default().settings),
            plain: PlainNotes::default(),
        };
        let ends = HostEnds {
            to_node,
            from_node,
            status,
        };
        (node, ends)
    }

    /// Takes a processor the host sent, giving back any it replaces.
    fn receive(&mut self) {
        while let Ok(processor) = self.from_host.pop() {
            if let Some(old) = self.processor.replace(processor) {
                self.give_back(old);
            }
            self.failed = false;
            // Spec: mpe R17. A unit activated again is opened again.
            self.layout.reopen();
        }
        if self.status.hand_back.swap(false, Relaxed)
            && let Some(processor) = self.processor.take()
        {
            self.give_back(processor);
        }
    }

    /// Sends a processor to the main thread to deactivate.
    fn give_back(&mut self, processor: Processor) {
        // The queue has room for the processor the node had and one it
        // replaced; if it's full the processor is dropped, and the host
        // deactivates the unit once its render resources come back.
        let _ = self.to_host.push(processor);
    }

    /// Keeps a change for the unit when it next runs.
    fn keep(&mut self, param: u32, value: ParamValue) {
        if let Some(slot) = self.pending.iter_mut().find(|(id, _)| *id == param) {
            slot.1 = value;
        } else if self.pending.len() < self.pending.capacity() {
            self.pending.push((param, value));
        } else {
            self.status.dropped.fetch_add(1, Relaxed);
        }
    }

    /// Keeps a block's changes for the unit, as it isn't running one.
    fn keep_all(&mut self, block: &Block<'_>) {
        for event in block.events() {
            self.keep(event.param.0, event.value);
        }
    }

    fn fail(&mut self, block: &mut Block<'_>) {
        self.failed = true;
        self.status.failed.store(true, Relaxed);
        silence(block);
    }
}

fn silence(block: &mut Block<'_>) {
    for channel in block.channels_mut() {
        channel.fill(0.0);
    }
}

impl Node for AuNode {
    fn process(&mut self, block: &mut Block<'_>) {
        self.receive();
        let frames = block.frames();
        let Some(processor) = self.processor.as_mut() else {
            // Not running: keep the changes for it, and pass the input
            // through (or stay silent after a failure).
            self.keep_all(block);
            if self.failed || self.status.silent.load(Relaxed) {
                silence(block);
            }
            return;
        };
        if self.failed {
            self.keep_all(block);
            silence(block);
            return;
        }
        if frames == 0 {
            return;
        }
        processor.fill_input(block);
        let status = Arc::clone(&self.status);
        let list = self
            .automation
            .as_ref()
            .and_then(|data| data.downcast_ref::<AutomationList>());
        collect(
            &mut self.events,
            &self.pending,
            block,
            &status,
            processor,
            list,
            (
                &mut self.pending_mpe,
                &mut self.mpe,
                &mut self.layout,
                &mut self.plain,
            ),
        );
        self.pending.clear();

        // The unit renders one segment at a time: the events at an offset
        // are applied before the segment they start.
        let base = self.steady;
        let mut at = 0;
        let mut index = 0;
        let mut failed = false;
        while index < self.events.len() {
            let offset = (self.events[index].0 as usize).min(frames - 1);
            if offset > at {
                if processor.render(block, base, at, offset, self.midi_effect) != 0 {
                    failed = true;
                    break;
                }
                let dropped = processor.take_midi_out(block, at, offset);
                self.status.dropped.fetch_add(dropped, Relaxed);
            }
            while index < self.events.len() && self.events[index].0 as usize <= offset {
                match self.events[index].1 {
                    In::Param(param, value) => {
                        if !processor.set_param(param, value, self.status.observer.load(Relaxed)) {
                            self.status.dropped.fetch_add(1, Relaxed);
                        }
                    }
                    In::Note(note) => {
                        if !processor.midi(&note) {
                            self.status.dropped.fetch_add(1, Relaxed);
                        }
                    }
                    In::Midi(message) => {
                        if !processor.midi_message(message) {
                            self.status.dropped.fetch_add(1, Relaxed);
                        }
                    }
                }
                index += 1;
            }
            at = offset;
        }
        if !failed && at < frames {
            if processor.render(block, base, at, frames, self.midi_effect) != 0 {
                failed = true;
            } else {
                let dropped = processor.take_midi_out(block, at, frames);
                self.status.dropped.fetch_add(dropped, Relaxed);
            }
        }
        self.steady = self.steady.wrapping_add(frames as u64);
        if failed {
            self.fail(block);
        }
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        self.receive();
        // A unit that isn't running takes its changes when it's next
        // activated: they're kept here and set before its first block.
        for event in events {
            self.keep(event.param.0, event.value);
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        if data.downcast_ref::<AutomationList>().is_some() {
            self.automation = Some(data);
        } else if let Some(PluginMpeData(mpe)) = data.downcast_ref() {
            self.pending_mpe = Some(*mpe);
        }
    }

    fn modulated(&self, report: &mut dyn FnMut(ParamId, f64, f64)) {
        if let Some(processor) = &self.processor {
            processor.drive.modulated(report);
        }
    }

    fn latency(&self) -> u32 {
        self.status.latency.load(Relaxed)
    }
}

impl Drop for AuNode {
    fn drop(&mut self) {
        // The drop thread drops nodes: the processor goes back to the main
        // thread to be deactivated there.
        if let Some(processor) = self.processor.take() {
            self.give_back(processor);
        }
    }
}

/// The block's parameter changes and notes as events, the kept changes at
/// its first frame, in order of frame, and the values automation and
/// modulation give the parameters they drive (the modulation spec, R10),
/// which stand in for the changes of those parameters' controls. Past the
/// room, events are dropped and counted.
fn collect(
    events: &mut Vec<(u32, In)>,
    pending: &[(u32, ParamValue)],
    block: &Block<'_>,
    status: &Status,
    processor: &mut Processor,
    list: Option<&AutomationList>,
    (pending_mpe, mpe, layout, plain): (
        &mut Option<PluginMpe>,
        &mut PluginMpe,
        &mut MpeLayout,
        &mut PlainNotes,
    ),
) {
    events.clear();
    let capacity = events.capacity();
    let push = |events: &mut Vec<(u32, In)>, offset: u32, event: In| {
        if events.len() < capacity {
            events.push((offset, event));
        } else {
            status.dropped.fetch_add(1, Relaxed);
        }
    };
    processor.drive.prepare(list, block);
    let mut change = |events: &mut Vec<(u32, In)>, offset: u32, param: u32, value: ParamValue| {
        let value = match value {
            ParamValue::Set(value) => Some(value),
            ParamValue::Default => processor.default_of(param),
        };
        if let Some(value) = value {
            processor.drive.set_control(param, value);
            if !processor.drive.is_driven(param) {
                push(events, offset, In::Param(param, value));
            }
        }
    };
    for &(param, value) in pending {
        change(events, 0, param, value);
    }
    for event in block.events() {
        change(events, event.offset, event.param.0, event.value);
    }
    processor.drive.play(list, block, |play| {
        push(events, play.offset, In::Param(play.id, play.played));
    });
    // Spec: mpe R15, R16. The new MPE Mode and settings take effect before
    // the block's notes: the notes MPE MIDI laid out for the old ones ends.
    if let Some(new) = pending_mpe.take() {
        let mut sink = |offset: u32, message: MidiMessage| {
            push(events, offset, In::Midi(message));
        };
        if mpe.on != new.on {
            layout.release_all(0, &mut sink);
            plain.release_all(0, &mut sink);
        }
        layout.configure(new.settings, &mut sink);
        *mpe = new;
    }
    if mpe.on {
        // Spec: au-hosting R20. The notes and their expressions go in as
        // MPE MIDI, each message at its frame in the order MPE MIDI lays
        // them out.
        let mut sink = |offset: u32, message: MidiMessage| {
            push(events, offset, In::Midi(message));
        };
        layout.open(&mut sink);
        layout.lay_out(block.notes(), &mut sink);
    } else {
        for note in block.notes() {
            match note.kind {
                // Spec: au-hosting R19. A note's pressure is polyphonic key
                // pressure on its key and channel.
                NoteKind::Expression {
                    expression: NoteExpression::Pressure,
                    value,
                } => {
                    let pressure = (value.clamp(0.0, 1.0) * 127.0).round() as u8;
                    let status = 0xA0 | note.channel.min(15);
                    if let Some(message) =
                        MidiMessage::from_bytes(&[status, note.key.min(127), pressure])
                    {
                        push(events, note.offset, In::Midi(message));
                    }
                }
                NoteKind::Expression { .. } => {}
                _ => {
                    plain.track(note);
                    push(events, note.offset, In::Note(*note));
                }
            }
        }
    }
    // Nearly in order already: an insertion sort, which allocates nothing.
    for i in 1..events.len() {
        let mut j = i;
        while j > 0 && events[j - 1].0 > events[j].0 {
            events.swap(j - 1, j);
            j -= 1;
        }
    }
}

impl Processor {
    /// A processor for an activated unit: where it reads its input and
    /// writes its output, its blocks, and its parameters.
    pub(crate) fn new(
        unit: Retained<AUAudioUnit>,
        config: &NodeConfig,
        params: Vec<(Retained<AUParameter>, f64)>,
        drive: Driver,
        render: AURenderBlock,
        midi: AUScheduleMIDIEventBlock,
        (input_channels, output_channels): (usize, usize),
    ) -> Self {
        let max_frames = config.max_frames.max(1);
        let pull = Box::new(Pull {
            data: vec![0.0; MAX_CHANNELS * max_frames].into_boxed_slice(),
            channels: input_channels,
            max_frames,
            offset: 0,
        });
        let state: *const Pull = &raw const *pull;
        let pull_block = RcBlock::new(
            move |_flags: NonNull<AudioUnitRenderActionFlags>,
                  _stamp: NonNull<AudioTimeStamp>,
                  frames: u32,
                  _bus: isize,
                  list: NonNull<AudioBufferList>|
                  -> AUAudioUnitStatus {
                // SAFETY: the pointer is this processor's own pull state,
                // which outlives the block (both live in the processor, and
                // the block is declared before it, so it drops first). The
                // unit calls the block only during a render call, on the
                // audio thread, and one processor runs for one node.
                let state = unsafe { &*state };
                // SAFETY: the unit lends the block its input list for the
                // call; it holds at least one buffer.
                let list = unsafe { &mut *list.as_ptr() };
                let count = list.mNumberBuffers as usize;
                // SAFETY: the list is an AudioBufferList whose buffer count
                // the unit set, in one allocation.
                let buffers =
                    unsafe { std::slice::from_raw_parts_mut(list.mBuffers.as_mut_ptr(), count) };
                let frames = (frames as usize).min(state.max_frames.saturating_sub(state.offset));
                let size = u32::try_from(frames * size_of::<f32>()).unwrap_or(u32::MAX);
                for (index, buffer) in buffers.iter_mut().enumerate() {
                    let channel = index.min(state.channels.saturating_sub(1));
                    let start = channel * state.max_frames + state.offset;
                    let source: *mut f32 = state.data[start..start + frames].as_ptr().cast_mut();
                    if buffer.mData.is_null() {
                        buffer.mData = source.cast();
                    } else if state.channels > 0 {
                        // SAFETY: the buffer holds the frames its byte size
                        // says, of f32 samples, as the unit asked.
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                source,
                                buffer.mData.cast::<f32>(),
                                frames,
                            );
                        }
                    } else {
                        // SAFETY: the same room, silence.
                        unsafe { std::ptr::write_bytes(buffer.mData.cast::<f32>(), 0, frames) };
                    }
                    buffer.mDataByteSize = size;
                }
                0
            },
        );
        // The unit's MIDI output (IMP-24): the block it calls with the MIDI
        // it sends out, and the events it writes for the node to drain.
        let midi_out: Box<MidiOut> = Box::default();
        let state: *const MidiOut = &raw const *midi_out;
        let block: RcBlock<MidiOutFn> = RcBlock::new(
            move |time: i64, _cable: u8, length: isize, bytes: NonNull<u8>| -> i32 {
                // SAFETY: the pointer is this processor's own output state,
                // which outlives the block (both live in the processor, and
                // the block is declared before it, so it drops first). The
                // unit calls the block only during a render call, on the
                // audio thread, and one processor runs for one node.
                let state = unsafe { &*state };
                let count = state.count.get();
                if count >= MIDI_OUT_EVENTS {
                    state.dropped.set(state.dropped.get() + 1);
                    return 0;
                }
                let length = usize::try_from(length).unwrap_or(0).min(MIDI_OUT_BYTES);
                let mut event = MidiOutEvent {
                    time,
                    bytes: [0; MIDI_OUT_BYTES],
                    length,
                };
                if length > 0 {
                    // SAFETY: the unit lends `length` bytes for the call.
                    let source = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), length) };
                    event.bytes[..length].copy_from_slice(source);
                }
                // SAFETY: one render call at a time writes the events, and
                // the node drains them after it.
                unsafe { (*state.events.get())[count] = event };
                state.count.set(count + 1);
                0
            },
        );
        // SAFETY: reading the unit's own MIDI output names is a property
        // read on the thread that made it.
        let names_midi = unsafe { unit.MIDIOutputNames() }.count() > 0;
        // Only a unit that names a MIDI output takes the block.
        let midi_out_block = names_midi.then(|| {
            // SAFETY: the block is kept in the processor, which lives as
            // long as the unit is active; the unit copies it too.
            unsafe { unit.setMIDIOutputEventBlock(RcBlock::as_ptr(&block)) };
            block
        });
        Self {
            unit,
            render,
            midi,
            _midi_out_block: midi_out_block,
            midi_out,
            params,
            drive,
            pull_block,
            pull,
            output: vec![0.0; MAX_CHANNELS * max_frames].into_boxed_slice(),
            list: Box::new(Output {
                list: AudioBufferList {
                    mNumberBuffers: 0,
                    mBuffers: [AudioBuffer {
                        mNumberChannels: 1,
                        mDataByteSize: 0,
                        mData: std::ptr::null_mut(),
                    }],
                },
                extra: [AudioBuffer {
                    mNumberChannels: 1,
                    mDataByteSize: 0,
                    mData: std::ptr::null_mut(),
                }; MAX_CHANNELS - 1],
            }),
            input_channels,
            output_channels,
            max_frames,
        }
    }

    /// The unit's parameter `param`'s default: its value when the unit was
    /// made, before the set's own values were set on it.
    pub(crate) fn default_of(&self, param: u32) -> Option<f64> {
        self.params.get(param as usize).map(|(_, default)| *default)
    }

    /// Copies the block's input into what the unit's pull block reads: the
    /// block's channel of a plug-in channel's number, the last for more.
    pub(crate) fn fill_input(&mut self, block: &Block<'_>) {
        let frames = block.frames();
        let channels = block.channels();
        if self.input_channels == 0 || channels == 0 {
            return;
        }
        for channel in 0..self.input_channels {
            let source = block.channel(channel.min(channels - 1));
            let start = channel * self.max_frames;
            self.pull.data[start..start + frames].copy_from_slice(source);
        }
    }

    /// Sets a parameter, before the segment it starts. `originator` is the
    /// editor's parameter-observer token, so the host's own change isn't
    /// reported back as the unit's own; null while no editor is open.
    pub(crate) fn set_param(&mut self, param: u32, value: f64, originator: *mut c_void) -> bool {
        let Some((parameter, _)) = self.params.get(param as usize) else {
            return false;
        };
        // SAFETY: setting a value is a host call on the unit's own thread,
        // for a parameter of its tree, between render calls.
        if originator.is_null() {
            // SAFETY: as above.
            unsafe { parameter.setValue(value as f32) };
        } else {
            // SAFETY: as above; `originator` is the observer token while an
            // editor is open, and setValue:originator: only compares it.
            unsafe { parameter.setValue_originator(value as f32, originator) };
        }
        true
    }

    /// Schedules a MIDI message for the next render call's first frame.
    pub(crate) fn midi_message(&mut self, message: MidiMessage) -> bool {
        if self.midi.is_null() {
            return false;
        }
        let mut bytes = message.padded();
        let length = message.bytes().len() as isize;
        let data = NonNull::from(&mut bytes[0]);
        // SAFETY: the MIDI block belongs to this unit and takes the
        // message's valid bytes, which it copies before it returns; the
        // call is made between render calls, where Apple's documentation
        // allows it.
        unsafe {
            (&*self.midi).call((AUEventSampleTimeImmediate, 0, length, data));
        }
        true
    }

    /// Schedules a note for the next render call's first frame.
    pub(crate) fn midi(&mut self, note: &NoteEvent) -> bool {
        if self.midi.is_null() {
            return false;
        }
        // Spec: au-hosting R18. A channel message goes in as its MIDI bytes.
        if let Some(message) = note.message() {
            return self.midi_message(message);
        }
        let channel = note.channel.min(15);
        let key = note.key.min(127);
        let mut bytes = match note.kind {
            NoteKind::Expression { .. }
            | NoteKind::Control { .. }
            | NoteKind::PitchBend { .. }
            | NoteKind::ChannelPressure { .. } => return false,
            NoteKind::On => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped to 0..=127"
                )]
                let velocity = (f64::from(note.velocity.clamp(0.0, 1.0)) * 127.0).round() as u8;
                [0x90 | channel, key, velocity]
            }
            NoteKind::Off | NoteKind::Choke => [0x80 | channel, key, 0],
        };
        // SAFETY: the MIDI block belongs to this unit and takes three valid
        // MIDI bytes, which it copies before it returns; the call is made
        // between render calls, where Apple's documentation allows it.
        let data = NonNull::from(&mut bytes[0]);
        // SAFETY: as above: the MIDI block takes these three valid bytes and
        // copies them before it returns.
        unsafe {
            (&*self.midi).call((AUEventSampleTimeImmediate, 0, 3, data));
        }
        true
    }

    /// Sends on the notes the unit sent out during its render call for
    /// frames `start..end` of the block, each on the frame the unit gave,
    /// clamped into the call (IMP-24; au-hosting R4), and returns how many
    /// of its events were dropped for lack of room. Anything else the unit
    /// sent is dropped.
    pub(crate) fn take_midi_out(&mut self, block: &mut Block<'_>, start: usize, end: usize) -> u64 {
        let count = self.midi_out.count.get();
        for index in 0..count {
            // SAFETY: one render call at a time wrote the events, and this
            // drains them after it.
            let event = unsafe { (*self.midi_out.events.get())[index] };
            let bytes = &event.bytes[..event.length.min(MIDI_OUT_BYTES)];
            let Some(message) = MidiMessage::from_bytes(bytes) else {
                continue;
            };
            let at = start + usize::try_from(event.time.max(0)).unwrap_or(0);
            let at = u32::try_from(at.min(end.saturating_sub(1))).unwrap_or(0);
            if let Some(note) = NoteEvent::from_midi(at, &message) {
                block.send_note(note);
            }
        }
        self.midi_out.count.set(0);
        let dropped = self.midi_out.dropped.get();
        self.midi_out.dropped.set(0);
        dropped
    }

    /// Renders frames `start..end` of the block through the unit, writing
    /// its output back into the block. Returns the unit's status: 0 for
    /// rendered audio. With `midi_effect` the audio it receives passes
    /// through and its own output is dropped (IMP-24).
    pub(crate) fn render(
        &mut self,
        block: &mut Block<'_>,
        base: u64,
        start: usize,
        end: usize,
        midi_effect: bool,
    ) -> AUAudioUnitStatus {
        let frames = end - start;
        if frames == 0 {
            return 0;
        }
        // What the unit's pull block reads for this segment.
        self.pull.offset = start;
        // Its output buffers, at this segment's frames.
        let output = self.output.as_mut_ptr();
        for (channel, buffer) in self
            .list
            .buffers(self.output_channels)
            .iter_mut()
            .enumerate()
        {
            let begin = channel * self.max_frames + start;
            // SAFETY: the pointer is this processor's own output buffer,
            // alive until it is dropped; the unit renders into it during
            // this call only, over the frames the byte size says.
            let data = unsafe { output.add(begin) };
            // SAFETY: as above.
            unsafe { std::ptr::write_bytes(data, 0, frames) };
            buffer.mData = data.cast();
            buffer.mDataByteSize = u32::try_from(frames * size_of::<f32>()).unwrap_or(u32::MAX);
        }
        let mut flags = AudioUnitRenderActionFlags(0);
        // SAFETY: an AudioTimeStamp of zeroes is the "nothing valid" value;
        // every field is a plain number.
        let mut stamp = unsafe { std::mem::zeroed::<AudioTimeStamp>() };
        stamp.mSampleTime = (base + start as u64) as f64;
        stamp.mFlags = AudioTimeStampFlags::SampleTimeValid;
        let list = NonNull::from(&mut self.list.list).cast::<AudioBufferList>();
        let pull: AURenderPullInputBlock = if self.input_channels > 0 {
            RcBlock::as_ptr(&self.pull_block)
        } else {
            std::ptr::null_mut()
        };
        // SAFETY: the render block belongs to this unit, which is active;
        // the pointers are this processor's own buffers and block, valid
        // for the call, and the call runs on the audio thread.
        let status = unsafe {
            (&*self.render).call((
                NonNull::from(&mut flags),
                NonNull::from(&stamp),
                u32::try_from(frames).unwrap_or(u32::MAX),
                0,
                list,
                pull,
            ))
        };
        if status != 0 {
            return status;
        }
        let channels = block.channels();
        if midi_effect {
            // Spec: device-chains R3 (IMP-24). A unit placed as a MIDI
            // effect passes the audio it receives on, and its own audio
            // output is dropped.
            return 0;
        }
        if self.output_channels == 0 {
            silence(block);
            return 0;
        }
        for channel in 0..channels {
            let from = channel.min(self.output_channels - 1) * self.max_frames;
            let source = &self.output[from + start..from + end];
            block.channel_mut(channel)[start..end].copy_from_slice(source);
        }
        status
    }
}

impl Drop for Processor {
    fn drop(&mut self) {
        // The host deactivates the unit on the main thread when the
        // processor comes back; this is the fallback for one dropped with
        // its node or when a node's queue was full.
        // SAFETY: deallocating render resources stops the unit rendering;
        // it is called after any render call has returned.
        unsafe {
            if self.unit.renderResourcesAllocated() {
                self.unit.deallocateRenderResources();
            }
        }
    }
}

impl std::fmt::Debug for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Processor")
            .field("input_channels", &self.input_channels)
            .field("output_channels", &self.output_channels)
            .finish_non_exhaustive()
    }
}
