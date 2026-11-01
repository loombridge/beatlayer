//! Following an outside tempo (the transport spec, R24 to R26).
//!
//! A [`Follower`] is what an incoming MIDI clock, a Link session or a CV
//! clock plugs into the engine. The processor asks it once per block, on the
//! audio thread, for the tempo to play that block at, any jumps and starts
//! and stops it decides, and whether it decides when a start happens. The
//! processor turns the tempo into a song speed that every node honours
//! ([`TransportInfo::speed`](crate::TransportInfo::speed)); the follower
//! itself knows nothing of nodes or tempo maps.

use bl_core::time::Ticks;

use crate::message::TransportCommand;

/// The slowest and fastest tempo a followed tempo can be, in BPM: the Tempo
/// field's range.
pub const MIN_BPM: f64 = 20.0;
/// See [`MIN_BPM`].
pub const MAX_BPM: f64 = 999.0;

/// What the processor tells a follower about the block it's about to play.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ask {
    /// The engine clock's frame the block starts on.
    pub clock: u64,
    /// Frames in the block.
    pub frames: usize,
    /// Whether the song plays.
    pub playing: bool,
    /// Where the song stands at the block's start, in ticks, perhaps
    /// fractional.
    pub position: f64,
    /// The frame rate.
    pub rate: f64,
    /// The tempo the tempo map plays at the position, in BPM: the tempo the
    /// set asks for, the song's own (the Link spec, R3 and R4).
    pub tempo: f64,
    /// Whether the tempo map holds more than one tempo: a tempo envelope.
    pub varies: bool,
    /// Whether an outside change of the tempo has taken the envelope's
    /// place, until Re-Enable Automation ([`Follow::override_tempo`]).
    pub overridden: bool,
    /// The tempo an incoming MIDI clock leads with this block, if one does
    /// (the Link spec, R10).
    pub leader: Option<f64>,
    /// The length of the global launch quantisation at the position, in
    /// quarter notes (the Link spec, R5).
    pub quantum: f64,
    /// How far into its quantum the position is, in quarter notes.
    pub phase: f64,
    /// How far into its quantum the position a start would play from is, in
    /// quarter notes: the song start time or insert marker of a start that
    /// waits, else the insert marker.
    pub start_phase: f64,
    /// Whether a start waits for the frame the follower gives (R25).
    pub waiting: bool,
    /// The audio interface's output latency, in frames.
    pub latency: u32,
}

/// What a follower answers for a block. The processor clears it before each
/// ask, so a follower that says nothing is not following this block.
#[derive(Debug)]
pub struct Follow {
    tempo: Option<f64>,
    decides_starts: bool,
    start_at: Option<u64>,
    commands: Vec<(u64, TransportCommand)>,
    no_count_in: bool,
    override_tempo: bool,
    moment: Option<i64>,
}

impl Follow {
    pub(crate) fn with_capacity(commands: usize) -> Self {
        Self {
            tempo: None,
            decides_starts: false,
            start_at: None,
            commands: Vec::with_capacity(commands),
            no_count_in: false,
            override_tempo: false,
            moment: None,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.tempo = None;
        self.decides_starts = false;
        self.start_at = None;
        self.commands.clear();
        self.no_count_in = false;
        self.override_tempo = false;
        self.moment = None;
    }

    /// Plays this block at `bpm`, limited to 20 to 999 BPM. While a follower
    /// gives one, it leads: Phase Nudge is refused (R29).
    pub fn tempo(&mut self, bpm: f64) {
        if bpm.is_finite() {
            self.tempo = Some(bpm.clamp(MIN_BPM, MAX_BPM));
        }
    }

    /// Says the follower decides when a start happens (R25): a start waits
    /// for the frame [`start_at`](Self::start_at) gives. Said every block the
    /// follower decides; a block that doesn't say it ends the wait, and any
    /// start waiting then happens at once.
    pub fn decides_starts(&mut self) {
        self.decides_starts = true;
    }

    /// Gives the frame of the engine clock a waiting start begins on, if one
    /// waits. A frame already past is the block's start.
    pub fn start_at(&mut self, frame: u64) {
        self.start_at = Some(frame);
    }

    /// Runs `command` on `frame` of the engine clock, as the follower's own:
    /// [`TransportCommand::Play`], [`Continue`](TransportCommand::Continue)
    /// and [`Stop`](TransportCommand::Stop) for a start or stop (R25), or
    /// [`Locate`](TransportCommand::Locate) for a jump (R24). It isn't held
    /// back by a start waiting. Returns whether there was room: as many as
    /// [`EngineConfig::max_scheduled`](crate::EngineConfig::max_scheduled) fit
    /// in a block.
    pub fn command(&mut self, frame: u64, command: TransportCommand) -> bool {
        if self.commands.len() == self.commands.capacity() {
            return false;
        }
        self.commands.push((frame, command));
        true
    }

    /// Jumps to `to` on `frame`: a locate on its frame, as R7's.
    pub fn jump(&mut self, frame: u64, to: Ticks) -> bool {
        self.command(frame, TransportCommand::Locate(to))
    }

    /// Says no count-in plays while this follower is on: playback and
    /// recording start as with Count-In set to None (the metronome spec, R10).
    pub fn no_count_in(&mut self) {
        self.no_count_in = true;
    }

    /// Says an outside change of the tempo took a tempo envelope's place:
    /// the envelope stops and Re-Enable Automation lights, until it is
    /// pressed (the Link spec, R4).
    pub fn override_tempo(&mut self) {
        self.override_tempo = true;
    }

    /// Says the block's first frame is heard at `micros` on Link's clock, as
    /// the output latency has it, before any shift the follower makes of its
    /// own: where the audio of Link Audio's peers is read and where the
    /// tracks offered to them are placed (the Link Audio spec, R6).
    pub fn moment(&mut self, micros: i64) {
        self.moment = Some(micros);
    }

    pub(crate) fn heard(&self) -> Option<i64> {
        self.moment
    }

    pub(crate) fn followed(&self) -> Option<f64> {
        self.tempo
    }

    pub(crate) fn deciding(&self) -> bool {
        self.decides_starts
    }

    pub(crate) fn release(&self) -> Option<u64> {
        self.start_at
    }

    pub(crate) fn counts_in_off(&self) -> bool {
        self.no_count_in
    }

    pub(crate) fn overrides_tempo(&self) -> bool {
        self.override_tempo
    }

    pub(crate) fn commands(&self) -> &[(u64, TransportCommand)] {
        &self.commands
    }
}

/// An outside tempo the song follows (R24): an incoming MIDI clock, a Link
/// session or a CV clock. It runs on the audio thread:
/// no allocation, lock or blocking call.
pub trait Follower: Send {
    /// Answers for the block `ask` describes by calling `answer`'s methods.
    fn block(&mut self, ask: &Ask, answer: &mut Follow);
}
