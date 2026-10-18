//! The engine program: what the compiler makes from a document and the audio
//! thread runs.

use core::fmt;
use core::ops::Range;

use bl_core::time::{Bpm, SampleRate, TempoMap, TimeSignature, TimeSignatureMap};
use bl_model::{Channel, LaunchMode, ParamId, SceneId, Tap, TrackId};

use crate::midi::{MidiRoutes, TrackMidiInput};
use crate::mixer::{MeterReading, Route, TrackMixer};
use crate::modbus::{Bus, Control, ROWS};
use crate::node::{
    Block, Node, NodeData, NodeEvent, NodeKey, NoteBuffer, NoteEvent, NoteKind, ParamValue,
    TransportInfo,
};
use crate::session::{SceneMaps, SessionPlayback};
use crate::sidechain::SidechainKey;
use crate::state::PlayedParam;

/// A compiled song, ready to run on the audio thread: a flat array of
/// nodes, the tasks that run them, the buffers they work in, and the order
/// to run the tasks in.
///
/// # Tasks
///
/// The unit of work is a task: one track, run in place on the track's own
/// buffer. A task plays its track's clips, if it has a clip player; sums
/// its inputs into its buffer (a group's tracks, Main's top-level tracks
/// and returns, a return's sends, and the tracks routed into its input),
/// each through a [`Route`] whose gain solo, send levels, and monitoring
/// set; runs the track's device chain; and then its [`TrackMixer`] (pan,
/// volume, the activator, the meters). A task whose signal another task
/// takes before its devices or before its mixer (a pre-fader send, or an
/// input's Pre FX or Post FX) keeps a copy there for it to read (the mixer
/// and routing specs).
///
/// A task can start once every task feeding it has finished, so the tasks
/// form a dependency graph. Each task lists the tasks waiting on it and how
/// many it waits on, which is what a pool of real-time workers needs to take
/// ready tasks from a queue (the multicore scheduler, a later package). For
/// now the audio thread runs them alone, in a topological order the compiler
/// worked out.
///
/// # Migration
///
/// A node that survives a recompile isn't made again. The compiler leaves
/// its slot in the new program empty and lists where it was in the program
/// before. When the audio thread swaps the new program in, it moves each
/// surviving node across, which moves a pointer and allocates nothing, so
/// the node keeps its state: a device its tail, a mixer its gains and
/// meters, a route a glide in progress. Whatever is left in the old program
/// goes with it to the drop thread.
pub struct EngineProgram {
    pub(crate) generation: u64,
    /// The generation whose nodes this one takes over.
    pub(crate) base: Option<u64>,
    pub(crate) slots: Vec<NodeSlot>,
    /// Slot of each node key, sorted by key.
    pub(crate) index: Vec<(NodeKey, u32)>,
    pub(crate) migrations: Vec<Migration>,
    pub(crate) tasks: Vec<Task>,
    /// The order the audio thread runs the tasks in.
    pub(crate) order: Vec<u32>,
    /// What each task sums into its buffer before running its chain.
    pub(crate) task_inputs: Vec<Input>,
    /// The steps of every chain, a track's or a rack chain's, each chain a
    /// run of them in signal order (WP-5.1).
    pub(crate) steps: Vec<Step>,
    /// The racks in the tasks' chains, children before the racks that hold
    /// them.
    pub(crate) racks: Vec<Rack>,
    /// The chains of the racks, each rack's together in list order.
    pub(crate) rack_chains: Vec<RackChain>,
    /// Where a MIDI effect rack merges its chains' notes: the buffer and the
    /// next note of each chain, with room for every chain in the program.
    pub(crate) merge: Vec<MergeCursor>,
    /// The sidechains of the devices each task runs (WP-2.19b).
    pub(crate) sidechains: Vec<DeviceSidechain>,
    /// What each sidechain sums into its buffer.
    pub(crate) sidechain_inputs: Vec<Input>,
    /// One buffer per sidechain, holding what its device's detector
    /// hears.
    pub(crate) sidechain_pool: Vec<f32>,
    /// Tasks waiting on each task. The single-threaded schedule runs `order`
    /// instead; the worker pool (a later package) will walk these.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) task_dependents: Vec<u32>,
    /// One buffer per task, then one per rack chain: `channels` channels of
    /// `max_frames` frames each.
    pub(crate) pool: Vec<f32>,
    /// The copies tasks keep of their signal before their devices or
    /// before their mixer, for routes that tap there: one buffer each.
    pub(crate) tap_pool: Vec<f32>,
    /// The delay lines that carry delay compensation (WP-2.11): one per
    /// delayed route, plus one per track whose clips are delayed.
    pub(crate) delay_lines: Vec<DelayLine>,
    /// The audio the delay lines hold: each line `channels` channels of
    /// `len` frames.
    pub(crate) delay_pool: Vec<f32>,
    /// A scratch buffer for a delayed route's input: `channels` channels of
    /// `max_frames` frames, reused by every route.
    pub(crate) delay_scratch: Vec<f32>,
    /// The audio device's input for the block being processed:
    /// `device_input_channels` channels of `max_frames` frames, filled by
    /// [`Processor::process`](crate::Processor::process) before the tasks
    /// run.
    pub(crate) device_input: Vec<f32>,
    /// The device's input channels; zero without a device that has input.
    pub(crate) device_input_channels: usize,
    /// One task's device input mapped to the tracks' stereo layout, one
    /// task at a time.
    pub(crate) device_scratch: Vec<f32>,
    pub(crate) channels: usize,
    pub(crate) max_frames: usize,
    /// The task whose buffer is the engine's output: Main's.
    pub(crate) output: u32,
    /// The tracks that play on the device's channels instead of into Main
    /// (Audio To set to Ext. Out), in display order.
    pub(crate) ext_outs: Vec<ExtOut>,
    /// The maps the program plays under: the set's, or the ones a launched
    /// scene put in place (WP-3.5), which the audio thread switches to and
    /// from among [`Program::scene_maps`] and the set's own below, so no map
    /// is ever the last one left holding its memory there.
    pub(crate) tempo: TempoMap,
    pub(crate) time_signatures: TimeSignatureMap,
    /// The set's own tempo map and time-signature map, which the program
    /// plays under unless a launched scene overrides them.
    pub(crate) base_tempo: TempoMap,
    pub(crate) base_signatures: TimeSignatureMap,
    /// The tempo and signature of every scene that has its own (WP-3.5).
    pub(crate) scene_maps: Vec<SceneMaps>,
    /// The maps of the tempo and signature a launched scene put in place, as
    /// the compiler last heard of them: they hold after the scene's own
    /// fields change or the scene goes.
    pub(crate) override_tempo: Option<(Bpm, TempoMap)>,
    pub(crate) override_signatures: Option<(TimeSignature, TimeSignatureMap)>,
    pub(crate) sample_rate: SampleRate,
    /// Every audio and MIDI slot that holds a clip, as `(scene, track,
    /// launch mode)` triples sorted by scene and then track: what a scene
    /// launch queues (WP-3.2) and where a press finds its clip's mode
    /// (WP-3.3).
    pub(crate) scene_slots: Vec<(SceneId, TrackId, LaunchMode)>,
    /// Every audio and MIDI track, in order: what Stop All Clips stops
    /// (WP-3.6).
    pub(crate) session_tracks: Vec<TrackId>,
    /// The pair of buffers notes pass along a track's chain in, then a pair
    /// for each rack chain (WP-5.1).
    pub(crate) note_buffers: Vec<NoteBuffer>,
    /// The slots of the modulators, in the order their rows sit on the bus:
    /// run before any task, each over the block's frames (WP-5.10).
    pub(crate) modulators: Vec<u32>,
    /// The modulators' rows: [`ROWS`] per modulator, one value per frame,
    /// `max_frames` apart.
    pub(crate) bus: Vec<f32>,
    /// The note-offs of devices that were removed or moved when this program
    /// replaced the last, waiting for the first block.
    pub(crate) releases: Releases,
    /// The MIDI tracks' inputs, and what each received in the last block
    /// (WP-2.2).
    pub(crate) midi: MidiRoutes,
}

