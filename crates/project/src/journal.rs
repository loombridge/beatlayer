//! The journal: every edit since the last save, in a file beside the set
//! (IMP-05, WP-1.11).
//!
//! While a set is open, each finished edit is appended to a journal, so a
//! crash loses nothing that was journalled. A journal belongs to one
//! session (one run of the app with one set open) and follows one saved
//! revision of the set. It's a hidden file beside the set, named after the
//! set's file and the session:
//!
//! ```text
//! My Song Project/
//!   My Song.blset
//!   .My Song.blset.1a2b-19c8d3e4f00-0.bljournal
//! ```
//!
//! A set that has never been saved journals into the recovery folder
//! instead ([`crate::autosave`]).
//!
//! # Entries
//!
//! A journal is a run of zstd frames like a set file's, each holding one
//! entry as JSON:
//!
//! ```text
//! {
//!   "format": "beatlayer-journal",
//!   "version": 1,
//!   "session": "1a2b-19c8d3e4f00-0",
//!   "seq": 3,                          1, 2, 3, ... within the journal
//!   "follows": { "revision": 7, "fingerprint": 1234 },   or null for a set never saved
//!   "time": 1791642612000,             milliseconds since 1970, UTC
//!   "label": "Move Clip",              the edit's undo label
//!   "committed": false,                a save point (Save Continuously) or not
//!   "change": { "snapshot": { … } }    or { "patch": { … } }
//! }
//! ```
//!
//! The first entry, and every so often another, is a `snapshot`: the whole
//! saved form of the set, as a set file holds it. The others are a `patch`
//! from the entry before. An entry is the state of the song after one
//! finished edit (or after several, when they come faster than the disk).
//!
//! # Save points and recovery points
//!
//! With Save Continuously on, entries are **committed**: they're save
//! points, part of the set as saved, so [`load`](crate::load) and
//! [`SetFile::open`](crate::SetFile::open) open the set at its newest save
//! point, and [`save_points`] lists them. Otherwise entries are recovery
//! points, which nothing applies unless the session ended without closing
//! (a crash) and the user chooses to recover it. A normal save writes the
//! next revision and retires the journal.
//!
//! A write cut short leaves a damaged last frame, which the reader ignores:
//! a journal reads as every whole entry before it.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::LoadError;
use crate::format::{LoadOptions, Project, SetInfo, compress, decode_value, decompress_frame};
use crate::patch::{self, Patch};

/// The file extension of a journal.
pub const JOURNAL_EXTENSION: &str = "bljournal";

/// The `format` field of every journal entry.
const FORMAT: &str = "beatlayer-journal";

/// The entry layout this build writes, and the newest it reads.
const VERSION: u32 = 1;

/// The most patches between snapshots, so reading a journal never replays
/// more than this many.
const MAX_PATCHES: u64 = 256;

/// A snapshot is written once the patches since the last one take this many
/// times its size, so snapshots add at most a quarter to a journal.
const SNAPSHOT_RATIO: u64 = 4;

/// The saved revision of a set that a journal follows: its revision number
/// and the [`fingerprint`] of its file, so a journal is never applied to a
/// different file that happens to have the same revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Follows {
    /// The set's revision.
    pub revision: u64,
    /// The fingerprint of the set file's bytes.
    pub fingerprint: u64,
}

/// A fingerprint of a set file's bytes (64-bit FNV-1a). It tells two files
/// apart; it isn't a cryptographic hash.
#[must_use]
pub fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// One entry in a journal: the song after a finished edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalEntry {
    /// Its place in the journal, from 1.
    pub seq: u64,
    /// When it was written.
    pub time: SystemTime,
    /// The edit's label, as the undo history names it.
    pub label: String,
    /// Whether it's a save point (written with Save Continuously on).
    pub committed: bool,
}

