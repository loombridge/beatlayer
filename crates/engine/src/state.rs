//! What the engine tells the UI: playhead, meters, CPU load, and reports.

use bl_core::time::{Frames, Ticks};
use bl_model::TrackId;
use rtrb::Consumer;

use crate::message::{LoopRegion, Report};

/// How fast a meter falls after a peak, in decibels per second. A
/// placeholder until the mixer (WP-1.6) sets the meters' ballistics.
pub const METER_RELEASE_DB_PER_SECOND: f64 = 24.0;

/// One track's meter: its peak level per channel, as linear amplitude,
/// after its fader. A peak shows at once and falls at
/// [`METER_RELEASE_DB_PER_SECOND`], so a reader that looks less often than
/// the engine publishes still sees every peak.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackMeter {
    /// The track.
    pub track: TrackId,
    /// The first two channels: left and right.
    pub peak: [f32; 2],
}

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
    /// Frames processed since the engine started: the engine clock.
    pub clock: u64,
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
    /// The running program's generation, if one has arrived.
    pub program: Option<u64>,
    /// Programs swapped in.
    pub swaps: u64,
    /// The last block's processing time over the time it lasts.
    pub cpu_load: f32,
    /// The highest `cpu_load` so far.
    pub cpu_load_max: f32,
    /// Every track's meter, in the order the tracks run.
    pub meters: Vec<TrackMeter>,
    /// Whether some tracks have no meter because there are more than
    /// [`EngineConfig::max_tracks`](crate::EngineConfig::max_tracks).
    pub meters_truncated: bool,
    /// The audio thread's counters.
    pub counters: EngineCounters,
}

impl EngineState {
    /// A state before the first block, with room for `tracks` meters.
    pub(crate) fn with_capacity(tracks: usize) -> Self {
        Self {
            clock: 0,
            playing: false,
            position: Frames::ZERO,
            position_ticks: Ticks::ZERO,
            insert_marker: Ticks::ZERO,
            loop_region: None,
            program: None,
            swaps: 0,
            cpu_load: 0.0,
            cpu_load_max: 0.0,
            meters: Vec::with_capacity(tracks),
            meters_truncated: false,
            counters: EngineCounters::default(),
        }
    }

    /// One track's meter.
    #[must_use]
    pub fn meter(&self, track: TrackId) -> Option<[f32; 2]> {
        self.meters
            .iter()
            .find(|meter| meter.track == track)
            .map(|meter| meter.peak)
    }
}

impl Clone for EngineState {
    /// Keeps the meters' capacity, which the audio thread relies on: each of
    /// the triple buffer's copies is made by cloning the first.
    fn clone(&self) -> Self {
        let mut meters = Vec::with_capacity(self.meters.capacity());
        meters.extend_from_slice(&self.meters);
        Self {
            clock: self.clock,
            playing: self.playing,
            position: self.position,
            position_ticks: self.position_ticks,
            insert_marker: self.insert_marker,
            loop_region: self.loop_region,
            program: self.program,
            swaps: self.swaps,
            cpu_load: self.cpu_load,
            cpu_load_max: self.cpu_load_max,
            meters,
            meters_truncated: self.meters_truncated,
            counters: self.counters,
        }
    }
}

/// The main thread's view of the engine: its latest state and its reports.
/// Reading takes no lock, so any one thread can own it (the UI's, say).
pub struct Monitor {
    pub(crate) state: triple_buffer::Output<EngineState>,
    pub(crate) reports: Consumer<Report>,
}

impl Monitor {
    /// The latest state the audio thread published.
    pub fn state(&mut self) -> &EngineState {
        self.state.read()
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
