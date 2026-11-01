//! Beats on Push 2 in the shell (`docs/specs/push-2-notes.md`, R1 to R5 and
//! R8 to R31): Note Mode on a track, the drum layouts, the pads of a drum
//! rack and step sequencing, driven through the simulated Push 2. Every edit
//! runs the app's own commands as one undo step.

use bl_audio_io::NullControl;
use bl_engine::testing as engine;
use bl_midi_io::TestBackend;
use bl_model::{ClipData, ClipId, DeviceId, SceneId, TrackId, TrackKind};
use bl_push::sim::Simulated;
use bl_shell::{Shell, testing};
use serde_json::{Value, json};

pub(crate) struct Rig {
    pub(crate) shell: Shell,
    pub(crate) control: NullControl,
    pub(crate) sim: Simulated,
    pub(crate) track: TrackId,
    pub(crate) rack: DeviceId,
}

impl Rig {
    /// The default set with a drum rack on its MIDI track, which is
    /// selected, pads 36 to 38 holding a chain each, and a Push 2 running.
    pub(crate) fn new() -> Self {
        let (mut shell, control) = testing::rig();
        let test = TestBackend::new();
        shell.use_midi_backend(Box::new(test.clone()));
        let clock = control.clock().expect("manual pacing");
        shell.use_test_clock(clock.clone());
        let sim = Simulated::new(&test, &clock);
        shell.use_push_display(sim.frames().claim());
        shell.start_audio();
        let track = shell
            .editor()
            .document()
            .tracks()
            .find(|t| t.kind() == TrackKind::Midi)
            .map(bl_model::Track::id)
            .unwrap();
        shell
            .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
            .unwrap();
        let added = shell
            .invoke(
                "device.add",
                json!({ "device": {
                    "kind": { "rack": { "kind": "drum" } },
                    "name": "Drum Rack",
                    "role": "instrument",
                } }),
            )
            .unwrap();
        let rack: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
        let gain = engine::gain_type();
        for note in [36, 37, 38] {
            shell
                .invoke(
                    "pad.drop",
                    json!({
                        "rack": rack,
                        "note": note,
                        "device": { "kind": gain.kind, "name": "Gain", "role": gain.role },
                    }),
                )
                .unwrap();
        }
        shell
            .invoke("pad.select", json!({ "rack": rack, "note": 36 }))
            .unwrap();
        let mut rig = Self {
            shell,
            control,
            sim,
            track,
            rack,
        };
        rig.sim.plug_in();
        rig.shell.refresh_midi();
        rig.settle();
        rig.pass(50);
        rig
    }

    pub(crate) fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    pub(crate) fn settle(&mut self) {
        loop {
            self.shell.midi().surfaces().settle();
            if self.sim.pump() == 0 {
                break;
            }
        }
    }

    pub(crate) fn step(&mut self) {
        self.shell.poll();
        self.shell.midi().surfaces().tick();
        self.settle();
        self.shell.poll();
    }

    pub(crate) fn pass(&mut self, millis: u64) {
        testing::advance(&self.control, &mut self.shell, millis);
        self.step();
    }

    pub(crate) fn state(&mut self) -> Value {
        self.run("push.state", json!({}))["pushes"][0].clone()
    }

    pub(crate) fn note(&mut self) -> Value {
        self.state()["note"].clone()
    }

    pub(crate) fn steps(&self) -> usize {
        self.shell.editor().undo_history().count()
    }

    pub(crate) fn tap_pad(&mut self, row: u8, column: u8) {
        assert!(self.sim.press_pad(row, column, 100));
        self.pass(20);
        assert!(self.sim.release_pad(row, column));
        self.pass(20);
        self.pass(20);
    }

    pub(crate) fn tap(&mut self, name: &str) {
        assert!(self.sim.press(name));
        self.pass(50);
        assert!(self.sim.let_go(name));
        self.pass(20);
        self.pass(20);
    }

    pub(crate) fn hold(&mut self, name: &str) {
        assert!(self.sim.press(name));
        self.pass(400);
    }

    pub(crate) fn release(&mut self, name: &str) {
        assert!(self.sim.let_go(name));
        self.pass(20);
    }

    /// The clip in the first MIDI track's slot at the first scene.
    pub(crate) fn clip(&self) -> Option<(ClipId, bl_model::MidiData)> {
        let doc = self.shell.editor().document();
        let scene: SceneId = doc.scenes().next()?.id;
        let clip = doc.slot(self.track, scene)?;
        match &clip.data {
            ClipData::Midi(midi) => Some((clip.id, midi.clone())),
            ClipData::Audio(_) => None,
        }
    }

    /// The text the display shows, line by line.
    pub(crate) fn sim_text(&mut self) -> Vec<String> {
        self.state()["display"]["text"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line.as_str().unwrap().to_owned())
            .collect()
    }

    pub(crate) fn led(&self, name: &str) -> bl_push::leds::Led {
        self.sim.led(name).unwrap()
    }
}

