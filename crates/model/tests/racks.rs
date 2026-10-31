//! Racks in the document (WP-5.1; the racks spec, R1 to R14).

use bl_core::time::Ticks;
use bl_model::{
    AutomationTarget, Chain, ChainId, ChainZones, Command, ContentHash, Device, DeviceId,
    DeviceKind, DeviceRole, Editor, FadeMode, Layer, LayerId, MixerParam, ModelError, ParamId,
    RackKind, RemoteMapping, RemoteSource, RemoteTarget, TrackId, TrackKind, Zone, ZoneKind,
};

fn rejected(editor: &mut Editor, command: Command) -> ModelError {
    match editor.apply(command) {
        Err(bl_model::EditError::Command(error)) => error,
        other => panic!("expected the document to refuse it, got {other:?}"),
    }
}

struct Song {
    editor: Editor,
    audio: TrackId,
    midi: TrackId,
}

impl Song {
    fn new() -> Self {
        let mut editor = Editor::new();
        let mut track = |kind, name: &str, index| {
            let id: TrackId = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id,
                    kind,
                    name: name.to_owned(),
                    parent: None,
                    index,
                })
                .unwrap();
            id
        };
        let audio = track(TrackKind::Audio, "Audio", 0);
        let midi = track(TrackKind::Midi, "Midi", 1);
        Self {
            editor,
            audio,
            midi,
        }
    }

    fn device(&mut self, role: DeviceRole) -> Device {
        Device::new(
            self.editor.new_id(),
            format!("{role:?}"),
            DeviceKind::Native {
                device_type: format!("test.{role:?}"),
            },
        )
        .with_role(role)
    }

    fn rack(&mut self, kind: RackKind, chains: usize) -> Device {
        let mut rack = Device::rack(self.editor.new_id(), kind);
        for n in 0..chains {
            rack = rack.with_chain(Chain::new(self.editor.new_id(), format!("Chain {n}")));
        }
        rack
    }

    fn insert(&mut self, track: TrackId, index: usize, device: Device) -> Result<(), ModelError> {
        match self.editor.apply(Command::InsertDevice {
            track,
            index,
            device,
        }) {
            Ok(_) => Ok(()),
            Err(bl_model::EditError::Command(error)) => Err(error),
            Err(other) => panic!("{other:?}"),
        }
    }

    fn insert_in(
        &mut self,
        chain: ChainId,
        index: usize,
        device: Device,
    ) -> Result<(), ModelError> {
        match self.editor.apply(Command::InsertChainDevice {
            chain,
            index,
            device,
        }) {
            Ok(_) => Ok(()),
            Err(bl_model::EditError::Command(error)) => Err(error),
            Err(other) => panic!("{other:?}"),
        }
    }

    fn doc(&self) -> &bl_model::Document {
        self.editor.document()
    }

    fn checked(&self) {
        self.doc().check_invariants().unwrap();
    }
}

