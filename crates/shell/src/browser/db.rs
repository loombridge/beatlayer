//! The browser's database on disk (spec: browser-tags R1 to R5).
//!
//! One file in Beatlayer's configuration folder, beside its preferences,
//! holds the index of the places' files, the tags and tag groups, and the
//! items that carry them. A second Beatlayer may have the file open at the
//! same time: SQLite's locking keeps the two apart.
//!
//! The file is written on a background thread, which owns the connection.
//! Every write is a transaction, and SQLite's journal makes each one atomic:
//! a write stopped at any point leaves the file as it was before the write
//! or after it.
//!
//! A file that can't be read is renamed aside, `.damaged` added to its
//! name, and a new one is built. A file from a later version of Beatlayer
//! is left alone, and that session keeps its index in memory.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use bl_shell_spec::browser::ItemKind;

use super::labels::SavedLabel;
use super::tags::{Tag, TagGroup};

/// The schema version this build writes. A file with a higher version is
/// left alone.
pub const SCHEMA_VERSION: i64 = 2;

/// The file's schema, as SQLite statements.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    lower_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    place TEXT NOT NULL,
    size INTEGER NOT NULL DEFAULT 0,
    modified INTEGER NOT NULL DEFAULT 0,
    hash TEXT
);
CREATE TABLE IF NOT EXISTS tag_groups (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    position INTEGER NOT NULL,
    builtin INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS tags (
    id INTEGER PRIMARY KEY,
    group_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    parent_id INTEGER,
    position INTEGER NOT NULL,
    builtin INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS item_tags (
    item_key TEXT NOT NULL,
    tag_id INTEGER NOT NULL,
    PRIMARY KEY (item_key, tag_id)
);
CREATE TABLE IF NOT EXISTS saved_labels (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    base TEXT NOT NULL,
    search TEXT NOT NULL,
    icon TEXT
);
CREATE TABLE IF NOT EXISTS saved_label_tags (
    label_id INTEGER NOT NULL,
    tag_id INTEGER NOT NULL,
    PRIMARY KEY (label_id, tag_id)
);
";

/// A file in the index, as the database holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRow {
    /// Its path on disk.
    pub path: String,
    /// Its name.
    pub name: String,
    /// Its name in lower case, for matching.
    pub lower: String,
    /// What it is, as [`ItemKind`]'s name.
    pub kind: String,
    /// The place it's in, by its root's path.
    pub place: String,
    /// Its length in bytes.
    pub size: u64,
    /// When it changed, in seconds since the epoch.
    pub modified: i64,
    /// Its content hash, for a file that carries a tag.
    pub hash: Option<String>,
}

/// A change to the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOp {
    /// A file that was added or changed.
    Upsert(FileRow),
    /// A file that isn't there any more.
    Delete(String),
    /// The content hash of a file that carries a tag.
    Hash { path: String, hash: String },
}

/// A change to the tags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagOp {
    /// A group that was made.
    AddGroup {
        id: i64,
        name: String,
        position: i32,
        builtin: bool,
    },
    /// A tag that was made.
    AddTag {
        id: i64,
        group: i64,
        name: String,
        parent: Option<i64>,
        position: i32,
        builtin: bool,
    },
    /// A tag's new name.
    RenameTag { id: i64, name: String },
    /// A group's new name.
    RenameGroup { id: i64, name: String },
    /// A tag that was deleted.
    DeleteTag { id: i64 },
    /// A group that was deleted.
    DeleteGroup { id: i64 },
    /// A tag's new place in its group.
    MoveTag { id: i64, position: i32 },
    /// A group's new place in the list.
    MoveGroup { id: i64, position: i32 },
    /// The tags an item carries.
    SetItemTags { key: String, tags: Vec<i64> },
}

/// A change to the saved labels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LabelOp {
    /// A label that was made or changed.
    Save(SavedLabel),
    /// A label that was removed.
    Delete { id: i64 },
}

/// What the database holds when it's opened.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    /// The index of the places' files.
    pub files: Vec<FileRow>,
    /// The tag groups.
    pub groups: Vec<TagGroup>,
    /// The tags.
    pub tags: Vec<Tag>,
    /// The tags each item carries, by its key.
    pub item_tags: Vec<(String, Vec<i64>)>,
    /// The saved labels, in the order they were made.
    pub saved_labels: Vec<SavedLabel>,
}

