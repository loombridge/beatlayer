//! What travels to the audio thread, and what it reports back.

use bl_core::time::{Frames, Ticks};
use bl_model::ParamId;

use crate::gc::Program;
use crate::node::{NodeData, NodeKey, ParamValue};
use crate::session::SessionCommand;

/// When a parameter change or a transport command takes effect.
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

/// Starts, stops, or moves the song, or sets the Arrangement loop: the
/// transport, as the transport spec specifies it.
///
/// The transport keeps three things: the *insert marker*, where Play
/// starts; the *song position* (the playhead), the next frame to play,
/// which stays where playback stopped so Continue can resume there; and the
/// loop, if it's on. Every command takes effect on a frame: the start of
/// the next block, or the frame it's sent for
/// ([`Sender::transport_at`](crate::Sender::transport_at)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportCommand {
    /// Plays from the insert marker. While playing, starts again from it.
    Play,
    /// While playing, stops, keeping the song position for
    /// [`Continue`](Self::Continue). While stopped, returns the insert
    /// marker and the song position to 1.1.1 (tick 0), as a second press of
    /// Stop does.
    Stop,
    /// Plays on from the song position: where playback last stopped. Does
    /// nothing while playing.
    Continue,
    /// Stops while playing (never returning to 1.1.1), and plays from the
    /// insert marker while stopped: the space bar.
    PlayStop,
    /// Moves the insert marker. While playing, playback jumps there too;
    /// while stopped, the song position stays where playback stopped. A
    /// position before 1.1.1 is taken as 1.1.1.
    Locate(Ticks),
    /// Moves the insert marker to the song position.
    InsertMarkerToPlayhead,
    /// Moves the insert marker and nothing else: playback, if the song
    /// plays, carries on where it is, and while stopped, Continue still
    /// resumes where playback stopped. A position before 1.1.1 is taken as
    /// 1.1.1. A click in the Arrangement sends it
    /// (the control bar transport spec, R15).
    SetInsertMarker(Ticks),
    /// Sets the Arrangement loop, or switches it off with `None`.
    SetLoop(Option<LoopRegion>),
    /// Sets the metronome settings (WP-2.15).
    SetMetronome(bl_model::MetronomeSettings),
    /// Sets whether the transport is recording.
    SetRecording(bool),
}

/// The part of the song the Arrangement loop repeats: playback that reaches
/// `end` jumps back to `start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LoopRegion {
    /// Where the loop starts.
    pub start: Ticks,
    /// Where it ends: the first position it doesn't play.
    pub end: Ticks,
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
    /// Runs a transport command on that frame. A scheduled command fires
    /// only while the song plays, so [`TransportCommand::Play`] and
    /// [`TransportCommand::Continue`] do nothing here, and
    /// [`TransportCommand::Stop`] and [`TransportCommand::PlayStop`] stop.
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
    Transport(TransportCommand, When),
    /// A Session View command (WP-3.2): a launch, a stop, or the global
    /// launch quantisation.
    Session(SessionCommand),
    Schedule(ScheduledCommand),
    Cancel(u64),
}
