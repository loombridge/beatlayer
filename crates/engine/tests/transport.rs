//! The transport (WP-1.5): play, stop, continue, the insert marker, and
//! the Arrangement loop, each on its exact frame, following the tempo map,
//! as `docs/specs/transport.md` specifies. A position probe on the only
//! track writes each frame's song position into the output, so a test reads
//! the transport sample by sample. Every block runs as a real-time section
//! under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_commands::Registry;
use bl_core::time::{Bpm, Frames, TempoMap, Ticks};
use bl_engine::{Action, LoopRegion, ScheduledCommand, TransportCommand, When, testing, transport};
use bl_model::{Command, TrackKind};
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
    run(&mut rig, "transport.locate", json!({ "position": 0 }));
    let out = rig.render(48_000, 256);
    for (clock, &sample) in out.iter().enumerate() {
        assert_eq!(sample, (clock % 24_000) as f32);
    }

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
                text
            })
            .collect()
    };
    // Spec: transport R5, R6, R8, R9, R10
    assert_eq!(bindings("transport.play_stop"), ["S-TRANS-001"]);
    assert_eq!(bindings("transport.continue"), ["S-TRANS-002"]);
    assert_eq!(bindings("transport.locate"), ["S-TRANS-005 position=0"]);
    assert_eq!(
        bindings("transport.insert_marker_to_playhead"),
        ["S-ARR-033"]
    );
    assert_eq!(bindings("transport.play_from_insert_marker"), ["S-ARR-032"]);
    for id in [
        "transport.play",
        "transport.stop",
        "transport.set_loop",
        "transport.set_loop_enabled",
    ] {
        assert_eq!(bindings(id), Vec::<String>::new(), "{id}");
    }
}
