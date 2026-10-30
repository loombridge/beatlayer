//! MIDI and key mappings in the shell (WP-7.1; `docs/specs/remote-mapping.md`,
//! R1 to R14): mappings are made, changed and deleted by command, the remote
//! input applies the messages they take, and a mapped control moves as a hand
//! move does. Everything runs against the in-process test backend and the
//! null device paced by hand, so every message lands on the test clock.

use bl_audio_io::NullControl;
use bl_core::clock::TestClock;
use bl_midi_io::{Direction, TestBackend};
use bl_model::{LaunchMode, SceneId, TrackId};
use bl_shell::{Shell, testing};
use serde_json::{Value, json};

struct Rig {
    shell: Shell,
    control: NullControl,
    test: TestBackend,
    clock: TestClock,
}

impl Rig {
    fn new() -> Self {
        let (mut shell, control) = testing::rig();
        let test = TestBackend::new();
        for name in ["Pads", "Keys"] {
            test.add_port(Direction::Input, name);
        }
        shell.use_midi_backend(Box::new(test.clone()));
        let clock = control.clock().expect("manual pacing");
        shell.use_test_clock(clock.clone());
        shell.start_audio();
        let mut rig = Self {
            shell,
            control,
            test,
            clock,
        };
        rig.run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        );
        rig
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    /// Switches a port: Remote on, Track as given.
    fn remote_port(&mut self, name: &str, track: bool) {
        self.run(
            "midi.set_port",
            json!({ "direction": "input", "name": name, "remote": true, "track": track }),
        );
    }

    fn track(&mut self, kind: &str) -> TrackId {
        self.shell
            .invoke("track.create", json!({ "kind": kind, "name": "T" }))
            .unwrap();
        self.shell.editor().document().top_level().last().unwrap()
    }

    fn scene(&self) -> SceneId {
        self.shell.editor().document().scenes().next().unwrap().id
    }

    fn map(&mut self, source: Value, target: Value) -> Value {
        self.run("remote.map", json!({ "source": source, "target": target }))
    }

    /// Sends a message from a port and applies what crossed.
    fn send(&mut self, port: &str, bytes: &[u8]) {
        assert!(self.test.send(port, self.clock.now(), bytes));
        self.shell.poll();
    }

    fn pass(&mut self, millis: u64) {
        testing::advance(&self.control, &mut self.shell, millis);
    }

    fn volume(&self, track: TrackId) -> f64 {
        self.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .mixer()
            .volume
    }

    /// Where the track's fader sits on its travel, from 0 to 1.
    fn position(&self, track: TrackId) -> f64 {
        bl_model::fader::fader_position(self.volume(track))
    }

    /// A move of the mouse: the fader goes to `position`.
    fn hand(&mut self, track: TrackId, position: f64) {
        self.run(
            "mixer.set_volume",
            json!({ "track": track, "volume": bl_model::fader::fader_gain(position) }),
        );
    }

    fn takeover(&mut self, mode: &str) {
        self.run("settings.set_takeover_mode", json!({ "mode": mode }));
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().count()
    }

    fn mappings(&mut self) -> Vec<Value> {
        self.run("remote.mappings", json!({}))["mappings"]
            .as_array()
            .unwrap()
            .clone()
    }
}

fn cc(controller: u8) -> Value {
    json!({ "type": "cc", "channel": 1, "controller": controller })
}

fn note(key: u8) -> Value {
    json!({ "type": "note", "channel": 1, "key": key })
}

fn volume_of(track: TrackId) -> Value {
    json!({ "type": "mixer", "track": track, "param": "volume" })
}

// Spec: remote-mapping R7, R8, R11. A controller mapped to a volume moves it
// as a hand move does, and a message a mapping takes never reaches a track.
#[test]
fn a_mapped_controller_moves_the_volume_and_stays_away_from_tracks() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", true);
    let track = rig.track("midi");
    rig.map(cc(20), volume_of(track));
    let stats = rig.shell.midi().sender().stats().clone();

    rig.send("Pads", &[0xB0, 20, 127]);
    let top = bl_model::fader::fader_gain(1.0);
    assert!(
        (rig.volume(track) - top).abs() < 1e-9,
        "{}",
        rig.volume(track)
    );
    assert_eq!(
        stats.snapshot().to_tracks,
        0,
        "the mapped controller stayed back"
    );

    rig.send("Pads", &[0xB0, 21, 127]);
    assert_eq!(stats.snapshot().to_tracks, 1, "an unmapped one goes on");
    assert!((rig.volume(track) - top).abs() < 1e-9);
}