/// How many note-offs a program swap holds for the devices it removed or
/// moved; more are dropped and counted.
const RELEASE_ROOM: usize = 256;

/// The note-offs of devices a program swap removed or moved: each is fed to
/// the chain of its track in front of the node at `at`, in the first block
/// the program runs, so the instrument below doesn't hang on a note the
/// device would have ended.
#[derive(Clone, Debug)]
pub(crate) struct Releases {
    /// `(task, at, note-off)`.
    pending: Vec<(u32, u32, NoteEvent)>,
    /// Where a node writes its note-offs, before they are sorted into
    /// `pending`.
    scratch: NoteBuffer,
    dropped: u64,
}

impl Releases {
    pub(crate) fn new() -> Self {
        Self {
            pending: Vec::with_capacity(RELEASE_ROOM),
            scratch: NoteBuffer::with_capacity(RELEASE_ROOM),
            dropped: 0,
        }
    }

    fn push(&mut self, task: u32, at: u32, note: NoteEvent) {
        if self.pending.len() < self.pending.capacity() {
            self.pending.push((task, at, note));
        } else {
            self.dropped += 1;
        }
    }

    /// Takes the note-offs `node` writes, to feed in front of the node at
    /// `at` of task `task`.
    fn take(&mut self, node: &mut dyn Node, task: u32, at: u32) {
        self.scratch.clear();
        node.release(&mut self.scratch);
        for i in 0..self.scratch.notes().len() {
            let note = self.scratch.notes()[i];
            self.push(task, at, note);
        }
    }
}

/// A track whose Audio To is Ext. Out: its task, and the device channels it
/// plays on (the routing spec, R3, R16).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExtOut {
    pub(crate) task: u32,
    pub(crate) channel: Channel,
}

/// What can sit in a program's slot.
pub(crate) enum Instance {
    /// A device's node, run in its track's chain.
    Device(Box<dyn Node>),
    /// A track's mixer, run after its chain.
    Mixer(Box<TrackMixer>),
    /// A route into a track's input: an output or a send; or into a
    /// device's sidechain.
    Route(Box<Route>),
    /// A device's sidechain settings, which make what its detector hears.
    Sidechain(Box<SidechainKey>),
}

impl Instance {
    /// The instance as a node, if it processes a track's buffer in place.
    fn as_node(&mut self) -> Option<&mut (dyn Node + 'static)> {
        match self {
            Self::Device(node) => Some(node.as_mut()),
            Self::Mixer(mixer) => Some(mixer.as_mut()),
            Self::Route(_) | Self::Sidechain(_) => None,
        }
    }

    /// What the instance's last block played under modulation (WP-5.9).
    fn modulated(&self, report: &mut dyn FnMut(ParamId, f64, f64)) {
        match self {
            Self::Device(node) => node.modulated(report),
            Self::Mixer(mixer) => Node::modulated(mixer.as_ref(), report),
            Self::Route(route) => route.modulated(report),
            Self::Sidechain(_) => {}
        }
    }

    /// Hands the instance node data, whatever it is: a route takes the
    /// envelope driving its level (WP-2.12) the way a node does.
    pub(crate) fn receive_data(&mut self, data: NodeData) {
        match self {
            Self::Device(node) => node.receive_data(data),
            Self::Mixer(mixer) => mixer.receive_data(data),
            Self::Route(route) => route.receive_data(data),
            Self::Sidechain(_) => drop(data),
        }
    }
}

/// A step in a chain (WP-5.1): a device's node, or a rack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The slot of a device's node.
    Node(u32),
    /// A rack, by its place in `racks`.
    Rack(u32),
}

/// What a rack does with the signals that reach it (the racks spec, R3, R4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RackMode {
    /// An audio effect rack: the chains' audio is summed.
    Audio,
    /// An instrument or drum rack: the chains take the notes, and their
    /// audio is summed.
    Instrument,
    /// A MIDI effect rack: the chains' notes are merged, and the audio
    /// passes through.
    Midi,
}

/// A rack in a chain: its chains run side by side on its input, and their
/// sum is its output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rack {
    pub(crate) mode: RackMode,
    /// The rack's chains, in `rack_chains`.
    pub(crate) chains: Span,
    /// Switched off: the rack passes its input through untouched.
    pub(crate) bypass: bool,
    /// The frames the rack adds, which are its slowest chain's (R16).
    pub(crate) latency: u32,
}

/// One chain of a rack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RackChain {
    /// The chain's devices, in `steps`.
    pub(crate) steps: Span,
    /// The slot of the chain's mixer.
    pub(crate) mixer: u32,
    /// The chain's own audio buffer in the pool (after the tasks') and its
    /// pair of note buffers (after the track's).
    pub(crate) buffer: u32,
    /// The delay line that holds the chain's output back to its rack's
    /// latency (R16), or [`NO_DELAY`].
    pub(crate) delay: u32,
    /// The chain's buffers in `tap_pool` for its signal at the rack's input
    /// (Pre FX), after its devices (Post FX) and after its mixer (Post
    /// Mixer), for routes that tap it (R38).
    pub(crate) taps: [Option<u32>; 3],
}

impl RackChain {
    /// The index of a tap in [`RackChain::taps`].
    pub(crate) fn tap_index(tap: Tap) -> usize {
        match tap {
            Tap::PreFx => 0,
            Tap::PostFx => 1,
            Tap::PostMixer => 2,
        }
    }
}

/// A node in a program, or the place for one that migrates in.
pub(crate) struct NodeSlot {
    pub(crate) key: NodeKey,
    pub(crate) node: Option<Instance>,
    /// Switched off: the node keeps its state and takes parameter changes,
    /// but doesn't process.
    pub(crate) bypass: bool,
    /// The frames its own processing adds, for delay compensation
    /// (WP-2.11): a device's `latency`, and zero for everything else.
    pub(crate) latency: u32,
    /// The frames a parameter change aimed at this node is held back, so
    /// automation lines up with the compensated audio (WP-2.11).
    pub(crate) delay: u32,
    /// Whether the node is a modulator: [`EngineProgram::run`] works its
    /// output out before the tasks (WP-5.10), and it leaves the chain's
    /// signal as it is.
    pub(crate) modulator: bool,
}

/// A node that moves from the program before into this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Migration {
    pub(crate) from: u32,
    pub(crate) to: u32,
}

/// A range of one of the program's flat lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) start: u32,
    pub(crate) end: u32,
}

impl Span {
    pub(crate) fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// One input of a task: another task's signal through a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Input {
    /// The slot of the route's gain.
    pub(crate) route: u32,
    /// The task whose signal it carries.
    pub(crate) from: u32,
    /// Where in that task the signal is taken.
    pub(crate) tap: Tap,
    /// The rack chain, in `rack_chains`, whose signal at `tap` it carries
    /// instead of the task's own, or [`NO_CHAIN`].
    pub(crate) chain: u32,
    /// The delay line its signal is delayed through to line it up with the
    /// destination's other inputs (WP-2.11), or [`NO_DELAY`].
    pub(crate) delay: u32,
}

/// An input that carries a task's own signal, not a rack chain's.
pub(crate) const NO_CHAIN: u32 = u32::MAX;

