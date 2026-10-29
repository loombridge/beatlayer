//! Launching clips and scenes in the Session View.

use super::{Effect, Shell, ShellError, to_value};
use bl_commands::NoArgs;
use bl_core::time::{SampleRate, Ticks};
use bl_engine::session as engine_session;
use bl_model::LaunchSettings;
use bl_model::SceneId;
use bl_model::TrackId;
use bl_shell_spec::session_launch as session_launch_ids;
use bl_shell_spec::session_view as session_ids;
use bl_shell_spec::session_view::{LaunchState, SlotLaunch, SlotRef};
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
            | engine_session::SessionCommand::LaunchAt { track, scene, .. }
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
        if id == "session.cancel_scene_launch" {
            // Spec: session-context-menus R10. With nothing of the scene
            // waiting for its line there is no launch to cancel.
            let scene = args
                .get("scene")
                .and_then(|scene| serde_json::from_value::<SceneId>(scene.clone()).ok());
            if scene.is_some_and(|scene| !self.scene_launch_queued(scene)) {
                return Err(ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "no clip of that scene is waiting to launch".to_owned(),
                });
            }
        }
        let command =
            engine_session::command_from_args(id, Self::object(args)).map_err(|error| {
                ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: error.to_string(),
                }
            })?;
        self.send_session(command)
            .map_err(|reason| ShellError::Unavailable {
                command: id.to_owned(),
                reason,
            })?;
        Ok(Value::Null)
    }

    /// Sends a Session View launch or stop to the engine, as the grid's
    /// buttons, keys and agents all do, with what a launch brings along:
    /// the selection the Select on Launch preferences move, the automation
    /// it re-enables, and the recording a scene launch starts.
    ///
    /// # Errors
    ///
    /// A message if no engine runs.
    pub fn send_session(&mut self, command: engine_session::SessionCommand) -> Result<(), String> {
        self.audio.send_session(command)?;
        if let engine_session::SessionCommand::Launch { scene, .. }
        | engine_session::SessionCommand::LaunchAt { scene, .. }
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
        Ok(())
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

    /// What the clipboard holds for the Session View to paste, for the
    /// context menus to offer Paste: clips (copied from slots, or Arrangement
    /// material) and scenes.
    #[must_use]
    pub fn session_clipboard(&self) -> (bool, bool) {
        use crate::shell::Clipboard;
        match self.clipboard() {
            Some(Clipboard::Slots(slots)) => (!slots.clips.is_empty(), false),
            Some(Clipboard::Material(material)) => (!material.lanes.is_empty(), false),
            Some(Clipboard::Scenes(scenes)) => (false, !scenes.scenes.is_empty()),
            _ => (false, false),
        }
    }

    /// Whether a clip of `scene` waits for its line of the launch
    /// quantisation grid, as the engine last published the slots.
    #[must_use]
    pub fn scene_launch_queued(&self, scene: SceneId) -> bool {
        self.session_states
            .iter()
            .any(|slot| slot.scene == scene && slot.state == bl_engine::SlotState::Queued)
    }

    /// Whether a Session View command can run on the selection now: the
    /// context menus' enabled states (session-context-menus R12). `None` for
    /// a command that always can.
    pub(super) fn can_session_view(&self, id: &str, args: &Value) -> Option<bool> {
        let doc = self.editor.document();
        let given = |key: &str| args.get(key).is_some();
        let clips = || {
            self.session
                .selected_slots()
                .iter()
                .any(|slot| doc.slot(slot.track, slot.scene).is_some())
        };
        let scenes = || !self.session.state().scenes.is_empty();
        Some(match id {
            session_ids::DELETE | session_ids::RENAME => clips() || scenes(),
            session_ids::DEACTIVATE
            | session_ids::CREATE_FOLLOW_ACTION_CHAIN
            | session_ids::TOGGLE_FOLLOW_ACTIONS => clips(),
            session_ids::DUPLICATE_SCENE => given("scene") || scenes(),
            session_ids::INSERT_EMPTY_MIDI_CLIP => {
                let slot = match (args.get("track"), args.get("scene")) {
                    (Some(track), Some(scene)) => serde_json::from_value(track.clone())
                        .ok()
                        .zip(serde_json::from_value(scene.clone()).ok()),
                    _ => self
                        .session
                        .selected_slots()
                        .first()
                        .map(|slot| (slot.track, slot.scene)),
                };
                slot.is_some_and(|(track, scene)| {
                    doc.slot(track, scene).is_none()
                        && doc
                            .track(track)
                            .is_some_and(|t| t.kind() == bl_model::TrackKind::Midi)
                })
            }
            "session.cancel_scene_launch" => args
                .get("scene")
                .and_then(|scene| serde_json::from_value::<SceneId>(scene.clone()).ok())
                .is_some_and(|scene| self.scene_launch_queued(scene)),
            _ => return None,
        })
    }

    /// `session.rename`: starts typing a new name for the selected clip, or
    /// else for the selected scene.
    fn run_session_rename(&mut self, id: &str) -> Result<Value, ShellError> {
        let doc = self.editor.document();
        let clip = self
            .session
            .selected_slots()
            .iter()
            .find_map(|slot| doc.slot(slot.track, slot.scene))
            .map(|clip| clip.id);
        let effect = match (clip, self.session.state().scenes.first()) {
            (Some(clip), _) => Effect::RenameClip(clip),
            (None, Some(scene)) => Effect::RenameScene(*scene),
            (None, None) => {
                return Err(ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "select a clip or a scene first".to_owned(),
                });
            }
        };
        self.effects.push(effect);
        self.touch();
        Ok(Value::Null)
    }

    /// The slots the engine has queued, is playing or records into, for
    /// `session.state`.
    fn session_launches(&self) -> Vec<SlotLaunch> {
        let rate = self.audio.sample_rate().and_then(SampleRate::new);
        let tempo = &self.editor.document().settings().tempo;
        let now = self.arrangement.transport().position;
        self.session_states
            .iter()
            .map(|status| {
                // Spec: launch-quantisation R34. The tick it launched on and
                // the clip time it has now: its offset plus the time since.
                let launched = rate
                    .zip(status.started)
                    .map(|(rate, started)| tempo.frames_to_ticks(started, rate));
                SlotLaunch {
                    slot: SlotRef {
                        track: status.track,
                        scene: status.scene,
                    },
                    state: match status.state {
                        bl_engine::SlotState::Queued => LaunchState::Queued,
                        bl_engine::SlotState::Playing => LaunchState::Playing,
                        bl_engine::SlotState::QueuedToStop => LaunchState::QueuedToStop,
                        bl_engine::SlotState::Recording => LaunchState::Recording,
                    },
                    launched,
                    clip_time: launched
                        .map(|launched| status.offset.ticks + (now - launched).max(Ticks::ZERO)),
                }
            })
            .collect()
    }

    /// Runs the Session View's commands.
    pub(super) fn run_session_view(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if id == session_ids::RENAME {
            Self::args::<NoArgs>(id, args)?;
            return self.run_session_rename(id);
        }
        if id == session_ids::STATE {
            Self::args::<NoArgs>(id, args)?;
            let mut state = self.session.state();
            state.launches = self.session_launches();
            return Ok(to_value(&state));
        }
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
                    look: &self.prefs.look,
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
