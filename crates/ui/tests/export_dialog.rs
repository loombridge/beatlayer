//! The Export Audio/Video dialog and its progress window (WP-6.71; the
//! export spec, R28 to R45): ⇧⌘R opens it, every control is there in the
//! manual's order with a new install's defaults, each disabled state holds
//! with its reason, Return exports and Esc closes it, Export asks for the
//! file, and the progress window's bar moves and Cancel leaves nothing
//! behind.

use crate::common;
use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_shell::export_commands::{self as ids, ExportDialogStatus};
use bl_shell::{Effect, browser_commands};
use common::{Harness, Node, Window, command};
use egui::accesskit::Role;
use egui::{Key, Modifiers, pos2};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-ui-export-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of one second at 48 kHz.
fn wav(path: &Path) {
    let frames = 48_000u32;
    let mut bytes = Vec::new();
    let data = frames * 2;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let sample = ((i % 100) as i16 - 50) * 20;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A harness whose set holds a one-second clip at bar 5.
fn harness(scratch: &Scratch) -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    let file = scratch.path("Tone.wav");
    wav(&file);
    h.shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 4 * BAR }),
        )
        .unwrap();
    h.settle();
    h
}

fn dialog(h: &mut Harness) -> ExportDialogStatus {
    let status: ids::ExportStatus =
        serde_json::from_value(h.shell.run(ids::STATUS, json!({})).unwrap()).unwrap();
    status.dialog.expect("the dialog is open")
}

fn set(h: &mut Harness, patch: Value) {
    h.shell.run(ids::SET_CHOICES, patch).unwrap();
}

fn labelled<'a>(nodes: &'a [Node], label: &str) -> &'a Node {
    nodes
        .iter()
        .find(|n| n.label == label && n.bounds.is_some())
        .unwrap_or_else(|| {
            let labels: Vec<_> = nodes.iter().map(|n| n.label.clone()).collect();
            panic!("no {label:?} in {labels:?}")
        })
}

fn click_node(h: &mut Harness, label: &str) {
    let nodes = Harness::nodes(&h.settle());
    let b = labelled(&nodes, label).bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    // Frames go on after a click; the focus is settled when keys come.
    h.settle();
}

/// Spec: export R16, R28. ⇧⌘R opens the dialog, and Esc closes it without
/// exporting.
#[test]
fn the_key_opens_the_dialog_and_esc_closes_it() {
    let scratch = Scratch::new("open");
    let mut h = harness(&scratch);
    assert!(h.shell.export_dialog().is_none());
    h.key(Key::R, command() | Modifiers::SHIFT);
    let opened = h.shell.export_dialog().expect("⇧⌘R opens the dialog");
    assert!(opened.can_export);
    let nodes = Harness::nodes(&h.settle());
    // A label is read from its text.
    assert!(nodes.iter().any(|n| n.value == "Export Audio/Video"));

    // Esc is Cancel: it closes the dialog without exporting.
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    assert!(h.shell.export_dialog().is_none());
    assert!(!h.shell.export_running());
    assert_eq!(scratch.names(), ["Tone.wav"]);
}

