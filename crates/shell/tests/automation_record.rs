//! WP-2.12's recording acceptance with the test backend: with Automation Arm
//! on and the transport recording, moving a control writes its values into
//! the parameter's envelope at the playback position, in one undo step made
//! when recording stops.
//!
//! - Moves during recording land at the positions they were made at.
//! - With Automation Arm off, nothing is recorded.
//! - A released control hands the parameter back to its old envelope.
//! - One undo removes the recording.
//! - The audio thread allocates nothing while it records.
//!
//! WP-6.46's modes: in Latch a control's value holds until the recording
//! stops, in Write its envelope is written from where the recording started,
//! and both carry on over the Arrangement loop's wrap.

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Envelope, MixerParam, TrackId};
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings, EngineTransport};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::{AutomationRecordMode, Preferences};
use bl_shell::testing;
use serde_json::json;

fn volume() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Volume,
    }
}

/// A shell playing through the null device paced by hand (T199), with one
/// track, and the control that runs its blocks.
fn shell_with_a_track() -> (Shell, NullControl, TrackId) {
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            pacing: Pacing::Manual,
            ..NullOptions::default()
        },
        &[("null", "Null")],
    );
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(move |_| Ok(Box::new(backend.clone()))),
        AudioSettings::default(),
    );
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        audio,
        Box::new(ProjectFiles::new()),
    );
    shell.use_test_clock(control.clock().expect("manual pacing"));
    shell.start_audio();
    shell
        .invoke("track.create", json!({ "kind": "audio", "name": "Tone" }))
        .unwrap();
    let track = shell.editor().document().top_level().last().unwrap();
    (shell, control, track)
}

/// Polls the engine as the app does, advancing the set in test-clock steps,
/// until `done` holds of what it shows. The engine is paced by hand, so this
/// lands on the same frames however busy the machine is.
fn poll_until(
    control: &NullControl,
    shell: &mut Shell,
    what: &str,
    done: impl Fn(&EngineTransport) -> bool,
) {
    for _ in 0..2_000 {
        if done(&shell.poll_engine()) {
            return;
        }
        testing::advance(control, shell, testing::STEP_MS);
    }
    panic!("timed out waiting for {what}");
}

/// Lets the playhead move on by 150 ms of test time, then reads where it is.
fn let_time_pass(control: &NullControl, shell: &mut Shell) -> Ticks {
    pass_time(control, shell, 150)
}

/// Lets the playhead move on by `millis` of test time, then reads where it
/// is.
fn pass_time(control: &NullControl, shell: &mut Shell, millis: u64) -> Ticks {
    testing::advance(control, shell, millis);
    shell.poll_engine().position
}

/// Advances until the playhead wraps round the Arrangement loop, and reads
/// where it is.
fn wait_for_the_wrap(control: &NullControl, shell: &mut Shell) -> Ticks {
    let mut previous = shell.poll_engine().position;
    for _ in 0..2_000 {
        let position = let_time_pass(control, shell);
        if position < previous {
            return position;
        }
        previous = position;
    }
    panic!("the playhead never wrapped round the loop");
}

/// Chooses the mode an Arrangement recording writes its moves in.
fn set_mode(shell: &mut Shell, mode: &str) {
    shell
        .run(
            "settings.set_automation_record_mode",
            json!({ "mode": mode }),
        )
        .unwrap();
}

fn start_recording(control: &NullControl, shell: &mut Shell) {
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    poll_until(control, shell, "recording", |engine| {
        engine.recording && engine.playing
    });
}

/// Stops, and waits for the recording to be written into the set.
fn stop_recording(control: &NullControl, shell: &mut Shell) {
    shell.run("transport.stop", json!({})).unwrap();
    poll_until(control, shell, "the stop", |engine| !engine.playing);
}

fn envelope(shell: &Shell, track: TrackId) -> Option<Envelope> {
    shell
        .editor()
        .document()
        .track(track)?
        .automation_for(&volume())
        .map(|automation| automation.envelope.clone())
}

fn recorded_steps(shell: &Shell) -> usize {
    shell
        .editor()
        .undo_history()
        .filter(|label| *label == "Record Automation")
        .count()
}

