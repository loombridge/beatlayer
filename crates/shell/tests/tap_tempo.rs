//! Tap Tempo in the shell (WP-7.31), headless: the tap law through
//! `transport.tap_tempo`, its undo step, the refusal while the song has
//! tempo changes, and the start a run of taps makes on the null device's
//! engine clock. The rules are in the control bar transport spec, R18 to R22.

use bl_audio_io::NullControl;
use bl_core::time::{Bpm, Frames, SampleRate, TempoMap, Ticks};
use bl_shell::audio::{AudioHost, AudioSettings, EngineTransport};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::testing;
use bl_shell::transport_commands::{TapTempo, TransportState};
use bl_shell::{Shell, ShellError};
use serde_json::{Value, json};

/// 48 kHz: frames in a millisecond.
const MS: u64 = 48;
const BAR: Ticks = Ticks::WHOLE;

fn shell_with_blocks(block: u32) -> (Shell, NullControl) {
    let (mut shell, control) = testing::rig_with(AudioSettings {
        buffer_size: Some(block),
        ..AudioSettings::default()
    });
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    testing::settle(&control, &mut shell);
    (shell, control)
}

fn rig() -> (Shell, NullControl) {
    shell_with_blocks(256)
}

fn tap(shell: &mut Shell, at: u64) -> TapTempo {
    serde_json::from_value(
        shell
            .run("transport.tap_tempo", json!({ "at": at }))
            .unwrap(),
    )
    .unwrap()
}

/// Taps at `gaps` milliseconds apart from frame 100, returning each tap's
/// result.
fn tap_run(shell: &mut Shell, gaps: &[u64]) -> Vec<TapTempo> {
    let mut frame = 100;
    std::iter::once(&0)
        .chain(gaps)
        .map(|gap| {
            frame += gap * MS;
            tap(shell, frame)
        })
        .collect()
}

fn state(shell: &mut Shell) -> TransportState {
    serde_json::from_value(shell.run("transport.state", json!({})).unwrap()).unwrap()
}

/// The song's tempo at its start.
fn tempo(shell: &Shell) -> f64 {
    shell
        .editor()
        .document()
        .settings()
        .tempo
        .bpm_at(Ticks::ZERO)
        .get()
}

fn bpm(value: f64) -> Bpm {
    Bpm::new(value).unwrap()
}

#[test]
fn taps_set_the_tempo_from_the_second_by_the_mean_of_the_last_eight_intervals() {
    // Spec: control-bar-transport R18
    let (mut shell, _control) = rig();
    shell
        .run(
            "song.set_tempo",
            json!({ "tempo": TempoMap::constant(bpm(90.0)) }),
        )
        .unwrap();
    let run = tap_run(&mut shell, &[500]);
    assert_eq!(run[0].tempo, None, "one tap sets nothing");
    assert_eq!(tempo(&shell), 120.0);
    assert_eq!((run[1].taps, run[1].tempo), (2, Some(120.0)));
    // A ninth tap after eight intervals, the first 1,000 ms: the mean of all
    // eight is 562.5 ms, and a tenth drops the long one.
    let (mut shell, _control) = rig();
    let run = tap_run(&mut shell, &[1000, 500, 500, 500, 500, 500, 500, 500, 500]);
    assert_eq!(run[8].tempo, Some(106.67));
    assert_eq!(run[9].tempo, Some(120.0));
    assert_eq!(tempo(&shell), 120.0);
    assert_eq!(state(&mut shell).tap_tempo.taps, 10);
}

