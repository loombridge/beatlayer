//! The save history (IMP-05, WP-6.39; the crash-recovery spec, R12 to R20):
//! whether its panel shows, the rows it lists, and the commands that list
//! and restore them. The rows and their songs come from `bl_project::history`.

use bl_project::SetFile;
use bl_project::history::{self, HistoryRow, Restored, Row, SaveHistory as SaveHistoryRows};
use bl_shell_spec::commands::SaveHistoryState;

use super::*;

/// What the history's panel shows between frames.
#[derive(Default)]
pub(super) struct SaveHistory {
    /// Whether the panel shows.
    pub open: bool,
    /// The rows it lists, newest first.
    rows: Vec<Row>,
    /// What the rows were listed for, so they are listed again only when
    /// one could have changed.
    listed: Option<Listed>,
}

/// What the rows depend on.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Listed {
    set: Option<(PathBuf, Option<bl_project::journal::Follows>)>,
    continuous: bool,
    session: Option<String>,
    journalled: u64,
    backups: Option<(usize, Option<PathBuf>)>,
}

impl Shell {
    /// Whether the Save History panel shows (R12).
    #[must_use]
    pub fn save_history_open(&self) -> bool {
        self.save_history.open
    }

    /// The rows the panel lists, newest first (R13 to R15). The host's
    /// [`Shell::poll`] keeps them current while the panel shows.
    #[must_use]
    pub fn save_history_rows(&self) -> &[Row] {
        &self.save_history.rows
    }

    /// The panel's state, for the command's result.
    #[must_use]
    pub fn save_history_state(&self) -> SaveHistoryState {
        SaveHistoryState {
            open: self.save_history.open,
            rows: self.history_rows().len(),
            continuous: self.prefs.save_continuously,
        }
    }

    /// The open set's history, listed now.
    fn history_rows(&self) -> Vec<Row> {
        self.store.set_file().map_or_else(Vec::new, |set| {
            history::rows(set, self.prefs.save_continuously)
        })
    }

    /// Lists the rows again if the set, a save point, a save or a backup
    /// could have changed them, while the panel shows (R15). Says whether
    /// they changed.
    // shortcut: every new save point lists the history afresh, reading the
    // journals whole; upgrade to reading only the new entries if sets with
    // thousands of save points feel slow with the panel open.
    pub(super) fn refresh_save_history(&mut self) -> bool {
        if !self.save_history.open {
            self.save_history.listed = None;
            return false;
        }
        let continuous = self.prefs.save_continuously;
        let status = self.autosave.as_ref().map(Autosave::status);
        let set = self.store.set_file();
        let listed = Listed {
            set: set.map(|set| (set.path().to_owned(), set.follows())),
            continuous,
            session: status.as_ref().and_then(|status| status.session.clone()),
            // Only save points are new rows, and only Save Continuously
            // makes them.
            journalled: status
                .filter(|_| continuous)
                .map_or(0, |status| status.journalled),
            backups: set.map(backup_files),
        };
        if self.save_history.listed.as_ref() == Some(&listed) {
            return false;
        }
        let rows = self.history_rows();
        self.save_history.listed = Some(listed);
        let changed = self.save_history.rows.len() != rows.len()
            || self
                .save_history
                .rows
                .iter()
                .zip(&rows)
                .any(|(shown, now)| shown.id != now.id);
        self.save_history.rows = rows;
        changed
    }

    /// Shows or hides the panel, hiding the Undo History when it opens: they
    /// share the right-hand area (R12).
    pub(super) fn toggle_save_history(&mut self) -> Value {
        self.save_history.open = !self.save_history.open;
        if self.save_history.open {
            self.undo_history_open = false;
        }
        self.refresh_save_history();
        self.touch();
        to_value(&self.save_history_state())
    }

    /// Runs `set.save_history` or `set.restore` (R16 to R20).
    pub(super) fn run_save_history(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if id == history::SAVE_HISTORY {
            Self::args::<NoArgs>(id, args)?;
            let rows = self.history_rows().iter().map(HistoryRow::from).collect();
            return Ok(to_value(&SaveHistoryRows {
                continuous: self.prefs.save_continuously,
                rows,
            }));
        }
        let args: history::RestoreArgs = Self::args(id, args)?;
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        if self.editor.in_transaction() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "an edit is in progress; finish it first".to_owned(),
            });
        }
        let Some(set) = self.store.set_file().cloned() else {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "the set has never been saved, so it has no history".to_owned(),
            });
        };
        let kind = history::kind_of(&args.id)
            .ok_or_else(|| failed(format!("the history has no row {}", args.id)))?;
        // A row that can't be read changes nothing, and the error says why
        // (R18).
        let (mut project, info) = history::load(&set, &args.id)
            .map_err(|error| failed(format!("couldn't read {}: {error}", args.id)))?;
        if info.before_relative_modulation() {
            bl_devices::relative_clip_envelopes(&mut project.document);
        }
        self.store.merge_media(&project.media);
        for (hash, path, size) in self.store.media_files() {
            self.media
                .add_reference(bl_media::ContentHash::from_bytes(hash.0), path, size);
        }
        let step = history::undo_step(kind);
        let changed = *self.editor.document() != project.document;
        self.edit(|editor| editor.replace(step, project.document));
        Ok(to_value(&Restored {
            id: args.id,
            undo_step: changed.then(|| step.to_owned()),
        }))
    }
}

/// How many backups a set has and the newest of them: what a save changes.
fn backup_files(set: &SetFile) -> (usize, Option<PathBuf>) {
    let stem = set
        .path()
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let backups = bl_project::backups(&set.folder().backup(), &stem);
    (
        backups.len(),
        backups.first().map(|backup| backup.path.clone()),
    )
}