// Spec: racks R1
#[test]
fn each_kind_holds_only_what_fits_in_lives_order() {
    let mut song = Song::new();
    let audio_rack = song.rack(RackKind::AudioEffect, 1);
    let audio_chain = audio_rack.chains[0].id;
    song.insert(song.audio, 0, audio_rack).unwrap();
    let midi_rack = song.rack(RackKind::MidiEffect, 1);
    let midi_chain = midi_rack.chains[0].id;
    song.insert(song.midi, 0, midi_rack).unwrap();
    let instrument_rack = song.rack(RackKind::Instrument, 1);
    let instrument_chain = instrument_rack.chains[0].id;
    song.insert(song.midi, 1, instrument_rack).unwrap();
    let drum_rack = song.rack(RackKind::Drum, 1);
    let drum_chain = drum_rack.chains[0].id;
    song.insert(song.midi, 2, drum_rack).unwrap_err();
    // A drum rack is an instrument, so the chain already holds one.
    assert!(song.doc().chain(drum_chain).is_none());

    // An audio effect rack's chains hold only audio effects.
    for role in [DeviceRole::MidiEffect, DeviceRole::Instrument] {
        let device = song.device(role);
        assert!(matches!(
            song.insert_in(audio_chain, 0, device),
            Err(ModelError::ChainDoesNotFit { .. })
        ));
    }
    let fx = song.device(DeviceRole::AudioEffect);
    song.insert_in(audio_chain, 0, fx).unwrap();

    // A MIDI effect rack's chains hold only MIDI effects.
    for role in [DeviceRole::AudioEffect, DeviceRole::Instrument] {
        let device = song.device(role);
        assert!(matches!(
            song.insert_in(midi_chain, 0, device),
            Err(ModelError::ChainDoesNotFit { .. })
        ));
    }
    let arp = song.device(DeviceRole::MidiEffect);
    song.insert_in(midi_chain, 0, arp).unwrap();

    // An instrument rack's chain keeps Live's order: MIDI effects, then at
    // most one instrument, then audio effects.
    let synth = song.device(DeviceRole::Instrument);
    song.insert_in(instrument_chain, 0, synth).unwrap();
    let second = song.device(DeviceRole::Instrument);
    assert!(matches!(
        song.insert_in(instrument_chain, 1, second),
        Err(ModelError::ChainOrder { .. })
    ));
    let fx = song.device(DeviceRole::AudioEffect);
    assert!(matches!(
        song.insert_in(instrument_chain, 0, fx.clone()),
        Err(ModelError::ChainOrder { .. })
    ));
    song.insert_in(instrument_chain, 1, fx).unwrap();
    let arp = song.device(DeviceRole::MidiEffect);
    song.insert_in(instrument_chain, 0, arp).unwrap();
    let roles: Vec<DeviceRole> = song
        .doc()
        .chain(instrument_chain)
        .unwrap()
        .devices
        .iter()
        .map(|d| d.role)
        .collect();
    assert_eq!(
        roles,
        [
            DeviceRole::MidiEffect,
            DeviceRole::Instrument,
            DeviceRole::AudioEffect
        ]
    );

    // A rack's role follows its kind: the saved form of one that doesn't is
    // refused.
    let mut value = serde_json::to_value(song.doc()).unwrap();
    value["tracks"][0]["devices"][0]["role"] = serde_json::json!("instrument");
    assert!(serde_json::from_value::<bl_model::Document>(value).is_err());
    song.checked();
}

// Spec: racks R2
#[test]
fn racks_go_where_their_role_goes_and_nest() {
    let mut song = Song::new();
    for kind in [RackKind::Instrument, RackKind::MidiEffect, RackKind::Drum] {
        let rack = song.rack(kind, 1);
        assert!(matches!(
            song.insert(song.audio, 0, rack),
            Err(ModelError::DeviceDoesNotFit { .. })
        ));
    }
    // An audio effect rack goes on any track, after the instrument on a MIDI
    // track.
    let synth = song.device(DeviceRole::Instrument);
    song.insert(song.midi, 0, synth).unwrap();
    let before = song.rack(RackKind::AudioEffect, 1);
    assert!(matches!(
        song.insert(song.midi, 0, before),
        Err(ModelError::DeviceOrder { .. })
    ));
    let after = song.rack(RackKind::AudioEffect, 1);
    song.insert(song.midi, 1, after).unwrap();
    let on_audio = song.rack(RackKind::AudioEffect, 1);
    song.insert(song.audio, 0, on_audio).unwrap();

    // Racks nest to any depth where their role fits.
    let outer = song.rack(RackKind::AudioEffect, 1);
    let mut chain = outer.chains[0].id;
    song.insert(song.audio, 1, outer.clone()).unwrap();
    let mut depth = vec![outer.id];
    for _ in 0..5 {
        let inner = song.rack(RackKind::AudioEffect, 1);
        let next = inner.chains[0].id;
        depth.push(inner.id);
        song.insert_in(chain, 0, inner).unwrap();
        chain = next;
    }
    let leaf = song.device(DeviceRole::AudioEffect);
    let leaf_id = leaf.id;
    song.insert_in(chain, 0, leaf).unwrap();
    assert_eq!(song.doc().device_track(leaf_id), Some(song.audio));
    assert_eq!(song.doc().device_place(leaf_id).unwrap().chain, Some(chain));
    // A role that doesn't fit the chain it's put in is refused, a rack's as
    // another's.
    let misfit = song.rack(RackKind::MidiEffect, 1);
    assert!(matches!(
        song.insert_in(chain, 0, misfit),
        Err(ModelError::ChainDoesNotFit { .. })
    ));
    // An instrument rack nests in an instrument rack's chain, in the
    // instrument's place: the MIDI track's chain already holds an instrument.
    let held = song
        .doc()
        .track(song.midi)
        .unwrap()
        .devices()
        .next()
        .unwrap()
        .id;
    song.editor
        .apply(Command::RemoveDevice { device: held })
        .unwrap();
    let outer = song.rack(RackKind::Instrument, 1);
    let (outer_chain, outer_id) = (outer.chains[0].id, outer.id);
    song.insert(song.midi, 0, outer).unwrap();
    let nested = song.rack(RackKind::Instrument, 1);
    let nested_chain = nested.chains[0].id;
    song.insert_in(outer_chain, 0, nested).unwrap();
    let second = song.rack(RackKind::Drum, 1);
    assert!(matches!(
        song.insert_in(outer_chain, 1, second),
        Err(ModelError::ChainOrder { .. })
    ));
    // A rack can't be moved into its own chains.
    let error = rejected(
        &mut song.editor,
        Command::MoveDeviceToChain {
            device: outer_id,
            chain: nested_chain,
            index: 0,
        },
    );
    assert_eq!(error, ModelError::RackIntoItself(outer_id));
    song.checked();
}

