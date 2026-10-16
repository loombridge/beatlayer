//! How a document serialises: a plain tree, checked on the way in.
//!
//! Groups hold their tracks inline, so the saved form mirrors the display
//! order and can't express a track in two places. The derived lookups aren't
//! saved; loading rebuilds them.

use std::collections::BTreeSet;

use bl_core::id::Id;
use bl_core::time::Ticks;
use imbl::{OrdMap, Vector};
use serde::{Deserialize, Serialize};

use crate::automation::{AutomationTarget, Breakpoint, Envelope};
use crate::clip::Clip;
use crate::command::{
    check_chain, check_clip, check_clip_fits, check_crossfader, check_device, check_extent,
    check_mixer, check_send_level,
};
use crate::device::Device;
use crate::document::{Document, Index, Settings};
use crate::error::ModelError;
use crate::ids::{SceneId, TrackId};
use crate::scene::{Scene, default_scenes};
use crate::shared::Shared;
use crate::track::{Mixer, Routing, Track, TrackKind};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DocumentRepr {
    settings: Settings,
    tracks: Vec<TrackRepr>,
    returns: Vec<TrackRepr>,
    main: TrackRepr,
    /// The scenes, in order. Nothing in a set saved before WP-3.1, which
    /// loads with the default a new set opens with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scenes: Option<Vec<Scene>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TrackRepr {
    id: TrackId,
    kind: TrackKind,
    name: String,
    mixer: Mixer,
    /// The sends that aren't at −∞ dB, as `[return track, level]` pairs.
    /// Sets saved before WP-1.6 have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sends: Vec<(TrackId, f64)>,
    /// The In/Out settings, saved only where they differ from a new
    /// track's of its kind. Sets saved before WP-2.19 have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    routing: Option<Routing>,
    /// The track's automation, one entry per automated parameter. Sets
    /// saved before WP-2.12 have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    automation: Vec<AutomationRepr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    devices: Vec<Device>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    clips: Vec<Clip>,
    /// The clips in the track's Session slots. Sets saved before WP-3.1
    /// have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    slots: Vec<SlotRepr>,
    /// The scenes whose slot's Clip Stop button was removed (WP-3.2). Sets
    /// saved before WP-3.2 have none: every slot has one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    removed_stop_buttons: Vec<SceneId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    children: Vec<TrackRepr>,
}

/// Whether an envelope drives its parameter: on unless a hand move turned
/// it off, so a set that overrode none reads as before.
fn on() -> bool {
    true
}

/// For serde's `skip_serializing_if`, which passes a reference.
fn is_on(value: &bool) -> bool {
    *value
}

/// One parameter's automation as the set saves it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct AutomationRepr {
    /// The parameter it drives.
    target: AutomationTarget,
    /// Its envelope's breakpoints, in position order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    points: Vec<Breakpoint>,
    /// Whether the envelope drives the parameter; left out while it does,
    /// so a set that overrode none reads the same as before.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    enabled: bool,
}

/// One saved Session slot that holds a clip.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SlotRepr {
    scene: SceneId,
    clip: Clip,
}

impl From<&Document> for DocumentRepr {
    fn from(doc: &Document) -> Self {
        let track = |id: &TrackId| TrackRepr::new(doc, *id);
        Self {
            settings: doc.settings.clone(),
            tracks: doc.top.iter().map(track).collect(),
            returns: doc.returns.iter().map(track).collect(),
            main: track(&doc.main),
            scenes: Some(doc.scenes().cloned().collect()),
        }
    }
}

impl From<Document> for DocumentRepr {
    fn from(doc: Document) -> Self {
        Self::from(&doc)
    }
}

impl TrackRepr {
    fn new(doc: &Document, id: TrackId) -> Self {
        let track = doc
            .tracks
            .get(&id)
            .expect("every track in the tree is in the track map");
        Self {
            id,
            kind: track.kind,
            name: track.name.clone(),
            mixer: track.mixer,
            sends: track.sends().collect(),
            routing: (track.routing != Routing::new(track.kind)).then(|| track.routing.clone()),
            automation: track
                .automation()
                .map(|(target, automation)| AutomationRepr {
                    target: *target,
                    points: automation.envelope.points().collect(),
                    enabled: automation.enabled,
                })
                .collect(),
            devices: track.devices().cloned().collect(),
            clips: track.clips().cloned().collect(),
            slots: track
                .slots()
                .map(|(scene, clip)| SlotRepr {
                    scene,
                    clip: clip.clone(),
                })
                .collect(),
            removed_stop_buttons: track.removed_stop_buttons().collect(),
            children: track
                .children
                .iter()
                .map(|id| Self::new(doc, *id))
                .collect(),
        }
    }
}

