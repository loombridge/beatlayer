//! The Status Bar's sound analysis progress and its Pause button, headless
//! (WP-9.8; the sound similarity spec, R6): while files wait to be
//! analysed the message area says how far analysis has got, with a Pause
//! button at its right end that reads Resume once it is pressed.

use std::time::Duration;

use crate::common::{Harness, Window};
use egui::pos2;

fn wav(path: &std::path::Path) {
    let data: Vec<u8> = (0..4_410u32)
        .flat_map(|i| (((i % 100) as i16 - 50) * 100).to_le_bytes())
        .collect();
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    for word in [1u16, 1] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(&22_050u32.to_le_bytes());
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    for word in [2u16, 16] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    std::fs::write(path, bytes).unwrap();
}

// Spec: sound-similarity R6.
#[test]
fn the_status_bar_shows_the_progress_and_pause_reads_resume_once_pressed() {
    let dir = std::env::temp_dir().join(format!("bl-ui-analysis-{}", std::process::id()));
    let library = dir.join("Library");
    std::fs::create_dir_all(&library).unwrap();
    for name in ["A.wav", "B.wav", "C.wav", "D.wav"] {
        wav(&library.join(name));
    }
    let mut h = Harness::manual_in(Window::Main, &dir.join("config"));
    h.shell.pace_analysis_by_hand();
    h.shell.set_user_library(Some(library));
    assert!(h.shell.wait_for_browser_index(Duration::from_secs(60)));
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "Status").value,
        "Analyzing Sounds 0 %"
    );
    assert!(nodes.iter().any(|n| n.label == "Pause"), "a Pause button");

    h.shell.release_analysis(1);
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "Status").value,
        "Analyzing Sounds 25 %"
    );

    let b = Harness::node(&nodes, "Pause").bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    assert!(h.shell.browser_view().analysis_state().paused);
    let nodes = Harness::nodes(&h.settle());
    assert!(nodes.iter().any(|n| n.label == "Resume"), "it reads Resume");
    assert!(Harness::node(&nodes, "Status").value.contains("paused"));

    h.shell.release_analysis(3);
    h.shell
        .run(
            "browser.set_analysis_paused",
            serde_json::json!({ "paused": false }),
        )
        .unwrap();
    h.shell.release_analysis(3);
    let nodes = Harness::nodes(&h.settle());
    assert!(
        !nodes
            .iter()
            .any(|n| n.label == "Pause" || n.label == "Resume")
    );
    assert!(!Harness::node(&nodes, "Status").value.contains("Analyzing"));
    let _ = std::fs::remove_dir_all(&dir);
}
