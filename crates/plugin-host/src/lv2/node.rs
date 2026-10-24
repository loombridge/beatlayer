//! An LV2 plug-in's node: what runs its instance on the audio thread.
//!
//! The node is made on the compiler thread without the plug-in: the main
//! thread makes the instance (`Lv2Host::idle`) and hands it over through a
//! one-slot queue, and the node gives back any instance it replaces, and the
//! one it holds when it is dropped, the same way, so lilv frees plug-ins on
//! the main thread. Until one arrives the node passes its input through;
//! after the plug-in fails a call it outputs silence. Every buffer, event
//! list and queue is made with the node, so nothing on the audio thread
//! allocates, locks, or blocks (the real-time rules); the plug-in's own code
//! is its own.
//!
//! LV2 reads a control port once a block, so a block is run in parts, split
//! at each frame a parameter changes on (spec: lv2-hosting R3).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

use bl_core::time::Frames;
use bl_engine::{
    AutomationList, Block, Node, NodeData, NodeEvent, NoteEvent, NoteKind, ParamValue,
};
use bl_model::ParamId;
use rtrb::{Consumer, Producer, RingBuffer};

use super::describe::ParamMap;
use super::{Instance, MidiEvent, Transport};
use crate::drive::Driver;

/// The most audio channels the node feeds each way.
pub(crate) const MAX_CHANNELS: usize = 8;
/// How many MIDI events a block holds each way, and how many parameter
/// changes.
const MAX_EVENTS: usize = 1024;
/// How many MIDI events a block holds: fewer than fit an Atom port with
/// the position, so a block's MIDI is never refused.
const MAX_MIDI: usize = 512;
/// How many parameter changes made while the plug-in isn't running are kept
/// for it; past that they're dropped and counted.
const PENDING_CAPACITY: usize = 512;
/// How many values the plug-in's editor can queue for the node at a time.
const EDITOR_CAPACITY: usize = 256;

/// What a node and the main thread both see.
#[derive(Debug, Default)]
pub(crate) struct Status {
    /// The latency the plug-in reported in its last block.
    pub(crate) latency: AtomicU32,
    /// The plug-in failed a call on the audio thread.
    pub(crate) failed: AtomicBool,
    /// Changes dropped because a queue was full.
    pub(crate) dropped: AtomicU64,
}

/// The main thread's ends of a node's queues.
pub(crate) struct HostEnds {
    pub(crate) to_node: Producer<Active>,
    pub(crate) from_node: Consumer<Active>,
    /// Values the plug-in's editor wrote to control ports, for the node to
    /// give the plug-in at the first frame of its next block.
    pub(crate) from_editor: Producer<(u32, f32)>,
    pub(crate) status: Arc<Status>,
}

/// The plug-in as the main thread made it for a node: the instance, how
/// its audio ports map to the block's channels, and its parameters.
pub(crate) struct Active {
    pub(crate) instance: Instance,
    /// Each audio input port, in port order: the block channel it reads, or
    /// none for a side-chain input, which hears silence.
    pub(crate) audio_in: Vec<Option<usize>>,
    /// How many audio output ports are the plug-in's main ones.
    pub(crate) audio_out: usize,
    /// Whether an Atom input takes `time:Position`.
    pub(crate) position: bool,
    /// Its parameters, for the device-to-port conversion.
    pub(crate) params: Vec<ParamMap>,
    /// Its parameters' automation and modulation, played into it (the
    /// modulation spec, R10).
    pub(crate) drive: Driver,
}

/// A control port's new value, `offset` frames into the block.
#[derive(Clone, Copy)]
struct Change {
    offset: u32,
    port: u32,
    value: f32,
}

/// An LV2 plug-in's node.
pub struct Lv2Node {
    active: Option<Active>,
    from_host: Consumer<Active>,
    to_host: Producer<Active>,
    from_editor: Consumer<(u32, f32)>,
    status: Arc<Status>,
    /// It failed while processing: it's never called again and the node is
    /// silent.
    dead: bool,
    /// Changes kept for the plug-in while it isn't running, by port.
    pending: Vec<(u32, ParamValue)>,
    /// The automation and clip modulation the compiler sent.
    automation: Option<NodeData>,
    /// The device sits in the MIDI-effect position (IMP-24): its notes go
    /// on, its audio output is dropped, and the audio it receives passes
    /// through.
    midi_effect: bool,
    /// The block's channels, in lanes of `max_frames`.
    ins: Vec<f32>,
    outs: Vec<f32>,
    zeros: Vec<f32>,
    changes: Vec<Change>,
    midi_in: Vec<MidiEvent>,
    midi_part: Vec<MidiEvent>,
    max_frames: usize,
}

