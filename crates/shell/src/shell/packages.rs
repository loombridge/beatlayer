//! The Packages manager through the shell (WP-9.1; the stem separation
//! spec, R15 to R21): the `packages.*` commands, the one installer thread's
//! queue, and what a feature reads of its packages. The manifest and the
//! download itself are in [`crate::packages`].

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use bl_shell_spec::packages::{self as ids, IdsArgs, PackageState, PackageStatus, PackagesStatus};
use bl_shell_spec::stems::{JobState, JobStatus};

use super::*;
use crate::packages::{self, Manifest, Package};
use crate::stems::{Ended, Worker};

/// The role whose library a process loads once (R19).
const ONNX_RUNTIME: &str = "onnx-runtime";

/// The Packages manager's state in the shell.
#[derive(Default)]
pub(super) struct PackagesState {
    /// The download under way: the installer thread's one package.
    running: Option<Running>,
    /// The packages waiting for it, in the order they were asked for.
    queue: VecDeque<Package>,
    /// How each package's last download ended.
    outcomes: HashMap<String, Outcome>,
    /// A manifest a test gave the shell, in place of the run's.
    manifest: Option<Manifest>,
}

struct Running {
    package: Package,
    worker: Worker<()>,
}

enum Outcome {
    Done,
    Cancelled,
    Failed(String),
}

impl Shell {
    /// The folder the packages install into: `Packages` in Beatlayer's
    /// application-data folder, or in the system's temporary folder for a
    /// headless run (R14, R21).
    #[must_use]
    pub fn packages_folder(&self) -> PathBuf {
        packages::folder(self.config_dir.as_deref())
    }

    /// The manifest in use (R21): the run's, unless a test gave this shell
    /// its own with [`Shell::use_manifest`].
    fn manifest(&self) -> &Manifest {
        self.packages
            .manifest
            .as_ref()
            .unwrap_or_else(|| packages::manifest())
    }

    /// Replaces the manifest for this shell alone, for tests that install
    /// from files and local servers.
    pub fn use_manifest(&mut self, manifest: Manifest) {
        self.packages.manifest = Some(manifest);
    }

    /// The packages `feature` needs, as `packages.list` gives them.
    #[must_use]
    pub fn feature_packages(&self, feature: &str) -> Vec<Package> {
        self.manifest()
            .feature_packages(feature, Some(&self.packages_folder()))
    }

    /// Whether any download is going or waiting.
    pub(super) fn packages_busy(&self) -> bool {
        self.packages.running.is_some() || !self.packages.queue.is_empty()
    }

    /// Whether any of `wanted` is downloading or waiting.
    pub(super) fn packages_active(&self, wanted: &[Package]) -> bool {
        let state = &self.packages;
        let is = |p: &Package, q: &Package| p.id == q.id;
        wanted.iter().any(|p| {
            state.running.as_ref().is_some_and(|r| is(&r.package, p))
                || state.queue.iter().any(|q| is(q, p))
        })
    }

    /// One package, as a feature's status or `packages.list` shows it.
    #[must_use]
    pub fn package_status(&self, package: &Package) -> PackageStatus {
        let folder = self.packages_folder();
        let running = self
            .packages
            .running
            .as_ref()
            .filter(|r| r.package.id == package.id);
        let (state, progress) = if let Some(r) = running {
            (PackageState::Downloading, r.worker.progress().fraction())
        } else if self.packages.queue.iter().any(|q| q.id == package.id) {
            (PackageState::Queued, 0.0)
        } else if package.removal_pending(&folder) {
            (PackageState::RemovedAtNextLaunch, 0.0)
        } else if package.installed(&folder) {
            (PackageState::Installed, 0.0)
        } else {
            (PackageState::NotInstalled, 0.0)
        };
        let loaded: Option<&Path> = bl_analysis::stems::onnx::loaded_runtime();
        PackageStatus {
            id: package.id.clone(),
            name: package.name.clone(),
            purpose: package.purpose.clone(),
            version: package.version.clone(),
            url: package.project.clone(),
            size: package.size,
            installed: state == PackageState::Installed,
            role: package.role.clone(),
            state,
            progress,
            // Spec: stem-separation R19. The library in use stays; a
            // package for its role installed after takes over next launch.
            at_next_launch: state == PackageState::Installed
                && package.role == ONNX_RUNTIME
                && loaded.is_some_and(|library| !library.starts_with(package.dir(&folder))),
            features: self.manifest().features_needing(package),
        }
    }

