//! The engine restarting with a new processor, as a device supervisor
//! restarts it when a stream closes and opens again (WP-1.13,
//! `docs/specs/device-changes.md`): by hand here, with no device, through
//! the engine's `Restarter`. `bl-audio-engine`'s tests do the same on the
//! null device. Blocks that check the transport run as real-time sections
//! under the allocation trap.

bl_rt::install_trap!();

use std::sync::Arc;
use std::time::{Duration, Instant};

use bl_core::time::{Frames, SampleRate, TempoMap, Ticks};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::testing::{self, Probe};
use bl_engine::{
    Action, Carry, Engine, EngineConfig, LoopRegion, Offline, Processor, Report, ScheduledCommand,
    TransportCommand, When,
};
use bl_model::{Command, Device, DeviceKind, Editor, TrackKind};

/// A song with one test tone at 1 kHz, −60 dBFS.
fn song() -> Editor {
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Tone".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let mut device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        },
    );
    device.params.insert(test_tone::FREQUENCY, 1_000.0);
    device.params.insert(test_tone::LEVEL, -60.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    editor
}

fn config(rate: SampleRate) -> EngineConfig {
    EngineConfig::new(rate, 256)
}

/// A detached engine with the song synced.
fn detached() -> (Engine, Editor) {
    let probe = Arc::new(Probe::default());
    let engine =
        Engine::start_detached(config(SampleRate::HZ_48000), testing::registry(&probe)).unwrap();
    let editor = song();
    engine.sync(editor.document());
    assert!(engine.flush(Duration::from_secs(5)));
    (engine, editor)
}

fn restart(engine: &Engine, rate: SampleRate, carry: Option<Carry>) -> Processor {
    engine
        .restarter()
        .restart(config(rate), carry, Duration::from_secs(5))
        .expect("a processor")
}

/// The first channel of `blocks` blocks of 256 frames.
fn render(processor: &mut Processor, blocks: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..blocks {
        out.extend_from_slice(processor.process(256).channel(0));
    }
    out
}

// Spec: device-changes R3
#[test]
fn a_restart_at_a_new_rate_renders_like_a_fresh_engine() {
    let (mut engine, editor) = detached();
    let mut first = restart(&engine, SampleRate::HZ_48000, None);
    render(&mut first, 4);
    let carry = first.park();
    drop(first);
    engine.restarter().detach();
    let mut restarted = restart(&engine, SampleRate::HZ_44100, Some(carry));

    let probe = Arc::new(Probe::default());
    let mut fresh = Offline::new(config(SampleRate::HZ_44100), testing::registry(&probe));
    fresh.sync(editor.document());
    let mut expected = Vec::new();
    for _ in 0..8 {
        expected.extend_from_slice(fresh.process(256).channel(0));
    }
    let actual = render(&mut restarted, 8);
    assert!(actual.iter().any(|&s| s != 0.0));
    assert_eq!(
        actual, expected,
        "the restarted engine plays the song afresh at 44.1 kHz"
    );
    assert_eq!(engine.state().sample_rate, SampleRate::HZ_44100);
}

// Spec: device-changes R7
#[test]
fn a_restart_keeps_the_beat_and_stops_the_transport() {
    let (mut engine, _editor) = detached();
    let mut first = restart(&engine, SampleRate::HZ_48000, None);
    engine.play();
    assert!(engine.flush(Duration::from_secs(5)));
    // Two beats at 120 bpm: 48,000 frames.
    for _ in 0..(48_000 / 256 + 1) {
        first.process(256);
    }
    let played = first.position();
    assert!(first.playing());
    let carry = first.park();
    assert!(
        !engine.state().playing,
        "parking publishes a stopped transport"
    );
    drop(first);
    engine.restarter().detach();

    let restarted = restart(&engine, SampleRate::HZ_44100, Some(carry.clone()));
    assert!(!restarted.playing());
    // The same beat, to the frame: a frame at 44.1 kHz and 120 bpm is 32
    // ticks.
    let at = TempoMap::default().frames_to_ticks(restarted.position(), SampleRate::HZ_44100);
    assert!((at.get() - carry.position().get()).abs() <= 32, "{at:?}");
    let expected = Frames((played.get() as f64 * 44_100.0 / 48_000.0).round() as i64);
    assert!((restarted.position().get() - expected.get()).abs() <= 1);
    // The engine reads the new processor's state, ready before a block.
    let state = engine.state();
    assert_eq!(state.sample_rate, SampleRate::HZ_44100);
    assert_eq!(state.program, Some(1));
    assert_eq!(state.position, restarted.position());
    assert_eq!(engine.counts().restarts, 2);
}

