//! The audio thread's side of the engine.

use std::sync::atomic::Ordering::Relaxed;
use std::time::Instant;

use bl_core::midi::{MidiInputEvent, MidiOutputEvent};
use bl_core::time::{Frames, SampleRate, TICKS_PER_QUARTER, TempoMap, Ticks, TimeSignatureMap};
use bl_model::{LaunchSettings, ParamId, Scale, SceneId, TrackId};
use bl_rt::log::{Level, Writer};
use bl_rt::rt_log;
use rtrb::{Consumer, Producer};

use crate::EngineConfig;
use crate::capture::CapturedMidi;
use crate::follower::{Ask, Follow, Follower};
use crate::gc::{Owned, Program};
use crate::link_audio::LinkAudio;
use crate::message::{
    Action, LoopRegion, Message, NudgeDirection, ParamEvent, Punch, Report, ScheduledCommand,
    TransportCommand, When,
};
use crate::metronome::{Block, MetronomeEngine};
use crate::midi::{MidiCounters, MidiIn, TrackMidiInput};
use crate::node::{NodeData, NodeKey, ParamValue, TransportInfo};
use crate::program::{Blk, EngineProgram, Resolved};
use crate::recording::{RecordingPlan, TakeBounds};
use crate::render::RenderPlan;
use crate::scales::ScaleRuns;
use crate::sched::{Lane, Pool};
use crate::session::{
    Button, ClipOffset, Played, Playhead, SceneOverride, SessionCommand, SessionEvent,
    SessionLauncher, SessionPlayback, SlotState, SlotStatus,
};
use crate::state::{EngineCounters, EngineState, TrackMeter};
use crate::sync_out::{Beats, Part as SyncPart, SyncOut};
use crate::tempo_follower::Leader;

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
    /// Whether the follower gave it: a start of its own isn't held back by
    /// the wait for one (the transport spec, R25).
    by_follower: bool,
}

/// What a start that waits for the follower's frame holds (the transport
/// spec, R25).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    Transport(TransportCommand),
    Session(SessionCommand),
}

/// How many starts can wait for the follower's frame at once.
const MAX_HELD: usize = 8;

/// The Phase Nudge buttons held (the transport spec, R27).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Nudge {
    down: bool,
    up: bool,
    /// The one pressed last, for when both are held.
    last: Option<NudgeDirection>,
}

impl Nudge {
    /// The button that nudges: the only one held, or the last pressed of two.
    fn active(self) -> Option<NudgeDirection> {
        match (self.down, self.up) {
            (false, false) => None,
            (true, false) => Some(NudgeDirection::Down),
            (false, true) => Some(NudgeDirection::Up),
            (true, true) => self.last,
        }
    }

    /// How much faster the song plays: 4 % either way (R27).
    fn factor(self) -> f64 {
        match self.active() {
            None => 1.0,
            Some(NudgeDirection::Down) => 0.96,
            Some(NudgeDirection::Up) => 1.04,
        }
    }
}

/// How far, in frames, a part's song frames may bend away from a straight
/// line against its frames before it is cut short (the transport spec, R24).
const BEND_FRAMES: f64 = 0.25;

/// A part of a block, with the song frames it covers worked out.
struct Part {
    /// Frames in the part: at most what was asked for.
    length: usize,
    transport: TransportInfo,
    /// Where the song stands when the part ends, if it moves other than a
    /// frame per frame.
    end: Option<Exact>,
}

/// Where the song stood exactly when the last part ended, so the next picks
/// up where it left off (the transport spec, R24): the song frame it
/// started on, the fraction of a frame past that the song really was, and,
/// while following, the song's exact position in ticks. The next part uses
/// it only if the playhead is still on `position`; a locate or a loop's
/// wrap moves it, and the part then starts afresh from the playhead.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Exact {
    position: Frames,
    phase: f64,
    ticks: Option<f64>,
}

/// The Arrangement loop in song frames, under the running program's tempo
/// map. Only a loop at least one frame long is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LoopFrames {
    start: Frames,
    end: Frames,
    /// The same region in ticks, which the Session's lines count in.
    ticks: LoopRegion,
}

/// A jump waiting for the next line of the global launch quantisation (the
/// transport spec, R22): the frame the playhead waits for, and where it goes
/// when it gets there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingLocate {
    /// The frame the jump waits for: the next line's frame.
    frame: Frames,
    /// Where the playhead goes when the frame comes.
    to: Frames,
}

/// [`Punch`] in song frames, under the running program's tempo map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PunchFrames {
    start: Option<Frames>,
    end: Option<Frames>,
}

/// The take being recorded (WP-2.13): the song frame it started at, how
/// many frames its rings hold, the song frame its next sample has, and the
/// loop it went round, if it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Take {
    start: Frames,
    written: u64,
    next: Frames,
    wrap: Option<LoopFrames>,
    /// The song frames the take has covered, wraps of the loop included: the
    /// song moves other than a frame per frame while it follows an outside
    /// tempo (the transport spec, R24).
    song: i64,
}

/// A Session recording asked for (WP-3.11): the grid frame its take starts
/// at, and, once it's asked to end, the grid frame it ends at and whether
/// the new clips are launched there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SessionTake {
    start: Frames,
    stop: Option<Frames>,
    launch: bool,
    /// Where the playhead stood at the last part captured: the playhead
    /// being behind it means the Arrangement loop wrapped.
    last: Option<Frames>,
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

/// The first frame, counted from a part starting at song frame `position`
/// and `phase` past it, that song frame `target` is nearest, at `speed` song
/// frames per frame: at least 1 (the transport spec, R24).
fn frames_until(position: i64, phase: f64, speed: f64, target: i64) -> usize {
    if speed == 1.0 && phase == 0.0 {
        return usize::try_from(target - position).unwrap_or(1).max(1);
    }
    let here = position as f64 + phase;
    let nearest = ((target as f64 - here + crate::node::TIE) / speed + 0.5).floor();
    nearest.max(1.0) as usize
}

/// The furthest position a locate, a scrub or the insert marker can name:
/// a million quarter notes. An agent's absurd position would otherwise
/// overflow the song-frame arithmetic at a slow tempo.
const MAX_POSITION: Ticks = Ticks::from_quarters(1_000_000);

/// `ticks` within the song's range, from the start to [`MAX_POSITION`].
fn clamp_position(ticks: Ticks) -> Ticks {
    ticks.clamp(Ticks::ZERO, MAX_POSITION)
}

/// The frame `frame` of the song under `before` is, under `after`: the
/// same beat.
fn same_beat(before: &TempoMap, after: &TempoMap, rate: SampleRate, frame: Frames) -> Frames {
    after.ticks_to_frames(before.frames_to_ticks(frame, rate), rate)
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
    /// The device's output channels of `max_frames` frames each, holding
    /// what the tracks on Ext. Out play (WP-2.13): `ext_width` of them are
    /// in use.
    ext_output: Vec<f32>,
    ext_width: usize,
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
    /// The span a held click in the scrub area loops, until it's released
    /// (the transport spec, R23).
    held_scrub: Option<LoopRegion>,
    /// `held_scrub` in frames, if it's at least a frame long.
    held_frames: Option<LoopFrames>,
    /// Where the song starts, when it has a song start time (the transport
    /// spec, R21).
    song_start: Option<Ticks>,
    /// The Global Amount of groove, 1 at 100 %.
    groove_amount: f32,
    /// The set's current scale (the scales spec, R9).
    scale: Scale,
    /// Whether the set's tuning hides the scales (the tunings spec, R16).
    scales_hidden: bool,
    /// A jump waiting for the next line of the global launch quantisation
    /// (the transport spec, R22).
    pending_locate: Option<PendingLocate>,
    /// Where recording punches in and out (WP-2.13).
    punch: Punch,
    /// `punch` in frames.
    punch_frames: PunchFrames,
    /// Whether the transport records (WP-2.13).
    recording: bool,
    /// What to record: the shell's plan, whose rings close when it goes.
    plan: Option<Owned<RecordingPlan>>,
    /// What to capture while the song plays a range in real time
    /// (WP-5.35): the shell's plan, whose rings close when it goes.
    rendering: Option<Owned<RenderPlan>>,
    /// The take being captured, if playback is running under recording.
    take: Option<Take>,
    /// The Session recording under way or waiting for the grid (WP-3.11).
    session_take: Option<SessionTake>,
    /// The take that last ended, kept for the shell to make its clips,
    /// until the next one starts.
    last_take: Option<TakeBounds>,
    /// Transport commands waiting for their frames, in the order they came.
    timed: Vec<Timed>,
    /// The outside tempo the song follows, if one is set (the transport
    /// spec, R24).
    follower: Option<Owned<Box<dyn Follower>>>,
    /// Tempo Follower: the tempo the tracker hears in the device's input
    /// leads the song, ahead of any Link session (the Tempo Follower spec,
    /// R6).
    leader: Option<Owned<Box<dyn Leader>>>,
    /// The Link Audio the song reads peers' channels from and offers its
    /// tracks through (the Link Audio spec, R4 to R6).
    link_audio: Option<Owned<Box<dyn LinkAudio>>>,
    /// Where Link's clock has this block's first frame heard, in
    /// microseconds, if a Link session follows (R6).
    moment: Option<i64>,
    /// What the follower answered for this block.
    follow: Follow,
    /// The tempo this block plays at, if the follower gave one.
    followed: Option<f64>,
    /// The CV Clock In leading the song's tempo this block (cv-tools R25).
    cv_leader: Option<crate::cv_clock::Leader>,
    /// Whether the follower decides when a start happens (R25).
    deciding: bool,
    /// The clock frame of the block the follower was last asked about.
    asked: Option<u64>,
    /// Starts waiting for the follower's frame, and the frame it gave.
    held: Vec<Held>,
    release: Option<u64>,
    /// Set while a held start or a follower's own command runs, so it isn't
    /// held again.
    releasing: bool,
    /// EXT: whether the incoming MIDI clock leads the song (midi-clock R17).
    ext: bool,
    /// Whether an outside change of the tempo has taken a tempo envelope's
    /// place, until Re-Enable Automation (the Link spec, R4).
    tempo_overridden: bool,
    /// Whether the follower says no count-in plays (the metronome spec,
    /// R10).
    no_count_in: bool,
    /// The Phase Nudge buttons (R27).
    nudge: Nudge,
    /// Where the song exactly stood at the end of the last part.
    exact: Option<Exact>,
    pending: Vec<Pending>,
    /// This block's parameter changes, sorted by slot, frame, and arrival.
    resolved: Vec<Resolved>,
    /// What the audio thread keeps to itself while it runs tasks.
    lane: Lane,
    /// The workers that run tasks beside the audio thread, if the engine
    /// has any (WP-5.37).
    pool: Option<Pool>,
    scheduled: Vec<Scheduled>,
    seq: u64,
    swaps: u64,
    /// The Session View's slots and the launch quantisation (WP-3.2).
    session: SessionLauncher,
    /// What the tracks' players play this block, rebuilt when the launcher
    /// changes.
    session_playback: Vec<SessionPlayback>,
    /// The tempo and signature a launched scene put in place (WP-3.5).
    scene_override: SceneOverride,
    /// The slots' states, published after each block.
    session_status: Vec<SlotStatus>,
    counters: EngineCounters,
    cpu_load_max: f32,
    log: Writer,
    /// MIDI input from the MIDI threads (WP-2.2).
    midi: MidiIn,
    /// The frames MIDI to ports is sent after its block starts to be heard
    /// with the audio (WP-5.34): the device's reported output latency.
    output_latency: u32,
    /// MIDI clock out to the ports with Sync on (WP-7.11).
    sync_out: SyncOut,
    /// The notes monitoring MIDI tracks received, for Capture MIDI
    /// (WP-3.14).
    captured: Producer<CapturedMidi>,
}

