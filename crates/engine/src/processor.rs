//! The audio thread's side of the engine.

use std::time::Instant;

use bl_core::midi::MidiInputEvent;
use bl_core::time::{Frames, SampleRate, TempoMap, Ticks, TimeSignatureMap};
use bl_model::{LaunchSettings, ParamId, SceneId, TrackId};
use bl_rt::log::{Level, Writer};
use bl_rt::rt_log;
use rtrb::{Consumer, Producer};

use crate::EngineConfig;
use crate::gc::{Owned, Program};
use crate::message::{
    Action, LoopRegion, Message, ParamEvent, Punch, Report, ScheduledCommand, TransportCommand,
    When,
};
use crate::metronome::{Block, MetronomeEngine};
use crate::midi::{MidiCounters, MidiIn, TrackMidiInput};
use crate::node::{NodeData, NodeEvent, NodeKey, ParamValue, TransportInfo};
use crate::program::{EngineProgram, Resolved};
use crate::recording::{RecordingPlan, TakeBounds};
use crate::session::{Playhead, SessionCommand, SessionLauncher, SessionPlayback, SlotStatus};
use crate::state::{EngineCounters, EngineState, TrackMeter};

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

/// A transport command waiting for its frame of the engine clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Timed {
    frame: u64,
    command: TransportCommand,
}

/// The Arrangement loop in song frames, under the running program's tempo
/// map. Only a loop at least one frame long is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LoopFrames {
    start: Frames,
    end: Frames,
}

/// [`Punch`] in song frames, under the running program's tempo map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PunchFrames {
    start: Option<Frames>,
    end: Option<Frames>,
}

/// The take being recorded (WP-2.13): the song frame it started at and how
/// many frames of playback it has covered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Take {
    start: Frames,
    written: u64,
}

/// What one processor hands the next when the engine restarts on a device
/// (WP-1.13): the engine clock, the transport (the song position and the
/// insert marker, in beats, and the Arrangement loop), and the commands
/// still waiting: scheduled ones for their beats, and transport commands
/// for their frames of the clock. [`Processor::park`] makes it, and
/// [`Restarter::restart`](crate::Restarter::restart) has the next
/// processor take it over at its own rate, with the song stopped.
#[derive(Clone, Debug, PartialEq)]
pub struct Carry {
    /// The engine clock when the stream closed. The next processor's clock
    /// carries on from it, so a frame sent for the clock means the same
    /// frame either side of a restart.
    clock: u64,
    /// The rate the clock ran at.
    sample_rate: SampleRate,
    /// The song position: where playback stopped, where Continue resumes.
    position: Ticks,
    /// The insert marker: where Play starts.
    insert_marker: Ticks,
    /// The Arrangement loop, if it was on.
    loop_region: Option<LoopRegion>,
    /// Where recording punches in and out (WP-2.13).
    punch: Punch,
    /// The take that last ended (WP-2.13), not yet handled by the shell.
    take: Option<TakeBounds>,
    /// Scheduled commands waiting to fire, in the order they arrived.
    scheduled: Vec<ScheduledCommand>,
    /// Transport commands waiting for their frames, in the order they came.
    timed: Vec<Timed>,
    /// The global launch quantisation and its enabled switch, which nothing
    /// else carries to the processor that takes over (WP-3.2).
    launch: LaunchSettings,
}

impl Carry {
    /// The engine clock when the stream closed.
    #[must_use]
    pub fn clock(&self) -> u64 {
        self.clock
    }

    /// The rate the processor ran at.
    #[must_use]
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// The song position, in beats: where playback stopped.
    #[must_use]
    pub fn position(&self) -> Ticks {
        self.position
    }

    /// The insert marker.
    #[must_use]
    pub fn insert_marker(&self) -> Ticks {
        self.insert_marker
    }

    /// The Arrangement loop, if it was on.
    #[must_use]
    pub fn loop_region(&self) -> Option<LoopRegion> {
        self.loop_region
    }

    /// The global launch quantisation and its switch.
    #[must_use]
    pub fn launch(&self) -> LaunchSettings {
        self.launch
    }

    /// The scheduled commands waiting to fire, in the order they arrived.
    #[must_use]
    pub fn scheduled(&self) -> &[ScheduledCommand] {
        &self.scheduled
    }

