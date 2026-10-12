//! How a document serialises: a plain tree, checked on the way in.
//!
//! Groups hold their tracks inline, so the saved form mirrors the display
//! order and can't express a track in two places. The derived lookups aren't
//! saved; loading rebuilds them.

use std::collections::BTreeSet;

use bl_core::id::Id;
use imbl::{OrdMap, Vector};
use serde::{Deserialize, Serialize};

use crate::clip::Clip;
use crate::command::{
    check_chain, check_clip, check_clip_fits, check_device, check_extent, check_mixer,
    check_send_level,
};
use crate::device::Device;
use crate::document::{Document, Index, Settings};
use crate::error::ModelError;
use crate::ids::TrackId;
use crate::shared::Shared;
use crate::track::{Mixer, Routing, Track, TrackKind};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DocumentRepr {
    settings: Settings,
    tracks: Vec<TrackRepr>,
    returns: Vec<TrackRepr>,
    main: TrackRepr,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    devices: Vec<Device>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    clips: Vec<Clip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    children: Vec<TrackRepr>,
}

impl From<&Document> for DocumentRepr {
    fn from(doc: &Document) -> Self {
        let track = |id: &TrackId| TrackRepr::new(doc, *id);
        Self {
            settings: doc.settings.clone(),
            tracks: doc.top.iter().map(track).collect(),
            returns: doc.returns.iter().map(track).collect(),
            main: track(&doc.main),
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
            routing: (track.routing != Routing::new(track.kind)).then_some(track.routing),
            devices: track.devices().cloned().collect(),
            clips: track.clips().cloned().collect(),
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
        let mut loader = Loader {
            tracks: OrdMap::new(),
            index: Index::default(),
            seen: BTreeSet::new(),
        };
        if repr.main.kind != TrackKind::Main {
            return Err(ModelError::MainTrack);
        }
        let main = loader.add(repr.main, None)?;
        let returns = repr
            .returns
            .into_iter()
            .map(|track| {
                if track.kind == TrackKind::Return {
                    loader.add(track, None)
                } else {
                    Err(wrong_kind(&track, "a return track"))
                }
            })
            .collect::<Result<Vector<_>, _>>()?;
        let top = repr
            .tracks
            .into_iter()
            .map(|track| loader.add_grouped(track, None))
            .collect::<Result<Vector<_>, _>>()?;
        let document = Self {
            settings: repr.settings,
            main,
            top,
            returns,
            tracks: loader.tracks,
            index: loader.index,
        };
        // Routing names tracks anywhere in the tree, so it's checked once
        // every track is in.
        for id in document.track_order() {
            document.check_routing(id)?;
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
        if !repr.clips.is_empty() && !repr.kind.holds_clips() {
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
        for child in repr.children {
            let child = self.add_grouped(child, Some(id))?;
            track.children.push_back(child);
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

    use crate::clip::{AudioData, Clip, ClipData};
    use crate::command::Command;
    use crate::device::{Device, DeviceKind};
    use crate::editor::Editor;
    use crate::ids::{ContentHash, ParamId, TrackId};
    use crate::track::TrackKind;
    use crate::{Document, Settings};

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
        let error = broken(&|v| v["tracks"][0]["children"][0]["clips"][0]["id"] = json!(0));
        assert!(
            error.contains("nonzero") || error.contains("zero"),
            "{error}"
        );
    }
}