    /// Every package of the manifest for this platform (R20).
    #[must_use]
    pub fn packages_status(&self) -> PackagesStatus {
        PackagesStatus {
            folder: self.packages_folder().to_string_lossy().into_owned(),
            packages: self
                .manifest()
                .for_platform()
                .into_iter()
                .map(|p| self.package_status(p))
                .collect(),
        }
    }

    /// How the download of `wanted` is going, for a feature's status: its
    /// share while any is downloading or waiting, else how the last one
    /// ended.
    pub(super) fn packages_job(&self, wanted: &[Package]) -> JobStatus {
        let state = &self.packages;
        if self.packages_active(wanted) {
            let folder = self.packages_folder();
            let total: u64 = wanted.iter().map(|p| p.size).sum();
            let done: f64 = wanted
                .iter()
                .map(|p| {
                    let share = match &state.running {
                        _ if p.installed(&folder) => 1.0,
                        Some(r) if r.package.id == p.id => {
                            f64::from(r.worker.progress().fraction())
                        }
                        _ => 0.0,
                    };
                    share * p.size as f64
                })
                .sum();
            let stage = match &state.running {
                Some(r) if wanted.iter().any(|p| p.id == r.package.id) => {
                    r.worker.progress().stage()
                }
                Some(r) => format!("Waiting for {}", r.package.name),
                None => String::new(),
            };
            return JobStatus {
                state: JobState::Running,
                progress: (done / total.max(1) as f64) as f32,
                stage,
                error: None,
            };
        }
        let outcomes: Vec<&Outcome> = wanted
            .iter()
            .filter_map(|p| state.outcomes.get(&p.id))
            .collect();
        let failed = outcomes.iter().find_map(|o| match o {
            Outcome::Failed(error) => Some(error.clone()),
            _ => None,
        });
        let (job, progress) = if failed.is_some() {
            (JobState::Failed, 0.0)
        } else if outcomes.iter().any(|o| matches!(o, Outcome::Cancelled)) {
            (JobState::Cancelled, 0.0)
        } else if !outcomes.is_empty() {
            (JobState::Done, 1.0)
        } else {
            (JobState::Idle, 0.0)
        };
        JobStatus {
            state: job,
            progress,
            stage: String::new(),
            error: failed,
        }
    }

    /// Asks for `wanted` to be installed (R17): what's installed, downloading
    /// or waiting isn't asked for again, so two features that need one
    /// package download it once.
    pub(super) fn request_packages(&mut self, wanted: Vec<Package>) -> Result<(), String> {
        let folder = self.packages_folder();
        for package in wanted {
            let known = self.packages_active(std::slice::from_ref(&package));
            if known || package.installed(&folder) {
                continue;
            }
            self.packages.outcomes.remove(&package.id);
            self.packages.queue.push_back(package);
        }
        self.start_next_package()
    }

    fn start_next_package(&mut self) -> Result<(), String> {
        if self.packages.running.is_some() {
            return Ok(());
        }
        let Some(package) = self.packages.queue.pop_front() else {
            return Ok(());
        };
        let worker = match packages::install(self.packages_folder(), package.clone()) {
            Ok(worker) => worker,
            Err(error) => {
                self.packages
                    .outcomes
                    .insert(package.id, Outcome::Failed(error.clone()));
                return Err(error);
            }
        };
        self.packages.running = Some(Running { package, worker });
        self.touch();
        Ok(())
    }

    /// Stops `wanted`: takes them off the queue and cancels the download if
    /// it's one of them.
    pub(super) fn cancel_packages(&mut self, wanted: &[Package]) {
        let state = &mut self.packages;
        state.queue.retain(|q| !wanted.iter().any(|p| p.id == q.id));
        if let Some(r) = state
            .running
            .as_ref()
            .filter(|r| wanted.iter().any(|p| p.id == r.package.id))
        {
            r.worker.progress().cancel();
        }
    }

