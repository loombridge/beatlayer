//! The Clip View's Audio Utilities panel, headless (WP-4.10b): each control
//! runs the command agents run and the document reads the change back, the
//! pending ones are announced with the package that brings them, and Warp
//! off disables the tempo controls. Spec: sample-editor R14, R15, R17.

use crate::common;
use std::path::{Path, PathBuf};

use bl_model::{ClipData, ClipId, Document};
use common::{Harness, Node, Window};
use egui::{Event, PointerButton, Pos2, Rect};
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

    /// A set with one audio clip in a Session slot and the Clip View
    /// showing it.
    fn new_session(name: &str) -> Self {
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
        let scene = h.shell.editor().document().scenes().next().unwrap().id;
        h.shell
            .run(
                "session.drop_files",
                json!({ "paths": [path.display().to_string()], "track": track, "scene": scene }),
            )
            .unwrap();
        h.shell
            .run("session.select", json!({ "track": track, "scene": scene }))
            .unwrap();
        h.shell.run("view.toggle_clip_view", json!({})).unwrap();
        h.settle();
        let clip = h.shell.editor().document().slot(track, scene).unwrap().id;
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
    for name in [
        "Warp",
        "Warp Mode",
        "BPM",
        "Lead/Follow",
        "Reverse",
        "Edit",
        "RAM",
        "HiQ",
        "Gain",
        "Transpose",
        "Detune",
    ] {
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

// In Re-Pitch the Transpose and the Detune don't act, and the panel draws
// them switched off (warp-modes R10).
// Spec: midi-tools R1. An audio clip's right group holds Audio Utilities and
// a Transform panel holding Quantize, and no Generate panel.
#[test]
fn the_right_group_holds_audio_utilities_and_a_transform_panel_of_quantize() {
    // Spec: midi-transformations R22. The Transform panel holds Quantize's
    // Grid, Triplets and Amount, and its button; there is no Start, End,
    // Auto or Reset.
    let mut p = Panel::new("transform");
    let all = p.nodes();
    let radio = |all: &[Node], name: &str| {
        all.iter()
            .find(|n| n.label == name && n.toggled.is_some())
            .unwrap_or_else(|| panic!("no button named {name:?}"))
            .clone()
    };
    assert_eq!(radio(&all, "Audio Utilities").toggled, Some(true));
    assert_eq!(
        radio(&all, "MIDI Transformation Tools").toggled,
        Some(false)
    );
    assert!(all.iter().all(|n| n.label != "MIDI Generative Tools"));
    Harness::node(&all, "Warp");
    p.h.click(centre(&radio(&all, "MIDI Transformation Tools")));
    let all = p.nodes();
    assert_eq!(radio(&all, "MIDI Transformation Tools").toggled, Some(true));
    assert!(
        all.iter().all(|n| n.label != "Warp"),
        "the utilities are gone"
    );
    for control in ["Grid", "Triplets", "Amount", "Transform"] {
        Harness::node(&all, control);
    }
    for gone in ["Quantize Auto", "Reset", "Transformation Selector"] {
        assert!(all.iter().all(|n| n.label != gone), "{gone}");
    }
    assert!(
        Harness::node(&all, "Triplets").disabled,
        "no triplets with Current Grid"
    );
    // The controls are `midi_tool.set` on the audio clip: Quantize's settings.
    p.h.shell
        .run(
            "midi_tool.set",
            json!({ "clip": p.clip, "tool": "quantize", "values": { "grid": "1_8", "amount": 40 } }),
        )
        .unwrap();
    let all = p.nodes();
    // (The Clip View has an editing grid named Grid too.)
    assert!(all.iter().any(|n| n.label == "Grid" && n.value == "1/8"));
    assert_eq!(Harness::node(&all, "Amount").value, "40 %");
    assert!(!Harness::node(&all, "Triplets").disabled);
    // Back to the utilities.
    p.h.click(centre(&radio(&all, "Audio Utilities")));
    let all = p.nodes();
    Harness::node(&all, "Warp");
}

#[test]
fn re_pitch_switches_the_pitch_controls_off() {
    let mut p = Panel::new("re-pitch");
    p.h.shell
        .run("clip.set_warp", json!({ "clip": p.clip, "warp": true }))
        .unwrap();
    p.h.shell
        .run(
            "clip.set_warp_mode",
            json!({ "clip": p.clip, "mode": "re_pitch" }),
        )
        .unwrap();
    p.h.settle();
    let all = p.nodes();
    for name in ["Transpose", "Detune"] {
        assert!(
            Harness::node(&all, name).disabled,
            "{name} is switched off in Re-Pitch"
        );
    }
}

// The control whose command hasn't landed is drawn pending, and says which
// package brings it (R14): the Clip Fade, on a Session clip.
#[test]
fn the_pending_controls_name_their_package() {
    let mut p = Panel::new_session("pending");
    let all = p.nodes();
    let node = Harness::node(&all, "Clip Fade");
    assert!(node.disabled, "the Clip Fade is pending");
    assert!(
        node.description.contains("Phase 3"),
        "the Clip Fade names its package: {}",
        node.description
    );
}

// The pitch and Reverse controls (WP-4.7) run their commands, and the
// document reads the change back.
#[test]
fn the_pitch_and_reverse_controls_run_their_commands() {
    let mut p = Panel::new("pitch");
    let all = p.nodes();
    for name in ["Reverse", "Transpose", "Detune"] {
        assert!(
            !Harness::node(&all, name).disabled,
            "{name} is live, not pending"
        );
    }
    // Transpose: a screen reader's increment raises it a semitone.
    let before = steps(&p.h);
    p.h.accesskit(
        egui::Id::new(("number-box", "audio-transpose")),
        egui::accesskit::Action::Increment,
    );
    assert_eq!(p.audio().transpose, 1);
    assert_eq!(steps(&p.h), before + 1);
    // Detune: a cent.
    p.h.accesskit(
        egui::Id::new(("number-box", "audio-detune")),
        egui::accesskit::Action::Increment,
    );
    assert_eq!(p.audio().detune, 1.0);
    // Reverse: a click, and the clip plays a reversed copy of its file.
    let media = p.audio().media;
    let all = p.nodes();
    p.h.click(centre(Harness::node(&all, "Reverse")));
    assert_ne!(p.audio().media, media, "the clip plays a reversed copy");
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

// Spec: warping R46, R55. With EXT on every clip follows the incoming clock:
// the Lead/Follow switch is disabled and keeps showing what the clip is, and
// it is live again with EXT off.
#[test]
fn ext_disables_the_lead_follow_switch_and_it_keeps_showing_lead() {
    let mut p = Panel::new("ext");
    p.h.shell
        .run("clip.set_warp", json!({ "clip": p.clip, "warp": true }))
        .unwrap();
    p.h.shell
        .run(
            "clip.set_tempo_leader",
            json!({ "clip": p.clip, "leader": true }),
        )
        .unwrap();
    let test = bl_midi_io::TestBackend::new();
    test.add_port(bl_midi_io::Direction::Input, "Clock in");
    p.h.shell.use_midi_backend(Box::new(test));
    p.h.shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Clock in", "sync": true }),
        )
        .unwrap();
    let all = p.nodes();
    let switch = Harness::node(&all, "Lead/Follow");
    assert!(!switch.disabled);
    assert_eq!(switch.value, "On");

    p.h.shell
        .run("transport.set_external_sync", json!({ "on": true }))
        .unwrap();
    let all = p.nodes();
    let switch = Harness::node(&all, "Lead/Follow");
    assert!(switch.disabled, "EXT on");
    assert_eq!(switch.value, "On", "it still shows Lead");
    assert!(switch.description.contains("External Sync"));
    assert!(p.audio().tempo_leader, "nothing in the set changed");

    p.h.shell
        .run("transport.set_external_sync", json!({ "on": false }))
        .unwrap();
    let all = p.nodes();
    assert!(!Harness::node(&all, "Lead/Follow").disabled, "EXT off");
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

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    }
}

// Spec: audio-clips R15. Dragging a number box is one undo step, not one
// for each step of the drag.
#[test]
fn dragging_gain_is_one_undo_step() {
    let mut p = Panel::new("gain-drag");
    let all = p.nodes();
    let at = centre(Harness::node(&all, "Gain"));
    let before = steps(&p.h);
    p.h.frame(vec![Event::PointerMoved(at)]);
    p.h.frame(vec![press(at, true)]);
    for i in 1..=20 {
        p.h.frame(vec![Event::PointerMoved(
            at - egui::vec2(0.0, 3.0 * i as f32),
        )]);
    }
    p.h.frame(vec![press(at - egui::vec2(0.0, 60.0), false)]);
    p.h.settle();
    assert!(p.audio().gain > 1.0, "the drag raised the gain");
    assert_eq!(steps(&p.h), before + 1, "one undo step for the drag");
}

// Spec: audio-clips R15. A click on a control with several clips selected
// is one undo step, and one Undo puts every clip back.
#[test]
fn warp_on_two_selected_clips_is_one_undo_step() {
    let mut p = Panel::new("warp-two");
    let track = p.doc().clip_track(p.clip).unwrap();
    let path = p.path.display().to_string();
    p.h.shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [path], "track": track, "at": bl_core::time::Ticks::WHOLE.get() * 3 }),
        )
        .unwrap();
    let clips: Vec<ClipId> = p
        .doc()
        .track(track)
        .unwrap()
        .clips()
        .map(|c| c.id)
        .collect();
    assert_eq!(clips.len(), 2);
    p.h.shell
        .run("arrangement.select_clips", json!({ "clips": clips }))
        .unwrap();
    let warp = |p: &Panel| -> Vec<bool> {
        clips
            .iter()
            .map(|c| match &p.doc().clip(*c).unwrap().data {
                ClipData::Audio(a) => a.warp,
                ClipData::Midi(_) => unreachable!("an audio clip"),
            })
            .collect()
    };
    let start = warp(&p);
    let before = steps(&p.h);
    let all = p.nodes();
    p.h.click(centre(Harness::node(&all, "Warp")));
    assert!(warp(&p).iter().zip(&start).all(|(now, was)| now != was));
    assert_eq!(steps(&p.h), before + 1);
    p.h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(warp(&p), start);
}
