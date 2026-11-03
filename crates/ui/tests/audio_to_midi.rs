//! Convert Harmony, Melody and Drums to New MIDI Track from an audio clip's
//! menu in the Arrangement and from the Create menu, and the window it shows
//! (IMP-06; the audio-to-midi spec, R1 to R3, R25). The Session View's menu is
//! tried in `session_menus`.

use crate::common;
use bl_core::time::Ticks;
use bl_model::{ClipId, TrackId, TrackKind};
use bl_shell::audio_to_midi_commands::{ConvertDialogStep, ConvertKind};
use bl_shell::files::factory_default_set;
use bl_ui::arrangement::benchmark;
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();
const LEFT: f32 = 320.0;
const POINTS_PER_BAR: f32 = 30.0;
const AUDIO_3: f32 = 320.0;

fn x(bar: f32) -> f32 {
    LEFT + bar * POINTS_PER_BAR
}

/// Removes the audio file when the test ends, even if it fails.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Two seconds, a bar at 120 BPM, of a tone as `Clicks.wav`.
fn file(name: &str) -> (Scratch, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("bl-ui-a2m-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let samples: Vec<f32> = (0..96_000)
        .map(|i| (i as f32 * 440.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25)
        .collect();
    let bytes = bl_media::wav_bytes(
        &samples,
        1,
        48_000,
        bl_media::BitDepth::Int16,
        bl_media::Dither::None,
    )
    .unwrap();
    let path = dir.join("Clicks.wav");
    std::fs::write(&path, bytes).unwrap();
    (Scratch(dir), path)
}

/// The default set with an audio clip on its third track, bar 2, and the
/// stand-in transcription finding two notes.
fn set(name: &str) -> (Harness, ClipId, TrackId, Scratch) {
    set_with(name, true)
}

/// [`set`], with the stand-in transcription if `fixture` is set.
fn set_with(name: &str, fixture: bool) -> (Harness, ClipId, TrackId, Scratch) {
    let editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    let (scratch, path) = file(name);
    let placed = h
        .shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [path.to_string_lossy()], "track": tracks[2], "at": 2 * BAR }),
        )
        .unwrap();
    let clip: ClipId = serde_json::from_value(placed["clips"][0]["clip"].clone()).unwrap();
    if fixture {
        h.shell
            .use_transcription_notes(&[(60, 0.5, 0.9), (64, 1.0, 1.4)]);
    }
    h.settle();
    (h, clip, tracks[2], scratch)
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

/// The node labelled `label`, or starting with it for a menu item with its
/// keys.
fn node(output: &egui::FullOutput, label: &str) -> common::Node {
    let nodes = Harness::nodes(output);
    nodes
        .iter()
        .rfind(|n| {
            (n.label == label || n.label.starts_with(&format!("{label} "))) && n.bounds.is_some()
        })
        .unwrap_or_else(|| {
            let labels: Vec<_> = nodes.iter().map(|n| n.label.clone()).collect();
            panic!("no {label:?} in {labels:?}")
        })
        .clone()
}

fn click(h: &mut Harness, node: &common::Node) {
    let b = node.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
}

/// Chooses the menu item the way a screen reader does: the menu is taller
/// than the room below the clip.
fn activate(h: &mut Harness, node: &common::Node) {
    h.frame(vec![Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: node.id,
            data: None,
        },
    )]);
}

const ROWS: [(&str, ConvertKind); 3] = [
    ("Convert Harmony to New MIDI Track", ConvertKind::Harmony),
    ("Convert Melody to New MIDI Track", ConvertKind::Melody),
    ("Convert Drums to New MIDI Track", ConvertKind::Drums),
];

// Spec: audio-to-midi R1 (C-CLIP-ARR-027a to 027c). An Arrangement audio
// clip's menu lists the three Convert items after Slice to New MIDI Track,
// enabled, and each runs the conversion of its kind on the clip.
#[test]
fn an_arrangement_audio_clips_menu_converts_it_with_each_kind() {
    for (label, kind) in ROWS {
        let (mut h, clip, track, _scratch) = set(&format!("menu-{}", kind.name()));
        let output = right_click(&mut h, pos2(x(2.5), AUDIO_3));
        let slice = node(&output, "Slice to New MIDI Track");
        let item = node(&output, label);
        assert!(!item.disabled, "{label} is enabled");
        assert!(
            slice.bounds.unwrap().y0 < item.bounds.unwrap().y0,
            "{label} follows Slice"
        );
        activate(&mut h, &item);
        h.shell.wait_audio_to_midi();
        h.settle();
        let status = h.shell.audio_to_midi_status();
        assert_eq!(status.kind, Some(kind));
        assert_eq!(status.clip, Some(clip));
        let made = status.created_track.expect("the track landed");
        let doc = h.shell.editor().document();
        let order: Vec<TrackId> = doc.top_level().collect();
        let place = order.iter().position(|t| *t == track).unwrap();
        assert_eq!(order[place + 1], made, "{label}");
        assert_eq!(doc.track(made).unwrap().kind(), TrackKind::Midi);
        assert_eq!(
            doc.track(made).unwrap().name(),
            format!("Clicks {}", kind.name())
        );
    }
}

/// The Create menu's item for `row` as it stands now.
fn create_item(h: &Harness, row: &str) -> bl_ui::menus::ItemState {
    let menus = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current());
    let item = menus
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap();
    bl_ui::menus::item_state(
        item,
        &h.shell,
        &h.keymap,
        bl_ui::keymap::Platform::current(),
    )
}