    /// Takes the installer thread's news and starts the next download.
    /// Returns whether any download is still going or waiting.
    pub(super) fn poll_packages(&mut self) -> bool {
        let ended = self.packages.running.as_mut().and_then(|r| r.worker.poll());
        if let Some(ended) = ended {
            let Running { package, .. } = self.packages.running.take().expect("polled above");
            let outcome = match ended {
                Ended::Done(()) => Outcome::Done,
                Ended::Cancelled => Outcome::Cancelled,
                Ended::Failed(error) => {
                    self.set_message(error.clone(), true);
                    // A failure stops what was waiting behind it.
                    for waiting in std::mem::take(&mut self.packages.queue) {
                        self.packages
                            .outcomes
                            .insert(waiting.id, Outcome::Cancelled);
                    }
                    Outcome::Failed(error)
                }
            };
            self.packages.outcomes.insert(package.id, outcome);
            if let Err(error) = self.start_next_package() {
                self.set_message(error, true);
            }
            self.touch();
        }
        self.packages_busy()
    }

    /// The packages `ids` name, for this platform.
    fn packages_named(&self, command: &str, named: &[String]) -> Result<Vec<Package>, ShellError> {
        named
            .iter()
            .map(|id| {
                self.manifest()
                    .for_platform()
                    .into_iter()
                    .find(|p| p.id == *id)
                    .cloned()
                    .ok_or_else(|| ShellError::InvalidArgs {
                        command: command.to_owned(),
                        message: format!("no package {id} for this platform: see `packages.list`"),
                    })
            })
            .collect()
    }

    pub(super) fn run_packages(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::LIST => {}
            ids::INSTALL => {
                let IdsArgs { ids } = Self::args(id, args)?;
                let wanted = self.packages_named(id, &ids)?;
                self.request_packages(wanted)
                    .map_err(|e| self.failed(id, e))?;
            }
            ids::REMOVE => {
                let IdsArgs { ids } = Self::args(id, args)?;
                let wanted = self.packages_named(id, &ids)?;
                if let Some(busy) = wanted
                    .iter()
                    .find(|p| self.packages_active(std::slice::from_ref(p)))
                {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: format!("{} is downloading: cancel it first", busy.name),
                    });
                }
                let folder = self.packages_folder();
                let loaded = bl_analysis::stems::onnx::loaded_runtime();
                for package in &wanted {
                    package
                        .remove(&folder, loaded.filter(|_| package.role == ONNX_RUNTIME))
                        .map_err(|e| self.failed(id, e))?;
                    self.packages.outcomes.remove(&package.id);
                }
                self.touch();
            }
            _ => return Err(ShellError::UnknownCommand(id.to_owned())),
        }
        Ok(to_value(&self.packages_status()))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // Spec: stem-separation R19, R20. A package removed while its library is
    // loaded shows `removed_at_next_launch`, and isn't installed meanwhile.
    // (Nothing here loads a real ONNX Runtime, so the library's own path is
    // given to `remove` as the loaded one.)
    #[test]
    fn a_package_whose_library_is_loaded_shows_removed_at_the_next_launch() {
        let id = format!("loaded-{}", std::process::id());
        let source = std::env::temp_dir().join(format!("{id}.source"));
        std::fs::write(&source, b"a library").unwrap();
        // Its SHA-256 is set below, from the bytes themselves.
        let manifest = json!({ "packages": [{
            "id": id, "name": "Loaded", "version": "1", "project": "https://example.com/",
            "purpose": "A test package", "platform": "any", "role": "test", "unpack": "file",
            "url": format!("file://{}", source.display()), "size": 9, "folder": format!("{id}-1"),
            "sha256": "",
            "files": [{ "name": "lib.bin" }],
        }]});
        let mut shell = Shell::headless();
        let mut manifest: Manifest = Manifest::parse(&manifest.to_string()).unwrap();
        manifest.packages[0].sha256 = crate::packages::sha256_of(b"a library");
        shell.use_manifest(manifest);
        shell
            .run("packages.install", json!({ "ids": [id] }))
            .unwrap();
        while shell.poll_packages() {
            std::thread::yield_now();
        }
        let package = shell.manifest().for_platform()[0].clone();
        let folder = shell.packages_folder();
        let state = |shell: &Shell| shell.package_status(&package).state;
        assert_eq!(state(&shell), PackageState::Installed);
        assert_eq!(
            package.remove(&folder, Some(&package.path(&folder))),
            Ok(false)
        );
        assert_eq!(state(&shell), PackageState::RemovedAtNextLaunch);
        packages::sweep(&folder);
        assert_eq!(state(&shell), PackageState::NotInstalled);
        let _ = std::fs::remove_file(source);
    }
}