/// Spec: export R29, R30, R31, R32, R35, R36, R45. Every control is in the
/// manual's order, with a new install's defaults, and Tab reaches them in
/// that order.
#[test]
fn every_control_is_there_in_the_manuals_order_with_its_default() {
    let scratch = Scratch::new("controls");
    let mut h = harness(&scratch);
    h.shell.run(ids::OPEN_DIALOG, json!({})).unwrap();
    let nodes = Harness::nodes(&h.settle());

    // R45: each section is a group with its name.
    for section in ["Selection", "Rendering Options", "Encoding", "Video"] {
        let group = nodes
            .iter()
            .find(|n| n.label == section && n.role == Role::Group)
            .unwrap_or_else(|| panic!("no group {section:?}"));
        assert!(!group.children.is_empty(), "{section} holds its controls");
    }

    // R29: the controls, top to bottom.
    let order = [
        "Rendered Track",
        "Render Start bars",
        "Render Start beats",
        "Render Start sixteenths",
        "Render Length bars",
        "Render Length beats",
        "Render Length sixteenths",
        "Include Return and Main Effects",
        "Render as Loop",
        "Convert to Mono",
        "Normalize",
        "Create Analysis File",
        "Sample Rate",
        "Encode PCM",
        "File Type",
        "Bit Depth",
        "Dither Options",
        "Encode MP3",
        "Create Video",
        "Video Encoder",
        "Edit",
        "Cancel",
        "Export",
    ];
    let mut tops = Vec::new();
    for label in order {
        let node = labelled(&nodes, label);
        tops.push((node.bounds.unwrap().y0, label));
    }
    let sorted = {
        let mut sorted = tops.clone();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        sorted
    };
    // Start and Length share a row of three parts each: compare by row.
    let rows: Vec<&str> = tops.iter().map(|(_, label)| *label).collect();
    let by_y: Vec<&str> = sorted.iter().map(|(_, label)| *label).collect();
    assert_eq!(rows.len(), by_y.len());
    for (a, b) in rows.iter().zip(&by_y) {
        let (ya, yb) = (
            tops.iter().find(|t| t.1 == *a).unwrap().0,
            tops.iter().find(|t| t.1 == *b).unwrap().0,
        );
        assert!((ya - yb).abs() < 1.0, "{a} comes after {b}");
    }

    // New-install values: switches off but Encode PCM, Main, WAV, 24 bits,
    // Triangular, the engine's 48000 Hz.
    for off in [
        "Render as Loop",
        "Convert to Mono",
        "Normalize",
        "Create Analysis File",
        "Encode MP3",
    ] {
        assert_eq!(labelled(&nodes, off).toggled, Some(false), "{off}");
    }
    assert_eq!(labelled(&nodes, "Encode PCM").toggled, Some(true));
    assert_eq!(labelled(&nodes, "Rendered Track").value, "Main");
    assert_eq!(labelled(&nodes, "Sample Rate").value, "48000 Hz");
    assert_eq!(labelled(&nodes, "File Type").value, "WAV");
    assert_eq!(labelled(&nodes, "Bit Depth").value, "24");
    assert_eq!(labelled(&nodes, "Dither Options").value, "Triangular");
    // R31: Start 1.1.1 and the whole set, 4 bars and 2 beats long.
    for (label, value) in [
        ("Render Start bars", "1"),
        ("Render Start beats", "1"),
        ("Render Start sixteenths", "1"),
        ("Render Length bars", "4"),
        ("Render Length beats", "2"),
        ("Render Length sixteenths", "0"),
    ] {
        assert_eq!(labelled(&nodes, label).value, value, "{label}");
    }

    // R45: Tab reaches the controls that can be used, in that order.
    let mut reached = Vec::new();
    for _ in 0..40 {
        h.key(Key::Tab, Modifiers::NONE);
        let output = h.settle();
        let nodes = Harness::nodes(&output);
        let Some(id) = h.ctx.memory(egui::Memory::focused) else {
            continue;
        };
        if let Some(node) = nodes.iter().find(|n| n.id == id.accesskit_id()) {
            if reached.first() == Some(&node.label) {
                break;
            }
            reached.push(node.label.clone());
        }
    }
    let usable: Vec<&str> = order
        .iter()
        .copied()
        // Disabled: Include Return and Main Effects waits for a track, and
        // the Video section is drawn disabled.
        .filter(|label| {
            !["Include Return and Main Effects", "Create Video", "Video Encoder", "Edit"]
                .contains(label)
        })
        .collect();
    let mut seen = reached.iter().map(String::as_str).peekable();
    for label in usable {
        let found = seen.by_ref().any(|reached| reached == label);
        assert!(found, "Tab reaches {label} in order: {reached:?}");
    }
}