// Spec: device-changes R7
#[test]
fn scheduled_commands_survive_a_restart() {
    let (mut engine, _editor) = detached();
    let mut first = restart(&engine, SampleRate::HZ_48000, None);

    for (id, at) in [
        (1, Ticks::QUARTER),
        (2, Ticks::from_quarters(3)),
        (3, Ticks::from_quarters(4)),
    ] {
        engine.schedule(ScheduledCommand {
            id,
            at,
            action: Action::Notify,
        });
    }
    engine.play();
    assert!(engine.flush(Duration::from_secs(5)));
    // Past the first beat (24,000 frames), short of the third.
    for _ in 0..120 {
        first.process(256);
    }
    engine.cancel(3);
    assert!(engine.flush(Duration::from_secs(5)));
    first.process(256);
    let carry = first.carry();
    assert_eq!(
        carry.scheduled().iter().map(|c| c.id).collect::<Vec<_>>(),
        [2]
    );
    drop(first);
    engine.restarter().detach();

    let mut restarted = restart(&engine, SampleRate::HZ_44100, Some(carry));
    // Play on from where the song stopped, through the new processor's
    // queue.
    engine.transport(TransportCommand::Continue);
    assert!(engine.flush(Duration::from_secs(5)));
    for _ in 0..400 {
        restarted.process(256);
    }
    let reports = engine.reports();
    let fired: Vec<(u64, Frames)> = reports
        .iter()
        .filter_map(|report| match *report {
            Report::Fired { id, position, .. } => Some((id, position)),
            _ => None,
        })
        .collect();
    // Beat 3 lands at 66,150 frames at 44.1 kHz.
    assert_eq!(fired, [(1, Frames(24_000)), (2, Frames(66_150))]);
}

// Spec: device-changes R7
#[test]
fn while_detached_play_is_ignored_and_a_locate_is_kept() {
    let (mut engine, mut editor) = detached();
    let first = restart(&engine, SampleRate::HZ_48000, None);
    drop(first);
    engine.restarter().detach();
    let programs = engine.counts().programs;
    let deferred = engine.counts().deferred;
    engine.play();
    engine.locate(Ticks::from_quarters(8));
    // Edits while detached are kept, not compiled.
    let track = editor.document().top_level().next().unwrap();
    editor
        .apply(Command::SetVolume { track, volume: 0.5 })
        .unwrap();
    engine.sync(editor.document());
    let began = Instant::now();
    assert!(engine.flush(Duration::from_secs(5)));
    assert!(
        began.elapsed() < Duration::from_secs(1),
        "a flush returns at once while detached"
    );
    assert_eq!(engine.counts().programs, programs);
    assert_eq!(engine.counts().deferred, deferred + 1);

    let mut restarted = restart(&engine, SampleRate::HZ_44100, None);
    assert!(!restarted.playing(), "play while detached is dropped");
    assert_eq!(engine.counts().programs, programs + 1);
    // A locate on the stopped song moves the insert marker, and the
    // playhead stays where playback stopped (transport R7).
    assert_eq!(restarted.insert_marker(), Ticks::from_quarters(8));
    assert_eq!(restarted.position(), Frames::ZERO);
    let state = engine.state();
    assert_eq!(state.insert_marker, Ticks::from_quarters(8));
    assert_eq!(state.position_ticks, Ticks::ZERO);
    // Play starts there.
    engine.play();
    assert!(engine.flush(Duration::from_secs(5)));
    bl_rt::rt_test!({ restarted.process(256) });
    assert!(restarted.playing());
    assert_eq!(restarted.position(), Frames(4 * 44_100 + 256));
}

/// A detached engine whose song's only track plays its song position:
/// each frame of the output is that frame's position while the song
/// plays, and −1 while it's stopped (`testing::POSITION`). For
/// reading the transport frame by frame; it never reaches a device.
fn positions() -> (Engine, Editor) {
    let probe = Arc::new(Probe::default());
    let engine =
        Engine::start_detached(config(SampleRate::HZ_48000), testing::registry(&probe)).unwrap();
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Position".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let device = Device::new(
        editor.new_id(),
        "",
        DeviceKind::Native {
            device_type: testing::POSITION.to_owned(),
        },
    );
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    engine.sync(editor.document());
    assert!(engine.flush(Duration::from_secs(5)));
    (engine, editor)
}

/// The first channel of `frames` frames, in blocks of up to 256, each
/// a real-time section under the allocation trap.
fn play(processor: &mut Processor, frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    let mut done = 0;
    while done < frames {
        let block = (frames - done).min(256);
        bl_rt::rt_test!({
            processor.process(block);
        });
        out.extend_from_slice(processor.output().channel(0));
        done += block;
    }
    out
}