fn keys(midi: &bl_model::MidiData) -> Vec<(u8, i64)> {
    let mut notes: Vec<(u8, i64)> = midi.notes.iter().map(|n| (n.key, n.start.get())).collect();
    notes.sort_unstable();
    notes
}

// Spec: push-2-notes R1, R8, R9. A drum rack track shows the Loop Selector,
// and Layout steps through the three layouts and back.
#[test]
fn a_drum_rack_track_shows_the_loop_selector_and_layout_steps_through_the_layouts() {
    let mut rig = Rig::new();
    rig.step();
    let note = rig.note();
    assert_eq!(note["layout"], "Loop Selector", "{note}");
    assert_eq!(note["bank"], 36);
    assert_eq!(note["selected_pad"], 36);
    for expected in ["16 Velocities", "64 Pads", "Loop Selector"] {
        rig.tap("layout");
        assert_eq!(rig.note()["layout"], expected);
    }
}

// Spec: push-2-notes R20, R22. The first step on an empty slot makes a
// one-bar clip with the note and launches it, as one undo step; tapping the
// step again deletes the note.
#[test]
fn a_first_step_makes_a_clip_and_launches_it_and_tapping_again_deletes_the_note() {
    let mut rig = Rig::new();
    assert!(rig.clip().is_none());
    let steps = rig.steps();
    // Step 1 is the bottom left pad of the step area.
    rig.tap_pad(4, 0);
    rig.pass(20);
    let (_, midi) = rig.clip().expect("a clip");
    assert_eq!(keys(&midi), [(36, 0)]);
    assert_eq!(rig.steps(), steps + 1, "one undo step");
    rig.tap_pad(4, 0);
    let (_, midi) = rig.clip().unwrap();
    assert!(midi.notes.is_empty());
}

/// A new MIDI track, which is selected as the app selects a new track.
pub(crate) fn new_track(rig: &mut Rig, name: &str) -> TrackId {
    rig.shell
        .invoke("track.create", json!({ "kind": "midi", "name": name }))
        .unwrap();
    rig.pass(50);
    rig.shell.arrangement().selected_tracks()[0]
}

fn armed(rig: &Rig, track: TrackId) -> bool {
    rig.shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .mixer()
        .arm
}

/// The lower display button that stands for `track`, by its place in the
/// ring.
fn lower_of(rig: &mut Rig, track: TrackId) -> String {
    let name = rig
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .name()
        .to_owned();
    let text = rig.sim_text();
    text.iter()
        .find_map(|line| {
            let (button, label) = line.strip_prefix("lower ")?.split_once(": ")?;
            (label.starts_with(&name)).then(|| format!("lower_{button}"))
        })
        .unwrap_or_else(|| panic!("{name} isn't on the display: {text:?}"))
}

// Spec: push-2-notes R3, R4. Selecting a MIDI track arms it by selection and
// disarms the one armed that way before, with no undo step; holding a lower
// display button arms by hand, and that arm stays when another track is
// selected.
#[test]
fn selecting_a_track_arms_it_by_selection_and_holding_a_lower_button_arms_by_hand() {
    let mut rig = Rig::new();
    let first = rig.track;
    assert!(armed(&rig, first), "the selected track is armed");
    assert!(rig.shell.armed_by_selection(first));
    let second = new_track(&mut rig, "Keys");
    let steps = rig.steps();
    assert!(!armed(&rig, first), "the earlier arm is released");
    assert!(armed(&rig, second));
    assert!(rig.shell.armed_by_selection(second));
    assert!(!rig.shell.armed_by_selection(first));
    assert_eq!(rig.steps(), steps, "no undo step");

    // Holding the lower button of the first track arms it by hand.
    let button = lower_of(&mut rig, first);
    rig.hold(&button);
    rig.release(&button);
    assert!(armed(&rig, first));
    assert!(!rig.shell.armed_by_selection(first), "a manual arm");
    // Selecting the first track on the Push, then the second, leaves the
    // manual arm alone and moves the selection arm.
    let (button, other) = (lower_of(&mut rig, first), lower_of(&mut rig, second));
    rig.tap(&button);
    assert_eq!(rig.shell.arrangement().selected_tracks(), [first]);
    assert!(!armed(&rig, second), "the selection arm moved");
    rig.tap(&other);
    assert!(armed(&rig, first), "a manual arm stays");
    assert!(armed(&rig, second));
    let label = rig.sim_text();
    assert!(label.iter().any(|l| l.contains("(armed)")), "{label:?}");
    // Holding it again disarms it.
    let button = lower_of(&mut rig, first);
    rig.hold(&button);
    rig.release(&button);
    assert!(!armed(&rig, first));
}

fn rgb(rig: &Rig, pad: &str) -> [u8; 3] {
    rig.sim.colour(pad).unwrap()
}

/// The colour of a swatch of the default track palette.
fn swatch(rig: &Rig, track: TrackId) -> [u8; 3] {
    let colour = rig.shell.editor().document().track(track).unwrap().colour();
    bl_model::swatch_rgb(&bl_model::DEFAULT_TRACK_PALETTE, colour)
}

