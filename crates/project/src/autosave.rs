//! Crash recovery and Save Continuously: the service the app calls
//! (WP-1.11, IMP-05).
//!
//! Crash recovery is always on. While a set is open, [`Autosave`] journals
//! every finished edit within a second, on a thread of its own
//! (`bl-autosave`), never the audio thread or the caller's. If the app
//! stops without closing the set (a crash, a kill, a power cut), the next
//! [`Autosave::start`] finds the journal and offers it: [`Autosave::offers`]
//! lists what can be recovered, [`Autosave::recover`] brings a set back
//! exactly as it was after its last journalled edit, and
//! [`Autosave::discard`] declines.
//!
//! With Save Continuously on ([`Autosave::set_continuous`], off by
//! default), the same entries are save points: the set opens at its newest
//! one with no question asked, and they form a history of the set since its
//! last normal save ([`SetFile::save_points`]). A normal save still writes
//! the next revision of the set file and retires the journal.
//!
//! # The recovery folder
//!
//! Each session (one run of the app with one set open) registers in a
//! recovery folder, by default in the user's application data
//! ([`default_recovery_dir`]):
//!
//! ```text
//! Recovery/
//!   <session>.lock        locked by the running session; the OS lets go when it dies
//!   <session>.session     where its set and journal are (JSON)
//!   <session>.bljournal   the journal of a set that has never been saved
//!   Discarded/            declined journals, deleted after seven days
//! ```
//!
//! A session's journal is beside its set (see [`crate::journal`]), or in
//! the recovery folder for a set that has never been saved. A session whose
//! lock can be taken has ended; if it didn't close its set, and its journal
//! holds edits after its last save point, it's offered. Another running
//! instance's sessions are locked, so they're never offered, and each
//! instance's recovery works on its own.
//!
//! # What the app does
//!
//! ```no_run
//! use bl_project::autosave::{Autosave, default_recovery_dir};
//! use bl_project::{Project, SetFile};
//!
//! // At launch: find what a crash left, and offer it.
//! let mut autosave = Autosave::start(default_recovery_dir().unwrap())?;
//! for offer in autosave.offers() {
//!     println!("Recover {} ({} edits)?", offer.name(), offer.edits);
//! }
//! // The user says yes to the first: open what comes back, unsaved.
//! let recovered = autosave.recover(None)?;
//! let mut editor = recovered.project.editor();
//! let mut set = recovered.set.unwrap_or_else(|| SetFile::new("Untitled.blset"));
//!
//! // After each finished edit (a command, a gesture's transaction, undo, redo):
//! autosave.edited(&Project::from_editor(&editor, recovered.project.media.clone()), "Move Clip");
//!
//! // After a save:
//! set.save(&Project::from_editor(&editor, recovered.project.media.clone()))?;
//! autosave.saved(&set);
//!
//! // When the set closes (saved, or the user chose not to save), and before
//! // opening another with `begin`:
//! autosave.close_set()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Dropping an [`Autosave`] without [`Autosave::close_set`] leaves the
//! session to be offered at the next launch, which is what a crash does,
//! and what an app quitting with unsaved changes it hasn't asked about
//! should do.

use core::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::LoadError;
use crate::format::{LoadOptions, Project, decode_value, to_value};
use crate::journal::{
    self, Follows, JOURNAL_EXTENSION, JournalWriter, Pending, discard_uncommitted_file, fingerprint,
};
use crate::save::{SetFile, utc_stamp};

/// The extension of a session's lock file in the recovery folder.
const LOCK_EXTENSION: &str = "lock";
/// The extension of a session's index in the recovery folder.
const SESSION_EXTENSION: &str = "session";
/// The `format` field of a session's index.
const SESSION_FORMAT: &str = "beatlayer-session";
/// The folder in the recovery folder that keeps declined journals.
const DISCARDED: &str = "Discarded";
/// How long declined journals are kept.
const KEEP_DISCARDED: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// The name of the writer thread.
const THREAD_NAME: &str = "bl-autosave";

/// Where recovery data goes unless the app says otherwise: in the user's
/// application data, `~/Library/Application Support/Beatlayer/Recovery` on
/// macOS, `%LOCALAPPDATA%\Beatlayer\Recovery` on Windows, and
/// `$XDG_STATE_HOME/beatlayer/recovery` (by default
/// `~/.local/state/beatlayer/recovery`) elsewhere. `None` if the
/// environment doesn't say where the user's folders are.
#[must_use]
pub fn default_recovery_dir() -> Option<PathBuf> {
    let var = |name| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(target_os = "macos") {
        Some(var("HOME")?.join("Library/Application Support/Beatlayer/Recovery"))
    } else if cfg!(windows) {
        Some(var("LOCALAPPDATA")?.join("Beatlayer").join("Recovery"))
    } else {
        let state = var("XDG_STATE_HOME").or_else(|| Some(var("HOME")?.join(".local/state")))?;
        Some(state.join("beatlayer").join("recovery"))
    }
}

/// A set a session left unsaved when it ended without closing, which can be
/// recovered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recoverable {
    /// The session's ID, which [`Autosave::recover`] and
    /// [`Autosave::discard`] take.
    pub id: String,
    /// The set's file, or `None` for a set that had never been saved.
    pub set: Option<PathBuf>,
    /// How many journal entries follow the set's last save: edits, or runs
    /// of edits written together.
    pub edits: usize,
    /// When the last of them was journalled.
    pub last_edit: SystemTime,
    /// The last edit's label, as the undo history named it.
    pub last_label: String,
    /// Whether the set file has been saved since the session's journal
    /// began, by another session or program, so recovering and saving would
    /// replace that save.
    pub saved_since: bool,
}

impl Recoverable {
    /// The set's name, as the title bar shows it: its file name without the
    /// extension, or "Untitled".
    #[must_use]
    pub fn name(&self) -> String {
        set_name(self.set.as_deref())
    }
}

pub(crate) fn set_name(set: Option<&Path>) -> String {
    set.and_then(Path::file_stem).map_or_else(
        || "Untitled".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

/// A recovered set, ready to open: unsaved, at its file if it had one,
/// with an empty undo history.
#[derive(Debug)]
pub struct Recovered {
    /// The session it came from.
    pub id: String,
    /// The set's file, at the revision now on disk, or `None` for a set
    /// that had never been saved. Saving writes the recovered song as the
    /// next revision.
    pub set: Option<SetFile>,
    /// The song as it was after the last journalled edit.
    pub project: Project,
    /// How many journal entries it recovered.
    pub edits: usize,
}

/// What the autosave service is doing, for the app's status display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutosaveStatus {
    /// The open set's session, if a set is open.
    pub session: Option<String>,
    /// Edits reported with [`Autosave::edited`] since the session began.
    pub edits: u64,
    /// How many of them are on disk.
    pub journalled: u64,
    /// Whether Save Continuously is on.
    pub continuous: bool,
    /// The last problem writing the journal, until a write succeeds.
    pub error: Option<String>,
}

/// Why the autosave service couldn't do something.
#[derive(Debug)]
#[non_exhaustive]
pub enum AutosaveError {
    /// The recovery folder or a journal couldn't be read or written.
    Io {
        /// The file or folder.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// No set is waiting to be recovered with this ID, or none at all.
    NoOffer {
        /// The ID asked for, if one was.
        id: Option<String>,
    },
    /// The recovered song didn't load.
    Load(LoadError),
    /// No set is open, so there's no session to act on.
    NoSession,
    /// The writer thread reported a problem.
    Journal {
        /// What went wrong.
        reason: String,
    },
}

impl fmt::Display for AutosaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::NoOffer { id: Some(id) } => {
                write!(f, "no set from session {id} can be recovered")
            }
            Self::NoOffer { id: None } => f.write_str("no set can be recovered"),
            Self::Load(error) => write!(f, "the recovered set doesn't load: {error}"),
            Self::NoSession => f.write_str("no set is open"),
            Self::Journal { reason } => write!(f, "couldn't write the journal: {reason}"),
        }
    }
}