// Spec: automation R17
#[test]
fn moves_during_recording_land_at_their_positions_and_undo_as_one_step() {
    let (mut shell, control, track) = shell_with_a_track();
    shell
        .run("arrangement.automation_arm", json!({ "on": true }))
        .unwrap();
    start_recording(&control, &mut shell);

    let first = shell.poll_engine().position;
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": track, "volume": 0.25 }),
        )
        .unwrap();
    let second = let_time_pass(&control, &mut shell);
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": track, "volume": 0.75 }),
        )
        .unwrap();
    assert!(second > first, "the playhead moved");
    assert!(
        envelope(&shell, track).is_none(),
        "nothing is written until recording stops"
    );
    stop_recording(&control, &mut shell);

    // The moves are in the envelope at the playback positions they were made
    // at, and with no envelope before, the last value holds: no release point.
    let recorded = envelope(&shell, track).expect("the recording made an envelope");
    let points: Vec<_> = recorded.points().map(|p| (p.at, p.value)).collect();
    assert_eq!(points, [(first, 0.25), (second, 0.75)]);
    assert!(
        shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .automation_for(&volume())
            .unwrap()
            .enabled,
        "the recorded envelope drives the control"
    );
    // Playback afterwards follows the recorded values.
    assert_eq!(recorded.value_at(first), Some(0.25));
    assert_eq!(recorded.value_at(second + Ticks::WHOLE), Some(0.75));

    // One undo step for the whole recording.
    assert_eq!(recorded_steps(&shell), 1);
    shell.run("edit.undo", json!({})).unwrap();
    assert!(envelope(&shell, track).is_none(), "undo removes it all");

    // The audio thread stayed off the heap.
    let stats = shell.audio().stats().expect("a stream is open");
    assert!(stats.blocks > 0);
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

// Spec: automation R17
#[test]
fn with_automation_arm_off_nothing_is_recorded() {
    let (mut shell, control, track) = shell_with_a_track();
    start_recording(&control, &mut shell);
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": track, "volume": 0.25 }),
        )
        .unwrap();
    let_time_pass(&control, &mut shell);
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": track, "volume": 0.75 }),
        )
        .unwrap();
    stop_recording(&control, &mut shell);
    assert!(envelope(&shell, track).is_none());
    assert_eq!(recorded_steps(&shell), 0);
}

// Spec: automation R17
#[test]
fn a_released_control_hands_the_parameter_back_to_its_old_envelope() {
    let (mut shell, control, track) = shell_with_a_track();
    let end = Ticks::WHOLE * 1000;
    for (at, value) in [(Ticks::ZERO, 0.0), (end, 1.0)] {
        shell
            .run(
                "automation.set_breakpoint",
                json!({ "track": track, "target": { "type": "mixer", "param": "volume" },
                        "at": at, "value": value }),
            )
            .unwrap();
    }
    let old = envelope(&shell, track).unwrap();
    shell
        .run("arrangement.automation_arm", json!({ "on": true }))
        .unwrap();
    start_recording(&control, &mut shell);

    // A drag: the control is moved, then let go.
    let first = shell.poll_engine().position;
    shell.begin_gesture("Change volume");
    shell
        .run("mixer.set_volume", json!({ "track": track, "volume": 0.9 }))
        .unwrap();
    let released = let_time_pass(&control, &mut shell);
    shell.end_gesture();
    let_time_pass(&control, &mut shell);
    stop_recording(&control, &mut shell);

    // The touch's point, then the old envelope's value at the release, and
    // the old envelope after it.
    let recorded = envelope(&shell, track).unwrap();
    let points: Vec<_> = recorded.points().map(|p| (p.at, p.value)).collect();
    assert_eq!(
        points,
        [
            (Ticks::ZERO, 0.0),
            (first, 0.9),
            (released, old.value_at(released).unwrap()),
            (end, 1.0),
        ]
    );
    assert_eq!(recorded_steps(&shell), 1);
}

// Spec: automation R19
#[test]
fn in_latch_a_released_control_holds_its_value_to_the_stop() {
    let (mut shell, control, track) = shell_with_a_track();
    set_mode(&mut shell, "latch");
    shell
        .run("arrangement.automation_arm", json!({ "on": true }))
        .unwrap();
    start_recording(&control, &mut shell);

    // A drag: the control is moved, then let go. In Latch the release
    // doesn't end the touch.
    let first = shell.poll_engine().position;
    shell.begin_gesture("Change volume");
    shell
        .run("mixer.set_volume", json!({ "track": track, "volume": 0.9 }))
        .unwrap();
    let released = let_time_pass(&control, &mut shell);
    shell.end_gesture();
    let_time_pass(&control, &mut shell);
    stop_recording(&control, &mut shell);
    let stop = shell.poll_engine().position;

    // The value the control was moved to holds from the move to the stop.
    let recorded = envelope(&shell, track).expect("the recording made an envelope");
    let points: Vec<_> = recorded.points().map(|p| (p.at, p.value)).collect();
    assert_eq!(points, [(first, 0.9), (stop, 0.9)], "{points:?}");
    assert!(stop > released, "the stop is after the release");
    assert_eq!(recorded_steps(&shell), 1);
}

