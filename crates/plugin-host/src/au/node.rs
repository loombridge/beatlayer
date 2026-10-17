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

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering::Relaxed};

use bl_engine::{
    AutomationList, Block, Node, NodeConfig, NodeData, NodeEvent, NoteEvent, NoteKind, ParamValue,
};
use bl_model::ParamId;
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
    steady: u64,
}

impl AuNode {
    /// A node for `config`, with the main thread's ends of its queues.
    pub(crate) fn new(_config: &NodeConfig) -> (Self, HostEnds) {
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
            steady: 0,
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
            if self.failed {
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
            if offset > at && processor.render(block, base, at, offset) != 0 {
                failed = true;
                break;
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
                }
                index += 1;
            }
            at = offset;
        }
        if !failed && at < frames && processor.render(block, base, at, frames) != 0 {
            failed = true;
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
    let transport = block.transport();
    processor.drive.prepare(list, transport);
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
    processor
        .drive
        .play(list, transport, block.frames(), |play| {
            push(events, play.offset, In::Param(play.id, play.played));
        });
    for note in block.notes() {
        push(events, note.offset, In::Note(*note));
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
        Self {
            unit,
            render,
            midi,
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

    /// Schedules a note for the next render call's first frame.
    pub(crate) fn midi(&mut self, note: &NoteEvent) -> bool {
        if self.midi.is_null() {
            return false;
        }
        // Spec: au-hosting R18. A channel message goes in as its MIDI bytes.
        if let Some(message) = note.message() {
            let mut bytes = message.padded();
            let length = message.bytes().len() as isize;
            let data = NonNull::from(&mut bytes[0]);
            // SAFETY: the MIDI block belongs to this unit and takes the
            // message's valid bytes, which it copies before it returns; the
            // call is made between render calls, where Apple's
            // documentation allows it.
            unsafe {
                (&*self.midi).call((AUEventSampleTimeImmediate, 0, length, data));
            }
            return true;
        }
        let channel = note.channel.min(15);
        let key = note.key.min(127);
        let mut bytes = match note.kind {
            // Per-note expressions wait for MPE hosting (the parity
            // matrix's note): a plug-in's note port takes them then.
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

    /// Renders frames `start..end` of the block through the unit, writing
    /// its output back into the block. Returns the unit's status: 0 for
    /// rendered audio.
    pub(crate) fn render(
        &mut self,
        block: &mut Block<'_>,
        base: u64,
        start: usize,
        end: usize,
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
