//! A CLAP plug-in's node: what runs it on the audio thread.
//!
//! The node is made on the compiler thread without the plug-in: the main
//! thread activates the plug-in ([`ClapHost::idle`](super::ClapHost::idle))
//! and hands its audio processor over through a one-slot queue, and the
//! node hands it back the same way when the host asks (to restart it) or
//! when the node is dropped. Until a processor arrives the node passes its
//! input through, as a device no factory makes does; after the plug-in
//! fails a call it outputs silence. Every buffer and event list is made
//! with the node, so nothing on the audio thread allocates, locks, or
//! blocks (the real-time rules); the plug-in's own code is its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

use bl_core::time::TICKS_PER_QUARTER;
use bl_engine::{AutomationList, Block, Node, NodeConfig, NodeData, NodeEvent, ParamValue};
use bl_model::ParamId;
use clack_host::events::EventFlags;
use clack_host::events::event_types::{TransportEvent, TransportFlags};
use clack_host::prelude::*;
use clack_host::utils::{BeatTime, SecondsTime};
use rtrb::{Consumer, Producer, RingBuffer};

use super::events::{InputList, Out, OutputList};
use super::handlers::Host;

/// The most channels a plug-in's main port has that Beatlayer feeds.
pub(crate) const MAX_CHANNELS: usize = 8;

/// How many parameter changes made while the plug-in isn't running a node
/// keeps for it; past that they're dropped and counted.
const PENDING_CAPACITY: usize = 512;

/// How many of the plug-in's own parameter changes wait for the main
/// thread.
const PARAMS_OUT_CAPACITY: usize = 1024;

pub(crate) type Processor = PluginAudioProcessor<Host>;

/// A parameter change the plug-in made itself, such as from its editor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ParamOut {
    /// A new value.
    Value(u32, f64),
    /// A gesture began.
    Begin(u32),
    /// It ended.
    End(u32),
}

/// What a node and the main thread both see.
#[derive(Debug, Default)]
pub(crate) struct Status {
    /// The latency the plug-in reported when it was last activated.
    pub(crate) latency: AtomicU32,
    /// The host wants the processor back.
    pub(crate) hand_back: AtomicBool,
    /// The plug-in failed a call on the audio thread.
    pub(crate) failed: AtomicBool,
    /// The host says the device is silent while the node has no plug-in:
    /// its plug-in failed elsewhere, in an offline render (WP-2.6b).
    pub(crate) silent: AtomicBool,
    /// Events dropped because a list was full.
    pub(crate) dropped: AtomicU64,
}

/// The main thread's ends of a node's queues.
pub(crate) struct HostEnds {
    pub(crate) to_node: Producer<Processor>,
    pub(crate) from_node: Consumer<Processor>,
    pub(crate) params: Consumer<ParamOut>,
    pub(crate) status: Arc<Status>,
}

/// A CLAP plug-in's node.
pub struct ClapNode {
    processor: Option<Processor>,
    from_host: Consumer<Processor>,
    to_host: Producer<Processor>,
    params_out: Producer<ParamOut>,
    status: Arc<Status>,
    failed: bool,
    pending: Vec<(u32, ParamValue)>,
    /// The automation and clip modulation the compiler sent (the modulation
    /// spec, R10).
    automation: Option<NodeData>,
    input: InputList,
    output: OutputList,
    ins: Vec<f32>,
    outs: Vec<f32>,
    in_ports: AudioPorts,
    out_ports: AudioPorts,
    max_frames: usize,
    steady: u64,
}

