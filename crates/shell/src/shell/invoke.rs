//! Invoking a command as a person does: keys, menus and the arguments they imply.

use super::{
    DELAY_COMPENSATION, Effect, Invocation, LOCK_ENVELOPES, REDUCED_LATENCY_MONITORING,
    SOLO_IN_PLACE, Shell, ShellError,
};
use crate::commands as ids;
use crate::mixer_controls;
use crate::mixer_controls::Switch;
use bl_model::TrackId;
use bl_model::TrackKind;
use bl_shell_spec::arrangement as view_ids;
use bl_shell_spec::browser as browser_ids;
use bl_shell_spec::devices as device_ids;
use bl_shell_spec::file_manager as file_manager_ids;
use bl_shell_spec::locator as locator_ids;
use bl_shell_spec::session_launch as session_launch_ids;
use bl_shell_spec::tracks as track_ids;
use bl_shell_spec::tunings as tuning_ids;
use serde_json::Value;
use serde_json::json;

impl Shell {
    /// The arguments a person's invocation implies: a new track's ID, name,
    /// and place, for the insert-track keys.
    pub(super) fn filled_args(&self, id: &str, args: Value) -> Result<Value, ShellError> {
        let Value::Object(mut fields) = args else {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: "the arguments must be a JSON object".to_owned(),
            });
        };
        if id == "track.create" && !fields.contains_key("id") {
            let kind: TrackKind = fields
                .get("kind")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or(TrackKind::Audio);
            let doc = self.editor.document();
            // Spec: app-shell R10. The new track goes directly after the
            // selected track, or after the last of several selected tracks,
            // in the list it belongs to; with no such track selected, after
            // the last track, as before.
            let anchor = self
                .arrangement
                .selected_tracks()
                .iter()
                .rev()
                .copied()
                .find(|&track| match doc.track(track) {
                    Some(track) if kind == TrackKind::Return => track.kind() == TrackKind::Return,
                    Some(track) => track.kind().can_be_grouped(),
                    None => false,
                });
            let (parent, index) = match anchor {
                Some(anchor) if kind == TrackKind::Return => (
                    None,
                    doc.returns()
                        .position(|r| r == anchor)
                        .map_or(doc.returns().len(), |i| i + 1),
                ),
                Some(anchor) => {
                    let parent = doc.parent(anchor);
                    let index = match parent {
                        Some(group) => {
                            let children: Vec<TrackId> =
                                doc.track(group).map_or_default(|g| g.children().collect());
                            children
                                .iter()
                                .position(|t| *t == anchor)
                                .map_or(children.len(), |i| i + 1)
                        }
                        None => doc
                            .top_level()
                            .position(|t| t == anchor)
                            .map_or(doc.top_level().len(), |i| i + 1),
                    };
                    (parent, index)
                }
                None => (
                    None,
                    if kind == TrackKind::Return {
                        doc.returns().len()
                    } else {
                        doc.top_level().len()
                    },
                ),
            };
            let name = if kind == TrackKind::Return {
                format!("{} Return", crate::routing::return_letter(index))
            } else {
                let word = match kind {
                    TrackKind::Midi => "MIDI",
                    TrackKind::Group => "Group",
                    _ => "Audio",
                };
                format!("{} {word}", index + 1)
            };
            // A placeholder that fits the schema; `invoke` issues the real ID.
            fields.insert("id".to_owned(), json!(1));
            if let Some(parent) = parent {
                fields.insert("parent".to_owned(), json!(parent));
            }
            fields.entry("name").or_insert_with(|| json!(name));
            fields.entry("index").or_insert_with(|| json!(index));
        }
        if device_ids::ON_SELECTED.contains(&id) && !fields.contains_key("device") {
            // Spec: device-chains R20. A key or menu item acts on the
            // selected device, switching it where it switches.
            let device = self
                .devices
                .selected()
                .ok_or_else(|| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "select a device first".to_owned(),
                })?;
            let current = self.editor.document().device(device);
            fields.insert("device".to_owned(), json!(device));
            if id == "device.set_show_preset_name" && !fields.contains_key("shown") {
                let on = current.is_some_and(|d| d.show_preset_name);
                fields.insert("shown".to_owned(), json!(!on));
            }
            if id == device_ids::SET_FLOATING && !fields.contains_key("floating") {
                let on = current.is_some_and(|d| d.floating);
                fields.insert("floating".to_owned(), json!(!on));
            }
            if id == "device.set_enabled" && !fields.contains_key("enabled") {
                let on = current.is_some_and(|d| d.enabled);
                fields.insert("enabled".to_owned(), json!(!on));
            }
        }
        if id == "transport.set_midi_overdub" && !fields.contains_key("enabled") {
            // Options ▸ MIDI Arrangement Overdub switches the setting to the
            // other state.
            let on = self.editor.document().settings().midi_overdub;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == LOCK_ENVELOPES && !fields.contains_key("enabled") {
            // Spec: automation-editing R8. Options ▸ Lock Envelopes switches
            // the setting to the other state.
            let on = self.editor.document().settings().lock_envelopes;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == SOLO_IN_PLACE && !fields.contains_key("enabled") {
            // Spec: accessibility R9. Options ▸ Solo in Place switches the
            // setting to the other state.
            let on = self.editor.document().settings().solo_in_place;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == DELAY_COMPENSATION && !fields.contains_key("enabled") {
            // Options ▸ Delay Compensation switches the setting to the
            // other state (the delay-compensation spec, R7).
            let on = self.editor.document().settings().delay_compensation;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == REDUCED_LATENCY_MONITORING && !fields.contains_key("enabled") {
            // Options ▸ Reduced Latency When Monitoring switches the
            // setting to the other state (the delay-compensation spec, R8).
            let on = self.editor.document().settings().reduced_latency_monitoring;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == session_launch_ids::SET_QUANTIZATION_ENABLED && !fields.contains_key("enabled") {
            // Spec: launch-quantisation R3. Options ▸ Launch Quantization
            // Enabled switches the setting to the other state.
            let on = self.store.launch().enabled;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == "transport.set_metronome_enabled" && !fields.contains_key("enabled") {
            let on = self.editor.document().settings().metronome.enabled;
            fields.insert("enabled".to_owned(), json!(!on));
        }
        if id == "transport.set_metronome_only_while_recording"
            && !fields.contains_key("only_while_recording")
        {
            let on = self
                .editor
                .document()
                .settings()
                .metronome
                .only_while_recording;
            fields.insert("only_while_recording".to_owned(), json!(!on));
        }
        if let Some(switch) = Switch::of_command(id)
            && !fields.contains_key("track")
        {
            // Spec: mixer-view R9. A title-bar key, or F1 to F8, names no
            // track: it acts on the selected tracks, or the numbered one,
            // toggling the first one's switch.
            let tracks = self.key_tracks(&mut fields);
            let invocations = mixer_controls::toggled(self.editor.document(), switch, &tracks);
            let first = invocations.first().ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: "no selected track has this switch".to_owned(),
            })?;
            return Ok(first.args.clone());
        }
        Ok(Value::Object(fields))
    }

    /// Runs a command as a person asked for it, from a menu, a key, or a
    /// control: fills in arguments the person didn't give, asks before
    /// losing unsaved changes, and asks for a file where one is needed (as
    /// [`Effect`]s). Errors also show in the Status Bar.
    ///
    /// # Errors
    ///
    /// As [`Shell::run`], when the command runs straight away.
    pub fn invoke(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let result = self.invoke_inner(id, args);
        if let Err(error) = &result {
            self.set_message(error.to_string(), true);
        }
        result
    }

    pub(super) fn invoke_inner(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let has = |key: &str| args.get(key).is_some();
        match id {
            ids::SET_NEW | ids::APP_QUIT if self.unsaved_changes() => {
                self.effects.push(Effect::ConfirmDiscard {
                    then: Invocation::new(id, args),
                });
                Ok(Value::Null)
            }
            ids::SET_OPEN if !has("path") => {
                if self.unsaved_changes() {
                    self.effects.push(Effect::ConfirmDiscard {
                        then: Invocation::new(id, args),
                    });
                } else {
                    self.effects.push(Effect::PickOpenPath {
                        discard_changes: false,
                    });
                }
                Ok(Value::Null)
            }
            ids::SET_OPEN if self.unsaved_changes() => {
                self.effects.push(Effect::ConfirmDiscard {
                    then: Invocation::new(id, args),
                });
                Ok(Value::Null)
            }
            ids::SET_SAVE if self.set_path.is_none() => {
                self.effects.push(Effect::PickSavePath { then: None });
                Ok(Value::Null)
            }
            ids::SET_SAVE_AS if !has("path") => {
                self.effects.push(Effect::PickSavePath { then: None });
                Ok(Value::Null)
            }
            browser_ids::ADD_FOLDER if !has("path") => {
                self.effects.push(Effect::PickFolder);
                Ok(Value::Null)
            }
            // Spec: tunings R15. With Retune Set On Loading Tuning Systems
            // on, a person who loads a tuning, or removes it, is asked
            // first; Cancel changes nothing.
            tuning_ids::LOAD | tuning_ids::REMOVE
                if !has("retune")
                    && self.tuning_change_asks()
                    && (id == tuning_ids::LOAD
                        || self.editor.document().settings().tuning.is_some()) =>
            {
                self.effects.push(Effect::ConfirmRetune {
                    then: Invocation::new(id, args),
                });
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_RETUNE_ON_TUNING_LOAD if !has("on") => {
                // Options ▸ Retune Set On Loading Tuning Systems switches
                // the setting to the other state.
                let on = !self.prefs.retune_on_tuning_load;
                self.run(id, json!({ "on": on }))
            }
            ids::SETTINGS_SET_SNAP_AUTOMATION_TO_GRID if !has("on") => {
                let on = !self.prefs.snap_automation_to_grid.0;
                self.run(id, json!({ "on": on }))
            }
            ids::SETTINGS_SET_TEMPORARY_FOLDER if !has("path") => {
                self.effects.push(Effect::PickTemporaryFolder);
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_USER_LIBRARY if !has("path") => {
                self.effects.push(Effect::PickUserLibrary);
                Ok(Value::Null)
            }
            ids::SETTINGS_SET_SAMPLE_EDITOR if !has("path") => {
                self.effects.push(Effect::PickSampleEditor);
                Ok(Value::Null)
            }
            file_manager_ids::SET_SEARCH_FOLDER if !has("folder") => {
                // Spec: file-management R14. The Set Folder… button asks the
                // person for the folder the search looks in.
                self.effects.push(Effect::PickSearchFolder);
                Ok(Value::Null)
            }
            _ if self.is_sample_key(id, &args) => {
                // Spec: sample-editor R10. A key or a menu item acts on what
                // the Sample Editor has selected.
                let (id, args) = self.complete_sample_command(id, args)?;
                self.run(&id, args)
            }
            _ if !has("track") && Switch::of_command(id).is_some() => {
                let switch = Switch::of_command(id).expect("just checked");
                self.toggle_tracks(id, switch, args)
            }
            "track.delete" if !has("track") => {
                // Spec: track-context-menu R4. Delete acts on every selected
                // track, as one undo step.
                let tracks = self.deletable_tracks();
                if tracks.is_empty() {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select a track first".to_owned(),
                    });
                }
                let opened = !self.gesture;
                if opened {
                    self.begin_gesture(track_ids::DELETE_STEP);
                }
                let mut result = Ok(Value::Null);
                for track in tracks {
                    // A group's delete took its selected members with it.
                    if self.editor.document().track(track).is_none() {
                        continue;
                    }
                    result = self.run(id, json!({ "track": track }));
                    if result.is_err() {
                        break;
                    }
                }
                if opened {
                    self.end_gesture();
                }
                result
            }
            "track.rename" if !has("name") => {
                let track = self
                    .renamable_track(&args)
                    .ok_or_else(|| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select a track first".to_owned(),
                    })?;
                self.effects.push(Effect::RenameTrack(track));
                self.touch();
                Ok(Value::Null)
            }
            "take_lane.rename" if !has("name") => {
                // Spec: comping R6, R28. Cmd-R and a take lane header's
                // Rename type the new name in the header.
                let lane = args
                    .get("lane")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .or_else(|| self.arrangement.selected_take_lanes().first().copied())
                    .ok_or_else(|| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select a take lane first".to_owned(),
                    })?;
                self.effects.push(Effect::RenameTakeLane(lane));
                self.touch();
                Ok(Value::Null)
            }
            "take_lane.delete_all" | "take_lane.delete_unused" if !has("tracks") => {
                // Spec: track-context-menu R10. They act on the selected
                // audio and MIDI tracks.
                let doc = self.editor.document();
                let tracks: Vec<TrackId> = self
                    .arrangement
                    .selected_tracks()
                    .iter()
                    .copied()
                    .filter(|track| {
                        doc.track(*track).is_some_and(|held| {
                            matches!(held.kind(), TrackKind::Audio | TrackKind::Midi)
                        })
                    })
                    .collect();
                if tracks.is_empty() {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select an audio or MIDI track first".to_owned(),
                    });
                }
                self.run(id, json!({ "tracks": tracks }))
            }
            locator_ids::RENAME if !has("name") => {
                // Spec: time-editing R8. Cmd-R and Edit ▸ Rename type the
                // new name in the scrub area.
                let locator =
                    self.renamable_locator(&args)
                        .ok_or_else(|| ShellError::Unavailable {
                            command: id.to_owned(),
                            reason: "select a locator first".to_owned(),
                        })?;
                self.effects.push(Effect::RenameLocator(locator));
                self.touch();
                Ok(Value::Null)
            }
            "device.rename" if !has("name") => {
                let args = self.filled_args(id, args)?;
                // Spec: macros R29. With a variation selected, Rename names
                // it, not the device.
                if let Some((rack, index)) = self.devices.variation_selected() {
                    self.effects.push(Effect::RenameVariation(rack, index));
                    self.touch();
                    return Ok(Value::Null);
                }
                // Spec: macros R14. With a macro selected, Rename names it,
                // not the device.
                if let Some((rack, number)) = self.devices.macro_selected() {
                    self.effects.push(Effect::RenameMacro(rack, number));
                    self.touch();
                    return Ok(Value::Null);
                }
                let device = serde_json::from_value(args["device"].clone()).map_err(|error| {
                    ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: error.to_string(),
                    }
                })?;
                self.effects.push(Effect::RenameDevice(device));
                self.touch();
                Ok(Value::Null)
            }
            "chain.rename" if !has("name") => {
                let chain = args
                    .get("chain")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .or_else(|| {
                        self.devices
                            .selected_chains(self.editor.document())
                            .first()
                            .copied()
                    })
                    .ok_or_else(|| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select a chain first".to_owned(),
                    })?;
                self.effects.push(Effect::RenameChain(chain));
                self.touch();
                Ok(Value::Null)
            }
            "rack.rename_macro" if !has("name") => {
                let (rack, number) =
                    self.devices
                        .macro_selected()
                        .ok_or_else(|| ShellError::Unavailable {
                            command: id.to_owned(),
                            reason: "select a macro first".to_owned(),
                        })?;
                self.effects.push(Effect::RenameMacro(rack, number));
                self.touch();
                Ok(Value::Null)
            }
            "device.duplicate" if !has("device") && self.devices.variation_selected().is_some() => {
                // Spec: macros R29. With a variation selected, Duplicate
                // copies it, not the device.
                let (rack, index) = self
                    .devices
                    .variation_selected()
                    .expect("checked to be selected");
                self.run(
                    "rack.duplicate_variation",
                    json!({ "rack": rack, "index": index }),
                )
            }
            "device.remove" if !has("device") && self.devices.variation_selected().is_some() => {
                // Spec: macros R29. With a variation selected, Delete removes
                // it, not the device.
                let (rack, index) = self
                    .devices
                    .variation_selected()
                    .expect("checked to be selected");
                self.run(
                    "rack.delete_variation",
                    json!({ "rack": rack, "index": index }),
                )
            }
            _ if device_ids::ON_SELECTED.contains(&id) && !has("device") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            SOLO_IN_PLACE | LOCK_ENVELOPES if !has("enabled") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            DELAY_COMPENSATION | REDUCED_LATENCY_MONITORING if !has("enabled") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            "transport.set_midi_overdub" if !has("enabled") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            "transport.set_metronome_enabled" if !has("enabled") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            "transport.set_metronome_only_while_recording" if !has("only_while_recording") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            _ if id == session_launch_ids::SET_QUANTIZATION_ENABLED && !has("enabled") => {
                let args = self.filled_args(id, args)?;
                self.run(id, args)
            }
            "track.create" if !has("id") => {
                let mut args = self.filled_args(id, args)?;
                let new: TrackId = self.editor.new_id();
                args["id"] = json!(new);
                let result = self.run(id, args)?;
                // Spec: app-shell R10. The new track is selected.
                self.arrangement
                    .run(
                        view_ids::SELECT_TRACKS,
                        json!({ "tracks": [new] }),
                        self.editor.document(),
                    )
                    .ok();
                Ok(result)
            }
            _ => self.run(id, args),
        }
    }
}