    /// The frame of the next processor's clock, running at `rate`, for
    /// `frame` of this one's: as long after the restart as it was after the
    /// stream closed. A frame already past stays past, so its command runs
    /// as soon as the next processor starts.
    pub(crate) fn frame_at(&self, frame: u64, rate: SampleRate) -> u64 {
        if frame <= self.clock {
            return frame;
        }
        let ahead = (frame - self.clock) as f64 * rate.as_f64() / self.sample_rate.as_f64();
        // A float-to-integer cast saturates.
        self.clock.saturating_add(ahead.round() as u64)
    }
}

/// Whether a transport command starts the song from where it's stopped,
/// which nothing does by itself across a restart (device-changes R7).
pub(crate) fn starts_playback(command: TransportCommand) -> bool {
    matches!(
        command,
        TransportCommand::Play | TransportCommand::Continue | TransportCommand::PlayStop
    )
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
/// 2. runs the program over the block, split wherever the transport acts
///    (a transport command's frame, a scheduled command's song position,
///    the loop's end) so each lands on its exact frame, and every part
///    plays one unbroken stretch of the song;
/// 3. publishes the playhead, every track's meters, and CPU load for the UI.
///
/// # The transport
///
/// The processor owns the transport (the transport spec): whether
/// the song plays, the song position (the next frame to play), the insert
/// marker (where Play starts), and the Arrangement loop. Positions in ticks
/// fall on frames through the running program's tempo map; when a new
/// program changes the map, the playhead keeps its beat and the loop its
/// bars. With the loop on, playback that reaches the loop's end goes on
/// from its start on the very next frame.
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
    fallback_time_signatures: TimeSignatureMap,
    metronome: MetronomeEngine,
    /// `channels` channels of `max_frames` frames.
    output: Vec<f32>,
    /// Frames in the last block.
    last: usize,
    clock: u64,
    playing: bool,
    position: Frames,
    /// Where Play starts.
    insert_marker: Ticks,
    /// The Arrangement loop, if it's on.
    loop_region: Option<LoopRegion>,
    /// `loop_region` in frames, if it's at least a frame long.
    loop_frames: Option<LoopFrames>,
    /// Where recording punches in and out (WP-2.13).
    punch: Punch,
    /// `punch` in frames.
    punch_frames: PunchFrames,
    /// Whether the transport records (WP-2.13).
    recording: bool,
    /// What to record: the shell's plan, whose rings close when it goes.
    plan: Option<Owned<RecordingPlan>>,
    /// The take being captured, if playback is running under recording.
    take: Option<Take>,
    /// The take that last ended, kept for the shell to make its clips,
    /// until the next one starts.
    last_take: Option<TakeBounds>,
    /// Transport commands waiting for their frames, in the order they came.
    timed: Vec<Timed>,
    pending: Vec<Pending>,
    /// This block's parameter changes, sorted by slot, frame, and arrival.
    resolved: Vec<Resolved>,
    /// One node's changes, handed to it.
    scratch: Vec<NodeEvent>,
    scheduled: Vec<Scheduled>,
    seq: u64,
    swaps: u64,
    /// The Session View's slots and the launch quantisation (WP-3.2).
    session: SessionLauncher,
    /// What the tracks' players play this block, rebuilt when the launcher
    /// changes.
    session_playback: Vec<SessionPlayback>,
    /// The slots' states, published after each block.
    session_status: Vec<SlotStatus>,
    counters: EngineCounters,
    cpu_load_max: f32,
    log: Writer,
    /// MIDI input from the MIDI threads (WP-2.2).
    midi: MidiIn,
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
            fallback_time_signatures: TimeSignatureMap::default(),
            metronome: MetronomeEngine::new(),
            output: vec![0.0; config.channels * config.max_frames],
            last: 0,
            clock: 0,
            playing: false,
            position: Frames::ZERO,
            insert_marker: Ticks::ZERO,
            loop_region: None,
            loop_frames: None,
            punch: Punch::default(),
            punch_frames: PunchFrames::default(),
            recording: false,
            plan: None,
            take: None,
            last_take: None,
            timed: Vec::with_capacity(config.max_scheduled),
            pending: Vec::with_capacity(config.max_pending_events),
            resolved: Vec::with_capacity(config.max_pending_events),
            scratch: Vec::with_capacity(config.max_pending_events),
            scheduled: Vec::with_capacity(config.max_scheduled),
            seq: 0,
            swaps: 0,
            session: SessionLauncher::new(config.max_slots),
            session_playback: Vec::with_capacity(config.max_slots),
            session_status: Vec::with_capacity(config.max_slots),
            counters: EngineCounters::default(),
            cpu_load_max: 0.0,
            log: Writer::new(),
            midi: MidiIn::new(config.sample_rate),
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

    /// Where Play starts.
    #[must_use]
    pub fn insert_marker(&self) -> Ticks {
        self.insert_marker
    }

    /// The Arrangement loop, if it's on.
    #[must_use]
    pub fn loop_region(&self) -> Option<LoopRegion> {
        self.loop_region
    }

    /// The running program.
    #[must_use]
    pub fn program(&self) -> Option<&EngineProgram> {
        self.program.as_ref().map(Program::get)
    }

    /// Gives the processor the consuming end of the MIDI input queue, which
    /// the MIDI device layer (`bl-midi-io`) and the computer MIDI keyboard
    /// fill (`docs/specs/midi-io.md`, R14). Call it before the processor
    /// runs on the audio thread: it returns the queue it had, if any, to be
    /// dropped there.
    pub fn set_midi_input(
        &mut self,
        queue: rtrb::Consumer<MidiInputEvent>,
    ) -> Option<rtrb::Consumer<MidiInputEvent>> {
        self.midi.set_queue(queue)
    }

    /// What a MIDI track's input received in the last block, and whether the
    /// track monitors. `None` before a program arrives, or if `track` isn't
    /// a MIDI track in it.
    #[must_use]
    pub fn midi_input(&self, track: bl_model::TrackId) -> Option<TrackMidiInput<'_>> {
        self.program()?.midi_input(track)
    }

    /// What the MIDI input stage has counted.
    #[must_use]
    pub fn midi_counters(&self) -> MidiCounters {
        self.midi.counters
    }

    /// Processes one block of `frames` frames (at most
    /// [`EngineConfig::max_frames`]; more are cut to that) and returns its
    /// output. Real-time safe. No device input is seen, and the block is
    /// timed from now: a device's callback goes through
    /// [`process_at`](Self::process_at), which takes its input and the time
    /// its callback ran.
    pub fn process(&mut self, frames: usize) -> Output<'_> {
        self.process_at(frames, &crate::NoInput, Instant::now())
    }

    /// Processes one block with the audio device's input, for tracks whose
    /// Audio From is Ext. In (WP-2.13). `frames` is at most
    /// [`EngineConfig::max_frames`]; more are cut to that. Real-time safe:
    /// the input is copied into the program's own buffers, which were made
    /// when the engine was.
    pub fn process_with_input(
        &mut self,
        frames: usize,
        input: &dyn crate::DeviceInput,
    ) -> Output<'_> {
        self.process_at(frames, input, Instant::now())
    }

    /// Processes one block, as [`Processor::process`] does, with the audio
    /// device's `input` and timed at `now`, the time the audio callback
    /// started the block. `now` is the clock MIDI input is placed against
    /// (`docs/specs/midi-io.md`, R15, R16): a device's callback passes the
    /// time it was called, so the block clock's delay-locked loop can smooth
    /// the jitter in when it runs, and a test passes times of its own.
    /// Real-time safe: the input is copied into the program's own buffers,
    /// which were made when the engine was.
    pub fn process_at(
        &mut self,
        frames: usize,
        input: &dyn crate::DeviceInput,
        now: Instant,
    ) -> Output<'_> {
        let began = Instant::now();
        let frames = frames.min(self.config.max_frames);
        self.drain();
        if let Some(program) = self.program.as_mut() {
            program.get_mut().copy_device_input(input, frames);
        }
        // Spec: midi-io R12, R14, R15
        let routes = self.program.as_mut().map(|p| &mut p.get_mut().midi);
        self.midi.take(now, self.clock, frames, routes);
        self.resolve_due(frames);
        let mut done = 0;
        while done < frames {
            self.apply_due_transport(done);
            self.apply_due_session();
            self.session.playback(&mut self.session_playback);
            self.fire_due(done);
            let length = self.until_next_event(done, frames - done);
            self.capture(done, length);
            self.render(done, length);
            done += length;
        }
        self.clock += frames as u64;
        self.last = frames;
        self.finish(began, frames);
        self.output()
    }

    /// Takes what's waiting on the queue and publishes the state, without
    /// processing audio: a processor about to start on a device is ready,
    /// and shows its program, position, and rate, before its first block.
    /// Called off the audio thread.
    pub(crate) fn prime(&mut self) {
        self.drain();
        self.publish(0.0);
    }

    /// Stops the transport where it is and publishes a state with it
    /// stopped, silent meters, and no load, for a processor whose stream has
    /// closed, and returns what the processor that takes over needs
    /// ([`carry`](Self::carry), taken before the stop). Called off the audio
    /// thread, once the stream is gone.
    #[must_use]
    pub fn park(&mut self) -> Carry {
        // A stream that closes ends the take, like a stop (WP-2.13); the
        // carry hands it to the next processor and the shell.
        self.end_recording();
        let carry = self.carry();
        // Spec: device-changes R1
        self.playing = false;
        self.metronome.stop();
        // The song stopped with the stream, so the Session View is at rest
        // too: nothing plays until a person launches again.
        self.session.clear();
        self.silence_meters();
        self.publish(0.0);
        carry
    }

    /// Lets every meter fall to silence at once, for a processor that has
    /// stopped making sound.
    fn silence_meters(&mut self) {
        if let Some(program) = self.program.as_mut() {
            let program = program.get_mut();
            program.silence_meters();
        }
    }

    /// What the processor that takes over needs: the clock, the transport
    /// in beats, the scheduled commands waiting to fire, and the transport
    /// commands waiting for their frames. Of those, it leaves out any that
    /// would start the song, which nothing does by itself after a restart
    /// (device-changes R7), and, if the song is playing, any Stop, which the
    /// restart has done already: run on the stopped song, it would send it
    /// back to 1.1.1. Allocates: call it off the audio thread.
    #[must_use]
    pub fn carry(&self) -> Carry {
        let rate = self.config.sample_rate;
        let timed = self
            .timed
            .iter()
            .filter(|entry| {
                !starts_playback(entry.command)
                    && !(self.playing && entry.command == TransportCommand::Stop)
            })
            .copied()
            .collect();
        Carry {
            clock: self.clock,
            sample_rate: rate,
            position: self.tempo().frames_to_ticks(self.position, rate),
            insert_marker: self.insert_marker,
            loop_region: self.loop_region,
            punch: self.punch,
            take: self.last_take,
            scheduled: self.scheduled.iter().map(|entry| entry.command).collect(),
            timed,
            launch: self.session.settings(),
        }
    }

    /// Takes over from the processor before this one (WP-1.13): carries on
    /// its clock, puts the song position and the insert marker on the same
    /// beats at this processor's rate, plays the same loop, and waits for
    /// the same commands, each transport command as long after the restart
    /// as it was after the stream closed. The song is stopped
    /// (device-changes R7), so Continue resumes where it was. Called off the
    /// audio thread, before the first block, once the program is in place,
    /// so positions fall on frames through its tempo map.
    pub(crate) fn take_over(&mut self, carry: &Carry) {
        let rate = self.config.sample_rate;
        self.clock = carry.clock;
        self.playing = false;
        self.metronome.stop();
        self.session.set_settings(carry.launch);
        self.position = self.frames_at(carry.position);
        self.insert_marker = carry.insert_marker;
        self.loop_region = carry.loop_region;
        self.punch = carry.punch;
        self.last_take = carry.take;
        self.update_loop_frames();
        for &command in &carry.scheduled {
            self.schedule(command);
        }
        for entry in &carry.timed {
            self.queue_transport(entry.command, carry.frame_at(entry.frame, rate));
        }
        self.publish(0.0);
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
                Message::Transport(command, When::Now) => self.transport(command),
                Message::Transport(command, When::Frame(frame)) => {
                    self.queue_transport(command, frame);
                }
                Message::Schedule(command) => self.schedule(command),
                Message::Session(command) => self.session_command(command),
                Message::Cancel(id) => self.scheduled.retain(|s| s.command.id != id),
                Message::Record(plan) => {
                    self.last_take = None;
                    self.plan = Some(plan);
                }
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
        // The loop keeps its bars under the new tempo map.
        self.update_loop_frames();
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
        // A change sent for a song frame is held back by its node's latency,
        // so automation lines up with the compensated audio (WP-2.11). One
        // sent to take effect now stays now: a person's control move isn't
        // delayed.
        let delay = match event.at {
            When::Now => 0,
            When::Frame(_) => self
                .program
                .as_ref()
                .and_then(|program| {
                    let program = program.get();
                    let slot = program.slot_of(event.key)?;
                    program.slots.get(slot as usize).map(|slot| slot.delay)
                })
                .unwrap_or(0),
        };
        let frame = match event.at {
            When::Now => self.clock,
            When::Frame(frame) => frame.saturating_add(u64::from(delay)),
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
        let instance = self.program.as_mut().and_then(|program| {
            let program = program.get_mut();
            let slot = program.slot_of(key)?;
            program.instance_mut(slot)
        });
        match instance {
            Some(instance) => {
                instance.receive_data(data);
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

    fn frames_at(&self, ticks: Ticks) -> Frames {
        self.tempo().ticks_to_frames(ticks, self.config.sample_rate)
    }

    /// Runs a transport command now, at the playhead
    /// (the transport spec).
    fn transport(&mut self, command: TransportCommand) {
        match command {
            // Spec: transport R1, R11
            TransportCommand::Play => self.play_from_insert_marker(),
            // Spec: transport R3, R4
            TransportCommand::Stop => {
                self.metronome.stop();
                // Spec: routing R11; WP-2.13. Stop ends the take, as Live's
                // Stop ends a recording.
                self.end_recording();
                if self.playing {
                    self.playing = false;
                } else {
                    self.insert_marker = Ticks::ZERO;
                    self.position = Frames::ZERO;
                }
            }
            // Spec: transport R6
            TransportCommand::Continue => self.playing = true,
            // Spec: transport R5
            TransportCommand::PlayStop => {
                if self.playing {
                    self.playing = false;
                    self.metronome.stop();
                    self.end_recording();
                } else {
                    self.play_from_insert_marker();
                }
            }
            // Spec: transport R7, R12
            TransportCommand::Locate(ticks) => {
                self.insert_marker = ticks.max(Ticks::ZERO);
                if self.playing {
                    self.position = self.frames_at(self.insert_marker);
                    self.metronome.stop();
                    // Playback jumps away from the take; it ends here
                    // (WP-2.13).
                    self.end_recording();
                }
            }
            // Spec: transport R9
            TransportCommand::InsertMarkerToPlayhead => {
                let ticks = self
                    .tempo()
                    .frames_to_ticks(self.position, self.config.sample_rate);
                self.insert_marker = ticks.max(Ticks::ZERO);
            }
            // Spec: control-bar-transport R15
            TransportCommand::SetInsertMarker(ticks) => {
                self.insert_marker = ticks.max(Ticks::ZERO);
            }
            TransportCommand::SetLoop(region) => {
                self.loop_region = region;
                self.update_loop_frames();
            }
            TransportCommand::SetMetronome(settings) => self.metronome.set_settings(settings),
            TransportCommand::SetPunch(punch) => {
                self.punch = punch;
                self.update_loop_frames();
            }
            TransportCommand::SetRecording(recording) => {
                self.metronome.set_recording(recording);
                if recording {
                    self.recording = true;
                } else {
                    self.end_recording();
                }
            }
        }
    }

    /// Ends the take being captured, publishing what it covers for the
    /// shell, and stops recording: what turning recording off, Stop, and a
    /// jump while recording do (WP-2.13). Dropping the plan closes the
    /// rings, so the writer thread knows the take is over.
    fn end_recording(&mut self) {
        if !self.recording {
            return;
        }
        self.end_take();
        self.recording = false;
        // Spec: metronome R7. The transport isn't recording any more, so
        // "only while recording" clicks stop with it, however it ended
        // (Stop, recording off, a locate, or a ring that filled).
        self.metronome.set_recording(false);
    }

    /// Ends the take being captured, publishing what it covers, and drops the
    /// plan, so nothing more is written; the transport keeps recording.
    /// What reaching the punch-out point does (WP-2.13).
    fn end_take(&mut self) {
        if let Some(take) = self.take.take() {
            self.last_take = Some(TakeBounds {
                start: take.start,
                end: take.start + Frames(take.written as i64),
            });
        }
        self.plan = None;
    }

    fn play_from_insert_marker(&mut self) {
        let (tempo, sigs) = match &self.program {
            Some(p) => (&p.get().tempo, &p.get().time_signatures),
            None => (&self.fallback_tempo, &self.fallback_time_signatures),
        };
        self.metronome
            .start_count_in(self.config.sample_rate, tempo, sigs, self.insert_marker);
        self.position = self.frames_at(self.insert_marker);
        self.playing = true;
    }

    /// Works out the loop's and the punch points' frames from their ticks,
    /// under the running tempo map. A loop shorter than a frame isn't played.
    fn update_loop_frames(&mut self) {
        self.punch_frames = PunchFrames {
            start: self
                .punch
                .start
                .map(|ticks| self.frames_at(ticks.max(Ticks::ZERO))),
            end: self.punch.end.map(|ticks| self.frames_at(ticks)),
        };
        self.loop_frames = self.loop_region.and_then(|region| {
            let start = self.frames_at(region.start.max(Ticks::ZERO));
            let end = self.frames_at(region.end);
            (end > start).then_some(LoopFrames { start, end })
        });
    }

    /// Holds a transport command for a frame of the engine clock, or runs it
    /// now if that frame has come.
    fn queue_transport(&mut self, command: TransportCommand, frame: u64) {
        if frame <= self.clock {
            self.transport(command);
        } else if self.timed.len() < self.timed.capacity() {
            self.timed.push(Timed { frame, command });
        } else {
            self.counters.transport_overflowed += 1;
        }
    }

    /// Runs the transport commands due by `offset` frames into the block,
    /// in the order they came.
    fn apply_due_transport(&mut self, offset: usize) {
        let now = self.clock + offset as u64;
        let mut i = 0;
        while i < self.timed.len() {
            if self.timed[i].frame <= now {
                // `remove` keeps the rest in the order they came.
                let due = self.timed.remove(i);
                self.transport(due.command);
            } else {
                i += 1;
            }
        }
    }

    /// Runs a Session View command (WP-3.2): a launch, a stop, or the
    /// global launch quantisation.
    fn session_command(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::Launch { track, scene } => self.launch_slot(track, scene),
            SessionCommand::LaunchScene(scene) => self.launch_scene(scene),
            SessionCommand::StopTrack(track) => self.stop_session(Some(track)),
            SessionCommand::StopAll => self.stop_session(None),
            SessionCommand::SetQuantization(settings) => self.session.set_settings(settings),
        }
    }

    /// Launches a slot's clip: on the next grid line while the song plays,
    /// and at once from the Arrangement position while it's stopped, which
    /// also starts the transport (the launch quantisation spec, R5).
    fn launch_slot(&mut self, track: TrackId, scene: SceneId) {
        let was_playing = self.playing;
        if !was_playing {
            // Spec: launch-quantisation R5. A launch with the transport
            // stopped starts it at once, from the Arrangement position, and
            // the clip starts with it.
            self.play_from_insert_marker();
        }
        let playhead = Self::playhead(
            self.program.as_ref().map(Program::get),
            &self.fallback_tempo,
            &self.fallback_time_signatures,
            was_playing,
            self.position,
            self.config.sample_rate,
        );
        self.session.launch(track, scene, playhead);
    }

    /// The launch scene's clips: a scene with nothing in it doesn't start the
    /// transport, so launching an empty scene does nothing (observed in Live
    /// 12.4.3; what a scene with a clip does while stopped is in the spec's
    /// "To observe in Live").
    fn launch_scene(&mut self, scene: SceneId) {
        let holds_a_clip = self
            .program
            .as_ref()
            .is_some_and(|program| !program.get().scene_slots(scene).is_empty());
        if !holds_a_clip {
            return;
        }
        let was_playing = self.playing;
        if !was_playing {
            self.play_from_insert_marker();
        }
        let slots = self
            .program
            .as_ref()
            .map_or(&[][..], |program| program.get().scene_slots(scene));
        let playhead = Self::playhead(
            self.program.as_ref().map(Program::get),
            &self.fallback_tempo,
            &self.fallback_time_signatures,
            was_playing,
            self.position,
            self.config.sample_rate,
        );
        self.session.launch_scene(scene, slots, playhead);
    }

    /// Stops one track's or every track's playing clip on the next grid
    /// line (R6).
    fn stop_session(&mut self, track: Option<TrackId>) {
        let playhead = Self::playhead(
            self.program.as_ref().map(Program::get),
            &self.fallback_tempo,
            &self.fallback_time_signatures,
            self.playing,
            self.position,
            self.config.sample_rate,
        );
        self.session.stop(track, playhead);
    }

    /// The transport as the launcher reads it. The maps are borrowed from
    /// the running program, which is a different field of the processor
    /// from the launcher, so both can be borrowed at once.
    fn playhead<'a>(
        program: Option<&'a EngineProgram>,
        fallback_tempo: &'a TempoMap,
        fallback_signatures: &'a TimeSignatureMap,
        wait_for_grid: bool,
        position: Frames,
        rate: SampleRate,
    ) -> Playhead<'a> {
        let (tempo, signatures) = match program {
            Some(program) => (&program.tempo, &program.time_signatures),
            None => (fallback_tempo, fallback_signatures),
        };
        Playhead {
            wait_for_grid,
            position: tempo.frames_to_ticks(position, rate),
            tempo,
            signatures,
            rate,
        }
    }

    /// Applies the Session View changes due at the playhead, which is where
    /// a block part starts. While the song is stopped the playhead stands
    /// still, so a change queued for a frame waits for it (R4).
    fn apply_due_session(&mut self) {
        if self.playing {
            self.session.apply_due(self.position);
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
            // A scheduled command fires only while the song plays, where
            // these would start it again or do nothing.
            Action::Transport(TransportCommand::Play | TransportCommand::Continue) => {}
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

    /// How many of the `remaining` frames from `offset` in the block to run
    /// before the transport next acts: a transport command's frame comes, a
    /// scheduled command fires, or the playhead reaches the loop's end. At
    /// least 1.
    fn until_next_event(&self, offset: usize, remaining: usize) -> usize {
        let now = self.clock + offset as u64;
        let timed = self
            .timed
            .iter()
            .map(|entry| entry.frame.saturating_sub(now));
        let mut next = timed
            .filter(|&ahead| ahead > 0 && ahead < remaining as u64)
            .min()
            .map_or(remaining, |ahead| ahead as usize);
        if !self.playing {
            return next;
        }
        if self.metronome.is_counting_in() {
            let count_in = self.metronome.count_in_remaining();
            if count_in > 0 && count_in < next {
                next = count_in;
            }
            return next;
        }
        let mut song = |frame: Frames| {
            let ahead = frame.saturating_sub(self.position).get();
            if ahead > 0 && ahead < next as i64 {
                next = ahead as usize;
            }
        };
        for entry in &self.scheduled {
            song(entry.frame);
        }
        // Spec: launch-quantisation R2. A launch or a stop lands on its
        // grid frame, so the block splits there.
        if let Some(due) = self.session.next_due(self.position) {
            song(due);
        }
        // Spec: transport R13
        if let Some(region) = self.loop_frames {
            song(region.end);
        }
        // Spec: recording R6. A part of a block lies wholly inside the punch
        // points or wholly outside them, so the take starts and ends on the
        // frame.
        if self.recording {
            for frame in [self.punch_frames.start, self.punch_frames.end]
                .into_iter()
                .flatten()
            {
                song(frame);
            }
        }
        next
    }

    /// Runs the program over `length` frames from `offset` in the block, and
    /// moves the playhead on if the song is playing. Playback that reaches
    /// the loop's end goes on from its start: the parts of a block end
    /// there, so the next part starts at the loop's start.
    fn render(&mut self, offset: usize, length: usize) {
        let stride = self.config.max_frames;
        if self.metronome.is_counting_in() {
            for c in 0..self.config.channels {
                let to = c * stride + offset;
                self.output[to..to + length].fill(0.0);
            }
            self.metronome.render_count_in(
                Block {
                    output: &mut self.output,
                    stride,
                    channels: self.config.channels,
                    offset,
                    length,
                },
                self.config.sample_rate,
            );
            return;
        }

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
                    &self.session_playback,
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
            let (tempo, sigs) = match &self.program {
                Some(p) => (&p.get().tempo, &p.get().time_signatures),
                None => (&self.fallback_tempo, &self.fallback_time_signatures),
            };
            // Spec: metronome R7. With Punch-In on, "only while recording"
            // clicks start at the punch-in point, not at play start.
            self.metronome.set_recording(
                self.recording && self.punch_frames.start.is_none_or(|at| self.position >= at),
            );
            self.metronome.render_active(
                Block {
                    output: &mut self.output,
                    stride,
                    channels: self.config.channels,
                    offset,
                    length,
                },
                self.position,
                self.config.sample_rate,
                tempo,
                sigs,
            );
            let before = self.position;
            self.position += Frames(length as i64);
            // Spec: transport R13, R14
            if let Some(region) = self.loop_frames
                && before < region.end
                && self.position >= region.end
            {
                debug_assert_eq!(
                    self.position, region.end,
                    "a block part crossed the loop's end"
                );
                self.position = region.start;
            }
        }
    }

    /// Copies this part of the block's device input into the recording
    /// rings, when the transport records (WP-2.13). The take starts at the
    /// first played part, so its first sample is the frame playback started
    /// on; a part the rings can't take in full, or playback jumping away
    /// from the take's next frame, ends the take before the part, so every
    /// file holds exactly the take's frames. Real-time safe: copies into
    /// rings made when the engine was, no locks.
    fn capture(&mut self, offset: usize, frames: usize) {
        if !self.recording || !self.playing || self.metronome.is_counting_in() || frames == 0 {
            return;
        }
        if self.plan.is_none() || self.program.is_none() {
            return;
        }
        // Spec: recording R6. Nothing is written before the punch-in point,
        // and the take ends at the punch-out point; the transport carries on.
        if self.punch_frames.start.is_some_and(|at| self.position < at) {
            return;
        }
        if self.punch_frames.end.is_some_and(|at| self.position >= at) {
            self.end_take();
            return;
        }
        let expected = self.take.map_or(self.position, |take| {
            take.start + Frames(take.written as i64)
        });
        if self.take.is_some() && expected != self.position {
            self.end_recording();
            return;
        }
        // Spec: recording R7. The recording is moved `delay` frames earlier:
        // the audio that arrived in the take's first `delay` frames sounds
        // before it started, so it isn't written. The shell places the
        // clips from the delay and the take.
        // shortcut: the take ends when playback does, so its last `delay`
        // frames of sound are never heard back and the clip is that much
        // shorter; upgrade by recording on past the end for `delay` frames.
        let delay = self.plan.as_mut().map_or(0, |plan| plan.get_mut().delay);
        if self.take.is_none() {
            self.last_take = None;
            self.take = Some(Take {
                start: self.position,
                written: 0,
            });
        }
        let written = self.take.map_or(0, |take| take.written);
        let skip = (delay.max(0) as u64)
            .saturating_sub(written)
            .min(frames as u64) as usize;
        let captured = match (self.program.as_ref(), self.plan.as_mut()) {
            (Some(program), Some(plan)) => {
                plan.get_mut()
                    .capture(program.get(), self.position, offset, frames, skip)
            }
            _ => false,
        };
        if !captured {
            self.end_recording();
            return;
        }
        if let Some(take) = &mut self.take {
            take.written += frames as u64;
        }
    }

    /// Ends the block: CPU load, and the state for the UI.
    fn finish(&mut self, began: Instant, frames: usize) {
        let rate = self.config.sample_rate.as_f64();
        let lasts = frames as f64 / rate;
        let cpu_load = if lasts > 0.0 {
            (began.elapsed().as_secs_f64() / lasts) as f32
        } else {
            0.0
        };
        self.cpu_load_max = self.cpu_load_max.max(cpu_load);
        self.publish(cpu_load);
    }

    /// Publishes the state for the UI, with this load.
    fn publish(&mut self, cpu_load: f32) {
        let position_ticks = self
            .tempo()
            .frames_to_ticks(self.position, self.config.sample_rate);

        let state = self.state.input_buffer_mut();
        state.clock = self.clock;
        state.playing = self.playing;
        state.position = self.position;
        state.position_ticks = position_ticks;
        state.insert_marker = self.insert_marker;
        state.loop_region = self.loop_region;
        state.recording = self.recording;
        state.take = self.last_take;
        state.program = self.program.as_ref().map(|p| p.get().generation);
        state.swaps = self.swaps;
        state.cpu_load = cpu_load;
        state.cpu_load_max = self.cpu_load_max;
        state.counters = self.counters;
        self.session.status(&mut self.session_status);
        state.session.clear();
        state.session.extend_from_slice(&self.session_status);
        state.meters.clear();
        state.meters_truncated = false;
        if let Some(program) = &self.program {
            let program = program.get();
            for &task in &program.order {
                let reading = program.meters(task);
                let meter = TrackMeter {
                    track: program.tasks[task as usize].track,
                    peak: reading.peak,
                    rms: reading.rms,
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
            .field("insert_marker", &self.insert_marker)
            .field("loop_region", &self.loop_region)
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