fn dark(rig: &Rig) -> bool {
    (0..8).all(|row| (0..8).all(|column| rig.led(&format!("pad_{row}_{column}")).colour == 0))
}

// Spec: push-2-notes R1. A drum rack track shows a drum layout, a melodic
// track doesn't, an audio track's pads are dark, and adding a drum rack to a
// track switches its layout at once.
#[test]
fn the_layout_follows_the_selected_tracks_instrument() {
    let mut rig = Rig::new();
    assert_eq!(rig.note()["layout"], "Loop Selector");
    assert!(!dark(&rig));

    let keys = new_track(&mut rig, "Keys");
    assert_eq!(rig.note()["layout"], "64 Notes");
    assert!(!dark(&rig), "a melodic track plays notes on its pads");
    rig.shell
        .invoke(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "drum" } },
                "name": "Drum Rack",
                "role": "instrument",
            } }),
        )
        .unwrap();
    rig.pass(50);
    assert_eq!(rig.note()["layout"], "Loop Selector", "at once");
    assert_eq!(rig.note()["track"], json!(keys));

    rig.shell
        .invoke("track.create", json!({ "kind": "audio", "name": "Audio" }))
        .unwrap();
    rig.pass(50);
    assert_eq!(rig.note()["layout"], "None");
    assert!(dark(&rig));
}

// Spec: push-2-notes R12. A drum pad is lit in the track's colour when a
// chain receives its note, a lighter version when none does, green while it
// plays, white while selected, blue while soloed and grey while muted.
#[test]
fn the_pads_colours_say_what_they_hold() {
    let mut rig = Rig::new();
    let track = swatch(&rig, rig.track);
    // Pad 36 is selected, 37 and 38 hold chains, 39 is empty.
    assert_eq!(rgb(&rig, "pad_0_0"), [204, 204, 204], "selected");
    assert_eq!(rgb(&rig, "pad_0_1"), track, "a chain receives it");
    assert_eq!(
        rgb(&rig, "pad_0_3"),
        bl_push::note::grid::lighter(track),
        "empty"
    );
    // Soloed and muted.
    let rack = rig.rack;
    rig.run("pad.set_solo", json!({ "rack": rack, "note": 37 }));
    rig.run("pad.set_mute", json!({ "rack": rack, "note": 38 }));
    rig.pass(50);
    assert_eq!(rgb(&rig, "pad_0_1"), [0, 0, 255], "soloed");
    assert_eq!(rgb(&rig, "pad_0_2"), [64, 64, 64], "muted");
    // Playing: green while held.
    assert!(rig.sim.press_pad(0, 3, 100));
    rig.pass(20);
    assert_eq!(rgb(&rig, "pad_0_3"), [0, 255, 0], "while it plays");
    assert!(rig.sim.release_pad(0, 3));
    rig.pass(20);
    // Tapping a pad selects it.
    assert_eq!(rgb(&rig, "pad_0_3"), [204, 204, 204]);
    assert_eq!(rgb(&rig, "pad_0_0"), track);
}

// Spec: push-2-notes R13. Octave Up and Down move the bank sixteen notes, or
// a row of four with Shift held, and are off where they can't move; the
// touch strip moves it to the place touched, and moving the view in the app
// moves the pads.
#[test]
fn octave_and_the_strip_move_the_bank_and_the_app_moves_the_pads() {
    let mut rig = Rig::new();
    assert_eq!(rig.note()["bank"], 36);
    rig.tap("octave_up");
    assert_eq!(rig.note()["bank"], 52);
    rig.tap("octave_down");
    rig.tap("octave_down");
    assert_eq!(rig.note()["bank"], 20);
    assert!(rig.sim.press("shift"));
    rig.pass(50);
    rig.tap("octave_up");
    assert!(rig.sim.let_go("shift"));
    rig.pass(20);
    assert_eq!(rig.note()["bank"], 24, "a row with Shift");

    // The strip: by sixteen notes, and by rows with Shift held.
    assert_eq!(rig.state()["touch_strip"]["configuration"], "range");
    assert!(rig.sim.strip_touch());
    assert!(rig.sim.strip_move(0.3));
    rig.pass(20);
    assert!(rig.sim.strip_release());
    rig.pass(20);
    assert_eq!(rig.note()["bank"], 32);
    let lit: Vec<u64> = rig.state()["touch_strip"]["leds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    assert!(
        lit[8] == 7 && lit[11] == 7 && lit[0] == 0 && lit[20] == 0,
        "{lit:?}"
    );

    // The app moves the view; the pads follow, and the end of the range
    // turns Octave Up off.
    let rack = rig.rack;
    rig.run("rack.move_pads", json!({ "rack": rack, "first": 112 }));
    rig.pass(50);
    assert_eq!(rig.note()["bank"], 112);
    assert_eq!(rig.led("octave_up").colour, 0, "off at the end");
    assert_ne!(rig.led("octave_down").colour, 0);
    rig.run("rack.move_pads", json!({ "rack": rack, "first": 0 }));
    rig.pass(50);
    assert_eq!(rig.led("octave_down").colour, 0);
}

