//! WP-2.13's acceptance in the engine: a track whose Audio To is Ext. Out
//! plays on the audio device's output channels it names, and not into Main
//! (the routing spec).
//!
//! - A pair plays left and right on its two channels, a single channel
//!   takes both added and 6 dB down, and Main's mix is untouched.
//! - A channel the device lacks plays nothing.
//! - A track that hears Main plays nothing on Ext. Out.
//! - Every block runs as a real-time section under the allocation trap.
//!
//! Routing commands run by ID with JSON arguments, as an agent runs them
//! (D17).

bl_rt::install_trap!();

mod common;

use bl_commands::Registry as Commands;
use bl_engine::TransportCommand;
use bl_model::{Command, CountIn, TrackId, TrackKind, registry};
use common::{Rig, dc, track};
use serde_json::{Value, json};

/// Runs a document command by ID, as an agent does.
fn run(editor: &mut bl_model::Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// Sets a track's Audio To to Ext. Out on `channel`.
fn ext_out(editor: &mut bl_model::Editor, track: TrackId, channel: Value) {
    run(
        editor,
        "routing.set_output",
        json!({ "track": track, "output": { "type": "external", "channel": channel } }),
    );
}

/// Frames to render after a change, so a 20 ms glide has finished before
/// the last block of 256 frames.
const SETTLE: usize = 1280;

/// Renders `SETTLE` frames of 256 and returns the last block's levels:
/// Main's left channel, then each device channel Ext. Out reaches (the
/// last sample of each; the signals are steady).
fn settled(rig: &mut Rig) -> (f32, Vec<f32>) {
    rig.render(SETTLE, 256);
    let output = rig.processor.output();
    let ext = (0..output.ext_channels())
        .map(|c| output.ext_channel(c)[255])
        .collect();
    (output.channel(0)[255], ext)
}

fn rig_with_dc(outputs: usize, value: f64) -> (Rig, TrackId) {
    let mut rig = Rig::with_output(256, outputs);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, a, value);
    (rig, a)
}

// Spec: routing R3 (WP-2.13)
#[test]
fn a_pair_plays_on_its_two_channels_and_not_in_main() {
    let (mut rig, a) = rig_with_dc(4, 0.5);
    ext_out(&mut rig.editor, a, json!({ "type": "pair", "channel": 3 }));
    assert!(rig.sync().program, "a routing change is a new program");
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.0, "Main doesn't carry a track on Ext. Out");
    assert_eq!(ext, [0.0, 0.0, 0.5, 0.5], "channels 3 and 4 only");

    // Back to Main: nothing on the device's channels.
    run(
        &mut rig.editor,
        "routing.set_output",
        json!({ "track": a, "output": { "type": "main" } }),
    );
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!((main, ext.len()), (0.5, 0));
}

// Spec: routing R3 (WP-2.13)
#[test]
fn a_single_channel_takes_both_sides_added_and_six_db_down() {
    let (mut rig, a) = rig_with_dc(4, 0.5);
    ext_out(&mut rig.editor, a, json!({ "type": "mono", "channel": 3 }));
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.0);
    assert_eq!(ext, [0.0, 0.0, 0.5], "channel 3 alone: (0.5 + 0.5) / 2");
}

// Spec: routing R3 (WP-2.13)
#[test]
fn tracks_on_the_same_channels_add_and_the_rest_play_in_main() {
    let (mut rig, a) = rig_with_dc(2, 0.25);
    let b = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, b, 0.125);
    let c = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, c, 0.5);
    ext_out(&mut rig.editor, a, json!({ "type": "pair", "channel": 1 }));
    ext_out(&mut rig.editor, b, json!({ "type": "pair", "channel": 1 }));
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.5, "C alone is in Main");
    assert_eq!(ext, [0.375, 0.375], "A and B add");
}

// Spec: routing R3 (WP-2.13)
#[test]
fn a_channel_the_device_lacks_plays_nothing() {
    let (mut rig, a) = rig_with_dc(2, 0.5);
    ext_out(&mut rig.editor, a, json!({ "type": "pair", "channel": 3 }));
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!(main, 0.0);
    assert_eq!(ext, [0.0, 0.0], "no channels 3 and 4 to play on");

    // 2/3 on a two-channel device: the left side plays on 2, the right is lost.
    ext_out(&mut rig.editor, a, json!({ "type": "pair", "channel": 2 }));
    rig.sync();
    let (_, ext) = settled(&mut rig);
    assert_eq!(ext, [0.0, 0.5]);

    // A channel number past any device, from the agent API, is just absent.
    ext_out(
        &mut rig.editor,
        a,
        json!({ "type": "pair", "channel": 65535 }),
    );
    rig.sync();
    let (_, ext) = settled(&mut rig);
    assert_eq!(ext, [0.0, 0.0]);

    // No device at all.
    let mut rig = Rig::new(256);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, a, 0.5);
    ext_out(&mut rig.editor, a, json!({ "type": "pair", "channel": 1 }));
    rig.sync();
    let (main, ext) = settled(&mut rig);
    assert_eq!((main, ext.len()), (0.0, 0));
}

// Spec: routing R3, R14 (WP-2.13)
#[test]
fn a_track_that_hears_main_is_silent_on_ext_out() {
    let (mut rig, a) = rig_with_dc(2, 0.5);
    run(
        &mut rig.editor,
        "routing.set_input",
        json!({ "track": a, "input": { "type": "resampling" } }),
    );
    ext_out(&mut rig.editor, a, json!({ "type": "pair", "channel": 1 }));
    rig.sync();
    let (_, ext) = settled(&mut rig);
    assert_eq!(ext.len(), 0, "a resampling track's output is suppressed");
}

// Spec: routing R3; mixer R10 (WP-2.13)
#[test]
fn mute_silences_a_track_on_ext_out() {
    let (mut rig, a) = rig_with_dc(2, 0.5);
    ext_out(&mut rig.editor, a, json!({ "type": "pair", "channel": 1 }));
    rig.sync();
    assert_eq!(settled(&mut rig).1, [0.5, 0.5]);
    run(
        &mut rig.editor,
        "mixer.set_mute",
        json!({ "track": a, "mute": true }),
    );
    rig.sync();
    assert_eq!(settled(&mut rig).1, [0.0, 0.0]);
}

// Spec: routing R3; metronome R5 (WP-2.13). A count-in plays only the
// clicks, so it leaves no earlier block on the device's channels.
#[test]
fn a_count_in_leaves_nothing_on_ext_out() {
    let (mut rig, a) = rig_with_dc(2, 0.5);
    for command in [
        Command::SetMetronomeEnabled { enabled: true },
        Command::SetMetronomeCountIn {
            count_in: CountIn::Bars1,
        },
    ] {
        rig.editor.apply(command).unwrap();
    }
    ext_out(&mut rig.editor, a, json!({ "type": "pair", "channel": 1 }));
    rig.sync();
    assert_eq!(settled(&mut rig).1, [0.5, 0.5]);
    rig.sender.transport(TransportCommand::Play);
    let (_, ext) = settled(&mut rig);
    assert!(ext.iter().all(|&s| s == 0.0), "{ext:?}");
}
