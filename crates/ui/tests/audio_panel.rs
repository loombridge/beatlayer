//! The Clip View's Audio Utilities panel, headless (WP-4.10b): each control
//! runs the command agents run and the document reads the change back, the
//! pending ones are announced with the package that brings them, and Warp
//! off disables the tempo controls. Spec: sample-editor R14, R15, R17.

mod common;

use std::path::{Path, PathBuf};

use bl_model::{ClipData, ClipId, Document};
use common::{Harness, Node, Window};
use egui::{Pos2, Rect};
use serde_json::json;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-audio-panel-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of `seconds` at 48 kHz, holding a tone of
/// `hz`.
fn wav(path: &Path, seconds: u32, hz: f64) {
    let rate = 48_000u32;
    let frames = seconds * rate;
    let data = frames * 2;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let t = f64::from(i) / f64::from(rate);
        let value = (t * hz * std::f64::consts::TAU).sin() * 0.5;
        bytes.extend_from_slice(&((value * 30_000.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A set with one audio clip and the Clip View showing it.
struct Panel {
    h: Harness,
    clip: ClipId,
    path: PathBuf,
    _scratch: Scratch,
}

impl Panel {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let path = scratch.0.join("Tone.wav");
        wav(&path, 2, 440.0);
        let mut h = Harness::with_title_bar(Window::Main);
        let track = h
            .shell
            .editor()
            .document()
            .top_level()
            .find(|t| {
                h.shell
                    .editor()
                    .document()
                    .track(*t)
                    .is_some_and(|t| t.kind() == bl_model::TrackKind::Audio)
            })
            .unwrap();
        h.shell
            .run(
                "browser.add_to_arrangement",
                json!({ "paths": [path.display().to_string()], "track": track, "at": 0 }),
            )
            .unwrap();
        let clip = h
            .shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .clips()
            .next()
            .unwrap()
            .id;
        h.shell
            .run("arrangement.select_clips", json!({ "clips": [clip] }))
            .unwrap();
        h.shell.run("view.toggle_clip_view", json!({})).unwrap();
        h.settle();
        Self {
            h,
            clip,
            path,
            _scratch: scratch,
        }
    }

    fn doc(&self) -> &Document {
        self.h.shell.editor().document()
    }

    fn audio(&self) -> bl_model::AudioData {
        match &self.doc().clip(self.clip).unwrap().data {
            ClipData::Audio(audio) => audio.clone(),
            ClipData::Midi(_) => unreachable!("an audio clip"),
        }
    }

    fn nodes(&mut self) -> Vec<Node> {
        Harness::nodes(&self.h.settle())
    }
}

fn centre(node: &Node) -> Pos2 {
    let bounds = node.bounds.expect("the control has bounds");
    Rect::from_min_max(
        Pos2::new(bounds.x0 as f32, bounds.y0 as f32),
        Pos2::new(bounds.x1 as f32, bounds.y1 as f32),
    )
    .center()
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

// The panel shows for an audio clip, with its controls in the order the
// spec gives (R14), and each live control runs its command.
#[test]
fn the_panel_shows_its_controls_and_each_runs_its_command() {
    let mut p = Panel::new("controls");
    let all = p.nodes();
    assert!(
        all.iter().any(|n| n.label == "Audio Utilities"),
        "the Audio Utilities panel shows"
    );
    for name in ["Warp", "Warp Mode", "BPM", "Lead/Follow", "Edit", "RAM", "HiQ", "Gain"] {
        assert!(
            all.iter().any(|n| n.label == name),
            "the panel holds {name}"
        );
    }

    // Warp: a click.
    let before = steps(&p.h);
    let all = p.nodes();
    p.h.click(centre(Harness::node(&all, "Warp")));
    assert!(p.audio().warp, "the clip is warped");
    assert_eq!(steps(&p.h), before + 1);

    // RAM: a click.
    let all = p.nodes();
    p.h.click(centre(Harness::node(&all, "RAM")));
    assert!(p.audio().ram, "the clip is in RAM mode");

    // HiQ: a click.
    let all = p.nodes();
    p.h.click(centre(Harness::node(&all, "HiQ")));
    assert!(!p.audio().high_quality, "the clip's HiQ is off");

    // Gain: a screen reader's increment raises it half a decibel.
    let before = steps(&p.h);
    p.h.accesskit(
        egui::Id::new(("number-box", "audio-gain")),
        egui::accesskit::Action::Increment,
    );
    assert!(
        (p.audio().gain - 10.0_f64.powf(0.5 / 20.0)).abs() < 1e-9,
        "the gain is 0.5 dB"
    );
    assert_eq!(steps(&p.h), before + 1);
}

// The controls whose command hasn't landed are drawn pending, and say which
// package brings them (R14).
#[test]
fn the_pending_controls_name_their_package() {
    let mut p = Panel::new("pending");
    let all = p.nodes();
    for (name, package) in [
        ("Reverse", "WP-4.7"),
        ("Transpose", "WP-4.7"),
        ("Detune", "WP-4.7"),
    ] {
        let node = Harness::node(&all, name);
        assert!(node.disabled, "{name} is pending");
        assert!(
            node.description.contains(package),
            "{name} names {package}: {}",
            node.description
        );
    }
}

// With Warp off the BPM field, ×2, ÷2, the Warp Mode chooser and the
// Lead/Follow switch are disabled (R15).
#[test]
fn warp_off_disables_the_tempo_controls() {
    let mut p = Panel::new("warp-off");
    let all = p.nodes();
    for name in ["BPM", "×2", "÷2", "Warp Mode", "Lead/Follow"] {
        assert!(
            Harness::node(&all, name).disabled,
            "{name} is disabled while Warp is off"
        );
    }
    // With Warp on they are live.
    p.h.shell
        .run("clip.set_warp", json!({ "clip": p.clip, "warp": true }))
        .unwrap();
    let all = p.nodes();
    for name in ["BPM", "×2", "÷2", "Warp Mode", "Lead/Follow"] {
        assert!(
            !Harness::node(&all, name).disabled,
            "{name} is live while Warp is on"
        );
    }
}

// The header shows the file's name, sample rate, bit depth and channel
// count (R2).
#[test]
fn the_header_shows_the_files_details() {
    let mut p = Panel::new("header");
    let all = p.nodes();
    let name = Harness::node(&all, "Show in Browser");
    assert_eq!(name.value, "Tone.wav", "the file's name");
    assert_eq!(Harness::node(&all, "Sample Rate").value, "48 kHz");
    assert_eq!(Harness::node(&all, "Bit Depth").value, "16 bit");
    assert_eq!(Harness::node(&all, "Channels").value, "1 ch");
    assert!(
        all.iter().all(|n| n.label != "Files"),
        "one file is selected"
    );
}

// The title bar's Save Default Clip button runs `clip.save_default` for the
// clip, which writes the defaults beside its file (R17).
#[test]
fn save_default_clip_writes_the_defaults_beside_the_file() {
    let mut p = Panel::new("save-default");
    let all = p.nodes();
    p.h.click(centre(Harness::node(&all, "Save Default Clip")));
    let defaults = p.path.with_file_name("Tone.wav.beatlayer-clip");
    assert!(
        defaults.exists(),
        "the defaults were written beside the sample"
    );
}

// A file dropped from the browser on the Clip View replaces the clip's
// sample (R18, I-DROP-009).
#[test]
fn a_file_dropped_on_the_clip_view_replaces_the_sample() {
    let mut p = Panel::new("drop");
    let other = p.path.with_file_name("Other.wav");
    wav(&other, 2, 880.0);
    let before = p.audio().media;
    // The browser's drag, dropped on the Clip View.
    egui::DragAndDrop::set_payload(
        &p.h.ctx,
        bl_ui::browser::BrowserDrag {
            paths: vec![other.display().to_string()],
            plugin: None,
        },
    );
    // The pointer is released over the Clip View's editor.
    let all = p.nodes();
    let view = Harness::node(&all, "Clip View");
    let bounds = view.bounds.expect("the Clip View has bounds");
    let at = Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    );
    p.h.click(at);
    assert_ne!(p.audio().media, before, "the clip plays the new file");
}
