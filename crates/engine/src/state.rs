//! What the engine tells the UI: playhead, meters, CPU load, and reports.

use bl_core::time::{Frames, SampleRate, Ticks};
use bl_model::{ChainId, DeviceId, ParamId, TrackId};
use rtrb::Consumer;

use crate::capture::CapturedMidi;
use crate::message::{LoopRegion, NudgeDirection, Report};
use crate::midi::TimeBase;
#[cfg(doc)]
use crate::mixer::{METER_RELEASE_DB_PER_SECOND, RMS_SECONDS};
use crate::node::NodeKey;
#[cfg(doc)]
use crate::points::MIDI_METER_FALL_SECONDS;
use crate::recording::TakeBounds;
use crate::session::{SceneOverride, SlotStatus};

/// One track's meters: its peak and RMS levels per channel, as linear
/// amplitude, measured after its mixer (pan, volume, and the Track
/// Activator) and before solo (the mixer spec, R19). A peak shows at
/// once and falls at [`METER_RELEASE_DB_PER_SECOND`], so a reader that looks
/// less often than the engine publishes still sees every peak; the RMS level
/// is an average of the squared signal over about [`RMS_SECONDS`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackMeter {
    /// The track.
    pub track: TrackId,
    /// The peak level of the first two channels: left and right.
    pub peak: [f32; 2],
    /// The RMS level of the first two channels.
    pub rms: [f32; 2],
    /// The track's share of the set's processing time, from 0 to 1 (the
    /// mixer spec, R31): what the Performance Impact indicator shows, in
    /// six segments.
    pub impact: f32,
    /// The most gain reduction any of the track's compressors or limiters
    /// reported for the block, in dB, as a compressor's Gain Reduction reads
    /// it: zero when none of them reduced (the mixer view spec, R40). Gates
    /// are left out, as they don't report one.
    pub gain_reduction: f32,
}

/// The chain a meter point sits in: a track's, or a rack chain's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MeterOwner {
    /// The device chain of a track.
    Track(TrackId),
    /// The device chain of a rack chain.
    Chain(ChainId),
}

/// Where a meter point sits in its chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MeterSite {
    /// In front of the device, between it and the one before.
    Before(DeviceId),
    /// After the chain's last device.
    End,
}

/// What passed a meter point (the MIDI meters spec, R6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointReading {
    /// MIDI: each value is 0 when nothing of its kind passed in the last
    /// [`MIDI_METER_FALL_SECONDS`], and otherwise falls from its peak to 0
    /// over that time.
    Midi {
        /// The highest note-on velocity that passed, from 0 to 1.
        velocity: f32,
        /// 1 when a control change, pitch bend or channel pressure passed.
        channel: f32,
        /// 1 when a per-note expression passed.
        expression: f32,
    },
    /// Audio: the peak level of the first two channels, as linear amplitude,
    /// falling at [`METER_RELEASE_DB_PER_SECOND`] like a track's.
    Audio {
        /// Left and right.
        peak: [f32; 2],
    },
}

/// A meter between two devices of a chain, before the first or after the
/// last (the MIDI meters spec, R2 and R6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChainMeter {
    /// The chain.
    pub owner: MeterOwner,
    /// The place in it.
    pub at: MeterSite,
    /// What passed.
    pub reading: PointReading,
}

/// A parameter under modulation, as the last block played it (the modulation
/// spec, R7 and R11): the value it had before modulation and the value it
/// played, in the parameter's own units, at the block's last frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayedParam {
    /// The node whose parameter it is.
    pub node: NodeKey,
    /// The parameter.
    pub param: ParamId,
    /// Its base: its envelope's value, or its control's.
    pub base: f64,
    /// The value it played.
    pub played: f64,
}

/// How many latency changes one block can report; the rest wait for the next.
pub(crate) const LATENCY_CHANGES: usize = 64;