impl std::error::Error for AutosaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Load(error) => Some(error),
            _ => None,
        }
    }
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> AutosaveError + '_ {
    move |source| AutosaveError::Io {
        path: path.to_owned(),
        source,
    }
}

/// A session's index in the recovery folder.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionIndex {
    format: String,
    version: u32,
    /// The process that ran it.
    pid: u32,
    /// When it began, in milliseconds since 1970.
    started: u64,
    /// The set's file, if it has one.
    set: Option<PathBuf>,
    /// Where its journal is, or will be once it has an entry.
    journal: PathBuf,
}

/// A set waiting to be recovered, with the lock that keeps another instance
/// from offering it too.
#[derive(Debug)]
struct Offer {
    info: Recoverable,
    lock: File,
    journal: PathBuf,
}

/// Crash recovery and Save Continuously for the app's open set. See the
/// [module documentation](self).
#[derive(Debug)]
pub struct Autosave {
    dir: PathBuf,
    continuous: bool,
    offers: Vec<Offer>,
    session: Option<Session>,
    /// The set the last session began with, so a session that couldn't
    /// start can be started again (the crash-recovery spec, R27).
    set: Option<SetFile>,
}

impl Autosave {
    /// Starts the service on a recovery folder, creating it if need be, and
    /// finds the sets that sessions which ended without closing left
    /// unsaved ([`Autosave::offers`]). Declined journals older than seven
    /// days are deleted. Save Continuously starts off.
    ///
    /// # Errors
    ///
    /// [`AutosaveError::Io`] if the folder can't be created or read.
    pub fn start(dir: impl Into<PathBuf>) -> Result<Self, AutosaveError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(io_error(&dir))?;
        prune_discarded(&dir.join(DISCARDED), SystemTime::now());
        let mut autosave = Self {
            dir,
            continuous: false,
            offers: Vec::new(),
            session: None,
            set: None,
        };
        autosave.scan()?;
        Ok(autosave)
    }

    /// The recovery folder.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The sets that can be recovered, newest first.
    #[must_use]
    pub fn offers(&self) -> Vec<Recoverable> {
        self.offers.iter().map(|offer| offer.info.clone()).collect()
    }

    /// Looks again for sessions that have ended without closing, adding
    /// what they left to [`Autosave::offers`], and tidies away those that
    /// left nothing.
    ///
    /// # Errors
    ///
    /// [`AutosaveError::Io`] if the recovery folder can't be read.
    pub fn scan(&mut self) -> Result<(), AutosaveError> {
        let own = self.session.as_ref().map(|session| session.id.clone());
        for id in session_ids(&self.dir).map_err(io_error(&self.dir))? {
            if own.as_ref() == Some(&id) || self.offers.iter().any(|offer| offer.info.id == id) {
                continue;
            }
            if let Some(offer) = self.examine(&id) {
                self.offers.push(offer);
            }
        }
        self.offers
            .sort_by_key(|offer| std::cmp::Reverse(offer.info.last_edit));
        Ok(())
    }

    /// Looks at one session: `None` if it's running, or has nothing to
    /// recover (it's tidied away then), or can't be read yet.
    fn examine(&self, id: &str) -> Option<Offer> {
        let lock_path = self.dir.join(format!("{id}.{LOCK_EXTENSION}"));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .ok()?;
        match lock.try_lock() {
            Ok(()) => {}
            // Running, here or in another instance.
            Err(TryLockError::WouldBlock | TryLockError::Error(_)) => return None,
        }
        let index_path = self.dir.join(format!("{id}.{SESSION_EXTENSION}"));
        let index = match read_index(&index_path) {
            Index::Read(index) => index,
            // A newer version of Beatlayer's: leave it to that version.
            Index::Newer => return None,
            // Stopped before it said where its journal would be.
            Index::Missing => {
                forget_session(&self.dir, id, lock);
                return None;
            }
        };
        let options = LoadOptions::default();
        let read = match journal::read_file(&index.journal, &options) {
            // Written by a newer version of Beatlayer: leave it to that one.
            Ok(read) if read.newer => return None,
            Ok(read) => read,
            // Unreadable for now, such as on a drive that isn't connected:
            // keep it for a later launch.
            Err(_) => return None,
        };
        if read.entries.is_empty() && !index.journal.parent().is_some_and(Path::is_dir) {
            // The journal's folder is missing: a drive that isn't
            // connected, or a project moved away. Keep it for later.
            return None;
        }
        if read.uncommitted() == 0 {
            // Nothing to recover. A journal with save points is part of
            // its set; any other is empty, and goes, if it's named as this
            // session's journal: an index that names some other file
            // (damaged, or not written by Beatlayer) never deletes it.
            if read.committed_len == 0 && journal::session_of(&index.journal).as_deref() == Some(id)
            {
                let _ = std::fs::remove_file(&index.journal);
            }
            forget_session(&self.dir, id, lock);
            return None;
        }
        let last = read.entries.last()?;
        let saved_since = index.set.as_deref().is_some_and(|set| {
            match (
                std::fs::read(set).ok().map(|bytes| fingerprint(&bytes)),
                read.follows,
            ) {
                (Some(now), Some(then)) => now != then.fingerprint,
                (Some(_), None) => true,
                (None, _) => false,
            }
        });
        Some(Offer {
            info: Recoverable {
                id: id.to_owned(),
                set: index.set.clone(),
                edits: read.uncommitted(),
                last_edit: last.time,
                last_label: last.label.clone(),
                saved_since,
            },
            lock,
            journal: index.journal,
        })
    }

    fn take_offer(&mut self, id: Option<&str>) -> Result<Offer, AutosaveError> {
        let at = match id {
            Some(id) => self.offers.iter().position(|offer| offer.info.id == id),
            None => (!self.offers.is_empty()).then_some(0),
        };
        at.map(|at| self.offers.remove(at))
            .ok_or_else(|| AutosaveError::NoOffer {
                id: id.map(str::to_owned),
            })
    }

    /// Recovers a set: the one from session `id`, or the newest. It comes
    /// back as it was after its last journalled edit, and becomes the open
    /// set: a new session journals it straight away, so a second crash
    /// doesn't lose it, and the old session's journal and files are
    /// tidied away. The set that was open stays recoverable if it had
    /// unsaved edits (see [`Autosave::begin`]).
    ///
    /// # Errors
    ///
    /// [`AutosaveError::NoOffer`] if nothing with that ID can be recovered,
    /// [`AutosaveError::Io`] or [`AutosaveError::Load`] if the journal
    /// can't be read back (the set stays offered), and any error of
    /// [`Autosave::begin`].
    pub fn recover(&mut self, id: Option<&str>) -> Result<Recovered, AutosaveError> {
        let offer = self.take_offer(id)?;
        let options = LoadOptions::default();
        let loaded = journal::read_file(&offer.journal, &options)
            .map_err(io_error(&offer.journal))
            .and_then(|read| {
                let value = read.last.as_ref().ok_or(AutosaveError::NoOffer {
                    id: Some(offer.info.id.clone()),
                })?;
                let (project, _) = decode_value(value, &options).map_err(AutosaveError::Load)?;
                Ok((project, read))
            });
        let (project, read) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                self.offers.insert(0, offer);
                return Err(error);
            }
        };
        let set =
            offer.info.set.as_ref().map(|path| {
                SetFile::open(path).map_or_else(|_| SetFile::new(path), |(set, _)| set)
            });
        if let Err(error) = self.begin(set.as_ref()) {
            self.offers.insert(0, offer);
            return Err(error);
        }
        self.edited(&project, "Recover");
        // Only once the recovered song is journalled again does the old
        // journal go; if that fails, the old session stays for next time.
        if self.flush().is_ok() {
            let _ = discard_uncommitted_file(&offer.journal, &read);
            forget_session(&self.dir, &offer.info.id, offer.lock);
        }
        Ok(Recovered {
            id: offer.info.id,
            set,
            project,
            edits: offer.info.edits,
        })
    }

    /// Declines to recover a set: the one from session `id`, or the newest.
    /// Its journal is copied into the recovery folder's `Discarded` folder,
    /// which keeps it for seven days, and its unsaved edits are removed
    /// from beside the set. Returns where the copy is.
    ///
    /// # Errors
    ///
    /// [`AutosaveError::NoOffer`] if nothing with that ID can be recovered,
    /// and [`AutosaveError::Io`] if the journal can't be copied (nothing is
    /// removed then, and the set stays offered).
    pub fn discard(&mut self, id: Option<&str>) -> Result<PathBuf, AutosaveError> {
        let offer = self.take_offer(id)?;
        let discarded = self.dir.join(DISCARDED);
        let name = format!(
            "{} [{}] {}.{JOURNAL_EXTENSION}",
            offer.info.name(),
            utc_stamp(SystemTime::now()),
            offer.info.id
        );
        let copy = discarded.join(name);
        let copied = std::fs::create_dir_all(&discarded)
            // Not `std::fs::copy`: on macOS it keeps the source's modified
            // time, and the copy is kept for seven days from now.
            .and_then(|()| std::fs::read(&offer.journal))
            .and_then(|bytes| std::fs::write(&copy, bytes))
            .map_err(io_error(&copy));
        if let Err(error) = copied {
            self.offers.insert(0, offer);
            return Err(error);
        }
        let cut = journal::read_file(&offer.journal, &LoadOptions::default())
            .and_then(|read| discard_uncommitted_file(&offer.journal, &read))
            .map_err(io_error(&offer.journal));
        if let Err(error) = cut {
            // Nothing was removed from beside the set: it stays offered, and
            // a retry makes its own copy.
            let _ = std::fs::remove_file(&copy);
            self.offers.insert(0, offer);
            return Err(error);
        }
        forget_session(&self.dir, &offer.info.id, offer.lock);
        Ok(copy)
    }

    /// Starts journalling a newly opened set: `set` as [`SetFile::open`]
    /// gave it, or `None` for a new set that has never been saved. Call it
    /// whenever the open set changes (New, Open, a recovery).
    ///
    /// If a set was open and wasn't closed with [`Autosave::close_set`],
    /// its session ends as a crash would, so its unsaved edits are offered
    /// rather than lost.
    ///
    /// # Errors
    ///
    /// [`AutosaveError::Io`] if the session can't register in the recovery
    /// folder. Crash recovery is off for the set then.
    pub fn begin(&mut self, set: Option<&SetFile>) -> Result<(), AutosaveError> {
        if let Some(session) = self.session.take() {
            session.park();
            self.scan()?;
        }
        // Kept even when the session can't start, so a later edit can try
        // again (the crash-recovery spec, R27).
        self.set = set.cloned();
        let id = new_session_id();
        let target = Target::of(set);
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let start = WriterStart {
            dir: self.dir.clone(),
            id: id.clone(),
            target,
            continuous: self.continuous,
            shared: Arc::clone(&shared),
        };
        let thread = std::thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || match Writer::start(start) {
                Ok(writer) => {
                    let _ = ready_tx.send(Ok(()));
                    writer.run(&rx);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            })
            .map_err(io_error(&self.dir))?;
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let _ = thread.join();
                return Err(error);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(AutosaveError::Journal {
                    reason: "the writer thread stopped as it started".to_owned(),
                });
            }
        }
        self.session = Some(Session {
            id,
            tx,
            thread: Some(thread),
            shared,
            sent: 0,
        });
        Ok(())
    }

    /// Starts the session again when it couldn't start, so that a later
    /// edit tries once more: the recovery folder may be writable now (the
    /// crash-recovery spec, R27). Does nothing when a session is running.
    ///
    /// # Errors
    ///
    /// [`AutosaveError::Io`] if the session can't register in the recovery
    /// folder, as [`Autosave::begin`] says.
    pub fn retry(&mut self) -> Result<(), AutosaveError> {
        if self.session.is_some() {
            return Ok(());
        }
        let set = self.set.clone();
        self.begin(set.as_ref())
    }

    /// Journals the open set after a finished edit: a command, a gesture's
    /// transaction once committed, an undo, or a redo. `label` names the
    /// edit as the undo history does. It returns at once; the entry is on
    /// disk within a second (see [`Autosave::flush`] and
    /// [`Autosave::status`]). Does nothing if no set is open.
    pub fn edited(&mut self, project: &Project, label: impl Into<String>) {
        let Some(session) = &mut self.session else {
            return;
        };
        session.sent += 1;
        let edit = Edit {
            n: session.sent,
            project: Box::new(project.clone()),
            label: label.into(),
            time: SystemTime::now(),
        };
        // The writer only stops when the session ends; a failed send means
        // it panicked, which the status reports.
        let _ = session.tx.send(Msg::Edit(edit));
    }

    /// Tells the service the open set was saved (Save or Save As), so the
    /// journal it kept is retired: the set file now holds every edit. Stale
    /// journals beside the set that hold nothing to recover go too.
    pub fn saved(&mut self, set: &SetFile) {
        let (Some(session), Some(follows)) = (&self.session, set.follows()) else {
            return;
        };
        let _ = session.tx.send(Msg::Saved {
            set: set.path().to_owned(),
            follows,
            through: session.sent,
        });
    }

    /// Turns Save Continuously on or off. Turning it on commits the open
    /// set's unsaved edits as a save point within a second, and from then
    /// on every finished edit of a set with a file is a save point. A set
    /// that has never been saved has nowhere to commit to, so crash
    /// recovery covers it until its first save.
    pub fn set_continuous(&mut self, on: bool) {
        self.continuous = on;
        if let Some(session) = &self.session {
            let _ = session.tx.send(Msg::Continuous(on));
        }
    }

    /// Whether Save Continuously is on.
    #[must_use]
    pub fn continuous(&self) -> bool {
        self.continuous
    }

    /// Waits until every edit reported so far is on disk.
    ///
    /// # Errors
    ///
    /// [`AutosaveError::NoSession`] if no set is open, and
    /// [`AutosaveError::Journal`] if the last write failed.
    pub fn flush(&self) -> Result<(), AutosaveError> {
        let session = self.session.as_ref().ok_or(AutosaveError::NoSession)?;
        session.call(Msg::Flush)
    }
    /// Closes the open set's session: after the set was saved, or when the
    /// user chose not to save it. Its unsaved edits are discarded (save
    /// points stay), and its files leave the recovery folder, so nothing is
    /// offered next time.
    ///
    /// # Errors
    ///
    /// [`AutosaveError::NoSession`] if no set is open, and
    /// [`AutosaveError::Journal`] if the journal couldn't be cut back.
    pub fn close_set(&mut self) -> Result<(), AutosaveError> {
        let mut session = self.session.take().ok_or(AutosaveError::NoSession)?;
        let result = session.call(Msg::Close);
        if let Some(thread) = session.thread.take() {
            let _ = thread.join();
        }
        result
    }

    /// What the service is doing.
    #[must_use]
    pub fn status(&self) -> AutosaveStatus {
        let session = self.session.as_ref();
        AutosaveStatus {
            session: session.map(|session| session.id.clone()),
            edits: session.map_or(0, |session| session.sent),
            journalled: session.map_or(0, |session| session.shared.durable.load(Ordering::Acquire)),
            continuous: self.continuous,
            error: session.and_then(|session| session.shared.error()),
        }
    }
}

