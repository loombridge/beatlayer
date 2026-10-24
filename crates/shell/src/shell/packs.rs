//! Packing a project into one file and unpacking it (WP-6.35b): the commands
//! `set.create_pack`, `set.install_pack`, `set.cancel_pack` and
//! `set.pack_status`.
//!
//! The behaviour is in the set exchange spec, R16 to R24; the pack's format
//! and its reader are `bl_project::pack`. A person's pack or unpack runs on a
//! worker thread, so the window goes on drawing, and ends in the Status Bar;
//! `Shell::run`, which an agent or a test calls, waits for the end and
//! returns the result.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use bl_project::pack;
use bl_shell_spec::file_manager::Location;
use bl_shell_spec::set_exchange::{
    self as ids, CreatePackArgs, CreatedPack, InstallPackArgs, InstalledPack, PackJob, PackStatus,
};

use super::*;

/// How a pack or unpack ended.
enum Done {
    Created(CreatedPack),
    Installed(InstalledPack),
}

/// A pack or unpack on its thread.
struct Running {
    job: PackJob,
    finished: Receiver<Result<Done, pack::Error>>,
    cancel: Arc<AtomicBool>,
    /// The share done, as the bits of an `f64`.
    share: Arc<AtomicU64>,
}

/// The pack or unpack in the shell.
#[derive(Default)]
pub(super) struct PackState {
    running: Option<Running>,
}

