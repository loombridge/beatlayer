//! Opening and saving sets: atomic replacement and backups.
//!
//! A save never writes over the set file. It writes the new set to a hidden
//! file beside it and flushes it to disk, copies the set it's about to
//! replace into `Backup/`, then renames the new file over the old one, which
//! the operating system does in one step. If the save stops at any point,
//! through an error, a crash, or a power cut, the set's path holds either the
//! old file or the new one, whole. A stopped save can leave a hidden
//! `.<name>.<process>-<n>.saving` file behind, which nothing reads and the
//! user can delete.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{LoadError, SaveError};
use crate::folder::{ProjectFolder, SET_EXTENSION};
use crate::format::{LoadOptions, Project, SAVED_BY, SetInfo, decode, encode};
use crate::journal::{self, Follows, fingerprint};
use crate::migrate::SCHEMA_VERSION;

/// Settings for saving a set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveOptions {
    /// How many replaced versions of each set to keep in `Backup/`. Zero
    /// turns backups off.
    pub backups: usize,
}

impl SaveOptions {
    /// The number of backups kept unless asked otherwise: 10.
    pub const DEFAULT_BACKUPS: usize = 10;
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self {
            backups: Self::DEFAULT_BACKUPS,
        }
    }
}

/// What a save did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveReport {
    /// What the saved file says about itself.
    pub info: SetInfo,
    /// Where the set this save replaced was backed up, if it was.
    pub backup: Option<PathBuf>,
    /// The old backups this save deleted to keep within the limit.
    pub removed_backups: Vec<PathBuf>,
    /// Things that went wrong without stopping the save, such as a backup
    /// that couldn't be written. The set itself was saved.
    pub warnings: Vec<String>,
}

/// A set file: where it is, and what its last load or save said about it.
///
/// ```no_run
/// use bl_project::{Project, ProjectFolder, SetFile};
///
/// let folder = ProjectFolder::named("Music".as_ref(), "My Song");
/// let mut set = SetFile::new(folder.set_path("My Song"));
/// set.save(&Project::new())?;
///
/// let (set, project) = SetFile::open(folder.root())?;
/// assert_eq!(set.revision(), 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetFile {
    path: PathBuf,
    info: Option<SetInfo>,
    /// The [`fingerprint`] of the file as last loaded or saved.
    fingerprint: Option<u64>,
}

