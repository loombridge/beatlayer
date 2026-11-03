//! Beatlayer's library through the shell (WP-9.46; the core library spec, R5
//! to R16): its packages through the Packages manager, the items they
//! install, and their credits. The format is [`bl_library`]'s, the
//! browser's part is [`crate::browser::library`].

use std::collections::BTreeSet;
use std::path::PathBuf;

use bl_shell_spec::browser::{self as browser_ids, Label, OpenPlaceArgs};
use bl_shell_spec::library::{
    self as ids, Credit, Credits, CreditsArgs, ExportCreditsArgs, ExportedCredits, LibraryState,
    PackageCredits,
};
use bl_shell_spec::stems::JobStatus;

use super::*;
use crate::browser::library::LibraryItem;
use crate::packages::{LIBRARY_ROLE, Package};

/// The library in the shell.
pub(super) struct LibraryShell {
    /// The installed packages are read again at the next poll.
    stale: bool,
    /// The installed packages: their folders and what they hold.
    installed: Vec<(PathBuf, bl_library::Package)>,
    /// Whether the Library Credits window is open.
    credits_open: bool,
}

impl Default for LibraryShell {
    fn default() -> Self {
        Self {
            stale: true,
            installed: Vec::new(),
            credits_open: false,
        }
    }
}

impl Shell {
    /// The folder the library's packages install into, read-only (R7).
    #[must_use]
    pub fn library_folder(&self) -> PathBuf {
        self.packages_folder().with_file_name("Library")
    }

    /// The library's packages of the manifest for this platform (R5).
    fn library_packages(&self) -> Vec<Package> {
        self.manifest()
            .for_platform()
            .into_iter()
            .filter(|p| p.role.starts_with(LIBRARY_ROLE))
            .cloned()
            .collect()
    }

    /// Reads the installed packages again, and tells the browser which
    /// items there are (R8).
    pub(super) fn refresh_library(&mut self) {
        self.library.stale = false;
        let folder = self.packages_folder();
        self.library.installed = self
            .library_packages()
            .iter()
            .filter(|p| p.installed(&folder))
            .filter_map(|p| {
                let dir = p.dir(&folder);
                bl_library::Package::read(&dir).ok().map(|read| (dir, read))
            })
            .collect();
        let items = self
            .library
            .installed
            .iter()
            .flat_map(|(dir, package)| {
                package.items.iter().map(|item| LibraryItem {
                    path: dir.join(&item.file),
                    name: item.name.clone(),
                    label: item.label.clone(),
                    tags: item.tags.clone(),
                    package: package.name.clone(),
                })
            })
            .collect();
        let dir = (!self.library.installed.is_empty()).then(|| self.library_folder());
        self.browser_view.set_library(dir, items);
        self.touch();
    }

    /// How many items an installed package holds.
    pub(super) fn library_items_in(&self, package: &Package) -> Option<u32> {
        let dir = package.dir(&self.packages_folder());
        self.library
            .installed
            .iter()
            .find(|(installed, _)| *installed == dir)
            .map(|(_, p)| u32::try_from(p.items.len()).unwrap_or(u32::MAX))
    }

    /// The library as `library.state` gives it (R13).
    #[must_use]
    pub fn library_state(&self) -> LibraryState {
        let wanted = self.library_packages();
        LibraryState {
            folder: self.library_folder().to_string_lossy().into_owned(),
            installed: !self.library.installed.is_empty(),
            packages: wanted.iter().map(|p| self.package_status(p)).collect(),
            install: if wanted.is_empty() {
                JobStatus::default()
            } else {
                self.packages_job(&wanted)
            },
            credits_open: self.library.credits_open,
        }
    }

    /// Whether the Library Credits window is open (R12).
    #[must_use]
    pub fn library_credits_open(&self) -> bool {
        self.library.credits_open
    }

    /// The files of the library the open set plays (R12, R14): the media
    /// its clips and devices use that lie in the library's folder.
    fn set_library_files(&self) -> BTreeSet<PathBuf> {
        let folder = self.library_folder();
        crate::dawproject::media_used(self.editor.document())
            .into_iter()
            .filter_map(|hash| self.media.entry(bl_media::ContentHash::from_bytes(hash.0)))
            .map(|entry| entry.path)
            .filter(|path| path.starts_with(&folder))
            .collect()
    }