/// Counters the audio thread keeps, since the engine started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineCounters {
    /// Messages taken from the queue.
    pub messages: u64,
    /// Parameter changes that reached a node.
    pub events_applied: u64,
    /// Parameter changes for a node the running program doesn't have.
    pub events_unrouted: u64,
    /// Parameter changes dropped because too many were waiting
    /// ([`EngineConfig::max_pending_events`](crate::EngineConfig::max_pending_events)).
    pub events_overflowed: u64,
    /// Node data that reached a node.
    pub data_delivered: u64,
    /// Node data for a node the running program doesn't have.
    pub data_unrouted: u64,
    /// Scheduled commands that fired.
    pub scheduled_fired: u64,
    /// Scheduled commands dropped because too many were waiting.
    pub scheduled_rejected: u64,
    /// Transport commands for a frame dropped because too many were
    /// waiting for theirs
    /// ([`EngineConfig::max_scheduled`](crate::EngineConfig::max_scheduled)).
    pub transport_overflowed: u64,
    /// Programs that didn't follow the running one, so their surviving nodes
    /// couldn't move across. Any is a bug.
    pub migration_mismatches: u64,
    /// Reports dropped because the main thread wasn't reading them.
    pub reports_dropped: u64,
}

/// The engine as the audio thread last published it, after a block.
///
/// Every field comes from the same block, so a reader on any thread sees
/// the playhead, the transport, and the meters as they were together: the
/// UI draws its playhead from `position_ticks` while `playing`, and its
/// insert marker and loop brace from the fields beside it.
#[derive(Debug, PartialEq)]
pub struct EngineState {
    /// The sample rate the engine runs at. It changes when the engine
    /// restarts on a device at another rate (WP-1.13).
    pub sample_rate: SampleRate,
    /// Frames processed since the engine started: the engine clock. It
    /// stands still while no device runs the engine and carries on when it
    /// restarts on one, at whatever rate that runs, so it never runs
    /// backwards and a frame sent for it ([`When::Frame`](crate::When::Frame))
    /// means the same frame either side of a restart.
    pub clock: u64,
    /// Where the last block started on the engine clock and on the system's
    /// clock; none before the first block.
    pub time_base: Option<TimeBase>,
    /// Whether the song is playing.
    pub playing: bool,
    /// The playhead: the song position of the next frame to play. While
    /// stopped, it's where playback stopped, where Continue resumes.
    pub position: Frames,
    /// The playhead in ticks, through the running program's tempo map.
    pub position_ticks: Ticks,
    /// The insert marker: where Play starts.
    pub insert_marker: Ticks,
    /// The Arrangement loop the engine plays, if it's on.
    pub loop_region: Option<LoopRegion>,
    /// Whether the transport is recording (WP-2.13).
    pub recording: bool,
    /// Whether a take is running: the transport records and has begun to
    /// write the armed tracks' input, so it's past the count-in and any
    /// punch-in point. It ends at the punch-out point or when recording does.
    pub take_running: bool,
    /// Whether the count-in is sounding, before playback and recording start.
    pub counting_in: bool,
    /// The take that last ended, until the next recording starts: the song
    /// frames it covers. The shell makes the recorded clips from it.
    pub take: Option<TakeBounds>,
    /// The take running now, as far as it has got: its start, and the frame
    /// past the last one captured (WP-3.11). The Session View grows a
    /// recording clip to it.
    pub take_so_far: Option<TakeBounds>,
    /// The running program's generation, if one has arrived.
    pub program: Option<u64>,
    /// Programs swapped in.
    pub swaps: u64,
    /// The last block's processing time over the time it lasts.
    pub cpu_load: f32,
    /// The highest `cpu_load` so far.
    pub cpu_load_max: f32,
    /// The outside tempo the song plays at, in BPM, while a follower gives
    /// one (the transport spec, R24): it leads, so Phase Nudge is refused
    /// (R29).
    pub following: Option<f64>,
    /// Whether a start waits for the frame the follower gives (R25).
    pub waiting_start: bool,
    /// The frame of the engine clock a waiting start begins on, once the
    /// follower has given it.
    pub waiting_frame: Option<u64>,
    /// The Phase Nudge button that nudges the song while held (R27).
    pub phase_nudge: Option<NudgeDirection>,
    /// The Session slots the engine is doing something with (WP-3.2), one
    /// entry per queued, playing, or stopping slot; a slot that stopped
    /// isn't listed, so an empty list is a Session View at rest.
    pub session: Vec<SlotStatus>,
    /// The tempo and time signature a launched scene put in place of the
    /// set's, until Re-Enable Automation or Back to Arrangement (WP-3.5).
    pub scene_override: SceneOverride,
    /// The tracks a Session clip has taken over from the Arrangement, until
    /// a return (WP-3.6): the Back to Arrangement buttons light while it
    /// lists any, and a track's own button lights while it lists the track.
    pub session_over: Vec<TrackId>,
    /// Every track's meter, in the order the tracks run.
    pub meters: Vec<TrackMeter>,
    /// Whether some tracks have no meter because there are more than
    /// [`EngineConfig::max_tracks`](crate::EngineConfig::max_tracks).
    pub meters_truncated: bool,
    /// The meters between the devices of every chain (WP-6.69), in the order
    /// the program's chains run.
    pub chain_meters: Vec<ChainMeter>,
    /// Whether some chain meters are missing because there are more than
    /// [`EngineConfig::max_chain_meters`](crate::EngineConfig::max_chain_meters).
    pub chain_meters_truncated: bool,
    /// The parameters under modulation while the song plays, as the last
    /// block played them (WP-5.9); the rest play their base.
    pub modulated: Vec<PlayedParam>,
    /// The devices whose node reports a latency other than the one the
    /// program compensates for (WP-2.11): what the shell hands to
    /// [`Engine::set_device_latency`](crate::Engine::set_device_latency), so
    /// a native device whose latency follows a parameter, such as a
    /// look-ahead, is compensated at its new latency.
    pub latency_changes: Vec<(DeviceId, u32)>,
    /// The rack chains that were processing in the last block (WP-5.3, the
    /// racks spec, R37): in an instrument or MIDI effect rack each chain a
    /// held note reached, in an audio effect rack each chain whose chain
    /// select zone held the selector's value.
    pub processing: Vec<ChainId>,
    /// The audio thread's counters.
    pub counters: EngineCounters,
    /// The MIDI clock the engine sends (midi-clock R12).
    pub sync_out: crate::SyncOutState,
    /// The MIDI clock the engine follows (midi-clock R16, R17).
    pub sync_in: crate::SyncInState,
}