/// Spec: export R30, R32, R36, R37, R38, R45. Each disabled state holds,
/// and the reason is on the control for assistive technology.
#[test]
fn each_disabled_state_holds_with_its_reason() {
    let scratch = Scratch::new("disabled");
    let mut h = harness(&scratch);
    h.shell.run(ids::OPEN_DIALOG, json!({})).unwrap();
    let nodes = Harness::nodes(&h.settle());
    // R32: Include Return and Main Effects waits for a track.
    let include = labelled(&nodes, "Include Return and Main Effects");
    assert!(include.disabled);
    assert!(
        include.description.contains("Main already holds"),
        "{}",
        include.description
    );
    // R38: the Video section is drawn disabled, as for a set without video.
    for label in ["Create Video", "Video Encoder", "Edit"] {
        let node = labelled(&nodes, label);
        assert!(node.disabled, "{label}");
    }
    assert!(
        labelled(&nodes, "Create Video")
            .description
            .contains("no video clips")
    );
    set(&mut h, json!({ "rendered_track": "all_tracks" }));
    let nodes = Harness::nodes(&h.settle());
    assert!(!labelled(&nodes, "Include Return and Main Effects").disabled);

    // R30: Selected Tracks Only with nothing selected disables Export.
    set(&mut h, json!({ "rendered_track": "selected_tracks" }));
    let nodes = Harness::nodes(&h.settle());
    let export = labelled(&nodes, "Export");
    assert!(export.disabled);
    assert!(
        export.description.contains("no track is selected"),
        "{}",
        export.description
    );
    set(&mut h, json!({ "rendered_track": "main" }));

    // R36: Encode PCM off disables File Type, Bit Depth and Dither Options;
    // R37: with Encode MP3 off too, Export is disabled.
    set(&mut h, json!({ "encode_pcm": false }));
    let nodes = Harness::nodes(&h.settle());
    for label in ["File Type", "Bit Depth", "Dither Options"] {
        let node = labelled(&nodes, label);
        assert!(node.disabled, "{label}");
        assert_eq!(node.description, "Encode PCM is off.");
    }
    let export = labelled(&nodes, "Export");
    assert!(export.disabled);
    assert!(export.description.contains("Encode PCM or Encode MP3"));
    // And an MP3 at a rate it can't hold.
    set(&mut h, json!({ "encode_mp3": true, "sample_rate": 96_000 }));
    let nodes = Harness::nodes(&h.settle());
    assert!(labelled(&nodes, "Export").disabled);
    assert!(
        labelled(&nodes, "Export")
            .description
            .contains("can't hold")
    );
    set(
        &mut h,
        json!({ "sample_rate": 44_100, "encode_pcm": true, "bit_depth": 32 }),
    );
    // R36: no dither at 32 bits; FLAC with 32 chosen sets 24.
    let nodes = Harness::nodes(&h.settle());
    assert!(labelled(&nodes, "Dither Options").disabled);
    set(&mut h, json!({ "file_type": "flac" }));
    assert_eq!(dialog(&mut h).choices.bit_depth, 24);
    // R31: a length of zero disables Export.
    set(&mut h, json!({ "length": 0 }));
    let nodes = Harness::nodes(&h.settle());
    assert!(
        labelled(&nodes, "Export")
            .description
            .contains("Render Length is zero")
    );
}

/// Spec: export R28, R40. Return presses Export when it is enabled: the
/// dialog closes behind the save dialog the shell asks for, which suggests
/// the set's name; when it is disabled, Return does nothing.
#[test]
fn return_exports_and_asks_for_the_file() {
    let scratch = Scratch::new("return");
    let mut h = harness(&scratch);
    h.shell.run(ids::OPEN_DIALOG, json!({})).unwrap();
    set(&mut h, json!({ "length": 0 }));
    h.settle();
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert!(h.effects.is_empty(), "Export is disabled: {:?}", h.effects);
    assert!(h.shell.export_dialog().is_some());

    set(&mut h, json!({ "length": BAR }));
    h.settle();
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    let [Effect::PickExportPath { file_name, .. }] = h.effects.as_slice() else {
        panic!("{:?}", h.effects);
    };
    assert_eq!(*file_name, format!("{}.wav", h.shell.set_name()));
    assert!(h.shell.export_dialog().is_none(), "behind the save dialog");

    // Choosing a file runs the export, and the progress window shows.
    let path = scratch.path("Out.wav");
    h.shell
        .run(ids::START, json!({ "path": path.to_string_lossy() }))
        .unwrap();
    while h.shell.export_running() {
        h.shell.poll();
        h.settle();
    }
    assert!(path.is_file());
    let message = h.shell.message().unwrap();
    assert!(message.text.contains("Out.wav"), "{}", message.text);
    assert_eq!(scratch.names(), ["Out.wav", "Tone.wav"]);
}

