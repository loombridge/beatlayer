//! Clip Mode (`docs/specs/push-2-tracks.md`, R43 to R58) on the simulated
//! Push 2, with a read model the test builds by hand: the clip's name and
//! colour, the notes' opacity, a waveform, the folded display and the
//! sequencers' boxes, the display following a playing clip, and the controls
//! `push.state` reads back. The display is judged by its pixels.

use std::sync::Arc;

use bl_control::Waveform;
use bl_core::time::Ticks;
use bl_engine::{ClipOffset, SlotState, SlotStatus};
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, MidiData, Note, SceneId, TrackId,
    swatch_rgb,
};
use bl_push::content::DisplayMode;
use bl_push::frame::WIDTH;
use bl_push::paint;
use serde_json::json;

use crate::rig::Rig;

/// A beat, in ticks.
const BEAT: i64 = 705_600;
/// The clip's colour swatch.
const SWATCH: u8 = 2;

fn colour() -> [u8; 3] {
    swatch_rgb(&bl_model::DEFAULT_TRACK_PALETTE, SWATCH)
}

fn scene(rig: &Rig) -> SceneId {
    rig.editor.document().scenes().next().map(|s| s.id).unwrap()
}

/// A clip of one bar in the first track's first slot.
fn clip(rig: &mut Rig, data: ClipData) -> ClipId {
    let (track, scene) = (rig.tracks[0], scene(rig));
    let id: ClipId = rig.editor.new_id();
    let mut clip = Clip::new(id, Ticks::ZERO, Ticks::WHOLE, data);
    clip.colour = Some(SWATCH);
    clip.name = "Riff".to_owned();
    rig.apply(Command::CreateSlotClip { track, scene, clip });
    id
}

fn notes(spans: &[(u8, i64, i64, u8)]) -> ClipData {
    ClipData::Midi(MidiData::new(spans.iter().map(
        |&(key, from, to, velocity)| Note::new(key, Ticks(from), Ticks(to - from), velocity),
    )))
}

/// Clip Mode shown over Note Mode, with the first track selected and the
/// Session View the main view.
fn started() -> Rig {
    let mut rig = Rig::clipping();
    rig.model.selection.tracks = vec![rig.tracks[0]];
    rig.model.session_view = true;
    rig.start();
    rig.tick();
    rig
}

fn show(rig: &mut Rig) {
    rig.tap("clip");
    rig.tick();
    assert_eq!(rig.snapshot().display_mode, DisplayMode::Clip);
}

fn frame(rig: &Rig) -> Vec<u8> {
    paint::render(&rig.snapshot().screen)
}

fn pixel(rgb: &[u8], x: usize, y: usize) -> [u8; 3] {
    let at = (y * WIDTH + x) * 3;
    [rgb[at], rgb[at + 1], rgb[at + 2]]
}

fn near(got: [u8; 3], want: [u8; 3], by: u8) -> bool {
    got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= by)
}

fn text(rig: &Rig) -> Vec<String> {
    rig.snapshot().screen.text()
}

fn has(rig: &Rig, line: &str) -> bool {
    text(rig).iter().any(|l| l == line)
}

// Spec: push-2-tracks R44. The display is drawn in the clip's colour with
// its name highlighted at the top left, and each note's opacity is its
// velocity.
#[test]
fn the_display_names_the_clip_in_its_colour_and_a_notes_opacity_is_its_velocity() {
    let mut rig = started();
    clip(
        &mut rig,
        notes(&[(40, 0, BEAT, 127), (60, 2 * BEAT, 3 * BEAT, 40)]),
    );
    show(&mut rig);
    let rgb = frame(&rig);
    // The name sits on a band of the clip's colour at the top left.
    assert!(
        near(pixel(&rgb, 16, 27), colour(), 1),
        "{:?}",
        pixel(&rgb, 16, 27)
    );
    // Folded to the two pitches that hold notes: 60 on the top row, 40
    // below. The first note is as opaque as velocity 127 makes it, the
    // second about a third of that.
    let loud = pixel(&rgb, 100, 80);
    let quiet = pixel(&rgb, 600, 57);
    assert!(near(loud, colour(), 1), "{loud:?}");
    let want = colour().map(|c| (f32::from(c) * 40.0 / 127.0) as u8);
    assert!(near(quiet, want, 4), "{quiet:?} not {want:?}");
    assert!(has(&rig, "clip: Riff"));
    assert!(has(&rig, "clip shows: 2 notes on 2 rows"));
    crate::display::golden("clip", &rig.snapshot().screen);
}

