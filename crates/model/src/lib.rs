//! The document: tracks, clips, devices, automation, scenes, and grooves;
//! commands, transactions, and undo.
//!
//! A [`Document`] is the whole song as an immutable, persistent tree:
//! settings, tracks, the clips on them, and the devices in their chains. An
//! edit makes a new version that shares every untouched node with the old
//! one, so versions are cheap to keep (that's the undo history) and cheap to
//! hand to other threads (that's how the compiler and autosave read the song
//! while the main thread edits).
//!
//! The only way to change a document is a [`Command`]. The [`Editor`] owns
//! the current version, groups commands into transactions (one per UI
//! gesture, one undo step each), keeps the undo history, and hands out the
//! stable [IDs](bl_core::id::Id) that name every object. A [`ChangeSet`]
//! tells the compiler which tracks, clips, and devices differ between two
//! versions.
//!
//! Documents and commands serialise with serde. Audio never goes in the
//! document: an audio clip names its media by [`ContentHash`].
//!
//! ```
//! use bl_core::time::Ticks;
//! use bl_model::{Clip, ClipData, Command, Editor, MidiData, TrackKind};
//!
//! let mut editor = Editor::new();
//! let track = editor.new_id();
//! editor.apply(Command::CreateTrack {
//!     id: track,
//!     kind: TrackKind::Midi,
//!     name: "Bass".to_owned(),
//!     parent: None,
//!     index: 0,
//! })?;
//! let clip = editor.new_id();
//! editor.apply(Command::CreateClip {
//!     track,
//!     clip: Clip::new(clip, Ticks::ZERO, Ticks::WHOLE, ClipData::Midi(MidiData::default())),
//! })?;
//!
//! // A drag is one transaction, so it's one undo step however far it goes.
//! editor.begin("Move clip");
//! for bar in 1..=4 {
//!     editor.apply(Command::MoveClip { clip, track, start: Ticks::WHOLE * bar })?;
//! }
//! editor.commit()?;
//! assert_eq!(editor.document().clip(clip).unwrap().start, Ticks::WHOLE * 4);
//!
//! editor.undo()?;
//! assert_eq!(editor.document().clip(clip).unwrap().start, Ticks::ZERO);
//! # Ok::<(), bl_model::EditError>(())
//! ```
//!
//! Every command, and undo and redo, is also in the command registry
//! (`bl-commands`), so people and agents can run it by ID with JSON
//! arguments: see [`registry`]. The command reference is
//! the command reference.
//!
//! The rules taken from Live are in the tracks and undo spec.

pub mod articulation;
mod auto_warp;
pub mod automation;
pub mod base64;
pub mod bounce;
mod changes;
mod clip;
mod command;
pub mod comp;
mod device;
pub mod device_display;
pub mod device_layout;
mod document;
mod editor;
mod error;
mod external;
pub mod fader;
mod follow;
pub mod graph;
mod ids;
mod launch;
mod layers;
mod locator;
mod metronome;
pub mod modulation;
pub mod modulator;
pub mod param;
pub mod rack;
pub mod registry;
mod repr;
mod routing;
mod sample_edits;
mod scale;
mod scene;
mod shared;
mod track;
mod warp;
mod warp_settings;