fn chains_on(rig: &Rig, note: u8) -> Vec<bl_model::Chain> {
    rig.shell
        .editor()
        .document()
        .device(rig.rack)
        .unwrap()
        .chains
        .iter()
        .filter(|c| !c.is_return && c.receive == bl_model::Receive::Note(note))
        .cloned()
        .collect()
}

/// Presses `button` with Shift held and lets both go.
fn shifted(rig: &mut Rig, button: &str) {
    assert!(rig.sim.press("shift"));
    rig.pass(50);
    rig.tap(button);
    assert!(rig.sim.let_go("shift"));
    rig.pass(40);
}

// Spec: push-2-notes R9. Holding Layout shows the velocities over the Loop
// Selector and the loop length pads over the others while it is held; Shift
// and Layout keep that view, and Layout alone then returns without stepping.
#[test]
fn holding_layout_shows_another_view_and_shift_layout_keeps_it() {
    let mut rig = Rig::new();
    rig.hold("layout");
    assert_eq!(rig.note()["overlay"], "16 Velocities");
    rig.release("layout");
    rig.pass(40);
    assert_eq!(rig.note()["overlay"], Value::Null);
    assert_eq!(rig.note()["layout"], "Loop Selector", "a hold doesn't step");

    rig.tap("layout");
    assert_eq!(rig.note()["layout"], "16 Velocities");
    rig.hold("layout");
    assert_eq!(rig.note()["overlay"], "Loop Length Pads");
    rig.release("layout");
    rig.pass(40);

    shifted(&mut rig, "layout");
    let note = rig.note();
    assert_eq!(note["overlay"], "Loop Length Pads", "{note}");
    assert_eq!(note["overlay_locked"], true);
    assert_eq!(note["layout"], "16 Velocities");
    rig.tap("layout");
    let note = rig.note();
    assert_eq!(note["overlay"], Value::Null);
    assert_eq!(note["layout"], "16 Velocities", "no step");
    rig.tap("layout");
    assert_eq!(rig.note()["layout"], "64 Pads");

    // Holding Layout over 64 Pads shows the loop length pads in the lower
    // right.
    rig.hold("layout");
    assert_eq!(rig.note()["overlay"], "Loop Length Pads");
    rig.release("layout");
}

// Spec: push-2-notes R5. The layout, the kept view and the step size are
// kept for each track while the surface runs, and selecting a track brings
// back its own.
#[test]
fn each_track_keeps_its_layout_and_step_size() {
    let mut rig = Rig::new();
    let drums = rig.track;
    rig.tap("layout");
    rig.tap("scene_5");
    assert_eq!(rig.note()["layout"], "16 Velocities");
    assert_eq!(rig.note()["step_size"], "1/8T");
    // Another drum track starts afresh.
    let second = new_track(&mut rig, "Drums 2");
    rig.shell
        .invoke(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "drum" } },
                "name": "Drum Rack",
                "role": "instrument",
            } }),
        )
        .unwrap();
    rig.pass(50);
    assert_eq!(rig.note()["track"], json!(second));
    assert_eq!(rig.note()["layout"], "Loop Selector");
    assert_eq!(rig.note()["step_size"], "1/16");
    rig.run("arrangement.select_tracks", json!({ "tracks": [drums] }));
    rig.pass(50);
    assert_eq!(rig.note()["layout"], "16 Velocities");
    assert_eq!(rig.note()["step_size"], "1/8T");
}

// Spec: push-2-notes R10. The sixteen velocities rise from 8 at the bottom
// left to 127 at the top right; tapping one plays the selected drum pad at
// that velocity, and new steps take it.
#[test]
fn a_velocity_pad_sets_the_velocity_new_steps_take() {
    let mut rig = Rig::new();
    rig.tap("layout");
    assert_eq!(rig.note()["layout"], "16 Velocities");
    rig.tap_pad(3, 7);
    assert_eq!(rig.note()["velocity"], 127);
    rig.tap_pad(4, 0);
    rig.pass(20);
    let (_, midi) = rig.clip().expect("a clip");
    assert_eq!(
        midi.notes.iter().map(|n| n.velocity).collect::<Vec<_>>(),
        [127]
    );
    rig.tap_pad(0, 4);
    assert_eq!(rig.note()["velocity"], 8);
    rig.tap_pad(4, 1);
    let (_, midi) = rig.clip().unwrap();
    let mut velocities: Vec<_> = midi
        .notes
        .iter()
        .map(|n| (n.start.get(), n.velocity))
        .collect();
    velocities.sort_unstable();
    assert_eq!(velocities[1].1, 8);
}