/// A task's own input taken from the audio interface, Ext. In (WP-2.13):
/// the device channels it hears, through a route whose Monitor gate opens
/// and closes it like any other input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeviceInput {
    /// The slot of the route's gain.
    pub(crate) route: u32,
    /// Which of the interface's inputs, counted from 1: one channel, or a
    /// stereo pair.
    pub(crate) channel: Channel,
}

/// No delay line: an input that arrives with no compensation of its own.
pub(crate) const NO_DELAY: u32 = u32::MAX;

/// A delay line that holds a signal back by `len` frames, for delay
/// compensation (WP-2.11). Its `at` is where its samples start in the
/// program's delay pool, and `write` the frame it writes next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DelayLine {
    pub(crate) at: u32,
    pub(crate) len: u32,
    pub(crate) write: u32,
}

/// Where the delay lines for `lengths` sit in one pool, with every write
/// position at zero: each line `channels` channels of `lengths[i]` frames.
pub(crate) fn delay_storage(lengths: &[u32], channels: usize) -> (Vec<DelayLine>, Vec<f32>) {
    let mut lines = Vec::with_capacity(lengths.len());
    let mut total = 0usize;
    for &len in lengths {
        lines.push(DelayLine {
            at: total as u32,
            len,
            write: 0,
        });
        total += channels * len as usize;
    }
    (lines, vec![0.0; total])
}

/// Delays `from` by its line into `scratch`, writing `from` into the line.
/// `scratch` and `from` are planar with `channels` channels `stride` apart,
/// and neither may alias `pool`.
fn delay_into(
    line: &mut DelayLine,
    pool: &mut [f32],
    scratch: &mut [f32],
    from: &[f32],
    channels: usize,
    stride: usize,
    frames: usize,
) {
    let len = line.len as usize;
    for c in 0..channels {
        let base = line.at as usize + c * len;
        let source = &from[c * stride..c * stride + frames];
        let out = &mut scratch[c * stride..c * stride + frames];
        let mut w = line.write as usize;
        for i in 0..frames {
            let at = base + w;
            out[i] = pool[at];
            pool[at] = source[i];
            w += 1;
            if w == len {
                w = 0;
            }
        }
        if c + 1 == channels {
            line.write = w as u32;
        }
    }
}

/// Delays `samples` in place by its line, writing the past samples out as
/// the present ones go in.
fn delay_in_place(
    line: &mut DelayLine,
    pool: &mut [f32],
    samples: &mut [f32],
    channels: usize,
    stride: usize,
    frames: usize,
) {
    let len = line.len as usize;
    for c in 0..channels {
        let base = line.at as usize + c * len;
        let channel = &mut samples[c * stride..c * stride + frames];
        let mut w = line.write as usize;
        for sample in channel.iter_mut() {
            let at = base + w;
            std::mem::swap(&mut pool[at], &mut *sample);
            w += 1;
            if w == len {
                w = 0;
            }
        }
        if c + 1 == channels {
            line.write = w as u32;
        }
    }
}

/// Maps the device's input into a task's stereo layout: one interface
/// channel feeds both sides, a stereo pair one each; a channel the device
/// doesn't have is silence (WP-2.13). `input` holds `device_channels`
/// channels `stride` frames apart; `scratch` is the program's channels.
fn map_device_input(
    scratch: &mut [f32],
    input: &[f32],
    device_channels: usize,
    channel: Channel,
    channels: usize,
    stride: usize,
    frames: usize,
) {
    let (left, right) = match channel {
        Channel::Mono(channel) => (channel, channel),
        Channel::Pair(channel) => (channel, channel.saturating_add(1)),
    };
    for (side, device) in [left, right].into_iter().enumerate().take(channels) {
        let to = &mut scratch[side * stride..side * stride + frames];
        match device
            .checked_sub(1)
            .map(usize::from)
            .filter(|&index| index < device_channels)
        {
            Some(index) => {
                let from = &input[index * stride..index * stride + frames];
                to.copy_from_slice(from);
            }
            None => to.fill(0.0),
        }
    }
}

/// A device's sidechain in a task's chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeviceSidechain {
    /// The device's slot.
    pub(crate) device: u32,
    /// The slot of its [`SidechainKey`].
    pub(crate) key: u32,
    /// Its buffer in `sidechain_pool`.
    pub(crate) buffer: u32,
    /// Its sources, in `sidechain_inputs`.
    pub(crate) inputs: Span,
    /// The frames its track's devices before it add, so its sources line up
    /// with the device's own input (WP-2.11).
    pub(crate) before: u32,
}

/// One track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Task {
    pub(crate) track: TrackId,
    pub(crate) inputs: Span,
    /// The task's input taken from the audio interface, if its Audio From
    /// is Ext. In (WP-2.13).
    pub(crate) device_input: Option<DeviceInput>,
    /// The slot of the track's clip player, which runs first.
    pub(crate) player: Option<u32>,
    /// The device chain, in `steps`.
    pub(crate) chain: Span,
    /// The sidechains of its devices, in `sidechains`.
    pub(crate) sidechains: Span,
    /// The slot of the track's mixer.
    pub(crate) mixer: u32,
    /// The delay line its clips are held back through so they line up with
    /// its inputs (WP-2.11), or [`NO_DELAY`].
    pub(crate) clip_delay: u32,
    /// The task's buffer in `tap_pool` for its signal before its devices,
    /// if a route reads it.
    pub(crate) pre_fx: Option<u32>,
    /// The task's buffer in `tap_pool` for its signal after its devices and
    /// before its mixer, if a route reads it.
    pub(crate) post_fx: Option<u32>,
    pub(crate) dependents: Span,
    /// How many tasks feed this one.
    pub(crate) dependencies: u32,
}

/// A parameter change routed to a slot, for the block being processed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Resolved {
    pub(crate) slot: u32,
    /// The frame in the whole device block.
    pub(crate) offset: u32,
    /// Arrival order, so changes on the same frame apply in the order sent.
    pub(crate) seq: u64,
    pub(crate) param: ParamId,
    pub(crate) value: ParamValue,
}

impl Resolved {
    pub(crate) fn sort_key(&self) -> (u32, u32, u64) {
        (self.slot, self.offset, self.seq)
    }
}

/// Puts into `scratch` the changes in `events` (sorted by slot, then frame)
/// for `slot` that land in `from..until`, with offsets from `from`. Returns
/// how many there are.
fn gather(
    events: &[Resolved],
    slot: u32,
    from: u32,
    until: u32,
    scratch: &mut Vec<NodeEvent>,
) -> u64 {
    scratch.clear();
    let first = events.partition_point(|e| e.slot < slot);
    for event in events[first..].iter().take_while(|e| e.slot == slot) {
        if (from..until).contains(&event.offset) && scratch.len() < scratch.capacity() {
            scratch.push(NodeEvent {
                offset: event.offset - from,
                param: event.param,
                value: event.value,
            });
        }
    }
    scratch.len() as u64
}

impl EngineProgram {
    /// The program's number: each compile makes the next one.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The program whose nodes this one takes over when it's swapped in.
    #[must_use]
    pub fn base(&self) -> Option<u64> {
        self.base
    }

