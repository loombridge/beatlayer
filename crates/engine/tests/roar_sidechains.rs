//! WP-8.39's acceptance in the engine: Roar's audio sidechain reaches its
//! envelope follower only (the roar spec R22, R28, R29), with the routing
//! spec's gain and mix. Every block runs as a real-time section under the
//! allocation trap, and the sidechain's commands run by ID, as an agent runs
//! them.

use crate::common;
use bl_commands::Registry as Commands;
use bl_devices::roar::{
    ENV_ATTACK, ENV_FREQUENCY, ENV_THRESHOLD, ENV_WIDTH, ON, matrix_param, roar_type, stage_param,
};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_model::{Command, DeviceId, Editor, TrackId, TrackKind, registry};
use common::{Rig, dc, device, track};
use serde_json::{Value, json};

fn run(editor: &mut Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// A song with a loud 1 kHz tone on one track, and on another a quiet DC
/// level through a Roar whose two stages are off (so the level passes) and
/// whose Env moves Drive by 50 % of its range.
fn song(rig: &mut Rig) -> (TrackId, DeviceId) {
    let ed = &mut rig.editor;
    let (key, roar) = (
        track(ed, TrackKind::Audio, None),
        track(ed, TrackKind::Audio, None),
    );
    device(
        ed,
        key,
        TEST_TONE,
        &[(test_tone::FREQUENCY, 1_000.0), (test_tone::LEVEL, -6.0)],
    );
    dc(ed, roar, 0.05);
    let id = ed.new_id();
    let mut device = roar_type().device(id);
    for (param, value) in [
        (stage_param(0, ON), 0.0),
        (stage_param(1, ON), 0.0),
        (ENV_THRESHOLD, -30.0),
        (ENV_FREQUENCY, 1_000.0),
        (ENV_WIDTH, 2.0),
        (ENV_ATTACK, 1.0),
        (matrix_param(2, 0), 50.0),
    ] {
        device.params.insert(param, value);
    }
    let index = ed.document().track(roar).unwrap().devices().len();
    ed.apply(Command::InsertDevice {
        track: roar,
        index,
        device,
    })
    .unwrap();
    run(
        ed,
        "device.set_sidechain_input",
        json!({ "device": id, "input": { "type": "track", "track": key, "tap": "post_fx" } }),
    );
    rig.sync();
    (roar, id)
}

/// The DC level of Main over its last 4,800 frames, a hundred cycles of the
/// tone, which averages out.
fn level(rig: &mut Rig) -> f32 {
    let out = rig.render(24_000, 512);
    out[out.len() - 4_800..].iter().sum::<f32>() / 4_800.0
}

// Spec: roar R22, R28, R29
#[test]
fn the_external_sidechain_drives_env_and_nothing_else() {
    let mut rig = Rig::with_registry(512, bl_devices::roar::register);
    let (_, roar) = song(&mut rig);
    // External off: Env follows the DC input, which its band-pass takes
    // away, so Drive stays where it is.
    let own = level(&mut rig);
    assert!((own - 0.05).abs() < 1e-3, "{own}");
    // External on: Env hears the tone, so it opens Drive.
    run(
        &mut rig.editor,
        "device.set_sidechain_enabled",
        json!({ "device": roar, "enabled": true }),
    );
    rig.sync();
    let keyed = level(&mut rig);
    assert!(keyed > own * 1.5, "{keyed} against {own}");
    // Mix at 0 hears the device's own input again.
    run(
        &mut rig.editor,
        "device.set_sidechain_mix",
        json!({ "device": roar, "mix": 0.0 }),
    );
    rig.sync();
    let mixed = level(&mut rig);
    assert!((mixed - own).abs() < 1e-3, "{mixed}");
}
