//! Separate Stems to New Audio Tracks from an audio clip's menu, and its
//! dialog (WP-9.1; the stem separation spec, R1 to R7).

mod common;

use bl_core::time::Ticks;
use bl_model::{AudioData, Clip, ClipData, ClipId, Command, TrackId};
use bl_shell::files::factory_default_set;
use bl_shell::stems_commands::{DialogStep, JobState};
use bl_ui::arrangement::benchmark;
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, pos2};

const BAR: i64 = Ticks::WHOLE.get();
const LEFT: f32 = 320.0;
const POINTS_PER_BAR: f32 = 30.0;
const AUDIO_3: f32 = 320.0;

fn x(bar: f32) -> f32 {
    LEFT + bar * POINTS_PER_BAR
}

/// The default set with an audio clip on its third track, bars 2 to 6.
fn set() -> (Harness, ClipId) {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let id: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: tracks[2],
            clip: Clip::new(
                id,
                Ticks(2 * BAR),
                Ticks(4 * BAR),
                ClipData::Audio(AudioData::new(benchmark::media_hash(0))),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.settle();
    (h, id)
}

fn right_click(h: &mut Harness, at: Pos2) -> egui::FullOutput {
    h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    h.settle()
}

/// Clicks the node labelled `label` (or starting with it, for a menu item
/// with its keys), and says whether it was enabled.
fn choose(h: &mut Harness, output: &egui::FullOutput, label: &str) -> bool {
    let nodes = Harness::nodes(output);
    let node = nodes
        .iter()
        .rfind(|n| {
            (n.label == label || n.label.starts_with(&format!("{label} "))) && n.bounds.is_some()
        })
        .unwrap_or_else(|| {
            let labels: Vec<_> = nodes.iter().map(|n| n.label.clone()).collect();
            panic!("no {label:?} in {labels:?}")
        });
    let enabled = !node.disabled;
    let b = node.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    enabled
}

/// Removes the stand-in packages when the test ends, even if it fails.
struct Remove(Vec<std::path::PathBuf>);

impl Drop for Remove {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn labels(output: &egui::FullOutput) -> Vec<String> {
    Harness::nodes(output)
        .iter()
        .map(|n| n.label.clone())
        .collect()
}

// Spec: stem-separation R1, R2, R3.
#[test]
fn an_audio_clips_menu_opens_the_packages_dialog_the_first_time() {
    let (mut h, clip) = set();
    if bl_shell::stems::packages().is_empty() || h.shell.stem_packages_installed() {
        return;
    }
    let output = right_click(&mut h, pos2(x(3.0), AUDIO_3));
    let shown = labels(&output);
    for item in [
        "Convert Harmony to New MIDI Track",
        "Convert Drums to New MIDI Track",
        "Separate Stems to New Audio Tracks",
    ] {
        assert!(
            shown.iter().any(|l| l.starts_with(item)),
            "{item} in {shown:?}"
        );
    }
    // The menu is taller than the room below the clip, so the item is
    // chosen as a screen reader chooses it.
    let item = Harness::nodes(&output)
        .into_iter()
        .rfind(|n| n.label.starts_with("Separate Stems to New Audio Tracks"))
        .unwrap();
    assert!(!item.disabled);
    h.frame(vec![Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: item.id,
            data: None,
        },
    )]);
    let dialog = h.shell.stems_dialog().expect("the dialog is open");
    assert_eq!((dialog.step, dialog.clip), (DialogStep::Packages, clip));
    let output = h.settle();
    let shown = labels(&output);
    for text in [
        "Download Required Packages Now",
        "Cancel",
        "ONNX Runtime 1.28.3",
        "Demucs (HTDemucs) 4.1.0",
    ] {
        assert!(
            shown.iter().any(|l| l.contains(text)),
            "{text} in {shown:?}"
        );
    }
    assert!(choose(&mut h, &output, "Cancel"));
    assert!(h.shell.stems_dialog().is_none());

    // Spec: stem-separation R5, R6. Once installed, it offers the parts
    // and separates. (One test, as the packages folder is shared.)
    let folder = h.shell.packages_folder();
    // Stand-ins: the dialog only looks for the files.
    let made: Vec<_> = bl_shell::stems::packages()
        .iter()
        .map(|p| p.path(&folder))
        .collect();
    let _cleanup = Remove(made.clone());
    for path in &made {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"stand-in").unwrap();
    }
    h.shell
        .run("stems.open", serde_json::json!({ "clip": clip }))
        .unwrap();
    let output = h.settle();
    let shown = labels(&output);
    for text in [
        "Vocals",
        "Drums",
        "Bass",
        "Others",
        "Merge to Single Track",
        "Quality: High Speed or High Quality",
        "Separate",
    ] {
        assert!(
            shown.iter().any(|l| l.contains(text)),
            "{text} in {shown:?}"
        );
    }
    assert!(choose(&mut h, &output, "Separate"));
    h.settle();
    let status = h.shell.stems_status();
    // The stand-ins can't separate anything, but the separation started.
    assert_ne!(status.separation.state, JobState::Idle, "{status:?}");
}