impl Processor {
    pub(crate) fn new(
        config: EngineConfig,
        queue: Consumer<Message>,
        reports: Producer<Report>,
        state: triple_buffer::Input<EngineState>,
        captured: Producer<CapturedMidi>,
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
            ext_output: vec![0.0; config.output_channels * config.max_frames],
            ext_width: 0,
            last: 0,
            clock: 0,
            playing: false,
            position: Frames::ZERO,
            insert_marker: Ticks::ZERO,
            loop_region: None,
            loop_frames: None,
            held_scrub: None,
            held_frames: None,
            song_start: None,
            groove_amount: 1.0,
            scale: Scale::default(),
            scales_hidden: false,
            pending_locate: None,
            punch: Punch::default(),
            punch_frames: PunchFrames::default(),
            recording: false,
            plan: None,
            rendering: None,
            take: None,
            session_take: None,
            last_take: None,
            timed: Vec::with_capacity(config.max_scheduled),
            follower: None,
            leader: None,
            link_audio: None,
            moment: None,
            ext: false,
            follow: Follow::with_capacity(config.max_scheduled),
            followed: None,
            cv_leader: None,
            deciding: false,
            asked: None,
            held: Vec::with_capacity(MAX_HELD),
            release: None,
            releasing: false,
            nudge: Nudge::default(),
            tempo_overridden: false,
            no_count_in: false,
            exact: None,
            pending: Vec::with_capacity(config.max_pending_events),
            resolved: Vec::with_capacity(config.max_pending_events),
            lane: Lane::new(&config),
            pool: None,
            scheduled: Vec::with_capacity(config.max_scheduled),
            seq: 0,
            swaps: 0,
            session: SessionLauncher::new(config.max_slots),
            session_playback: Vec::with_capacity(config.max_slots),
            scene_override: SceneOverride::default(),
            session_status: Vec::with_capacity(config.max_slots),
            counters: EngineCounters::default(),
            cpu_load_max: 0.0,
            log: Writer::new(),
            midi: MidiIn::new(config.sample_rate),
            output_latency: config.output_latency,
            sync_out: SyncOut::new(),
            captured,
            config,
        }
    }

    /// Gives the processor a pool of workers to run tasks with from the next
    /// block on (the multicore scheduler, WP-5.37), and returns the pool it
    /// had. The output is the same to the bit with or without one. Dropping
    /// a pool stops its workers, so a caller on the audio thread hands the
    /// old one to another thread to drop.
    pub fn set_pool(&mut self, pool: Option<Pool>) -> Option<Pool> {
        std::mem::replace(&mut self.pool, pool)
    }

    /// The pool of workers, if there is one.
    #[must_use]
    pub fn pool(&self) -> Option<&Pool> {
        self.pool.as_ref()
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

    /// The tempo map the processor converts ticks with.
    #[must_use]
    pub fn tempo_map(&self) -> &TempoMap {
        self.program()
            .map_or(&self.fallback_tempo, EngineProgram::tempo)
    }

    /// The time-signature map the processor reads bar lines from.
    #[must_use]
    pub fn time_signatures(&self) -> &TimeSignatureMap {
        self.program().map_or(
            &self.fallback_time_signatures,
            EngineProgram::time_signatures,
        )
    }

    /// The global launch quantisation and its switch.
    #[must_use]
    pub fn launch(&self) -> LaunchSettings {
        self.session.settings()
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

    /// Gives the processor the producing end of the queue to the MIDI output
    /// thread (`bl-midi-io`'s `MidiPorts::attach_output`), which the messages
    /// for tracks' MIDI To ports and external instruments leave on, each
    /// stamped with the moment its frame is heard at the audio interface's
    /// output (`docs/specs/external-devices.md`, R4, R5). Call it before the
    /// processor runs on the audio thread: it returns the queue it had, if
    /// any, to be dropped there. A processor without one, such as an offline
    /// render's, sends nothing (R8).
    pub fn set_midi_output(
        &mut self,
        queue: rtrb::Producer<MidiOutputEvent>,
    ) -> Option<rtrb::Producer<MidiOutputEvent>> {
        self.midi.set_out(queue)
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
        // Spec: cv-tools R24. A follower that arrives in the drain is asked at
        // once, and a CV Clock In reads this block's input then: copy it
        // before, and again after, for a program that arrives in the drain.
        if let Some(program) = self.program.as_mut() {
            program.get_mut().copy_device_input(input, frames);
        }
        self.drain(frames);
        if let Some(program) = self.program.as_mut() {
            program.get_mut().copy_device_input(input, frames);
        }
        // Spec: midi-io R12, R14, R15
        let routes = self.program.as_mut().map(|p| &mut p.get_mut().midi);
        self.midi.take(now, self.clock, frames, routes);
        self.resolve_due(frames);
        // Spec: tempo-follower R10, R11. Tempo Follower hears every block,
        // stopped or playing, before it is asked.
        if let Some(leader) = self.leader.as_mut() {
            leader.get_mut().listen(input, frames);
        }
        self.ask_follower(frames);
        self.read_link_audio(frames);
        let mut done = 0;
        while done < frames {
            self.apply_due_transport(done);
            self.apply_due_release(done);
            // Spec: transport R22. A jump waiting for the next line of the
            // global launch quantisation.
            self.apply_due_locate();
            self.apply_due_session();
            self.session.playback(&mut self.session_playback);
            self.fire_due(done);
            let length = self.until_next_event(done, frames - done);
            let part = self.pace(length);
            let length = part.length;
            self.capture(done, length, part.transport);
            self.log_captured(done, length, part.transport);
            self.render(done, length, part);
            done += length;
        }
        self.send_link_audio(frames);
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
        self.drain(0);
        self.publish(0.0, 0);
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
        self.end_recordings();
        let carry = self.carry();
        // Spec: device-changes R1
        self.playing = false;
        self.metronome.stop();
        // Spec: external-devices R6. No note stays held on the hardware when
        // the stream stops or restarts.
        if let Some(program) = self.program.as_mut() {
            self.midi.release(&mut program.get_mut().midi);
        }
        // Spec: midi-clock R5. Hardware that was following stops with the
        // stream.
        self.sync_out.park(&mut self.midi);
        // The song stopped with the stream, so the Session View is at rest
        // too: nothing plays until a person launches again.
        self.session.clear();
        self.silence_meters();
        self.publish(0.0, 0);
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
                !entry.by_follower
                    && !starts_playback(entry.command)
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
            self.queue_transport(entry.command, carry.frame_at(entry.frame, rate), false);
        }
        self.publish(0.0, 0);
    }

    /// The last block's output.
    #[must_use]
    pub fn output(&self) -> Output<'_> {
        Output {
            data: &self.output,
            channels: self.config.channels,
            frames: self.last,
            stride: self.config.max_frames,
            ext: &self.ext_output,
            ext_channels: self.ext_width,
        }
    }

    /// Takes what's on the queue, up to its capacity, so the work is
    /// bounded even if the sender keeps sending.
    fn drain(&mut self, frames: usize) {
        for _ in 0..self.config.queue_capacity {
            let Ok(message) = self.queue.pop() else {
                break;
            };
            self.counters.messages += 1;
            match message {
                Message::Program(program) => self.swap(program),
                Message::Param(event) => self.queue_param(event),
                Message::Data { key, data } => self.deliver(key, data),
                // Swapped, not assigned: the tables the program held go to
                // the drop thread with the message.
                Message::SceneTables(mut tables) => {
                    if let Some(program) = self.program.as_mut() {
                        let (program, tables) = (program.get_mut(), tables.get_mut());
                        std::mem::swap(&mut program.scene_slots, &mut tables.slots);
                        std::mem::swap(&mut program.follow, &mut tables.follow);
                    }
                }
                Message::Transport(command, When::Now) => {
                    // Spec: control-bar-transport R20. A Stop abandons a
                    // start that waits for its frame.
                    if command == TransportCommand::Stop {
                        self.timed.retain(|entry| !starts_playback(entry.command));
                    }
                    self.transport(command);
                }
                Message::Transport(command, When::Frame(frame)) => {
                    self.queue_transport(command, frame, false);
                }
                Message::Follower(follower) => {
                    // Dropping the old one only lets go of it: the drop
                    // thread frees it.
                    drop(std::mem::replace(&mut self.follower, follower));
                    // Asked at once, so a start sent behind it already waits
                    // for it (the transport spec, R25).
                    self.asked = None;
                    self.ask_follower(frames);
                }
                Message::Leader(leader) => {
                    // Dropping the old one only lets go of it: the drop
                    // thread frees it.
                    drop(std::mem::replace(&mut self.leader, leader));
                }
                Message::LinkAudio(link_audio) => {
                    // As the follower: dropping the old one only lets go of it.
                    drop(std::mem::replace(&mut self.link_audio, link_audio));
                }
                Message::Schedule(command) => self.schedule(command),
                Message::Session(command) => self.session_command(command),
                Message::Cancel(id) => self.scheduled.retain(|s| s.command.id != id),
                Message::Record(plan) => {
                    self.last_take = None;
                    self.plan = Some(plan);
                }
                Message::Render(mut plan) => {
                    // Spec: external-devices R24. Only a plan with outputs
                    // renders anything: an empty one only closes the rings of
                    // the render before it, and the song must play on after
                    // one, not stop at the frame it names.
                    self.rendering = (!plan.get_mut().outputs.is_empty()).then_some(plan);
                }
                Message::OutputLatency(frames) => self.output_latency = frames,
                Message::SyncOutput {
                    index,
                    count,
                    output,
                } => self.sync_out.set_output(index, count, output),
                Message::SyncInput {
                    index,
                    count,
                    input,
                } => self.midi.sync_in.set_input(index, count, input),
                Message::ExternalSync(on) => self.ext = on,
                Message::MidiOutputOpened(port) => {
                    if let Some(program) = self.program.as_mut() {
                        program.get_mut().midi.reopen_port(port);
                    }
                }
            }
        }
    }

    /// Swaps a new program in at the block boundary. Its surviving nodes
    /// move across from the running one, and the running one, with the nodes
    /// that didn't survive, goes to the drop thread.
    fn swap(&mut self, mut program: Program) {
        let mut mismatch = None;
        // Spec: session-view R23. A launched scene's tempo and signature
        // carry over to the new program, which may have been compiled before
        // the audio thread applied them.
        self.align_with_override(program.get_mut());
        match self.program.as_mut() {
            Some(old) => {
                if !program.get_mut().adopt(old.get_mut()) {
                    mismatch = Some(Some(old.get().generation));
                }
                let before = old.get().tempo.clone();
                let after = program.get().tempo.clone();
                if before != after {
                    self.move_to_tempo(&before, &after);
                }
            }
            None => {
                if program.get().base.is_some() {
                    mismatch = Some(None);
                }
                let before = self.fallback_tempo.clone();
                let after = program.get().tempo.clone();
                self.move_to_tempo(&before, &after);
            }
        }
        if let Some(running) = mismatch {
            self.mismatch(program.get(), running);
        }
        self.session.retain_tracks(&program.get().session_tracks);
        // Dropping the old program here only lets go of it: the drop thread
        // frees it.
        drop(self.program.replace(program));
        self.swaps += 1;
        // The loop keeps its bars under the new tempo map.
        self.update_loop_frames();
        // A take that went round the loop keeps going round it, at the loop's
        // new frames.
        if let Some(take) = &mut self.take
            && take.wrap.is_some()
        {
            take.wrap = self.held_frames.or(self.loop_frames);
        }
    }

    /// Puts the playhead, the scheduled commands, and the Session View's
    /// frames on the same beats under `after`, a tempo map that replaces
    /// `before`.
    fn move_to_tempo(&mut self, before: &TempoMap, after: &TempoMap) {
        let rate = self.config.sample_rate;
        // Keep the playhead on the same beat.
        self.position = same_beat(before, after, rate, self.position);
        // The take being captured goes on from the playhead. Its start stays:
        // its MIDI is stamped `start + written` and the shell takes `start`
        // off again, so moving it would drop or shift what came before.
        if let Some(take) = &mut self.take {
            take.next = same_beat(before, after, rate, take.next);
        }
        self.session
            .remap(|frame| same_beat(before, after, rate, frame));
        // Spec: transport R22. A jump waiting for its line keeps the line and
        // its target on their beats.
        if let Some(pending) = &mut self.pending_locate {
            pending.frame = same_beat(before, after, rate, pending.frame);
            pending.to = same_beat(before, after, rate, pending.to);
        }
        for entry in &mut self.scheduled {
            entry.frame = after.ticks_to_frames(entry.command.at, rate);
        }
        // shortcut: transport commands queued for a clock frame (`timed`) keep
        // their frames across a tempo change, as they did across a program
        // swap before scenes had tempos; rework them if a command sent for a
        // frame must follow its beat.
    }

    /// Makes `program`'s tempo map and time-signature map what the audio
    /// thread has put in place: the override's maps the compiler made for
    /// it, or else those of the scene that launched them if it still holds
    /// the same tempo and signature, else the set's. Switching among maps
    /// the program already holds allocates nothing.
    fn align_with_override(&mut self, program: &mut EngineProgram) {
        let held = |scene: SceneId| program.scene_maps.iter().find(|maps| maps.scene == scene);
        let tempo = self.scene_override.tempo.and_then(|(scene, bpm)| {
            let scene = held(scene).and_then(|maps| maps.tempo.as_ref());
            let (_, map) = (program.override_tempo.iter().chain(scene))
                .find(|(held_bpm, _)| *held_bpm == bpm)?;
            Some(map.clone())
        });
        let signature = self
            .scene_override
            .signature
            .and_then(|(scene, signature)| {
                let scene = held(scene).and_then(|maps| maps.signature.as_ref());
                let (_, map) = (program.override_signatures.iter().chain(scene))
                    .find(|(held_signature, _)| *held_signature == signature)?;
                Some(map.clone())
            });
        // A program that holds no map for the override (compiled before the
        // launch, from a scene changed since) gives it up.
        if tempo.is_none() {
            self.scene_override.tempo = None;
        }
        if signature.is_none() {
            self.scene_override.signature = None;
        }
        program.tempo = tempo.unwrap_or_else(|| program.base_tempo.clone());
        program.time_signatures = signature.unwrap_or_else(|| program.base_signatures.clone());
    }

    /// Launches `scene`'s own tempo and signature, on the frame it launches
    /// on: the maps change inside the running program, with no rebuild, and
    /// everything that holds a frame keeps its beat (WP-3.5, transport R15).
    fn apply_scene(&mut self, scene: SceneId) {
        let Some(program) = self.program.as_mut() else {
            return;
        };
        let program = program.get_mut();
        let Some(maps) = program.scene_maps.iter().find(|maps| maps.scene == scene) else {
            return;
        };
        let (tempo, signature) = (maps.tempo.clone(), maps.signature.clone());
        let before = program.tempo.clone();
        if let Some((bpm, map)) = tempo {
            program.tempo = map;
            self.scene_override.tempo = Some((scene, bpm));
        }
        if let Some((value, map)) = signature {
            program.time_signatures = map;
            self.scene_override.signature = Some((scene, value));
        }
        let after = program.tempo.clone();
        if before != after {
            self.move_to_tempo(&before, &after);
            self.update_loop_frames();
        }
    }

    /// Gives back the set's own tempo and signature, at the playhead: what
    /// Re-Enable Automation and Back to Arrangement do after a scene
    /// launched its own.
    fn clear_scene_override(&mut self) {
        // Spec: link R4. Re-Enable Automation also gives the tempo envelope
        // back after a peer's change took its place.
        self.tempo_overridden = false;
        if self.scene_override.is_none() {
            return;
        }
        self.scene_override = SceneOverride::default();
        let Some(program) = self.program.as_mut() else {
            return;
        };
        let program = program.get_mut();
        let before = program.tempo.clone();
        program.tempo = program.base_tempo.clone();
        program.time_signatures = program.base_signatures.clone();
        let after = program.tempo.clone();
        if before != after {
            self.move_to_tempo(&before, &after);
            self.update_loop_frames();
        }
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

    /// Runs a transport command that was waiting for its frame: the
    /// follower's own isn't held back by a start waiting for the follower
    /// (the transport spec, R25).
    fn run_timed(&mut self, command: TransportCommand, by_follower: bool) {
        let was = self.releasing;
        self.releasing |= by_follower;
        self.transport(command);
        self.releasing = was;
    }

    /// Whether `command` starts the song from where it stands: what waits
    /// for the follower's frame when the follower decides starts (R25).
    fn starts_song(&self, command: TransportCommand) -> bool {
        match command {
            TransportCommand::Play | TransportCommand::PlayFromInsertMarker => true,
            TransportCommand::Continue
            | TransportCommand::PlayStop
            | TransportCommand::Scrub { .. } => !self.playing,
            _ => false,
        }
    }

    /// Keeps a start for the follower's frame (R25). A second one the same
    /// as one already waiting changes nothing: the wait stays.
    fn hold(&mut self, held: Held) {
        if !self.held.contains(&held) && self.held.len() < MAX_HELD {
            self.held.push(held);
        }
    }

    /// Runs the starts that waited, in the order they came, on this frame.
    fn release_held(&mut self) {
        self.release = None;
        let was = std::mem::replace(&mut self.releasing, true);
        while !self.held.is_empty() {
            match self.held.remove(0) {
                Held::Transport(command) => self.transport(command),
                Held::Session(command) => self.session_command(command),
            }
        }
        self.releasing = was;
    }

    /// Releases the waiting starts if their frame has come by `offset`
    /// frames into the block.
    fn apply_due_release(&mut self, offset: usize) {
        if self.held.is_empty() {
            return;
        }
        if self
            .release
            .is_some_and(|frame| frame <= self.clock + offset as u64)
        {
            self.release_held();
        }
    }

    /// A Phase Nudge button pressed or released (R27). A press does nothing
    /// while the song is stopped, and, since the follower keeps the phase,
    /// while it leads (R29).
    fn phase_nudge(&mut self, direction: NudgeDirection, held: bool) {
        if held && (!self.playing || self.followed.is_some()) {
            return;
        }
        match direction {
            NudgeDirection::Down => self.nudge.down = held,
            NudgeDirection::Up => self.nudge.up = held,
        }
        if held {
            self.nudge.last = Some(direction);
        }
    }

    /// Runs a transport command now, at the playhead
    /// (the transport spec).
    fn transport(&mut self, command: TransportCommand) {
        if command == TransportCommand::Stop
            && !self.releasing
            && !self.playing
            && !self.held.is_empty()
        {
            // Spec: transport R25. Stop while a start waits cancels it.
            self.held.clear();
            self.release = None;
            return;
        }
        if self.deciding && !self.releasing && self.starts_song(command) {
            // Spec: transport R25. The follower decides when it starts.
            self.hold(Held::Transport(command));
            return;
        }
        match command {
            // Spec: transport R1, R11, R21
            TransportCommand::Play => self.play(),
            // Spec: transport R10, R21
            TransportCommand::PlayFromInsertMarker => self.play_from_marker(),
            // Spec: transport R3, R4
            TransportCommand::Stop => {
                // Spec: transport R25. A stop of the follower's own cancels
                // a start that waits too.
                self.held.clear();
                self.release = None;
                // Spec: transport R22. A jump waiting for a line is
                // abandoned when the transport stops.
                self.pending_locate = None;
                self.metronome.stop();
                // Spec: routing R11; WP-2.13. Stop ends the take, as Live's
                // Stop ends a recording.
                self.end_recordings();
                if self.playing {
                    self.playing = false;
                    self.release_midi();
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
                    // Spec: transport R22. A jump waiting for a line is
                    // abandoned when the transport stops.
                    self.pending_locate = None;
                    // Spec: transport R25. A stop cancels a held restart.
                    self.held.clear();
                    self.release = None;
                    self.playing = false;
                    self.release_midi();
                    self.metronome.stop();
                    self.end_recordings();
                } else {
                    self.play();
                }
            }
            // Spec: transport R7, R12
            TransportCommand::Locate(ticks) => {
                // Spec: transport R22. A jump waiting for a line is
                // replaced by this one.
                self.pending_locate = None;
                self.insert_marker = clamp_position(ticks);
                if self.playing {
                    self.position = self.frames_at(self.insert_marker);
                    self.relocate_session();
                    self.metronome.stop();
                    // Playback jumps away from the take; it ends here
                    // (WP-2.13).
                    self.end_recordings();
                }
            }
            // Spec: transport R22
            TransportCommand::QuantizedLocate(ticks) => {
                let target = clamp_position(ticks);
                self.insert_marker = target;
                if self.playing {
                    self.quantized_locate(target);
                }
            }
            // Spec: transport R23
            TransportCommand::Scrub { point, held } => self.scrub(point, held),
            // Spec: transport R9
            TransportCommand::InsertMarkerToPlayhead => {
                let ticks = self
                    .tempo()
                    .frames_to_ticks(self.position, self.config.sample_rate);
                self.insert_marker = clamp_position(ticks);
            }
            // Spec: control-bar-transport R15
            TransportCommand::SetInsertMarker(ticks) => {
                self.insert_marker = clamp_position(ticks);
            }
            TransportCommand::SetLoop(region) => {
                self.loop_region = region;
                self.update_loop_frames();
            }
            // Spec: transport R21
            TransportCommand::SetSongStart(start) => {
                self.song_start = start;
            }
            TransportCommand::SetMetronome(settings) => self.metronome.set_settings(settings),
            // Spec: grooves R21
            TransportCommand::SetGrooveAmount(tenths) => {
                self.groove_amount = f32::from(tenths) / 1000.0;
            }
            // Spec: scales R15
            TransportCommand::SetScale(scale) => self.scale = scale,
            // Spec: tunings R16
            TransportCommand::SetScalesHidden(hidden) => self.scales_hidden = hidden,
            TransportCommand::SetPunch(punch) => {
                self.punch = punch;
                self.update_loop_frames();
            }
            // Spec: midi-clock R11
            TransportCommand::ResyncExternalHardware => {
                let position = self
                    .tempo()
                    .frames_to_ticks(self.position, self.config.sample_rate);
                let signatures = match &self.program {
                    Some(p) => &p.get().time_signatures,
                    None => &self.fallback_time_signatures,
                };
                self.sync_out.resync(
                    position,
                    signatures,
                    self.playing && !self.metronome.is_counting_in(),
                );
            }
            TransportCommand::SetRecording(recording) => {
                self.metronome.set_recording(recording);
                if recording {
                    self.recording = true;
                } else {
                    self.end_recording();
                }
            }
            // Spec: transport R27, R29
            TransportCommand::PhaseNudge { direction, held } => self.phase_nudge(direction, held),
        }
    }

    /// Spec: external-devices R6. Ends the notes sent to ports, at the start
    /// of the next block: the transport stopped.
    fn release_midi(&mut self) {
        if let Some(program) = self.program.as_mut() {
            program.get_mut().midi.release_all();
        }
    }

    /// Ends every recording, as Stop does: the Arrangement's and a Session
    /// one, which ends where the playhead is rather than on the grid.
    fn end_recordings(&mut self) {
        self.end_session_take(false, self.position);
        self.end_recording();
    }

    /// Ends the Session recording at `at`, publishing the take for the
    /// shell. With `launch`, the clips it recorded play on as loops from
    /// there (WP-3.11).
    fn end_session_take(&mut self, launch: bool, at: Frames) {
        if self.session_take.take().is_none() {
            return;
        }
        if launch && let Some(plan) = self.plan.as_mut() {
            for target in plan.get_mut().session.iter().filter(|t| !t.overdub) {
                self.session.launch_at(target.track, target.scene, at);
            }
        }
        self.end_take();
        self.metronome.set_recording(self.recording);
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
                end: take.next,
                wrap: take.wrap.map(|region| (region.start, region.end)),
                frames: take.written,
            });
        }
        self.plan = None;
    }

    /// Starts playback: from the song's start time when it has one, and
    /// from the insert marker otherwise (the transport spec, R21).
    fn play(&mut self) {
        let start = self.song_start.unwrap_or(self.insert_marker);
        self.start_playing_at(start);
    }

    /// Starts playback from the insert marker, whatever the song's start
    /// time is (the transport spec, R10 and R21).
    fn play_from_marker(&mut self) {
        let start = self.insert_marker;
        self.start_playing_at(start);
    }

    fn start_playing_at(&mut self, start: Ticks) {
        // Spec: transport R22. A jump waiting for a line is abandoned when
        // the song starts again somewhere else.
        self.pending_locate = None;
        let (tempo, sigs) = match &self.program {
            Some(p) => (&p.get().tempo, &p.get().time_signatures),
            None => (&self.fallback_tempo, &self.fallback_time_signatures),
        };
        // Spec: metronome R10. No count-in plays while Link is on.
        if self.no_count_in {
            self.metronome.stop();
        } else {
            self.metronome
                .start_count_in(self.config.sample_rate, tempo, sigs, start);
        }
        self.position = self.frames_at(start);
        self.playing = true;
    }

    /// A jump that waits for the next line of the global launch quantisation
    /// (the transport spec, R22): the playhead goes on to that line and
    /// jumps to `target` on its exact frame. With the quantisation off or
    /// None, the jump happens at once.
    fn quantized_locate(&mut self, target: Ticks) {
        let rate = self.config.sample_rate;
        let playhead = self.tempo().frames_to_ticks(self.position, rate);
        let signatures = match &self.program {
            Some(program) => &program.get().time_signatures,
            None => &self.fallback_time_signatures,
        };
        let line = self.session.settings().next_line(playhead, signatures);
        let frame = self.frames_at(line);
        let to = self.frames_at(target);
        if frame <= self.reached() {
            self.position = to;
            self.relocate_session();
            self.metronome.stop();
            self.end_recordings();
        } else {
            self.pending_locate = Some(PendingLocate { frame, to });
        }
    }

    /// Moves what the Session View waits for to the first line after the
    /// playhead, which a locate has just moved (launch-quantisation R39).
    fn relocate_session(&mut self) {
        let playhead = Self::playhead(
            self.program.as_ref().map(Program::get),
            &self.fallback_tempo,
            &self.fallback_time_signatures,
            true,
            self.position,
            self.config.sample_rate,
        );
        self.session.requantise_pending(playhead);
    }

    /// A press in the scrub area, or its release (the transport spec, R23).
    /// A press loops the span of the global launch quantisation at `point`,
    /// jumping to its start as a quantised locate does and starting playback
    /// there if the song is stopped. A release ends the loop and leaves
    /// playback where it is; with none held, it plays from `point` as a click
    /// does.
    fn scrub(&mut self, point: Ticks, held: bool) {
        let mut target = clamp_position(point);
        if held {
            let signatures = match &self.program {
                Some(program) => &program.get().time_signatures,
                None => &self.fallback_time_signatures,
            };
            let (start, end) = self.session.settings().scrub_span(target, signatures);
            self.held_scrub = Some(LoopRegion { start, end });
            self.update_loop_frames();
            target = start;
        } else if self.held_scrub.take().is_some() {
            self.update_loop_frames();
            return;
        }
        self.insert_marker = target;
        if self.playing {
            self.quantized_locate(target);
        } else {
            self.start_playing_at(target);
        }
    }

    /// The loop the song plays now: a held click in the scrub area's, which
    /// the Arrangement loop gives way to, or else the Arrangement loop
    /// unless timecode leads.
    fn active_loop(&self) -> Option<LoopFrames> {
        // Spec: midi-timecode R9. Timecode that leads plays its own
        // position through the Arrangement loop's end.
        let arrangement = self
            .loop_frames
            .filter(|_| !self.midi.sync_in.state().timecode_leads);
        self.held_frames.or(arrangement)
    }

    /// Applies a jump waiting for the next line of the global launch
    /// quantisation, once the playhead reaches that line (the transport
    /// spec, R22).
    fn apply_due_locate(&mut self) {
        if let Some(pending) = self.pending_locate
            && self.playing
            && self.reached() >= pending.frame
        {
            self.pending_locate = None;
            self.position = pending.to;
            self.relocate_session();
            self.metronome.stop();
            self.end_recordings();
        }
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
        let frames = |region: LoopRegion| {
            let start = self.frames_at(region.start.max(Ticks::ZERO));
            let end = self.frames_at(region.end);
            (end > start).then_some(LoopFrames {
                start,
                end,
                ticks: region,
            })
        };
        let (loop_frames, held_frames) = (
            self.loop_region.and_then(frames),
            self.held_scrub.and_then(frames),
        );
        self.loop_frames = loop_frames;
        self.held_frames = held_frames;
    }

    /// Holds a transport command for a frame of the engine clock, or runs it
    /// now if that frame has come.
    fn queue_transport(&mut self, command: TransportCommand, frame: u64, by_follower: bool) {
        if frame <= self.clock {
            self.run_timed(command, by_follower);
        } else if self.timed.len() < self.timed.capacity() {
            self.timed.push(Timed {
                frame,
                command,
                by_follower,
            });
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
                self.run_timed(due.command, due.by_follower);
            } else {
                i += 1;
            }
        }
    }

    /// Runs a Session View command (WP-3.2): a launch, a stop, or the
    /// global launch quantisation.
    fn session_command(&mut self, command: SessionCommand) {
        if self.deciding && !self.releasing && !self.playing && self.session_starts_song(command) {
            // Spec: transport R25. A launch that starts the song waits for
            // the follower's frame.
            self.hold(Held::Session(command));
            return;
        }
        match command {
            SessionCommand::Launch { track, scene } => {
                self.launch_button(track, scene, Played::PLAIN, false);
            }
            SessionCommand::LaunchLegato { track, scene } => {
                self.launch_button(track, scene, Played::PLAIN, true);
            }
            SessionCommand::LaunchAt {
                track,
                scene,
                velocity,
                transpose,
            } => self.launch_button(
                track,
                scene,
                Played {
                    velocity,
                    transpose,
                },
                false,
            ),
            SessionCommand::Release { track, scene } => self.release_slot(track, scene),
            SessionCommand::Nudge {
                track,
                scene,
                steps,
            } => self.nudge_slot(track, scene, steps),
            SessionCommand::LaunchScene(scene) => self.launch_scene(scene),
            SessionCommand::CancelSceneLaunch(scene) => {
                let slots = self
                    .program
                    .as_ref()
                    .map_or(&[][..], |program| program.get().scene_slots(scene));
                self.session.cancel_scene_launch(scene, slots);
            }
            SessionCommand::StopTrack(track) => self.stop_session(Some(track)),
            SessionCommand::StopAll => self.stop_session(None),
            SessionCommand::BackToArrangement => {
                self.session.back_to_arrangement(None, self.position);
                // Spec: session-view R23. Returning every track also gives
                // the set's own tempo and signature back.
                self.clear_scene_override();
            }
            SessionCommand::TrackBackToArrangement(track) => {
                self.session.back_to_arrangement(Some(track), self.position);
            }
            SessionCommand::SetQuantization(settings) => self.session.set_settings(settings),
            SessionCommand::Record => self.record_session(None),
            SessionCommand::RecordFixed {
                length,
                phrase_sync,
            } => self.record_session((length.0 > 0).then_some((length, phrase_sync))),
            SessionCommand::StopRecording => self.stop_session_take(true),
            SessionCommand::MoveToScene(scene) => {
                let slots = self
                    .program
                    .as_ref()
                    .map_or(&[][..], |program| program.get().scene_slots(scene));
                self.session.move_to_scene(scene, slots);
            }
            SessionCommand::ClearSceneOverride => self.clear_scene_override(),
        }
    }

    /// Whether `command`, with the song stopped, starts it: a launch of a
    /// clip, or a Session recording (the launch quantisation spec, R5).
    fn session_starts_song(&self, command: SessionCommand) -> bool {
        match command {
            SessionCommand::Launch { .. }
            | SessionCommand::LaunchLegato { .. }
            | SessionCommand::Record => true,
            SessionCommand::LaunchScene(scene) => self
                .program
                .as_ref()
                .is_some_and(|program| !program.get().scene_slots(scene).is_empty()),
            _ => false,
        }
    }

    /// Starts the Session recording the plan describes (WP-3.11): on the
    /// next grid line while the song plays, at once from the playhead while
    /// it's stopped, when the transport starts too, after any count-in. A
    /// clip playing on a track that records a new clip stops where the
    /// recording starts.
    fn record_session(&mut self, fixed: Option<(Ticks, bool)>) {
        let targets = self
            .plan
            .as_mut()
            .is_some_and(|plan| !plan.get_mut().session.is_empty());
        if !targets || self.program.is_none() || self.session_take.is_some() || self.recording {
            return;
        }
        let was_playing = self.playing;
        self.metronome.set_recording(true);
        if !was_playing {
            self.play_from_marker();
        }
        let playhead = Self::playhead(
            self.program.as_ref().map(Program::get),
            &self.fallback_tempo,
            &self.fallback_time_signatures,
            was_playing,
            self.position,
            self.config.sample_rate,
        );
        let (start, stop) = match fixed {
            // Spec: push-2-notes R51. A fixed-length take ends on its own,
            // and its clip plays on.
            Some((length, phrase_sync)) => {
                let (start, stop) = self.session.fixed_take(playhead, length, phrase_sync);
                (start, Some(stop))
            }
            None => (self.session.frame_for(playhead), None),
        };
        if let Some(plan) = self.plan.as_mut() {
            for target in plan.get_mut().session.iter().filter(|t| !t.overdub) {
                self.session.stop(Some(target.track), &[], playhead);
            }
        }
        self.session_take = Some(SessionTake {
            start,
            stop,
            launch: stop.is_some(),
            last: None,
        });
    }

    /// Asks the Session recording to end on the next grid line, keeping the
    /// earlier of two asks (WP-3.11).
    fn stop_session_take(&mut self, launch: bool) {
        if self.session_take.is_none() {
            return;
        }
        let playhead = Self::playhead(
            self.program.as_ref().map(Program::get),
            &self.fallback_tempo,
            &self.fallback_time_signatures,
            self.playing,
            self.position,
            self.config.sample_rate,
        );
        let mut at = self.session.frame_for(playhead);
        if let Some(region) = self.active_loop()
            && self.position < region.end
            && at > region.end
        {
            at = region.end;
        }
        if let Some(take) = self.session_take.as_mut() {
            take.stop = Some(take.stop.map_or(at, |stop| stop.min(at)));
            take.launch = launch;
        }
    }

    /// Whether a Session recording goes into the slot.
    fn is_recording_slot(&mut self, track: TrackId, scene: SceneId) -> bool {
        self.session_take.is_some()
            && self.plan.as_mut().is_some_and(|plan| {
                plan.get_mut()
                    .session
                    .iter()
                    .any(|t| t.track == track && t.scene == scene)
            })
    }

    /// Presses a slot's launch button as `played` says.
    fn launch_button(&mut self, track: TrackId, scene: SceneId, played: Played, legato: bool) {
        if self.is_recording_slot(track, scene) {
            // Spec: session-recording R4. The launch button of a recording
            // slot ends the recording and loops the clip.
            self.stop_session_take(true);
        } else {
            self.launch_slot(track, scene, played, legato);
        }
    }

    /// Launches a slot's clip at a velocity and transposition (remote-mapping
    /// R16, R35): on the next grid line while the song plays, and at once from
    /// the Arrangement position while it's stopped, which also starts the
    /// transport (the launch quantisation spec, R5).
    fn launch_slot(&mut self, track: TrackId, scene: SceneId, played: Played, legato: bool) {
        let was_playing = self.playing;
        if !was_playing {
            // Spec: launch-quantisation R5. A launch with the transport
            // stopped starts it at once, from the Arrangement position, and
            // the clip starts with it.
            self.play_from_marker();
        }
        let playhead = Self::playhead(
            self.program.as_ref().map(Program::get),
            &self.fallback_tempo,
            &self.fallback_time_signatures,
            was_playing,
            self.position,
            self.config.sample_rate,
        );
        let mut button = self.slot_button(track, scene);
        if legato {
            // Spec: push-2-notes R46. Legato at once, whatever the clip says.
            button.legato = true;
            button.quantization = Some(bl_model::LaunchQuantization::None);
        }
        self.session
            .press_with(track, scene, button, played, playhead);
    }

    /// Releases a slot's launch button: a Gate or Repeat clip stops on the
    /// grid (the launch quantisation spec, R18 and R21).
    fn release_slot(&mut self, track: TrackId, scene: SceneId) {
        let button = self.slot_button(track, scene);
        let playhead = Self::playhead(
            self.program.as_ref().map(Program::get),
            &self.fallback_tempo,
            &self.fallback_time_signatures,
            self.playing,
            self.position,
            self.config.sample_rate,
        );
        self.session.release(track, scene, button, playhead);
    }

    /// Nudges the clip in a slot at once, on the frame this block part
    /// starts at (the launch quantisation spec, R30).
    fn nudge_slot(&mut self, track: TrackId, scene: SceneId, steps: i32) {
        let looping = self
            .program
            .as_ref()
            .and_then(|program| program.get().slot_row(track, scene))
            .and_then(|row| row.looping);
        let playhead = Self::playhead(
            self.program.as_ref().map(Program::get),
            &self.fallback_tempo,
            &self.fallback_time_signatures,
            self.playing,
            self.position,
            self.config.sample_rate,
        );
        self.session.nudge(track, scene, steps, looping, playhead);
    }

    /// What a press or a release of a slot's launch button reads from its
    /// clip: its mode, Legato and own quantisation; a Trigger on the global
    /// grid for a slot with no clip (R22, R25).
    fn slot_button(&self, track: TrackId, scene: SceneId) -> Button {
        self.program
            .as_ref()
            .map_or_else(Button::default, |program| {
                program.get().launch_button(track, scene)
            })
    }

    /// Launches a scene: its clips, all on one grid line, and the tempo and
    /// signature it holds, on that same frame. A scene with nothing in it
    /// doesn't start the transport (observed in Live 12.4.3), and its tempo
    /// applies even then, with the song stopped (observed on 15 October
    /// 2026).
    fn launch_scene(&mut self, scene: SceneId) {
        let program = self.program.as_ref().map(Program::get);
        let holds_a_clip = program.is_some_and(|program| !program.scene_slots(scene).is_empty());
        let holds_maps =
            program.is_some_and(|program| program.scene_maps.iter().any(|m| m.scene == scene));
        if !holds_a_clip && !holds_maps {
            return;
        }
        let was_playing = self.playing;
        if holds_a_clip && !was_playing {
            self.play_from_marker();
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
        let at = if holds_a_clip {
            self.session.launch_scene(scene, slots, playhead)
        } else {
            self.session.landing(playhead)
        };
        if !holds_maps {
            return;
        }
        if at <= self.reached() {
            self.apply_scene(scene);
        } else {
            self.session.queue_scene(scene, at);
        }
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
        let tracks = self
            .program
            .as_ref()
            .map_or(&[][..], |program| program.get().session_tracks.as_slice());
        self.session.stop(track, tracks, playhead);
        let recorded = self.plan.as_mut().is_some_and(|plan| {
            plan.get_mut()
                .session
                .iter()
                .any(|t| track.is_none_or(|only| t.track == only))
        });
        if recorded {
            // Spec: session-recording R5. A clip stop ends the recording on
            // the grid without looping the clip.
            self.stop_session_take(false);
        }
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
        // Spec: metronome R5. What is due at the start frame waits for the
        // count-in to end.
        if self.playing && !self.metronome.is_counting_in() {
            let reached = self.reached();
            self.session.apply_due(reached);
            // Spec: launch-quantisation R21
            let playhead = Self::playhead(
                self.program.as_ref().map(Program::get),
                &self.fallback_tempo,
                &self.fallback_time_signatures,
                true,
                self.position,
                self.config.sample_rate,
            );
            self.session.repeat(playhead);
            // Spec: follow-actions R2. A follow due at this frame runs
            // before the scene maps and the clips below read the launcher.
            if let Some(program) = self.program.as_ref() {
                self.session.run_follow(program.get(), playhead, reached);
            }
            // Spec: session-view R22. A launched scene's tempo and signature
            // apply on its launch frame.
            while let Some(scene) = self.session.take_due_scene(reached) {
                self.apply_scene(scene);
            }
        }
        self.flush_launches();
    }

    /// Hands the starts and ends the launcher logged to the Arrangement
    /// take's ring, or drops them when no such take runs (WP-3.13).
    fn flush_launches(&mut self) {
        if self.take.is_some()
            && self.session_take.is_none()
            && let Some(log) = self
                .plan
                .as_mut()
                .and_then(|plan| plan.get_mut().launches.as_mut())
        {
            for event in self.session.log.drain(..) {
                log.push(event);
            }
        }
        self.session.log.clear();
    }

    /// A take that begins with Session clips playing logs a start for each,
    /// at the take's first frame (WP-3.13).
    fn log_playing_clips(&mut self) {
        let at = self.position;
        let Some(log) = self
            .plan
            .as_mut()
            .and_then(|plan| plan.get_mut().launches.as_mut())
        else {
            return;
        };
        for (track, scene, launched, offset) in self.session.started() {
            log.push(SessionEvent {
                track,
                scene,
                at,
                launched: Some(launched),
                offset,
            });
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
        // A part starts on the first song frame at or past the one a command
        // is due on; at a speed over 1 that's up to a speed's worth past it
        // (the transport spec, R24).
        let slack = Frames(self.speed_estimate().ceil().max(1.0) as i64);
        let reached = self.reached();
        while self.playing && !self.metronome.is_counting_in() && i < self.scheduled.len() {
            let due = self.scheduled[i].frame;
            if due <= reached && due + slack > reached {
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

    /// Asks the follower, once per block, for the tempo to play it at, the
    /// jumps, starts and stops it decides, and whether it decides when a
    /// start happens (the transport spec, R24, R25). Without one, nothing is
    /// followed, and any start that waited goes ahead.
    fn ask_follower(&mut self, frames: usize) {
        if self.asked == Some(self.clock) {
            return;
        }
        self.asked = Some(self.clock);
        self.followed = None;
        self.deciding = false;
        // The ticks of where the song is now, not where the last part ended:
        // a wrap or a locate moves the playhead after it.
        let exact = self.exact_at_position().ticks;
        let rate = self.config.sample_rate.as_f64();
        let ticks = match exact {
            Some(ticks) if self.playing => ticks,
            _ => {
                let tempo = self
                    .program
                    .as_ref()
                    .map_or(&self.fallback_tempo, |p| &p.get().tempo);
                tempo.seconds_to_ticks_exact(self.position.get() as f64 / rate)
            }
        };
        let (tempo, varies, quantum, phase, start_phase) = self.quantum_view(ticks);
        let mut ask = Ask {
            clock: self.clock,
            frames,
            playing: self.playing,
            position: ticks,
            rate,
            tempo,
            varies,
            overridden: self.tempo_overridden,
            leader: None,
            quantum,
            phase,
            start_phase,
            waiting: !self.held.is_empty(),
            latency: self.output_latency,
        };
        self.follow.clear();
        // Spec: midi-clock R14 to R24. An incoming clock is a follower of
        // its own; it counts what arrives for the lamp whether EXT is on or
        // not. It answers first, so a Link session sees the tempo it leads
        // with (link R10).
        let looped = self
            .loop_region
            .map(|region| (region.start.get() as f64, region.end.get() as f64));
        let tempo = self
            .program
            .as_ref()
            .map_or(&self.fallback_tempo, |p| &p.get().tempo);
        self.midi.follow_clock(
            &ask,
            self.output_latency,
            self.ext,
            looped,
            tempo,
            &mut self.follow,
        );
        ask.leader = self.follow.followed();
        // Spec: cv-tools R24, R25. A CV Clock In leads when no MIDI clock
        // does, and a Link session sees the tempo it leads with.
        let others = if ask.leader.is_some() {
            crate::cv_clock::Others::Midi
        } else {
            crate::cv_clock::Others::Nobody
        };
        self.cv_leader = self.program.as_mut().and_then(|program| {
            program
                .get_mut()
                .follow_cv_clocks(&ask, looped, others, &mut self.follow)
        });
        ask.leader = self.follow.followed();
        // Spec: tempo-follower R5, R20. Tempo Follower leads ahead of a Link
        // session, which then shares the tempo it leads with (link R26).
        if let Some(leader) = self.leader.as_mut() {
            leader.get_mut().block(&ask, &mut self.follow);
            ask.leader = self.follow.followed();
        }
        if let Some(follower) = self.follower.as_mut() {
            follower.get_mut().block(&ask, &mut self.follow);
        }
        self.no_count_in = self.follow.counts_in_off();
        self.moment = self.follow.heard();
        if self.follow.overrides_tempo() && !self.tempo_overridden {
            self.tempo_overridden = true;
        }
        self.followed = self.follow.followed();
        self.deciding = self.follow.deciding();
        if self.deciding && !self.held.is_empty() && self.follow.release().is_some() {
            self.release = self.follow.release();
        }
        for i in 0..self.follow.commands().len() {
            let (frame, command) = self.follow.commands()[i];
            self.queue_transport(command, frame, true);
        }
        // A follower that stops deciding, or goes, lets what waited start.
        if !self.deciding && !self.held.is_empty() {
            self.release_held();
        }
    }

    /// Fills the block's Link Audio inputs with what the peers' channels
    /// hold for the frames it plays on (the Link Audio spec, R5, R6): silence
    /// while Link Audio is off or no channel is there. Real-time safe.
    fn read_link_audio(&mut self, frames: usize) {
        let Some(program) = self.program.as_mut() else {
            return;
        };
        let link = match (self.link_audio.as_mut(), self.moment) {
            (Some(link), Some(moment)) => {
                let link: &dyn LinkAudio = &**link.get_mut();
                link.active().then_some((link, moment))
            }
            _ => None,
        };
        program
            .get_mut()
            .read_link_audio(link, frames, self.config.sample_rate.hz());
    }

    /// Sends the block's offered tracks to the peers (the Link Audio spec,
    /// R4). Real-time safe.
    fn send_link_audio(&mut self, frames: usize) {
        if let (Some(link), Some(moment)) = (self.link_audio.as_mut(), self.moment)
            && link.get_mut().active()
        {
            link.get_mut()
                .send(moment, frames, self.config.sample_rate.hz());
        }
    }

    /// What a follower is told of the song's tempo and launch quantisation
    /// at `ticks`: the map's tempo there, whether the map holds more than
    /// one tempo, the quantum in quarter notes, how far into it `ticks` is,
    /// and how far into it the position a start would play from is (the
    /// Link spec, R5 and R7).
    fn quantum_view(&self, ticks: f64) -> (f64, bool, f64, f64, f64) {
        let (tempo, signatures) = match &self.program {
            Some(program) => (&program.get().tempo, &program.get().time_signatures),
            None => (&self.fallback_tempo, &self.fallback_time_signatures),
        };
        let settings = self.session.settings();
        let at = Ticks(ticks.floor() as i64);
        let (unit, into) = settings.quantum_phase(at, signatures);
        let quarter = bl_core::time::TICKS_PER_QUARTER as f64;
        let phase = into.as_quarters() + (ticks - ticks.floor()) / quarter;
        let (_, start_into) = settings.quantum_phase(self.start_ticks(), signatures);
        let mut segments = tempo.segments();
        let varies = segments.len() > 1
            || segments
                .next()
                .is_some_and(|segment| segment.start_bpm() != segment.end_bpm());
        (
            tempo.bpm_at(at).get(),
            varies,
            unit.as_quarters(),
            phase,
            start_into.as_quarters(),
        )
    }

    /// Where a start that waits, or one a peer asks for, would play from:
    /// the song start time or the insert marker, as the first start that
    /// waits does, else the insert marker (the Link spec, R7 and R8).
    fn start_ticks(&self) -> Ticks {
        match self.held.first() {
            Some(Held::Transport(TransportCommand::Play)) => {
                self.song_start.unwrap_or(self.insert_marker)
            }
            Some(Held::Transport(TransportCommand::Continue)) => self
                .tempo()
                .frames_to_ticks(self.position, self.config.sample_rate),
            _ => self.insert_marker,
        }
    }

    /// How many song frames a frame of the block moves the song now, as the
    /// block's blocks are planned: the followed tempo against the map's,
    /// times the nudge.
    fn speed_estimate(&self) -> f64 {
        let factor = self.nudge.factor();
        let Some(bpm) = self.followed else {
            return factor;
        };
        let tempo = self.tempo();
        let ticks = tempo.frames_to_ticks(self.position, self.config.sample_rate);
        bpm * factor / tempo.bpm_at(ticks).get()
    }

    /// The last song frame that has come due by now: a command, a launch or
    /// a loop's end placed on a song frame acts when the part that starts now
    /// is the one it is nearest (the transport spec, R24). That is the
    /// playhead while the song moves a frame per frame, and, while it moves
    /// faster or slower, the song frames the part's first frame is nearer
    /// than the second.
    fn reached(&self) -> Frames {
        let speed = self.speed_estimate();
        let phase = self.exact_phase();
        if speed == 1.0 && phase == 0.0 {
            return self.position;
        }
        let here = self.position.get() as f64 + phase;
        Frames((here + speed / 2.0 - crate::node::TIE).ceil() as i64 - 1)
    }

    /// Works out how the song moves over a part of at most `length` frames
    /// that starts now (the transport spec, R24, R27): the part's speed
    /// and where the song stands exactly. At a speed of one, nothing is
    /// worked out. A followed tempo advances the song's ticks in a straight
    /// line, so where the tempo map bends the song's frames against them
    /// (a ramp, a change of tempo), the part is cut short until the bend
    /// within it is a fraction of a frame, and every node can read the
    /// part's frames at one speed.
    fn pace(&mut self, length: usize) -> Part {
        let mut info = TransportInfo::at(self.playing, self.position);
        let steady = Part {
            length,
            transport: info,
            end: None,
        };
        if !self.playing || self.metronome.is_counting_in() {
            self.exact = None;
            return steady;
        }
        let factor = self.nudge.factor();
        let Some(bpm) = self.followed else {
            if factor == 1.0 {
                self.exact = None;
                return steady;
            }
            let phase = self.exact_phase();
            info.speed = factor;
            info.phase = phase;
            return self.part(length, info, None);
        };
        let rate = self.config.sample_rate.as_f64();
        let start = self.exact_at_position();
        let pos0 = self.position.get() as f64 + start.phase;
        let tempo = self.tempo();
        let ticks0 = start
            .ticks
            .unwrap_or_else(|| tempo.seconds_to_ticks_exact(pos0 / rate));
        let per_frame = bpm * factor * TICKS_PER_QUARTER as f64 / (60.0 * rate);
        let frame_of = |ticks: f64| tempo.ticks_to_seconds_exact(ticks) * rate;
        let bent = tempo.segments().len() > 1;
        let mut length = length;
        let (ticks1, pos1) = loop {
            let ticks1 = ticks0 + length as f64 * per_frame;
            let pos1 = frame_of(ticks1);
            if !bent || length == 1 {
                break (ticks1, pos1);
            }
            let half = length / 2;
            let linear = pos0 + (pos1 - pos0) * half as f64 / length as f64;
            if (frame_of(ticks0 + half as f64 * per_frame) - linear).abs() <= BEND_FRAMES {
                break (ticks1, pos1);
            }
            length = half;
        };
        info.speed = (pos1 - pos0) / length as f64;
        info.phase = start.phase;
        self.part(length, info, Some(ticks1))
    }

    /// Where the song exactly stands: what the last part left, if the
    /// playhead is still where it left it, else the playhead itself.
    fn exact_at_position(&self) -> Exact {
        match self.exact {
            Some(exact) if exact.position == self.position => exact,
            _ => Exact {
                position: self.position,
                phase: 0.0,
                ticks: None,
            },
        }
    }

    fn exact_phase(&self) -> f64 {
        self.exact_at_position().phase
    }

    /// The part `info` describes: where the song is when it ends, the first
    /// song frame past it, and the song frame no part may play past while a
    /// boundary lies ahead (the loop's end, a jump's line).
    fn part(&mut self, length: usize, mut info: TransportInfo, ticks: Option<f64>) -> Part {
        info.until = self.next_boundary().unwrap_or(i64::MAX);
        let end = info.window_end(length);
        let exact = info.exact(length);
        Part {
            length,
            transport: info,
            end: Some(Exact {
                position: Frames(end),
                phase: exact - end as f64,
                ticks,
            }),
        }
    }

    /// The nearest song frame ahead that the transport acts on: no part
    /// plays past it. Playing a part up to the frame before it takes the
    /// part's song frames to it exactly, whatever the speed.
    fn next_boundary(&self) -> Option<i64> {
        let mut nearest: Option<i64> = None;
        self.song_boundaries(|frame| {
            if frame > self.position {
                let at = frame.get();
                nearest = Some(nearest.map_or(at, |n| n.min(at)));
            }
        });
        nearest
    }

    /// Calls `each` with every song frame the transport acts on: a scheduled
    /// command, a Session launch or stop, a Session take's ends, the loop's
    /// end, a jump waiting for a line, and the punch points.
    fn song_boundaries(&self, mut each: impl FnMut(Frames)) {
        for entry in &self.scheduled {
            each(entry.frame);
        }
        // Spec: launch-quantisation R2. A launch or a stop lands on its
        // grid frame, so the block splits there.
        if let Some(due) = self.session.next_due(self.reached()) {
            each(due);
        }
        // Spec: session-recording R3. A Session take starts and ends on its
        // grid frames.
        if let Some(take) = self.session_take {
            each(take.start);
            if let Some(stop) = take.stop {
                each(stop);
            }
        }
        // Spec: transport R13
        if let Some(region) = self.active_loop() {
            each(region.end);
        }
        // Spec: transport R22. A jump waiting for the next line of the
        // global launch quantisation lands on that line's frame.
        if let Some(pending) = self.pending_locate {
            each(pending.frame);
        }
        // Spec: recording R6. A part of a block lies wholly inside the punch
        // points or wholly outside them, so the take starts and ends on the
        // frame.
        if self.recording {
            for frame in [self.punch_frames.start, self.punch_frames.end]
                .into_iter()
                .flatten()
            {
                each(frame);
            }
        }
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
            .map(|entry| entry.frame.saturating_sub(now))
            .chain(
                self.release
                    .filter(|_| !self.held.is_empty())
                    .map(|frame| frame.saturating_sub(now)),
            );
        let mut next = timed
            .filter(|&ahead| ahead > 0 && ahead < remaining as u64)
            .min()
            .map_or(remaining, |ahead| ahead as usize);
        // Spec: modulation R28. A target that runs before the modulator
        // driven by its signal takes the modulator's output from 32 frames
        // before, which the modulator has written by the part's start only
        // if the part is no longer than that.
        if self.program.as_ref().is_some_and(|p| p.get().lagged) {
            next = next.min(crate::program::LAG_FRAMES);
        }
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
        // Distances to the transport's song frames are in song frames; a
        // frame of the block moves the song by the speed (R24).
        let speed = self.speed_estimate();
        let phase = self.exact_phase();
        let position = self.position.get();
        self.song_boundaries(|frame| {
            if frame.get() > position {
                let frames = frames_until(position, phase, speed, frame.get());
                next = next.min(frames);
            }
        });
        next
    }

    /// Runs the program over `length` frames from `offset` in the block, and
    /// moves the playhead on if the song is playing. Playback that reaches
    /// the loop's end goes on from its start: the parts of a block end
    /// there, so the next part starts at the loop's start.
    fn render(&mut self, offset: usize, length: usize, part: Part) {
        self.write_sync(offset, length, part.transport);
        let stride = self.config.max_frames;
        let transport = part.transport;
        // The song frame this part starts at, for a real-time render's
        // range (WP-5.35).
        let song = self.position;
        // Every device channel, so a count-in or a part that plays fewer
        // of them leaves no earlier block's Ext. Out behind.
        for c in 0..self.config.output_channels {
            let to = c * stride + offset;
            self.ext_output[to..to + length].fill(0.0);
        }
        // Spec: metronome R5. The song waits at the insert marker through a
        // count-in, so the program runs as a stopped transport would: monitored
        // input, audio inputs and effect tails play on.
        let counting_in = self.metronome.is_counting_in();
        let run_transport = if counting_in {
            TransportInfo::at(false, self.position)
        } else {
            transport
        };

        match self.program.as_mut() {
            Some(program) => {
                let program = program.get_mut();
                self.counters.events_applied += program.run(
                    self.pool.as_mut(),
                    &mut self.lane,
                    Blk {
                        frames: length,
                        from: offset as u32,
                        until: offset as u32 + length as u32,
                        events: &self.resolved,
                        transport: run_transport,
                        session: &self.session_playback,
                        groove_amount: self.groove_amount,
                        set_scale: self.scale,
                        scales_hidden: self.scales_hidden,
                        scale: ScaleRuns::of(self.scale),
                    },
                );
                // Spec: link-audio R4. The tracks offered to the peers, as
                // they leave their mixers.
                if self.moment.is_some()
                    && let Some(link) = self.link_audio.as_mut()
                    && link.get_mut().active()
                {
                    program.stage_link_audio(&mut **link.get_mut(), offset, length);
                }
                // Spec: external-devices R4, R5
                self.midi.emit(&mut program.midi, self.output_latency);
                for c in 0..self.config.channels {
                    let to = c * stride + offset;
                    let from = c.min(program.channels - 1);
                    self.output[to..to + length].copy_from_slice(program.output(from, length));
                }
                // Spec: routing R3. A channel the device lacks plays nothing.
                self.ext_width = program.ext_width().min(self.config.output_channels);
                program.mix_external(&mut self.ext_output, self.ext_width, stride, offset, length);
                // Spec: external-devices R18. An external effect sends its
                // signal to the interface's outputs, where it adds to
                // whatever else plays there.
                program.mix_external_effects(
                    &mut self.ext_output,
                    self.ext_width,
                    stride,
                    offset,
                    length,
                );
                // Spec: cv-tools R3. A CV output adds to its channel at full
                // value.
                program.mix_cv(&mut self.ext_output, self.ext_width, stride, offset, length);
            }
            None => {
                self.ext_width = 0;
                for c in 0..self.config.channels {
                    let to = c * stride + offset;
                    self.output[to..to + length].fill(0.0);
                }
            }
        }
        if counting_in {
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
        if self.playing {
            let (tempo, sigs) = match &self.program {
                Some(p) => (&p.get().tempo, &p.get().time_signatures),
                None => (&self.fallback_tempo, &self.fallback_time_signatures),
            };
            // Spec: metronome R7. With Punch-In on, "only while recording"
            // clicks start at the punch-in point, not at play start.
            self.metronome.set_recording(
                (self.recording
                    && self
                        .punch_frames
                        .start
                        .is_none_or(|at| self.reached() >= at))
                    || self.session_take.is_some(),
            );
            self.metronome.render_active(
                Block {
                    output: &mut self.output,
                    stride,
                    channels: self.config.channels,
                    offset,
                    length,
                },
                transport,
                self.config.sample_rate,
                tempo,
                sigs,
            );
            let before = self.position;
            self.position = Frames(transport.window_end(length));
            self.exact = part.end;
            // Spec: transport R13, R14
            if let Some(region) = self.active_loop()
                && before < region.end
                && self.position >= region.end
            {
                debug_assert_eq!(
                    self.position, region.end,
                    "a block part crossed the loop's end"
                );
                self.position = region.start;
                // What waited for a line at or past the loop's end lands as
                // the playhead reaches that line round the loop.
                let back = region.start - region.end;
                self.session.remap_pending(|frame| frame + back);
                self.session
                    .wrap_lines(region.ticks.end, region.ticks.start - region.ticks.end);
                // Spec: transport R22. A jump waiting for the loop's end
                // (or a line past it) lands on the wrap, as a launch does.
                if let Some(pending) = &mut self.pending_locate
                    && pending.frame >= region.end
                {
                    pending.frame += back;
                }
            }
        }
        // Spec: external-devices R24. A real-time render captures the
        // outputs it renders while the song plays the range, and the
        // transport stops at the range's end.
        // shortcut: the range is counted in frames a song frame each, which
        // holds unless the song follows an outside tempo or is nudged while
        // it renders; upgrade by counting the part's song frames.
        if self.rendering.is_some() {
            self.capture_render(song, length);
        }
    }

    /// Writes the MIDI clock and transport messages for the part of the
    /// block that starts at `offset` and runs `length` frames (midi-clock R3
    /// to R8, R11).
    fn write_sync(&mut self, offset: usize, length: usize, info: TransportInfo) {
        let (tempo, signatures) = match &self.program {
            Some(p) => (&p.get().tempo, &p.get().time_signatures),
            None => (&self.fallback_tempo, &self.fallback_time_signatures),
        };
        let counting_in = self.metronome.is_counting_in();
        let loop_end = self
            .held_frames
            .or(self.loop_frames)
            .filter(|region| self.position < region.end)
            .map(|region| region.end);
        self.sync_out.part(
            &SyncPart {
                offset: offset as u32,
                length: length as u32,
                output_latency: self.output_latency,
                playing: self.playing && !counting_in,
                counting_in,
                position: self.position,
                info,
                insert_marker: self.insert_marker,
                loop_end,
                beats: Beats {
                    tempo,
                    rate: self.config.sample_rate,
                },
                signatures,
            },
            &mut self.midi,
        );
    }

    /// Copies the part of the block the real-time render's range covers
    /// into its rings, and stops the song at the range's end. Real-time
    /// safe: copies into rings made when the engine was, no locks.
    fn capture_render(&mut self, position: Frames, frames: usize) {
        if !self.playing || frames == 0 {
            return;
        }
        let mut plan = match self.rendering.take() {
            Some(plan) => plan,
            None => return,
        };
        if let Some(program) = self.program.as_mut() {
            let program = program.get_mut();
            // A ring too full for the part counts the frames it dropped
            // (R25); the shell sees the count and restarts the render.
            let _ = crate::render::capture(
                plan.get_mut(),
                program,
                position,
                frames,
                self.config.channels,
            );
        }
        let done = {
            let plan = plan.get_mut();
            let frames = plan.frames;
            plan.outputs
                .iter()
                .all(|output| output.captured.load(Relaxed) >= frames)
        };
        if done {
            // The range is rendered: the song stops at its end and stays
            // stopped.
            self.playing = false;
            self.metronome.stop();
            self.position = plan.get_mut().end();
        }
        self.rendering = Some(plan);
    }

    /// Copies this part of the block's device input into the recording
    /// rings, when the transport records (WP-2.13). The take starts at the
    /// first played part, so its first sample is the frame playback started
    /// on; a part the rings can't take in full, or playback jumping away
    /// from the take's next frame, ends the take before the part, so every
    /// file holds exactly the take's frames. Real-time safe: copies into
    /// rings made when the engine was, no locks.
    fn capture(&mut self, offset: usize, frames: usize, transport: TransportInfo) {
        if self.session_take.is_some() {
            self.capture_session(offset, frames, transport);
            return;
        }
        if !self.recording || !self.playing || self.metronome.is_counting_in() || frames == 0 {
            return;
        }
        if self.plan.is_none() || self.program.is_none() {
            return;
        }
        // Spec: recording R9. Nothing is written before the punch-in point,
        // and the take ends at the punch-out point; the transport carries on.
        if self
            .punch_frames
            .start
            .is_some_and(|at| self.reached() < at)
        {
            return;
        }
        if self.punch_frames.end.is_some_and(|at| self.reached() >= at) {
            self.end_take();
            return;
        }
        if let Some(take) = &mut self.take
            && take.next != self.position
        {
            // Spec: recording R11. The loop's wrap is no jump away: the take
            // goes on into the next pass, unless Punch-Out is on, when the
            // wrap ends it (R9). Any other jump ends it.
            let wrapped = self.held_frames.or(self.loop_frames).filter(|region| {
                self.punch_frames.end.is_none()
                    && take.wrap.is_none_or(|seen| seen == *region)
                    && take.next == region.end
                    && self.position == region.start
            });
            match wrapped {
                Some(region) => take.wrap = Some(region),
                None => {
                    self.end_recording();
                    return;
                }
            }
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
                next: self.position,
                wrap: None,
                song: 0,
            });
            self.log_playing_clips();
        }
        let written = self.take.map_or(0, |take| take.written);
        let skip = (delay.max(0) as u64)
            .saturating_sub(written)
            .min(frames as u64) as usize;
        // The messages are stamped with the frame the take has run on to, not
        // the song frame, which a loop's wrap sets back (recording R12).
        let stamp = self
            .take
            .map_or(self.position, |take| take.start + Frames(take.song));
        let captured = match (self.program.as_ref(), self.plan.as_mut()) {
            (Some(program), Some(plan)) => {
                plan.get_mut()
                    .capture(program.get(), stamp, offset, frames, skip, transport)
            }
            _ => false,
        };
        if !captured {
            self.end_recording();
            return;
        }
        if let Some(take) = &mut self.take {
            take.written += frames as u64;
            take.song += transport.span(frames);
            take.next = Frames(transport.window_end(frames));
        }
    }

    /// Copies the notes and controllers the monitoring MIDI tracks received in
    /// the frames `offset..offset + frames` of the block into Capture MIDI's ring,
    /// stamped with the engine clock and, while the song plays, the song
    /// frame (WP-3.14).
    fn log_captured(&mut self, offset: usize, frames: usize, transport: TransportInfo) {
        let Some(program) = self.program.as_ref() else {
            return;
        };
        let song = (self.playing && !self.metronome.is_counting_in()).then_some(self.position);
        for (track, events) in program.get().midi.monitored_inputs() {
            for event in events {
                let at = event.offset as usize;
                if !(event.mpe != crate::mpe::MpeRole::Plain
                    || crate::recording::is_recorded(&event.message))
                    || at < offset
                    || at >= offset + frames
                {
                    continue;
                }
                let into = (at - offset) as i64;
                let logged = CapturedMidi {
                    track,
                    clock: self.clock + at as u64,
                    song: song.map(|position| {
                        position
                            + Frames(transport.song_frame(into as usize) - transport.position.get())
                    }),
                    message: event.message,
                    mpe: event.mpe,
                };
                if self.captured.push(logged).is_err() {
                    self.midi.counters.capture_dropped += 1;
                }
            }
        }
    }

    /// Copies this part of the block's input into the Session recording's
    /// rings (WP-3.11). The take starts at its grid frame and ends at the
    /// one it was asked to end on; it ignores the Arrangement's loop and
    /// punch points, and stamps its MIDI with a frame count that runs on
    /// through the loop's wraps, so the shell lays the clip out from 0.
    fn capture_session(&mut self, offset: usize, frames: usize, transport: TransportInfo) {
        let Some(session) = self.session_take else {
            return;
        };
        if !self.playing || self.metronome.is_counting_in() || frames == 0 {
            return;
        }
        if self.plan.is_none() || self.program.is_none() || self.reached() < session.start {
            return;
        }
        // The Arrangement loop wrapping the playhead moves it back, so a stop
        // that waited for a grid line the loop never reaches ends where the
        // loop wraps instead.
        let wrapped = session.last.is_some_and(|last| self.position < last);
        if let Some(stop) = session.stop
            && (wrapped || self.reached() >= stop)
        {
            self.end_session_take(session.launch, if wrapped { self.position } else { stop });
            return;
        }
        if self.take.is_none() {
            self.last_take = None;
            self.take = Some(Take {
                start: self.position,
                written: 0,
                next: self.position,
                wrap: None,
                song: 0,
            });
        }
        let Some(take) = self.take else {
            return;
        };
        let delay = self.plan.as_mut().map_or(0, |plan| plan.get_mut().delay);
        let skip = (delay.max(0) as u64)
            .saturating_sub(take.written)
            .min(frames as u64) as usize;
        let stamp = take.start + Frames(take.song);
        let captured = match (self.program.as_ref(), self.plan.as_mut()) {
            (Some(program), Some(plan)) => {
                plan.get_mut()
                    .capture(program.get(), stamp, offset, frames, skip, transport)
            }
            _ => false,
        };
        if !captured {
            self.end_session_take(false, self.position);
            return;
        }
        if let Some(take) = &mut self.take {
            take.written += frames as u64;
            take.song += transport.span(frames);
            take.next = stamp + Frames(transport.span(frames));
        }
        if let Some(session) = &mut self.session_take {
            session.last = Some(self.position);
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
        self.publish(cpu_load, frames);
    }

    /// Publishes the state for the UI, with this load.
    fn publish(&mut self, cpu_load: f32, frames: usize) {
        let position_ticks = self
            .tempo()
            .frames_to_ticks(self.position, self.config.sample_rate);

        let state = self.state.input_buffer_mut();
        state.clock = self.clock;
        state.time_base = self.midi.time_base();
        state.playing = self.playing;
        state.position = self.position;
        state.position_ticks = position_ticks;
        state.insert_marker = self.insert_marker;
        state.loop_region = self.loop_region;
        state.recording = self.recording;
        state.take_running = self.take.is_some();
        state.take_so_far = self.take.map(|take| TakeBounds {
            start: take.start,
            end: take.next,
            wrap: None,
            frames: take.written,
        });
        state.counting_in = self.metronome.is_counting_in();
        state.take = self.last_take;
        state.program = self.program.as_ref().map(|p| p.get().generation);
        state.swaps = self.swaps;
        state.cpu_load = cpu_load;
        state.cpu_load_max = self.cpu_load_max;
        state.following = self.followed;
        state.sync_in = self.midi.sync_in.state();
        state.cv_leader = self.cv_leader;
        state.waiting_start = !self.held.is_empty();
        state.waiting_frame = self.release.filter(|_| !self.held.is_empty());
        state.phase_nudge = self.nudge.active();
        state.tempo_overridden = self.tempo_overridden;
        state.counters = self.counters;
        state.sync_out = self
            .sync_out
            .state(self.playing, self.metronome.is_counting_in());
        self.session.status(&mut self.session_status);
        if self.session_take.is_some()
            && let Some(plan) = self.plan.as_mut()
        {
            // Spec: session-recording R2. The slots a recording goes into
            // show as recording, whatever the launcher says of them.
            for target in &plan.get_mut().session {
                let status = SlotStatus {
                    track: target.track,
                    scene: target.scene,
                    state: SlotState::Recording,
                    started: None,
                    offset: ClipOffset::ZERO,
                    velocity: 127,
                    transpose: 0,
                };
                let found = self
                    .session_status
                    .iter()
                    .position(|s| s.track == target.track && s.scene == target.scene);
                match found {
                    Some(i) => self.session_status[i] = status,
                    None if self.session_status.len() < self.session_status.capacity() => {
                        self.session_status.push(status);
                    }
                    None => {}
                }
            }
        }
        state.session.clear();
        state.session.extend_from_slice(&self.session_status);
        state.scene_override = self.scene_override;
        state.session_over.clear();
        let room = state.session_over.capacity();
        state
            .session_over
            .extend(self.session.overridden().iter().copied().take(room));
        state.meters.clear();
        state.meters_truncated = false;
        state.modulated.clear();
        if let Some(program) = &self.program {
            program.get().modulated(&mut state.modulated);
        }
        state.latency_changes.clear();
        if let Some(program) = &self.program {
            program.get().latency_changes(&mut state.latency_changes);
        }
        state.processing.clear();
        if let Some(program) = &self.program {
            program.get().processing(&mut state.processing);
        }
        if let Some(program) = &self.program {
            let program = program.get();
            let total = program.total_time();
            for &task in program.order.iter().filter(|&&t| Some(t) != program.cue) {
                let reading = program.meters(task);
                let meter = TrackMeter {
                    track: program.tasks[task as usize].track,
                    peak: reading.peak,
                    rms: reading.rms,
                    impact: program.impact(task, total),
                    gain_reduction: program.gain_reduction(task),
                };
                if state.meters.len() < state.meters.capacity() {
                    state.meters.push(meter);
                } else {
                    state.meters_truncated = true;
                }
            }
            state.chain_meters.clear();
            state.chain_meters_truncated = program.chain_meters(&mut state.chain_meters, frames);
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
    ext: &'a [f32],
    ext_channels: usize,
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

    /// Device channels the tracks on Ext. Out play on: channel 0 up to
    /// this many, counted from the device's first.
    #[must_use]
    pub fn ext_channels(&self) -> usize {
        self.ext_channels
    }

    /// Device channel `index` of what the Ext. Out tracks play. It is added
    /// to the device's channel, which Main Out may share.
    ///
    /// # Panics
    ///
    /// If `index` isn't less than [`ext_channels`](Self::ext_channels).
    #[must_use]
    pub fn ext_channel(&self, index: usize) -> &[f32] {
        assert!(index < self.ext_channels, "channel {index} out of range");
        let start = index * self.stride;
        &self.ext[start..start + self.frames]
    }
}
