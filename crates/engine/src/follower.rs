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
}

/// What a follower answers for a block. The processor clears it before each
/// ask, so a follower that says nothing is not following this block.
#[derive(Debug)]
pub struct Follow {
    tempo: Option<f64>,
    decides_starts: bool,
    start_at: Option<u64>,
    commands: Vec<(u64, TransportCommand)>,
}

impl Follow {
    pub(crate) fn with_capacity(commands: usize) -> Self {
        Self {
            tempo: None,
            decides_starts: false,
            start_at: None,
            commands: Vec::with_capacity(commands),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.tempo = None;
        self.decides_starts = false;
        self.start_at = None;
        self.commands.clear();
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

    pub(crate) fn followed(&self) -> Option<f64> {
        self.tempo
    }

    pub(crate) fn deciding(&self) -> bool {
        self.decides_starts
    }

    pub(crate) fn release(&self) -> Option<u64> {
        self.start_at
    }

    pub(crate) fn commands(&self) -> &[(u64, TransportCommand)] {
        &self.commands
    }
}

/// An outside tempo the song follows (R24). T324 (MIDI clock in), T326
/// (Link) and T330 (CV Clock In) implement it. It runs on the audio thread:
/// no allocation, lock or blocking call.
pub trait Follower: Send {
    /// Answers for the block `ask` describes by calling `answer`'s methods.
    fn block(&mut self, ask: &Ask, answer: &mut Follow);
}
