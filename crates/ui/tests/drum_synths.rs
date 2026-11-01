//! A Drum Synth's display in the Device View (WP-8.24), headless: a click in
//! its upper half runs `device.preview_hit`, a click in its lower half does
//! not, and the click changes nothing in the set.
//!
//! Spec: drum-synths R6.

use crate::common;
use bl_devices::ds_kick::ds_kick_type;
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_ui::main_window::UiEvent;
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, pos2};
use serde_json::json;

fn window() -> (Harness, DeviceId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();
    let kick = ds_kick_type();
    let device = json!({ "kind": kick.kind, "name": kick.name, "role": kick.role });
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    h.settle();
    (h, added.device)
}

fn click_at(h: &mut Harness, at: egui::Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    h.settle();
}

// Spec: drum-synths R6
#[test]
fn a_click_in_the_upper_half_of_the_display_previews_a_hit_and_changes_nothing() {
    let (mut h, device) = window();
    let nodes = Harness::nodes(&h.settle());
    let button = Harness::node(&nodes, "Preview Hit").bounds.unwrap();
    let before = h.shell.editor().document().clone();
    let steps = h.shell.editor().undo_history().len();
    h.take_events();

    // The button is the display's upper half; below it is the lower half.
    let x = ((button.x0 + button.x1) / 2.0) as f32;
    let lower = pos2(x, (button.y1 + 20.0) as f32);
    click_at(&mut h, lower);
    assert!(
        !h.take_events().iter().any(|event| matches!(
            event,
            UiEvent::Invoke(invocation) if invocation.id == "device.preview_hit"
        )),
        "a click in the lower half plays nothing"
    );
    let upper = pos2(x, (button.y0 + 10.0) as f32);
    click_at(&mut h, upper);
    let wanted = bl_shell::Invocation::new("device.preview_hit", json!({ "device": device }));
    assert!(
        h.take_events().contains(&UiEvent::Invoke(wanted)),
        "a click in the upper half previews a hit"
    );
    assert_eq!(h.shell.editor().undo_history().len(), steps);
    assert_eq!(*h.shell.editor().document(), before);
}