#[test]
fn taps_3_5_seconds_apart_start_a_new_run_and_a_tap_30_ms_after_another_changes_nothing() {
    // Spec: control-bar-transport R18
    let (mut shell, _control) = rig();
    tap_run(&mut shell, &[500]);
    assert_eq!(tempo(&shell), 120.0);
    let last = 100 + 500 * MS;
    let bounce = tap(&mut shell, last + 30 * MS);
    assert_eq!((bounce.taps, bounce.tempo), (2, Some(120.0)));
    let fresh = tap(&mut shell, last + 3_500 * MS);
    assert_eq!((fresh.taps, fresh.tempo), (1, None), "a new run");
    assert_eq!(tempo(&shell), 120.0, "the new run's first tap sets nothing");
    let second = tap(&mut shell, last + 3_500 * MS + 400 * MS);
    assert_eq!((second.taps, second.tempo), (2, Some(150.0)));
}

#[test]
fn a_tap_is_one_beat_of_the_time_signature() {
    // Spec: control-bar-transport R18. Eighth notes 250 ms apart in 6/8.
    let (mut shell, _control) = rig();
    shell
        .run(
            "song.set_time_signatures",
            json!({ "time_signatures": bl_core::time::TimeSignatureMap::constant(
                bl_core::time::TimeSignature::new(6, 8).unwrap()) }),
        )
        .unwrap();
    shell
        .run(
            "song.set_tempo",
            json!({ "tempo": TempoMap::constant(bpm(90.0)) }),
        )
        .unwrap();
    let run = tap_run(&mut shell, &[250]);
    assert_eq!(run[1].tempo, Some(120.0));
    assert_eq!(tempo(&shell), 120.0);
}

#[test]
fn a_run_is_one_undo_step_and_an_edit_between_taps_starts_another() {
    // Spec: control-bar-transport R21
    let (mut shell, _control) = rig();
    shell
        .run(
            "song.set_tempo",
            json!({ "tempo": TempoMap::constant(bpm(90.0)) }),
        )
        .unwrap();
    let before = shell.editor().undo_history().count();
    tap_run(&mut shell, &[500; 9]);
    let history: Vec<_> = shell.editor().undo_history().collect();
    assert_eq!(history.len(), before + 1, "ten taps are one step");
    assert_eq!(history[0], "Tap Tempo");
    assert_eq!(tempo(&shell), 120.0);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(tempo(&shell), 90.0, "the tempo before the run");
    shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(tempo(&shell), 120.0);

    // An edit between two taps ends the step.
    let (mut shell, _control) = rig();
    shell
        .run(
            "song.set_tempo",
            json!({ "tempo": TempoMap::constant(bpm(90.0)) }),
        )
        .unwrap();
    let last = tap_run(&mut shell, &[500, 500]);
    assert_eq!(last[2].tempo, Some(120.0));
    shell
        .run("transport.set_loop_enabled", json!({ "enabled": true }))
        .unwrap();
    let next = 100 + 1_400 * MS;
    tap(&mut shell, next);
    let history: Vec<_> = shell.editor().undo_history().collect();
    assert_eq!(history[..3], ["Tap Tempo", "Switch loop", "Tap Tempo"]);
}

#[test]
fn a_tap_without_a_frame_is_taken_onto_the_engine_clock_as_the_test_clock_has_it() {
    // Spec: control-bar-transport R19
    let (mut shell, control) = rig();
    let clock = control.clock().unwrap();
    shell
        .run(
            "song.set_tempo",
            json!({ "tempo": TempoMap::constant(bpm(90.0)) }),
        )
        .unwrap();
    let first = clock.now();
    shell.run("transport.tap_tempo", json!({})).unwrap();
    testing::advance(&control, &mut shell, 500);
    let second = clock.now();
    let result: TapTempo =
        serde_json::from_value(shell.run("transport.tap_tempo", json!({})).unwrap()).unwrap();
    let expected = 60.0 / (second - first).as_secs_f64();
    let tapped = result.tempo.expect("the second tap sets the tempo");
    assert!(
        (tapped - expected).abs() < 0.1,
        "{tapped} against {expected}"
    );
    assert_eq!(tempo(&shell), tapped);
}

