//! The Clip View's Launch Properties panel, headless (WP-3.9): it shows for
//! a Session clip only, shows the recorded defaults, and each live control
//! runs the clip launch command agents run, one undo step each. The
//! follow-action controls and the nudge buttons are drawn in place and
//! disabled. Spec: launch-quantisation R14; L-CLIP-003, L-CLIP-005.

use crate::common;
use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, LaunchMode, LaunchQuantization, MidiData};
use bl_shell::files::factory_default_set;
use bl_ui::arrangement::benchmark;
use bl_ui::launch_panel::launch_panel_id;
use common::{Harness, Node, Window};
use egui::accesskit::Action;
use egui::{Id, Key, Modifiers, Pos2, Rect};
use serde_json::json;

/// A Session clip on the first MIDI track, selected, with the Clip View
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

// A Session clip gets the panel, with the selector row naming Main
// Properties (disabled, it comes with its own package) and Launch
// Properties; a clip in the Arrangement gets neither.
#[test]
fn the_panel_shows_for_session_clips_only() {
    let (mut h, _) = session_clip();
    let all = nodes(&mut h);
    assert!(
        all.iter()
            .any(|n| n.id == launch_panel_id().accesskit_id() && n.label == "Launch Properties")
    );
    let launch = all
        .iter()
        .find(|n| n.label == "Launch Properties" && n.toggled == Some(true))
        .expect("the Launch Properties selector");
    assert!(!launch.disabled);
    assert!(Harness::node(&all, "Main Properties").disabled);

    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::new(Vec::new())),
            ),
        })
        .unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    let all = nodes(&mut h);
    assert!(Harness::node(&all, "Clip View").bounds.is_some());
    assert!(all.iter().all(|n| n.label != "Launch Properties"));
    assert!(all.iter().all(|n| n.label != "Main Properties"));
}

// The recorded defaults: Trigger, Legato off, Global, 0.0 %; the follow
// actions off, A Next 100 % and B No Action 0 %, Linked 1x, all live; only
// the nudge buttons are pending.
#[test]
fn the_recorded_defaults_show_and_only_the_nudge_buttons_are_pending() {
    let (mut h, _) = session_clip();
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Launch Mode").value, "Trigger");
    assert_eq!(Harness::node(&all, "Legato").toggled, Some(false));
    assert_eq!(Harness::node(&all, "Launch Quantization").value, "Global");
    let velocity = Harness::node(&all, "Velocity");
    assert_eq!(velocity.value, "0.0 %");
    assert_eq!(velocity.numeric, Some(0.0));
    // Spec: follow-actions R1, R13
    assert_eq!(Harness::node(&all, "Follow Action").toggled, Some(false));
    for (name, value) in [
        ("Follow Action A", "Next"),
        ("Follow Action B", "No Action"),
        ("Chance A", "100 %"),
        ("Chance B", "0 %"),
    ] {
        let node = Harness::node(&all, name);
        assert!(!node.disabled, "{name} is live");
        assert_eq!(node.value, value, "{name}");
    }
    assert_eq!(
        Harness::node(&all, "Follow Time Linked").toggled,
        Some(true)
    );
    assert!(
        all.iter().all(|n| n.label != "Jump A"),
        "Jump shows only for Jump"
    );
    for (name, value) in [("Nudge Back", "«"), ("Nudge Forward", "»")] {
        let node = Harness::node(&all, name);
        assert!(node.disabled, "{name} is pending");
        assert_eq!(node.value, value, "{name}");
        assert!(node.description.starts_with("Not built yet"), "{name}");
    }
}