/// An entry as it's written.
#[derive(Serialize)]
struct EntryOut<'a> {
    format: &'static str,
    version: u32,
    session: &'a str,
    seq: u64,
    follows: Option<Follows>,
    time: u64,
    label: &'a str,
    committed: bool,
    change: ChangeOut<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ChangeOut<'a> {
    Snapshot(&'a Value),
    Patch(&'a Patch),
}

/// An entry as it's read.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryIn {
    format: String,
    version: u32,
    session: String,
    seq: u64,
    follows: Option<Follows>,
    time: u64,
    label: String,
    committed: bool,
    change: ChangeIn,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum ChangeIn {
    Snapshot(Value),
    Patch(Patch),
}

/// The two fields every version of an entry keeps, to recognise an entry
/// from a newer version of Beatlayer.
#[derive(Deserialize)]
struct EntryProbe {
    format: String,
    version: u32,
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn from_millis(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

/// What a journal file holds, as far as it reads.
#[derive(Clone, Debug, Default)]
pub(crate) struct JournalRead {
    /// The revision it follows (the same for every entry).
    pub(crate) follows: Option<Follows>,
    /// Every whole entry, in order.
    pub(crate) entries: Vec<JournalEntry>,
    /// The saved form after the newest committed entry.
    pub(crate) committed: Option<Value>,
    /// The saved form after the last entry.
    pub(crate) last: Option<Value>,
    /// The file's length through the newest committed entry (0 if none).
    pub(crate) committed_len: u64,
    /// Whether bytes after the last whole entry were ignored: a write cut
    /// short, or damage.
    pub(crate) damaged: bool,
    /// Whether reading stopped at an entry from a newer version of
    /// Beatlayer, which this build leaves alone.
    pub(crate) newer: bool,
}

impl JournalRead {
    /// The index of the newest committed entry.
    pub(crate) fn last_committed(&self) -> Option<usize> {
        self.entries.iter().rposition(|entry| entry.committed)
    }

    /// How many entries follow the newest committed one: the edits only a
    /// recovery can bring back.
    pub(crate) fn uncommitted(&self) -> usize {
        self.entries.len() - self.last_committed().map_or(0, |i| i + 1)
    }
}

/// Reads a journal's bytes: every whole entry up to the first that's
/// damaged, out of sequence, or from a newer version.
pub(crate) fn read_bytes(bytes: &[u8], options: &LoadOptions) -> JournalRead {
    read_up_to(bytes, options, None)
}

/// Reads a journal file. A missing file reads as an empty journal.
pub(crate) fn read_file(path: &Path, options: &LoadOptions) -> io::Result<JournalRead> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(read_bytes(&bytes, options)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(JournalRead::default()),
        Err(error) => Err(error),
    }
}

/// Reads a journal, keeping the state after entry `keep` (an index) in
/// `committed` instead of the newest committed one.
fn read_up_to(bytes: &[u8], options: &LoadOptions, keep: Option<usize>) -> JournalRead {
    let mut rest = bytes;
    let mut parsed: Vec<(EntryIn, u64)> = Vec::new();
    let mut damaged = false;
    let mut newer = false;
    while !rest.is_empty() {
        let json = decompress_frame(&mut rest, options.max_json_bytes).ok();
        let entry = json
            .as_ref()
            .and_then(|json| serde_json::from_slice::<EntryIn>(json).ok());
        let Some(entry) = entry.filter(|entry| fits(entry, parsed.last().map(|(e, _)| e))) else {
            damaged = true;
            newer = json
                .and_then(|json| serde_json::from_slice::<EntryProbe>(&json).ok())
                .is_some_and(|probe| probe.format == FORMAT && probe.version > VERSION);
            break;
        };
        let end = u64::try_from(bytes.len() - rest.len()).unwrap_or(u64::MAX);
        parsed.push((entry, end));
    }

    let keep = keep.or_else(|| parsed.iter().rposition(|(entry, _)| entry.committed));
    let mut read = JournalRead {
        follows: parsed.first().and_then(|(entry, _)| entry.follows),
        damaged,
        newer,
        ..JournalRead::default()
    };
    let mut state: Option<Value> = None;
    let ends: Vec<u64> = parsed.iter().map(|(_, end)| *end).collect();
    for (i, (entry, end)) in parsed.into_iter().enumerate() {
        match entry.change {
            ChangeIn::Snapshot(value) => state = Some(value),
            ChangeIn::Patch(patch) => {
                let applied = state
                    .as_mut()
                    .ok_or_else(String::new)
                    .and_then(|state| patch::apply(state, &patch));
                if applied.is_err() {
                    // A patch that doesn't fit is damage: read again without
                    // it and what follows. Entry 0 is a snapshot, so i > 0.
                    let cut = usize::try_from(ends[i - 1]).unwrap_or(bytes.len());
                    let mut again = read_up_to(&bytes[..cut], options, keep.filter(|k| *k < i));
                    again.damaged = true;
                    return again;
                }
            }
        }
        if Some(i) == keep {
            read.committed.clone_from(&state);
            read.committed_len = if entry.committed { end } else { 0 };
        }
        read.entries.push(JournalEntry {
            seq: entry.seq,
            time: from_millis(entry.time),
            label: entry.label,
            committed: entry.committed,
        });
    }
    read.last = state;
    read
}

/// Whether an entry can follow the one before it in the same journal.
fn fits(entry: &EntryIn, before: Option<&EntryIn>) -> bool {
    if entry.format != FORMAT || entry.version == 0 || entry.version > VERSION {
        return false;
    }
    match before {
        None => entry.seq == 1 && matches!(entry.change, ChangeIn::Snapshot(_)),
        Some(before) => {
            Some(entry.seq) == before.seq.checked_add(1)
                && entry.session == before.session
                && entry.follows == before.follows
        }
    }
}

/// A new state to journal.
#[derive(Debug)]
pub(crate) struct Pending {
    /// The saved form of the song after the edit.
    pub(crate) value: Value,
    /// The edit's label.
    pub(crate) label: String,
    /// When the edit finished.
    pub(crate) time: SystemTime,
    /// Whether it's a save point.
    pub(crate) committed: bool,
}

/// Appends entries to one journal file. Every append is one write, flushed
/// to disk before it returns; one that fails is cut off again, so the file
/// always ends with a whole entry.
#[derive(Debug)]
pub(crate) struct JournalWriter {
    path: PathBuf,
    file: File,
    session: String,
    follows: Option<Follows>,
    /// The last entry's sequence number: 0 before the first.
    seq: u64,
    /// The saved form after the last entry.
    state: Value,
    /// Whether the last entry is committed.
    last_committed: bool,
    len: u64,
    committed_len: u64,
    uncommitted: u64,
    since_snapshot: u64,
    patch_bytes: u64,
    snapshot_bytes: u64,
}

impl JournalWriter {
    /// Creates a new journal file. It fails if the file exists.
    pub(crate) fn create(
        path: PathBuf,
        session: &str,
        follows: Option<Follows>,
    ) -> io::Result<Self> {
        // Not in append mode: on Windows a file opened to append can't be
        // cut short again, so each write seeks to the end it keeps.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self {
            path,
            file,
            session: session.to_owned(),
            follows,
            seq: 0,
            state: Value::Null,
            last_committed: false,
            len: 0,
            committed_len: 0,
            uncommitted: 0,
            since_snapshot: 0,
            patch_bytes: 0,
            snapshot_bytes: 0,
        })
    }

    /// The journal's path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// How many entries follow the newest committed one.
    #[cfg(test)]
    pub(crate) fn uncommitted(&self) -> u64 {
        self.uncommitted
    }

    /// Appends an entry for each new state, in order, skipping any that
    /// match the state before (unless it commits what was uncommitted), and
    /// flushes them to disk. Returns how many entries it wrote.
    pub(crate) fn append(&mut self, mut batch: Vec<Pending>) -> io::Result<usize> {
        let mut buffer = Vec::new();
        let mut seq = self.seq;
        let mut since = self.since_snapshot;
        let mut patch_bytes = self.patch_bytes;
        let mut snapshot_bytes = self.snapshot_bytes;
        let mut committed_len = self.committed_len;
        let mut uncommitted = self.uncommitted;
        let mut last_committed = self.last_committed;
        let mut last: Option<usize> = None;
        let empty = Patch::Object {
            set: std::collections::BTreeMap::new(),
            remove: Vec::new(),
        };
        for (i, pending) in batch.iter().enumerate() {
            let before = last.map_or(&self.state, |j| &batch[j].value);
            let first = seq == 0;
            let patch = if first {
                None
            } else {
                patch::diff(before, &pending.value)
            };
            let commits = pending.committed && !last_committed;
            if !first && patch.is_none() && !commits {
                continue;
            }
            let snapshot = first
                || since >= MAX_PATCHES
                || patch_bytes >= snapshot_bytes.saturating_mul(SNAPSHOT_RATIO);
            seq += 1;
            let entry = EntryOut {
                format: FORMAT,
                version: VERSION,
                session: &self.session,
                seq,
                follows: self.follows,
                time: millis(pending.time),
                label: &pending.label,
                committed: pending.committed,
                change: if snapshot {
                    ChangeOut::Snapshot(&pending.value)
                } else {
                    ChangeOut::Patch(patch.as_ref().unwrap_or(&empty))
                },
            };
            let frame = compress(&serde_json::to_vec(&entry).map_err(io::Error::other)?);
            let frame_len = u64::try_from(frame.len()).unwrap_or(u64::MAX);
            if snapshot {
                snapshot_bytes = frame_len;
                patch_bytes = 0;
                since = 0;
            } else {
                patch_bytes = patch_bytes.saturating_add(frame_len);
                since += 1;
            }
            buffer.extend_from_slice(&frame);
            if pending.committed {
                committed_len = self.len + u64::try_from(buffer.len()).unwrap_or(u64::MAX);
                uncommitted = 0;
            } else {
                uncommitted += 1;
            }
            last_committed = pending.committed;
            last = Some(i);
        }
        let Some(last) = last else {
            return Ok(0);
        };
        let written = self
            .file
            .seek(SeekFrom::Start(self.len))
            .and_then(|_| self.file.write_all(&buffer))
            .and_then(|()| self.file.sync_data());
        if let Err(error) = written {
            // Best effort: cut the partial write off again.
            let _ = self.file.set_len(self.len);
            return Err(error);
        }
        let count = usize::try_from(seq - self.seq).unwrap_or(usize::MAX);
        self.len += u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        self.seq = seq;
        self.since_snapshot = since;
        self.patch_bytes = patch_bytes;
        self.snapshot_bytes = snapshot_bytes;
        self.committed_len = committed_len;
        self.uncommitted = uncommitted;
        self.last_committed = last_committed;
        self.state = batch.swap_remove(last).value;
        Ok(count)
    }

    /// Appends an entry that commits the current state, if the newest
    /// entry isn't committed already: what turning Save Continuously on
    /// does to the edits before it.
    pub(crate) fn commit(&mut self, label: &str, time: SystemTime) -> io::Result<usize> {
        if self.seq == 0 || self.last_committed {
            return Ok(0);
        }
        let pending = Pending {
            value: self.state.clone(),
            label: label.to_owned(),
            time,
            committed: true,
        };
        self.append(vec![pending])
    }

    /// Cuts off every entry after the newest committed one, as closing a
    /// set without saving does. Returns whether any entry is left.
    pub(crate) fn discard_uncommitted(self) -> io::Result<bool> {
        if self.uncommitted > 0 {
            self.file.set_len(self.committed_len)?;
            self.file.sync_all()?;
        }
        Ok(self.committed_len > 0)
    }
}

