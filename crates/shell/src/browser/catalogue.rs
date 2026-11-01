//! The browser's lists for a control surface that browses (spec:
//! push-2-tracks R5, R6 and R13): the labels a role is offered, and what each
//! label, vendor folder, collection and preset folder holds.
//!
//! A [`Catalogue`] is a copy made when something the lists come from changes,
//! so a surface reads it on its own thread without waiting for the index or
//! the disk. It lists what the window's browser lists under the same labels
//! (racks R43) and, under Sounds, Audio Effects and MIDI Effects, the User
//! Library's presets of that category, folders first.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_control::browse::{Catalogue as Lists, Entry, Load};
use bl_model::DeviceRole;
use bl_plugin_host::scan::ScannedPlugin;
use bl_shell_spec::browser::{COLLECTIONS, ItemKind, Label};
use bl_shell_spec::devices::NewDevice;

use super::index::Snapshot;
use super::listing::Entry as Listed;
use super::{BrowserDevice, BrowserView, device_by_id, label_devices, vendor_key, vendor_of};

/// The labels a role is offered, in the browser's order, each with the role
/// of what it holds (none for every role).
const LABELS: [(Label, Option<DeviceRole>); 6] = [
    (Label::Sounds, Some(DeviceRole::Instrument)),
    (Label::Drums, Some(DeviceRole::Instrument)),
    (Label::Instruments, Some(DeviceRole::Instrument)),
    (Label::AudioEffects, Some(DeviceRole::AudioEffect)),
    (Label::MidiEffects, Some(DeviceRole::MidiEffect)),
    (Label::PlugIns, None),
];

/// What the User Library keeps presets under, by role (`Presets/<category>`).
const fn category(role: DeviceRole) -> &'static str {
    match role {
        DeviceRole::Instrument => "Instruments",
        DeviceRole::AudioEffect => "Audio Effects",
        DeviceRole::MidiEffect => "MIDI Effects",
    }
}

/// The role a preset's folder says it is for: the category after `Presets`.
fn preset_role(path: &Path) -> Option<DeviceRole> {
    let mut parts = path.components().map(|c| c.as_os_str().to_string_lossy());
    parts.by_ref().find(|part| part == "Presets")?;
    let category = parts.next()?;
    [
        DeviceRole::Instrument,
        DeviceRole::AudioEffect,
        DeviceRole::MidiEffect,
    ]
    .into_iter()
    .find(|role| self::category(*role) == category)
}

fn folder(key: String, name: String) -> Entry {
    Entry {
        key,
        name,
        load: None,
        folder: true,
    }
}

fn device(device: BrowserDevice) -> Entry {
    let new = NewDevice {
        kind: device.kind,
        name: device.name.clone(),
        role: device.role,
        sidechain: device.sidechain,
        midi_sidechain: device.midi_sidechain,
    };
    Entry {
        key: device.id,
        name: device.name,
        load: Some(Load::Device {
            new_device: serde_json::to_value(new).unwrap_or_default(),
            role: device.role,
        }),
        folder: false,
    }
}

fn plugin(plugin: &ScannedPlugin) -> Entry {
    Entry {
        key: super::plugin_key(plugin),
        name: plugin.name.clone(),
        load: Some(Load::Plugin {
            path: plugin.path.to_string_lossy().into_owned(),
            plugin: plugin.plugin_id.clone(),
            role: plugin.role,
        }),
        folder: false,
    }
}

/// A file or folder of the places as an entry, if the surface can use it: a
/// folder, or a preset.
fn listed(path: PathBuf, kind: ItemKind) -> Option<Entry> {
    let name = path.file_stem()?.to_string_lossy().into_owned();
    match kind {
        ItemKind::Folder => Some(folder(
            path.to_string_lossy().into_owned(),
            path.file_name()?.to_string_lossy().into_owned(),
        )),
        ItemKind::Preset => Some(Entry {
            key: path.to_string_lossy().into_owned(),
            name,
            load: Some(Load::Preset {
                role: preset_role(&path),
                path: path.to_string_lossy().into_owned(),
            }),
            folder: false,
        }),
        _ => None,
    }
}

