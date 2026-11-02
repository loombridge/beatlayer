//! Edits to the set: grouped edits, macros and the automation and note commands.

use super::{MacroEdit, Shell, ShellError, edit_error};
use crate::arrangement::automation;
use crate::clip_view;
use bl_model::ChangeSet;
use bl_model::registry as model;
use bl_shell_spec::scales as scales_ids;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeSet;

use super::{automation_command, sample_edit_command, to_value};
use crate::arrangement::editing::{self, Clipboard};
use bl_engine::session as engine_session;
use bl_model::AutomationTarget;
use bl_model::TrackId;
use bl_shell_spec::devices as device_ids;

impl Shell {
    /// Checks a document edit against the media and fills in what only the
    /// app knows, before it runs (the warping spec, R7, R11, R23 to R27 and
    /// R39 to R41). [`Shell::run`] does it for every edit; an agent's batch,
    /// which runs edits with the registry, calls it for each of its steps.
    ///
    /// # Errors
    ///
    /// If the edit names media it can't use, or needs an analysis that isn't
    /// ready.
    /// Runs an edit through `bl_model::registry::run`, inside a transaction
    /// named for the macro when the command moves one, and writes the values
    /// the macro law gives each mapped parameter before the step closes
    /// (macros spec, R13).
    pub(super) fn run_document_edit(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<ChangeSet, ShellError> {
        let command = model::command_from_args(id, args.clone()).ok();
        // Spec: macros R11. A mapped parameter belongs to its macro: its own
        // control and `device.set_param` are refused, naming the macro.
        if let Some(bl_model::Command::SetDeviceParam { device, param, .. }) = &command
            && let Some(name) = self
                .devices
                .macro_holding(self.editor.document(), *device, *param)
        {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: format!(
                    "{device}'s parameter {} is held by macro \"{name}\"",
                    param.0
                ),
            });
        }
        // Spec: themes R19, R23. A track made new takes the colour the
        // settings give, in the step that makes it.
        if let Some(created @ bl_model::Command::CreateTrack { id: track, .. }) = &command {
            let look = &self.prefs.look;
            return self
                .editor
                .transact(created.label(), |editor| {
                    let changes =
                        model::run(editor, id, args).map_err(|error| error.to_string())?;
                    crate::colours::colour_new(editor, look, &[*track], &[])
                        .map_err(|error| error.to_string())?;
                    Ok::<_, String>(changes)
                })
                .map_err(|message| ShellError::Failed {
                    command: id.to_owned(),
                    message,
                });
        }
        let edit = command
            .as_ref()
            .and_then(|command| self.macro_edit(command));
        let Some(edit) = edit else {
            return model::run(&mut self.editor, id, args).map_err(|error| ShellError::Failed {
                command: id.to_owned(),
                message: error.to_string(),
            });
        };
        let label = match &edit {
            MacroEdit::Macro { label, .. } => std::borrow::Cow::Owned(label.clone()),
            _ => std::borrow::Cow::Borrowed(
                command
                    .as_ref()
                    .map_or("Change macros", bl_model::Command::label),
            ),
        };
        self.editor.begin(label);
        let changes = match model::run(&mut self.editor, id, args) {
            Ok(changes) => changes,
            Err(error) => {
                self.editor.cancel().ok();
                return Err(ShellError::Failed {
                    command: id.to_owned(),
                    message: error.to_string(),
                });
            }
        };
        let updates = self.macro_edit_commands(&edit);
        for update in &updates {
            if let Err(error) = self.editor.apply(update.clone()) {
                self.editor.cancel().ok();
                return Err(ShellError::Failed {
                    command: id.to_owned(),
                    message: error.to_string(),
                });
            }
        }
        self.editor.commit().ok();
        Ok(changes)
    }

    /// Applies the document edits `steps` as one undo step named `label`,
    /// each with the macro law's follow-ups (macros spec, R6, R7, R11 and
    /// R13), as [`Shell::run`] does for one. The agent's `run` tool calls it
    /// for a batch of edits, so a macro moves the same way through the agent
    /// API as through a gesture. The step is made by `maker`, so the history
    /// says who made it.
    ///
    /// # Errors
    ///
    /// [`ShellError`] saying which step failed and why; the set is unchanged.
    pub fn run_document_edits(
        &mut self,
        maker: bl_model::Maker,
        label: &str,
        steps: &[(String, Value)],
    ) -> Result<(Vec<Value>, bool), ShellError> {
        self.editor.begin_by(maker, label.to_owned());
        let start = self.editor.document().clone();
        let mut results = Vec::with_capacity(steps.len());
        for (index, (id, args)) in steps.iter().enumerate() {
            let shown = self.shown_clip();
            let shown_scale = crate::scales::shown_scale(self.editor.document(), shown);
            match self.run_document_edit(id, args.clone()) {
                Ok(changes) => {
                    // Spec: scales R10. A change to the shown clip's scale
                    // changes the current scale with it, however the change
                    // was made.
                    self.follow_shown_scale(id, shown, shown_scale);
                    results.push(json!({ "command": id, "result": changes }))
                }
                Err(error) => {
                    self.editor.cancel().ok();
                    return Err(ShellError::Failed {
                        command: id.clone(),
                        message: format!("command {} (`{id}`): {error}", index + 1),
                    });
                }
            }
        }
        let changed = !ChangeSet::between(&start, self.editor.document()).is_empty();
        self.editor.commit().ok();
        if changed {
            self.dirty = true;
            self.document_changed();
            // Spec: crash-recovery R1. The batch is journalled as the one
            // edit it was.
            self.journal_edit(Some(label));
        }
        self.touch();
        Ok((results, changed))
    }

    /// Tries the document edits `steps` in turn on the set as it is, as
    /// [`Shell::run_document_edits`] would run them, and then undoes them: a
    /// dry run that applies the macro rules too (macros spec, R11). Each
    /// step's changes, or the step that failed, counting from 0, and why.
    ///
    /// # Errors
    ///
    /// The index of the step that failed, and its [`ShellError`]; the set is
    /// unchanged either way.
    pub fn try_document_edits(
        &mut self,
        steps: &[(String, Value)],
    ) -> Result<Vec<ChangeSet>, (usize, ShellError)> {
        self.editor.begin("Dry run");
        let mut results = Vec::with_capacity(steps.len());
        let mut failure = None;
        for (index, (id, args)) in steps.iter().enumerate() {
            match self.run_document_edit(id, args.clone()) {
                Ok(changes) => results.push(changes),
                Err(error) => {
                    failure = Some((index, error));
                    break;
                }
            }
        }
        self.editor.cancel().ok();
        failure.map_or(Ok(results), Err)
    }

    /// The macro edit `command` makes, if it makes one.
    fn macro_edit(&self, command: &bl_model::Command) -> Option<MacroEdit> {
        let doc = self.editor.document();
        match command {
            bl_model::Command::SetDeviceParam { device, param, .. } => {
                let number = bl_model::macro_number(*param)?;
                let rack = doc.device(*device)?;
                rack.rack_kind()?;
                let name = self.devices.macro_name(doc, rack, number);
                Some(MacroEdit::Macro {
                    rack: *device,
                    number,
                    label: format!("Change \"{name}\""),
                })
            }
            bl_model::Command::MapMacro {
                rack,
                macro_number,
                target,
            } => {
                let device = doc.device(*rack)?;
                let first = device.macros.mappings_of(*macro_number).next().is_none();
                let before = first
                    .then(|| self.devices.target_value(doc, *target))
                    .flatten();
                Some(MacroEdit::Mapping {
                    rack: *rack,
                    number: *macro_number,
                    target: *target,
                    first: before,
                })
            }
            bl_model::Command::SetMappingRange { rack, target, .. }
            | bl_model::Command::InvertMapping { rack, target } => Some(MacroEdit::Target {
                rack: *rack,
                target: *target,
            }),
            bl_model::Command::RandomizeMacros { rack, .. } => {
                Some(MacroEdit::Randomize { rack: *rack })
            }
            bl_model::Command::SetMacroStyle {
                rack, macro_number, ..
            } => Some(MacroEdit::Style {
                rack: *rack,
                number: *macro_number,
            }),
            bl_model::Command::LaunchVariation { rack, .. } => {
                Some(MacroEdit::Variation { rack: *rack })
            }
            _ => None,
        }
    }

    /// The commands that write the values `edit` gives the parameters it
    /// maps, after the edit itself has run.
    fn macro_edit_commands(&self, edit: &MacroEdit) -> Vec<bl_model::Command> {
        let doc = self.editor.document();
        match edit {
            MacroEdit::Macro { rack, number, .. } => {
                doc.device(*rack).map_or_else(Vec::new, |rack| {
                    self.devices.macro_commands(doc, rack, *number)
                })
            }
            MacroEdit::Mapping {
                rack,
                number,
                target,
                first,
            } => {
                let Some(device) = doc.device(*rack) else {
                    return Vec::new();
                };
                if let Some(value) = first {
                    // R6. The macro moves to the position that gives the
                    // parameter the value it has, so nothing jumps.
                    let Some(info) = self.devices.target_info(doc, *target) else {
                        return Vec::new();
                    };
                    let position = info.travel.position(*value);
                    // Spec: macros R22. A stepped macro takes only its
                    // steps, so the macro moves to the nearest step, and
                    // the parameter takes the value that step gives.
                    let step = device.macros.setting(*number).position_of(position);
                    let mut commands = vec![bl_model::Command::SetMappedValue {
                        target: bl_model::MacroTarget::Device {
                            device: *rack,
                            param: bl_model::macro_param(*number),
                        },
                        value: step,
                    }];
                    if step != position
                        && let Some(mapping) = device.macros.mapping(*target)
                    {
                        commands.push(bl_model::Command::SetMappedValue {
                            target: *target,
                            value: info.travel.value(mapping.position(step)),
                        });
                    }
                    commands
                } else {
                    self.devices.target_commands(doc, device, *target)
                }
            }
            MacroEdit::Target { rack, target } => doc.device(*rack).map_or_else(Vec::new, |rack| {
                self.devices.target_commands(doc, rack, *target)
            }),
            MacroEdit::Randomize { rack } => doc
                .device(*rack)
                .map_or_else(Vec::new, |rack| self.devices.randomized_commands(doc, rack)),
            MacroEdit::Style { rack, number } => doc.device(*rack).map_or_else(Vec::new, |rack| {
                self.devices.macro_commands(doc, rack, *number)
            }),
            MacroEdit::Variation { rack } => doc.device(*rack).map_or_else(Vec::new, |rack| {
                (1..=bl_model::MACROS)
                    .filter(|number| !rack.macros.setting(*number).variation_excluded)
                    .flat_map(|number| self.devices.macro_commands(doc, rack, number))
                    .collect()
            }),
        }
    }

    pub fn prepare_edit(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        self.check_warp_files(id, &args)?;
        let args = self.clamp_to_range(id, args);
        let args = self.prepare_device_role(id, args)?;
        let args = self.prepare_scale_clips(id, args)?;
        let args = self.prepare_groove_clips(id, args)?;
        // Spec: tunings R15. A tuning set or removed without `retune`
        // follows Retune Set On Loading Tuning Systems.
        let args = self.with_retune(id, args);
        self.prepare_warping(id, args)
    }

    /// A device parameter's value, set directly or at an automation
    /// breakpoint, is cut to the parameter's range: the document doesn't
    /// hold the descriptors, so this is done here, as the app knows them.
    fn clamp_to_range(&self, id: &str, mut args: Value) -> Value {
        use bl_model::Command;
        let (device, param, value) = match model::command_from_args(id, args.clone()) {
            Ok(Command::SetDeviceParam {
                device,
                param,
                value,
            })
            | Ok(Command::SetAutomationBreakpoint {
                target: AutomationTarget::Device { device, param },
                value,
                ..
            }) => (device, param, value),
            _ => return args,
        };
        let descriptor = self
            .editor
            .document()
            .device(device)
            .and_then(|device| self.devices.description(&device.kind))
            .and_then(|description| description.param(param));
        if let Some(descriptor) = descriptor.filter(|_| value.is_finite()) {
            args["value"] = json!(value.clamp(descriptor.min(), descriptor.max()));
        }
        args
    }

    /// Spec: scales R5. `clip.set_scale` names the clips it sets; left out,
    /// it sets the selected ones.
    fn prepare_scale_clips(&self, id: &str, mut args: Value) -> Result<Value, ShellError> {
        if id != scales_ids::SET_CLIP_SCALE {
            return Ok(args);
        }
        let Some(object) = args.as_object_mut() else {
            return Ok(args);
        };
        if object.contains_key("clips") {
            return Ok(args);
        }
        let clips = self.scale_targets();
        if clips.is_empty() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "no clip is selected".to_owned(),
            });
        }
        object.insert("clips".to_owned(), json!(clips));
        Ok(args)
    }

    /// Spec: warping R7, R11. A warp marker edit on several clips needs
    /// files as long as each other's, and a marker can't leave its file; the
    /// document doesn't hold the media, so the check is made here, when the
    /// media store knows the files' lengths.
    pub(super) fn check_warp_files(&self, id: &str, args: &Value) -> Result<(), ShellError> {
        use bl_model::Command;
        let refuse = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let Ok(command) = model::command_from_args(id, args.clone()) else {
            return Ok(());
        };
        let document = self.editor.document();
        // Each clip's file length in seconds, where the store knows it.
        let length = |clip: &bl_model::ClipId| {
            let Some(bl_model::ClipData::Audio(audio)) = document.clip(*clip).map(|c| &c.data)
            else {
                return None;
            };
            self.file_seconds(audio)
        };
        let (clips, places) =
            match &command {
                Command::InsertWarpMarker { clips, .. }
                | Command::ShiftWarpMarker { clips, .. } => (clips, true),
                Command::MoveWarpMarkers { clips, .. }
                | Command::DeleteWarpMarkers { clips, .. } => (clips, false),
                _ => return Ok(()),
            };
        let lengths: Vec<Option<f64>> = clips.iter().map(length).collect();
        let known: Vec<f64> = lengths.iter().flatten().copied().collect();
        if known.len() == clips.len()
            && known
                .windows(2)
                .any(|pair| (pair[0] - pair[1]).abs() > 1e-9)
        {
            return Err(refuse(
                "the clips' files aren't all as long as each other's".to_owned(),
            ));
        }
        if !places {
            return Ok(());
        }
        for (&clip, length) in clips.iter().zip(lengths) {
            let Some(bl_model::ClipData::Audio(audio)) = document.clip(clip).map(|c| &c.data)
            else {
                continue;
            };
            let at = match &command {
                // A shift moves a marker from where it is.
                Command::ShiftWarpMarker { marker, by, .. } => {
                    audio.warp_markers.get(*marker).map(|m| m.time + by)
                }
                Command::InsertWarpMarker {
                    time: Some(time), ..
                } => Some(*time),
                // A marker put on a beat goes at the time the map places it.
                Command::InsertWarpMarker {
                    beat: Some(beat), ..
                } => audio
                    .warp_map()
                    .ok()
                    .map(|map| map.beat_to_time(beat.get() as f64)),
                _ => None,
            };
            if let (Some(at), Some(length)) = (at, length)
                && at > length + 1e-9
            {
                return Err(refuse(format!(
                    "the marker would leave the file, which is {length} seconds long"
                )));
            }
        }
        Ok(())
    }

    /// Runs an automation editing command, on the lanes the selection
    /// covers (the automation editing spec, R5, R7).
    pub(super) fn run_automation(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let before = self.editor.document().clone();
        let result = automation::run(
            id,
            args,
            &mut self.editor,
            &mut self.arrangement,
            &mut self.envelope_clipboard,
            &self.devices,
        )
        .map_err(|failure| edit_error(id, failure));
        if *self.editor.document() != before {
            self.dirty = true;
            self.document_changed();
        }
        self.touch();
        result
    }

    /// Runs one of the Envelope Editor's editing commands, on the clip's
    /// envelope (the clip envelopes spec, R12, R13).
    pub(super) fn run_clip_envelope_edit(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let before = self.editor.document().clone();
        let result = crate::clip_envelope_edit::run(
            id,
            args,
            &mut self.editor,
            &mut self.clip_view,
            &self.arrangement,
            &mut self.envelope_clipboard,
            &self.devices,
        )
        .map_err(|failure| edit_error(id, failure));
        if *self.editor.document() != before {
            self.dirty = true;
            self.document_changed();
        }
        self.touch();
        result
    }

    /// Runs one of the Clip View's note edits, as one undo step.
    pub(super) fn run_note_edit(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        // Spec: piano-roll. The Clip View's note edits.
        let before = self.editor.document().clone();
        let result = clip_view::run(
            id,
            args,
            &mut self.editor,
            &mut self.clip_view,
            &self.arrangement,
        )
        .map_err(|failure| edit_error(id, failure));
        if *self.editor.document() != before {
            self.dirty = true;
            self.document_changed();
        }
        // Spec: piano-roll R35. An edit in the note editor pauses Follow
        // until playback starts or stops.
        if result.is_ok() && clip_view::pauses_follow(id) {
            self.clip_view.note_follow(true);
        }
        self.touch();
        result
    }

    /// Runs the commands of the model registry: edits to the set, with the automation they record.
    pub(super) fn run_model(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        // Spec: midi-tools R21. Undo and redo close the open MIDI Tool
        // application.
        if matches!(id, model::UNDO | model::REDO | model::GO_TO_STEP) {
            self.clip_view.tools.close();
        }
        let args = self.prepare_edit(id, args)?;
        let args_floating = args.get("floating") == Some(&Value::Bool(true));
        // Spec: automation R17. With Automation Arm on and the transport
        // recording, a control move is noted for the envelope too.
        // Spec: session-to-arrangement R5. A Session clip that has taken a
        // track over writes the moves too, whether or not Automation Arm
        // is on.
        let moved = ((self.arrangement.automation_arm() || !self.session_over.is_empty())
            && self.automation_recording)
            .then(|| model::command_from_args(id, args.clone()).ok())
            .flatten()
            .map(|command| {
                crate::automation_record::moved_all(self.editor.document(), &command)
                    .into_iter()
                    .map(|(track, target, value)| {
                        // The value the control had before the move, which
                        // Write starts the envelope from (automation R20).
                        let before = crate::modulation::control_value(
                            self.editor.document(),
                            &self.devices,
                            track,
                            &target,
                        )
                        .unwrap_or(value);
                        (track, target, before, value)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // Spec: session-automation R1. With Automation Arm on and a
        // Session recording running, a control move is noted for the
        // clip it goes into.
        let session_moved: Vec<(TrackId, AutomationTarget, f64, f64)> =
            (self.arrangement.automation_arm() && self.session_run.is_some())
                .then(|| model::command_from_args(id, args.clone()).ok())
                .flatten()
                .map(|command| {
                    crate::automation_record::moved_all(self.editor.document(), &command)
                        .into_iter()
                        .filter_map(|(track, target, value)| {
                            let before = crate::modulation::control_value(
                                self.editor.document(),
                                &self.devices,
                                track,
                                &target,
                            )?;
                            Some((track, target, before, value))
                        })
                        .collect()
                })
                .unwrap_or_default();
        let changes = self.run_document_edit(id, args)?;
        // Spec: sample-editor R11. An edit in the Sample Editor pauses
        // Follow until playback starts or stops.
        if sample_edit_command(id) {
            self.clip_view.sample_follow(true);
        }
        if clip_view::pauses_follow(id) {
            self.clip_view.note_follow(true);
        }
        for (track, target, before, value) in moved {
            self.automation_take.write(
                track,
                target,
                self.arrangement.transport().position,
                value,
                before,
            );
        }
        for (track, target, before, value) in session_moved {
            self.note_session_move(track, target, before, value);
        }
        if id == bl_model::registry::RE_ENABLE_AUTOMATION {
            // Spec: session-view R23. A launched scene's tempo and
            // signature hold until Re-Enable Automation.
            let _ = self
                .audio
                .send_session(engine_session::SessionCommand::ClearSceneOverride);
        }
        if id == device_ids::SET_FLOATING && args_floating {
            // A device floated shows the windows again (R33).
            self.plug_in_windows = true;
            self.apply_plug_in_windows();
        }
        self.dirty = true;
        self.document_changed();
        self.touch();
        Ok(to_value(&changes))
    }

    /// Runs the comping commands.
    pub(super) fn run_comping(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        // Spec: comping R4, R5, R17 to R21.
        let before = self.editor.document().clone();
        let edge_fades = self.create_fades_on_clip_edges();
        let result = crate::arrangement::comping::run(
            id,
            args,
            &mut self.editor,
            &mut self.arrangement,
            edge_fades,
        )
        .map_err(|failure| edit_error(id, failure));
        if *self.editor.document() != before {
            self.dirty = true;
            self.document_changed();
        }
        self.touch();
        result
    }

    /// Runs the editing commands.
    pub(super) fn run_edit(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        // Spec: automation-editing R5. With the time selection in
        // automation lanes, the edit commands act on the envelopes
        // alone, and the clips stay as they are.
        if let Some(automation_id) = automation_command(id, &self.arrangement) {
            return self.run_automation(automation_id, args);
        }
        let before = self.editor.document().clone();
        self.leave_set_before_paste(id);
        let result = if id == bl_shell_spec::editing::INSERT_EMPTY_MIDI_CLIP {
            editing::insert_empty_midi_clips(
                args,
                &mut self.editor,
                &self.arrangement,
                &self.prefs.look,
            )
        } else {
            editing::run(
                id,
                args,
                &mut self.editor,
                &mut self.arrangement,
                &mut self.clipboard,
                self.prefs.create_fades_on_clip_edges.0,
            )
        }
        .map_err(|failure| edit_error(id, failure));
        if *self.editor.document() != before {
            self.dirty = true;
            self.document_changed();
        }
        if result.is_ok() {
            self.note_clipboard_after(id);
        }
        // Spec: second-window R20. Pasted Session clips that weren't placed.
        if let Ok(value) = &result
            && let Some(skipped) = value.get("skipped").and_then(Value::as_u64)
            && let Some(message) = crate::session_clips::skipped_message(skipped as usize)
        {
            self.set_message(message, false);
        }
        self.touch();
        result
    }

    /// Before a paste, takes from a clipboard copied in another set what
    /// only means something there (the set exchange spec, R6).
    pub(super) fn leave_set_before_paste(&mut self, id: &str) {
        if is_paste(id) && self.clipboard_set != self.editor_generation {
            if let Some(clipboard) = self.clipboard.as_mut() {
                clipboard.leave_set();
            }
            self.clipboard_set = self.editor_generation;
        }
    }

    /// After a copy, notes which set the clipboard came from. After a paste,
    /// lists the files of the audio clips the clipboard holds in the open
    /// set's media table: the clipboard outlives a set, so the clips may come
    /// from one whose table doesn't hold their files.
    pub(super) fn note_clipboard_after(&mut self, id: &str) {
        if is_copy(id) {
            self.clipboard_set = self.editor_generation;
        }
        if !is_paste(id) {
            return;
        }
        let listed: BTreeSet<bl_model::ContentHash> = self
            .store
            .media_files()
            .into_iter()
            .map(|(hash, ..)| hash)
            .collect();
        let hashes = self
            .clipboard
            .as_ref()
            .map(Clipboard::media)
            .unwrap_or_default();
        for hash in hashes.difference(&listed) {
            if let Some(entry) = self.media.entry(bl_media::ContentHash::from_bytes(hash.0)) {
                self.store.add_media(*hash, &entry.path, entry.size);
            }
        }
    }

    /// Runs the arranger track's commands.
    pub(super) fn run_arranger(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        // Spec: arranger-track R1 to R12. Sections are part of the set; the
        // moves, copies and deletes are the …Time operations.
        let before = self.editor.document().clone();
        let edge_fades = self.create_fades_on_clip_edges();
        let result = crate::arrangement::arranger::run(
            id,
            args,
            &mut self.editor,
            &mut self.arrangement,
            edge_fades,
        )
        .map_err(|failure| edit_error(id, failure));
        if *self.editor.document() != before {
            self.dirty = true;
            self.document_changed();
        }
        self.touch();
        result
    }

    /// Runs the locator commands.
    pub(super) fn run_locator(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        // Spec: time-editing R1 to R13. The locators are part of the
        // set, and the jumps between them need the engine.
        let before = self.editor.document().clone();
        let launch = self.launch_settings();
        let result = crate::arrangement::locator::run(
            id,
            args,
            &mut self.editor,
            &mut self.arrangement,
            &launch,
            Some(&mut self.audio),
        )
        .map_err(|failure| edit_error(id, failure));
        if *self.editor.document() != before {
            self.dirty = true;
            self.document_changed();
        }
        self.touch();
        result
    }
}

/// Whether `id` puts the Arrangement's or the Session View's clips, scenes or
/// time on the clipboard.
pub(super) fn is_copy(id: &str) -> bool {
    use bl_shell_spec::{editing as arrangement, session_view as session};
    matches!(
        id,
        arrangement::COPY
            | arrangement::CUT
            | arrangement::COPY_TIME
            | arrangement::CUT_TIME
            | session::COPY
            | session::CUT
    )
}

/// Whether `id` pastes what the clipboard holds.
pub(super) fn is_paste(id: &str) -> bool {
    use bl_shell_spec::{editing as arrangement, session_view as session};
    matches!(
        id,
        arrangement::PASTE | arrangement::PASTE_TIME | session::PASTE
    )
}