// Spec: push-2-notes R14 to R17. Select and a pad selects it without a
// note; Mute and Solo run `pad.set_mute` and `pad.set_solo`; Delete deletes
// the pad's notes in the clip the steps edit, and the pad's chains where it
// has none; Duplicate copies a pad's chains onto another, and no clip's
// notes change.
#[test]
fn modifiers_and_a_pad_select_mute_solo_delete_and_duplicate() {
    let mut rig = Rig::new();
    // Select + pad 37.
    assert!(rig.sim.press("select"));
    rig.pass(50);
    rig.tap_pad(0, 1);
    assert!(rig.sim.let_go("select"));
    rig.pass(40);
    assert_eq!(rig.note()["selected_pad"], 37);

    // Mute and Solo.
    let steps = rig.steps();
    assert!(rig.sim.press("mute"));
    rig.pass(50);
    rig.tap_pad(0, 2);
    assert!(rig.sim.let_go("mute"));
    rig.pass(40);
    assert!(!chains_on(&rig, 38)[0].mixer.enabled, "muted");
    assert!(rig.sim.press("solo"));
    rig.pass(50);
    rig.tap_pad(0, 1);
    assert!(rig.sim.let_go("solo"));
    rig.pass(40);
    assert!(chains_on(&rig, 37)[0].mixer.solo, "soloed");
    assert_eq!(rig.steps(), steps + 2, "an undo step each");

    // Steps for pad 37, then Delete + pad 37 deletes its notes only.
    rig.tap_pad(4, 0);
    rig.pass(20);
    rig.tap_pad(4, 2);
    let (_, midi) = rig.clip().expect("a clip");
    assert_eq!(keys(&midi).len(), 2);
    assert!(rig.sim.press("delete"));
    rig.pass(50);
    rig.tap_pad(0, 1);
    assert!(rig.sim.let_go("delete"));
    rig.pass(40);
    assert!(rig.clip().unwrap().1.notes.is_empty(), "its notes");
    assert_eq!(chains_on(&rig, 37).len(), 1, "its chain stays");
    // A pad with no notes deletes its chains instead.
    assert!(rig.sim.press("delete"));
    rig.pass(50);
    rig.tap_pad(0, 1);
    assert!(rig.sim.let_go("delete"));
    rig.pass(40);
    assert!(chains_on(&rig, 37).is_empty(), "its chains");

    // Notes for pad 36, then Duplicate pad 36 onto pad 40.
    rig.tap_pad(0, 0);
    rig.tap_pad(4, 4);
    let before = keys(&rig.clip().unwrap().1);
    assert_eq!(before, [(36, 4 * 176_400)]);
    let steps = rig.steps();
    assert!(rig.sim.press("duplicate"));
    rig.pass(50);
    rig.tap_pad(0, 0);
    rig.tap_pad(1, 0);
    assert!(rig.sim.let_go("duplicate"));
    rig.pass(40);
    assert_eq!(chains_on(&rig, 40).len(), 1, "copied to pad 40");
    assert_eq!(chains_on(&rig, 36).len(), 1);
    assert_eq!(keys(&rig.clip().unwrap().1), before, "notes untouched");
    assert_eq!(rig.steps(), steps + 1, "one undo step");
}

// Spec: push-2-notes R19. Shift and a pad opens the colour picker for it,
// and a swatch tapped gives the pad's chains that colour as one undo step,
// shown on the pad; `pad.set_push_colour` reads back in the query's chains.
#[test]
fn shift_and_a_pad_colours_the_pad() {
    let mut rig = Rig::new();
    let steps = rig.steps();
    assert!(rig.sim.press("shift"));
    rig.pass(50);
    rig.tap_pad(0, 1);
    assert!(rig.sim.let_go("shift"));
    rig.pass(40);
    assert_eq!(rig.state()["colour_picker"], true);
    // Swatch 9 is the ring pad at row 7, column 2.
    assert!(rig.sim.press_pad(7, 2, 100));
    assert!(rig.sim.release_pad(7, 2));
    rig.pass(40);
    rig.pass(40);
    assert_eq!(rig.state()["colour_picker"], false);
    assert_eq!(chains_on(&rig, 37)[0].push_colour, Some(9));
    assert_eq!(rig.steps(), steps + 1);
    assert_eq!(
        rgb(&rig, "pad_0_1"),
        bl_model::swatch_rgb(&bl_model::DEFAULT_TRACK_PALETTE, 9)
    );
    // The track's own colour is untouched.
    assert_eq!(chains_on(&rig, 38)[0].push_colour, None);
}

const STEP: i64 = 176_400;
const BAR: i64 = 4 * 705_600;

fn clip_colour(rig: &Rig) -> [u8; 3] {
    let doc = rig.shell.editor().document();
    let (id, _) = rig.clip().unwrap();
    let swatch = doc.clip(id).unwrap().colour.unwrap();
    bl_model::swatch_rgb(&bl_model::DEFAULT_TRACK_PALETTE, swatch)
}