// Spec: remote-mapping R8. A Remote-only input's unmapped note reaches no
// track and is counted.
#[test]
fn a_remote_only_inputs_unmapped_note_reaches_no_track_and_is_counted() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", false);
    rig.send("Pads", &[0x90, 60, 100]);
    let counts = rig.shell.midi().sender().stats().snapshot();
    assert_eq!((counts.to_tracks, counts.remote_unmapped), (0, 1));
}

// Spec: remote-mapping R7. Every Remote input feeds one remote input.
#[test]
fn two_remote_inputs_drive_one_mapping() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", false);
    rig.remote_port("Keys", false);
    let track = rig.track("audio");
    rig.map(cc(20), volume_of(track));
    rig.send("Pads", &[0xB0, 20, 127]);
    let high = rig.volume(track);
    rig.send("Keys", &[0xB0, 20, 0]);
    assert!(rig.volume(track) < high && rig.volume(track) == 0.0);
    rig.send("Pads", &[0xB0, 20, 127]);
    assert_eq!(rig.volume(track), high);
}

// Spec: remote-mapping R12. A run of controller messages is one undo step; a
// pause of a second, or a message to another mapping, starts another.
#[test]
fn a_run_of_controller_messages_is_one_undo_step_and_a_pause_starts_another() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", false);
    let (a, b) = (rig.track("audio"), rig.track("audio"));
    rig.map(cc(20), volume_of(a));
    rig.map(cc(21), volume_of(b));
    let before = rig.steps();

    for value in [20, 40, 60, 80] {
        rig.send("Pads", &[0xB0, 20, value]);
        rig.pass(100);
    }
    rig.pass(1_000);
    rig.shell.poll();
    assert_eq!(rig.steps(), before + 1, "the run is one step");
    let first = rig.volume(a);

    rig.send("Pads", &[0xB0, 20, 100]);
    rig.pass(100);
    rig.send("Pads", &[0xB0, 21, 100]);
    rig.pass(1_000);
    rig.shell.poll();
    assert_eq!(rig.steps(), before + 3, "another mapping ends the run");

    rig.run("edit.undo", json!({}));
    assert_eq!(rig.volume(b), 1.0, "the second mapping's step is undone");
    rig.run("edit.undo", json!({}));
    assert_eq!(rig.volume(a), first, "the next run's step is undone");
}

// Spec: remote-mapping R12. Another edit ends a run too.
#[test]
fn another_edit_ends_the_run() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", false);
    let track = rig.track("audio");
    rig.map(cc(20), volume_of(track));
    let before = rig.steps();
    rig.send("Pads", &[0xB0, 20, 50]);
    rig.run("mixer.set_pan", json!({ "track": track, "pan": 0.5 }));
    rig.send("Pads", &[0xB0, 20, 70]);
    rig.pass(1_000);
    rig.shell.poll();
    assert_eq!(rig.steps(), before + 3);
}

// Spec: remote-mapping R11, R15, launch-quantisation R18. A mapped note
// launches a Gate clip when it goes down and stops it when it comes up.
#[test]
fn a_mapped_note_launches_and_releases_a_gate_clip() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", false);
    let track = rig.track("midi");
    let scene = rig.scene();
    rig.run(
        "session.insert_empty_midi_clip",
        json!({ "track": track, "scene": scene }),
    );
    let clip = rig.shell.editor().document().slot(track, scene).unwrap().id;
    rig.run(
        "clip.set_launch_mode",
        json!({ "clip": clip, "mode": LaunchMode::Gate }),
    );
    rig.map(
        note(36),
        json!({ "type": "slot", "track": track, "scene": scene }),
    );
    let playing = |shell: &Shell| {
        shell
            .session_states()
            .iter()
            .any(|s| s.track == track && s.state == bl_engine::SlotState::Playing)
    };

    rig.send("Pads", &[0x90, 36, 100]);
    testing::wait_for(&rig.control, &mut rig.shell, "the clip to play", |shell| {
        playing(shell)
    });
    let steps = rig.steps();
    rig.send("Pads", &[0x80, 36, 0]);
    testing::wait_for(&rig.control, &mut rig.shell, "the clip to stop", |shell| {
        !playing(shell)
    });
    assert_eq!(rig.steps(), steps, "launches are no undo step");
}

