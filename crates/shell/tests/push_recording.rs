//! Recording on Push 2 in the shell (`docs/specs/push-2-notes.md`, R48 to
//! R60): Record, Fixed Length, New and Duplicate, Accent, Repeat and
//! Quantize, driven through the simulated Push 2 on the null device's
//! manual pacing.

use bl_core::time::Ticks;
use bl_model::ClipData;
use serde_json::{Value, json};

use crate::push_beats::Rig;

const BAR: i64 = 4 * bl_core::time::TICKS_PER_QUARTER;

impl Rig {
    fn record(&mut self) -> String {
        self.state()["recording"]["record"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The clips of the first MIDI track, by scene order: their lengths.
    fn lengths(&self) -> Vec<i64> {
        let doc = self.shell.editor().document();
        doc.scenes()
            .filter_map(|scene| doc.slot(self.track, scene.id))
            .map(|clip| clip.length.get())
            .collect()
    }

    fn wait(&mut self, what: &str, done: impl Fn(&mut Rig) -> bool) {
        for _ in 0..4000 {
            if done(self) {
                return;
            }
            self.pass(10);
        }
        panic!("waited for {what}");
    }
}

fn session_rig() -> Rig {
    let mut rig = Rig::new();
    rig.run("session.focus", json!({}));
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "none" }),
    );
    rig.pass(50);
    rig
}

// Spec: push-2-notes R48. Record starts Session Record; pressed while it
// runs it ends it and the clip plays on; pressed with the clip playing it
// overdubs, and again it stops overdubbing. Record's LED follows.
#[test]
fn four_presses_of_record_record_a_slot_end_it_overdub_it_and_stop() {
    let mut rig = session_rig();
    assert_eq!(rig.record(), "off");
    rig.tap("record");
    rig.wait("the recording", |rig| rig.record() == "session");
    assert_eq!(rig.led("record").colour, bl_push::leds::fixed::RED);
    rig.pass(600);
    rig.tap("record");
    rig.wait("the recording to end", |rig| rig.record() == "off");
    assert_eq!(rig.led("record").colour, bl_push::leds::fixed::DARK_GREY);
    assert_eq!(rig.lengths().len(), 1, "a clip was recorded");
    let scene = rig.shell.editor().document().scenes().next().unwrap().id;
    rig.wait("the clip to play", |rig| {
        rig.shell
            .session_states()
            .iter()
            .any(|s| s.track == rig.track && s.scene == scene && s.state == bl_engine::SlotState::Playing)
    });
    rig.tap("record");
    rig.wait("the overdub", |rig| rig.record() == "session");
    assert_eq!(rig.lengths().len(), 1, "overdubbing makes no new clip");
    rig.tap("record");
    rig.wait("the overdub to stop", |rig| rig.record() == "off");
}

// Spec: push-2-notes R49. With the Arrangement in focus Record switches
// Arrangement Record, and Shift with Record switches Session Record; with
// the Session View in focus it is the other way round.
#[test]
fn with_the_arrangement_in_focus_record_and_shift_record_switch_arrangement_and_session() {
    let mut rig = Rig::new();
    rig.run("focus.arrangement", json!({}));
    rig.pass(50);
    rig.tap("record");
    assert_eq!(rig.record(), "arrangement");
    rig.tap("record");
    assert_eq!(rig.record(), "off");
    rig.wait("the recording to finish", |rig| rig.shell.running_recording().is_none());
    assert!(rig.sim.press("shift"));
    rig.pass(50);
    assert!(rig.sim.press("record"));
    rig.pass(50);
    assert!(rig.sim.let_go("record"));
    assert!(rig.sim.let_go("shift"));
    rig.wait("a Session recording", |rig| rig.record() == "session");
    rig.run("session.stop_recording", json!({}));
    rig.wait("it to end", |rig| rig.record() == "off");
    rig.run("session.focus", json!({}));
    rig.pass(50);
    rig.tap("record");
    rig.wait("a Session recording", |rig| rig.record() == "session");
    rig.run("session.stop_recording", json!({}));
    rig.wait("it to end", |rig| rig.record() == "off");
    assert!(rig.sim.press("shift"));
    rig.pass(50);
    assert!(rig.sim.press("record"));
    rig.pass(50);
    assert!(rig.sim.let_go("record"));
    assert!(rig.sim.let_go("shift"));
    rig.pass(50);
    assert_eq!(rig.record(), "arrangement");
}

