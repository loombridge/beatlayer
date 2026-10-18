//! WP-3.12's acceptance with the test backend: with Automation Arm on, a
//! control move during a Session recording lands in the Automation envelope
//! of the clip being recorded, at the right ticks, as part of the recording's
//! undo step; an overdub writes into the clip that plays; Record Session
//! automation in chooses which tracks write; a hand move takes the parameter
//! back from the clip until it launches again. The rules are in the Session
//! automation spec.
//!
//! - A move during a slot recording is a step in the new clip's envelope.
//! - A move during an overdub is in the playing clip, in its own undo step.
//! - Armed Tracks writes the armed tracks' clips; All Tracks every playing
//!   clip.
//! - With Automation Arm off, nothing is recorded.

bl_rt::install_trap!();

use std::time::{Duration, Instant};

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions};
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Clip, EnvelopeKind, MixerParam, SceneId, TrackId};
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

fn volume() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Volume,
    }
}

/// A shell on a null device whose launches and stops take effect at once.
fn rig() -> (Shell, NullControl) {
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            channels: 4,
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
    shell.start_audio();
    shell
        .run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        )
        .unwrap();
    (shell, control)
}

/// A new MIDI track, armed or not.
fn midi_track(shell: &mut Shell, arm: bool) -> TrackId {
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Keys" }))
        .unwrap();
    let track = shell.editor().document().top_level().last().unwrap();
    shell
        .run("mixer.set_arm", json!({ "track": track, "arm": arm }))
        .unwrap();
    track
}

fn scenes(shell: &Shell) -> Vec<SceneId> {
    shell
        .editor()
        .document()
        .scenes()
        .map(|scene| scene.id)
        .collect()
}