// Spec: remote-mapping R13. A mapped key toggles a switch on its press and
// does nothing on its release; no mapping acts while a map mode is on.
#[test]
fn a_mapped_key_toggles_a_switch_and_no_mapping_acts_in_a_map_mode() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    rig.map(
        json!({ "type": "key", "key": "S" }),
        json!({ "type": "solo", "track": track }),
    );
    let solo = |rig: &Rig| {
        rig.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .mixer()
            .solo
    };
    assert!(rig.shell.key_is_mapped("S") && !rig.shell.key_is_mapped("A"));
    rig.run("remote.input", json!({ "key": "S" }));
    assert!(solo(&rig), "pressed and released once: on");
    rig.run("remote.input", json!({ "key": "S", "pressed": true }));
    rig.run("remote.input", json!({ "key": "S", "pressed": false }));
    assert!(!solo(&rig));

    rig.run("remote.map_mode", json!({ "mode": "midi" }));
    assert!(!rig.shell.key_is_mapped("S"));
    let result = rig.run("remote.input", json!({ "key": "S" }));
    assert_eq!(result["taken"], false);
    assert!(!solo(&rig), "no mapping acts while a map mode is on");

    rig.remote_port("Pads", true);
    rig.map(note(40), json!({ "type": "arm", "track": track }));
    let applied = rig.shell.remote_counts().0;
    rig.send("Pads", &[0x90, 40, 100]);
    assert_eq!(
        rig.shell.remote_counts().0,
        applied,
        "the mapping didn't act"
    );
    assert!(
        !rig.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .mixer()
            .arm,
        "and armed nothing"
    );
}

// Spec: remote-mapping R11. A control that refuses its command is counted.
#[test]
fn a_refused_control_changes_nothing_and_is_counted() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", false);
    let group = rig.track("group");
    rig.map(note(40), json!({ "type": "arm", "track": group }));
    rig.send("Pads", &[0x90, 40, 100]);
    assert_eq!(rig.shell.remote_counts(), (0, 1));

    // A button's release is nothing to do, not a refusal.
    let scene = rig.scene();
    rig.map(note(41), json!({ "type": "scene_launch", "scene": scene }));
    rig.send("Pads", &[0x90, 41, 100]);
    rig.send("Pads", &[0x80, 41, 0]);
    assert_eq!(rig.shell.remote_counts(), (1, 1));
}

// Spec: remote-mapping R6. Deleting a track takes its mappings in the same
// undo step; a copy of a track takes none.
#[test]
fn deleting_a_track_takes_its_mappings_and_a_copy_has_none() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    rig.map(note(40), json!({ "type": "solo", "track": track }));
    rig.run("track.duplicate", json!({ "tracks": [track] }));
    assert_eq!(rig.mappings().len(), 1, "the copy took no mapping");
    rig.run("track.delete", json!({ "track": track }));
    assert!(rig.mappings().is_empty());
    rig.run("edit.undo", json!({}));
    assert_eq!(rig.mappings().len(), 1);
}

