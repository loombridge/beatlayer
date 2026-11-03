//! The Video Window (WP-9.26), headless: View ▸ Video Window and its key,
//! the picture at the playhead scaled to fit with black around it, black
//! where no video clip plays, double-click and Option-double-click, and the
//! window in the accessibility tree. The behaviour is in the video spec, R16
//! to R23, and the accessibility spec, R13.

use std::path::PathBuf;

use crate::common;
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_shell::video::SET_WINDOW;
use bl_shell::{Effect, WindowKind};
use bl_ui::keymap::Platform;
use bl_ui::menus::{ItemState, item_state};
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Shape, TextureId, pos2};
use serde_json::json;

fn movie() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/video/mjpeg-24f.mov")
}

/// A harness on a set with the test movie dropped on its first audio track.
fn with_movie(window: Window) -> Harness {
    let mut h = Harness::manual(window);
    let track = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Audio)
        .map(bl_model::Track::id)
        .unwrap();
    h.shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [movie().display().to_string()], "track": track, "at": 0 }),
        )
        .unwrap();
    h.settle();
    h
}

/// Draws until the video thread has the frame the window wants.
fn decoded(h: &mut Harness) -> egui::FullOutput {
    h.frame(vec![]);
    h.shell.wait_for_video();
    h.frame(vec![])
}

/// The rectangles of the pictures drawn in a frame: meshes with a texture of
/// their own, not the typeface's.
fn pictures(output: &egui::FullOutput) -> Vec<Rect> {
    fn walk(shape: &Shape, out: &mut Vec<Rect>) {
        match shape {
            Shape::Mesh(mesh) if mesh.texture_id != TextureId::default() => {
                out.push(mesh.calc_bounds());
            }
            Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

fn double_click(h: &mut Harness, at: Pos2, modifiers: Modifiers) {
    // Long enough after the last click that this isn't a third.
    for _ in 0..40 {
        h.frame(vec![]);
    }
    h.frame(vec![
        Event::ModifiersChanged(modifiers),
        Event::PointerMoved(at),
    ]);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            }]);
        }
    }
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
}

// Spec: video R17, R18, R23.
#[test]
fn the_picture_is_scaled_to_fit_with_black_around_it_and_black_where_none_plays() {
    let mut h = with_movie(Window::Video);
    let output = decoded(&mut h);
    let drawn = pictures(&output);
    assert_eq!(drawn.len(), 1, "one picture: {drawn:?}");
    // The 64 x 48 movie in the 640 x 360 window: as tall as the window, a
    // third wider than tall, centred, with black either side.
    let window = Rect::from_min_size(Pos2::ZERO, h.size);
    let shown = drawn[0];
    assert!((shown.height() - 360.0).abs() < 0.5, "{shown:?}");
    assert!((shown.width() - 480.0).abs() < 0.5, "{shown:?}");
    assert!((shown.center() - window.center()).length() < 0.5);

    // Playing from after the clip, no video clip plays: the window is black.
    h.shell
        .run("transport.locate", json!({ "position": 705_600 * 6 }))
        .unwrap();
    h.shell.run("transport.play", json!({})).unwrap();
    for _ in 0..20 {
        h.frame(vec![]);
    }
    let output = decoded(&mut h);
    assert!(pictures(&output).is_empty(), "black after the clip");
}