/// The tags the database holds: the groups, the tags, and the items that
/// carry them.
pub type Tags = (Vec<TagGroup>, Vec<Tag>, Vec<(String, Vec<i64>)>);

/// What the writer thread is asked to do.
enum Request {
    /// Read everything, and answer.
    Load(Sender<Loaded>),
    /// Read the tags, and answer.
    LoadTags(Sender<Tags>),
    /// Write the changes to the index.
    Files(Vec<FileOp>),
    /// Write the changes to the tags.
    Tags(Vec<TagOp>),
    /// Write the changes to the saved labels.
    Labels(Vec<LabelOp>),
    /// Write everything that's waiting, and answer when it's done.
    Flush(Sender<()>),
}

/// The browser's database: the file, and the thread that writes it.
pub struct Db {
    path: PathBuf,
    requests: Option<Sender<Request>>,
    /// Whether the file is from a later version of Beatlayer, and is left
    /// alone: that session keeps its index in memory.
    newer: bool,
    /// What to tell the person, if a file that couldn't be read was set
    /// aside.
    damaged: Option<String>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db")
            .field("path", &self.path)
            .field("newer", &self.newer)
            .field("damaged", &self.damaged)
            .finish_non_exhaustive()
    }
}

impl Db {
    /// Opens the database at `path`, or makes it. A file that can't be read
    /// is renamed aside and a new one is built; a file from a later version
    /// of Beatlayer is left alone.
    #[must_use]
    pub fn open(path: PathBuf) -> Self {
        let mut db = Self {
            path,
            requests: None,
            newer: false,
            damaged: None,
        };
        let Some(parent) = db.path.parent().map(Path::to_path_buf) else {
            return db;
        };
        if std::fs::create_dir_all(&parent).is_err() {
            return db;
        }
        let existing = db.path.exists();
        match connect(&db.path) {
            Ok(Some(connection)) => {
                db.start(connection);
            }
            Ok(None) => {
                // A file from a later version of Beatlayer: it is left
                // alone, and this session keeps its index in memory.
                db.newer = true;
            }
            Err(error) => {
                if existing {
                    db.set_aside(&error);
                } else {
                    db.damaged = Some(format!("The browser's database couldn't be made: {error}."));
                }
            }
        }
        db
    }

    /// The file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the file is from a later version of Beatlayer, and is left
    /// alone.
    #[must_use]
    pub fn is_newer(&self) -> bool {
        self.newer
    }

    /// What to tell the person, if a file that couldn't be read was set
    /// aside.
    #[must_use]
    pub fn damaged(&self) -> Option<&str> {
        self.damaged.as_deref()
    }