// Spec: remote-mapping R5, R14. Mappings are made, changed and deleted by
// command and read back through `remote.mappings`.
#[test]
fn the_mapping_commands_read_back_through_remote_mappings() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    let before = rig.steps();
    let made = rig.map(cc(20), volume_of(track));
    assert_eq!(made["kind"], json!({ "type": "continuous" }));
    assert_eq!(made["min"], 0.0);
    assert_eq!(rig.steps(), before + 1, "making a mapping is one step");

    // A source maps one control: mapping CC 20 to pan moves it.
    rig.map(
        cc(20),
        json!({ "type": "mixer", "track": track, "param": "pan" }),
    );
    let listed = rig.mappings();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["target"]["param"], "pan");
    assert_eq!(listed[0]["min"], -1.0);
    assert_eq!(listed[0]["name"], "Pan");

    rig.run(
        "remote.set_range",
        json!({ "source": cc(20), "min": -0.5, "max": 0.5 }),
    );
    rig.run("remote.invert_range", json!({ "source": cc(20) }));
    rig.run(
        "remote.set_mode",
        json!({ "source": cc(20), "mode": { "type": "relative", "encoding": "bin_offset", "accelerated": false } }),
    );
    let listed = rig.mappings();
    assert_eq!(
        (listed[0]["min"].as_f64(), listed[0]["max"].as_f64()),
        (Some(0.5), Some(-0.5))
    );
    assert_eq!(listed[0]["mode"]["encoding"], "bin_offset");

    // The agent can try it without a controller.
    rig.run(
        "remote.set_mode",
        json!({ "source": cc(20), "mode": { "type": "absolute" } }),
    );
    let taken = rig.run("remote.input", json!({ "bytes": [0xB0, 20, 127] }));
    assert_eq!(taken["taken"], true);
    let pan = rig
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .mixer()
        .pan;
    assert!(
        (pan - -0.5).abs() < 1e-9,
        "Min above Max turns it round: {pan}"
    );

    let removed = rig.run(
        "remote.unmap",
        json!({ "target": { "type": "mixer", "track": track, "param": "pan" } }),
    );
    assert_eq!(removed["removed"], 1);
    assert!(rig.mappings().is_empty());
    assert_eq!(
        rig.run("remote.unmap", json!({ "source": cc(20) }))["removed"],
        0
    );

    // A control the set lacks, and a mapping that breaks a rule, are refused.
    let missing = json!({ "type": "solo", "track": 987_654 });
    assert!(
        rig.shell
            .run(
                "remote.map",
                json!({ "source": note(1), "target": missing })
            )
            .is_err()
    );
    assert!(
        rig.shell
            .run(
                "remote.set_range",
                json!({ "source": cc(99), "min": 0, "max": 1 })
            )
            .is_err()
    );
}

fn near(value: f64, want: f64) {
    assert!((value - want).abs() < 1e-6, "{value} vs {want}");
}

// Spec: remote-mapping R28, R29. With None a fader that the mouse moved jumps
// to the controller on its next message.
#[test]
fn takeover_none_jumps_a_fader_the_mouse_moved() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", false);
    let track = rig.track("audio");
    rig.map(cc(20), volume_of(track));
    rig.send("Pads", &[0xB0, 20, 100]);
    near(rig.position(track), 100.0 / 127.0);
    rig.hand(track, 0.2);
    rig.send("Pads", &[0xB0, 20, 101]);
    near(rig.position(track), 101.0 / 127.0);
}

// Spec: remote-mapping R29, R30. Pick-Up waits until the controller crosses
// the fader, follows from then, and waits again after the mouse moves it.
#[test]
fn pick_up_waits_for_the_controller_to_cross_the_fader_and_waits_again_after_a_mouse_move() {
    let mut rig = Rig::new();
    rig.takeover("pick_up");
    rig.remote_port("Pads", false);
    let track = rig.track("audio");
    rig.map(cc(20), volume_of(track));
    rig.hand(track, 0.5);
    for value in [10, 40, 63] {
        rig.send("Pads", &[0xB0, 20, value]);
        near(rig.position(track), 0.5);
    }
    rig.send("Pads", &[0xB0, 20, 64]);
    near(rig.position(track), 64.0 / 127.0);
    rig.send("Pads", &[0xB0, 20, 90]);
    near(rig.position(track), 90.0 / 127.0);

    rig.hand(track, 0.1);
    for value in [100, 20] {
        rig.send("Pads", &[0xB0, 20, value]);
        near(rig.position(track), 0.1);
    }
    rig.send("Pads", &[0xB0, 20, 5]);
    near(rig.position(track), 5.0 / 127.0);
}

// Spec: remote-mapping R29 to R31. Value Scaling brings the fader and the
// controller to the top together.
#[test]
fn value_scaling_brings_the_fader_and_the_controller_to_the_top_together() {
    let mut rig = Rig::new();
    rig.takeover("value_scaling");
    rig.remote_port("Pads", false);
    let track = rig.track("audio");
    rig.map(cc(20), volume_of(track));
    rig.hand(track, 0.8);
    rig.send("Pads", &[0xB0, 20, 20]);
    near(rig.position(track), 0.8);
    let (c, c2) = (20.0 / 127.0, 40.0 / 127.0);
    rig.send("Pads", &[0xB0, 20, 40]);
    near(
        rig.position(track),
        0.8 + (c2 - c) * (1.0 - 0.8) / (1.0 - c),
    );
    let mut last = rig.position(track);
    for value in [60, 80, 100] {
        rig.send("Pads", &[0xB0, 20, value]);
        assert!(rig.position(track) > last, "the fader climbs with it");
        last = rig.position(track);
    }
    // Within one step of each other, the fader follows as None does.
    rig.send("Pads", &[0xB0, 20, 120]);
    near(rig.position(track), 120.0 / 127.0);
    rig.send("Pads", &[0xB0, 20, 127]);
    near(rig.position(track), 1.0);
    // Together, it follows as None does.
    rig.send("Pads", &[0xB0, 20, 126]);
    near(rig.position(track), 126.0 / 127.0);
}