/// Plays a loop of one beat from beat 2, set by a transport command
/// rather than the document so that only the carry keeps it, until the
/// song is mid-loop after a wrap; restarts at `rate`; and checks that
/// the song comes back stopped on the same beat in the same loop, and
/// that Continue plays on from there and wraps where the loop's end
/// falls at `rate`, frame by frame.
fn the_loop_and_the_playhead_survive_a_restart_at(rate: SampleRate) {
    let (mut engine, _editor) = positions();
    let mut first = restart(&engine, SampleRate::HZ_48000, None);
    let region = LoopRegion {
        start: Ticks::QUARTER,
        end: Ticks::from_quarters(2),
    };
    engine.transport(TransportCommand::SetLoop(Some(region)));
    engine.play();
    assert!(engine.flush(Duration::from_secs(5)));
    // At 48 kHz and 120 bpm the loop is frames 24,000 to 48,000: play
    // into it, round it once, and on to frame 34,000.
    let out = play(&mut first, 58_000);
    assert_eq!(out[47_999], 47_999.0);
    assert_eq!(out[48_000], 24_000.0, "the loop wraps");
    assert_eq!(first.position(), Frames(34_000));
    // 34,000 frames at 48 kHz and 120 bpm, in ticks: 29.4 a frame.
    let beat = Ticks(999_600);
    let clock = first.clock();
    let carry = first.park();
    assert_eq!(carry.position(), beat);
    drop(first);
    engine.restarter().detach();

    let mut restarted = restart(&engine, rate, Some(carry));
    // The song's tempo: 120 bpm throughout.
    let tempo = TempoMap::default();
    let at = tempo.ticks_to_frames(beat, rate);
    // Spec: device-changes R7
    assert!(!restarted.playing());
    assert_eq!(restarted.position(), at, "the same beat at {rate:?}");
    assert_eq!(restarted.loop_region(), Some(region));
    assert_eq!(restarted.insert_marker(), Ticks::ZERO);
    assert_eq!(restarted.clock(), clock, "the clock carries on");
    let state = engine.state();
    assert_eq!(state.sample_rate, rate);
    assert!(!state.playing);
    assert_eq!(state.position, at);
    assert_eq!(state.loop_region, Some(region));
    let frame = tempo.frames_to_ticks(Frames(1), rate).get();
    assert!((state.position_ticks.get() - beat.get()).abs() <= frame);

    // Continue plays on from there and wraps at the loop's end, on the
    // frames the loop falls on at the new rate.
    engine.transport(TransportCommand::Continue);
    assert!(engine.flush(Duration::from_secs(5)));
    let start = tempo.ticks_to_frames(region.start, rate).get();
    let end = tempo.ticks_to_frames(region.end, rate).get();
    let out = play(&mut restarted, (2 * end - start - at.get() + 100) as usize);
    let mut expected = at.get();
    for (i, &sample) in out.iter().enumerate() {
        assert_eq!(sample, expected as f32, "frame {i} after the restart");
        expected += 1;
        if expected == end {
            expected = start;
        }
    }
    assert!(engine.state().playing);
    assert_eq!(engine.state().loop_region, Some(region));
}

// Spec: device-changes R7; transport R3, R6, R7
#[test]
fn a_restart_while_stopped_mid_song_keeps_where_it_stopped_and_the_insert_marker() {
    let (mut engine, _editor) = positions();
    let mut first = restart(&engine, SampleRate::HZ_48000, None);
    // Play from bar 2 for 30,000 frames and stop: at 48 kHz and 120 bpm,
    // bar 2 is frame 96,000, so the song stops at 126,000. Then move the
    // insert marker to beat 2 while stopped.
    engine.locate(Ticks::WHOLE);
    engine.play();
    assert!(engine.flush(Duration::from_secs(5)));
    play(&mut first, 30_000);
    engine.stop();
    engine.locate(Ticks::QUARTER);
    assert!(engine.flush(Duration::from_secs(5)));
    play(&mut first, 256);
    assert!(!first.playing());
    assert_eq!(first.position(), Frames(126_000));
    assert_eq!(first.insert_marker(), Ticks::QUARTER);
    let carry = first.park();
    drop(first);
    engine.restarter().detach();

    let mut restarted = restart(&engine, SampleRate::HZ_44100, Some(carry));
    // 126,000 frames at 48 kHz is 115,762.5 at 44.1 kHz: the beat falls
    // between two frames, and the playhead takes the nearer.
    let tempo = TempoMap::default();
    let stopped_at = tempo.ticks_to_frames(
        tempo.frames_to_ticks(Frames(126_000), SampleRate::HZ_48000),
        SampleRate::HZ_44100,
    );
    assert!((stopped_at.get() - 115_762).abs() <= 1);
    assert!(!restarted.playing());
    assert_eq!(restarted.position(), stopped_at, "where playback stopped");
    assert_eq!(restarted.insert_marker(), Ticks::QUARTER);
    let state = engine.state();
    assert_eq!(state.position, stopped_at);
    assert_eq!(state.insert_marker, Ticks::QUARTER);

    // Continue resumes where playback stopped, at the new rate.
    engine.transport(TransportCommand::Continue);
    assert!(engine.flush(Duration::from_secs(5)));
    let out = play(&mut restarted, 256);
    assert_eq!(out[0], stopped_at.get() as f32);
    assert_eq!(out[255], (stopped_at.get() + 255) as f32);
    // Play starts at the insert marker: beat 2 is frame 22,050.
    engine.play();
    assert!(engine.flush(Duration::from_secs(5)));
    let out = play(&mut restarted, 256);
    assert_eq!(out[0], 22_050.0);
}

