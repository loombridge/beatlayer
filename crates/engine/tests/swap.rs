//! Program swaps: surviving nodes keep their state, and what the audio
//! thread lets go of is freed elsewhere. Every block runs as a real-time
//! section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::sync::atomic::Ordering::Relaxed;

use bl_engine::{Block, Node, testing};
use bl_model::{Command, TrackKind};
use common::{Rig, dc, device, track};

/// Breaks the real-time rules: allocates in every block.
struct Allocates;

impl Node for Allocates {
    fn process(&mut self, block: &mut Block<'_>) {
        let scratch = vec![0.0f32; block.frames()];
        block.channel_mut(0)[0] += scratch[0];
    }
}

#[test]
#[should_panic(expected = "heap operations in a real-time section")]
fn a_node_that_allocates_fails_the_rig() {
    let mut rig = Rig::with_registry(64, |registry| {
        registry.add_native("test.allocates", |_, _| Box::new(Allocates));
    });
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, a, "test.allocates", &[]);
    rig.sync();
    rig.block(64);
}

#[test]
fn a_counter_keeps_its_state_across_a_thousand_swaps() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    let b = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, a, testing::COUNTER, &[]);
    dc(&mut rig.editor, a, 0.25);
    assert!(rig.sync().program);
    rig.block(64);
    let counter = rig.probe.counter.load(Relaxed);
    assert_eq!(counter, 1);

    let mut spare = None;
    let mut frames = 64u64;
    for round in 0..1000 {
        // An edit that changes the song's structure, so each one compiles.
        let command = match (round % 4, spare) {
            (0, _) => {
                spare = Some(rig.editor.new_id());
                Command::InsertDevice {
                    track: b,
                    index: 0,
                    device: bl_model::Device::new(
                        spare.unwrap(),
                        "",
                        bl_model::DeviceKind::Native {
                            device_type: testing::DC.to_owned(),
                        },
                    ),
                }
            }
            (1, _) => Command::MoveTrack {
                track: b,
                parent: None,
                index: if round % 8 == 1 { 1 } else { 0 },
            },
            (2, Some(device)) => Command::SetDeviceEnabled {
                device,
                enabled: false,
            },
            (_, Some(device)) => Command::RemoveDevice { device },
            _ => unreachable!(),
        };
        assert!(rig.apply(command).program, "round {round} didn't compile");
        rig.block(64);
        frames += 64;
        // Every frame is A's DC through the swap: nothing was made again,
        // nothing dropped out.
        assert!(rig.last().iter().all(|&s| s == 0.25), "round {round}");
        assert_eq!(rig.probe.counter.load(Relaxed), counter, "round {round}");
        assert_eq!(rig.probe.counted.load(Relaxed), frames, "round {round}");
    }
    let state = rig.monitor.state();
    assert_eq!(state.swaps, 1001);
    assert_eq!(state.program, Some(1001));
    assert_eq!(state.counters.migration_mismatches, 0);

    // The spare DC nodes, made and removed 250 times, were all freed once
    // collected, and only the running program is still held.
    rig.collector.collect();
    let made = rig.probe.made.load(Relaxed);
    assert_eq!(made, 2 + 250);
    assert_eq!(rig.probe.dropped.load(Relaxed), 250);
    assert_eq!(rig.collector.held(), 1);
}

#[test]
fn removed_nodes_wait_for_the_collector() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    let spare = dc(&mut rig.editor, a, 0.5);
    rig.sync();
    rig.block(64);
    assert_eq!(rig.last()[0], 0.5);

    rig.apply(Command::RemoveDevice { device: spare });
    rig.block(64);
    assert_eq!(rig.last()[0], 0.0);
    // The audio thread let go of the old program, and the node in it, but
    // freed nothing: the collector still holds both programs.
    assert_eq!(rig.probe.dropped.load(Relaxed), 0);
    assert_eq!(rig.collector.held(), 2);
    rig.collector.collect();
    assert_eq!(rig.probe.dropped.load(Relaxed), 1);
    assert_eq!(rig.collector.held(), 1);
}

#[test]
fn a_switched_off_device_keeps_its_state_and_takes_parameter_changes() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    let counter = device(&mut rig.editor, a, testing::COUNTER, &[]);
    let level = dc(&mut rig.editor, a, 0.5);
    rig.sync();
    rig.block(64);

    for device in [counter, level] {
        rig.apply(Command::SetDeviceEnabled {
            device,
            enabled: false,
        });
    }
    rig.block(64);
    assert_eq!(rig.last()[0], 0.0, "off: no DC");
    assert_eq!(rig.probe.counted.load(Relaxed), 64, "off: not counting");
    // A change while it's off still reaches it, without a recompile.
    let synced = rig.apply(Command::SetDeviceParam {
        device: level,
        param: testing::VALUE,
        value: 0.75,
    });
    assert!(!synced.program);
    rig.block(64);

    for device in [counter, level] {
        rig.apply(Command::SetDeviceEnabled {
            device,
            enabled: true,
        });
    }
    rig.block(64);
    assert_eq!(
        rig.last()[0],
        0.75,
        "back on, with the change made while off"
    );
    assert_eq!(rig.probe.counted.load(Relaxed), 128, "counting on from 64");
    assert_eq!(rig.probe.made.load(Relaxed), 2, "neither was made again");
}

#[test]
fn a_program_out_of_order_is_counted_and_runs_without_the_missing_nodes() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, a, 0.5);
    // Compile twice but send only the second, which expects the first.
    drop(rig.compiler.sync(rig.editor.document()));
    track(&mut rig.editor, TrackKind::Midi, None);
    rig.sync();
    rig.block(64);
    let state = rig.monitor.state();
    assert_eq!(state.counters.migration_mismatches, 1);
    assert_eq!(state.program, Some(2));
    assert_eq!(rig.last()[0], 0.0, "the DC node never arrived");
}
