//! The engine program: what the compiler makes from a document and the audio
//! thread runs.

use core::fmt;
use core::ops::Range;

use bl_core::time::{SampleRate, TempoMap};
use bl_model::{ParamId, Tap, TrackId};

use crate::mixer::{MeterReading, Route, TrackMixer};
use crate::node::{Block, Node, NodeEvent, NodeKey, NoteBuffer, ParamValue, TransportInfo};
use crate::sidechain::SidechainKey;

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
    /// Device slots each task runs, in chain order.
    pub(crate) task_nodes: Vec<u32>,
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
    /// One buffer per task: `channels` channels of `max_frames` frames each.
    pub(crate) pool: Vec<f32>,
    /// The copies tasks keep of their signal before their devices or
    /// before their mixer, for routes that tap there: one buffer each.
    pub(crate) tap_pool: Vec<f32>,
    pub(crate) channels: usize,
    pub(crate) max_frames: usize,
    /// The task whose buffer is the engine's output: Main's.
    pub(crate) output: u32,
    pub(crate) tempo: TempoMap,
    pub(crate) sample_rate: SampleRate,
    pub(crate) note_buffers: [NoteBuffer; 2],
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
}

/// A node in a program, or the place for one that migrates in.
pub(crate) struct NodeSlot {
    pub(crate) key: NodeKey,
    pub(crate) node: Option<Instance>,
    /// Switched off: the node keeps its state and takes parameter changes,
    /// but doesn't process.
    pub(crate) bypass: bool,
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
}

/// One track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Task {
    pub(crate) track: TrackId,
    pub(crate) inputs: Span,
    /// The slot of the track's clip player, which runs first.
    pub(crate) player: Option<u32>,
    /// The device chain.
    pub(crate) nodes: Span,
    /// The sidechains of its devices, in `sidechains`.
    pub(crate) sidechains: Span,
    /// The slot of the track's mixer.
    pub(crate) mixer: u32,
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

    /// The slot holding `key`'s node.
    pub(crate) fn slot_of(&self, key: NodeKey) -> Option<u32> {
        self.index
            .binary_search_by_key(&key, |&(k, _)| k)
            .ok()
            .map(|i| self.index[i].1)
    }

    /// The node in `slot`, if it has one that processes a track's buffer.
    pub(crate) fn node_mut(&mut self, slot: u32) -> Option<&mut (dyn Node + 'static)> {
        self.slots
            .get_mut(slot as usize)
            .and_then(|slot| slot.node.as_mut())
            .and_then(Instance::as_node)
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
        true
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
    ) -> u64 {
        let frames = frames.min(self.max_frames);
        let (channels, stride) = (self.channels, self.max_frames);
        let size = channels * stride;
        let until = from + frames as u32;
        let mut delivered = 0;
        for position in 0..self.order.len() {
            let t = self.order[position] as usize;
            let task = self.tasks[t];
            let own = t * size..(t + 1) * size;
            for c in 0..channels {
                let start = own.start + c * stride;
                self.pool[start..start + frames].fill(0.0);
            }

            let mut note_idx = 0;
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
                    });
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
                let source = input.from as usize;
                let kept = match input.tap {
                    Tap::PreFx => self.tasks[source].pre_fx,
                    Tap::PostFx => self.tasks[source].post_fx,
                    Tap::PostMixer => None,
                };
                match (input.tap, kept) {
                    (Tap::PostMixer, _) => {
                        let theirs = source * size..(source + 1) * size;
                        let Ok([mine, theirs]) = self.pool.get_disjoint_mut([own.clone(), theirs])
                        else {
                            continue;
                        };
                        route.mix(mine, theirs, channels, stride, frames, scratch);
                    }
                    (_, Some(buffer)) => {
                        let theirs = buffer as usize * size..(buffer as usize + 1) * size;
                        route.mix(
                            &mut self.pool[own.clone()],
                            &self.tap_pool[theirs],
                            channels,
                            stride,
                            frames,
                            scratch,
                        );
                    }
                    (_, None) => {}
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
            for &slot in &self.task_nodes[task.nodes.range()] {
                let sidechain = self.sidechains[task.sidechains.range()]
                    .iter()
                    .find(|s| s.device == slot)
                    .copied();
                let bypass = self.slots[slot as usize].bypass;
                // What the detector hears, made before the device runs.
                let mut heard = None;
                if let Some(sidechain) = sidechain {
                    delivered += gather(events, sidechain.key, from, until, scratch);
                    let at =
                        sidechain.buffer as usize * size..(sidechain.buffer as usize + 1) * size;
                    if let Some(Some(Instance::Sidechain(key))) = self
                        .slots
                        .get_mut(sidechain.key as usize)
                        .map(|slot| slot.node.as_mut())
                    {
                        if bypass {
                            key.flush(scratch);
                        } else {
                            key.make(
                                &mut self.sidechain_pool[at.clone()],
                                &self.pool[own.clone()],
                                channels,
                                stride,
                                frames,
                                scratch,
                            );
                            if key.enabled() {
                                heard = Some((at, key.listening()));
                            }
                        }
                    }
                }
                delivered += gather(events, slot, from, until, scratch);
                let NodeSlot { node, .. } = &mut self.slots[slot as usize];
                let Some(node) = node.as_mut().and_then(Instance::as_node) else {
                    continue;
                };
                let next_note_idx = 1 - note_idx;
                let (in_buf, out_buf) = if note_idx == 0 {
                    let (first, second) = self.note_buffers.split_at_mut(1);
                    (&first[0], &mut second[0])
                } else {
                    let (first, second) = self.note_buffers.split_at_mut(1);
                    (&second[0], &mut first[0])
                };
                out_buf.clear();
                if bypass {
                    node.flush(scratch);
                    for &note in in_buf.notes() {
                        out_buf.push(note);
                    }
                    note_idx = next_note_idx;
                    continue;
                }
                node.process(&mut Block {
                    data: &mut self.pool[own.clone()],
                    channels,
                    frames,
                    stride,
                    events: scratch.as_slice(),
                    transport,
                    sample_rate: self.sample_rate,
                    tempo: &self.tempo,
                    sidechain: heard
                        .as_ref()
                        .map(|(at, _)| &self.sidechain_pool[at.clone()]),
                    notes: in_buf.notes(),
                    notes_out: Some(out_buf),
                });
                note_idx = next_note_idx;
                // Sidechain Listen: the device's output is what it heard.
                if let Some((at, true)) = heard {
                    for c in 0..channels {
                        let (to, from) = (own.start + c * stride, at.start + c * stride);
                        self.pool[to..to + frames]
                            .copy_from_slice(&self.sidechain_pool[from..from + frames]);
                    }
                }
            }

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
                });
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
        let (channels, stride) = (self.channels, self.max_frames);
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
            if let Some((pool, buffer)) = theirs {
                route.mix(
                    &mut self.sidechain_pool[at.clone()],
                    &pool[buffer * size..(buffer + 1) * size],
                    channels,
                    stride,
                    frames,
                    scratch,
                );
            }
        }
        delivered
    }

    /// Copies `frames` frames of the task buffer at `own` into the tap
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
        let start = self.output as usize * self.channels * self.max_frames + c * self.max_frames;
        &self.pool[start..start + frames.min(self.max_frames)]
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