// Spec: remote-mapping R23 to R26, R28. A relative encoder moves a fader the
// same in every takeover mode: by its size when accelerated, by one step when
// linear.
#[test]
fn a_relative_encoder_moves_a_fader_in_every_takeover_mode() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", false);
    let track = rig.track("audio");
    rig.map(cc(21), volume_of(track));
    for (mode, accelerated, up, step) in [
        ("none", true, 67u8, 3.0),
        ("pick_up", true, 67, 3.0),
        ("value_scaling", true, 67, 3.0),
        ("none", false, 67, 1.0),
        ("pick_up", false, 67, 1.0),
        ("value_scaling", false, 67, 1.0),
    ] {
        rig.takeover(mode);
        rig.run(
            "remote.set_mode",
            json!({ "source": cc(21), "mode": {
                "type": "relative", "encoding": "bin_offset", "accelerated": accelerated } }),
        );
        rig.hand(track, 0.5);
        rig.send("Pads", &[0xB0, 21, up]);
        near(rig.position(track), 0.5 + step / 127.0);
        rig.send("Pads", &[0xB0, 21, 128 - up]);
        near(rig.position(track), 0.5);
    }
    // The ends hold.
    rig.hand(track, 1.0);
    rig.send("Pads", &[0xB0, 21, 127]);
    near(rig.position(track), 1.0);
}

// Spec: remote-mapping R16, R37. A mapped note launches its clip with its own
// velocity; a velocity outside 1 to 127 is refused.
#[test]
fn a_mapped_note_launches_at_its_velocity_and_session_launch_checks_it() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", false);
    let track = rig.track("midi");
    let scene = rig.scene();
    rig.run(
        "session.insert_empty_midi_clip",
        json!({ "track": track, "scene": scene }),
    );
    rig.map(
        note(36),
        json!({ "type": "slot", "track": track, "scene": scene }),
    );
    rig.send("Pads", &[0x90, 36, 1]);
    let playing = |shell: &Shell| {
        shell
            .session_states()
            .iter()
            .any(|s| s.track == track && s.state == bl_engine::SlotState::Playing)
    };
    testing::wait_for(&rig.control, &mut rig.shell, "the clip to play", |shell| {
        playing(shell)
    });
    assert_eq!(rig.shell.remote_counts(), (1, 0));
    for velocity in [0, 128] {
        assert!(
            rig.shell
                .run(
                    "session.launch",
                    json!({ "track": track, "scene": scene, "velocity": velocity })
                )
                .is_err()
        );
    }
    rig.run(
        "session.launch",
        json!({ "track": track, "scene": scene, "velocity": 64 }),
    );
}