// Spec: racks R6
#[test]
fn chain_mixer_controls_are_automation_targets() {
    let mut song = Song::new();
    let rack = song.rack(RackKind::AudioEffect, 2);
    let (a, b) = (rack.chains[0].id, rack.chains[1].id);
    song.insert(song.audio, 0, rack).unwrap();
    let volume = AutomationTarget::Chain {
        chain: a,
        param: MixerParam::Volume,
    };
    let pan = AutomationTarget::Chain {
        chain: a,
        param: MixerParam::Pan,
    };
    let activator = AutomationTarget::Chain {
        chain: b,
        param: MixerParam::Mute,
    };
    for (target, value) in [(volume, 0.5), (pan, -1.0), (activator, 1.0)] {
        song.editor
            .apply(Command::SetAutomationBreakpoint {
                track: song.audio,
                target,
                at: Ticks::ZERO,
                value,
            })
            .unwrap();
    }
    // Moving a control by hand turns its envelope off, as a track's does.
    song.editor
        .apply(Command::SetChainVolume {
            chain: a,
            volume: 0.25,
        })
        .unwrap();
    let track = song.doc().track(song.audio).unwrap();
    assert!(!track.automation_for(&volume).unwrap().enabled);
    assert!(track.automation_for(&pan).unwrap().enabled);
    // A chain's envelopes go with the chain, and a chain of another track
    // is no target.
    song.editor
        .apply(Command::RemoveChain { chain: b })
        .unwrap();
    assert!(
        song.doc()
            .track(song.audio)
            .unwrap()
            .automation_for(&activator)
            .is_none()
    );
    let error = rejected(
        &mut song.editor,
        Command::SetAutomationBreakpoint {
            track: song.midi,
            target: volume,
            at: Ticks::ZERO,
            value: 1.0,
        },
    );
    assert!(matches!(error, ModelError::ChainNotInTrack { .. }));
    // A MIDI effect rack's chains have no volume or pan.
    let midi_rack = song.rack(RackKind::MidiEffect, 1);
    let midi_chain = midi_rack.chains[0].id;
    song.insert(song.midi, 0, midi_rack).unwrap();
    assert_eq!(
        rejected(
            &mut song.editor,
            Command::SetChainVolume {
                chain: midi_chain,
                volume: 0.5,
            }
        ),
        ModelError::ChainHasNoMixer(midi_chain)
    );
    song.editor
        .apply(Command::SetChainEnabled {
            chain: midi_chain,
            enabled: false,
        })
        .unwrap();
    song.checked();
}

