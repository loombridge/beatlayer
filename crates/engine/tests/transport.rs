//! The transport (WP-1.5): play, stop, continue, the insert marker, and
//! the Arrangement loop, each on its exact frame, following the tempo map,
//! as the transport spec specifies. A position probe on the only
//! track writes each frame's song position into the output, so a test reads
//! the transport sample by sample. Every block runs as a real-time section
//! under the allocation trap.

use crate::common;
use bl_commands::Registry;
use bl_core::time::{Bpm, Frames, TempoMap, Ticks};
use bl_engine::{
    Action, LoopRegion, ScheduledCommand, SessionCommand, TransportCommand, When, testing,
    transport,
};
use bl_model::{Command, LaunchQuantization, LaunchSettings, TrackKind};
use common::{RATE, Rig, device, track};
use serde_json::json;

const BAR: Ticks = Ticks::WHOLE;
const BEAT: Ticks = Ticks::QUARTER;

/// A rig with one track whose output is the song position of each frame,
/// or −1 while stopped.
fn rig(max_frames: usize, tempo: Option<TempoMap>) -> Rig {
    let mut rig = Rig::new(max_frames);
    if let Some(tempo) = tempo {
        rig.editor.apply(Command::SetTempo { tempo }).unwrap();
    }
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, a, testing::POSITION, &[]);
    rig.sync();
    rig
}

fn bpm(bpm: f64) -> Bpm {
    Bpm::new(bpm).unwrap()
}

fn frames(tempo: &TempoMap, at: Ticks) -> i64 {
    tempo.ticks_to_frames(at, RATE).get()
}

/// Checks that `out[range]` plays the song from `first` on, frame by frame.
fn plays_from(out: &[f32], range: std::ops::Range<usize>, first: i64) {
    for i in range.clone() {
        let expected = (first + (i - range.start) as i64) as f32;
        assert_eq!(
            out[i], expected,
            "frame {i} of {range:?}, playing from {first}"
        );
    }
}

fn stopped(out: &[f32], range: std::ops::Range<usize>) {
    assert!(
        out[range.clone()].iter().all(|&s| s == -1.0),
        "stopped through {range:?}"
    );
}

#[test]
fn play_starts_from_the_insert_marker_on_its_exact_frame_at_any_block_size() {
    let tempo = TempoMap::constant(bpm(97.0));
    // 2.2.3: a bar, a beat, and two sixteenths in.
    let marker = BAR + BEAT + Ticks::SIXTEENTH * 2;
    let first = frames(&tempo, marker);
    for block in [1, 100, 4096] {
        let mut rig = rig(4096, Some(tempo.clone()));
        rig.sender.transport(TransportCommand::Locate(marker));
        // On a frame no block of 100 or 4,096 starts on.
        rig.sender
            .transport_at(TransportCommand::Play, When::Frame(12_345));
        let out = rig.render(20_000, block);
        // Spec: transport R1, R2
        stopped(&out, 0..12_345);
        plays_from(&out, 12_345..20_000, first);
        let state = rig.monitor.state();
        assert!(state.playing);
        assert_eq!(state.insert_marker, marker);
        assert_eq!(state.position, Frames(first + 7_655));
    }
}

#[test]
fn play_starts_from_the_song_start_time_and_play_from_marker_ignores_it() {
    let tempo = TempoMap::constant(bpm(97.0));
    // 4.2.3: three bars, a beat, and two sixteenths in.
    let start = BAR * 3 + BEAT + Ticks::SIXTEENTH * 2;
    let first = frames(&tempo, start);
    // 2.2.1: the insert marker, where Play From Insert Marker plays from.
    let marker = BAR + BEAT;
    for block in [1, 100, 4096] {
        let mut rig = rig(4096, Some(tempo.clone()));
        rig.sender.transport(TransportCommand::Locate(marker));
        rig.sender
            .transport(TransportCommand::SetSongStart(Some(start)));
        rig.sender
            .transport_at(TransportCommand::Play, When::Frame(12_345));
        let out = rig.render(20_000, block);
        // Spec: transport R21. Play starts from the song start time, and
        // the insert marker stays where it was.
        stopped(&out, 0..12_345);
        plays_from(&out, 12_345..20_000, first);
        let state = rig.monitor.state();
        assert!(state.playing);
        assert_eq!(state.insert_marker, marker);
    }
    // Play From Insert Marker plays from the marker whatever the song start
    // time is (the transport spec, R10 and R21).
    for block in [1, 4096] {
        let mut rig = rig(4096, Some(tempo.clone()));
        rig.sender.transport(TransportCommand::Locate(marker));
        rig.sender
            .transport(TransportCommand::SetSongStart(Some(start)));
        rig.sender
            .transport_at(TransportCommand::PlayFromInsertMarker, When::Frame(12_345));
        let out = rig.render(20_000, block);
        stopped(&out, 0..12_345);
        plays_from(&out, 12_345..20_000, frames(&tempo, marker));
    }
}