    /// The nodes in the program, including those that migrate in: every
    /// device, track mixer, output, and send.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.slots.len()
    }

    /// The nodes that migrate in from the program before.
    #[must_use]
    pub fn migration_count(&self) -> usize {
        self.migrations.len()
    }

    /// The tasks: one per track.
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Every node key, in key order.
    pub fn keys(&self) -> impl ExactSizeIterator<Item = NodeKey> + '_ {
        self.index.iter().map(|&(key, _)| key)
    }

    /// The tracks, in the order the audio thread runs them.
    pub fn track_order(&self) -> impl ExactSizeIterator<Item = TrackId> + '_ {
        self.order
            .iter()
            .map(|&task| self.tasks[task as usize].track)
    }

    /// The tempo map the program converts ticks with.
    #[must_use]
    pub fn tempo(&self) -> &TempoMap {
        &self.tempo
    }

    /// The slots of `scene` that hold a clip, as `(scene, track, launch
    /// mode)` triples in track order: what a scene launch queues.
    pub(crate) fn scene_slots(&self, scene: SceneId) -> &[(SceneId, TrackId, LaunchMode)] {
        let start = self.scene_slots.partition_point(|&(s, ..)| s < scene);
        let end = start + self.scene_slots[start..].partition_point(|&(s, ..)| s == scene);
        &self.scene_slots[start..end]
    }

    /// The launch mode of the clip in the slot `(track, scene)`, or `None`
    /// for an empty slot.
    pub(crate) fn launch_mode(&self, track: TrackId, scene: SceneId) -> Option<LaunchMode> {
        self.scene_slots(scene)
            .iter()
            .find(|&&(_, t, _)| t == track)
            .map(|&(.., mode)| mode)
    }

    /// What a MIDI track's input received in the last block, and whether
    /// the track monitors (`docs/specs/midi-io.md`, R12). `None` if `track`
    /// isn't a MIDI track in this program.
    #[must_use]
    pub fn midi_input(&self, track: TrackId) -> Option<TrackMidiInput<'_>> {
        self.midi.input(track)
    }

    /// The slot holding `key`'s node.
    pub(crate) fn slot_of(&self, key: NodeKey) -> Option<u32> {
        self.index
            .binary_search_by_key(&key, |&(k, _)| k)
            .ok()
            .map(|i| self.index[i].1)
    }

    /// The instance in `slot`, if one is there.
    pub(crate) fn instance_mut(&mut self, slot: u32) -> Option<&mut Instance> {
        self.slots
            .get_mut(slot as usize)
            .and_then(|slot| slot.node.as_mut())
    }

    /// Drops every track's meters to silence at once. Real-time safe: it
    /// only writes to the mixers already in the program.
    pub(crate) fn silence_meters(&mut self) {
        for slot in &mut self.slots {
            if let Some(Instance::Mixer(mixer)) = slot.node.as_mut() {
                mixer.silence_meters();
            }
        }
    }

    /// Copies the device's input for the block being processed into the
    /// program's own buffer, so the tasks never read the device's
    /// (WP-2.13). `frames` is cut to the program's own, and a device
    /// channel past the program's is dropped. Real-time safe: the buffer
    /// was made with the program.
    pub(crate) fn copy_device_input(&mut self, input: &dyn crate::DeviceInput, frames: usize) {
        let channels = self.device_input_channels;
        let stride = self.max_frames;
        let frames = frames.min(stride);
        for c in 0..channels {
            let to = &mut self.device_input[c * stride..c * stride + frames];
            match (c < input.channels()).then(|| input.channel(c)) {
                Some(from) => {
                    let len = frames.min(from.len());
                    to[..len].copy_from_slice(&from[..len]);
                    to[len..].fill(0.0);
                }
                None => to.fill(0.0),
            }
        }
    }

    /// Adds to `out`, up to its capacity, every parameter the last block
    /// played under modulation (WP-5.9). Real-time safe: `out` was made
    /// with room for them.
    pub(crate) fn modulated(&self, out: &mut Vec<PlayedParam>) {
        for slot in &self.slots {
            let Some(instance) = &slot.node else { continue };
            instance.modulated(&mut |param, base, played| {
                if out.len() < out.capacity() {
                    out.push(PlayedParam {
                        node: slot.key,
                        param,
                        base,
                        played,
                    });
                }
            });
        }
    }

    /// The meters of a task's track.
    pub(crate) fn meters(&self, task: u32) -> MeterReading {
        let mixer = self.tasks[task as usize].mixer as usize;
        match self.slots.get(mixer).and_then(|slot| slot.node.as_ref()) {
            Some(Instance::Mixer(mixer)) => mixer.meters(),
            _ => MeterReading::default(),
        }
    }

    /// Moves the nodes that survive from `old` into this program. Real-time
    /// safe: it swaps pointers, so whatever was in either slot stays owned
    /// by one of the two programs. Returns `false`, and moves nothing, if
    /// `old` isn't the program this one was compiled after.
    pub(crate) fn adopt(&mut self, old: &mut EngineProgram) -> bool {
        if self.base != Some(old.generation) {
            return false;
        }
        for &Migration { from, to } in &self.migrations {
            let (Some(new), Some(old)) = (
                self.slots.get_mut(to as usize),
                old.slots.get_mut(from as usize),
            ) else {
                continue;
            };
            debug_assert_eq!(new.key, old.key, "a migration joins different nodes");
            std::mem::swap(&mut new.node, &mut old.node);
        }
        self.midi.inherit(&mut old.midi);
        // Note-offs the last swap held that no block has run yet carry over.
        for &(task, at, note) in &old.releases.pending {
            self.releases.push(task, at, note);
        }
        self.release_departed(old);
        true
    }

    /// The slot that stands for each step of task `task`'s own chain, in
    /// order: a device's node, or a rack's first chain mixer, which has its
    /// place in the slots' order where the rack is (WP-5.1). The notes of
    /// devices that left a rack's chains aren't ended yet.
    fn top_slots(&self, task: usize) -> impl Iterator<Item = u32> + '_ {
        self.steps[self.tasks[task].chain.range()]
            .iter()
            .map(|&step| match step {
                Step::Node(slot) => slot,
                Step::Rack(rack) => {
                    self.rack_chains[self.racks[rack as usize].chains.range().start].mixer
                }
            })
    }

    /// Spec: device-framework R32. Asks the devices that left their chain
    /// in this swap to end their notes: those the swap removed, and those it
    /// moved relative to the others that stayed. Each device's note-offs
    /// enter the chain in front of the first node that sat after it, so the
    /// nodes after it end the notes it made as they would have when it ended
    /// them itself. Real-time safe: bounded loops, nothing allocated.
    fn release_departed(&mut self, old: &mut EngineProgram) {
        // Most swaps remove and move nothing: every device node migrated, in
        // the order it was in. Skip the search then.
        let removed = old
            .slots
            .iter()
            .any(|s| matches!(s.key, NodeKey::Device(_)) && s.node.is_some());
        let reordered = self.migrations.windows(2).any(|w| w[0].from >= w[1].from);
        if !removed && !reordered {
            return;
        }
        let old_task_of = |old: &EngineProgram, slot: u32| -> Option<usize> {
            (0..old.tasks.len()).find(|&t| old.top_slots(t).any(|s| s == slot))
        };
        // Where, in the new chain of `task`, the device that sat in old slot
        // `slot` of task `from_task` left a gap: in front of the first node
        // that sat after it.
        let gap = |program: &EngineProgram, old: &EngineProgram, task: usize, from_task, slot| {
            program
                .top_slots(task)
                .filter(|&to| {
                    program.migrations.iter().any(|m| {
                        m.to == to && m.from < slot && old_task_of(old, m.from) == Some(from_task)
                    })
                })
                .count() as u32
        };
        // Removed: the old program still holds the node.
        for s in 0..old.slots.len() {
            let slot = s as u32;
            if !matches!(old.slots[s].key, NodeKey::Device(_)) || old.slots[s].node.is_none() {
                continue;
            }
            let Some(from_task) = old_task_of(old, slot) else {
                continue;
            };
            let Some(task) = self
                .tasks
                .iter()
                .position(|t| t.track == old.tasks[from_task].track)
            else {
                continue;
            };
            let at = gap(self, old, task, from_task, slot);
            if let Some(Instance::Device(node)) = old.slots[s].node.as_mut() {
                self.releases.take(node.as_mut(), task as u32, at);
            }
        }
        // Moved: a node that stayed but now sits before one it sat after.
        for task in 0..self.tasks.len() {
            let mut high: Option<(u32, usize)> = None;
            for i in 0..self.tasks[task].chain.range().len() {
                let Some(to) = self.top_slots(task).nth(i) else {
                    continue;
                };
                let Some(from) = self.migrations.iter().find(|m| m.to == to).map(|m| m.from) else {
                    continue;
                };
                let Some(from_task) = old_task_of(old, from) else {
                    continue;
                };
                let moved = match high {
                    Some((h, t)) => from_task != t || from < h,
                    None => old.tasks[from_task].track != self.tasks[task].track,
                };
                high = Some((high.map_or(from, |(h, _)| h.max(from)), from_task));
                if !moved || !matches!(self.slots[to as usize].key, NodeKey::Device(_)) {
                    continue;
                }
                let at = gap(self, old, task, from_task, from);
                if let Some(Instance::Device(node)) = self.slots[to as usize].node.as_mut() {
                    self.releases.take(node.as_mut(), task as u32, at);
                }
            }
        }
    }

    /// Runs every task over `frames` frames, the part of the device block
    /// that starts at `from`. `events` are the block's changes, sorted by
    /// slot then frame; each node and route gets those in its part,
    /// `scratch` holding them. Returns how many changes reached a node or a
    /// route.
    pub(crate) fn run(
        &mut self,
        frames: usize,
        from: u32,
        events: &[Resolved],
        scratch: &mut Vec<NodeEvent>,
        transport: TransportInfo,
        session: &[SessionPlayback],
    ) -> u64 {
        let frames = frames.min(self.max_frames);
        let (channels, stride) = (self.channels, self.max_frames);
        let size = channels * stride;
        let until = from + frames as u32;
        // Where the block sits in the song, for the automation a route plays
        // (WP-2.12): no time while the song is stopped.
        let song = transport.playing.then(|| transport.position.get());
        let shape = crate::mixer::Planar {
            channels,
            stride,
            frames,
        };
        let mut delivered = self.run_modulators(frames, from, until, events, scratch, transport);
        for position in 0..self.order.len() {
            let t = self.order[position] as usize;
            let task = self.tasks[t];
            let own = t * size..(t + 1) * size;
            for c in 0..channels {
                let start = own.start + c * stride;
                self.pool[start..start + frames].fill(0.0);
            }

            self.note_buffers[0].clear();

            // The clips first, alone in the buffer, so the player's gate
            // (Monitor In silences them) scales only them.
            if let Some(slot) = task.player {
                delivered += gather(events, slot, from, until, scratch);
                if let Some(node) = self.slots[slot as usize]
                    .node
                    .as_mut()
                    .and_then(Instance::as_node)
                {
                    node.process(&mut Block {
                        data: &mut self.pool[own.clone()],
                        channels,
                        frames,
                        stride,
                        events: scratch.as_slice(),
                        transport,
                        sample_rate: self.sample_rate,
                        tempo: &self.tempo,
                        sidechain: None,
                        notes: &[],
                        notes_out: Some(&mut self.note_buffers[0]),
                        session,
                        modulation: Bus::new(&self.bus, stride, frames),
                    });
                }
            }

            // Spec: midi-io R12. What the track's input played merges with
            // its clips' notes, on its own frames.
            self.midi
                .play(task.track, from, until, &mut self.note_buffers[0]);

            // Its clips, held back to line up with its inputs.
            if task.clip_delay != NO_DELAY {
                let line = &mut self.delay_lines[task.clip_delay as usize];
                delay_in_place(
                    line,
                    &mut self.delay_pool,
                    &mut self.pool[own.clone()],
                    channels,
                    stride,
                    frames,
                );
            }

            // Spec: routing R11. Whether the track played a clip this block;
            // under Monitor Auto its input routes close while one did. The
            // player ran first, so it knows.
            let playing_clip = match task.player {
                Some(slot) => match self.slots[slot as usize].node.as_ref() {
                    Some(Instance::Device(node)) => node.playing_clip(),
                    _ => false,
                },
                None => false,
            };
            // The interface's input, if the track takes Ext. In: its
            // channels, through the route whose Monitor gate opens and
            // closes it (WP-2.13).
            if let Some(device) = task.device_input {
                delivered += gather(events, device.route, from, until, scratch);
                map_device_input(
                    &mut self.device_scratch,
                    &self.device_input,
                    self.device_input_channels,
                    device.channel,
                    channels,
                    stride,
                    frames,
                );
                if let Some(Some(Instance::Route(route))) = self
                    .slots
                    .get_mut(device.route as usize)
                    .map(|slot| slot.node.as_mut())
                {
                    route.set_playing_clip(playing_clip);
                    route.mix(
                        &mut self.pool[own.clone()],
                        &self.device_scratch,
                        shape,
                        None,
                        Bus::EMPTY,
                        scratch,
                    );
                }
            }

            // Add the inputs, each through its route.
            for &input in &self.task_inputs[task.inputs.range()] {
                delivered += gather(events, input.route, from, until, scratch);
                let Some(Some(Instance::Route(route))) = self
                    .slots
                    .get_mut(input.route as usize)
                    .map(|slot| slot.node.as_mut())
                else {
                    continue;
                };
                route.set_playing_clip(playing_clip);
                let source = input.from as usize;
                // The task's own Post Mixer is its buffer in `pool`; every
                // other tap, a rack chain's included, a copy in `tap_pool`.
                let kept = match (input.chain, input.tap) {
                    (NO_CHAIN, Tap::PostMixer) => None,
                    (NO_CHAIN, Tap::PreFx) => self.tasks[source].pre_fx,
                    (NO_CHAIN, Tap::PostFx) => self.tasks[source].post_fx,
                    (chain, tap) => {
                        self.rack_chains[chain as usize].taps[RackChain::tap_index(tap)]
                    }
                };
                let direct = input.chain == NO_CHAIN && input.tap == Tap::PostMixer;
                let (pool, at) = match kept {
                    Some(buffer) => (&self.tap_pool, buffer as usize * size),
                    None if direct => (&self.pool, source * size),
                    None => continue,
                };
                if input.delay != NO_DELAY {
                    // Hold the source back, then add what comes out.
                    delay_into(
                        &mut self.delay_lines[input.delay as usize],
                        &mut self.delay_pool,
                        &mut self.delay_scratch[..size],
                        &pool[at..at + size],
                        channels,
                        stride,
                        frames,
                    );
                    route.mix(
                        &mut self.pool[own.clone()],
                        &self.delay_scratch[..size],
                        shape,
                        song.map(|frame| crate::automation::Song {
                            frame,
                            session,
                            tempo: &self.tempo,
                            rate: self.sample_rate,
                        }),
                        Bus::new(&self.bus, stride, frames),
                        scratch,
                    );
                } else if direct {
                    let theirs = source * size..(source + 1) * size;
                    let Ok([mine, theirs]) = self.pool.get_disjoint_mut([own.clone(), theirs])
                    else {
                        continue;
                    };
                    route.mix(
                        mine,
                        theirs,
                        shape,
                        song.map(|frame| crate::automation::Song {
                            frame,
                            session,
                            tempo: &self.tempo,
                            rate: self.sample_rate,
                        }),
                        Bus::new(&self.bus, stride, frames),
                        scratch,
                    );
                } else {
                    route.mix(
                        &mut self.pool[own.clone()],
                        &self.tap_pool[at..at + size],
                        shape,
                        song.map(|frame| crate::automation::Song {
                            frame,
                            session,
                            tempo: &self.tempo,
                            rate: self.sample_rate,
                        }),
                        Bus::new(&self.bus, stride, frames),
                        scratch,
                    );
                }
            }

            // Keep the signal before the devices for routes that tap it.
            if let Some(buffer) = task.pre_fx {
                self.keep(buffer, own.start, frames);
            }

            // Sum each sidechain's sources into its buffer.
            for s in task.sidechains.range() {
                delivered += self.sum_sidechain(s, frames, from, until, events, scratch);
            }

            // Run the chain in place.
            let mut pass = Pass {
                frames,
                from,
                until,
                events,
                scratch: &mut *scratch,
                transport,
                session,
                delivered: 0,
            };
            self.run_steps(&mut pass, task, task.chain, t, 0, 0);
            delivered += pass.delivered;

            // Keep the chain's output for pre-fader sends and Post FX
            // inputs.
            if let Some(buffer) = task.post_fx {
                self.keep(buffer, own.start, frames);
            }

            // The track's mixer.
            delivered += gather(events, task.mixer, from, until, scratch);
            if let Some(Some(Instance::Mixer(mixer))) = self
                .slots
                .get_mut(task.mixer as usize)
                .map(|slot| slot.node.as_mut())
            {
                mixer.process(&mut Block {
                    data: &mut self.pool[own.clone()],
                    channels,
                    frames,
                    stride,
                    events: scratch.as_slice(),
                    transport,
                    sample_rate: self.sample_rate,
                    tempo: &self.tempo,
                    sidechain: None,
                    notes: &[],
                    notes_out: None,
                    session,
                    modulation: Bus::new(&self.bus, stride, frames),
                });
            }
        }
        self.releases.pending.clear();
        delivered
    }

    /// Works out every modulator's rows over `frames` frames, in the order of
    /// their rows, before any task runs (WP-5.10; the modulation spec, R20).
    /// A modulator switched off, or one that doesn't give rows, leaves its
    /// rows idle. Returns how many changes reached a modulator.
    fn run_modulators(
        &mut self,
        frames: usize,
        from: u32,
        until: u32,
        events: &[Resolved],
        scratch: &mut Vec<NodeEvent>,
        transport: TransportInfo,
    ) -> u64 {
        let stride = self.max_frames;
        let mut delivered = 0;
        for m in 0..self.modulators.len() {
            let slot = self.modulators[m];
            delivered += gather(events, slot, from, until, scratch);
            let (earlier, rest) = self.bus.split_at_mut(m * ROWS * stride);
            let mut ctl = Control {
                frames,
                events: scratch.as_slice(),
                transport,
                sample_rate: self.sample_rate,
                tempo: &self.tempo,
                earlier: Bus::new(earlier, stride, frames),
                out: &mut rest[..ROWS * stride],
                stride,
            };
            let NodeSlot { node, bypass, .. } = &mut self.slots[slot as usize];
            let Some(node) = node.as_mut().and_then(Instance::as_node) else {
                ctl.idle();
                continue;
            };
            if *bypass {
                node.flush(ctl.events);
                ctl.idle();
            } else if !node.control(&mut ctl) {
                ctl.idle();
            }
        }
        delivered
    }

    /// Sums sidechain `s`'s sources into its buffer, each through its
    /// route, over `frames` frames. Returns how many changes reached a
    /// route.
    fn sum_sidechain(
        &mut self,
        s: usize,
        frames: usize,
        from: u32,
        until: u32,
        events: &[Resolved],
        scratch: &mut Vec<NodeEvent>,
    ) -> u64 {
        // A sidechain's route has no level to automate, so it plays none.
        let (channels, stride) = (self.channels, self.max_frames);
        let shape = crate::mixer::Planar {
            channels,
            stride,
            frames,
        };
        let size = channels * stride;
        let sidechain = self.sidechains[s];
        let at = sidechain.buffer as usize * size..(sidechain.buffer as usize + 1) * size;
        for c in 0..channels {
            let start = at.start + c * stride;
            self.sidechain_pool[start..start + frames].fill(0.0);
        }
        let mut delivered = 0;
        for &input in &self.sidechain_inputs[sidechain.inputs.range()] {
            delivered += gather(events, input.route, from, until, scratch);
            let Some(Some(Instance::Route(route))) = self
                .slots
                .get_mut(input.route as usize)
                .map(|slot| slot.node.as_mut())
            else {
                continue;
            };
            let source = &self.tasks[input.from as usize];
            let theirs = match input.tap {
                Tap::PreFx => source.pre_fx.map(|b| (&self.tap_pool, b as usize)),
                Tap::PostFx => source.post_fx.map(|b| (&self.tap_pool, b as usize)),
                Tap::PostMixer => Some((&self.pool, input.from as usize)),
            };
            let Some((pool, buffer)) = theirs else {
                continue;
            };
            let theirs = &pool[buffer * size..(buffer + 1) * size];
            if input.delay != NO_DELAY {
                delay_into(
                    &mut self.delay_lines[input.delay as usize],
                    &mut self.delay_pool,
                    &mut self.delay_scratch[..size],
                    theirs,
                    channels,
                    stride,
                    frames,
                );
                route.mix(
                    &mut self.sidechain_pool[at.clone()],
                    &self.delay_scratch[..size],
                    shape,
                    None,
                    Bus::EMPTY,
                    scratch,
                );
            } else {
                route.mix(
                    &mut self.sidechain_pool[at.clone()],
                    theirs,
                    shape,
                    None,
                    Bus::EMPTY,
                    scratch,
                );
            }
        }
        delivered
    }

    /// Fills `frames` frames of the tap buffer `buffer` with silence.
    fn silence(&mut self, buffer: u32, frames: usize) {
        let size = self.channels * self.max_frames;
        let kept = buffer as usize * size;
        for c in 0..self.channels {
            let at = kept + c * self.max_frames;
            self.tap_pool[at..at + frames].fill(0.0);
        }
    }

    /// Copies `frames` frames of the pool buffer at `own` into the tap
    /// buffer `buffer`.
    fn keep(&mut self, buffer: u32, own: usize, frames: usize) {
        let size = self.channels * self.max_frames;
        let kept = buffer as usize * size;
        for c in 0..self.channels {
            let at = c * self.max_frames;
            self.tap_pool[kept + at..kept + at + frames]
                .copy_from_slice(&self.pool[own + at..own + at + frames]);
        }
    }

    /// The output's channel `c` from the last run, `frames` long.
    pub(crate) fn output(&self, c: usize, frames: usize) -> &[f32] {
        self.task_output(self.output, c, frames)
    }

    /// Task `task`'s channel `c` from the last run, `frames` long.
    fn task_output(&self, task: u32, c: usize, frames: usize) -> &[f32] {
        let start = task as usize * self.channels * self.max_frames + c * self.max_frames;
        &self.pool[start..start + frames.min(self.max_frames)]
    }

    /// How many device channels the Ext. Out tracks reach: one past the
    /// highest.
    pub(crate) fn ext_width(&self) -> usize {
        self.ext_outs
            .iter()
            .map(|e| match e.channel {
                Channel::Mono(c) => usize::from(c),
                Channel::Pair(c) => usize::from(c) + 1,
            })
            .max()
            .unwrap_or(0)
    }

    /// Adds the Ext. Out tracks from the last run onto `out`, which holds
    /// `width` device channels `stride` apart, `length` frames from
    /// `offset`. A pair plays left and right on its two channels, a single
    /// channel takes both added and 6 dB down (the manual, 17.2.1); a
    /// channel past `width` is the device's absence and plays nothing.
    pub(crate) fn mix_external(
        &self,
        out: &mut [f32],
        width: usize,
        stride: usize,
        offset: usize,
        length: usize,
    ) {
        for ext in &self.ext_outs {
            let left = self.task_output(ext.task, 0, length);
            let right = self.task_output(ext.task, 1.min(self.channels - 1), length);
            let mut add = |channel: usize, gain_l: f32, gain_r: f32| {
                // Channels count from 1; 0 names nothing.
                let Some(channel) = channel.checked_sub(1).filter(|&c| c < width) else {
                    return;
                };
                let to = channel * stride + offset;
                for (i, sample) in out[to..to + length].iter_mut().enumerate() {
                    *sample += left[i] * gain_l + right[i] * gain_r;
                }
            };
            match ext.channel {
                Channel::Mono(c) => add(usize::from(c), 0.5, 0.5),
                Channel::Pair(c) => {
                    add(usize::from(c), 1.0, 0.0);
                    add(usize::from(c) + 1, 0.0, 1.0);
                }
            }
        }
    }

    /// The order a pool of workers would finish the tasks in if each took
    /// ready tasks from a queue: Kahn's algorithm over the dependency
    /// counts, taking the newest ready task first. A check that the graph
    /// is complete and acyclic, and a second valid order for tests.
    #[cfg(test)]
    pub(crate) fn ready_order(&self) -> Vec<u32> {
        let mut waiting: Vec<u32> = self.tasks.iter().map(|t| t.dependencies).collect();
        let mut ready: Vec<u32> = (0..self.tasks.len() as u32)
            .filter(|&t| waiting[t as usize] == 0)
            .collect();
        let mut done = Vec::with_capacity(self.tasks.len());
        while let Some(t) = ready.pop() {
            done.push(t);
            for &next in &self.task_dependents[self.tasks[t as usize].dependents.range()] {
                waiting[next as usize] -= 1;
                if waiting[next as usize] == 0 {
                    ready.push(next);
                }
            }
        }
        done
    }
}

