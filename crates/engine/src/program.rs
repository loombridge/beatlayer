//! The engine program: what the compiler makes from a document and the audio
//! thread runs.

use core::fmt;
use core::ops::Range;
use std::panic::{self, AssertUnwindSafe};

use bl_core::time::{Bpm, SampleRate, TempoMap, TimeSignature, TimeSignatureMap};
use bl_model::{ChainId, Channel, LaunchMode, ParamId, Scale, SceneId, Tap, TrackId};

use crate::disjoint::Shared;
use crate::drum::DrumNotes;
use crate::midi::{MidiRoutes, OutOwner, TrackMidiInput};
use crate::mixer::{MeterReading, Route, TrackMixer};
use crate::modbus::{Bus, Control, ROWS};
use crate::node::{
    Block, Node, NodeData, NodeEvent, NodeKey, NoteBuffer, NoteEvent, NoteKind, ParamValue,
    TransportInfo,
};
use crate::scales::ScaleRuns;
use crate::sched::{Lane, Pool, Sched, Wake};
use crate::session::{SceneMaps, SessionPlayback};
use crate::sidechain::SidechainKey;
use crate::state::PlayedParam;
use crate::zones::{ChainSelector, ZoneFilter};

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
/// many it waits on. Without a pool of workers the audio thread runs them
/// alone, in a topological order the compiler worked out. With one
/// ([`Pool`](crate::Pool), WP-5.37) the audio thread and the workers take
/// ready tasks from a lock-free queue, and a finished task makes ready the
/// tasks it was the last to hold up. A task reads the buffers that feed it
/// and writes only its own, so the order tasks finish in changes nothing in
/// the output. A rack's chains are work of their own inside the task: they
/// are offered to the same threads and joined before the rack's sum, which
/// is taken in the chains' order.
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
    pub(crate) slots: Shared<NodeSlot>,
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
    /// The sends of the drum chains to their racks' return chains.
    pub(crate) chain_sends: Vec<ChainSendRef>,
    /// Where a MIDI effect rack merges its chains' notes: the buffer and the
    /// next note of each chain, one cursor for every chain in the program.
    pub(crate) merge: Shared<MergeCursor>,
    /// The selector's value on each frame of the block, for each rack:
    /// `max_frames` values at the rack's place in `racks`, which the rack
    /// writes and its chains read (WP-5.3).
    pub(crate) selects: Shared<f32>,
    /// The sidechains of the devices each task runs (WP-2.19b).
    pub(crate) sidechains: Vec<DeviceSidechain>,
    /// What each sidechain sums into its buffer.
    pub(crate) sidechain_inputs: Vec<Input>,
    /// The MIDI sidechains of the devices each task runs (WP-5.27).
    pub(crate) midi_sidechains: Vec<DeviceMidiSidechain>,
    /// The notes of each MIDI track a MIDI sidechain listens to, kept for
    /// the block: what its clips and input played, before its devices.
    pub(crate) midi_kept: Shared<NoteBuffer>,
    /// One buffer per sidechain, holding what its device's detector
    /// hears.
    pub(crate) sidechain_pool: Shared<f32>,
    /// Tasks waiting on each task. The single-threaded schedule runs `order`
    /// instead; a pool of workers walks these.
    pub(crate) task_dependents: Vec<u32>,
    /// What a run through a pool of workers counts and queues with.
    pub(crate) sched: Sched,
    /// One buffer per task, then one per rack chain: `channels` channels of
    /// `max_frames` frames each.
    pub(crate) pool: Shared<f32>,
    /// The copies tasks keep of their signal before their devices or
    /// before their mixer, for routes that tap there: one buffer each.
    pub(crate) tap_pool: Shared<f32>,
    /// The delay lines that carry delay compensation (WP-2.11): one per
    /// delayed route, plus one per track whose clips are delayed.
    pub(crate) delay_lines: Shared<DelayLine>,
    /// The audio the delay lines hold: each line `channels` channels of
    /// `len` frames.
    pub(crate) delay_pool: Shared<f32>,
    /// The audio device's input for the block being processed:
    /// `device_input_channels` channels of `max_frames` frames, filled by
    /// [`Processor::process`](crate::Processor::process) before the tasks
    /// run.
    pub(crate) device_input: Vec<f32>,
    /// The device's input channels; zero without a device that has input.
    pub(crate) device_input_channels: usize,
    pub(crate) channels: usize,
    pub(crate) max_frames: usize,
    /// The task whose buffer is the engine's output: Main's.
    pub(crate) output: u32,
    /// The tracks that play on the device's channels instead of into Main
    /// (Audio To set to Ext. Out), in display order.
    pub(crate) ext_outs: Vec<ExtOut>,
    /// The external effects in the program, with their slots and the
    /// interface's outputs they send to (WP-5.35).
    pub(crate) external_effects: Vec<(u32, Channel)>,
    /// The task that sums the cue-switched tracks, in Cue mode (WP-3.18):
    /// the last of `ext_outs`, and no track of the set's, so it has no
    /// meters.
    pub(crate) cue: Option<u32>,
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
    /// Every slot clip's and scene's follow actions and the quantisation
    /// and length a follow launch needs (WP-3.10).
    pub(crate) follow: crate::session::FollowTable,
    /// Every audio and MIDI track, in order: what Stop All Clips stops
    /// (WP-3.6).
    pub(crate) session_tracks: Vec<TrackId>,
    /// The pair of buffers notes pass along a track's chain in, one pair for
    /// each task, then a pair for each rack chain (WP-5.1).
    pub(crate) note_buffers: Shared<NoteBuffer>,
    /// The slots of the modulators, in the order their rows sit on the bus:
    /// run before any task, each over the block's frames (WP-5.10).
    pub(crate) modulators: Vec<u32>,
    /// Which of the modulators work their output out while their tracks run
    /// (WP-5.11): their places among `modulators`, in order. After the
    /// modulators' rows the bus holds, for each, the rows of what it wrote
    /// 32 frames ago.
    pub(crate) signals: Vec<u32>,
    /// Whether a target reads such a late row, so the program runs in parts
    /// of at most 32 frames (R28).
    pub(crate) lagged: bool,
    /// The modulators' rows: [`ROWS`] per modulator, then per signal
    /// modulator, one value per frame, `max_frames` apart. The rows of the
    /// modulators that work before the tasks run, and the late rows, are
    /// written then and only read while the tasks do. A signal modulator's
    /// direct rows are written by its own task alone, and read by the
    /// tasks that wait for it.
    pub(crate) bus: Shared<f32>,
    /// The note-offs of devices that were removed or moved when this program
    /// replaced the last, waiting for the first block.
    pub(crate) releases: Releases,
    /// The MIDI tracks' inputs, and what each received in the last block
    /// (WP-2.2).
    pub(crate) midi: MidiRoutes,
    /// How long each task ran, in nanoseconds, smoothed: the engine times
    /// each task on the thread that ran it, and the share of the set's
    /// processing time a task's is what the Performance Impact indicator
    /// shows (the mixer spec, R31).
    pub(crate) task_times: Vec<core::sync::atomic::AtomicU64>,
}

/// The frames a late row (WP-5.11) lags its modulator's by, and the longest
/// part a program with one runs in.
pub(crate) const LAG_FRAMES: usize = 32;

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
    /// A rack's chain selector (WP-5.3), which the rack runs before its
    /// chains.
    Selector(Box<ChainSelector>),
    /// The filter at a rack chain's input (WP-5.3).
    Zones(Box<ZoneFilter>),
    /// A drum chain's note routing (the drum racks spec, R2, R4).
    Drum(Box<DrumNotes>),
}

impl Instance {
    /// The instance as a node, if it processes a track's buffer in place.
    fn as_node(&mut self) -> Option<&mut (dyn Node + 'static)> {
        match self {
            Self::Device(node) => Some(node.as_mut()),
            Self::Mixer(mixer) => Some(mixer.as_mut()),
            Self::Selector(selector) => Some(selector.as_mut()),
            Self::Route(_) | Self::Sidechain(_) | Self::Zones(_) | Self::Drum(_) => None,
        }
    }

    /// What the instance's last block played under modulation (WP-5.9).
    fn modulated(&self, report: &mut dyn FnMut(ParamId, f64, f64)) {
        match self {
            Self::Device(node) => node.modulated(report),
            Self::Mixer(mixer) => Node::modulated(mixer.as_ref(), report),
            Self::Route(route) => route.modulated(report),
            Self::Sidechain(_) | Self::Selector(_) | Self::Zones(_) | Self::Drum(_) => {}
        }
    }