// Spec: racks R8, R13
#[test]
fn chain_edits_are_one_undo_step_each_and_undo_whole() {
    let mut song = Song::new();
    let rack = song.rack(RackKind::AudioEffect, 1);
    let (rack_id, first) = (rack.id, rack.chains[0].id);
    song.insert(song.audio, 0, rack).unwrap();
    let fx = song.device(DeviceRole::AudioEffect);
    let fx_id = fx.id;
    song.insert_in(first, 0, fx).unwrap();
    let second: ChainId = song.editor.new_id();
    let tail: DeviceId = song.editor.new_id();
    let edits = [
        Command::AddChain {
            rack: rack_id,
            index: 1,
            chain: Chain::new(second, "Chain").with_devices(vec![Device::new(
                tail,
                "Tail",
                DeviceKind::Native {
                    device_type: "test.tail".to_owned(),
                },
            )]),
        },
        Command::RenameChain {
            chain: second,
            name: "Wet".to_owned(),
        },
        Command::SetChainColour {
            chain: second,
            colour: Some(69),
        },
        Command::SetChainInfo {
            chain: second,
            info: "reverb".to_owned(),
        },
        Command::SetChainVolume {
            chain: second,
            volume: 0.5,
        },
        Command::SetChainPan {
            chain: second,
            pan: 0.5,
        },
        Command::SetChainEnabled {
            chain: second,
            enabled: false,
        },
        Command::SetChainSolo {
            chain: second,
            solo: true,
        },
        Command::MoveChain {
            chain: second,
            rack: rack_id,
            index: 0,
        },
        Command::MoveDeviceToChain {
            device: fx_id,
            chain: second,
            index: 0,
        },
        Command::RemoveChain { chain: first },
    ];
    let before = song.editor.undo_history().len();
    let mut seen = vec![song.doc().clone()];
    for edit in edits {
        song.editor.apply(edit).unwrap();
        song.checked();
        seen.push(song.doc().clone());
    }
    assert_eq!(song.editor.undo_history().len(), before + 11);
    let chain = song.doc().chain(second).unwrap();
    assert_eq!(chain.name, "Wet");
    assert_eq!(chain.colour, Some(69));
    assert_eq!(chain.devices.len(), 2);
    assert_eq!(chain.devices[0].id, fx_id);
    // Each edit undoes whole, in turn.
    for before in seen.iter().rev().skip(1) {
        song.editor.undo().unwrap();
        assert_eq!(song.doc(), before);
        song.checked();
    }
    // Out-of-range values are refused and change nothing.
    for bad in [
        Command::SetChainColour {
            chain: first,
            colour: Some(70),
        },
        Command::SetChainVolume {
            chain: first,
            volume: -1.0,
        },
        Command::SetChainPan {
            chain: first,
            pan: 1.5,
        },
        Command::AddChain {
            rack: rack_id,
            index: 9,
            chain: Chain::new(song.editor.new_id(), "Chain"),
        },
        Command::AddChain {
            rack: rack_id,
            index: 0,
            chain: Chain::new(first, "Reused"),
        },
    ] {
        let was = song.doc().clone();
        assert!(song.editor.apply(bad).is_err());
        assert_eq!(song.doc(), &was);
    }
}