impl Lv2Node {
    /// A node for `config`, with the main thread's ends of its queues. The
    /// plug-in's `latency` is the last one it reported, if the host knows
    /// it, so a node made after a restart compensates at once. With
    /// `midi_effect` the device is placed as a MIDI effect (IMP-24).
    pub(crate) fn new(
        config: &bl_engine::NodeConfig,
        latency: u32,
        midi_effect: bool,
    ) -> (Self, HostEnds) {
        let (to_node, from_host) = RingBuffer::new(2);
        let (to_host, from_node) = RingBuffer::new(4);
        let (to_node_editor, from_editor) = RingBuffer::new(EDITOR_CAPACITY);
        let status = Arc::new(Status::default());
        status.latency.store(latency, Relaxed);
        let max_frames = config.max_frames.max(1);
        let node = Self {
            active: None,
            from_host,
            to_host,
            from_editor,
            status: Arc::clone(&status),
            dead: false,
            pending: Vec::with_capacity(PENDING_CAPACITY),
            automation: None,
            midi_effect,
            ins: vec![0.0; MAX_CHANNELS * max_frames],
            outs: vec![0.0; MAX_CHANNELS * max_frames],
            zeros: vec![0.0; max_frames],
            changes: Vec::with_capacity(MAX_EVENTS),
            midi_in: Vec::with_capacity(MAX_MIDI),
            midi_part: Vec::with_capacity(MAX_MIDI),
            max_frames,
        };
        let ends = HostEnds {
            to_node,
            from_node,
            from_editor: to_node_editor,
            status,
        };
        (node, ends)
    }

    /// Takes an instance the host sent, giving back any it replaces.
    fn receive(&mut self) {
        while let Ok(active) = self.from_host.pop() {
            // The old instance goes to the main thread to be freed there;
            // if the queue is full it is dropped here, which frees it where
            // it is.
            if let Some(old) = self.active.replace(active) {
                let _ = self.to_host.push(old);
            }
            self.dead = false;
            if let Some(active) = self.active.as_ref() {
                self.status.latency.store(active.instance.latency(), Relaxed);
            }
        }
    }

    /// Keeps a change for the plug-in when it next runs.
    fn keep(&mut self, port: u32, value: ParamValue) {
        if let Some(slot) = self.pending.iter_mut().find(|(p, _)| *p == port) {
            slot.1 = value;
        } else if self.pending.len() < self.pending.capacity() {
            self.pending.push((port, value));
        } else {
            self.status.dropped.fetch_add(1, Relaxed);
        }
    }

    fn fail(&mut self, block: &mut Block<'_>) {
        self.dead = true;
        self.status.failed.store(true, Relaxed);
        silence(block);
    }
}

// SAFETY: the node's `Active` holds a plug-in instance, which LV2 lets any
// one thread run at a time: the compiler thread makes the node, the audio
// thread runs it, and the main thread's ends are the queues.
unsafe impl Send for Lv2Node {}

fn silence(block: &mut Block<'_>) {
    for channel in block.channels_mut() {
        channel.fill(0.0);
    }
}

fn map_of(params: &[ParamMap], port: u32) -> Option<&ParamMap> {
    params.iter().find(|map| map.port == port)
}

/// The MIDI message a note event is, as the three bytes LV2 takes; none for
/// an expression, which waits for MPE hosting.
fn midi_of(note: &NoteEvent) -> Option<MidiEvent> {
    let seven = |velocity: f32| (velocity.clamp(0.0, 1.0) * 127.0).round() as u8;
    let channel = note.channel & 0x0F;
    let data = if let Some(message) = note.message() {
        let len = message.bytes().len();
        return Some(MidiEvent {
            frame: note.offset,
            len: len as u8,
            data: message.padded(),
        });
    } else {
        match note.kind {
            // A note-on at velocity 0 would be a note-off.
            NoteKind::On => [0x90 | channel, note.key & 0x7F, seven(note.velocity).max(1)],
            NoteKind::Off => [0x80 | channel, note.key & 0x7F, seven(note.velocity)],
            NoteKind::Choke => [0x80 | channel, note.key & 0x7F, 0],
            _ => return None,
        }
    };
    Some(MidiEvent {
        frame: note.offset,
        len: 3,
        data,
    })
}