// Spec: follow-actions R3, R4, R9, R13. Each follow control runs the clip's
// command, one undo step, and shows the saved value back; Jump's target
// shows while the action is Jump; the two chances add up to 100.
#[test]
fn the_follow_controls_drive_their_command_and_read_back_the_document() {
    let (mut h, clip) = session_clip();
    let follow = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .clip(clip)
            .unwrap()
            .launch
            .follow
    };

    // The Follow Action switch: a click.
    let before = steps(&h);
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Follow Action")));
    assert!(follow(&h).enabled);
    assert_eq!(steps(&h), before + 1);
    assert_eq!(
        Harness::node(&nodes(&mut h), "Follow Action").toggled,
        Some(true)
    );

    // Action A: Down steps from Next to First, and Jump shows its target
    // box once A is Jump.
    h.shell
        .run(
            "clip.set_follow_actions",
            json!({ "clip": clip, "action_a": "jump", "jump_a": 99 }),
        )
        .unwrap();
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Follow Action A").value, "Jump");
    let jump = Harness::node(&all, "Jump A");
    assert_eq!(jump.value, "8", "held at the number of scenes");

    // Chance B moves chance A the other way.
    let before = steps(&h);
    h.accesskit(Id::new(("number-box", "follow-chance-B")), Action::Focus);
    h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!((follow(&h).chance_a, follow(&h).chance_b()), (99, 1));
    assert_eq!(steps(&h), before + 1);
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Chance A").value, "99 %");
    assert_eq!(Harness::node(&all, "Chance B").value, "1 %");

    // With the loop on, Linked shows its multiplier.
    h.shell
        .run(
            "clip.set_midi_loop",
            json!({ "clip": clip, "start": 0, "end": 3_225_600 }),
        )
        .unwrap();
    h.shell
        .run(
            "clip.set_loop_enabled",
            json!({ "clip": clip, "enabled": true }),
        )
        .unwrap();
    assert_eq!(
        Harness::node(&nodes(&mut h), "Follow Time Multiplier").value,
        "1x"
    );

    // Unlinked: the time reads bars.beats.sixteenths, 1.0.0 to begin with.
    h.shell
        .run(
            "clip.set_follow_actions",
            json!({ "clip": clip, "linked": false }),
        )
        .unwrap();
    let all = nodes(&mut h);
    assert_eq!(Harness::node(&all, "Follow Time").value, "1.0.0");
    assert!(all.iter().all(|n| n.label != "Follow Time Multiplier"));
    h.accesskit(Id::new(("number-box", "follow-time")), Action::Focus);
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(Harness::node(&nodes(&mut h), "Follow Time").value, "0.3.3");
}

// Each live control runs its command: one undo step, and the document and
// the control read back the change.
#[test]
fn every_live_control_drives_its_command_and_reads_back_the_document() {
    let (mut h, clip) = session_clip();
    let launch = |h: &Harness| h.shell.editor().document().clip(clip).unwrap().launch;

    // Legato: a click.
    let before = steps(&h);
    let all = nodes(&mut h);
    h.click(centre(Harness::node(&all, "Legato")));
    assert!(launch(&h).legato);
    assert_eq!(steps(&h), before + 1);
    assert_eq!(Harness::node(&nodes(&mut h), "Legato").toggled, Some(true));

    // Launch mode: Down steps a focused chooser to the next mode.
    let before = steps(&h);
    h.accesskit(Id::new(("chooser", "launch-mode")), Action::Focus);
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(launch(&h).mode, LaunchMode::Gate);
    assert_eq!(steps(&h), before + 1);
    assert_eq!(Harness::node(&nodes(&mut h), "Launch Mode").value, "Gate");

    // Quantize: Down steps from Global to the list's first step, None.
    let before = steps(&h);
    h.accesskit(Id::new(("chooser", "launch-quantize")), Action::Focus);
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(launch(&h).quantization, Some(LaunchQuantization::None));
    assert_eq!(steps(&h), before + 1);
    assert_eq!(
        Harness::node(&nodes(&mut h), "Launch Quantization").value,
        "None"
    );

    // Velocity: a screen reader's increment steps a hundredth.
    let before = steps(&h);
    h.accesskit(
        Id::new(("number-box", "launch-velocity")),
        Action::Increment,
    );
    assert!((launch(&h).velocity_amount - 0.01).abs() < 1e-9);
    assert_eq!(steps(&h), before + 1);
    assert_eq!(Harness::node(&nodes(&mut h), "Velocity").value, "1.0 %");

    // An agent's change shows too.
    h.shell
        .run("clip.set_launch_quantization", json!({ "clip": clip }))
        .unwrap();
    assert_eq!(
        Harness::node(&nodes(&mut h), "Launch Quantization").value,
        "Global"
    );
}
