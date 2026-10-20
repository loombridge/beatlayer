//! How a document serialises: a plain tree, checked on the way in.
//!
//! Groups hold their tracks inline, so the saved form mirrors the display
//! order and can't express a track in two places. The derived lookups aren't
//! saved; loading rebuilds them.

use std::collections::BTreeSet;

use bl_core::id::Id;
use bl_core::time::Ticks;
use imbl::{OrdMap, Vector};
use schemars::{JsonSchema, Schema, schema_for};
use serde::{Deserialize, Serialize};

use crate::automation::{AutomationTarget, Breakpoint, Envelope};
use crate::clip::{Clip, TakeLane};
use crate::command::{
    check_chain, check_clip, check_clip_fits, check_crossfader, check_device, check_extent,
    check_mixer, check_send_level, check_take_lane,
};
use crate::device::Device;
use crate::document::{Document, Index, Settings};
use crate::error::ModelError;
use crate::ids::{SceneId, TrackId};
use crate::launch::LaunchSettings;
use crate::scene::{Scene, default_scenes};
use crate::shared::Shared;
use crate::track::{Mixer, Routing, Track, TrackDelay, TrackKind};

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

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
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
    /// The take lanes under the Arrangement lane (WP-2.13). Sets saved
    /// before it have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    take_lanes: Vec<TakeLane>,
    /// The track's delay (the mixer spec). Sets saved before WP-5.36 have
    /// none, which loads as 0 frames and milliseconds.
    #[serde(default, skip_serializing_if = "TrackDelay::is_default")]
    delay: TrackDelay,
    /// Keep Monitoring Latency (the delay-compensation spec). On in a new
    /// track, so a set writes it only when it's off; sets saved before
    /// WP-5.36 have none, which loads as on.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    keep_monitoring_latency: bool,
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
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
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
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
struct SlotRepr {
    scene: SceneId,
    clip: Clip,
}

/// The song's own state as the set saves it: its settings (the tempo map,
/// time signatures, loop, punch and record switches), the launch
/// quantisation saved beside them, and the scenes.
#[derive(Serialize, JsonSchema)]
pub struct SavedSong<'a> {
    /// The song-wide settings.
    pub settings: &'a Settings,
    /// The Session View's launch quantisation.
    pub launch: LaunchSettings,
    /// The scenes, in order.
    pub scenes: Vec<&'a Scene>,
}

