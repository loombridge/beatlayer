//! Feedback to controllers, the remote indicators and remote control for
//! recording (WP-7.4; `docs/specs/remote-mapping.md`, R38 to R44): a mapped
//! control's value goes to every output with Remote on whatever moved it,
//! once per poll and never back to the mapping that moved it. Everything runs
//! against the in-process test backend and the null device paced by hand.

use std::path::PathBuf;

use bl_audio_io::NullControl;
use bl_core::clock::TestClock;
use bl_midi_io::{Direction, TestBackend};
use bl_model::fader::{fader_gain, fader_position};
use bl_model::{SceneId, TrackId};
use bl_shell::{Shell, testing};
use serde_json::{Value, json};

const LAMPS: &str = "Lamps";
const QUIET: &str = "Quiet";

struct Rig {
    shell: Shell,
    control: NullControl,
    test: TestBackend,
    clock: TestClock,
    /// How much of an output's log earlier checks have taken.
    seen: usize,
}

impl Rig {
    fn new() -> Self {
        let (mut shell, control) = testing::rig();
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Pads");
        for name in [LAMPS, QUIET, "Late"] {
            test.add_port(Direction::Output, name);
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
            seen: 0,
        };
        rig.run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        );
        rig.run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Pads", "remote": true, "track": false }),
        );
        // Remote on for one output, and an output that is open with Remote off.
        rig.output(LAMPS, true);
        rig.output(QUIET, false);
        rig
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    fn output(&mut self, name: &str, remote: bool) {
        self.run(
            "midi.set_port",
            json!({ "direction": "output", "name": name, "remote": remote, "track": !remote }),
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

    fn map(&mut self, source: Value, target: Value) {
        self.run("remote.map", json!({ "source": source, "target": target }));
    }

    /// A move of the mouse: the fader goes to `position`.
    fn hand(&mut self, track: TrackId, position: f64) {
        self.run(
            "mixer.set_volume",
            json!({ "track": track, "volume": fader_gain(position) }),
        );
    }

    /// Polls and takes what `LAMPS` was sent since the last call.
    fn fed(&mut self) -> Vec<Vec<u8>> {
        self.shell.poll();
        let all = self.test.sent(LAMPS);
        let new = all[self.seen..].to_vec();
        self.seen = all.len();
        new
    }

    fn pass(&mut self, millis: u64) {
        testing::advance(&self.control, &mut self.shell, millis);
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

// Spec: remote-mapping R38. A moved volume sends its position as the CC's
// value to an output with Remote on, and nothing to one with it off.
#[test]
fn a_moved_volume_sends_its_position_to_an_output_with_remote_on_only() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    rig.map(cc(28), volume_of(track));
    rig.fed();

    rig.hand(track, 0.5);
    assert_eq!(rig.fed(), [[0xB0, 28, 64]]);
    assert!(rig.test.sent(QUIET).is_empty(), "Remote off: nothing");
    assert!(rig.fed().is_empty(), "nothing moved: nothing sent");
}

// Spec: remote-mapping R38. Min above Max sends the inverted value.
#[test]
fn min_above_max_sends_the_inverted_value() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    rig.map(cc(28), volume_of(track));
    rig.run(
        "remote.set_range",
        json!({ "source": cc(28), "min": fader_gain(1.0), "max": 0.0 }),
    );
    rig.fed();

    rig.hand(track, 0.25);
    assert_eq!(rig.fed(), [[0xB0, 28, 95]]);
}

// Spec: remote-mapping R22, R38. A 14-bit mapping sends its pair, the high
// part then the low.
#[test]
fn a_14_bit_mapping_sends_its_pair() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    rig.map(
        json!({ "type": "cc14", "channel": 1, "controller": 7 }),
        volume_of(track),
    );
    rig.fed();

    rig.hand(track, 0.5);
    assert_eq!(rig.fed(), [[0xB0, 7, 64], [0xB0, 39, 0]]);
}

// Spec: remote-mapping R38. A note on a switch is velocity 127 while it is
// on and 0 while it is off.
#[test]
fn a_note_mapped_solo_sends_velocity_127_then_0() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    rig.map(note(36), json!({ "type": "solo", "track": track }));
    rig.fed();

    rig.run("mixer.set_solo", json!({ "track": track, "solo": true }));
    assert_eq!(rig.fed(), [[0x90, 36, 127]]);
    rig.run("mixer.set_solo", json!({ "track": track, "solo": false }));
    assert_eq!(rig.fed(), [[0x90, 36, 0]]);
}