// Spec: automation R19
#[test]
fn in_latch_the_old_envelope_resumes_a_tick_after_the_stop() {
    let (mut shell, control, track) = shell_with_a_track();
    let end = Ticks::WHOLE * 1000;
    for (at, value) in [(Ticks::ZERO, 0.0), (end, 1.0)] {
        shell
            .run(
                "automation.set_breakpoint",
                json!({ "track": track, "target": { "type": "mixer", "param": "volume" },
                        "at": at, "value": value }),
            )
            .unwrap();
    }
    let old = envelope(&shell, track).unwrap();
    set_mode(&mut shell, "latch");
    shell
        .run("arrangement.automation_arm", json!({ "on": true }))
        .unwrap();
    start_recording(&control, &mut shell);

    let first = shell.poll_engine().position;
    shell.begin_gesture("Change volume");
    shell
        .run("mixer.set_volume", json!({ "track": track, "volume": 0.9 }))
        .unwrap();
    let_time_pass(&control, &mut shell);
    shell.end_gesture();
    let_time_pass(&control, &mut shell);
    stop_recording(&control, &mut shell);
    let stop = shell.poll_engine().position;

    // The value holds to the stop, and the old envelope resumes there, a
    // tick later.
    let recorded = envelope(&shell, track).unwrap();
    let points: Vec<_> = recorded.points().map(|p| (p.at, p.value)).collect();
    assert_eq!(
        points,
        [
            (Ticks::ZERO, 0.0),
            (first, 0.9),
            (stop, 0.9),
            (stop + Ticks(1), old.value_at(stop + Ticks(1)).unwrap()),
            (end, 1.0),
        ],
        "{points:?}"
    );
    assert_eq!(recorded_steps(&shell), 1);
}

// Spec: automation R20
#[test]
fn in_write_a_moved_control_is_written_from_where_the_recording_started() {
    let (mut shell, control, track) = shell_with_a_track();
    set_mode(&mut shell, "write");
    shell
        .run("arrangement.automation_arm", json!({ "on": true }))
        .unwrap();
    start_recording(&control, &mut shell);
    let start = shell.poll_engine().position;

    // The control is moved once, after the recording has begun.
    let first = pass_time(&control, &mut shell, 150);
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": track, "volume": 0.25 }),
        )
        .unwrap();
    pass_time(&control, &mut shell, 150);
    stop_recording(&control, &mut shell);
    let stop = shell.poll_engine().position;

    // The envelope is written from where the recording started: the value
    // the control had then holds until its first move, and the value it was
    // moved to holds from there to the stop.
    let recorded = envelope(&shell, track).expect("the recording made an envelope");
    let points: Vec<_> = recorded.points().map(|p| (p.at, p.value)).collect();
    assert_eq!(
        points,
        [(start, 1.0), (first, 0.25), (stop, 0.25)],
        "{points:?}"
    );
    assert_eq!(recorded_steps(&shell), 1);
}

// Spec: automation R21
#[test]
fn a_latch_touch_carries_on_over_the_loops_wrap() {
    let (mut shell, control, track) = shell_with_a_track();
    // A loop of one bar, so the playhead wraps while the recording runs.
    shell
        .run(
            "transport.set_loop",
            json!({ "start": 0, "length": 4 * Ticks::WHOLE.get() }),
        )
        .unwrap();
    shell
        .run("transport.set_loop_enabled", json!({ "enabled": true }))
        .unwrap();
    set_mode(&mut shell, "latch");
    shell
        .run("arrangement.automation_arm", json!({ "on": true }))
        .unwrap();
    start_recording(&control, &mut shell);

    // The control is moved in the first pass of the loop.
    pass_time(&control, &mut shell, 500);
    let first = shell.poll_engine().position;
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": track, "volume": 0.25 }),
        )
        .unwrap();
    // The playhead wraps round the loop, and the control is moved again,
    // early in the second pass.
    wait_for_the_wrap(&control, &mut shell);
    pass_time(&control, &mut shell, 100);
    let second = shell.poll_engine().position;
    assert!(second < first, "the second move is in the second pass");
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": track, "volume": 0.75 }),
        )
        .unwrap();
    pass_time(&control, &mut shell, 150);
    stop_recording(&control, &mut shell);
    let stop = shell.poll_engine().position;

    // The touch carries on over the wrap: the value it held holds from the
    // loop's start, and the second move overwrites what the first pass
    // wrote.
    let recorded = envelope(&shell, track).expect("the recording made an envelope");
    let points: Vec<_> = recorded.points().map(|p| (p.at, p.value)).collect();
    assert_eq!(
        points,
        [(Ticks::ZERO, 0.25), (second, 0.75), (stop, 0.75)],
        "{points:?}"
    );
    assert_eq!(recorded_steps(&shell), 1);
}