impl TryFrom<DocumentRepr> for Document {
    type Error = ModelError;

    fn try_from(repr: DocumentRepr) -> Result<Self, ModelError> {
        let DocumentRepr {
            settings,
            tracks,
            returns,
            main,
            scenes,
        } = repr;
        let mut loader = Loader {
            tracks: OrdMap::new(),
            index: Index::default(),
            seen: BTreeSet::new(),
        };
        if main.kind != TrackKind::Main {
            return Err(ModelError::MainTrack);
        }
        check_crossfader(settings.crossfader)?;
        let main = loader.add(main, None)?;
        let returns = returns
            .into_iter()
            .map(|track| {
                if track.kind == TrackKind::Return {
                    loader.add(track, None)
                } else {
                    Err(wrong_kind(&track, "a return track"))
                }
            })
            .collect::<Result<Vector<_>, _>>()?;
        let top = tracks
            .into_iter()
            .map(|track| loader.add_grouped(track, None))
            .collect::<Result<Vector<_>, _>>()?;
        if let Some(scenes) = &scenes {
            for scene in scenes {
                loader.claim(scene.id.0)?;
            }
        }
        let mut document = Self {
            settings,
            main,
            top,
            returns,
            tracks: loader.tracks,
            // Filled below: a set saved before WP-3.1 brings no scenes and
            // opens with the default a new set gets, whose IDs sit after
            // the document's largest.
            scenes: Vector::new(),
            index: loader.index,
        };
        document.scenes = match scenes {
            Some(scenes) => scenes.into_iter().collect(),
            None => default_scenes(document.max_id())?.into_iter().collect(),
        };
        // Slots name their scenes, which may be the defaults just made.
        for track in document.tracks.values() {
            for scene in track.slots.keys() {
                if document.scene(*scene).is_none() {
                    return Err(ModelError::NoSuchScene(*scene));
                }
            }
            for scene in &track.removed_stop_buttons {
                if document.scene(*scene).is_none() {
                    return Err(ModelError::NoSuchScene(*scene));
                }
            }
        }
        // Routing, automation, and clips' envelopes name tracks and devices
        // anywhere in the tree, so they're checked once every track is in.
        for id in document.track_order() {
            document.check_routing(id)?;
            document.check_automation(id)?;
            let track = document.track_ref(id)?;
            for clip in track.clips().chain(track.slots().map(|(_, clip)| clip)) {
                document.check_clip_envelopes(id, clip)?;
            }
        }
        Ok(document)
    }
}

/// Builds a document's track map and lookups while checking every rule.
struct Loader {
    tracks: OrdMap<TrackId, Shared<Track>>,
    index: Index,
    seen: BTreeSet<Id>,
}

impl Loader {
    /// Adds an audio, MIDI, or group track at the top level or in a group.
    fn add_grouped(
        &mut self,
        track: TrackRepr,
        parent: Option<TrackId>,
    ) -> Result<TrackId, ModelError> {
        match track.kind {
            TrackKind::Audio | TrackKind::Midi | TrackKind::Group => self.add(track, parent),
            TrackKind::Main => Err(ModelError::MainTrack),
            kind => Err(ModelError::CannotGroup { kind }),
        }
    }

