//! The real-time render's dialog (WP-5.35; the external devices spec,
//! R23): the waiting period counts down, and Restart, Skip and Cancel do
//! what they say.

use crate::common;
use bl_shell::export_commands::RealtimePhase;
use common::{Harness, Window};
use egui::{FullOutput, pos2};

fn labels(output: &FullOutput) -> Vec<String> {
    Harness::nodes(output)
        .iter()
        .map(|node| node.label.clone())
        .collect()
}

/// Clicks the button labelled `label`.
fn click(h: &mut Harness, label: &str) -> FullOutput {
    let nodes = Harness::nodes(&h.settle());
    let node = nodes
        .iter()
        .find(|node| node.label == label)
        .unwrap_or_else(|| panic!("no {label:?} in {nodes:?}"));
    let bounds = node.bounds.expect("the button is drawn");
    h.click(pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ))
}

// Spec: external-devices R23
#[test]
fn the_dialog_counts_down_and_its_buttons_work() {
    let mut h = Harness::with_title_bar(Window::Main);
    // The render waits, as it does before a render through hardware.
    bl_shell::testing::realtime_waiting(&h.shell, 10);
    let shown = labels(&h.settle());
    for button in ["Restart", "Skip", "Cancel"] {
        assert!(
            shown.iter().any(|label| label == button),
            "the dialog has {button}: {shown:?}"
        );
    }
    let status = h.shell.realtime_status();
    assert!(
        matches!(status.phase, RealtimePhase::Waiting { seconds } if seconds > 1.0),
        "the render is waiting: {status:?}"
    );

    // Skip starts the render at once: no time is left to wait.
    click(&mut h, "Skip");
    let status = h.shell.realtime_status();
    assert_eq!(
        status.phase,
        RealtimePhase::Waiting { seconds: 0.0 },
        "the wait is over: {status:?}"
    );

    // Restart starts the count again.
    click(&mut h, "Restart");
    let status = h.shell.realtime_status();
    assert!(
        matches!(status.phase, RealtimePhase::Waiting { seconds } if seconds > 1.0),
        "the count started again: {status:?}"
    );

    // Cancel stops the render.
    click(&mut h, "Cancel");
    assert!(
        h.shell.realtime_session().cancelled(),
        "the render was cancelled"
    );
}
