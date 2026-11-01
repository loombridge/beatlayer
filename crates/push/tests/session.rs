//! Session Mode on the simulated Push 2
//! (`docs/specs/push-2-tracks.md`, R59 to R70 and R73): the pads and the
//! Scene/Grid buttons as the ring's slots and scenes, their lights, the
//! clip edits, moving the ring, Stop Clip and Session Overview.

use bl_core::time::Ticks;
use bl_engine::{ClipOffset, SlotState, SlotStatus};
use bl_model::{Clip, ClipData, ClipId, Command, MidiData, SceneId, TrackId};
use bl_push::content::PadMode;
use bl_push::leds::{PULSE, fixed};
use serde_json::json;

use crate::rig::Rig;

fn started() -> Rig {
    let mut rig = Rig::sessions();
    rig.start();
    rig.tick();
    rig.tap("session");
    assert_eq!(rig.snapshot().pad_mode, PadMode::Session);
    rig.commands();
    rig
}

fn scenes(rig: &Rig) -> Vec<SceneId> {
    rig.editor.document().scenes().map(|s| s.id).collect()
}

/// A clip in the slot of track `track` and scene `scene`, both counted from 0.
fn clip_in(rig: &mut Rig, track: usize, scene: usize, colour: Option<u8>) -> ClipId {
    clip_named(rig, track, scene, colour, "")
}

fn clip_named(rig: &mut Rig, track: usize, scene: usize, colour: Option<u8>, name: &str) -> ClipId {
    let (track, scene) = (rig.tracks[track], scenes(rig)[scene]);
    let id: ClipId = rig.editor.new_id();
    let mut clip = Clip::new(
        id,
        Ticks::ZERO,
        Ticks::WHOLE,
        ClipData::Midi(MidiData::default()),
    );
    clip.colour = colour;
    clip.name = name.to_owned();
    rig.apply(Command::CreateSlotClip { track, scene, clip });
    id
}

/// Adds scenes until the set has `count`, and puts the ring at the start.
fn with_scenes(rig: &mut Rig, count: usize) {
    while scenes(rig).len() < count {
        let (id, index) = (rig.editor.new_id(), scenes(rig).len());
        rig.apply(Command::CreateScene { id, index });
    }
    rig.ring_at(0, 0);
}

/// Presses the pad in row `row` from the top and column `column`.
fn press(rig: &mut Rig, row: u8, column: u8) {
    assert!(rig.sim.press_pad(7 - row, column, 100));
    rig.advance(1);
}

fn release(rig: &mut Rig, row: u8, column: u8) {
    assert!(rig.sim.release_pad(7 - row, column));
    rig.advance(1);
}

fn pad(row: u8, column: u8) -> String {
    format!("pad_{}_{column}", 7 - row)
}

fn status(track: TrackId, scene: SceneId, state: SlotState) -> SlotStatus {
    SlotStatus {
        track,
        scene,
        state,
        started: None,
        offset: ClipOffset::ZERO,
        velocity: 127,
        transpose: 0,
    }
}

fn command(id: &str, args: serde_json::Value) -> String {
    format!("{id} {args}")
}

// Spec: push-2 R21, push-2-tracks R59. Holding Session in Note Mode switches
// the pads to Session Mode while it is held, and releasing it returns; the
// display keeps its mode.
#[test]
fn holding_session_in_note_mode_switches_the_pads_while_it_is_held() {
    let mut rig = Rig::sessions();
    rig.start();
    rig.tick();
    clip_in(&mut rig, 0, 0, None);
    assert_eq!(rig.snapshot().pad_mode, PadMode::Note);
    assert!(rig.sim.press("session"));
    rig.advance(300);
    assert_eq!(rig.snapshot().pad_mode, PadMode::Session);
    press(&mut rig, 0, 0);
    release(&mut rig, 0, 0);
    assert!(rig.sim.let_go("session"));
    rig.advance(1);
    let (track, scene) = (rig.tracks[0], scenes(&rig)[0]);
    assert_eq!(
        rig.commands(),
        [
            command("session.launch", json!({ "track": track, "scene": scene })),
            command("session.release", json!({ "track": track, "scene": scene })),
        ]
    );
    let state = rig.snapshot();
    assert_eq!(state.pad_mode, PadMode::Note, "back on release");
    assert_eq!(state.display_mode, bl_push::content::DisplayMode::Device);
}

