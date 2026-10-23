//! WP-3.13's acceptance with the test backend: Session clips launched while
//! Arrangement Record runs land in the Arrangement where they played, the
//! pass is one undo step, the Session clips keep playing when the recording
//! ends, and the commands run by ID. The rules are in the Session to
//! Arrangement spec.
//!
//! - A clip playing when the recording starts and a clip launched during it
//!   each get an Arrangement clip on their track.
//! - A track's clip stop ends its copy.
//! - The pass is one undo step named "Record", and Undo removes every copy.
//! - Ending the recording leaves Back to Arrangement lit.
//! - The whole path allocates nothing on the audio thread.

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Clip, ClipData, MixerParam, SceneId, TrackId};
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::testing;
use serde_json::json;

/// A shell on a null device, with launches taking effect at once.
fn rig() -> (Shell, NullControl) {
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            dropout_after: None,
            channels: 4,
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
        .run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        )
        .unwrap();
    (shell, control)
}

/// A new MIDI track with a clip in the first scene's slot.
fn track_with_clip(shell: &mut Shell, name: &str) -> TrackId {
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": name }))
        .unwrap();
    let track = shell.editor().document().top_level().last().unwrap();
    let scene = first_scene(shell);
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    track
}

fn first_scene(shell: &Shell) -> SceneId {
    shell.editor().document().scenes().next().unwrap().id
}

/// Advances the set by `millis` of test time, so the recording's positions
/// land on their frames however busy the machine is (T199).
fn pass_time(control: &NullControl, shell: &mut Shell, millis: u64) {
    testing::advance(control, shell, millis);
}

/// Advances the set until `done` holds, up to about ten seconds of test time.
fn poll_until(
    control: &NullControl,
    shell: &mut Shell,
    what: &str,
    done: impl Fn(&mut Shell) -> bool,
) {
    testing::wait_for(control, shell, what, done);
}

fn arrangement(shell: &Shell, track: TrackId) -> Vec<Clip> {
    let mut clips: Vec<Clip> = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .cloned()
        .collect();
    clips.sort_by_key(|clip| clip.start);
    clips
}

// Spec: session-to-arrangement R1, R2, R5, R6, R7. A clip playing when the
// recording starts gets a copy from there, a clip launched during it gets
// one from its launch, a clip stop ends its copy, and the transport's stop
// ends the rest; the pass is one undo step, the copies replace what lay
// under them, and the tracks stay taken over.
#[test]
fn session_clips_launched_during_arrangement_record_land_in_the_arrangement() {
    let (mut shell, control) = rig();
    let first = track_with_clip(&mut shell, "First");
    let second = track_with_clip(&mut shell, "Second");
    let scene = first_scene(&shell);
    let slot = |track: TrackId| json!({ "track": track, "scene": scene });
    shell.run("session.launch", slot(first)).unwrap();
    pass_time(&control, &mut shell, 100);
    let steps = shell.editor().undo_history().len();

    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    poll_until(&control, &mut shell, "the take to begin", |shell| {
        shell.run("transport.state", json!({})).unwrap()["take_running"] == true
    });
    pass_time(&control, &mut shell, 100);
    shell.run("session.launch", slot(second)).unwrap();
    pass_time(&control, &mut shell, 50);
    // Automation Arm is off, and the Session clip has taken the track over.
    let volume = |value: f64| json!({ "track": second, "volume": value });
    shell.run("mixer.set_volume", volume(0.25)).unwrap();
    pass_time(&control, &mut shell, 50);
    shell.run("mixer.set_volume", volume(0.75)).unwrap();
    pass_time(&control, &mut shell, 50);
    shell
        .run("session.stop_track", json!({ "track": second }))
        .unwrap();
    pass_time(&control, &mut shell, 100);
    shell.run("transport.stop", json!({})).unwrap();
    poll_until(&control, &mut shell, "the copies", |shell| {
        shell.running_recording().is_none() && !shell.editor().in_transaction()
    });

    let moves = shell
        .editor()
        .document()
        .track(second)
        .unwrap()
        .automation_for(&AutomationTarget::Mixer {
            param: MixerParam::Volume,
        })
        .map(|automation| automation.envelope.points().count());
    assert_eq!(moves, Some(2));
    let (a, b) = (arrangement(&shell, first), arrangement(&shell, second));
    assert_eq!((a.len(), b.len()), (1, 1), "{a:?} {b:?}");
    assert!(matches!(a[0].data, ClipData::Midi(_)));
    assert!(matches!(b[0].data, ClipData::Midi(_)));
    assert!(a[0].length > Ticks::ZERO && b[0].length > Ticks::ZERO);
    assert!(
        a[0].start < b[0].start,
        "the clip playing at the start is copied from the start, the launched one from its launch"
    );
    assert!(
        b[0].end() <= a[0].end(),
        "the stopped clip's copy ends before the transport's stop"
    );
    let count = |shell: &Shell, label: &str| {
        shell
            .editor()
            .undo_history()
            .filter(|step| *step == label)
            .count()
    };
    assert_eq!(count(&shell, "Record"), 1, "one step for the pass");
    assert_eq!(
        count(&shell, "Record Automation"),
        1,
        "the moves are written too, with Automation Arm off"
    );
    assert!(shell.editor().undo_history().len() > steps);
    assert_eq!(
        shell.session_over().len(),
        2,
        "Back to Arrangement stays lit"
    );

    while count(&shell, "Record") > 0 {
        shell.run("edit.undo", json!({})).unwrap();
    }
    assert!(arrangement(&shell, first).is_empty());
    assert!(arrangement(&shell, second).is_empty());
    while count(&shell, "Record") == 0 {
        shell.run("edit.redo", json!({})).unwrap();
    }
    assert_eq!(arrangement(&shell, first).len(), 1);

    let stats = shell.audio().stats().expect("a stream is open");
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}
