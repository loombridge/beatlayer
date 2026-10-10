//! The audio thread's side of the engine.

use std::time::Instant;

use bl_core::time::{Frames, TempoMap};
use bl_model::ParamId;
use bl_rt::log::{Level, Writer};
use bl_rt::rt_log;
use rtrb::{Consumer, Producer};

use crate::EngineConfig;
use crate::gc::Program;
use crate::message::{
    Action, Message, ParamEvent, Report, ScheduledCommand, TransportCommand, When,
};
use crate::node::{NodeData, NodeEvent, NodeKey, ParamValue, TransportInfo};
use crate::program::{EngineProgram, Resolved};
use crate::state::{EngineCounters, EngineState, METER_RELEASE_DB_PER_SECOND, TrackMeter};

/// A parameter change waiting for its frame.
#[derive(Clone, Copy, Debug)]
struct Pending {
    frame: u64,
    seq: u64,
    key: NodeKey,
    param: ParamId,
    value: ParamValue,
}

/// A scheduled command with the song frame it fires on, under the running
/// program's tempo map.
#[derive(Clone, Copy, Debug)]
struct Scheduled {
    command: ScheduledCommand,
    frame: Frames,
}

/// The engine on the audio thread: it runs the current program once per
/// block and takes everything the main thread sends.
///
/// [`process`](Self::process) follows the real-time rules: it allocates
/// nothing, frees nothing, takes no lock, and its work is bounded by the
/// sizes in [`EngineConfig`], all reserved when the engine is made. Each
/// block it:
///
/// 1. takes what's waiting on the queue, in order: new programs (swapped in
///    on the spot, their surviving nodes moved across from the one before),
///    parameter changes, node data, transport commands, and scheduled
///    commands;
/// 2. runs the program over the block, split where a scheduled command
///    fires so the command lands on its exact frame;
/// 3. publishes the playhead, meters, and CPU load for the UI.
///
/// What it drops (old programs, removed nodes, replaced data) is freed on
/// the drop thread.
pub struct Processor {
    config: EngineConfig,
    queue: Consumer<Message>,
    reports: Producer<Report>,
    state: triple_buffer::Input<EngineState>,
    program: Option<Program>,
    /// The tempo map until a program arrives with one.
    fallback_tempo: TempoMap,
    /// `channels` channels of `max_frames` frames.
    output: Vec<f32>,
    /// Frames in the last block.
    last: usize,
    clock: u64,
    playing: bool,
    position: Frames,
    pending: Vec<Pending>,
    /// This block's parameter changes, sorted by slot, frame, and arrival.
    resolved: Vec<Resolved>,
    /// One node's changes, handed to it.
    scratch: Vec<NodeEvent>,
    scheduled: Vec<Scheduled>,
    seq: u64,
    swaps: u64,
    counters: EngineCounters,
    cpu_load_max: f32,
    log: Writer,
}

impl Processor {
    pub(crate) fn new(
        config: EngineConfig,
        queue: Consumer<Message>,
        reports: Producer<Report>,
        state: triple_buffer::Input<EngineState>,
    ) -> Self {
        Self {
            queue,
            reports,
            state,
            program: None,
            fallback_tempo: TempoMap::default(),
            output: vec![0.0; config.channels * config.max_frames],
            last: 0,
            clock: 0,
            playing: false,
            position: Frames::ZERO,
            pending: Vec::with_capacity(config.max_pending_events),
            resolved: Vec::with_capacity(config.max_pending_events),
            scratch: Vec::with_capacity(config.max_pending_events),
            scheduled: Vec::with_capacity(config.max_scheduled),
            seq: 0,
            swaps: 0,
            counters: EngineCounters::default(),
            cpu_load_max: 0.0,
            log: Writer::new(),
            config,
        }
    }

    /// The engine's settings.
    #[must_use]
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Frames processed so far: the engine clock.
    #[must_use]
    pub fn clock(&self) -> u64 {
        self.clock
    }