#[test]
fn a_stop_abandons_a_start_waiting_for_its_frame() {
    // Spec: control-bar-transport R20. A start sent for a later frame, as a
    // tap start is, does not happen once a Stop has come before the frame.
    for block in [1, 64, 512] {
        let mut rig = rig(512, None);
        rig.sender
            .transport_at(TransportCommand::Play, When::Frame(1_000));
        let out = rig.render(600, block);
        stopped(&out, 0..600);
        rig.sender.transport(TransportCommand::Stop);
        let out = rig.render(1_000, block);
        stopped(&out, 0..1_000);
        assert!(!rig.monitor.state().playing);
    }
}

#[test]
fn stop_keeps_where_playback_stopped_and_continue_resumes_there() {
    let mut rig = rig(256, None);
    let bar_2 = frames(&TempoMap::default(), BAR);
    rig.sender.transport(TransportCommand::Locate(BAR));
    for (frame, command) in [
        (0, TransportCommand::Play),
        (10_000, TransportCommand::Stop),
        // Moving the insert marker while stopped leaves the resume point.
        (15_000, TransportCommand::Locate(Ticks::ZERO)),
        (20_000, TransportCommand::Continue),
        // Continue while playing does nothing.
        (25_000, TransportCommand::Continue),
    ] {
        rig.sender.transport_at(command, When::Frame(frame));
    }
    let out = rig.render(30_000, 256);
    // Spec: transport R3
    plays_from(&out, 0..10_000, bar_2);
    stopped(&out, 10_000..20_000);
    // Spec: transport R6, R7
    plays_from(&out, 20_000..30_000, bar_2 + 10_000);
    assert_eq!(rig.monitor.state().insert_marker, Ticks::ZERO);
}

#[test]
fn stop_while_stopped_returns_to_the_start() {
    let mut rig = rig(256, None);
    rig.sender.transport(TransportCommand::Locate(BAR * 2));
    for (frame, command) in [
        (0, TransportCommand::Play),
        (5_000, TransportCommand::Stop),
        (6_000, TransportCommand::Stop),
        (7_000, TransportCommand::Continue),
    ] {
        rig.sender.transport_at(command, When::Frame(frame));
    }
    rig.render(6_500, 256);
    // Spec: transport R4
    let state = rig.monitor.state();
    assert!(!state.playing);
    assert_eq!(state.insert_marker, Ticks::ZERO);
    assert_eq!(state.position, Frames::ZERO);
    let out = rig.render(3_500, 256);
    stopped(&out, 0..500);
    plays_from(&out, 500..3_500, 0);
}

#[test]
fn play_stop_toggles_and_never_returns_to_the_start() {
    let mut rig = rig(256, None);
    let bar_2 = frames(&TempoMap::default(), BAR);
    rig.sender.transport(TransportCommand::Locate(BAR));
    for frame in [0, 5_000, 6_000, 9_000, 9_500] {
        rig.sender
            .transport_at(TransportCommand::PlayStop, When::Frame(frame));
    }
    let out = rig.render(10_000, 256);
    // Spec: transport R5
    plays_from(&out, 0..5_000, bar_2);
    stopped(&out, 5_000..6_000);
    plays_from(&out, 6_000..9_000, bar_2);
    stopped(&out, 9_000..9_500);
    plays_from(&out, 9_500..10_000, bar_2);
    assert_eq!(rig.monitor.state().insert_marker, BAR);
}

#[test]
fn the_insert_marker_moves_and_playback_jumps_only_while_playing() {
    let mut rig = rig(256, None);
    let tempo = TempoMap::default();
    for (frame, command) in [
        (0, TransportCommand::Play),
        // Spec: transport R7
        (4_000, TransportCommand::Locate(BAR * 3)),
        // Spec: transport R12
        (6_000, TransportCommand::Locate(-BEAT)),
        (8_000, TransportCommand::Locate(BEAT)),
        // Spec: transport R11
        (9_000, TransportCommand::Play),
    ] {
        rig.sender.transport_at(command, When::Frame(frame));
    }
    let out = rig.render(10_000, 256);
    plays_from(&out, 0..4_000, 0);
    plays_from(&out, 4_000..6_000, frames(&tempo, BAR * 3));
    plays_from(&out, 6_000..8_000, 0);
    plays_from(&out, 8_000..9_000, frames(&tempo, BEAT));
    plays_from(&out, 9_000..10_000, frames(&tempo, BEAT));
    assert_eq!(rig.monitor.state().insert_marker, BEAT);
}

#[test]
fn the_insert_marker_moves_to_the_playhead() {
    let mut rig = rig(256, None);
    for (frame, command) in [
        (0, TransportCommand::Play),
        (30_000, TransportCommand::InsertMarkerToPlayhead),
        (31_000, TransportCommand::Stop),
        (32_000, TransportCommand::Play),
    ] {
        rig.sender.transport_at(command, When::Frame(frame));
    }
    let out = rig.render(33_000, 256);
    // Spec: transport R9
    let marker = TempoMap::default().frames_to_ticks(Frames(30_000), RATE);
    assert_eq!(rig.monitor.state().insert_marker, marker);
    plays_from(&out, 32_000..33_000, 30_000);
}

