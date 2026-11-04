//! Impulse-response slots (WP-8.30; the impulse responses spec): loading a
//! response into a device's slot, clearing it, and the chooser.

use super::{Shell, ShellError, file_stem, to_value};
use bl_devices::impulse;
use bl_model::{Command, ContentHash, Device, DeviceId};
use bl_shell_spec::devices::{
    self as device_ids, ImpulseCategory, ImpulseChoice, ImpulseResponses, ImpulseSlot, ImpulseState,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The User Library's folder of the person's own responses (R11).
const USER_FOLDER: &str = "Impulse Responses";
/// The bundled folder's name in the app's resources.
const BUNDLED_FOLDER: &str = "Impulse Responses";
/// The chooser's own category for the person's files (R10, R11).
const USER: &str = "User";
/// The bundled categories in the chooser's order (R17); any other folder of
/// the bundled folder follows, by name.
const CATEGORIES: [&str; 8] = [
    "Early Reflections",
    "Chambers and Large Rooms",
    "Made for Drums",
    "Halls",
    "Plates",
    "Springs",
    "Bigger Spaces",
    "Textures",
];

impl Shell {
    /// Uses `folder` as the app's bundled impulse-response folder, instead
    /// of the one in its resources.
    pub fn use_bundled_impulse_responses(&mut self, folder: Option<PathBuf>) {
        self.bundled_impulses = folder;
    }

    fn bundled_impulses(&self) -> Option<PathBuf> {
        self.bundled_impulses
            .clone()
            .or_else(|| crate::resources::folder().map(|f| f.join(BUNDLED_FOLDER)))
    }

    /// The names of the impulse-response slots `device` has: none for a
    /// device of a type without any.
    pub(super) fn impulse_slot_names(&self, device: &Device) -> Vec<String> {
        self.devices
            .description(&device.kind)
            .map(|d| d.impulse_responses.clone())
            .unwrap_or_default()
    }

    /// Loads the file at `path` into impulse-response slot `slot` of `device`
    /// (R2 to R7): the file is read, hashed, decoded, resampled and prepared
    /// on a loader thread, and the slot names it from then on; one undo step.
    pub(super) fn load_impulse_response(
        &mut self,
        id: &str,
        device: &Device,
        path: &Path,
        slot: Option<u8>,
    ) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let names = self.impulse_slot_names(device);
        let slot = slot.unwrap_or(0);
        if usize::from(slot) >= names.len() {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!(
                    "{} has {} impulse-response slots, counted from 0; there is no slot {slot}",
                    device.id,
                    names.len()
                ),
            });
        }
        let rate = self.engine_rate();
        let file = path.to_path_buf();
        let loader = std::thread::spawn(move || {
            let imported = bl_media::MediaStore::read_import(&file).map_err(|e| e.to_string())?;
            let media = ContentHash(*imported.hash().as_bytes());
            let loaded = impulse::load_cached(media, &file, rate).map_err(|e| e.to_string())?;
            Ok::<_, String>((imported, loaded))
        });
        let (imported, _) = match loader.join() {
            Ok(Ok(loaded)) => loaded,
            Ok(Err(why)) => {
                // R14: the slot stays as it was; the Status Bar says why.
                let why = format!("{}: {why}", path.display());
                self.set_message(why.clone(), true);
                return Err(failed(why));
            }
            Err(_) => return Err(failed("the loader stopped".to_owned())),
        };
        let hash = self.media.record_imported(imported);
        let entry = self
            .media
            .entry(hash)
            .ok_or_else(|| failed("the media store lost the file".to_owned()))?;
        let media = ContentHash(*hash.as_bytes());
        self.store.add_media(media, &entry.path, entry.size);
        self.editor
            .apply(Command::SetImpulseResponse {
                device: device.id,
                slot,
                media: Some(media),
            })
            .map_err(|error| failed(error.to_string()))?;
        self.dirty = true;
        self.document_changed();
        self.touch();
        let reading = self
            .impulse_slots(device.id)
            .into_iter()
            .find(|s| s.slot == slot);
        Ok(to_value(&device_ids::LoadSampleResult {
            device: device.id,
            media,
            slot,
            name: entry
                .path
                .file_name()
                .map_or(String::new(), |n| n.to_string_lossy().into_owned()),
            impulse_response: reading,
        }))
    }

    /// `device.set_sample` on a device with impulse-response slots: a null
    /// `media` clears the slot, one undo step named Clear Impulse Response
    /// (R2). `None` for a device without any, which the model's command
    /// handles.
    pub(super) fn set_impulse_sample(
        &mut self,
        id: &str,
        args: &Value,
    ) -> Option<Result<Value, ShellError>> {
        #[derive(serde::Deserialize)]
        struct Args {
            #[serde(default)]
            device: Option<DeviceId>,
            #[serde(default)]
            media: Option<ContentHash>,
            #[serde(default)]
            slot: u8,
        }
        let Args {
            device,
            media,
            slot,
        } = serde_json::from_value(args.clone()).ok()?;
        let device = device.or_else(|| self.devices.selected())?;
        let target = self.editor.document().device(device)?.clone();
        let names = self.impulse_slot_names(&target);
        if names.is_empty() {
            return None;
        }
        if usize::from(slot) >= names.len() {
            return Some(Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!(
                    "{device} has {} impulse-response slots, counted from 0; there is no slot {slot}",
                    names.len()
                ),
            }));
        }
        let result = self.editor.apply(Command::SetImpulseResponse {
            device,
            slot,
            media,
        });
        Some(match result {
            Ok(_) => {
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(Value::Null)
            }
            Err(error) => Err(ShellError::Failed {
                command: id.to_owned(),
                message: error.to_string(),
            }),
        })
    }

    /// What each impulse-response slot of `device` holds (R12).
    pub fn impulse_slots(&self, device: DeviceId) -> Vec<ImpulseSlot> {
        let Some(device) = self.editor.document().device(device) else {
            return Vec::new();
        };
        let rate = self.engine_rate();
        let bundled = self.bundled_impulses();
        self.impulse_slot_names(device)
            .into_iter()
            .enumerate()
            .map(|(at, name)| {
                let mut slot = ImpulseSlot {
                    name,
                    slot: at as u8,
                    state: ImpulseState::Empty,
                    file: None,
                    path: None,
                    media: None,
                    category: None,
                    response: None,
                    layout: None,
                    file_frames: None,
                    frames: None,
                    gain_db: None,
                };
                let Some(media) = device.media_at(at) else {
                    return slot;
                };
                slot.media = Some(media);
                let entry = self.media.entry(bl_media::ContentHash::from_bytes(media.0));
                let Some(entry) = entry.filter(|e| e.path.exists()) else {
                    slot.state = ImpulseState::Missing;
                    return slot;
                };
                slot.file = entry
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                slot.path = Some(entry.path.to_string_lossy().into_owned());
                let (category, response) = category_of(&entry.path, bundled.as_deref());
                slot.category = Some(category);
                slot.response = Some(response);
                match impulse::cached(media, rate) {
                    Some(loaded) => {
                        slot.state = ImpulseState::Ready;
                        slot.layout = Some(loaded.layout_name().to_owned());
                        slot.file_frames = Some(loaded.file_frames as u64);
                        slot.frames = Some(loaded.frames as u64);
                        slot.gain_db = Some(loaded.gain_db);
                    }
                    None => slot.state = ImpulseState::Preparing,
                }
                slot
            })
            .collect()
    }

    /// `device.impulse_responses` (R10): what each slot holds and the
    /// chooser's categories.
    pub(super) fn run_impulse_responses(&self, id: &str, args: Value) -> Result<Value, ShellError> {
        let device_ids::DeviceArgs { device } = Self::args(id, args)?;
        let device =
            device
                .or_else(|| self.devices.selected())
                .ok_or_else(|| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "select a device first".to_owned(),
                })?;
        let target =
            self.editor
                .document()
                .device(device)
                .ok_or_else(|| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!("no {device}"),
                })?;
        if self.impulse_slot_names(target).is_empty() {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: format!("{device} has no impulse-response slot"),
            });
        }
        Ok(to_value(&ImpulseResponses {
            device,
            slots: self.impulse_slots(device),
            categories: self.impulse_categories(),
        }))
    }

    /// The chooser's categories: the bundled ones, then User (R10, R11).
    fn impulse_categories(&self) -> Vec<ImpulseCategory> {
        let mut categories = Vec::new();
        if let Some(bundled) = self.bundled_impulses() {
            let mut found: Vec<String> = std::fs::read_dir(&bundled)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            found.sort_by_key(|name| {
                (
                    CATEGORIES
                        .iter()
                        .position(|c| c == name)
                        .unwrap_or(CATEGORIES.len()),
                    name.clone(),
                )
            });
            for name in found {
                let responses = audio_files(&bundled.join(&name));
                categories.push(ImpulseCategory { name, responses });
            }
        }
        let mut user = self
            .browser_view
            .user_library()
            .map(|library| audio_files(&library.join(USER_FOLDER)))
            .unwrap_or_default();
        // Responses loaded from anywhere else, by file name, without repeats.
        let bundled = self.bundled_impulses();
        let doc = self.editor.document();
        for device in doc.tracks().flat_map(|t| t.devices_deep()) {
            for slot in 0..self.impulse_slot_names(device).len() {
                let Some(media) = device.media_at(slot) else {
                    continue;
                };
                let Some(entry) = self.media.entry(bl_media::ContentHash::from_bytes(media.0))
                else {
                    continue;
                };
                let file = entry.path.to_string_lossy().into_owned();
                let (category, name) = category_of(&entry.path, bundled.as_deref());
                let listed = user.iter().any(|c| c.file == file || c.name == name);
                if category == USER && !listed {
                    user.push(ImpulseChoice { name, file });
                }
            }
        }
        categories.push(ImpulseCategory {
            name: USER.to_owned(),
            responses: user,
        });
        categories
    }
}

/// The category a file is in and its name there: its folder in the bundled
/// folder, or User with its file name's stem (R11, R12).
fn category_of(path: &Path, bundled: Option<&Path>) -> (String, String) {
    let category = bundled
        .and_then(|folder| path.strip_prefix(folder).ok())
        .filter(|rest| rest.components().count() > 1)
        .and_then(|rest| rest.components().next())
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    (category.unwrap_or_else(|| USER.to_owned()), file_stem(path))
}

/// The audio files in `folder` and its subfolders, by name.
fn audio_files(folder: &Path) -> Vec<ImpulseChoice> {
    let mut found = Vec::new();
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if crate::browser::listing::file_kind(&entry.file_name().to_string_lossy())
                == Some(bl_shell_spec::browser::ItemKind::Audio)
            {
                found.push(ImpulseChoice {
                    name: file_stem(&path),
                    file: path.to_string_lossy().into_owned(),
                });
            }
        }
    }
    found.sort_by(|a, b| (&a.name, &a.file).cmp(&(&b.name, &b.file)));
    found
}
