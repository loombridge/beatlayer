//! Macros in the document (WP-5.4; the macros spec, R1 to R19): mappings and
//! their ranges, the law, a device leaving its rack, and a copied rack's
//! mappings.

use bl_model::{
    Chain, ChainId, Command, Device, DeviceId, DeviceKind, DeviceRole, EditError, Editor,
    MacroMapping, MacroTarget, MixerParam, ParamDescriptor, ParamId, ParamUnit, RackKind, TrackId,
    TrackKind, macro_param,
};

struct Song {
    editor: Editor,
    audio: TrackId,
}

impl Song {
    fn new() -> Self {
        let mut editor = Editor::new();
        let audio: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: audio,
                kind: TrackKind::Audio,
                name: "Audio".to_owned(),
                parent: None,
                index: 0,
            })
            .unwrap();
        Self { editor, audio }
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

    fn insert(&mut self, device: Device) {
        let index = self
            .editor
            .document()
            .track(self.audio)
            .unwrap()
            .devices()
            .count();
        self.editor
            .apply(Command::InsertDevice {
                track: self.audio,
                index,
                device,
            })
            .unwrap();
    }

    fn insert_in(&mut self, chain: bl_model::ChainId, index: usize, device: Device) {
        self.editor
            .apply(Command::InsertChainDevice {
                chain,
                index,
                device,
            })
            .unwrap();
    }
}

fn rejected(editor: &mut Editor, command: Command) -> bl_model::ModelError {
    match editor.apply(command) {
        Err(EditError::Command(error)) => error,
        other => panic!("expected the document to refuse it, got {other:?}"),
    }
}

// Spec: macros R1
#[test]
fn a_rack_has_sixteen_macros_as_parameters() {
    let description = bl_model::rack_description();
    for number in 1..=16 {
        let param = macro_param(number);
        assert_eq!(bl_model::macro_number(param), Some(number));
        let descriptor = description.param(param).expect("a macro is described");
        assert_eq!(descriptor.name, format!("Macro {number}"));
        assert_eq!((descriptor.min(), descriptor.max()), (0.0, 1.0));
    }
    assert_eq!(bl_model::macro_number(ParamId(0)), None);
    assert_eq!(bl_model::macro_number(ParamId(999)), None);
}

// Spec: macros R2
#[test]
fn a_new_rack_shows_eight_macros_and_the_count_runs_from_one_to_sixteen() {
    let views = bl_model::RackViews::default();
    assert_eq!(views.macro_count, 8);
    for count in 1..=16 {
        let views = bl_model::RackViews {
            macro_count: count,
            ..views
        };
        assert_eq!(views.macro_count, count);
    }
}

// Spec: macros R4
#[test]
fn a_macro_maps_only_parameters_inside_its_rack() {
    let mut song = Song::new();
    let rack = song.rack(RackKind::AudioEffect, 1);
    let rack_id = rack.id;
    let chain = rack.chains[0].id;
    song.insert(rack);
    let inside = song.device(DeviceRole::AudioEffect);
    let inside_id = inside.id;
    song.insert_in(chain, 0, inside);
    let outside = song.device(DeviceRole::AudioEffect);
    let outside_id = outside.id;
    song.insert(outside);

    song.editor
        .apply(Command::MapMacro {
            rack: rack_id,
            macro_number: 1,
            target: MacroTarget::Device {
                device: inside_id,
                param: ParamId(0),
            },
        })
        .unwrap();
    // The rack's own chain selector maps too.
    song.editor
        .apply(Command::MapMacro {
            rack: rack_id,
            macro_number: 2,
            target: MacroTarget::Device {
                device: rack_id,
                param: bl_model::CHAIN_SELECTOR,
            },
        })
        .unwrap();
    // A chain's mixer control maps too.
    song.editor
        .apply(Command::MapMacro {
            rack: rack_id,
            macro_number: 3,
            target: MacroTarget::Chain {
                chain,
                param: MixerParam::Volume,
            },
        })
        .unwrap();
    // A device outside the rack doesn't.
    rejected(
        &mut song.editor,
        Command::MapMacro {
            rack: rack_id,
            macro_number: 1,
            target: MacroTarget::Device {
                device: outside_id,
                param: ParamId(0),
            },
        },
    );
    // Nor does the rack's own macro.
    rejected(
        &mut song.editor,
        Command::MapMacro {
            rack: rack_id,
            macro_number: 1,
            target: MacroTarget::Device {
                device: rack_id,
                param: macro_param(4),
            },
        },
    );
}

// Spec: macros R5
#[test]
fn mapping_a_parameter_to_a_second_macro_moves_it() {
    let mut song = Song::new();
    let rack = song.rack(RackKind::AudioEffect, 1);
    let rack_id = rack.id;
    let chain = rack.chains[0].id;
    song.insert(rack);
    let device = song.device(DeviceRole::AudioEffect);
    let device_id = device.id;
    song.insert_in(chain, 0, device);
    let target = MacroTarget::Device {
        device: device_id,
        param: ParamId(3),
    };
    for number in [1, 2] {
        song.editor
            .apply(Command::MapMacro {
                rack: rack_id,
                macro_number: number,
                target,
            })
            .unwrap();
    }
    let rack = song.editor.document().device(rack_id).unwrap();
    assert_eq!(rack.macros.mappings.len(), 1);
    assert_eq!(rack.macros.mappings[0].macro_number, 2);
}