#[test]
fn the_insert_marker_moves_alone_and_playback_carries_on() {
    // A click in the Arrangement (WP-1.18): the insert marker moves, the
    // playhead doesn't, while playing and while stopped.
    let mut rig = rig(256, None);
    let tempo = TempoMap::default();
    for (frame, command) in [
        (0, TransportCommand::Play),
        // Spec: control-bar-transport R15
        (4_000, TransportCommand::SetInsertMarker(BAR * 2)),
        (5_000, TransportCommand::SetInsertMarker(-BEAT)),
        (6_000, TransportCommand::SetInsertMarker(BAR * 3)),
        (7_000, TransportCommand::Stop),
        (8_000, TransportCommand::SetInsertMarker(BAR)),
        (9_000, TransportCommand::Continue),
        (10_000, TransportCommand::Stop),
        (11_000, TransportCommand::Play),
    ] {
        rig.sender.transport_at(command, When::Frame(frame));
    }
    let out = rig.render(12_000, 256);
    plays_from(&out, 0..7_000, 0);
    stopped(&out, 7_000..9_000);
    // Continue resumes where playback stopped, not at the moved marker.
    plays_from(&out, 9_000..10_000, 7_000);
    // Play starts from the marker.
    plays_from(&out, 11_000..12_000, frames(&tempo, BAR));
    assert_eq!(rig.monitor.state().insert_marker, BAR);
    // A marker before 1.1.1 is taken as 1.1.1.
    let mut rig = rig_with_marker(-BEAT);
    assert_eq!(rig.monitor.state().insert_marker, Ticks::ZERO);
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(100, 100);
    plays_from(&out, 0..100, 0);
}

fn rig_with_marker(at: Ticks) -> Rig {
    let mut rig = rig(256, None);
    rig.sender.transport(TransportCommand::SetInsertMarker(at));
    rig.render(256, 256);
    rig
}

/// The frame of the engine clock `clock` plays this song position, for a
/// song that plays from frame 0 into a loop from `start` to `end` frames.
fn looped(clock: i64, start: i64, end: i64) -> i64 {
    if clock < end {
        clock
    } else {
        start + (clock - end) % (end - start)
    }
}

fn set_loop(rig: &mut Rig, start: Ticks, length: Ticks) {
    rig.editor
        .apply(Command::SetLoop { start, length })
        .unwrap();
    rig.editor
        .apply(Command::SetLoopEnabled { enabled: true })
        .unwrap();
    let synced = rig.sync();
    assert!(!synced.program, "a loop edit compiles nothing");
}

#[test]
fn the_loop_wraps_on_its_exact_frame_again_and_again_at_any_block_size() {
    let tempo = TempoMap::constant(bpm(130.0));
    // From 1.2.3 to 1.4.1.
    let (start, end) = (BEAT * 3 / 2, BEAT * 3);
    let (first, last) = (frames(&tempo, start), frames(&tempo, end));
    assert_eq!((first, last), (33_231, 66_462));
    for block in [1, 100, 4096] {
        let mut rig = rig(4096, Some(tempo.clone()));
        set_loop(&mut rig, start, end - start);
        rig.sender.transport(TransportCommand::Play);
        let out = rig.render(200_000, block);
        // Spec: transport R13, R14 (from before the loop, into it)
        for (clock, &sample) in out.iter().enumerate() {
            assert_eq!(
                sample,
                looped(clock as i64, first, last) as f32,
                "clock {clock} in blocks of {block}"
            );
        }
        let wraps = out.windows(2).filter(|w| w[1] < w[0]).count();
        assert_eq!(wraps, 5, "{block}");
        let state = rig.monitor.state();
        assert_eq!(state.loop_region, Some(LoopRegion { start, end }));
        assert_eq!(state.position, Frames(looped(200_000, first, last)));
    }
}

#[test]
fn playback_at_or_after_the_loops_end_plays_on() {
    let mut rig = rig(256, None);
    let tempo = TempoMap::default();
    set_loop(&mut rig, BEAT, BEAT);
    // Spec: transport R14
    rig.sender.transport(TransportCommand::Locate(BEAT * 2));
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(60_000, 256);
    plays_from(&out, 0..60_000, frames(&tempo, BEAT * 2));

    // Inside the loop it wraps, until the end moves before the playhead.
    rig.sender.transport(TransportCommand::Locate(BEAT));
    let out = rig.render(30_000, 256);
    plays_from(&out, 0..24_000, 24_000);
    plays_from(&out, 24_000..30_000, 24_000);
    rig.editor
        .apply(Command::SetLoop {
            start: Ticks::ZERO,
            length: BEAT / 4,
        })
        .unwrap();
    rig.sync();
    let out = rig.render(30_000, 256);
    plays_from(&out, 0..30_000, 30_000);

    // Switched off, it plays straight through.
    rig.sender.transport(TransportCommand::Locate(Ticks::ZERO));
    rig.editor
        .apply(Command::SetLoopEnabled { enabled: false })
        .unwrap();
    rig.sync();
    let out = rig.render(10_000, 256);
    plays_from(&out, 0..10_000, 0);
    assert_eq!(rig.monitor.state().loop_region, None);
}