/// Cuts off every entry of a journal file after the newest committed one,
/// deleting the file if none is left. A missing file is fine.
pub(crate) fn discard_uncommitted_file(path: &Path, read: &JournalRead) -> io::Result<()> {
    if read.committed_len == 0 {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(read.committed_len)?;
    file.sync_all()
}

/// The journal a session keeps beside a set:
/// `.<set file name>.<session>.bljournal`.
pub(crate) fn beside(set: &Path, session: &str) -> Option<PathBuf> {
    let name = set.file_name()?.to_string_lossy();
    let dir = match set.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Some(dir.join(format!(".{name}.{session}.{JOURNAL_EXTENSION}")))
}

/// Every session's journal beside a set, ordered by path. A missing folder
/// has none.
///
/// # Errors
///
/// If the set's folder can't be read.
pub fn journals_beside(set: &Path) -> io::Result<Vec<PathBuf>> {
    let Some(name) = set.file_name() else {
        return Ok(Vec::new());
    };
    let prefix = format!(".{}.", name.to_string_lossy());
    let suffix = format!(".{JOURNAL_EXTENSION}");
    let dir = match set.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let mut journals = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(journals),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let session = file_name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(&suffix));
        if session.is_some_and(|s| !s.is_empty() && !s.contains('.')) {
            journals.push(entry.path());
        }
    }
    journals.sort();
    Ok(journals)
}

