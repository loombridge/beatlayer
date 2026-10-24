//! The engine's events, the transport and the Session's launch state.

use super::{MIDI_RELIST, MainView, Shell, WindowKind};
use crate::arrangement::TransportView;
use crate::audio::EngineTransport;
use crate::auto_warp;
use bl_core::time::SampleRate;
use bl_model::AutomationTarget;
use bl_model::Command;
use bl_model::TrackId;

use super::ShellError;
use bl_engine::transport as engine_transport;
use serde_json::Value;

impl Shell {
    /// Takes background work's news: the browser's index, a decoded
    /// preview, a preview that finished. The host calls it every frame and
    /// when woken; a failure shows in the Status Bar. Returns whether
    /// anything a window shows changed.
    pub fn poll(&mut self) -> bool {
        self.analysis_cache();
        self.hear_analyses();
        let warping = auto_warp::Warping {
            prefs: self.prefs.warp,
            analysis: self.analysis.as_ref(),
        };
        let set_bpm = self
            .editor
            .document()
            .settings()
            .tempo
            .bpm_at(self.arrangement.transport().position)
            .get();
        let polled = self
            .browser_view
            .poll(&mut self.audio, &self.media, warping, set_bpm);
        if let Some(error) = polled.error {
            self.set_message(error, true);
        }
        // Spec: browser-tags R3. A database that couldn't be read is set
        // aside and a new one is built; the Status Bar says so.
        if let Some(message) = self.browser_view.take_message() {
            self.set_message(message, true);
        }
        let mut changed = polled.changed | self.poll_processed() | self.poll_stems();
        // Spec: crash-recovery R15. The Save History follows new save points
        // and saves while it shows.
        changed |= self.refresh_save_history();
        // Spec: export R41. The export on its thread: its share moves the
        // progress window, and its end lands in the Status Bar.
        changed |= self.poll_export();
        // Spec: multisampler R73. The autosampler makes one sample at a
        // time, so the dialog sees its progress and Cancel stops it.
        self.poll_autosample();
        // A MIDI device plugged in or removed (WP-2.2).
        if self.midi_listed.elapsed() >= MIDI_RELIST {
            changed |= self.refresh_midi();
        }
        changed |= self.learn_midi();
        self.expire_previews();
        changed |= self.track_midi_activity();
        // Plug-ins' main-thread work (WP-2.6 to WP-2.8), and the changes
        // they made themselves.
        changed |= self.poll_plugin_scan();
        let plugins_changed =
            self.plugins.idle() | self.au.idle() | self.vst3.idle() | self.lv2.idle();
        // Spec: ara-hosting R2, R8, R12. The ARA plug-ins' documents follow
        // the set, and their editors the Arrangement's selection.
        self.vst3
            .sync_ara(self.editor.document(), self.document_revision);
        self.follow_ara();
        if plugins_changed {
            self.refresh_device_types();
            changed = true;
        }
        // A plug-in that changed its latency: the engine recompiles with
        // it, so delay compensation follows it (WP-2.7, WP-2.11).
        for (device, frames) in self
            .vst3
            .take_latency_changes()
            .into_iter()
            .chain(self.lv2.take_latency_changes())
        {
            self.audio.set_device_latency(device, frames);
            changed = true;
        }
        // The open plug-in editors follow View ▸ Plug-In Windows and
        // Auto-Hide Plug-In Windows, as the tracks' floating windows do.
        self.apply_plug_in_windows();
        if self.kit.idle() {
            changed = true;
        }
        let generation = self.plugins.generation();
        if plugins_changed || generation != self.plugins_generation {
            // A plug-in that started may list commands.
            self.plugins_generation = generation;
            self.refresh_device_commands(false);
            changed = true;
        }
        let changes: Vec<(bl_model::DeviceId, bl_model::ParamId, f64)> = self
            .plugins
            .take_param_changes()
            .into_iter()
            .map(|c| (c.device, c.param, c.value))
            .chain(
                self.au
                    .take_param_changes()
                    .into_iter()
                    .map(|c| (c.device, c.param, c.value)),
            )
            .chain(
                self.vst3
                    .take_param_changes()
                    .into_iter()
                    .map(|c| (c.device, c.param, c.value)),
            )
            .chain(
                self.lv2
                    .take_param_changes()
                    .into_iter()
                    .map(|c| (c.device, c.param, c.value)),
            )
            .chain(
                self.kit
                    .take_param_changes()
                    .into_iter()
                    .map(|c| (c.device, c.param, c.value)),
            )
            .collect();
        if crate::plugins::apply_param_changes(changes, &mut self.editor, &mut self.devices) {
            self.dirty = true;
            self.document_changed();
            changed = true;
        }
        if changed {
            self.touch();
        }
        changed
    }