    /// Renames a file that couldn't be read aside and builds a new one.
    fn set_aside(&mut self, reason: &str) {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "browser.db".to_owned());
        let aside = self.path.parent().map_or_else(
            || PathBuf::from(format!("{name}.damaged")),
            |parent| parent.join(format!("{name}.damaged")),
        );
        let moved = std::fs::rename(&self.path, &aside).is_ok();
        self.damaged = Some(if moved {
            format!(
                "The browser's database couldn't be read ({reason}). It was set aside as \
                 {} and a new one was built.",
                aside.file_name().unwrap_or_default().to_string_lossy()
            )
        } else {
            format!("The browser's database couldn't be read ({reason}). A new one was built.")
        });
        match connect(&self.path) {
            Ok(Some(connection)) => self.start(connection),
            _ => {
                self.damaged = Some(format!(
                    "The browser's database couldn't be read ({reason}), and a new one couldn't be \
                 built. Its tags and index start empty."
                ))
            }
        }
    }

    /// Starts the writer thread, which owns the connection.
    fn start(&mut self, connection: rusqlite::Connection) {
        let (sender, receiver) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("bl-browser-db".to_owned())
            .spawn(move || write_loop(connection, receiver));
        if spawned.is_ok() {
            self.requests = Some(sender);
        }
    }

    /// Reads everything the file holds.
    #[must_use]
    pub fn load(&self) -> Loaded {
        let Some(sender) = &self.requests else {
            return Loaded::default();
        };
        let (reply, answers) = mpsc::channel();
        if sender.send(Request::Load(reply)).is_err() {
            return Loaded::default();
        }
        answers.recv().unwrap_or_default()
    }

    /// Reads the tags the file holds: the groups, the tags, and the items
    /// that carry them.
    #[must_use]
    pub fn load_tags(&self) -> Tags {
        let Some(sender) = &self.requests else {
            return (Vec::new(), Vec::new(), Vec::new());
        };
        let (reply, answers) = mpsc::channel();
        if sender.send(Request::LoadTags(reply)).is_err() {
            return (Vec::new(), Vec::new(), Vec::new());
        }
        answers.recv().unwrap_or_default()
    }

    /// Writes the changes to the index.
    pub fn save_files(&self, ops: Vec<FileOp>) {
        if let Some(sender) = &self.requests {
            let _ = sender.send(Request::Files(ops));
        }
    }

    /// Writes the changes to the tags.
    pub fn save_tags(&self, ops: Vec<TagOp>) {
        if let Some(sender) = &self.requests {
            let _ = sender.send(Request::Tags(ops));
        }
    }

    /// Writes the changes to the saved labels.
    pub fn save_labels(&self, ops: Vec<LabelOp>) {
        if let Some(sender) = &self.requests {
            let _ = sender.send(Request::Labels(ops));
        }
    }

    /// Writes everything that's waiting, and waits for it, for tests.
    pub fn flush(&self) {
        let Some(sender) = &self.requests else {
            return;
        };
        let (reply, answers) = mpsc::channel();
        if sender.send(Request::Flush(reply)).is_ok() {
            let _ = answers.recv();
        }
    }
}

/// Opens the file, or makes it, and says which schema it holds: `None`
/// when the file is from a later version of Beatlayer and is left alone.
fn connect(path: &Path) -> Result<Option<rusqlite::Connection>, String> {
    let existed = path.exists();
    let connection = rusqlite::Connection::open(path)
        .map_err(|error| format!("couldn't open {}: {error}", path.display()))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("couldn't set the busy timeout: {error}"))?;
    let version = schema_version(&connection)?;
    if version > SCHEMA_VERSION {
        return Ok(None);
    }
    if version == 0 {
        // A file with no schema version: it's either new or damaged.
        let tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN \
                 ('files', 'tag_groups', 'tags', 'item_tags')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("couldn't read the file: {error}"))?;
        if tables > 0 && existed {
            return Err("the file isn't a database Beatlayer wrote".to_owned());
        }
        migrate(&connection, 0)?;
        return Ok(Some(connection));
    }
    if version < SCHEMA_VERSION {
        migrate(&connection, version)?;
    }
    Ok(Some(connection))
}

/// The schema version the file records.
fn schema_version(connection: &rusqlite::Connection) -> Result<i64, String> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("couldn't read the schema version: {error}"))
}

/// Brings a file of an earlier version up to this one.
fn migrate(connection: &rusqlite::Connection, from: i64) -> Result<(), String> {
    if from > SCHEMA_VERSION {
        return Err("the file is from a later version of Beatlayer".to_owned());
    }
    connection
        .execute_batch(SCHEMA)
        .map_err(|error| format!("couldn't make the tables: {error}"))?;
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|error| format!("couldn't record the schema version: {error}"))?;
    Ok(())
}

/// The writer thread: it owns the connection and does what it's asked.
fn write_loop(connection: rusqlite::Connection, requests: Receiver<Request>) {
    while let Ok(request) = requests.recv() {
        match request {
            Request::Load(reply) => {
                let loaded = read_all(&connection);
                let _ = reply.send(loaded);
            }
            Request::LoadTags(reply) => {
                let (groups, tags, item_tags) = read_tags(&connection);
                let _ = reply.send((groups, tags, item_tags));
            }
            Request::Files(ops) => write_files(&connection, &ops),
            Request::Tags(ops) => write_tags(&connection, &ops),
            Request::Labels(ops) => write_labels(&connection, &ops),
            Request::Flush(reply) => {
                let _ = reply.send(());
            }
        }
    }
}

