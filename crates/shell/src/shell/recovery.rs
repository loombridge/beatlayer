//! Crash recovery: the journal of the set's edits and the recovery commands.

use super::{Shell, ShellError, to_value};
use crate::files::{after_load, new_set_from};
use bl_project::autosave::Autosave;
use bl_project::autosave::Recoverable;
use bl_project::autosave::Recovered;
use bl_project::registry as recovery_ids;
use serde_json::Value;
use std::path::PathBuf;

impl Shell {
    /// Runs one of the recovery commands against the app's own recovery
    /// (the crash-recovery spec, R26): `set.list_recoverable`,
    /// `set.recover`, `set.discard_recovery` and
    /// `settings.set_continuous_save`.
    pub(super) fn run_recovery(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if id == recovery_ids::SET_CONTINUOUS_SAVE {
            let args: recovery_ids::ContinuousSaveArgs =
                serde_json::from_value(args).map_err(|error| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: error.to_string(),
                })?;
            self.prefs.save_continuously = args.on;
            self.save_preferences();
            if let Some(autosave) = self.autosave.as_mut() {
                autosave.set_continuous(args.on);
            }
            return Ok(to_value(&recovery_ids::ContinuousSave { on: args.on }));
        }
        let ran = {
            let Some(autosave) = self.autosave.as_mut() else {
                return Err(ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "crash recovery isn't running in this session".to_owned(),
                });
            };
            recovery_ids::run(autosave, id, args).map_err(|error| ShellError::Failed {
                command: id.to_owned(),
                message: error.to_string(),
            })?
        };
        if let Some(recovered) = ran.recovered {
            self.open_recovered(recovered);
        }
        Ok(ran.result)
    }

    /// Opens a recovered set in place of the open one (the crash-recovery
    /// spec, R3): as it was after its last journalled edit, unsaved, with
    /// an empty undo history.
    fn open_recovered(&mut self, recovered: Recovered) {
        let Recovered {
            set,
            mut project,
            info,
            ..
        } = recovered;
        // The journal holds the song in the form it was written in, as a set
        // file does: finish its migrations as `set.open` does.
        let notice = after_load(&mut project, &info);
        self.set_replaced();
        self.editor = project.editor();
        self.set_path = set.as_ref().map(|set| set.path().to_owned());
        // Discard Changes goes back to the set file, or, for a set never
        // saved, to the set a new one starts from.
        self.saved = new_set_from(
            self.set_path
                .as_deref()
                .or(self.prefs.default_set.as_deref()),
        )
        .0;
        self.dirty = true;
        self.store.adopt(set, project);
        self.apply_window_views(self.store.window_views());
        // The views saved beside the song, and its files, as `set.open`
        // takes them.
        self.devices.set_windows(self.store.windows());
        self.devices.set_rack_views(self.store.rack_views());
        self.devices
            .set_multisampler_views(self.store.multisampler_views());
        self.clip_view.set_folds(self.store.folds());
        self.clip_view.set_scale_folds(self.store.scale_folds());
        self.clip_view.restore_mpe(self.store.mpe_view());
        self.clip_view.restore_midi_tools(&self.store.midi_tools());
        for (hash, path, size) in self.store.media_files() {
            self.media
                .add_reference(bl_media::ContentHash::from_bytes(hash.0), path, size);
        }
        self.arrangement.reset();
        self.arrangement.restore_lanes(self.store.lane_views());
        self.clip_view.forget_selection();
        self.mixer.reset();
        self.mixer.restore_widths(self.store.column_widths());
        self.session.reset();
        self.new_set_transport();
        self.document_changed();
        self.set_file_changed();
        if let Some(notice) = notice {
            self.set_message(notice, false);
        }
        self.touch();
    }

    /// Tells the journal the set was saved, so it retires it: the set file
    /// holds every edit (the crash-recovery spec, R6).
    pub(super) fn journal_saved(&mut self) {
        let set = self.store.set_file().cloned();
        if let (Some(autosave), Some(set)) = (self.autosave.as_mut(), set.as_ref()) {
            autosave.saved(set);
        }
    }

    /// Closes the journal: the set was saved, or its changes were thrown
    /// away, so nothing is offered at the next launch (the crash-recovery
    /// spec, R6).
    pub(super) fn close_journal(&mut self) {
        if let Some(autosave) = self.autosave.as_mut() {
            let _ = autosave.close_set();
        }
    }

    /// Runs crash recovery in `dir` (WP-6.62, the crash-recovery spec R21):
    /// the open set's edits are journalled there, and the sets a crash left
    /// are offered at the next launch. The app calls it at start-up; a
    /// headless run with a recovery folder of its own calls it too.
    ///
    /// # Errors
    ///
    /// A message saying why the recovery folder couldn't be used. The shell
    /// goes on working without it.
    pub fn use_recovery(&mut self, dir: PathBuf) -> Result<(), String> {
        let mut autosave = Autosave::start(&dir).map_err(|error| error.to_string())?;
        autosave.set_continuous(self.prefs.save_continuously);
        self.autosave = Some(autosave);
        self.recovery_error = None;
        self.begin_journal();
        self.check_recovery();
        Ok(())
    }

    /// The sets a crash left unsaved, newest first (the crash-recovery
    /// spec, R2). They are offered at the next launch.
    #[must_use]
    pub fn recovery_offers(&self) -> Vec<Recoverable> {
        self.autosave
            .as_ref()
            .map_or_else(Vec::new, Autosave::offers)
    }

    /// Whether the recovery prompt is open: there is something to offer,
    /// and it hasn't been deferred (the crash-recovery spec, R22).
    #[must_use]
    pub fn recovery_prompt_open(&self) -> bool {
        !self.recovery_deferred && !self.recovery_offers().is_empty()
    }

    /// Whether Save Continuously is on (the crash-recovery spec, R8).
    #[must_use]
    pub fn save_continuously(&self) -> bool {
        self.prefs.save_continuously
    }

    /// Starts the journal for the open set, so that its edits are kept for
    /// recovery (the crash-recovery spec, R21).
    pub(super) fn begin_journal(&mut self) {
        let Some(autosave) = self.autosave.as_mut() else {
            return;
        };
        let set = self.store.set_file().cloned();
        if let Err(error) = autosave.begin(set.as_ref()) {
            self.recovery_error = Some(error.to_string());
        }
    }

    /// Journals the open set after a finished edit, so that the set can be
    /// recovered after a crash (the crash-recovery spec, R1 and R21).
    /// `label` names the edit as the undo history does; without one, the
    /// newest undo step's name is used.
    pub(super) fn journal_edit(&mut self, label: Option<&str>) {
        if self.editor.in_transaction() {
            // The edit isn't finished: the transaction is still open.
            return;
        }
        if self.autosave.is_none() {
            return;
        }
        let label = label
            .map(str::to_owned)
            .or_else(|| self.editor.undo_history().next().map(str::to_owned))
            .unwrap_or_else(|| self.journal_label.clone());
        self.journal_label = label.clone();
        let project = self.store.project(&self.editor);
        if let Some(autosave) = self.autosave.as_mut() {
            autosave.edited(&project, label);
        }
        self.check_recovery();
    }

    /// Waits until every edit of the open set is on disk (the crash-recovery
    /// spec, R1). The app doesn't need it: the journal is written within a
    /// second on a thread of its own. A test that kills the app uses it, so
    /// that the edit it made is on disk before the kill.
    pub fn flush_recovery(&self) -> Result<(), String> {
        let Some(autosave) = self.autosave.as_ref() else {
            return Ok(());
        };
        autosave.flush().map_err(|error| error.to_string())
    }

    /// Tells the Status Bar about the journal when it can't be written, once
    /// (the crash-recovery spec, R27). The app goes on working, and the
    /// message goes when a write succeeds.
    pub(super) fn check_recovery(&mut self) {
        let problem = self.recovery_problem();
        self.recovery_error = problem;
        let Some(problem) = self.recovery_error.clone() else {
            if let Some(message) = self.recovery_message.take()
                && self
                    .message
                    .as_ref()
                    .is_some_and(|shown| shown.text == message)
            {
                self.clear_message();
            }
            return;
        };
        let message = format!("Edits aren't being kept for recovery: {problem}");
        self.recovery_message = Some(message.clone());
        // The message says once, and stays until a write succeeds. Another
        // message in the Status Bar keeps its place until there is room.
        if self.message.is_none() {
            self.set_message(message, true);
        }
    }

    /// What's wrong with the journal now, if anything: the last write's
    /// error, or the reason the session couldn't start.
    fn recovery_problem(&mut self) -> Option<String> {
        let autosave = self.autosave.as_mut()?;
        let set = self.store.set_file();
        if let Some(error) = autosave.status().error {
            return Some(error);
        }
        if autosave.status().session.is_none() {
            // The session never started: try again, so a folder that is
            // writable now is used (the crash-recovery spec, R27).
            if let Err(error) = autosave.retry(set) {
                return Some(error.to_string());
            }
        }
        autosave.status().error
    }
}