    /// Takes what the engine last published (the playhead, whether the song
    /// plays, the insert marker) for the Arrangement View and the Control
    /// Bar, with the loop brace from the document. The window calls it
    /// before drawing each frame, so what it draws lags the engine by at
    /// most a frame and an audio block (the control bar transport spec,
    /// R14). Returns the transport it took.
    pub fn poll_engine(&mut self) -> EngineTransport {
        let engine = self.audio.transport_state();
        self.poll_capture();
        // Spec: automation-editing R23. A recording edits the set as it
        // runs, so it ends the edit a selected breakpoint opened.
        if self.recording.is_some() || self.session_run.is_some() {
            self.end_key_edit();
        }
        // WP-2.13. A recording's take reaches the shell with the engine's
        // state; the clips are made from it once the writer has finished.
        self.poll_recording(&engine);
        self.poll_automation_record(&engine);
        // Spec: mixer-view R10. The meters the strips and headers draw: the
        // engine's while one runs, and none once it stops. Without an
        // engine, meters set with `set_meters` stay.
        let running = self.audio.engine_running();
        if running || self.metering {
            self.metering = running;
            // Not a revision: the window redraws while a meter shows a
            // level, and nothing else depends on them.
            self.audio.meters_into(&mut self.meters);
            // Spec: device-framework R35. The display data the devices'
            // nodes published, which the device displays draw. Not a
            // revision either.
            self.audio.displays_into(self.devices.displays_mut());
            // Spec: articulation-map R15. A row whose Learn button is lit
            // takes the next note-on or control change the device hears as
            // its trigger, which is one undo step.
            for (device, row, trigger) in self.devices.learned_triggers() {
                self.devices.stop_learning(device);
                self.edit(|editor| {
                    // The list may have changed while the button was lit,
                    // and then the row is gone: the trigger is dropped
                    // rather than ending the shell.
                    let _ = editor.apply(Command::SetArticulation {
                        device,
                        index: row,
                        name: None,
                        trigger: Some(trigger),
                        outputs: None,
                        channel: None,
                    });
                });
            }
        }
        // Spec: drum-racks R20. With Auto Select on, a drum rack's chain
        // list selects each chain a note reaches.
        if running {
            let mut hits = std::collections::BTreeMap::new();
            self.audio.chain_hits_into(&mut hits);
            if self.devices.auto_select(self.editor.document(), &hits) {
                self.touch();
            }
        }
        // Spec: launch-quantisation R10. The slots the engine is launching
        // or stopping, which the grid's launch buttons draw.
        self.audio.session_into(&mut self.session_states);
        // Spec: scales R11. A clip that starts playing, and the clip the
        // playhead enters, make their scale the current scale.
        self.follow_playing_scale(&engine);
        // A drag in progress waits: its end is one change, not a split for
        // every frame of it.
        if self.recording.is_some()
            && !self.gesture
            && let Some(rate) = self.audio.sample_rate().and_then(SampleRate::new)
        {
            // Spec: session-to-arrangement R3. The clips that play, as they
            // were, for the copies a recording makes.
            self.session_watch.poll(
                self.editor.document(),
                &self.session_states,
                engine.position,
                &self.editor.document().settings().tempo,
                rate,
            );
        }
        self.scene_override = self.audio.scene_override();
        // Spec: session-view R24. The tracks a Session clip has taken over,
        // which the Back to Arrangement buttons light from.
        self.audio.session_over_into(&mut self.session_over);
        // Spec: modulation R7. What the modulated controls played in the
        // last block: the rings and the fader marks draw from it. Spec:
        // racks R37. The chains processing, which Auto Select follows,
        // clear with them when the engine stops.
        if running || self.modulating {
            self.modulating = running;
            self.audio.modulated_into(&mut self.modulated);
            self.audio.processing_into(&mut self.processing);
        }
        if self
            .devices
            .follow_processing(self.editor.document(), &self.processing)
        {
            self.touch();
        }
        // Spec: sample-editor R11. Starting or stopping playback lets the
        // Sample Editor follow it again.
        if engine.playing != self.arrangement.transport().playing {
            self.clip_view.sample_follow(false);
        }
        let brace = self.editor.document().settings().arrangement_loop;
        self.set_transport(TransportView {
            playing: engine.playing,
            position: engine.position,
            loop_start: brace.start,
            loop_length: brace.length,
            loop_on: brace.enabled,
        });
        // Spec: control-bar-transport R15. A move of the engine's insert
        // marker (the position fields, Home, a second Stop, an agent)
        // reaches the view once.
        if self.seen_insert_marker != Some(engine.insert_marker) {
            self.seen_insert_marker = Some(engine.insert_marker);
            if self.arrangement.move_insert_marker(engine.insert_marker) {
                self.touch();
            }
        }
        engine
    }