impl SetFile {
    /// A set at `path` that hasn't been saved yet. Nothing on disk changes
    /// until [`SetFile::save`].
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            info: None,
            fingerprint: None,
        }
    }

    /// Opens a set file, or the only set in a project folder, at its newest
    /// save point: with Save Continuously's edits since the last save
    /// applied, if there are any (see [`crate::journal`]).
    ///
    /// # Errors
    ///
    /// Any [`LoadError`]: see [`load`].
    pub fn open(path: impl AsRef<Path>) -> Result<(Self, Project), LoadError> {
        Self::open_with(path, &LoadOptions::default())
    }

    /// [`SetFile::open`] with options.
    ///
    /// # Errors
    ///
    /// Any [`LoadError`]: see [`load`].
    pub fn open_with(
        path: impl AsRef<Path>,
        options: &LoadOptions,
    ) -> Result<(Self, Project), LoadError> {
        let path = find_set(path.as_ref())?;
        let (project, info, fingerprint) = load_file(&path, options)?;
        let set = Self {
            path,
            info: Some(info),
            fingerprint: Some(fingerprint),
        };
        Ok((set, project))
    }

    /// The set file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The project folder the set is in.
    #[must_use]
    pub fn folder(&self) -> ProjectFolder {
        ProjectFolder::of_set(&self.path)
    }

    /// What the file said about itself when it was last loaded or saved, or
    /// `None` if it hasn't been yet.
    #[must_use]
    pub fn info(&self) -> Option<&SetInfo> {
        self.info.as_ref()
    }

    /// The revision last loaded or saved: 0 for a set never saved.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.info.as_ref().map_or(0, |info| info.revision)
    }

    /// The [`fingerprint`] of the file as last loaded or saved, or `None` if
    /// it hasn't been yet.
    #[must_use]
    pub fn fingerprint(&self) -> Option<u64> {
        self.fingerprint
    }

    /// The saved revision a journal of this set's edits follows, or `None`
    /// for a set never saved.
    #[must_use]
    pub fn follows(&self) -> Option<Follows> {
        Some(Follows {
            revision: self.info.as_ref()?.revision,
            fingerprint: self.fingerprint?,
        })
    }

    /// The save points Save Continuously has kept since this revision was
    /// saved, oldest first (see [`journal::save_points`]).
    #[must_use]
    pub fn save_points(&self) -> Vec<journal::SavePoint> {
        self.follows()
            .map(|follows| journal::save_points(&self.path, follows))
            .unwrap_or_default()
    }

    /// Saves `project` as the next revision, keeping the last 10 replaced
    /// versions in `Backup/`.
    ///
    /// # Errors
    ///
    /// Any [`SaveError`]. A failed save leaves the file it would have
    /// replaced as it was.
    pub fn save(&mut self, project: &Project) -> Result<SaveReport, SaveError> {
        self.save_with(project, &SaveOptions::default())
    }

    /// [`SetFile::save`] with options.
    ///
    /// # Errors
    ///
    /// As [`SetFile::save`].
    pub fn save_with(
        &mut self,
        project: &Project,
        options: &SaveOptions,
    ) -> Result<SaveReport, SaveError> {
        let revision = self
            .revision()
            .checked_add(1)
            .ok_or(SaveError::RevisionsExhausted)?;
        let bytes = encode(project, revision)?;
        let written = write_set(&RealFs, &self.path, &bytes, options, SystemTime::now())?;
        let info = SetInfo {
            revision,
            saved_by: SAVED_BY.to_owned(),
            schema: SCHEMA_VERSION,
        };
        self.info = Some(info.clone());
        self.fingerprint = Some(fingerprint(&bytes));
        Ok(SaveReport {
            info,
            backup: written.backup,
            removed_backups: written.removed_backups,
            warnings: written.warnings,
        })
    }

    /// Saves `project` to a new path, which becomes this set's path. The
    /// revision count carries on from this set's.
    ///
    /// # Errors
    ///
    /// As [`SetFile::save`]. If the save fails, the set keeps its old path.
    pub fn save_as(
        &mut self,
        path: impl Into<PathBuf>,
        project: &Project,
    ) -> Result<SaveReport, SaveError> {
        let old = core::mem::replace(&mut self.path, path.into());
        let result = self.save(project);
        if result.is_err() {
            self.path = old;
        }
        result
    }
}

/// Loads a set file, or the only set in a project folder, at its newest
/// save point (as [`SetFile::open`] does).
///
/// # Errors
///
/// - [`LoadError::Io`] if the file can't be read.
/// - [`LoadError::NoSet`] or [`LoadError::SeveralSets`] if `path` is a
///   folder without exactly one set in it.
/// - Any error of [`decode`](crate::decode).
pub fn load(path: impl AsRef<Path>) -> Result<(Project, SetInfo), LoadError> {
    load_with(path, &LoadOptions::default())
}

/// [`load`] with options.
///
/// # Errors
///
/// As [`load`].
pub fn load_with(
    path: impl AsRef<Path>,
    options: &LoadOptions,
) -> Result<(Project, SetInfo), LoadError> {
    let path = find_set(path.as_ref())?;
    load_file(&path, options).map(|(project, info, _)| (project, info))
}

/// The set a path names: the path itself, or the only set in a folder.
fn find_set(path: &Path) -> Result<PathBuf, LoadError> {
    if !path.is_dir() {
        return Ok(path.to_owned());
    }
    let mut sets = ProjectFolder::new(path)
        .sets()
        .map_err(|source| LoadError::Io {
            path: path.to_owned(),
            source,
        })?;
    match sets.len() {
        0 => Err(LoadError::NoSet {
            folder: path.to_owned(),
        }),
        1 => Ok(sets.remove(0)),
        _ => Err(LoadError::SeveralSets { sets }),
    }
}