// Spec: remote-mapping R38. A slot's note is 127 while its clip plays or is
// queued, and 0 when it stops.
#[test]
fn a_mapped_slot_sends_127_while_its_clip_plays_or_is_queued() {
    let mut rig = Rig::new();
    let track = rig.track("midi");
    let scene = rig.scene();
    rig.run(
        "session.insert_empty_midi_clip",
        json!({ "track": track, "scene": scene }),
    );
    rig.map(
        note(40),
        json!({ "type": "slot", "track": track, "scene": scene }),
    );
    assert_eq!(rig.fed(), [[0x90, 40, 0]], "an empty slot is dark");

    rig.run("session.launch", json!({ "track": track, "scene": scene }));
    rig.pass(100);
    assert_eq!(rig.fed(), [[0x90, 40, 127]]);
    rig.run("session.stop_track", json!({ "track": track }));
    rig.pass(100);
    assert_eq!(rig.fed(), [[0x90, 40, 0]]);
}

// Spec: remote-mapping R38. A relative mapping sends the control's position
// in its range, 0 to 127, on its controller.
#[test]
fn a_relative_mapping_sends_its_position() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    rig.run(
        "remote.map",
        json!({
            "source": cc(29),
            "target": volume_of(track),
            "mode": { "type": "relative", "encoding": "signed_bit", "accelerated": false },
        }),
    );
    rig.fed();

    rig.hand(track, 1.0);
    assert_eq!(rig.fed(), [[0xB0, 29, 127]]);
    rig.hand(track, 0.0);
    assert_eq!(rig.fed(), [[0xB0, 29, 0]]);
}

// Spec: remote-mapping R39. The mapping's own message isn't sent back to
// it; a hand move after it is.
#[test]
fn the_mappings_own_message_is_not_echoed() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    rig.map(cc(28), volume_of(track));
    rig.fed();

    assert!(rig.test.send("Pads", rig.clock.now(), &[0xB0, 28, 100]));
    assert!(rig.fed().is_empty(), "the controller moved it itself");
    assert!(
        (fader_position(
            rig.shell
                .editor()
                .document()
                .track(track)
                .unwrap()
                .mixer()
                .volume
        ) - 100.0 / 127.0)
            .abs()
            < 1e-6
    );

    rig.hand(track, 0.1);
    assert_eq!(rig.fed(), [[0xB0, 28, 13]]);
}

// Spec: remote-mapping R39. Fifty changes between two polls send one message,
// with the latest value.
#[test]
fn fifty_changes_in_one_poll_send_one_message() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    rig.map(cc(28), volume_of(track));
    rig.fed();

    for step in 1..=50 {
        rig.hand(track, f64::from(step) / 100.0);
    }
    assert_eq!(rig.fed(), [[0xB0, 28, 64]], "0.50 of 127");
}

// Spec: remote-mapping R39. Making a mapping, switching an output's Remote
// on and loading a set each send every mapping's value.
#[test]
fn making_a_mapping_switching_remote_on_and_loading_a_set_send_every_value() {
    let mut rig = Rig::new();
    let (a, b) = (rig.track("audio"), rig.track("audio"));
    rig.map(cc(20), volume_of(a));
    rig.hand(a, 0.5);
    assert_eq!(rig.fed(), [[0xB0, 20, 64]], "the first mapping's value");

    rig.map(cc(21), volume_of(b));
    rig.hand(b, 1.0);
    rig.fed();
    rig.run(
        "remote.map",
        json!({ "source": note(36), "target": json!({ "type": "solo", "track": a }) }),
    );
    let mut all = rig.fed();
    all.sort();
    assert_eq!(
        all,
        [vec![0x90, 36, 0], vec![0xB0, 20, 64], vec![0xB0, 21, 127]],
        "making a mapping sends every value"
    );

    rig.output("Late", true);
    rig.shell.poll();
    let mut late = rig.test.sent("Late");
    late.sort();
    assert_eq!(
        late, all,
        "an output whose Remote turned on gets every value"
    );
    assert_eq!(
        rig.fed(),
        Vec::<Vec<u8>>::new(),
        "the output that was on already gets nothing again"
    );

    let folder = Scratch::new("set");
    let saved = rig.run(
        "set.save_as",
        json!({ "path": folder.0.join("Song.blset").to_str().unwrap() }),
    );
    rig.run("set.open", json!({ "path": saved["path"] }));
    let mut loaded = rig.fed();
    loaded.sort();
    assert_eq!(loaded, all, "a loaded set sends every value");
}