/// What a run over a block hands down to each chain: where the block sits,
/// the parameter changes for it, and the count of those delivered.
struct Pass<'a> {
    frames: usize,
    from: u32,
    until: u32,
    events: &'a [Resolved],
    scratch: &'a mut Vec<NodeEvent>,
    transport: TransportInfo,
    session: &'a [SessionPlayback],
    delivered: u64,
}

impl EngineProgram {
    /// Runs a chain's steps in place on the pool buffer `buffer`, with the
    /// notes in the pair of buffers starting at `pair`, whose `note_idx`th
    /// holds them now. Returns which of the pair holds the notes after.
    #[allow(clippy::too_many_lines)]
    fn run_steps(
        &mut self,
        pass: &mut Pass<'_>,
        task: Task,
        steps: Span,
        buffer: usize,
        pair: usize,
        mut note_idx: usize,
    ) -> usize {
        let (channels, stride, frames) = (self.channels, self.max_frames, pass.frames);
        let size = channels * stride;
        let own = buffer * size..(buffer + 1) * size;
        for step_at in steps.range() {
            // Spec: device-framework R32. The note-offs of devices that left
            // the track's chain in the swap enter in front of the step that
            // followed them, ahead of the notes already there. Only the
            // track's own chain, in task `buffer`, uses the first note pair.
            if pair == 0 {
                let at = step_at - steps.range().start;
                for &(_, _, note) in self
                    .releases
                    .pending
                    .iter()
                    .filter(|&&(task, to, _)| task as usize == buffer && to as usize == at)
                {
                    self.note_buffers[note_idx].insert(note);
                }
            }
            let slot = match self.steps[step_at] {
                Step::Node(slot) => slot,
                Step::Rack(rack) => {
                    note_idx = self.run_rack(pass, task, rack as usize, buffer, pair, note_idx);
                    continue;
                }
            };
            let sidechain = self.sidechains[task.sidechains.range()]
                .iter()
                .find(|s| s.device == slot)
                .copied();
            let bypass = self.slots[slot as usize].bypass;
            // What the detector hears, made before the device runs.
            let mut heard = None;
            if let Some(sidechain) = sidechain {
                pass.delivered += gather(
                    pass.events,
                    sidechain.key,
                    pass.from,
                    pass.until,
                    pass.scratch,
                );
                let at = sidechain.buffer as usize * size..(sidechain.buffer as usize + 1) * size;
                if let Some(Some(Instance::Sidechain(key))) = self
                    .slots
                    .get_mut(sidechain.key as usize)
                    .map(|slot| slot.node.as_mut())
                {
                    if bypass {
                        key.flush(pass.scratch);
                    } else {
                        key.make(
                            &mut self.sidechain_pool[at.clone()],
                            &self.pool[own.clone()],
                            channels,
                            stride,
                            frames,
                            pass.scratch,
                        );
                        if key.enabled() {
                            heard = Some((at, key.listening()));
                        }
                    }
                }
            }
            // A modulator worked out its rows before the tasks ran and
            // leaves the chain's audio and notes as they are.
            if self.slots[slot as usize].modulator {
                let [in_buf, out_buf] = self
                    .note_buffers
                    .get_disjoint_mut([pair + note_idx, pair + 1 - note_idx])
                    .expect("the pair's buffers are two");
                out_buf.clear();
                for &note in in_buf.notes() {
                    out_buf.push(note);
                }
                note_idx = 1 - note_idx;
                continue;
            }
            pass.delivered += gather(pass.events, slot, pass.from, pass.until, pass.scratch);
            let NodeSlot { node, .. } = &mut self.slots[slot as usize];
            let Some(node) = node.as_mut().and_then(Instance::as_node) else {
                continue;
            };
            let [in_buf, out_buf] = self
                .note_buffers
                .get_disjoint_mut([pair + note_idx, pair + 1 - note_idx])
                .expect("the pair's buffers are two");
            out_buf.clear();
            if bypass {
                node.flush(pass.scratch);
                // Spec: device-framework R32. A device switched off ends
                // the notes it started, on this block's first frame.
                node.release(out_buf);
                for &note in in_buf.notes() {
                    out_buf.push(note);
                }
                note_idx = 1 - note_idx;
                continue;
            }
            node.process(&mut Block {
                data: &mut self.pool[own.clone()],
                channels,
                frames,
                stride,
                events: pass.scratch.as_slice(),
                transport: pass.transport,
                sample_rate: self.sample_rate,
                tempo: &self.tempo,
                sidechain: heard
                    .as_ref()
                    .map(|(at, _)| &self.sidechain_pool[at.clone()]),
                notes: in_buf.notes(),
                notes_out: Some(out_buf),
                session: pass.session,
                modulation: Bus::new(&self.bus, stride, frames),
            });
            note_idx = 1 - note_idx;
            // Sidechain Listen: the device's output is what it heard.
            if let Some((at, true)) = heard {
                for c in 0..channels {
                    let (to, from) = (own.start + c * stride, at.start + c * stride);
                    self.pool[to..to + frames]
                        .copy_from_slice(&self.sidechain_pool[from..from + frames]);
                }
            }
        }
        note_idx
    }