impl EngineState {
    /// A state before the first block at `sample_rate`, with room for
    /// `tracks` meters and `slots` Session slots.
    pub(crate) fn with_capacity(
        tracks: usize,
        slots: usize,
        modulated: usize,
        processing: usize,
        chain_meters: usize,
        sample_rate: SampleRate,
    ) -> Self {
        Self {
            sample_rate,
            clock: 0,
            time_base: None,
            playing: false,
            position: Frames::ZERO,
            position_ticks: Ticks::ZERO,
            insert_marker: Ticks::ZERO,
            loop_region: None,
            recording: false,
            take_running: false,
            counting_in: false,
            take: None,
            take_so_far: None,
            program: None,
            swaps: 0,
            cpu_load: 0.0,
            cpu_load_max: 0.0,
            following: None,
            waiting_start: false,
            waiting_frame: None,
            phase_nudge: None,
            session: Vec::with_capacity(slots),
            scene_override: SceneOverride::default(),
            session_over: Vec::with_capacity(tracks),
            meters: Vec::with_capacity(tracks),
            meters_truncated: false,
            chain_meters: Vec::with_capacity(chain_meters),
            chain_meters_truncated: false,
            modulated: Vec::with_capacity(modulated),
            latency_changes: Vec::with_capacity(LATENCY_CHANGES),
            processing: Vec::with_capacity(processing),
            counters: EngineCounters::default(),
            sync_out: crate::SyncOutState::default(),
            sync_in: crate::SyncInState::default(),
        }
    }

    /// The base and played value of `param` of `node` while it's under
    /// modulation, `None` when it plays its base.
    #[must_use]
    pub fn played(&self, node: NodeKey, param: ParamId) -> Option<&PlayedParam> {
        self.modulated
            .iter()
            .find(|entry| entry.node == node && entry.param == param)
    }

    /// One track's peak meter, left and right.
    #[must_use]
    pub fn meter(&self, track: TrackId) -> Option<[f32; 2]> {
        self.track_meter(track).map(|meter| meter.peak)
    }