impl ClapNode {
    /// A node for `config`, with the main thread's ends of its queues.
    pub(crate) fn new(config: &NodeConfig) -> (Self, HostEnds) {
        let (to_node, from_host) = RingBuffer::new(1);
        let (to_host, from_node) = RingBuffer::new(2);
        let (params_out, params) = RingBuffer::new(PARAMS_OUT_CAPACITY);
        let status = Arc::new(Status::default());
        let max_frames = config.max_frames.max(1);
        let node = Self {
            processor: None,
            from_host,
            to_host,
            params_out,
            status: Arc::clone(&status),
            failed: false,
            pending: Vec::with_capacity(PENDING_CAPACITY),
            automation: None,
            input: InputList::new(),
            output: OutputList::new(),
            ins: vec![0.0; MAX_CHANNELS * max_frames],
            outs: vec![0.0; MAX_CHANNELS * max_frames],
            in_ports: AudioPorts::with_capacity(MAX_CHANNELS, 1),
            out_ports: AudioPorts::with_capacity(MAX_CHANNELS, 1),
            max_frames,
            steady: 0,
        };
        let ends = HostEnds {
            to_node,
            from_node,
            params,
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

    /// Stops a processor and sends it to the main thread to deactivate.
    fn give_back(&mut self, mut processor: Processor) {
        processor.ensure_processing_stopped();
        // The queue has room for the processor the node had and one it
        // replaced; if it's full the processor is dropped, and the host
        // deactivates the plug-in once it's gone.
        let _ = self.to_host.push(processor);
    }

    /// Keeps a change for the plug-in when it next runs.
    fn keep(&mut self, param: u32, value: ParamValue) {
        if let Some(slot) = self.pending.iter_mut().find(|(id, _)| *id == param) {
            slot.1 = value;
        } else if self.pending.len() < self.pending.capacity() {
            self.pending.push((param, value));
        } else {
            self.status.dropped.fetch_add(1, Relaxed);
        }
    }

    /// Sends the plug-in's own events on: notes into the block, parameter
    /// changes to the main thread.
    fn send_out(&mut self, block: Option<&mut Block<'_>>) {
        let mut block = block;
        let drive = self.processor.as_ref();
        for event in &self.output.events {
            match *event {
                Out::Note(note) => {
                    if let Some(block) = block.as_deref_mut() {
                        block.send_note(note);
                    }
                }
                Out::Param(id, value) => {
                    // Spec: modulation R10. A value the plug-in reports back
                    // because the engine just sent it, as automation or
                    // modulation, is neither an undo step nor an edit.
                    if drive.is_some_and(|p| p.access_handler(|d| d.drive.is_echo(id, value))) {
                        continue;
                    }
                    if self.params_out.push(ParamOut::Value(id, value)).is_err() {
                        self.status.dropped.fetch_add(1, Relaxed);
                    }
                }
                Out::Begin(id) => {
                    let _ = self.params_out.push(ParamOut::Begin(id));
                }
                Out::End(id) => {
                    let _ = self.params_out.push(ParamOut::End(id));
                }
            }
        }
        self.output.events.clear();
        let dropped = self.input.dropped + self.output.dropped;
        if dropped > 0 {
            self.status.dropped.fetch_add(dropped, Relaxed);
            self.input.dropped = 0;
            self.output.dropped = 0;
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

/// The block's transport for the plug-in, as of its first frame.
fn transport(block: &Block<'_>) -> TransportEvent {
    let info = block.transport();
    let rate = block.sample_rate();
    let tempo = block.tempo();
    let ticks = tempo.frames_to_ticks(info.position, rate);
    let beats = ticks.0 as f64 / TICKS_PER_QUARTER as f64;
    let seconds = tempo.ticks_to_seconds(ticks);
    let mut flags = TransportFlags::HAS_TEMPO
        | TransportFlags::HAS_BEATS_TIMELINE
        | TransportFlags::HAS_SECONDS_TIMELINE;
    if info.playing {
        flags |= TransportFlags::IS_PLAYING;
    }
    TransportEvent {
        header: EventHeader::new_core(0, EventFlags::empty()),
        flags,
        song_pos_beats: BeatTime::from_float(beats),
        song_pos_seconds: SecondsTime::from_float(seconds),
        tempo: tempo.bpm_at(ticks).get(),
        tempo_inc: 0.0,
        loop_start_beats: BeatTime::from_int(0),
        loop_end_beats: BeatTime::from_int(0),
        loop_start_seconds: SecondsTime::from_int(0),
        loop_end_seconds: SecondsTime::from_int(0),
        bar_start: BeatTime::from_int(0),
        bar_number: 0,
        time_signature_numerator: 4,
        time_signature_denominator: 4,
    }
}

impl Node for ClapNode {
    fn process(&mut self, block: &mut Block<'_>) {
        self.receive();
        let frames = block.frames();
        let Some(processor) = self.processor.as_mut() else {
            // Not running: keep the changes for it, and pass the input
            // through (or stay silent after a failure).
            for event in block.events() {
                self.keep(event.param.0, event.value);
            }
            if self.failed || self.status.silent.load(Relaxed) {
                silence(block);
            }
            return;
        };
        if self.failed {
            for event in block.events() {
                self.keep(event.param.0, event.value);
            }
            silence(block);
            return;
        }
        let Ok(started) = processor.ensure_processing_started() else {
            self.fail(block);
            return;
        };
        let (inputs, outputs) = started.access_handler(|d| (d.inputs, d.outputs));
        // The events: changes kept while it wasn't running, at the first
        // frame, then the block's changes and notes on their frames, and
        // the values automation and modulation give the parameters they
        // drive (the modulation spec, R10), which stand in for the changes
        // of those parameters' controls.
        self.input.clear();
        let list = self
            .automation
            .as_ref()
            .and_then(|data| data.downcast_ref::<AutomationList>());
        let (input, pending) = (&mut self.input, &self.pending);
        started.access_handler_mut(|d| {
            d.drive.prepare(list, block);
            for &(id, value) in pending {
                if let Some(value) = d.resolve_undriven(id, value) {
                    input.param(0, id, value);
                }
            }
            for event in block.events() {
                if let Some(value) = d.resolve_undriven(event.param.0, event.value) {
                    input.param(event.offset, event.param.0, value);
                }
            }
            d.drive.play(list, block, |play| {
                if play.modulatable {
                    input.param(play.offset, play.id, play.base);
                    input.param_mod(play.offset, play.id, play.played - play.base);
                } else {
                    input.param(play.offset, play.id, play.played);
                }
            });
        });
        self.pending.clear();
        for note in block.notes() {
            self.input.note(note);
        }
        self.input.sort();

        // The audio: the block's channels in, each of the plug-in's
        // channels taking the block's channel of its number (the last one
        // for more), and its output back into the block the same way.
        let channels = block.channels();
        let stride = self.max_frames;
        for i in 0..inputs {
            if channels == 0 {
                break;
            }
            let from = block.channel(i.min(channels - 1));
            self.ins[i * stride..i * stride + frames].copy_from_slice(from);
        }
        let ins = self.in_ports.with_input_buffers((inputs > 0).then(|| {
            AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(
                    self.ins
                        .chunks_exact_mut(stride)
                        .take(inputs)
                        .map(|c| InputChannel::variable(&mut c[..frames])),
                ),
            }
        }));
        let mut outs = self.out_ports.with_output_buffers((outputs > 0).then(|| {
            AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_output_only(
                    self.outs
                        .chunks_exact_mut(stride)
                        .take(outputs)
                        .map(|c| &mut c[..frames]),
                ),
            }
        }));
        let transport = transport(block);
        let result = started.process(
            &ins,
            &mut outs,
            &InputEvents::from_buffer(&self.input),
            &mut OutputEvents::from_buffer(&mut self.output),
            Some(self.steady),
            Some(&transport),
        );
        self.steady = self.steady.wrapping_add(frames as u64);
        if result.is_err() {
            self.output.events.clear();
            self.fail(block);
            return;
        }
        for c in 0..channels {
            let out = block.channel_mut(c);
            if outputs == 0 {
                out.fill(0.0);
            } else {
                let from = c.min(outputs - 1) * stride;
                out.copy_from_slice(&self.outs[from..from + frames]);
            }
        }
        self.send_out(Some(block));
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        self.receive();
        let Some(processor) = self.processor.as_mut().filter(|_| !self.failed) else {
            for event in events {
                self.keep(event.param.0, event.value);
            }
            return;
        };
        let params = processor.access_handler(|d| d.params);
        self.input.clear();
        for event in events {
            if let Some(value) =
                processor.access_handler_mut(|d| d.resolve(event.param.0, event.value))
            {
                self.input.param(0, event.param.0, value);
            }
        }
        if let Some(params) = params {
            params.flush_active(
                &mut processor.plugin_handle(),
                &InputEvents::from_buffer(&self.input),
                &mut OutputEvents::from_buffer(&mut self.output),
            );
        }
        self.send_out(None);
    }

    fn receive_data(&mut self, data: NodeData) {
        if data.downcast_ref::<AutomationList>().is_some() {
            self.automation = Some(data);
        }
    }

    fn modulated(&self, report: &mut dyn FnMut(ParamId, f64, f64)) {
        if let Some(processor) = &self.processor {
            processor.access_handler(|d| d.drive.modulated(report));
        }
    }

    fn latency(&self) -> u32 {
        self.status.latency.load(Relaxed)
    }
}

impl Drop for ClapNode {
    fn drop(&mut self) {
        // The drop thread drops nodes: the processor goes back to the main
        // thread to be deactivated there.
        if let Some(processor) = self.processor.take() {
            self.give_back(processor);
        }
    }
}