/// Loads a set file and applies its newest save point, returning the file's
/// fingerprint too.
fn load_file(path: &Path, options: &LoadOptions) -> Result<(Project, SetInfo, u64), LoadError> {
    let io = |source| LoadError::Io {
        path: path.to_owned(),
        source,
    };
    // zstd grows data it can't compress by at most a few bytes per 128 KiB
    // block, so a larger file can't hold JSON within the limit.
    let limit = u64::try_from(options.max_json_bytes).unwrap_or(u64::MAX);
    let most = limit.saturating_add(limit / 1024).saturating_add(1024);
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(io)?
        .take(most.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > most {
        return Err(LoadError::TooLarge {
            limit: options.max_json_bytes,
        });
    }
    let (project, info) = decode(&bytes, options)?;
    let fingerprint = fingerprint(&bytes);
    let follows = Follows {
        revision: info.revision,
        fingerprint,
    };
    let project = journal::newest_save_point(path, follows, options).unwrap_or(project);
    Ok((project, info, fingerprint))
}

/// The file-system operations a save makes, so tests can stop a save after
/// any one of them.
pub(crate) trait Fs {
    /// Creates a folder and its parents.
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    /// Writes a new file, failing if it exists, and flushes it to disk.
    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// Copies a file to a new file, failing if it exists, and flushes the
    /// copy to disk.
    fn copy_new(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Renames a file, replacing any file at `to`.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Flushes a folder's entries to disk, where the platform allows it.
    fn sync_dir(&self, path: &Path) -> io::Result<()>;
    /// Deletes a file.
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// Whether something exists at a path.
    fn exists(&self, path: &Path) -> io::Result<bool>;
    /// Whether a regular file exists at a path.
    fn is_file(&self, path: &Path) -> io::Result<bool>;
    /// The names of the entries in a folder that are valid Unicode.
    fn list(&self, dir: &Path) -> io::Result<Vec<String>>;
}

/// The real file system.
pub(crate) struct RealFs;

impl Fs for RealFs {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    fn copy_new(&self, from: &Path, to: &Path) -> io::Result<()> {
        let mut source = File::open(from)?;
        let mut copy = OpenOptions::new().write(true).create_new(true).open(to)?;
        io::copy(&mut source, &mut copy)?;
        copy.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        // On Windows a virus scanner or indexer can hold a file it has just
        // seen written for a moment, refusing the rename; try again briefly.
        let mut attempts = if cfg!(windows) { 20 } else { 1 };
        loop {
            match std::fs::rename(from, to) {
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied && attempts > 1 => {
                    attempts -= 1;
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                result => return result,
            }
        }
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        if cfg!(unix) {
            File::open(path)?.sync_all()
        } else {
            // Windows can't open a folder as a file to flush it; a rename
            // there is flushed with the volume's metadata.
            Ok(())
        }
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn exists(&self, path: &Path) -> io::Result<bool> {
        path.try_exists()
    }

    fn is_file(&self, path: &Path) -> io::Result<bool> {
        match std::fs::metadata(path) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            if let Ok(name) = entry?.file_name().into_string() {
                names.push(name);
            }
        }
        Ok(names)
    }
}

/// What [`write_set`] did besides saving.
#[derive(Debug, Default)]
pub(crate) struct Written {
    pub(crate) backup: Option<PathBuf>,
    pub(crate) removed_backups: Vec<PathBuf>,
    pub(crate) warnings: Vec<String>,
}

/// Saves a set's bytes to `path` atomically, backing up the file it
/// replaces.
pub(crate) fn write_set(
    fs: &dyn Fs,
    path: &Path,
    bytes: &[u8],
    options: &SaveOptions,
    now: SystemTime,
) -> Result<Written, SaveError> {
    let Some(file_name) = path.file_name() else {
        return Err(SaveError::NoFileName {
            path: path.to_owned(),
        });
    };
    let folder = ProjectFolder::of_set(path);
    fs.create_dir_all(folder.root())
        .map_err(SaveError::io(folder.root()))?;

    let temp = folder.root().join(temporary_name(file_name));
    if let Err(source) = fs.write_new(&temp, bytes) {
        // Best effort: the file may never have been created.
        let _ = fs.remove_file(&temp);
        return Err(SaveError::Io {
            path: path.to_owned(),
            source,
        });
    }

    let mut written = Written::default();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let backups = folder.backup();
    if options.backups > 0 {
        match fs.is_file(path) {
            Ok(false) => {}
            Ok(true) => match back_up(fs, path, &backups, &stem, now) {
                Ok(backup) => written.backup = Some(backup),
                Err(error) => written
                    .warnings
                    .push(format!("couldn't back up the replaced set: {error}")),
            },
            Err(error) => written
                .warnings
                .push(format!("couldn't check for a set to back up: {error}")),
        }
    }

    if let Err(source) = fs.rename(&temp, path) {
        let _ = fs.remove_file(&temp);
        return Err(SaveError::Io {
            path: path.to_owned(),
            source,
        });
    }
    if let Err(error) = fs.sync_dir(folder.root()) {
        written
            .warnings
            .push(format!("couldn't flush the project folder: {error}"));
    }

    if options.backups > 0 && written.backup.is_some() {
        prune_backups(fs, &backups, &stem, options.backups, &mut written);
    }
    Ok(written)
}

/// A hidden name beside `file_name` that no other save, in this process or
/// another, is using.
fn temporary_name(file_name: &std::ffi::OsStr) -> OsString {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(format!(".{}-{n}.saving", std::process::id()));
    name
}

/// Copies the set at `set` into the backup folder under a new name, through
/// a hidden file so a partial copy never has a backup's name.
fn back_up(
    fs: &dyn Fs,
    set: &Path,
    dir: &Path,
    stem: &str,
    now: SystemTime,
) -> io::Result<PathBuf> {
    fs.create_dir_all(dir)?;
    let stamp = utc_stamp(now);
    let mut n = 1;
    let (name, target) = loop {
        let name = backup_name(stem, &stamp, n);
        let target = dir.join(&name);
        if !fs.exists(&target)? {
            break (name, target);
        }
        if n == 9_999 {
            return Err(io::Error::other("too many backups in one second"));
        }
        n += 1;
    };
    let temp = dir.join(temporary_name(name.as_ref()));
    let copied = fs
        .copy_new(set, &temp)
        .and_then(|()| fs.rename(&temp, &target));
    if let Err(error) = copied {
        let _ = fs.remove_file(&temp);
        return Err(error);
    }
    Ok(target)
}

/// Deletes the oldest backups of the set `stem` beyond `keep`, never the one
/// just made.
fn prune_backups(fs: &dyn Fs, dir: &Path, stem: &str, keep: usize, written: &mut Written) {
    let names = match fs.list(dir) {
        Ok(names) => names,
        Err(error) => {
            written
                .warnings
                .push(format!("couldn't list the backups: {error}"));
            return;
        }
    };
    let newest = written.backup.as_deref();
    let mut backups: Vec<(BackupStamp, PathBuf)> = names
        .into_iter()
        .filter_map(|name| Some((parse_backup_name(stem, &name)?, dir.join(name))))
        .filter(|(_, path)| Some(path.as_path()) != newest)
        .collect();
    backups.sort();
    let keep_others = keep.saturating_sub(usize::from(newest.is_some()));
    let excess = backups.len().saturating_sub(keep_others);
    for (_, path) in backups.drain(..excess) {
        match fs.remove_file(&path) {
            Ok(()) => written.removed_backups.push(path),
            Err(error) => written.warnings.push(format!(
                "couldn't delete the old backup {}: {error}",
                path.display()
            )),
        }
    }
}

/// When a backup was made, from its name: the UTC date and time, and a
/// number counting backups made in the same second.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BackupStamp {
    stamp: String,
    n: u32,
}

/// A backup's file name: `<stem> [YYYY-MM-DD HHMMSS].blset`, with ` <n>`
/// after the time for the second and later backups in the same second.
pub(crate) fn backup_name(stem: &str, stamp: &str, n: u32) -> String {
    if n <= 1 {
        format!("{stem} [{stamp}].{SET_EXTENSION}")
    } else {
        format!("{stem} [{stamp} {n}].{SET_EXTENSION}")
    }
}

/// Reads a backup name made by [`backup_name`] for the set `stem`; `None`
/// for any other file, which pruning leaves alone.
pub(crate) fn parse_backup_name(stem: &str, name: &str) -> Option<BackupStamp> {
    let inner = name
        .strip_suffix(SET_EXTENSION)?
        .strip_suffix("].")?
        .strip_prefix(stem)?
        .strip_prefix(" [")?;
    let mut parts = inner.split(' ');
    let (date, time, n) = (parts.next()?, parts.next()?, parts.next());
    if parts.next().is_some() {
        return None;
    }
    let digits =
        |text: &str, len: usize| text.len() == len && text.bytes().all(|b| b.is_ascii_digit());
    let date_ok = date.len() == 10
        && date.split('-').map(str::len).eq([4, 2, 2])
        && date.split('-').all(|part| digits(part, part.len()));
    if !date_ok || !digits(time, 6) {
        return None;
    }
    let n = match n {
        None => 1,
        Some(n) if digits(n, n.len()) && !n.starts_with('0') => {
            n.parse().ok().filter(|&n| n >= 2)?
        }
        Some(_) => return None,
    };
    Some(BackupStamp {
        stamp: format!("{date} {time}"),
        n,
    })
}

/// A time as `YYYY-MM-DD HHMMSS` in UTC. Times before 1970 count as 1970.
pub(crate) fn utc_stamp(time: SystemTime) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX / 2);
    let (year, month, day) = civil_from_days(days);
    let second = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}{:02}{:02}",
        second / 3_600,
        second / 60 % 60,
        second % 60
    )
}

