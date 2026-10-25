//! Tracks, clips and scenes drawn in their colours (WP-6.30b; the themes
//! spec, R18), headless: the Arrangement's headers, the Session View's title
//! bars, colour strips, slots and scene name fields, and the Clip View's
//! title bar, striped with several clips selected.

use crate::common;
use bl_core::time::Ticks;
use bl_model::{AudioData, Clip, ClipData, ClipId, Command, ContentHash, SceneId, TrackId};
use bl_shell::files::factory_default_set;
use bl_ui::arrangement::benchmark;
use common::{Harness, Window, painted_rects};
use egui::{Color32, Rect};
use serde_json::json;

fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    h
}

fn tracks(h: &Harness) -> Vec<TrackId> {
    h.shell.editor().document().top_level().collect()
}

/// Whether the frame paints a rectangle in `colour`.
fn paints(h: &mut Harness, colour: Color32) -> bool {
    let output = h.settle();
    painted_rects(&output)
        .iter()
        .any(|(_, fill)| *fill == colour)
}

// Spec: themes R18. A track's title in the Arrangement's headers is drawn in
// its colour, and follows it when it changes.
#[test]
fn a_track_s_header_is_drawn_in_its_colour() {
    let mut h = harness();
    let audio = tracks(&h)[2];
    // Swatch 55 is not what any track of the factory set has.
    let swatch = h.theme.swatch(55).gamma_multiply(0.82);
    assert!(!paints(&mut h, swatch));
    h.shell
        .run(
            "track.set_colour",
            json!({ "tracks": [audio], "colour": 55 }),
        )
        .unwrap();
    assert!(paints(&mut h, swatch), "the header's colour is the track's");
}

// Spec: themes R18. In the Session View a track's title bar and the strip
// along its column's bottom are its colour, a clip's slot is the clip's, and
// a scene's name field is the scene's.
#[test]
fn the_session_view_draws_titles_strips_slots_and_scene_names_in_colours() {
    let mut h = harness();
    let audio = tracks(&h)[2];
    let scene: SceneId = h.shell.editor().document().scenes().next().unwrap().id;
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": tracks(&h)[0], "scene": scene }),
        )
        .unwrap();
    let midi = tracks(&h)[0];
    let clip = h
        .shell
        .editor()
        .document()
        .slot(midi, scene)
        .map(|clip| clip.id)
        .unwrap();
    h.shell
        .run(
            "track.set_colour",
            json!({ "tracks": [audio], "colour": 55 }),
        )
        .unwrap();
    h.shell
        .run("clip.set_colour", json!({ "clips": [clip], "colour": 31 }))
        .unwrap();
    h.shell
        .run(
            "scene.set_colour",
            json!({ "scenes": [scene], "colour": 66 }),
        )
        .unwrap();
    let theme = h.theme.clone();
    // The colour strip is the track's colour as it is; the title bar is
    // drawn a little dimmer.
    assert!(paints(&mut h, theme.swatch(55)), "the colour strip");
    assert!(
        paints(&mut h, theme.swatch(55).gamma_multiply(0.82)),
        "the title bar"
    );
    assert!(
        paints(&mut h, theme.swatch(31).gamma_multiply(0.75)),
        "the clip's slot is the clip's colour"
    );
    assert!(paints(&mut h, theme.swatch(66)), "the scene's name field");
}

// Spec: themes R18. An audio clip's Clip View title bar is the shown clip's colour,
// and with several clips selected it holds a stripe of each one's.
#[test]
fn the_clip_view_s_title_bar_is_striped_with_several_clips_selected() {
    let mut editor = factory_default_set();
    // The first audio track: the Clip View's title bar is an audio clip's.
    let track = editor.document().top_level().nth(2).unwrap();
    let mut clips: Vec<ClipId> = Vec::new();
    for bar in 0..2 {
        let id: ClipId = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    id,
                    Ticks::WHOLE * 4 * bar,
                    Ticks::WHOLE * 2,
                    ClipData::Audio(AudioData::new(ContentHash([3; 32]))),
                ),
            })
            .unwrap();
        clips.push(id);
    }
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.settle();
    h.shell
        .run(
            "clip.set_colour",
            json!({ "clips": [clips[0]], "colour": 20 }),
        )
        .unwrap();
    h.shell
        .run(
            "clip.set_colour",
            json!({ "clips": [clips[1]], "colour": 62 }),
        )
        .unwrap();
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clips[0]] }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    let theme = h.theme.clone();
    let title = |h: &mut Harness, colour: Color32| -> Vec<Rect> {
        let output = h.settle();
        painted_rects(&output)
            .into_iter()
            .filter(|(rect, fill)| {
                *fill == colour && (rect.height() - bl_ui::clip_view::TITLE_BAR_HEIGHT).abs() < 0.5
            })
            .map(|(rect, _)| rect)
            .collect()
    };
    let one = title(&mut h, theme.swatch(20));
    assert_eq!(one.len(), 1, "the shown clip's colour fills its title bar");
    assert!(title(&mut h, theme.swatch(62)).is_empty());

    h.shell
        .run(
            "arrangement.select_clips",
            json!({ "clips": [clips[0], clips[1]] }),
        )
        .unwrap();
    let first = title(&mut h, theme.swatch(20));
    let second = title(&mut h, theme.swatch(62));
    assert_eq!((first.len(), second.len()), (1, 1), "a stripe of each");
    assert!(first[0].width() < one[0].width());
    assert!((first[0].width() - second[0].width()).abs() < 0.5);
    assert!(first[0].left() < second[0].left());
}

// Spec: themes R15. The palette's top row is the theme's 14 track colours as
// its file gives them, the second and third rows mix them 30 % and 60 % of the
// way towards white, the fourth and fifth 30 % and 50 % towards black.
#[test]
fn the_palette_s_rows_are_the_tracks_colours_lightened_and_darkened() {
    let h = harness();
    let theme = &h.theme;
    let light = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
    let mut lighter = 0;
    for column in 0..14u8 {
        let base = theme.colors.track_palette[usize::from(column)].0;
        assert_eq!(theme.swatch(column), base, "row 1 is the theme's colour");
        let rows: Vec<u32> = (0..5)
            .map(|row| light(theme.swatch(row * 14 + column)))
            .collect();
        assert!(
            rows[1] >= rows[0] && rows[2] >= rows[1],
            "rows 2 and 3 are lighter"
        );
        assert!(
            rows[3] <= rows[0] && rows[4] <= rows[3],
            "rows 4 and 5 are darker"
        );
        lighter += usize::from(rows[2] > rows[0] && rows[4] < rows[0]);
    }
    assert!(lighter > 0, "the rows differ");
}
