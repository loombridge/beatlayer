//! The document's commands in the command registry (WP-0.14, decisions D16
//! and D17).
//!
//! Every [`Command`] variant, and the editor's undo and redo, is a
//! registered command with an ID, a label, a description written for a
//! model, a permission class, contexts, default bindings, and argument and
//! result schemas. [`register`] adds them to a host's registry, and [`run`]
//! runs one by ID with JSON arguments, which is how menus, shortcuts, key
//! and MIDI maps, and agents reach them.
//!
//! A command's arguments are the variant's fields, as a JSON object without
//! the `op` tag; its schema is cut out of the schema of [`Command`] itself,
//! so the two can't disagree. Every document command returns the
//! [`ChangeSet`] it caused.
//!
//! ```
//! use bl_commands::Registry;
//! use bl_model::{Editor, TrackId, registry};
//! use serde_json::json;
//!
//! let mut commands = Registry::new();
//! registry::register(&mut commands)?;
//! assert!(commands.get("track.create").is_some());
//!
//! let mut editor = Editor::new();
//! let id: TrackId = editor.new_id();
//! let changes = registry::run(
//!     &mut editor,
//!     "track.create",
//!     json!({ "id": id, "kind": "midi", "name": "Bass", "index": 0 }),
//! )?;
//! assert!(changes.layout);
//! assert_eq!(editor.document().track(id).unwrap().name(), "Bass");
//!
//! registry::run(&mut editor, "edit.undo", json!({}))?;
//! assert!(editor.document().track(id).is_none());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use core::fmt;

use bl_commands::Context::{
    self, Arrangement, Browser, ClipView, DeviceView, Global, Session, TrackHeaders,
};
use bl_commands::{
    Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry, args_schema, result_schema,
    tagged_variant,
};
use schemars::Schema;
use serde_json::Value;

use crate::changes::ChangeSet;
use crate::command::Command;
use crate::editor::{EditError, Editor};

/// The ID of the command that undoes the newest step.
pub const UNDO: &str = "edit.undo";
/// The ID of the command that redoes the newest undone step.
pub const REDO: &str = "edit.redo";

/// Adds every document command, and undo and redo, to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    let commands = args_schema::<Command>();
    let changes = result_schema::<ChangeSet>();
    for entry in ENTRIES {
        registry.register(entry.spec(&commands, &changes))?;
    }
    Ok(())
}

/// Whether `id` is one of the commands [`register`] adds, so a host knows
/// to send it to [`run`].
#[must_use]
pub fn handles(id: &str) -> bool {
    ENTRIES.iter().any(|entry| entry.id == id)
}

/// Runs a command by its registry ID with JSON arguments, as a person's
/// shortcut or an agent does, and returns what it changed.
///
/// A document command is applied through [`Editor::apply`], so it's one
/// undo step unless a transaction is open. `edit.undo` and `edit.redo`
/// take no arguments: send `{}`.
///
/// # Errors
///
/// [`RunError::UnknownCommand`] if `id` isn't a document command,
/// [`RunError::InvalidArgs`] if the arguments don't fit its schema, and
/// [`RunError::Edit`] if the editor refuses it. The document is unchanged
/// in every case.
pub fn run(editor: &mut Editor, id: &str, args: Value) -> Result<ChangeSet, RunError> {
    let entry = entry(id)?;
    match entry.action {
        Action::Apply(_) => Ok(editor.apply(command_from_args(id, args)?)?),
        Action::Undo => {
            no_args(entry.id, args)?;
            Ok(editor.undo()?)
        }
        Action::Redo => {
            no_args(entry.id, args)?;
            Ok(editor.redo()?)
        }
    }
}