// Spec: device-changes R2, R7; transport R13
#[test]
fn a_device_change_during_looped_playback_keeps_the_loop_and_the_playhead() {
    the_loop_and_the_playhead_survive_a_restart_at(SampleRate::HZ_48000);
}

// Spec: device-changes R3, R7; transport R13, R15
#[test]
fn a_sample_rate_change_during_looped_playback_keeps_the_loop_and_the_playhead() {
    the_loop_and_the_playhead_survive_a_restart_at(SampleRate::HZ_44100);
}

// Spec: device-changes R7; transport R7, R16
#[test]
fn a_locate_and_timed_transport_commands_sent_while_detached_land_on_restart() {
    let (mut engine, mut editor) = positions();
    let mut first = restart(&engine, SampleRate::HZ_48000, None);
    engine.play();
    assert!(engine.flush(Duration::from_secs(5)));
    play(&mut first, 10_000);
    let carry = first.park();
    let clock = carry.clock();
    assert_eq!(clock, 10_000);
    let stopped_at = carry.position();
    drop(first);
    engine.restarter().detach();

    // While detached: a locate, at once.
    engine.locate(Ticks::from_quarters(8));
    // The set's loop switched on: a beat from beat 2.
    editor
        .apply(Command::SetLoop {
            start: Ticks::QUARTER,
            length: Ticks::QUARTER,
        })
        .unwrap();
    editor
        .apply(Command::SetLoopEnabled { enabled: true })
        .unwrap();
    engine.sync(editor.document());
    // The loop switched off again on a frame of the clock: 100 ms after
    // the stream closed.
    engine.transport_at(TransportCommand::SetLoop(None), When::Frame(clock + 4_800));
    // Nothing starts the song, at once or on a frame.
    engine.transport_at(TransportCommand::Play, When::Frame(clock + 100));
    engine.transport(TransportCommand::Continue);
    engine.play();
    assert!(engine.flush(Duration::from_secs(5)));

    let mut restarted = restart(&engine, SampleRate::HZ_44100, Some(carry));
    let tempo = TempoMap::default();
    assert!(!restarted.playing());
    assert_eq!(restarted.clock(), clock);
    // The locate moved the insert marker; the playhead stays where
    // playback stopped (transport R7).
    assert_eq!(restarted.insert_marker(), Ticks::from_quarters(8));
    assert_eq!(
        restarted.position(),
        tempo.ticks_to_frames(stopped_at, SampleRate::HZ_44100)
    );
    let set = LoopRegion {
        start: Ticks::QUARTER,
        end: Ticks::from_quarters(2),
    };
    assert_eq!(restarted.loop_region(), Some(set), "the set's loop");
    let state = engine.state();
    assert_eq!(state.insert_marker, Ticks::from_quarters(8));
    assert_eq!(state.loop_region, Some(set));

    // 100 ms after the restart at 44.1 kHz is 4,410 frames on: the loop
    // goes off on that frame, not before.
    let out = play(&mut restarted, 4_410);
    assert_eq!(restarted.loop_region(), Some(set));
    play(&mut restarted, 1);
    assert_eq!(restarted.loop_region(), None);
    assert!(!restarted.playing(), "nothing started the song");
    assert!(out.iter().all(|&sample| sample == -1.0));

    // Play starts at the insert marker.
    engine.play();
    assert!(engine.flush(Duration::from_secs(5)));
    let out = play(&mut restarted, 256);
    assert_eq!(out[0], (8 * 22_050) as f32);
}