    /// Whether the song is playing, as the shell last heard from the
    /// engine.
    #[must_use]
    pub fn playing(&self) -> bool {
        self.arrangement.transport().playing
    }

    /// Points the set's current scale at the clip that starts playing, and
    /// at the Arrangement clip the playhead enters (the scales spec, R11).
    /// It is no undo step, though the set is marked as changed.
    fn follow_playing_scale(&mut self, engine: &EngineTransport) {
        if !engine.playing {
            self.launched.clear();
            self.playing_clip = None;
            return;
        }
        let doc = self.editor.document().clone();
        let order = doc.track_order();
        let (launches, launched) =
            crate::scales::new_launches(&self.session_states, &self.launched);
        self.launched = launched;
        if let Some(scale) = crate::scales::launched_scale(&doc, &launches, &order) {
            self.follow_scale(scale);
            return;
        }
        let clip = crate::scales::arrangement_clip(&doc, engine.position, &self.session_over);
        if clip != self.playing_clip {
            self.playing_clip = clip;
            if let Some(scale) = clip.and_then(|clip| doc.clip(clip)).map(|clip| clip.scale) {
                self.follow_scale(scale);
            }
        }
    }

    /// The global launch quantisation and its enabled switch (WP-3.2), which
    /// the Control Bar's Quantization Menu shows and the Options items
    /// change. It's saved with the set.
    #[must_use]
    pub fn launch_settings(&self) -> bl_model::LaunchSettings {
        self.store.launch()
    }

    /// The Session slots' states the engine last published (WP-3.2), which
    /// the grid's launch buttons draw: one entry per queued, playing, or
    /// stopping slot, and nothing once every slot has stopped.
    #[must_use]
    pub fn session_states(&self) -> &[bl_engine::SlotStatus] {
        &self.session_states
    }

    /// The scene the Scene View shows in the detail area (L-SES-011): the
    /// first scene selected in the Session View, while it's in front.
    #[must_use]
    pub fn scene_view_scene(&self) -> Option<bl_model::SceneId> {
        self.scene_view_scene_in(WindowKind::Main)
    }

    /// The same for `window`: only the window that shows the Session View
    /// has it.
    #[must_use]
    pub fn scene_view_scene_in(&self, window: WindowKind) -> Option<bl_model::SceneId> {
        (self.main_view_of(window) == MainView::Session)
            .then(|| self.session.state().scenes.first().copied())
            .flatten()
            .filter(|scene| self.editor.document().scene(*scene).is_some())
    }