#[test]
fn a_new_run_that_changes_nothing_at_first_does_not_amend_the_last_runs_step() {
    // Spec: control-bar-transport R21
    let (mut shell, _control) = rig();
    shell
        .run(
            "song.set_tempo",
            json!({ "tempo": TempoMap::constant(bpm(90.0)) }),
        )
        .unwrap();
    tap_run(&mut shell, &[500, 500]);
    // Four seconds on, a run whose second tap sets the same 120 BPM, then a
    // third that sets 150.
    let start = 100 + 1_000 * MS + 4_000 * MS;
    tap(&mut shell, start);
    tap(&mut shell, start + 500 * MS);
    tap(&mut shell, start + 900 * MS);
    assert_eq!(tempo(&shell), 133.33);
    let history: Vec<_> = shell.editor().undo_history().take(2).collect();
    assert_eq!(history, ["Tap Tempo", "Tap Tempo"]);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(tempo(&shell), 120.0, "the first run's step is whole");
}

#[test]
fn tap_tempo_is_refused_while_the_song_has_tempo_changes() {
    // Spec: control-bar-transport R21
    let (mut shell, _control) = rig();
    let ramp =
        TempoMap::from_breakpoints([(Ticks::ZERO, bpm(100.0)), (BAR * 4, bpm(140.0))]).unwrap();
    shell
        .run("song.set_tempo", json!({ "tempo": ramp }))
        .unwrap();
    let error = shell
        .run("transport.tap_tempo", json!({ "at": 100 }))
        .unwrap_err();
    assert!(
        matches!(&error, ShellError::Unavailable { reason, .. } if reason.contains("tempo changes")),
        "{error}"
    );
}

/// The engine after one block, as the shell reads it.
fn block(control: &NullControl, shell: &mut Shell) -> EngineTransport {
    control.run_blocks(1);
    shell.poll_engine()
}

/// Four taps 500 ms apart from `first`, the insert marker at two bars: where
/// the start waits, as `transport.tap_tempo` reports it.
fn four_taps(shell: &mut Shell, first: u64) -> TapTempo {
    shell
        .run("transport.locate", json!({ "position": BAR * 2 }))
        .unwrap();
    let run: Vec<_> = (0..4).map(|n| tap(shell, first + n * 500 * MS)).collect();
    run[3]
}

#[test]
fn four_taps_start_the_transport_from_the_insert_marker_a_beat_after_the_fourth() {
    // Spec: control-bar-transport R20, at any block size (the null device
    // takes 16 frames at least; the engine's own test runs the start at 1).
    for size in [16, 64, 512] {
        let (mut shell, control) = shell_with_blocks(size);
        let first = 100;
        let fourth = first + 3 * 500 * MS;
        let result = four_taps(&mut shell, first);
        let start = fourth + 500 * MS;
        assert_eq!(result.start_at, Some(start), "block size {size}");
        assert_eq!(state(&mut shell).tap_tempo.start_at, Some(start));
        testing::settle(&control, &mut shell);
        // Run up to the start's block, then block by block across it.
        let mut engine = shell.poll_engine();
        let near = start.saturating_sub(4 * u64::from(size));
        while engine.clock < near {
            let frames = (near - engine.clock).min(2_000);
            control.run_frames(frames);
            engine = shell.poll_engine();
        }
        let tempo = TempoMap::constant(bpm(120.0));
        for _ in 0..8 {
            engine = block(&control, &mut shell);
            // The song plays from the first frame of the start, and not before.
            assert_eq!(engine.playing, engine.clock > start, "{engine:?}");
            if engine.playing {
                let played = Frames((engine.clock - start) as i64);
                let expected = BAR * 2 + tempo.frames_to_ticks(played, SampleRate::HZ_48000);
                assert_eq!(engine.position, expected, "block size {size}");
            }
        }
        shell.audio_mut().stop();
    }
}