#[test]
fn the_loop_follows_the_tempo_map_and_a_ramp_across_its_end() {
    // 90 BPM for a bar, then a ramp to 150 over two bars.
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, bpm(90.0)),
        (BAR, bpm(90.0)),
        (BAR * 3, bpm(150.0)),
    ])
    .unwrap();
    // From 2.2.1 to 2.3.3: inside the ramp, which runs on past the end.
    let (start, end) = (BAR + BEAT, BAR + BEAT * 5 / 2);
    let (first, last) = (frames(&tempo, start), frames(&tempo, end));
    let mut rig = rig(512, Some(tempo.clone()));
    set_loop(&mut rig, start, end - start);
    rig.sender.transport(TransportCommand::Locate(BAR));
    rig.sender.transport(TransportCommand::Play);
    let bar_2 = frames(&tempo, BAR);
    let out = rig.render(150_000, 512);
    // Spec: transport R15
    for (clock, &sample) in out.iter().enumerate() {
        let song = looped(bar_2 + clock as i64, first, last);
        assert_eq!(sample, song as f32, "clock {clock}");
    }
    assert!(out.windows(2).filter(|w| w[1] < w[0]).count() >= 2);

    // A new tempo map moves the loop's frames with its bars, and the
    // playhead keeps its beat.
    let before = rig.monitor.state().position_ticks;
    rig.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(60.0)),
    });
    rig.block(1);
    let slow = TempoMap::constant(bpm(60.0));
    let position = rig.monitor.state().position;
    assert_eq!(
        position - Frames(1),
        slow.ticks_to_frames(before, RATE),
        "the playhead kept its beat"
    );
    let until_end = (frames(&slow, end) - position.get()) as usize;
    let out = rig.render(until_end + 10, 512);
    plays_from(&out, 0..until_end, position.get());
    plays_from(&out, until_end..until_end + 10, frames(&slow, start));
}

#[test]
fn the_documents_loop_reaches_the_engine_without_a_compile_and_undoes() {
    let mut rig = rig(256, None);
    let synced = rig.apply(Command::SetLoop {
        start: BAR,
        length: BAR * 2,
    });
    assert!(!synced.program);
    rig.block(1);
    // Spec: transport R16
    assert_eq!(rig.monitor.state().loop_region, None, "the switch is off");
    rig.apply(Command::SetLoopEnabled { enabled: true });
    rig.block(1);
    let region = LoopRegion {
        start: BAR,
        end: BAR * 3,
    };
    assert_eq!(rig.monitor.state().loop_region, Some(region));
    rig.editor.undo().unwrap();
    rig.sync();
    rig.block(1);
    assert_eq!(rig.monitor.state().loop_region, None);
    rig.editor.redo().unwrap();
    rig.sync();
    rig.block(1);
    assert_eq!(rig.monitor.state().loop_region, Some(region));
    // A compile for another reason keeps the loop.
    let b = track(&mut rig.editor, TrackKind::Audio, None);
    assert!(rig.sync().program);
    rig.block(1);
    assert_eq!(rig.monitor.state().loop_region, Some(region));
    assert!(rig.editor.document().track(b).is_some());
}

#[test]
fn scheduled_transport_commands_act_on_their_song_frames() {
    let mut rig = rig(256, None);
    let tempo = TempoMap::default();
    let schedule = |id, at, command| ScheduledCommand {
        id,
        at,
        action: Action::Transport(command),
    };
    rig.sender
        .schedule(schedule(1, BEAT, TransportCommand::Play));
    rig.sender
        .schedule(schedule(2, BEAT * 2, TransportCommand::Locate(BAR * 2)));
    rig.sender
        .schedule(schedule(3, BAR * 2 + BEAT, TransportCommand::PlayStop));
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(100_000, 256);
    // A scheduled Play doesn't start the song again at beat 2.
    plays_from(&out, 0..48_000, 0);
    plays_from(&out, 48_000..72_000, frames(&tempo, BAR * 2));
    stopped(&out, 72_000..100_000);
    assert_eq!(rig.monitor.reports().len(), 3);
}

#[test]
fn transport_commands_for_a_frame_are_bounded_and_counted() {
    let mut rig = rig(256, None);
    let capacity = rig.processor.config().max_scheduled as u64;
    for frame in 0..=capacity {
        rig.sender
            .transport_at(TransportCommand::Continue, When::Frame(1_000 + frame));
    }
    rig.render(512, 256);
    assert_eq!(rig.monitor.state().counters.transport_overflowed, 1);
    let out = rig.render(2_000, 256);
    stopped(&out, 0..488);
    plays_from(&out, 488..2_000, 0);
}