// Spec: racks R9, R11, R14
#[test]
fn group_and_ungroup_keep_every_device_and_its_settings() {
    let mut song = Song::new();
    let mut ids = Vec::new();
    for (n, role) in [
        DeviceRole::MidiEffect,
        DeviceRole::Instrument,
        DeviceRole::AudioEffect,
        DeviceRole::AudioEffect,
    ]
    .into_iter()
    .enumerate()
    {
        let mut device = song.device(role);
        device.params.insert(ParamId(1), n as f64);
        ids.push(device.id);
        song.insert(song.midi, n, device).unwrap();
    }
    let rack = song.rack(RackKind::Instrument, 0);
    let chain: ChainId = song.editor.new_id();
    let (rack_id, rack) = (rack.id, rack.with_chain(Chain::new(chain, "Group")));
    let before = song.doc().clone();
    song.editor
        .apply(Command::GroupDevices {
            devices: vec![ids[1], ids[2]],
            rack,
        })
        .unwrap();
    song.checked();
    let track = song.doc().track(song.midi).unwrap();
    let order: Vec<DeviceId> = track.devices().map(|d| d.id).collect();
    assert_eq!(order, [ids[0], rack_id, ids[3]]);
    let grouped: Vec<DeviceId> = song
        .doc()
        .chain(chain)
        .unwrap()
        .devices
        .iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(grouped, [ids[1], ids[2]]);
    assert_eq!(
        song.doc().device(ids[2]).unwrap().params.get(&ParamId(1)),
        Some(&2.0)
    );
    assert_eq!(song.editor.undo_history().next(), Some("Group"));

    // Ungroup puts them back in the rack's place, and takes the chain with
    // it.
    song.editor
        .apply(Command::UngroupDevice { device: rack_id })
        .unwrap();
    song.checked();
    let order: Vec<DeviceId> = song
        .doc()
        .track(song.midi)
        .unwrap()
        .devices()
        .map(|d| d.id)
        .collect();
    assert_eq!(order, ids);
    assert!(song.doc().device(rack_id).is_none());
    assert!(song.doc().chain(chain).is_none());
    assert_eq!(song.editor.undo_history().next(), Some("Ungroup"));
    assert_eq!(song.doc().track(song.midi), before.track(song.midi));
    song.editor.undo().unwrap();
    song.editor.undo().unwrap();
    assert_eq!(song.doc(), &before);

    // An instrument rack can't take the instrument's place beside a second
    // instrument: grouping an audio effect into an instrument rack in front
    // of the instrument breaks the order.
    let rack = song.rack(RackKind::Instrument, 0);
    let rack = rack.with_chain(Chain::new(song.editor.new_id(), "Group"));
    assert!(matches!(
        rejected(
            &mut song.editor,
            Command::GroupDevices {
                devices: vec![ids[2], ids[3]],
                rack,
            }
        ),
        ModelError::DeviceOrder { .. }
    ));
    // The devices must be in one chain.
    let nested = song.rack(RackKind::AudioEffect, 1);
    let nested_chain = nested.chains[0].id;
    song.insert(song.midi, 4, nested).unwrap();
    let inside = song.device(DeviceRole::AudioEffect);
    let inside_id = inside.id;
    song.insert_in(nested_chain, 0, inside).unwrap();
    let rack = song.rack(RackKind::AudioEffect, 0);
    let rack = rack.with_chain(Chain::new(song.editor.new_id(), "Group"));
    assert!(matches!(
        rejected(
            &mut song.editor,
            Command::GroupDevices {
                devices: vec![ids[3], inside_id],
                rack,
            }
        ),
        ModelError::BadGroup(_)
    ));
    // A rack of several chains can't be taken apart.
    let several = song.rack(RackKind::AudioEffect, 2);
    let several_id = several.id;
    song.insert(song.audio, 0, several).unwrap();
    assert!(matches!(
        rejected(
            &mut song.editor,
            Command::UngroupDevice { device: several_id }
        ),
        ModelError::BadGroup(_)
    ));
    // An empty rack is just removed.
    let empty = song.rack(RackKind::AudioEffect, 0);
    let empty_id = empty.id;
    song.insert(song.audio, 0, empty).unwrap();
    song.editor
        .apply(Command::UngroupDevice { device: empty_id })
        .unwrap();
    assert!(song.doc().device(empty_id).is_none());
    song.checked();
}

