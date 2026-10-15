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
/// The ID of the command that turns every overridden envelope in the song
/// back on: the Control Bar's Re-Enable Automation button.
pub const RE_ENABLE_AUTOMATION: &str = "automation.re_enable";
/// The ID of the command that redoes the newest undone step.
pub const REDO: &str = "edit.redo";
/// The ID of `clip.set_midi_region`: a MIDI clip's Start and End markers.
pub const SET_MIDI_REGION: &str = "clip.set_midi_region";
/// The ID of `clip.set_midi_loop`: a MIDI clip's loop brace.
pub const SET_MIDI_LOOP: &str = "clip.set_midi_loop";

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
/// the command bindings spec.
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
        id: "song.set_tempo_breakpoint",
        action: Action::Apply("set_tempo_breakpoint"),
        label: "Set Tempo Breakpoint",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Sets the tempo at `at` on the Main track's Song Tempo envelope, adding a \
            breakpoint there or replacing the one that's there, as dragging a breakpoint does. \
            Tempos are quarter notes per minute, from 1 to 1,000, and `at` is 0 (1.1.1) or \
            later. A breakpoint between two others splits the segment there, keeping the \
            neighbours. Clips and notes keep their positions in ticks, so a new tempo changes \
            when they sound, not where they sit in bars. Use `song.set_tempo` to replace the \
            whole map at once.",
    },
    Entry {
        id: "song.move_tempo_breakpoint",
        action: Action::Apply("move_tempo_breakpoint"),
        label: "Move Tempo Breakpoint",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Moves the tempo breakpoint at `at` to `to`, keeping its tempo, as dragging \
            one along the Main track's Song Tempo envelope does. Both are tick positions, `to` \
            is 0 (1.1.1) or later, and it mustn't land on another breakpoint. The first \
            breakpoint, at 1.1.1, can't move; `at` there is refused.",
    },
    Entry {
        id: "song.delete_tempo_breakpoint",
        action: Action::Apply("delete_tempo_breakpoint"),
        label: "Delete Tempo Breakpoint",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Deletes the tempo breakpoint at `at` from the Main track's Song Tempo \
            envelope, joining its neighbours with a straight segment. The first breakpoint, at \
            1.1.1, can't be deleted; `at` there is refused. A position with no breakpoint is \
            refused too.",
    },
    Entry {
        id: "song.set_time_signature_change",
        action: Action::Apply("set_time_signature_change"),
        label: "Set Time Signature Change",
        permission: Permission::Edit,
        contexts: &[Global, Arrangement],
        keys: &[],
        description: "Sets the time-signature marker at `at` to `signature`, adding one when \
            there's none, as Create > Insert Time Signature Change and the marker's numerator \
            and denominator fields do. The marker sits anywhere at 0 (1.1.1) or later and \
            always starts a new bar there, so a change before the previous bar is full cuts it \
            short. Numerators run 1 to 99 and denominators are 1, 2, 4, 8, or 16. Use \
            `song.set_time_signatures` to replace the whole map.",
    },
    Entry {
        id: "song.move_time_signature_change",
        action: Action::Apply("move_time_signature_change"),
        label: "Move Time Signature Change",
        permission: Permission::Edit,
        contexts: &[Global, Arrangement],
        keys: &[],
        description: "Moves the time-signature marker at `at` to `to`, as dragging it along the \
            arrangement's time-signature lane does. Both are tick positions, `to` is 0 (1.1.1) \
            or later, and it mustn't land on another marker. The first marker, at 1.1.1, can't \
            move; `at` there is refused.",
    },
    Entry {
        id: "song.delete_time_signature_change",
        action: Action::Apply("delete_time_signature_change"),
        label: "Delete Time Signature Change",
        permission: Permission::Edit,
        contexts: &[Global, Arrangement],
        keys: &[],
        description: "Deletes the time-signature marker at `at`, so the signature before it runs \
            on through that position. The first marker, at 1.1.1, can't be deleted; `at` there \
            is refused. A position with no marker is refused too.",
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
        id: "transport.set_metronome_enabled",
        action: Action::Apply("set_metronome_enabled"),
        label: "Metronome",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[Key::Toggles("S-TRANS-011", "enabled")],
        description: "Switches the metronome on or off, as the Control Bar's Metronome switch and \
            the Playback menu's Metronome item do. When on, the metronome clicks on each beat of the \
            song, with the downbeat accented. The switch is saved with the set, and undo switches it back.",
    },
    Entry {
        id: "transport.set_metronome_count_in",
        action: Action::Apply("set_metronome_count_in"),
        label: "Metronome Count-In",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Sets the metronome count-in length: `none`, `1_bar`, `2_bars`, or `4_bars`. \
            When enabled, starting playback or recording plays the chosen bars of metronome clicks \
            before the transport moves. The setting is saved with the set, and undo restores it.",
    },
    Entry {
        id: "transport.set_metronome_sound",
        action: Action::Apply("set_metronome_sound"),
        label: "Metronome Sound",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Chooses the metronome's sound character: `classic`, `click`, or `wood`. \
            All sounds are synthesized in code. The setting is saved with the set, and undo restores it.",
    },
    Entry {
        id: "transport.set_metronome_rhythm",
        action: Action::Apply("set_metronome_rhythm"),
        label: "Metronome Rhythm",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Sets the rhythmic division the metronome clicks at: `auto` (follows the time \
            signature's denominator), `1_bar`, `1/2`, `1/2t`, `1/4`, `1/4t`, `1/8`, `1/8t`, `1/16`, \
            or `1/16t`. If a division exceeds the bar length under the current time signature, \
            clicks follow the auto interval. The setting is saved with the set, and undo restores it.",
    },
    Entry {
        id: "transport.set_metronome_only_while_recording",
        action: Action::Apply("set_metronome_only_while_recording"),
        label: "Hear Metronome Only While Recording",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Switches whether the metronome clicks only while recording or during playback as \
            well, as the Playback menu and Metronome Settings menu do. The setting is saved with the \
            set, and undo switches it back.",
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
        permission: Permission::Destructive,
        contexts: &[TrackHeaders],
        keys: &[Key::Runs("S-TRACK-018")],
        description: "Deletes a track with its clips and devices. Deleting a group track also \
            deletes every track inside it. Use it to remove a track the song no longer needs; \
            to silence a track and keep it, use `mixer.set_mute`. Undo restores everything it \
            deleted, but because it removes the person's work an agent asks first. The Main \
            track can't be deleted.",
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
        id: "automation.set_breakpoint",
        action: Action::Apply("set_automation_breakpoint"),
        label: "Set Automation Breakpoint",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Sets one of a track's automatable parameters at `at` on its envelope, \
            adding a breakpoint there or replacing the value of the one that's there, as \
            clicking or dragging a breakpoint on an automation lane does. `target` names the \
            parameter: `{\"type\": \"mixer\", \"param\": \"volume\"}`, `{\"type\": \"send\", \
            \"to\": <return track>}`, or `{\"type\": \"device\", \"device\": <device>, \
            \"param\": <parameter>}`. A parameter with no envelope yet gets its first \
            breakpoint and its envelope drives it from then on. Values are in the parameter's \
            own units: volume as linear gain, pan from -1 to 1, the Track Activator 1 for \
            muted, and a device parameter as the device's own value. `at` is 0 (1.1.1) or \
            later. The value between two breakpoints follows the segment's shape: the straight \
            line unless `automation.set_curve` bent it; before the first and after the last \
            the nearest one's value holds.",
    },
    Entry {
        id: "automation.set_curve",
        action: Action::Apply("set_automation_curve"),
        label: "Curve Automation Segment",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Curves one of a track's automation segments, as holding Alt (Windows) or \
            Option (macOS) while dragging a line segment on an automation lane does. `track` \
            and `target` name the parameter as `automation.set_breakpoint` does, and `at` is \
            the position of the breakpoint the segment starts at. `bend` is a signed number \
            from -1 to 1: 0 is the straight line, the sign chooses which side the segment bows \
            to, and its size how far. Between the two breakpoints the value is `from + (to - \
            from) * t^(2^bend)`, `t` running from 0 at the segment's start to 1 at its end, so \
            the value stays between the two breakpoints' values. A bend of 0 straightens the \
            segment again, as the same drag with a double-click does. Refused when no \
            breakpoint is at `at`, or when `at` is the last breakpoint, with no segment after \
            it. Beatlayer's own law: Live's curved shape isn't measured yet.",
    },
    Entry {
        id: "automation.move_breakpoint",
        action: Action::Apply("move_automation_breakpoint"),
        label: "Move Automation Breakpoint",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Moves one of a track's automation breakpoints from `at` to `to` and, with \
            `value`, to that value, as dragging it on an automation lane does. `to` is 0 (1.1.1) \
            or later and mustn't land on another breakpoint of the same envelope, and `value` is \
            in the parameter's own units. Without `value` the breakpoint keeps its value. A \
            position with no breakpoint is refused.",
    },
    Entry {
        id: "automation.delete_breakpoint",
        action: Action::Apply("delete_automation_breakpoint"),
        label: "Delete Automation Breakpoint",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Deletes one of a track's automation breakpoints, so the segments either \
            side of it become the one starting at the breakpoint before it, as clicking a \
            breakpoint on an automation lane does. Deleting the last one takes the parameter's \
            envelope with it, so the parameter follows its control again. A position with no \
            breakpoint is refused.",
    },
    Entry {
        id: "automation.delete",
        action: Action::Apply("delete_automation"),
        label: "Delete Automation",
        permission: Permission::Destructive,
        contexts: &[Global],
        keys: &[],
        description: "Deletes all of one of a track's parameters' automation: its envelope and \
            every breakpoint on it, as Delete Automation in the control's context menu does. \
            The control's value stays where it is and the parameter follows it again. Nothing \
            changes if the parameter has no automation.",
    },
    Entry {
        id: RE_ENABLE_AUTOMATION,
        action: Action::Apply("re_enable_automation"),
        label: "Re-Enable Automation",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Turns every overridden envelope in the song back on, as the Control Bar's \
            Re-Enable Automation button does: every automated control that a hand move turned \
            off follows its envelope again. It's one undo step, and nothing changes when no \
            envelope is overridden. Use `automation.re_enable_for` for one parameter.",
    },
    Entry {
        id: "automation.re_enable_for",
        action: Action::Apply("re_enable_automation_for"),
        label: "Re-Enable Automation for a Parameter",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Turns one parameter's overridden envelope back on, as Re-Enable \
            Automation in that control's context menu does. `track` and `target` name the \
            parameter the same way `automation.set_breakpoint` does. Nothing changes if the \
            parameter's envelope isn't overridden.",
    },
    Entry {
        id: "routing.set_input",
        action: Action::Apply("set_input"),
        label: "Set Input",
        permission: Permission::Edit,
        contexts: &[Session, Arrangement],
        keys: &[],
        description: "Sets where an audio or MIDI track takes its input from, as the In/Out \
            section's Audio From or MIDI From chooser and its channel do. An audio track can \
            take `external` (Ext. In, the audio interface's inputs, with a `channel`: a single \
            channel (`{ \"type\": \"mono\", \"channel\": 3 }`) or a stereo pair (`{ \"type\": \
            \"pair\", \"channel\": 3 }`, the pair 3/4), counted from 1; silent until the \
            interface has input), `resampling` (the Main output, for recording the mix; the \
            track's own output and sends are then silent so it can't feed back), `none`, or \
            `track` with another audio track, a group, a return track, or Main and a `tap`: \
            `pre_fx` (before its devices), `post_fx` (after its devices, before its mixer), or \
            `post_mixer` (its output, the default). A MIDI track can take `all_ins`, \
            `computer_keyboard`, `none`, or another MIDI track (MIDI reaches tracks once MIDI \
            playback is built). The input is heard only while the track monitors it \
            (`routing.set_monitor`). A routing may make a loop, as in Live; the engine plays \
            the routing that closes it as silence.",
    },
    Entry {
        id: "routing.set_monitor",
        action: Action::Apply("set_monitor"),
        label: "Set Monitor",
        permission: Permission::Edit,
        contexts: &[Session, Arrangement],
        keys: &[],
        description: "Sets an audio or MIDI track's Monitor switch: `in` always plays the \
            track's input through its devices and silences its clips, `auto` plays the input \
            while the track is armed, and `off` never plays it. A new track is `auto`. Use `in` to hear another track routed into this one.",
    },
    Entry {
        id: "routing.set_output",
        action: Action::Apply("set_output"),
        label: "Set Output",
        permission: Permission::Edit,
        contexts: &[Session, Arrangement],
        keys: &[],
        description: "Sets where a track's audio goes, as the In/Out section's Audio To chooser \
            does: `default` (Main, or the group the track is in), `main` (Main, even from \
            inside a group), `track` with another audio track (its input, heard while that \
            track monitors it) or the track's own group, `sends_only` (only through its sends \
            to the return tracks), or `external` (Ext. Out, which carries nothing until audio \
            outputs are built). Audio, group, and return tracks take any of these. A routing \
            may make a loop, as in Live; the engine then plays the routing that closes the \
            loop as silence.",
    },
    Entry {
        id: "routing.set_midi_output",
        action: Action::Apply("set_midi_output"),
        label: "Set MIDI Output",
        permission: Permission::Edit,
        contexts: &[Session, Arrangement],
        keys: &[],
        description: "Sets where a MIDI track's MIDI goes, as its MIDI To chooser does: \
            another MIDI track (its Track In), or `null` for No Output (a new MIDI track's).",
    },
    Entry {
        id: "routing.set_main_out",
        action: Action::Apply("set_main_out"),
        label: "Set Main Out",
        permission: Permission::Edit,
        contexts: &[Session, Arrangement],
        keys: &[],
        description: "Sets Main's Main Out, the audio interface's output channels the Main mix \
            goes to, as Main's In/Out section does: `channel` is a stereo pair of the \
            interface's outputs (`{ \"type\": \"pair\", \"channel\": 3 }` is the pair 3/4), \
            counted from 1. The default is the first pair, 1/2.",
    },
    Entry {
        id: "routing.set_cue_out",
        action: Action::Apply("set_cue_out"),
        label: "Set Cue Out",
        permission: Permission::Edit,
        contexts: &[Session, Arrangement],
        keys: &[],
        description: "Sets Main's Cue Out, the audio interface's output channels the cue mix \
            goes to, as Main's In/Out section does: `channel` is a stereo pair of the \
            interface's outputs, counted from 1; the default is the first pair, 1/2. It \
            carries the cue mix once cue mode is built (a later package); the choice is kept \
            and saved with the set.",
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
            names its audio file by content hash and plays it unwarped, at the file's own \
            speed, from the moment the song reaches the clip; its optional settings (Start \
            and End markers, loop, gain, fades) default to playing the whole file once at 0 \
            dB with no fades. A MIDI clip carries its notes, placed from the clip's start. \
            Audio clips go only on audio tracks and MIDI clips only on MIDI tracks. `clip.id` \
            must be a new ID the project has issued for it.",
    },
    Entry {
        id: "clip.delete",
        action: Action::Apply("delete_clip"),
        label: "Delete Clip",
        permission: Permission::Destructive,
        contexts: &[Arrangement],
        // The Delete key deletes the Arrangement's selection
        // (`arrangement.delete`, WP-1.8b), which may be clips or time.
        keys: &[],
        description: "Deletes a clip, from the Arrangement or from a track's Session slot. \
            Undo restores it, but because it removes the person's work an agent asks first. To \
            keep a clip but stop it playing, use `clip.set_mute`. To delete the clips or the \
            time selected in the Arrangement, use `arrangement.delete`.",
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
            by content hash with all its settings at once; for a MIDI clip, its whole list of \
            notes. Use it to write or change a MIDI clip's notes, sending every note the clip \
            should hold; for one setting of an audio clip, the `clip.set_region`, \
            `clip.set_loop`, `clip.set_gain`, and `clip.set_fades` commands are simpler. The \
            data must match the clip's track: audio on audio tracks, MIDI on MIDI tracks. \
            Sending fewer notes than a MIDI clip has deletes notes, which is destructive, so an \
            agent asks first; adding notes or keeping their count isn't.",
    },
    Entry {
        id: "clip.set_region",
        action: Action::Apply("set_clip_region"),
        label: "Set Clip Start and End",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Moves an audio clip's Start and End markers, in seconds into its audio \
            file: the clip starts playing its file at `start` when the song reaches the clip, \
            and, while its loop is off, plays until `end` (`null` for the end of the file) or \
            the clip's own end in the arrangement, whichever comes first, then is silent. \
            Clips play unwarped, at the file's own speed. While the loop is on, `start` must \
            be before the loop's end. Use `clip.set_length` to change how long the clip lasts \
            in the arrangement.",
    },
    Entry {
        id: "clip.set_loop",
        action: Action::Apply("set_clip_loop"),
        label: "Set Clip Loop",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Moves an audio clip's loop brace, in seconds into its audio file \
            (`end` `null` for the end of the file), as the Clip View's loop Position and \
            Length fields do. It doesn't switch the loop on or off: use \
            `clip.set_loop_enabled`. While the loop is on, the clip plays from its Start \
            marker into the brace, then repeats the brace until the clip ends in the \
            arrangement; make the clip longer with `clip.set_length` to hear more repeats. A \
            new clip's brace is the whole file.",
    },
    Entry {
        id: SET_MIDI_REGION,
        action: Action::Apply("set_midi_clip_region"),
        label: "Set MIDI Clip Start and End",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Moves a MIDI clip's Start and End markers, in ticks from the clip's \
            beginning: the clip starts playing at `start` when the song reaches the clip, \
            and, while its loop is off, plays until `end` (`null` for the end of the clip) \
            or the clip's own end in the arrangement, whichever comes first, then is silent. \
            While the loop is on, `start` must be before the loop's end. Use \
            `clip.set_length` to change how long the clip lasts in the arrangement.",
    },
    Entry {
        id: SET_MIDI_LOOP,
        action: Action::Apply("set_midi_clip_loop"),
        label: "Set MIDI Clip Loop",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Moves a MIDI clip's loop brace, in ticks from the clip's beginning \
            (`end` `null` for the end of the clip), as the Clip View's loop Position and \
            Length fields do. It doesn't switch the loop on or off: use \
            `clip.set_loop_enabled`. While the loop is on, the clip plays from its Start \
            marker into the brace, then repeats the brace until the clip ends in the \
            arrangement; make the clip longer with `clip.set_length` to hear more repeats.",
    },
    Entry {
        id: "clip.set_loop_enabled",
        action: Action::Apply("set_clip_loop_enabled"),
        label: "Clip Loop On/Off",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Switches an audio or MIDI clip's Loop switch on or off, leaving its loop brace \
            where it is. On, the clip repeats its brace (`clip.set_loop` or `clip.set_midi_loop`) \
            for as long as it lasts in the arrangement; off, it plays from its Start marker to its \
            End marker once. The Start marker must be before the brace's end to switch it on.",
    },
    Entry {
        id: "clip.set_gain",
        action: Action::Apply("set_clip_gain"),
        label: "Set Clip Gain",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Sets an audio clip's gain as a linear gain: 1 is 0 dB (where every clip \
            starts), 0.5 is about -6 dB (10^(dB/20) converts decibels), 2 is about +6 dB, \
            and 0 is silence (-inf dB). Use it to balance one clip against others on its \
            track; to change the whole track, use `mixer.set_volume`. The value must be \
            finite and at least 0.",
    },
    Entry {
        id: "clip.set_fades",
        action: Action::Apply("set_clip_fades"),
        label: "Set Clip Fades",
        permission: Permission::Edit,
        contexts: &[Arrangement],
        keys: &[],
        description: "Sets the lengths, in seconds, of an audio clip's fade-in, from silence \
            at the clip's start in the arrangement, and its fade-out, to silence at its end; \
            0 is no fade. Each fade is a straight line in amplitude. Short fades stop a clip \
            that starts or ends mid-waveform from clicking: 0.004 (4 ms) is the length that \
            Create Fades on Clip Edges gives new clips. Lengths must be finite and at least \
            0.",
    },
    Entry {
        id: "clip.trim_start",
        action: Action::Apply("trim_clip_start"),
        label: "Trim Clip Start",
        permission: Permission::Edit,
        contexts: &[Arrangement],
        keys: &[],
        description: "Moves a clip's left edge to `start`, keeping its end and keeping its \
            contents where they play, as dragging the clip's left edge does: a later `start` \
            hides the beginning of the clip, an earlier one shows more of it. An audio clip's \
            Start marker moves by the time the edge moved (seconds under the tempo map), as \
            Live's does, so it can't go before the start of the file; a MIDI clip's offset \
            moves by as many ticks and stays at 0 or more. The clip can't start before 0, or at \
            or after its end. To move a clip with its contents, use `clip.move`; to change \
            where it ends, use `clip.set_length`.",
    },
    Entry {
        id: "clip.create_in_slot",
        action: Action::Apply("create_slot_clip"),
        label: "Create Clip in Slot",
        permission: Permission::Edit,
        contexts: &[Session],
        keys: &[],
        description: "Puts a clip in a track's Session slot at `scene`, as Insert Empty MIDI \
            Clip (Cmd+Shift+M) and dropping a clip in a slot do. Only audio and MIDI tracks hold \
            slot clips, the slot must be empty, and the clip's data must match the track. A slot \
            clip is a clip like an Arrangement clip, but it has no position there, so its \
            `start` must be 0. `clip.id` must be a new ID the project has issued for it.",
    },
    Entry {
        id: "clip.move_to_slot",
        action: Action::Apply("move_slot_clip"),
        label: "Move Clip to Slot",
        permission: Permission::Edit,
        contexts: &[Session],
        keys: &[],
        description: "Moves a clip into a track's Session slot at `scene`, out of the slot or \
            the Arrangement lane it sits in, as dragging a clip between slots does. The target \
            slot must be empty, unless it's the clip's own slot, and the clip's data must match \
            the track. A slot clip has no position in the Arrangement: it arrives with its \
            start at 0.",
    },
    Entry {
        id: "scene.create",
        action: Action::Apply("create_scene"),
        label: "Insert Scene",
        permission: Permission::Edit,
        contexts: &[Session],
        keys: &[],
        description: "Inserts a new empty scene at `index` among the scenes, as Create ▸ \
            Insert Scene (Cmd+I) does: it holds an empty slot in every audio and MIDI track. A \
            new set opens with eight scenes. `id` must be a new ID the project has issued for \
            it.",
    },
    Entry {
        id: "scene.delete",
        action: Action::Apply("delete_scene"),
        label: "Delete Scene",
        permission: Permission::Destructive,
        contexts: &[Session],
        keys: &[],
        description: "Deletes a scene, and the clips in its slots with it; the clips in the \
            other scenes stay. Undo restores everything it deleted, but because it removes the \
            person's work an agent asks first.",
    },
    Entry {
        id: "scene.move",
        action: Action::Apply("move_scene"),
        label: "Move Scene",
        permission: Permission::Edit,
        contexts: &[Session],
        keys: &[],
        description: "Moves a scene to `index` among the scenes, with the clips in its slots, as \
            dragging a scene does. The index counts positions after the scene is taken out of \
            its current place, so moving a scene down past its neighbour is its current index \
            plus one.",
    },
    Entry {
        id: "scene.duplicate",
        action: Action::Apply("duplicate_scene"),
        label: "Duplicate Scene",
        permission: Permission::Edit,
        contexts: &[Session],
        keys: &[],
        description: "Copies a scene and the clips in its slots into a new scene directly after \
            it, as Edit ▸ Duplicate does. `clips` lists every track that holds a clip in the \
            scene, each with the new ID its copy takes; each copy is an independent clip. A \
            scene whose slots are all empty copies no clips. `id` must be a new ID the project \
            has issued for it.",
    },
    Entry {
        id: "scene.rename",
        action: Action::Apply("rename_scene"),
        label: "Rename Scene",
        permission: Permission::Edit,
        contexts: &[Session],
        keys: &[],
        description: "Renames a scene. A name may be empty: a scene without a name shows its \
            number, which is its position.",
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
            type ID, or a plug-in named by its format and plug-in ID, with its `name` (the \
            device's own, such as the title bar shows), its `role` (`midi_effect`, \
            `instrument`, or `audio_effect`, the default), and any parameter values to set. A \
            chain keeps Live's order: MIDI effects first, then at most one instrument, then \
            audio effects, and only MIDI tracks hold MIDI effects and instruments; an index \
            that breaks the order is refused. `device.id` must be a new ID the project has \
            issued for it. To add a device as a person does, after the selected device and \
            with its default preset, use `device.add`.",
    },
    Entry {
        id: "device.remove",
        action: Action::Apply("remove_device"),
        label: "Delete Device",
        permission: Permission::Destructive,
        contexts: &[DeviceView],
        keys: &[Key::Runs("S-EDIT-005")],
        description: "Deletes a device from its track's chain, with its settings. Undo \
            restores it, but because it removes the person's work an agent asks first. To \
            bypass a device but keep it, use `device.set_enabled`. From the keyboard it \
            deletes the device selected in the Device View.",
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
            out of its current place. The chain must stay in Live's order (MIDI effects, the \
            instrument, then audio effects), so an audio effect can't go before an \
            instrument, and MIDI effects and instruments only go on MIDI tracks.",
    },
    Entry {
        id: "device.rename",
        action: Action::Apply("rename_device"),
        label: "Rename Device",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[Key::Runs("S-EDIT-008")],
        description: "Renames a device: the name its title bar shows while Show Preset Name \
            is on (`device.set_show_preset_name`). An empty name gives the device back its \
            own name, such as `Auto Filter`. It changes nothing in the sound.",
    },
    Entry {
        id: "device.set_show_preset_name",
        action: Action::Apply("set_show_preset_name"),
        label: "Show Preset Name",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Switches a device's Show Preset Name, the context menu item: on (as \
            every new device is), the title bar shows the name of the preset last loaded or \
            saved, or the name Rename gave it; off, the device's own name. It changes nothing \
            in the sound.",
    },
    Entry {
        id: "device.set_state",
        action: Action::Apply("set_device_state"),
        label: "Set Device State",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Replaces a device's whole state at once, as loading a preset does: \
            `params` lists every parameter value to set, as `[parameter, value]` pairs (any \
            parameter not listed goes back to its default), and `name` is what its title bar \
            shows while Show Preset Name is on (empty for the device's own name). One undo \
            step. To load a preset file, use `device.load_preset`; to change one parameter, \
            `device.set_param`.",
    },
    Entry {
        id: "device.set_enabled",
        action: Action::Apply("set_device_enabled"),
        label: "Activate/Deactivate Device",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Switches a device on or off, as its Device Activator in the title bar \
            does. A device that's off passes its input through unchanged, so you can hear the \
            track with and without it; it keeps its settings and its state.",
    },
    Entry {
        id: "device.set_floating",
        action: Action::Apply("set_device_floating"),
        label: "Float",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[Key::Runs("S-DEV-015")],
        description: "Floats a native device in a window of its own (`floating`: true), or \
            docks it back in the Device View (false), as the device's context menu item Float, \
            its window's Dock button, and closing its window do. While it floats, its place in \
            the chain shows as a narrow strip, so the signal order stays readable, and its \
            window can be made larger: only its displays grow, its controls keeping their size \
            and place. Floating and docking are one undo step each (Float Device, Dock Device); \
            whether a device floats is saved with the set. Moving or resizing the window is \
            `device.set_window`, which isn't an undo step. Plug-ins open their own editor \
            windows instead, so they can't float. It changes nothing in the sound. From the \
            keyboard (Option-F on macOS, Ctrl-Alt-F on Windows and Linux, with the Device View \
            or a floating device's window focused) and the menu it switches the selected \
            device.",
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
        id: "device.set_sample",
        action: Action::Apply("set_device_sample"),
        label: "Set Sample",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets the audio a device plays (`media`, a content hash, or null to play \
            none), which the media store holds like a clip's audio. A device whose type plays a \
            sample plays that file; other devices ignore it. The set keeps the reference, so it \
            reopens playing the same file. To load a file from disk as a person does, use \
            `device.load_sample`.",
    },
    Entry {
        id: "device.set_sidechain_input",
        action: Action::Apply("set_sidechain_input"),
        label: "Set Sidechain Input",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets where a device's sidechain listens, as the Source chooser in its \
            sidechain section does: `none` (No Input, a new device's), or `track` with another \
            audio, group, or return track, Main, or the device's own track, and a `tap`: \
            `pre_fx`, `post_fx` (what choosing a track in the window picks), or `post_mixer`. \
            Only devices that declare a sidechain input have one. The source track still \
            plays as before; the device only listens to it. The detector hears it only while \
            External is on (`device.set_sidechain_enabled`). To make a kick duck a bass, set \
            the bass compressor's source to the kick's track and switch External on.",
    },
    Entry {
        id: "device.set_sidechain_enabled",
        action: Action::Apply("set_sidechain_enabled"),
        label: "External Sidechain On/Off",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Switches a device's sidechain on or off, as the External switch in its \
            sidechain section does. On, the device's detector (a compressor's level detector, \
            say) listens to its sidechain input instead of the track's own signal; its \
            output is still the track's signal, processed. Off, as on a new device, it \
            listens to its own input.",
    },
    Entry {
        id: "device.set_sidechain_gain",
        action: Action::Apply("set_sidechain_gain"),
        label: "Set SC Gain",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets the gain a device's sidechain signal gets before its detector \
            hears it, as linear gain: 1 is 0 dB (a new device's), 2 about +6 dB, 0 silence. \
            Raise it when the sidechain source is too quiet to trigger the device.",
    },
    Entry {
        id: "device.set_sidechain_mix",
        action: Action::Apply("set_sidechain_mix"),
        label: "Set Sidechain Mix",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets how much of what a device's detector hears is its sidechain, from \
            0 (only the track's own signal) to 1 (only the sidechain, a new device's). \
            Values between blend the two.",
    },
    Entry {
        id: "device.set_sidechain_listen",
        action: Action::Apply("set_sidechain_listen"),
        label: "Sidechain Listen",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Switches a device's Sidechain Listen: on, the device's output is what \
            its detector hears (the sidechain, after its gain and mix) instead of the \
            processed track, so you can check what triggers it. Switch it off to hear the \
            track again.",
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
