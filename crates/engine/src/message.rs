//! What travels to the audio thread, and what it reports back.

use bl_core::time::{Frames, Ticks};
use bl_model::ParamId;

use crate::follower::Follower;
use crate::gc::{Owned, Program};
use crate::link_audio::LinkAudio;
use crate::node::{NodeData, NodeKey, ParamValue};
use crate::recording::RecordingPlan;
use crate::render::RenderPlan;
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
    /// Plays from the insert marker, or from the song's start time when it
    /// has one. While playing, starts again from it.
    Play,
    /// Plays from the insert marker, whatever the song's start time is.
    PlayFromInsertMarker,
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
    /// Moves the insert marker as [`Locate`](Self::Locate) does, but while
    /// the song plays the jump waits for the next line of the global launch
    /// quantisation, and lands on that line's exact frame (the transport
    /// spec, R22). A second one before the line replaces the first.
    QuantizedLocate(Ticks),
    /// A press, or its release, in the scrub area (the time editing spec,
    /// R25 and R28). With `held`, loops the span of the global launch
    /// quantisation at `point` until it is released, and starts playback at
    /// its start if the song is stopped (the transport spec, R23). Without
    /// it, ends a held loop and plays on from where it is; with none held,
    /// plays from `point` as a click in the scrub area does, a quantised
    /// locate that starts playback when the song is stopped.
    Scrub {
        /// Where the scrub area was pressed or clicked.
        point: Ticks,
        /// Whether the button is held.
        held: bool,
    },
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
    /// Sets where the song starts, or clears it with `None` (the transport
    /// spec, R21).
    SetSongStart(Option<Ticks>),
    /// Sets the metronome settings (WP-2.15).
    SetMetronome(bl_model::MetronomeSettings),
    /// Sets the Global Amount of groove every device's block carries, in
    /// tenths of a percent: 1,000 is 100 % (the grooves spec, R21).
    SetGrooveAmount(u16),
    /// Sets the set's current scale, which the devices of a track with no
    /// clip playing read (the scales spec, R9 and R15).
    SetScale(bl_model::Scale),
    /// Sets whether the set's tuning hides the scales, so a scale-aware
    /// device's Use Current Scale acts as off (the tunings spec, R16).
    SetScalesHidden(bool),
    /// Sets whether the transport is recording.
    SetRecording(bool),
    /// Sets where recording punches in and out (WP-2.13).
    SetPunch(Punch),
    /// Resync External Hardware (`docs/specs/midi-clock.md`, R11): while the
    /// song plays, sends each output with Sync on its Hardware Resync choice
    /// before the clock of the next bar line. Does nothing while the
    /// transport is stopped.
    ResyncExternalHardware,
    /// Presses or releases Phase Nudge Down or Up: while one is held and the
    /// song plays, the song plays 4 % slower or faster (the transport spec,
    /// R27). With both held, the one pressed last nudges. The engine ignores
    /// a press while the song follows an outside tempo (R29).
    PhaseNudge {
        /// Which button.
        direction: NudgeDirection,
        /// Whether it is held: true on the press, false on the release.
        held: bool,
    },
}

/// Which Phase Nudge button (the transport spec, R27).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum NudgeDirection {
    /// Phase Nudge Down: 4 % slower.
    Down,
    /// Phase Nudge Up: 4 % faster.
    Up,
}

/// Where recording starts and stops, as the Arrangement loop brace and the
/// Punch-In and Punch-Out switches give it, whether or not the Loop switch is
/// on (WP-2.13). A side whose switch is off is `None`: recording starts
/// when playback does, or runs until it stops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Punch {
    /// The punch-in point: the brace's start.
    pub start: Option<Ticks>,
    /// The punch-out point: the brace's end, the first position not recorded.
    pub end: Option<Ticks>,
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
    Data {
        key: NodeKey,
        data: NodeData,
    },
    Transport(TransportCommand, When),
    /// The Session View's slot rows and follow table after an edit of their
    /// values: swapped into the running program, which keeps its shape.
    SceneTables(Owned<crate::session::SceneTables>),
    /// A Session View command (WP-3.2): a launch, a stop, or the global
    /// launch quantisation.
    Session(SessionCommand),
    Schedule(ScheduledCommand),
    Cancel(u64),
    /// What to record while the transport records (WP-2.13).
    Record(Owned<RecordingPlan>),
    /// What to capture while the song plays a range in real time (WP-5.35).
    Render(Owned<RenderPlan>),
    /// The frames between the engine handing the device a sound and it
    /// coming out, as the device reports it now (WP-5.34).
    OutputLatency(u32),
    /// The outside tempo the song follows from the next block on, or `None`
    /// for none (the transport spec, R24).
    Follower(Option<Owned<Box<dyn Follower>>>),
    /// The Link Audio the engine reads peers' channels from and offers its
    /// tracks through, or `None` for none (the Link Audio spec, R4 to R6).
    LinkAudio(Option<Owned<Box<dyn LinkAudio>>>),
    /// Output `index` of the `count` output ports with Sync on and their
    /// options (midi-clock R3); a `count` of 0 clears the list.
    SyncOutput {
        index: u8,
        count: u8,
        output: Option<bl_core::midi::SyncOutput>,
    },
    /// Input `index` of the `count` input ports with Sync on and their Sync
    /// Delay (midi-clock R14); a `count` of 0 clears the list.
    SyncInput {
        index: u8,
        count: u8,
        input: Option<bl_core::midi::SyncInput>,
    },
    /// Whether the song follows the incoming clock: EXT (midi-clock R15).
    ExternalSync(bool),
    /// The output port opened, or its Track switch went on: the MPE routes to
    /// it send their configuration again (mpe R17).
    MidiOutputOpened(bl_core::midi::PortId),
}
