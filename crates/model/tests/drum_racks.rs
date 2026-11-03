//! Drum racks in the document (WP-5.6; the drum racks spec, R1, R6, R7, R8
//! and R27): a drum chain's Receive, Play, Choke and sends, up to six return
//! chains, a return chain's Audio To, and what a set saves.

use bl_core::time::Ticks;
use bl_model::{
    AutomationTarget, Chain, ChainId, Command, Device, DeviceId, DeviceKind, DeviceRole, Editor,
    ModelError, RackKind, Receive, TrackId, TrackKind,
};

fn rejected(editor: &mut Editor, command: Command) -> ModelError {
    match editor.apply(command) {
        Err(bl_model::EditError::Command(error)) => error,
        other => panic!("expected the document to refuse it, got {other:?}"),
    }
}

struct Drums {
    editor: Editor,
    track: TrackId,
    rack: DeviceId,
    drums: Vec<ChainId>,
    returns: Vec<ChainId>,
}

/// A MIDI track holding a drum rack of `drums` pads from note 36 and
/// `returns` return chains.
fn kit(drums: usize, returns: usize) -> Drums {
    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Drums".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let mut rack = Device::rack(editor.new_id(), RackKind::Drum);
    for n in 0..drums {
        rack = rack.with_chain(Chain::for_pad(
            editor.new_id(),
            format!("Pad {n}"),
            36 + n as u8,
        ));
    }
    for n in 0..returns {
        rack = rack.with_chain(Chain::new_return(editor.new_id(), format!("Return {n}")));
    }
    let drums = rack
        .chains
        .iter()
        .filter(|c| !c.is_return)
        .map(|c| c.id)
        .collect();
    let returns = rack
        .chains
        .iter()
        .filter(|c| c.is_return)
        .map(|c| c.id)
        .collect();
    let id = rack.id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: rack,
        })
        .unwrap();
    Drums {
        editor,
        track,
        rack: id,
        drums,
        returns,
    }
}

fn effect(editor: &mut Editor) -> Device {
    Device::new(
        editor.new_id(),
        "Effect",
        DeviceKind::Native {
            device_type: "test.effect".to_owned(),
        },
    )
    .with_role(DeviceRole::AudioEffect)
}

fn chain(kit: &Drums, id: ChainId) -> Chain {
    kit.editor.document().chain(id).unwrap().clone()
}

// Spec: drum-racks R1
#[test]
fn drum_chains_have_receive_play_and_choke_and_no_zones() {
    let mut kit = kit(2, 0);
    let pad = kit.drums[0];
    let new = chain(&kit, pad);
    assert_eq!(
        (new.receive, new.play, new.choke),
        (Receive::Note(36), 60, None),
        "a pad's chain receives its note and plays C3"
    );
    for command in [
        Command::SetChainReceive {
            chain: pad,
            receive: Receive::Note(41),
        },
        Command::SetChainPlay {
            chain: pad,
            play: 64,
        },
        Command::SetChainChoke {
            chain: pad,
            choke: Some(2),
        },
    ] {
        kit.editor.apply(command).unwrap();
    }
    let set = chain(&kit, pad);
    assert_eq!(
        (set.receive, set.play, set.choke),
        (Receive::Note(41), 64, Some(2))
    );
    kit.editor
        .apply(Command::SetChainReceive {
            chain: pad,
            receive: Receive::AllNotes,
        })
        .unwrap();
    assert_eq!(chain(&kit, pad).receive, Receive::AllNotes);
    // Out of range values are refused, and leave the chain as it was.
    for command in [
        Command::SetChainReceive {
            chain: pad,
            receive: Receive::Note(128),
        },
        Command::SetChainPlay {
            chain: pad,
            play: 200,
        },
        Command::SetChainChoke {
            chain: pad,
            choke: Some(17),
        },
        Command::SetChainChoke {
            chain: pad,
            choke: Some(0),
        },
    ] {
        rejected(&mut kit.editor, command);
    }
    assert_eq!(chain(&kit, pad).play, 64);
    // Each is one undo step; an undone one comes back as it was.
    let steps = kit.editor.undo_history().len();
    kit.editor
        .apply(Command::SetChainPlay {
            chain: pad,
            play: 70,
        })
        .unwrap();
    assert_eq!(kit.editor.undo_history().len(), steps + 1);
    kit.editor.undo().unwrap();
    assert_eq!(chain(&kit, pad).play, 64);
    // Only a drum rack's chains have them: another rack's chain is refused.
    let other: DeviceId = kit.editor.new_id();
    let inner: ChainId = kit.editor.new_id();
    let second: TrackId = kit.editor.new_id();
    kit.editor
        .apply(Command::CreateTrack {
            id: second,
            kind: TrackKind::Midi,
            name: "Keys".to_owned(),
            parent: None,
            index: 1,
        })
        .unwrap();
    kit.editor
        .apply(Command::InsertDevice {
            track: second,
            index: 0,
            device: Device::rack(other, RackKind::Instrument)
                .with_chain(Chain::new(inner, "Chain")),
        })
        .unwrap();
    assert_eq!(
        rejected(
            &mut kit.editor,
            Command::SetChainPlay {
                chain: inner,
                play: 61
            }
        ),
        ModelError::NotADrumChain(inner)
    );
    // A chain moved into another kind of rack loses them.
    kit.editor
        .apply(Command::MoveChain {
            chain: pad,
            rack: other,
            index: 0,
        })
        .unwrap();
    let moved = chain(&kit, pad);
    assert_eq!(
        (moved.receive, moved.play, moved.choke),
        (Receive::Note(36), 60, None)
    );
}