impl Drop for Autosave {
    /// Stops the writer once it has written what it was sent, leaving the
    /// session to be offered next time, as a crash would.
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            session.park();
        }
    }
}

/// The open set's session, seen from the caller's thread.
#[derive(Debug)]
struct Session {
    id: String,
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    /// Edits sent so far.
    sent: u64,
}

impl Session {
    /// Sends a message that answers, and waits for the answer.
    fn call(
        &self,
        message: impl FnOnce(SyncSender<Result<(), String>>) -> Msg,
    ) -> Result<(), AutosaveError> {
        let (reply, answer) = mpsc::sync_channel(1);
        let stopped = || AutosaveError::Journal {
            reason: "the writer thread has stopped".to_owned(),
        };
        self.tx.send(message(reply)).map_err(|_| stopped())?;
        answer
            .recv()
            .map_err(|_| stopped())?
            .map_err(|reason| AutosaveError::Journal { reason })
    }

    /// Ends the session without closing it: the writer writes what it was
    /// sent and stops, and its lock goes with it.
    fn park(mut self) {
        drop(self.tx);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the writer thread and the caller share.
#[derive(Debug, Default)]
struct Shared {
    /// The number of the newest edit on disk.
    durable: AtomicU64,
    /// The last write's error, until a write succeeds.
    error: Mutex<Option<String>>,
}

impl Shared {
    fn error(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_error(&self, error: Option<String>) {
        *self.error.lock().unwrap_or_else(PoisonError::into_inner) = error;
    }
}

/// One finished edit, on its way to the journal.
#[derive(Debug)]
struct Edit {
    /// Its number in the session, from 1.
    n: u64,
    project: Box<Project>,
    label: String,
    time: SystemTime,
}

#[derive(Debug)]
enum Msg {
    Edit(Edit),
    Continuous(bool),
    Saved {
        set: PathBuf,
        follows: Follows,
        /// The newest edit the save holds.
        through: u64,
    },
    Flush(SyncSender<Result<(), String>>),
    Close(SyncSender<Result<(), String>>),
}

/// The set a session journals.
#[derive(Clone, Debug)]
struct Target {
    set: Option<PathBuf>,
    follows: Option<Follows>,
}

impl Target {
    fn of(set: Option<&SetFile>) -> Self {
        Self {
            set: set.map(|set| set.path().to_owned()),
            follows: set.and_then(SetFile::follows),
        }
    }
}

struct WriterStart {
    dir: PathBuf,
    id: String,
    target: Target,
    continuous: bool,
    shared: Arc<Shared>,
}

/// The writer thread's state: all of a session's file work happens here.
struct Writer {
    dir: PathBuf,
    id: String,
    started: u64,
    lock: File,
    target: Target,
    /// Where the journal is, or will be.
    journal_path: PathBuf,
    journal: Option<JournalWriter>,
    continuous: bool,
    shared: Arc<Shared>,
}

impl Writer {
    /// Takes the session's lock, then says where its journal will be.
    fn start(start: WriterStart) -> Result<Self, AutosaveError> {
        let WriterStart {
            dir,
            id,
            target,
            continuous,
            shared,
        } = start;
        // The lock is made under a hidden name and locked before it takes
        // its real one, so another instance never finds it unlocked.
        let lock_path = dir.join(format!("{id}.{LOCK_EXTENSION}"));
        let making = dir.join(format!(".{id}.{LOCK_EXTENSION}.tmp"));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&making)
            .map_err(io_error(&making))?;
        let locked = lock
            .try_lock()
            .map_err(|error| match error {
                TryLockError::Error(error) => error,
                TryLockError::WouldBlock => io::Error::from(io::ErrorKind::WouldBlock),
            })
            .and_then(|()| std::fs::rename(&making, &lock_path));
        if let Err(source) = locked {
            drop(lock);
            let _ = std::fs::remove_file(&making);
            return Err(AutosaveError::Io {
                path: lock_path,
                source,
            });
        }
        let writer = Self {
            journal_path: planned_journal(&dir, &id, &target),
            dir,
            id,
            started: millis(SystemTime::now()),
            lock,
            target,
            journal: None,
            continuous,
            shared,
        };
        if let Err(error) = writer.write_index() {
            let _ = std::fs::remove_file(&lock_path);
            return Err(error);
        }
        Ok(writer)
    }

    fn run(mut self, rx: &Receiver<Msg>) {
        debug_assert_eq!(std::thread::current().name(), Some(THREAD_NAME));
        let mut pending: Option<Edit> = None;
        while let Ok(first) = rx.recv() {
            for message in std::iter::once(first).chain(rx.try_iter()) {
                match message {
                    // Edits that arrive together are written as one entry:
                    // the newest state holds every one of them.
                    Msg::Edit(edit) => pending = Some(edit),
                    Msg::Continuous(on) => {
                        self.write(&mut pending);
                        self.set_continuous(on);
                    }
                    Msg::Saved {
                        set,
                        follows,
                        through,
                    } => {
                        // The save holds these edits; journal only later ones.
                        if pending.as_ref().is_some_and(|edit| edit.n <= through) {
                            pending = None;
                        }
                        self.retire(set, follows, through);
                    }
                    Msg::Flush(reply) => {
                        self.write(&mut pending);
                        let _ = reply.send(self.shared.error().map_or(Ok(()), Err));
                    }
                    Msg::Close(reply) => {
                        self.write(&mut pending);
                        let _ = reply.send(self.close());
                        return;
                    }
                }
            }
            self.write(&mut pending);
        }
        // Parked: the session ends as a crash would, its lock released as
        // the file closes.
    }

    /// Whether entries are save points.
    fn commits(&self) -> bool {
        self.continuous && self.target.follows.is_some() && self.journal_is_beside_set()
    }

    fn journal_is_beside_set(&self) -> bool {
        self.journal_path.parent() != Some(self.dir.as_path())
    }

    fn write(&mut self, pending: &mut Option<Edit>) {
        let Some(edit) = pending.take() else {
            return;
        };
        let revision = self.target.follows.map_or(0, |follows| follows.revision);
        let result = to_value(&edit.project, revision)
            .map_err(|error| error.to_string())
            .and_then(|value| {
                let committed = self.commits();
                let journal = self.journal().map_err(|error| error.to_string())?;
                let pending = Pending {
                    value,
                    label: edit.label,
                    time: edit.time,
                    committed,
                };
                journal
                    .append(vec![pending])
                    .map_err(|error| format!("{}: {error}", journal.path().display()))
            });
        match result {
            Ok(_) => {
                self.shared.durable.store(edit.n, Ordering::Release);
                self.shared.set_error(None);
            }
            Err(error) => self.shared.set_error(Some(error)),
        }
    }

    /// The open journal, created on the first entry: beside the set, or in
    /// the recovery folder if that fails (a read-only project, say).
    fn journal(&mut self) -> io::Result<&mut JournalWriter> {
        if self.journal.is_none() {
            let created =
                JournalWriter::create(self.journal_path.clone(), &self.id, self.target.follows);
            let journal = match created {
                Ok(journal) => journal,
                Err(_) if self.journal_is_beside_set() => {
                    let beside = std::mem::replace(
                        &mut self.journal_path,
                        self.dir.join(format!("{}.{JOURNAL_EXTENSION}", self.id)),
                    );
                    // The index still names the journal beside the set if it
                    // can't be rewritten, so the next edit must try again.
                    if let Err(error) = self.write_index() {
                        self.journal_path = beside;
                        return Err(io::Error::other(error));
                    }
                    JournalWriter::create(self.journal_path.clone(), &self.id, self.target.follows)?
                }
                Err(error) => return Err(error),
            };
            self.journal = Some(journal);
        }
        Ok(self.journal.as_mut().expect("created above"))
    }

    fn set_continuous(&mut self, on: bool) {
        self.continuous = on;
        if self.commits()
            && let Some(journal) = &mut self.journal
        {
            match journal.commit("Save Continuously", SystemTime::now()) {
                Ok(_) => self.shared.set_error(None),
                Err(error) => self.shared.set_error(Some(error.to_string())),
            }
        }
    }

    /// After a save: the journal's edits are in the set file, so it goes,
    /// and the next edit starts a new one that follows the new revision.
    /// After Save As the old set keeps the save points it already has.
    fn retire(&mut self, set: PathBuf, follows: Follows, through: u64) {
        let save_as = self.journal_is_beside_set() && self.target.set.as_ref() != Some(&set);
        if let Some(journal) = self.journal.take() {
            let path = journal.path().to_owned();
            let kept = if save_as {
                // On an error, leave the file as it is.
                journal.discard_uncommitted().unwrap_or(true)
            } else {
                drop(journal);
                false
            };
            if !kept {
                let _ = std::fs::remove_file(path);
            }
        }
        tidy_beside(&self.dir, &set, follows, &self.id);
        self.target = Target {
            set: Some(set),
            follows: Some(follows),
        };
        self.journal_path = planned_journal(&self.dir, &self.id, &self.target);
        match self.write_index() {
            Ok(()) => {
                self.shared.durable.fetch_max(through, Ordering::AcqRel);
                self.shared.set_error(None);
            }
            Err(error) => self.shared.set_error(Some(error.to_string())),
        }
    }

    /// Ends the session cleanly: unsaved edits go, save points stay, and
    /// the session leaves the recovery folder.
    fn close(self) -> Result<(), String> {
        let mut result = Ok(());
        if let Some(journal) = self.journal {
            let path = journal.path().to_owned();
            match journal.discard_uncommitted() {
                Ok(true) => {}
                Ok(false) => {
                    let _ = std::fs::remove_file(&path);
                }
                Err(error) => result = Err(format!("{}: {error}", path.display())),
            }
        }
        forget_session(&self.dir, &self.id, self.lock);
        result
    }

    /// Writes the session's index, replacing the last one in one step.
    fn write_index(&self) -> Result<(), AutosaveError> {
        let index = SessionIndex {
            format: SESSION_FORMAT.to_owned(),
            version: 1,
            pid: std::process::id(),
            started: self.started,
            set: self.target.set.clone(),
            journal: self.journal_path.clone(),
        };
        let path = self.dir.join(format!("{}.{SESSION_EXTENSION}", self.id));
        let temp = self
            .dir
            .join(format!(".{}.{SESSION_EXTENSION}.tmp", self.id));
        let bytes = serde_json::to_vec_pretty(&index).map_err(|error| AutosaveError::Journal {
            reason: format!("the session's paths can't be written: {error}"),
        })?;
        let written = File::create(&temp)
            .and_then(|mut file| {
                file.write_all(&bytes)?;
                file.sync_all()
            })
            .and_then(|()| std::fs::rename(&temp, &path));
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temp);
            return Err(AutosaveError::Io {
                path,
                source: error,
            });
        }
        if cfg!(unix) {
            let _ = File::open(&self.dir).and_then(|dir| dir.sync_all());
        }
        Ok(())
    }
}