// Spec: push-2-notes R21 to R23. A step with a note is the clip's colour,
// brighter for a higher velocity; Mute and a step deactivates its note, which
// shows in a lighter colour, and activates it again; a triplet size leaves
// the right two columns of the step area unlit and without effect, and the
// chosen size is the lit scene button.
#[test]
fn steps_show_their_notes_and_triplet_sizes_darken_two_columns() {
    let mut rig = Rig::new();
    rig.tap_pad(4, 0);
    rig.pass(40);
    rig.run("transport.stop", json!({}));
    rig.run("session.stop_all", json!({}));
    rig.pass(100);
    let colour = clip_colour(&rig);
    let loud = bl_push::note::grid::scaled(colour, 40 + 100 * 60 / 127);
    assert_eq!(rgb(&rig, "pad_4_0"), loud, "a note at velocity 100");
    assert_eq!(rgb(&rig, "pad_4_1"), [64, 64, 64], "grey without a note");

    // Mute and a step deactivates its note, and again activates it.
    for mute in [true, false] {
        assert!(rig.sim.press("mute"));
        rig.pass(50);
        rig.tap_pad(4, 0);
        assert!(rig.sim.let_go("mute"));
        rig.pass(40);
        let (_, midi) = rig.clip().unwrap();
        assert_eq!(midi.notes.iter().next().unwrap().mute, mute);
        let expected = if mute {
            bl_push::note::grid::lighter(colour)
        } else {
            loud
        };
        assert_eq!(rgb(&rig, "pad_4_0"), expected);
    }

    // A triplet size: six steps to a row.
    assert_eq!(rig.led("scene_4").colour, 122, "1/16 is chosen");
    rig.tap("scene_3");
    assert_eq!(rig.note()["step_size"], "1/16T");
    assert_eq!(rig.led("scene_3").colour, 122, "the chosen size is white");
    assert_eq!(rig.led("scene_4").colour, 124);
    assert_eq!(rig.led("pad_4_6").colour, 0);
    assert_eq!(rig.led("pad_4_7").colour, 0);
    assert_ne!(rig.led("pad_4_5").colour, 0);
    let notes = rig.clip().unwrap().1.notes.len();
    rig.tap_pad(4, 6);
    rig.tap_pad(4, 7);
    assert_eq!(rig.clip().unwrap().1.notes.len(), notes, "they do nothing");
    // Step 7 of a triplet row is at the sixth place.
    rig.tap_pad(5, 0);
    let mut starts: Vec<i64> = rig
        .clip()
        .unwrap()
        .1
        .notes
        .iter()
        .map(|n| n.start.get())
        .collect();
    starts.sort_unstable();
    assert_eq!(starts, [0, 6 * (BAR / 24)]);
}

// Spec: push-2-notes R26, R27. The loop length pads are pages of the clip:
// holding one and tapping another sets the loop from the first page's start
// to the last page's end; a double tap sets it to one page; a tap on a page
// outside the loop sets the loop to it. Each is one undo step and turns the
// loop on.
#[test]
fn loop_length_pads_set_the_loop() {
    let mut rig = Rig::new();
    rig.tap_pad(4, 0);
    rig.pass(40);
    rig.run("transport.stop", json!({}));
    rig.run("session.stop_all", json!({}));
    rig.pass(100);
    let (_, midi) = rig.clip().unwrap();
    assert!(midi.looping, "the first step turns the loop on");
    assert_eq!(midi.loop_end, None, "over the whole bar");
    let lit = |rig: &Rig, row, column| rig.led(&format!("pad_{row}_{column}")).colour;
    assert_eq!(lit(&rig, 0, 4), 122, "page 1 is in the loop and in view");
    assert_eq!(lit(&rig, 0, 5), 0, "page 2 is outside it");

    // Hold page 1, tap page 3.
    let steps = rig.steps();
    assert!(rig.sim.press_pad(0, 4, 100));
    rig.pass(20);
    rig.tap_pad(0, 6);
    assert!(rig.sim.release_pad(0, 4));
    rig.pass(40);
    let (_, midi) = rig.clip().unwrap();
    assert!(midi.looping);
    assert_eq!(
        (midi.loop_start.get(), midi.loop_end.unwrap().get()),
        (0, 3 * BAR)
    );
    assert_eq!(rig.steps(), steps + 1, "one undo step");
    assert_eq!(lit(&rig, 0, 5), 122, "pages 1 and 2 are in view");
    assert_eq!(lit(&rig, 0, 6), 123, "page 3 is in the loop, out of view");
    assert_eq!(lit(&rig, 0, 7), 0, "page 4 is outside it");

    // A double tap on page 2 sets the loop to it.
    let steps = rig.steps();
    assert!(rig.sim.press_pad(0, 5, 100));
    rig.pass(20);
    assert!(rig.sim.release_pad(0, 5));
    rig.pass(20);
    assert_eq!(
        rig.clip().unwrap().1.loop_start.get(),
        0,
        "one tap changes nothing"
    );
    assert!(rig.sim.press_pad(0, 5, 100));
    rig.pass(20);
    assert!(rig.sim.release_pad(0, 5));
    rig.pass(40);
    let (_, midi) = rig.clip().unwrap();
    assert_eq!(
        (midi.loop_start.get(), midi.loop_end.unwrap().get()),
        (BAR, 2 * BAR)
    );
    assert_eq!(rig.steps(), steps + 1);

    // A tap outside the loop, on page 5.
    let steps = rig.steps();
    rig.tap_pad(1, 4);
    let (_, midi) = rig.clip().unwrap();
    assert_eq!(
        (midi.loop_start.get(), midi.loop_end.unwrap().get()),
        (4 * BAR, 5 * BAR)
    );
    assert_eq!(rig.steps(), steps + 1);
    assert_eq!(rig.note()["page"], 5, "the pair that holds page 5 shows");
}