// Spec: push-2-tracks R43. An empty MIDI slot says so, a track with nothing
// selected has nothing to show, and the display follows the window's
// selection.
#[test]
fn an_empty_slot_says_so_and_the_display_follows_the_selection() {
    let mut rig = started();
    show(&mut rig);
    assert_eq!(rig.snapshot().clip.as_ref().unwrap().shows, "empty_midi");
    assert!(has(&rig, "clip shows: This slot is empty."));
    // A clip put in the slot shows at once.
    let id = clip(&mut rig, notes(&[(40, 0, BEAT, 100)]));
    rig.tick();
    assert_eq!(rig.snapshot().clip.as_ref().unwrap().clip, Some(id));
    // With nothing selected there is nothing.
    rig.model.selection.tracks.clear();
    rig.tick();
    assert_eq!(rig.snapshot().clip.as_ref().unwrap().shows, "nothing");
}

// Spec: push-2-tracks R44. An audio clip shows its waveform, drawn in the
// clip's colour.
#[test]
fn an_audio_clip_shows_its_waveform() {
    let mut rig = started();
    // The set's tracks are MIDI tracks: an audio track holds the clip.
    let track: TrackId = rig.editor.new_id();
    rig.apply(Command::CreateTrack {
        id: track,
        kind: bl_model::TrackKind::Audio,
        name: "Audio".to_owned(),
        parent: None,
        index: 10,
    });
    rig.model.selection.tracks = vec![track];
    let (scene, id): (SceneId, ClipId) = (scene(&rig), rig.editor.new_id());
    let mut audio = Clip::new(
        id,
        Ticks::ZERO,
        Ticks::WHOLE,
        ClipData::Audio(AudioData::new(ContentHash([7; 32]))),
    );
    audio.colour = Some(SWATCH);
    rig.apply(Command::CreateSlotClip {
        track,
        scene,
        clip: audio,
    });
    show(&mut rig);
    assert!(has(&rig, "clip shows: Building the waveform"));
    rig.model.waveform = Some(Arc::new(Waveform {
        clip: id,
        from: 0.0,
        to: 4.0,
        peaks: vec![[-0.5, 0.5]; 100],
    }));
    rig.tick();
    assert!(has(&rig, "clip shows: waveform of 240"));
    let rgb = frame(&rig);
    // The middle of the display is inside the wave, its top outside.
    assert!(near(pixel(&rgb, 400, 69), colour(), 1));
    assert_eq!(pixel(&rgb, 400, 48), [0, 0, 0]);
    let names: Vec<_> = rig
        .snapshot()
        .clip
        .unwrap()
        .controls
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(
        names,
        ["Zoom", "Start", "End", "Warp Mode", "Gain", "Transpose"]
    );
}