// Spec: video R20.
#[test]
fn a_double_click_goes_full_screen_and_back_and_option_double_click_sets_the_movies_size() {
    let mut h = with_movie(Window::Video);
    decoded(&mut h);
    let middle = pos2(320.0, 180.0);
    h.effects.clear();

    double_click(&mut h, middle, Modifiers::NONE);
    assert_eq!(h.effects, [Effect::SetVideoFullScreen(true)]);
    assert!(h.shell.video_window_full_screen());
    double_click(&mut h, middle, Modifiers::NONE);
    assert_eq!(
        h.effects,
        [
            Effect::SetVideoFullScreen(true),
            Effect::SetVideoFullScreen(false)
        ]
    );
    assert!(!h.shell.video_window_full_screen());

    // Option-double-click (Alt) sets the window to the movie's own size.
    h.effects.clear();
    double_click(&mut h, middle, Modifiers::ALT);
    let [Effect::PlaceVideoWindow(frame)] = h.effects[..] else {
        panic!("one placing: {:?}", h.effects);
    };
    assert_eq!((frame.width, frame.height), (64.0, 48.0));
    assert!(!h.shell.video_window_full_screen());

    // Esc leaves full screen.
    h.shell.set_focused_window(WindowKind::Video);
    h.shell
        .run(SET_WINDOW, json!({ "full_screen": true }))
        .unwrap();
    h.settle();
    h.effects.clear();
    h.key(Key::Escape, Modifiers::NONE);
    assert_eq!(h.effects, [Effect::SetVideoFullScreen(false)]);
    assert!(!h.shell.video_window_full_screen());
    // And does nothing outside it.
    h.effects.clear();
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.effects.is_empty());
}

// Spec: video R16; S-VIEW-009, M-VIEW-029.
#[test]
fn the_view_menu_item_and_its_key_show_and_hide_the_window() {
    let mut h = Harness::manual(Window::Main);
    h.settle();
    let toggle = bl_shell::video::TOGGLE_WINDOW;
    // A new set has no video clip: the item is disabled and not ticked, and
    // the key does nothing.
    let before = menu_item(&h);
    assert_eq!(
        before.invocation.as_ref().map(|i| i.id.as_str()),
        Some(toggle)
    );
    assert!(!before.enabled);
    assert_eq!(before.checked, Some(false));
    let keys = common::command() | Modifiers::ALT | Modifiers::SHIFT;
    h.key(Key::V, keys);
    assert!(!h.shell.video_window_shown());

    let track = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Audio)
        .map(bl_model::Track::id)
        .unwrap();
    h.shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [movie().display().to_string()], "track": track, "at": 0 }),
        )
        .unwrap();
    h.settle();
    // Dropping the movie showed it; the item is enabled and ticked, and
    // shows the key.
    let shown = menu_item(&h);
    assert!(shown.enabled);
    assert_eq!(shown.checked, Some(true));
    h.key(Key::V, keys);
    assert!(!h.shell.video_window_shown());
    assert_eq!(menu_item(&h).checked, Some(false));
    assert!(menu_item(&h).enabled);
    h.key(Key::V, keys);
    assert!(h.shell.video_window_shown());
    assert_eq!(menu_item(&h).checked, Some(true));
}

/// The View menu's Video Window item as the menu draws it now.
fn menu_item(h: &Harness) -> ItemState {
    let bar = menu_bar(MenuPlatform::current());
    let item = bar
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == "M-VIEW-029")
        .expect("M-VIEW-029 is in the menus");
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

// Spec: video R22; accessibility R13.
#[test]
fn the_window_is_named_video_and_its_picture_is_an_image_of_the_clip_and_the_frames_time() {
    let mut h = with_movie(Window::Video);
    assert_eq!(
        bl_ui::main_window::window_title(&h.shell, WindowKind::Video),
        "Video"
    );
    let nodes = Harness::nodes(&decoded(&mut h));
    let roots: Vec<_> = nodes.iter().filter(|n| n.role == Role::Window).collect();
    assert_eq!(roots.len(), 1, "one root, the window");
    let picture = Harness::node(&nodes, "Picture");
    assert_eq!(picture.role, Role::Image);
    assert!(Harness::descends_from(&nodes, picture.id, roots[0].id));
    let name = h.shell.video_view().name;
    assert_eq!(picture.description, format!("{name}, 0:00.000"));

    // The description follows the playhead: black, with no clip playing.
    h.shell
        .run("transport.locate", json!({ "position": 705_600 * 6 }))
        .unwrap();
    h.shell.run("transport.play", json!({})).unwrap();
    for _ in 0..20 {
        h.frame(vec![]);
    }
    let nodes = Harness::nodes(&decoded(&mut h));
    assert_eq!(
        Harness::node(&nodes, "Picture").description,
        "Black, no video clip at the playhead"
    );
}
