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
//! `docs/commands.md`.
//!
//! The rules taken from Live are in `docs/specs/tracks-and-undo.md`.

mod changes;
mod clip;
mod command;
mod device;
mod document;
mod editor;
mod error;
mod ids;
pub mod registry;
mod repr;
mod shared;
mod track;

pub use changes::{Change, ChangeSet};
pub use clip::{AudioData, Clip, ClipData, MidiData, Note};
pub use command::Command;
pub use device::{Device, DeviceKind, PluginFormat};
pub use document::{ArrangementLoop, Document, Settings};
pub use editor::{EditError, Editor};
pub use error::ModelError;
pub use ids::{ClipId, ContentHash, DeviceId, ParamId, ParseContentHashError, TrackId};
/// The persistent collections the document's public types hold.
pub use imbl::{OrdMap, Vector};
pub use track::{Mixer, Track, TrackKind};
