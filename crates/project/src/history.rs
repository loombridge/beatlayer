//! The save history (the crash-recovery spec, R13 and R16 to R20, IMP-05):
//! the rows a set's history lists, and the song each holds.
//!
//! [`rows`] lists, newest first, the set's save points (with Save
//! Continuously on), the set as last saved, and its backups. [`load`] reads
//! the song of one of them by its `id`. The commands that show and restore
//! them, [`register`]ed here, are run by the app, which owns the open set.
//!
//! ```
//! use bl_model::Editor;
//! use bl_project::{Project, SetFile, history};
//!
//! # let dir = std::env::temp_dir().join(format!("bl-history-doc-{}", std::process::id()));
//! let mut set = SetFile::new(dir.join("Song.blset"));
//! assert!(history::rows(&set, true).is_empty(), "a set never saved has no history");
//! set.save(&Project::from_editor(&Editor::new(), Default::default()))?;
//! let rows = history::rows(&set, false);
//! assert_eq!(rows[0].kind, history::Kind::Saved);
//! assert!(history::load(&set, &rows[0].id).is_ok());
//! # std::fs::remove_dir_all(&dir)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::LoadError;
use crate::format::{LoadOptions, Project, SetInfo};
use crate::journal::{self, SavePoint};
use crate::save::{SetFile, backups, read_set, utc_rfc3339};

/// `set.save_history`: the rows of the open set's save history.
pub const SAVE_HISTORY: &str = "set.save_history";
/// `set.restore`: replace the open song with one of the history's rows.
pub const RESTORE: &str = "set.restore";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[SAVE_HISTORY, RESTORE];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// What a row of the history is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A song Save Continuously kept after an edit.
    SavePoint,
    /// The set as its file last saved it.
    Saved,
    /// A version of the set that a save replaced, from the `Backup` folder.
    Backup,
}

/// Where a row's song is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    SavePoint(SavePoint),
    File(PathBuf),
}

/// One row of the history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Names the row for [`load`] and `set.restore`; it stays the same while
    /// the row exists.
    pub id: String,
    /// What the row is.
    pub kind: Kind,
    /// When it was journalled or saved.
    pub time: SystemTime,
    /// A save point's edit, as the undo history names it; `Saved` for the
    /// others.
    pub label: String,
    source: Source,
}

/// The rows of a set's history, newest first: its save points (only when
/// `continuous`, since without Save Continuously the history is its saved
/// versions), the set as last saved, then its backups. A set never saved
/// has none.
#[must_use]
pub fn rows(set: &SetFile, continuous: bool) -> Vec<Row> {
    if set.info().is_none() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    if continuous {
        for point in set.save_points().into_iter().rev() {
            let session = journal::session_of(&point.journal).unwrap_or_default();
            rows.push(Row {
                id: format!("save_point:{session}:{}", point.entry.seq),
                kind: Kind::SavePoint,
                time: point.entry.time,
                label: point.entry.label.clone(),
                source: Source::SavePoint(point),
            });
        }
    }
    rows.push(Row {
        id: format!("saved:{}", set.revision()),
        kind: Kind::Saved,
        time: std::fs::metadata(set.path())
            .and_then(|meta| meta.modified())
            .unwrap_or(UNIX_EPOCH),
        label: "Saved".to_owned(),
        source: Source::File(set.path().to_owned()),
    });
    let stem = set
        .path()
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    for backup in backups(&set.folder().backup(), &stem) {
        let name = backup
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        rows.push(Row {
            id: format!("backup:{name}"),
            kind: Kind::Backup,
            time: backup.time.unwrap_or(UNIX_EPOCH),
            label: "Saved".to_owned(),
            source: Source::File(backup.path),
        });
    }
    rows
}

