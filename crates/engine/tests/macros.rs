//! Macros in the engine (WP-5.4; the macros spec, R8 and R11): a macro's
//! change and its envelope reach every mapped parameter on their frame, a
//! mapped parameter's own envelope waits, and a hidden macro still drives.
//! Every block runs under the allocation trap, at blocks of 1, 64 and 512
//! frames.

bl_rt::install_trap!();

mod common;

use bl_core::time::Ticks;
use bl_engine::TransportCommand;
use bl_engine::testing::{GAIN_DESCRIBED, GAIN_PARAM};
use bl_model::{
    AutomationTarget, Command, DeviceId, MacroStyle, MacroTarget, RackKind, TrackId, TrackKind,
    macro_param,
};
use common::{Rig, dc, effect_in, rack, track};

const SIZES: [usize; 3] = [1, 64, 512];
const QUARTER: usize = 24_000;

/// A rig with a DC source and an audio effect rack of two chains, each with a
/// described gain.
fn setup(macro_number: u8) -> (Rig, TrackId, DeviceId, Vec<DeviceId>) {
    let mut rig = Rig::new(4096);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, audio, 1.0);
    let (rack, chains) = rack(&mut rig.editor, audio, RackKind::AudioEffect, 2);
    let gains: Vec<DeviceId> = chains
        .iter()
        .map(|chain| effect_in(&mut rig.editor, *chain, GAIN_DESCRIBED, &[]))
        .collect();
    for gain in &gains {
        rig.apply(Command::MapMacro {
            rack,
            macro_number,
            target: MacroTarget::Device {
                device: *gain,
                param: GAIN_PARAM,
            },
        });
    }
    rig.sync();
    (rig, audio, rack, gains)
}

fn set_macro(rig: &mut Rig, rack: DeviceId, number: u8, value: f64) {
    rig.apply(Command::SetDeviceParam {
        device: rack,
        param: macro_param(number),
        value,
    });
}

/// The level the rig settles at, the same on every frame.
fn settled(rig: &mut Rig, size: usize) -> f32 {
    rig.render(4096, size);
    let out = rig.render(size.max(64), size);
    assert!(out.iter().all(|s| *s == out[0]), "{out:?}");
    out[0]
}

// Spec: macros R8
#[test]
fn a_macro_change_lands_on_its_frame_in_every_mapped_device() {
    for size in SIZES {
        let (mut rig, _, rack, _) = setup(1);
        // Gain's travel is 0 to 2: a macro at a quarter gives each gain 0.5,
        // and two chains each put 0.5 of the DC source out.
        set_macro(&mut rig, rack, 1, 0.25);
        assert!((settled(&mut rig, size) - 1.0).abs() < 1e-6, "block {size}");
        set_macro(&mut rig, rack, 1, 0.5);
        assert!((settled(&mut rig, size) - 2.0).abs() < 1e-6, "block {size}");
    }
}

// Spec: macros R8
#[test]
fn a_macro_envelope_drives_its_parameters_frame_by_frame() {
    for size in SIZES {
        let (mut rig, audio, rack, _) = setup_chain_volume();
        let target = AutomationTarget::Device {
            device: rack,
            param: macro_param(1),
        };
        rig.apply(Command::SetAutomationBreakpoint {
            track: audio,
            target,
            at: Ticks::ZERO,
            value: 0.0,
        });
        rig.apply(Command::SetAutomationBreakpoint {
            track: audio,
            target,
            at: Ticks::QUARTER,
            value: 1.0,
        });
        rig.sender.transport(TransportCommand::Play);
        let out = rig.render(QUARTER, size);
        // The macro rises over the quarter, so each chain's volume does too.
        assert!(out[0].abs() < 1e-6, "{:?} at block {size}", out[0]);
        assert!(
            out[QUARTER / 2] > out[QUARTER / 4],
            "louder by half: {} at block {size}",
            out[QUARTER / 2]
        );
        assert!(
            out[QUARTER - 1] > out[QUARTER / 2],
            "louder by the end: {} at block {size}",
            out[QUARTER - 1]
        );
    }
}

// Spec: macros R22
#[test]
fn a_stepped_macros_envelope_plays_as_steps() {
    for size in SIZES {
        let (mut rig, audio, rack, _) = setup_chain_activator();
        // Three steps: the macro takes 0, a half and 1.
        rig.apply(Command::SetMacroStyle {
            rack,
            macro_number: 1,
            style: MacroStyle::Stepped,
            steps: Some(3),
        });
        let target = AutomationTarget::Device {
            device: rack,
            param: macro_param(1),
        };
        rig.apply(Command::SetAutomationBreakpoint {
            track: audio,
            target,
            at: Ticks::ZERO,
            value: 0.0,
        });
        rig.apply(Command::SetAutomationBreakpoint {
            track: audio,
            target,
            at: Ticks::QUARTER,
            value: 1.0,
        });
        rig.sender.transport(TransportCommand::Play);
        let out = rig.render(QUARTER, size);
        // The macro's steps play: the chain's activator is a switch, and it
        // is muted at the macro's middle step, a quarter of the way up the
        // envelope, not at its half.
        let on = QUARTER / 4;
        assert!(
            out[..on].iter().all(|s| (s - 1.0).abs() < 1e-6),
            "sounding until the macro's middle step: {:?}",
            &out[on - 1..on + 1]
        );
        assert!(
            out[on..].iter().all(|s| s.abs() < 1e-6),
            "muted from the macro's middle step: {:?}",
            &out[on - 1..on + 1]
        );
    }
}