/// The note a MIDI message the plug-in sent is, if it is a note.
fn note_out(event: &MidiEvent, offset: u32) -> Option<NoteEvent> {
    if event.len != 3 {
        return None;
    }
    let kind = match event.data[0] & 0xF0 {
        0x90 if event.data[2] > 0 => NoteKind::On,
        0x80 | 0x90 => NoteKind::Off,
        _ => return None,
    };
    Some(NoteEvent {
        offset,
        kind,
        channel: event.data[0] & 0x0F,
        key: event.data[1] & 0x7F,
        velocity: f32::from(event.data[2] & 0x7F) / 127.0,
        note_id: None,
    })
}

/// Where the song is at `frame` of the block, as LV2's `time:Position`.
fn transport_at(block: &Block<'_>, frame: usize) -> Transport {
    let info = block.transport();
    let rate = block.sample_rate();
    let position = Frames(info.position.0 + frame as i64);
    let tempo = block.tempo();
    let ticks = tempo.frames_to_ticks(position, rate);
    let signature = block.signatures().signature_at(ticks);
    let (bar, into_bar) = block.signatures().bar_at(ticks);
    let beat = signature.beat_length().0.max(1) as f64;
    // The tempo counts quarter notes; LV2's beats are the signature's.
    let per_quarter = f64::from(signature.denominator()) / 4.0;
    Transport {
        frame: position.0,
        playing: info.playing,
        bpm: (tempo.bpm_at(ticks).get() * per_quarter) as f32,
        bar,
        bar_beat: (into_bar.0 as f64 / beat) as f32,
        beats_per_bar: f32::from(signature.numerator()),
        beat_unit: u32::from(signature.denominator()),
    }
}