/// The session a journal beside a set belongs to, from its name.
pub(crate) fn session_of(journal: &Path) -> Option<String> {
    let name = journal.file_name()?.to_string_lossy().into_owned();
    let stem = name.strip_suffix(&format!(".{JOURNAL_EXTENSION}"))?;
    let session = stem.rsplit('.').next()?;
    (!session.is_empty()).then(|| session.to_owned())
}

/// A save point: a committed entry in a journal beside a set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavePoint {
    /// The journal it's in.
    pub journal: PathBuf,
    /// Its index in that journal.
    pub index: usize,
    /// The entry.
    pub entry: JournalEntry,
}

/// The save points that follow a set's saved revision, oldest first: the
/// history Save Continuously keeps since the last normal save. A journal
/// that can't be read is skipped.
#[must_use]
pub fn save_points(set: &Path, follows: Follows) -> Vec<SavePoint> {
    let options = LoadOptions::default();
    let mut points = Vec::new();
    for journal in journals_beside(set).unwrap_or_default() {
        let Ok(read) = read_file(&journal, &options) else {
            continue;
        };
        if read.follows != Some(follows) {
            continue;
        }
        for (index, entry) in read.entries.into_iter().enumerate() {
            if entry.committed {
                points.push(SavePoint {
                    journal: journal.clone(),
                    index,
                    entry,
                });
            }
        }
    }
    points.sort_by(|a, b| {
        (a.entry.time, &a.journal, a.index).cmp(&(b.entry.time, &b.journal, b.index))
    });
    points
}