// Spec: drum-racks R7
#[test]
fn six_return_chains_and_no_seventh() {
    let mut kit = kit(1, 6);
    let seventh: ChainId = kit.editor.new_id();
    let error = rejected(
        &mut kit.editor,
        Command::AddChain {
            rack: kit.rack,
            index: 7,
            chain: Chain::new_return(seventh, "Seventh"),
        },
    );
    assert_eq!(error, ModelError::TooManyReturnChains(kit.rack));
    // A return chain holds audio effects, and no instrument.
    kit.editor
        .apply(Command::RemoveChain {
            chain: kit.returns[5],
        })
        .unwrap();
    let synth = Device::new(
        kit.editor.new_id(),
        "Synth",
        DeviceKind::Native {
            device_type: "test.synth".to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    let error = rejected(
        &mut kit.editor,
        Command::InsertChainDevice {
            chain: kit.returns[0],
            index: 0,
            device: synth,
        },
    );
    assert!(
        matches!(error, ModelError::ChainDoesNotFit { .. }),
        "{error:?}"
    );
    let fx = effect(&mut kit.editor);
    kit.editor
        .apply(Command::InsertChainDevice {
            chain: kit.returns[0],
            index: 0,
            device: fx,
        })
        .unwrap();
    // Return chains come after the drum chains.
    let early: ChainId = kit.editor.new_id();
    let error = rejected(
        &mut kit.editor,
        Command::AddChain {
            rack: kit.rack,
            index: 0,
            chain: Chain::new_return(early, "Early"),
        },
    );
    assert_eq!(error, ModelError::ReturnChainOrder(kit.rack));
}

// Spec: drum-racks R6
#[test]
fn sends_start_silent_and_go_with_their_return_chain() {
    let mut kit = kit(2, 2);
    let (a, b) = (kit.drums[0], kit.drums[1]);
    assert_eq!(chain(&kit, a).send(kit.returns[0]), 0.0, "−∞ dB to start");
    kit.editor
        .apply(Command::SetChainSend {
            chain: a,
            to: kit.returns[0],
            level: 0.5,
        })
        .unwrap();
    kit.editor
        .apply(Command::SetChainSend {
            chain: b,
            to: kit.returns[0],
            level: 1.0,
        })
        .unwrap();
    assert_eq!(chain(&kit, a).send(kit.returns[0]), 0.5);
    // A send goes to a return chain of the rack, and only from a drum chain.
    assert_eq!(
        rejected(
            &mut kit.editor,
            Command::SetChainSend {
                chain: a,
                to: b,
                level: 0.1
            }
        ),
        ModelError::NoSuchChain(b)
    );
    assert!(matches!(
        rejected(
            &mut kit.editor,
            Command::SetChainSend {
                chain: kit.returns[0],
                to: kit.returns[1],
                level: 0.1
            }
        ),
        ModelError::NotADrumChain(_)
    ));
    // Deleting the return chain takes the sends to it, and undo brings them
    // back.
    kit.editor
        .apply(Command::RemoveChain {
            chain: kit.returns[0],
        })
        .unwrap();
    assert!(chain(&kit, a).sends.is_empty() && chain(&kit, b).sends.is_empty());
    kit.editor.undo().unwrap();
    assert_eq!(chain(&kit, a).send(kit.returns[0]), 0.5);
}

// Spec: drum-racks R6
#[test]
fn a_copied_kit_keeps_its_sends_to_its_own_return_chains() {
    let mut kit = kit(1, 1);
    let (pad, ret) = (kit.drums[0], kit.returns[0]);
    kit.editor
        .apply(Command::SetChainSend {
            chain: pad,
            to: ret,
            level: 0.5,
        })
        .unwrap();
    let mut copy = kit.editor.document().device(kit.rack).unwrap().clone();
    let editor = &mut kit.editor;
    copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
    let (new_pad, new_ret) = (&copy.chains[0], copy.chains[1].id);
    assert_ne!(new_ret, ret);
    assert_eq!(new_pad.send(new_ret), 0.5);
    let other: TrackId = kit.editor.new_id();
    kit.editor
        .apply(Command::CreateTrack {
            id: other,
            kind: TrackKind::Midi,
            name: "Copy".to_owned(),
            parent: None,
            index: 1,
        })
        .unwrap();
    kit.editor
        .apply(Command::InsertDevice {
            track: other,
            index: 0,
            device: copy,
        })
        .unwrap();
}

// Spec: drum-racks R6, G2
#[test]
fn sends_are_automation_targets() {
    let mut kit = kit(1, 1);
    let target = AutomationTarget::ChainSend {
        chain: kit.drums[0],
        to: kit.returns[0],
    };
    kit.editor
        .apply(Command::SetAutomationBreakpoint {
            track: kit.track,
            target,
            at: Ticks::ZERO,
            value: 0.25,
        })
        .unwrap();
    let track = kit.editor.document().track(kit.track).unwrap();
    assert!(track.automation().any(|(t, _)| *t == target));
    // A hand move turns the envelope off, as a track's send does.
    kit.editor
        .apply(Command::SetChainSend {
            chain: kit.drums[0],
            to: kit.returns[0],
            level: 0.75,
        })
        .unwrap();
    let track = kit.editor.document().track(kit.track).unwrap();
    assert!(
        track
            .automation()
            .find(|(t, _)| **t == target)
            .is_some_and(|(_, a)| !a.enabled)
    );
    // The target goes with its chains.
    kit.editor
        .apply(Command::RemoveChain {
            chain: kit.returns[0],
        })
        .unwrap();
    let track = kit.editor.document().track(kit.track).unwrap();
    assert!(track.automation().all(|(t, _)| *t != target));
    // A drum chain's own send needs a return chain to name.
    let pad = kit.drums[0];
    let error = kit
        .editor
        .apply(Command::SetAutomationBreakpoint {
            track: kit.track,
            target: AutomationTarget::ChainSend {
                chain: pad,
                to: pad,
            },
            at: Ticks::ZERO,
            value: 0.5,
        })
        .unwrap_err();
    assert!(matches!(
        error,
        bl_model::EditError::Command(ModelError::NotADrumChain(_))
    ));
}

// Spec: drum-racks R6, G2
#[test]
fn a_send_envelope_goes_when_its_chain_leaves_the_rack_on_its_track() {
    let mut kit = kit(1, 1);
    let target = AutomationTarget::ChainSend {
        chain: kit.drums[0],
        to: kit.returns[0],
    };
    kit.editor
        .apply(Command::SetAutomationBreakpoint {
            track: kit.track,
            target,
            at: Ticks::ZERO,
            value: 0.25,
        })
        .unwrap();
    let effects = Device::rack(kit.editor.new_id(), RackKind::AudioEffect);
    let effects_id = effects.id;
    kit.editor
        .apply(Command::InsertDevice {
            track: kit.track,
            index: 1,
            device: effects,
        })
        .unwrap();
    kit.editor
        .apply(Command::MoveChain {
            chain: kit.drums[0],
            rack: effects_id,
            index: 0,
        })
        .unwrap();
    let track = kit.editor.document().track(kit.track).unwrap();
    assert!(track.automation().all(|(t, _)| *t != target));
    kit.editor.undo().unwrap();
    let track = kit.editor.document().track(kit.track).unwrap();
    assert!(track.automation().any(|(t, _)| *t == target));
}

// Spec: drum-racks R8
#[test]
fn a_return_chain_sends_to_a_return_track_or_the_rack() {
    let mut kit = kit(1, 1);
    let ret: TrackId = kit.editor.new_id();
    kit.editor
        .apply(Command::CreateTrack {
            id: ret,
            kind: TrackKind::Return,
            name: "A Reverb".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let chain_id = kit.returns[0];
    assert_eq!(
        chain(&kit, chain_id).audio_to,
        None,
        "the rack's output to start"
    );
    kit.editor
        .apply(Command::SetChainAudioTo {
            chain: chain_id,
            track: Some(ret),
        })
        .unwrap();
    assert_eq!(chain(&kit, chain_id).audio_to, Some(ret));
    // Only a return track is offered, and only a return chain sends.
    let audio: TrackId = kit.editor.new_id();
    kit.editor
        .apply(Command::CreateTrack {
            id: audio,
            kind: TrackKind::Audio,
            name: "Audio".to_owned(),
            parent: None,
            index: 1,
        })
        .unwrap();
    rejected(
        &mut kit.editor,
        Command::SetChainAudioTo {
            chain: chain_id,
            track: Some(audio),
        },
    );
    assert!(matches!(
        rejected(
            &mut kit.editor,
            Command::SetChainAudioTo {
                chain: kit.drums[0],
                track: Some(ret)
            }
        ),
        ModelError::NotADrumChain(_)
    ));
    // A return chain added with an output that isn't a return track is
    // refused too.
    let mut stray = Chain::new_return(kit.editor.new_id(), "Stray");
    stray.audio_to = Some(audio);
    rejected(
        &mut kit.editor,
        Command::AddChain {
            rack: kit.rack,
            index: 2,
            chain: stray,
        },
    );
    // Deleting the return track sends the chain home; undo restores both.
    kit.editor
        .apply(Command::DeleteTrack { track: ret })
        .unwrap();
    assert_eq!(chain(&kit, chain_id).audio_to, None);
    kit.editor.undo().unwrap();
    assert_eq!(chain(&kit, chain_id).audio_to, Some(ret));
}

// Spec: drum-racks R27
#[test]
fn a_set_with_a_kit_saves_and_loads_what_it_holds() {
    let mut kit = kit(3, 2);
    for command in [
        Command::SetChainReceive {
            chain: kit.drums[1],
            receive: Receive::AllNotes,
        },
        Command::SetChainPlay {
            chain: kit.drums[0],
            play: 62,
        },
        Command::SetChainChoke {
            chain: kit.drums[2],
            choke: Some(4),
        },
        Command::SetChainSend {
            chain: kit.drums[0],
            to: kit.returns[1],
            level: 0.3,
        },
        // Spec: push-2-notes R19.
        Command::SetChainPushColour {
            chain: kit.drums[1],
            colour: Some(21),
        },
    ] {
        kit.editor.apply(command).unwrap();
    }
    let json = serde_json::to_value(kit.editor.document()).unwrap();
    let loaded: bl_model::Document = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(&loaded, kit.editor.document());
    assert_eq!(chain(&kit, kit.drums[1]).push_colour, Some(21));
    // A drum chain with its defaults saves none of it, so sets saved before
    // drum racks load unchanged.
    let rack = json.to_string();
    let defaults =
        serde_json::to_value(Chain::new(ChainId(bl_core::id::Id::new(9).unwrap()), "x")).unwrap();
    for field in [
        "receive",
        "play",
        "choke",
        "sends",
        "is_return",
        "audio_to",
        "push_colour",
    ] {
        assert!(defaults.get(field).is_none(), "{field}");
    }
    assert!(rack.contains("all_notes") && rack.contains("is_return"));
}

// Spec: push-2-notes R19. The colour Push 2 shows for a pad belongs to a drum
// rack's drum chain: any swatch of the palette, or none, one undo step, and
// refused for a return chain, a chain of another rack and a swatch past the
// palette; a chain that leaves the drum rack loses it.
#[test]
fn a_pads_push_colour_belongs_to_a_drum_chain() {
    let mut kit = kit(2, 1);
    let steps = kit.editor.undo_history().count();
    kit.editor
        .apply(Command::SetChainPushColour {
            chain: kit.drums[0],
            colour: Some(69),
        })
        .unwrap();
    assert_eq!(kit.editor.undo_history().count(), steps + 1);
    assert_eq!(chain(&kit, kit.drums[0]).push_colour, Some(69));
    kit.editor.undo().unwrap();
    assert_eq!(chain(&kit, kit.drums[0]).push_colour, None);

    let error = rejected(
        &mut kit.editor,
        Command::SetChainPushColour {
            chain: kit.drums[0],
            colour: Some(70),
        },
    );
    assert!(error.to_string().contains("pad colour"), "{error}");
    let error = rejected(
        &mut kit.editor,
        Command::SetChainPushColour {
            chain: kit.returns[0],
            colour: Some(3),
        },
    );
    assert!(matches!(error, ModelError::NotADrumChain(_)), "{error}");

    // A chain of an effect rack has none to give.
    let inner: DeviceId = kit.editor.new_id();
    let chain_id: ChainId = kit.editor.new_id();
    kit.editor
        .apply(Command::InsertDevice {
            track: kit.track,
            index: 1,
            device: Device::rack(inner, RackKind::AudioEffect)
                .with_chain(Chain::new(chain_id, "Layer")),
        })
        .unwrap();
    let error = rejected(
        &mut kit.editor,
        Command::SetChainPushColour {
            chain: chain_id,
            colour: Some(3),
        },
    );
    assert!(matches!(error, ModelError::NotADrumChain(_)), "{error}");

    // Moving a coloured pad's chain into it drops the colour.
    kit.editor
        .apply(Command::SetChainPushColour {
            chain: kit.drums[1],
            colour: Some(5),
        })
        .unwrap();
    kit.editor
        .apply(Command::MoveChain {
            chain: kit.drums[1],
            rack: inner,
            index: 0,
        })
        .unwrap();
    assert_eq!(chain(&kit, kit.drums[1]).push_colour, None);
}
