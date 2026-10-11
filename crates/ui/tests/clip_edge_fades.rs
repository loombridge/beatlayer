//! Create Fades on Clip Edges in the Settings window (WP-1.4b), headless:
//! on the Record, Warp & Launch page, in its Warp and Fades section
//! (L-SET-018, laid out as L-SET-003), a switch that a screen reader reaches
//! and that runs the registered command.

mod common;

use bl_shell::settings::SettingsPage;
use common::{Harness, Window};
use egui::Id;
use egui::accesskit::{Action, Role};
use serde_json::json;

// Spec: audio-clips R22
#[test]
fn the_record_warp_and_launch_page_switches_create_fades_on_clip_edges() {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": "record-warp-launch" }))
        .unwrap();
    h.shell.take_effects();
    h.settle();
    assert_eq!(h.shell.settings_page(), SettingsPage::RecordWarpLaunch);
    assert!(SettingsPage::RecordWarpLaunch.is_built());

    let nodes = Harness::nodes(&h.frame(vec![]));
    let switch = Harness::node(&nodes, "Create Fades on Clip Edges");
    assert_eq!(switch.role, Role::Button);
    assert!(!h.shell.create_fades_on_clip_edges());

    h.accesskit(
        Id::new(("switch", "create-fades-on-clip-edges")),
        Action::Click,
    );
    h.settle();
    assert!(h.shell.create_fades_on_clip_edges());
    assert!(h.events.iter().any(|e| matches!(
        e,
        bl_ui::main_window::UiEvent::Invoke(i)
            if i.id == "settings.set_create_fades_on_clip_edges" && i.args == json!({ "on": true })
    )));

    h.accesskit(
        Id::new(("switch", "create-fades-on-clip-edges")),
        Action::Click,
    );
    h.settle();
    assert!(!h.shell.create_fades_on_clip_edges());
}