// A rig whose rack's chain activator is mapped to macro 1.
fn setup_chain_activator() -> (Rig, TrackId, DeviceId, bl_model::ChainId) {
    let mut rig = Rig::new(4096);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, audio, 1.0);
    let (rack, chains) = rack(&mut rig.editor, audio, RackKind::AudioEffect, 1);
    let chain = chains[0];
    rig.apply(Command::MapMacro {
        rack,
        macro_number: 1,
        target: MacroTarget::Chain {
            chain,
            param: bl_model::MixerParam::Mute,
        },
    });
    rig.sync();
    (rig, audio, rack, chain)
}

// A rig whose rack's two chain volumes are both mapped to macro 1.
fn setup_chain_volume() -> (Rig, TrackId, DeviceId, Vec<DeviceId>) {
    let mut rig = Rig::new(4096);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, audio, 1.0);
    let (rack, chains) = rack(&mut rig.editor, audio, RackKind::AudioEffect, 2);
    let mut gains = Vec::new();
    for chain in &chains {
        gains.push(effect_in(&mut rig.editor, *chain, GAIN_DESCRIBED, &[]));
        rig.apply(Command::MapMacro {
            rack,
            macro_number: 1,
            target: MacroTarget::Chain {
                chain: *chain,
                param: bl_model::MixerParam::Volume,
            },
        });
    }
    rig.sync();
    (rig, audio, rack, gains)
}

// Spec: macros R11
#[test]
fn a_mapped_parameters_own_envelope_waits_while_its_macro_drives_it() {
    let (mut rig, audio, rack, _) = setup_chain_volume();
    // Each chain volume's own envelope holds it silent; the macro's holds it
    // at unity. The macro wins while it drives the volume.
    let chain = rig.editor.document().device(rack).unwrap().chains[0].id;
    rig.apply(Command::SetAutomationBreakpoint {
        track: audio,
        target: AutomationTarget::Chain {
            chain,
            param: bl_model::MixerParam::Volume,
        },
        at: Ticks::ZERO,
        value: 0.0,
    });
    rig.apply(Command::SetAutomationBreakpoint {
        track: audio,
        target: AutomationTarget::Device {
            device: rack,
            param: macro_param(1),
        },
        at: Ticks::ZERO,
        value: 1.0,
    });
    rig.apply(Command::SetAutomationBreakpoint {
        track: audio,
        target: AutomationTarget::Device {
            device: rack,
            param: macro_param(1),
        },
        at: Ticks::QUARTER,
        value: 1.0,
    });
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(256, 64);
    assert!(
        out.iter().all(|s| *s > 1.9),
        "the macro drives the volume, not its own envelope: {out:?}"
    );
}

// Spec: macros R2
#[test]
fn a_hidden_macro_still_drives_its_parameters() {
    let (mut rig, _, rack, _) = setup(9);
    set_macro(&mut rig, rack, 9, 0.25);
    assert!((settled(&mut rig, 64) - 1.0).abs() < 1e-6);
}

// Spec: macros R27
#[test]
fn launching_a_variation_moves_every_macro_on_one_frame() {
    for size in SIZES {
        let mut rig = Rig::new(4096);
        let audio = track(&mut rig.editor, TrackKind::Audio, None);
        dc(&mut rig.editor, audio, 1.0);
        let (rack, chains) = rack(&mut rig.editor, audio, RackKind::AudioEffect, 2);
        let gains: Vec<DeviceId> = chains
            .iter()
            .map(|chain| effect_in(&mut rig.editor, *chain, GAIN_DESCRIBED, &[]))
            .collect();
        // Macro 1 drives the first chain's gain, macro 2 the second's.
        for (number, gain) in [(1u8, gains[0]), (2, gains[1])] {
            rig.apply(Command::MapMacro {
                rack,
                macro_number: number,
                target: MacroTarget::Device {
                    device: gain,
                    param: GAIN_PARAM,
                },
            });
        }
        // Macro 2 is left out of variations.
        rig.apply(Command::SetMacroVariationExcluded {
            rack,
            macro_number: 2,
            excluded: true,
        });
        // A variation of both macros at 0, then both macros raised.
        rig.apply(Command::StoreVariation { rack });
        rig.apply(Command::SetDeviceParam {
            device: rack,
            param: macro_param(1),
            value: 1.0,
        });
        rig.apply(Command::SetDeviceParam {
            device: rack,
            param: macro_param(2),
            value: 1.0,
        });
        rig.sync();
        assert!(
            (settled(&mut rig, size) - 4.0).abs() < 1e-6,
            "both gains at 2, block {size}"
        );
        // The launch sets macro 1 back to 0 and leaves excluded macro 2 alone.
        rig.apply(Command::LaunchVariation { rack, index: 0 });
        rig.sync();
        assert!(
            (settled(&mut rig, size) - 2.0).abs() < 1e-6,
            "one gain left, block {size}"
        );
    }
}

// Spec: macros R4
#[test]
fn a_macro_drives_a_chains_mixer_control() {
    let mut rig = Rig::new(4096);
    let audio = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, audio, 1.0);
    let (rack, chains) = rack(&mut rig.editor, audio, RackKind::AudioEffect, 1);
    effect_in(&mut rig.editor, chains[0], GAIN_DESCRIBED, &[]);
    rig.apply(Command::MapMacro {
        rack,
        macro_number: 1,
        target: MacroTarget::Chain {
            chain: chains[0],
            param: bl_model::MixerParam::Volume,
        },
    });
    rig.sync();
    set_macro(&mut rig, rack, 1, 0.0);
    assert!(settled(&mut rig, 64).abs() < 1e-6, "the chain is silent");
}
