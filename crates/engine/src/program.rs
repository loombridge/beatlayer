//! The engine program: what the compiler makes from a document and the audio
//! thread runs.

use core::fmt;
use core::ops::Range;

use bl_core::time::{SampleRate, TempoMap};
use bl_model::{ParamId, TrackId};

use crate::node::{Block, Node, NodeEvent, NodeKey, ParamValue, TransportInfo};

/// A compiled song, ready to run on the audio thread: a flat array of
/// nodes, the tasks that run them, the buffers they work in, and the order
/// to run the tasks in.
///
/// # Tasks
///
/// The unit of work is a task: one track's device chain, run in place on
/// the track's own buffer after the buffers that feed it (a group's tracks,
/// or Main's) are summed into it. A task can start once every task feeding
/// it has finished, so the tasks form a dependency graph. Each task lists
/// the tasks waiting on it and how many it waits on, which is what a pool of
/// real-time workers needs to take ready tasks from a queue (the multicore
/// scheduler, a later package). For now the audio thread runs them alone, in
/// a topological order the compiler worked out.
///
/// # Migration
///
/// A node that survives a recompile isn't made again. The compiler leaves
/// its slot in the new program empty and lists where it was in the program
/// before. When the audio thread swaps the new program in, it moves each
/// surviving node across, which moves a pointer and allocates nothing, so
/// the node keeps its state. Whatever is left in the old program goes with
/// it to the drop thread.
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
    /// Tasks whose buffers each task sums before running its chain.
    pub(crate) task_inputs: Vec<u32>,
    /// Slots each task runs, in chain order.
    pub(crate) task_nodes: Vec<u32>,
    /// Tasks waiting on each task. The single-threaded schedule runs `order`
    /// instead; the worker pool (a later package) will walk these.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) task_dependents: Vec<u32>,
    /// One buffer per task: `channels` channels of `max_frames` frames each.
    pub(crate) pool: Vec<f32>,
    pub(crate) channels: usize,
    pub(crate) max_frames: usize,
    /// The task whose buffer is the engine's output: Main's.
    pub(crate) output: u32,
    pub(crate) tempo: TempoMap,
    pub(crate) sample_rate: SampleRate,
    /// Each task's peak in the current block, per channel (the first two).
    pub(crate) peaks: Vec<[f32; 2]>,
    /// Each task's meter: peaks with a fixed release.
    pub(crate) meters: Vec<[f32; 2]>,
}

/// A node in a program, or the place for one that migrates in.
pub(crate) struct NodeSlot {
    pub(crate) key: NodeKey,
    pub(crate) node: Option<Box<dyn Node>>,
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

/// One track's chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Task {
    pub(crate) track: TrackId,
    pub(crate) inputs: Span,
    pub(crate) nodes: Span,
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

    /// The nodes in the program, including those that migrate in.
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

    /// The tracks, in the order the audio thread runs their chains.
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

    /// The node in `slot`, if it has one.
    pub(crate) fn node_mut(&mut self, slot: u32) -> Option<&mut (dyn Node + 'static)> {
        self.slots
            .get_mut(slot as usize)
            .and_then(|slot| slot.node.as_deref_mut())
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
    /// slot then frame; each node gets those in its part, `scratch` holding
    /// them. Returns how many changes reached a node.
    pub(crate) fn run(
        &mut self,
        frames: usize,
        from: u32,
        events: &[Resolved],
        scratch: &mut Vec<NodeEvent>,
        transport: TransportInfo,
    ) -> u64 {
        let frames = frames.min(self.max_frames);
        let size = self.channels * self.max_frames;
        let until = from + frames as u32;
        let mut delivered = 0;
        for position in 0..self.order.len() {
            let t = self.order[position] as usize;
            let task = self.tasks[t];
            let own = t * size..(t + 1) * size;

            // Sum the inputs into the task's own buffer.
            for c in 0..self.channels {
                let start = own.start + c * self.max_frames;
                self.pool[start..start + frames].fill(0.0);
            }
            for &input in &self.task_inputs[task.inputs.range()] {
                let theirs = input as usize * size..(input as usize + 1) * size;
                let Ok([mine, theirs]) = self.pool.get_disjoint_mut([own.clone(), theirs]) else {
                    continue;
                };
                for c in 0..self.channels {
                    let at = c * self.max_frames..c * self.max_frames + frames;
                    for (sum, sample) in mine[at.clone()].iter_mut().zip(&theirs[at]) {
                        *sum += *sample;
                    }
                }
            }

            // Run the chain in place.
            for &slot in &self.task_nodes[task.nodes.range()] {
                let NodeSlot { node, bypass, .. } = &mut self.slots[slot as usize];
                let Some(node) = node.as_deref_mut() else {
                    continue;
                };
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
                delivered += scratch.len() as u64;
                if *bypass {
                    node.flush(scratch);
                    continue;
                }
                let mut block = Block {
                    data: &mut self.pool[own.clone()],
                    channels: self.channels,
                    frames,
                    stride: self.max_frames,
                    events: scratch.as_slice(),
                    transport,
                    sample_rate: self.sample_rate,
                    tempo: &self.tempo,
                };
                node.process(&mut block);
            }

            // Meter the result.
            for (c, peak) in self.peaks[t].iter_mut().enumerate().take(self.channels) {
                let start = own.start + c * self.max_frames;
                let block_peak = self.pool[start..start + frames]
                    .iter()
                    .fold(0.0f32, |p, s| p.max(s.abs()));
                *peak = peak.max(block_peak);
            }
        }
        delivered
    }

    /// The output's channel `c` from the last run, `frames` long.
    pub(crate) fn output(&self, c: usize, frames: usize) -> &[f32] {
        let start = self.output as usize * self.channels * self.max_frames + c * self.max_frames;
        &self.pool[start..start + frames.min(self.max_frames)]
    }

    /// Ends a block's metering: each meter takes its peak, or falls by
    /// `release` (a factor below 1) if that's higher.
    pub(crate) fn finish_meters(&mut self, release: f32) {
        for (meter, peak) in self.meters.iter_mut().zip(self.peaks.iter_mut()) {
            for c in 0..2 {
                meter[c] = peak[c].max(meter[c] * release);
                peak[c] = 0.0;
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
