//! A plug-in's presets under its row in the Plug-Ins label (WP-6.66; the
//! plug-in scanning spec, R15 to R20).
//!
//! A preset row's key is its plug-in's key, `#preset/`, and the preset file's
//! path. The Beatlayer presets saved from a plug-in are the `.blpreset` files
//! in the places whose device is that plug-in; an Audio Unit's own are the
//! `.aupreset` files in its folders of the system's and the person's Audio
//! Units presets. VST3 and CLAP list Beatlayer presets only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_model::PluginFormat;
use bl_plugin_host::scan::ScannedPlugin;
use bl_project::preset::{self, Preset};
use bl_shell_spec::browser::{Item, ItemKind};

use super::index::Snapshot;
use super::{BrowserView, plugin_key};

/// What a preset row of a plug-in loads: the file it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginPreset {
    /// A Beatlayer preset saved from the plug-in (`.blpreset`).
    Beatlayer(PathBuf),
    /// An Audio Unit's preset file (`.aupreset`), whose property list is the
    /// unit's state.
    AudioUnit(PathBuf),
}

/// What sits between a plug-in's key and a preset's file in a preset row's
/// key.
const PRESET_MARK: &str = "#preset/";

/// Whether `key` is a plug-in preset's row key.
#[must_use]
pub fn is_preset_key(key: &str) -> bool {
    key.contains(PRESET_MARK)
}

/// The extension of an Audio Unit's preset files.
const AU_PRESET_EXTENSION: &str = "aupreset";

/// The folders of Audio Units presets on this platform: the person's, then
/// the system's.
pub(super) fn default_au_preset_folders() -> Vec<PathBuf> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let mut folders = Vec::new();
    if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        folders.push(Path::new(&home).join("Library/Audio/Presets"));
    }
    folders.push(PathBuf::from("/Library/Audio/Presets"));
    folders
}

/// A plug-in's preset rows, by the plug-in's key, as the browser last built
/// them from the index it held then.
#[derive(Debug)]
pub(super) struct Cache {
    snapshot: Arc<Snapshot>,
    saved: HashMap<(PluginFormat, String), Vec<PathBuf>>,
}

impl BrowserView {
    /// Uses `folders` as the Audio Units presets' folders, each holding a
    /// folder per manufacturer and in it one per plug-in.
    pub fn set_au_preset_folders(&mut self, folders: Vec<PathBuf>) {
        self.au_preset_folders = folders;
        self.rebuild();
    }

    /// The Beatlayer presets saved from each plug-in, by format and
    /// identifier: read once for each index the browser holds.
    // shortcut: every preset file in the places is opened once per index
    // snapshot, when a library holds tens of thousands, cache the answer by
    // file and date.
    fn saved_presets(&mut self) -> &HashMap<(PluginFormat, String), Vec<PathBuf>> {
        let snapshot = self.index.snapshot();
        let stale = self
            .plugin_presets
            .as_ref()
            .is_none_or(|cache| !Arc::ptr_eq(&cache.snapshot, snapshot));
        if stale {
            let mut saved: HashMap<(PluginFormat, String), Vec<PathBuf>> = HashMap::new();
            for index in 0..snapshot.len() as u32 {
                if snapshot.entry(index).kind != ItemKind::Preset {
                    continue;
                }
                let path = snapshot.path(index);
                if let Some(plugin) = Preset::plugin_of(&path) {
                    saved.entry(plugin).or_default().push(path);
                }
            }
            self.plugin_presets = Some(Cache {
                snapshot: Arc::clone(snapshot),
                saved,
            });
        }
        &self
            .plugin_presets
            .as_ref()
            .expect("the cache was just made")
            .saved
    }

    /// The preset files listed under each plug-in's row, by the plug-in's
    /// key, each in the order the row lists them: Beatlayer's by name, then an
    /// Audio Unit's.
    pub(super) fn presets_by_plugin(&mut self) -> HashMap<String, Vec<PluginPreset>> {
        let plugins = self.plugins.clone();
        let au_folders = self.au_preset_folders.clone();
        let saved = self.saved_presets();
        let mut found = HashMap::new();
        for plugin in &plugins {
            let mut presets: Vec<PluginPreset> = saved
                .get(&(plugin.format, plugin.plugin_id.clone()))
                .into_iter()
                .flatten()
                .cloned()
                .map(PluginPreset::Beatlayer)
                .collect();
            presets.sort_by_key(|preset| name_of(preset).to_lowercase());
            if plugin.format == PluginFormat::AudioUnit {
                let mut files = au_preset_files(&au_folders, plugin);
                files.sort_by_key(|file| preset::preset_name(file).to_lowercase());
                presets.extend(files.into_iter().map(PluginPreset::AudioUnit));
            }
            if !presets.is_empty() {
                found.insert(plugin_key(plugin), presets);
            }
        }
        found
    }

    /// The plug-in a preset row's key names and what the row loads, if the
    /// label lists the plug-in and the file is there.
    #[must_use]
    pub fn plugin_preset(&self, key: &str) -> Option<(&ScannedPlugin, PluginPreset)> {
        let (plugin, file) = key.split_once(PRESET_MARK)?;
        let plugin = self.plugin(Path::new(plugin))?;
        let file = PathBuf::from(file);
        if !file.is_file() {
            return None;
        }
        let preset = if plugin.format == PluginFormat::AudioUnit && is_au_preset(&file) {
            PluginPreset::AudioUnit(file)
        } else if preset::is_preset(&file) {
            PluginPreset::Beatlayer(file)
        } else {
            return None;
        };
        Some((plugin, preset))
    }