/// The [`Command`] a document command's ID and JSON arguments describe,
/// without applying it: for checking a batch before running it, or for
/// running it inside a transaction.
///
/// # Errors
///
/// [`RunError::UnknownCommand`] if `id` isn't a document command,
/// [`RunError::NotADocumentEdit`] for `edit.undo` and `edit.redo`, and
/// [`RunError::InvalidArgs`] if the arguments don't fit.
pub fn command_from_args(id: &str, args: Value) -> Result<Command, RunError> {
    let entry = entry(id)?;
    let Action::Apply(op) = entry.action else {
        return Err(RunError::NotADocumentEdit(entry.id));
    };
    let Value::Object(mut fields) = args else {
        return Err(RunError::InvalidArgs {
            command: entry.id,
            message: format!("the arguments must be a JSON object, not {args}"),
        });
    };
    fields.insert("op".to_owned(), Value::from(op));
    serde_json::from_value(Value::Object(fields)).map_err(|error| RunError::InvalidArgs {
        command: entry.id,
        message: error.to_string(),
    })
}

fn entry(id: &str) -> Result<&'static Entry, RunError> {
    ENTRIES
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| RunError::UnknownCommand(id.to_owned()))
}

fn no_args(command: &'static str, args: Value) -> Result<(), RunError> {
    serde_json::from_value::<NoArgs>(args)
        .map(|_| ())
        .map_err(|error| RunError::InvalidArgs {
            command,
            message: error.to_string(),
        })
}

/// Why [`run`] or [`command_from_args`] refused a command.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum RunError {
    /// No document command has this ID.
    UnknownCommand(String),
    /// The command is undo or redo, which isn't a [`Command`].
    NotADocumentEdit(&'static str),
    /// The arguments don't fit the command's schema.
    InvalidArgs {
        /// The command.
        command: &'static str,
        /// What's wrong, from the deserialiser.
        message: String,
    },
    /// The editor refused the command; the error says why.
    Edit(EditError),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(id) => write!(f, "no document command is called `{id}`"),
            Self::NotADocumentEdit(id) => {
                write!(f, "`{id}` isn't a document edit; run it by ID instead")
            }
            Self::InvalidArgs { command, message } => {
                write!(f, "invalid arguments for `{command}`: {message}")
            }
            Self::Edit(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Edit(error) => Some(error),
            _ => None,
        }
    }
}

impl From<EditError> for RunError {
    fn from(error: EditError) -> Self {
        Self::Edit(error)
    }
}

