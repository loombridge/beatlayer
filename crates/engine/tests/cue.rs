//! WP-3.18's acceptance in the engine: Cue mode and the Cue Out (the mixer
//! spec).
//!
//! - With Cue mode on, a cue-switched track plays on the Cue Out's device
//!   channels at the Cue Volume while Main keeps what the activators leave,
//!   at any block size.
//! - The cue is taken after the track's devices and before its volume, pan
//!   and activator.
//! - With Cue mode off, or while Cue Out overlaps Main Out, solo behaves as
//!   it always did.
//! - Cue switches and the Cue Volume reach the engine without a new program.
//!
//! Commands run by ID with JSON arguments, as an agent runs them (D17).
//! Every block runs as a real-time section under the allocation trap.

use crate::common;
use bl_commands::Registry as Commands;
use bl_engine::testing::{GAIN, GAIN_PARAM};
use bl_model::{Command, Editor, TrackId, TrackKind, registry};
use common::{Rig, dc, device, track};
use serde_json::{Value, json};

/// Runs a document command by ID, as an agent does.
fn run(editor: &mut Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// Frames to render after a change, so a 20 ms glide has finished before
/// the last block of 256 frames.
const SETTLE: usize = 1280;

/// Main Out 1/2 and Cue Out 3/4 on a four-channel device, so Cue mode can be
/// on.
fn separate_outputs(editor: &mut Editor) {
    run(
        editor,
        "routing.set_cue_out",
        json!({ "channel": { "type": "pair", "channel": 3 } }),
    );
}

fn cue_mode(editor: &mut Editor, enabled: bool) {
    run(editor, "mixer.set_cue_mode", json!({ "enabled": enabled }));
}

fn cue_switch(editor: &mut Editor, track: TrackId, on: bool) {
    run(
        editor,
        "mixer.set_solo",
        json!({ "track": track, "solo": on }),
    );
}

/// Renders `SETTLE` frames of 256 and returns the last block's levels:
/// Main's left channel, then each device channel Ext. Out and the Cue Out
/// reach.
fn settled(rig: &mut Rig) -> (f32, Vec<f32>) {
    rig.render(SETTLE, 256);
    let output = rig.processor.output();
    let ext = (0..output.ext_channels())
        .map(|c| output.ext_channel(c)[255])
        .collect();
    (output.channel(0)[255], ext)
}

/// Two tracks, A at 0.5 and B at 0.25, on a four-channel device with the
/// outputs separated and Cue mode on.
fn rig() -> (Rig, TrackId, TrackId) {
    let mut rig = Rig::with_output(256, 4);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, a, 0.5);
    let b = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, b, 0.25);
    separate_outputs(&mut rig.editor);
    cue_mode(&mut rig.editor, true);
    rig.sync();
    (rig, a, b)
}

// Spec: mixer S5, R26
#[test]
fn a_cue_switched_track_feeds_the_cue_out_while_main_keeps_what_the_activators_leave() {
    let (mut rig, a, b) = rig();
    // Nothing cue-switched: the Cue Out is silent and Main has both.
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.75);
    assert_eq!(ext, [0.0; 4]);

    cue_switch(&mut rig.editor, a, true);
    let synced = rig.sync();
    assert!(!synced.program, "a Cue switch is a parameter change");
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.75, "cueing doesn't change Main");
    assert_eq!(ext, [0.0, 0.0, 0.5, 0.5], "A on 3/4 only");

    cue_switch(&mut rig.editor, b, true);
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.75);
    assert_eq!(ext, [0.0, 0.0, 0.75, 0.75], "A and B add");

    // A track's activator off leaves the cue as it is and silences Main.
    run(
        &mut rig.editor,
        "mixer.set_mute",
        json!({ "track": a, "mute": true }),
    );
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.25, "Main plays what the activators leave");
    assert_eq!(ext, [0.0, 0.0, 0.75, 0.75]);
}

// Spec: mixer S5, R27
#[test]
fn the_cue_volume_scales_the_cue_out_without_a_new_program() {
    let (mut rig, a, _) = rig();
    cue_switch(&mut rig.editor, a, true);
    rig.sync();
    let (_, ext) = settled(&mut rig);
    assert_eq!(ext, [0.0, 0.0, 0.5, 0.5]);

    run(
        &mut rig.editor,
        "mixer.set_cue_volume",
        json!({ "volume": 0.5 }),
    );
    let synced = rig.sync();
    assert!(!synced.program && synced.params > 0);
    let (main, ext) = settled(&mut rig);
    assert_eq!(ext, [0.0, 0.0, 0.25, 0.25]);
    assert_eq!(main, 0.75, "the Cue Volume doesn't touch Main");

    run(
        &mut rig.editor,
        "mixer.set_cue_volume",
        json!({ "volume": 0.0 }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig).1, [0.0; 4]);

    // +6 dB.
    run(
        &mut rig.editor,
        "mixer.set_cue_volume",
        json!({ "volume": bl_model::MAX_CUE_VOLUME }),
    );
    rig.sync();
    let (_, ext) = settled(&mut rig);
    assert!((f64::from(ext[2]) - 0.5 * bl_model::MAX_CUE_VOLUME).abs() < 1e-6);
}