    /// The installed items' credits, or those of the open set's (R11, R12,
    /// R13).
    #[must_use]
    pub fn library_credits(&self, set: bool) -> Credits {
        let used = set.then(|| self.set_library_files());
        let mut packages = Vec::new();
        let mut shown: Vec<(&str, Vec<bl_library::Item>)> = Vec::new();
        for (dir, package) in &self.library.installed {
            let items: Vec<bl_library::Item> = package
                .items
                .iter()
                .filter(|item| {
                    used.as_ref()
                        .is_none_or(|used| used.contains(&dir.join(&item.file)))
                })
                .cloned()
                .collect();
            if items.is_empty() {
                continue;
            }
            packages.push(PackageCredits {
                package: package.id.clone(),
                name: package.name.clone(),
                items: items
                    .iter()
                    .map(|item| Credit {
                        id: item.id.clone(),
                        name: item.name.clone(),
                        credit: item.credit(),
                    })
                    .collect(),
            });
            shown.push((package.name.as_str(), items));
        }
        let text = bl_library::credits_text(shown.iter().map(|(n, i)| (*n, i.as_slice())));
        Credits { packages, text }
    }

    /// The credit of the library item whose file is `path`, for the Info
    /// View and the info of what's loaded from it (R11).
    #[must_use]
    pub fn library_credit_of(&self, path: &std::path::Path) -> Option<String> {
        self.library.installed.iter().find_map(|(dir, package)| {
            let item = package
                .items
                .iter()
                .find(|item| dir.join(&item.file) == path)?;
            Some(item.credit())
        })
    }

    /// Reads the installed packages again if one changed.
    pub(super) fn refresh_stale_library(&mut self) {
        if self.library.stale {
            self.refresh_library();
        }
    }

    /// A package of the library changed: read it again at the next poll.
    pub(super) fn library_changed(&mut self) {
        self.library.stale = true;
    }

    pub(super) fn run_library(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        self.refresh_stale_library();
        match id {
            ids::STATE => {}
            ids::INSTALL => {
                let wanted = self.library_packages();
                self.request_packages(wanted)
                    .map_err(|e| self.failed(id, e))?;
            }
            ids::CANCEL => {
                let wanted = self.library_packages();
                self.cancel_packages(&wanted);
                // The page's Cancel leaves it: back to All.
                if *self.browser_view.label() == Label::CoreLibrary
                    && !self.browser_view.library_installed()
                {
                    let args = to_value(&OpenPlaceArgs { label: Label::All });
                    self.invoke_inner(browser_ids::OPEN_PLACE, args)?;
                }
            }
            ids::CREDITS => {
                let CreditsArgs { set } = Self::args(id, args)?;
                return Ok(to_value(&self.library_credits(set)));
            }
            ids::EXPORT_CREDITS => {
                let ExportCreditsArgs { path, set } = Self::args(id, args)?;
                if path.is_empty() {
                    return Err(self.failed(id, "`path` is empty"));
                }
                let credits = self.library_credits(set);
                let items = credits
                    .packages
                    .iter()
                    .map(|p| p.items.len())
                    .sum::<usize>();
                if items == 0 {
                    return Err(self.failed(
                        id,
                        if set {
                            "the set uses no library items"
                        } else {
                            "no library items are installed"
                        },
                    ));
                }
                std::fs::write(&path, credits.text)
                    .map_err(|e| self.failed(id, format!("couldn't write {path}: {e}")))?;
                return Ok(to_value(&ExportedCredits {
                    path,
                    items: u32::try_from(items).unwrap_or(u32::MAX),
                }));
            }
            ids::SHOW_CREDITS | ids::CLOSE_CREDITS => {
                self.library.credits_open = id == ids::SHOW_CREDITS;
                self.touch();
            }
            _ => return Err(ShellError::UnknownCommand(id.to_owned())),
        }
        Ok(to_value(&self.library_state()))
    }
}