/// What the UI draws its playhead from is what the engine played: after
/// every block, the published state's playhead is the next frame the block
/// would have played, in frames and in ticks, through starts, a loop, a
/// stop, and Continue.
#[test]
fn the_published_playhead_follows_every_block() {
    let tempo = TempoMap::constant(bpm(123.0));
    let mut rig = rig(333, Some(tempo.clone()));
    let (start, end) = (BEAT, BEAT * 3);
    set_loop(&mut rig, start, end - start);
    rig.sender.transport(TransportCommand::Locate(BEAT / 2));
    rig.sender
        .transport_at(TransportCommand::Play, When::Frame(1_000));
    rig.sender
        .transport_at(TransportCommand::Stop, When::Frame(100_000));
    rig.sender
        .transport_at(TransportCommand::Continue, When::Frame(110_000));
    // The spec's rules, frame by frame, apart from the engine's code: the
    // playhead after each frame of the engine clock.
    let (first, last) = (frames(&tempo, start), frames(&tempo, end));
    let mut model = Vec::with_capacity(150_000);
    let (mut playing, mut position) = (false, 0i64);
    for clock in 0..150_000u64 {
        match clock {
            1_000 => (playing, position) = (true, frames(&tempo, BEAT / 2)),
            100_000 => playing = false,
            110_000 => playing = true,
            _ => {}
        }
        if playing {
            position += 1;
            if position == last {
                position = first;
            }
        }
        model.push((playing, position));
    }

    for block in 1..=450u64 {
        rig.block(333);
        let state = rig.monitor.state().clone();
        let clock = block * 333;
        let (playing, position) = model[clock as usize - 1];
        // Spec: transport R20
        assert_eq!(state.clock, clock);
        assert_eq!((state.playing, state.position), (playing, Frames(position)));
        assert_eq!(
            state.position_ticks,
            tempo.frames_to_ticks(state.position, RATE)
        );
        assert_eq!(state.insert_marker, BEAT / 2);
        assert_eq!(state.loop_region, Some(LoopRegion { start, end }));
        // The playhead is the frame after the last one the block played, or
        // the loop's start after the loop's last frame.
        let played = *rig.last().last().unwrap();
        if playing && played as i64 + 1 != position {
            assert_eq!((played as i64 + 1, position), (last, first), "a wrap");
        }
    }
    assert!(model.windows(2).filter(|w| w[1].1 < w[0].1).count() >= 2);
}

/// Every transport command runs by its ID with JSON arguments that fit its
/// schema, as an agent runs it (D17), and does what the spec says.
#[test]
fn every_transport_command_runs_by_id() {
    let mut commands = Registry::new();
    transport::register(&mut commands).unwrap();
    bl_model::registry::register(&mut commands).unwrap();
    assert_eq!(commands.problems(), Vec::new());

    let mut rig = rig(256, None);
    let tempo = TempoMap::default();
    let mut ran = Vec::new();
    let mut run = |rig: &mut Rig, id: &str, args: serde_json::Value| {
        let spec = commands.get(id).unwrap();
        bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
        if transport::handles(id) {
            let command = transport::command_from_args(id, args).unwrap();
            rig.sender.transport(command);
        } else {
            bl_model::registry::run(&mut rig.editor, id, args).unwrap();
            rig.sync();
        }
        ran.push(id.to_owned());
    };

    // Spec: transport R19
    run(
        &mut rig,
        "transport.locate",
        json!({ "position": BAR.get() }),
    );
    run(&mut rig, "transport.play", json!({}));
    let out = rig.render(1_000, 256);
    plays_from(&out, 0..1_000, frames(&tempo, BAR));
    run(&mut rig, "transport.stop", json!({}));
    let out = rig.render(1_000, 256);
    stopped(&out, 0..1_000);
    run(&mut rig, "transport.continue", json!({}));
    let out = rig.render(1_000, 256);
    plays_from(&out, 0..1_000, frames(&tempo, BAR) + 1_000);
    run(&mut rig, "transport.insert_marker_to_playhead", json!({}));
    run(&mut rig, "transport.play_stop", json!({}));
    rig.render(256, 256);
    assert!(!rig.monitor.state().playing);
    run(&mut rig, "transport.play_from_insert_marker", json!({}));
    let out = rig.render(1_000, 256);
    plays_from(&out, 0..1_000, frames(&tempo, BAR) + 2_000);
    run(
        &mut rig,
        "transport.set_loop",
        json!({ "start": 0, "length": BEAT.get() }),
    );
    run(
        &mut rig,
        "transport.set_loop_enabled",
        json!({ "enabled": true }),
    );
    run(
        &mut rig,
        "transport.locate",
        json!({ "position": BAR.get() }),
    );
    // Spec: transport R8; control-bar-transport R3
    run(&mut rig, "transport.return_to_start", json!({}));
    let out = rig.render(48_000, 256);
    for (clock, &sample) in out.iter().enumerate() {
        assert_eq!(sample, (clock % 24_000) as f32);
    }

    // Metronome & recording (WP-2.15).
    run(
        &mut rig,
        "transport.set_metronome_enabled",
        json!({ "enabled": true }),
    );
    run(
        &mut rig,
        "transport.set_metronome_count_in",
        json!({ "count_in": "1_bar" }),
    );
    run(
        &mut rig,
        "transport.set_metronome_sound",
        json!({ "sound": "wood" }),
    );
    run(
        &mut rig,
        "transport.set_metronome_rhythm",
        json!({ "rhythm": "1/8" }),
    );
    run(
        &mut rig,
        "transport.set_metronome_only_while_recording",
        json!({ "only_while_recording": true }),
    );
    // Punch-In and Punch-Out (WP-2.13).
    for id in ["transport.set_punch_in", "transport.set_punch_out"] {
        run(&mut rig, id, json!({ "enabled": true }));
    }
    run(
        &mut rig,
        "transport.set_midi_overdub",
        json!({ "enabled": true }),
    );
    run(
        &mut rig,
        "transport.set_record_quantization",
        json!({ "quantization": "1/16" }),
    );
    run(
        &mut rig,
        "transport.set_recording",
        json!({ "recording": true }),
    );
    run(&mut rig, "transport.stop_recording", json!({}));
    // Spec: transport R23
    run(&mut rig, "transport.scrub", json!({ "position": 0 }));
    // Spec: midi-clock R11. With no output port on, it changes nothing.
    run(&mut rig, "transport.resync_external_hardware", json!({}));

    let mut registered: Vec<String> = commands
        .iter()
        .map(|spec| spec.id.to_string())
        .filter(|id| id.starts_with("transport."))
        .collect();
    registered.sort();
    ran.sort();
    ran.dedup();
    assert_eq!(ran, registered, "every transport command is run here");
}