// Spec: remote-mapping R33 to R37. A slot mapped to C2 to C4 with root C3
// launches on E3 four semitones up at E3's velocity and the root as it is; a
// Gate clip stops on the release of the latest key, a release of an earlier
// key does nothing, and the clip in the set doesn't change.
#[test]
fn a_chromatic_slot_launches_transposed_and_only_the_latest_keys_release_stops_it() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", false);
    let track = rig.track("midi");
    let scene = rig.scene();
    rig.run(
        "session.insert_empty_midi_clip",
        json!({ "track": track, "scene": scene }),
    );
    let clip = rig.shell.editor().document().slot(track, scene).unwrap().id;
    rig.run(
        "clip.set_launch_mode",
        json!({ "clip": clip, "mode": LaunchMode::Gate }),
    );
    // Live names middle C C3: C2 is 48, C3 60 and C4 72.
    rig.map(
        json!({ "type": "note_range", "channel": 1, "low": 48, "high": 72, "root": 60 }),
        json!({ "type": "slot", "track": track, "scene": scene }),
    );
    let before = rig.shell.editor().document().clone();
    let slot = |shell: &Shell| {
        shell
            .session_states()
            .iter()
            .find(|s| s.track == track && s.state == bl_engine::SlotState::Playing)
            .map(|s| (s.velocity, s.transpose))
    };
    let launched = |rig: &mut Rig, what: &str, want| {
        testing::wait_for(&rig.control, &mut rig.shell, what, |shell| {
            slot(shell) == Some(want)
        });
    };

    rig.send("Pads", &[0x90, 64, 90]);
    launched(&mut rig, "E3 to launch four up at 90", (90, 4));
    rig.send("Pads", &[0x90, 67, 100]);
    launched(&mut rig, "G3 to relaunch seven up at 100", (100, 7));
    // E3 is released after G3 was pressed: nothing happens.
    rig.send("Pads", &[0x80, 64, 0]);
    testing::advance(&rig.control, &mut rig.shell, 100);
    assert_eq!(slot(&rig.shell), Some((100, 7)));
    rig.send("Pads", &[0x80, 67, 0]);
    testing::wait_for(
        &rig.control,
        &mut rig.shell,
        "G3's release to stop it",
        |shell| slot(shell).is_none(),
    );
    // The root plays the clip as it is.
    rig.send("Pads", &[0x90, 60, 127]);
    launched(&mut rig, "C3 to launch untransposed", (127, 0));
    assert_eq!(
        rig.shell.remote_counts(),
        (4, 0),
        "E3's release isn't a message to act on"
    );
    assert_eq!(
        rig.shell.editor().document(),
        &before,
        "nothing in the set changed"
    );
}

// Spec: remote-mapping R29. A mapping that is changed forgets where its
// controller was.
#[test]
fn changing_a_mapping_starts_takeover_afresh() {
    let mut rig = Rig::new();
    rig.takeover("pick_up");
    rig.remote_port("Pads", false);
    let track = rig.track("audio");
    rig.map(cc(20), volume_of(track));
    rig.hand(track, 0.5);
    rig.send("Pads", &[0xB0, 20, 63]);
    rig.send("Pads", &[0xB0, 20, 64]);
    near(rig.position(track), 64.0 / 127.0);
    rig.run(
        "remote.set_range",
        json!({ "source": cc(20), "min": 0.0, "max": 1.0 }),
    );
    rig.hand(track, 0.9);
    rig.send("Pads", &[0xB0, 20, 100]);
    near(rig.position(track), 0.9);
}

fn state(rig: &mut Rig) -> Value {
    rig.run("remote.state", json!({}))
}

fn select(rig: &mut Rig, target: Value) {
    rig.run("remote.select", json!({ "target": target }));
}

fn mode_of(rig: &mut Rig, source: &Value) -> Value {
    rig.mappings()
        .iter()
        .find(|m| &m["source"] == source)
        .map_or(Value::Null, |m| m["mode"].clone())
}

// Spec: map-modes R2, R4. A map mode is view state: no undo step, one at a
// time, and a control is selected by its ID while one is on.
#[test]
fn a_map_mode_is_view_state_one_at_a_time_and_selects_by_id() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    let steps = rig.steps();
    assert_eq!(state(&mut rig)["mode"], "off");
    assert!(
        rig.shell
            .run("remote.select", json!({ "target": volume_of(track) }))
            .is_err()
    );

    rig.run("remote.map_mode", json!({ "mode": "midi" }));
    assert_eq!(state(&mut rig)["mode"], "midi");
    rig.run("remote.map_mode", json!({ "mode": "midi", "toggle": true }));
    assert_eq!(state(&mut rig)["mode"], "off");
    rig.run("remote.map_mode", json!({ "mode": "midi", "toggle": true }));
    rig.run("remote.map_mode", json!({ "mode": "key", "toggle": true }));
    assert_eq!(
        state(&mut rig)["mode"],
        "key",
        "turning one on turns the other off"
    );

    select(&mut rig, json!({ "type": "solo", "track": track }));
    assert_eq!(state(&mut rig)["selected"]["type"], "solo");
    assert!(
        rig.shell
            .run("remote.select", json!({ "target": volume_of(track) }))
            .is_err(),
        "Key Map Mode takes no continuous control"
    );
    rig.run("remote.map_mode", json!({ "mode": "midi" }));
    assert!(
        state(&mut rig)["selected"].is_null(),
        "a new mode starts with no selection"
    );
    select(&mut rig, volume_of(track));
    rig.run("remote.select", json!({}));
    assert!(state(&mut rig)["selected"].is_null());
    assert_eq!(rig.steps(), steps, "view state is no undo step");
}