impl Node for Lv2Node {
    fn process(&mut self, block: &mut Block<'_>) {
        self.receive();
        let frames = block.frames();
        if self.active.is_none() || self.dead {
            // Not running: keep the changes for it, and pass the input
            // through (or stay silent after a failure).
            for event in block.events() {
                self.keep(event.param.0, event.value);
            }
            if self.dead {
                silence(block);
            }
            return;
        }
        let stride = self.max_frames;
        let active = self.active.as_mut().expect("the check above");

        // The input channels, into the node's lanes.
        let block_channels = block.channels();
        let wanted = active
            .audio_in
            .iter()
            .flatten()
            .map(|c| c + 1)
            .max()
            .unwrap_or(0)
            .min(MAX_CHANNELS);
        if block_channels > 0 {
            for lane in 0..wanted {
                let from = block.channel(lane.min(block_channels - 1));
                self.ins[lane * stride..lane * stride + frames].copy_from_slice(from);
            }
        }

        // The changes to the plug-in's control ports, on their frames: the
        // values automation and modulation give the parameters they drive;
        // the values kept while it wasn't running, then the block's own,
        // those of driven parameters only moving their controls; and what its
        // editor wrote.
        self.changes.clear();
        {
            let list = self
                .automation
                .as_ref()
                .and_then(|data| data.downcast_ref::<AutomationList>());
            let Active { params, drive, .. } = &mut *active;
            let params: &[ParamMap] = params;
            let plain_of = |map: &ParamMap, value: ParamValue| match value {
                ParamValue::Set(value) => value,
                ParamValue::Default => map.default,
            };
            drive.prepare(list, block);
            for &(port, value) in &self.pending {
                if let Some(map) = map_of(params, port) {
                    drive.set_control(port, plain_of(map, value));
                }
            }
            for event in block.events() {
                if let Some(map) = map_of(params, event.param.0) {
                    drive.set_control(event.param.0, plain_of(map, event.value));
                }
            }
            let changes = &mut self.changes;
            let dropped = &self.status.dropped;
            let mut push = |change: Change| {
                if changes.len() < changes.capacity() {
                    changes.push(change);
                } else {
                    dropped.fetch_add(1, Relaxed);
                }
            };
            drive.play(list, block, |play| {
                if let Some(map) = map_of(params, play.id) {
                    push(Change {
                        offset: play.offset,
                        port: play.id,
                        value: map.to_port(play.played),
                    });
                }
            });
            for &(port, value) in &self.pending {
                if let Some(map) = map_of(params, port)
                    && !drive.is_driven(port)
                {
                    push(Change {
                        offset: 0,
                        port,
                        value: map.to_port(plain_of(map, value)),
                    });
                }
            }
            self.pending.clear();
            for event in block.events() {
                if let Some(map) = map_of(params, event.param.0)
                    && !drive.is_driven(event.param.0)
                {
                    push(Change {
                        offset: event.offset,
                        port: event.param.0,
                        value: map.to_port(plain_of(map, event.value)),
                    });
                }
            }
            while let Ok((port, value)) = self.from_editor.pop() {
                push(Change {
                    offset: 0,
                    port,
                    value,
                });
            }
        }
        // Into order of frame, keeping the order of changes at one frame: an
        // insertion sort, which allocates nothing.
        for i in 1..self.changes.len() {
            let mut j = i;
            while j > 0 && self.changes[j - 1].offset > self.changes[j].offset {
                self.changes.swap(j - 1, j);
                j -= 1;
            }
        }

        // The block's notes and channel messages, as MIDI.
        self.midi_in.clear();
        for note in block.notes() {
            let Some(event) = midi_of(note) else { continue };
            if self.midi_in.len() < self.midi_in.capacity() {
                self.midi_in.push(event);
            } else {
                self.status.dropped.fetch_add(1, Relaxed);
            }
        }

        // The block in parts, between the frames a parameter changes on.
        let (mut start, mut next) = (0usize, 0usize);
        let mut failed = false;
        while start < frames {
            while next < self.changes.len() && self.changes[next].offset as usize <= start {
                let change = self.changes[next];
                active.instance.set_control(change.port, change.value);
                next += 1;
            }
            let end = self
                .changes
                .get(next)
                .map_or(frames, |c| (c.offset as usize).min(frames))
                .max(start + 1);
            self.midi_part.clear();
            for event in &self.midi_in {
                let frame = event.frame as usize;
                if (start..end).contains(&frame) {
                    self.midi_part.push(MidiEvent {
                        frame: (frame - start) as u32,
                        ..*event
                    });
                }
            }
            let mut inputs: [&[f32]; MAX_CHANNELS] = [&[]; MAX_CHANNELS];
            let mut input_count = 0;
            for channel in active.audio_in.iter().take(MAX_CHANNELS) {
                inputs[input_count] = match channel {
                    Some(c) => &self.ins[c * stride + start..c * stride + end],
                    None => &self.zeros[start..end],
                };
                input_count += 1;
            }
            let mut outputs: [&mut [f32]; MAX_CHANNELS] = Default::default();
            for (lane, out) in self
                .outs
                .chunks_mut(stride)
                .take(active.audio_out.min(MAX_CHANNELS))
                .enumerate()
            {
                outputs[lane] = &mut out[start..end];
            }
            if active.position {
                active
                    .instance
                    .set_transport(Some(transport_at(block, start)));
            }
            let run = active.instance.run(
                end - start,
                &inputs[..input_count],
                &mut outputs[..active.audio_out.min(MAX_CHANNELS)],
                &self.midi_part,
            );
            if run.is_err() {
                failed = true;
                break;
            }
            for event in active.instance.midi_out() {
                if let Some(note) = note_out(&event, event.frame + start as u32) {
                    block.send_note(note);
                }
            }
            start = end;
        }
        self.status
            .latency
            .store(active.instance.latency(), Relaxed);
        if failed {
            self.fail(block);
            return;
        }

        // The audio: the plug-in's main outputs back into the block. A
        // plug-in placed as a MIDI effect passes the audio it receives on,
        // and its own output is dropped (IMP-24).
        if !self.midi_effect {
            let outs = active.audio_out.min(MAX_CHANNELS);
            for channel in 0..block_channels {
                let out = block.channel_mut(channel);
                if outs == 0 {
                    out.fill(0.0);
                } else {
                    let from = channel.min(outs - 1) * stride;
                    out.copy_from_slice(&self.outs[from..from + frames]);
                }
            }
        }
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        self.receive();
        // LV2 has no flush call: the changes wait for the plug-in's next
        // block, at its first frame.
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
        if let Some(active) = &self.active {
            active.drive.modulated(report);
        }
    }

    fn latency(&self) -> u32 {
        self.status.latency.load(Relaxed)
    }
}

impl Drop for Lv2Node {
    fn drop(&mut self) {
        // The drop thread drops nodes: the instance goes back to the main
        // thread to be freed there.
        if let Some(active) = self.active.take() {
            let _ = self.to_host.push(active);
        }
    }
}
