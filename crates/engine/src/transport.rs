//! The transport's commands in the command registry (WP-1.5, decisions D16
//! and D17).
//!
//! Play, Stop, Continue Playback, the insert marker, and the rest of
//! Live's Playback menu are registered commands with an ID, a label, a
//! description written for a model, a permission class, contexts, default
//! bindings, and argument and result schemas. [`register`] adds them to a
//! host's registry, and [`command_from_args`] turns an ID and its JSON
//! arguments into the [`TransportCommand`] the host sends to the engine
//! ([`Engine::transport`](crate::Engine::transport),
//! [`Sender::transport`](crate::Sender::transport)). The engine runs it at
//! the start of its next block, or on a frame
//! ([`Sender::transport_at`](crate::Sender::transport_at)).
//!
//! The Arrangement loop is part of the document, so its two commands,
//! `transport.set_loop` and `transport.set_loop_enabled`, are document
//! commands that `bl_model::registry` registers; the engine follows the
//! document's loop through [`Compiler::sync`](crate::Compiler::sync).
//!
//! The rules behind every command are in the transport spec.
//!
//! ```
//! use bl_commands::Registry;
//! use bl_engine::{TransportCommand, transport};
//! use serde_json::json;
//!
//! let mut commands = Registry::new();
//! transport::register(&mut commands)?;
//! assert!(commands.get("transport.play").is_some());
//!
//! let command = transport::command_from_args("transport.locate", json!({ "position": 705_600 }))?;
//! assert_eq!(command, TransportCommand::Locate(bl_core::time::Ticks::QUARTER));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use core::fmt;

use bl_commands::Context::{self, Arrangement, Global};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::message::TransportCommand;

/// The arguments of `transport.locate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocateArgs {
    /// The new insert marker, in ticks from 1.1.1. A position before 1.1.1
    /// is taken as 1.1.1.
    pub position: Ticks,
}

/// Adds the transport's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for entry in ENTRIES {
        registry.register(entry.spec())?;
    }
    Ok(())
}

/// Whether `id` is one of the commands [`register`] adds, so a host knows
/// to send it to [`command_from_args`].
#[must_use]
pub fn handles(id: &str) -> bool {
    ENTRIES.iter().any(|entry| entry.id == id)
}

/// The [`TransportCommand`] a transport command's ID and JSON arguments
/// describe, for the host to send to the engine.
///
/// # Errors
///
/// [`RunError::UnknownCommand`] if `id` isn't a transport command, and
/// [`RunError::InvalidArgs`] if the arguments don't fit its schema.
pub fn command_from_args(id: &str, args: Value) -> Result<TransportCommand, RunError> {
    let entry = ENTRIES
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| RunError::UnknownCommand(id.to_owned()))?;
    let invalid = |error: serde_json::Error| RunError::InvalidArgs {
        command: entry.id,
        message: error.to_string(),
    };
    match entry.action {
        Action::Run(command) => {
            serde_json::from_value::<NoArgs>(args).map_err(invalid)?;
            Ok(command)
        }
        Action::Locate => {
            let LocateArgs { position } = serde_json::from_value(args).map_err(invalid)?;
            Ok(TransportCommand::Locate(position))
        }
    }
}

/// Why [`command_from_args`] refused a command.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunError {
    /// No transport command has this ID.
    UnknownCommand(String),
    /// The arguments don't fit the command's schema.
    InvalidArgs {
        /// The command.
        command: &'static str,
        /// What's wrong, from the deserialiser.
        message: String,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(id) => write!(f, "no transport command is called `{id}`"),
            Self::InvalidArgs { command, message } => {
                write!(f, "invalid arguments for `{command}`: {message}")
            }
        }
    }
}

impl std::error::Error for RunError {}

/// What running a registered command sends.
#[derive(Clone, Copy)]
enum Action {
    /// This command, which takes no arguments.
    Run(TransportCommand),
    /// [`TransportCommand::Locate`], at the position in [`LocateArgs`].
    Locate,
}

/// One registered command.
struct Entry {
    id: &'static str,
    action: Action,
    label: &'static str,
    contexts: &'static [Context],
    /// Shortcuts that run it, and for `transport.locate`, the position the
    /// shortcut fixes.
    keys: &'static [(&'static str, Option<i64>)],
    description: &'static str,
}

impl Entry {
    fn spec(&self) -> CommandSpec {
        // Playback changes nothing in the project or on disk, so it's
        // `read` (D17's permission classes): an agent plays and stops
        // without asking.
        let mut spec = CommandSpec::new(self.id, self.label)
            .description(self.description)
            .permission(Permission::Read)
            .contexts(self.contexts.iter().copied())
            .result::<()>();
        spec = match self.action {
            Action::Run(_) => spec.args::<NoArgs>(),
            Action::Locate => spec.args::<LocateArgs>(),
        };
        for &(shortcut, position) in self.keys {
            let binding = Binding::new(shortcut);
            spec = spec.binding(match position {
                Some(position) => binding.arg("position", position),
                None => binding,
            });
        }
        spec
    }
}