// Spec: push-2-notes R50, R51. A 2-bar Fixed Length recording makes a
// 2-bar clip and ends on its own.
#[test]
fn a_two_bar_fixed_length_recording_makes_a_two_bar_clip_and_ends_by_itself() {
    let mut rig = session_rig();
    rig.run(
        "push.set_fixed_length",
        json!({ "enabled": true, "length": "2 bars" }),
    );
    rig.pass(50);
    let fixed = rig.state()["recording"]["fixed_length"].clone();
    assert_eq!(fixed, json!({ "on": true, "length": "2 bars", "phrase_sync": false }));
    rig.tap("record");
    rig.wait("the recording", |rig| rig.record() == "session");
    assert_eq!(rig.lengths(), [2 * BAR], "the clip has its length at once");
    rig.wait("it to end by itself", |rig| rig.record() == "off");
    assert_eq!(rig.lengths(), [2 * BAR]);
}

// Spec: push-2-notes R50. A new install has Fixed Length off at 1 bar; the
// button's tap switches it and its hold shows the menu, whose lower buttons
// choose the length and whose first upper button is Phrase Sync.
#[test]
fn fixed_lengths_button_and_menu_set_its_preferences() {
    let mut rig = Rig::new();
    let fixed = rig.state()["recording"]["fixed_length"].clone();
    assert_eq!(fixed, json!({ "on": false, "length": "1 bar", "phrase_sync": false }));
    rig.tap("fixed_length");
    assert_eq!(rig.state()["recording"]["fixed_length"]["on"], true);
    rig.hold("fixed_length");
    assert_eq!(rig.state()["menu"], "fixed_length");
    assert!(rig.sim.press("lower_5"));
    rig.pass(50);
    assert!(rig.sim.let_go("lower_5"));
    assert!(rig.sim.press("upper_1"));
    rig.pass(50);
    assert!(rig.sim.let_go("upper_1"));
    rig.release("fixed_length");
    rig.pass(50);
    let state = rig.state();
    assert_eq!(state["menu"], Value::Null);
    assert_eq!(
        state["recording"]["fixed_length"],
        json!({ "on": true, "length": "4 bars", "phrase_sync": true }),
        "the menu's choices, with the tap's switch kept"
    );
}

// Spec: push-2-notes R51. Switching Fixed Length on while a recording runs
// ends it and loops the clip's last stretch of the length.
#[test]
fn fixed_length_switched_on_mid_recording_loops_the_last_stretch() {
    let mut rig = session_rig();
    rig.tap("record");
    rig.wait("the recording", |rig| rig.record() == "session");
    rig.pass(5000);
    rig.tap("fixed_length");
    rig.wait("it to end", |rig| rig.record() == "off");
    let doc = rig.shell.editor().document();
    let scene = doc.scenes().next().unwrap().id;
    let clip = doc.slot(rig.track, scene).unwrap();
    let ClipData::Midi(midi) = &clip.data else {
        panic!("a MIDI clip")
    };
    assert!(midi.looping);
    assert_eq!(midi.loop_end, Some(clip.length));
    assert_eq!(midi.loop_end.unwrap() - midi.loop_start, Ticks(BAR).min(clip.length));
}

impl Rig {
    fn scenes(&self) -> Vec<bl_model::SceneId> {
        self.shell.editor().document().scenes().map(|s| s.id).collect()
    }

    /// An empty one-bar MIDI clip in `scene`'s slot of the track, which is
    /// selected there.
    fn clip_in(&mut self, scene: bl_model::SceneId) -> bl_model::ClipId {
        let track = self.track;
        self.run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        );
        self.run("session.select", json!({ "track": track, "scene": scene }));
        self.pass(50);
        self.shell.editor().document().slot(track, scene).unwrap().id
    }

    /// The slot the Session View has selected.
    fn selected_slot(&mut self) -> Option<(bl_model::TrackId, bl_model::SceneId)> {
        let state = self.run("session.state", json!({}));
        let slot = state["slots"].as_array()?.first()?.clone();
        Some((
            serde_json::from_value(slot["track"].clone()).ok()?,
            serde_json::from_value(slot["scene"].clone()).ok()?,
        ))
    }

    fn playing(&self, track: bl_model::TrackId, scene: bl_model::SceneId) -> bool {
        self.shell.session_states().iter().any(|s| {
            s.track == track && s.scene == scene && s.state == bl_engine::SlotState::Playing
        })
    }
}