    /// Hands the instance node data, whatever it is: a route takes the
    /// envelope driving its level (WP-2.12) the way a node does.
    pub(crate) fn receive_data(&mut self, data: NodeData) {
        match self {
            Self::Device(node) => node.receive_data(data),
            Self::Mixer(mixer) => mixer.receive_data(data),
            Self::Route(route) => route.receive_data(data),
            Self::Selector(selector) => selector.receive_data(data),
            Self::Sidechain(_) | Self::Zones(_) | Self::Drum(_) => drop(data),
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
    /// An instrument rack: the chains take the notes, and their audio is
    /// summed.
    Instrument,
    /// A drum rack: each chain takes the notes it answers, at its Play
    /// note, and the drum chains' sends feed its return chains, whose audio
    /// is summed with theirs.
    Drum,
    /// A MIDI effect rack: the chains' notes are merged, and the audio
    /// passes through.
    Midi,
}

/// A rack in a chain: its chains run side by side on its input, and their
/// sum is its output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rack {
    /// The task whose chain holds the rack, however deep.
    pub(crate) task: u32,
    pub(crate) mode: RackMode,
    /// The rack's chains, in `rack_chains`: a drum rack's drum chains.
    pub(crate) chains: Span,
    /// A drum rack's return chains, in `rack_chains` after its chains; empty
    /// in any other rack.
    pub(crate) returns: Span,
    /// Switched off: the rack passes its input through untouched.
    pub(crate) bypass: bool,
    /// The frames the rack adds, which are its slowest chain's (R16).
    pub(crate) latency: u32,
    /// The slot of the rack's chain selector (R33); the rack's place in
    /// `racks` is also its place in `selects`.
    pub(crate) selector: u32,
}

impl Rack {
    /// Every chain of the rack, return chains included.
    pub(crate) fn all(&self) -> Span {
        Span {
            start: self.chains.start,
            end: self.returns.end,
        }
    }
}

/// No node: a chain that has no note routing.
pub(crate) const NO_SLOT: u32 = u32::MAX;

/// A drum chain's send to one of its rack's return chains: the slot of the
/// send's route, and the return chain's place in `rack_chains`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChainSendRef {
    pub(crate) route: u32,
    pub(crate) to: u32,
}

/// One chain of a rack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RackChain {
    /// The chain's devices, in `steps`.
    pub(crate) steps: Span,
    /// The slot of the chain's mixer.
    pub(crate) mixer: u32,
    /// The slot of the filter at the chain's input (R31).
    pub(crate) zones: u32,
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
    /// The slot of a drum chain's note routing, or [`NO_SLOT`].
    pub(crate) notes: u32,
    /// A drum chain's sends, in `chain_sends`.
    pub(crate) sends: Span,
    /// The delay line that holds a drum chain's output back from the time
    /// its sends take it at to the rack's latency (drum racks R22), or
    /// [`NO_DELAY`].
    pub(crate) sum_delay: u32,
    /// Whether the chain's output is added into its rack's: all but a
    /// return chain sent to a return track.
    pub(crate) summed: bool,
}

impl RackChain {
    /// The slot to look a chain's note routing up in: a slot that is always
    /// there, though only a drum chain's holds a [`DrumNotes`].
    fn notes_slot(&self) -> u32 {
        if self.notes == NO_SLOT {
            self.mixer
        } else {
            self.notes
        }
    }

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
    /// Whether the node is a modulator, and how it works its output out.
    pub(crate) modulator: ModKind,
    /// For an external instrument (WP-5.34): the interface inputs its sound
    /// comes back on, which the program maps into the block it is given.
    pub(crate) external: Option<Channel>,
    /// For an external effect (WP-5.35): the interface outputs it sends
    /// its signal to.
    pub(crate) external_to: Option<Channel>,
    /// For an external effect (WP-5.35): the buffer in `tap_pool` it writes
    /// the signal it sends to the interface into.
    pub(crate) external_out: Option<u32>,
    /// For a device with extra audio outputs (WP-5.45): its buffers in
    /// `tap_pool`, one per output in the order the device declares them,
    /// which the program hands it to render into. Empty for a device with
    /// none, or one no route reads.
    pub(crate) outputs: Span,
}

/// How a modulator node works its rows out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModKind {
    /// Not a modulator.
    None,
    /// [`EngineProgram::run`] works its output out before the tasks
    /// (WP-5.10), and it leaves the chain's signal as it is.
    Control,
    /// It works its output out while its track runs, hearing the audio,
    /// notes or MIDI at its place in the chain (WP-5.11); its rows sit at
    /// this place among the bus's modulators.
    Signal(u32),
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
    /// The device, in `slots`, whose extra output it carries, or
    /// [`NO_TASK`].
    pub(crate) device: u32,
    /// The device's output's buffer in `tap_pool`, [`NO_OUTPUT`] for an
    /// input that carries the task's own signal, or [`NO_OUTPUT_BUFFER`] for
    /// an extra output the program has no buffer for.
    pub(crate) output: u32,
    /// The frames into the source task's chain at which the device's output
    /// is, for delay compensation, with an output.
    pub(crate) before: u32,
    /// The delay line its signal is delayed through to line it up with the
    /// destination's other inputs (WP-2.11), or [`NO_DELAY`].
    pub(crate) delay: u32,
}

/// An input that carries a task's own signal, not a rack chain's.
pub(crate) const NO_CHAIN: u32 = u32::MAX;

/// An input that carries a task's own signal, not a device's extra output.
pub(crate) const NO_OUTPUT: u32 = u32::MAX;

/// An input that carries a device's extra output the program has no buffer
/// for: the device has no node in this program, its type declares no extra
/// outputs, or the output index is past those it declares. The route carries
/// nothing.
pub(crate) const NO_OUTPUT_BUFFER: u32 = u32::MAX - 1;

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
pub(crate) fn delay_storage(lengths: &[u32], channels: usize) -> (Shared<DelayLine>, Shared<f32>) {
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
    (Shared::new(lines), Shared::new(vec![0.0; total]))
}