// Spec: push-2-tracks R50, R71. In a real-time layout the display is folded
// to the pitches that hold notes, and in Session Mode it is folded too;
// R51. The step sequencing layouts show their sequenceable area.
#[test]
fn the_layouts_fold_the_display_or_mark_their_sequenceable_area() {
    let mut rig = started();
    clip(
        &mut rig,
        notes(&[
            (40, 0, BEAT, 100),
            (50, BEAT, 2 * BEAT, 100),
            (62, 2 * BEAT, 3 * BEAT, 100),
        ]),
    );
    show(&mut rig);
    // 64 Notes: folded, three rows.
    let state = rig.snapshot().clip.unwrap();
    assert!(state.folded, "{state:?}");
    assert!(has(&rig, "clip shows: 3 notes on 3 rows"));

    // The Melodic Sequencer: a page of eight pitches, with a box over it
    // and the pitch range at the left edge.
    rig.tap("layout");
    rig.tick();
    let state = rig.snapshot().clip.unwrap();
    assert!(!state.folded);
    let area = state.area.expect("an area");
    assert_eq!(area.fit, bl_push::note::Fit::Sequencer);
    assert_eq!((area.from.get(), area.to.get()), (0, 2 * BEAT));
    assert!(has(&rig, "clip area: 0.00 to 2.00"));
    assert!(text(&rig).iter().any(|l| l.starts_with("clip range: ")));
    let rgb = frame(&rig);
    // The box is a translucent white over the clip's first two beats and
    // nothing beyond them.
    let inside = pixel(&rgb, 400, 47);
    assert!(inside[0] > 20 && inside[0] < 80, "{inside:?}");
    assert_eq!(pixel(&rgb, 700, 47), [0, 0, 0]);
    // The left edge holds the box of the pitch range and a line for each
    // range of eight pitches that holds notes.
    let edge: Vec<[u8; 3]> = (46..92).map(|y| pixel(&rgb, 5, y)).collect();
    assert!(edge.iter().any(|p| *p != [0, 0, 0]), "the range box");
    assert!(
        (46..92).any(|y| near(pixel(&rgb, 1, y), colour(), 2)),
        "the lines of the notes' ranges"
    );

    // A later page moves the box along the clip.
    rig.tap("page_right");
    rig.tick();
    assert!(has(&rig, "clip area: 2.00 to 4.00"), "{:?}", text(&rig));

    // The Melodic Sequencer + 32 Notes fits every note of the clip.
    rig.tap("layout");
    rig.tick();
    assert!(
        has(&rig, "clip shows: 3 notes on 23 rows"),
        "{:?}",
        text(&rig)
    );

    // Session Mode shows the folded display (R71).
    rig.tap("session");
    rig.tick();
    assert!(rig.snapshot().clip.unwrap().folded);
    assert!(has(&rig, "clip shows: 3 notes on 3 rows"));
}

// Spec: push-2-tracks R44. The display follows a playing clip: zoomed in,
// its span scrolls with the play position on the test clock.
#[test]
fn the_display_follows_a_playing_clip() {
    let mut rig = started();
    let id = clip(&mut rig, notes(&[(40, 0, BEAT, 100)]));
    let (track, scene) = (rig.tracks[0], scene(&rig));
    show(&mut rig);
    assert!(rig.sim.touch(0));
    rig.advance(10);
    assert!(rig.sim.turn(0, 40));
    rig.advance(10);
    assert!(rig.sim.release(0));
    rig.advance(10);
    let state = rig.snapshot().clip.unwrap();
    assert_eq!(state.zoom, 2.0, "four levels double it");
    assert_eq!(state.view, (0.0, 2.0));

    rig.model.slots = vec![SlotStatus {
        track,
        scene,
        state: SlotState::Playing,
        started: None,
        offset: ClipOffset::ZERO,
        velocity: 127,
        transpose: 0,
    }];
    rig.model.clip_time = vec![(track, Ticks(BEAT / 2))];
    rig.advance(20);
    assert!(has(&rig, "clip playhead: 0.50"), "{:?}", text(&rig));
    assert_eq!(rig.snapshot().clip.unwrap().view, (0.0, 2.0));
    rig.model.clip_time = vec![(track, Ticks(3 * BEAT))];
    rig.advance(20);
    assert!(has(&rig, "clip playhead: 3.00"), "{:?}", text(&rig));
    assert_eq!(
        rig.snapshot().clip.unwrap().view,
        (2.0, 4.0),
        "the span moved on with the play position"
    );
    let _ = id;
}