// Spec: push-2-tracks R60, R61. The top-left pad is the ring's first slot:
// pressing it launches the clip and releasing it sends the release, to the
// slot the press launched even if the ring moved; a pad past the last track
// does nothing.
#[test]
fn a_pad_launches_its_clip_and_its_release_goes_to_the_same_slot() {
    let mut rig = started();
    clip_in(&mut rig, 0, 0, None);
    clip_in(&mut rig, 3, 2, None);
    let (t, s) = (rig.tracks.clone(), scenes(&rig));
    rig.tick();

    press(&mut rig, 0, 0);
    assert_eq!(
        rig.commands(),
        [command(
            "session.launch",
            json!({ "track": t[0], "scene": s[0] })
        )]
    );
    release(&mut rig, 0, 0);
    assert_eq!(
        rig.commands(),
        [command(
            "session.release",
            json!({ "track": t[0], "scene": s[0] })
        )]
    );

    press(&mut rig, 2, 3);
    assert_eq!(
        rig.commands(),
        [command(
            "session.launch",
            json!({ "track": t[3], "scene": s[2] })
        )]
    );
    rig.ring_at(1, 0);
    rig.tick();
    release(&mut rig, 2, 3);
    assert_eq!(
        rig.commands(),
        [command(
            "session.release",
            json!({ "track": t[3], "scene": s[2] })
        )],
        "the slot the press launched, not the one now under the pad"
    );

    // Ten tracks, the ring at the fifth: its last two columns are past them.
    rig.ring_at(4, 0);
    rig.tick();
    press(&mut rig, 0, 7);
    release(&mut rig, 0, 7);
    assert!(rig.commands().is_empty());
    assert_eq!(rig.sim.led(&pad(0, 7)).unwrap().colour, 0);
}

// Spec: push-2-tracks R61. An empty slot on an armed track records into it; on
// an unarmed track it stops the track where the slot has a stop button, and
// does nothing where it has none.
#[test]
fn an_empty_pad_records_on_an_armed_track_and_stops_an_unarmed_one() {
    let mut rig = started();
    let (t, s) = (rig.tracks.clone(), scenes(&rig));
    press(&mut rig, 1, 0);
    assert_eq!(
        rig.commands(),
        [command("session.stop_track", json!({ "track": t[0] }))]
    );
    rig.apply(Command::SetSlotStopButton {
        track: t[0],
        scene: s[1],
        stop_button: false,
    });
    rig.tick();
    press(&mut rig, 1, 0);
    assert!(rig.commands().is_empty(), "no stop button, nothing");
    rig.apply(Command::SetArm {
        track: t[0],
        arm: true,
    });
    rig.tick();
    press(&mut rig, 1, 0);
    assert_eq!(
        rig.commands(),
        [command(
            "session.record_slot",
            json!({ "track": t[0], "scene": s[1] })
        )]
    );
}

// Spec: push-2-tracks R62, R65. The Scene/Grid buttons launch the ring's
// scenes from the top down; with Select held a button selects its scene.
#[test]
fn the_scene_buttons_launch_scenes_and_with_select_select_them() {
    let mut rig = started();
    let s = scenes(&rig);
    rig.tap("scene_1");
    rig.tap("scene_8");
    assert_eq!(
        rig.commands(),
        [
            command("session.launch_scene", json!({ "scene": s[0] })),
            command("session.launch_scene", json!({ "scene": s[7] })),
        ]
    );
    assert!(rig.sim.press("select"));
    rig.advance(10);
    rig.tap("scene_3");
    assert!(rig.sim.let_go("select"));
    rig.advance(10);
    assert_eq!(
        rig.commands(),
        [command("session.select", json!({ "scene": s[2] }))]
    );
}

