//! Launching clips and scenes in the Session View.

use super::{Shell, ShellError, to_value};
use bl_commands::NoArgs;
use bl_engine::session as engine_session;
use bl_model::LaunchSettings;
use bl_model::TrackId;
use bl_shell_spec::session_launch as session_launch_ids;
use bl_shell_spec::session_view as session_ids;
use serde_json::Value;
use serde_json::json;

use super::MainView;
use crate::auto_warp;
use crate::session_view::SessionViewError;

impl Shell {
    /// Select on Launch and Select Next Scene on Launch (Spec:
    /// launch-quantisation R16): a launched slot that holds a clip selects
    /// itself, and a launched scene selects the scene after it, while the
    /// preferences are on.
    pub(super) fn select_on_launch(&mut self, command: engine_session::SessionCommand) {
        let document = self.editor.document();
        let target = match command {
            engine_session::SessionCommand::Launch { track, scene }
                if self.prefs.launch.select_on_launch && document.slot(track, scene).is_some() =>
            {
                json!({ "track": track, "scene": scene })
            }
            engine_session::SessionCommand::LaunchScene(scene)
                if self.prefs.launch.select_next_scene_on_launch =>
            {
                let next = document
                    .scene_position(scene)
                    .and_then(|at| document.scenes().nth(at + 1));
                let Some(next) = next else { return };
                json!({ "scene": next.id })
            }
            _ => return,
        };
        // A selection that can't be made leaves the one there is.
        let _ = self.run(session_ids::SELECT, target);
    }

    /// The launch commands Enter runs: one for each selected slot that
    /// holds a clip and one for each selected scene.
    pub(super) fn launch_targets(&self) -> Vec<(&'static str, Value)> {
        let state = self.session.state();
        let document = self.editor.document();
        let slots = state
            .slots
            .iter()
            .filter(|slot| document.slot(slot.track, slot.scene).is_some())
            .map(|slot| {
                (
                    "session.launch",
                    json!({ "track": slot.track, "scene": slot.scene }),
                )
            });
        let scenes = state
            .scenes
            .iter()
            .map(|scene| ("session.launch_scene", json!({ "scene": scene })));
        slots.chain(scenes).collect()
    }

