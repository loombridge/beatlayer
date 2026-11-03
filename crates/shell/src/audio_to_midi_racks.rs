//! The instruments the tracks of Convert Harmony, Melody and Drums to New
//! MIDI Track hold (IMP-06; the audio-to-midi spec, R9): racks of Beatlayer's
//! own devices, set by the parameter tables below. Nothing here is Live's
//! content.

use bl_devices::{ds_kick, ds_snare, electric, polysynth};
use bl_model::fader::fader_position;
use bl_model::{
    Chain, ChainId, Device, DeviceId, DeviceKind, DeviceRole, Editor, MacroMapping, MacroTarget,
    MixerParam, ParamId, RackKind, macro_param,
};
use bl_shell_spec::audio_to_midi::ConvertKind;

/// The note a drum chain plays (drum racks R3): C3, so a device that tunes
/// by the key it is played at sounds at its own pitch.
const PLAY_NOTE: u8 = 60;

/// The pads of the Drums rack (R9): General MIDI's kick, snare and closed
/// hi-hat.
pub const KICK: u8 = 36;
pub const SNARE: u8 = 38;
pub const HI_HAT: u8 = 42;

/// Drift as a keys sound: a triangle with a sine an octave up, a quick
/// attack and a long decay to a low sustain, softer for softer keys.
const KEYS: &[(ParamId, f64)] = &[
    (polysynth::OSC1_WAVE, 1.0),
    (polysynth::OSC2_WAVE, 0.0),
    (polysynth::OSC2_OCT, 1.0),
    (polysynth::OSC2_GAIN, -14.0),
    (polysynth::FREQ, 3_500.0),
    (polysynth::ENV1_ATTACK, 2.0),
    (polysynth::ENV1_DECAY, 1_800.0),
    (polysynth::ENV1_SUSTAIN, 20.0),
    (polysynth::ENV1_RELEASE, 300.0),
    (polysynth::VEL_VOL, 50.0),
];

/// Drift as a synth: two saws an octave apart through a low-pass filter
/// with a little resonance.
const SYNTH: &[(ParamId, f64)] = &[
    (polysynth::OSC1_WAVE, 4.0),
    (polysynth::OSC2_WAVE, 3.0),
    (polysynth::OSC2_OCT, -1.0),
    (polysynth::OSC2_GAIN, -9.0),
    (polysynth::FREQ, 2_500.0),
    (polysynth::RES, 15.0),
    (polysynth::ENV1_ATTACK, 5.0),
    (polysynth::ENV1_DECAY, 400.0),
    (polysynth::ENV1_SUSTAIN, 70.0),
    (polysynth::ENV1_RELEASE, 150.0),
];

/// DS Kick as a kick: DS Kick's Punch preset.
const KICK_SOUND: &[(ParamId, f64)] = ds_kick::PRESETS[1].params;

/// DS Snare as a snare: DS Snare's Tight preset.
const SNARE_SOUND: &[(ParamId, f64)] = ds_snare::PRESETS[0].params;

/// DS Snare as a closed hi-hat: only its noise, high-passed from its top
/// cutoff, tuned up and gone in about 30 ms.
const HAT_SOUND: &[(ParamId, f64)] = &[
    (ds_snare::TUNE, 100.0),
    (ds_snare::COLOR, 100.0),
    (ds_snare::TONE, 100.0),
    (ds_snare::FILTER, 1.0),
    (bl_devices::drum_synth::DECAY, 18.0),
];

/// A native device of type `device_type`, an instrument, with `params` set
/// over its defaults.
fn instrument(
    editor: &mut Editor,
    device_type: &str,
    name: &str,
    params: &[(ParamId, f64)],
) -> Device {
    let id: DeviceId = editor.new_id();
    let mut device = Device::new(
        id,
        name,
        DeviceKind::Native {
            device_type: device_type.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    for (param, value) in params {
        device.params.insert(*param, *value);
    }
    device
}

/// A chain named `name` holding `device`.
fn chain(editor: &mut Editor, name: &str, device: Device) -> Chain {
    let id: ChainId = editor.new_id();
    Chain::new(id, name).with_devices(vec![device])
}

/// The instrument the track of a `kind` conversion holds (R9).
pub fn instrument_for(editor: &mut Editor, kind: ConvertKind) -> Device {
    let id: DeviceId = editor.new_id();
    let mut rack = Device::rack(
        id,
        match kind {
            ConvertKind::Drums => RackKind::Drum,
            ConvertKind::Harmony | ConvertKind::Melody => RackKind::Instrument,
        },
    );
    rack.name = kind.name().to_owned();
    match kind {
        ConvertKind::Harmony => {
            let keys = instrument(editor, polysynth::POLYSYNTH, "Drift", KEYS);
            rack.chains.push(chain(editor, "Keys", keys));
        }
        ConvertKind::Melody => {
            let synth = instrument(editor, polysynth::POLYSYNTH, "Drift", SYNTH);
            let piano = instrument(
                editor,
                electric::ELECTRIC,
                "Electric",
                electric::PRESETS[0].1,
            );
            let synth = chain(editor, "Synth", synth);
            let piano = chain(editor, "Electric Piano", piano);
            // Macro 1, Synth to Piano: the synth's chain falls from unity
            // to silence as the electric piano's rises from silence to
            // unity, on the fader's travel.
            rack.macros.setting_mut(1).name = "Synth to Piano".to_owned();
            rack.params.insert(macro_param(1), 0.0);
            let unity = fader_position(1.0);
            for (chain, min, max) in [(&synth, unity, 0.0), (&piano, 0.0, unity)] {
                rack.macros.mappings.push(MacroMapping {
                    macro_number: 1,
                    target: MacroTarget::Chain {
                        chain: chain.id,
                        param: MixerParam::Volume,
                    },
                    min,
                    max,
                });
            }
            rack.chains.push(synth);
            rack.chains.push(piano);
        }
        ConvertKind::Drums => {
            for (note, name, device_type, device_name, sound) in [
                (KICK, "Kick", ds_kick::DS_KICK, "DS Kick", KICK_SOUND),
                (SNARE, "Snare", ds_snare::DS_SNARE, "DS Snare", SNARE_SOUND),
                (
                    HI_HAT,
                    "Closed Hi-Hat",
                    ds_snare::DS_SNARE,
                    "DS Snare",
                    HAT_SOUND,
                ),
            ] {
                let device = instrument(editor, device_type, device_name, sound);
                let id: ChainId = editor.new_id();
                let mut pad = Chain::for_pad(id, name, note).with_devices(vec![device]);
                pad.play = PLAY_NOTE;
                rack.chains.push(pad);
            }
        }
    }
    rack
}