/// Every transport command this crate registers. The rules behind them,
/// and behind their bindings, are in the transport spec.
const ENTRIES: &[Entry] = &[
    Entry {
        id: "transport.play",
        action: Action::Run(TransportCommand::Play),
        label: "Play",
        contexts: &[Global],
        keys: &[],
        description: "Starts playback from the insert marker, as the Control Bar's Play button \
            does; while the song plays, starts it again from the insert marker. Set the insert \
            marker first with `transport.locate`. With the Arrangement loop on, playback that \
            reaches the loop's end goes on from its start. It changes nothing in the project.",
    },
    Entry {
        id: "transport.stop",
        action: Action::Run(TransportCommand::Stop),
        label: "Stop",
        contexts: &[Global],
        keys: &[],
        description: "Stops playback, as the Control Bar's Stop button does, and keeps the \
            position where it stopped, so `transport.continue` can resume there. Run it again \
            while stopped to return the insert marker and the play position to the start of \
            the arrangement (1.1.1), as pressing Stop twice does. It changes nothing in the \
            project.",
    },
    Entry {
        id: "transport.play_stop",
        action: Action::Run(TransportCommand::PlayStop),
        label: "Play/Stop",
        contexts: &[Global],
        keys: &[("S-TRANS-001", None)],
        description: "Stops playback if the song is playing, and otherwise starts it from the \
            insert marker: the space bar, and the Playback menu's Play item, which reads Stop \
            while the song plays. Unlike `transport.stop`, it never returns to 1.1.1. An agent \
            that knows what it wants should use `transport.play` or `transport.stop` instead.",
    },
    Entry {
        id: "transport.continue",
        action: Action::Run(TransportCommand::Continue),
        label: "Continue Playback",
        contexts: &[Global],
        keys: &[("S-TRANS-002", None)],
        description: "Resumes playback from where it last stopped rather than from the insert \
            marker. It does nothing while the song plays. It changes nothing in the project.",
    },
    Entry {
        id: "transport.play_from_insert_marker",
        action: Action::Run(TransportCommand::Play),
        label: "Play From Insert Marker",
        contexts: &[Global, Arrangement],
        keys: &[("S-ARR-032", None)],
        description: "Starts playback from the insert marker. Until the arrangement has time \
            selections, this does what `transport.play` does. It changes nothing in the \
            project.",
    },
    Entry {
        id: "transport.locate",
        action: Action::Locate,
        label: "Set Arrangement Position",
        contexts: &[Global],
        keys: &[("S-TRANS-005", Some(0))],
        description: "Moves the insert marker, where playback starts, to `position`, in ticks \
            from the start of the arrangement (705,600 to the quarter note, so bar 2 of a 4/4 \
            song is 2,822,400), as typing into the Control Bar's Arrangement Position fields \
            does. While the song plays, playback jumps there at once; while it's stopped, \
            `transport.continue` still resumes where playback stopped. Position 0 returns to \
            1.1.1, as the Home key and Return Play Position to 1.1.1 do. A position before 0 \
            is taken as 0. It changes nothing in the project.",
    },
    Entry {
        id: "transport.insert_marker_to_playhead",
        action: Action::Run(TransportCommand::InsertMarkerToPlayhead),
        label: "Move Insert Marker To Playhead",
        contexts: &[Global, Arrangement],
        keys: &[("S-ARR-033", None)],
        description: "Moves the insert marker to the playhead: where the song is playing now, \
            or where playback stopped. Use it to start again later from the current point. It \
            changes nothing in the project.",
    },
];

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_entry_registers_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), ENTRIES.len());
        assert_eq!(registry.problems(), Vec::new());
        assert!(ENTRIES.iter().all(|entry| handles(entry.id)));
        assert!(!handles("transport.set_loop"), "a document command");
    }

    #[test]
    fn arguments_are_checked() {
        assert_eq!(
            command_from_args("transport.play", json!({})),
            Ok(TransportCommand::Play)
        );
        assert!(matches!(
            command_from_args("transport.play", json!({ "from": 0 })),
            Err(RunError::InvalidArgs { .. })
        ));
        assert!(matches!(
            command_from_args("transport.locate", json!({})),
            Err(RunError::InvalidArgs { .. })
        ));
        assert!(matches!(
            command_from_args("transport.locate", json!({ "position": "1.1.1" })),
            Err(RunError::InvalidArgs { .. })
        ));
        assert_eq!(
            command_from_args("transport.rewind", json!({})),
            Err(RunError::UnknownCommand("transport.rewind".to_owned()))
        );
    }
}