fn poll_until(shell: &mut Shell, what: &str, done: impl Fn(&mut Shell) -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        shell.poll_engine();
        if done(shell) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

fn pass_time(shell: &mut Shell, millis: u64) {
    let until = Instant::now() + Duration::from_millis(millis);
    while Instant::now() < until {
        shell.poll_engine();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn slot_clip(shell: &Shell, track: TrackId, scene: SceneId) -> Option<Clip> {
    shell.editor().document().slot(track, scene).cloned()
}

fn recorded(shell: &mut Shell) -> bool {
    shell.running_recording().is_none() && !shell.editor().in_transaction()
}

fn set_volume(shell: &mut Shell, track: TrackId, volume: f64) {
    shell
        .run(
            "mixer.set_volume",
            json!({ "track": track, "volume": volume }),
        )
        .unwrap();
}

fn arm_automation(shell: &mut Shell) {
    shell
        .run("arrangement.automation_arm", json!({ "on": true }))
        .unwrap();
}

/// The points of the clip's Automation envelope for the volume.
fn points(clip: &Clip) -> Vec<(Ticks, f64)> {
    clip.envelope_of(&volume(), EnvelopeKind::Automation)
        .map(|e| e.envelope.points().map(|p| (p.at, p.value)).collect())
        .unwrap_or_default()
}

/// Records a clip over about 400 ms with the volume moved to `value` half
/// way, and returns it.
fn record_with_move(shell: &mut Shell, track: TrackId, scene: SceneId, value: f64) -> Clip {
    shell.run("session.record", json!({})).unwrap();
    poll_until(shell, "the clip to appear", |shell| {
        slot_clip(shell, track, scene).is_some()
    });
    pass_time(shell, 200);
    set_volume(shell, track, value);
    pass_time(shell, 200);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(shell, "the recording to end", recorded);
    slot_clip(shell, track, scene).expect("the recorded clip")
}

// Spec: session-automation R1, R3, R11
#[test]
fn a_move_during_a_slot_recording_is_a_step_in_the_new_clips_envelope() {
    let (mut shell, _control) = rig();
    let keys = midi_track(&mut shell, true);
    let scene = scenes(&shell)[0];
    arm_automation(&mut shell);
    let old = shell
        .editor()
        .document()
        .track(keys)
        .unwrap()
        .mixer()
        .volume;
    let steps = shell.editor().undo_history().len();

    let clip = record_with_move(&mut shell, keys, scene, 0.25);

    // The envelope starts at the value the control had when the take began,
    // holds it until the move, and steps to the new value there.
    let envelope = points(&clip);
    assert_eq!(envelope.len(), 3, "{envelope:?}");
    let (start, hold, step) = (envelope[0], envelope[1], envelope[2]);
    assert_eq!(start, (Ticks::ZERO, old));
    assert_eq!(hold.1, old);
    assert_eq!(step.1, 0.25);
    assert_eq!(
        hold.0,
        step.0 - Ticks(1),
        "a step: one tick before the move"
    );
    assert!(
        step.0 > Ticks::ZERO && step.0 < clip.length,
        "the move is inside the clip: {} of {}",
        step.0.get(),
        clip.length.get()
    );
    let kept = clip
        .envelope_of(&volume(), EnvelopeKind::Automation)
        .unwrap();
    assert!(
        kept.linked && kept.enabled,
        "linked, and driving the volume"
    );
    assert!(clip.envelope(&volume()).is_none(), "no modulation envelope");
    assert_eq!(
        shell.editor().undo_history().len(),
        steps + 1,
        "the clip and its envelope are one undo step"
    );
    shell.run("edit.undo", json!({})).unwrap();
    assert!(slot_clip(&shell, keys, scene).is_none());
    let stats = shell.audio().stats().expect("a stream is open");
    assert!(stats.blocks > 0);
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
}

/// Whether `track`'s Session clip plays, its recording over: what Session
/// Record overdubs.
fn playing(shell: &Shell, track: TrackId) -> bool {
    shell
        .session_states()
        .iter()
        .any(|s| s.track == track && s.state == bl_engine::SlotState::Playing)
}

// Spec: session-automation R1
#[test]
fn with_automation_arm_off_nothing_is_recorded() {
    let (mut shell, _control) = rig();
    let keys = midi_track(&mut shell, true);
    let scene = scenes(&shell)[0];
    let clip = record_with_move(&mut shell, keys, scene, 0.25);
    assert!(points(&clip).is_empty());
}

// Spec: session-automation R2, R4, R11
#[test]
fn a_move_during_an_overdub_goes_into_the_playing_clip_as_a_step_of_its_own() {
    let (mut shell, _control) = rig();
    let keys = midi_track(&mut shell, true);
    let scene = scenes(&shell)[0];
    let map = bl_core::time::TempoMap::constant(bl_core::time::Bpm::new(240.0).unwrap());
    shell
        .run("song.set_tempo", json!({ "tempo": map }))
        .unwrap();
    // A clip with no envelope that loops after its recording.
    shell.run("session.record", json!({})).unwrap();
    pass_time(&mut shell, 300);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&mut shell, "the recording to end", recorded);
    poll_until(&mut shell, "the clip to play", |shell| playing(shell, keys));
    arm_automation(&mut shell);
    let clips = shell
        .editor()
        .document()
        .track(keys)
        .unwrap()
        .slots()
        .count();
    let steps = shell.editor().undo_history().len();

    // Session Record over the playing clip overdubs it: no new clip, and the
    // move goes into the clip that plays.
    shell.run("session.record", json!({})).unwrap();
    pass_time(&mut shell, 200);
    set_volume(&mut shell, keys, 0.25);
    pass_time(&mut shell, 200);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&mut shell, "the overdub to end", recorded);
    let track = shell.editor().document().track(keys).unwrap();
    assert_eq!(track.slots().count(), clips, "no new clip");
    let clip = slot_clip(&shell, keys, scene).unwrap();
    let first = points(&clip);
    assert_eq!(first.last().map(|p| p.1), Some(0.25), "{first:?}");
    assert!(
        shell.editor().undo_history().next() == Some("Overdub Record"),
        "the step carries Live's name"
    );
    assert_eq!(
        shell.editor().undo_history().len(),
        steps + 1,
        "the move and the envelope are one step"
    );

    // A second take replaces the envelope from its move to the end of the
    // pass and leaves what came before.
    poll_until(&mut shell, "the clip to play again", |shell| {
        playing(shell, keys)
    });
    shell.run("session.record", json!({})).unwrap();
    pass_time(&mut shell, 300);
    set_volume(&mut shell, keys, 0.1);
    pass_time(&mut shell, 100);
    shell.run("session.stop_recording", json!({})).unwrap();
    poll_until(&mut shell, "the second take to end", recorded);
    let clip = slot_clip(&shell, keys, scene).unwrap();
    let second = points(&clip);
    assert_eq!(second[0], first[0], "the start stays");
    assert_eq!(second.last().map(|p| p.1), Some(0.1), "{second:?}");
}

// Spec: session-automation R5
#[test]
fn record_session_automation_in_chooses_which_tracks_write() {
    for (setting, writes) in [("armed_tracks", false), ("all_tracks", true)] {
        let (mut shell, _control) = rig();
        let armed = midi_track(&mut shell, true);
        let idle = midi_track(&mut shell, false);
        let scene = scenes(&shell)[0];
        shell
            .run(
                "settings.set_record_session_automation_in",
                json!({ "tracks": setting }),
            )
            .unwrap();
        let state = shell.run("settings.state", json!({})).unwrap();
        assert_eq!(state["record_session_automation_in"], json!(setting));
        arm_automation(&mut shell);
        // The idle track has a clip that plays.
        shell
            .run(
                "session.insert_empty_midi_clip",
                json!({ "track": idle, "scene": scene }),
            )
            .unwrap();
        shell
            .run("session.launch", json!({ "track": idle, "scene": scene }))
            .unwrap();
        poll_until(&mut shell, "the clip to play", |shell| {
            shell
                .session_states()
                .iter()
                .any(|s| s.track == idle && s.state == bl_engine::SlotState::Playing)
        });
        record_with_move(&mut shell, armed, scene, 0.25);
        // Moves of the idle track's volume were made after the armed one's;
        // make one in a second take.
        shell.run("session.record", json!({})).unwrap();
        pass_time(&mut shell, 200);
        set_volume(&mut shell, idle, 0.4);
        pass_time(&mut shell, 100);
        shell.run("session.stop_recording", json!({})).unwrap();
        poll_until(&mut shell, "the take to end", recorded);
        let clip = slot_clip(&shell, idle, scene).unwrap();
        assert_eq!(
            !points(&clip).is_empty(),
            writes,
            "{setting}: {:?}",
            points(&clip)
        );
    }
}

// Spec: session-automation R9
#[test]
fn a_hand_move_takes_the_parameter_back_until_the_clip_launches_again() {
    let (mut shell, _control) = rig();
    let keys = midi_track(&mut shell, true);
    let scene = scenes(&shell)[0];
    arm_automation(&mut shell);
    record_with_move(&mut shell, keys, scene, 0.25);
    poll_until(&mut shell, "the clip to play", |shell| playing(shell, keys));
    let enabled = |shell: &Shell| {
        slot_clip(shell, keys, scene)
            .unwrap()
            .envelope_of(&volume(), EnvelopeKind::Automation)
            .unwrap()
            .enabled
    };
    assert!(enabled(&shell));
    // A hand move turns the clip's envelope off, and lights Re-Enable
    // Automation.
    set_volume(&mut shell, keys, 0.9);
    assert!(!enabled(&shell));
    assert!(
        shell
            .editor()
            .document()
            .track(keys)
            .unwrap()
            .automation_overridden()
    );
    // Launching the clip gives it back.
    shell
        .run("session.launch", json!({ "track": keys, "scene": scene }))
        .unwrap();
    assert!(enabled(&shell));
    assert!(
        !shell
            .editor()
            .document()
            .track(keys)
            .unwrap()
            .automation_overridden()
    );
}