// Spec: macros R7
#[test]
fn a_mapping_is_a_straight_line_in_the_parameters_travel() {
    let mapping = MacroMapping {
        macro_number: 1,
        target: MacroTarget::Chain {
            chain: ChainId(bl_core::id::Id::new(1).unwrap()),
            param: MixerParam::Volume,
        },
        min: 0.25,
        max: 0.75,
    };
    assert_eq!(mapping.position(0.0), 0.25);
    assert_eq!(mapping.position(1.0), 0.75);
    assert_eq!(mapping.position(0.5), 0.5);
}

// Spec: macros R7
#[test]
fn min_above_max_inverts_a_mapping_on_the_parameters_travel() {
    let mapping = MacroMapping {
        macro_number: 1,
        target: MacroTarget::Chain {
            chain: ChainId(bl_core::id::Id::new(1).unwrap()),
            param: MixerParam::Pan,
        },
        min: 0.9,
        max: 0.1,
    };
    let pan = ParamDescriptor::continuous(ParamId(0), "Pan", (-1.0, 1.0), 0.0, ParamUnit::None);
    let value = |position| pan.denormalize(mapping.position(position));
    assert!(
        value(0.0) > value(1.0),
        "the parameter falls as the macro rises"
    );
}

// Spec: macros R18
#[test]
fn a_device_leaving_its_rack_takes_its_mappings() {
    let mut song = Song::new();
    let rack = song.rack(RackKind::AudioEffect, 1);
    let rack_id = rack.id;
    let chain = rack.chains[0].id;
    song.insert(rack);
    let device = song.device(DeviceRole::AudioEffect);
    let device_id = device.id;
    song.insert_in(chain, 0, device);
    song.editor
        .apply(Command::MapMacro {
            rack: rack_id,
            macro_number: 1,
            target: MacroTarget::Device {
                device: device_id,
                param: ParamId(0),
            },
        })
        .unwrap();
    song.editor
        .apply(Command::RemoveDevice { device: device_id })
        .unwrap();
    let rack = song.editor.document().device(rack_id).unwrap();
    assert!(rack.macros.mappings.is_empty());
}

// Spec: macros R18
#[test]
fn a_copied_rack_maps_its_own_devices() {
    let mut song = Song::new();
    let rack = song.rack(RackKind::AudioEffect, 1);
    let rack_id = rack.id;
    let chain = rack.chains[0].id;
    song.insert(rack);
    let device = song.device(DeviceRole::AudioEffect);
    let device_id = device.id;
    song.insert_in(chain, 0, device);
    let target = MacroTarget::Device {
        device: device_id,
        param: ParamId(0),
    };
    song.editor
        .apply(Command::MapMacro {
            rack: rack_id,
            macro_number: 1,
            target,
        })
        .unwrap();

    let mut copy = song.editor.document().device(rack_id).unwrap().clone();
    copy.reassign_ids(&mut || song.editor.new_id::<DeviceId>().0);
    let copied = copy.macros.mappings[0].target;
    let MacroTarget::Device { device, .. } = copied else {
        panic!("a device mapping");
    };
    assert_ne!(device, device_id, "the copy maps its own device");
    assert!(
        copy.find(device).is_some(),
        "the copy holds the device it maps"
    );
}

// Spec: macros R17
#[test]
fn a_broken_mapping_doesnt_load() {
    let mut song = Song::new();
    let rack = song.rack(RackKind::AudioEffect, 0);
    let outside = song.device(DeviceRole::AudioEffect);
    let outside_id = outside.id;
    song.insert(outside);
    let mut broken = rack.clone();
    broken.macros.mappings.push(MacroMapping {
        macro_number: 1,
        target: MacroTarget::Device {
            device: outside_id,
            param: ParamId(0),
        },
        min: 0.0,
        max: 1.0,
    });
    let index = song
        .editor
        .document()
        .track(song.audio)
        .unwrap()
        .devices()
        .count();
    song.editor
        .apply(Command::InsertDevice {
            track: song.audio,
            index,
            device: broken,
        })
        .unwrap_err();
}

// Spec: macros R17
#[test]
fn macros_and_mappings_save_with_the_set_and_load_back() {
    let mut song = Song::new();
    let rack = song.rack(RackKind::AudioEffect, 1);
    let rack_id = rack.id;
    let chain = rack.chains[0].id;
    song.insert(rack);
    let device = song.device(DeviceRole::AudioEffect);
    let device_id = device.id;
    song.insert_in(chain, 0, device);
    let target = MacroTarget::Device {
        device: device_id,
        param: ParamId(2),
    };
    song.editor
        .apply(Command::MapMacro {
            rack: rack_id,
            macro_number: 3,
            target,
        })
        .unwrap();
    song.editor
        .apply(Command::SetMappingRange {
            rack: rack_id,
            target,
            min: 0.75,
            max: 0.25,
        })
        .unwrap();
    let saved = song.editor.document().device(rack_id).unwrap().clone();
    let json = serde_json::to_value(&saved).unwrap();
    let back: Device = serde_json::from_value(json).unwrap();
    assert_eq!(back.macros, saved.macros);
    assert_eq!(back.macros.mappings.len(), 1);
    assert_eq!(back.macros.mappings[0].macro_number, 3);
    assert_eq!(
        (back.macros.mappings[0].min, back.macros.mappings[0].max),
        (0.75, 0.25)
    );
}

// Spec: macros R17
#[test]
fn a_set_saved_before_macros_loads_with_none() {
    let mut song = Song::new();
    let rack = song.rack(RackKind::AudioEffect, 1);
    let mut json = serde_json::to_value(&rack).unwrap();
    json.as_object_mut().unwrap().remove("macros");
    let back: Device = serde_json::from_value(json).unwrap();
    assert_eq!(back.macros, bl_model::RackMacros::default());
    assert!(back.macros.mappings.is_empty());
}