// Spec: push-2-tracks R63. A pad shows its clip's colour; a playing clip's
// pulses and a recording one's pulses between red and its colour; a slot with
// no clip, an empty slot on an armed track included, is dark.
#[test]
fn the_pads_show_clip_colours_pulse_while_playing_and_recording() {
    let mut rig = started();
    clip_in(&mut rig, 0, 0, Some(3));
    clip_in(&mut rig, 1, 0, Some(8));
    clip_in(&mut rig, 2, 0, None);
    let (t, s) = (rig.tracks.clone(), scenes(&rig));
    rig.apply(Command::SetArm {
        track: t[4],
        arm: true,
    });
    rig.model.slots = vec![
        status(t[1], s[0], SlotState::Playing),
        status(t[3], s[0], SlotState::Recording),
    ];
    rig.tick();
    let colours = rig.shared.track_colours();
    let rgb = |swatch| Some(bl_model::swatch_rgb(&colours, swatch));
    let track_swatch = rig.editor.document().track(t[2]).unwrap().colour();

    let stopped = rig.sim.led(&pad(0, 0)).unwrap();
    assert_eq!(rig.sim.colour(&pad(0, 0)), rgb(3));
    assert_eq!(stopped.animation, None);
    let playing = rig.sim.led(&pad(0, 1)).unwrap();
    assert_eq!(rig.sim.colour(&pad(0, 1)), rgb(8));
    assert!(matches!(playing.animation, Some((_, PULSE))));
    assert_eq!(rig.sim.colour(&pad(0, 2)), rgb(track_swatch), "the track's");
    let recording = rig.sim.led(&pad(0, 3)).unwrap();
    assert_eq!(recording.animation, Some((fixed::RED, PULSE)));
    for (row, column) in [(0, 4), (1, 0), (7, 7)] {
        assert_eq!(rig.sim.led(&pad(row, column)).unwrap().colour, 0, "dark");
    }

    let state = rig.snapshot().session.expect("Session Mode's state");
    let whats: Vec<_> = state.pads.iter().take(5).map(|p| p.what).collect();
    assert_eq!(
        whats,
        ["stopped", "playing", "stopped", "recording", "empty"]
    );
}

// Spec: push-2-tracks R63. A Scene/Grid button is lit while its scene holds a
// clip, in the scene's colour or white when it has none.
#[test]
fn a_scene_button_is_lit_while_its_scene_holds_a_clip() {
    let mut rig = started();
    clip_in(&mut rig, 5, 0, None);
    clip_in(&mut rig, 2, 2, None);
    let s = scenes(&rig);
    rig.apply(Command::SetSceneColour {
        scenes: vec![s[2]],
        colour: Some(6),
    });
    rig.tick();
    let colours = rig.shared.track_colours();
    assert_eq!(rig.sim.led("scene_1").unwrap().colour, fixed::WHITE);
    assert_eq!(
        rig.sim.colour("scene_3"),
        Some(bl_model::swatch_rgb(&colours, 6))
    );
    assert_eq!(rig.sim.led("scene_2").unwrap().colour, 0);
    assert_eq!(rig.sim.led("scene_8").unwrap().colour, 0);
}

// Spec: push-2-tracks R64. Shift and a pad that holds a clip open the colour
// picker for it, and a ring pad gives the clip that colour.
#[test]
fn shift_and_a_pad_open_the_colour_picker_for_the_clip() {
    let mut rig = started();
    let clip = clip_in(&mut rig, 1, 1, Some(4));
    rig.tick();
    assert!(rig.sim.press("shift"));
    rig.advance(10);
    press(&mut rig, 0, 0);
    assert!(
        !rig.snapshot().picker,
        "an empty slot has nothing to colour"
    );
    press(&mut rig, 1, 1);
    release(&mut rig, 1, 1);
    assert!(rig.sim.let_go("shift"));
    rig.advance(10);
    assert!(rig.snapshot().picker);
    assert!(rig.commands().is_empty(), "opening it changes nothing");
    // Swatch 2 is the third pad up the left of the ring.
    press(&mut rig, 5, 0);
    assert_eq!(
        rig.commands(),
        [command(
            "clip.set_colour",
            json!({ "clips": [clip], "colour": 2 })
        )]
    );
    assert!(!rig.snapshot().picker);
}

// Spec: push-2-tracks R65. Select and a pad select the slot without launching
// it, and the display shows the clip's name.
#[test]
fn select_and_a_pad_select_the_slot_and_show_the_clips_name() {
    let mut rig = started();
    let (track, scene) = (rig.tracks[2], scenes(&rig)[3]);
    clip_named(&mut rig, 2, 3, None, "Riff");
    rig.tick();
    assert!(rig.sim.press("select"));
    rig.advance(10);
    press(&mut rig, 3, 2);
    release(&mut rig, 3, 2);
    assert!(rig.sim.let_go("select"));
    rig.advance(10);
    assert_eq!(
        rig.commands(),
        [command(
            "session.select",
            json!({ "track": track, "scene": scene })
        )]
    );
    assert_eq!(rig.snapshot().screen.notification.as_deref(), Some("Riff"));
}

