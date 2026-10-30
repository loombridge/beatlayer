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

use crate::message::{NudgeDirection, TransportCommand};

/// The arguments of `transport.locate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocateArgs {
    /// The new insert marker, in ticks from 1.1.1. A position before 1.1.1
    /// is taken as 1.1.1.
    pub position: Ticks,
    /// Whether the jump waits for the next line of the global launch
    /// quantisation while the song plays, as a locator's jump and a click in
    /// the scrub area do (the transport spec, R22). With it off, or with
    /// the quantisation off or None, the jump happens at once.
    #[serde(default)]
    pub quantized: bool,
}

/// The arguments of `transport.scrub`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrubArgs {
    /// The point pressed or clicked in the scrub area, in ticks from the
    /// start of the arrangement.
    pub position: Ticks,
    /// Whether the mouse button is held: loops the span at `position`
    /// until a call with `held` false ends it.
    #[serde(default)]
    pub held: bool,
}

/// The arguments of `transport.phase_nudge`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhaseNudgeArgs {
    /// Which button: `down` plays the song 4 % slower while held, `up` 4 % faster.
    pub direction: NudgeDirection,
    /// Whether the button is held: true on the press, false on the release.
    pub held: bool,
}

/// The arguments of `transport.set_recording`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRecordingArgs {
    /// Whether the transport is recording.
    pub recording: bool,
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
            let LocateArgs {
                position,
                quantized,
            } = serde_json::from_value(args).map_err(invalid)?;
            Ok(if quantized {
                TransportCommand::QuantizedLocate(position)
            } else {
                TransportCommand::Locate(position)
            })
        }
        Action::Scrub => {
            let ScrubArgs { position, held } = serde_json::from_value(args).map_err(invalid)?;
            Ok(TransportCommand::Scrub {
                point: position,
                held,
            })
        }
        Action::PhaseNudge => {
            let PhaseNudgeArgs { direction, held } =
                serde_json::from_value(args).map_err(invalid)?;
            Ok(TransportCommand::PhaseNudge { direction, held })
        }
        Action::SetRecording => {
            let SetRecordingArgs { recording } = serde_json::from_value(args).map_err(invalid)?;
            Ok(TransportCommand::SetRecording(recording))
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
    /// [`TransportCommand::Scrub`], with [`ScrubArgs`].
    Scrub,
    /// [`TransportCommand::PhaseNudge`], with [`PhaseNudgeArgs`].
    PhaseNudge,
    /// [`TransportCommand::SetRecording`], with [`SetRecordingArgs`].
    SetRecording,
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
        // without asking. Recording captures an input device, so it's
        // `external`; `transport.stop_recording` is how an agent stops free.
        let permission = match self.action {
            Action::SetRecording => Permission::External,
            _ => Permission::Read,
        };
        let mut spec = CommandSpec::new(self.id, self.label)
            .description(self.description)
            .permission(permission)
            .contexts(self.contexts.iter().copied())
            .result::<()>();
        spec = match self.action {
            Action::Run(_) => spec.args::<NoArgs>(),
            Action::Locate => spec.args::<LocateArgs>(),
            Action::Scrub => spec.args::<ScrubArgs>(),
            Action::PhaseNudge => spec.args::<PhaseNudgeArgs>(),
            Action::SetRecording => spec.args::<SetRecordingArgs>(),
        };
        if self.id == "transport.set_recording" {
            for &(shortcut, argument) in TOGGLES {
                spec = spec.binding(Binding::new(shortcut).toggles(argument));
            }
        }
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

/// The shortcuts that switch a boolean argument of `transport.set_recording`
/// to the other state: F9, Record to Arrangement (the transport spec, R30).
const TOGGLES: &[(&str, &str)] = &[("S-TRANS-006", "recording")];

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
            does; while the song plays, starts it again from the insert marker. When the song \
            has a song start time (the transport spec, R21), it starts from that instead and \
            leaves the insert marker where it is. Set the insert marker first with \
            `transport.locate`. With the Arrangement loop on, playback that reaches the loop's \
            end goes on from its start. It changes nothing in the project.",
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
        action: Action::Run(TransportCommand::PlayFromInsertMarker),
        label: "Play From Insert Marker",
        contexts: &[Global, Arrangement],
        keys: &[("S-ARR-032", None)],
        description: "Starts playback from the insert marker, even when the song has a song \
            start time (the transport spec, R21). Until the arrangement has time selections, \
            this does what `transport.play` does. It changes nothing in the project.",
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
            `transport.continue` still resumes where playback stopped. With `quantized` set, \
            the jump waits while the song plays for the next line of the global launch \
            quantisation and lands on that line's exact frame, as a locator's jump and a click \
            in the scrub area do (the transport spec, R22); with the quantisation off or None, \
            it jumps at once. Position 0 returns to 1.1.1, as the Home key and Return Play \
            Position to 1.1.1 do. A position before 0 is taken as 0. It changes nothing in the \
            project.",
    },
    Entry {
        id: "transport.scrub",
        action: Action::Scrub,
        label: "Scrub",
        contexts: &[Global],
        keys: &[],
        description: "Presses or releases the Arrangement's scrub area at `position`, in ticks \
            from the start of the arrangement. With `held` true it loops one step of the global \
            launch quantisation (a bar with the quantisation off or None) from the grid line at \
            or before `position`, until you call it again with `held` false: while the song \
            plays, playback jumps to the span's start on the next quantisation line, and while \
            it's stopped, playback starts there. The Arrangement loop is left as it is and \
            applies again afterwards. With `held` false and a loop held, it ends that loop and \
            playback carries on from where it is; with none held, it plays from `position`, \
            as a click in the scrub area does (the jump waits for the quantisation line while \
            the song plays; while it's stopped, playback starts there). It changes nothing in \
            the project.",
    },
    Entry {
        id: "transport.phase_nudge",
        action: Action::PhaseNudge,
        label: "Phase Nudge",
        contexts: &[Global],
        keys: &[],
        description: "Presses or releases the Control Bar's Phase Nudge Down or Up button. While \
            `direction` `down` or `up` is held (`held` true) and the song plays, the song plays \
            4 % slower or faster than the tempo it plays at otherwise, to bring it into phase \
            with another player; with both held, the one pressed last nudges. Call it again with \
            `held` false to end the nudge: the next block plays at the tempo again, the song's \
            beat staying where the nudge moved it. A nudge you leave held ends when your agent \
            session ends. While the song is stopped a nudge does nothing, and while an incoming \
            clock or a Link session leads the tempo, a press is refused and only a release \
            is accepted. The Tempo field, the tempo map and \
            the set don't change, and it is no undo step. `transport.state`'s `phase_nudge` \
            reads which button is held.",
    },
    Entry {
        id: "transport.return_to_start",
        action: Action::Run(TransportCommand::Locate(Ticks::ZERO)),
        label: "Return Play Position to 1.1.1",
        contexts: &[Global],
        keys: &[],
        description: "Moves the insert marker, where playback starts, back to the start of the \
            arrangement (1.1.1), as the Playback menu's Return Play Position to 1.1.1 does. \
            While the song plays, playback jumps there at once; while it's stopped, \
            `transport.continue` still resumes where playback stopped. The same as \
            `transport.locate` at position 0, which the Home key runs. It changes nothing in \
            the project.",
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
    Entry {
        id: "transport.resync_external_hardware",
        action: Action::Run(TransportCommand::ResyncExternalHardware),
        label: "Resync External Hardware",
        contexts: &[Global],
        keys: &[],
        description: "Sends each output port with Sync on its own Hardware Resync choice, as \
            the Control Bar's Resync External Hardware button does: Stop then Start, Start \
            only, or nothing. While the song plays they go out at the next bar line, just \
            before that bar line's clock, so hardware that has drifted starts again in step on \
            the bar; while the song is stopped, or counting in, it does nothing. Set the \
            choices with `midi.set_port`. It changes nothing in the project.",
    },
    Entry {
        id: "transport.set_recording",
        action: Action::SetRecording,
        label: "Set Recording",
        contexts: &[Global],
        keys: &[],
        description: "Sets whether the transport is recording: while it is, each armed audio \
            track records its input into the project's Samples/Recorded/, each armed MIDI track \
            records the notes it receives, and the recording becomes a clip on its track when \
            it ends; a count-in starts it a bar or more later. With MIDI Arrangement Overdub on, \
            recorded notes merge into the MIDI clips already there. Features like Hear \
            Metronome Only While Recording follow it too. Recording captures input devices, \
            including a microphone, so the person is asked first, whichever way you set it: \
            to stop a recording without asking, run `transport.stop_recording`.",
    },
    Entry {
        id: "transport.stop_recording",
        action: Action::Run(TransportCommand::SetRecording(false)),
        label: "Stop Recording",
        contexts: &[Global],
        keys: &[],
        description: "Stops recording, the same as `transport.set_recording` with `recording` \
            false, and without asking the person. The recording becomes a clip on its track; \
            playback goes on. It does nothing when the transport isn't recording.",
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
            command_from_args("transport.scrub", json!({ "position": 0, "held": true })),
            Ok(TransportCommand::Scrub {
                point: Ticks::ZERO,
                held: true
            })
        );
        assert_eq!(
            command_from_args("transport.scrub", json!({ "position": 0 })),
            Ok(TransportCommand::Scrub {
                point: Ticks::ZERO,
                held: false
            })
        );
        assert_eq!(
            command_from_args(
                "transport.phase_nudge",
                json!({ "direction": "up", "held": true })
            ),
            Ok(TransportCommand::PhaseNudge {
                direction: NudgeDirection::Up,
                held: true
            })
        );
        assert!(matches!(
            command_from_args(
                "transport.phase_nudge",
                json!({ "direction": "left", "held": true })
            ),
            Err(RunError::InvalidArgs { .. })
        ));
        assert!(matches!(
            command_from_args("transport.phase_nudge", json!({ "direction": "down" })),
            Err(RunError::InvalidArgs { .. })
        ));
        assert_eq!(
            command_from_args("transport.rewind", json!({})),
            Err(RunError::UnknownCommand("transport.rewind".to_owned()))
        );
    }
}