// Spec: push-2-tracks R46. Zoom is the first encoder: it zooms around the
// marker of the position encoder last touched, and changes only the display.
#[test]
fn zoom_centres_on_the_marker_last_touched_and_asks_for_no_command() {
    let mut rig = started();
    clip(&mut rig, notes(&[(40, 0, BEAT, 100)]));
    show(&mut rig);
    rig.commands();
    // The loop is off, so touching End makes the clip's end the anchor.
    assert!(rig.sim.touch(2));
    rig.advance(10);
    assert!(rig.sim.release(2));
    rig.advance(10);
    assert!(rig.sim.touch(0));
    rig.advance(10);
    assert!(rig.sim.turn(0, 40));
    assert!(rig.sim.turn(0, 40));
    rig.advance(10);
    assert!(rig.sim.release(0));
    rig.advance(10);
    let state = rig.snapshot().clip.unwrap();
    assert_eq!(state.zoom, 4.0);
    assert_eq!(state.view, (3.0, 4.0), "around End, at the clip's end");
    assert!(
        rig.commands().iter().all(|c| c.starts_with("end ")),
        "no command but the gesture's end"
    );
}

// Spec: push-2-tracks R58. `push.state` reads the clip, the controls on the
// encoders with their values, the zoom and the view.
#[test]
fn the_state_reads_back_the_clip_its_controls_and_the_zoom() {
    let mut rig = started();
    let id = clip(&mut rig, notes(&[(40, 0, BEAT, 100)]));
    show(&mut rig);
    let state = rig.snapshot().clip.unwrap();
    assert_eq!(
        (state.clip, state.shows, state.name.as_str()),
        (Some(id), "midi", "Riff")
    );
    assert_eq!(state.looping, Some(false));
    assert_eq!(state.zoom, 1.0);
    assert_eq!(state.view, (0.0, 4.0));
    let controls: Vec<_> = state
        .controls
        .iter()
        .map(|c| (c.column, c.name.as_str(), c.value.as_str()))
        .collect();
    assert_eq!(
        controls,
        [
            (0, "Zoom", "1.0x"),
            (1, "Start", "1.1.1"),
            (2, "End", "2.1.1")
        ],
        "Loop is off, and the clip is a bar long"
    );
    // The second upper button switches Loop, and the third is Crop.
    rig.commands();
    rig.tap("upper_2");
    rig.tap("upper_3");
    assert_eq!(
        rig.commands(),
        [
            json!({ "clip": id, "enabled": true }),
            json!({ "clip": id }),
        ]
        .iter()
        .zip(["clip.set_loop_enabled", "clip.crop"])
        .map(|(args, id)| format!("{id} {args}"))
        .collect::<Vec<_>>()
    );
    let upper = text(&rig);
    assert!(upper.iter().any(|l| l == "upper 2: Loop"), "{upper:?}");
    assert!(upper.iter().any(|l| l == "upper 3: Crop"), "{upper:?}");
}

// Spec: push-2-tracks R45. A loop or a clip shorter than the step an encoder
// moves by leaves the markers where they are, and the surface carries on.
#[test]
fn a_loop_shorter_than_a_step_leaves_the_markers_alone() {
    let mut rig = started();
    let mut data = MidiData::new([Note::new(40, Ticks(0), Ticks(BEAT / 8), 100)]);
    data.looping = true;
    data.loop_end = Some(Ticks(BEAT / 4));
    clip(&mut rig, ClipData::Midi(data));
    show(&mut rig);
    for column in [1, 2, 3] {
        assert!(rig.sim.touch(column));
        rig.advance(10);
        assert!(rig.sim.turn(column, 10));
        rig.advance(10);
        assert!(rig.sim.turn(column, -10));
        rig.advance(10);
        assert!(rig.sim.release(column));
        rig.advance(10);
    }
    assert_eq!(rig.snapshot().clip.unwrap().shows, "midi");
}