/// The song at a save point.
///
/// # Errors
///
/// [`LoadError::Io`] if the journal can't be read, [`LoadError::Corrupt`] if
/// the save point is no longer in it, and any error of loading the saved
/// form it holds.
pub fn restore(point: &SavePoint) -> Result<Project, LoadError> {
    restore_with_info(point).map(|(project, _)| project)
}

/// [`restore`], with what the save point's saved form says about itself, as
/// opening the set would migrate it by (the schema it was saved in).
///
/// # Errors
///
/// As [`restore`].
pub fn restore_with_info(point: &SavePoint) -> Result<(Project, SetInfo), LoadError> {
    let options = LoadOptions::default();
    let bytes = read_whole(&point.journal)?;
    let read = read_up_to(&bytes, &options, Some(point.index));
    let value = read
        .committed
        .filter(|_| read.entries.get(point.index) == Some(&point.entry))
        .ok_or_else(|| LoadError::corrupt("the save point is no longer in its journal"))?;
    decode_value(&value, &options)
}

fn read_whole(path: &Path) -> Result<Vec<u8>, LoadError> {
    let io = |source| LoadError::Io {
        path: path.to_owned(),
        source,
    };
    let mut bytes = Vec::new();
    // A journal holds many entries, each up to the JSON limit; reading it
    // whole is bounded by the disk, as it's Beatlayer's own file.
    File::open(path)
        .map_err(io)?
        .read_to_end(&mut bytes)
        .map_err(io)?;
    Ok(bytes)
}

/// The song at the newest save point that follows a set's revision, if
/// any journal beside it has one: what opening the set gives with Save
/// Continuously's edits applied, with what the save point's saved form says
/// about itself. Journals that can't be read, or whose save point doesn't
/// load, are skipped.
pub(crate) fn newest_save_point(
    set: &Path,
    follows: Follows,
    options: &LoadOptions,
) -> Option<(Project, SetInfo)> {
    let mut newest: Option<((SystemTime, u64), Value)> = None;
    for journal in journals_beside(set).ok()? {
        let Ok(read) = read_file(&journal, options) else {
            continue;
        };
        if read.follows != Some(follows) {
            continue;
        }
        let (Some(index), Some(value)) = (read.last_committed(), read.committed) else {
            continue;
        };
        let entry = &read.entries[index];
        let key = (entry.time, entry.seq);
        if newest.as_ref().is_none_or(|(best, _)| key > *best) {
            newest = Some((key, value));
        }
    }
    let (_, value) = newest?;
    decode_value(&value, options).ok()
}

#[cfg(test)]
mod tests {
    use bl_model::{Command, Editor, TrackKind};