// Spec: audio-to-midi R1 (M-CREATE-012 to 014). The Create menu's items are
// disabled with nothing selected, enabled with one audio clip selected, and
// run the conversion of their kind on it; with two audio clips they are
// disabled again.
#[test]
fn the_create_menus_items_act_on_the_one_selected_audio_clip() {
    let (mut h, clip, track, _scratch) = set("create");
    let rows = ["M-CREATE-012", "M-CREATE-013", "M-CREATE-014"];
    for row in rows {
        assert!(!create_item(&h, row).enabled, "{row}: nothing is selected");
    }
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    for (row, (_, kind)) in rows.into_iter().zip(ROWS) {
        let item = create_item(&h, row);
        assert!(item.enabled, "{row}");
        let invocation = item.invocation.unwrap();
        assert_eq!(invocation.id, "audio_to_midi.convert");
        assert_eq!(invocation.args["kind"], json!(kind));
        h.shell.run(&invocation.id, invocation.args).unwrap();
        h.shell.wait_audio_to_midi();
        let status = h.shell.audio_to_midi_status();
        assert_eq!(status.kind, Some(kind));
        assert_eq!(status.clip, Some(clip));
        assert!(status.created_track.is_some());
        h.shell.run("edit.undo", json!({})).unwrap();
        h.shell
            .run("arrangement.select_clips", json!({ "clips": [clip] }))
            .unwrap();
    }
    // Two audio clips: a second one on another track.
    let other = h.shell.editor().document().top_level().next().unwrap();
    let _ = (track, other);
    let (_second_scratch, path) = file("create-second");
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    let second = h
        .shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [path.to_string_lossy()], "track": tracks[2], "at": 5 * BAR }),
        )
        .unwrap();
    let second: ClipId = serde_json::from_value(second["clips"][0]["clip"].clone()).unwrap();
    h.shell
        .run(
            "arrangement.select_clips",
            json!({ "clips": [clip, second] }),
        )
        .unwrap();
    for row in rows {
        assert!(!create_item(&h, row).enabled, "{row}: two clips");
    }
}

// Spec: audio-to-midi R3, R25. While a conversion runs, a progress window
// sits over the main window: a dialog named after the command, a progress
// indicator, and Cancel. Cancel, and Esc, stop it and change nothing. The
// conversion's result is held back by an open drag, so the window can be seen
// and used.
#[test]
fn the_progress_window_has_a_bar_and_cancel_and_esc_cancels() {
    for by_escape in [false, true] {
        let (mut h, clip, _track, _scratch) = set(&format!("window-{by_escape}"));
        h.shell
            .run("arrangement.select_clips", json!({ "clips": [clip] }))
            .unwrap();
        let drag =
            |by: i64| bl_shell::Invocation::new("arrangement.move_clips", json!({ "by": by }));
        h.shell
            .gesture(bl_shell::GesturePhase::Update, Some(&drag(960)))
            .unwrap();
        let before = h.shell.editor().document().clone();
        h.shell
            .run(
                "audio_to_midi.convert",
                json!({ "clip": clip, "kind": "melody" }),
            )
            .unwrap();
        let dialog = h.shell.audio_to_midi_dialog().expect("the window is open");
        assert_eq!(dialog.step, ConvertDialogStep::Converting);
        let output = h.settle();
        let nodes = Harness::nodes(&output);
        let name = "Convert Melody to New MIDI Track";
        assert!(
            nodes
                .iter()
                .any(|n| n.role == Role::Dialog && n.label == name),
            "a dialog named {name}"
        );
        assert!(
            nodes
                .iter()
                .any(|n| n.role == Role::ProgressIndicator && n.label == name),
            "a progress indicator"
        );
        let cancel = nodes
            .iter()
            .find(|n| n.role == Role::Button && n.label == "Cancel")
            .expect("Cancel")
            .clone();
        if by_escape {
            h.key(egui::Key::Escape, Modifiers::NONE);
        } else {
            click(&mut h, &cancel);
        }
        assert!(h.shell.audio_to_midi_dialog().is_none(), "{by_escape}");
        h.shell
            .gesture(bl_shell::GesturePhase::Commit, Some(&drag(960)))
            .unwrap();
        h.shell.wait_audio_to_midi();
        h.settle();
        let status = h.shell.audio_to_midi_status();
        assert!(status.created_track.is_none());
        // Only the drag changed the set.
        assert_eq!(
            h.shell.editor().document().top_level().count(),
            before.top_level().count()
        );
        assert_eq!(
            serde_json::to_value(&status.conversion).unwrap()["state"],
            "cancelled"
        );
    }
}

// Spec: audio-to-midi R2. The first time Harmony runs without its packages
// the page opens instead of the conversion, with Download Required Packages
// Now and Cancel; Drums needs none.
#[test]
fn harmony_without_its_packages_opens_the_package_page() {
    let (mut h, clip, _track, _scratch) = set_with("packages", false);
    if h.shell.audio_to_midi_status().packages.is_empty()
        || h.shell.audio_to_midi_packages_installed()
    {
        return;
    }
    h.shell
        .run(
            "audio_to_midi.convert",
            json!({ "clip": clip, "kind": "harmony" }),
        )
        .unwrap();
    let dialog = h.shell.audio_to_midi_dialog().expect("the page is open");
    assert_eq!(dialog.step, ConvertDialogStep::Packages);
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let shown: Vec<&str> = nodes.iter().map(|n| n.label.as_str()).collect();
    for text in [
        "Download Required Packages Now",
        "Cancel",
        "ONNX Runtime",
        "basic-pitch",
    ] {
        assert!(
            shown.iter().any(|l| l.contains(text)),
            "{text} in {shown:?}"
        );
    }
    click(&mut h, &node(&output, "Cancel"));
    assert!(h.shell.audio_to_midi_dialog().is_none());
    assert!(h.shell.audio_to_midi_status().created_track.is_none());
}