    /// The tempo and time signature a launched scene put in place of the
    /// set's, as the engine last published them (WP-3.5): they hold until
    /// Re-Enable Automation or Back to Arrangement. Nothing while no engine
    /// runs.
    #[must_use]
    pub fn scene_override(&self) -> bl_engine::SceneOverride {
        self.scene_override
    }

    /// The tempo in force at the playhead: a launched scene's, or the
    /// set's tempo map there (WP-3.5).
    #[must_use]
    pub fn current_tempo(&self) -> bl_core::time::Bpm {
        self.scene_override.tempo.map_or_else(
            || {
                let at = self.arrangement.transport().position;
                self.editor.document().settings().tempo.bpm_at(at)
            },
            |(_, bpm)| bpm,
        )
    }

    /// The time signature in force at the playhead: a launched scene's, or
    /// the set's map there (WP-3.5).
    #[must_use]
    pub fn current_signature(&self) -> bl_core::time::TimeSignature {
        self.scene_override.signature.map_or_else(
            || {
                let at = self.arrangement.transport().position;
                self.editor
                    .document()
                    .settings()
                    .time_signatures
                    .signature_at(at)
            },
            |(_, signature)| signature,
        )
    }

    /// What `target` of `track` played in the last block while a clip's
    /// modulation acted on it (the modulation spec, R7): its base and the
    /// value it played, in the parameter's own units. `None` while it plays
    /// its base.
    #[must_use]
    pub fn played(&self, track: TrackId, target: &AutomationTarget) -> Option<(f64, f64)> {
        let (node, param) = bl_engine::target_node(track, target)?;
        self.modulated
            .iter()
            .find(|entry| entry.node == node && entry.param == param)
            .map(|entry| (entry.base, entry.played))
    }

    /// The tracks a Session clip has taken over from the Arrangement
    /// (WP-3.6): the Main column's and the Arrangement's Back to Arrangement
    /// buttons light while it lists any, and a track's own button while it
    /// lists the track.
    #[must_use]
    pub fn session_over(&self) -> &[TrackId] {
        &self.session_over
    }

    /// Sends a transport command to the engine, as menus, keys, the Control
    /// Bar and agents all do. Recording gives the engine what to write
    /// before it knows the transport records (WP-2.13). A reason no files
    /// can come of it (a set with no project folder, say) is reported in the
    /// Status Bar; the transport still records, as the metronome's "only
    /// while recording" follows it.
    ///
    /// # Errors
    ///
    /// A message if no engine runs.
    pub fn send_transport(&mut self, command: bl_engine::TransportCommand) -> Result<(), String> {
        if command == bl_engine::TransportCommand::SetRecording(true) {
            // Spec: session-recording R11. One recording at a time: a new
            // plan would replace the Session recording's.
            if self.session_run.is_some() {
                return Err("a Session recording is under way; end it first".to_owned());
            }
            if let Err(reason) = self.begin_recording() {
                self.set_message(reason, true);
            }
        }
        self.audio.send_transport(command)
    }

    /// Whether the transport is currently playing.
    #[must_use]
    pub fn is_playing(&mut self) -> bool {
        self.audio.transport_state().playing
    }

    /// The Arrangement's time selection, if there is one.
    #[must_use]
    pub fn time_selection(&self) -> Option<(bl_core::time::Ticks, bl_core::time::Ticks)> {
        self.arrangement.time_selection()
    }

    /// Runs the commands that reach the engine's transport.
    pub(super) fn run_engine_transport(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        // Spec: control-bar-transport R12, R13. Menus, keys, the Control
        // Bar, and agents all reach the engine's transport here.
        let command =
            engine_transport::command_from_args(id, Self::object(args)).map_err(|error| {
                ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: error.to_string(),
                }
            })?;
        self.send_transport(command)
            .map_err(|reason| ShellError::Unavailable {
                command: id.to_owned(),
                reason,
            })?;
        Ok(Value::Null)
    }
}
