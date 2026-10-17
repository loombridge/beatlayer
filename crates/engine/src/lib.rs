//! The real-time engine: compiler, graph, scheduler, transport, clip players,
//! mixer, delay compensation, automation, and metering.
//!
//! The engine never reads the document while it plays. A [`Compiler`] turns
//! a document snapshot into an [`EngineProgram`] off the audio thread: a
//! flat array of [`Node`]s, the tasks that run them (one per track: its
//! inputs, its device chain, and its [`mixer`]), their buffers, and the order
//! to run them in. The program travels to the
//! audio thread on a lock-free queue, and the [`Processor`] swaps it in at
//! the next block boundary, moving every node that survived the edit across
//! from the old program, so a reverb's tail or a plug-in instance carries on.
//! The old program, with the nodes that didn't survive, goes to the drop
//! thread to be freed. The design is in the architecture notes, "The
//! engine" (decisions D5 and D14).
//!
//! Most edits never compile: [`Compiler::sync`] turns mixer and parameter
//! edits into [`ParamEvent`]s, which reach their nodes on an exact frame.
//! The [`mixer`] glides every gain to its new value, so a fader move never
//! clicks, and works out who solo silences (the mixer spec).
//! Node data is a message of its own ([`Sender::send_data`]): each audio
//! track's [`clips`] reach its clip player that way, placed through the
//! tempo map, so clip edits never compile either (the audio clips spec).
//! Each clip reads its audio through a source of its own from the
//! registry's [`MediaLibrary`]: [`MediaStreams`] streams long files from
//! disk. A running engine opens sources on a loader thread, so loading a
//! long file never holds up the compiler thread, or Play and Stop behind it. A
//! [`ScheduledCommand`] runs at a musical position on its exact frame,
//! which is how an agent plays in time (D17).
//!
//! The [`Processor`] owns the transport: play, stop, continue, the insert
//! marker, and the Arrangement loop, each a [`TransportCommand`] that acts
//! on an exact frame (the transport spec). The [`transport`] module
//! registers them as commands people and agents run by ID.
//!
//! The pieces, and the threads they live on:
//!
//! | Piece | Thread | Job |
//! |---|---|---|
//! | [`Engine`] | main | Starts the threads below, takes documents and commands, shows the state |
//! | [`Compiler`] and [`Sender`] | compiler | Document → program and parameter changes; the queue's one sender |
//! | The loader | loader | Opens clips' sources, which reads and decodes files, for the compiler |
//! | [`Processor`] | audio | Runs the program each block |
//! | [`DropThread`] | drop | Frees what the audio thread let go of |
//! | [`Monitor`] | any one | Reads the playhead, meters, and CPU load the audio thread publishes |
//! | [`Offline`] | any one | All of the above on one thread, faster than real time, with no device |
//! | [`Restarter`] | any one | Restarts the engine with a new processor when a device's stream closes and opens again |
//! | [`preview::PreviewVoice`] | audio | The browser's preview, a voice of its own beside the song (WP-1.10) |
//!
//! The engine knows no audio backend. On a device, `bl-audio-engine`'s
//! `AudioEngine` runs it on a stream that `bl-audio-io`'s supervisor keeps
//! open (WP-1.13), through a [`Restarter`]: when the device goes away,
//! changes sample rate, stalls, or the machine sleeps, the processor comes
//! back from the closed stream and [parks](Processor::park), handing on a
//! [`Carry`] of its transport, clock, and waiting commands; when the stream
//! opens again, the engine restarts with a processor made for the stream's
//! rate and block size, the song recompiled for it and its transport taken
//! over, stopped where it was.
//!
//! An offline render needs no threads: [`Offline`] compiles, sends, and
//! processes in turn on one thread, collecting garbage as it goes, and runs
//! each block as a real-time section, as a device's audio thread does.
//! Underneath, [`channel`] makes the sender, processor, and monitor:
//!
//! ```
//! use bl_core::time::SampleRate;
//! use bl_engine::{Collector, Compiler, EngineConfig, Registry, channel};
//! use bl_model::Editor;
//!
//! let config = EngineConfig::new(SampleRate::HZ_48000, 512);
//! let mut collector = Collector::new();
//! let mut compiler = Compiler::new(config, Registry::new(), collector.handle());
//! let (mut sender, mut processor, mut monitor) = channel(&config);
//!
//! let editor = Editor::new();
//! sender.send_update(compiler.sync(editor.document()));
//! let output = processor.process(512);
//! assert_eq!(output.channel(0), &[0.0; 512][..]);
//! assert_eq!(monitor.state().program, Some(1));
//! collector.collect();
//! ```

#[cfg(test)]
bl_rt::install_trap!();

mod automation;
pub mod clips;
mod compiler;
pub mod display;
mod engine;
mod gc;
mod input;
mod layers;
mod message;
mod metronome;
mod midi;
pub mod midi_clips;
pub mod mixer;
mod node;
pub mod nodes;
mod offline;
pub mod preview;
mod processor;
mod program;
pub mod recording;
mod registry;
pub mod routing;
pub mod session;
pub mod sidechain;
mod sources;
mod state;
pub mod transport;

