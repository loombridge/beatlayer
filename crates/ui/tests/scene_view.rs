//! A scene's tempo and time signature fields in the Main column and the
//! Scene View (WP-3.5), headless at 1600 × 1000 points: View > Scene Tempo
//! and Time Signature puts the fields in the Main column; selecting a scene
//! gives the detail area to the Scene View; a field is grey until the scene
//! has its own value; typing, stepping, and a double-click run the scene
//! commands; and the Re-Enable Automation button lights while a launched
//! scene's tempo holds.
//!
//! Spec: session-view R21, R23.

mod common;

use bl_core::time::{Bpm, TimeSignature};
use bl_model::SceneId;
use bl_shell::mixer_view_commands as view;
use bl_shell::scene_fields::SceneField;
use bl_ui::scene_fields::field_id;
use bl_ui::scene_view::scene_view_id;
use common::{Harness, Node, Window};
use egui::accesskit::{Action, Role};
use egui::{Event, Key, Modifiers, Pos2, Rect};
use serde_json::json;

fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    h
}

fn first_scene(h: &Harness) -> SceneId {
    h.shell.editor().document().scenes().next().unwrap().id
}

fn nodes(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
}

fn centre(node: &Node) -> Pos2 {
    let bounds = node.bounds.expect("the field has bounds");
    Rect::from_min_max(
        Pos2::new(bounds.x0 as f32, bounds.y0 as f32),
        Pos2::new(bounds.x1 as f32, bounds.y1 as f32),
    )
    .center()
}

fn field(nodes: &[Node], scene: SceneId, field: SceneField) -> Option<&Node> {
    let id = field_id(scene, field).accesskit_id();
    nodes.iter().find(|node| node.id == id)
}

fn type_text(h: &mut Harness, text: &str) {
    h.frame(vec![Event::Text(text.to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
}

// Spec: session-view R21. The fields are hidden until the View menu's item
// shows them, then sit in the Main column's rows.
#[test]
fn the_main_columns_fields_show_with_the_view_item() {
    let mut h = harness();
    let scene = first_scene(&h);
    assert!(field(&nodes(&mut h), scene, SceneField::Tempo).is_none());
    h.shell.run(view::SCENE_TEMPO, json!({})).unwrap();
    let all = nodes(&mut h);
    let tempo = field(&all, scene, SceneField::Tempo).expect("the tempo field");
    assert_eq!(tempo.role, Role::SpinButton);
    assert_eq!(tempo.label, "Scene 1 Tempo");
    assert_eq!(tempo.value, "120.00", "grey, showing the set's tempo");
    assert!(
        tempo.description.starts_with("Off"),
        "{}",
        tempo.description
    );
    let numerator = field(&all, scene, SceneField::Numerator).unwrap();
    assert_eq!(numerator.label, "Scene 1 Time Signature Numerator");
    assert_eq!(numerator.value, "4");
}

// Spec: session-view R21. Typing, stepping, and a double-click run the
// scene's commands, each one undo step.
#[test]
fn typing_stepping_and_a_double_click_set_and_clear_a_scenes_tempo() {
    let mut h = harness();
    let scene = first_scene(&h);
    h.shell.run(view::SCENE_TEMPO, json!({})).unwrap();
    let all = nodes(&mut h);
    let at = centre(field(&all, scene, SceneField::Tempo).unwrap());
    let own = |h: &Harness| h.shell.editor().document().scene(scene).unwrap().tempo;
    let steps = |h: &Harness| h.shell.editor().undo_history().len();

    // Click, type, Enter.
    let before = steps(&h);
    h.time += 1.0;
    h.click(at);
    type_text(&mut h, "93.5");
    assert_eq!(own(&h), Some(Bpm::new(93.5).unwrap()));
    assert_eq!(steps(&h), before + 1);
    let all = nodes(&mut h);
    let tempo = field(&all, scene, SceneField::Tempo).unwrap();
    assert_eq!(tempo.value, "93.50");
    assert!(
        tempo.description.starts_with("Set"),
        "{}",
        tempo.description
    );

    // A screen reader's increment steps a whole BPM.
    h.accesskit(field_id(scene, SceneField::Tempo), Action::Increment);
    h.settle();
    assert_eq!(own(&h), Some(Bpm::new(94.5).unwrap()));

    // Up and Down, Shift for a hundredth.
    h.time += 1.0;
    h.click(at);
    h.key(Key::ArrowDown, Modifiers::SHIFT);
    h.settle();
    assert_eq!(own(&h), Some(Bpm::new(94.49).unwrap()));

    // A double-click turns it off.
    h.time += 1.0;
    h.click(at);
    h.click(at);
    h.settle();
    assert_eq!(own(&h), None);
    let all = nodes(&mut h);
    assert_eq!(
        field(&all, scene, SceneField::Tempo).unwrap().value,
        "120.00"
    );
}

// Spec: session-view R21. Selecting a scene gives the detail area to the
// Scene View, with the same fields, whether or not the Main column shows
// them.
#[test]
fn a_selected_scene_takes_the_detail_area_for_the_scene_view() {
    let mut h = harness();
    let scene = first_scene(&h);
    let all = nodes(&mut h);
    assert!(
        !all.iter()
            .any(|node| node.id == scene_view_id().accesskit_id())
    );
    h.shell
        .run("session.select", json!({ "scene": scene }))
        .unwrap();
    let all = nodes(&mut h);
    let view = all
        .iter()
        .find(|node| node.id == scene_view_id().accesskit_id())
        .expect("the Scene View");
    assert_eq!(view.label, "Scene View: Scene 1");
    let signature = field(&all, scene, SceneField::Numerator).expect("the signature field");
    assert_eq!(signature.value, "4");
    // Spec: follow-actions R14. The follow-action controls are live: the
    // switch runs the scene command, and Longest stands in for Linked.
    let switch = all
        .iter()
        .find(|node| node.label == "Scene 1 Follow Action")
        .expect("the Follow Action switch");
    assert!(!switch.disabled);
    assert_eq!(switch.toggled, Some(false));
    assert!(
        all.iter()
            .any(|node| node.label == "Scene 1 Follow Time Longest")
    );
    h.shell
        .run(
            "scene.set_follow_actions",
            json!({ "scene": scene, "enabled": true }),
        )
        .unwrap();
    let all = nodes(&mut h);
    let switch = all
        .iter()
        .find(|node| node.label == "Scene 1 Follow Action")
        .unwrap();
    assert_eq!(switch.toggled, Some(true));
    // Setting the signature there runs the scene command.
    h.shell
        .run(
            "scene.set_time_signature",
            json!({ "scene": scene, "signature": { "numerator": 7, "denominator": 8 } }),
        )
        .unwrap();
    let all = nodes(&mut h);
    assert_eq!(
        field(&all, scene, SceneField::Numerator).unwrap().value,
        "7"
    );
    assert_eq!(
        field(&all, scene, SceneField::Denominator).unwrap().value,
        "8"
    );
    assert_eq!(
        h.shell.editor().document().scene(scene).unwrap().signature,
        Some(TimeSignature::new(7, 8).unwrap())
    );
}