// Spec: racks R7
#[test]
fn a_device_moves_between_a_chain_and_a_track_and_between_racks() {
    let mut song = Song::new();
    let first = song.rack(RackKind::AudioEffect, 1);
    let second = song.rack(RackKind::AudioEffect, 1);
    let (a, b) = (first.chains[0].id, second.chains[0].id);
    song.insert(song.audio, 0, first).unwrap();
    song.insert(song.audio, 1, second).unwrap();
    let mut fx = song.device(DeviceRole::AudioEffect);
    fx.params.insert(ParamId(3), 0.75);
    let id = fx.id;
    song.insert_in(a, 0, fx).unwrap();
    song.editor
        .apply(Command::SetDeviceParam {
            device: id,
            param: ParamId(7),
            value: 0.5,
        })
        .unwrap();
    // Between racks, then out onto the track, then to another track.
    song.editor
        .apply(Command::MoveDeviceToChain {
            device: id,
            chain: b,
            index: 0,
        })
        .unwrap();
    assert_eq!(song.doc().device_place(id).unwrap().chain, Some(b));
    song.editor
        .apply(Command::MoveDevice {
            device: id,
            track: song.audio,
            index: 2,
        })
        .unwrap();
    assert_eq!(song.doc().device_place(id).unwrap().chain, None);
    song.editor
        .apply(Command::MoveDevice {
            device: id,
            track: song.midi,
            index: 0,
        })
        .unwrap();
    let device = song.doc().device(id).unwrap();
    assert_eq!(device.params.get(&ParamId(3)), Some(&0.75));
    assert_eq!(device.params.get(&ParamId(7)), Some(&0.5));
    assert_eq!(song.doc().device_track(id), Some(song.midi));
    song.checked();
    // Edits reach a device inside a rack.
    song.editor
        .apply(Command::SetDeviceEnabled {
            device: id,
            enabled: false,
        })
        .unwrap();
    let nested = song.device(DeviceRole::AudioEffect);
    let nested_id = nested.id;
    song.insert_in(a, 0, nested).unwrap();
    for command in [
        Command::SetDeviceEnabled {
            device: nested_id,
            enabled: false,
        },
        Command::RenameDevice {
            device: nested_id,
            name: "Deep".to_owned(),
        },
        Command::SetDeviceParam {
            device: nested_id,
            param: ParamId(1),
            value: 0.25,
        },
    ] {
        song.editor.apply(command).unwrap();
    }
    let nested = song.doc().device(nested_id).unwrap();
    assert!(!nested.enabled);
    assert_eq!(nested.name, "Deep");
    assert_eq!(nested.params.get(&ParamId(1)), Some(&0.25));
    song.editor
        .apply(Command::RemoveDevice { device: nested_id })
        .unwrap();
    assert!(song.doc().device(nested_id).is_none());
    song.checked();
}

// Spec: racks R12
#[test]
fn switching_a_rack_with_its_contents_reaches_every_depth_in_one_step() {
    let mut song = Song::new();
    let outer = song.rack(RackKind::AudioEffect, 1);
    let outer_id = outer.id;
    let chain = outer.chains[0].id;
    song.insert(song.audio, 0, outer).unwrap();
    let inner = song.rack(RackKind::AudioEffect, 1);
    let (inner_id, inner_chain) = (inner.id, inner.chains[0].id);
    song.insert_in(chain, 0, inner).unwrap();
    let mut ids = Vec::new();
    for chain in [chain, inner_chain] {
        let device = song.device(DeviceRole::AudioEffect);
        ids.push(device.id);
        song.insert_in(chain, 0, device).unwrap();
    }
    song.editor
        .apply(Command::SetDeviceEnabled {
            device: ids[0],
            enabled: false,
        })
        .unwrap();
    let steps = song.editor.undo_history().len();
    song.editor
        .apply(Command::SetDeviceEnabledAll {
            device: outer_id,
            enabled: true,
        })
        .unwrap();
    assert_eq!(song.editor.undo_history().len(), steps + 1);
    for id in ids.iter().chain(&[outer_id, inner_id]) {
        assert!(song.doc().device(*id).unwrap().enabled);
    }
    song.editor
        .apply(Command::SetDeviceEnabledAll {
            device: outer_id,
            enabled: false,
        })
        .unwrap();
    for id in ids.iter().chain(&[outer_id, inner_id]) {
        assert!(!song.doc().device(*id).unwrap().enabled);
    }
    song.editor.undo().unwrap();
    assert!(song.doc().device(ids[1]).unwrap().enabled);
}

#[test]
fn deleting_a_track_forgets_everything_inside_its_racks() {
    let mut song = Song::new();
    let outer = song.rack(RackKind::AudioEffect, 1);
    let chain = outer.chains[0].id;
    song.insert(song.audio, 0, outer).unwrap();
    let inner = song.rack(RackKind::AudioEffect, 1);
    let inner_chain = inner.chains[0].id;
    song.insert_in(chain, 0, inner).unwrap();
    let effect = song.device(DeviceRole::AudioEffect);
    song.insert_in(inner_chain, 0, effect).unwrap();

    song.editor
        .apply(Command::DeleteTrack { track: song.audio })
        .unwrap();
    song.checked();
    assert_eq!(song.doc().device_count(), 0);
    assert_eq!(song.doc().chain_ids().len(), 0);
}

