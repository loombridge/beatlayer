//! Edit Info Text: which item a key, a menu item or a command with no text
//! given acts on, and what the Info View's field starts from and runs.
//!
//! Spec: info-view R10, R11, R12.

use super::{Area, Effect, Invocation, MainView, Shell};
use bl_model::{ChainId, ClipId, DeviceId, LocatorId, SceneId, TrackId};
use bl_shell_spec::locator as locator_ids;
use serde_json::{Value, json};

/// `track.set_info_text`.
const TRACK: &str = "track.set_info_text";
/// `clip.set_info_text`.
const CLIP: &str = "clip.set_info_text";
/// `scene.set_info_text`.
const SCENE: &str = "scene.set_info_text";
/// `device.set_info_text`.
const DEVICE: &str = "device.set_info_text";
/// `chain.set_info`.
const CHAIN: &str = "chain.set_info";
/// `rack.set_macro_info_text`.
const MACRO: &str = "rack.set_macro_info_text";

/// An item that carries info text of the user's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfoTarget {
    /// A track.
    Track(TrackId),
    /// A clip, in the Arrangement, a Session slot or a take lane.
    Clip(ClipId),
    /// A scene.
    Scene(SceneId),
    /// A device.
    Device(DeviceId),
    /// A rack's chain.
    Chain(ChainId),
    /// A rack's macro, numbered from 1.
    Macro(DeviceId, u8),
    /// A locator.
    Locator(LocatorId),
}

/// Whether `id` is one of the seven commands that set an item's info text.
pub(super) fn is_info_command(id: &str) -> bool {
    matches!(
        id,
        TRACK | CLIP | SCENE | DEVICE | CHAIN | MACRO | locator_ids::SET_INFO_TEXT
    )
}

/// The name of the argument that holds the text.
fn text_key(id: &str) -> &'static str {
    match id {
        CHAIN | MACRO => "info",
        _ => "info_text",
    }
}

impl Shell {
    /// Whether `args` give the text, so the command sets it rather than
    /// opening the Info View's field.
    pub(super) fn has_info_text(id: &str, args: &Value) -> bool {
        args.get(text_key(id)).is_some()
    }

    /// The item command `id` acts on: the one `args` name, else the
    /// selected one.
    pub(super) fn info_target_of(&self, id: &str, args: &Value) -> Option<InfoTarget> {
        fn named<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Option<T> {
            args.get(key)
                .and_then(|v| serde_json::from_value(v.clone()).ok())
        }
        let target = match id {
            TRACK => InfoTarget::Track(
                named(args, "track")
                    .or_else(|| self.arrangement.selected_tracks().first().copied())?,
            ),
            CLIP => InfoTarget::Clip(named(args, "clip").or_else(|| self.selected_clip())?),
            SCENE => InfoTarget::Scene(
                named(args, "scene").or_else(|| self.session.state().scenes.first().copied())?,
            ),
            DEVICE => {
                InfoTarget::Device(named(args, "device").or_else(|| self.devices.selected())?)
            }
            CHAIN => InfoTarget::Chain(named(args, "chain").or_else(|| {
                self.devices
                    .selected_chains(self.editor.document())
                    .first()
                    .copied()
            })?),
            MACRO => {
                let (rack, number) = match (named(args, "rack"), named(args, "macro_number")) {
                    (Some(rack), Some(number)) => (rack, number),
                    _ => self.devices.macro_selected()?,
                };
                InfoTarget::Macro(rack, number)
            }
            locator_ids::SET_INFO_TEXT => InfoTarget::Locator(
                named(args, "locator")
                    .or_else(|| self.arrangement.selected_locators().first().copied())?,
            ),
            _ => return None,
        };
        self.info_text_of(target).map(|_| target)
    }

    /// The clip the main view has selected: the Arrangement's, or the first
    /// selected Session slot's.
    fn selected_clip(&self) -> Option<ClipId> {
        let doc = self.editor.document();
        if self.main_view == MainView::Session {
            let slot = self.session.first_slot()?;
            return doc.track(slot.track)?.slot(slot.scene).map(|clip| clip.id);
        }
        self.arrangement.selected_clips().first().copied()
    }