// Spec: push-2-notes R52. With Workflow on Clip, New selects the next empty
// slot below on the selected track, inserting a scene when there is none;
// each is one undo step.
#[test]
fn new_with_workflow_clip_selects_the_next_empty_slot_or_inserts_a_scene() {
    let mut rig = session_rig();
    rig.run("push.set_setup", json!({ "workflow": "clip" }));
    let scenes = rig.scenes();
    rig.clip_in(scenes[0]);
    let steps = rig.steps();
    rig.tap("new");
    assert_eq!(rig.selected_slot(), Some((rig.track, scenes[1])));
    assert_eq!(rig.scenes(), scenes, "the slot below was empty");
    let last = *scenes.last().unwrap();
    rig.clip_in(last);
    let steps = rig.steps().max(steps);
    rig.tap("new");
    let after = rig.scenes();
    assert_eq!(after.len(), scenes.len() + 1);
    assert_eq!(rig.selected_slot(), Some((rig.track, *after.last().unwrap())));
    assert_eq!(rig.steps(), steps + 1, "one undo step");
}

// Spec: push-2-notes R52. With Workflow on Scene, New stops the selected
// track's clip and inserts a scene holding copies of the clips playing on
// the other tracks, and selects the selected track's empty slot in it.
#[test]
fn new_with_workflow_scene_captures_the_other_tracks_and_selects_an_empty_slot() {
    let mut rig = session_rig();
    let other = crate::push_beats::new_track(&mut rig, "Other");
    rig.run(
        "arrangement.select_tracks",
        json!({ "tracks": [rig.track] }),
    );
    let first = rig.scenes()[0];
    rig.clip_in(first);
    rig.run(
        "session.insert_empty_midi_clip",
        json!({ "track": other, "scene": first }),
    );
    for track in [rig.track, other] {
        rig.run("session.launch", json!({ "track": track, "scene": first }));
    }
    rig.pass(100);
    let steps = rig.steps();
    rig.tap("new");
    let scenes = rig.scenes();
    let doc = rig.shell.editor().document();
    assert_eq!(scenes.len(), 9, "a scene was inserted");
    assert_eq!(scenes[1..].iter().filter(|s| doc.slot(other, **s).is_some()).count(), 1);
    assert!(doc.slot(rig.track, scenes[1]).is_none(), "the selected track's slot is empty");
    assert_eq!(rig.selected_slot(), Some((rig.track, scenes[1])));
    assert_eq!(rig.steps(), steps + 1, "one undo step");
}

// Spec: push-2-notes R53. Duplicate alone with Workflow on Scene captures
// and inserts a scene; with Workflow on Clip it copies the selected clip into
// the slot below and launches the copy; each is one undo step.
#[test]
fn duplicate_in_both_workflows_makes_the_scene_or_the_copy() {
    let mut rig = session_rig();
    let scenes = rig.scenes();
    rig.clip_in(scenes[0]);
    rig.run("session.launch", json!({ "track": rig.track, "scene": scenes[0] }));
    rig.pass(100);
    let steps = rig.steps();
    rig.tap("duplicate");
    assert_eq!(rig.scenes().len(), scenes.len() + 1, "Capture and Insert Scene");
    assert_eq!(rig.steps(), steps + 1, "one undo step");

    let mut rig = session_rig();
    rig.run("push.set_setup", json!({ "workflow": "clip" }));
    let scenes = rig.scenes();
    rig.clip_in(scenes[0]);
    rig.run("session.launch", json!({ "track": rig.track, "scene": scenes[0] }));
    rig.pass(100);
    let steps = rig.steps();
    rig.tap("duplicate");
    rig.pass(100);
    let doc = rig.shell.editor().document();
    assert!(doc.slot(rig.track, scenes[1]).is_some(), "the copy is in the slot below");
    assert_eq!(rig.scenes().len(), scenes.len());
    assert!(rig.playing(rig.track, scenes[1]), "the copy plays");
    assert_eq!(rig.steps(), steps + 1, "one undo step");
}

fn velocities(rig: &Rig) -> Vec<u8> {
    let (_, midi) = rig.clip().expect("a clip");
    let mut notes: Vec<_> = midi.notes.iter().map(|n| (n.start.get(), n.velocity)).collect();
    notes.sort_unstable();
    notes.into_iter().map(|n| n.1).collect()
}