/// The song of the row `id` (as [`rows`] lists it with save points), with
/// what its file says about itself, so that the caller migrates it as
/// opening the set would.
///
/// # Errors
///
/// [`LoadError::Corrupt`] if no row has this `id` any more, and the error
/// of reading the row's song otherwise: a journal damaged, a backup moved
/// away.
pub fn load(set: &SetFile, id: &str) -> Result<(Project, SetInfo), LoadError> {
    let row = rows(set, true)
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(|| LoadError::corrupt(format!("the history has no row {id}")))?;
    match &row.source {
        Source::SavePoint(point) => journal::restore_with_info(point),
        Source::File(path) => {
            read_set(path, &LoadOptions::default()).map(|(project, info, _)| (project, info))
        }
    }
}

/// A row, as `set.save_history` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryRow {
    /// The ID `set.restore` takes.
    pub id: String,
    /// What the row is.
    pub kind: Kind,
    /// When it was journalled or saved, in UTC (RFC 3339).
    pub time: String,
    /// A save point's edit, as the undo history names it; `Saved` for the
    /// others.
    pub label: String,
}

impl From<&Row> for HistoryRow {
    fn from(row: &Row) -> Self {
        Self {
            id: row.id.clone(),
            kind: row.kind,
            time: utc_rfc3339(row.time),
            label: row.label.clone(),
        }
    }
}

/// What `set.save_history` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SaveHistory {
    /// Whether Save Continuously is on.
    pub continuous: bool,
    /// The rows, newest first.
    pub rows: Vec<HistoryRow>,
}

/// Arguments of `set.restore`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreArgs {
    /// The row's `id`, as `set.save_history` gives it.
    pub id: String,
}

/// What `set.restore` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Restored {
    /// The row that was restored.
    pub id: String,
    /// The undo step the restore is, or null when the song already was
    /// the row's, so nothing changed.
    pub undo_step: Option<String>,
}

/// The undo step a restore of a row of this kind is.
#[must_use]
pub fn undo_step(kind: Kind) -> &'static str {
    match kind {
        Kind::SavePoint => "Restore Save Point",
        Kind::Saved | Kind::Backup => "Restore Saved Version",
    }
}

/// The kind of the row `id` names, from the `id` alone.
#[must_use]
pub fn kind_of(id: &str) -> Option<Kind> {
    match id.split(':').next()? {
        "save_point" => Some(Kind::SavePoint),
        "saved" => Some(Kind::Saved),
        "backup" => Some(Kind::Backup),
        _ => None,
    }
}

/// Adds the commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(SAVE_HISTORY, "Save History")
            .description(
                "Lists the open set's save history, newest first: the save points Save \
                 Continuously kept since the last save (none while it is off), then the set as \
                 last saved, then its backups. Each row has an `id` for `set.restore`, its kind \
                 (`save_point`, `saved` or `backup`), its time in UTC and its label, which is \
                 the edit's name for a save point. A set never saved has no rows. Changes \
                 nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<SaveHistory>(),
    )?;
    registry.register(
        CommandSpec::new(RESTORE, "Restore")
            .description(
                "Replaces the whole song with the one a row of `set.save_history` holds, as one \
                 undo step named `Restore Save Point` or `Restore Saved Version` that undo takes \
                 back. The set keeps its file and name, and the transport keeps playing. With \
                 Save Continuously on the restore is a new save point; every row stays listed. \
                 A row that can't be read changes nothing. Read the result back with the query \
                 tool's `song` (`undo.next_undo`).",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .args::<RestoreArgs>()
            .result::<Restored>(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use bl_commands::Registry;

    use super::*;

    #[test]
    fn every_command_is_registered_and_complete() {
        // Spec: crash-recovery R20
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        assert_eq!(registry.problems(), Vec::new());
        assert!(IDS.iter().all(|id| handles(id) && registry.contains(id)));
    }

    #[test]
    fn a_row_id_names_its_kind() {
        assert_eq!(kind_of("save_point:a:3"), Some(Kind::SavePoint));
        assert_eq!(kind_of("saved:2"), Some(Kind::Saved));
        assert_eq!(kind_of("backup:Song [x].blset"), Some(Kind::Backup));
        assert_eq!(kind_of("nonsense"), None);
    }
}