    /// The plug-in a row's key names, whether the row is the plug-in's or one
    /// of its presets.
    #[must_use]
    pub fn plugin_of_row(&self, key: &str) -> Option<&ScannedPlugin> {
        self.plugin(Path::new(key))
            .or_else(|| self.plugin_preset(key).map(|(plugin, _)| plugin))
    }

    /// The scanned plug-in of this format and identifier, if the label lists
    /// it.
    #[must_use]
    pub fn scanned_plugin(&self, format: PluginFormat, plugin_id: &str) -> Option<&ScannedPlugin> {
        self.plugins
            .iter()
            .find(|p| p.format == format && p.plugin_id == plugin_id)
    }

    /// The selected row's preset, when the selection is one plug-in preset.
    #[must_use]
    pub fn selected_plugin_preset(&self) -> Option<PluginPreset> {
        match self.selected.as_slice() {
            [one] => self.plugin_preset(&one.to_string_lossy()).map(|(_, p)| p),
            _ => None,
        }
    }

    /// The plug-in's row and its presets as the label lists them: the row,
    /// then, while it is open, a row for each preset.
    pub(super) fn plugin_with_presets(
        &self,
        plugin: Item,
        presets: Option<&Vec<PluginPreset>>,
    ) -> Vec<Item> {
        let Some(presets) = presets else {
            return vec![plugin];
        };
        let open = self.open.contains(Path::new(&plugin.path));
        let rows = presets.iter().filter(|_| open).map(|preset| Item {
            path: format!(
                "{}{PRESET_MARK}{}",
                plugin.path,
                match preset {
                    PluginPreset::Beatlayer(file) | PluginPreset::AudioUnit(file) => file.display(),
                }
            ),
            name: name_of(preset),
            kind: ItemKind::Preset,
            depth: plugin.depth + 1,
            open: false,
            folder: None,
            format: None,
            clip_kind: None,
            details: None,
            collections: Vec::new(),
            hidden: false,
            presets: false,
            nested: Vec::new(),
        });
        let shown = Item {
            open,
            presets: true,
            ..plugin.clone()
        };
        std::iter::once(shown).chain(rows).collect()
    }

    /// Opens or closes a plug-in row's presets. Opening one closes the
    /// presets open under another plug-in (spec: plugin-scanning R15), and
    /// leaves the vendors' folders as they are.
    pub(super) fn toggle_plugin_presets(&mut self, key: &Path, open: Option<bool>) {
        if open.unwrap_or(!self.open.contains(key)) {
            let rows: Vec<PathBuf> = self.plugins.iter().map(|p| plugin_key(p).into()).collect();
            self.open.retain(|row| !rows.contains(row));
            self.open.insert(key.to_path_buf());
        } else {
            self.open.remove(key);
        }
        self.rebuild();
    }

    /// The rows to open, in order, to show `plugin`'s presets and which to
    /// select: its vendor's folder if it has one, its own row, and the preset
    /// named `last` if the plug-in has one by that name, else its row (spec:
    /// plugin-scanning R18).
    pub fn reveal_plugin(
        &mut self,
        format: PluginFormat,
        plugin_id: &str,
        last: Option<&str>,
    ) -> Option<Reveal> {
        let plugin = self.scanned_plugin(format, plugin_id)?.clone();
        let key = plugin_key(&plugin);
        let select = self
            .presets_by_plugin()
            .remove(&key)
            .and_then(|presets| {
                presets
                    .into_iter()
                    .find(|preset| Some(name_of(preset).as_str()) == last)
            })
            .map_or_else(
                || key.clone(),
                |preset| match preset {
                    PluginPreset::Beatlayer(file) | PluginPreset::AudioUnit(file) => {
                        format!("{key}{PRESET_MARK}{}", file.display())
                    }
                },
            );
        Some(Reveal {
            vendor: super::vendor_of(&plugin).map(super::vendor_key),
            plugin: key,
            select,
        })
    }
}

/// What to open and select to show a plug-in's presets ([`BrowserView::reveal_plugin`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reveal {
    /// The vendor's folder, if the plug-in names a vendor.
    pub vendor: Option<String>,
    /// The plug-in's row.
    pub plugin: String,
    /// The row to select.
    pub select: String,
}

fn name_of(preset: &PluginPreset) -> String {
    match preset {
        PluginPreset::Beatlayer(file) | PluginPreset::AudioUnit(file) => preset::preset_name(file),
    }
}

fn is_au_preset(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(AU_PRESET_EXTENSION))
}

/// The preset files in `folders` under `plugin`'s manufacturer and name,
/// where Audio Units keep their presets.
fn au_preset_files(folders: &[PathBuf], plugin: &ScannedPlugin) -> Vec<PathBuf> {
    let Some(vendor) = super::vendor_of(plugin) else {
        return Vec::new();
    };
    folders
        .iter()
        .filter_map(|folder| {
            std::fs::read_dir(
                folder
                    .join(preset::file_safe(vendor))
                    .join(preset::file_safe(&plugin.name)),
            )
            .ok()
        })
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_au_preset(path) && path.is_file())
        .collect()
}
