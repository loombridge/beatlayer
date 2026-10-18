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
    self, Arrangement, Browser, ClipView, DeviceView, Global, RackChains, Session, TrackHeaders,
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
        id: "transport.set_punch_in",
        action: Action::Apply("set_punch_in"),
        label: "Punch-In",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Switches Punch-In on or off, as the Control Bar's Punch-In switch does. \
            With it on, recording into the Arrangement starts at the Arrangement loop brace's \
            start, whether or not the Loop switch is on: playback that starts before the brace \
            records nothing until it reaches the brace, and playback that starts inside the \
            brace records from where it starts. The metronome's \"only while recording\" setting \
            sounds from the punch-in point. Use `transport.set_loop` to move the brace. The \
            switch is saved with the set, and undo switches it back.",
    },
    Entry {
        id: "transport.set_punch_out",
        action: Action::Apply("set_punch_out"),
        label: "Punch-Out",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Switches Punch-Out on or off, as the Control Bar's Punch-Out switch does. \
            With it on, recording into the Arrangement ends at the Arrangement loop brace's \
            end, whether or not the Loop switch is on: the recording stops there, playback \
            carries on, and Arrangement Record stays on but writes nothing more. Use \
            `transport.set_loop` to move the brace. The switch is saved with the set, and undo \
            switches it back.",
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
        id: "transport.set_midi_overdub",
        action: Action::Apply("set_midi_overdub"),
        label: "MIDI Arrangement Overdub",
        permission: Permission::Edit,
        contexts: &[Global],
        keys: &[],
        description: "Switches MIDI Arrangement Overdub on or off, as the Control Bar's overdub \
            switch does. With it on, notes recorded into the Arrangement merge into the MIDI \
            clips already on the track instead of replacing them. The switch is saved with the \
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
        id: "mixer.set_crossfader",
        action: Action::Apply("set_crossfader"),
        label: "Set Crossfader",
        permission: Permission::Edit,
        contexts: &[Session, Arrangement],
        keys: &[],
        description: "Sets the Main track's crossfader, as the Crossfader section's fader does. \
            `position` runs from 0 (the A end) through 0.5 (the centre, where a new set starts) \
            to 1 (the B end). Tracks assigned to A and B fade between the two ends on the \
            default Constant Power curve: at the centre each side plays at -3 dB, at either end \
            the near side plays at 0 dB, the far side is silent, and a track assigned to \
            neither plays through at every position. The engine glides to the new position over \
            20 ms, so a move never clicks. The position is saved with the set.",
    },
    Entry {
        id: "mixer.set_crossfade_assign",
        action: Action::Apply("set_crossfade_assign"),
        label: "Assign Crossfade",
        permission: Permission::Edit,
        contexts: MIXER,
        keys: &[],
        description: "Assigns a track to a side of the crossfader, as its A and B buttons under \
            the mixer's Crossfader section do: `a`, `b`, or `none` (neither, where every track \
            starts). A track's sides are exclusive, so assigning one releases the other; a \
            track assigned to neither plays through at every crossfader position. Every track \
            but Main can be assigned, and Main carries the crossfader itself \
            (`mixer.set_crossfader`). The assignment is saved with the set, and undo releases \
            it.",
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
            \"pair\", \"channel\": 3 }`, the pair 3/4), counted from 1), `resampling` (the Main \
            output, for recording the mix; the track's own output and sends are then silent so \
            it can't feed back), `none`, or `track` with another audio track, a group, a return \
            track, or Main and a `tap`: `pre_fx` (before its devices), `post_fx` (after its \
            devices, before its mixer), or `post_mixer` (its output, the default). A MIDI track \
            can take `all_ins` (every MIDI input whose Track switch is on in the settings, and \
            the computer MIDI keyboard), `computer_keyboard`, `port` with the port's `name` as \
            the MIDI settings list it, `none`, or another MIDI track (which carries MIDI once \
            MIDI playback is built). `channel` is 1 to 16 to take only that channel, or null \
            (the default) for All Channels; only a port input has one. The input is heard only \
            while the track monitors it (`routing.set_monitor`). A routing may make a loop, as \
            in Live; the engine plays the routing that closes it as silence.",
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
            to the return tracks), or `external` (Ext. Out: the track plays on the audio \
            interface's output channels, a single channel or a stereo pair, the first pair by \
            default, and not into Main). Audio, group, and return tracks take any of these. A routing \
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
        id: "track.add_take_lane",
        action: Action::Apply("add_take_lane"),
        label: "Add Take Lane",
        permission: Permission::Edit,
        contexts: &[Arrangement],
        keys: &[],
        description: "Adds a take lane under an audio track's arrangement lane, after the lanes \
            it has. `lane.name` names it and `lane.clips` are the recorded passes it holds, each \
            with a `name`, a `start` and `length` in ticks (705,600 to the quarter note), and \
            audio `data` as `clip.create` takes it. The track plays only its arrangement lane: \
            loop recording adds a lane for every pass and copies the latest pass to the \
            arrangement lane. Audio tracks only.",
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
        id: "clip.set_launch_mode",
        action: Action::Apply("set_clip_launch_mode"),
        label: "Set Clip Launch Mode",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Sets what a press and a release of a Session clip's launch button do (a \
            clip that sits in a slot; `clip` is its ID): \
            `trigger` (the default) starts the clip on the launch grid and it plays on, a second \
            press restarts it; `gate` starts it on a press and stops it on the grid after the \
            release; `toggle` starts it on a press and the next press stops it on the grid; \
            `repeat` starts it on a press and restarts it every launch quantisation step while \
            the press lasts, then stops it on the grid after the release. A new clip starts with \
            the Default Launch Mode setting. The mode is read at each press and release, so \
            changing it doesn't touch a clip that is playing.",
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
        description: "Moves an audio clip's Start and End markers: the clip starts playing at \
            `start` when the song reaches the clip, and, while its loop is off, plays until \
            `end` (`null` for the end of the file) or the clip's own end in the arrangement, \
            whichever comes first, then is silent. An unwarped clip plays at its file's own \
            speed and its positions are seconds into the file (`ticks` false, the default). A \
            warped clip (`clip.set_warp`) follows the set's tempo and its positions are ticks \
            of its contents, from the clip's 1.1.1, with `ticks` true; a command in the other \
            form is refused. While the loop is on, `start` must be before the loop's end. Use \
            `clip.set_length` to change how long the clip lasts in the arrangement.",
    },
    Entry {
        id: "clip.set_loop",
        action: Action::Apply("set_clip_loop"),
        label: "Set Clip Loop",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Moves an audio clip's loop brace, in seconds into its audio file for an \
            unwarped clip or in ticks of its contents (`ticks` true) for a warped one (a \
            command in the other form is refused; `end` `null` for the end of the file), as \
            the Clip View's loop Position and Length fields do. It doesn't switch the loop on \
            or off: use \
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
            End marker once. The Start marker must be before the brace's end to switch it on. \
            An audio clip can only loop while Warp is on (`clip.set_warp`): switching the loop on \
            for an unwarped audio clip is refused.",
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
        id: "clip.set_warp",
        action: Action::Apply("set_clip_warp"),
        label: "Clip Warp On/Off",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Switches an audio clip's Warp on or off. Off, the clip plays at its \
            file's own speed whatever the set's tempo, and its Start and End markers and loop \
            brace are seconds into the file. On, it follows the set's tempo through its warp \
            map (`data.audio.warp_markers`: each pairs a time in the file with a beat, in ticks \
            from the clip's 1.1.1, and `data.audio.tempo` is the speed of the file after the \
            last marker), and its markers and brace are ticks of its contents. Switching on a \
            clip that has no map gives it one marker at its Start marker on 1.1.1, at the set's \
            tempo there, so it goes on playing at the speed it did; switching off keeps the \
            map. Either way the markers and brace convert, so each keeps its place in the \
            audio, and the clip keeps its place and length in the arrangement. A warped clip \
            plays through its warp map at the set's tempo, in its warp mode \
            (`clip.set_warp_mode`); only a warped clip can loop, so switching Warp off also \
            switches the clip's loop off in the same undo step.",
    },
    Entry {
        id: "clip.insert_warp_marker",
        action: Action::Apply("insert_warp_marker"),
        label: "Insert Warp Marker",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Adds a warp marker to each of the warped audio clips `clips`, which must \
            hold files of the same length: at a `beat` of the clip's contents (ticks from its \
            1.1.1, negative before it), at the time in the file the map already places there, \
            or at a `time` in the file (seconds), on the beat the map already places it. \
            Give one of the two. Nothing moves; the audio either side stays where it is. \
            Inserting where a marker is already changes nothing. Markers are named by their \
            place in the map (`data.audio.warp_markers`), the first being 0. The edit is made \
            to every clip in one undo step, or to none if it's refused for one. With \
            `pin_neighbours` (Cmd or Ctrl held on a pseudo-warp marker) it also puts markers at \
            the transients just before and just after the new one, read from the clip's \
            transients (`clip.transients`) unless `transients` lists them; it works on one \
            clip.",
    },
    Entry {
        id: "clip.insert_warp_markers",
        action: Action::Apply("insert_warp_markers"),
        label: "Insert Warp Markers",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Puts warp markers in a range of a warped audio clip's contents, `from` to \
            `to` (ticks from the clip's 1.1.1): at every transient of the clip in the range, or, \
            when it holds none, at its start and its end. The transients are the clip's own \
            (`clip.transients`) unless `transients` lists times in the file. Each marker sits \
            on the beat the map already places its time, so nothing moves; a marker already \
            there is left. Refused for a clip that isn't warped, or while the file's analysis \
            is not ready and `transients` is left out. One undo step.",
    },
    Entry {
        id: "clip.insert_transient",
        action: Action::Apply("insert_transient"),
        label: "Insert Transient",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Puts a transient into an audio clip at `time`, seconds in its file: Insert \
            Transient, at the Sample Editor's insert marker when the app runs it. Putting one \
            where a detected transient was taken away brings that one back. The edit belongs to \
            the clip, is saved with the set, and shows in `clip.transients`. One undo step.",
    },
    Entry {
        id: "clip.delete_transients",
        action: Action::Apply("delete_transients"),
        label: "Delete Transients",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Takes transients out of an audio clip: `times` are their times in the \
            file, in seconds, as `clip.transients` lists them. A transient put in by hand is \
            forgotten; a detected one is noted as taken away and stays away until \
            `clip.reset_transients`. Refused for a time that is none of the clip's transients, \
            once the file's analysis is ready. One undo step.",
    },
    Entry {
        id: "clip.reset_transients",
        action: Action::Apply("reset_transients"),
        label: "Reset Transients",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Takes away all of an audio clip's transient edits: the transients put in \
            by hand go, and the detected ones that were taken away come back. One undo step.",
    },
    Entry {
        id: "clip.move_transient",
        action: Action::Apply("move_transient"),
        label: "Move Transient",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Moves the transient at `from` (seconds in the file, as `clip.transients` \
            lists it) to `to`, as Shift-dragging a pseudo-warp marker does. It changes no \
            audio and no warp marker. One undo step.",
    },
    Entry {
        id: "clip.move_warp_markers",
        action: Action::Apply("move_warp_markers"),
        label: "Move Warp Markers",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Moves the warp markers named in `markers` (their place in the map, the \
            first being 0) of each of the warped audio clips `clips`, which must hold files of \
            the same length, by the same number of ticks `by` (negative moves them earlier). \
            Each keeps its time in the file, so the audio either side of it stretches or \
            squeezes to fit. Refused, changing nothing, if a marker would pass or reach a \
            neighbour that isn't moving with it. Moving the last marker sets the clip's tempo \
            to the new last segment's, so the audio after it carries on at that speed.",
    },
    Entry {
        id: "clip.shift_warp_marker",
        action: Action::Apply("shift_warp_marker"),
        label: "Shift Warp Marker",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Moves the time in the file of warp marker `marker` (its place in the \
            map, the first being 0) of each of the warped audio clips `clips`, which must hold \
            files of the same length, by `by` seconds (negative moves it earlier), keeping \
            its beat: the waveform moves under the marker. Refused, changing nothing, if the \
            marker would pass a neighbour or leave the file.",
    },
    Entry {
        id: "clip.delete_warp_markers",
        action: Action::Apply("delete_warp_markers"),
        label: "Delete Warp Markers",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Deletes the warp markers named in `markers` (their place in the map, the \
            first being 0) of each of the warped audio clips `clips`, which must hold files of \
            the same length: the segments either side of each join into one straight segment. \
            Deleting the last marker leaves the audio after the new last one at the clip's \
            tempo. A warped clip keeps at least one marker: deleting them all is refused.",
    },
    Entry {
        id: "clip.set_segment_bpm",
        action: Action::Apply("set_segment_bpm"),
        label: "Set Warp Segment BPM",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Sets a warped audio clip's BPM field. With `marker` (its place in the \
            map, the first being 0), `bpm` is the tempo of the segment that starts at that \
            marker: every later marker moves by the same number of ticks, so that segment \
            alone changes and each later one keeps its tempo; for the last marker it is the \
            clip's own tempo, from 1 to 1,000. Without `marker`, `bpm` is the first segment's \
            tempo (the clip's when there's one marker) and every tempo of the clip scales by \
            the same factor, as `clip.scale_tempo` does.",
    },
    Entry {
        id: "clip.scale_tempo",
        action: Action::Apply("scale_tempo"),
        label: "Scale Clip Tempo",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Multiplies every segment's tempo and the clip's own tempo of a warped \
            audio clip by `factor`: 2 is Live's ×2 and 0.5 its ÷2, which correct a tempo found \
            at half or double. Each marker's distance in ticks from the first marker is \
            multiplied by it, and the Start and End markers and loop brace move with them \
            about the first marker, so each keeps its place in the audio. The clip's place \
            and length in the arrangement don't change. Refused if the clip's tempo would \
            leave 1 to 1,000 BPM.",
    },
    Entry {
        id: "clip.set_warp_mode",
        action: Action::Apply("set_clip_warp_mode"),
        label: "Set Warp Mode",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Sets how a warped audio clip's audio is fitted to the speed its warp map \
            sets: `beats`, `tones`, `texture`, `re_pitch`, `complex` or `complex_pro` \
            (`data.audio.warp_mode`; a new clip is `beats`). `re_pitch` plays the file faster \
            or slower, so its pitch moves with the speed. An unwarped clip keeps its mode for \
            when Warp goes back on, and every mode's controls stay with the clip. Until the \
            other modes are built, they play as `re_pitch` does.",
    },
    Entry {
        id: "clip.set_warp_settings",
        action: Action::Apply("set_clip_warp_settings"),
        label: "Set Warp Settings",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Sets controls of an audio clip's warp modes, named by mode, any of them in \
            one undo step (`data.audio.warp_settings`; a control at its default isn't listed \
            there). `beats`: `preserve` (`transients`, `1_bar`, `half`, `quarter`, `eighth`, \
            `sixteenth`, `thirty_second`), `transient_loop_mode` (`loop_off`, `loop_forward`, \
            `loop_back_and_forth`), `transient_envelope` (0 to 100, default 100). `tones`: \
            `grain_size` (10 to 100 ms, default 30). `texture`: `grain_size` (10 to 500 ms, \
            default 65), `flux` (0 to 100 %, default 25). `complex_pro`: `formants` (0 to 100 \
            %, default 100), `envelope` (8 to 512, default 128). Give at least one control; a \
            value out of range refuses the whole command.",
    },
    Entry {
        id: "clip.set_ram_mode",
        action: Action::Apply("set_clip_ram_mode"),
        label: "Set Clip RAM Mode",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Switches an audio clip's RAM mode (`data.audio.ram`, off for a new clip). \
            On, the clip's whole file is held in memory instead of streamed from disk, so it \
            never underruns; the audio it plays is the same. Switching it while the clip plays \
            leaves no gap.",
    },
    Entry {
        id: "clip.set_high_quality",
        action: Action::Apply("set_clip_high_quality"),
        label: "Set Clip High Quality",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Switches an audio clip's high-quality interpolation (`data.audio.high_quality`, \
            on for a new clip). On, the clip is resampled by a band-limited interpolator wherever \
            it is read at a rate other than 1, as Re-Pitch does at any speed but 1, at more CPU \
            and with less distortion; off, by a cheaper one. Where no resampling happens it \
            changes nothing.",
    },
    Entry {
        id: "clip.set_legato",
        action: Action::Apply("set_clip_legato"),
        label: "Set Clip Legato",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Switches Legato on or off for a Session clip, one that sits in a slot \
            (`clip` is its ID). A Legato clip takes over the play position of the clip \
            playing on its track when it launches, so the two stay in sync. Off is the \
            default. The setting is saved with the clip and is one undo step; launching \
            doesn't read it yet.",
    },
    Entry {
        id: "clip.set_launch_quantization",
        action: Action::Apply("set_clip_launch_quantization"),
        label: "Set Clip Launch Quantization",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Sets a Session clip's own launch quantisation, the grid its launches \
            wait for: one of the global quantisation's steps (`none`, `8_bars`, `4_bars`, \
            `2_bars`, `1_bar`, `half`, `half_triplet`, `quarter`, `quarter_triplet`, `eighth`, \
            `eighth_triplet`, `sixteenth`, `sixteenth_triplet`, `thirty_second`), or `null` \
            (the default, Live's \"Global\") to follow the global quantisation. `clip` is the \
            ID of a clip that sits in a slot. The setting is saved with the clip and is one \
            undo step; launching doesn't read it yet.",
    },
    Entry {
        id: "clip.set_velocity_amount",
        action: Action::Apply("set_clip_velocity_amount"),
        label: "Set Clip Velocity Amount",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Sets how much the velocity of the MIDI note that launches a Session clip \
            changes the clip's volume: `amount` is from 0 (no influence, the default) to 1 \
            (100 %, where the softest note plays the clip silently). `clip` is the ID of a \
            clip that sits in a slot. The setting is saved with the clip and is one undo \
            step; launching doesn't read it yet.",
    },
    Entry {
        id: "clip.set_follow_actions",
        action: Action::Apply("set_clip_follow_actions"),
        label: "Set Clip Follow Actions",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Changes a Session clip's follow actions, as the Launch Properties panel \
            does (`clip` is the ID of a clip that sits in a slot). `enabled` is the Follow Action switch; `action_a` and \
            `action_b` are one of `no_action`, `stop`, `play_again`, `previous`, `next`, `first`, \
            `last`, `any`, `other` and `jump`; `chance_a` is action A's chance in whole percent, \
            0 to 100, and action B has the rest; `jump_a` and `jump_b` are the slot (scene) \
            numbers Jump goes to, 1 up to the number of scenes, and 0 for none, where Jump stops \
            the clip; `linked` is Linked: the follow time is the multiplier times the clip's loop, or the clip's end when its Loop switch is off; Unlinked: `time`; `multiplier` is the Linked multiplier, a whole number from \
            1; and `time` is the Unlinked follow time in ticks, from a sixteenth (a bar is \
            2822400 ticks in 4/4). Fields left out stay as they are, and a number past its \
            limit is held at the limit. One undo step. The defaults are off, A Next, B No \
            Action, 100 %, Linked, 1x, one bar. While the song plays the action runs when the \
            follow time has passed after the clip started, and the global switch \
            (`session.set_follow_actions_enabled`) must be on.",
    },
    Entry {
        id: "clip.set_envelope_breakpoint",
        action: Action::Apply("set_clip_envelope_breakpoint"),
        label: "Add Envelope Breakpoint",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Sets the value of one of a clip's envelopes at a position, adding a \
            breakpoint there or replacing the one that's there, as clicking or dragging a \
            breakpoint in the Envelopes tab does. `clip` is the clip, `target` the parameter of \
            the clip's track the envelope modulates (a control of its mixer, one of its sends, \
            or a parameter of a device in its chain), `at` the position in ticks from the \
            beginning of the clip's contents, and `value` the value there, in the parameter's \
            own units. A MIDI clip's `target` can instead be a MIDI controller (MIDI Ctrl in \
            the Envelopes tab): `{\"type\": \"midi.cc\", \"number\": 1}` (a controller from 1 to \
            31 or 33 to 119, value 0 to 127), `{\"type\": \"midi.pitch_bend\"}` (-8192 to 8191, \
            0 at rest) or `{\"type\": \"midi.channel_pressure\"}` (0 to 127); the value is \
            the message the clip sends to its track's devices while it plays, on any channel. \
            A parameter with no clip envelope gets one, linked to the clip, with this \
            breakpoint as its first.",
    },
    Entry {
        id: "clip.set_envelope_curve",
        action: Action::Apply("set_clip_envelope_curve"),
        label: "Curve Envelope Segment",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Curves the segment starting at one of a clip envelope's breakpoints, as \
            holding Alt (Windows) or Option (macOS) while dragging a segment in the Envelopes \
            tab does. `clip` and `target` name the envelope as `clip.set_envelope_breakpoint` \
            does, `at` is the breakpoint the segment starts at, and `bend` is from -1 to 1: 0 \
            makes the segment a straight line again, and the sign chooses which side it bows \
            to.",
    },
    Entry {
        id: "clip.move_envelope_breakpoint",
        action: Action::Apply("move_clip_envelope_breakpoint"),
        label: "Move Envelope Breakpoint",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Moves one of a clip envelope's breakpoints from `at` to `to`, as dragging it \
            in the Envelopes tab does. `clip` and `target` name the envelope as \
            `clip.set_envelope_breakpoint` does. `to` is 0 or later, and not where another \
            breakpoint of the same envelope already is. `value` gives the breakpoint a new \
            value in the parameter's own units; left out, it keeps the one it has.",
    },
    Entry {
        id: "clip.delete_envelope_breakpoint",
        action: Action::Apply("delete_clip_envelope_breakpoint"),
        label: "Delete Envelope Breakpoint",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Deletes one of a clip envelope's breakpoints, so the segments either side \
            of it become the one starting at the breakpoint before it, as clicking a breakpoint \
            in the Envelopes tab does. `clip` and `target` name the envelope as \
            `clip.set_envelope_breakpoint` does, and `at` is where the breakpoint is. Deleting \
            the last one takes the envelope with it.",
    },
    Entry {
        id: "clip.delete_envelope",
        action: Action::Apply("delete_clip_envelope"),
        label: "Clear Envelope",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Deletes a clip's envelope for a parameter, as Clear Envelope in the \
            Envelopes tab does: the parameter goes back to what it does without the clip \
            envelope. `clip` and `target` name the envelope as `clip.set_envelope_breakpoint` \
            does. Nothing changes if the clip has no envelope for it.",
    },
    Entry {
        id: "clip.set_envelope_linked",
        action: Action::Apply("set_clip_envelope_linked"),
        label: "Link Envelope",
        permission: Permission::Edit,
        contexts: &[ClipView],
        keys: &[],
        description: "Switches a clip envelope's Linked switch: linked (`true`, the default), it \
            follows the clip's region and loop, playing what the clip's contents play; \
            unlinked, it plays once from the clip's start, one tick per tick, for as long as the \
            clip lasts. `clip` and `target` name the envelope as \
            `clip.set_envelope_breakpoint` does; a target with no envelope gets an empty linked \
            one, so a switch thrown before anything is drawn sets what drawing will use.",
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
        id: "slot.set_stop_button",
        action: Action::Apply("set_slot_stop_button"),
        label: "Add Stop Button",
        permission: Permission::Edit,
        contexts: &[Session],
        keys: &[Key::Toggles("S-SES-005", "stop_button")],
        description: "Adds or removes a Session slot's Clip Stop button, which the person \
            reaches through Edit ▸ Remove Stop Button (Cmd+E on macOS, Ctrl+E on Windows) and \
            the slot's and Session clip's context menus. The button stops the track's playing \
            clip; the track's own stop button (its status row) and Stop All Clips do the same \
            for one or every track. A slot has one until it's removed, so a set saved before \
            this command loads with one in every slot of every audio and MIDI track. The \
            switch is saved with the set, and undo adds or removes the button again.",
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
        id: "scene.set_tempo",
        action: Action::Apply("set_scene_tempo"),
        label: "Set Scene Tempo",
        permission: Permission::Edit,
        contexts: &[Session],
        keys: &[],
        description: "Gives a scene a tempo, from 20 to 999 BPM, that launching the scene sets \
            for the set, as the Tempo field in the Main column and the Scene View does. `tempo` \
            left out or null switches the scene's tempo off, as double-clicking the field or \
            Return to Default does, so launching it leaves the set's tempo as it is. One undo \
            step.",
    },
    Entry {
        id: "scene.set_follow_actions",
        action: Action::Apply("set_scene_follow_actions"),
        label: "Set Scene Follow Actions",
        permission: Permission::Edit,
        contexts: &[Session],
        keys: &[],
        description: "Changes a scene's follow actions, as the Scene View does: what the scene \
            does after it was launched, over the scenes as a clip's do over its track's slots. \
            `enabled` is the Follow Action switch; `action_a` and \
            `action_b` are one of `no_action`, `stop`, `play_again`, `previous`, `next`, `first`, \
            `last`, `any`, `other` and `jump`; `chance_a` is action A's chance in whole percent, \
            0 to 100, and action B has the rest; `jump_a` and `jump_b` are the slot (scene) \
            numbers Jump goes to, 1 up to the number of scenes, and 0 for none, where Jump stops \
            the scene; `linked` is Longest (on by default): the follow time is the multiplier times the scene's longest clip; off: `time`; `multiplier` is the Linked multiplier, a whole number from \
            1; and `time` is the Unlinked follow time in ticks, from a sixteenth (a bar is \
            2822400 ticks in 4/4). Fields left out stay as they are, and a number past its \
            limit is held at the limit. One undo step. The defaults are off, A Next, B No \
            Action, 100 %, Linked, 1x, one bar. While the song plays the action runs when the \
            follow time has passed after the scene started, and the global switch \
            (`session.set_follow_actions_enabled`) must be on.",
    },
    Entry {
        id: "scene.set_time_signature",
        action: Action::Apply("set_scene_time_signature"),
        label: "Set Scene Time Signature",
        permission: Permission::Edit,
        contexts: &[Session],
        keys: &[],
        description: "Gives a scene a time signature that launching the scene sets for the set, \
            as the Signature field in the Main column and the Scene View does: a numerator from \
            1 to 99 over a denominator of 1, 2, 4, 8, or 16. `signature` left out or null \
            switches the scene's signature off, so launching it leaves the set's signature as it \
            is. One undo step.",
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
        id: "modulator.map",
        action: Action::Apply("map_modulator"),
        label: "Map Modulator",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Maps one of a modulator's slots (`slot`, 0 to 7; the Play Mod's are 0 to 4) \
            to a parameter, as the slot's Map button and a click on the parameter's control do. \
            `target` is a mixer control (`volume`, `pan` or `mute`) or a send of any track, or a \
            parameter of any device on any track, but not a parameter of the modulator itself. \
            The slot's earlier mapping is replaced; one modulator doesn't map one parameter \
            twice. A modulator in Remote Control can't map a parameter another modulator in \
            Remote Control holds. A modulator driven by audio, notes or MIDI (the Level Mod, \
            Gate Mod, Strike Mod and Play Mod) doesn't map another modulator's parameters, nor \
            one on a track that feeds its own track, which would make the two tracks wait on each \
            other. The mapping is saved with the set, goes with its target when that is deleted, \
            and shows in the `mappings` field of the query tool's `device`. One undo step (Map \
            Modulator).",
    },
    Entry {
        id: "modulator.unmap",
        action: Action::Apply("unmap_modulator"),
        label: "Unmap Modulator",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Removes the mapping of a modulator's slot (`slot`, 0 to 7; the Play Mod's \
            are 0 to 4), as its Unmap \
            button does: the parameter plays its own value again from the next frame. A slot \
            with no mapping stays as it is. One undo step (Unmap Modulator).",
    },
    Entry {
        id: "modulator.set_shape",
        action: Action::Apply("set_shape"),
        label: "Edit Shape",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Replaces the breakpoint shape of a Curve Mod or a Strike Mod, as every edit \
            of its display does (adding, moving, bending or deleting a breakpoint, making one \
            the sustain breakpoint, Clear and the six presets). `points` lists up to 64 \
            breakpoints in order along the cycle, each with `x` (its place, 0 to 1), `y` (the \
            output there, 0 to 1), `bend` (of the segment starting there, from -1 to 1, 0 \
            straight) and `sustain` (true on the Strike Mod's sustain breakpoint, at most one in \
            a shape). An empty list outputs 0. One undo step (Edit Shape).",
    },
    Entry {
        id: "device.compare_switch",
        action: Action::Apply("compare_switch"),
        label: "Compare: Switch",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[Key::Runs("S-DEV-011")],
        description: "Makes the other of a native device's two states, A and B, the active \
            one, as the Edit menu's and the device menu's Compare: Switch to B/A and the P key \
            do. Every parameter whose value differs between the states moves to the newly \
            active state's value, a continuous one gliding as any change does, and every \
            parameter of the device that has an envelope is overridden until Re-Enable \
            Automation. One undo step, named as the item that ran it. A change to a parameter \
            goes to the active state only. Racks and plug-ins have no states, so a command for \
            one is refused. `device.parameters` says which state is active and each \
            parameter's value in the other; the query tool's `device` has both in `compare`. \
            From the keyboard and the menus it switches the selected device.",
    },
    Entry {
        id: "device.compare_copy",
        action: Action::Apply("compare_copy"),
        label: "Compare: Copy",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Copies every value of a native device's active state, A or B, over the \
            other, as the Edit menu's and the device menu's Compare: Copy A to B (Copy B to A \
            while B is active) do. What the device plays doesn't change, and no envelope is \
            overridden. One undo step, named as the item that ran it. Racks and plug-ins have \
            no states, so a command for one is refused. From the menus it copies the selected \
            device's state.",
    },
    Entry {
        id: "device.set_sample",
        action: Action::Apply("set_device_sample"),
        label: "Set Sample",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets the audio a device plays (`media`, a content hash, or null to play \
            none), which the media store holds like a clip's audio, in sample slot `slot` \
            (counted from 0, and 0 unless given: the sampler has the one, the eight-slot drum \
            sampler eight). A device whose type plays samples plays that file; other devices \
            ignore it. The set keeps the reference, so it reopens playing the same file. Null \
            clears the slot, one undo step named Clear Sample. To load a file from disk as a \
            person does, use `device.load_sample`.",
    },
    Entry {
        id: "device.insert_layers",
        action: Action::Apply("insert_layers"),
        label: "Add Samples",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Adds layers to a multisampler at `index` (`null` for the end), each a sample named by content hash with its zones (every zone whole and unfaded in a new layer), Root Key, loop points and levels. The multisampler gives each layer the next of its own layer IDs, in order, ignoring the `id` you send; read them back from the device's `layers` in the query tool's `device` result. A sample must be in the media store to play; to add files from disk as a person does, use `device.add_layers`. One undo step.",
    },
    Entry {
        id: "device.delete_layers",
        action: Action::Apply("delete_layers"),
        label: "Delete Samples",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Removes the listed layers from a multisampler, as the layer list's Delete does. A layer that isn't there refuses the whole command. One undo step.",
    },
    Entry {
        id: "device.duplicate_layers",
        action: Action::Apply("duplicate_layers"),
        label: "Duplicate Samples",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Copies the listed layers, each copy straight after its original with the next layer ID, as the layer list's Duplicate does. One undo step.",
    },
    Entry {
        id: "device.rename_layer",
        action: Action::Apply("rename_layer"),
        label: "Rename Sample",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Renames a layer, as the layer list's Rename does. The name is what the title bar shows for the first layer while Show Preset Name is on.",
    },
    Entry {
        id: "device.move_layers",
        action: Action::Apply("move_layers"),
        label: "Move Samples",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Moves the listed layers, keeping their order, so the first sits at `index` among the layers that stay; the list's order decides Round Robin's turns and which layer a sort leaves first.",
    },
    Entry {
        id: "device.sort_layers",
        action: Action::Apply("sort_layers"),
        label: "Sort Samples",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sorts a multisampler's layers by name, key, velocity or selector (the lowest value of that zone), ascending or `descending`, as the layer list's sorting items do. Layers that tie keep their order.",
    },
    Entry {
        id: "device.set_layer_zone",
        action: Action::Apply("set_layer_zones"),
        label: "Change Zones",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets one kind of zone (`key` 0 to 127, `velocity` 1 to 127, or `select` 0 to 127) of the listed layers, each with its own `low`, `high` and fades (`fade_low` and `fade_high`, how many values each fade spans). A zone holds at least one value and its fades stay inside it. In a fade the layer is silent at the zone's edge and reaches full level at the fade's inner edge, linearly or constant-power as the device's Fade Mode says. One undo step, as one drag in the zone editor is.",
    },
    Entry {
        id: "device.update_layers",
        action: Action::Apply("update_layers"),
        label: "Change Sample Settings",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Changes settings of the listed layers exactly as given, without snapping markers to zero crossings (`device.set_layer` does that while Snap is on): `root_key`, `detune`, `volume` (a linear gain, up to +24 dB), `pan`, `start`, `end`, `link`, `sustain` and `release` modes, `loop_start`, `loop_end`, `release_loop` (frames of the sample at its own rate), `crossfade_ms`, `loop_detune` and `one_shot` (the layer's voices play on to Sample End whatever the note does, as an SFZ `one_shot` region's do). Settings left out stay as they were. The markers must stay ordered: Start, Loop Start, Loop End, Release Loop and End, inside the sample. One setting out of range refuses the whole command.",
    },
    Entry {
        id: "device.set_layer_media",
        action: Action::Apply("set_layer_media"),
        label: "Replace Sample",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Replaces one layer's sample with another already in the media store, keeping the layer's zones and settings and its markers at the same share of the new length, as hot-swapping a sample does. `length`, `sample_rate` and `channels` describe the new sample. To pick a file from disk, use `device.set_layer_sample`.",
    },
    Entry {
        id: "device.add_sample_edit",
        action: Action::Apply("add_sample_edit"),
        label: "Edit Sample",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Adds one edit to the end of a sample's edit list without looking at the audio: `edit.kind` is `trim`, `cut`, `reverse`, `fade_in`, `fade_out` or `normalize`, and `edit.range` (`from` to `to`, in frames of the sample as the earlier edits left it) is its selection, left out for the whole sample (Trim and Cut need one). With a `layer` the edit joins that layer's list and its length and markers follow the audio; with none it joins the sample of the device itself, which this command doesn't check against the sample's length. The source file is never written. One undo step named for the edit. `device.edit_sample` checks the selection against the sample.",
    },
    Entry {
        id: "device.clear_sample_edits",
        action: Action::Apply("clear_sample_edits"),
        label: "Revert Edits",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Empties a sample's edit list, a layer's (`layer`) or the device's own (none given). A layer's sample is `length` frames long again, the unedited sample's length at its own rate, and its markers keep their share of it. One undo step. `device.edit_sample` with `revert` reads the length itself.",
    },
    Entry {
        id: "device.set_processed_layer_sample",
        action: Action::Apply("set_processed_layer_sample"),
        label: "Replace With Processed Copy",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Replaces a layer's sample with a processed copy of its edited sample that is already in the media store (`media`, `length` frames at its own rate): the copy holds the edits, so the layer's list empties. `crop` is the span of the edited sample the copy keeps, which moves the markers as a Trim does; with none the markers stay where they are. One undo step, named Crop with a `crop` and Reverse without. `device.crop_sample` and `device.reverse_sample` write the copy.",
    },
    Entry {
        id: "device.distribute_layers",
        action: Action::Apply("distribute_layers"),
        label: "Distribute Ranges",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Spreads the listed layers' ranges, in list order and without fades: `equally` gives each an equal share of the range of the zone named by `kind` (keys from 0 to 127, velocities 1 to 127, select 0 to 127), and `around_root_key` splits the keyboard between the layers' root keys, layers that share a root dividing its zone. With one layer or none listed, every layer. One undo step.",
    },
    Entry {
        id: "device.set_layer_options",
        action: Action::Apply("set_layer_options"),
        label: "Change Sample Options",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets the multisampler options that aren't parameters: `ram_mode` (decode every layer whole into memory; off, long layers stream from disk), `snap` (markers set afterwards land on zero crossings of the left channel) and `linear_loop_fades` (the title bar's Use Constant Power Fade for Loops, off when true). Options left out stay as they were.",
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
        id: "device.set_midi_sidechain_source",
        action: Action::Apply("set_midi_sidechain_source"),
        label: "Set MIDI Sidechain Source",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets the MIDI track a device's MIDI sidechain listens to, as the MIDI \
            source chooser in its sidechain parameters does: `track` is a MIDI track (the \
            device's own track too), or left out for No Input, a new device's. The device \
            hears the notes and channel messages that track's clips and input play, before \
            its own devices change them; the track still plays as before. Only devices that \
            declare a MIDI sidechain input have one. Deleting the track leaves No Input.",
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
        id: "device.insert_in_chain",
        action: Action::Apply("insert_chain_device"),
        label: "Insert Device in Chain",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Inserts a device into a rack's chain at `index`, where 0 is first in the chain's signal flow, as `device.insert` does for a track's chain. The chain may belong to a rack at any depth. The device is described as for `device.insert`, with a new ID. A chain keeps the order for its kind of rack: an audio effect rack's chains hold only audio effects, a MIDI effect rack's only MIDI effects, and an instrument or drum rack's chains hold MIDI effects, then at most one instrument, then audio effects; an index or device that breaks it is refused.",
    },
    Entry {
        id: "device.move_to_chain",
        action: Action::Apply("move_device_to_chain"),
        label: "Move Device to Chain",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Moves a device to `index` in a rack's chain, from a track's chain or another chain, on the same track or another, keeping its settings and running state. The index counts positions after the device is taken out of its current place. The chain must stay in order for its kind of rack. A rack can't be moved into one of its own chains. To move a device out of a rack, use `device.move`.",
    },
    Entry {
        id: "device.set_enabled_all",
        action: Action::Apply("set_device_enabled_all"),
        label: "Switch Device and Contents",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[Key::Toggles("S-DEV-003", "enabled")],
        description: "Switches a device and every device inside it, at any depth, on or off with `enabled`: Option-click (Alt-click) on a rack's Device Activator. On a device that isn't a rack it is `device.set_enabled`. One undo step.",
    },
    Entry {
        id: "chain.add",
        action: Action::Apply("add_chain"),
        label: "Add Chain",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Adds a chain to a rack at `index` in the rack's list of chains, where 0 is first. The chain has a new ID from the project, a `name` (left empty, a chain made with a device in it is named after the device's title and an empty one `Chain`), an optional `colour` (0 to 69), `info` text, a `mixer` (leave it out for a new chain's: `volume` 1, `pan` 0, `enabled` true, `solo` false) and its `devices`, each with a new ID, in the order for the kind of rack. An empty chain passes its input straight to its mixer, which makes a dry path. One undo step.",
    },
    Entry {
        id: "chain.remove",
        action: Action::Apply("remove_chain"),
        label: "Delete Chain",
        permission: Permission::Destructive,
        contexts: &[DeviceView],
        keys: &[],
        description: "Deletes a chain from its rack, with its devices and its automation. Undo restores it, but because it removes the person's work an agent asks first.",
    },
    Entry {
        id: "chain.move",
        action: Action::Apply("move_chain"),
        label: "Move Chain",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Moves a chain to `index` in a rack's list of chains, within its rack or into another rack, on the same track or another, keeping its devices' settings and running state. The index counts positions after the chain is taken out of its current place. The chain must fit the other rack's kind, and a chain can't go into a rack it holds.",
    },
    Entry {
        id: "chain.rename",
        action: Action::Apply("rename_chain"),
        label: "Rename Chain",
        permission: Permission::Edit,
        contexts: &[RackChains],
        keys: &[Key::Runs("S-EDIT-008")],
        description: "Renames a rack's chain: the name the chain list shows. An empty name gives it the name a chain has without one: its first device's title, or `Chain` while it holds none.",
    },
    Entry {
        id: "chain.set_colour",
        action: Action::Apply("set_chain_colour"),
        label: "Set Chain Colour",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Colours a chain with swatch `colour` of the palette of 5 rows of 14 (0 to 69), or clears its colour with null.",
    },
    Entry {
        id: "chain.set_info",
        action: Action::Apply("set_chain_info"),
        label: "Edit Chain Info Text",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets the info text of a chain, Edit Info Text in its context menu. Empty clears it.",
    },
    Entry {
        id: "chain.set_gain",
        action: Action::Apply("set_chain_volume"),
        label: "Set Chain Gain",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets a chain's Volume as linear gain with the track fader's range: 1 is 0 dB (a new chain's), 2 about +6 dB, 0 silence. The chain's level glides to the new value over 20 ms with no new program. Chains of a MIDI effect rack have no volume. Moving it by hand turns off its automation envelope.",
    },
    Entry {
        id: "chain.set_pan",
        action: Action::Apply("set_chain_pan"),
        label: "Set Chain Pan",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Sets a chain's Pan from -1 (left) to 1 (right), 0 centre (a new chain's), by the track mixer's constant-power law. Chains of a MIDI effect rack have no pan.",
    },
    Entry {
        id: "chain.set_enabled",
        action: Action::Apply("set_chain_enabled"),
        label: "Chain Activator",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Switches a chain's Chain Activator: a chain that's off adds nothing to its rack's output, and its level glides down over 20 ms. Its devices keep running, and its latency still counts.",
    },
    Entry {
        id: "chain.set_solo",
        action: Action::Apply("set_chain_solo"),
        label: "Solo Chain",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Solos or unsolos a chain: while any chain of a rack is soloed, the rack's unsoloed chains add nothing. It leaves every other rack and every track as it is.",
    },
    Entry {
        id: "device.wrap_in_rack",
        action: Action::Apply("group_devices"),
        label: "Wrap Devices in a Rack",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Wraps devices of one chain in a new rack, which takes the first device's place: the rack holds one chain that holds the devices in their order, each keeping its settings and running state. `devices` are in one chain of a track or a rack; `rack` is a new rack device (its kind `rack` of `instrument`, `audio_effect`, `midi_effect` or `drum`, and the role of its kind) with new IDs and exactly one empty chain. The rack's kind must be one that can hold them: an audio effect rack for audio effects alone, a MIDI effect rack for MIDI effects alone, an instrument rack for any other mix. A grouping that would break the chain's order is refused. One undo step. To group the selection as a person does, use `device.group`.",
    },
    Entry {
        id: "device.unwrap_rack",
        action: Action::Apply("ungroup_device"),
        label: "Take a Rack Apart",
        permission: Permission::Edit,
        contexts: &[DeviceView],
        keys: &[],
        description: "Takes a rack with one chain, or none, apart: the chain's devices take the rack's place in order, keeping their settings and running state; the rack, its chain and its mixer are gone. A rack of several chains can't be taken apart. One undo step. To ungroup the selected rack as a person does, use `device.ungroup`.",
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