// Spec: push-2-notes R54. Accent gives 127 over the 16 velocities, switched
// on by a tap and, held longer than a tap, on until released.
#[test]
fn accent_gives_127_tapped_and_held() {
    let mut rig = Rig::new();
    rig.tap("layout");
    rig.tap_pad(0, 4);
    assert_eq!(rig.note()["velocity"], 8);
    rig.tap("accent");
    assert_eq!(rig.note()["accent"], true);
    rig.tap_pad(4, 0);
    rig.tap("accent");
    assert_eq!(rig.note()["accent"], false);
    rig.tap_pad(4, 1);
    rig.hold("accent");
    assert_eq!(rig.note()["accent"], true);
    rig.tap_pad(4, 2);
    rig.release("accent");
    assert_eq!(rig.note()["accent"], false);
    rig.tap_pad(4, 3);
    assert_eq!(velocities(&rig), [127, 8, 127, 8]);
}

fn starts(rig: &Rig) -> Vec<i64> {
    let doc = rig.shell.editor().document();
    let mut starts: Vec<i64> = doc
        .track(rig.track)
        .unwrap()
        .clips()
        .flat_map(|clip| match &clip.data {
            ClipData::Midi(midi) => midi.notes.iter().map(|n| clip.start.get() + n.start.get()).collect(),
            ClipData::Audio(_) => Vec::new(),
        })
        .collect();
    starts.sort_unstable();
    starts
}

/// Holds drum pad 36 for a beat with Repeat on and returns the gaps between
/// the notes it recorded.
fn repeated_gaps(rig: &mut Rig) -> Vec<i64> {
    rig.run("transport.set_record_quantization", json!({ "quantization": "none" }));
    rig.tap("repeat");
    assert_eq!(rig.note()["repeat"], true);
    rig.run("transport.set_recording", json!({ "recording": true }));
    rig.run("transport.play", json!({}));
    rig.pass(100);
    assert!(rig.sim.press_pad(0, 0, 100));
    for _ in 0..100 {
        rig.pass(5);
    }
    assert!(rig.sim.release_pad(0, 0));
    rig.pass(50);
    rig.run("transport.stop", json!({}));
    let track = rig.track;
    testing::wait_for(&rig.control, &mut rig.shell, "the clip", |shell| {
        shell.running_recording().is_none()
            && shell.editor().document().track(track).is_some_and(|t| t.clips().count() > 0)
    });
    let starts = starts(rig);
    starts.windows(2).map(|pair| pair[1] - pair[0]).collect()
}

use bl_shell::testing;

// Spec: push-2-notes R55. Repeat at 1/16 plays a held pad's note on the
// grid, a step apart.
#[test]
fn repeat_plays_a_held_pad_on_the_grid() {
    let mut rig = Rig::new();
    let gaps = repeated_gaps(&mut rig);
    let step = Ticks::SIXTEENTH.get();
    assert!(gaps.len() >= 3, "{gaps:?}");
    assert!(gaps.iter().all(|gap| (gap - step).abs() < step / 8), "{gaps:?} vs {step}");
}

// Spec: push-2-notes R55, R56. The swing amount delays every second note by
// that share of half a step.
#[test]
fn repeat_delays_every_second_note_by_the_swing_amount() {
    let mut rig = Rig::new();
    rig.run("push.set_swing", json!({ "amount": 100 }));
    let gaps = repeated_gaps(&mut rig);
    let step = Ticks::SIXTEENTH.get();
    assert!(gaps.len() >= 3, "{gaps:?}");
    let (long, short) = (gaps.iter().max().unwrap(), gaps.iter().min().unwrap());
    assert!((long - short - step).abs() < step / 4, "{gaps:?} vs {step}");
}

/// A clip with a note on each of pads 36 and 37, a little off the 1/16 grid.
fn off_grid() -> (Rig, i64) {
    let mut rig = session_rig();
    let scene = rig.scenes()[0];
    let clip = rig.clip_in(scene);
    let step = Ticks::SIXTEENTH.get();
    rig.run("clip_view.quantize_settings", json!({ "grid": step }));
    for (key, start) in [(36, step + 5000), (37, 2 * step - 3000)] {
        rig.run(
            "note.add",
            json!({ "clip": clip, "key": key, "start": start, "length": step, "velocity": 100 }),
        );
    }
    rig.run("clip_view.select_notes", json!({ "clip": clip, "notes": [] }));
    (rig, step)
}