/// The bindings follow the spec: the keys Live gives these commands.
#[test]
fn the_bindings_follow_the_spec() {
    let mut commands = Registry::new();
    transport::register(&mut commands).unwrap();
    bl_model::registry::register(&mut commands).unwrap();
    let bindings = |id: &str| -> Vec<String> {
        commands
            .get(id)
            .unwrap()
            .bindings
            .iter()
            .map(|binding| {
                let mut text = binding.shortcut.to_string();
                for (name, value) in &binding.args {
                    text.push_str(&format!(" {name}={value}"));
                }
                if let Some(arg) = &binding.toggles {
                    text.push_str(&format!(" toggles {arg}"));
                }
                text
            })
            .collect()
    };
    // Spec: transport R30. F9 switches recording.
    assert_eq!(
        bindings("transport.set_recording"),
        ["S-TRANS-006 toggles recording"]
    );
    // Spec: transport R5, R6, R8, R9, R10
    assert_eq!(bindings("transport.play_stop"), ["S-TRANS-001"]);
    assert_eq!(bindings("transport.continue"), ["S-TRANS-002"]);
    assert_eq!(bindings("transport.locate"), ["S-TRANS-005 position=0"]);
    assert_eq!(
        bindings("transport.insert_marker_to_playhead"),
        ["S-ARR-033"]
    );
    assert_eq!(bindings("transport.play_from_insert_marker"), ["S-ARR-032"]);
    assert_eq!(
        bindings("transport.set_metronome_enabled"),
        ["S-TRANS-011 toggles enabled"]
    );
    for id in [
        "transport.play",
        "transport.stop",
        // Return Play Position to 1.1.1 shows no key (M-PLAY-004); Home
        // runs `transport.locate` at 1.1.1.
        "transport.return_to_start",
        "transport.set_loop",
        "transport.set_loop_enabled",
        "transport.stop_recording",
        "transport.set_metronome_count_in",
        "transport.set_metronome_sound",
        "transport.set_metronome_rhythm",
        "transport.set_metronome_only_while_recording",
    ] {
        assert_eq!(bindings(id), Vec::<String>::new(), "{id}");
    }
}

/// A rig with the global launch quantisation set.
fn rig_with_quantization(
    max_frames: usize,
    tempo: TempoMap,
    quantization: LaunchQuantization,
) -> Rig {
    let mut rig = rig(max_frames, Some(tempo));
    rig.sender
        .session(SessionCommand::SetQuantization(LaunchSettings {
            quantization,
            enabled: true,
            ..LaunchSettings::default()
        }));
    rig
}

/// The frame the output jumps at, and the position it jumps to.
fn jump(out: &[f32]) -> (usize, i64) {
    for i in 1..out.len() {
        if out[i] != out[i - 1] + 1.0 {
            return (i, out[i] as i64);
        }
    }
    panic!("the output never jumps");
}

#[test]
fn a_quantised_locate_waits_for_the_next_line_and_jumps_there() {
    let tempo = TempoMap::constant(bpm(120.0));
    let bar = frames(&tempo, BAR);
    let quarter = frames(&tempo, BEAT);
    for block in [1, 64, 512, 4096] {
        for (quantization, line) in [
            (LaunchQuantization::Bar, bar),
            (LaunchQuantization::Quarter, quarter),
        ] {
            let mut rig = rig_with_quantization(4096, tempo.clone(), quantization);
            rig.sender.transport(TransportCommand::Play);
            // Play into the middle of the song, so the next line is ahead.
            let mut out: Vec<f32> = Vec::new();
            while out.len() < (bar / 2) as usize {
                rig.block(block);
                out.extend_from_slice(&rig.last());
            }
            let target = BAR * 5;
            rig.sender
                .transport(TransportCommand::QuantizedLocate(target));
            while out.len() < (bar / 2 + bar * 2) as usize {
                rig.block(block);
                out.extend_from_slice(&rig.last());
            }
            // The jump lands on the next line of the global launch
            // quantisation, exactly, and goes to the target.
            let (at, to) = jump(&out);
            assert_eq!(to, frames(&tempo, target), "block {block}");
            let landed = out[at - 1] as i64 + 1;
            assert_eq!(landed % line, 0, "block {block}: landed at {landed}");
            assert!(landed > bar / 2, "block {block}: the jump was early");
        }
    }
}

