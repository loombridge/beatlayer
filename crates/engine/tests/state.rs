//! Engine state for the UI, through a triple buffer, and node data swapped
//! in on its own message. Every block runs as a real-time section under the
//! allocation trap.

use crate::common;
use std::thread;

use bl_core::time::{Frames, TempoMap};
use bl_engine::{METER_RELEASE_DB_PER_SECOND, NodeKey, TransportCommand, testing};
use bl_model::{Command, TrackKind};
use common::{RATE, Rig, dc, device, key, track};

#[test]
fn the_state_shows_the_playhead_meters_and_cpu_load_to_another_thread() {
    let mut rig = Rig::new(256);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    let b = track(&mut rig.editor, TrackKind::Audio, None);
    dc(&mut rig.editor, a, 0.5);
    dc(&mut rig.editor, b, 0.25);
    let main = rig.editor.document().main_id();
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    rig.render(256 * 10, 256);

    // The UI reads from its own thread, without a lock.
    let mut monitor = rig.monitor;
    let state = thread::spawn(move || monitor.state().clone())
        .join()
        .unwrap();
    assert_eq!(state.clock, 2560);
    assert!(state.playing);
    assert_eq!(state.position, Frames(2560));
    assert_eq!(
        state.position_ticks,
        TempoMap::default().frames_to_ticks(Frames(2560), RATE)
    );
    assert_eq!(state.program, Some(1));
    assert_eq!(state.meter(a), Some([0.5, 0.5]));
    assert_eq!(state.meter(b), Some([0.25, 0.25]));
    assert_eq!(state.meter(main), Some([0.75, 0.75]));
    assert_eq!(
        state.meters.iter().map(|m| m.track).collect::<Vec<_>>(),
        vec![b, a, main],
        "in the order the tracks run"
    );
    assert!(state.cpu_load > 0.0 && state.cpu_load <= state.cpu_load_max);
    assert!(!state.meters_truncated);
}

#[test]
fn a_meter_falls_at_its_release_rate() {
    let mut rig = Rig::new(480);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, a, 1.0);
    rig.sync();
    rig.block(480);
    assert_eq!(rig.monitor.state().meter(a), Some([1.0, 1.0]));
    rig.apply(Command::SetMute {
        track: a,
        mute: true,
    });
    // One second of silence.
    rig.render(48_000, 480);
    let [left, _] = rig.monitor.state().meter(a).unwrap();
    let db = 20.0 * f64::from(left).log10();
    assert!(
        (db + METER_RELEASE_DB_PER_SECOND).abs() < 0.01,
        "fell to {db} dB"
    );
}

#[test]
fn node_data_swaps_in_at_a_block_and_the_old_data_is_freed_off_the_audio_thread() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    let probe = device(&mut rig.editor, a, testing::DATA, &[]);
    rig.sync();
    let gc = rig.collector.handle();

    rig.sender.send_data(key(probe), gc.data(0.5f32));
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.5));
    rig.sender.send_data(key(probe), gc.data(0.25f32));
    rig.block(64);
    assert!(rig.last().iter().all(|&s| s == 0.25));

    // The program and both values are held: the audio thread dropped the
    // first value but freed nothing.
    assert_eq!(rig.collector.held(), 3);
    rig.collector.collect();
    assert_eq!(rig.collector.held(), 2);

    // Data for a node the program doesn't have is dropped, safely.
    let gone = NodeKey::Mixer(rig.editor.new_id());
    rig.sender.send_data(gone, gc.data(1.0f32));
    rig.block(64);
    let counters = rig.monitor.state().counters;
    assert_eq!(counters.data_delivered, 2);
    assert_eq!(counters.data_unrouted, 1);
    rig.collector.collect();
    assert_eq!(rig.collector.held(), 2);
}