#[doc(hidden)]
pub mod testing;

use bl_core::time::SampleRate;

pub use automation::{AutomationList, clip_envelope_value_at, target_node};
pub use clips::{
    ClipPlayer, MediaLibrary, MediaMap, MediaStreams, OpenedSource, SlotSample, SourceCues,
};
pub use compiler::{Compiler, Update};
pub use display::{DisplayData, DisplayReader, DisplayWriter, Displays, display_channel};
pub use engine::{CompileCounts, Engine, Restarter, Sender, channel};
pub use gc::{Collector, DROP_PERIOD, DropThread, GcHandle, Owned, Program};
pub use input::{DeviceInput, NoInput};
pub use layers::{DeviceLayers, Group, LayerAudio, LayerSample, RoundRobin};
pub use message::{
    Action, LoopRegion, ParamEvent, Punch, Report, ScheduledCommand, TransportCommand, When,
};
pub use midi::{
    MIDI_EVENTS_PER_BLOCK, MIDI_EVENTS_PER_TRACK, MidiCounters, MidiEvent, TrackMidiInput,
};
pub use midi_clips::{MidiClipList, MidiClipPlayer};
pub use mixer::METER_RELEASE_DB_PER_SECOND;
pub use node::{
    Block, Node, NodeConfig, NodeData, NodeEvent, NodeKey, NoteBuffer, NoteEvent, NoteExpression,
    NoteKind, ParamValue, Segment, Segments, TransportInfo, segments,
};
pub use offline::Offline;
pub use processor::{Carry, Output, Processor};
pub use program::EngineProgram;
pub use recording::{
    LaunchLog, MidiRecordTrack, RecordTrack, RecordedMidi, RecordingPlan, SessionTarget, TakeBounds,
};
pub use registry::{DeviceType, NodeFactory, Registry};
pub use session::{
    SceneOverride, SessionCommand, SessionEvent, SessionPlayback, SlotState, SlotStatus,
};
pub use state::{EngineCounters, EngineState, Monitor, PlayedParam, TrackMeter};

/// An engine's fixed sizes, chosen when it's made. The audio thread works
/// within them and allocates nothing, so each is a ceiling: what goes over
/// is dropped and counted ([`EngineCounters`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// The sample rate.
    pub sample_rate: SampleRate,
    /// The most frames [`Processor::process`] takes at once.
    pub max_frames: usize,
    /// Output channels, and channels in every track's buffer.
    pub channels: usize,
    /// The audio device's input channels, for tracks whose Audio From is
    /// Ext. In (WP-2.13). Zero — no input — until a device with input
    /// opens.
    pub input_channels: usize,
    /// The audio device's output channels, for tracks whose Audio To is
    /// Ext. Out (WP-2.13). Zero — no Ext. Out plays — until a device opens.
    pub output_channels: usize,
    /// Messages the queue to the audio thread holds. More wait on the
    /// sending side ([`Sender::backlog`]).
    pub queue_capacity: usize,
    /// Parameter changes waiting for their frame, and in one block.
    pub max_pending_events: usize,
    /// Scheduled commands waiting to fire, and, separately, transport
    /// commands waiting for their frames.
    pub max_scheduled: usize,
    /// Tracks with a meter in [`EngineState::meters`].
    pub max_tracks: usize,
    /// Session slots the engine can launch at once, for
    /// [`EngineState::session`]: two per track, the clip playing (or
    /// stopping) and the one queued to replace it.
    pub max_slots: usize,
    /// Reports waiting for the main thread.
    pub report_capacity: usize,
    /// Modulated parameters in [`EngineState::modulated`].
    pub max_modulated: usize,
}

impl EngineConfig {
    /// Stereo at `sample_rate`, in blocks of up to `max_frames` frames, with
    /// room for 4,096 queued messages and pending parameter changes, 256
    /// scheduled commands, 1,024 metered tracks, 2,048 Session slots, and
    /// 1,024 reports, and 1,024 modulated parameters.
    ///
    /// # Panics
    ///
    /// If `max_frames` is 0.
    #[must_use]
    pub const fn new(sample_rate: SampleRate, max_frames: usize) -> Self {
        assert!(
            max_frames > 0,
            "an engine needs blocks of at least one frame"
        );
        Self {
            sample_rate,
            max_frames,
            channels: 2,
            input_channels: 0,
            output_channels: 0,
            queue_capacity: 4096,
            max_pending_events: 4096,
            max_scheduled: 256,
            max_tracks: 1024,
            max_slots: 2048,
            report_capacity: 1024,
            max_modulated: 1024,
        }
    }

    /// What every node is made for.
    #[must_use]
    pub fn node_config(&self) -> NodeConfig {
        NodeConfig {
            sample_rate: self.sample_rate,
            max_frames: self.max_frames,
            channels: self.channels,
        }
    }
}