/// Delays `from` by its line into `scratch`, writing `from` into the line.
/// `scratch` and `from` are planar with `channels` channels `stride` apart,
/// and neither may alias `ring`, the samples the line holds: `channels`
/// channels of its `len` frames.
fn delay_into(
    line: &mut DelayLine,
    ring: &mut [f32],
    scratch: &mut [f32],
    from: &[f32],
    channels: usize,
    stride: usize,
    frames: usize,
) {
    let len = line.len as usize;
    for c in 0..channels {
        let base = c * len;
        let source = &from[c * stride..c * stride + frames];
        let out = &mut scratch[c * stride..c * stride + frames];
        let mut w = line.write as usize;
        for i in 0..frames {
            let at = base + w;
            out[i] = ring[at];
            ring[at] = source[i];
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
    ring: &mut [f32],
    samples: &mut [f32],
    channels: usize,
    stride: usize,
    frames: usize,
) {
    let len = line.len as usize;
    for c in 0..channels {
        let base = c * len;
        let channel = &mut samples[c * stride..c * stride + frames];
        let mut w = line.write as usize;
        for sample in channel.iter_mut() {
            let at = base + w;
            std::mem::swap(&mut ring[at], &mut *sample);
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
#[expect(clippy::too_many_arguments, reason = "the input's mapping")]
fn map_device_input(
    scratch: &mut [f32],
    input: &[f32],
    device_channels: usize,
    channel: Channel,
    channels: usize,
    stride: usize,
    offset: usize,
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
                // The interface's input for this part of the block, which
                // starts at the part's first frame.
                let from = &input[index * stride + offset..index * stride + offset + frames];
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

/// A device's MIDI sidechain in a task's chain (WP-5.27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeviceMidiSidechain {
    /// The device's slot.
    pub(crate) device: u32,
    /// The task whose notes it hears, or [`NO_TASK`].
    pub(crate) from: u32,
    /// The source's notes in `midi_kept`, or [`NO_TASK`] with no source.
    pub(crate) kept: u32,
}

/// No source task: a MIDI sidechain at No Input.
pub(crate) const NO_TASK: u32 = u32::MAX;

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
    /// The MIDI sidechains of its devices, in `midi_sidechains`.
    pub(crate) midi_sidechains: Span,
    /// Where it keeps the notes it plays for the MIDI sidechains that
    /// listen to it, in `midi_kept`.
    pub(crate) midi_keep: Option<u32>,
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
    pub fn track_order(&self) -> impl Iterator<Item = TrackId> + '_ {
        self.order
            .iter()
            .filter(|&&task| Some(task) != self.cue)
            .map(|&task| self.tasks[task as usize].track)
    }

    /// The tempo map the program converts ticks with.
    #[must_use]
    pub fn tempo(&self) -> &TempoMap {
        &self.tempo
    }

    /// The time-signature map the program reads bar lines from.
    #[must_use]
    pub fn time_signatures(&self) -> &TimeSignatureMap {
        &self.time_signatures
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
        for slot in self.slots.iter_mut() {
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
        for slot in self.slots.iter() {
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

    /// Adds to `out`, up to its capacity, the chains the last block found
    /// processing (WP-5.3, R37), in the order of the program's racks.
    /// Real-time safe: `out` was made with room for them.
    pub(crate) fn processing(&self, out: &mut Vec<ChainId>) {
        for chain in &self.rack_chains {
            let slot = &self.slots[chain.zones as usize];
            if let (NodeKey::ChainZones(id), Some(Instance::Zones(zones))) = (slot.key, &slot.node)
                && zones.processing()
                && out.len() < out.capacity()
            {
                out.push(id);
            }
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

    /// How long the task ran, smoothed over the blocks, in nanoseconds: the
    /// engine times each task on the thread that ran it (the mixer spec,
    /// R31). The first measure is the value; the ones after it move a
    /// eighth of the way towards the last, so the indicator moves rather
    /// than jumps. Real-time safe: an atomic read and a write.
    pub(crate) fn record_time(&self, task: usize, took: std::time::Duration) {
        let Some(slot) = self.task_times.get(task) else {
            return;
        };
        let nanos = took.as_nanos() as u64;
        let previous = slot.load(core::sync::atomic::Ordering::Relaxed);
        let smoothed = if previous == 0 {
            nanos
        } else {
            let difference = nanos as i64 - previous as i64;
            (previous as i64 + difference / 8).max(0) as u64
        };
        slot.store(smoothed, core::sync::atomic::Ordering::Relaxed);
    }

    /// How long the tasks ran in all, in nanoseconds: the set's processing
    /// time, which the Performance Impact indicator's shares are of (the
    /// mixer spec, R31).
    pub(crate) fn total_time(&self) -> u64 {
        self.task_times
            .iter()
            .map(|time| time.load(core::sync::atomic::Ordering::Relaxed))
            .sum()
    }

    /// The task's share of the set's processing time, from 0 to 1: what the
    /// Performance Impact indicator shows (the mixer spec, R31).
    pub(crate) fn impact(&self, task: u32, total: u64) -> f32 {
        if total == 0 {
            return 0.0;
        }
        let time = self.task_times[task as usize].load(core::sync::atomic::Ordering::Relaxed);
        (time as f64 / total as f64) as f32
    }

    /// The most gain reduction any of a task's track's compressors or
    /// limiters reported for the last block, in dB, as a compressor's Gain
    /// Reduction reads it: zero when none of them reduced. It reaches every
    /// device at any depth inside a rack (the mixer view spec, R40).
    pub(crate) fn gain_reduction(&self, task: u32) -> f32 {
        let mut most = 0.0f32;
        self.chain_gain_reduction(self.tasks[task as usize].chain, &mut most);
        most
    }

    /// Folds the gain reduction of every device in `chain`, racks at any
    /// depth included, into `most` (the mixer view spec, R40). A device
    /// that's switched off processes nothing, so it counts as reducing
    /// nothing.
    fn chain_gain_reduction(&self, chain: Span, most: &mut f32) {
        for &step in &self.steps[chain.range()] {
            match step {
                Step::Node(slot) => {
                    let Some(slot) = self.slots.get(slot as usize) else {
                        continue;
                    };
                    if let Some(Instance::Device(node)) = &slot.node
                        && !slot.bypass
                    {
                        *most = most.min(node.gain_reduction());
                    }
                }
                Step::Rack(rack) => {
                    for chain in self.racks[rack as usize].all().range() {
                        self.chain_gain_reduction(self.rack_chains[chain].steps, most);
                    }
                }
            }
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
                    self.rack_chains[self.racks[rack as usize].all().range().start].mixer
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
        // Spec: IMP-15. A device that was switched off in this swap left the
        // chain: it ends the notes it started, and they enter the chain in
        // front of the node that followed it.
        for s in 0..old.slots.len() {
            let slot = s as u32;
            if !matches!(old.slots[s].key, NodeKey::Device(_)) {
                continue;
            }
            let Some(from_task) = old_task_of(old, slot) else {
                continue;
            };
            let Some(to) = self
                .migrations
                .iter()
                .find(|m| m.from == slot)
                .map(|m| m.to)
            else {
                continue;
            };
            if !self.slots[to as usize].bypass {
                continue;
            }
            let Some(task) = self
                .tasks
                .iter()
                .position(|t| t.track == old.tasks[from_task].track)
            else {
                continue;
            };
            let at = gap(self, old, task, from_task, slot);
            if let Some(Instance::Device(node)) = self.slots[to as usize].node.as_mut() {
                self.releases.take(node.as_mut(), task as u32, at);
            }
        }
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
    /// slot then frame; each node and route gets those in its part, `lane`
    /// holding them. With a `pool` of workers the audio thread and the
    /// workers share the tasks; without one the audio thread runs them in
    /// `order`. Returns how many changes reached a node or a route.
    pub(crate) fn run(
        &mut self,
        pool: Option<&mut Pool>,
        lane: &mut Lane,
        mut block: Blk<'_>,
    ) -> u64 {
        block.frames = block.frames.min(self.max_frames);
        block.until = block.from + block.frames as u32;
        // Spec: modulation R20. The modulators work their rows out first, on
        // this thread alone; the tasks only read them.
        let modulated = self.run_modulators(
            block.frames,
            block.from,
            block.until,
            block.events,
            &mut lane.events,
            block.transport,
        );
        let delivered = modulated
            + match pool {
                Some(pool) => pool.run(self, lane, &block),
                None => self.run_serial(lane, &block),
            };
        self.releases.pending.clear();
        delivered
    }

    /// Runs the tasks one after another on this thread, in `order`.
    pub(crate) fn run_serial(&self, lane: &mut Lane, block: &Blk<'_>) -> u64 {
        let mut delivered = 0;
        for &t in &self.order {
            // SAFETY: `order` runs every task after the tasks that feed it,
            // and each once, on this thread alone.
            delivered += unsafe { self.run_task(t, lane, block, None) };
        }
        delivered
    }

    /// Runs task `t` over the block: its clips, its inputs, its devices,
    /// and its mixer. With `wake`, the threads of a pool are working on
    /// the block too, and a rack's chains are offered to them. Returns how
    /// many changes reached a node or a route.
    ///
    /// # Safety
    ///
    /// Every task that feeds `t` has finished this block, and no thread
    /// but this one (and, through `wake`, the ones a rack's chains are
    /// offered to) runs `t` in it.
    pub(crate) unsafe fn run_task(
        &self,
        t: u32,
        lane: &mut Lane,
        block: &Blk<'_>,
        wake: Option<&Wake>,
    ) -> u64 {
        let t = t as usize;
        let task = self.tasks[t];
        // Spec: the mixer spec, R31. The task is timed on the thread that
        // ran it, so the pool's workers' work counts too.
        let began = std::time::Instant::now();
        // SAFETY: the caller's promises are the turn's.
        let turn = unsafe { Turn::new(self, t) };
        let (channels, stride, frames) = (self.channels, self.max_frames, block.frames);
        let size = channels * stride;
        let Blk {
            from,
            until,
            events,
            transport,
            session,
            groove_amount,
            set_scale,
            ..
        } = *block;
        // Where the block sits in the song, for the automation a route plays
        // (WP-2.12): no time while the song is stopped.
        let song = transport.playing.then(|| crate::automation::Song {
            frame: transport.position.get(),
            session,
            tempo: &self.tempo,
            rate: self.sample_rate,
        });
        let shape = crate::mixer::Planar {
            channels,
            stride,
            frames,
        };
        let mut delivered = 0;
        for c in 0..channels {
            turn.buffer(t)[c * stride..c * stride + frames].fill(0.0);
        }

        let notes = 2 * t;
        turn.notes(notes).clear();

        // The clips first, alone in the buffer, so the player's gate
        // (Monitor In silences them) scales only them.
        if let Some(slot) = task.player {
            delivered += gather(events, slot, from, until, &mut lane.events);
            if let Some(node) = turn.slot(slot).node.as_mut().and_then(Instance::as_node) {
                node.process(&mut Block {
                    data: turn.buffer(t),
                    channels,
                    frames,
                    stride,
                    events: lane.events.as_slice(),
                    transport,
                    sample_rate: self.sample_rate,
                    tempo: &self.tempo,
                    signatures: &self.time_signatures,
                    sidechain: None,
                    external: None,
                    external_out: None,
                    outputs: None,
                    notes: &[],
                    notes_out: Some(turn.notes(notes)),
                    session,
                    groove_amount,
                    set_scale,
                    scale: ScaleRuns::of(set_scale),
                    modulation: turn.bus(frames),
                    rows: None,
                });
            }
        }

        // Spec: midi-io R12. What the track's input played merges with
        // its clips' notes, on its own frames.
        self.midi.play(task.track, from, until, turn.notes(notes));

        // Spec: routing R32. What the track played, kept for the MIDI
        // sidechains that listen to it.
        if let Some(kept) = task.midi_keep {
            let to = turn.keep_notes(kept);
            to.clear();
            for &note in turn.notes(notes).notes() {
                to.push(note);
            }
        }

        // Its clips, held back to line up with its inputs.
        if task.clip_delay != NO_DELAY {
            let (line, ring) = turn.delay(task.clip_delay);
            delay_in_place(line, ring, turn.buffer(t), channels, stride, frames);
        }

        // Spec: routing R11. Whether the track played a clip this block;
        // under Monitor Auto its input routes close while one did. The
        // player ran first, so it knows.
        let playing_clip = match task.player {
            Some(slot) => match turn.slot(slot).node.as_ref() {
                Some(Instance::Device(node)) => node.playing_clip(),
                _ => false,
            },
            None => false,
        };
        // Spec: scales R15. The player knows which clip plays, and so which
        // scale the devices of the track, in its racks too, read; a track
        // without a player reads the set's.
        let scale = match task.player.map(|slot| turn.slot(slot).node.as_ref()) {
            Some(Some(Instance::Device(node))) => node.track_scale(),
            _ => None,
        }
        .unwrap_or_else(|| ScaleRuns::of(set_scale));
        let block = &Blk { scale, ..*block };
        // The interface's input, if the track takes Ext. In: its
        // channels, through the route whose Monitor gate opens and
        // closes it (WP-2.13).
        if let Some(device) = task.device_input {
            delivered += gather(events, device.route, from, until, &mut lane.events);
            map_device_input(
                &mut lane.device,
                &self.device_input,
                self.device_input_channels,
                device.channel,
                channels,
                stride,
                from as usize,
                frames,
            );
            if let Some(Instance::Route(route)) = turn.slot(device.route).node.as_mut() {
                route.set_playing_clip(playing_clip);
                route.mix(
                    turn.buffer(t),
                    &lane.device,
                    shape,
                    None,
                    Bus::EMPTY,
                    &lane.events,
                );
            }
        }

        // Add the inputs, each through its route.
        for &input in &self.task_inputs[task.inputs.range()] {
            delivered += gather(events, input.route, from, until, &mut lane.events);
            let Some(Instance::Route(route)) = turn.slot(input.route).node.as_mut() else {
                continue;
            };
            route.set_playing_clip(playing_clip);
            let source = input.from as usize;
            // An extra output the program has no buffer for — its device has
            // no node here, or the output isn't one its type declares —
            // carries nothing.
            if input.output == NO_OUTPUT_BUFFER {
                continue;
            }
            // The task's own Post Mixer is its buffer in `pool`; a
            // device's extra output, and a rack chain's tap, are copies in
            // `tap_pool`.
            let kept = match (input.chain, input.tap) {
                _ if input.output != NO_OUTPUT => Some(input.output),
                (NO_CHAIN, Tap::PostMixer) => None,
                (NO_CHAIN, Tap::PreFx) => self.tasks[source].pre_fx,
                (NO_CHAIN, Tap::PostFx) => self.tasks[source].post_fx,
                (chain, tap) => self.rack_chains[chain as usize].taps[RackChain::tap_index(tap)],
            };
            let theirs = match kept {
                Some(buffer) => turn.kept(buffer),
                // A task can't feed itself its own mixer's output.
                None if input.chain == NO_CHAIN && input.tap == Tap::PostMixer && source != t => {
                    turn.finished(source)
                }
                None => continue,
            };
            if input.delay != NO_DELAY {
                // Hold the source back, then add what comes out.
                let (line, ring) = turn.delay(input.delay);
                let held = &mut lane.delay[..size];
                delay_into(line, ring, held, theirs, channels, stride, frames);
                let bus = turn.bus(frames);
                route.mix(turn.buffer(t), held, shape, song, bus, &lane.events);
            } else {
                let bus = turn.bus(frames);
                route.mix(turn.buffer(t), theirs, shape, song, bus, &lane.events);
            }
        }

        // Keep the signal before the devices for routes that tap it.
        if let Some(buffer) = task.pre_fx {
            self.keep(&turn, buffer, t, frames);
        }

        // Sum each sidechain's sources into its buffer.
        for s in task.sidechains.range() {
            delivered += self.sum_sidechain(&turn, s, block, lane);
        }

        // Run the chain in place.
        let mut pass = Pass {
            turn,
            block,
            scratch: &mut lane.events,
            device: &mut lane.device,
            wake,
            delivered: 0,
        };
        let ended = self.run_steps(&mut pass, task, task.chain, t, notes, 0);
        delivered += pass.delivered;

        // Spec: external-devices R3. What reached the end of the chain
        // leaves for the port the track's MIDI To names.
        self.midi.send(
            OutOwner::Track(task.track),
            turn.notes(notes + ended).notes(),
            from,
        );

        // Keep the chain's output for pre-fader sends and Post FX
        // inputs.
        if let Some(buffer) = task.post_fx {
            self.keep(&turn, buffer, t, frames);
        }

        // The track's mixer.
        delivered += gather(events, task.mixer, from, until, &mut lane.events);
        if let Some(Instance::Mixer(mixer)) = turn.slot(task.mixer).node.as_mut() {
            mixer.process(&mut Block {
                data: turn.buffer(t),
                channels,
                frames,
                stride,
                events: lane.events.as_slice(),
                transport,
                sample_rate: self.sample_rate,
                tempo: &self.tempo,
                signatures: &self.time_signatures,
                sidechain: None,
                external: None,
                external_out: None,
                outputs: None,
                notes: &[],
                notes_out: None,
                session,
                groove_amount,
                set_scale,
                scale: ScaleRuns::of(set_scale),
                modulation: turn.bus(frames),
                rows: None,
            });
        }
        self.record_time(t, began.elapsed());
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
            let signal = matches!(self.slots[slot as usize].modulator, ModKind::Signal(_));
            if !signal {
                delivered += gather(events, slot, from, until, scratch);
            }
            let at = m * ROWS * stride;
            let mut ctl = Control {
                frames,
                events: if signal { &[] } else { scratch.as_slice() },
                transport,
                sample_rate: self.sample_rate,
                tempo: &self.tempo,
                // SAFETY: no task runs yet, and the bus reads only the rows
                // before the ones `out` holds.
                earlier: unsafe { Bus::new(&self.bus, stride, frames) }.below(m * ROWS),
                // SAFETY: as above; nothing else reaches these rows.
                out: unsafe { self.bus.elements(at..at + ROWS * stride) },
                stride,
            };
            let NodeSlot { node, bypass, .. } = &mut self.slots[slot as usize];
            let Some(node) = node.as_mut().and_then(Instance::as_node) else {
                ctl.idle();
                continue;
            };
            if signal {
                ctl.idle();
            } else if *bypass {
                node.flush(ctl.events);
                ctl.idle();
            } else if !node.control(&mut ctl) {
                ctl.idle();
            }
        }
        // What a signal modulator wrote 32 frames ago, for the targets that
        // run before it.
        for k in 0..self.signals.len() {
            let slot = self.modulators[self.signals[k] as usize];
            let at = (self.modulators.len() + k) * ROWS * stride;
            let mut ctl = Control {
                frames,
                events: &[],
                transport,
                sample_rate: self.sample_rate,
                tempo: &self.tempo,
                earlier: Bus::EMPTY,
                // SAFETY: no task runs yet, and nothing else reads these rows.
                out: unsafe { self.bus.elements(at..at + ROWS * stride) },
                stride,
            };
            let NodeSlot { node, bypass, .. } = &mut self.slots[slot as usize];
            let wrote = match node.as_mut().and_then(Instance::as_node) {
                Some(node) if !*bypass => node.control(&mut ctl),
                _ => false,
            };
            if !wrote {
                ctl.idle();
            }
        }
        delivered
    }

    /// Sums sidechain `s`'s sources into its buffer, each through its
    /// route, over the block's frames. Returns how many changes reached a
    /// route.
    fn sum_sidechain(&self, turn: &Turn<'_>, s: usize, block: &Blk<'_>, lane: &mut Lane) -> u64 {
        // A sidechain's route has no level to automate, so it plays none.
        let (channels, stride, frames) = (self.channels, self.max_frames, block.frames);
        let shape = crate::mixer::Planar {
            channels,
            stride,
            frames,
        };
        let size = channels * stride;
        let sidechain = self.sidechains[s];
        let at = sidechain.buffer as usize;
        for c in 0..channels {
            turn.sidechain_mut(at)[c * stride..c * stride + frames].fill(0.0);
        }
        let mut delivered = 0;
        for &input in &self.sidechain_inputs[sidechain.inputs.range()] {
            delivered += gather(
                block.events,
                input.route,
                block.from,
                block.until,
                &mut lane.events,
            );
            let Some(Instance::Route(route)) = turn.slot(input.route).node.as_mut() else {
                continue;
            };
            let source = &self.tasks[input.from as usize];
            let theirs = match input.tap {
                Tap::PreFx => source.pre_fx.map(|b| turn.kept(b)),
                Tap::PostFx => source.post_fx.map(|b| turn.kept(b)),
                Tap::PostMixer => Some(turn.finished(input.from as usize)),
            };
            let Some(theirs) = theirs else {
                continue;
            };
            if input.delay != NO_DELAY {
                let (line, ring) = turn.delay(input.delay);
                let held = &mut lane.delay[..size];
                delay_into(line, ring, held, theirs, channels, stride, frames);
                route.mix(
                    turn.sidechain_mut(at),
                    held,
                    shape,
                    None,
                    Bus::EMPTY,
                    &lane.events,
                );
            } else {
                route.mix(
                    turn.sidechain_mut(at),
                    theirs,
                    shape,
                    None,
                    Bus::EMPTY,
                    &lane.events,
                );
            }
        }
        delivered
    }

    /// Fills `frames` frames of the tap buffer `buffer` with silence.
    fn silence(&self, turn: &Turn<'_>, buffer: u32, frames: usize) {
        let to = turn.keep(buffer);
        for c in 0..self.channels {
            let at = c * self.max_frames;
            to[at..at + frames].fill(0.0);
        }
    }

    /// Copies `frames` frames of the pool buffer `own` into the tap buffer
    /// `buffer`.
    fn keep(&self, turn: &Turn<'_>, buffer: u32, own: usize, frames: usize) {
        let from = turn.buffer(own);
        let to = turn.keep(buffer);
        for c in 0..self.channels {
            let at = c * self.max_frames;
            to[at..at + frames].copy_from_slice(&from[at..at + frames]);
        }
    }

    /// The output's channel `c` from the last run, `frames` long.
    pub(crate) fn output(&self, c: usize, frames: usize) -> &[f32] {
        self.task_output(self.output, c, frames)
    }

    /// Task `task`'s channel `c` from the last run, `frames` long.
    pub(crate) fn task_output(&self, task: u32, c: usize, frames: usize) -> &[f32] {
        let start = task as usize * self.channels * self.max_frames + c * self.max_frames;
        &self.pool[start..start + frames.min(self.max_frames)]
    }

    /// The task a track's signal is in, for a real-time render's capture
    /// (WP-5.35).
    pub(crate) fn task_of(&self, track: bl_model::TrackId) -> Option<u32> {
        self.tasks
            .iter()
            .position(|task| task.track == track)
            .and_then(|at| u32::try_from(at).ok())
    }

    /// How many device channels the Ext. Out tracks reach: one past the
    /// highest.
    pub(crate) fn ext_width(&self) -> usize {
        let tracks = self
            .ext_outs
            .iter()
            .map(|e| match e.channel {
                Channel::Mono(c) => usize::from(c),
                Channel::Pair(c) => usize::from(c) + 1,
            })
            .max()
            .unwrap_or(0);
        // Spec: external-devices R18. An external effect sends its signal to
        // the interface's outputs too.
        let effects = self
            .external_effects
            .iter()
            .map(|(_, channel)| match channel {
                Channel::Mono(c) => usize::from(*c),
                Channel::Pair(c) => usize::from(*c) + 1,
            })
            .max()
            .unwrap_or(0);
        tracks.max(effects)
    }

    /// Adds the external effects' signals from the last run onto `out`,
    /// which holds `width` device channels `stride` apart, `length` frames
    /// from `offset`. A pair plays left and right on its two channels, a
    /// single channel takes both added and 6 dB down (the manual, 17.2.1);
    /// a channel past `width` is the device's absence and plays nothing.
    /// Spec: external-devices R18.
    pub(crate) fn mix_external_effects(
        &self,
        out: &mut [f32],
        width: usize,
        stride: usize,
        offset: usize,
        length: usize,
    ) {
        let size = self.channels * self.max_frames;
        for (slot, channel) in &self.external_effects {
            let Some(buffer) = self.slots[*slot as usize].external_out else {
                continue;
            };
            let at = buffer as usize * size;
            let data = &self.tap_pool[at..at + size];
            let left = &data[..length.min(self.max_frames)];
            let right = &data[self.max_frames..self.max_frames + length.min(self.max_frames)];
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
            match channel {
                Channel::Mono(c) => add(usize::from(*c), 0.5, 0.5),
                Channel::Pair(c) => {
                    add(usize::from(*c), 1.0, 0.0);
                    add(usize::from(*c) + 1, 0.0, 1.0);
                }
            }
        }
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

/// One block's frames and what goes with them, shared by every thread that
/// runs tasks in it.
#[derive(Clone, Copy)]
pub(crate) struct Blk<'a> {
    /// The frames to run: the part of the device block that starts at
    /// `from`.
    pub(crate) frames: usize,
    pub(crate) from: u32,
    pub(crate) until: u32,
    /// The block's parameter changes, sorted by slot then frame.
    pub(crate) events: &'a [Resolved],
    pub(crate) transport: TransportInfo,
    pub(crate) session: &'a [SessionPlayback],
    /// The set's Global Amount of groove, 1 at 100 % (the grooves spec,
    /// R21).
    pub(crate) groove_amount: f32,
    /// The set's current scale (the scales spec, R9).
    pub(crate) set_scale: Scale,
    /// The scale of the clip playing on the task's track over the block (the
    /// scales spec, R15): the set's until the task's clip player has run.
    pub(crate) scale: ScaleRuns,
}

/// A thread's hold on the parts of the program that one task owns, while it
/// runs that task (or a chain of one of its racks that the task's thread
/// offered).
///
/// Every part of a program that a run writes belongs to one task: the
/// nodes and routes of its chain, its racks' chains and mixers, its
/// sidechains and clip player; its buffer, its rack chains' buffers, the
/// copies it keeps of its signal, and its sidechains' buffers; the delay
/// lines of its inputs, its clips and its rack chains; its pair of note
/// buffers and its rack chains', and the notes it keeps for MIDI
/// sidechains; its rack chains' merge cursors; and its track's MIDI input. The compiler gives each part to one task and one
/// place in it, so two threads running different tasks, or different chains
/// of one rack, never reach the same part. What a task reads from another
/// is that task's finished output, or the copy it kept, which the
/// dependencies make final before this task starts.
///
/// The methods hand out those parts through [`Shared`]'s `unsafe`
/// accessors. They are not `unsafe` themselves: asking for a part the task
/// doesn't own is the compiler's mistake, not a caller's, and
/// `docs/specs/racks.md` R15 and the scheduler tests keep it honest. The
/// references they return must not outlive their use in one step of the
/// run.
#[derive(Clone, Copy)]
pub(crate) struct Turn<'p> {
    program: &'p EngineProgram,
    task: usize,
}

impl<'p> Turn<'p> {
    /// The hold of the thread running task `task` in this block.
    ///
    /// # Safety
    ///
    /// Every task that feeds `task` has finished this block, and no other
    /// thread holds a turn on `task`, except ones working on different
    /// chains of its racks while this thread waits for them.
    pub(crate) unsafe fn new(program: &'p EngineProgram, task: usize) -> Self {
        Self { program, task }
    }

    fn size(&self) -> usize {
        self.program.channels * self.program.max_frames
    }

    /// Pool buffer `index`: the task's own (`index` is the task), or one of
    /// its rack chains' (past the tasks').
    pub(crate) fn buffer(&self, index: usize) -> &'p mut [f32] {
        debug_assert!(
            index == self.task || index >= self.program.tasks.len(),
            "task {} reached task {index}'s buffer",
            self.task
        );
        let size = self.size();
        // SAFETY: the task owns this buffer, and the turn's callers use the
        // returned reference before asking for another to the same one.
        unsafe { self.program.pool.elements(index * size..(index + 1) * size) }
    }

    /// The modulators' rows over `frames` frames, for the nodes of the task
    /// to read.
    pub(crate) fn bus(&self, frames: usize) -> Bus<'p> {
        // SAFETY: a target reads a signal modulator's direct row only if
        // its track waits for the modulator's task, or its place in the
        // chain comes after the modulator's, in a chain no thread runs at
        // once with the modulator's; any other target reads the late row,
        // which only the thread before the tasks writes (R28).
        unsafe { Bus::new(&self.program.bus, self.program.max_frames, frames) }
    }

    /// The direct rows of the signal modulator at place `m` among the
    /// modulators, to write.
    fn signal_rows(&self, m: u32) -> &'p mut [f32] {
        let len = ROWS * self.program.max_frames;
        let at = m as usize * len;
        // SAFETY: only the task that holds the modulator's node writes its
        // rows, and the compiler gives the node to one task.
        unsafe { self.program.bus.elements(at..at + len) }
    }

    /// Task `index`'s buffer once the task has finished its mixer.
    pub(crate) fn finished(&self, index: usize) -> &'p [f32] {
        let size = self.size();
        // SAFETY: the task has finished and nothing writes its buffer for
        // the rest of the block (the dependencies); other threads may read
        // it too.
        unsafe { self.program.pool.view(index * size..(index + 1) * size) }
    }

    /// Tap buffer `index`, to write the copy the task keeps of its signal.
    fn keep(&self, index: u32) -> &'p mut [f32] {
        let size = self.size();
        let at = index as usize * size;
        // SAFETY: only the task that keeps it writes it.
        unsafe { self.program.tap_pool.elements(at..at + size) }
    }

    /// Tap buffer `index`, read: the task's own copy, or a copy another
    /// task made and finished before this one started.
    fn kept(&self, index: u32) -> &'p [f32] {
        let size = self.size();
        let at = index as usize * size;
        // SAFETY: the task that keeps it wrote it before it finished, or
        // is this one and isn't writing it now.
        unsafe { self.program.tap_pool.view(at..at + size) }
    }

    /// A device's extra output buffers, one tap buffer each, to write
    /// (WP-5.45): only the device's own task writes them.
    fn outputs(&self, span: Span) -> &'p mut [f32] {
        let size = self.size();
        let at = span.start as usize * size;
        let end = span.end as usize * size;
        // SAFETY: only the task whose chain holds the device writes them,
        // and the routes that read them run after it.
        unsafe { self.program.tap_pool.elements(at..end) }
    }

    /// Sidechain buffer `index`, to write what the detector hears.
    fn sidechain_mut(&self, index: usize) -> &'p mut [f32] {
        let size = self.size();
        // SAFETY: only the task of the sidechain's device writes it.
        unsafe {
            self.program
                .sidechain_pool
                .elements(index * size..(index + 1) * size)
        }
    }

    /// Sidechain buffer `index`, read.
    fn sidechain(&self, index: usize) -> &'p [f32] {
        let size = self.size();
        // SAFETY: only the task of the sidechain's device reaches it, and
        // it isn't writing it now.
        unsafe {
            self.program
                .sidechain_pool
                .view(index * size..(index + 1) * size)
        }
    }

    /// The node slot `slot`, which belongs to the task.
    pub(crate) fn slot(&self, slot: u32) -> &'p mut NodeSlot {
        // SAFETY: the compiler puts each slot in one task.
        unsafe { self.program.slots.element(slot as usize) }
    }

    /// Note buffer `index`: one of the task's pair, or one of its rack
    /// chains'.
    fn notes(&self, index: usize) -> &'p mut NoteBuffer {
        // SAFETY: the pairs are the task's and its chains' alone.
        unsafe { self.program.note_buffers.element(index) }
    }

    /// Kept-notes buffer `index`, to write the notes the task played.
    fn keep_notes(&self, index: u32) -> &'p mut NoteBuffer {
        // SAFETY: only the task that is the source of the MIDI sidechains
        // that read it writes it.
        unsafe { self.program.midi_kept.element(index as usize) }
    }

    /// Kept-notes buffer `index`, read: the notes a source task played,
    /// finished before this task started or the task's own.
    fn kept_notes(&self, index: u32) -> &'p NoteBuffer {
        // SAFETY: the task that keeps it wrote it before it finished, or
        // is this one and isn't writing it now.
        unsafe {
            &self
                .program
                .midi_kept
                .view(index as usize..index as usize + 1)[0]
        }
    }

    /// Two different note buffers of the task, in the order asked.
    fn note_pair(&self, first: usize, second: usize) -> [&'p mut NoteBuffer; 2] {
        assert_ne!(first, second, "a pair of note buffers is two");
        [self.notes(first), self.notes(second)]
    }

    /// Delay line `index` of the task, and the ring of samples it keeps.
    fn delay(&self, index: u32) -> (&'p mut DelayLine, &'p mut [f32]) {
        // SAFETY: each line belongs to one input, one task's clips, or one
        // rack chain; its ring is that line's alone.
        unsafe {
            let line = self.program.delay_lines.element(index as usize);
            let start = line.at as usize;
            let len = self.program.channels * line.len as usize;
            (line, self.program.delay_pool.elements(start..start + len))
        }
    }

    /// The selector's values for rack `rack` (in `racks`), to write.
    fn selects_mut(&self, rack: usize) -> &'p mut [f32] {
        let size = self.program.max_frames;
        // SAFETY: only the rack's owner writes them, before its chains run.
        unsafe {
            self.program
                .selects
                .elements(rack * size..(rack + 1) * size)
        }
    }

    /// The selector's values for rack `rack`, to read.
    fn selects(&self, rack: usize) -> &'p [f32] {
        let size = self.program.max_frames;
        // SAFETY: the rack's owner wrote them before its chains ran, and
        // nothing writes them until the block is over.
        unsafe { self.program.selects.view(rack * size..(rack + 1) * size) }
    }

    /// The merge cursor of rack chain `index` (in `rack_chains`).
    fn cursor(&self, index: usize) -> &'p mut MergeCursor {
        // SAFETY: one cursor per chain, and only its rack's owner and the
        // thread running the chain reach it, one after the other.
        unsafe { self.program.merge.element(index) }
    }
}