pub use articulation::{
    ARTICULATION_MAP, Articulation, ArticulationOutput, ArticulationTrigger,
    Entry as ArticulationEntry, List as ArticulationList, MAX_ARTICULATIONS, MAX_OUTPUTS,
};
pub use auto_warp::{
    ClipDefaults, ClipEdge, EVEN_LOOP_BARS, FileBeats, LOOP_BARS, WarpFrom, WarpPoint, loop_bpm,
    nearest_bars,
};
pub use automation::{
    Automation, AutomationTarget, Breakpoint, Envelope, MixerParam, TrackAutomation, curve_value,
    envelope_value,
};
pub use bl_stretch::WarpMap;
pub use bounce::{BouncedAutomation, BouncedTrack};
pub use changes::{Change, ChangeSet};
pub use clip::{
    AudioData, Clip, ClipData, ClipEnvelope, ClipFolds, EDGE_FADE_SECONDS, EnvelopeKind, MidiData,
    NewTakeLane, Note, PressurePoint, TakeClip, TakeClipData, TakeLane, consolidated_midi,
};
pub use command::Command;
pub use device::{
    Device, DeviceKind, DeviceRole, DeviceWarp, DeviceWindows, MAX_MEDIA_SLOTS, MidiSidechain,
    PluginFormat, PluginState, Sidechain, WindowFrame,
};
pub use device_display::{DeviceDisplay, DisplayMarker, DisplayReading, DisplaySeries};
pub use device_layout::DeviceLayout;
pub use document::{ArrangementLoop, Document, MAX_CUE_VOLUME, Settings};
pub use editor::{EditError, Editor, HistoryStep, Maker};
pub use error::ModelError;
pub mod freeze;
pub use external::{
    External, ExternalPort, HardwareLatency, MAX_HARDWARE_LATENCY_MS, MAX_HARDWARE_LATENCY_SAMPLES,
};
pub use follow::{
    FOLLOW_MULTIPLIER_MAX, FOLLOW_TIME_MAX, FOLLOW_TIME_MIN, FollowAction, FollowActions,
    FollowPatch,
};
pub use freeze::{FrozenClip, FrozenOutput, FrozenTail, FrozenTrack};
pub use ids::{
    ChainId, ClipId, ContentHash, DeviceId, LocatorId, ParamId, ParseContentHashError, SceneId,
    TakeLaneId, TrackId,
};
/// The persistent collections the document's public types hold.
pub use imbl::{OrdMap, Vector};
pub use launch::{ClipLaunch, LaunchMode, LaunchQuantization, LaunchSettings};
pub use layers::{
    DEFAULT_ROOT_KEY, Distribution, EditorSize, FadeMode, LAYER_GAIN_MAX, Layer, LayerId,
    LayerSettings, LayerSort, LayerZone, Layers, MultisamplerView, MultisamplerViews, ReleaseMode,
    SamplerTab, SustainMode, Zone, ZoneKind, snap_to_zero_crossing,
};
pub use locator::{Locator, SongStart};
pub use metronome::{CountIn, MetronomeRhythm, MetronomeSettings, MetronomeSound};
pub use param::{DeviceDescription, ParamDescriptor, ParamKind, ParamUnit};
pub use rack::{
    CHAIN_COLOURS, CHAIN_SELECTOR, Chain, ChainMixer, ChainSend, ChainZones, DEFAULT_PLAY,
    DevicePlace, FILTER_MIDI_CTRL, FIRST_PAD, MACRO_BASE, MACRO_STEPS_DEFAULT, MACRO_STEPS_MAX,
    MACRO_STEPS_MIN, MACROS, MAX_RETURN_CHAINS, Macro, MacroMapping, MacroStyle, MacroTarget,
    RackKind, RackMacros, RackView, RackViewMap, RackViews, Receive, SHOWN_MACROS,
    default_macro_name, description as rack_description, general_midi_drum, macro_number,
    macro_param, macro_step_position, note_name, receive_choices, receive_label,
};
pub use repr::{SavedSong, saved_schema};
pub use sample_edits::{SampleEdit, SampleEditKind, SampleRange};
pub use scale::{CHROMATIC, Scale, ScaleName};
pub use scene::{DEFAULT_SCENE_COUNT, SCENE_TEMPO_MAX, SCENE_TEMPO_MIN, Scene};
pub use track::{
    Channel, CrossfadeAssign, DeviceOutput, Input, MAX_TRACK_DELAY_MS, MAX_TRACK_DELAY_SAMPLES,
    MidiTo, Mixer, Monitor, Output, PanMode, Routing, SendTap, Tap, Track, TrackDelay, TrackKind,
};
pub use warp::{TRANSIENT_MATCH, TransientEdits, WarpMarker};
pub use warp_settings::{
    BeatsSettings, BeatsUpdate, ComplexProSettings, ComplexProUpdate, DETUNE_MAX, DETUNE_MIN,
    Preserve, TRANSPOSE_MAX, TRANSPOSE_MIN, TextureSettings, TextureUpdate, TonesSettings,
    TonesUpdate, TransientLoopMode, WarpMode, WarpSettings,
};