// Spec: racks R30
#[test]
fn new_zones_pass_everything_and_keep_their_order() {
    let mut song = Song::new();
    let instrument = song.rack(RackKind::Instrument, 1);
    let instrument_chain = instrument.chains[0].id;
    song.insert(song.midi, 0, instrument).unwrap();
    let effect = song.rack(RackKind::AudioEffect, 1);
    let effect_chain = effect.chains[0].id;
    song.insert(song.audio, 0, effect).unwrap();

    // A new chain passes every note and velocity and holds selector 0.
    let zones = song.doc().chain(instrument_chain).unwrap().zones;
    assert_eq!(zones, ChainZones::default());
    assert_eq!(zones.key, Zone::new(0, 127));
    assert_eq!(zones.velocity, Zone::new(1, 127));
    assert_eq!(zones.select, Zone::new(0, 0));

    // Each zone is set alone, as one undo step, and keeps the order: a
    // value of its scale, and fades that fit inside it.
    let steps = song.editor.undo_history().len();
    let faded = Zone {
        low: 20,
        high: 100,
        fade_low: 30,
        fade_high: 50,
    };
    for (kind, zone) in [
        (ZoneKind::Key, faded),
        (ZoneKind::Velocity, Zone::new(10, 120)),
        (ZoneKind::Select, Zone::new(3, 9)),
    ] {
        song.editor
            .apply(Command::SetChainZone {
                chain: instrument_chain,
                kind,
                zone,
            })
            .unwrap();
        song.checked();
    }
    assert_eq!(song.editor.undo_history().len(), steps + 3);
    let zones = song.doc().chain(instrument_chain).unwrap().zones;
    assert_eq!(
        (zones.key, zones.velocity, zones.select),
        (faded, Zone::new(10, 120), Zone::new(3, 9))
    );
    // Setting a zone to what it is changes nothing.
    let before = song.doc().clone();
    song.editor
        .apply(Command::SetChainZone {
            chain: instrument_chain,
            kind: ZoneKind::Select,
            zone: Zone::new(3, 9),
        })
        .unwrap();
    assert_eq!(song.doc(), &before);

    // Zones out of order, off their scale or with fades too wide for them
    // are refused and change nothing.
    for (kind, zone) in [
        (ZoneKind::Key, Zone::new(50, 40)),
        (ZoneKind::Key, Zone::new(0, 128)),
        (ZoneKind::Velocity, Zone::new(0, 100)),
        (
            ZoneKind::Select,
            Zone {
                low: 0,
                high: 10,
                fade_low: 6,
                fade_high: 6,
            },
        ),
    ] {
        let error = rejected(
            &mut song.editor,
            Command::SetChainZone {
                chain: instrument_chain,
                kind,
                zone,
            },
        );
        assert!(matches!(error, ModelError::InvalidInput(_)), "{error:?}");
    }
    assert_eq!(song.doc(), &before);

    // An audio effect rack's chains have only a chain select zone.
    for kind in [ZoneKind::Key, ZoneKind::Velocity] {
        let error = rejected(
            &mut song.editor,
            Command::SetChainZone {
                chain: effect_chain,
                kind,
                zone: Zone::new(1, 5),
            },
        );
        assert!(
            matches!(error, ModelError::ChainHasNoZone { .. }),
            "{error:?}"
        );
    }
    song.editor
        .apply(Command::SetChainZone {
            chain: effect_chain,
            kind: ZoneKind::Select,
            zone: Zone::new(2, 4),
        })
        .unwrap();

    // The zones save with the set, only where they differ from a new
    // chain's, and load back; a file whose audio effect rack has a key zone
    // is refused.
    let value = serde_json::to_value(song.doc()).unwrap();
    let saved = &value["tracks"][0]["devices"][0]["chains"][0];
    assert_eq!(
        saved["zones"],
        serde_json::json!({ "select": { "low": 2, "high": 4 } })
    );
    let saved = &value["tracks"][1]["devices"][0]["chains"][0];
    assert_eq!(
        saved["zones"]["key"],
        serde_json::json!({ "low": 20, "high": 100, "fade_low": 30, "fade_high": 50 })
    );
    let back: bl_model::Document = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(&back, song.doc());
    back.check_invariants().unwrap();
    let mut broken = value;
    broken["tracks"][0]["devices"][0]["chains"][0]["zones"]["key"] =
        serde_json::json!({ "low": 1, "high": 5 });
    let broken: Result<bl_model::Document, _> = serde_json::from_value(broken);
    assert!(
        broken.map_or(true, |doc| doc.check_invariants().is_err()),
        "an audio effect rack's chain has no key zone"
    );

    // Every undo step goes back whole.
    while song.editor.undo_history().len() > steps {
        song.editor.undo().unwrap();
        song.checked();
    }
    assert_eq!(
        song.doc().chain(instrument_chain).unwrap().zones,
        ChainZones::default()
    );
}