/// What a run over a block hands down to each chain: the thread's access to
/// the task, where the block sits, the parameter changes for it, and the
/// count of those delivered.
struct Pass<'a> {
    turn: Turn<'a>,
    block: &'a Blk<'a>,
    scratch: &'a mut Vec<NodeEvent>,
    /// The thread's scratch for the interface's inputs mapped to the tracks'
    /// layout, for an external instrument (WP-5.34).
    device: &'a mut [f32],
    /// The threads working on the block with this one, if there are any.
    wake: Option<&'a Wake>,
    delivered: u64,
}

impl EngineProgram {
    /// Runs a chain's steps in place on the pool buffer `buffer`, with the
    /// notes in the pair of buffers starting at `pair`, whose `note_idx`th
    /// holds them now. Returns which of the pair holds the notes after.
    #[allow(clippy::too_many_lines)]
    fn run_steps(
        &self,
        pass: &mut Pass<'_>,
        task: Task,
        steps: Span,
        buffer: usize,
        pair: usize,
        mut note_idx: usize,
    ) -> usize {
        let turn = pass.turn;
        let block = pass.block;
        let (channels, stride, frames) = (self.channels, self.max_frames, block.frames);
        for step_at in steps.range() {
            // Spec: device-framework R32. The note-offs of devices that left
            // the track's chain in the swap enter in front of the step that
            // followed them, ahead of the notes already there. Only the
            // track's own chain, in task `buffer`, takes them.
            if buffer < self.tasks.len() {
                let at = step_at - steps.range().start;
                for &(_, _, note) in self
                    .releases
                    .pending
                    .iter()
                    .filter(|&&(task, to, _)| task as usize == buffer && to as usize == at)
                {
                    turn.notes(pair + note_idx).insert(note);
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
            let bypass = turn.slot(slot).bypass;
            // What the detector hears, made before the device runs.
            let mut heard = None;
            if let Some(sidechain) = sidechain {
                pass.delivered += gather(
                    block.events,
                    sidechain.key,
                    block.from,
                    block.until,
                    pass.scratch,
                );
                let at = sidechain.buffer as usize;
                if let Some(Instance::Sidechain(key)) = turn.slot(sidechain.key).node.as_mut() {
                    if bypass {
                        key.flush(pass.scratch);
                    } else {
                        key.make(
                            turn.sidechain_mut(at),
                            turn.buffer(buffer),
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
            let [in_buf, out_buf] = turn.note_pair(pair + note_idx, pair + 1 - note_idx);
            // Spec: routing R32. A device with a MIDI sidechain hears its
            // source's notes, or none at No Input, in place of the chain's.
            let heard_notes = match self.midi_sidechains[task.midi_sidechains.range()]
                .iter()
                .find(|m| m.device == slot)
            {
                Some(m) if m.kept == NO_TASK => Some(&[][..]),
                Some(m) => Some(turn.kept_notes(m.kept).notes()),
                None => None,
            };
            // A modulator worked out its rows before the tasks ran and
            // leaves the chain's audio and notes as they are.
            let modulator = turn.slot(slot).modulator;
            if modulator == ModKind::Control {
                out_buf.clear();
                for &note in in_buf.notes() {
                    out_buf.push(note);
                }
                note_idx = 1 - note_idx;
                continue;
            }
            pass.delivered += gather(block.events, slot, block.from, block.until, pass.scratch);
            let Some(node) = turn.slot(slot).node.as_mut().and_then(Instance::as_node) else {
                continue;
            };
            out_buf.clear();
            // Spec: external-devices R10. An external instrument sends what
            // reaches it to its port, and hears the interface's inputs it
            // is set to.
            let mut returned = None;
            if let (Some(audio_from), NodeKey::Device(device), false) =
                (turn.slot(slot).external, turn.slot(slot).key, bypass)
            {
                self.midi
                    .send(OutOwner::Device(device), in_buf.notes(), block.from);
                map_device_input(
                    pass.device,
                    &self.device_input,
                    self.device_input_channels,
                    audio_from,
                    channels,
                    stride,
                    block.from as usize,
                    frames,
                );
                returned = Some(&pass.device[..channels * stride]);
            }
            let outputs = turn.slot(slot).outputs;
            let output_tapped = outputs.start != outputs.end;
            // Spec: external-devices R18. An external effect writes the
            // signal it sends to the interface into a buffer of its own.
            let external_out = turn.slot(slot).external_out;
            if bypass {
                // A switched-off device renders nothing: its extra outputs
                // carry silence (WP-5.45).
                if output_tapped {
                    turn.outputs(outputs).fill(0.0);
                }
                if let Some(at) = external_out {
                    turn.keep(at).fill(0.0);
                }
                node.flush(pass.scratch);
                // Spec: device-framework R32. A device switched off ends
                // the notes it started, on this block's first frame.
                node.release(out_buf);
                if let (Some(_), NodeKey::Device(device)) =
                    (turn.slot(slot).external, turn.slot(slot).key)
                {
                    self.midi.release(OutOwner::Device(device));
                }
                for &note in in_buf.notes() {
                    out_buf.push(note);
                }
                note_idx = 1 - note_idx;
                continue;
            }
            node.process(&mut Block {
                data: turn.buffer(buffer),
                channels,
                frames,
                stride,
                events: pass.scratch.as_slice(),
                transport: block.transport,
                sample_rate: self.sample_rate,
                tempo: &self.tempo,
                signatures: &self.time_signatures,
                sidechain: heard.map(|(at, _)| turn.sidechain(at)),
                external: returned,
                external_out: external_out.map(|at| turn.keep(at)),
                outputs: output_tapped.then(|| turn.outputs(outputs)),
                notes: heard_notes.unwrap_or_else(|| in_buf.notes()),
                notes_out: Some(out_buf),
                session: block.session,
                groove_amount: block.groove_amount,
                set_scale: block.set_scale,
                scale: block.scale,
                // A modulator driven by the signal writes its own rows, and
                // hears the rows of the modulators before it.
                modulation: match modulator {
                    ModKind::Signal(m) => turn.bus(frames).below(m as usize * ROWS),
                    _ => turn.bus(frames),
                },
                rows: match modulator {
                    ModKind::Signal(m) => Some(turn.signal_rows(m)),
                    _ => None,
                },
            });
            note_idx = 1 - note_idx;
            // Sidechain Listen: the device's output is what it heard.
            if let Some((at, true)) = heard {
                let (to, from) = (turn.buffer(buffer), turn.sidechain(at));
                for c in 0..channels {
                    let edge = c * stride;
                    to[edge..edge + frames].copy_from_slice(&from[edge..edge + frames]);
                }
            }
        }
        note_idx
    }

    /// Runs a rack in place on the pool buffer `buffer` (the racks spec,
    /// R3 to R5, R15, R16): every chain takes the rack's input, a copy of
    /// its audio and notes, runs its devices and then its mixer, and is held
    /// back to the rack's latency; then the chains are added into the
    /// buffer in the chains' order. With threads to share the work (R15),
    /// the chains are offered to them, and the rack waits for every chain
    /// before it adds any, so the sum is the same whichever finishes first.
    /// A drum rack (the drum racks spec) gives each chain the notes it
    /// answers, lets a note choke the chains of its group, and after the
    /// chains feeds its return chains with their sends and runs them.
    /// Returns which of the pair of note buffers at `pair` holds the notes
    /// after.
    fn run_rack(
        &self,
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
            returns,
            bypass,
            ..
        } = self.racks[rack];
        if bypass {
            self.flush_rack(pass, task, rack);
            return note_idx;
        }
        if chains.start == returns.end {
            return note_idx;
        }
        let turn = pass.turn;
        let block = pass.block;
        let (channels, stride, frames) = (self.channels, self.max_frames, block.frames);
        let audio = mode != RackMode::Midi;
        let notes = mode != RackMode::Audio;
        let filter_ctrl = self.run_selector(pass, rack, buffer);
        let selector = &turn.selects(rack)[..frames];
        // Every chain takes the rack's input through its zones (R31 to R33).
        for i in chains.range() {
            let chain = self.rack_chains[i];
            let at = self.tasks.len() + chain.buffer as usize;
            let block = pass.block;
            pass.delivered += gather(
                block.events,
                chain.zones,
                block.from,
                block.until,
                pass.scratch,
            );
            let Some(Instance::Zones(zones)) = turn.slot(chain.zones).node.as_mut() else {
                continue;
            };
            zones.apply(pass.scratch);
            for c in 0..channels {
                let edge = c * stride;
                let to = &mut turn.buffer(at)[edge..edge + frames];
                if audio {
                    to.copy_from_slice(&turn.buffer(buffer)[edge..edge + frames]);
                } else {
                    to.fill(0.0);
                }
            }
            if mode == RackMode::Audio {
                zones.gate(selector, turn.buffer(at), channels, stride);
            }
            if notes {
                let to = 2 * self.tasks.len() + 2 * chain.buffer as usize;
                let [from, to] = turn.note_pair(pair + note_idx, to);
                match turn.slot(chain.notes_slot()).node.as_mut() {
                    // Spec: drum-racks R2. A drum chain gets the notes it
                    // answers, at its Play note.
                    Some(Instance::Drum(drum)) => {
                        pass.delivered += gather(
                            block.events,
                            chain.notes,
                            block.from,
                            block.until,
                            pass.scratch,
                        );
                        drum.route(from.notes(), to, pass.scratch);
                    }
                    _ => {
                        to.clear();
                        zones.filter(
                            selector,
                            from.notes(),
                            to,
                            filter_ctrl,
                            mode == RackMode::Midi,
                        );
                    }
                }
            }
        }
        if mode == RackMode::Drum {
            self.choke(&turn, chains);
        }
        if audio {
            for c in 0..channels {
                let edge = c * stride;
                turn.buffer(buffer)[edge..edge + frames].fill(0.0);
            }
            for i in chains.range() {
                let chain = self.rack_chains[i];
                if let Some(kept) = chain.taps[RackChain::tap_index(Tap::PreFx)] {
                    self.keep(
                        &turn,
                        kept,
                        self.tasks.len() + chain.buffer as usize,
                        frames,
                    );
                }
            }
        }
        let count = chains.range().len();
        match pass.wake {
            Some(wake) if count > 1 => {
                let fork = &self.sched.forks[rack];
                fork.open();
                for _ in 1..count {
                    if !self.sched.queue.push(Sched::ticket(rack)) {
                        break;
                    }
                    wake.one();
                }
                // The chains finish, and this thread waits for the others'
                // before it lets a panic go on: the threads still running
                // a chain of the task must be done before the task is.
                let mut panicked = None;
                while let Some(i) = fork.claim(count) {
                    let ran = panic::catch_unwind(AssertUnwindSafe(|| {
                        self.run_chain(pass, task, rack, i)
                    }));
                    fork.finish();
                    if let Err(payload) = ran {
                        panicked.get_or_insert(payload);
                    }
                }
                fork.join(count);
                if let Some(payload) = panicked {
                    panic::resume_unwind(payload);
                }
            }
            _ => {
                for i in 0..count {
                    self.run_chain(pass, task, rack, i);
                }
            }
        }
        if audio {
            self.run_returns(pass, task, rack);
            for i in chains.start as usize..returns.end as usize {
                let chain = self.rack_chains[i];
                if !chain.summed {
                    continue;
                }
                let at = self.tasks.len() + chain.buffer as usize;
                if chain.sum_delay != NO_DELAY {
                    let (line, ring) = turn.delay(chain.sum_delay);
                    delay_in_place(line, ring, turn.buffer(at), channels, stride, frames);
                }
                for c in 0..channels {
                    let edge = c * stride;
                    let sum = &mut turn.buffer(buffer)[edge..edge + frames];
                    let chain_out = &turn.buffer(at)[edge..edge + frames];
                    for (sum, sample) in sum.iter_mut().zip(chain_out) {
                        *sum += *sample;
                    }
                }
            }
        }
        if !notes {
            return note_idx;
        }
        let out = pair + 1 - note_idx;
        turn.notes(out).clear();
        if mode == RackMode::Midi {
            self.merge_notes(&turn, chains, out);
        }
        1 - note_idx
    }

    /// Works out rack `rack`'s chain selector for every frame of the block
    /// into `selects`, for its chains to read (R33), and returns whether the
    /// chain select zones filter messages other than notes (R31).
    fn run_selector(&self, pass: &mut Pass<'_>, rack: usize, buffer: usize) -> bool {
        let turn = pass.turn;
        let block = pass.block;
        let (channels, stride, frames) = (self.channels, self.max_frames, block.frames);
        let slot = self.racks[rack].selector;
        pass.delivered += gather(block.events, slot, block.from, block.until, pass.scratch);
        let Some(Instance::Selector(selector)) = turn.slot(slot).node.as_mut() else {
            turn.selects_mut(rack)[..frames].fill(0.0);
            return false;
        };
        selector.process(&mut Block {
            data: turn.buffer(buffer),
            channels,
            frames,
            stride,
            events: pass.scratch.as_slice(),
            transport: block.transport,
            sample_rate: self.sample_rate,
            tempo: &self.tempo,
            signatures: &self.time_signatures,
            sidechain: None,
            notes: &[],
            notes_out: None,
            session: block.session,
            groove_amount: block.groove_amount,
            set_scale: block.set_scale,
            scale: block.scale,
            modulation: turn.bus(frames),
            rows: None,
            external: None,
            external_out: None,
            outputs: None,
        });
        turn.selects_mut(rack)[..frames].copy_from_slice(selector.values(frames));
        selector.filters_ctrl()
    }

    /// Spec: drum-racks R4. A note-on that reaches a chain in a choke group
    /// ends what the other chains of the group started, on its own frame:
    /// each gets a choke for every key it has started since the last one,
    /// which its devices fade out. A chain doesn't choke itself, and a
    /// chain on All Notes is in no group.
    fn choke(&self, turn: &Turn<'_>, chains: Span) {
        // shortcut: past this many choking notes in one block the rest
        // choke nothing; upgrade if a kit ever needs more in one block.
        const ROOM: usize = 256;
        let mut found = [(0u32, 0u32, 0u8, 0u8); ROOM];
        let mut count = 0;
        for i in chains.range() {
            let chain = self.rack_chains[i];
            let Some(Instance::Drum(drum)) = turn.slot(chain.notes_slot()).node.as_ref() else {
                continue;
            };
            if drum.group() == 0 {
                continue;
            }
            let buffer = 2 * self.tasks.len() + 2 * chain.buffer as usize;
            for note in turn.notes(buffer).notes() {
                if note.kind == NoteKind::On && count < ROOM {
                    found[count] = (note.offset, i as u32, note.key, note.channel);
                    count += 1;
                }
            }
        }
        found[..count].sort_unstable();
        let group_of = |i: usize| match turn.slot(self.rack_chains[i].notes_slot()).node.as_ref() {
            Some(Instance::Drum(drum)) => drum.group(),
            _ => 0,
        };
        for &(offset, i, key, channel) in &found[..count] {
            let i = i as usize;
            let group = group_of(i);
            for j in chains.range().filter(|&j| j != i && group_of(j) == group) {
                let chain = self.rack_chains[j];
                let Some(Instance::Drum(drum)) = turn.slot(chain.notes_slot()).node.as_mut() else {
                    continue;
                };
                let mut sounded = drum.take_sounded();
                let buffer = turn.notes(2 * self.tasks.len() + 2 * chain.buffer as usize);
                while sounded != 0 {
                    let key = sounded.trailing_zeros() as u8;
                    buffer.insert(NoteEvent {
                        offset,
                        kind: NoteKind::Choke,
                        channel: drum.channel(key),
                        key,
                        velocity: 0.0,
                        note_id: None,
                    });
                    sounded &= sounded - 1;
                }
            }
            if let Some(Instance::Drum(drum)) =
                turn.slot(self.rack_chains[i].notes_slot()).node.as_mut()
            {
                drum.sound(key, channel);
            }
        }
    }

    /// Spec: drum-racks R6, R7. Feeds a drum rack's return chains with the
    /// drum chains' sends, each taken after its chain's mixer, and runs
    /// them.
    fn run_returns(&self, pass: &mut Pass<'_>, task: Task, rack: usize) {
        let Rack {
            chains, returns, ..
        } = self.racks[rack];
        let turn = pass.turn;
        let block = pass.block;
        let (channels, stride, frames) = (self.channels, self.max_frames, block.frames);
        let song = block.transport.playing.then(|| crate::automation::Song {
            frame: block.transport.position.get(),
            session: block.session,
            tempo: &self.tempo,
            rate: self.sample_rate,
        });
        let shape = crate::mixer::Planar {
            channels,
            stride,
            frames,
        };
        for j in returns.range() {
            let to = self.rack_chains[j];
            let at = self.tasks.len() + to.buffer as usize;
            for c in 0..channels {
                turn.buffer(at)[c * stride..c * stride + frames].fill(0.0);
            }
            for i in chains.range() {
                let from = self.rack_chains[i];
                let from_at = self.tasks.len() + from.buffer as usize;
                for send in self.chain_sends[from.sends.range()]
                    .iter()
                    .filter(|send| send.to as usize == j)
                {
                    pass.delivered += gather(
                        block.events,
                        send.route,
                        block.from,
                        block.until,
                        pass.scratch,
                    );
                    if let Some(Instance::Route(route)) = turn.slot(send.route).node.as_mut() {
                        route.mix(
                            turn.buffer(at),
                            turn.buffer(from_at),
                            shape,
                            song,
                            turn.bus(frames),
                            pass.scratch,
                        );
                    }
                }
            }
            // A return chain has no notes.
            turn.notes(2 * self.tasks.len() + 2 * to.buffer as usize)
                .clear();
            if let Some(kept) = to.taps[RackChain::tap_index(Tap::PreFx)] {
                self.keep(&turn, kept, at, frames);
            }
            self.run_chain_at(pass, task, rack, true, j);
        }
    }

    /// Runs the `i`th chain of `rack`: its devices, then its mixer, held
    /// back to the rack's latency, all on the chain's own buffer. A MIDI
    /// effect rack's chain leaves a cursor on its notes for the merge.
    fn run_chain(&self, pass: &mut Pass<'_>, task: Task, rack: usize, i: usize) {
        let Rack { mode, chains, .. } = self.racks[rack];
        self.run_chain_at(
            pass,
            task,
            rack,
            mode != RackMode::Midi,
            chains.start as usize + i,
        );
    }

    /// Runs the chain at `index` in `rack_chains`, of rack `rack`; `audio`
    /// if it carries audio, not a MIDI effect rack's notes alone.
    fn run_chain_at(
        &self,
        pass: &mut Pass<'_>,
        task: Task,
        rack: usize,
        audio: bool,
        index: usize,
    ) {
        let chain = self.rack_chains[index];
        let turn = pass.turn;
        let block = pass.block;
        let (channels, stride, frames) = (self.channels, self.max_frames, block.frames);
        let at_buffer = self.tasks.len() + chain.buffer as usize;
        let chain_pair = 2 * self.tasks.len() + 2 * chain.buffer as usize;
        let end = self.run_steps(pass, task, chain.steps, at_buffer, chain_pair, 0);
        if audio && let Some(kept) = chain.taps[RackChain::tap_index(Tap::PostFx)] {
            self.keep(&turn, kept, at_buffer, frames);
        }
        pass.delivered += gather(
            block.events,
            chain.mixer,
            block.from,
            block.until,
            pass.scratch,
        );
        let mut muted = false;
        if let Some(Instance::Mixer(mixer)) = turn.slot(chain.mixer).node.as_mut() {
            if audio {
                mixer.process(&mut Block {
                    data: turn.buffer(at_buffer),
                    channels,
                    frames,
                    stride,
                    events: pass.scratch.as_slice(),
                    transport: block.transport,
                    sample_rate: self.sample_rate,
                    tempo: &self.tempo,
                    signatures: &self.time_signatures,
                    sidechain: None,
                    external: None,
                    external_out: None,
                    outputs: None,
                    notes: &[],
                    notes_out: None,
                    session: block.session,
                    groove_amount: block.groove_amount,
                    set_scale: block.set_scale,
                    scale: block.scale,
                    modulation: turn.bus(frames),
                    rows: None,
                });
            } else {
                mixer.flush(pass.scratch);
            }
            muted = mixer.muted();
        }
        if audio && let Some(Instance::Zones(zones)) = turn.slot(chain.zones).node.as_mut() {
            let selector = &turn.selects(rack)[..frames];
            zones.scale(selector, turn.buffer(at_buffer), channels, stride);
        }
        if audio && let Some(kept) = chain.taps[RackChain::tap_index(Tap::PostMixer)] {
            self.keep(&turn, kept, at_buffer, frames);
        }
        if audio && chain.delay != NO_DELAY {
            let (line, ring) = turn.delay(chain.delay);
            delay_in_place(line, ring, turn.buffer(at_buffer), channels, stride, frames);
        }
        if !audio {
            *turn.cursor(index) = MergeCursor {
                buffer: chain_pair + end,
                cursor: 0,
                muted,
            };
        }
    }

    /// Runs a chain that the owner of `rack` offered to the threads working
    /// on the block, if one is left. Returns how many changes reached a node
    /// or a route.
    ///
    /// # Safety
    ///
    /// The rack's owner is running it in this block, waiting in
    /// [`Fork::join`] after offering its chains.
    pub(crate) unsafe fn run_offered(
        &self,
        rack: usize,
        lane: &mut Lane,
        block: &Blk<'_>,
        wake: &Wake,
    ) -> u64 {
        let fork = &self.sched.forks[rack];
        let Some(i) = fork.claim(self.racks[rack].chains.range().len()) else {
            return 0;
        };
        let t = self.racks[rack].task as usize;
        let mut pass = Pass {
            // SAFETY: the owner runs task `t` and waits for this chain, and
            // the chains' parts of the task are not shared.
            turn: unsafe { Turn::new(self, t) },
            block,
            scratch: &mut lane.events,
            device: &mut lane.device,
            wake: Some(wake),
            delivered: 0,
        };
        let ran = panic::catch_unwind(AssertUnwindSafe(|| {
            self.run_chain(&mut pass, self.tasks[t], rack, i);
        }));
        fork.finish();
        if let Err(payload) = ran {
            panic::resume_unwind(payload);
        }
        pass.delivered
    }

    /// Hands the nodes of a switched-off rack, and of the racks inside it,
    /// their parameter changes without running them, so they come back on
    /// with what was set while it was off (the racks spec, R7).
    fn flush_rack(&self, pass: &mut Pass<'_>, task: Task, rack: usize) {
        let turn = pass.turn;
        let block = pass.block;
        let selector = self.racks[rack].selector;
        pass.delivered += gather(
            block.events,
            selector,
            block.from,
            block.until,
            pass.scratch,
        );
        if let Some(Instance::Selector(node)) = turn.slot(selector).node.as_mut() {
            node.flush(pass.scratch);
        }
        for i in self.racks[rack].all().range() {
            let chain = self.rack_chains[i];
            pass.delivered += gather(
                block.events,
                chain.zones,
                block.from,
                block.until,
                pass.scratch,
            );
            if let Some(Instance::Zones(zones)) = turn.slot(chain.zones).node.as_mut() {
                zones.apply(pass.scratch);
                zones.idle();
            }
            // A rack that is off carries nothing to a route tapping its
            // chains.
            for kept in chain.taps.into_iter().flatten() {
                self.silence(&turn, kept, block.frames);
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
                                block.events,
                                sidechain.key,
                                block.from,
                                block.until,
                                pass.scratch,
                            );
                            if let Some(Instance::Sidechain(key)) =
                                turn.slot(sidechain.key).node.as_mut()
                            {
                                key.flush(pass.scratch);
                            }
                        }
                        pass.delivered +=
                            gather(block.events, slot, block.from, block.until, pass.scratch);
                        if let Some(node) =
                            turn.slot(slot).node.as_mut().and_then(Instance::as_node)
                        {
                            node.flush(pass.scratch);
                        }
                    }
                }
            }
            pass.delivered += gather(
                block.events,
                chain.mixer,
                block.from,
                block.until,
                pass.scratch,
            );
            if let Some(Instance::Mixer(mixer)) = turn.slot(chain.mixer).node.as_mut() {
                mixer.flush(pass.scratch);
            }
            // A drum chain's note routing and sends take their changes too.
            if chain.notes != NO_SLOT {
                pass.delivered += gather(
                    block.events,
                    chain.notes,
                    block.from,
                    block.until,
                    pass.scratch,
                );
                if let Some(Instance::Drum(drum)) = turn.slot(chain.notes).node.as_mut() {
                    drum.flush(pass.scratch);
                }
            }
            for send in &self.chain_sends[chain.sends.range()] {
                pass.delivered += gather(
                    block.events,
                    send.route,
                    block.from,
                    block.until,
                    pass.scratch,
                );
                if let Some(Instance::Route(route)) = turn.slot(send.route).node.as_mut() {
                    route.flush(pass.scratch);
                }
            }
        }
    }

    /// Merges the notes of the chains in `chains`, whose cursors the chains
    /// left, into the buffer `out`, in time order, the chain higher in the
    /// rack's list first where two fall on one frame (the racks spec, R4). A
    /// chain that's off passes only its note-offs, so no note it started
    /// hangs.
    fn merge_notes(&self, turn: &Turn<'_>, chains: Span, out: usize) {
        loop {
            let mut next: Option<(usize, NoteEvent)> = None;
            for at in chains.range() {
                let chain = turn.cursor(at);
                let notes = turn.notes(chain.buffer).notes();
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
            turn.notes(out).push(note);
            turn.cursor(at).cursor += 1;
        }
    }
}

/// A MIDI effect rack's chain whose notes are being merged: where they are,
/// how far through them the merge has got, and whether the chain is off.
#[derive(Clone, Copy, Debug, Default)]
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