    /// Adds a track whose kind fits where it is.
    fn add(&mut self, repr: TrackRepr, parent: Option<TrackId>) -> Result<TrackId, ModelError> {
        let id = repr.id;
        self.claim(id.0)?;
        if (!repr.clips.is_empty()
            || !repr.slots.is_empty()
            || !repr.removed_stop_buttons.is_empty())
            && !repr.kind.holds_clips()
        {
            return Err(wrong_kind(&repr, "an audio or MIDI track"));
        }
        if !repr.children.is_empty() && repr.kind != TrackKind::Group {
            return Err(wrong_kind(&repr, "a group track"));
        }
        let mut track = Track::new(id, repr.kind, repr.name);
        track.mixer = repr.mixer;
        if let Some(routing) = repr.routing {
            track.routing = routing;
        }
        check_mixer(&track)?;
        if !repr.sends.is_empty() && !repr.kind.has_sends() {
            return Err(wrong_kind_of(
                id,
                repr.kind,
                "an audio, MIDI, or group track",
            ));
        }
        for (to, level) in repr.sends {
            // Main and the returns load before the tracks that send to them.
            let target = self.tracks.get(&to).ok_or(ModelError::NoSuchTrack(to))?;
            if target.kind != TrackKind::Return {
                return Err(wrong_kind_of(to, target.kind, "a return track"));
            }
            check_send_level(level)?;
            if track.sends.insert(to, level).is_some() {
                return Err(ModelError::DuplicateId(to.0));
            }
        }
        for entry in repr.automation {
            let envelope = Envelope::from_points(entry.points)?;
            let automation = crate::automation::Automation {
                envelope,
                enabled: entry.enabled,
            };
            if track.automation.insert(entry.target, automation).is_some() {
                return Err(ModelError::DuplicateId(id.0));
            }
        }
        for device in repr.devices {
            self.claim(device.id.0)?;
            check_device(&device)?;
            self.index.device_track.insert(device.id, id);
            track.devices.push_back(Shared::new(device));
        }
        check_chain(&track)?;
        for clip in repr.clips {
            self.claim(clip.id.0)?;
            check_clip_fits(&track, &clip.data)?;
            check_extent(clip.start, clip.length)?;
            check_clip(&clip)?;
            self.index.clip_track.insert(clip.id, id);
            track.clips.insert(clip.id, Shared::new(clip));
        }
        for slot in repr.slots {
            self.claim(slot.clip.id.0)?;
            check_clip_fits(&track, &slot.clip.data)?;
            if slot.clip.start != Ticks::ZERO {
                return Err(ModelError::SlotClipPosition(slot.clip.start));
            }
            check_extent(slot.clip.start, slot.clip.length)?;
            check_clip(&slot.clip)?;
            self.index.clip_track.insert(slot.clip.id, id);
            if track
                .slots
                .insert(slot.scene, Shared::new(slot.clip))
                .is_some()
            {
                return Err(ModelError::DuplicateId(slot.scene.0));
            }
        }
        for child in repr.children {
            let child = self.add_grouped(child, Some(id))?;
            track.children.push_back(child);
        }
        for scene in repr.removed_stop_buttons {
            if !track.removed_stop_buttons.insert(scene) {
                return Err(ModelError::DuplicateId(scene.0));
            }
        }
        if let Some(parent) = parent {
            self.index.parent.insert(id, parent);
        }
        self.tracks.insert(id, Shared::new(track));
        Ok(id)
    }

    fn claim(&mut self, id: Id) -> Result<(), ModelError> {
        if self.seen.insert(id) {
            Ok(())
        } else {
            Err(ModelError::DuplicateId(id))
        }
    }
}

fn wrong_kind(track: &TrackRepr, needs: &'static str) -> ModelError {
    wrong_kind_of(track.id, track.kind, needs)
}

fn wrong_kind_of(track: TrackId, kind: TrackKind, needs: &'static str) -> ModelError {
    ModelError::WrongTrackKind { track, kind, needs }
}

#[cfg(test)]
mod tests {
    use bl_core::time::Ticks;
    use serde_json::{Value, json};

    use crate::AutomationTarget;
    use crate::clip::{AudioData, Clip, ClipData, MidiData};
    use crate::command::Command;
    use crate::device::{Device, DeviceKind};
    use crate::editor::Editor;
    use crate::ids::{ClipId, ContentHash, ParamId, SceneId, TrackId};
    use crate::track::TrackKind;
    use crate::{DEFAULT_SCENE_COUNT, Document, Settings};