/// A time in RFC 3339 form in UTC, to the second: `YYYY-MM-DDTHH:MM:SSZ`.
/// Times before 1970 count as 1970.
pub(crate) fn utc_rfc3339(time: SystemTime) -> String {
    let stamp = utc_stamp(time);
    let (date, clock) = stamp.split_at(10);
    let clock = clock.trim_start();
    format!("{date}T{}:{}:{}Z", &clock[..2], &clock[2..4], &clock[4..6])
}

/// The proleptic Gregorian date `days` after 1 January 1970, by Howard
/// Hinnant's `civil_from_days` algorithm (public domain).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    // Both are small and positive by construction.
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::time::Duration;

    use bl_model::{Command, TrackKind};

    use super::*;
    use crate::format::encode;
    use crate::media::Media;

    /// A folder under the system's temporary folder, deleted when dropped.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("bl-project-{label}-{}-{n}", std::process::id()));
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

    /// A song whose one track is named after the revision, so each revision
    /// is a different song.
    fn song(revision: u64) -> Project {
        let mut editor = bl_model::Editor::new();
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind: TrackKind::Midi,
                name: format!("Revision {revision}"),
                parent: None,
                index: 0,
            })
            .unwrap();
        Project::from_editor(&editor, Media::new())
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    /// 10 October 2026, 14:30:12 UTC.
    const NOW: u64 = 1_791_642_612;

    fn backups_in(dir: &Path, stem: &str) -> Vec<String> {
        let mut names: Vec<String> = RealFs
            .list(dir)
            .unwrap()
            .into_iter()
            .filter(|name| parse_backup_name(stem, name).is_some())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn utc_stamps() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "1970-01-01 000000");
        assert_eq!(utc_stamp(at(NOW)), "2026-10-10 143012");
        assert_eq!(utc_rfc3339(at(NOW)), "2026-10-10T14:30:12Z");
        assert_eq!(utc_rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(utc_stamp(at(951_782_400)), "2000-02-29 000000");
        assert_eq!(utc_stamp(at(951_868_799)), "2000-02-29 235959");
        assert_eq!(utc_stamp(at(4_102_444_800)), "2100-01-01 000000");
        assert_eq!(
            utc_stamp(UNIX_EPOCH - Duration::from_secs(5)),
            "1970-01-01 000000"
        );
        // Every day for 400 years follows the one before.
        let mut previous = civil_from_days(-1);
        for days in 0..146_097 {
            let date = civil_from_days(days);
            let next_day = (previous.0, previous.1, previous.2 + 1);
            let next_month = (previous.0, previous.1 + 1, 1);
            let next_year = (previous.0 + 1, 1, 1);
            assert!(
                date == next_day || date == next_month || date == next_year,
                "{previous:?} then {date:?}"
            );
            previous = date;
        }
    }

    #[test]
    fn backup_names() {
        assert_eq!(
            backup_name("My Song", "2026-10-10 143012", 1),
            "My Song [2026-10-10 143012].blset"
        );
        assert_eq!(
            backup_name("My Song", "2026-10-10 143012", 2),
            "My Song [2026-10-10 143012 2].blset"
        );
        let parse = |name| parse_backup_name("My Song", name);
        assert!(
            parse("My Song [2026-10-10 143012 2].blset")
                > parse("My Song [2026-10-10 143012].blset")
        );
        assert!(
            parse("My Song [2026-10-10 143012 10].blset")
                > parse("My Song [2026-10-10 143012 9].blset")
        );
        assert!(
            parse("My Song [2026-10-10 143013].blset")
                > parse("My Song [2026-10-10 143012 9].blset")
        );
        let stamp = parse_backup_name("Song [demo]", "Song [demo] [2026-10-10 143012].blset");
        assert!(stamp.is_some());
        for other in [
            "My Song.blset",
            "My Song [draft].blset",
            "My Song [2026-10-10 143012].blset.tmp",
            "My Songs [2026-10-10 143012].blset",
            "Other [2026-10-10 143012].blset",
            "My Song [2026-10-10 1430].blset",
            "My Song [2026-1-10 143012].blset",
            "My Song [2026-10-10 143012 1].blset",
            "My Song [2026-10-10 143012 02].blset",
            "My Song [2026-10-10 143012 x].blset",
            "My Song [2026-10-10 143012 2 3].blset",
            ".My Song [2026-10-10 143012].blset.4-0.saving",
        ] {
            assert_eq!(parse(other), None, "{other}");
        }
    }

    #[test]
    fn saves_back_up_what_they_replace_and_keep_ten() {
        let dir = TestDir::new("backups");
        let path = dir.0.join("Song.blset");
        let backup_dir = dir.0.join("Backup");
        // Files that aren't this set's backups are never touched.
        std::fs::create_dir_all(&backup_dir).unwrap();
        let others = [
            "Notes.txt",
            "Song [draft].blset",
            "Other [2026-01-01 000000].blset",
        ];
        for name in others {
            std::fs::write(backup_dir.join(name), "keep me").unwrap();
        }

        let options = SaveOptions::default();
        for revision in 1..=15 {
            // Several saves in the same second.
            let now = at(NOW + revision / 3);
            let bytes = encode(&song(revision), revision).unwrap();
            let written = write_set(&RealFs, &path, &bytes, &options, now).unwrap();
            assert_eq!(written.warnings, Vec::<String>::new());
            assert_eq!(written.backup.is_some(), revision > 1);
            let (_, info) = load(&path).unwrap();
            assert_eq!(info.revision, revision);
        }
        let backups = backups_in(&backup_dir, "Song");
        assert_eq!(backups.len(), 10, "{backups:?}");
        // They're the ten revisions before the last.
        let mut revisions: Vec<u64> = backups
            .iter()
            .map(|name| load(backup_dir.join(name)).unwrap().1.revision)
            .collect();
        revisions.sort_unstable();
        assert_eq!(revisions, (5..=14).collect::<Vec<_>>());
        for name in others {
            assert_eq!(
                std::fs::read_to_string(backup_dir.join(name)).unwrap(),
                "keep me"
            );
        }
        // No temporary files are left.
        let hidden: Vec<String> = RealFs
            .list(&dir.0)
            .unwrap()
            .into_iter()
            .chain(RealFs.list(&backup_dir).unwrap())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert_eq!(hidden, Vec::<String>::new());

        // Backups can be turned off.
        let none = SaveOptions { backups: 0 };
        let bytes = encode(&song(16), 16).unwrap();
        let written = write_set(&RealFs, &path, &bytes, &none, at(NOW + 99)).unwrap();
        assert_eq!(written.backup, None);
        assert_eq!(backups_in(&backup_dir, "Song").len(), 10);
    }

    #[test]
    fn a_failed_backup_doesnt_stop_the_save() {
        let dir = TestDir::new("backup-fails");
        let path = dir.0.join("Song.blset");
        let options = SaveOptions::default();
        write_set(
            &RealFs,
            &path,
            &encode(&song(1), 1).unwrap(),
            &options,
            at(NOW),
        )
        .unwrap();
        // A file where the backup folder should be.
        std::fs::write(dir.0.join("Backup"), "not a folder").unwrap();
        let written = write_set(
            &RealFs,
            &path,
            &encode(&song(2), 2).unwrap(),
            &options,
            at(NOW),
        )
        .unwrap();
        assert_eq!(written.backup, None);
        assert_eq!(written.warnings.len(), 1, "{:?}", written.warnings);
        assert!(written.warnings[0].contains("couldn't back up"));
        assert_eq!(load(&path).unwrap().1.revision, 2);
    }

    #[test]
    fn a_failed_save_leaves_the_old_set() {
        let dir = TestDir::new("save-fails");
        let path = dir.0.join("Song.blset");
        let mut set = SetFile::new(&path);
        set.save(&song(1)).unwrap();
        // A folder where a file should be: the rename fails.
        let blocked = dir.0.join("Blocked.blset");
        std::fs::create_dir_all(blocked.join("inside")).unwrap();
        let error = set.save_as(&blocked, &song(2)).unwrap_err();
        assert!(matches!(error, SaveError::Io { .. }), "{error}");
        assert_eq!(set.path(), path);
        assert_eq!(set.revision(), 1);
        assert_eq!(load(&path).unwrap().0, song(1));
        let hidden = RealFs
            .list(&dir.0)
            .unwrap()
            .into_iter()
            .filter(|name| name.starts_with('.'))
            .count();
        assert_eq!(hidden, 0);
    }

    #[test]
    fn opening_a_folder_opens_its_one_set() {
        let dir = TestDir::new("open-folder");
        let folder = ProjectFolder::named(&dir.0, "Song");
        assert!(matches!(
            SetFile::open(&dir.0),
            Err(LoadError::NoSet { .. })
        ));
        let mut set = SetFile::new(folder.set_path("Song"));
        set.save(&song(1)).unwrap();
        // A first save has nothing to back up, so it makes no Backup folder.
        assert!(folder.root().is_dir());
        assert!(!folder.backup().exists());
        let (opened, project) = SetFile::open(folder.root()).unwrap();
        assert_eq!(opened.path(), folder.set_path("Song"));
        assert_eq!(opened.revision(), 1);
        assert_eq!(opened.folder(), folder);
        assert_eq!(project, song(1));

        set.save_as(folder.set_path("Song 2"), &song(2)).unwrap();
        assert_eq!(set.revision(), 2);
        match SetFile::open(folder.root()) {
            Err(LoadError::SeveralSets { sets }) => {
                assert_eq!(sets, [folder.set_path("Song 2"), folder.set_path("Song")]);
            }
            other => panic!("{other:?}"),
        }
        let missing = folder.set_path("Missing");
        assert!(matches!(
            load(&missing),
            Err(LoadError::Io { path, .. }) if path == missing
        ));
    }

    /// The real file system until a set number of changes, then a crash:
    /// the change in progress is left half done and nothing changes after
    /// it, as when the process is killed or the power goes.
    struct CrashFs {
        changes_left: Cell<usize>,
        crashed: Cell<bool>,
        log: RefCell<Vec<String>>,
    }

    impl CrashFs {
        fn after(changes: usize) -> Self {
            Self {
                changes_left: Cell::new(changes),
                crashed: Cell::new(false),
                log: RefCell::new(Vec::new()),
            }
        }

        /// Whether this change happens (`Ok(true)`), happens halfway and
        /// crashes (`Ok(false)`), or can't happen because of an earlier crash.
        fn change(&self, what: String) -> io::Result<bool> {
            if self.crashed.get() {
                return Err(io::Error::other("crashed earlier"));
            }
            self.log.borrow_mut().push(what);
            match self.changes_left.get() {
                0 => {
                    self.crashed.set(true);
                    Ok(false)
                }
                n => {
                    self.changes_left.set(n - 1);
                    Ok(true)
                }
            }
        }
    }

    fn crash() -> io::Error {
        io::Error::other("crashed")
    }

    fn name(path: &Path) -> String {
        path.file_name().unwrap().to_string_lossy().into_owned()
    }

    impl Fs for CrashFs {
        fn create_dir_all(&self, path: &Path) -> io::Result<()> {
            if self.change(format!("create {}", name(path)))? {
                RealFs.create_dir_all(path)
            } else {
                Err(crash())
            }
        }

        fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            if self.change(format!("write {}", name(path)))? {
                RealFs.write_new(path, bytes)
            } else {
                // Half the bytes reach the file.
                std::fs::write(path, &bytes[..bytes.len() / 2])?;
                Err(crash())
            }
        }

        fn copy_new(&self, from: &Path, to: &Path) -> io::Result<()> {
            if self.change(format!("copy {} to {}", name(from), name(to)))? {
                RealFs.copy_new(from, to)
            } else {
                let bytes = std::fs::read(from)?;
                std::fs::write(to, &bytes[..bytes.len() / 2])?;
                Err(crash())
            }
        }

        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            if self.change(format!("rename {} to {}", name(from), name(to)))? {
                RealFs.rename(from, to)
            } else {
                Err(crash())
            }
        }

        fn sync_dir(&self, path: &Path) -> io::Result<()> {
            if self.change(format!("flush {}", name(path)))? {
                RealFs.sync_dir(path)
            } else {
                Err(crash())
            }
        }

        fn remove_file(&self, path: &Path) -> io::Result<()> {
            if self.change(format!("delete {}", name(path)))? {
                RealFs.remove_file(path)
            } else {
                Err(crash())
            }
        }

        fn exists(&self, path: &Path) -> io::Result<bool> {
            if self.crashed.get() {
                return Err(crash());
            }
            RealFs.exists(path)
        }

        fn is_file(&self, path: &Path) -> io::Result<bool> {
            if self.crashed.get() {
                return Err(crash());
            }
            RealFs.is_file(path)
        }

        fn list(&self, dir: &Path) -> io::Result<Vec<String>> {
            if self.crashed.get() {
                return Err(crash());
            }
            RealFs.list(dir)
        }
    }

    /// The acceptance test for interrupted saves, step by step: a save
    /// stopped after each of its changes to the disk, including halfway
    /// through each write and copy, leaves the set loadable as the old
    /// revision or the new one, every backup loadable, and the folder ready
    /// for the next save. `tests/interrupted_save.rs` kills real processes.
    #[test]
    fn a_save_interrupted_at_any_step_leaves_a_readable_set() {
        let options = SaveOptions::default();
        let mut outcomes = Vec::new();
        for crash_after in 0.. {
            let dir = TestDir::new("crash");
            let path = dir.0.join("Song.blset");
            let backup_dir = dir.0.join("Backup");
            // Revision 11 saved, with backups of revisions 1 to 10.
            std::fs::create_dir_all(&backup_dir).unwrap();
            for revision in 1..=10 {
                let name = backup_name("Song", &utc_stamp(at(NOW - 100 + revision)), 1);
                std::fs::write(
                    backup_dir.join(name),
                    encode(&song(revision), revision).unwrap(),
                )
                .unwrap();
            }
            std::fs::write(&path, encode(&song(11), 11).unwrap()).unwrap();

            let fs = CrashFs::after(crash_after);
            let bytes = encode(&song(12), 12).unwrap();
            let result = write_set(&fs, &path, &bytes, &options, at(NOW));

            // The set loads as the old revision or the new one.
            let (project, info) = load(&path).unwrap();
            assert!(
                info.revision == 11 || info.revision == 12,
                "revision {}",
                info.revision
            );
            assert_eq!(project, song(info.revision));
            // Every backup loads, and there are never more than 11.
            let backups = backups_in(&backup_dir, "Song");
            assert!((10..=11).contains(&backups.len()), "{backups:?}");
            for backup in &backups {
                load(backup_dir.join(backup)).unwrap();
            }
            // The next save works, whatever the crash left behind.
            let bytes = encode(&song(13), 13).unwrap();
            let written = write_set(&RealFs, &path, &bytes, &options, at(NOW)).unwrap();
            assert_eq!(written.warnings, Vec::<String>::new());
            assert_eq!(load(&path).unwrap().1.revision, 13);
            assert_eq!(backups_in(&backup_dir, "Song").len(), 10);

            let steps = fs.log.borrow().clone();
            if !fs.crashed.get() {
                // Nothing stopped this one: it's a whole save.
                result.unwrap();
                println!("a save makes {} changes: {steps:#?}", steps.len());
                break;
            }
            outcomes.push((info.revision, steps.last().cloned()));
        }
        // Stopping before the rename keeps revision 11; after it, 12.
        let switch = outcomes
            .iter()
            .position(|(revision, _)| *revision == 12)
            .unwrap();
        assert!(outcomes[..switch].iter().all(|(r, _)| *r == 11));
        assert!(outcomes[switch..].iter().all(|(r, _)| *r == 12));
        let renamed = outcomes[switch - 1].1.as_deref().unwrap();
        assert!(renamed.starts_with("rename .Song.blset."), "{renamed}");
        for (crash_after, (revision, step)) in outcomes.iter().enumerate() {
            println!("crash at change {crash_after} ({step:?}): loads revision {revision}");
        }
    }
}