impl Shell {
    /// Runs the pack commands (R24). Creating and installing wait for the end
    /// of the work.
    // shortcut: an agent's call blocks the thread that runs it until the pack
    // ends, when a pack of many gigabytes should return at once and be polled.
    pub(super) fn run_pack(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::CREATE_PACK => {
                self.start_create_pack(args)?;
                self.wait_pack().expect("a pack that started ends")
            }
            ids::INSTALL_PACK => {
                self.start_install_pack(args)?;
                self.wait_pack().expect("an unpack that started ends")
            }
            ids::CANCEL_PACK => {
                self.cancel_pack();
                Ok(to_value(&self.pack_status()))
            }
            ids::PACK_STATUS => Ok(to_value(&self.pack_status())),
            other => Err(ShellError::UnknownCommand(other.to_owned())),
        }
    }

    /// What the pack or unpack under way is doing, and how far it has got.
    #[must_use]
    pub fn pack_status(&self) -> PackStatus {
        self.packs
            .running
            .as_ref()
            .map_or_else(PackStatus::default, |running| PackStatus {
                job: Some(running.job),
                share: f64::from_bits(running.share.load(Ordering::Relaxed)),
            })
    }

    /// Stops the pack or unpack under way, if any (R23); its thread removes
    /// what it wrote, and the Status Bar says it was cancelled.
    pub fn cancel_pack(&mut self) {
        if let Some(running) = &self.packs.running {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Cancels the pack or unpack under way and waits until its thread has
    /// removed what it wrote: for quitting.
    pub fn stop_pack(&mut self) {
        if let Some(running) = self.packs.running.take() {
            running.cancel.store(true, Ordering::Relaxed);
            let _ = running.finished.recv();
        }
    }

    /// The project folder a pack is made of: `project`, or the open set's.
    /// Refused while the open set has unsaved changes or was never saved,
    /// when its project is the one packed (R18), and for a folder that holds
    /// no set.
    pub(super) fn pack_root(&self, project: Option<&str>) -> Result<PathBuf, ShellError> {
        let unavailable = |reason: String| ShellError::Unavailable {
            command: ids::CREATE_PACK.to_owned(),
            reason,
        };
        let open = self.set_project_folder();
        let root = match project {
            Some(project) => std::path::absolute(project).map_err(|e| ShellError::InvalidArgs {
                command: ids::CREATE_PACK.to_owned(),
                message: e.to_string(),
            })?,
            None => open.clone().ok_or_else(|| {
                unavailable("the open set has never been saved: save it first".to_owned())
            })?,
        };
        if open.as_deref().is_some_and(|open| same_folder(open, &root)) && self.dirty {
            return Err(unavailable(
                "the open set has unsaved changes: save it first".to_owned(),
            ));
        }
        if !self.is_project_folder(&root) {
            return Err(unavailable(format!(
                "{} isn't a project folder: it holds no set",
                root.display()
            )));
        }
        Ok(root)
    }

    /// Starts `set.create_pack` on its thread and returns at once; the end
    /// lands in the Status Bar, and [`Shell::wait_pack`] gives the result.
    ///
    /// # Errors
    ///
    /// [`ShellError`] for arguments that don't fit, a project with unsaved
    /// changes (R18), or a pack or unpack already under way.
    pub fn start_create_pack(&mut self, args: Value) -> Result<(), ShellError> {
        let id = ids::CREATE_PACK;
        let CreatePackArgs { project, path } = Self::args(id, args)?;
        let path = path.ok_or_else(|| ShellError::InvalidArgs {
            command: id.to_owned(),
            message: "`path` is needed: where to write the pack".to_owned(),
        })?;
        let root = self.pack_root(project.as_deref())?;
        let mut path = std::path::absolute(path).map_err(|e| ShellError::InvalidArgs {
            command: id.to_owned(),
            message: e.to_string(),
        })?;
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case(pack::EXTENSION))
        {
            let mut named = path.into_os_string();
            named.push(format!(".{}", pack::EXTENSION));
            path = named.into();
        }
        let left_out: Vec<String> = self
            .project_file_rows(&root)
            .into_iter()
            .filter(|row| matches!(row.location, Location::External | Location::UserLibrary))
            .map(|row| row.path)
            .collect();
        self.start_pack(id, PackJob::Create, move |cancel, progress| {
            let packed = pack::create(&root, &path, cancel, progress)?;
            Ok(Done::Created(CreatedPack {
                path: packed.path.to_string_lossy().into_owned(),
                files: packed.files,
                bytes: packed.bytes,
                left_out,
            }))
        })
    }

    /// Starts `set.install_pack` on its thread and returns at once, as
    /// [`Shell::start_create_pack`] does.
    ///
    /// # Errors
    ///
    /// [`ShellError`] for arguments that don't fit, or a pack or unpack
    /// already under way.
    pub fn start_install_pack(&mut self, args: Value) -> Result<(), ShellError> {
        let id = ids::INSTALL_PACK;
        let InstallPackArgs { pack, folder } = Self::args(id, args)?;
        let invalid = |message: &str| ShellError::InvalidArgs {
            command: id.to_owned(),
            message: message.to_owned(),
        };
        let absolute =
            |path: String| std::path::absolute(path).map_err(|e| invalid(&e.to_string()));
        let pack_file =
            absolute(pack.ok_or_else(|| invalid("`pack` is needed: the pack to unpack"))?)?;
        let parent = absolute(
            folder
                .ok_or_else(|| invalid("`folder` is needed: where to make the project folder"))?,
        )?;
        self.start_pack(id, PackJob::Install, move |cancel, progress| {
            let unpacked = pack::install(&pack_file, &parent, cancel, progress)?;
            Ok(Done::Installed(InstalledPack {
                folder: unpacked.folder.to_string_lossy().into_owned(),
                sets: unpacked
                    .sets
                    .iter()
                    .map(|set| set.to_string_lossy().into_owned())
                    .collect(),
                files: unpacked.files,
                bytes: unpacked.bytes,
            }))
        })
    }

    fn start_pack(
        &mut self,
        id: &str,
        job: PackJob,
        work: impl FnOnce(&AtomicBool, &mut dyn FnMut(f64)) -> Result<Done, pack::Error>
        + Send
        + 'static,
    ) -> Result<(), ShellError> {
        if self.packs.running.is_some() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "a pack or unpack is under way".to_owned(),
            });
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let share = Arc::new(AtomicU64::new(0f64.to_bits()));
        let (send, finished) = mpsc::channel();
        let (thread_cancel, thread_share) = (Arc::clone(&cancel), Arc::clone(&share));
        std::thread::Builder::new()
            .name("bl-shell-pack".to_owned())
            .spawn(move || {
                let mut progress =
                    |done: f64| thread_share.store(done.to_bits(), Ordering::Relaxed);
                let _ = send.send(work(&thread_cancel, &mut progress));
            })
            .map_err(|e| ShellError::Failed {
                command: id.to_owned(),
                message: format!("couldn't start the work: {e}"),
            })?;
        self.packs.running = Some(Running {
            job,
            finished,
            cancel,
            share,
        });
        self.touch();
        Ok(())
    }

    /// Waits for the pack or unpack under way to end, and gives its result;
    /// `None` when none is going. For tools and tests; the app never waits.
    pub fn wait_pack(&mut self) -> Option<Result<Value, ShellError>> {
        let running = self.packs.running.take()?;
        let ended = running.finished.recv().unwrap_or_else(|_| {
            Err(pack::Error::Refused(
                "the work stopped unexpectedly".to_owned(),
            ))
        });
        Some(self.pack_ended(running.job, ended))
    }

    /// Takes the pack's or unpack's news: the Status Bar says how far it has
    /// got, and when it has ended, where it went or why it failed (R16, R20).
    /// Returns whether it is still going, so the progress window keeps
    /// drawing.
    pub(super) fn poll_pack(&mut self) -> bool {
        let Some(running) = &self.packs.running else {
            return false;
        };
        let job = running.job;
        match running.finished.try_recv() {
            Err(TryRecvError::Empty) => return true,
            Ok(ended) => {
                self.packs.running = None;
                let _ = self.pack_ended(job, ended);
            }
            Err(TryRecvError::Disconnected) => {
                self.packs.running = None;
                let stopped = Err(pack::Error::Refused(
                    "the work stopped unexpectedly".to_owned(),
                ));
                let _ = self.pack_ended(job, stopped);
            }
        }
        false
    }

    fn pack_ended(
        &mut self,
        job: PackJob,
        ended: Result<Done, pack::Error>,
    ) -> Result<Value, ShellError> {
        let command = match job {
            PackJob::Create => ids::CREATE_PACK,
            PackJob::Install => ids::INSTALL_PACK,
        };
        let name = |path: &str| {
            Path::new(path)
                .file_name()
                .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
        };
        let result = match ended {
            Ok(Done::Created(created)) => {
                let mut text = format!("Created the pack {}", created.path);
                if !created.left_out.is_empty() {
                    text.push_str(&format!(
                        "; {} file{} used from outside the project {} left out",
                        created.left_out.len(),
                        if created.left_out.len() == 1 { "" } else { "s" },
                        if created.left_out.len() == 1 {
                            "was"
                        } else {
                            "were"
                        },
                    ));
                }
                self.set_message(text, false);
                Ok(to_value(&created))
            }
            Ok(Done::Installed(installed)) => {
                let sets: Vec<String> = installed.sets.iter().map(|set| name(set)).collect();
                self.set_message(
                    format!(
                        "Installed the pack as {} with {}",
                        name(&installed.folder),
                        sets.join(", ")
                    ),
                    false,
                );
                Ok(to_value(&installed))
            }
            Err(pack::Error::Cancelled) => {
                self.set_message(
                    match job {
                        PackJob::Create => "The pack was cancelled.",
                        PackJob::Install => "The unpack was cancelled.",
                    },
                    false,
                );
                Err(ShellError::Failed {
                    command: command.to_owned(),
                    message: "cancelled".to_owned(),
                })
            }
            Err(error) => {
                let error = ShellError::Failed {
                    command: command.to_owned(),
                    message: error.to_string(),
                };
                self.set_message(error.to_string(), true);
                Err(error)
            }
        };
        self.touch();
        result
    }
}

/// Whether two paths are the same folder, even through links.
fn same_folder(a: &Path, b: &Path) -> bool {
    a == b
        || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}