/// Reads everything the file holds.
fn read_all(connection: &rusqlite::Connection) -> Loaded {
    let files = rows(
        connection,
        "SELECT path, name, lower_name, kind, place, size, modified, hash FROM files",
        |row| {
            Ok(FileRow {
                path: row.get(0)?,
                name: row.get(1)?,
                lower: row.get(2)?,
                kind: row.get(3)?,
                place: row.get(4)?,
                size: row.get::<_, i64>(5)?.max(0) as u64,
                modified: row.get(6)?,
                hash: row.get(7)?,
            })
        },
    );
    let (groups, tags, item_tags) = read_tags(connection);
    Loaded {
        files,
        groups,
        tags,
        item_tags,
        saved_labels: read_labels(connection),
    }
}

/// Reads the saved labels the file holds.
fn read_labels(connection: &rusqlite::Connection) -> Vec<SavedLabel> {
    let mut labels: Vec<SavedLabel> = rows(
        connection,
        "SELECT id, name, base, search, icon FROM saved_labels ORDER BY id",
        |row| {
            let base: String = row.get(2)?;
            Ok(SavedLabel {
                id: row.get(0)?,
                name: row.get(1)?,
                base: serde_json::from_str(&base).unwrap_or(bl_shell_spec::browser::Label::All),
                search: row.get(3)?,
                icon: row.get(4)?,
                tags: Vec::new(),
            })
        },
    );
    for (label, tag) in rows(
        connection,
        "SELECT label_id, tag_id FROM saved_label_tags ORDER BY label_id, tag_id",
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    ) {
        if let Some(saved) = labels.iter_mut().find(|saved| saved.id == label) {
            saved.tags.push(tag);
        }
    }
    labels
}

/// Writes the changes to the saved labels, in one transaction.
fn write_labels(connection: &rusqlite::Connection, ops: &[LabelOp]) {
    if ops.is_empty() {
        return;
    }
    let Ok(transaction) = connection.unchecked_transaction() else {
        return;
    };
    for op in ops {
        match op {
            LabelOp::Save(label) => {
                let base = serde_json::to_string(&label.base).unwrap_or_default();
                let _ = transaction.execute(
                    "INSERT INTO saved_labels (id, name, base, search, icon) \
                     VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(id) DO UPDATE SET \
                     name = excluded.name, base = excluded.base, search = excluded.search, \
                     icon = excluded.icon",
                    rusqlite::params![label.id, label.name, base, label.search, label.icon],
                );
                let _ = transaction.execute(
                    "DELETE FROM saved_label_tags WHERE label_id = ?1",
                    rusqlite::params![label.id],
                );
                for tag in &label.tags {
                    let _ = transaction.execute(
                        "INSERT OR IGNORE INTO saved_label_tags (label_id, tag_id) VALUES (?1, ?2)",
                        rusqlite::params![label.id, tag],
                    );
                }
            }
            LabelOp::Delete { id } => {
                let _ = transaction.execute(
                    "DELETE FROM saved_label_tags WHERE label_id = ?1",
                    rusqlite::params![id],
                );
                let _ = transaction.execute(
                    "DELETE FROM saved_labels WHERE id = ?1",
                    rusqlite::params![id],
                );
            }
        }
    }
    let _ = transaction.commit();
}

/// The rows of a query, read into a vector. A query that can't be run or
/// read holds nothing.
fn rows<T>(
    connection: &rusqlite::Connection,
    sql: &str,
    read: impl Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Vec<T> {
    let Ok(mut statement) = connection.prepare(sql) else {
        return Vec::new();
    };
    let Ok(entries) = statement.query_map([], read) else {
        return Vec::new();
    };
    entries.flatten().collect()
}

/// Reads the tags the file holds.
fn read_tags(connection: &rusqlite::Connection) -> Tags {
    let groups = rows(
        connection,
        "SELECT id, name, position, builtin FROM tag_groups ORDER BY position",
        |row| {
            Ok(TagGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                position: row.get(2)?,
                builtin: row.get::<_, i64>(3)? != 0,
                first: row.get(2)?,
            })
        },
    );
    let tags = rows(
        connection,
        "SELECT id, group_id, name, parent_id, position, builtin FROM tags ORDER BY position",
        |row| {
            Ok(Tag {
                id: row.get(0)?,
                group: row.get(1)?,
                name: row.get(2)?,
                parent: row.get(3)?,
                position: row.get(4)?,
                builtin: row.get::<_, i64>(5)? != 0,
                first: row.get(4)?,
            })
        },
    );
    let mut item_tags: Vec<(String, Vec<i64>)> = Vec::new();
    for (key, tag) in rows(
        connection,
        "SELECT item_key, tag_id FROM item_tags ORDER BY item_key",
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    ) {
        match item_tags.iter_mut().find(|(name, _)| *name == key) {
            Some((_, tags)) => tags.push(tag),
            None => item_tags.push((key, vec![tag])),
        }
    }
    (groups, tags, item_tags)
}