// Spec: push-2-tracks R66. Duplicate, a pad with a clip, then other pads copy
// the clip into each; Delete and a pad deletes its clip.
#[test]
fn duplicate_copies_a_clip_to_each_pad_and_delete_deletes_it() {
    let mut rig = started();
    let clip = clip_in(&mut rig, 0, 0, None);
    clip_in(&mut rig, 6, 6, None);
    let (t, s) = (rig.tracks.clone(), scenes(&rig));
    rig.tick();
    assert!(rig.sim.press("duplicate"));
    rig.advance(10);
    press(&mut rig, 0, 0);
    assert!(rig.commands().is_empty(), "the first pad names the clip");
    press(&mut rig, 0, 1);
    press(&mut rig, 4, 2);
    assert!(rig.sim.let_go("duplicate"));
    rig.advance(10);
    assert_eq!(
        rig.commands(),
        [
            command(
                "session.copy_clip_to_slot",
                json!({ "clip": clip, "track": t[1], "scene": s[0] })
            ),
            command(
                "session.copy_clip_to_slot",
                json!({ "clip": clip, "track": t[2], "scene": s[4] })
            ),
        ]
    );
    // Duplicate let go: the next pad launches as it always does.
    press(&mut rig, 6, 6);
    assert_eq!(
        rig.commands(),
        [command(
            "session.launch",
            json!({ "track": t[6], "scene": s[6] })
        )]
    );
    release(&mut rig, 6, 6);
    rig.commands();

    assert!(rig.sim.press("delete"));
    rig.advance(10);
    press(&mut rig, 6, 6);
    press(&mut rig, 7, 7);
    assert!(rig.sim.let_go("delete"));
    rig.advance(10);
    assert_eq!(
        rig.commands(),
        [
            command("session.select", json!({ "track": t[6], "scene": s[6] })),
            command("session.delete", json!({})),
        ],
        "an empty slot is left alone"
    );
}

// Spec: push-2-tracks R67. The arrows move the ring one place, Octave and Page
// eight, and Shift with an arrow eight; a button that can't move it is dark.
#[test]
fn the_arrows_octave_and_page_move_the_ring_and_light_where_they_can() {
    let mut rig = started();
    with_scenes(&mut rig, 20);
    rig.tick();
    let step = |name: &str| {
        command(
            "control_surface.move_ring",
            json!({ "row": 1, "step": name }),
        )
    };
    // At the first track and scene only Down, Right, Octave Down and Page
    // Right move it.
    for (button, lit) in [
        ("up", false),
        ("octave_up", false),
        ("left", false),
        ("page_left", false),
        ("down", true),
        ("octave_down", true),
        ("right", true),
        ("page_right", true),
    ] {
        let colour = rig.sim.led(button).unwrap().colour;
        assert_eq!(colour != 0, lit, "{button}");
    }
    rig.tap("up");
    rig.tap("left");
    assert!(rig.commands().is_empty(), "nothing to move");

    for (button, expected) in [
        ("down", "down"),
        ("right", "right"),
        ("octave_down", "page_down"),
        ("page_right", "page_right"),
    ] {
        rig.tap(button);
        assert_eq!(rig.commands(), [step(expected)], "{button}");
    }
    rig.ring_at(1, 3);
    rig.tick();
    for (button, expected) in [
        ("up", "up"),
        ("left", "left"),
        ("octave_up", "page_up"),
        ("page_left", "page_left"),
    ] {
        rig.tap(button);
        assert_eq!(rig.commands(), [step(expected)], "{button}");
    }
    assert!(rig.sim.press("shift"));
    rig.advance(10);
    rig.tap("right");
    rig.tap("up");
    assert!(rig.sim.let_go("shift"));
    rig.advance(10);
    assert_eq!(rig.commands(), [step("page_right"), step("page_up")]);
}