fn note_starts(rig: &Rig) -> Vec<(u8, i64)> {
    let (_, midi) = rig.clip().unwrap();
    let mut notes: Vec<_> = midi.notes.iter().map(|n| (n.key, n.start.get())).collect();
    notes.sort_unstable();
    notes
}

// Spec: push-2-notes R57. Quantize quantises all the notes when none is
// selected, as one undo step.
#[test]
fn quantize_quantises_all_notes_when_none_is_selected() {
    let (mut rig, step) = off_grid();
    let steps = rig.steps();
    rig.tap("quantize");
    rig.pass(50);
    assert_eq!(note_starts(&rig), [(36, step), (37, 2 * step)]);
    assert_eq!(rig.steps(), steps + 1, "one undo step");
}

// Spec: push-2-notes R57. With Quantize held, tapping a drum pad quantises
// only that pad's notes.
#[test]
fn quantize_and_a_drum_pad_quantise_only_its_notes() {
    let (mut rig, step) = off_grid();
    assert!(rig.sim.press("quantize"));
    rig.pass(50);
    rig.tap_pad(0, 0);
    assert!(rig.sim.let_go("quantize"));
    rig.pass(50);
    assert_eq!(note_starts(&rig), [(36, step), (37, 2 * step - 3000)]);
}

// Spec: push-2-notes R58. Holding Quantize shows its menu, which stays after
// a value changed and the button is released; the next tap closes it without
// quantising.
#[test]
fn the_quantize_menu_stays_after_a_change_until_the_next_tap() {
    let mut rig = session_rig();
    let scene = rig.scenes()[0];
    let clip = rig.clip_in(scene);
    let step = Ticks::SIXTEENTH.get();
    rig.run(
        "note.add",
        json!({ "clip": clip, "key": 36, "start": step + 5000, "length": step, "velocity": 100 }),
    );
    rig.hold("quantize");
    assert_eq!(rig.state()["menu"], "quantize");
    assert_eq!(rig.state()["recording"]["quantize_menu"]["amount"], 100);
    assert!(rig.sim.turn(2, -20));
    rig.pass(50);
    rig.release("quantize");
    rig.pass(50);
    let state = rig.state();
    assert_eq!(state["menu"], "quantize", "a value changed, so the menu stays");
    assert_eq!(state["recording"]["quantize_menu"]["amount"], 90);
    rig.tap("quantize");
    assert_eq!(rig.state()["menu"], Value::Null);
    let (_, midi) = rig.clip().unwrap();
    assert_eq!(midi.notes.iter().next().unwrap().start.get(), step + 5000, "nothing quantised");
}

impl Rig {
    /// Shows Mix Mode and returns the encoder that sets the track's volume.
    fn mix_column(&mut self) -> u8 {
        self.run("push.set_mode", json!({ "display": "mix" }));
        self.pass(100);
        let state = self.state();
        let places = state["mix_mode"]["places"].as_array().unwrap().clone();
        let at = places
            .iter()
            .position(|p| p["track"] == json!(self.track))
            .expect("the track is in the ring");
        at as u8
    }

    /// The clip's Automation envelopes' targets and breakpoints.
    fn automation(&self, clip: bl_model::ClipId) -> Vec<(bl_model::AutomationTarget, Vec<(i64, f64)>)> {
        let doc = self.shell.editor().document();
        doc.clip(clip)
            .unwrap()
            .envelopes
            .iter()
            .filter(|e| e.kind == bl_model::EnvelopeKind::Automation)
            .map(|e| {
                let points = e.envelope.points().map(|p| (p.at.get(), p.value)).collect();
                (e.target.clone(), points)
            })
            .collect()
    }

    fn volume(&self) -> f64 {
        self.shell.editor().document().track(self.track).unwrap().mixer().volume
    }

    fn text_has(&mut self, needle: &str) -> bool {
        self.sim_text().iter().any(|line| line.contains(needle))
    }
}