    /// Runs a rack in place on the pool buffer `buffer` (the racks spec,
    /// R3 to R5, R15, R16): every chain takes the rack's input, a copy of
    /// its audio and notes, runs its devices and then its mixer, is held
    /// back to the rack's latency, and is added into the buffer in the
    /// chains' order. Returns which of the pair of note buffers at `pair`
    /// holds the notes after.
    fn run_rack(
        &mut self,
        pass: &mut Pass<'_>,
        task: Task,
        rack: usize,
        buffer: usize,
        pair: usize,
        note_idx: usize,
    ) -> usize {
        let Rack {
            mode,
            chains,
            bypass,
            ..
        } = self.racks[rack];
        if bypass {
            self.flush_rack(pass, task, rack);
            return note_idx;
        }
        if chains.start == chains.end {
            return note_idx;
        }
        let (channels, stride, frames) = (self.channels, self.max_frames, pass.frames);
        let size = channels * stride;
        let audio = mode != RackMode::Midi;
        let notes = mode != RackMode::Audio;
        let first_chain_buffer = self.tasks.len();
        let own = buffer * size;
        // Every chain takes the rack's input.
        for i in chains.range() {
            let chain = self.rack_chains[i];
            let at = (first_chain_buffer + chain.buffer as usize) * size;
            for c in 0..channels {
                let edge = c * stride;
                if audio {
                    self.pool
                        .copy_within(own + edge..own + edge + frames, at + edge);
                } else {
                    self.pool[at + edge..at + edge + frames].fill(0.0);
                }
            }
            if notes {
                let to = 2 + 2 * chain.buffer as usize;
                let [from, to] = self
                    .note_buffers
                    .get_disjoint_mut([pair + note_idx, to])
                    .expect("a chain's notes are its own");
                to.clear();
                for &note in from.notes() {
                    to.push(note);
                }
            }
        }
        if audio {
            for c in 0..channels {
                let edge = own + c * stride;
                self.pool[edge..edge + frames].fill(0.0);
            }
            for i in chains.range() {
                let chain = self.rack_chains[i];
                if let Some(kept) = chain.taps[RackChain::tap_index(Tap::PreFx)] {
                    self.keep(
                        kept,
                        (first_chain_buffer + chain.buffer as usize) * size,
                        frames,
                    );
                }
            }
        }
        let merge_from = self.merge.len();
        for i in chains.range() {
            let chain = self.rack_chains[i];
            let at_buffer = first_chain_buffer + chain.buffer as usize;
            let at = at_buffer * size;
            let chain_pair = 2 + 2 * chain.buffer as usize;
            let end = self.run_steps(pass, task, chain.steps, at_buffer, chain_pair, 0);
            if audio && let Some(kept) = chain.taps[RackChain::tap_index(Tap::PostFx)] {
                self.keep(kept, at, frames);
            }
            pass.delivered += gather(
                pass.events,
                chain.mixer,
                pass.from,
                pass.until,
                pass.scratch,
            );
            let mut muted = false;
            if let Some(Some(Instance::Mixer(mixer))) = self
                .slots
                .get_mut(chain.mixer as usize)
                .map(|slot| slot.node.as_mut())
            {
                if audio {
                    mixer.process(&mut Block {
                        data: &mut self.pool[at..at + size],
                        channels,
                        frames,
                        stride,
                        events: pass.scratch.as_slice(),
                        transport: pass.transport,
                        sample_rate: self.sample_rate,
                        tempo: &self.tempo,
                        sidechain: None,
                        notes: &[],
                        notes_out: None,
                        session: pass.session,
                        modulation: Bus::new(&self.bus, stride, frames),
                    });
                } else {
                    mixer.flush(pass.scratch);
                }
                muted = mixer.muted();
            }
            if audio {
                if let Some(kept) = chain.taps[RackChain::tap_index(Tap::PostMixer)] {
                    self.keep(kept, at, frames);
                }
                if chain.delay != NO_DELAY {
                    delay_in_place(
                        &mut self.delay_lines[chain.delay as usize],
                        &mut self.delay_pool,
                        &mut self.pool[at..at + size],
                        channels,
                        stride,
                        frames,
                    );
                }
                for c in 0..channels {
                    let edge = c * stride;
                    let Ok([sum, chain_out]) = self.pool.get_disjoint_mut([
                        own + edge..own + edge + frames,
                        at + edge..at + edge + frames,
                    ]) else {
                        continue;
                    };
                    for (sum, sample) in sum.iter_mut().zip(chain_out.iter()) {
                        *sum += *sample;
                    }
                }
            }
            if mode == RackMode::Midi && self.merge.len() < self.merge.capacity() {
                self.merge.push(MergeCursor {
                    buffer: chain_pair + end,
                    cursor: 0,
                    muted,
                });
            }
        }
        if !notes {
            return note_idx;
        }
        let out = pair + 1 - note_idx;
        self.note_buffers[out].clear();
        if mode == RackMode::Midi {
            self.merge_notes(merge_from, out);
        }
        1 - note_idx
    }

