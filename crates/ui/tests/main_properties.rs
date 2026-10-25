//! The Clip View's Main Properties panel, headless (WP-6.55): it shows for
//! MIDI and audio clips in the Arrangement and in Session slots with its
//! controls in L-CLIP-004's order, each field reads as the spec says and
//! typing into it runs the command agents run, Loop is disabled on unwarped
//! audio, and the keyboard reaches the panel. Spec: clip-properties R1 to
//! R12.

use crate::common;
use std::path::PathBuf;

use bl_core::time::{Ticks, TimeSignature};
use bl_model::{Clip, ClipData, ClipId, MidiData};
use bl_shell::clip_properties_commands::Properties;
use common::{Harness, Node, Window};
use egui::{Key, Modifiers, Pos2, Rect};
use serde_json::json;

fn nodes(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
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

fn midi(h: &Harness, clip: ClipId) -> MidiData {
    match &h.shell.editor().document().clip(clip).unwrap().data {
        ClipData::Midi(midi) => midi.clone(),
        ClipData::Audio(_) => unreachable!("a MIDI clip"),
    }
}

fn audio(h: &Harness, clip: ClipId) -> bl_model::AudioData {
    match &h.shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

/// Clicks the field named `name` and types `keys`, then Return.
fn type_into(h: &mut Harness, name: &str, keys: &[Key]) {
    let all = nodes(h);
    h.click(centre(Harness::node(&all, name)));
    for key in keys {
        h.key(*key, Modifiers::NONE);
    }
    h.key(Key::Enter, Modifiers::NONE);
}

/// A Session MIDI clip on the first MIDI track, selected, with the Clip View
/// open.
fn session_clip() -> (Harness, ClipId) {
    let mut h = Harness::with_title_bar(Window::Main);
    let track = h.shell.editor().document().top_level().next().unwrap();
    let scene = h.shell.editor().document().scenes().next().unwrap().id;
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    h.shell
        .run("session.select", json!({ "track": track, "scene": scene }))
        .unwrap();
    let clip = h.shell.editor().document().slot(track, scene).unwrap().id;
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.settle();
    (h, clip)
}

/// Two four-bar MIDI clips in the Arrangement, both selected.
fn arrangement_clips() -> (Harness, ClipId, ClipId) {
    let mut h = Harness::with_title_bar(Window::Main);
    let track = h.shell.editor().document().top_level().next().unwrap();
    let mut ids = Vec::new();
    for bar in [0, 5] {
        let id: ClipId = h.shell.new_id();
        let clip = Clip::new(
            id,
            Ticks::WHOLE * bar,
            Ticks::WHOLE * 4,
            ClipData::Midi(MidiData::new([])),
        );
        h.shell
            .run("clip.create", json!({ "track": track, "clip": clip }))
            .unwrap();
        ids.push(id);
    }
    h.shell
        .run("arrangement.select_clips", json!({ "clips": ids }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.settle();
    (h, ids[0], ids[1])
}

/// A folder under the system's temporary folder holding one WAV file.
struct Tone(PathBuf);

impl Tone {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("bl-main-properties-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Tone.wav");
        let frames = 96_000u32;
        let data = frames * 2;
        let mut bytes = Vec::new();
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
            let phase = f64::from(i) * 440.0 / 48_000.0 * std::f64::consts::TAU;
            bytes.extend_from_slice(&((phase.sin() * 3000.0) as i16).to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }
}

impl Drop for Tone {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        if let Some(dir) = self.0.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// An Arrangement audio clip, selected, with the Clip View open.
fn audio_clip(name: &str) -> (Harness, ClipId, Tone) {
    let tone = Tone::new(name);
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
            json!({ "paths": [tone.0.display().to_string()], "track": track, "at": 0 }),
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
    (h, clip, tone)
}

// Spec: clip-properties R1, R2. A Session MIDI clip shows Main Properties
// first, its controls in L-CLIP-004's order down the left group (x 251 to
// 417 at the reference's size), and the selector switches to Launch
// Properties and back without an undo step.
#[test]
fn the_panel_shows_its_controls_in_order_and_the_selector_switches() {
    let (mut h, _) = session_clip();
    let all = nodes(&mut h);
    let order = [
        "Start",
        "End",
        "Duplicate",
        "Loop",
        "Position",
        "Length",
        "Signature Numerator",
        "Groove",
        "Scale",
    ];
    let mut last = f64::MIN;
    for name in order {
        let bounds = Harness::node(&all, name).bounds.expect(name);
        assert!(bounds.y0 > last, "{name} comes under the control before it");
        last = bounds.y0;
        assert!(
            bounds.x1 - bounds.x0 > 0.0 && bounds.x1 - bounds.x0 <= 166.0,
            "{name}"
        );
    }
    let panel = Harness::node(&all, "Main Properties");
    let group = panel.bounds.expect("the panel's bounds");
    for name in order {
        let bounds = Harness::node(&all, name).bounds.unwrap();
        assert!(
            bounds.x0 >= group.x0 && bounds.x1 <= group.x0 + 174.0,
            "{name} is in the left group"
        );
    }
    // The Show buttons and the Set buttons sit beside Start, End, Position
    // and Length; Scale waits for its package, and Groove is live (the
    // grooves spec R29).
    for marker in ["Start", "End", "Position", "Length"] {
        Harness::node(&all, &format!("Show {marker}"));
        assert!(
            Harness::node(&all, &format!("Set {marker}")).disabled,
            "Set waits for playback"
        );
    }
    assert!(Harness::node(&all, "Scale").disabled);
    assert!(
        Harness::node(&all, "Scale")
            .description
            .starts_with("Not built yet")
    );
    assert!(!Harness::node(&all, "Groove").disabled);

    let before = steps(&h);
    let launch = all
        .iter()
        .find(|n| n.label == "Launch Properties" && n.toggled == Some(false))
        .expect("the Launch Properties selector");
    h.click(centre(launch));
    assert_eq!(
        h.shell
            .clip_view()
            .shown_properties(h.shell.editor().document(), h.shell.arrangement()),
        Properties::Launch
    );
    let all = nodes(&mut h);
    assert!(
        all.iter().all(|n| n.label != "Start"),
        "Main Properties is gone"
    );
    Harness::node(&all, "Launch Mode");
    let main = all
        .iter()
        .find(|n| n.label == "Main Properties" && n.toggled == Some(false))
        .expect("the Main Properties selector");
    h.click(centre(main));
    let all = nodes(&mut h);
    Harness::node(&all, "Start");
    assert_eq!(steps(&h), before);
}

// Spec: clip-properties R3, R6, R7, R9, R10, R12. The fields read the
// clip's markers, and typing into each runs its command as one undo step.
#[test]
fn the_fields_read_the_markers_and_typing_runs_their_commands() {
    let (mut h, clip) = session_clip();
    h.shell
        .run(
            "clip.set_midi_loop",
            json!({ "clip": clip, "start": 0, "end": Ticks::WHOLE.get() }),
        )
        .unwrap();
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Start").value, "1. 1. 1");
    assert_eq!(Harness::node(&all, "Position").value, "1. 1. 1");
    // A one-bar Length reads 1. 0. 0.
    assert_eq!(Harness::node(&all, "Length").value, "1. 0. 0");
    assert_eq!(Harness::node(&all, "Signature Numerator").value, "4");
    assert_eq!(Harness::node(&all, "Signature Denominator").value, "4");
    assert_eq!(Harness::node(&all, "Loop").toggled, Some(false));

    // Start: 1.2.1 is a quarter in.
    let before = steps(&h);
    type_into(
        &mut h,
        "Start",
        &[Key::Num1, Key::Period, Key::Num2, Key::Period, Key::Num1],
    );
    assert_eq!(midi(&h, clip).start, Ticks::QUARTER);
    assert_eq!(steps(&h), before + 1);
    assert_eq!(Harness::node(&nodes(&mut h), "Start").value, "1. 2. 1");

    // Esc cancels what was typed.
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Start")));
    h.key(Key::Num3, Modifiers::NONE);
    h.key(Key::Escape, Modifiers::NONE);
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(midi(&h, clip).start, Ticks::QUARTER);
    assert_eq!(steps(&h), before + 1);

    // A value the command refuses changes nothing and the field reads the
    // marker again.
    type_into(
        &mut h,
        "End",
        &[Key::Num1, Key::Period, Key::Num1, Key::Period, Key::Num1],
    );
    assert_eq!(midi(&h, clip).end, None);
    assert_eq!(Harness::node(&nodes(&mut h), "End").value, "2. 2. 1");
    assert_eq!(steps(&h), before + 1);

    // Length: 2.0.0 is two bars.
    type_into(
        &mut h,
        "Length",
        &[Key::Num2, Key::Period, Key::Num0, Key::Period, Key::Num0],
    );
    assert_eq!(midi(&h, clip).loop_end, Some(Ticks::WHOLE * 2));
    assert_eq!(steps(&h), before + 2);

    // The Loop switch and the Signature.
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Loop")));
    assert!(midi(&h, clip).looping);
    assert_eq!(steps(&h), before + 3);
    h.accesskit(
        egui::Id::new(("number-box", "Signature Numerator")),
        egui::accesskit::Action::Increment,
    );
    assert_eq!(midi(&h, clip).signature, TimeSignature::new(5, 4).ok());
    assert_eq!(steps(&h), before + 4);
    assert_eq!(
        Harness::node(&nodes(&mut h), "Signature Numerator").value,
        "5"
    );
    // Position now counts in 5/4: 2.1.1 is five quarters.
    type_into(
        &mut h,
        "Position",
        &[Key::Num2, Key::Period, Key::Num1, Key::Period, Key::Num1],
    );
    assert_eq!(midi(&h, clip).loop_start, Ticks::QUARTER * 5);

    // Duplicate runs Duplicate Loop: the loop doubles.
    h.shell
        .run(
            "clip.set_midi_loop",
            json!({ "clip": clip, "start": 0, "end": Ticks::WHOLE.get() }),
        )
        .unwrap();
    let before = steps(&h);
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Duplicate")));
    assert_eq!(midi(&h, clip).loop_end, Some(Ticks::WHOLE * 2));
    assert_eq!(steps(&h), before + 1);
}

// Spec: clip-properties R3, R6, R10. Unwarped audio reads in time, can't loop
// and has no signature; warped audio reads in bars, takes positions before
// 1.1.1 and a signature of its own.
#[test]
fn audio_clips_read_in_time_or_bars_and_loop_only_when_warped() {
    let (mut h, clip, _tone) = audio_clip("audio");
    h.shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": false }))
        .unwrap();
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Start").value, "0:00.000");
    assert_eq!(Harness::node(&all, "End").value, "0:02.000");
    for name in [
        "Loop",
        "Signature Numerator",
        "Signature Denominator",
        "Duplicate",
    ] {
        assert!(Harness::node(&all, name).disabled, "{name}");
    }
    // Typing a time sets the Start marker in seconds.
    type_into(
        &mut h,
        "Start",
        &[
            Key::Num0,
            Key::Colon,
            Key::Num0,
            Key::Num1,
            Key::Period,
            Key::Num5,
        ],
    );
    assert!((audio(&h, clip).start - 1.5).abs() < 1e-9);

    h.shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    h.shell
        .run(
            "clip.set_region",
            json!({ "clip": clip, "start": 0, "ticks": true }),
        )
        .unwrap();
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Start").value, "1. 1. 1");
    for name in ["Loop", "Signature Numerator", "Signature Denominator"] {
        assert!(!Harness::node(&all, name).disabled, "{name}");
    }
    // 0.3.1 is before the beginning: -1. 3. 1.
    type_into(
        &mut h,
        "Start",
        &[Key::Num0, Key::Period, Key::Num3, Key::Period, Key::Num1],
    );
    assert_eq!(audio(&h, clip).start, -((Ticks::QUARTER * 2).get() as f64));
    assert_eq!(Harness::node(&nodes(&mut h), "Start").value, "-1. 3. 1");
    h.accesskit(
        egui::Id::new(("number-box", "Signature Denominator")),
        egui::accesskit::Action::Increment,
    );
    assert_eq!(audio(&h, clip).signature, TimeSignature::new(4, 8).ok());
}

// Spec: clip-properties R11. With two clips selected, Start typed sets both
// in one undo step.
#[test]
fn start_typed_with_two_clips_selected_sets_both_in_one_step() {
    let (mut h, first, second) = arrangement_clips();
    let before = steps(&h);
    type_into(
        &mut h,
        "Start",
        &[Key::Num1, Key::Period, Key::Num2, Key::Period, Key::Num1],
    );
    assert_eq!(midi(&h, first).start, Ticks::QUARTER);
    assert_eq!(midi(&h, second).start, Ticks::QUARTER);
    assert_eq!(steps(&h), before + 1);
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(midi(&h, first).start, Ticks::ZERO);
    assert_eq!(midi(&h, second).start, Ticks::ZERO);
}

// Spec: clip-properties R11; S-FOCUS-010. Option-Shift-P (Alt-Shift-P on
// Windows and Linux) is `focus.clip_panels`.
#[test]
fn the_keymap_reaches_clip_panels_with_option_shift_p() {
    let keymap = bl_ui::keymap::Keymap::live12();
    for platform in [bl_ui::keymap::Platform::Mac, bl_ui::keymap::Platform::Other] {
        let hit = keymap
            .resolve(
                platform,
                Key::P,
                bl_ui::keymap::Mods {
                    alt: true,
                    shift: true,
                    ..bl_ui::keymap::Mods::default()
                },
                &[bl_commands::Context::Global],
            )
            .expect("Option-Shift-P");
        assert_eq!(hit.command.id, "focus.clip_panels", "{platform:?}");
    }
}

// Spec: clip-properties R11. Navigate ▸ Clip Panels gives the panel's first
// control the keyboard focus, and Option-Shift-P does the same.
#[test]
fn clip_panels_focuses_the_first_control_of_the_panel() {
    let (mut h, _) = session_clip();
    h.shell.set_focused_area(bl_shell::Area::MainView);
    h.key(Key::P, Modifiers::ALT | Modifiers::SHIFT);
    h.settle();
    let start = egui::Id::new(("number-box", "Start"));
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(start));
    // Launch Properties' first control is its Follow Action switch.
    h.shell
        .run("clip_view.show_properties", json!({ "panel": "launch" }))
        .unwrap();
    h.settle();
    h.shell.run("focus.clip_panels", json!({})).unwrap();
    h.settle();
    assert_eq!(
        h.ctx.memory(|m| m.focused()),
        Some(egui::Id::new(("switch", "follow-enabled")))
    );
}