/// Spec: export R31. Render Start and Render Length are stepped and typed
/// as the Control Bar's Loop Start and Loop Length fields are.
#[test]
fn the_start_and_length_fields_step_and_take_typed_values() {
    let scratch = Scratch::new("fields");
    let mut h = harness(&scratch);
    h.shell.run(ids::OPEN_DIALOG, json!({})).unwrap();
    // Up steps the bars part of Render Start by a bar, and Down back.
    click_node(&mut h, "Render Start bars");
    h.key(Key::ArrowUp, Modifiers::NONE);
    h.settle();
    assert_eq!(dialog(&mut h).start, Ticks(BAR));
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(dialog(&mut h).start, Ticks::ZERO);
    // Typed: 3, Period on to the beats part, 2, Return sets 3.2.1.
    h.key(Key::Num3, Modifiers::NONE);
    h.key(Key::Period, Modifiers::NONE);
    h.key(Key::Num2, Modifiers::NONE);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(dialog(&mut h).start, Ticks(2 * BAR + BAR / 4));
    // Esc drops a typed value and leaves the dialog open.
    click_node(&mut h, "Render Length bars");
    h.key(Key::Num9, Modifiers::NONE);
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    assert_eq!(dialog(&mut h).length, Ticks(4 * BAR + BAR / 2));
    assert!(h.shell.export_dialog().is_some());
    // Delete gives a field its opening value back.
    click_node(&mut h, "Render Start bars");
    h.key(Key::Delete, Modifiers::NONE);
    h.settle();
    assert_eq!(dialog(&mut h).start, Ticks::ZERO);
    // A screen reader's increment steps a length by a bar.
    let nodes = Harness::nodes(&h.settle());
    let id = labelled(&nodes, "Render Length bars").id;
    h.frame(vec![egui::Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Increment,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: id,
            data: None,
        },
    )]);
    h.settle();
    assert_eq!(dialog(&mut h).length, Ticks(5 * BAR + BAR / 2));
}

/// Spec: export R41, R45. While an export runs a modal window shows the
/// file being written and a bar that moves; Esc is Cancel, which stops the
/// export and leaves nothing behind, without reopening the dialog.
#[test]
fn the_progress_windows_bar_moves_and_cancel_leaves_nothing_behind() {
    let scratch = Scratch::new("progress");
    let mut h = harness(&scratch);
    // A day of silence: more than any test waits for.
    let day = 172_800 * Ticks::QUARTER.0;
    h.shell
        .start_export(json!({ "path": scratch.path("Long.wav").to_string_lossy(),
                              "bit_depth": 16, "sample_rate": 8000, "start": 0,
                              "length": day }))
        .unwrap();
    let bar = loop {
        h.shell.poll();
        let nodes = Harness::nodes(&h.settle());
        if let Some(bar) = nodes
            .iter()
            .find(|n| n.role == Role::ProgressIndicator && n.numeric.is_some_and(|v| v > 0.0))
        {
            assert_eq!(bar.label, "Export progress");
            break bar.clone();
        }
    };
    assert!(bar.numeric.unwrap() < 100.0);
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes.iter().any(|n| n.value == "Long.wav"),
        "the file's name"
    );
    assert!(nodes.iter().any(|n| n.label == "Cancel"));
    // Esc is Cancel.
    h.key(Key::Escape, Modifiers::NONE);
    while h.shell.export_running() {
        h.shell.poll();
        h.settle();
    }
    h.settle();
    assert_eq!(scratch.names(), ["Tone.wav"]);
    assert!(
        h.shell.export_dialog().is_none(),
        "the dialog doesn't reopen"
    );
    let nodes = Harness::nodes(&h.settle());
    assert!(!nodes.iter().any(|n| n.label == "Export progress"));
}