// Spec: map-modes R4. While a map mode is on no mapping acts, and leaving it
// lets them act.
#[test]
fn no_mapping_acts_in_a_map_mode_and_leaving_it_lets_them() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", false);
    let track = rig.track("audio");
    rig.map(cc(20), volume_of(track));
    rig.run("remote.map_mode", json!({ "mode": "midi" }));
    let before = rig.volume(track);
    rig.send("Pads", &[0xB0, 20, 127]);
    assert_eq!(rig.volume(track), before);
    rig.run("remote.map_mode", json!({ "mode": "off" }));
    rig.send("Pads", &[0xB0, 20, 127]);
    assert_ne!(rig.volume(track), before);
}

// Spec: map-modes R5, R10. The next controller maps the selected control and
// stays selected; its mode is guessed from the messages after it, and the
// whole learn is one undo step.
#[test]
fn a_controller_maps_the_selected_control_and_each_message_refines_the_guess() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", true);
    let track = rig.track("audio");
    rig.run("remote.map_mode", json!({ "mode": "midi" }));
    select(&mut rig, volume_of(track));
    let (steps, before) = (rig.steps(), rig.volume(track));

    rig.send("Pads", &[0xB0, 30, 127]);
    let listed = rig.mappings();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["source"], cc(30));
    assert_eq!(listed[0]["target"], volume_of(track));
    assert_eq!(listed[0]["mode"]["type"], "absolute");
    assert_eq!(rig.volume(track), before, "the message moved nothing");
    let learned = state(&mut rig);
    assert_eq!(learned["rows"][0]["name"], "Volume");
    assert_eq!(learned["guess"]["type"], "absolute");
    assert_eq!(learned["selected"]["type"], "mixer", "it stays selected");

    rig.send("Pads", &[0xB0, 30, 127]);
    let mode = mode_of(&mut rig, &cc(30));
    assert_eq!(mode["type"], "relative");
    assert_eq!(mode["encoding"], "twos_complement");
    assert_eq!(mode["accelerated"], false);
    assert_eq!(state(&mut rig)["guess"]["encoding"], "twos_complement");

    rig.pass(1_000);
    rig.shell.poll();
    assert_eq!(
        rig.steps(),
        steps + 1,
        "learning one control is one undo step"
    );
    rig.run("edit.undo", json!({}));
    assert!(rig.mappings().is_empty());
}

// Spec: map-modes R5, remote-mapping R27. A controller followed by its
// partner is a 14-bit pair, and a controller from another source maps that
// source instead.
#[test]
fn a_14_bit_pair_is_learned_and_another_source_replaces_the_mapping() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", true);
    let track = rig.track("audio");
    rig.run("remote.map_mode", json!({ "mode": "midi" }));
    select(&mut rig, volume_of(track));

    rig.send("Pads", &[0xB0, 1, 64]);
    rig.send("Pads", &[0xB0, 33, 0]);
    let listed = rig.mappings();
    assert_eq!(
        listed[0]["source"],
        json!({ "type": "cc14", "channel": 1, "controller": 1 })
    );
    assert_eq!(listed[0]["mode"]["type"], "absolute14");

    rig.send("Pads", &[0xB0, 31, 10]);
    let listed = rig.mappings();
    assert_eq!(listed.len(), 1, "a control has one MIDI mapping");
    assert_eq!(listed[0]["source"], cc(31));
    assert_eq!(listed[0]["mode"]["type"], "absolute");
}

// Spec: map-modes R10. A mode chosen by hand stays while the controller keeps
// sending.
#[test]
fn a_mode_chosen_by_hand_is_not_guessed_over() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", true);
    let track = rig.track("audio");
    rig.run("remote.map_mode", json!({ "mode": "midi" }));
    select(&mut rig, volume_of(track));
    rig.send("Pads", &[0xB0, 30, 64]);
    let relative = json!({ "type": "relative", "encoding": "bin_offset", "accelerated": true });
    rig.run(
        "remote.set_mode",
        json!({ "source": cc(30), "mode": relative }),
    );
    rig.send("Pads", &[0xB0, 30, 65]);
    rig.send("Pads", &[0xB0, 30, 65]);
    assert_eq!(mode_of(&mut rig, &cc(30)), relative);
    assert!(state(&mut rig)["guess"].is_null());
}

