//! WP-1.18: the transport in the app, headless. The shell registers the
//! engine's transport commands and runs them on the engine it plays the set
//! through (the null device here, paced like a sound card), so menus, keys,
//! the Control Bar, and agents reach one implementation; the Control Bar's
//! fields turn into registered commands; and what the shell shows is what
//! the engine published. The rules are in
//! the control bar transport spec.

use bl_audio_io::NullControl;
use bl_commands::Permission;
use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_shell::audio::{AudioHost, EngineTransport};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::testing;
use bl_shell::transport::{self, Field, Group};
use bl_shell::transport_commands::{PhaseNudge, TransportState};
use bl_shell::{Invocation, Shell, ShellError};
use serde_json::{Value, json};

const BAR: Ticks = Ticks::WHOLE;

/// A shell playing through the null device, paced by hand (T199), with the
/// control that runs its blocks.
fn shell() -> (Shell, NullControl) {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    (shell, control)
}

/// Advances the set in [`testing::STEP_MS`] steps until `done` holds. The
/// engine is paced by hand, so this lands on the same frames however busy the
/// machine is; two thousand steps are about ten seconds of test time.
fn wait(
    control: &NullControl,
    shell: &mut Shell,
    what: &str,
    done: impl Fn(&EngineTransport) -> bool,
) -> EngineTransport {
    let mut engine = shell.poll_engine();
    for _ in 0..2_000 {
        if done(&engine) {
            return engine;
        }
        testing::advance(control, shell, testing::STEP_MS);
        engine = shell.poll_engine();
    }
    panic!("timed out waiting for {what}: {engine:?}");
}

fn state(shell: &mut Shell) -> TransportState {
    serde_json::from_value(shell.run("transport.state", json!({})).unwrap()).unwrap()
}

/// Runs the command a change of the Control Bar's fields makes, as the
/// window does.
fn run(shell: &mut Shell, change: impl FnOnce(&Shell) -> Option<Invocation>) {
    let invocation = change(shell).expect("the change runs a command");
    shell.invoke(&invocation.id, invocation.args).unwrap();
}

/// The transport commands, with arguments an agent might send.
const TRANSPORT: &[(&str, &str)] = &[
    ("transport.play", "{}"),
    ("transport.stop", "{}"),
    ("transport.play_stop", "{}"),
    ("transport.continue", "{}"),
    ("transport.play_from_insert_marker", "{}"),
    ("transport.locate", "{\"position\": 2822400}"),
    ("transport.scrub", "{\"position\": 2822400}"),
    ("transport.return_to_start", "{}"),
    ("transport.resync_external_hardware", "{}"),
    ("transport.insert_marker_to_playhead", "{}"),
    ("transport.stop_recording", "{}"),
    ("transport.set_recording", "{\"recording\": true}"),
    (
        "transport.phase_nudge",
        "{\"direction\": \"up\", \"held\": false}",
    ),
];

