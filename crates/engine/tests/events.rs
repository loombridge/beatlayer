//! Parameter changes: they reach their nodes on their exact frame, through
//! the queue, without a recompile. Every block runs as a real-time section
//! under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_engine::nodes::fader;
use bl_engine::{NodeKey, ParamEvent, ParamValue, When, testing};
use bl_model::{Command, TrackKind};
use common::{Rig, dc, key, track};

#[test]
fn a_parameter_change_lands_on_its_frame() {
    let mut rig = Rig::new(128);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    let level = dc(&mut rig.editor, a, 1.0);
    rig.sync();
    // Frame 1,000 is 104 frames into the eighth block of 128. Two changes
    // land on it, and the later one sent wins; another lands a frame on.
    for (value, frame) in [(0.5, 1000), (0.25, 1000), (0.75, 1001)] {
        rig.sender.send_param(ParamEvent::at_frame(
            key(level),
            testing::VALUE,
            value,
            frame,
        ));
    }
    let out = rig.render(2048, 128);
    assert!(out[..1000].iter().all(|&s| s == 1.0));
    assert_eq!(out[1000], 0.25);
    assert!(out[1001..].iter().all(|&s| s == 0.75));
    let state = rig.monitor.state();
    assert_eq!(state.swaps, 1, "no recompile");
    assert_eq!(state.counters.events_applied, 3);
}

#[test]
fn mixer_edits_reach_the_fader_without_a_recompile() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, a, 1.0);
    rig.sync();
    rig.block(64);
    assert_eq!(rig.last()[0], 1.0);

    let synced = rig.apply(Command::SetVolume {
        track: a,
        volume: 0.5,
    });
    assert!(!synced.program);
    assert_eq!(synced.params, 1);
    rig.block(64);
    // From the first frame of the next block.
    assert!(rig.last().iter().all(|&s| s == 0.5));

    rig.apply(Command::SetMute {
        track: a,
        mute: true,
    });
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.0));

    // Main's fader too.
    rig.apply(Command::SetMute {
        track: a,
        mute: false,
    });
    let main = rig.editor.document().main_id();
    rig.apply(Command::SetVolume {
        track: main,
        volume: 0.25,
    });
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.125));
    assert_eq!(rig.monitor.state().swaps, 1);
}

#[test]
fn device_parameter_edits_reach_the_node_without_a_recompile() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    let level = dc(&mut rig.editor, a, 1.0);
    rig.sync();
    rig.block(64);

    let synced = rig.apply(Command::SetDeviceParam {
        device: level,
        param: testing::VALUE,
        value: 0.3,
    });
    assert!(!synced.program);
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.3f32));

    // Undo puts the old value back, still without a recompile.
    rig.editor.undo().unwrap();
    assert!(!rig.sync().program);
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 1.0));
    assert_eq!(rig.monitor.state().swaps, 1);
}

#[test]
fn a_change_waits_for_its_frame_across_a_swap() {
    let mut rig = Rig::new(128);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    let level = dc(&mut rig.editor, a, 1.0);
    rig.sync();
    rig.sender
        .send_param(ParamEvent::at_frame(key(level), testing::VALUE, 0.5, 1500));
    let mut out = rig.render(640, 128);
    // A structural edit before the change's frame: the node migrates into
    // the new program, and the change still finds it.
    let b = track(&mut rig.editor, TrackKind::Audio, None);
    assert!(rig.sync().program);
    out.extend(rig.render(1408, 128));
    assert!(out[..1500].iter().all(|&s| s == 1.0));
    assert!(out[1500..].iter().all(|&s| s == 0.5));
    let state = rig.monitor.state();
    assert_eq!(state.swaps, 2);
    assert!(state.meter(b).is_some());
}

#[test]
fn a_default_value_and_a_change_for_a_missing_node() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    let level = dc(&mut rig.editor, a, 1.0);
    rig.sync();
    rig.sender.send_param(ParamEvent {
        key: key(level),
        param: testing::VALUE,
        value: ParamValue::Default,
        at: When::Now,
    });
    let gone = NodeKey::Fader(rig.editor.new_id());
    rig.sender
        .send_param(ParamEvent::now(gone, fader::VOLUME, 0.0));
    rig.block(64);
    assert!(
        rig.last().iter().all(|&s| s == 0.0),
        "the DC's default is 0"
    );
    let counters = rig.monitor.state().counters;
    assert_eq!(counters.events_applied, 1);
    assert_eq!(counters.events_unrouted, 1);
}