// Spec: push-2-notes R28. Page Left and Right show the pair of pages before
// or after and stop following; holding either starts following again; a tap
// on a loop length pad in the loop shows its page and stops following.
#[test]
fn the_page_buttons_and_a_loop_pad_tap_stop_following() {
    let mut rig = Rig::new();
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "none" }),
    );
    rig.tap_pad(4, 0);
    rig.pass(40);
    assert_eq!(rig.note()["following"], true);
    rig.tap("page_right");
    let note = rig.note();
    assert_eq!(
        (note["page"].clone(), note["following"].clone()),
        (json!(3), json!(false))
    );
    rig.tap("page_left");
    assert_eq!(rig.note()["page"], 1);
    rig.hold("page_right");
    rig.release("page_right");
    assert_eq!(rig.note()["following"], true);
    // A tap on page 1 in the loop shows it without changing the loop.
    rig.tap_pad(0, 4);
    assert_eq!(rig.note()["following"], false);
    let (_, midi) = rig.clip().unwrap();
    assert_eq!(
        (midi.loop_start.get(), midi.loop_end),
        (0, None),
        "the loop is as it was"
    );
}

// Spec: push-2-notes R28, R23. The step area follows the page that plays:
// with a clip looping over four bars, its pair shows while the playhead is
// in it, and a green step marks the playhead.
#[test]
fn the_step_area_follows_the_page_that_plays() {
    let mut rig = Rig::new();
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "none" }),
    );
    rig.tap_pad(4, 0);
    rig.pass(40);
    // Hold page 1 and tap page 4: the loop is four bars.
    assert!(rig.sim.press_pad(0, 4, 100));
    rig.pass(20);
    rig.tap_pad(1, 4);
    assert!(rig.sim.release_pad(0, 4));
    rig.pass(40);
    // Hold page 1, tap page 4: pages 1 to 4 (page 4 is pad row 0, column 7).
    let (_, midi) = rig.clip().unwrap();
    assert!(midi.looping);
    // Four seconds in at 120 BPM is bar 3, whose pair is pages 3 and 4.
    for _ in 0..120 {
        testing::advance(&rig.control, &mut rig.shell, 50);
        rig.step();
        if rig.note()["page"] == 3 {
            break;
        }
    }
    assert_eq!(rig.note()["page"], 3, "following the playing page");
    assert_eq!(rig.note()["following"], true);
}

// Spec: push-2-notes R30. With Duplicate held, tapping a loop length pad and
// then another copies the first page's notes onto the second, adding them to
// its notes; with Delete held, tapping one deletes that page's notes. One
// undo step each.
#[test]
fn duplicate_and_delete_with_loop_length_pads_copy_and_clear_pages() {
    let mut rig = Rig::new();
    rig.tap_pad(4, 0);
    rig.pass(40);
    rig.run("transport.stop", json!({}));
    rig.pass(100);
    rig.tap_pad(4, 3);
    assert_eq!(keys(&rig.clip().unwrap().1), [(36, 0), (36, 3 * STEP)]);

    // Page 2 is outside the loop, but Duplicate copies onto it anyway.
    let steps = rig.steps();
    assert!(rig.sim.press("duplicate"));
    rig.pass(50);
    rig.tap_pad(0, 4);
    rig.tap_pad(0, 5);
    assert!(rig.sim.let_go("duplicate"));
    rig.pass(40);
    assert_eq!(
        keys(&rig.clip().unwrap().1),
        [(36, 0), (36, 3 * STEP), (36, BAR), (36, BAR + 3 * STEP)]
    );
    assert_eq!(rig.steps(), steps + 1);

    // Delete + page 2 clears it.
    let steps = rig.steps();
    assert!(rig.sim.press("delete"));
    rig.pass(50);
    rig.tap_pad(0, 5);
    assert!(rig.sim.let_go("delete"));
    rig.pass(40);
    assert_eq!(keys(&rig.clip().unwrap().1), [(36, 0), (36, 3 * STEP)]);
    assert_eq!(rig.steps(), steps + 1);
}

// Spec: push-2-notes R24. Delete pressed alone deletes the clip the steps
// edit.
#[test]
fn delete_alone_deletes_the_clip() {
    let mut rig = Rig::new();
    rig.tap_pad(4, 0);
    rig.pass(40);
    assert!(rig.clip().is_some());
    let steps = rig.steps();
    rig.tap("delete");
    assert!(rig.clip().is_none());
    assert_eq!(rig.steps(), steps + 1);
}