// Spec: push-2-tracks R68. Stop Clip stops the selected track's clip; with it
// held, a lower button stops its track and the buttons of tracks that play
// pulse; Shift and Stop Clip stop every clip. These hold in every mode,
// including one that uses the lower buttons itself.
#[test]
fn stop_clip_stops_the_selected_track_a_lower_buttons_track_or_every_clip() {
    for claim_lower in [false, true] {
        let mut rig = started();
        rig.flags.lock().unwrap().claim_lower = claim_lower;
        rig.tap("note");
        let (t, s) = (rig.tracks.clone(), scenes(&rig));
        rig.model.selection.tracks = vec![t[2]];
        rig.model.slots = vec![status(t[1], s[0], SlotState::Playing)];
        rig.tick();
        let stop = |track: TrackId| command("session.stop_track", json!({ "track": track }));

        rig.tap("stop_clip");
        assert_eq!(rig.commands(), [stop(t[2])]);

        assert!(rig.sim.press("stop_clip"));
        rig.advance(10);
        assert!(matches!(
            rig.sim.led("lower_2").unwrap().animation,
            Some((_, PULSE))
        ));
        assert_eq!(rig.sim.led("lower_3").unwrap().animation, None);
        rig.tap("lower_4");
        assert!(rig.sim.let_go("stop_clip"));
        rig.advance(10);
        assert_eq!(rig.commands(), [stop(t[3])], "the held button does no tap");

        assert!(rig.sim.press("shift"));
        rig.advance(10);
        rig.tap("stop_clip");
        assert!(rig.sim.let_go("shift"));
        rig.advance(10);
        assert_eq!(rig.commands(), [command("session.stop_all", json!({}))]);
    }
}

// Spec: push-2-tracks R68, push-2 R24. While Stop Clip is locked, each lower
// button stops its track instead of selecting it, the lit tracks pulse, and
// pressing Stop Clip again unlocks it.
#[test]
fn a_locked_stop_clip_makes_the_lower_buttons_stop_their_tracks() {
    let mut rig = started();
    let (t, s) = (rig.tracks.clone(), scenes(&rig));
    rig.model.slots = vec![status(t[4], s[0], SlotState::Playing)];
    rig.tick();
    assert!(rig.sim.press("stop_clip"));
    rig.advance(1000);
    assert!(rig.sim.let_go("stop_clip"));
    rig.advance(10);
    assert_eq!(rig.snapshot().locked, ["stop_clip"]);
    assert!(rig.commands().is_empty());
    assert!(matches!(
        rig.sim.led("lower_5").unwrap().animation,
        Some((_, PULSE))
    ));
    assert!(matches!(
        rig.sim.led("stop_clip").unwrap().animation,
        Some((_, PULSE))
    ));

    rig.tap("lower_5");
    rig.tap("lower_6");
    assert_eq!(
        rig.commands(),
        [
            command("session.stop_track", json!({ "track": t[4] })),
            command("session.stop_track", json!({ "track": t[5] })),
        ]
    );
    rig.tap("stop_clip");
    assert!(rig.snapshot().locked.is_empty());
    rig.commands();
    rig.tap("lower_5");
    assert_eq!(
        rig.commands(),
        [
            command("session.select", json!({ "track": t[4] })),
            command("arrangement.select_tracks", json!({ "tracks": [t[4]] })),
        ]
    );
}

// Spec: push-2-tracks R69, R70. Holding Layout shows Session Overview while
// it is held: the ring's block is white, a block with a clip playing in it
// green, a block with no track or scene dark and any other dim; a pad moves
// the ring to its block and a Scene/Grid button shows a part of 64 scenes.
#[test]
fn session_overview_shows_blocks_and_a_pad_moves_the_ring() {
    let mut rig = started();
    with_scenes(&mut rig, 70);
    let (t, s) = (rig.tracks.clone(), scenes(&rig));
    clip_in(&mut rig, 9, 17, None);
    rig.model.slots = vec![status(t[9], s[17], SlotState::Playing)];
    rig.ring_at(0, 8);
    rig.tick();
    assert!(!rig.snapshot().session.unwrap().overview);

    assert!(rig.sim.press("layout"));
    rig.advance(299);
    assert!(!rig.snapshot().session.unwrap().overview);
    rig.advance(1);
    let state = rig.snapshot().session.unwrap();
    assert!(state.overview && !state.locked);
    // Ten tracks make two columns of blocks, seventy scenes nine rows, the
    // first part holding the first eight; the ring is at scene 8, the second
    // block of the first column.
    assert_eq!(rig.sim.led(&pad(1, 0)).unwrap().colour, fixed::WHITE);
    assert_eq!(rig.sim.led(&pad(2, 1)).unwrap().colour, fixed::GREEN);
    assert_eq!(rig.sim.led(&pad(0, 0)).unwrap().colour, 16, "dim");
    assert_eq!(rig.sim.led(&pad(0, 1)).unwrap().colour, 16, "dim");
    assert_eq!(rig.sim.led(&pad(0, 2)).unwrap().colour, 0, "no track");
    assert_eq!(rig.sim.led("scene_1").unwrap().colour, fixed::WHITE);
    assert_eq!(rig.sim.led("scene_2").unwrap().colour, 16, "a part");
    assert_eq!(rig.sim.led("scene_3").unwrap().colour, 0, "no such part");

    press(&mut rig, 2, 1);
    assert_eq!(
        rig.commands(),
        [command(
            "control_surface.move_ring",
            json!({ "row": 1, "track": t[8], "scene": s[16] })
        )]
    );
    release(&mut rig, 2, 1);
    rig.tap("scene_2");
    assert_eq!(rig.snapshot().session.unwrap().part, 1);
    assert_eq!(rig.sim.led("scene_2").unwrap().colour, fixed::WHITE);
    assert_eq!(rig.sim.led(&pad(0, 0)).unwrap().colour, 16);
    assert_eq!(
        rig.sim.led(&pad(1, 0)).unwrap().colour,
        0,
        "scene 72 is past the set"
    );
    assert!(
        rig.commands().is_empty(),
        "a button of the overview launches nothing"
    );

    assert!(rig.sim.let_go("layout"));
    rig.advance(10);
    assert!(!rig.snapshot().session.unwrap().overview, "released");
}