    /// Runs one of the launch quantisation's commands (WP-3.2) and the
    /// Session View's Cmd+Enter stop: the quantisation and its switch are
    /// saved with the set, beside the song, and the engine that plays the
    /// set hears every change.
    pub(super) fn run_session_launch(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let current = self.store.launch();
        let settings = match id {
            session_launch_ids::SET_QUANTIZATION => {
                let session_launch_ids::SetQuantizationArgs { quantization } =
                    Self::args(id, args)?;
                LaunchSettings {
                    quantization,
                    ..current
                }
            }
            session_launch_ids::SMALLER_QUANTIZATION => {
                Self::args::<NoArgs>(id, args)?;
                LaunchSettings {
                    quantization: current.quantization.smaller(),
                    ..current
                }
            }
            session_launch_ids::LARGER_QUANTIZATION => {
                Self::args::<NoArgs>(id, args)?;
                LaunchSettings {
                    quantization: current.quantization.larger(),
                    ..current
                }
            }
            session_launch_ids::TRIPLET_QUANTIZATION => {
                Self::args::<NoArgs>(id, args)?;
                LaunchSettings {
                    quantization: current.quantization.triplet(),
                    ..current
                }
            }
            session_launch_ids::SET_QUANTIZATION_ENABLED => {
                let session_launch_ids::SetQuantizationEnabledArgs { enabled } =
                    Self::args(id, args)?;
                LaunchSettings {
                    enabled: enabled.unwrap_or(!current.enabled),
                    ..current
                }
            }
            session_launch_ids::SET_FOLLOW_ACTIONS_ENABLED => {
                let session_launch_ids::SetFollowActionsEnabledArgs { enabled } =
                    Self::args(id, args)?;
                LaunchSettings {
                    follow_actions: enabled.unwrap_or(!current.follow_actions),
                    ..current
                }
            }
            session_launch_ids::STOP_SELECTED => {
                Self::args::<NoArgs>(id, args)?;
                // Spec: launch-quantisation R6. Every track with a selected
                // slot stops on the grid, as Cmd+Enter does.
                let mut tracks: Vec<TrackId> = Vec::new();
                for slot in self.session.selected_slots() {
                    if !tracks.contains(&slot.track) {
                        tracks.push(slot.track);
                    }
                }
                if tracks.is_empty() {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select a clip slot first".to_owned(),
                    });
                }
                for track in tracks {
                    self.audio
                        .send_session(engine_session::SessionCommand::StopTrack(track))
                        .map_err(|reason| ShellError::Unavailable {
                            command: id.to_owned(),
                            reason,
                        })?;
                }
                return Ok(Value::Null);
            }
            session_launch_ids::LAUNCH_SELECTED => {
                Self::args::<NoArgs>(id, args)?;
                // Spec: session-view R32. Enter launches what the grid has
                // selected, one launch command each.
                let targets = self.launch_targets();
                if targets.is_empty() {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "select a clip or a scene first".to_owned(),
                    });
                }
                for (command, args) in targets {
                    self.run(command, args)?;
                }
                return Ok(Value::Null);
            }
            _ => return Err(ShellError::UnknownCommand(id.to_owned())),
        };
        self.store.set_launch(settings);
        self.audio.set_launch_settings(settings);
        self.touch();
        Ok(to_value(&settings))
    }

    /// Runs the commands that reach the engine's clip launching.
    pub(super) fn run_engine_session(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        // Spec: launch-quantisation R9. The grid's launch buttons, the
        // status row, and agents all reach the engine's launching here.
        let command =
            engine_session::command_from_args(id, Self::object(args)).map_err(|error| {
                ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: error.to_string(),
                }
            })?;
        self.audio
            .send_session(command)
            .map_err(|reason| ShellError::Unavailable {
                command: id.to_owned(),
                reason,
            })?;
        if let engine_session::SessionCommand::Launch { scene, .. }
        | engine_session::SessionCommand::LaunchScene(scene) = command
        {
            self.last_launched = Some(scene);
        }
        self.select_on_launch(command);
        self.re_enable_launched_automation(command);
        if let engine_session::SessionCommand::LaunchScene(scene) = command {
            // Spec: session-recording R10. Start Recording on Scene
            // Launch.
            self.record_on_scene_launch(scene);
        }
        Ok(Value::Null)
    }

    /// Runs the Session View's clipboard and the copies between the views.
    fn run_session_clips(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let before = self.editor.document().clone();
        let result = crate::session_clips::run(
            id,
            args,
            &mut self.editor,
            &mut self.session,
            &mut self.arrangement,
            &mut self.clipboard,
            self.prefs.create_fades_on_clip_edges.0,
        )
        .map_err(|failure| super::edit_error(id, failure));
        if *self.editor.document() != before {
            self.dirty = true;
            self.document_changed();
        }
        if let Ok(outcome) = &result
            && let Some(message) = &outcome.message
        {
            self.set_message(message.clone(), false);
        }
        self.touch();
        result.map(|outcome| outcome.value)
    }

    /// Runs the Session View's commands.
    pub(super) fn run_session_view(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if crate::session_clips::handles(id) {
            return self.run_session_clips(id, args);
        }
        // Spec: session-view. The grid's selection and scroll are view
        // state; its edits run the set's scene and slot commands as one
        // undo step.
        let columns = crate::mixer_view::columns(self.editor.document(), &self.arrangement);
        let scenes: Vec<bl_model::SceneId> = self
            .editor
            .document()
            .scenes()
            .map(|scene| scene.id)
            .collect();
        let edge_fades = self.prefs.create_fades_on_clip_edges.0;
        let search = self.browser_view.search_folders();
        self.analysis_cache();
        // Spec: session-view R24. Capture and Insert Scene copies the
        // clips the engine plays, whichever scene they sit in.
        let capture = crate::session_view::Capture {
            playing: self
                .session_states
                .iter()
                .filter(|slot| slot.state != bl_engine::SlotState::Queued)
                .map(|slot| (slot.track, slot.scene))
                .collect(),
            last_launched: self.last_launched,
            tempo: self.current_tempo(),
            signature: self.current_signature(),
        };
        let outcome = self
            .session
            .run(
                id,
                args,
                crate::session_view::Context {
                    editor: &mut self.editor,
                    media: &self.media,
                    columns: &columns,
                    scenes: &scenes,
                    edge_fades,
                    warping: auto_warp::Warping {
                        prefs: self.prefs.warp,
                        analysis: self.analysis.as_ref(),
                    },
                    launch_mode: self.prefs.launch.default_mode,
                    capture,
                    launch_quantization: self.prefs.launch.default_quantization,
                    search: &search,
                },
            )
            .map_err(|error| match error {
                SessionViewError::InvalidArgs(message) => ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message,
                },
                SessionViewError::Unavailable(reason) => ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                },
                SessionViewError::Failed(message) => ShellError::Failed {
                    command: id.to_owned(),
                    message,
                },
            })?;
        for file in &outcome.media {
            self.store.add_media(file.hash, &file.path, file.size);
        }
        if outcome.document_changed {
            self.dirty = true;
            self.document_changed();
        }
        if let Some(message) = outcome.message {
            self.set_message(message, false);
        }
        if let Some(scene) = outcome.captured {
            // Spec: session-view R24. The playing clips carry on in the
            // copies. Without an engine nothing plays, and the scene
            // is just inserted.
            let _ = self
                .audio
                .send_session(engine_session::SessionCommand::MoveToScene(scene));
        }
        if id == session_ids::FOCUS {
            // Spec: session-view. Navigate ▸ Session View shows it and
            // moves the focus into it.
            self.focus_main_view(MainView::Session);
        }
        if id != session_ids::STATE {
            self.touch();
        }
        Ok(outcome.value)
    }
}