/// Writes the changes to the index, in one transaction.
fn write_files(connection: &rusqlite::Connection, ops: &[FileOp]) {
    if ops.is_empty() {
        return;
    }
    let Ok(transaction) = connection.unchecked_transaction() else {
        return;
    };
    for op in ops {
        match op {
            FileOp::Upsert(row) => {
                let _ = transaction.execute(
                    "INSERT INTO files (path, name, lower_name, kind, place, size, modified, hash) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(path) DO UPDATE SET \
                     name = excluded.name, lower_name = excluded.lower_name, \
                     kind = excluded.kind, place = excluded.place, size = excluded.size, \
                     modified = excluded.modified",
                    rusqlite::params![
                        row.path,
                        row.name,
                        row.lower,
                        row.kind,
                        row.place,
                        row.size as i64,
                        row.modified,
                        row.hash,
                    ],
                );
            }
            FileOp::Delete(path) => {
                let _ = transaction.execute("DELETE FROM files WHERE path = ?1", [path]);
            }
            FileOp::Hash { path, hash } => {
                let _ = transaction.execute(
                    "UPDATE files SET hash = ?2 WHERE path = ?1",
                    rusqlite::params![path, hash],
                );
            }
        }
    }
    let _ = transaction.commit();
}

/// Writes the changes to the tags, in one transaction.
fn write_tags(connection: &rusqlite::Connection, ops: &[TagOp]) {
    if ops.is_empty() {
        return;
    }
    let Ok(transaction) = connection.unchecked_transaction() else {
        return;
    };
    for op in ops {
        match op {
            TagOp::AddGroup {
                id,
                name,
                position,
                builtin,
            } => {
                let _ = transaction.execute(
                    "INSERT INTO tag_groups (id, name, position, builtin) VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT(id) DO UPDATE SET name = excluded.name, \
                     position = excluded.position, builtin = excluded.builtin",
                    rusqlite::params![id, name, position, i64::from(*builtin)],
                );
            }
            TagOp::AddTag {
                id,
                group,
                name,
                parent,
                position,
                builtin,
            } => {
                let _ = transaction.execute(
                    "INSERT INTO tags (id, group_id, name, parent_id, position, builtin) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(id) DO UPDATE SET \
                     group_id = excluded.group_id, name = excluded.name, \
                     parent_id = excluded.parent_id, position = excluded.position, \
                     builtin = excluded.builtin",
                    rusqlite::params![id, group, name, parent, position, i64::from(*builtin)],
                );
            }
            TagOp::RenameTag { id, name } => {
                let _ = transaction.execute(
                    "UPDATE tags SET name = ?2 WHERE id = ?1",
                    rusqlite::params![id, name],
                );
            }
            TagOp::RenameGroup { id, name } => {
                let _ = transaction.execute(
                    "UPDATE tag_groups SET name = ?2 WHERE id = ?1",
                    rusqlite::params![id, name],
                );
            }
            TagOp::DeleteTag { id } => {
                let _ = transaction.execute(
                    "DELETE FROM item_tags WHERE tag_id = ?1",
                    rusqlite::params![id],
                );
                let _ = transaction.execute(
                    "DELETE FROM saved_label_tags WHERE tag_id = ?1",
                    rusqlite::params![id],
                );
                let _ =
                    transaction.execute("DELETE FROM tags WHERE id = ?1", rusqlite::params![id]);
            }
            TagOp::DeleteGroup { id } => {
                let _ = transaction.execute(
                    "DELETE FROM item_tags WHERE tag_id IN (SELECT id FROM tags WHERE group_id = ?1)",
                    rusqlite::params![id],
                );
                let _ = transaction.execute(
                    "DELETE FROM saved_label_tags WHERE tag_id IN \
                     (SELECT id FROM tags WHERE group_id = ?1)",
                    rusqlite::params![id],
                );
                let _ = transaction.execute(
                    "DELETE FROM tags WHERE group_id = ?1",
                    rusqlite::params![id],
                );
                let _ = transaction.execute(
                    "DELETE FROM tag_groups WHERE id = ?1",
                    rusqlite::params![id],
                );
            }
            TagOp::MoveTag { id, position } => {
                let _ = transaction.execute(
                    "UPDATE tags SET position = ?2 WHERE id = ?1",
                    rusqlite::params![id, position],
                );
            }
            TagOp::MoveGroup { id, position } => {
                let _ = transaction.execute(
                    "UPDATE tag_groups SET position = ?2 WHERE id = ?1",
                    rusqlite::params![id, position],
                );
            }
            TagOp::SetItemTags { key, tags } => {
                let _ = transaction.execute(
                    "DELETE FROM item_tags WHERE item_key = ?1",
                    rusqlite::params![key],
                );
                for tag in tags {
                    let _ = transaction.execute(
                        "INSERT OR IGNORE INTO item_tags (item_key, tag_id) VALUES (?1, ?2)",
                        rusqlite::params![key, tag],
                    );
                }
            }
        }
    }
    let _ = transaction.commit();
}