/// The JSON Schema of `object` as the set saves it, for checking that a
/// field an agent is told to read back exists: `track` (without the tracks
/// inside it), `clip`, `device`, `scene` or `song`.
#[must_use]
pub fn saved_schema(object: &str) -> Option<Schema> {
    Some(match object {
        "track" => schema_for!(TrackRepr),
        "clip" => schema_for!(Clip),
        "device" => schema_for!(Device),
        "scene" => schema_for!(Scene),
        "song" => schema_for!(SavedSong<'static>),
        _ => return None,
    })
}

impl From<&Document> for DocumentRepr {
    fn from(doc: &Document) -> Self {
        let track = |id: &TrackId| TrackRepr::new(doc, *id, true);
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

impl Document {
    /// How the set saves this track, without the tracks inside it if it's a
    /// group: the same code the set format uses, so a field added to the
    /// format is in it.
    #[must_use]
    pub fn saved_track(&self, id: TrackId) -> Option<impl Serialize> {
        self.tracks
            .contains_key(&id)
            .then(|| TrackRepr::new(self, id, false))
    }
}

impl TrackRepr {
    fn new(doc: &Document, id: TrackId, with_children: bool) -> Self {
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
            take_lanes: track.take_lanes().cloned().collect(),
            delay: track.delay(),
            keep_monitoring_latency: track.keep_monitoring_latency(),
            children: if with_children {
                track
                    .children
                    .iter()
                    .map(|id| Self::new(doc, *id, true))
                    .collect()
            } else {
                Vec::new()
            },
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
        // Spec: modulation R19. A mapping naming a parameter the set doesn't
        // have doesn't load.
        document.prune_mappings();
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
        if !repr.take_lanes.is_empty() && repr.kind != TrackKind::Audio {
            return Err(wrong_kind(&repr, "an audio track"));
        }
        if !repr.children.is_empty() && repr.kind != TrackKind::Group {
            return Err(wrong_kind(&repr, "a group track"));
        }
        let mut track = Track::new(id, repr.kind, repr.name);
        track.mixer = repr.mixer;
        track.delay = repr.delay;
        if !track.delay.is_valid() {
            return Err(ModelError::InvalidValue {
                what: "track delay",
                value: match track.delay {
                    TrackDelay::Milliseconds { value } => value,
                    TrackDelay::Samples { value } => value as f64,
                },
            });
        }
        track.keep_monitoring_latency = repr.keep_monitoring_latency;
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
            for used in device.ids() {
                self.claim(used)?;
            }
            check_device(&device)?;
            self.index.add_device(&device, id);
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
        for lane in repr.take_lanes {
            check_take_lane(&lane)?;
            track.take_lanes.push_back(lane);
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
    use crate::ids::{ClipId, ContentHash, DeviceId, ParamId, SceneId, TrackId};
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

    // Spec: drum-racks R27
    #[test]
    fn drum_racks_save_and_load_with_returns() {
        use crate::{Chain, ChainId, DeviceId, RackKind, Receive};
        let (mut doc, [_, audio, ret]) = song();
        let (rack, pad, hat, reverb): (DeviceId, ChainId, ChainId, ChainId) = (
            DeviceId(bl_core::id::Id::new(900).unwrap()),
            ChainId(bl_core::id::Id::new(901).unwrap()),
            ChainId(bl_core::id::Id::new(902).unwrap()),
            ChainId(bl_core::id::Id::new(903).unwrap()),
        );
        // A drum rack holds instruments, so it sits on a MIDI track.
        let midi = TrackId(bl_core::id::Id::new(904).unwrap());
        for command in [
            Command::CreateTrack {
                id: midi,
                kind: TrackKind::Midi,
                name: "Drums".to_owned(),
                parent: None,
                index: 0,
            },
            Command::InsertDevice {
                track: midi,
                index: 0,
                device: Device::rack(rack, RackKind::Drum)
                    .with_chain(Chain::for_pad(pad, "Kick", 36))
                    .with_chain(Chain::for_pad(hat, "Hat", 42))
                    .with_chain(Chain::new_return(reverb, "Reverb")),
            },
            Command::SetChainReceive {
                chain: hat,
                receive: Receive::Note(46),
            },
            Command::SetChainPlay {
                chain: hat,
                play: 61,
            },
            Command::SetChainChoke {
                chain: hat,
                choke: Some(1),
            },
            Command::SetChainSend {
                chain: pad,
                to: reverb,
                level: 0.5,
            },
            Command::SetChainAudioTo {
                chain: reverb,
                track: Some(ret),
            },
            Command::SetAutomationBreakpoint {
                track: midi,
                target: AutomationTarget::ChainSend {
                    chain: pad,
                    to: reverb,
                },
                at: Ticks::ZERO,
                value: 0.25,
            },
        ] {
            doc.apply(&command).unwrap();
        }
        let _ = audio;
        let value = serde_json::to_value(&doc).unwrap();
        let back = load(value.clone()).unwrap();
        assert_eq!(back, doc);
        let chains = &value["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == json!(midi))
            .unwrap()["devices"][0]["chains"];
        // A chain with its defaults saves none of a drum chain's fields.
        assert!(chains[0].get("play").is_none() && chains[0].get("choke").is_none());
        assert_eq!(chains[1]["receive"], json!({ "note": 46 }));
        assert_eq!(chains[1]["play"], json!(61));
        assert_eq!(chains[1]["choke"], json!(1));
        assert_eq!(chains[0]["sends"], json!([{ "to": reverb, "level": 0.5 }]));
        assert_eq!(chains[2]["is_return"], json!(true));
        assert_eq!(chains[2]["audio_to"], json!(ret));
    }

    // Spec: modulation R19
    #[test]
    fn modulator_mappings_save_with_the_set_and_load_back() {
        use crate::modulator::{Mapping, ModTarget, ShapePoint, WAVE_MOD};
        let (mut doc, [_, audio, _]) = song();
        let target = doc.device_ids().next().unwrap();
        let mut wave = Device::new(
            crate::DeviceId(bl_core::id::Id::new(doc.max_id().get() + 1).unwrap()),
            "LFO",
            DeviceKind::Native {
                device_type: WAVE_MOD.to_owned(),
            },
        );
        wave.mappings = vec![
            Mapping {
                slot: 0,
                target: ModTarget::Device {
                    device: target,
                    param: ParamId(4),
                },
            },
            Mapping {
                slot: 3,
                target: ModTarget::Mixer {
                    track: audio,
                    param: crate::MixerParam::Pan,
                },
            },
        ];
        wave.shape = Some(vec![ShapePoint {
            x: 0.0,
            y: 0.5,
            bend: 0.25,
            sustain: false,
        }]);
        doc.apply(&Command::InsertDevice {
            track: audio,
            index: 1,
            device: wave.clone(),
        })
        .unwrap();
        let json = serde_json::to_string(&doc).unwrap();
        let back: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(back.device(wave.id).unwrap().mappings, wave.mappings);
        assert_eq!(back.device(wave.id).unwrap().shape, wave.shape);
        // A mapping that names a parameter the set lacks doesn't load.
        let stale = json.replacen(
            &format!("\"device\":{}", target.0.get()),
            "\"device\":777777",
            1,
        );
        let loaded: Document = serde_json::from_str(&stale).unwrap();
        assert!(
            loaded.device(wave.id).unwrap().mappings.len() == 1,
            "the stale mapping went"
        );
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

    // Spec: racks R13
    #[test]
    fn racks_save_and_load_with_every_chain() {
        use crate::rack::{Chain, RackKind};
        let (mut doc, [_, audio, _]) = song();
        let mut editor = Editor::from_document(doc.clone(), bl_core::id::IdAllocator::new());
        let (rack, inner, tail): (DeviceId, DeviceId, DeviceId) =
            (editor.new_id(), editor.new_id(), editor.new_id());
        let (wet, dry, deep): (crate::ChainId, crate::ChainId, crate::ChainId) =
            (editor.new_id(), editor.new_id(), editor.new_id());
        let mut wet_chain = Chain::new(wet, "Wet").with_devices(vec![
            Device::rack(inner, RackKind::AudioEffect).with_chain(Chain::new(deep, "Deep")),
            Device::new(
                tail,
                "Tail",
                DeviceKind::Native {
                    device_type: "beatlayer.utility".to_owned(),
                },
            ),
        ]);
        wet_chain.colour = Some(12);
        wet_chain.info = "the wet one".to_owned();
        wet_chain.mixer.volume = 0.5;
        wet_chain.mixer.pan = -0.25;
        wet_chain.mixer.enabled = false;
        wet_chain.mixer.solo = true;
        let device = Device::rack(rack, RackKind::AudioEffect)
            .with_chain(wet_chain)
            .with_chain(Chain::new(dry, "Dry"));
        editor
            .apply(Command::InsertDevice {
                track: audio,
                index: 1,
                device,
            })
            .unwrap();
        doc = editor.document().clone();

        let value = serde_json::to_value(&doc).unwrap();
        let saved = &value["tracks"][0]["children"][0]["devices"][1];
        assert_eq!(saved["kind"], json!({ "rack": { "kind": "audio_effect" } }));
        assert_eq!(saved["chains"][0]["name"], json!("Wet"));
        assert_eq!(saved["chains"][0]["colour"], json!(12));
        assert_eq!(saved["chains"][0]["info"], json!("the wet one"));
        assert_eq!(
            saved["chains"][0]["mixer"],
            json!({ "volume": 0.5, "pan": -0.25, "enabled": false, "solo": true })
        );
        // A chain at its defaults writes none of them, and a device that
        // isn't a rack writes no chains.
        assert_eq!(
            saved["chains"][1],
            json!({ "id": dry.0.get(), "name": "Dry", "mixer": {}, "devices": [] })
        );
        assert!(
            value["tracks"][0]["children"][0]["devices"][0]
                .get("chains")
                .is_none()
        );
        let back: Document = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(back, doc);
        back.check_invariants().unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), value);
        // The lookups find every device and chain at every depth.
        for chain in [wet, dry, deep] {
            assert_eq!(back.chain_track(chain), Some(audio));
        }
        for device in [rack, inner, tail] {
            assert_eq!(back.device_track(device), Some(audio));
        }

        // What a damaged file does: a chain ID used twice, a pan out of
        // range, a MIDI effect in an audio effect rack, and a rack whose
        // role isn't its kind's are all refused.
        let broken = |edit: &dyn Fn(&mut Value)| {
            let mut v = value.clone();
            edit(&mut v);
            load(v).unwrap_err()
        };
        let error = broken(&|v| {
            v["tracks"][0]["children"][0]["devices"][1]["chains"][1]["id"] = json!(wet.0.get());
        });
        assert!(error.contains("more than once"), "{error}");
        let error = broken(&|v| {
            v["tracks"][0]["children"][0]["devices"][1]["chains"][0]["mixer"]["pan"] = json!(2.0);
        });
        assert!(error.contains("pan"), "{error}");
        let error = broken(&|v| {
            v["tracks"][0]["children"][0]["devices"][1]["chains"][1]["devices"] = json!([{
                "id": 9_999, "name": "Arp", "enabled": true, "params": [],
                "role": "midi_effect",
                "kind": { "native": { "device_type": "test.arp" } },
            }]);
        });
        assert!(error.contains("can't hold"), "{error}");
        let error = broken(&|v| {
            v["tracks"][0]["children"][0]["devices"][1]["role"] = json!("midi_effect");
        });
        assert!(error.contains("role"), "{error}");
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

    // Spec: device-compare R10
    #[test]
    fn both_states_save_with_the_set_and_load_back() {
        let (mut doc, [_, audio, _]) = song();
        let device = doc.track(audio).unwrap().devices().next().unwrap().id;
        let saved = |doc: &Document| {
            serde_json::to_value(doc).unwrap()["tracks"][0]["children"][0]["devices"][0].clone()
        };
        // Equal states write neither the active state nor the other.
        let before = saved(&doc);
        assert!(before.get("b_active").is_none() && before.get("other_params").is_none());
        // A change leaves the other state as it was, and it's written.
        let set = |value| Command::SetDeviceParam {
            device,
            param: ParamId(4),
            value,
        };
        doc.apply(&set(-12.0)).unwrap();
        assert_eq!(saved(&doc)["other_params"], json!([[4, -6.0]]));
        doc.apply(&Command::CompareSwitch { device }).unwrap();
        let after = saved(&doc);
        assert_eq!(after["b_active"], json!(true));
        assert_eq!(after["params"], json!([[4, -6.0]]));
        assert_eq!(after["other_params"], json!([[4, -12.0]]));
        assert_eq!(load(serde_json::to_value(&doc).unwrap()).unwrap(), doc);
        // Copying makes them equal again, and the other isn't written.
        doc.apply(&Command::CompareCopy { device }).unwrap();
        assert!(saved(&doc).get("other_params").is_none());
        assert_eq!(load(serde_json::to_value(&doc).unwrap()).unwrap(), doc);
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
        // Spec: session-view R20. A scene's own tempo and signature save
        // with it, and a scene without them writes neither.
        editor
            .apply(Command::SetSceneTempo {
                scene,
                tempo: Some(bl_core::time::Bpm::new(96.5).unwrap()),
            })
            .unwrap();
        editor
            .apply(Command::SetSceneTimeSignature {
                scene,
                signature: Some(bl_core::time::TimeSignature::new(7, 8).unwrap()),
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
        assert_eq!(value["scenes"][1]["tempo"], json!(96.5));
        assert_eq!(
            value["scenes"][1]["signature"],
            json!({ "numerator": 7, "denominator": 8 })
        );
        assert!(value["scenes"][0].get("tempo").is_none());
        assert!(value["scenes"][0].get("signature").is_none());
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

    // Spec: launch-quantisation R14
    #[test]
    fn a_clips_launch_settings_save_and_a_clip_saved_without_them_has_the_defaults() {
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
        let scene = editor.document().scenes().next().unwrap().id;
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
        let plain = serde_json::to_value(editor.document()).unwrap();
        assert!(
            plain["tracks"][0]["slots"][0]["clip"]
                .get("launch")
                .is_none(),
            "the defaults aren't written"
        );

        editor
            .apply(Command::SetClipLegato { clip, legato: true })
            .unwrap();
        editor
            .apply(Command::SetClipLaunchQuantization {
                clip,
                quantization: Some(crate::LaunchQuantization::Eighth),
            })
            .unwrap();
        editor
            .apply(Command::SetClipVelocityAmount { clip, amount: 0.75 })
            .unwrap();
        // Spec: follow-actions R1. Follow actions save with the clip and the
        // scene.
        let patch = crate::FollowPatch {
            enabled: Some(true),
            action_b: Some(crate::FollowAction::Any),
            chance_a: Some(40),
            linked: Some(false),
            time: Some(Ticks::WHOLE * 2),
            ..crate::FollowPatch::default()
        };
        editor
            .apply(Command::SetClipFollowActions { clip, patch })
            .unwrap();
        editor
            .apply(Command::SetSceneFollowActions { scene, patch })
            .unwrap();
        let doc = editor.document().clone();
        let loaded = load(serde_json::to_value(&doc).unwrap()).unwrap();
        assert_eq!(loaded, doc);
        assert_eq!(
            loaded.slot(track, scene).unwrap().launch.velocity_amount,
            0.75
        );
        let follow = loaded.slot(track, scene).unwrap().launch.follow;
        assert_eq!((follow.chance_a, follow.linked), (40, false));
        assert_eq!(loaded.scene(scene).unwrap().follow, follow);

        // The set as it was saved without them opens with the defaults.
        let older = load(plain).unwrap();
        assert_eq!(
            older.slot(track, scene).unwrap().launch,
            crate::ClipLaunch::default()
        );
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

    // Spec: session-automation R10
    #[test]
    fn session_automation_envelopes_save_with_the_set_and_load_back() {
        let (mut doc, [_, audio, _]) = song();
        let scene = doc.scenes().next().unwrap().id;
        let clip = ClipId(bl_core::id::Id::new(900).unwrap());
        doc.apply(&Command::CreateSlotClip {
            track: audio,
            scene,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Audio(AudioData::new(ContentHash([0xcd; 32]))),
            ),
        })
        .unwrap();
        let volume = AutomationTarget::Mixer {
            param: crate::MixerParam::Volume,
        };
        for kind in [
            crate::EnvelopeKind::Modulation,
            crate::EnvelopeKind::Automation,
        ] {
            doc.apply(&Command::SetClipEnvelopeBreakpoint {
                kind,
                clip,
                target: volume,
                at: Ticks::ZERO,
                value: 0.5,
            })
            .unwrap();
        }
        // A hand move turns the automation envelope off: it saves so.
        doc.apply(&Command::SetVolume {
            track: audio,
            volume: 0.3,
        })
        .unwrap();
        let value = serde_json::to_value(&doc).unwrap();
        let slots = value["tracks"][0]["children"][0]["slots"].to_string();
        assert!(
            slots.contains(r#""kind":"automation""#) && slots.contains(r#""enabled":false"#),
            "{slots}"
        );
        let loaded = load(value.clone()).unwrap();
        assert_eq!(loaded, doc);
        let slot = loaded.clip(clip).unwrap();
        assert_eq!(slot.envelopes().count(), 2, "one of each kind");

        // An Arrangement clip with an automation envelope doesn't load.
        let mut bad = serde_json::to_value(&doc).unwrap();
        let arrangement = bad["tracks"][0]["children"][0]["clips"][0].clone();
        let mut automation = arrangement;
        automation["envelopes"] = json!([{
            "target": { "type": "mixer", "param": "volume" },
            "envelope": { "points": [{ "at": 0, "value": 0.5 }] },
            "kind": "automation"
        }]);
        bad["tracks"][0]["children"][0]["clips"][0] = automation;
        let error = load(bad).unwrap_err().to_string();
        assert!(error.contains("isn't in a Session slot"), "{error}");
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
                kind: crate::EnvelopeKind::Modulation,
                clip,
                target,
                at,
                value,
            })
            .unwrap();
        }
        doc.apply(&Command::SetClipEnvelopeBreakpoint {
            kind: crate::EnvelopeKind::Modulation,
            clip,
            target: param,
            at: Ticks::ZERO,
            value: -6.0,
        })
        .unwrap();
        // The volume's segment is curved and the envelope is unlinked: both
        // save.
        doc.apply(&Command::SetClipEnvelopeCurve {
            kind: crate::EnvelopeKind::Modulation,
            clip,
            target: volume,
            at: Ticks::ZERO,
            bend: -0.5,
        })
        .unwrap();
        doc.apply(&Command::SetClipEnvelopeLinked {
            kind: crate::EnvelopeKind::Modulation,
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

    // Spec: midi-controller-data R7
    #[test]
    fn controller_envelopes_and_note_pressure_save_with_the_clip_and_load_back() {
        use crate::clip::{MidiData, Note, PressurePoint};
        let mut editor = Editor::new();
        let midi = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: midi,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let clip = editor.new_id();
        let note = Note {
            pressure: vec![
                PressurePoint {
                    at: Ticks::ZERO,
                    value: 90,
                },
                PressurePoint {
                    at: Ticks(500),
                    value: 20,
                },
            ],
            ..Note::new(60, Ticks::ZERO, Ticks::QUARTER, 100)
        };
        editor
            .apply(Command::CreateClip {
                track: midi,
                clip: Clip::new(
                    clip,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Midi(MidiData::new([note])),
                ),
            })
            .unwrap();
        for (target, at, value) in [
            (AutomationTarget::MidiCc { number: 1 }, 0, 12.0),
            (AutomationTarget::MidiCc { number: 1 }, 960, 99.0),
            (AutomationTarget::MidiPitchBend, 0, -8192.0),
            (AutomationTarget::MidiChannelPressure, 0, 127.0),
        ] {
            editor
                .apply(Command::SetClipEnvelopeBreakpoint {
                    clip,
                    kind: crate::EnvelopeKind::Modulation,
                    target,
                    at: Ticks(at),
                    value,
                })
                .unwrap();
        }
        let doc = editor.document().clone();
        let value = serde_json::to_value(&doc).unwrap();
        let saved = find_clip(&value, clip).clone();
        let wheel = saved["envelopes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["target"] == json!({ "type": "midi.cc", "number": 1 }))
            .unwrap();
        assert_eq!(
            wheel["envelope"]["points"],
            json!([{ "at": 0, "value": 12.0 }, { "at": 960, "value": 99.0 }])
        );
        assert!(
            saved["envelopes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["target"] == json!({ "type": "midi.pitch_bend" }))
        );
        assert_eq!(
            saved["data"]["midi"]["notes"][0]["pressure"],
            json!([{ "at": 0, "value": 90 }, { "at": 500, "value": 20 }])
        );
        let loaded = load(value.clone()).unwrap();
        assert_eq!(loaded, doc);
        loaded.check_invariants().unwrap();

        // A set saved before this package has neither, and loads with none.
        let mut old = value.clone();
        let entry = find_clip_mut(&mut old, clip);
        entry.as_object_mut().unwrap().remove("envelopes");
        entry["data"]["midi"]["notes"][0]
            .as_object_mut()
            .unwrap()
            .remove("pressure");
        let back = load(old).unwrap();
        assert_eq!(back.clip(clip).unwrap().envelopes().count(), 0);
        let ClipData::Midi(data) = &back.clip(clip).unwrap().data else {
            panic!("a MIDI clip")
        };
        assert!(data.notes[0].pressure.is_empty());

        // Bad data doesn't load: a controller number a clip can't hold, a
        // value outside the controller's range, a controller on an audio
        // clip, and pressure out of order, outside the note or too strong.
        let broken = |edit: &dyn Fn(&mut Value)| {
            let mut value = value.clone();
            edit(find_clip_mut(&mut value, clip));
            load(value).unwrap_err()
        };
        let error = broken(&|clip| {
            clip["envelopes"][0]["target"] = json!({ "type": "midi.cc", "number": 32 });
        });
        assert!(error.contains("MIDI controller number"), "{error}");
        let error = broken(&|clip| {
            clip["envelopes"][0]["envelope"]["points"] = json!([{ "at": 0, "value": 300.0 }]);
            clip["envelopes"][0]["target"] = json!({ "type": "midi.channel_pressure" });
        });
        assert!(error.contains("out of range"), "{error}");
        for bad in [
            json!([{ "at": 500, "value": 20 }, { "at": 0, "value": 90 }]),
            json!([{ "at": 5_000_000, "value": 20 }]),
            json!([{ "at": 0, "value": 128 }]),
        ] {
            let error = broken(&|clip| {
                clip["data"]["midi"]["notes"][0]["pressure"] = bad.clone();
            });
            assert!(error.contains("note"), "{bad}: {error}");
        }
        let (with_audio, [_, audio, _]) = song();
        let audio_clip = with_audio.track(audio).unwrap().clips().next().unwrap().id;
        let mut audio = serde_json::to_value(&with_audio).unwrap();
        find_clip_mut(&mut audio, audio_clip)["envelopes"] = json!([{
            "target": { "type": "midi.cc", "number": 1 },
            "envelope": { "points": [{ "at": 0, "value": 1.0 }] }
        }]);
        let error = load(audio).unwrap_err();
        assert!(error.contains("audio clip"), "{error}");
    }

    /// Where a clip's saved form sits in the tree of tracks, as a JSON
    /// pointer.
    fn clip_pointer(value: &Value, clip: ClipId) -> String {
        fn walk(value: &Value, id: &Value, path: &str) -> Option<String> {
            match value {
                Value::Object(map) => {
                    if map.get("id") == Some(id) && map.contains_key("data") {
                        return Some(path.to_owned());
                    }
                    map.iter()
                        .find_map(|(key, v)| walk(v, id, &format!("{path}/{key}")))
                }
                Value::Array(items) => items
                    .iter()
                    .enumerate()
                    .find_map(|(i, v)| walk(v, id, &format!("{path}/{i}"))),
                _ => None,
            }
        }
        walk(value, &json!(clip), "").expect("the clip is saved")
    }

    fn find_clip(value: &Value, clip: ClipId) -> &Value {
        value.pointer(&clip_pointer(value, clip)).unwrap()
    }

    fn find_clip_mut(value: &mut Value, clip: ClipId) -> &mut Value {
        let pointer = clip_pointer(value, clip);
        value.pointer_mut(&pointer).unwrap()
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
    // Spec: external-devices R2, R15
    #[test]
    fn a_midi_to_port_saves_and_a_set_without_one_opens_as_before() {
        use crate::external::HardwareLatency;
        use crate::track::MidiTo;

        let mut editor = Editor::new();
        let mut track = |kind| {
            let id = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id,
                    kind,
                    name: "T".to_owned(),
                    parent: None,
                    index: 0,
                })
                .unwrap();
            id
        };
        let (keys, bass) = (track(TrackKind::Midi), track(TrackKind::Midi));
        let outboard = editor.new_id();
        let mut set = |to| {
            editor
                .apply(Command::SetMidiOutput { track: keys, to })
                .unwrap();
            serde_json::to_value(editor.document()).unwrap()
        };
        // A set saved before MIDI out: no MIDI To is left out, a track is its
        // bare ID.
        let none = set(None);
        let routing = |value: &Value| value["tracks"][1]["routing"]["midi_output"].clone();
        assert_eq!(routing(&none), Value::Null);
        let by_track = set(Some(MidiTo::Track(bass)));
        assert_eq!(routing(&by_track), json!(bass.0.get()));
        let by_port = set(Some(MidiTo::Port {
            name: "Synth".to_owned(),
            channel: 3,
        }));
        assert_eq!(routing(&by_port), json!({ "name": "Synth", "channel": 3 }));
        for (value, expected) in [
            (none, None),
            (by_track, Some(MidiTo::Track(bass))),
            (
                by_port,
                Some(MidiTo::Port {
                    name: "Synth".to_owned(),
                    channel: 3,
                }),
            ),
        ] {
            let back = load(value).unwrap();
            assert_eq!(back.track(keys).unwrap().routing().midi_output, expected);
            back.check_invariants().unwrap();
        }
        // A port needs a name and a channel from 1 to 16.
        for (name, channel) in [("", 1), ("Synth", 0), ("Synth", 17)] {
            let to = Some(MidiTo::Port {
                name: name.to_owned(),
                channel,
            });
            assert!(
                editor
                    .apply(Command::SetMidiOutput { track: keys, to })
                    .is_err()
            );
        }

        // The external device's settings save with the set and load back.
        editor
            .apply(Command::InsertDevice {
                track: keys,
                index: 0,
                device: Device::new(
                    outboard,
                    "Hardware",
                    DeviceKind::Native {
                        device_type: "beatlayer.test".to_owned(),
                    },
                )
                .with_external(),
            })
            .unwrap();
        let saved = serde_json::to_value(editor.document()).unwrap();
        assert_eq!(
            saved["tracks"][1]["devices"][0]["external"],
            json!({}),
            "a new device's routing is all defaults"
        );
        editor
            .apply(Command::SetExternal {
                device: outboard,
                midi_to: None,
                channel: Some(2),
                audio_from: None,
                hardware_latency: Some(HardwareLatency::Samples { value: 64 }),
            })
            .unwrap();
        let back = load(serde_json::to_value(editor.document()).unwrap()).unwrap();
        assert_eq!(&back, editor.document());
        let external = back.device(outboard).unwrap().external.clone().unwrap();
        assert_eq!(external.channel, 2);
        assert_eq!(
            external.hardware_latency,
            HardwareLatency::Samples { value: 64 }
        );
    }

    // Spec: macros R31
    #[test]
    fn variations_save_with_the_set_and_load_back() {
        use crate::{Chain, RackKind, macro_param};
        let mut editor = Editor::new();
        let track: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: "Audio".to_owned(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let rack: DeviceId = editor.new_id();
        let chain = Chain::new(editor.new_id(), "Chain");
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: Device::rack(rack, RackKind::AudioEffect).with_chain(chain),
            })
            .unwrap();
        editor
            .apply(Command::SetDeviceParam {
                device: rack,
                param: macro_param(1),
                value: 0.25,
            })
            .unwrap();
        editor.apply(Command::StoreVariation { rack }).unwrap();
        editor
            .apply(Command::SetDeviceParam {
                device: rack,
                param: macro_param(1),
                value: 0.75,
            })
            .unwrap();
        editor.apply(Command::StoreVariation { rack }).unwrap();
        editor
            .apply(Command::RenameVariation {
                rack,
                index: 0,
                name: "Quiet".to_owned(),
            })
            .unwrap();
        editor
            .apply(Command::SetMacroVariationExcluded {
                rack,
                macro_number: 2,
                excluded: true,
            })
            .unwrap();

        let saved = editor.document().clone();
        let back = load(serde_json::to_value(&saved).unwrap()).expect("the set loads");
        let rack_back = back.device(rack).unwrap();
        assert_eq!(rack_back.macros.variations.len(), 2);
        assert_eq!(rack_back.macros.variations[0].name, "Quiet");
        assert_eq!(rack_back.macros.variations[0].values[0], 0.25);
        assert_eq!(rack_back.macros.variations[1].name, "Variation 2");
        assert_eq!(rack_back.macros.variations[1].values[0], 0.75);
        assert!(rack_back.macros.setting(2).variation_excluded);

        // Spec: macros R31. A stored value outside 0 to 1 doesn't load.
        fn set_first_value(value: &mut Value, to: f64) -> bool {
            match value {
                Value::Object(map) => {
                    if let Some(Value::Array(variations)) = map.get_mut("variations")
                        && let Some(Value::Object(first)) = variations.first_mut()
                        && let Some(Value::Array(values)) = first.get_mut("values")
                        && let Some(slot) = values.first_mut()
                    {
                        *slot = json!(to);
                        return true;
                    }
                    map.values_mut().any(|value| set_first_value(value, to))
                }
                Value::Array(items) => items.iter_mut().any(|value| set_first_value(value, to)),
                _ => false,
            }
        }
        let mut broken = serde_json::to_value(&saved).unwrap();
        assert!(
            set_first_value(&mut broken, 2.0),
            "the variations are saved"
        );
        assert!(load(broken).is_err(), "a value outside 0 to 1 doesn't load");
    }
}
