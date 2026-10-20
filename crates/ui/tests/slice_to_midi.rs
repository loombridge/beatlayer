//! Slice to New MIDI Track from an audio clip's menu, and its dialog
//! (WP-4.16; the slice-to-midi spec, R1 and R2).

mod common;

use bl_core::time::Ticks;
use bl_model::{AudioData, Clip, ClipData, ClipId, Command, TrackId};
use bl_shell::files::factory_default_set;
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

/// Clicks the node labelled `label` (or starting with it), and says whether
/// it was enabled.
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

fn labels(output: &egui::FullOutput) -> Vec<String> {
    Harness::nodes(output)
        .iter()
        .map(|n| n.label.clone())
        .collect()
}

// Spec: slice-to-midi R1. The command is enabled with one audio clip
// selected and disabled otherwise.
#[test]
fn slice_to_new_midi_track_is_enabled_with_one_audio_clip() {
    let (mut h, _clip) = set();
    assert!(
        !h.shell
            .can_invoke("clip.open_slice_dialog", &serde_json::json!({})),
        "disabled with nothing selected"
    );
    let output = right_click(&mut h, pos2(x(3.0), AUDIO_3));
    let enabled = choose(&mut h, &output, "Slice to New MIDI Track");
    assert!(enabled, "enabled with one audio clip selected");
    assert!(h.shell.slice_dialog().is_some(), "it opened the dialog");
}

// Spec: slice-to-midi R2. The dialog holds the divisions, Transients and
// Warp Markers, the presets, Preserve warped timing, and Cancel and OK.
#[test]
fn the_slicing_dialog_offers_divisions_transients_and_warp_markers() {
    let (mut h, clip) = set();
    h.shell
        .run(
            "clip.open_slice_dialog",
            serde_json::json!({ "clip": clip }),
        )
        .unwrap();
    let output = h.settle();
    let shown = labels(&output);
    for label in [
        "8 Bars",
        "4 Bars",
        "2 Bars",
        "1 Bar",
        "1/2",
        "1/4",
        "1/8",
        "1/16",
        "1/32",
        "Transients",
        "Warp Markers",
        "Drum Rack",
        "One Sampler",
        "Preserve warped timing",
        "Cancel",
        "OK",
    ] {
        assert!(shown.iter().any(|l| l == label), "{label} in {shown:?}");
    }
    // Preserve warped timing is on in a new dialog (R2).
    let nodes = Harness::nodes(&output);
    let preserve = nodes
        .iter()
        .find(|n| n.label == "Preserve warped timing")
        .expect("the switch is there");
    assert_eq!(preserve.toggled, Some(true), "Preserve is on");
    // The presets are the factory ones, the default first (R11).
    let presets = h.shell.slice_dialog().unwrap().presets;
    assert_eq!(presets.first().unwrap(), "Drum Rack");
    assert!(presets.contains(&"One Sampler".to_owned()), "{presets:?}");
}