/// The lists as of one moment.
#[derive(Debug)]
pub struct Catalogue {
    snapshot: Arc<Snapshot>,
    user_library: Option<PathBuf>,
    plugins: Vec<ScannedPlugin>,
    /// What each of the seven collections holds.
    collections: Vec<Vec<Entry>>,
}

impl BrowserView {
    /// The lists a surface that browses reads; copied from what the browser
    /// holds now.
    #[must_use]
    pub fn catalogue(&self) -> Catalogue {
        let by_key: std::collections::HashMap<String, &ScannedPlugin> =
            self.plugins.iter().map(|p| (p.key(), p)).collect();
        let collections = (1..=COLLECTIONS.len() as u8)
            .map(|number| {
                self.collections
                    .members(number)
                    .into_iter()
                    .filter_map(|key| {
                        if let Some(found) = by_key.get(&key) {
                            Some(plugin(found))
                        } else if let Some(found) = device_by_id(&key) {
                            Some(device(found))
                        } else {
                            let Listed { path, kind, .. } = Self::collected_entry(&key)?;
                            listed(path, kind).filter(|entry| entry.load.is_some())
                        }
                    })
                    .collect()
            })
            .collect();
        Catalogue {
            snapshot: Arc::clone(self.index.snapshot()),
            user_library: self.user_library.clone(),
            plugins: self.plugins.clone(),
            collections,
        }
    }

    /// What decides whether [`BrowserView::catalogue`] would differ: it
    /// changes when the plug-ins, the collections, the User Library or the
    /// index change.
    #[must_use]
    pub fn catalogue_key(&self) -> (u64, usize, Option<PathBuf>) {
        (
            self.generation,
            Arc::as_ptr(self.index.snapshot()) as usize,
            self.user_library.clone(),
        )
    }
}

impl Catalogue {
    /// The presets and folders directly in the User Library's presets of
    /// `role`'s category, or in `folder`.
    fn files_in(&self, folder: &Path) -> Vec<Entry> {
        self.snapshot
            .in_folder(folder)
            .into_iter()
            .filter_map(|index| listed(self.snapshot.path(index), self.snapshot.entry(index).kind))
            .collect()
    }

    /// Plug-ins of `role` (all of them for none), filed under their vendors
    /// as the Plug-Ins label files them.
    fn plugins_under(&self, vendor: Option<&str>, role: Option<DeviceRole>) -> Vec<Entry> {
        let wanted = |p: &&ScannedPlugin| role.is_none_or(|role| p.role == role);
        match vendor {
            Some(vendor) => self
                .plugins
                .iter()
                .filter(wanted)
                .filter(|p| vendor_of(p) == Some(vendor))
                .map(plugin)
                .collect(),
            None => {
                let mut vendors: Vec<&str> = self
                    .plugins
                    .iter()
                    .filter(wanted)
                    .filter_map(vendor_of)
                    .collect();
                vendors.sort_by_key(|v| v.to_lowercase());
                vendors.dedup();
                let mut entries: Vec<Entry> = vendors
                    .into_iter()
                    .map(|v| folder(vendor_key(v), v.to_owned()))
                    .collect();
                entries.extend(
                    self.plugins
                        .iter()
                        .filter(wanted)
                        .filter(|p| vendor_of(p).is_none())
                        .map(plugin),
                );
                entries
            }
        }
    }

    /// Keeps what a role can take: folders, and entries whose role is that
    /// one or isn't known.
    fn fit(entries: Vec<Entry>, role: Option<DeviceRole>) -> Vec<Entry> {
        let mut entries: Vec<Entry> = entries
            .into_iter()
            .filter(|e| {
                e.load
                    .as_ref()
                    .and_then(Load::role)
                    .is_none_or(|own| role.is_none_or(|role| own == role))
            })
            .collect();
        // Folders first, as the browser lists them; the rest keep their order.
        entries.sort_by_key(|e| !e.folder);
        entries
    }
}

impl Lists for Catalogue {
    fn labels(&self, role: Option<DeviceRole>) -> Vec<Entry> {
        let labels = LABELS
            .iter()
            .filter(|(_, own)| role.is_none() || own.is_none() || *own == role)
            .map(|(label, _)| folder(label.key(), label.name()));
        let collections = COLLECTIONS
            .iter()
            .enumerate()
            .map(|(n, name)| folder(format!("collection:{}", n + 1), (*name).to_owned()));
        labels.chain(collections).collect()
    }