    /// Whether the song is playing.
    #[must_use]
    pub fn playing(&self) -> bool {
        self.playing
    }

    /// The song position of the next frame to play.
    #[must_use]
    pub fn position(&self) -> Frames {
        self.position
    }

    /// The running program.
    #[must_use]
    pub fn program(&self) -> Option<&EngineProgram> {
        self.program.as_ref().map(Program::get)
    }

    /// Processes one block of `frames` frames (at most
    /// [`EngineConfig::max_frames`]; more are cut to that) and returns its
    /// output. Real-time safe.
    pub fn process(&mut self, frames: usize) -> Output<'_> {
        let began = Instant::now();
        let frames = frames.min(self.config.max_frames);
        self.drain();
        self.resolve_due(frames);
        let mut done = 0;
        while done < frames {
            self.fire_due(done);
            let length = self.until_next_command(frames - done);
            self.render(done, length);
            done += length;
        }
        self.clock += frames as u64;
        self.last = frames;
        self.finish(began, frames);
        self.output()
    }

    /// The last block's output.
    #[must_use]
    pub fn output(&self) -> Output<'_> {
        Output {
            data: &self.output,
            channels: self.config.channels,
            frames: self.last,
            stride: self.config.max_frames,
        }
    }

    /// Takes what's on the queue, up to its capacity, so the work is
    /// bounded even if the sender keeps sending.
    fn drain(&mut self) {
        for _ in 0..self.config.queue_capacity {
            let Ok(message) = self.queue.pop() else {
                break;
            };
            self.counters.messages += 1;
            match message {
                Message::Program(program) => self.swap(program),
                Message::Param(event) => self.queue_param(event),
                Message::Data { key, data } => self.deliver(key, data),
                Message::Transport(command) => self.transport(command),
                Message::Schedule(command) => self.schedule(command),
                Message::Cancel(id) => self.scheduled.retain(|s| s.command.id != id),
            }
        }
    }

    /// Swaps a new program in at the block boundary. Its surviving nodes
    /// move across from the running one, and the running one, with the nodes
    /// that didn't survive, goes to the drop thread.
    fn swap(&mut self, mut program: Program) {
        let rate = self.config.sample_rate;
        let mut mismatch = None;
        match self.program.as_mut() {
            Some(old) => {
                if !program.get_mut().adopt(old.get_mut()) {
                    mismatch = Some(Some(old.get().generation));
                }
                let (before, after) = (&old.get().tempo, &program.get().tempo);
                if before != after {
                    // Keep the playhead on the same beat.
                    let ticks = before.frames_to_ticks(self.position, rate);
                    self.position = after.ticks_to_frames(ticks, rate);
                    for entry in &mut self.scheduled {
                        entry.frame = after.ticks_to_frames(entry.command.at, rate);
                    }
                }
            }
            None => {
                if program.get().base.is_some() {
                    mismatch = Some(None);
                }
                let ticks = self.fallback_tempo.frames_to_ticks(self.position, rate);
                self.position = program.get().tempo.ticks_to_frames(ticks, rate);
                for entry in &mut self.scheduled {
                    entry.frame = program.get().tempo.ticks_to_frames(entry.command.at, rate);
                }
            }
        }
        if let Some(running) = mismatch {
            self.mismatch(program.get(), running);
        }
        // Dropping the old program here only lets go of it: the drop thread
        // frees it.
        drop(self.program.replace(program));
        self.swaps += 1;
    }

    fn mismatch(&mut self, program: &EngineProgram, running: Option<u64>) {
        self.counters.migration_mismatches += 1;
        rt_log!(
            to: self.log,
            Level::Error,
            "program {} was compiled after program {}, not the running {}; its surviving nodes are missing",
            program.generation,
            program.base.unwrap_or(0),
            running.unwrap_or(0)
        );
    }

    fn queue_param(&mut self, event: ParamEvent) {
        let frame = match event.at {
            When::Now => self.clock,
            When::Frame(frame) => frame,
        };
        if self.pending.len() == self.pending.capacity() {
            self.counters.events_overflowed += 1;
            return;
        }
        self.seq += 1;
        self.pending.push(Pending {
            frame,
            seq: self.seq,
            key: event.key,
            param: event.param,
            value: event.value,
        });
    }

    fn deliver(&mut self, key: NodeKey, data: NodeData) {
        let node = self.program.as_mut().and_then(|program| {
            let program = program.get_mut();
            let slot = program.slot_of(key)?;
            program.node_mut(slot)
        });
        match node {
            Some(node) => {
                node.receive_data(data);
                self.counters.data_delivered += 1;
            }
            // Dropping the data only lets go of it.
            None => self.counters.data_unrouted += 1,
        }
    }

    fn tempo(&self) -> &TempoMap {
        self.program
            .as_ref()
            .map_or(&self.fallback_tempo, |program| &program.get().tempo)
    }

    fn transport(&mut self, command: TransportCommand) {
        match command {
            TransportCommand::Play => self.playing = true,
            TransportCommand::Stop => self.playing = false,
            TransportCommand::Locate(ticks) => {
                self.position = self.tempo().ticks_to_frames(ticks, self.config.sample_rate);
            }
        }
    }

    fn schedule(&mut self, command: ScheduledCommand) {
        if self.scheduled.len() == self.scheduled.capacity() {
            self.counters.scheduled_rejected += 1;
            self.report(Report::Rejected { id: command.id });
            return;
        }
        let frame = self
            .tempo()
            .ticks_to_frames(command.at, self.config.sample_rate);
        self.scheduled.push(Scheduled { command, frame });
    }

    fn report(&mut self, report: Report) {
        if self.reports.push(report).is_err() {
            self.counters.reports_dropped += 1;
        }
    }

    /// Routes the parameter changes due in this block to their nodes.
    fn resolve_due(&mut self, frames: usize) {
        self.resolved.clear();
        let end = self.clock + frames as u64;
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].frame < end {
                let due = self.pending.swap_remove(i);
                let offset = due.frame.saturating_sub(self.clock) as u32;
                self.route(due.key, due.param, due.value, offset, due.seq);
            } else {
                i += 1;
            }
        }
        self.resolved.sort_unstable_by_key(Resolved::sort_key);
    }

    fn route(&mut self, key: NodeKey, param: ParamId, value: ParamValue, offset: u32, seq: u64) {
        let Some(slot) = self.program.as_ref().and_then(|p| p.get().slot_of(key)) else {
            self.counters.events_unrouted += 1;
            return;
        };
        if self.resolved.len() == self.resolved.capacity() {
            self.counters.events_overflowed += 1;
            return;
        }
        self.resolved.push(Resolved {
            slot,
            offset,
            seq,
            param,
            value,
        });
    }

    /// Fires every scheduled command at the playhead, which is `offset`
    /// frames into the block.
    fn fire_due(&mut self, offset: usize) {
        let mut i = 0;
        while self.playing && i < self.scheduled.len() {
            if self.scheduled[i].frame == self.position {
                // `remove` keeps the rest in the order they arrived.
                let due = self.scheduled.remove(i);
                self.fire(due.command, offset);
                i = 0;
            } else {
                i += 1;
            }
        }
    }

    fn fire(&mut self, command: ScheduledCommand, offset: usize) {
        let position = self.position;
        match command.action {
            Action::Param { key, param, value } => {
                self.seq += 1;
                self.route(key, param, value, offset as u32, self.seq);
                self.resolved.sort_unstable_by_key(Resolved::sort_key);
            }
            Action::Transport(transport) => self.transport(transport),
            Action::Notify => {}
        }
        self.counters.scheduled_fired += 1;
        self.report(Report::Fired {
            id: command.id,
            position,
            clock: self.clock + offset as u64,
        });
    }

    /// How many of the next `remaining` frames to run before a scheduled
    /// command fires: at least 1.
    fn until_next_command(&self, remaining: usize) -> usize {
        if !self.playing {
            return remaining;
        }
        self.scheduled
            .iter()
            .filter_map(|entry| {
                let ahead = entry.frame.saturating_sub(self.position).get();
                (ahead > 0 && ahead < remaining as i64).then_some(ahead as usize)
            })
            .min()
            .unwrap_or(remaining)
    }

    /// Runs the program over `length` frames from `offset` in the block, and
    /// moves the playhead on if the song is playing.
    fn render(&mut self, offset: usize, length: usize) {
        let stride = self.config.max_frames;
        let transport = TransportInfo {
            playing: self.playing,
            position: self.position,
        };
        match self.program.as_mut() {
            Some(program) => {
                let program = program.get_mut();
                self.counters.events_applied += program.run(
                    length,
                    offset as u32,
                    &self.resolved,
                    &mut self.scratch,
                    transport,
                );
                for c in 0..self.config.channels {
                    let to = c * stride + offset;
                    let from = c.min(program.channels - 1);
                    self.output[to..to + length].copy_from_slice(program.output(from, length));
                }
            }
            None => {
                for c in 0..self.config.channels {
                    let to = c * stride + offset;
                    self.output[to..to + length].fill(0.0);
                }
            }
        }
        if self.playing {
            self.position += Frames(length as i64);
        }
    }

    /// Ends the block: meters, CPU load, and the state for the UI.
    fn finish(&mut self, began: Instant, frames: usize) {
        let rate = self.config.sample_rate.as_f64();
        let lasts = frames as f64 / rate;
        let release = 10f64.powf(-METER_RELEASE_DB_PER_SECOND * lasts / 20.0) as f32;
        if let Some(program) = self.program.as_mut() {
            program.get_mut().finish_meters(release);
        }
        let cpu_load = if lasts > 0.0 {
            (began.elapsed().as_secs_f64() / lasts) as f32
        } else {
            0.0
        };
        self.cpu_load_max = self.cpu_load_max.max(cpu_load);
        let position_ticks = self
            .tempo()
            .frames_to_ticks(self.position, self.config.sample_rate);

        let state = self.state.input_buffer_mut();
        state.clock = self.clock;
        state.playing = self.playing;
        state.position = self.position;
        state.position_ticks = position_ticks;
        state.program = self.program.as_ref().map(|p| p.get().generation);
        state.swaps = self.swaps;
        state.cpu_load = cpu_load;
        state.cpu_load_max = self.cpu_load_max;
        state.counters = self.counters;
        state.meters.clear();
        state.meters_truncated = false;
        if let Some(program) = &self.program {
            let program = program.get();
            for &task in &program.order {
                let meter = TrackMeter {
                    track: program.tasks[task as usize].track,
                    peak: program.meters[task as usize],
                };
                if state.meters.len() < state.meters.capacity() {
                    state.meters.push(meter);
                } else {
                    state.meters_truncated = true;
                }
            }
        }
        self.state.publish();
    }
}

impl std::fmt::Debug for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Processor")
            .field("clock", &self.clock)
            .field("playing", &self.playing)
            .field("position", &self.position)
            .field("program", &self.program)
            .finish_non_exhaustive()
    }
}

/// A block of the engine's output: planar channels.
#[derive(Debug)]
pub struct Output<'a> {
    data: &'a [f32],
    channels: usize,
    frames: usize,
    stride: usize,
}

impl Output<'_> {
    /// Frames in the block.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Channels.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// One channel.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`channels`](Self::channels).
    #[must_use]
    pub fn channel(&self, index: usize) -> &[f32] {
        assert!(index < self.channels, "channel {index} out of range");
        let start = index * self.stride;
        &self.data[start..start + self.frames]
    }
}
