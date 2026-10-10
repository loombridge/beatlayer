//! Commands scheduled at a musical position (D17): the engine runs each on
//! the exact frame the song reaches it, and reports when it did. Every block
//! runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Bpm, Frames, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::{Action, ParamValue, Report, ScheduledCommand, TransportCommand, testing};
use bl_model::{Command, TrackKind};
use common::{RATE, Rig, dc, device, key, track};

fn notify(id: u64, at: Ticks) -> ScheduledCommand {
    ScheduledCommand {
        id,
        at,
        action: Action::Notify,
    }
}

#[test]
fn a_scheduled_parameter_change_lands_on_its_frame() {
    let mut rig = Rig::new(100);
    let tempo = TempoMap::constant(Bpm::new(97.0).unwrap());
    rig.editor
        .apply(Command::SetTempo {
            tempo: tempo.clone(),
        })
        .unwrap();
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    let level = dc(&mut rig.editor, a, 1.0);
    rig.sync();

    // Bar 3, beat 2, and a triplet eighth: 3.2.1 plus a third of a beat.
    let at = Ticks::WHOLE * 2 + Ticks::QUARTER + Ticks(TICKS_PER_QUARTER / 3);
    let expected = tempo.ticks_to_frames(at, RATE).get() as usize;
    assert_eq!(
        expected, 277_113,
        "an exact frame that blocks of 100 don't line up with"
    );
    rig.sender.transport(TransportCommand::Play);
    rig.sender.schedule(ScheduledCommand {
        id: 7,
        at,
        action: Action::Param {
            key: key(level),
            param: testing::VALUE,
            value: ParamValue::Set(0.5),
        },
    });
    let out = rig.render(expected + 500, 100);
    assert_eq!(out.iter().position(|&s| s == 0.5), Some(expected));
    assert!(out[..expected].iter().all(|&s| s == 1.0));
    assert!(out[expected..].iter().all(|&s| s == 0.5));
    assert_eq!(
        rig.monitor.reports(),
        vec![Report::Fired {
            id: 7,
            position: Frames(expected as i64),
            clock: expected as u64,
        }]
    );
}

#[test]
fn a_scheduled_stop_stops_the_song_on_its_frame() {
    let mut rig = Rig::new(1000);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, a, testing::TRANSPORT, &[]);
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    // Bar 2 at 120 BPM: two seconds in.
    rig.sender.schedule(ScheduledCommand {
        id: 1,
        at: Ticks::WHOLE,
        action: Action::Transport(TransportCommand::Stop),
    });
    let out = rig.render(97_000, 1000);
    assert!(out[..96_000].iter().all(|&s| s == 1.0), "playing");
    assert!(out[96_000..].iter().all(|&s| s == 0.0), "stopped");
    let state = rig.monitor.state();
    assert!(!state.playing);
    assert_eq!(state.position, Frames(96_000));
    assert_eq!(state.position_ticks, Ticks::WHOLE);
}

#[test]
fn commands_fire_where_the_song_reaches_them_after_a_locate() {
    let mut rig = Rig::new(1000);
    track(&mut rig.editor, TrackKind::Midi, None);
    rig.sync();
    rig.sender.schedule(notify(1, Ticks::ZERO));
    rig.sender.schedule(notify(2, Ticks::WHOLE));
    rig.sender.transport(TransportCommand::Play);
    rig.render(48_000, 1000);
    // The song's own start fires at once.
    assert_eq!(
        rig.monitor.reports(),
        vec![Report::Fired {
            id: 1,
            position: Frames::ZERO,
            clock: 0,
        }]
    );
    // Jumping past bar 2 doesn't fire it...
    rig.sender
        .transport(TransportCommand::Locate(Ticks::WHOLE * 4));
    rig.render(4800, 1000);
    assert!(rig.monitor.reports().is_empty());
    // ...but playing through it again does.
    rig.sender.transport(TransportCommand::Locate(Ticks::ZERO));
    rig.render(100_000, 1000);
    assert_eq!(
        rig.monitor.reports(),
        vec![Report::Fired {
            id: 2,
            position: Frames(96_000),
            clock: 52_800 + 96_000,
        }]
    );
}

#[test]
fn a_tempo_change_keeps_the_playhead_and_commands_on_their_beats() {
    let mut rig = Rig::new(1000);
    track(&mut rig.editor, TrackKind::Midi, None);
    rig.sync();
    rig.sender.schedule(notify(3, Ticks::WHOLE * 2));
    rig.sender.transport(TransportCommand::Play);
    // One second at 120 BPM: beat 3 of bar 1.
    rig.render(48_000, 1000);
    rig.apply(Command::SetTempo {
        tempo: TempoMap::constant(Bpm::new(60.0).unwrap()),
    });
    rig.block(1000);
    let state = rig.monitor.state();
    // Two beats in at 60 BPM is two seconds, plus the block just played.
    assert_eq!(state.position, Frames(96_000 + 1000));
    // Bar 3 is eight beats: six more seconds from beat 3.
    rig.render(300_000, 1000);
    assert_eq!(
        rig.monitor.reports(),
        vec![Report::Fired {
            id: 3,
            position: Frames(384_000),
            clock: 48_000 + 288_000,
        }]
    );
}

#[test]
fn cancelled_and_overflowing_commands_never_fire() {
    let mut rig = Rig::new(1000);
    track(&mut rig.editor, TrackKind::Midi, None);
    rig.sync();
    let capacity = rig.processor.config().max_scheduled as u64;
    for id in 0..=capacity {
        rig.sender.schedule(notify(id, Ticks::QUARTER));
    }
    rig.sender.cancel(0);
    rig.sender.transport(TransportCommand::Play);
    rig.render(48_000, 1000);
    let reports = rig.monitor.reports();
    assert_eq!(reports[0], Report::Rejected { id: capacity });
    let fired: Vec<u64> = reports[1..]
        .iter()
        .map(|report| match report {
            Report::Fired { id, position, .. } => {
                assert_eq!(*position, Frames(24_000));
                *id
            }
            Report::Rejected { .. } => panic!("rejected twice"),
        })
        .collect();
    assert_eq!(
        fired,
        (1..capacity).collect::<Vec<_>>(),
        "in the order sent"
    );
    let counters = rig.monitor.state().counters;
    assert_eq!(counters.scheduled_rejected, 1);
    assert_eq!(counters.scheduled_fired, capacity - 1);
}