    /// The item Edit ▸ Edit Info Text acts on: the selection of the area
    /// with the keyboard focus (M-EDIT-025).
    pub(super) fn info_target_selected(&self) -> Option<InfoTarget> {
        let none = json!({});
        let first = |ids: &[&str]| ids.iter().find_map(|id| self.info_target_of(id, &none));
        if self.focused_area == Area::Detail {
            // A chain list with the focus takes the chain (racks R22).
            return if self.devices.chain_list_focus() {
                first(&[CHAIN, MACRO, DEVICE])
            } else {
                first(&[MACRO, DEVICE, CHAIN])
            };
        }
        match self.main_view {
            MainView::Session => first(&[SCENE, CLIP, TRACK]),
            // A selected title bar's keys come first, as in the contexts.
            MainView::Arrangement if self.arrangement.title_bars_selected() => first(&[TRACK]),
            MainView::Arrangement => first(&[locator_ids::SET_INFO_TEXT, CLIP, TRACK]),
        }
    }

    /// Asks the Info View to take the text for `target`, showing it first
    /// if it's hidden.
    pub(super) fn edit_info(&mut self, target: InfoTarget) {
        self.info_view = true;
        self.effects.push(Effect::EditInfoText(target));
        self.touch();
    }

    /// An item's name and its info text, or `None` if the item is gone.
    #[must_use]
    pub fn info_text_of(&self, target: InfoTarget) -> Option<String> {
        self.info_item(target).map(|(_, text)| text)
    }

    /// The name the Info View shows for an item, and its info text, while
    /// the item has any (R11).
    #[must_use]
    pub fn info_text_shown(&self, target: InfoTarget) -> Option<(String, String)> {
        self.info_item(target).filter(|(_, text)| !text.is_empty())
    }

    /// An item's name and its info text (empty for none), or `None` once
    /// the item is gone.
    #[must_use]
    pub fn info_item(&self, target: InfoTarget) -> Option<(String, String)> {
        let doc = self.editor.document();
        Some(match target {
            InfoTarget::Track(id) => {
                let track = doc.track(id)?;
                (track.name().to_owned(), track.info_text().to_owned())
            }
            InfoTarget::Clip(id) => {
                let clip = doc.clip(id)?;
                (clip.name.clone(), clip.info_text.clone())
            }
            InfoTarget::Scene(id) => {
                let scene = doc.scene(id)?;
                (scene.name.clone(), scene.info_text.clone())
            }
            InfoTarget::Device(id) => {
                let device = doc.device(id)?;
                (device.name.clone(), device.info_text.clone())
            }
            InfoTarget::Chain(id) => {
                let chain = doc.chain(id)?;
                (chain.name.clone(), chain.info.clone())
            }
            InfoTarget::Macro(rack, number) => {
                let device = doc.device(rack)?;
                device.rack_kind()?;
                let setting = device.macros.setting(number);
                let name = if setting.name.is_empty() {
                    format!("Macro {number}")
                } else {
                    setting.name.clone()
                };
                (name, setting.info.clone())
            }
            InfoTarget::Locator(id) => {
                let locator = doc.locator(id)?;
                (locator.name.clone(), locator.info_text.clone())
            }
        })
    }

    /// The invocation that keeps `text` as the info text of `target`.
    #[must_use]
    pub fn info_text_invocation(target: InfoTarget, text: &str) -> Invocation {
        match target {
            InfoTarget::Track(track) => {
                Invocation::new(TRACK, json!({ "track": track, "info_text": text }))
            }
            InfoTarget::Clip(clip) => {
                Invocation::new(CLIP, json!({ "clip": clip, "info_text": text }))
            }
            InfoTarget::Scene(scene) => {
                Invocation::new(SCENE, json!({ "scene": scene, "info_text": text }))
            }
            InfoTarget::Device(device) => {
                Invocation::new(DEVICE, json!({ "device": device, "info_text": text }))
            }
            InfoTarget::Chain(chain) => {
                Invocation::new(CHAIN, json!({ "chain": chain, "info": text }))
            }
            InfoTarget::Macro(rack, number) => Invocation::new(
                MACRO,
                json!({ "rack": rack, "macro_number": number, "info": text }),
            ),
            InfoTarget::Locator(locator) => Invocation::new(
                locator_ids::SET_INFO_TEXT,
                json!({ "locator": locator, "info_text": text }),
            ),
        }
    }
}