    fn children(&self, key: &str, role: Option<DeviceRole>) -> Vec<Entry> {
        let presets = |role: DeviceRole| {
            self.user_library
                .as_ref()
                .map(|library| library.join("Presets").join(category(role)))
                .map_or_else(Vec::new, |folder| self.files_in(&folder))
        };
        let entries = if let Some(number) = key.strip_prefix("collection:") {
            let at = number.parse::<usize>().unwrap_or(0).wrapping_sub(1);
            self.collections.get(at).cloned().unwrap_or_default()
        } else if let Some(vendor) = key.strip_prefix(super::PLUGIN_FOLDER) {
            self.plugins_under(Some(vendor), role)
        } else if key == Label::PlugIns.key() {
            self.plugins_under(None, role)
        } else if key == Label::Sounds.key() {
            presets(DeviceRole::Instrument)
        } else if key == Label::Drums.key() || key == Label::Instruments.key() {
            let label = if key == Label::Drums.key() {
                Label::Drums
            } else {
                Label::Instruments
            };
            label_devices(&label).into_iter().map(device).collect()
        } else if key == Label::AudioEffects.key() || key == Label::MidiEffects.key() {
            let (label, own) = if key == Label::AudioEffects.key() {
                (Label::AudioEffects, DeviceRole::AudioEffect)
            } else {
                (Label::MidiEffects, DeviceRole::MidiEffect)
            };
            let mut entries = label_devices(&label)
                .into_iter()
                .map(device)
                .collect::<Vec<_>>();
            entries.extend(presets(own));
            entries
        } else {
            self.files_in(Path::new(key))
        };
        Self::fit(entries, role)
    }

    fn samples(&self) -> Vec<Entry> {
        let snapshot = &self.snapshot;
        let mut entries: Vec<Entry> = (0..u32::try_from(snapshot.len()).unwrap_or(u32::MAX))
            .filter(|&index| snapshot.entry(index).kind == ItemKind::Audio)
            .filter_map(|index| {
                let path = snapshot.path(index);
                Some(Entry {
                    key: path.to_string_lossy().into_owned(),
                    name: path.file_stem()?.to_string_lossy().into_owned(),
                    load: Some(Load::Sample {
                        path: path.to_string_lossy().into_owned(),
                    }),
                    folder: false,
                })
            })
            .collect();
        entries.sort_by_key(|entry| entry.name.to_lowercase());
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: push-2-tracks R5. A preset's folder says which role it is for.
    #[test]
    fn a_presets_category_is_its_role() {
        let path = Path::new("/lib/Presets/Audio Effects/Utility/a.blpreset");
        assert_eq!(preset_role(path), Some(DeviceRole::AudioEffect));
        let path = Path::new("/lib/Presets/Instruments/Synth/a.blpreset");
        assert_eq!(preset_role(path), Some(DeviceRole::Instrument));
        assert_eq!(preset_role(Path::new("/elsewhere/a.blpreset")), None);
    }

    // Spec: push-2-tracks R5. A role is offered its own labels and the
    // plug-ins; everything is offered all of them, with the collections after.
    #[test]
    fn labels_follow_the_role() {
        let empty = Catalogue {
            snapshot: Arc::default(),
            user_library: None,
            plugins: Vec::new(),
            collections: vec![Vec::new(); 7],
        };
        let names = |role| {
            empty
                .labels(role)
                .into_iter()
                .map(|e| e.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            &names(None)[..6],
            [
                "Sounds",
                "Drums",
                "Instruments",
                "Audio Effects",
                "MIDI Effects",
                "Plug-Ins"
            ]
        );
        assert_eq!(names(None).len(), 13);
        assert_eq!(
            &names(Some(DeviceRole::AudioEffect))[..2],
            ["Audio Effects", "Plug-Ins"]
        );
        assert_eq!(
            &names(Some(DeviceRole::Instrument))[..4],
            ["Sounds", "Drums", "Instruments", "Plug-Ins"]
        );
    }
}