// Spec: automation R19
#[test]
fn in_latch_two_moves_at_one_position_keep_the_last() {
    let (mut shell, control, track) = shell_with_a_track();
    set_mode(&mut shell, "latch");
    shell
        .run("arrangement.automation_arm", json!({ "on": true }))
        .unwrap();
    start_recording(&control, &mut shell);

    // Two moves before the playhead moves on: the second replaces the
    // first, as in Touch.
    let at = pass_time(&control, &mut shell, 150);
    for volume in [0.25, 0.5] {
        shell
            .run(
                "mixer.set_volume",
                json!({ "track": track, "volume": volume }),
            )
            .unwrap();
    }
    pass_time(&control, &mut shell, 150);
    stop_recording(&control, &mut shell);
    let stop = shell.poll_engine().position;

    let recorded = envelope(&shell, track).unwrap();
    let points: Vec<_> = recorded.points().map(|p| (p.at, p.value)).collect();
    assert_eq!(points, [(at, 0.5), (stop, 0.5)], "{points:?}");
}

// Spec: automation R21
#[test]
fn a_latch_touch_holds_over_the_wrap_without_another_move() {
    let (mut shell, control, track) = shell_with_a_track();
    let end = Ticks::WHOLE * 1000;
    for (at, value) in [(Ticks::ZERO, 0.0), (end, 1.0)] {
        shell
            .run(
                "automation.set_breakpoint",
                json!({ "track": track, "target": { "type": "mixer", "param": "volume" },
                        "at": at, "value": value }),
            )
            .unwrap();
    }
    let old = envelope(&shell, track).unwrap();
    shell
        .run(
            "transport.set_loop",
            json!({ "start": 0, "length": 4 * Ticks::WHOLE.get() }),
        )
        .unwrap();
    shell
        .run("transport.set_loop_enabled", json!({ "enabled": true }))
        .unwrap();
    set_mode(&mut shell, "latch");
    shell
        .run("arrangement.automation_arm", json!({ "on": true }))
        .unwrap();
    start_recording(&control, &mut shell);

    // One move in the first pass, then the recording stops early in the
    // second, before the move's position.
    let first = pass_time(&control, &mut shell, 500);
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": track, "volume": 0.25 }),
        )
        .unwrap();
    wait_for_the_wrap(&control, &mut shell);
    pass_time(&control, &mut shell, 100);
    stop_recording(&control, &mut shell);
    let stop = shell.poll_engine().position;
    assert!(stop < first, "the recording stops in the second pass");

    // The held value carries on from the loop's start to the stop, and the
    // old envelope resumes a tick after it.
    let recorded = envelope(&shell, track).unwrap();
    let points: Vec<_> = recorded.points().map(|p| (p.at, p.value)).collect();
    assert_eq!(
        points,
        [
            (Ticks::ZERO, 0.25),
            (stop, 0.25),
            (stop + Ticks(1), old.value_at(stop + Ticks(1)).unwrap()),
            (end, 1.0),
        ],
        "{points:?}"
    );
    assert_eq!(recorded_steps(&shell), 1);
}

// Spec: automation R18
#[test]
fn the_mode_is_kept_between_launches() {
    let dir = std::env::temp_dir().join(format!("bl-shell-record-mode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (backend, _control) = NullBackend::with_devices(
        NullOptions {
            pacing: Pacing::Manual,
            ..NullOptions::default()
        },
        &[("null", "Null")],
    );
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(move |_| Ok(Box::new(backend.clone()))),
        AudioSettings::default(),
    );
    let mut shell = Shell::new(
        Preferences::default(),
        Some(dir.clone()),
        audio,
        Box::new(ProjectFiles::new()),
    );
    set_mode(&mut shell, "write");
    assert_eq!(shell.automation_record_mode(), AutomationRecordMode::Write);

    // A relaunch: the preferences are read from the config directory.
    let prefs = Preferences::load(&dir).unwrap();
    assert_eq!(prefs.automation_record_mode, AutomationRecordMode::Write);
    let _ = std::fs::remove_dir_all(&dir);
}
