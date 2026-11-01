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
