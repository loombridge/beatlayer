//! WP-8.45's acceptance in the engine: the Vocoder's External carrier is the
//! device's audio sidechain (the vocoder spec R10, the routing spec R22 to
//! R24). With no source chosen, or the sidechain off, it plays silence; with
//! one, it plays that track through the modulator's bands. Every block runs
//! as a real-time section under the allocation trap, and the sidechain's
//! commands run by ID, as an agent runs them.

use crate::common;
use bl_commands::Registry as Commands;
use bl_devices::vocoder::{CARRIER, vocoder_type};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_model::{Command, DeviceId, Editor, TrackKind, registry};
use common::{Rig, device, track};
use serde_json::{Value, json};

fn run(editor: &mut Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// What counts as silence here, far under the 1e-3 or so the key plays at.
const SILENT: f64 = 1e-9;

/// The root mean square of Main over the last half second of a second's
/// rendering.
fn level(rig: &mut Rig) -> f64 {
    let out = rig.render(48_000, 512);
    let tail = &out[out.len() - 24_000..];
    (tail.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / tail.len() as f64).sqrt()
}

// Spec: vocoder R10
#[test]
fn external_plays_the_sidechain_source_through_the_modulators_bands_and_silence_without_one() {
    let mut rig = Rig::with_registry(512, bl_devices::vocoder::register);
    let ed = &mut rig.editor;
    let (key, speaker) = (
        track(ed, TrackKind::Audio, None),
        track(ed, TrackKind::Audio, None),
    );
    // The same tone on both tracks, the key's silenced on Main by its
    // fader (its Post FX tap, before the fader, still carries it); the
    // other track's is the modulator.
    for track in [key, speaker] {
        device(
            ed,
            track,
            TEST_TONE,
            &[(test_tone::FREQUENCY, 1_000.0), (test_tone::LEVEL, -6.0)],
        );
    }
    run(
        ed,
        "mixer.set_volume",
        json!({ "track": key, "volume": 0.0 }),
    );
    let id: DeviceId = ed.new_id();
    let mut vocoder = vocoder_type().device(id);
    vocoder.params.insert(CARRIER, 1.0);
    let index = ed.document().track(speaker).unwrap().devices().len();
    ed.apply(Command::InsertDevice {
        track: speaker,
        index,
        device: vocoder,
    })
    .unwrap();
    rig.sync();
    // No source chosen, the sidechain off: the carrier is silent.
    assert!(level(&mut rig) < SILENT);

    run(
        &mut rig.editor,
        "device.set_sidechain_input",
        json!({ "device": id, "input": { "type": "track", "track": key, "tap": "post_fx" } }),
    );
    rig.sync();
    assert!(
        level(&mut rig) < SILENT,
        "a source, but the sidechain still off"
    );

    run(
        &mut rig.editor,
        "device.set_sidechain_enabled",
        json!({ "device": id, "enabled": true }),
    );
    rig.sync();
    let heard = level(&mut rig);
    assert!(heard > 1e-3, "the key plays through the bands: {heard}");

    run(
        &mut rig.editor,
        "device.set_sidechain_enabled",
        json!({ "device": id, "enabled": false }),
    );
    rig.sync();
    // (What is left of the key's tone is the bands' ringing, dying away.)
    assert!(level(&mut rig) < SILENT);
}