#[test]
fn a_second_quantised_locate_before_the_line_replaces_the_first() {
    let tempo = TempoMap::constant(bpm(120.0));
    let bar = frames(&tempo, BAR);
    let mut rig = rig_with_quantization(4096, tempo.clone(), LaunchQuantization::Bar);
    rig.sender.transport(TransportCommand::Play);
    let mut out: Vec<f32> = Vec::new();
    while out.len() < (bar / 2) as usize {
        rig.block(64);
        out.extend_from_slice(&rig.last());
    }
    rig.sender
        .transport(TransportCommand::QuantizedLocate(BAR * 5));
    rig.sender
        .transport(TransportCommand::QuantizedLocate(BAR * 7));
    while out.len() < (bar / 2 + bar * 2) as usize {
        rig.block(64);
        out.extend_from_slice(&rig.last());
    }
    let (_, to) = jump(&out);
    assert_eq!(to, frames(&tempo, BAR * 7));
}

#[test]
fn a_quantised_locate_with_no_quantisation_jumps_at_once() {
    let tempo = TempoMap::constant(bpm(120.0));
    let bar = frames(&tempo, BAR);
    let mut rig = rig_with_quantization(4096, tempo.clone(), LaunchQuantization::None);
    rig.sender.transport(TransportCommand::Play);
    let mut out: Vec<f32> = Vec::new();
    while out.len() < (bar / 2) as usize {
        rig.block(64);
        out.extend_from_slice(&rig.last());
    }
    let target = BAR * 5;
    rig.sender
        .transport(TransportCommand::QuantizedLocate(target));
    while out.len() < (bar / 2 + 4096) as usize {
        rig.block(64);
        out.extend_from_slice(&rig.last());
    }
    // The jump happens at once, not on a line.
    let (at, to) = jump(&out);
    assert_eq!(to, frames(&tempo, target));
    assert!((out[at - 1] as i64 + 1) % bar != 0, "it waited for a line");
}

#[test]
fn a_quantised_locate_while_stopped_locates_at_once() {
    let tempo = TempoMap::constant(bpm(120.0));
    let mut rig = rig_with_quantization(4096, tempo.clone(), LaunchQuantization::Bar);
    let target = BAR * 5;
    rig.sender
        .transport(TransportCommand::QuantizedLocate(target));
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(4096, 64);
    plays_from(&out, 0..1024, frames(&tempo, target));
}

#[test]
fn a_stop_abandons_a_jump_waiting_for_a_line() {
    // Spec: transport R22. A jump waiting for a line is abandoned when the
    // transport stops, so the song doesn't jump on its own later.
    let tempo = TempoMap::constant(bpm(120.0));
    let bar = frames(&tempo, BAR);
    let mut rig = rig_with_quantization(4096, tempo.clone(), LaunchQuantization::Bar);
    rig.sender.transport(TransportCommand::Play);
    let mut out: Vec<f32> = Vec::new();
    while out.len() < (bar * 2) as usize {
        rig.block(64);
        out.extend_from_slice(&rig.last());
    }
    // A jump back to the first bar, waiting for the next line.
    rig.sender
        .transport(TransportCommand::QuantizedLocate(Ticks::ZERO));
    rig.sender.transport(TransportCommand::Stop);
    rig.sender.transport(TransportCommand::Play);
    let mut after: Vec<f32> = Vec::new();
    while after.len() < (bar * 4) as usize {
        rig.block(64);
        after.extend_from_slice(&rig.last());
    }
    // The song plays on from the insert marker, which the jump set to the
    // first bar, and doesn't jump again on the line it was waiting for.
    plays_from(&after, 0..after.len(), frames(&tempo, Ticks::ZERO));
}

#[test]
fn a_locate_replaces_a_jump_waiting_for_a_line() {
    // Spec: transport R22. A plain locate replaces a jump waiting for a
    // line, so the song doesn't jump again on the line.
    let tempo = TempoMap::constant(bpm(120.0));
    let bar = frames(&tempo, BAR);
    let mut rig = rig_with_quantization(4096, tempo.clone(), LaunchQuantization::Bar);
    rig.sender.transport(TransportCommand::Play);
    let mut out: Vec<f32> = Vec::new();
    while out.len() < (bar * 2) as usize {
        rig.block(64);
        out.extend_from_slice(&rig.last());
    }
    // A jump to the fifth bar, waiting for the next line.
    rig.sender
        .transport(TransportCommand::QuantizedLocate(BAR * 5));
    // A plain locate goes to the fourth bar at once and replaces it.
    rig.sender.transport(TransportCommand::Locate(BAR * 4));
    let mut after: Vec<f32> = Vec::new();
    while after.len() < (bar * 2) as usize {
        rig.block(64);
        after.extend_from_slice(&rig.last());
    }
    plays_from(&after, 0..after.len(), frames(&tempo, BAR * 4));
}