#[test]
fn the_shell_registers_the_transport_and_runs_it_on_the_engine() {
    // Spec: control-bar-transport R2, R12, R13
    let (mut shell, control) = shell();
    let registry = shell.registry();
    assert!(registry.problems().is_empty(), "{:?}", registry.problems());
    let mut registered: Vec<&str> = registry
        .iter()
        .map(|spec| spec.id.as_ref())
        .filter(|id| id.starts_with("transport."))
        .collect();
    registered.sort_unstable();
    let mut expected: Vec<&str> = TRANSPORT.iter().map(|(id, _)| *id).collect();
    expected.extend([
        "transport.set_loop",
        "transport.set_loop_enabled",
        "transport.set_metronome_count_in",
        "transport.set_metronome_enabled",
        "transport.set_metronome_only_while_recording",
        "transport.set_metronome_rhythm",
        "transport.set_metronome_sound",
        "transport.set_midi_overdub",
        "transport.set_punch_in",
        "transport.set_punch_out",
        "transport.state",
        "transport.tap_tempo",
    ]);
    expected.sort_unstable();
    assert_eq!(registered, expected);
    for (id, args) in TRANSPORT {
        let spec = shell.registry().get(id).unwrap();
        // Recording captures input devices, so it asks first.
        let class = if *id == "transport.set_recording" {
            Permission::External
        } else {
            Permission::Read
        };
        assert_eq!(spec.permission, Some(class), "{id}");
        let args: Value = serde_json::from_str(args).unwrap();
        assert!(shell.can_invoke(id, &args), "{id}");
        shell.run(id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
    }

    // Play, then the playhead moves with the engine; Stop keeps where it
    // stopped; Continue resumes there; a second Stop returns to 1.1.1.
    shell.run("transport.stop", json!({})).unwrap();
    shell.run("transport.stop", json!({})).unwrap();
    wait(&control, &mut shell, "the return to 1.1.1", |e| {
        !e.playing && e.position == Ticks::ZERO && e.insert_marker == Ticks::ZERO
    });
    shell.run("transport.play", json!({})).unwrap();
    let first = wait(&control, &mut shell, "playback", |e| {
        e.playing && e.position > Ticks::ZERO
    });
    let later = wait(&control, &mut shell, "the playhead to move", |e| {
        e.clock > first.clock + 4_800
    });
    assert!(later.position > first.position, "{first:?} then {later:?}");
    let shown = state(&mut shell);
    assert!(shown.playing && shown.engine_running);
    shell.run("transport.stop", json!({})).unwrap();
    let stopped = wait(&control, &mut shell, "the stop", |e| !e.playing);
    assert!(stopped.position > Ticks::ZERO);
    assert_eq!(stopped.insert_marker, Ticks::ZERO);
    shell.run("transport.continue", json!({})).unwrap();
    let resumed = wait(&control, &mut shell, "Continue", |e| e.playing);
    assert!(resumed.position >= stopped.position);
    shell.run("transport.stop", json!({})).unwrap();
    shell.run("transport.stop", json!({})).unwrap();
    wait(&control, &mut shell, "the second Stop", |e| {
        !e.playing && e.position == Ticks::ZERO
    });
    shell.audio_mut().stop();
}

#[test]
fn space_and_the_playback_menu_toggle_play_and_stop() {
    // Spec: control-bar-transport R3. The menu item's label follows.
    let (mut shell, control) = shell();
    assert_eq!(
        shell.dynamic_label("transport.play_stop").as_deref(),
        Some("Play")
    );
    shell.invoke("transport.play_stop", json!({})).unwrap();
    wait(&control, &mut shell, "playback", |e| e.playing);
    assert!(shell.playing());
    assert_eq!(
        shell.dynamic_label("transport.play_stop").as_deref(),
        Some("Stop")
    );
    shell.invoke("transport.play_stop", json!({})).unwrap();
    let stopped = wait(&control, &mut shell, "the stop", |e| !e.playing);
    assert!(
        stopped.position > Ticks::ZERO,
        "Play/Stop never returns to 1.1.1"
    );
    assert_eq!(
        shell.dynamic_label("transport.play_stop").as_deref(),
        Some("Play")
    );
    shell.audio_mut().stop();
}

#[test]
fn without_an_engine_the_transport_says_why() {
    // Spec: control-bar-transport R13
    let mut shell = Shell::headless();
    assert!(!shell.can_invoke("transport.play", &json!({})));
    let error = shell.run("transport.play", json!({})).unwrap_err();
    assert!(
        matches!(&error, ShellError::Unavailable { reason, .. } if reason.contains("no audio device")),
        "{error}"
    );
    shell.start_audio();
    shell.run("audio.toggle_engine", json!({})).unwrap();
    let error = shell.run("transport.play", json!({})).unwrap_err();
    assert!(error.to_string().contains("audio engine is off"), "{error}");
    let shown = state(&mut shell);
    assert!(!shown.engine_running && !shown.playing);
    // Spec: time-editing R28. A scrub is refused without an engine too.
    let error = shell
        .run("transport.scrub", json!({ "position": 0, "held": true }))
        .unwrap_err();
    assert!(error.to_string().contains("audio engine is off"), "{error}");
    // Bad arguments are refused as such.
    shell.run("audio.toggle_engine", json!({})).unwrap();
    let error = shell
        .run("transport.locate", json!({ "position": "3.3.1" }))
        .unwrap_err();
    assert!(matches!(error, ShellError::InvalidArgs { .. }), "{error}");
    shell.audio_mut().stop();
}

#[test]
fn the_position_fields_locate_and_show_the_insert_marker_or_the_playhead() {
    // Spec: control-bar-transport R4, R9
    let (mut shell, control) = shell();
    let bars = Field::new(Group::Position, 0);
    assert_eq!(state(&mut shell).arrangement_position, "1.1.1");
    let typed = [
        Some("3".to_owned()),
        Some("3".to_owned()),
        Some("1".to_owned()),
    ];
    run(&mut shell, |s| transport::typed(s, Group::Position, &typed));
    let marker = BAR * 2 + Ticks::QUARTER * 2;
    wait(&control, &mut shell, "the locate", |e| {
        e.insert_marker == marker
    });
    assert_eq!(
        shell.arrangement().insert_marker(),
        marker,
        "the view follows"
    );
    assert_eq!(state(&mut shell).arrangement_position, "3.3.1");
    assert_eq!(transport::text(&shell, bars), "3");
    // A field not typed in keeps its value; values out of range come into
    // range.
    let typed = [None, Some("9".to_owned()), None];
    run(&mut shell, |s| transport::typed(s, Group::Position, &typed));
    wait(&control, &mut shell, "the locate", |e| {
        e.insert_marker == BAR * 2 + Ticks::QUARTER * 3
    });
    let typed = [Some("0".to_owned()), None, None];
    run(&mut shell, |s| transport::typed(s, Group::Position, &typed));
    wait(&control, &mut shell, "the locate", |e| {
        e.insert_marker == Ticks::QUARTER * 3
    });
    // Steps: a bar up, a sixteenth down.
    run(&mut shell, |s| transport::step(s, bars, 1, false));
    wait(&control, &mut shell, "a bar on", |e| {
        e.insert_marker == BAR + Ticks::QUARTER * 3
    });
    run(&mut shell, |s| {
        transport::step(s, Field::new(Group::Position, 2), -1, false)
    });
    let at = BAR + Ticks::QUARTER * 3 - Ticks::SIXTEENTH;
    wait(&control, &mut shell, "a sixteenth back", |e| {
        e.insert_marker == at
    });
    assert_eq!(state(&mut shell).arrangement_position, "2.3.4");
    // While the song plays, the fields show the playhead.
    shell.run("transport.play", json!({})).unwrap();
    let playing = wait(&control, &mut shell, "playback", |e| {
        e.playing && e.position > at + Ticks::SIXTEENTH
    });
    let _ = playing;
    let shown = state(&mut shell);
    let engine = shell.audio_mut().transport_state();
    assert!(shown.playing);
    assert_ne!(shown.arrangement_position, "2.3.4", "{shown:?} {engine:?}");
    // Delete returns to 1.1.1, and playback jumps there.
    run(&mut shell, |s| transport::reset(s, bars));
    wait(&control, &mut shell, "the return", |e| {
        e.insert_marker == Ticks::ZERO && e.position < BAR
    });
    shell.audio_mut().stop();
}

#[test]
fn the_loop_fields_and_switch_are_undoable_document_edits() {
    // Spec: control-bar-transport R5, R8
    let (mut shell, _) = shell();
    let shown = state(&mut shell);
    assert_eq!(
        (
            shown.loop_start_position.as_str(),
            shown.loop_length_bars.as_str()
        ),
        ("1.1.1", "4.0.0")
    );
    assert!(!shown.loop_on);
    let start = Field::new(Group::LoopStart, 0);
    let length = Field::new(Group::LoopLength, 1);
    run(&mut shell, |s| transport::step(s, start, 2, false));
    run(&mut shell, |s| transport::step(s, length, 2, false));
    let typed = [Some("2".to_owned()), None, None];
    run(&mut shell, |s| {
        transport::typed(s, Group::LoopLength, &typed)
    });
    shell
        .run("transport.set_loop_enabled", json!({ "enabled": true }))
        .unwrap();
    let shown = state(&mut shell);
    assert_eq!(shown.loop_start_position, "3.1.1");
    assert_eq!(shown.loop_length_bars, "2.2.0");
    assert!(shown.loop_on);
    assert_eq!(
        shell.editor().undo_history().len(),
        4,
        "each change is one undo step"
    );
    // A drag is one undo step, however far it goes.
    shell.begin_gesture(Group::LoopLength.gesture_label().unwrap());
    for _ in 0..5 {
        run(&mut shell, |s| {
            transport::step(s, Field::new(Group::LoopLength, 2), 1, false)
        });
    }
    shell.end_gesture();
    assert_eq!(shell.editor().undo_history().len(), 5);
    assert_eq!(state(&mut shell).loop_length_bars, "2.3.1");
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(state(&mut shell).loop_length_bars, "2.2.0");
    // The length never reaches zero; Delete gives a new set's four bars.
    let zero = [Some("0".into()), Some("0".into()), Some("0".into())];
    run(&mut shell, |s| {
        transport::typed(s, Group::LoopLength, &zero)
    });
    assert_eq!(
        shell.editor().document().settings().arrangement_loop.length,
        Ticks::SIXTEENTH
    );
    run(&mut shell, |s| transport::reset(s, length));
    assert_eq!(state(&mut shell).loop_length_bars, "4.0.0");
    assert_eq!(transport::reset(&shell, length), None, "already there");
    shell.audio_mut().stop();
}

#[test]
fn the_engine_plays_the_documents_loop() {
    // Spec: control-bar-transport R13. A one-beat loop at 1.1.1: the
    // playhead never leaves it.
    let (mut shell, control) = shell();
    shell
        .run(
            "transport.set_loop",
            json!({ "start": 0, "length": Ticks::QUARTER }),
        )
        .unwrap();
    shell
        .run("transport.set_loop_enabled", json!({ "enabled": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    let first = wait(&control, &mut shell, "playback", |e| e.playing);
    let mut wrapped = false;
    let mut last = Ticks::ZERO;
    let steps = 2_000;
    for _ in 0..steps {
        if wrapped {
            break;
        }
        let engine = shell.poll_engine();
        assert!(engine.position < Ticks::QUARTER, "{engine:?}");
        wrapped |= engine.position < last && engine.clock > first.clock;
        last = engine.position;
        testing::advance(&control, &mut shell, testing::STEP_MS);
    }
    assert!(wrapped, "the loop wrapped");
    assert!(
        shell.arrangement().transport().loop_on,
        "the brace is the document's"
    );
    shell.audio_mut().stop();
}

#[test]
fn tempo_and_signature_fields_set_the_song_as_undo_steps() {
    // Spec: control-bar-transport R6, R7, R8, R9
    let (mut shell, _) = shell();
    let tempo = Field::new(Group::Tempo, 0);
    assert_eq!(transport::text(&shell, tempo), "120.00");
    run(&mut shell, |s| transport::step(s, tempo, 1, false));
    run(&mut shell, |s| transport::step(s, tempo, -5, true));
    assert_eq!(transport::text(&shell, tempo), "120.95");
    run(&mut shell, |s| {
        transport::typed(s, Group::Tempo, &[Some("1500".into()), None, None])
    });
    assert_eq!(state(&mut shell).tempo, 999.0, "within 20 to 999");
    run(&mut shell, |s| {
        transport::typed(s, Group::Tempo, &[Some("98,5".into()), None, None])
    });
    assert_eq!(state(&mut shell).tempo, 98.5);
    let numerator = Field::new(Group::Numerator, 0);
    let denominator = Field::new(Group::Denominator, 0);
    run(&mut shell, |s| {
        transport::typed(s, Group::Numerator, &[Some("7".into()), None, None])
    });
    run(&mut shell, |s| transport::step(s, denominator, 1, false));
    assert_eq!(state(&mut shell).time_signature, "7/8");
    // A typed denominator comes down to an allowed one: 12 to 8, which
    // changes nothing here, and 3 to 2.
    assert_eq!(
        transport::typed(&shell, Group::Denominator, &[Some("12".into()), None, None]),
        None
    );
    run(&mut shell, |s| {
        transport::typed(s, Group::Denominator, &[Some("3".into()), None, None])
    });
    assert_eq!(state(&mut shell).time_signature, "7/2");
    run(&mut shell, |s| {
        transport::typed(s, Group::Numerator, &[Some("120".into()), None, None])
    });
    assert_eq!(state(&mut shell).time_signature, "99/2");
    assert_eq!(
        shell.editor().undo_history().collect::<Vec<_>>(),
        [
            "Change time signatures",
            "Change time signatures",
            "Change time signatures",
            "Change time signatures",
            "Change tempo",
            "Change tempo",
            "Change tempo",
            "Change tempo",
        ],
        "each field's edit is its own undo step, newest first"
    );
    run(&mut shell, |s| transport::reset(s, numerator));
    run(&mut shell, |s| transport::reset(s, denominator));
    run(&mut shell, |s| transport::reset(s, tempo));
    let shown = state(&mut shell);
    assert_eq!((shown.tempo, shown.time_signature.as_str()), (120.0, "4/4"));
    // With tempo changes along the song, the field shows them and can't set
    // one tempo over them.
    let ramp = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(100.0).unwrap()),
        (BAR * 4, Bpm::new(140.0).unwrap()),
    ])
    .unwrap();
    shell
        .run("song.set_tempo", json!({ "tempo": ramp }))
        .unwrap();
    assert!(!transport::editable(&shell, Group::Tempo));
    assert!(!state(&mut shell).tempo_editable);
    assert_eq!(transport::step(&shell, tempo, 1, false), None);
    shell.audio_mut().stop();
}

#[test]
fn a_click_in_the_arrangement_moves_the_insert_marker_and_playback_carries_on() {
    // Spec: control-bar-transport R15
    let (mut shell, control) = shell();
    shell.run("transport.play", json!({})).unwrap();
    let before = wait(&control, &mut shell, "playback", |e| {
        e.playing && e.position > Ticks::ZERO
    });
    shell
        .run("arrangement.set_insert_marker", json!({ "at": BAR * 16 }))
        .unwrap();
    let after = wait(&control, &mut shell, "the marker", |e| {
        e.insert_marker == BAR * 16
    });
    assert!(after.playing, "playback carries on");
    assert!(
        after.position >= before.position && after.position < BAR * 16,
        "the playhead didn't jump: {before:?} then {after:?}"
    );
    assert_eq!(shell.arrangement().insert_marker(), BAR * 16);
    // A time selection's start is where Play starts.
    shell
        .run(
            "arrangement.select_time",
            json!({ "start": BAR * 2, "end": BAR * 4 }),
        )
        .unwrap();
    wait(&control, &mut shell, "the marker", |e| {
        e.insert_marker == BAR * 2
    });
    // Move Insert Marker To Playhead, and a second Stop, move the view's
    // marker too.
    shell.run("transport.stop", json!({})).unwrap();
    let stopped = wait(&control, &mut shell, "the stop", |e| !e.playing);
    shell
        .run("transport.insert_marker_to_playhead", json!({}))
        .unwrap();
    wait(&control, &mut shell, "the marker", |e| {
        e.insert_marker == stopped.position
    });
    assert_eq!(shell.arrangement().insert_marker(), stopped.position);
    shell.run("transport.stop", json!({})).unwrap();
    wait(&control, &mut shell, "the return", |e| {
        e.insert_marker == Ticks::ZERO
    });
    assert_eq!(shell.arrangement().insert_marker(), Ticks::ZERO);
    shell.audio_mut().stop();
}

#[test]
fn a_device_change_keeps_the_song_where_it_was_stopped() {
    // Spec: control-bar-transport R16
    let (mut shell, control) = shell();
    shell
        .run("transport.locate", json!({ "position": BAR * 8 }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    wait(&control, &mut shell, "playback", |e| {
        e.playing && e.position > BAR * 8
    });
    shell
        .run("arrangement.set_insert_marker", json!({ "at": BAR * 3 }))
        .unwrap();
    wait(&control, &mut shell, "the marker", |e| {
        e.insert_marker == BAR * 3
    });
    shell
        .run("audio.configure", json!({ "buffer_size": 128 }))
        .unwrap();
    let moved = shell.audio_mut().transport_state();
    assert!(moved.position > BAR * 8, "{moved:?}");
    let back = wait(&control, &mut shell, "the new engine's first block", |e| {
        e.clock > 0
    });
    assert!(!back.playing, "stopped after the restart");
    assert_eq!(back.insert_marker, BAR * 3);
    // Within a block of where the old engine last showed it.
    assert!(
        (back.position - moved.position).get().abs() <= Ticks::QUARTER.get() / 2,
        "{moved:?} then {back:?}"
    );
    shell.run("transport.continue", json!({})).unwrap();
    let resumed = wait(&control, &mut shell, "Continue", |e| e.playing);
    assert!(resumed.position >= back.position);
    shell.audio_mut().stop();
}

#[test]
fn a_new_set_starts_stopped_at_the_start() {
    // Spec: control-bar-transport R16
    let (mut shell, control) = shell();
    shell
        .run("transport.locate", json!({ "position": BAR * 5 }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    wait(&control, &mut shell, "playback", |e| e.playing);
    shell.run("set.new", json!({})).unwrap();
    let fresh = wait(&control, &mut shell, "the stop", |e| {
        !e.playing && e.position == Ticks::ZERO && e.insert_marker == Ticks::ZERO
    });
    assert_eq!(fresh.insert_marker, shell.arrangement().insert_marker());
    shell.audio_mut().stop();
}

#[test]
fn a_held_scrub_loops_its_span_by_id_until_it_is_released() {
    // Spec: time-editing R25, R28; transport R23. The default quantisation
    // is a bar, so a press at 3.2.1 loops bar 3.
    let (mut shell, control) = shell();
    let (start, end) = (Ticks::WHOLE * 2, Ticks::WHOLE * 3);
    shell
        .run(
            "transport.scrub",
            json!({ "position": start + Ticks::QUARTER, "held": true }),
        )
        .unwrap();
    let first = wait(&control, &mut shell, "the held loop to play", |e| e.playing);
    let mut wrapped = false;
    let mut last = Ticks::ZERO;
    for _ in 0..2_000 {
        if wrapped {
            break;
        }
        let engine = shell.poll_engine();
        assert!(
            (start..end).contains(&engine.position),
            "the playhead left the span: {engine:?}"
        );
        wrapped |= engine.position < last && engine.clock > first.clock;
        last = engine.position;
        testing::advance(&control, &mut shell, testing::STEP_MS);
    }
    assert!(wrapped, "the span repeated");
    // Released, playback goes on past the span's end.
    shell
        .run(
            "transport.scrub",
            json!({ "position": start, "held": false }),
        )
        .unwrap();
    let later = wait(&control, &mut shell, "playback past the span", |e| {
        e.position >= end
    });
    assert!(later.playing);
    shell.audio_mut().stop();
}

#[test]
fn permanent_scrub_areas_is_on_kept_between_launches_and_read_back() {
    // Spec: time-editing R26. On in a new install; the setting is read back
    // from `settings.state` and survives a relaunch.
    let dir = std::env::temp_dir().join(format!("bl-shell-scrub-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let launch = || {
        Shell::new(
            Preferences::load(&dir).unwrap(),
            Some(dir.clone()),
            AudioHost::null(Default::default()),
            Box::new(ProjectFiles::new()),
        )
    };
    let read = |shell: &mut Shell| {
        shell.run("settings.state", json!({})).unwrap()["display_input"]["permanent_scrub_areas"]
            .clone()
    };
    let mut first = launch();
    assert!(first.permanent_scrub_areas());
    assert_eq!(read(&mut first), json!(true));
    let result = first
        .run("settings.set_permanent_scrub_areas", json!({ "on": false }))
        .unwrap();
    assert_eq!(result, json!({ "on": false }));
    assert_eq!(read(&mut first), json!(false));
    assert!(matches!(
        first
            .run("settings.set_permanent_scrub_areas", json!({ "on": "yes" }))
            .unwrap_err(),
        ShellError::InvalidArgs { .. }
    ));
    drop(first);
    let mut second = launch();
    assert!(!second.permanent_scrub_areas(), "the next start has it off");
    assert_eq!(read(&mut second), json!(false));
    // The shell holds a file in `dir` open, which Windows won't delete.
    drop(second);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn phase_nudge_runs_by_id_reads_back_and_the_last_pressed_of_two_counts() {
    // Spec: transport R27, R28.
    let (mut shell, control) = shell();
    shell.run("transport.play", json!({})).unwrap();
    wait(&control, &mut shell, "playback", |e| e.playing);
    let undoable = shell.editor().undo_history().count();
    let nudge = |shell: &mut Shell, direction: &str, held: bool| {
        shell
            .run(
                "transport.phase_nudge",
                json!({ "direction": direction, "held": held }),
            )
            .unwrap();
    };
    assert_eq!(state(&mut shell).phase_nudge, PhaseNudge::None);
    nudge(&mut shell, "down", true);
    wait(&control, &mut shell, "Down", |e| e.phase_nudge.is_some());
    assert_eq!(state(&mut shell).phase_nudge, PhaseNudge::Down);
    nudge(&mut shell, "up", true);
    wait(&control, &mut shell, "Up", |e| {
        e.phase_nudge == Some(bl_engine::NudgeDirection::Up)
    });
    assert_eq!(state(&mut shell).phase_nudge, PhaseNudge::Up);
    // Letting go of the one pressed last leaves the other nudging.
    nudge(&mut shell, "up", false);
    wait(&control, &mut shell, "Down again", |e| {
        e.phase_nudge == Some(bl_engine::NudgeDirection::Down)
    });
    assert_eq!(state(&mut shell).phase_nudge, PhaseNudge::Down);
    nudge(&mut shell, "down", false);
    wait(&control, &mut shell, "the release", |e| {
        e.phase_nudge.is_none()
    });
    assert_eq!(state(&mut shell).phase_nudge, PhaseNudge::None);
    // The tempo and the set are as they were, and nothing is an undo step.
    assert_eq!(state(&mut shell).tempo, 120.0);
    assert_eq!(shell.editor().undo_history().count(), undoable);
    // A direction that isn't one is refused.
    assert!(
        shell
            .run(
                "transport.phase_nudge",
                json!({ "direction": "left", "held": true })
            )
            .is_err()
    );
    shell.audio_mut().stop();
}

#[test]
fn phase_nudge_is_refused_while_an_outside_tempo_leads_but_a_release_is_taken() {
    // Spec: transport R29.
    let (mut shell, control) = shell();
    shell.run("transport.play", json!({})).unwrap();
    wait(&control, &mut shell, "playback", |e| e.playing);
    shell
        .run(
            "transport.phase_nudge",
            json!({ "direction": "down", "held": true }),
        )
        .unwrap();
    wait(&control, &mut shell, "the nudge", |e| {
        e.phase_nudge.is_some()
    });
    shell
        .audio()
        .set_follower(Some(Box::new(bl_engine::testing::following(140.0))));
    wait(&control, &mut shell, "the follower", |e| e.following);
    let refused = shell
        .run(
            "transport.phase_nudge",
            json!({ "direction": "up", "held": true }),
        )
        .unwrap_err();
    assert!(
        matches!(refused, ShellError::Unavailable { .. }),
        "{refused}"
    );
    assert!(refused.to_string().contains("leads the tempo"), "{refused}");
    // The release of the nudge held before the follower came is taken.
    shell
        .run(
            "transport.phase_nudge",
            json!({ "direction": "down", "held": false }),
        )
        .unwrap();
    wait(&control, &mut shell, "the release", |e| {
        e.phase_nudge.is_none()
    });
    assert_eq!(state(&mut shell).phase_nudge, PhaseNudge::None);
    shell.audio_mut().stop();
}

#[test]
fn a_start_that_waits_for_the_follower_is_in_transport_state_until_stop_cancels_it() {
    // Spec: transport R25.
    let (mut shell, control) = shell();
    shell
        .audio()
        .set_follower(Some(Box::new(bl_engine::testing::stand_in(|_, answer| {
            answer.tempo(120.0);
            answer.decides_starts();
        }))));
    wait(&control, &mut shell, "the follower", |e| e.following);
    let before = state(&mut shell);
    assert!(!before.waiting_start && before.waiting_frame.is_none());
    shell.run("transport.play", json!({})).unwrap();
    wait(&control, &mut shell, "the wait", |e| e.waiting_start);
    let waiting = state(&mut shell);
    assert!(waiting.waiting_start && !waiting.playing, "{waiting:?}");
    assert_eq!(waiting.waiting_frame, None, "no frame yet");
    // The follower gives a frame, far ahead.
    shell
        .audio()
        .set_follower(Some(Box::new(bl_engine::testing::stand_in(|_, answer| {
            answer.tempo(120.0);
            answer.decides_starts();
            answer.start_at(1_000_000_000);
        }))));
    wait(&control, &mut shell, "the frame", |e| {
        e.waiting_frame.is_some()
    });
    assert_eq!(state(&mut shell).waiting_frame, Some(1_000_000_000));
    shell.run("transport.stop", json!({})).unwrap();
    wait(&control, &mut shell, "the cancel", |e| !e.waiting_start);
    let cancelled = state(&mut shell);
    assert!(!cancelled.waiting_start && cancelled.waiting_frame.is_none());
    assert!(!cancelled.playing);
    shell.audio_mut().stop();
}