    /// Hands the nodes of a switched-off rack, and of the racks inside it,
    /// their parameter changes without running them, so they come back on
    /// with what was set while it was off (the racks spec, R7).
    fn flush_rack(&mut self, pass: &mut Pass<'_>, task: Task, rack: usize) {
        for i in self.racks[rack].chains.range() {
            let chain = self.rack_chains[i];
            // A rack that is off carries nothing to a route tapping its
            // chains.
            for kept in chain.taps.into_iter().flatten() {
                self.silence(kept, pass.frames);
            }
            for at in chain.steps.range() {
                match self.steps[at] {
                    Step::Rack(inner) => self.flush_rack(pass, task, inner as usize),
                    Step::Node(slot) => {
                        let sidechain = self.sidechains[task.sidechains.range()]
                            .iter()
                            .find(|s| s.device == slot)
                            .copied();
                        if let Some(sidechain) = sidechain {
                            pass.delivered += gather(
                                pass.events,
                                sidechain.key,
                                pass.from,
                                pass.until,
                                pass.scratch,
                            );
                            if let Some(Some(Instance::Sidechain(key))) = self
                                .slots
                                .get_mut(sidechain.key as usize)
                                .map(|slot| slot.node.as_mut())
                            {
                                key.flush(pass.scratch);
                            }
                        }
                        pass.delivered +=
                            gather(pass.events, slot, pass.from, pass.until, pass.scratch);
                        if let Some(node) = self.slots[slot as usize]
                            .node
                            .as_mut()
                            .and_then(Instance::as_node)
                        {
                            node.flush(pass.scratch);
                        }
                    }
                }
            }
            pass.delivered += gather(
                pass.events,
                chain.mixer,
                pass.from,
                pass.until,
                pass.scratch,
            );
            if let Some(Some(Instance::Mixer(mixer))) = self
                .slots
                .get_mut(chain.mixer as usize)
                .map(|slot| slot.node.as_mut())
            {
                mixer.flush(pass.scratch);
            }
        }
    }

