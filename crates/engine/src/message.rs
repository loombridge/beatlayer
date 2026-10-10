//! What travels to the audio thread, and what it reports back.

use bl_core::time::{Frames, Ticks};
use bl_model::ParamId;

use crate::gc::Program;
use crate::node::{NodeData, NodeKey, ParamValue};

/// When a parameter change takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// At the start of the next block the audio thread runs.
    Now,
    /// At this frame of the engine clock, which counts every frame the engine
    /// has processed, whether or not the transport runs
    /// ([`EngineState::clock`](crate::EngineState::clock)). A frame already
    /// past takes effect at the start of the next block.
    Frame(u64),
}

/// A parameter change for one node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamEvent {
    /// The node.
    pub key: NodeKey,
    /// The parameter.
    pub param: ParamId,
    /// The new value.
    pub value: ParamValue,
    /// When it takes effect.
    pub at: When,
}

impl ParamEvent {
    /// A change that takes effect at the start of the next block.
    #[must_use]
    pub fn now(key: NodeKey, param: ParamId, value: f64) -> Self {
        Self {
            key,
            param,
            value: ParamValue::Set(value),
            at: When::Now,
        }
    }

    /// A change that takes effect on frame `frame` of the engine clock.
    #[must_use]
    pub fn at_frame(key: NodeKey, param: ParamId, value: f64, frame: u64) -> Self {
        Self {
            key,
            param,
            value: ParamValue::Set(value),
            at: When::Frame(frame),
        }
    }
}

/// Starts, stops, or moves the song. The skeleton of the transport; the
/// whole of it (loop, follow, tempo-map following) is WP-1.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportCommand {
    /// Starts playing from the song position.
    Play,
    /// Stops, keeping the song position.
    Stop,
    /// Moves the song position.
    Locate(Ticks),
}

/// What a scheduled command does when the song reaches its position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// Changes a parameter on that frame.
    Param {
        /// The node.
        key: NodeKey,
        /// The parameter.
        param: ParamId,
        /// The new value.
        value: ParamValue,
    },
    /// Stops or moves the song on that frame. ([`TransportCommand::Play`]
    /// does nothing here: a scheduled command only fires while the song
    /// plays.)
    Transport(TransportCommand),
    /// Nothing in the engine. The report tells the main thread it's time, for
    /// a command whose effect is on the document (D17).
    Notify,
}

/// A command to run when the song reaches a musical position (D17): the
/// engine runs its action on the exact frame and reports
/// [`Report::Fired`].
///
/// It fires when the playing song reaches `at`, once. A position the song
/// has already passed fires when the song gets there again, after a locate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScheduledCommand {
    /// The caller's ID for it, which the report carries.
    pub id: u64,
    /// Where in the song it fires.
    pub at: Ticks,
    /// What it does.
    pub action: Action,
}

/// What the audio thread tells the main thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    /// A scheduled command fired.
    Fired {
        /// Its ID.
        id: u64,
        /// The song position it fired on: the first frame with its effect.
        position: Frames,
        /// The engine clock's frame it fired on.
        clock: u64,
    },
    /// A scheduled command was dropped because the engine holds as many as
    /// it can ([`EngineConfig::max_scheduled`](crate::EngineConfig::max_scheduled)).
    Rejected {
        /// Its ID.
        id: u64,
    },
}

/// One message on the queue to the audio thread. Programs, parameter
/// changes, node data, and transport commands share one queue, so they
/// arrive in the order they were sent: a parameter change never overtakes
/// the program that holds its node.
pub(crate) enum Message {
    Program(Program),
    Param(ParamEvent),
    Data { key: NodeKey, data: NodeData },
    Transport(TransportCommand),
    Schedule(ScheduledCommand),
    Cancel(u64),
}