/// What running a registered command does.
#[derive(Clone, Copy)]
enum Action {
    /// Applies the [`Command`] variant whose `op` tag this is.
    Apply(&'static str),
    /// Undoes the newest step.
    Undo,
    /// Redoes the newest undone step.
    Redo,
}

/// A default binding, in a form a constant can hold.
#[derive(Clone, Copy)]
enum Key {
    /// The shortcut runs the command.
    Runs(&'static str),
    /// The shortcut runs it with a string argument fixed: (shortcut,
    /// argument, value).
    Fixes(&'static str, &'static str, &'static str),
    /// The shortcut toggles a boolean argument: (shortcut, argument).
    Toggles(&'static str, &'static str),
}

/// One registered command.
struct Entry {
    id: &'static str,
    action: Action,
    label: &'static str,
    permission: Permission,
    contexts: &'static [Context],
    keys: &'static [Key],
    description: &'static str,
}

impl Entry {
    fn spec(&self, commands: &Schema, changes: &Schema) -> CommandSpec {
        let mut spec = CommandSpec::new(self.id, self.label)
            .description(self.description)
            .permission(self.permission)
            .contexts(self.contexts.iter().copied())
            .result_schema(changes.clone());
        spec = match self.action {
            // A tag missing from the enum's schema leaves the spec without
            // arguments, which the registry reports.
            Action::Apply(op) => match tagged_variant(commands, "op", op) {
                Some(args) => spec.args_schema(args),
                None => spec,
            },
            Action::Undo | Action::Redo => spec.args::<NoArgs>(),
        };
        for key in self.keys {
            spec = spec.binding(match *key {
                Key::Runs(shortcut) => Binding::new(shortcut),
                Key::Fixes(shortcut, arg, value) => Binding::new(shortcut).arg(arg, value),
                Key::Toggles(shortcut, arg) => Binding::new(shortcut).toggles(arg),
            });
        }
        spec
    }
}

/// Where a person reaches a track's mixer switches: its title bar, and the
/// mixer in either view.
const MIXER: &[Context] = &[TrackHeaders, Session, Arrangement];

/// Every command this crate registers. The rules behind the bindings are in
/// `docs/specs/command-bindings.md`.
const ENTRIES: &[Entry] = &[
    Entry {
        id: "song.set_tempo",
        action: Action::Apply("set_tempo"),
        label: "Set Tempo",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Replaces the song's tempo map. To give the whole song one tempo, send a \
            single constant segment at tick 0; add segments to change or ramp the tempo at \
            given positions. Tempos are quarter notes per minute, from 1 to 1,000. Clips and \
            notes keep their positions in ticks, so a new tempo changes when they sound, not \
            where they sit in bars.",
    },
    Entry {
        id: "song.set_time_signatures",
        action: Action::Apply("set_time_signatures"),
        label: "Set Time Signatures",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Replaces the song's time-signature map. To give the whole song one \
            meter, send a single change at tick 0, such as 4/4; add changes to switch meter at \
            given positions, each of which starts a new bar. Numerators run from 1 to 99 and \
            denominators are 1, 2, 4, 8, or 16. It changes how positions are counted in bars \
            and beats, not where clips sit in ticks.",
    },
    Entry {
        id: "transport.set_loop",
        action: Action::Apply("set_loop"),
        label: "Set Loop Brace",
        permission: Permission::Edit,
        contexts: &[Global, Arrangement],
        keys: &[],
        description: "Moves the Arrangement loop brace to start at `start` and run for `length` \
            ticks (705,600 to the quarter note; a 4/4 bar is 2,822,400), as the Control Bar's \
            Loop Start and Loop Length fields do. It doesn't switch the loop on or off: use \
            `transport.set_loop_enabled` for that. While the loop is on, playback that reaches \
            the brace's end jumps back to its start; playback that starts at or after the end \
            plays on. The start must be 0 (1.1.1) or later and the length more than 0. The \
            brace is saved with the set, and undo moves it back.",
    },
    Entry {
        id: "transport.set_loop_enabled",
        action: Action::Apply("set_loop_enabled"),
        label: "Arrangement Loop On/Off",
        permission: Permission::Edit,
        contexts: &[Global, Arrangement],
        keys: &[],
        description: "Switches the Arrangement loop on or off, as the Control Bar's Loop switch \
            does, leaving the brace where it is. With it on, playback that reaches the brace's \
            end jumps back to its start, on the exact sample, until it's switched off or \
            playback stops. Use `transport.set_loop` to move the brace first. The switch is \
            saved with the set, and undo switches it back.",
    },
    Entry {
        id: "track.create",
        action: Action::Apply("create_track"),
        label: "Insert Track",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[
            Key::Fixes("S-TRACK-001", "kind", "audio"),
            Key::Fixes("S-TRACK-002", "kind", "midi"),
            Key::Fixes("S-TRACK-003", "kind", "return"),
        ],
        description: "Creates an empty track: audio (holds audio clips), MIDI (holds MIDI \
            clips), group (holds other tracks), or return (receives sends). It starts at unity \
            volume and centre pan, with no devices or clips. Audio, MIDI, and group tracks go \
            at `index` among the top-level tracks, or inside the group `parent`; return tracks \
            go at `index` among the returns. `id` must be a new ID the project has issued for \
            it. The Main track always exists and can't be created.",
    },
    Entry {
        id: "track.delete",
        action: Action::Apply("delete_track"),
        label: "Delete Track",
        permission: Permission::Edit,
        contexts: &[TrackHeaders],
        keys: &[Key::Runs("S-TRACK-018")],
        description: "Deletes a track with its clips and devices. Deleting a group track also \
            deletes every track inside it. Use it to remove a track the song no longer needs; \
            to silence a track and keep it, use `mixer.set_mute`. Undo restores everything it \
            deleted. The Main track can't be deleted.",
    },
    Entry {
        id: "track.move",
        action: Action::Apply("move_track"),
        label: "Move Track",
        permission: Permission::Edit,
        contexts: &[TrackHeaders],
        keys: &[],
        description: "Moves a track to `index` in the top-level track list, or inside the \
            group `parent`. A return track moves among the returns. The index counts positions \
            after the track is taken out of its current place, so moving a track down past its \
            neighbour is its current index plus one. Use it to reorder tracks or to put tracks \
            into or out of groups. A group can't go inside itself, and the Main track can't be \
            moved.",
    },
    Entry {
        id: "track.rename",
        action: Action::Apply("rename_track"),
        label: "Rename Track",
        permission: Permission::Edit,
        contexts: &[TrackHeaders],
        keys: &[Key::Runs("S-TRACK-004")],
        description: "Renames a track. A name may be empty, and tracks may share a name. It \
            changes only the name in the track's title bar.",
    },
    Entry {
        id: "mixer.set_volume",
        action: Action::Apply("set_volume"),
        label: "Set Track Volume",
        permission: Permission::Edit,
        contexts: &[Session, Arrangement],
        keys: &[],
        description: "Sets a track's volume as a linear gain: 1 is unity (0 dB), 0.5 is about \
            -6 dB (10^(dB/20) converts decibels), 2 is about +6 dB, and 0 is silence (-inf \
            dB). Use it to balance tracks; the Main track's volume sets the song's output \
            level. The engine glides to the new level over 20 ms, so a change never clicks. \
            The value must be finite and at least 0.",
    },
    Entry {
        id: "mixer.set_pan",
        action: Action::Apply("set_pan"),
        label: "Set Track Pan",
        permission: Permission::Edit,
        contexts: &[Session, Arrangement],
        keys: &[],
        description: "Sets a track's pan from -1 (fully left) through 0 (centre) to 1 (fully \
            right). Use it to place a track in the stereo field. Panning keeps the power \
            constant: at the centre both channels pass at 0 dB, and fully to one side that \
            side is 3 dB louder and the other silent.",
    },
    Entry {
        id: "mixer.set_mute",
        action: Action::Apply("set_mute"),
        label: "Activate/Deactivate Track",
        permission: Permission::Edit,
        contexts: MIXER,
        keys: &[
            Key::Toggles("S-TRACK-016", "mute"),
            Key::Toggles("S-TRANS-010", "mute"),
        ],
        description: "Switches a track's activator: `mute` true deactivates the track, so its \
            output and its post-fader sends are silent (its pre-fader sends still carry it), \
            and false activates it again. Use it to silence a track without deleting it. \
            Every track but Main has an activator.",
    },
    Entry {
        id: "mixer.set_solo",
        action: Action::Apply("set_solo"),
        label: "Solo Track",
        permission: Permission::Edit,
        contexts: MIXER,
        keys: &[Key::Toggles("S-TRACK-014", "solo")],
        description: "Solos or unsolos one track, leaving other tracks' solo switches as they \
            are, as a Cmd-click (Ctrl-click) on a Solo switch does; to solo one track alone, \
            use `mixer.solo_exclusive`. While any track is soloed, only soloed tracks, the \
            groups that hold them, and the tracks inside soloed groups are heard. Return \
            tracks stay audible if Solo in Place is on (`mixer.set_solo_in_place`); soloing a \
            return silences every other track's output but keeps their sends to it. Every \
            track but Main can be soloed.",
    },
    Entry {
        id: "mixer.solo_exclusive",
        action: Action::Apply("solo_exclusive"),
        label: "Solo Track Exclusively",
        permission: Permission::Edit,
        contexts: MIXER,
        keys: &[],
        description: "Solos one track and unsolos every other track, in one undo step, as a \
            click on a Solo switch does while Exclusive Solo is on (Live's default). Use it to \
            hear one track on its own. To unsolo, use `mixer.set_solo` with `solo` false. Every \
            track but Main can be soloed.",
    },
    Entry {
        id: "mixer.set_arm",
        action: Action::Apply("set_arm"),
        label: "Arm Track",
        permission: Permission::Edit,
        contexts: MIXER,
        keys: &[Key::Toggles("S-TRACK-013", "arm")],
        description: "Arms or disarms one track for recording, leaving other tracks' arm \
            switches as they are, so it records its input when recording starts (recording \
            arrives in Phase 2; until then the switch is only saved). Only audio and MIDI \
            tracks can be armed.",
    },
    Entry {
        id: "mixer.arm_exclusive",
        action: Action::Apply("arm_exclusive"),
        label: "Arm Track Exclusively",
        permission: Permission::Edit,
        contexts: MIXER,
        keys: &[],
        description: "Arms one track and disarms every other track, in one undo step, as a \
            click on an Arm switch does while Exclusive Arm is on (Live's default). Only audio \
            and MIDI tracks can be armed.",
    },
    Entry {
        id: "mixer.set_send",
        action: Action::Apply("set_send"),
        label: "Set Send Level",
        permission: Permission::Edit,
        contexts: &[Session, Arrangement],
        keys: &[],
        description: "Sets how much of a track's signal its send feeds to the return track \
            `to`, as a linear gain from 0 (-inf dB, where every send starts) to 1 (0 dB). Use \
            it to put a track through a return's effects, such as a shared reverb. Audio, \
            MIDI, and group tracks have a send to every return; the return's Pre/Post switch \
            (`mixer.set_send_tap`) decides whether the send takes the track's signal before \
            or after its pan, volume, and activator.",
    },
    Entry {
        id: "mixer.set_send_tap",
        action: Action::Apply("set_send_tap"),
        label: "Set Sends Pre/Post",
        permission: Permission::Edit,
        contexts: &[Session, Arrangement],
        keys: &[],
        description: "Sets a return track's Pre/Post switch, which decides where every send to \
            that return takes its signal from each sending track: `post` (the default) after \
            the track's pan, volume, and activator, so the send follows the fader; `pre` \
            before them, straight after the track's devices, for a mix that ignores the \
            faders, such as a monitor mix. Only return tracks have it.",
    },
    Entry {
        id: "mixer.set_solo_in_place",
        action: Action::Apply("set_solo_in_place"),
        label: "Solo in Place",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Switches Solo in Place, the Options menu item. On (the default in a new \
            set), soloing an audio, MIDI, or group track keeps the return tracks audible, so \
            the soloed track is heard with its sends' effects; off, soloing such a track \
            silences the returns too. It doesn't change what soloing a return does. The \
            setting is saved with the set.",
    },
    Entry {
        id: "clip.create",
        action: Action::Apply("create_clip"),
        label: "Create Clip",
        permission: Permission::Edit,
        contexts: &[Arrangement],
        keys: &[],
        description: "Puts a new clip on an audio or MIDI track's arrangement lane, at \
            `clip.start` for `clip.length` ticks (705,600 to the quarter note). An audio clip \
            names its audio file by content hash; a MIDI clip carries its notes, placed from \
            the clip's start. Audio clips go only on audio tracks and MIDI clips only on MIDI \
            tracks. `clip.id` must be a new ID the project has issued for it.",
    },
    Entry {
        id: "clip.delete",
        action: Action::Apply("delete_clip"),
        label: "Delete Clip",
        permission: Permission::Edit,
        contexts: &[Arrangement],
        keys: &[Key::Runs("S-EDIT-005")],
        description: "Deletes a clip from the arrangement. Undo restores it. To keep a clip \
            but stop it playing, use `clip.set_mute`.",
    },
    Entry {
        id: "clip.move",
        action: Action::Apply("move_clip"),
        label: "Move Clip",
        permission: Permission::Edit,
        contexts: &[Arrangement],
        keys: &[],
        description: "Moves a clip so it starts at `start`, on the same track or another track \
            of the same kind (audio or MIDI). Its length and contents don't change. To move \
            several clips together, use `clip.move_by`.",
    },
    Entry {
        id: "clip.move_by",
        action: Action::Apply("move_clips"),
        label: "Move Clips",
        permission: Permission::Edit,
        contexts: &[Arrangement],
        keys: &[],
        description: "Moves several clips earlier or later by the same number of ticks, \
            keeping them on their tracks and keeping their spacing; a negative `by` moves them \
            earlier. Either every clip moves or, if any would start before 0, none does. List \
            each clip once.",
    },
    Entry {
        id: "clip.set_length",
        action: Action::Apply("set_clip_length"),
        label: "Resize Clip",
        permission: Permission::Edit,
        contexts: &[Arrangement],
        keys: &[],
        description: "Changes how long a clip is, in ticks, keeping its start and its \
            contents. The length must be positive.",
    },
    Entry {
        id: "clip.rename",
        action: Action::Apply("rename_clip"),
        label: "Rename Clip",
        permission: Permission::Edit,
        contexts: &[Arrangement],
        keys: &[Key::Runs("S-EDIT-008")],
        description: "Renames a clip. A name may be empty. It changes only the name shown on \
            the clip.",
    },
    Entry {
        id: "clip.set_mute",
        action: Action::Apply("set_clip_mute"),
        label: "Activate/Deactivate Clip",
        permission: Permission::Edit,
        contexts: &[Arrangement],
        keys: &[Key::Toggles("S-ARR-027", "mute")],
        description: "Switches a clip's activator: `mute` true deactivates the clip, so it \
            stays in place but doesn't play, and false activates it again.",
    },
    Entry {
        id: "clip.set_data",
        action: Action::Apply("set_clip_data"),
        label: "Replace Clip Contents",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Replaces what a clip plays: for an audio clip, the audio file it names \
            by content hash; for a MIDI clip, its whole list of notes. Use it to write or \
            change a MIDI clip's notes, sending every note the clip should hold. The data must \
            match the clip's track: audio on audio tracks, MIDI on MIDI tracks.",
    },
    Entry {
        id: "device.insert",
        action: Action::Apply("insert_device"),
        label: "Insert Device",
        permission: Permission::Edit,
        contexts: &[DeviceView, Browser],
        keys: &[],
        description: "Inserts a device into a track's device chain at `index`, where 0 is \
            first in the signal flow. The device is a native Beatlayer device named by its \
            type ID, or a plug-in named by its format and plug-in ID, with any parameter \
            values to set. Every kind of track has a chain. `device.id` must be a new ID the \
            project has issued for it.",
    },
    Entry {
        id: "device.remove",
        action: Action::Apply("remove_device"),
        label: "Delete Device",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[Key::Runs("S-EDIT-005")],
        description: "Deletes a device from its track's chain, with its settings. Undo \
            restores it. To bypass a device but keep it, use `device.set_enabled`.",
    },
    Entry {
        id: "device.move",
        action: Action::Apply("move_device"),
        label: "Move Device",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Moves a device to `index` in a track's chain, on the same track or \
            another, with its settings. The index counts positions after the device is taken \
            out of its current place.",
    },
    Entry {
        id: "device.rename",
        action: Action::Apply("rename_device"),
        label: "Rename Device",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[Key::Runs("S-EDIT-008")],
        description: "Renames a device. It changes only the name in the device's title bar.",
    },
    Entry {
        id: "device.set_enabled",
        action: Action::Apply("set_device_enabled"),
        label: "Activate/Deactivate Device",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Switches a device on or off. A device that's off passes its input \
            through unchanged, so you can hear the track with and without it.",
    },
    Entry {
        id: "device.set_param",
        action: Action::Apply("set_device_param"),
        label: "Set Device Parameter",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets one of a device's parameters to a value in the parameter's own \
            units. The value must be finite. Parameters that haven't been set keep their \
            defaults.",
    },
    Entry {
        id: UNDO,
        action: Action::Undo,
        label: "Undo",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[Key::Runs("S-EDIT-006")],
        description: "Undoes the newest step in the undo history: the last command or \
            gesture, however many changes it made. Use it to reverse a mistake; `edit.redo` \
            puts the step back. It fails if there's nothing to undo.",
    },
    Entry {
        id: REDO,
        action: Action::Redo,
        label: "Redo",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[Key::Runs("S-EDIT-007")],
        description: "Redoes the step most recently undone. It fails if nothing has been \
            undone since the last new change, because a new change discards the steps that \
            could have been redone.",
    },
];