/// Where a session's journal goes: beside its set once the set has a file,
/// otherwise in the recovery folder.
fn planned_journal(dir: &Path, id: &str, target: &Target) -> PathBuf {
    match (&target.set, target.follows) {
        (Some(set), Some(_)) => journal::beside(set, id),
        _ => None,
    }
    .unwrap_or_else(|| dir.join(format!("{id}.{JOURNAL_EXTENSION}")))
}

/// Deletes journals beside a set that follow an older revision, belong to
/// no running session, and hold nothing to recover: save points a normal
/// save has superseded.
fn tidy_beside(dir: &Path, set: &Path, follows: Follows, own: &str) {
    let options = LoadOptions::default();
    for path in journal::journals_beside(set).unwrap_or_default() {
        let Some(session) = journal::session_of(&path) else {
            continue;
        };
        if session == own || is_running(dir, &session) {
            continue;
        }
        let Ok(read) = journal::read_file(&path, &options) else {
            continue;
        };
        // A save point that doesn't decode may be from a newer version too.
        if read.follows != Some(follows)
            && read.uncommitted() == 0
            && !read.newer
            && read
                .committed
                .as_ref()
                .is_none_or(|value| decode_value(value, &options).is_ok())
        {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether a session holds its lock: running here or in another instance,
/// or waiting in this instance's offers.
fn is_running(dir: &Path, session: &str) -> bool {
    let lock_path = dir.join(format!("{session}.{LOCK_EXTENSION}"));
    let Ok(lock) = OpenOptions::new().read(true).write(true).open(lock_path) else {
        return false;
    };
    match lock.try_lock() {
        Ok(()) => {
            let _ = lock.unlock();
            false
        }
        Err(_) => true,
    }
}

/// Removes a session's index and lock from the recovery folder. The lock is
/// held by the caller and released here.
fn forget_session(dir: &Path, id: &str, lock: File) {
    let _ = std::fs::remove_file(dir.join(format!("{id}.{SESSION_EXTENSION}")));
    let _ = lock.unlock();
    drop(lock);
    let _ = std::fs::remove_file(dir.join(format!("{id}.{LOCK_EXTENSION}")));
}

/// What a session's index says.
enum Index {
    /// Where its set and journal are.
    Read(SessionIndex),
    /// It's from a newer version of Beatlayer.
    Newer,
    /// There's none, or it isn't a session's index. Indexes are replaced in
    /// one step, so a damaged one isn't a write cut short.
    Missing,
}

fn read_index(path: &Path) -> Index {
    let Some(value) = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    else {
        return Index::Missing;
    };
    if value["format"] == SESSION_FORMAT && value["version"].as_u64().is_some_and(|v| v > 1) {
        return Index::Newer;
    }
    match serde_json::from_value::<SessionIndex>(value) {
        Ok(index) if index.format == SESSION_FORMAT && index.version == 1 => Index::Read(index),
        _ => Index::Missing,
    }
}

/// The IDs of the sessions in a recovery folder, from their index and lock
/// files.
fn session_ids(dir: &Path) -> io::Result<Vec<String>> {
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        let id = name
            .strip_suffix(&format!(".{SESSION_EXTENSION}"))
            .or_else(|| name.strip_suffix(&format!(".{LOCK_EXTENSION}")));
        if let Some(id) = id
            && !id.is_empty()
            && !id.starts_with('.')
            && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// A session ID no other session has: the process, the time, and a count.
fn new_session_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{:x}-{:x}-{:x}",
        std::process::id(),
        millis(SystemTime::now()),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Deletes declined journals older than [`KEEP_DISCARDED`].
fn prune_discarded(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| {
                now.duration_since(modified)
                    .is_ok_and(|age| age > KEEP_DISCARDED)
            });
        let journal = path
            .extension()
            .is_some_and(|extension| extension == JOURNAL_EXTENSION);
        if old && journal {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use bl_model::{Command, Editor, TrackKind};

    use super::*;
    use crate::media::Media;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("bl-autosave-{label}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn recovery(&self) -> PathBuf {
            self.0.join("Recovery")
        }

        fn files(&self, dir: &Path) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn rename(editor: &mut Editor, name: &str) -> Project {
        let existing = editor.document().top_level().next();
        let track = match existing {
            Some(track) => track,
            None => {
                let id = editor.new_id();
                editor
                    .apply(Command::CreateTrack {
                        id,
                        kind: TrackKind::Audio,
                        name: String::new(),
                        parent: None,
                        index: 0,
                    })
                    .unwrap();
                id
            }
        };
        editor
            .apply(Command::RenameTrack {
                track,
                name: name.to_owned(),
            })
            .unwrap();
        Project::from_editor(editor, Media::new())
    }

    #[test]
    fn a_session_that_closes_leaves_nothing() {
        // Spec: crash-recovery R5, R6
        let dir = TestDir::new("close");
        let set_path = dir.0.join("Song.blset");
        let mut set = SetFile::new(&set_path);
        set.save(&Project::new()).unwrap();

        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        assert!(autosave.offers().is_empty());
        autosave.begin(Some(&set)).unwrap();
        let mut editor = Project::new().editor();
        for n in 0..5 {
            autosave.edited(&rename(&mut editor, &format!("Take {n}")), "Rename Track");
        }
        autosave.flush().unwrap();
        let status = autosave.status();
        assert_eq!((status.edits, status.journalled), (5, 5));
        assert_eq!(status.error, None);
        // The journal is beside the set, hidden.
        let beside = journal::journals_beside(&set_path).unwrap();
        assert_eq!(beside.len(), 1);
        assert!(
            beside[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".Song.blset.")
        );
        // A second instance sees a running session, and offers nothing.
        let other = Autosave::start(dir.recovery()).unwrap();
        assert!(other.offers().is_empty());
        drop(other);

        autosave.close_set().unwrap();
        assert_eq!(dir.files(&dir.recovery()), Vec::<String>::new());
        assert!(journal::journals_beside(&set_path).unwrap().is_empty());
        assert!(Autosave::start(dir.recovery()).unwrap().offers().is_empty());
    }

    #[test]
    fn a_session_that_stops_is_offered_and_recovers() {
        // Spec: crash-recovery R2, R3
        let dir = TestDir::new("offer");
        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        // A new set, never saved: its journal is in the recovery folder.
        autosave.begin(None).unwrap();
        let mut editor = Editor::new();
        let mut last = Project::new();
        for n in 0..20 {
            last = rename(&mut editor, &format!("Take {n}"));
            autosave.edited(&last, "Rename Track");
        }
        autosave.flush().unwrap();
        // Stopped without closing, as a crash does.
        drop(autosave);

        let mut relaunched = Autosave::start(dir.recovery()).unwrap();
        let offers = relaunched.offers();
        assert_eq!(offers.len(), 1, "{offers:?}");
        assert_eq!(offers[0].set, None);
        assert_eq!(offers[0].name(), "Untitled");
        assert!(offers[0].edits >= 1);
        assert_eq!(offers[0].last_label, "Rename Track");
        assert!(!offers[0].saved_since);

        let recovered = relaunched.recover(None).unwrap();
        assert_eq!(recovered.project, last);
        assert!(recovered.set.is_none());
        assert!(relaunched.offers().is_empty());
        // The recovered song is journalled in a new session at once, and the
        // old session is gone: a crash now offers the recovered song again.
        drop(relaunched);
        let mut again = Autosave::start(dir.recovery()).unwrap();
        let offers = again.offers();
        assert_eq!(offers.len(), 1);
        assert_ne!(offers[0].id, recovered.id);
        assert_eq!(again.recover(None).unwrap().project, last);
        again.close_set().unwrap();
        assert_eq!(dir.files(&dir.recovery()), Vec::<String>::new());
    }

    #[test]
    fn discarding_keeps_a_copy_and_removes_the_edits() {
        // Spec: crash-recovery R4
        let dir = TestDir::new("discard");
        let set_path = dir.0.join("Song.blset");
        let mut set = SetFile::new(&set_path);
        set.save(&Project::new()).unwrap();
        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        autosave.begin(Some(&set)).unwrap();
        let mut editor = Project::new().editor();
        autosave.edited(&rename(&mut editor, "Lost"), "Rename Track");
        autosave.flush().unwrap();
        drop(autosave);

        let mut relaunched = Autosave::start(dir.recovery()).unwrap();
        let offer = relaunched.offers().remove(0);
        assert_eq!(offer.set.as_deref(), Some(set_path.as_path()));
        assert_eq!(offer.name(), "Song");
        assert!(matches!(
            relaunched.discard(Some("nope")),
            Err(AutosaveError::NoOffer { .. })
        ));
        let copy = relaunched.discard(Some(&offer.id)).unwrap();
        assert!(copy.starts_with(dir.recovery().join(DISCARDED)));
        let kept = journal::read_file(&copy, &LoadOptions::default()).unwrap();
        assert_eq!(kept.entries.len(), 1);
        assert!(journal::journals_beside(&set_path).unwrap().is_empty());
        assert!(Autosave::start(dir.recovery()).unwrap().offers().is_empty());
        assert!(matches!(
            relaunched.recover(None),
            Err(AutosaveError::NoOffer { id: None })
        ));

        // Copies older than a week go at the next start.
        let old = SystemTime::now() + KEEP_DISCARDED + Duration::from_secs(60);
        prune_discarded(&dir.recovery().join(DISCARDED), old);
        assert!(!copy.exists());
    }

    #[test]
    fn a_save_point_reports_the_schema_it_was_written_in() {
        // Spec: modulation R5. An older set journalled by this build opens
        // at a save point in this build's form, which mustn't be converted
        // again.
        let dir = TestDir::new("schema");
        let set_path = dir.0.join("Song.blset");
        std::fs::write(
            &set_path,
            include_bytes!("../tests/fixtures/schema-4.blset"),
        )
        .unwrap();
        let (set, project) = SetFile::open(&set_path).unwrap();
        assert!(set.info().unwrap().before_relative_modulation());
        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        autosave.begin(Some(&set)).unwrap();
        autosave.set_continuous(true);
        let mut editor = project.editor();
        let edited = rename(&mut editor, "Edited");
        autosave.edited(&edited, "Rename Track");
        autosave.flush().unwrap();
        let (opened, project) = SetFile::open(&set_path).unwrap();
        assert_eq!(project, edited);
        assert!(!opened.info().unwrap().before_relative_modulation());
        assert!(
            !crate::load(&set_path)
                .unwrap()
                .1
                .before_relative_modulation()
        );
    }

    #[test]
    fn saving_retires_the_journal_and_continuous_save_commits() {
        // Spec: crash-recovery R6, R8, R9, R10
        let dir = TestDir::new("continuous");
        let set_path = dir.0.join("Song.blset");
        let mut set = SetFile::new(&set_path);
        set.save(&Project::new()).unwrap();
        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        autosave.begin(Some(&set)).unwrap();
        let mut editor = Project::new().editor();

        // Unsaved edits, then Save: the journal is retired.
        let first = rename(&mut editor, "One");
        autosave.edited(&first, "Rename Track");
        autosave.flush().unwrap();
        assert_eq!(journal::journals_beside(&set_path).unwrap().len(), 1);
        set.save(&first).unwrap();
        autosave.saved(&set);
        autosave.flush().unwrap();
        assert!(journal::journals_beside(&set_path).unwrap().is_empty());
        assert_eq!(autosave.status().journalled, 1);

        // An unsaved edit, then Save Continuously on: it's committed, so
        // opening the set shows it.
        let second = rename(&mut editor, "Two");
        autosave.edited(&second, "Rename Track");
        autosave.flush().unwrap();
        assert_eq!(SetFile::open(&set_path).unwrap().1, first);
        autosave.set_continuous(true);
        autosave.flush().unwrap();
        assert_eq!(SetFile::open(&set_path).unwrap().1, second);
        let third = rename(&mut editor, "Three");
        autosave.edited(&third, "Rename Track");
        autosave.flush().unwrap();
        let (opened, project) = SetFile::open(&set_path).unwrap();
        assert_eq!(project, third);
        assert_eq!(crate::load(&set_path).unwrap().0, third);
        // The set file itself is still revision 2; the save points are the
        // history since.
        assert_eq!(opened.revision(), 2);
        let points = opened.save_points();
        assert_eq!(points.len(), 2, "{points:?}");
        assert_eq!(journal::restore(&points[0]).unwrap(), second);
        assert_eq!(journal::restore(&points[1]).unwrap(), third);

        // Stopping now offers nothing: every edit is saved.
        drop(autosave);
        let mut relaunched = Autosave::start(dir.recovery()).unwrap();
        assert!(relaunched.offers().is_empty());
        assert_eq!(SetFile::open(&set_path).unwrap().1, third);

        // A normal save writes revision 3 and retires the old save points.
        let (mut set, project) = SetFile::open(&set_path).unwrap();
        relaunched.begin(Some(&set)).unwrap();
        set.save(&project).unwrap();
        relaunched.saved(&set);
        relaunched.flush().unwrap();
        assert!(journal::journals_beside(&set_path).unwrap().is_empty());
        assert_eq!(SetFile::open(&set_path).unwrap().0.revision(), 3);
        relaunched.close_set().unwrap();
        assert_eq!(dir.files(&dir.recovery()), Vec::<String>::new());
    }

    #[test]
    fn beginning_another_set_keeps_the_last_ones_edits() {
        // Spec: crash-recovery R7
        let dir = TestDir::new("switch");
        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        autosave.begin(None).unwrap();
        let mut editor = Editor::new();
        let edited = rename(&mut editor, "Unsaved");
        autosave.edited(&edited, "Rename Track");
        // Opening another set without closing this one: it's offered.
        autosave.begin(None).unwrap();
        let offers = autosave.offers();
        assert_eq!(offers.len(), 1);
        assert_eq!(autosave.recover(None).unwrap().project, edited);
        // The recovered set is unsaved too, so leaving it offers it again.
        autosave.begin(None).unwrap();
        assert_eq!(autosave.offers().len(), 1);
        autosave.discard(None).unwrap();
        // A session with no edits leaves nothing to offer.
        autosave.begin(None).unwrap();
        assert!(autosave.offers().is_empty());
        autosave.close_set().unwrap();
        assert!(matches!(
            autosave.close_set(),
            Err(AutosaveError::NoSession)
        ));
        assert!(matches!(autosave.flush(), Err(AutosaveError::NoSession)));
    }

    #[test]
    fn a_read_only_project_journals_in_the_recovery_folder() {
        let dir = TestDir::new("fallback");
        let set_path = dir.0.join("Project").join("Song.blset");
        let mut set = SetFile::new(&set_path);
        set.save(&Project::new()).unwrap();
        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        autosave.begin(Some(&set)).unwrap();
        // The project folder goes away (a drive unplugged, say).
        std::fs::remove_dir_all(dir.0.join("Project")).unwrap();
        let mut editor = Project::new().editor();
        let edited = rename(&mut editor, "Kept");
        autosave.edited(&edited, "Rename Track");
        autosave.flush().unwrap();
        drop(autosave);
        let mut relaunched = Autosave::start(dir.recovery()).unwrap();
        assert_eq!(relaunched.offers().len(), 1);
        let recovered = relaunched.recover(None).unwrap();
        assert_eq!(recovered.project, edited);
        assert_eq!(recovered.set.unwrap().path(), set_path);
    }

    #[test]
    fn a_newer_versions_sessions_are_left_alone() {
        let dir = TestDir::new("newer");
        let recovery = dir.recovery();
        std::fs::create_dir_all(&recovery).unwrap();
        // A session a newer Beatlayer left, its process gone.
        std::fs::write(recovery.join("1-2-3.lock"), "").unwrap();
        std::fs::write(
            recovery.join("1-2-3.session"),
            r#"{"format":"beatlayer-session","version":2,"journal":"elsewhere"}"#,
        )
        .unwrap();
        // And stale files this version wrote: a lock without an index, and
        // an index that isn't one.
        std::fs::write(recovery.join("4-5-6.lock"), "").unwrap();
        std::fs::write(recovery.join("7-8-9.session"), "{}").unwrap();
        let autosave = Autosave::start(&recovery).unwrap();
        assert!(autosave.offers().is_empty());
        assert_eq!(dir.files(&recovery), ["1-2-3.lock", "1-2-3.session"]);
    }

    #[test]
    fn an_index_never_deletes_a_file_that_isnt_its_journal() {
        let dir = TestDir::new("foreign");
        let recovery = dir.recovery();
        std::fs::create_dir_all(&recovery).unwrap();
        // A session whose index names a file that isn't its journal.
        let other = dir.0.join("Notes.txt");
        std::fs::write(&other, "not a journal").unwrap();
        std::fs::write(recovery.join("1-2-4.lock"), "").unwrap();
        let index = serde_json::json!({
            "format": SESSION_FORMAT,
            "version": 1,
            "pid": 1,
            "started": 0,
            "set": null,
            "journal": other,
        });
        std::fs::write(recovery.join("1-2-4.session"), index.to_string()).unwrap();
        let autosave = Autosave::start(&recovery).unwrap();
        assert!(autosave.offers().is_empty());
        // The session is tidied away; the file it named is left alone.
        assert_eq!(dir.files(&recovery), Vec::<String>::new());
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "not a journal");
    }

    #[test]
    fn default_recovery_dir_is_in_the_users_folders() {
        if let Some(dir) = default_recovery_dir() {
            assert!(dir.is_absolute() || cfg!(not(unix)), "{}", dir.display());
            let text = dir.to_string_lossy().to_lowercase();
            assert!(
                text.contains("beatlayer") && text.contains("recovery"),
                "{text}"
            );
        }
    }

    #[test]
    fn save_as_leaves_the_old_sets_save_points() {
        let dir = TestDir::new("save-as");
        let set_path = dir.0.join("Song.blset");
        let mut set = SetFile::new(&set_path);
        set.save(&Project::new()).unwrap();
        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        autosave.begin(Some(&set)).unwrap();
        autosave.set_continuous(true);
        let mut editor = Project::new().editor();
        let three = rename(&mut editor, "Three");
        autosave.edited(&three, "Rename Track");
        autosave.flush().unwrap();
        assert_eq!(SetFile::open(&set_path).unwrap().1, three);

        let copy_path = dir.0.join("Song v2.blset");
        let mut copy = SetFile::new(&copy_path);
        copy.save(&three).unwrap();
        autosave.saved(&copy);
        autosave.flush().unwrap();
        // The old set still opens at its save point.
        assert_eq!(SetFile::open(&set_path).unwrap().1, three);
        assert_eq!(SetFile::open(&copy_path).unwrap().1, three);

        // Saving the same set again still retires its journal.
        let four = rename(&mut editor, "Four");
        autosave.edited(&four, "Rename Track");
        autosave.flush().unwrap();
        copy.save(&four).unwrap();
        autosave.saved(&copy);
        autosave.flush().unwrap();
        assert!(journal::journals_beside(&copy_path).unwrap().is_empty());
    }

    #[test]
    fn a_save_point_that_wont_decode_survives_a_normal_save() {
        let dir = TestDir::new("undecodable");
        let set_path = dir.0.join("Song.blset");
        let mut set = SetFile::new(&set_path);
        set.save(&Project::new()).unwrap();
        let follows = set.follows().unwrap();
        // A dead session's save point, as a newer Beatlayer might write it.
        let stale = journal::beside(&set_path, "dead-1").unwrap();
        let bad = serde_json::json!({ "format": "not a set" });
        assert!(decode_value(&bad, &LoadOptions::default()).is_err());
        JournalWriter::create(stale.clone(), "dead-1", Some(follows))
            .unwrap()
            .append(vec![Pending {
                value: bad,
                label: "Rename Track".to_owned(),
                time: SystemTime::now(),
                committed: true,
            }])
            .unwrap();

        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        autosave.begin(Some(&set)).unwrap();
        set.save(&Project::new()).unwrap();
        autosave.saved(&set);
        autosave.flush().unwrap();
        assert!(stale.exists());
    }

    #[test]
    fn a_discarded_copy_is_kept_a_week_from_the_discard() {
        let dir = TestDir::new("discard-age");
        let set_path = dir.0.join("Song.blset");
        let mut set = SetFile::new(&set_path);
        set.save(&Project::new()).unwrap();
        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        autosave.begin(Some(&set)).unwrap();
        let mut editor = Project::new().editor();
        autosave.edited(&rename(&mut editor, "Lost"), "Rename Track");
        autosave.flush().unwrap();
        drop(autosave);
        // The crash was a month ago.
        let month = Duration::from_secs(30 * 24 * 60 * 60);
        let beside = journal::journals_beside(&set_path).unwrap().remove(0);
        File::options()
            .write(true)
            .open(&beside)
            .unwrap()
            .set_modified(SystemTime::now() - month)
            .unwrap();

        let mut relaunched = Autosave::start(dir.recovery()).unwrap();
        let copy = relaunched.discard(None).unwrap();
        prune_discarded(&dir.recovery().join(DISCARDED), SystemTime::now());
        assert!(copy.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_discard_leaves_the_set_offered_and_no_copy() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TestDir::new("discard-fails");
        let folder = dir.0.join("Project");
        let set_path = folder.join("Song.blset");
        let mut set = SetFile::new(&set_path);
        set.save(&Project::new()).unwrap();
        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        autosave.begin(Some(&set)).unwrap();
        let mut editor = Project::new().editor();
        autosave.edited(&rename(&mut editor, "Lost"), "Rename Track");
        autosave.flush().unwrap();
        drop(autosave);

        let mut relaunched = Autosave::start(dir.recovery()).unwrap();
        // The project's folder turns read-only: its journal can't be cut.
        let writable = std::fs::metadata(&folder).unwrap().permissions();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = relaunched.discard(None);
        let offered = relaunched.offers().len();
        let copies = dir.files(&dir.recovery().join(DISCARDED));
        std::fs::set_permissions(&folder, writable).unwrap();
        if result.is_ok() {
            // Running as a user who ignores permissions: nothing to test.
            return;
        }
        assert_eq!(offered, 1);
        assert_eq!(copies, Vec::<String>::new());
        relaunched.discard(None).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_index_write_leaves_the_journal_where_the_index_says() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TestDir::new("index-fails");
        let set_path = dir.0.join("Project").join("Song.blset");
        let mut set = SetFile::new(&set_path);
        set.save(&Project::new()).unwrap();
        let mut autosave = Autosave::start(dir.recovery()).unwrap();
        autosave.begin(Some(&set)).unwrap();
        // The project's folder goes, and the recovery folder can't be
        // written to for one edit.
        std::fs::remove_dir_all(dir.0.join("Project")).unwrap();
        let writable = std::fs::metadata(dir.recovery()).unwrap().permissions();
        std::fs::set_permissions(&dir.recovery(), std::fs::Permissions::from_mode(0o555))
            .unwrap();
        let mut editor = Project::new().editor();
        autosave.edited(&rename(&mut editor, "Lost"), "Rename Track");
        let failed = autosave.flush();
        std::fs::set_permissions(&dir.recovery(), writable).unwrap();
        if failed.is_ok() {
            // Running as a user who ignores permissions: nothing to test.
            return;
        }
        let kept = rename(&mut editor, "Kept");
        autosave.edited(&kept, "Rename Track");
        autosave.flush().unwrap();
        drop(autosave);

        let mut relaunched = Autosave::start(dir.recovery()).unwrap();
        assert_eq!(relaunched.offers().len(), 1);
        assert_eq!(relaunched.recover(None).unwrap().project, kept);
    }
}