/// Where the song plays `count` frames on from `from`, with the Arrangement
/// loop from `start` to `end` frames, if it is on.
fn runs_on(from: i64, count: usize, arrangement_loop: Option<(i64, i64)>) -> Vec<f32> {
    let mut at = from;
    (0..count)
        .map(|_| {
            let now = at as f32;
            at += 1;
            if let Some((start, end)) = arrangement_loop
                && at == end
            {
                at = start;
            }
            now
        })
        .collect()
}

#[test]
fn a_held_scrub_loops_its_span_on_exact_frames_until_released_then_playback_goes_on() {
    // Spec: transport R23; time-editing R25
    let tempo = TempoMap::constant(bpm(120.0));
    let bar = frames(&tempo, BAR);
    // The span: bar 3, a bar long, from the line at or before 3.2.1.
    let (span_start, span_end) = (BAR * 2, BAR * 3);
    let (first, last) = (frames(&tempo, span_start), frames(&tempo, span_end));
    // An Arrangement loop over the span's second half and beyond, which the
    // held loop must ignore while it's held.
    let brace = (BAR * 2 + BAR / 2, BAR);
    let brace_frames = (frames(&tempo, brace.0), frames(&tempo, brace.0 + brace.1));
    for block in [1, 64, 512, 4096] {
        for arrangement_loop in [None, Some(brace_frames)] {
            let mut rig = rig_with_quantization(4096, tempo.clone(), LaunchQuantization::Bar);
            if arrangement_loop.is_some() {
                set_loop(&mut rig, brace.0, brace.1);
            }
            rig.sender.transport(TransportCommand::Play);
            let mut out = rig.render((bar / 2) as usize, block);
            rig.sender.transport(TransportCommand::Scrub {
                point: BAR * 2 + BEAT,
                held: true,
            });
            // The jump waits for the next bar, then the span repeats.
            out.extend(rig.render((bar * 3) as usize, block));
            let (at, to) = jump(&out);
            assert_eq!(to, first, "block {block}");
            assert_eq!(
                out[at - 1] as i64 + 1,
                bar,
                "block {block}: the next bar line"
            );
            for (i, &sample) in out[at..].iter().enumerate() {
                assert_eq!(
                    sample as i64,
                    first + (i as i64) % (last - first),
                    "block {block}: frame {i} of the held loop"
                );
            }
            // Released a third of the way into a pass: playback carries on
            // from there, past the span's end.
            let left = out.len();
            let release = (bar * 2 / 3) as usize - (left - at) % bar as usize;
            out.extend(rig.render(release, block));
            rig.sender.transport(TransportCommand::Scrub {
                point: BAR * 2 + BEAT,
                held: false,
            });
            let held_to = *out.last().unwrap() as i64;
            let after = rig.render((bar * 2) as usize, block);
            let expected = runs_on(held_to + 1, after.len(), arrangement_loop);
            assert_eq!(after, expected, "block {block}, loop {arrangement_loop:?}");
            assert!(rig.monitor.state().playing);
        }
    }
}

#[test]
fn a_held_scrub_while_stopped_starts_playback_at_its_start() {
    // Spec: transport R23; time-editing R25
    let tempo = TempoMap::constant(bpm(120.0));
    // A quarter note long, from the line at or before the point.
    let (first, last) = (frames(&tempo, BAR + BEAT), frames(&tempo, BAR + BEAT * 2));
    for block in [1, 64, 512, 4096] {
        let mut rig = rig_with_quantization(4096, tempo.clone(), LaunchQuantization::Quarter);
        rig.sender.transport(TransportCommand::Scrub {
            point: BAR + BEAT * 3 / 2,
            held: true,
        });
        let out = rig.render(((last - first) * 3 + 100) as usize, block);
        for (i, &sample) in out.iter().enumerate() {
            assert_eq!(
                sample as i64,
                first + (i as i64) % (last - first),
                "block {block}"
            );
        }
        rig.sender.transport(TransportCommand::Scrub {
            point: BAR + BEAT * 3 / 2,
            held: false,
        });
        let after = rig.render(2_000, block);
        let from = out.len() as i64 % (last - first) + first;
        plays_from(&after, 0..2_000, from);
    }
}

#[test]
fn a_scrub_with_none_held_plays_from_the_point() {
    // Spec: transport R23; time-editing R28
    let tempo = TempoMap::constant(bpm(120.0));
    let bar = frames(&tempo, BAR);
    let mut rig = rig_with_quantization(4096, tempo.clone(), LaunchQuantization::Bar);
    rig.sender.transport(TransportCommand::Scrub {
        point: BAR * 5,
        held: false,
    });
    let out = rig.render(1_000, 64);
    plays_from(&out, 0..1_000, frames(&tempo, BAR * 5));
    // While the song plays, the jump waits for the next bar line.
    rig.sender.transport(TransportCommand::Scrub {
        point: BAR * 8,
        held: false,
    });
    let out = rig.render((bar * 2) as usize, 64);
    let (at, to) = jump(&out);
    assert_eq!(to, frames(&tempo, BAR * 8));
    assert_eq!((out[at - 1] as i64 + 1) % bar, 0);
}
