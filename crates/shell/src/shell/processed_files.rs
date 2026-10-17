//! Processed files through the shell (WP-4.20): where they go, the Status
//! Bar while they're written, and the edit that lands once each is whole.
//! The writer is [`crate::processed`]; the behaviour is in the audio clip
//! editing spec, R1 to R6.

use std::time::Duration;

use super::*;
use crate::processed::{Job, Kind, Request, Written};

/// How long [`Shell::wait_processed`] sleeps between looks.
const WAIT: Duration = Duration::from_millis(1);

/// The edit that lands once a file is whole.
type Landed = Box<dyn FnOnce(&mut Shell, Written) -> Result<(), String>>;

/// A file being written, and what to do once it's whole.
pub(super) struct Pending {
    job: Job,
    done: Landed,
}

impl Shell {
    /// The Temporary Folder (File & Folder): the setting, or the default, a
    /// `Temporary` folder in Beatlayer's application-data folder (R5).
    #[must_use]
    pub fn temporary_folder(&self) -> PathBuf {
        self.prefs.temporary_folder.clone().unwrap_or_else(|| {
            self.config_dir
                .clone()
                .unwrap_or_else(|| std::env::temp_dir().join("Beatlayer"))
                .join("Temporary")
        })
    }

    /// The folder a processed file of `kind` is written to now: the
    /// project's `Samples/Processed/<kind>` (R1), or, in a set never saved,
    /// the kind's folder in the Temporary Folder (R5).
    #[must_use]
    pub fn processed_folder(&self, kind: Kind) -> PathBuf {
        match &self.set_path {
            Some(set) => ProjectFolder::of_set(set).processed(kind.folder()),
            None => self.temporary_folder().join(kind.folder()),
        }
    }

    /// Starts writing a processed file on a worker thread, with its progress
    /// in the Status Bar (R6). When the file is whole, it joins the set's
    /// media (R4) and `done` runs on the main thread with it: `done` makes
    /// the edit through [`Shell::run`], inside [`Shell::begin_gesture`] and
    /// [`Shell::end_gesture`] if it takes several commands, so the edit is
    /// one undo step. A failure deletes what was written and leaves the set
    /// as it was, with a message in the Status Bar. [`Shell::poll`] drives
    /// it; an agent's call waits with [`Shell::wait_processed`].
    ///
    /// # Errors
    ///
    /// A message if the file couldn't be started, also shown in the Status
    /// Bar.
    pub fn start_processed(
        &mut self,
        request: Request,
        done: impl FnOnce(&mut Shell, Written) -> Result<(), String> + 'static,
    ) -> Result<(), String> {
        let name = request.name.clone();
        let started = Job::start(&self.processed_folder(request.kind), request);
        match started {
            Ok(job) => {
                self.processed.push(Pending {
                    job,
                    done: Box::new(done),
                });
                self.show_processing();
                Ok(())
            }
            Err(error) => {
                self.set_message(format!("Couldn't write {name}: {error}"), true);
                Err(error)
            }
        }
    }

    /// Whether processed files are still being written.
    #[must_use]
    pub fn is_processing(&self) -> bool {
        !self.processed.is_empty()
    }

    /// Waits until every processed file is written and its edit has landed,
    /// as an agent's call does before it returns (R6).
    pub fn wait_processed(&mut self) {
        while self.is_processing() {
            self.poll_processed();
            std::thread::sleep(WAIT);
        }
    }

    /// Lands the files the workers have finished. Returns whether any file
    /// is still being written, so a window keeps redrawing its progress.
    pub(super) fn poll_processed(&mut self) -> bool {
        let mut index = 0;
        let mut failed = false;
        while index < self.processed.len() {
            let Some(result) = self.processed[index].job.poll() else {
                index += 1;
                continue;
            };
            let Pending { job, done } = self.processed.remove(index);
            match result.and_then(|part| self.land_processed(part, done)) {
                Ok(()) if self.processed.is_empty() => self.message = None,
                Ok(()) => {}
                Err(error) => {
                    failed = true;
                    self.set_message(format!("Couldn't write {}: {error}", job.name()), true);
                }
            }
        }
        // The progress doesn't hide a failure from this poll.
        if !failed {
            self.show_processing();
        }
        self.is_processing()
    }

    /// Puts a whole file in its place, adds it to the set's media (R3, R4)
    /// and runs the edit that uses it.
    fn land_processed(&mut self, part: crate::processed::Part, done: Landed) -> Result<(), String> {
        // Bytes the set has already are not kept twice (R3).
        let existing = self
            .media
            .entry(part.hash)
            .map(|entry| entry.path)
            .filter(|path| path.is_file());
        let written = part.place(existing)?;
        self.media
            .import(&written.path)
            .map_err(|e| e.to_string())?;
        self.store.add_media(
            bl_model::ContentHash(*written.hash.as_bytes()),
            &written.path,
            written.size,
        );
        done(self, written)
    }

    fn show_processing(&mut self) {
        if let Some(pending) = self.processed.first() {
            let percent = (pending.job.progress() * 100.0).round();
            self.message = Some(Message {
                text: format!("Writing {} ({percent}%)", pending.job.name()),
                error: false,
                action: None,
            });
        }
    }
}