// Spec: mixer S5, R26
#[test]
fn the_cue_is_taken_after_the_devices_and_before_the_fader_pan_and_activator() {
    let mut rig = Rig::with_output(256, 4);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, a, 0.5);
    device(&mut rig.editor, a, GAIN, &[(GAIN_PARAM, 0.5)]);
    separate_outputs(&mut rig.editor);
    cue_mode(&mut rig.editor, true);
    cue_switch(&mut rig.editor, a, true);
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!(
        (main, ext.as_slice()),
        (0.25, [0.0, 0.0, 0.25, 0.25].as_slice())
    );

    // Fader at −∞, pan hard left and the activator off: Main is silent or
    // moved, the cue is where it was.
    for (id, args) in [
        ("mixer.set_volume", json!({ "track": a, "volume": 0.0 })),
        ("mixer.set_pan", json!({ "track": a, "pan": -1.0 })),
        ("mixer.set_mute", json!({ "track": a, "mute": true })),
    ] {
        run(&mut rig.editor, id, args);
    }
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.0);
    assert_eq!(ext, [0.0, 0.0, 0.25, 0.25]);
}

// Spec: mixer S5, R25
#[test]
fn groups_and_returns_have_cue_switches_too() {
    let mut rig = Rig::with_output(256, 4);
    let group = track(&mut rig.editor, TrackKind::Group, None);
    let inner = track(&mut rig.editor, TrackKind::Audio, Some(group));
    dc(&mut rig.editor, inner, 0.5);
    let ret = track(&mut rig.editor, TrackKind::Return, None);
    separate_outputs(&mut rig.editor);
    cue_mode(&mut rig.editor, true);
    cue_switch(&mut rig.editor, group, true);
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.5);
    assert_eq!(ext, [0.0, 0.0, 0.5, 0.5], "the group's sum, not the track");

    // A return carries what is sent to it.
    run(
        &mut rig.editor,
        "mixer.set_send",
        json!({ "track": inner, "to": ret, "level": 0.5 }),
    );
    cue_switch(&mut rig.editor, group, false);
    cue_switch(&mut rig.editor, ret, true);
    rig.sync();
    let (_, ext) = settled(&mut rig);
    assert_eq!(ext, [0.0, 0.0, 0.25, 0.25]);
}

// Spec: mixer S5, R10, R25
#[test]
fn cue_mode_off_solo_silences_the_other_tracks_as_it_always_did() {
    let (mut rig, a, _) = rig();
    cue_mode(&mut rig.editor, false);
    cue_switch(&mut rig.editor, a, true);
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.5, "only A is heard");
    assert!(ext.is_empty(), "no Cue Out in Solo mode: {ext:?}");

    // Cue mode with A's switch still on: nothing is silenced.
    cue_mode(&mut rig.editor, true);
    let synced = rig.sync();
    assert!(synced.program, "Cue mode is a new program");
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.75);
    assert_eq!(ext, [0.0, 0.0, 0.5, 0.5], "the switch is the cue switch");
}

// Spec: mixer S10, R24
#[test]
fn an_overlap_of_cue_out_and_main_out_leaves_solo_mode() {
    let (mut rig, a, _) = rig();
    cue_switch(&mut rig.editor, a, true);
    rig.sync();
    assert_eq!(settled(&mut rig).1, [0.0, 0.0, 0.5, 0.5]);

    // Cue Out onto Main Out's channels: Cue mode stays set but plays
    // nothing, and the soloed A silences B as in Solo mode.
    run(
        &mut rig.editor,
        "routing.set_cue_out",
        json!({ "channel": { "type": "pair", "channel": 1 } }),
    );
    assert!(rig.editor.document().settings().cue_mode);
    assert!(!rig.editor.document().cue_active());
    assert!(rig.sync().program);
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.5);
    assert!(ext.is_empty());

    // Cue Out moves to 2/3: still an overlap with 1/2.
    run(
        &mut rig.editor,
        "routing.set_cue_out",
        json!({ "channel": { "type": "pair", "channel": 2 } }),
    );
    rig.sync();
    assert!(settled(&mut rig).1.is_empty());

    // And back to 3/4: the cue returns.
    separate_outputs(&mut rig.editor);
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.75);
    assert_eq!(ext, [0.0, 0.0, 0.5, 0.5]);
}

// Spec: mixer S5, R26
#[test]
fn the_cue_out_is_the_same_to_the_bit_at_any_block_size() {
    let render = |block: usize| {
        let (mut rig, a, b) = rig();
        // A track that isn't constant: the device chain's gain at 0.5 on A.
        device(&mut rig.editor, a, GAIN, &[(GAIN_PARAM, 0.5)]);
        cue_switch(&mut rig.editor, a, true);
        rig.sync();
        let mut cue = Vec::new();
        let mut play = |rig: &mut Rig, total: usize| {
            let mut done = 0;
            while done < total {
                let n = block.min(total - done);
                rig.block(n);
                cue.extend_from_slice(&rig.processor.output().ext_channel(2)[..n]);
                done += n;
            }
        };
        play(&mut rig, 1024);
        // Switching B in mid-render glides over 20 ms.
        cue_switch(&mut rig.editor, b, true);
        rig.sync();
        play(&mut rig, 3072);
        cue
    };
    let reference = render(256);
    for block in [1, 61, 128, 256] {
        let got = render(block);
        assert_eq!(got.len(), reference.len());
        assert!(
            got.iter()
                .zip(&reference)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "block {block}"
        );
    }
    assert!(
        reference.last().copied().unwrap() > 0.37,
        "A and B at the end"
    );
}

// Spec: mixer S5, R26
#[test]
fn a_new_cue_track_and_a_deleted_one_change_the_cue() {
    let (mut rig, a, _) = rig();
    cue_switch(&mut rig.editor, a, true);
    rig.sync();
    let c = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, c, 0.125);
    cue_switch(&mut rig.editor, c, true);
    rig.sync();
    assert_eq!(settled(&mut rig).1, [0.0, 0.0, 0.625, 0.625]);
    rig.editor.apply(Command::DeleteTrack { track: a }).unwrap();
    rig.sync();
    assert_eq!(settled(&mut rig).1, [0.0, 0.0, 0.125, 0.125]);
}
