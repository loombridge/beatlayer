//! Similar sample swapping (WP-9.10; sound similarity R20 to R29, drum
//! sampler R26 to R28, drum racks R30 and R31): Simpler's, Drum Sampler's
//! and a Drum Rack pad's sample is exchanged for the next or previous in the
//! list of samples most like its reference, or the reference is loaded
//! back.
//!
//! A swap is one undo step of document commands: the sample (which clears
//! the region, the manual slices and the edits, as any other sample does),
//! the reference, and a pad chain's name when it was the old sample's. Every
//! parameter, the pad's notes, choke group and mixer are left as they are.

use std::path::Path;

use bl_model::{Command, ContentHash, DeviceId};
use bl_shell_spec::devices::{
    self as ids, SimilarState, SwapDirection, SwapSimilarArgs, SwapTarget, Swapped,
};

use super::*;

impl Shell {
    pub(super) fn run_sample_swap(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::SWAP_SIMILAR => {
                let args: SwapSimilarArgs = Self::args(id, args)?;
                self.swap_similar(id, args.target(), Some(args.direction))
            }
            ids::RETURN_TO_REFERENCE => {
                let target: SwapTarget = Self::args(id, args)?;
                self.swap_similar(id, target, None)
            }
            _ => {
                let target: SwapTarget = Self::args(id, args)?;
                let device = self.swap_device(id, target)?;
                Ok(to_value(&self.similar_state(device)))
            }
        }
    }

    /// Whether a key or menu item for `id` has a sampler to act on (R24):
    /// the device or pad `args` name, else the selected device or pad. A
    /// press of a key with none falls through to what else it does, such as
    /// Cmd-Up and Cmd-Down moving the pads by a row.
    pub(super) fn can_swap(&self, args: &Value) -> bool {
        let device = |name: &str| {
            args.get(name)
                .cloned()
                .and_then(|v| serde_json::from_value::<DeviceId>(v).ok())
        };
        let target = SwapTarget {
            device: device("device"),
            rack: device("rack"),
            note: args
                .get("note")
                .and_then(Value::as_u64)
                .and_then(|note| u8::try_from(note).ok()),
        };
        self.devices
            .swap_device(self.editor.document(), target)
            .ok()
            .and_then(|device| self.editor.document().device(device))
            .is_some_and(|device| device.media_at(0).is_some())
    }

    /// Where the sampler of the pad `note` of the drum rack `rack` stands in
    /// its reference's list, if the pad's one chain plays a Simpler or a Drum
    /// Sampler: what the arrows over the pad are enabled by (R26, R27).
    #[must_use]
    pub fn pad_similar_state(&self, rack: DeviceId, note: u8) -> Option<SimilarState> {
        let target = SwapTarget {
            device: None,
            rack: Some(rack),
            note: Some(note),
        };
        let device = self
            .devices
            .swap_device(self.editor.document(), target)
            .ok()?;
        Some(self.similar_state(device))
    }

    /// The sampler a swap command acts on.
    fn swap_device(&self, id: &str, target: SwapTarget) -> Result<DeviceId, ShellError> {
        self.devices
            .swap_device(self.editor.document(), target)
            .map_err(|error| device_error(id, error))
    }

    /// [`Shell::similar_state`] for a Simpler or a Drum Sampler; `None` for
    /// any other device (R20).
    #[must_use]
    pub fn sampler_similar_state(&self, device: DeviceId) -> Option<SimilarState> {
        let swaps = matches!(
            &self.editor.document().device(device)?.kind,
            bl_model::DeviceKind::Native { device_type } if bl_devices::swaps_similar(device_type)
        );
        swaps.then(|| self.similar_state(device))
    }

    /// Where `device`'s sample stands among the samples like its reference:
    /// what the swap buttons and the menu items are enabled by, with the
    /// reason when they aren't (R27, R28).
    #[must_use]
    pub fn similar_state(&self, device: DeviceId) -> SimilarState {
        let held = self.editor.document().device(device);
        let sample = held.and_then(|d| d.media_at(0));
        let reference = held.and_then(|d| d.similarity_reference());
        let mut state = SimilarState {
            device,
            sample,
            reference,
            position: None,
            count: None,
            reference_path: None,
            can_previous: false,
            can_next: false,
            can_return: false,
            reason: None,
        };
        let Some(reference) = reference else {
            state.reason = Some("it holds no sample".to_owned());
            return state;
        };
        let list = match self.browser_view.swap_list(reference) {
            Ok(list) if list.items.len() > 1 => list,
            Ok(_) => {
                state.reason = Some("its reference has no similar samples".to_owned());
                return state;
            }
            Err(reason) => {
                state.reason = Some(reason);
                return state;
            }
        };
        let position = sample.and_then(|sample| list.position(sample));
        state.reference_path = Some(list.items[0].path.to_string_lossy().into_owned());
        state.count = Some(list.items.len() as u32);
        state.position = position.map(|at| at as u32);
        state.can_return = sample != Some(reference);
        match position {
            Some(at) => {
                state.can_previous = at > 0;
                state.can_next = at + 1 < list.items.len();
            }
            None => {
                state.reason = Some(
                    "its sample isn't in its reference's similar list; Return to Reference loads \
                     the reference"
                        .to_owned(),
                );
            }
        }
        state
    }

    /// `device.swap_similar` (`direction` given) and
    /// `device.return_to_reference` (none).
    fn swap_similar(
        &mut self,
        id: &str,
        target: SwapTarget,
        direction: Option<SwapDirection>,
    ) -> Result<Value, ShellError> {
        let device = self.swap_device(id, target)?;
        let unavailable = |reason: String| ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        };
        let state = self.similar_state(device);
        let reference = state
            .reference
            .ok_or_else(|| unavailable("it holds no sample".to_owned()))?;
        let list = self
            .browser_view
            .swap_list(reference)
            .map_err(unavailable)?;
        let (at, label) = match direction {
            None => {
                if !state.can_return {
                    return Err(unavailable("it holds its reference already".to_owned()));
                }
                (0, "Return to Reference")
            }
            Some(direction) => {
                if list.items.len() < 2 {
                    return Err(unavailable(
                        "its reference has no similar samples".to_owned(),
                    ));
                }
                let position = state
                    .position
                    .ok_or_else(|| unavailable(state.reason.clone().unwrap_or_default()))?
                    as usize;
                let at = match direction {
                    SwapDirection::Next if state.can_next => position + 1,
                    SwapDirection::Previous if state.can_previous => position - 1,
                    SwapDirection::Next => {
                        return Err(unavailable("its sample is the last similar one".to_owned()));
                    }
                    SwapDirection::Previous => {
                        return Err(unavailable(
                            "its sample is the reference, the first of its similar list".to_owned(),
                        ));
                    }
                };
                (at, "Swap Sample")
            }
        };
        let item = &list.items[at];
        let swapped = self.load_swap(id, device, &item.path, reference, label)?;
        Ok(to_value(&Swapped {
            position: at as u32,
            count: list.items.len() as u32,
            ..swapped
        }))
    }

    /// Puts the sample of the file `path` in `device` and keeps `reference`
    /// as its reference, as one undo step named `label` (R21 to R23, R29):
    /// the media store takes the file here, off the audio thread, and the
    /// engine gets the decoded sample as it does for any sample change.
    fn load_swap(
        &mut self,
        id: &str,
        device: DeviceId,
        path: &Path,
        reference: ContentHash,
        label: &'static str,
    ) -> Result<Swapped, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let hash = self
            .media
            .import(path)
            .map_err(|error| failed(format!("{}: {error}", path.display())))?;
        let entry = self
            .media
            .entry(hash)
            .ok_or_else(|| failed("the media store lost the file".to_owned()))?;
        let media = ContentHash(*hash.as_bytes());
        self.store.add_media(media, &entry.path, entry.size);
        let name = file_stem(&entry.path);
        // A pad's chain follows the sample's name only while it was the old
        // sample's (drum racks R31).
        let doc = self.editor.document();
        let was = doc
            .device(device)
            .and_then(|d| d.media_at(0))
            .and_then(|old| self.media.entry(bl_media::ContentHash::from_bytes(old.0)))
            .map(|old| file_stem(&old.path));
        let renamed = doc
            .device_place(device)
            .and_then(|place| place.chain)
            .filter(|&chain| {
                doc.chain(chain)
                    .is_some_and(|chain| was.as_deref() == Some(chain.name.as_str()))
            });
        self.editor
            .transact(label, |editor| {
                editor.apply(Command::SetDeviceSample {
                    device,
                    media: Some(media),
                    slot: 0,
                })?;
                editor.apply(Command::SetSimilarityReference {
                    device,
                    media: Some(reference),
                })?;
                if let Some(chain) = renamed {
                    editor.apply(Command::RenameChain {
                        chain,
                        name: name.clone(),
                    })?;
                }
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|error| failed(error.to_string()))?;
        self.dirty = true;
        self.document_changed();
        self.touch();
        Ok(Swapped {
            device,
            media,
            name,
            position: 0,
            count: 0,
        })
    }
}