// Spec: push-2-tracks R69. Shift and Layout lock Session Overview on, pressing
// a pad leaves it locked, and Layout again unlocks it.
#[test]
fn shift_and_layout_lock_the_overview_and_layout_unlocks_it() {
    let mut rig = started();
    assert!(rig.sim.press("shift"));
    rig.advance(10);
    rig.tap("layout");
    assert!(rig.sim.let_go("shift"));
    rig.advance(10);
    let state = rig.snapshot().session.unwrap();
    assert!(state.overview && state.locked);

    press(&mut rig, 0, 0);
    release(&mut rig, 0, 0);
    assert!(rig.snapshot().session.unwrap().overview, "a pad leaves it");

    // Pressing Layout unlocks it, and holding that press shows nothing.
    assert!(rig.sim.press("layout"));
    rig.advance(400);
    assert!(!rig.snapshot().session.unwrap().overview);
    assert!(rig.sim.let_go("layout"));
    rig.advance(10);
    assert!(!rig.snapshot().session.unwrap().overview);
}

// Spec: push-2-tracks R73. The state reads the ring and what each pad and
// Scene/Grid button shows.
#[test]
fn the_state_reads_back_the_ring_and_what_the_pads_show() {
    let mut rig = started();
    clip_in(&mut rig, 0, 0, Some(3));
    rig.ring_at(1, 1);
    rig.tick();
    let state = rig.snapshot().session.unwrap();
    assert_eq!(state.first_track, Some(rig.tracks[1]));
    assert_eq!(state.first_scene, Some(scenes(&rig)[1]));
    assert_eq!((state.tracks, state.scenes), (8, 7));
    assert_eq!(state.pads.len(), 64);
    assert_eq!(state.pads[0].what, "empty");
    assert_eq!(state.pads[7].what, "empty");
    assert_eq!(state.pads[7 * 8 + 7].what, "none", "past the last scene");
    assert_eq!(state.scene_buttons.len(), 8);
    assert_eq!(state.scene_buttons[0].what, "empty");
    assert_eq!(state.scene_buttons[7].what, "none");
    rig.ring_at(0, 0);
    rig.tick();
    let state = rig.snapshot().session.unwrap();
    assert_eq!(state.pads[0].what, "stopped");
    assert_eq!(state.pads[0].colour, Some(3));
    assert_eq!(state.scene_buttons[0].what, "clip");
}

// Spec: push-2-tracks R59, R61. A pad pressed while Session is held, and let go
// after Session was, still sends its release.
#[test]
fn a_pad_let_go_after_the_momentary_mode_ended_still_sends_its_release() {
    let mut rig = Rig::sessions();
    rig.start();
    rig.tick();
    clip_in(&mut rig, 0, 0, None);
    assert!(rig.sim.press("session"));
    rig.advance(300);
    press(&mut rig, 0, 0);
    assert!(rig.sim.let_go("session"));
    rig.advance(1);
    assert_eq!(rig.snapshot().pad_mode, PadMode::Note);
    rig.commands();
    release(&mut rig, 0, 0);
    let (track, scene) = (rig.tracks[0], scenes(&rig)[0]);
    assert_eq!(
        rig.commands(),
        [command(
            "session.release",
            json!({ "track": track, "scene": scene })
        )]
    );
}