    /// Merges the notes of the chains listed in `merge` from `from` on into
    /// the buffer `out`, in time order, the chain higher in the rack's list
    /// first where two fall on one frame (the racks spec, R4). A chain
    /// that's off passes only its note-offs, so no note it started hangs.
    fn merge_notes(&mut self, from: usize, out: usize) {
        loop {
            let mut next: Option<(usize, NoteEvent)> = None;
            for at in from..self.merge.len() {
                let chain = &mut self.merge[at];
                let notes = self.note_buffers[chain.buffer].notes();
                while chain.muted
                    && notes
                        .get(chain.cursor)
                        .is_some_and(|n| !matches!(n.kind, NoteKind::Off | NoteKind::Choke))
                {
                    chain.cursor += 1;
                }
                if let Some(&note) = notes.get(chain.cursor)
                    && next.is_none_or(|(_, best)| note.offset < best.offset)
                {
                    next = Some((at, note));
                }
            }
            let Some((at, note)) = next else { break };
            self.note_buffers[out].push(note);
            self.merge[at].cursor += 1;
        }
        self.merge.truncate(from);
    }
}

/// A MIDI effect rack's chain whose notes are being merged: where they are,
/// how far through them the merge has got, and whether the chain is off.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MergeCursor {
    pub(crate) buffer: usize,
    pub(crate) cursor: usize,
    pub(crate) muted: bool,
}

impl fmt::Debug for EngineProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineProgram")
            .field("generation", &self.generation)
            .field("base", &self.base)
            .field("nodes", &self.slots.len())
            .field("migrations", &self.migrations.len())
            .field("tasks", &self.tasks.len())
            .finish_non_exhaustive()
    }
}