// Spec: remote-mapping R15, R42, session-recording R2. A mapped slot on an
// armed track records into its empty slot as the slot's record button does,
// a mapped Arm key arms the track as its button does, and an unarmed track's
// empty slot does what clicking it does.
#[test]
fn a_mapped_slot_on_an_armed_track_records_as_its_record_button_does() {
    use bl_engine::SlotState;

    // What a slot of an armed track shows after a press and after the same
    // press's release: from the mapping, or from the button's command.
    let outcome = |mapped: bool| {
        let mut rig = Rig::new();
        let track = rig.track("midi");
        let scene = rig.scene();
        rig.map(note(30), json!({ "type": "arm", "track": track }));
        rig.map(
            note(31),
            json!({ "type": "slot", "track": track, "scene": scene }),
        );
        let send = |rig: &mut Rig, bytes: &[u8]| {
            assert!(rig.test.send("Pads", rig.clock.now(), bytes));
            rig.shell.poll();
        };
        rig.run("mixer.set_arm", json!({ "track": track, "arm": false }));
        if mapped {
            send(&mut rig, &[0x90, 30, 100]);
            send(&mut rig, &[0x90, 31, 100]);
        } else {
            rig.run("mixer.set_arm", json!({ "track": track, "arm": true }));
            rig.run(
                "session.record_slot",
                json!({ "track": track, "scene": scene }),
            );
        }
        rig.pass(100);
        if mapped {
            send(&mut rig, &[0x80, 31, 0]);
        }
        let doc = rig.shell.editor().document();
        (
            doc.track(track).unwrap().mixer().arm,
            doc.slot(track, scene).is_some(),
            rig.shell
                .session_states()
                .iter()
                .map(|slot| slot.state == SlotState::Recording)
                .collect::<Vec<_>>(),
            rig.shell.editor().undo_history().count(),
        )
    };
    let by_button = outcome(false);
    assert_eq!(by_button.2, [true], "the button records");
    let by_mapping = outcome(true);
    assert_eq!(
        (by_mapping.0, by_mapping.1, &by_mapping.2),
        (true, true, &by_button.2)
    );
    assert_eq!(by_mapping.3, by_button.3, "the same undo steps");
}

// Spec: remote-mapping R15. An unarmed track's empty slot stops the track as
// its stop button does, and launches nothing.
#[test]
fn a_mapped_empty_slot_on_an_unarmed_track_stops_it_as_the_stop_button_does() {
    let mut rig = Rig::new();
    let track = rig.track("midi");
    let scene = rig.scene();
    rig.run("mixer.set_arm", json!({ "track": track, "arm": false }));
    rig.map(
        note(31),
        json!({ "type": "slot", "track": track, "scene": scene }),
    );
    let applied = rig.shell.remote_counts();
    assert!(rig.test.send("Pads", rig.clock.now(), &[0x90, 31, 100]));
    rig.shell.poll();
    assert!(rig.test.send("Pads", rig.clock.now(), &[0x80, 31, 0]));
    rig.shell.poll();
    rig.pass(100);
    assert!(rig.shell.session_states().is_empty(), "nothing plays");
    assert!(rig.shell.editor().document().slot(track, scene).is_none());
    assert_eq!(
        rig.shell.remote_counts().1,
        applied.1,
        "nothing was refused"
    );
}

// Spec: remote-mapping R43. `remote.state` reads back the counts, the lamps
// and each mapping's takeover state by ID.
#[test]
fn remote_state_reads_back_the_counts_the_lamps_and_the_takeover_states() {
    let mut rig = Rig::new();
    let track = rig.track("audio");
    rig.map(cc(20), volume_of(track));
    let state = rig.run("remote.state", json!({}));
    assert_eq!(state["counts"]["taken"], 0);
    assert_eq!(
        state["lamps"]["remote_input"]["last_lit_ms_ago"],
        Value::Null
    );
    rig.fed();

    assert!(rig.test.send("Pads", rig.clock.now(), &[0xB0, 20, 100]));
    rig.shell.poll();
    assert!(rig.test.send("Pads", rig.clock.now(), &[0xB0, 21, 100]));
    rig.hand(track, 0.1);
    rig.shell.poll();
    let state = rig.run("remote.state", json!({}));
    let counts = &state["counts"];
    assert_eq!(
        [
            &counts["taken"],
            &counts["unmapped"],
            &counts["applied"],
            &counts["refused"],
            &counts["feedback_sent"],
        ],
        [1, 1, 1, 0, 2],
        "taken, unmapped, applied, refused; the first value and the hand move"
    );
    for lamp in ["remote_input", "remote_output"] {
        assert!(state["lamps"][lamp]["last_lit_ms_ago"].is_u64(), "{lamp}");
    }
    assert_eq!(state["lamps"]["key_input"]["last_lit_ms_ago"], Value::Null);
    assert_eq!(state["takeover"][0]["state"], "following");
    let last = state["takeover"][0]["last"].as_f64().unwrap();
    assert!((last - 100.0 / 127.0).abs() < 1e-9, "{last}");
    assert_eq!(state["guess"], Value::Null);
}

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-remote-feedback-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
