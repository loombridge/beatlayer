//! Create Fades on Clip Edges in the Settings window (WP-1.4b), headless:
//! on the Record, Warp & Launch page, the fourth row of its Warp/Fades
//! section, below the Record section and above Launch and Tap Tempo, as
//! observed in Live 12.4.6 (L-SET-018, laid out as L-SET-003): an On/Off
//! switch that a screen reader reaches and that runs the registered
//! command. The page's other rows keep their places, pending.

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
    assert!(!switch.disabled);
    // Live's rows in Live's order, every other one pending: Record's seven,
    // then Warp/Fades' three before the switch, then Launch and Tap Tempo.
    let top = |label: &str| {
        let node = Harness::node(&nodes, label);
        assert!(
            node.disabled || label == "Create Fades on Clip Edges",
            "{label} is pending"
        );
        if label != "Create Fades on Clip Edges" {
            assert!(node.description.contains("Not built yet"), "{label}");
        }
        node.bounds.expect("drawn").y0
    };
    let order = [
        "File Type",
        "Bit Depth",
        "Count-In",
        "Exclusive Arm",
        "Clip Update Rate",
        "Record Session automation in",
        "Start Transport With Record",
        "Loop/Warp Short Samples",
        "Auto-Warp Long Samples",
        "Default Warp Mode",
        "Create Fades on Clip Edges",
        "Default Launch Mode",
        "Default Launch Quantization",
        "Select on Launch",
        "Select Next Scene on Launch",
        "Start Recording on Scene Launch",
        "Start Transport With Tap Tempo",
    ];
    let tops: Vec<f64> = order.iter().map(|label| top(label)).collect();
    assert!(
        tops.windows(2).all(|pair| pair[0] < pair[1]),
        "rows in Live's order: {tops:?}"
    );
    assert_eq!(top("Exclusive Solo"), top("Exclusive Arm"), "side by side");
    let window = Harness::node(&nodes, "Start Transport With Tap Tempo");
    assert!(
        window.bounds.unwrap().y1 <= 617.0,
        "the page fits the window"
    );
    // On, as Live's is on a new install.
    assert!(h.shell.create_fades_on_clip_edges());

    h.accesskit(
        Id::new(("switch", "create-fades-on-clip-edges")),
        Action::Click,
    );
    h.settle();
    assert!(!h.shell.create_fades_on_clip_edges());
    assert!(h.events.iter().any(|e| matches!(
        e,
        bl_ui::main_window::UiEvent::Invoke(i)
            if i.id == "settings.set_create_fades_on_clip_edges" && i.args == json!({ "on": false })
    )));

    h.accesskit(
        Id::new(("switch", "create-fades-on-clip-edges")),
        Action::Click,
    );
    h.settle();
    assert!(h.shell.create_fades_on_clip_edges());
}

// Spec: recording R13. Warp recorded audio is a switch after Create Fades on
// Clip Edges, on the Record, Warp & Launch page, and runs its command.
#[test]
fn the_record_warp_and_launch_page_switches_warp_recorded_audio() {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": "record-warp-launch" }))
        .unwrap();
    h.shell.take_effects();
    h.settle();

    let nodes = Harness::nodes(&h.frame(vec![]));
    let switch = Harness::node(&nodes, "Warp recorded audio");
    assert_eq!(switch.role, Role::Button);
    assert!(!switch.disabled);
    let fades = Harness::node(&nodes, "Create Fades on Clip Edges");
    let launch = Harness::node(&nodes, "Default Launch Mode");
    assert!(
        fades.bounds.unwrap().y0 < switch.bounds.unwrap().y0
            && switch.bounds.unwrap().y0 < launch.bounds.unwrap().y0,
        "after the Warp/Fades rows, before Launch"
    );
    // On, as Live warps its recordings.
    assert!(h.shell.warp_recorded_audio());

    h.accesskit(Id::new(("switch", "warp-recorded-audio")), Action::Click);
    h.settle();
    assert!(!h.shell.warp_recorded_audio());
    assert!(h.events.iter().any(|e| matches!(
        e,
        bl_ui::main_window::UiEvent::Invoke(i)
            if i.id == "settings.set_warp_recorded_audio" && i.args == json!({ "on": false })
    )));

    h.accesskit(Id::new(("switch", "warp-recorded-audio")), Action::Click);
    h.settle();
    assert!(h.shell.warp_recorded_audio());
}