// Spec: push-2-notes R61, R65. Automate arms automation (red while on); with
// it on, an encoder's touch and turns during a Session recording go into the
// clip's Automation envelope.
#[test]
fn automate_arms_and_an_encoders_turns_are_recorded_into_the_clip() {
    let mut rig = session_rig();
    let column = rig.mix_column();
    rig.tap("automate");
    assert_eq!(rig.state()["recording"]["automation_arm"], true);
    assert_eq!(rig.led("automate").colour, bl_push::leds::fixed::RED);
    rig.tap("record");
    rig.wait("the recording", |rig| rig.record() == "session");
    assert!(rig.sim.touch(column));
    rig.pass(50);
    assert!(rig.sim.turn(column, -20));
    rig.pass(50);
    assert!(rig.sim.release(column));
    rig.pass(300);
    rig.tap("record");
    rig.wait("it to end", |rig| rig.record() == "off");
    let scene = rig.scenes()[0];
    let clip = rig.shell.editor().document().slot(rig.track, scene).unwrap().id;
    let automation = rig.automation(clip);
    assert_eq!(automation.len(), 1, "{automation:?}");
    assert!(matches!(
        automation[0].0,
        bl_model::AutomationTarget::Mixer { param: bl_model::MixerParam::Volume }
    ));
    rig.tap("automate");
    assert_eq!(rig.state()["recording"]["automation_arm"], false);
}

/// A clip with an Automation envelope for the track's volume, selected, with
/// Mix Mode shown: the column of the volume encoder.
fn automated_clip() -> (Rig, bl_model::ClipId, u8) {
    let mut rig = session_rig();
    let column = rig.mix_column();
    let scene = rig.scenes()[0];
    let clip = rig.clip_in(scene);
    rig.run(
        "clip.set_envelope_breakpoint",
        json!({
            "clip": clip,
            "target": { "type": "mixer", "param": "volume" },
            "kind": "automation",
            "at": 0,
            "value": 0.5,
        }),
    );
    rig.pass(50);
    (rig, clip, column)
}

// Spec: push-2-notes R64. The white dot shows while the clip playing on the
// track has an enabled Automation envelope for the parameter.
#[test]
fn the_dot_follows_the_playing_clips_automation() {
    let (mut rig, _clip, _) = automated_clip();
    assert!(!rig.text_has("white dot"));
    let scene = rig.scenes()[0];
    rig.run("session.launch", json!({ "track": rig.track, "scene": scene }));
    rig.pass(200);
    assert!(rig.text_has("white dot"), "{:?}", rig.sim_text());
}

// Spec: push-2-notes R62. With Delete held, touching an encoder deletes its
// parameter's Automation envelope in the clip the steps edit, as one undo
// step; a parameter with none goes back to its default.
#[test]
fn delete_and_a_touch_delete_the_envelope_or_reset_the_parameter() {
    let (mut rig, clip, column) = automated_clip();
    assert_eq!(rig.automation(clip).len(), 1);
    let steps = rig.steps();
    assert!(rig.sim.press("delete"));
    rig.pass(50);
    assert!(rig.sim.touch(column));
    rig.pass(50);
    assert!(rig.sim.release(column));
    rig.pass(50);
    assert!(rig.automation(clip).is_empty());
    assert_eq!(rig.steps(), steps + 1, "one undo step");
    // Nothing to delete now: the volume goes back to its default.
    rig.run("mixer.set_volume", json!({ "track": rig.track, "volume": 0.25 }));
    rig.pass(50);
    assert!(rig.sim.touch(column));
    rig.pass(50);
    assert!(rig.sim.release(column));
    assert!(rig.sim.let_go("delete"));
    rig.pass(50);
    assert!((rig.volume() - 1.0).abs() < 1e-6, "{}", rig.volume());
    assert!(rig.clip().is_some(), "Delete used with an encoder doesn't delete the clip");
}

// Spec: push-2-notes R63. Delete with Automate deletes every Automation
// envelope of the clip the steps edit, as one undo step.
#[test]
fn delete_and_automate_clear_the_clips_automation() {
    let (mut rig, clip, _) = automated_clip();
    rig.run(
        "clip.set_envelope_breakpoint",
        json!({
            "clip": clip,
            "target": { "type": "mixer", "param": "pan" },
            "kind": "automation",
            "at": 0,
            "value": 0.1,
        }),
    );
    assert_eq!(rig.automation(clip).len(), 2);
    let steps = rig.steps();
    assert!(rig.sim.press("delete"));
    rig.pass(50);
    assert!(rig.sim.press("automate"));
    rig.pass(50);
    assert!(rig.sim.let_go("automate"));
    assert!(rig.sim.let_go("delete"));
    rig.pass(50);
    assert!(rig.automation(clip).is_empty());
    assert_eq!(rig.steps(), steps + 1, "one undo step");
    assert_eq!(rig.state()["recording"]["automation_arm"], false, "no arm toggle");
}