#[test]
fn with_the_setting_off_taps_only_set_the_tempo() {
    // Spec: control-bar-transport R20
    let (mut shell, control) = rig();
    shell
        .run(
            "settings.set_start_transport_with_tap_tempo",
            json!({ "on": false }),
        )
        .unwrap();
    let result = four_taps(&mut shell, 100);
    assert_eq!(result.start_at, None);
    testing::advance(&control, &mut shell, 3_000);
    assert!(!shell.poll_engine().playing);
    assert_eq!(tempo(&shell), 120.0);
}

#[test]
fn a_stop_before_the_start_cancels_it_and_later_taps_leave_it_be() {
    // Spec: control-bar-transport R20
    let (mut shell, control) = rig();
    let result = four_taps(&mut shell, 100);
    let start = result.start_at.unwrap();
    // A fifth tap before the start changes the tempo and not the start.
    let fifth = tap(&mut shell, 100 + 4 * 500 * MS - 100 * MS);
    assert_eq!(fifth.start_at, Some(start));
    shell.run("transport.stop", json!({})).unwrap();
    assert_eq!(state(&mut shell).tap_tempo.start_at, None);
    testing::advance(&control, &mut shell, 4_000);
    assert!(!shell.poll_engine().playing, "the start was cancelled");
}

#[test]
fn three_taps_start_the_transport_in_3_4() {
    // Spec: control-bar-transport R20
    let (mut shell, control) = rig();
    shell
        .run(
            "song.set_time_signatures",
            json!({ "time_signatures": bl_core::time::TimeSignatureMap::constant(
                bl_core::time::TimeSignature::new(3, 4).unwrap()) }),
        )
        .unwrap();
    let run: Vec<_> = (0..3)
        .map(|n| tap(&mut shell, 100 + n * 500 * MS))
        .collect();
    assert_eq!(run[1].start_at, None);
    let start = 100 + 2 * 500 * MS + 500 * MS;
    assert_eq!(run[2].start_at, Some(start));
    testing::settle(&control, &mut shell);
    while shell.poll_engine().clock <= start + 1_000 {
        control.run_frames(2_000);
    }
    assert!(shell.poll_engine().playing);
}

#[test]
fn a_song_that_plays_is_not_started_again() {
    // Spec: control-bar-transport R20
    let (mut shell, control) = rig();
    shell.run("transport.play", json!({})).unwrap();
    testing::advance(&control, &mut shell, 50);
    assert!(shell.poll_engine().playing);
    let result = four_taps(&mut shell, 100);
    assert_eq!((result.taps, result.start_at), (4, None));
}

#[test]
fn the_preference_is_on_in_a_new_install_kept_and_read_back() {
    // Spec: control-bar-transport R20, R22
    let dir = std::env::temp_dir().join(format!("bl-shell-tap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let launch = || {
        Shell::new(
            Preferences::load(&dir).unwrap(),
            Some(dir.clone()),
            AudioHost::null(Default::default()),
            Box::new(ProjectFiles::new()),
        )
    };
    let read = |shell: &mut Shell| -> Value {
        shell.run("settings.state", json!({})).unwrap()["start_transport_with_tap_tempo"].clone()
    };
    let mut first = launch();
    assert!(first.start_transport_with_tap_tempo());
    assert_eq!(read(&mut first), json!(true));
    let result = first
        .run(
            "settings.set_start_transport_with_tap_tempo",
            json!({ "on": false }),
        )
        .unwrap();
    assert_eq!(result, json!({ "on": false }));
    assert_eq!(read(&mut first), json!(false));
    drop(first);
    let mut second = launch();
    assert!(
        !second.start_transport_with_tap_tempo(),
        "kept for the next start"
    );
    // Playback ▸ Options switches it to the other state.
    second
        .invoke("settings.set_start_transport_with_tap_tempo", json!({}))
        .unwrap();
    assert_eq!(read(&mut second), json!(true));
    drop(second);
    let _ = std::fs::remove_dir_all(&dir);
}