    /// A song with a group holding an audio track with a clip and a device,
    /// and a return track.
    fn song() -> (Document, [TrackId; 3]) {
        let mut editor = Editor::new();
        let track = |editor: &mut Editor, kind, parent| {
            let id = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id,
                    kind,
                    name: format!("{kind}"),
                    parent,
                    index: 0,
                })
                .unwrap();
            id
        };
        let group = track(&mut editor, TrackKind::Group, None);
        let audio = track(&mut editor, TrackKind::Audio, Some(group));
        let ret = track(&mut editor, TrackKind::Return, None);
        let clip = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track: audio,
                clip: Clip::new(
                    clip,
                    Ticks::WHOLE,
                    Ticks::WHOLE * 2,
                    ClipData::Audio(AudioData::new(ContentHash([0xab; 32]))),
                ),
            })
            .unwrap();
        let mut device = Device::new(
            editor.new_id(),
            "Utility",
            DeviceKind::Native {
                device_type: "beatlayer.utility".to_owned(),
            },
        );
        device.params.insert(ParamId(4), -6.0);
        editor
            .apply(Command::InsertDevice {
                track: audio,
                index: 0,
                device,
            })
            .unwrap();
        (editor.document().clone(), [group, audio, ret])
    }

    fn load(value: Value) -> Result<Document, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    #[test]
    fn round_trip() {
        let (doc, _) = song();
        let json = serde_json::to_string(&doc).unwrap();
        let back: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(back, doc);
        back.check_invariants().unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn saved_form_is_a_plain_tree() {
        let (doc, [group, audio, ret]) = song();
        let value = serde_json::to_value(&doc).unwrap();
        let tracks = &value["tracks"];
        assert_eq!(tracks[0]["id"], json!(group.0.get()));
        assert_eq!(tracks[0]["kind"], json!("group"));
        let child = &tracks[0]["children"][0];
        assert_eq!(child["id"], json!(audio.0.get()));
        assert_eq!(
            child["clips"][0]["data"]["audio"]["media"],
            json!("ab".repeat(32))
        );
        assert_eq!(child["devices"][0]["params"], json!([[4, -6.0]]));
        assert_eq!(value["returns"][0]["id"], json!(ret.0.get()));
        assert_eq!(value["main"]["kind"], json!("main"));
        assert_eq!(
            value["settings"],
            serde_json::to_value(Settings::default()).unwrap()
        );
    }

    // Spec: device-chains R1
    #[test]
    fn device_chains_save_and_load_with_their_roles_and_names() {
        let (mut doc, [_, audio, _]) = song();
        let device = doc.track(audio).unwrap().devices().next().unwrap().id;
        // A new audio effect's role, own name, and Show Preset Name aren't
        // written, so sets from before WP-2.4 read the same.
        let value = serde_json::to_value(&doc).unwrap();
        let saved = &value["tracks"][0]["children"][0]["devices"][0];
        for field in ["role", "own_name", "show_preset_name", "floating"] {
            assert!(saved.get(field).is_none(), "{field}");
        }
        doc.apply(&Command::RenameDevice {
            device,
            name: "Trim".to_owned(),
        })
        .unwrap();
        doc.apply(&Command::SetShowPresetName {
            device,
            shown: false,
        })
        .unwrap();
        let value = serde_json::to_value(&doc).unwrap();
        let saved = &value["tracks"][0]["children"][0]["devices"][0];
        assert_eq!(saved["name"], json!("Trim"));
        assert_eq!(saved["own_name"], json!("Utility"));
        assert_eq!(saved["show_preset_name"], json!(false));
        assert_eq!(load(value).unwrap(), doc);

        // A floating device is saved floating (WP-2.5b); a docked one
        // doesn't write it.
        doc.apply(&Command::SetDeviceFloating {
            device,
            floating: true,
        })
        .unwrap();
        let value = serde_json::to_value(&doc).unwrap();
        let saved = &value["tracks"][0]["children"][0]["devices"][0];
        assert_eq!(saved["floating"], json!(true));
        assert_eq!(load(value).unwrap(), doc);

        // A chain out of Live's order doesn't load.
        let mut value = serde_json::to_value(&doc).unwrap();
        value["tracks"][0]["children"][0]["devices"][0]["role"] = json!("instrument");
        assert!(load(value).unwrap_err().contains("instrument"));
    }

    // Spec: clap-hosting R8
    #[test]
    fn a_plugins_state_saves_as_base64_and_loads_back() {
        let (mut doc, [_, audio, _]) = song();
        let device = doc.track(audio).unwrap().devices().next().unwrap().id;
        let value = serde_json::to_value(&doc).unwrap();
        assert!(
            value["tracks"][0]["children"][0]["devices"][0]
                .get("plugin")
                .is_none(),
            "a native device saves no plug-in state"
        );
        let state = crate::PluginState {
            path: "/Library/Audio/Plug-Ins/CLAP/Gain.clap".to_owned(),
            data: (0..=255).collect(),
        };
        doc.set_plugin_state(device, Some(state.clone())).unwrap();
        let value = serde_json::to_value(&doc).unwrap();
        let saved = &value["tracks"][0]["children"][0]["devices"][0]["plugin"];
        assert_eq!(saved["path"], json!(state.path));
        assert_eq!(
            saved["data"],
            json!(crate::base64::encode(&state.data)),
            "base64"
        );
        let loaded = load(value).unwrap();
        assert_eq!(loaded.device(device).unwrap().plugin, Some(state));
        assert_eq!(loaded, doc);
    }

    // Spec: session-view R13
    #[test]
    fn scenes_and_slot_clips_save_and_load() {
        let mut editor = Editor::new();
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: "Keys".to_owned(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let scene: SceneId = editor.new_id();
        editor
            .apply(Command::CreateScene {
                id: scene,
                index: 1,
            })
            .unwrap();
        editor
            .apply(Command::RenameScene {
                scene,
                name: "Chorus".to_owned(),
            })
            .unwrap();
        let clip: ClipId = editor.new_id();
        editor
            .apply(Command::CreateSlotClip {
                track,
                scene,
                clip: Clip::new(
                    clip,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Midi(MidiData::default()),
                ),
            })
            .unwrap();
        let doc = editor.document().clone();
        let value = serde_json::to_value(&doc).unwrap();
        // The scenes save in order, and the clip under its track's slots.
        assert_eq!(value["scenes"][1]["name"], json!("Chorus"));
        let saved = &value["tracks"][0]["slots"][0];
        assert_eq!(saved["scene"], json!(scene.0.get()));
        assert_eq!(saved["clip"]["id"], json!(clip.0.get()));
        let loaded = load(value).unwrap();
        assert_eq!(loaded, doc);
        assert_eq!(loaded.scene_position(scene), Some(1));
        assert_eq!(loaded.slot(track, scene).unwrap().id, clip);
        loaded.check_invariants().unwrap();

        // A slot that names a scene the set doesn't hold doesn't load.
        let mut broken = serde_json::to_value(&doc).unwrap();
        broken["tracks"][0]["slots"][0]["scene"] = json!(9999);
        let error = load(broken).unwrap_err();
        assert!(error.contains("no scene"), "{error}");
        // A slot clip has no position, and a slot holds one clip.
        let mut broken = serde_json::to_value(&doc).unwrap();
        broken["tracks"][0]["slots"][0]["clip"]["start"] = json!(Ticks::WHOLE.0);
        let error = load(broken).unwrap_err();
        assert!(error.contains("no position"), "{error}");
        let mut broken = serde_json::to_value(&doc).unwrap();
        let slot = broken["tracks"][0]["slots"][0].clone();
        broken["tracks"][0]["slots"]
            .as_array_mut()
            .unwrap()
            .push(slot);
        let error = load(broken).unwrap_err();
        assert!(error.contains("more than once"), "{error}");
        // Only audio and MIDI tracks hold slot clips.
        let mut broken = serde_json::to_value(&doc).unwrap();
        let slot = broken["tracks"][0]["slots"][0].clone();
        broken["main"]["slots"] = json!([slot]);
        let error = load(broken).unwrap_err();
        assert!(error.contains("audio or MIDI"), "{error}");
    }

    // Spec: session-view R14
    #[test]
    fn a_set_saved_before_scenes_and_slots_loads_with_the_default() {
        let (doc, _) = song();
        let mut value = serde_json::to_value(&doc).unwrap();
        value.as_object_mut().unwrap().remove("scenes");
        let loaded = load(value).unwrap();
        assert_eq!(loaded.scene_count(), DEFAULT_SCENE_COUNT);
        assert!(loaded.scenes().all(|scene| scene.name.is_empty()));
        assert!(
            !loaded.tracks().any(|track| track.slots().len() > 0),
            "no slots in a set saved before them"
        );
        assert_eq!(
            loaded.max_id(),
            loaded.scenes().last().expect("eight scenes").id.0,
            "the default scenes take the IDs after the document's largest"
        );
        loaded.check_invariants().unwrap();
        // Saving it again writes the scenes, so the next load keeps them.
        let again: Document =
            serde_json::from_value(serde_json::to_value(&loaded).unwrap()).unwrap();
        assert_eq!(again, loaded);

        // A set whose IDs leave no room for the scenes is refused, not a
        // panic.
        let mut broken = serde_json::to_value(&doc).unwrap();
        broken.as_object_mut().unwrap().remove("scenes");
        broken["main"]["id"] = json!(u64::MAX - 4);
        let error = load(broken).unwrap_err();
        assert!(error.contains("no room"), "{error}");
    }

    // Spec: audio-clips R15
    #[test]
    fn audio_clip_settings_are_saved_only_where_they_differ_from_a_new_clips() {
        let (doc, _) = song();
        let value = serde_json::to_value(&doc).unwrap();
        // A new clip's data saves as its media alone, as before WP-1.4, so a
        // set saved then loads as a new clip of the whole file.
        let data = &value["tracks"][0]["children"][0]["clips"][0]["data"];
        assert_eq!(data, &json!({ "audio": { "media": "ab".repeat(32) } }));

        let mut editor = Editor::new();
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let audio = AudioData {
            start: 0.25,
            end: Some(1.0),
            looping: true,
            loop_start: 0.5,
            loop_end: Some(0.75),
            gain: 0.5,
            ..AudioData::new(ContentHash([1; 32])).with_edge_fades()
        };
        let clip = Clip::new(
            editor.new_id(),
            Ticks::ZERO,
            Ticks::WHOLE,
            ClipData::Audio(audio),
        );
        editor.apply(Command::CreateClip { track, clip }).unwrap();
        let value = serde_json::to_value(editor.document()).unwrap();
        assert_eq!(
            value["tracks"][0]["clips"][0]["data"]["audio"],
            json!({
                "media": "01".repeat(32),
                "start": 0.25,
                "end": 1.0,
                "looping": true,
                "loop_start": 0.5,
                "loop_end": 0.75,
                "gain": 0.5,
                "fade_in": 0.004,
                "fade_out": 0.004,
            })
        );
        let back = load(value).unwrap();
        assert_eq!(&back, editor.document());

        // Loading checks the settings as the commands do.
        let mut bad = serde_json::to_value(editor.document()).unwrap();
        bad["tracks"][0]["clips"][0]["data"]["audio"]["loop_end"] = json!(0.1);
        let error = load(bad).unwrap_err();
        assert!(error.contains("loop end 0.1 is out of range"), "{error}");
    }

    // Spec: automation R7
    #[test]
    fn automation_saves_with_the_set_and_loads_back() {
        let (mut doc, [_, audio, ret]) = song();
        let device = doc.track(audio).unwrap().devices().next().unwrap().id;
        let volume = AutomationTarget::Mixer {
            param: crate::MixerParam::Volume,
        };
        for (target, at, value) in [
            (volume, Ticks::ZERO, 1.0),
            (volume, Ticks::WHOLE, 0.25),
            (AutomationTarget::Send { to: ret }, Ticks::ZERO, 0.5),
            (
                AutomationTarget::Device {
                    device,
                    param: ParamId(4),
                },
                Ticks::ZERO,
                -6.0,
            ),
        ] {
            doc.apply(&Command::SetAutomationBreakpoint {
                track: audio,
                target,
                at,
                value,
            })
            .unwrap();
        }
        // A hand move overrides the volume's envelope; it's saved overridden.
        doc.apply(&Command::SetVolume {
            track: audio,
            volume: 0.5,
        })
        .unwrap();
        // The volume's segment is curved: its bend saves with the breakpoint
        // at the segment's start.
        doc.apply(&Command::SetAutomationCurve {
            track: audio,
            target: volume,
            at: Ticks::ZERO,
            bend: -0.5,
        })
        .unwrap();
        let value = serde_json::to_value(&doc).unwrap();
        let saved = value["tracks"][0]["children"][0]["automation"]
            .as_array()
            .unwrap();
        assert_eq!(
            saved.len(),
            3,
            "one entry per parameter, not per breakpoint"
        );
        let entry = |param: &str| {
            saved
                .iter()
                .find(|entry| entry["target"]["param"] == json!(param))
                .unwrap()
        };
        assert_eq!(
            entry("volume")["target"],
            json!({ "type": "mixer", "param": "volume" })
        );
        assert_eq!(
            entry("volume")["points"],
            json!([
                { "at": 0, "value": 1.0, "bend": -0.5 },
                { "at": Ticks::WHOLE.0, "value": 0.25 }
            ])
        );
        assert_eq!(entry("volume")["enabled"], json!(false));
        assert_eq!(
            saved
                .iter()
                .find(|entry| entry["target"]["type"] == json!("send"))
                .unwrap()["target"],
            json!({ "type": "send", "to": ret.0.get() })
        );
        assert_eq!(
            saved
                .iter()
                .find(|entry| entry["target"]["type"] == json!("device"))
                .unwrap()["points"],
            json!([{ "at": 0, "value": -6.0 }])
        );
        let loaded = load(value).unwrap();
        assert_eq!(loaded, doc);
        loaded.check_invariants().unwrap();

        // Broken saved automation doesn't load.
        let broken = |edit: &dyn Fn(&mut Value)| {
            let mut value = serde_json::to_value(&doc).unwrap();
            edit(&mut value);
            load(value).unwrap_err()
        };
        fn first_entry(value: &mut Value) -> &mut Value {
            value["tracks"][0]["children"][0]["automation"]
                .as_array_mut()
                .unwrap()
                .first_mut()
                .unwrap()
        }
        let error = broken(&|value| {
            first_entry(value)["points"] = json!([
                { "at": 10, "value": 0.0 },
                { "at": 5, "value": 1.0 }
            ]);
        });
        assert!(error.contains("already a breakpoint"), "{error}");
        let error = broken(&|value| {
            first_entry(value)["points"] = json!([
                { "at": 0, "value": 0.0 },
                { "at": 0, "value": 1.0 }
            ]);
        });
        assert!(error.contains("already a breakpoint"), "{error}");
        let error = broken(&|value| {
            first_entry(value)["points"] = json!([{ "at": 0, "value": true }]);
        });
        assert!(!error.is_empty());
        let error = broken(&|value| {
            first_entry(value)["enabled"] = json!("yes");
        });
        assert!(!error.is_empty());
        let error = broken(&|value| {
            first_entry(value)["points"] = json!([{ "at": -5, "value": 0.0 }]);
        });
        assert!(error.contains("before zero"), "{error}");
        let error = broken(&|value| {
            first_entry(value)["points"] = json!([
                { "at": 0, "value": 0.0, "bend": 2.0 },
                { "at": 1_000, "value": 1.0 }
            ]);
        });
        assert!(
            error.contains("automation bend 2 is out of range"),
            "{error}"
        );
    }

    // Spec: clip-envelopes R6
    #[test]
    fn clip_envelopes_save_with_the_set_and_load_back() {
        let (mut doc, [_, audio, _]) = song();
        let clip = doc.track(audio).unwrap().clips().next().unwrap().id;
        let device = doc.track(audio).unwrap().devices().next().unwrap().id;
        let volume = AutomationTarget::Mixer {
            param: crate::MixerParam::Volume,
        };
        let param = AutomationTarget::Device {
            device,
            param: ParamId(4),
        };
        for (target, at, value) in [(volume, Ticks::ZERO, 1.0), (volume, Ticks::WHOLE, 0.25)] {
            doc.apply(&Command::SetClipEnvelopeBreakpoint {
                clip,
                target,
                at,
                value,
            })
            .unwrap();
        }
        doc.apply(&Command::SetClipEnvelopeBreakpoint {
            clip,
            target: param,
            at: Ticks::ZERO,
            value: -6.0,
        })
        .unwrap();
        // The volume's segment is curved and the envelope is unlinked: both
        // save.
        doc.apply(&Command::SetClipEnvelopeCurve {
            clip,
            target: volume,
            at: Ticks::ZERO,
            bend: -0.5,
        })
        .unwrap();
        doc.apply(&Command::SetClipEnvelopeLinked {
            clip,
            target: volume,
            linked: false,
        })
        .unwrap();
        let value = serde_json::to_value(&doc).unwrap();
        let saved = value["tracks"][0]["children"][0]["clips"][0]["envelopes"]
            .as_array()
            .unwrap();
        assert_eq!(
            saved.len(),
            2,
            "one entry per parameter, not per breakpoint"
        );
        let volume_entry = saved
            .iter()
            .find(|entry| entry["target"]["param"] == json!("volume"))
            .unwrap();
        assert_eq!(
            volume_entry["target"],
            json!({ "type": "mixer", "param": "volume" })
        );
        assert_eq!(volume_entry["linked"], json!(false));
        assert_eq!(
            volume_entry["envelope"]["points"],
            json!([
                { "at": 0, "value": 1.0, "bend": -0.5 },
                { "at": Ticks::WHOLE.0, "value": 0.25 }
            ])
        );
        let device_entry = saved
            .iter()
            .find(|entry| entry["target"]["type"] == json!("device"))
            .unwrap();
        assert!(
            device_entry.get("linked").is_none(),
            "a linked envelope leaves its default out"
        );
        assert_eq!(
            device_entry["envelope"]["points"],
            json!([{ "at": 0, "value": -6.0 }])
        );
        let loaded = load(value).unwrap();
        assert_eq!(loaded, doc);
        loaded.check_invariants().unwrap();

        // A set saved before clip envelopes loads with none: the flag isn't
        // there at all.
        let mut old = serde_json::to_value(&doc).unwrap();
        old["tracks"][0]["children"][0]["clips"][0]
            .as_object_mut()
            .unwrap()
            .remove("envelopes");
        let back = load(old).unwrap();
        assert_eq!(back.clip(clip).unwrap().envelopes().count(), 0);
        back.check_invariants().unwrap();

        // Broken saved envelopes don't load: breakpoints out of order, a
        // position before zero, a bend outside −1 to 1, and a target the
        // clip's track doesn't have.
        let broken = |edit: &dyn Fn(&mut Value)| {
            let mut value = serde_json::to_value(&doc).unwrap();
            edit(&mut value);
            load(value).unwrap_err()
        };
        fn first_envelope(value: &mut Value) -> &mut Value {
            value["tracks"][0]["children"][0]["clips"][0]["envelopes"]
                .as_array_mut()
                .unwrap()
                .first_mut()
                .unwrap()
        }
        let error = broken(&|value| {
            first_envelope(value)["envelope"]["points"] = json!([
                { "at": 10, "value": 0.0 },
                { "at": 5, "value": 1.0 }
            ]);
        });
        assert!(error.contains("already a breakpoint"), "{error}");
        let error = broken(&|value| {
            first_envelope(value)["envelope"]["points"] = json!([{ "at": -5, "value": 0.0 }]);
        });
        assert!(error.contains("before zero"), "{error}");
        let error = broken(&|value| {
            first_envelope(value)["envelope"]["points"] = json!([
                { "at": 0, "value": 0.0, "bend": 2.0 },
                { "at": 1_000, "value": 1.0 }
            ]);
        });
        assert!(
            error.contains("automation bend 2 is out of range"),
            "{error}"
        );
        let error = broken(&|value| {
            first_envelope(value)["target"] = json!({
                "type": "device",
                "device": 9_999,
                "param": 0
            });
        });
        assert!(error.contains("isn't in"), "{error}");
        let error = broken(&|value| {
            let first = first_envelope(value).clone();
            value["tracks"][0]["children"][0]["clips"][0]["envelopes"]
                .as_array_mut()
                .unwrap()
                .push(first);
        });
        assert!(error.contains("two envelopes"), "{error}");
    }

    #[test]
    fn malformed_documents_are_refused() {
        let (doc, _) = song();
        let good = serde_json::to_value(&doc).unwrap();
        let broken = |edit: &dyn Fn(&mut Value)| {
            let mut value = good.clone();
            edit(&mut value);
            load(value).unwrap_err()
        };

        let error = broken(&|v| v["returns"][0]["id"] = v["main"]["id"].clone());
        assert!(error.contains("appears more than once"), "{error}");
        let error = broken(&|v| v["main"]["kind"] = json!("audio"));
        assert!(error.contains("Main track"), "{error}");
        let error = broken(&|v| v["returns"][0]["kind"] = json!("audio"));
        assert!(error.contains("needs a return track"), "{error}");
        let error = broken(&|v| {
            let ret = v["returns"][0].clone();
            v["tracks"][0]["children"].as_array_mut().unwrap().push(ret);
            v["returns"] = json!([]);
        });
        assert!(error.contains("can't go inside a group"), "{error}");
        let error = broken(&|v| v["tracks"][0]["children"][0]["kind"] = json!("midi"));
        assert!(error.contains("needs an audio track"), "{error}");
        let error = broken(&|v| v["tracks"][0]["kind"] = json!("audio"));
        assert!(error.contains("needs a group track"), "{error}");
        let error = broken(&|v| v["main"]["mixer"]["solo"] = json!(true));
        assert!(error.contains("other than Main"), "{error}");
        let error = broken(&|v| v["returns"][0]["mixer"]["arm"] = json!(true));
        assert!(error.contains("audio or MIDI"), "{error}");
        let error = broken(&|v| v["tracks"][0]["children"][0]["clips"][0]["length"] = json!(0));
        assert!(error.contains("isn't positive"), "{error}");
        let error = broken(&|v| {
            v["tracks"][0]["children"][0]["devices"][0]["params"] = json!([[1, 0.0], [1, 1.0]]);
        });
        assert!(error.contains("listed twice"), "{error}");
        let error = broken(&|v| {
            v["returns"][0]["routing"] =
                json!({ "input": { "type": "all_ins" }, "monitor": "auto" });
        });
        assert!(error.contains("audio or MIDI"), "{error}");
        let error = broken(&|v| {
            v["tracks"][0]["children"][0]["routing"] = json!({
                "input": { "type": "external" },
                "channel": 3,
                "monitor": "auto",
            });
        });
        assert!(error.contains("only a port"), "{error}");
        let error = broken(&|v| v["tracks"][0]["children"][0]["clips"][0]["id"] = json!(0));
        assert!(
            error.contains("nonzero") || error.contains("zero"),
            "{error}"
        );
    }
}