    use super::*;
    use crate::format::to_value;
    use crate::media::Media;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("bl-journal-{label}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Songs after each of `n` edits, to a song of 40 tracks.
    fn states(n: usize) -> Vec<Project> {
        let mut editor = Editor::new();
        let mut track = editor.document().main_id();
        for index in 0..40 {
            track = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id: track,
                    kind: TrackKind::Audio,
                    name: format!("Track {index}"),
                    parent: None,
                    index,
                })
                .unwrap();
        }
        let mut out = Vec::new();
        for step in 0..n {
            let volume = f64::from(u32::try_from(step).unwrap()) / 100.0;
            editor.apply(Command::SetVolume { track, volume }).unwrap();
            out.push(Project::from_editor(&editor, Media::new()));
        }
        out
    }

    fn pending(project: &Project, committed: bool) -> Pending {
        Pending {
            value: to_value(project, 3).unwrap(),
            label: "Set Track Volume".to_owned(),
            time: SystemTime::now(),
            committed,
        }
    }

    fn decoded(value: &Value) -> Project {
        decode_value(value, &LoadOptions::default()).unwrap().0
    }

    const FOLLOWS: Follows = Follows {
        revision: 3,
        fingerprint: 99,
    };

    #[test]
    fn frames_follow_one_another() {
        // The reader relies on a frame's decoder stopping at its end.
        let mut bytes = compress(b"{\"a\":1}");
        bytes.extend(compress(b"[2]"));
        let mut rest = &bytes[..];
        assert_eq!(decompress_frame(&mut rest, 100).unwrap(), b"{\"a\":1}");
        assert_eq!(decompress_frame(&mut rest, 100).unwrap(), b"[2]");
        assert!(rest.is_empty());
    }

    #[test]
    fn a_journal_reads_back_every_state() {
        let dir = TestDir::new("states");
        let path = dir.0.join("j.bljournal");
        let songs = states(600);
        let mut writer = JournalWriter::create(path.clone(), "s", Some(FOLLOWS)).unwrap();
        // One at a time, then in batches.
        for song in &songs[..10] {
            assert_eq!(writer.append(vec![pending(song, false)]).unwrap(), 1);
        }
        for chunk in songs[10..].chunks(7) {
            let batch = chunk.iter().map(|song| pending(song, false)).collect();
            assert_eq!(writer.append(batch).unwrap(), chunk.len());
        }
        // The same state again writes nothing.
        assert_eq!(writer.append(vec![pending(&songs[599], false)]).unwrap(), 0);

        let bytes = std::fs::read(&path).unwrap();
        let read = read_bytes(&bytes, &LoadOptions::default());
        assert!(!read.damaged);
        assert_eq!(read.entries.len(), 600);
        assert_eq!(read.uncommitted(), 600);
        assert_eq!(read.follows, Some(FOLLOWS));
        assert_eq!(decoded(read.last.as_ref().unwrap()), songs[599]);
        assert!(read.committed.is_none());
        // Every state on the way.
        for (index, song) in songs.iter().enumerate().step_by(37) {
            let read = read_up_to(&bytes, &LoadOptions::default(), Some(index));
            assert_eq!(&decoded(read.committed.as_ref().unwrap()), song, "{index}");
        }
        // Patches keep it small: an edit costs a few hundred bytes, and
        // snapshots add at most a quarter.
        let per_entry = bytes.len() / 600;
        assert!(per_entry < 600, "{per_entry} bytes per entry");
        println!(
            "600 entries in {} bytes, {per_entry} per entry",
            bytes.len()
        );
    }

    #[test]
    fn a_cut_or_damaged_tail_is_ignored() {
        let dir = TestDir::new("tail");
        let path = dir.0.join("j.bljournal");
        let songs = states(5);
        let mut writer = JournalWriter::create(path.clone(), "s", None).unwrap();
        for song in &songs {
            writer.append(vec![pending(song, false)]).unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        let whole = read_bytes(&bytes, &LoadOptions::default());
        // Cut anywhere: the whole entries before the cut read.
        let mut last_count = 0;
        for cut in 0..bytes.len() {
            let read = read_bytes(&bytes[..cut], &LoadOptions::default());
            assert!(read.entries.len() >= last_count);
            last_count = read.entries.len();
            assert_eq!(read.entries[..], whole.entries[..read.entries.len()]);
            if let Some(last) = &read.last {
                assert_eq!(decoded(last), songs[read.entries.len() - 1]);
            }
        }
        // A flipped byte loses that entry and what follows, nothing else.
        for at in (0..bytes.len()).step_by(3) {
            let mut damaged = bytes.clone();
            damaged[at] ^= 0x40;
            let read = read_bytes(&damaged, &LoadOptions::default());
            if let Some(last) = &read.last {
                assert_eq!(decoded(last), songs[read.entries.len() - 1], "byte {at}");
            }
        }
    }

    #[test]
    fn committed_entries_are_save_points() {
        // Spec: crash-recovery R9
        let dir = TestDir::new("commit");
        let set = dir.0.join("Song.blset");
        let journal = beside(&set, "abc-1").unwrap();
        assert_eq!(journal, dir.0.join(".Song.blset.abc-1.bljournal"));
        assert_eq!(session_of(&journal).as_deref(), Some("abc-1"));
        let songs = states(6);
        let mut writer = JournalWriter::create(journal.clone(), "abc-1", Some(FOLLOWS)).unwrap();
        writer.append(vec![pending(&songs[0], false)]).unwrap();
        writer.append(vec![pending(&songs[1], false)]).unwrap();
        // Turning Save Continuously on commits what's there.
        assert_eq!(
            writer
                .commit("Save Continuously", SystemTime::now())
                .unwrap(),
            1
        );
        assert_eq!(
            writer
                .commit("Save Continuously", SystemTime::now())
                .unwrap(),
            0
        );
        writer.append(vec![pending(&songs[2], true)]).unwrap();
        writer.append(vec![pending(&songs[3], true)]).unwrap();
        writer.append(vec![pending(&songs[4], false)]).unwrap();
        writer.append(vec![pending(&songs[5], false)]).unwrap();
        assert_eq!(writer.uncommitted(), 2);

        let read = read_file(&journal, &LoadOptions::default()).unwrap();
        assert_eq!(read.entries.len(), 7);
        assert_eq!(read.uncommitted(), 2);
        assert_eq!(decoded(read.committed.as_ref().unwrap()), songs[3]);

        assert_eq!(
            journals_beside(&set).unwrap(),
            std::slice::from_ref(&journal)
        );
        let points = save_points(&set, FOLLOWS);
        assert_eq!(points.len(), 3);
        assert_eq!(restore(&points[0]).unwrap(), songs[1]);
        assert_eq!(restore(&points[2]).unwrap(), songs[3]);
        assert_eq!(
            newest_save_point(&set, FOLLOWS, &LoadOptions::default()).map(|(song, _)| song),
            Some(songs[3].clone())
        );
        // Another revision has none.
        let other = Follows {
            revision: 4,
            ..FOLLOWS
        };
        assert!(save_points(&set, other).is_empty());
        assert_eq!(
            newest_save_point(&set, other, &LoadOptions::default()).map(|(song, _)| song),
            None
        );

        // Closing without saving keeps the save points and drops the rest.
        assert!(writer.discard_uncommitted().unwrap());
        let read = read_file(&journal, &LoadOptions::default()).unwrap();
        assert_eq!(read.entries.len(), 5);
        assert_eq!(read.uncommitted(), 0);
        assert!(!read.damaged);
    }

    #[test]
    fn foreign_and_out_of_order_entries_stop_the_read() {
        let dir = TestDir::new("foreign");
        let songs = states(2);
        let a = dir.0.join("a");
        let b = dir.0.join("b");
        JournalWriter::create(a.clone(), "one", None)
            .unwrap()
            .append(songs.iter().map(|s| pending(s, false)).collect())
            .unwrap();
        JournalWriter::create(b.clone(), "two", None)
            .unwrap()
            .append(songs.iter().map(|s| pending(s, false)).collect())
            .unwrap();
        // Two journals end to end: the second's entries start again at 1.
        let mut joined = std::fs::read(&a).unwrap();
        joined.extend(std::fs::read(&b).unwrap());
        let read = read_bytes(&joined, &LoadOptions::default());
        assert_eq!(read.entries.len(), 2);
        assert!(read.damaged);
        // A set file isn't a journal.
        let set = crate::format::encode(&songs[0], 1).unwrap();
        let read = read_bytes(&set, &LoadOptions::default());
        assert!(read.entries.is_empty() && read.damaged);
    }

    #[test]
    fn a_newer_versions_entries_are_recognised() {
        let dir = TestDir::new("newer");
        let path = dir.0.join("j");
        let songs = states(1);
        JournalWriter::create(path.clone(), "s", None)
            .unwrap()
            .append(vec![pending(&songs[0], false)])
            .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let newer = br#"{"format":"beatlayer-journal","version":2,"something":"new"}"#;
        bytes.extend(compress(newer));
        let read = read_bytes(&bytes, &LoadOptions::default());
        assert_eq!(read.entries.len(), 1);
        assert!(read.damaged && read.newer);
        // Damage isn't a newer version.
        let read = read_bytes(&bytes[..bytes.len() - 1], &LoadOptions::default());
        assert!(read.damaged && !read.newer);
    }

    #[test]
    fn fingerprints_tell_files_apart() {
        assert_eq!(fingerprint(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fingerprint(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(fingerprint(b"ab"), fingerprint(b"ba"));
    }
}