/// R32's share, as the rule words it and not as the model works it out.
fn share(zone: Zone, value: u8) -> f32 {
    let (low, high) = (i32::from(zone.low), i32::from(zone.high));
    let (n, m) = (i32::from(zone.fade_low), i32::from(zone.fade_high));
    let v = i32::from(value);
    if v < low || v > high {
        0.0
    } else if v < low + n {
        (v - low) as f32 / n as f32
    } else if v > high - m {
        (high - v) as f32 / m as f32
    } else {
        1.0
    }
}

// Spec: racks R32, multisampler R5
#[test]
fn a_racks_fades_and_a_multisamplers_give_the_same_share_at_every_value() {
    let layer = |zone: Zone| {
        let mut layer = Layer::new(LayerId(1), "Layer", ContentHash([0; 32]), 1000, 48_000, 1);
        layer.key = zone;
        layer
    };
    for (low, high) in [(0u8, 127u8), (10, 20), (33, 33), (0, 1), (60, 127)] {
        for fade_low in (0..=high - low).step_by(7) {
            for fade_high in (0..=high - low - fade_low).step_by(5) {
                let zone = Zone {
                    low,
                    high,
                    fade_low,
                    fade_high,
                };
                assert!(zone.is_valid(ZoneKind::Key));
                let rack = ChainZones::default().with(ZoneKind::Key, zone);
                for value in 0..=127u8 {
                    let expected = share(zone, value);
                    assert_eq!(
                        rack.key.gain(value, FadeMode::Lin),
                        expected,
                        "a rack's {zone:?} at {value}"
                    );
                    assert_eq!(
                        layer(zone).key.gain(value, FadeMode::Lin),
                        expected,
                        "a multisampler's {zone:?} at {value}"
                    );
                }
            }
        }
    }
}

// Spec: modulation R19, remote-mapping R6. Taking a rack apart takes the
// mappings to the rack and its chain with it, in the same undo step.
#[test]
fn ungrouping_a_rack_takes_the_mappings_to_it_and_its_chain() {
    let mut song = Song::new();
    let rack = song.rack(RackKind::AudioEffect, 1);
    let (rack_id, chain) = (rack.id, rack.chains[0].id);
    song.insert(song.midi, 0, rack).unwrap();
    let fx = song.device(DeviceRole::AudioEffect);
    song.insert_in(chain, 0, fx).unwrap();
    let lfo: DeviceId = song.editor.new_id();
    song.insert(
        song.midi,
        1,
        Device::new(
            lfo,
            "Shaper",
            DeviceKind::Native {
                device_type: "beatlayer.curve_mod".to_owned(),
            },
        ),
    )
    .unwrap();
    song.editor
        .apply(Command::MapModulator {
            device: lfo,
            slot: 0,
            target: bl_model::modulator::ModTarget::Device {
                device: rack_id,
                param: ParamId(1),
            },
        })
        .unwrap();
    let target = RemoteTarget::DeviceOn { device: rack_id };
    let kind = target.natural_kind().unwrap();
    song.editor
        .apply(Command::SetRemoteMappings {
            mappings: vec![
                RemoteMapping::new(
                    RemoteSource::Note {
                        channel: 1,
                        key: 36,
                    },
                    target,
                    kind,
                    None,
                    None,
                )
                .unwrap(),
            ],
        })
        .unwrap();
    let before = song.doc().clone();

    song.editor
        .apply(Command::UngroupDevice { device: rack_id })
        .unwrap();
    assert!(song.doc().device(lfo).unwrap().mappings.is_empty());
    assert!(song.doc().settings().remote.is_empty());

    song.editor.undo().unwrap();
    assert_eq!(song.doc(), &before);
    assert_eq!(song.doc().device(lfo).unwrap().mappings.len(), 1);
    assert_eq!(song.doc().settings().remote.len(), 1);
}