// Spec: push-2-notes R66. Two held steps and a turn write flat values over
// those steps' spans, one undo step per touch.
#[test]
fn held_steps_and_a_turn_write_the_value_over_their_spans() {
    let mut rig = session_rig();
    let column = rig.mix_column();
    let scene = rig.scenes()[0];
    let clip = rig.clip_in(scene);
    let step = Ticks::SIXTEENTH.get();
    let steps = rig.steps();
    assert!(rig.sim.press_pad(4, 1, 100));
    rig.pass(50);
    assert!(rig.sim.press_pad(4, 2, 100));
    rig.pass(50);
    assert_eq!(rig.note()["held_steps"].as_array().unwrap().len(), 2);
    assert!(rig.sim.touch(column));
    rig.pass(50);
    assert!(rig.sim.turn(column, -30));
    rig.pass(50);
    assert!(rig.sim.turn(column, -10));
    rig.pass(50);
    assert!(rig.sim.release(column));
    rig.pass(50);
    assert!(rig.sim.release_pad(4, 1));
    assert!(rig.sim.release_pad(4, 2));
    rig.pass(50);
    let automation = rig.automation(clip);
    assert_eq!(automation.len(), 1, "{automation:?}");
    let points = &automation[0].1;
    let ats: Vec<i64> = points.iter().map(|p| p.0).collect();
    assert_eq!(ats, [step, 2 * step, 3 * step], "breakpoints at the steps' edges");
    assert!(points.iter().all(|p| (p.1 - points[0].1).abs() < 1e-9), "flat over the spans");
    assert!(points[0].1 < 1.0, "the turn lowered it: {points:?}");
    assert_eq!(rig.steps() - steps, 1 + 2, "the clip's notes aside, one undo step for the touch: {}", rig.steps());
}

// Spec: push-2-notes R59. The count-in's bar shows while it runs, and a
// progress bar under the lower button of a track whose clip plays.
#[test]
fn the_display_shows_the_count_in_and_the_playing_clips_progress() {
    let mut rig = session_rig();
    let scene = rig.scenes()[0];
    rig.clip_in(scene);
    rig.run("session.launch", json!({ "track": rig.track, "scene": scene }));
    rig.pass(300);
    assert!(rig.text_has("progress"), "{:?}", rig.sim_text());
    rig.run("transport.stop", json!({}));
    rig.pass(300);
    rig.run("transport.set_metronome_count_in", json!({ "count_in": "1_bar" }));
    rig.tap("record");
    let mut counted = false;
    for _ in 0..100 {
        rig.pass(10);
        counted |= rig.text_has("count-in");
    }
    assert!(counted, "the count-in's bar showed");
}

// Spec: push-2-notes R51. With Phrase Sync on, a take started at bar 6 of a
// 4-bar phrase starts at bar 2 of the clip, and ends with the phrase.
#[test]
fn phrase_sync_starts_the_take_part_way_into_the_clip() {
    let mut rig = session_rig();
    rig.run("push.set_fixed_length", json!({ "enabled": true, "length": "4 bars", "phrase_sync": true }));
    rig.run("transport.locate", json!({ "position": 5 * BAR }));
    rig.run("transport.play", json!({}));
    rig.pass(100);
    rig.tap("record");
    rig.wait("the recording", |rig| rig.record() == "session");
    assert!(rig.sim.press_pad(0, 0, 100));
    rig.pass(100);
    assert!(rig.sim.release_pad(0, 0));
    rig.wait("it to end", |rig| rig.record() == "off");
    let doc = rig.shell.editor().document();
    let scene = doc.scenes().next().unwrap().id;
    let clip = doc.slot(rig.track, scene).unwrap();
    assert_eq!(clip.length.get(), 4 * BAR);
    let ClipData::Midi(midi) = &clip.data else {
        panic!("a MIDI clip")
    };
    let start = midi.notes.iter().next().expect("the note").start.get();
    assert!(start >= BAR && start < BAR + BAR / 2, "the note is at bar 2 of the clip: {start}");
}