    /// One track's RMS meter, left and right.
    #[must_use]
    pub fn rms(&self, track: TrackId) -> Option<[f32; 2]> {
        self.track_meter(track).map(|meter| meter.rms)
    }

    /// One track's gain reduction, in dB, zero when none of its compressors
    /// or limiters reduced (the mixer view spec, R40).
    #[must_use]
    pub fn gain_reduction(&self, track: TrackId) -> Option<f32> {
        self.track_meter(track).map(|meter| meter.gain_reduction)
    }

    /// The meter at `at` in the chain `owner`.
    #[must_use]
    pub fn chain_meter(&self, owner: MeterOwner, at: MeterSite) -> Option<&ChainMeter> {
        self.chain_meters
            .iter()
            .find(|meter| meter.owner == owner && meter.at == at)
    }

    /// One track's meters.
    #[must_use]
    pub fn track_meter(&self, track: TrackId) -> Option<&TrackMeter> {
        self.meters.iter().find(|meter| meter.track == track)
    }
}

impl Clone for EngineState {
    /// Keeps the meters' capacity, which the audio thread relies on: each of
    /// the triple buffer's copies is made by cloning the first.
    fn clone(&self) -> Self {
        let mut meters = Vec::with_capacity(self.meters.capacity());
        meters.extend_from_slice(&self.meters);
        let mut chain_meters = Vec::with_capacity(self.chain_meters.capacity());
        chain_meters.extend_from_slice(&self.chain_meters);
        let mut session = Vec::with_capacity(self.session.capacity());
        session.extend_from_slice(&self.session);
        let mut modulated = Vec::with_capacity(self.modulated.capacity());
        modulated.extend_from_slice(&self.modulated);
        let mut latency_changes = Vec::with_capacity(self.latency_changes.capacity());
        latency_changes.extend_from_slice(&self.latency_changes);
        let mut processing = Vec::with_capacity(self.processing.capacity());
        processing.extend_from_slice(&self.processing);
        let mut session_over = Vec::with_capacity(self.session_over.capacity());
        session_over.extend_from_slice(&self.session_over);
        Self {
            sample_rate: self.sample_rate,
            clock: self.clock,
            time_base: self.time_base,
            playing: self.playing,
            position: self.position,
            position_ticks: self.position_ticks,
            insert_marker: self.insert_marker,
            loop_region: self.loop_region,
            recording: self.recording,
            take_running: self.take_running,
            counting_in: self.counting_in,
            take: self.take,
            take_so_far: self.take_so_far,
            program: self.program,
            swaps: self.swaps,
            cpu_load: self.cpu_load,
            cpu_load_max: self.cpu_load_max,
            following: self.following,
            waiting_start: self.waiting_start,
            waiting_frame: self.waiting_frame,
            phase_nudge: self.phase_nudge,
            session,
            scene_override: self.scene_override,
            session_over,
            meters,
            meters_truncated: self.meters_truncated,
            chain_meters,
            chain_meters_truncated: self.chain_meters_truncated,
            modulated,
            latency_changes,
            processing,
            counters: self.counters,
            sync_out: self.sync_out,
            sync_in: self.sync_in,
        }
    }
}

/// The main thread's view of the engine: its latest state and its reports.
/// Reading takes no lock, so any one thread can own it (the UI's, say).
pub struct Monitor {
    pub(crate) state: triple_buffer::Output<EngineState>,
    pub(crate) reports: Consumer<Report>,
    pub(crate) captured: Consumer<CapturedMidi>,
}

impl Monitor {
    /// The latest state the audio thread published.
    pub fn state(&mut self) -> &EngineState {
        self.state.read()
    }

    /// The notes monitoring MIDI tracks received since the last call,
    /// oldest first, for Capture MIDI (WP-3.14).
    pub fn captured(&mut self) -> Vec<CapturedMidi> {
        let mut notes = Vec::with_capacity(self.captured.slots());
        while let Ok(note) = self.captured.pop() {
            notes.push(note);
        }
        notes
    }

    /// Reports since the last call, oldest first.
    pub fn reports(&mut self) -> Vec<Report> {
        let mut reports = Vec::with_capacity(self.reports.slots());
        while let Ok(report) = self.reports.pop() {
            reports.push(report);
        }
        reports
    }
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor").finish_non_exhaustive()
    }
}