// Spec: map-modes R5. Note-offs map nothing; a note held while a second goes
// down maps the range between them on a continuous control, and a single
// note on a switch.
#[test]
fn notes_map_a_note_a_range_or_nothing() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", true);
    let track = rig.track("audio");
    rig.run("remote.map_mode", json!({ "mode": "midi" }));
    select(&mut rig, volume_of(track));

    rig.send("Pads", &[0x80, 60, 0]);
    assert!(rig.mappings().is_empty(), "a note-off maps nothing");
    rig.send("Pads", &[0x90, 60, 100]);
    assert_eq!(rig.mappings()[0]["source"], note(60));
    rig.send("Pads", &[0x90, 64, 100]);
    assert_eq!(
        rig.mappings()[0]["source"],
        json!({ "type": "note_range", "channel": 1, "low": 60, "high": 64 })
    );
    rig.send("Pads", &[0x80, 60, 0]);
    rig.send("Pads", &[0x80, 64, 0]);

    select(&mut rig, json!({ "type": "solo", "track": track }));
    rig.send("Pads", &[0x90, 50, 100]);
    rig.send("Pads", &[0x90, 52, 100]);
    let listed = rig.mappings();
    let solo = listed
        .iter()
        .find(|m| m["target"]["type"] == "solo")
        .unwrap();
    assert_eq!(solo["source"], note(52), "a switch takes the last key");
}

// Spec: map-modes R6. On a slot the first key is the root; a key below and a
// key above it, held, make the range; a root released first maps the note.
#[test]
fn a_slot_learns_a_root_and_then_a_range_around_it() {
    let mut rig = Rig::new();
    rig.remote_port("Pads", true);
    let track = rig.track("midi");
    let scene = rig.scene();
    let slot = json!({ "type": "slot", "track": track, "scene": scene });
    rig.run("remote.map_mode", json!({ "mode": "midi" }));
    select(&mut rig, slot.clone());

    rig.send("Pads", &[0x90, 60, 100]);
    assert_eq!(rig.mappings()[0]["source"], note(60));
    rig.send("Pads", &[0x90, 59, 100]);
    assert_eq!(
        rig.mappings()[0]["source"],
        note(60),
        "one side isn't a range yet"
    );
    rig.send("Pads", &[0x90, 62, 100]);
    assert_eq!(
        rig.mappings()[0]["source"],
        json!({ "type": "note_range", "channel": 1, "low": 59, "high": 62, "root": 60 })
    );
    for key in [60, 59, 62] {
        rig.send("Pads", &[0x80, key, 0]);
    }
    rig.send("Pads", &[0x90, 70, 100]);
    rig.send("Pads", &[0x80, 70, 0]);
    rig.send("Pads", &[0x90, 65, 100]);
    assert_eq!(
        rig.mappings()[0]["source"],
        note(65),
        "a released root maps only its note"
    );
}

// Spec: map-modes R8. The Mapping Browser's rows are the mode's mappings in
// order of their source: MIDI by channel, then note or controller; keys by
// name.
#[test]
fn the_mapping_rows_are_the_modes_in_order_of_their_source() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    rig.map(cc(20), volume_of(track));
    rig.map(note(60), json!({ "type": "solo", "track": track }));
    rig.map(
        json!({ "type": "key", "key": "S" }),
        json!({ "type": "arm", "track": track }),
    );
    rig.run("remote.map_mode", json!({ "mode": "midi" }));
    let rows = state(&mut rig)["rows"].clone();
    let sources: Vec<&Value> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["source"])
        .collect();
    assert_eq!(sources, [&note(60), &cc(20)]);
    rig.run("remote.map_mode", json!({ "mode": "key" }));
    assert_eq!(state(&mut rig)["rows"][0]["source"]["type"], "key");
    rig.run("remote.map_mode", json!({ "mode": "off" }));
    assert!(state(&mut rig)["rows"].as_array().unwrap().is_empty());
}