/// What an item's kind is called in the database.
#[must_use]
pub fn kind_name(kind: ItemKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "audio".to_owned())
}

/// The kind a name in the database stands for.
#[must_use]
pub fn kind_from_name(name: &str) -> Option<ItemKind> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bl-browser-db-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the folder");
        dir
    }

    #[test]
    fn a_database_is_made_read_and_written_again() {
        // Spec: browser-tags R1, R4.
        let dir = scratch("cycle");
        let path = dir.join("browser.db");
        let db = Db::open(path.clone());
        assert!(db.damaged().is_none(), "{:?}", db.damaged());
        let loaded = db.load();
        assert!(loaded.files.is_empty());
        db.save_files(vec![FileOp::Upsert(FileRow {
            path: "/lib/Kick.wav".to_owned(),
            name: "Kick.wav".to_owned(),
            lower: "kick.wav".to_owned(),
            kind: kind_name(ItemKind::Audio),
            place: "/lib".to_owned(),
            size: 10,
            modified: 5,
            hash: None,
        })]);
        db.flush();
        drop(db);
        let db = Db::open(path);
        let loaded = db.load();
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files[0].name, "Kick.wav");
        assert_eq!(loaded.files[0].size, 10);
    }

    #[test]
    fn a_damaged_database_is_set_aside_and_built_again() {
        // Spec: browser-tags R3.
        let dir = scratch("damaged");
        let path = dir.join("browser.db");
        std::fs::write(&path, b"this is not a database").expect("the file");
        let db = Db::open(path.clone());
        assert!(db.damaged().is_some(), "{:?}", db.damaged());
        assert!(path.exists(), "a new database is built");
        assert!(
            dir.join("browser.db.damaged").exists(),
            "the damaged file is set aside"
        );
    }

    #[test]
    fn a_newer_database_is_left_alone() {
        // Spec: browser-tags R4.
        let dir = scratch("newer");
        let path = dir.join("browser.db");
        {
            let db = Db::open(path.clone());
            db.flush();
        }
        let connection = rusqlite::Connection::open(&path).expect("the file");
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .expect("the version");
        drop(connection);
        let before = std::fs::metadata(&path).expect("the file").len();
        let db = Db::open(path.clone());
        assert!(db.is_newer());
        assert!(db.damaged().is_none());
        db.save_files(vec![FileOp::Delete("/lib/Kick.wav".to_owned())]);
        db.flush();
        drop(db);
        let connection = rusqlite::Connection::open(&path).expect("the file");
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("the version");
        assert_eq!(version, SCHEMA_VERSION + 1, "the version is untouched");
        assert_eq!(
            std::fs::metadata(&path).expect("the file").len(),
            before,
            "the file isn't written to"
        );
    }
}