// Spec: push-2-notes R31. The loop length pads of the Loop Selector use
// reduced pad sensitivity (Select Pad Settings 1), which the surface sets
// when the layout appears and puts back to regular when it goes.
#[test]
fn the_loop_length_pads_have_reduced_sensitivity() {
    let mut rig = Rig::new();
    let reduced = |on: u8| -> Vec<Vec<u8>> {
        (0..4)
            .flat_map(|row| (4..8).map(move |column| (row, column)))
            .map(|(row, column)| bl_push::sysex::select_pad_settings(row, column, on))
            .collect()
    };
    let sent = rig.sim.sysex();
    for message in reduced(1) {
        assert!(sent.contains(&message), "{message:02X?}");
    }
    // 16 Velocities has no loop length pads.
    rig.sim.forget_sent();
    rig.tap("layout");
    let sent = rig.sim.sysex();
    for message in reduced(0) {
        assert!(sent.contains(&message), "back to regular");
    }
    // Holding Layout in 16 Velocities shows them again.
    rig.sim.forget_sent();
    rig.hold("layout");
    let sent = rig.sim.sysex();
    for message in reduced(1) {
        assert!(sent.contains(&message), "while it is held");
    }
    rig.release("layout");
}

// Spec: push-2-notes R25. The steps held down are exposed to the display
// modes: their pad, clip, start and length read in `push.state`.
#[test]
fn held_steps_are_exposed() {
    let mut rig = Rig::new();
    rig.tap_pad(4, 0);
    rig.pass(40);
    let (clip, _) = rig.clip().unwrap();
    assert!(rig.sim.press_pad(4, 3, 100));
    rig.pass(20);
    let held = rig.note()["held_steps"].clone();
    assert_eq!(
        held,
        json!([{ "clip": clip, "pad": 36, "start": 3 * STEP, "length": STEP }])
    );
    assert!(rig.sim.release_pad(4, 3));
    rig.pass(20);
    assert_eq!(rig.note()["held_steps"], json!([]));
}

// Spec: push-2-notes R2, R14. What the pads play reaches the selected,
// selection-armed track and its recording; a pad tapped with Select held
// plays nothing.
#[test]
fn the_pads_notes_reach_the_armed_track_and_its_recording() {
    let mut rig = Rig::new();
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "none" }),
    );
    rig.run("transport.set_recording", json!({ "recording": true }));
    rig.run("transport.play", json!({}));
    rig.pass(100);
    // Select + pad 38 plays nothing.
    assert!(rig.sim.press("select"));
    rig.pass(50);
    rig.tap_pad(0, 2);
    assert!(rig.sim.let_go("select"));
    rig.pass(40);
    assert_eq!(rig.note()["selected_pad"], 38);
    // Pad 39 plays a note.
    assert!(rig.sim.press_pad(0, 3, 100));
    rig.pass(200);
    assert!(rig.sim.release_pad(0, 3));
    rig.pass(100);
    rig.run("transport.stop", json!({}));
    let track = rig.track;
    testing::wait_for(&rig.control, &mut rig.shell, "the clip", |shell| {
        shell.running_recording().is_none()
            && shell
                .editor()
                .document()
                .track(track)
                .is_some_and(|t| t.clips().count() > 0)
    });
    let doc = rig.shell.editor().document();
    let notes: Vec<u8> = doc
        .track(track)
        .unwrap()
        .clips()
        .flat_map(|clip| match &clip.data {
            ClipData::Midi(midi) => midi.notes.iter().map(|n| n.key).collect::<Vec<_>>(),
            ClipData::Audio(_) => Vec::new(),
        })
        .collect();
    assert_eq!(notes, [39], "only the pad that was played");
}

// Spec: push-2-notes R3. An arm by selection isn't saved: the set written
// while one stands holds the track disarmed, and the arm stays on in the
// open set.
#[test]
fn an_arm_by_selection_isnt_saved() {
    let mut rig = Rig::new();
    assert!(armed(&rig, rig.track));
    let dir = std::env::temp_dir().join(format!("bl-push-arm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Armed.blset");
    let result: bl_shell::commands::SetResult =
        serde_json::from_value(rig.run("set.save_as", json!({ "path": path.to_string_lossy() })))
            .unwrap();
    assert!(armed(&rig, rig.track), "still armed here");
    let (mut other, _) = testing::rig();
    other
        .run("set.open", json!({ "path": result.path.unwrap() }))
        .unwrap();
    let track = rig.track;
    assert!(!other.editor().document().track(track).unwrap().mixer().arm);
    // A copy saved with Save a Copy leaves it out too.
    let copy = rig.run(
        "set.save_copy",
        json!({ "path": dir.join("Copy.blset").to_string_lossy() }),
    );
    other
        .run("set.open", json!({ "path": copy["path"] }))
        .unwrap();
    assert!(!other.editor().document().track(track).unwrap().mixer().arm);
    let _ = std::fs::remove_dir_all(&dir);
}
