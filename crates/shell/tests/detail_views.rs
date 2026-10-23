//! The detail views, driven headless the way the menus, the keymap and
//! agents drive them (detail-views R1 to R17): every command by ID, the
//! detail state after each, and no change to the set.

use bl_commands::Context;
use bl_shell::commands::{DetailState, DetailView};
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

/// Runs a detail command and reads the state it returns.
fn run(shell: &mut Shell, id: &str, args: Value) -> DetailState {
    let result = shell
        .run(id, args)
        .unwrap_or_else(|error| panic!("{id} failed: {error}"));
    let state: DetailState = serde_json::from_value(result).unwrap();
    assert_eq!(state, shell.detail().state(), "{id} returns the state");
    state
}

fn shows(state: &DetailState) -> (bool, bool) {
    (state.clip_view, state.device_view)
}

#[test]
fn a_new_session_shows_the_device_view_alone() {
    // Spec: detail-views R16.
    let shell = Shell::headless();
    let state = shell.detail().state();
    assert_eq!(shows(&state), (false, true));
    assert_eq!(state.alone_last, DetailView::Device);
    assert!(state.hidden.is_empty());
    assert_eq!(state.clip_height, 225.0);
    assert!(!state.expanded);
}

#[test]
fn each_view_has_its_own_switch() {
    // Spec: detail-views R1.
    let mut shell = Shell::headless();
    let both = run(&mut shell, "view.toggle_clip_view", json!({}));
    assert_eq!(shows(&both), (true, true), "the Device View stays");
    assert_eq!(shell.switch_state("view.toggle_clip_view"), Some(true));
    assert_eq!(shell.switch_state("view.toggle_device_view"), Some(true));
    assert!(shell.clip_view().visible());
    let clip = run(&mut shell, "view.toggle_device_view", json!({}));
    assert_eq!(shows(&clip), (true, false), "the Clip View stays");
    assert_eq!(shell.switch_state("view.toggle_device_view"), Some(false));
    let neither = run(&mut shell, "view.toggle_clip_view", json!({}));
    assert_eq!(shows(&neither), (false, false));
    assert!(!shell.clip_view().visible());
    let device = run(&mut shell, "view.toggle_device_view", json!({}));
    assert_eq!(shows(&device), (false, true));
}

#[test]
fn option_click_shows_both_and_changes_nothing_when_both_show() {
    // Spec: detail-views R2.
    let mut shell = Shell::headless();
    let both = run(&mut shell, "view.show_detail_views", json!({}));
    assert_eq!(shows(&both), (true, true));
    let again = run(&mut shell, "view.show_detail_views", json!({}));
    assert_eq!(again, both);
    run(&mut shell, "view.toggle_device_view", json!({}));
    let both = run(&mut shell, "view.show_detail_views", json!({}));
    assert_eq!(shows(&both), (true, true));
}

#[test]
fn switching_shows_the_other_view_moves_the_focus_or_brings_one_back() {
    // Spec: detail-views R5.
    let mut shell = Shell::headless();
    let clip = run(&mut shell, "view.switch_detail_view", json!({}));
    assert_eq!(
        shows(&clip),
        (true, false),
        "one view: the other in its place"
    );
    let device = run(&mut shell, "view.switch_detail_view", json!({}));
    assert_eq!(shows(&device), (false, true));

    run(&mut shell, "view.show_detail_views", json!({}));
    run(&mut shell, "focus.device_view", json!({}));
    let moved = run(&mut shell, "view.switch_detail_view", json!({}));
    assert_eq!(moved.focus, DetailView::Clip, "both: the focus moves");
    assert_eq!(shows(&moved), (true, true));
    assert_eq!(shell.focused_area(), Area::Detail);
    let moved = run(&mut shell, "view.switch_detail_view", json!({}));
    assert_eq!(moved.focus, DetailView::Device);

    // Neither shows: the view that showed alone last comes back.
    run(&mut shell, "view.toggle_clip_view", json!({}));
    run(&mut shell, "view.toggle_device_view", json!({}));
    assert_eq!(shows(&shell.detail().state()), (false, false));
    assert_eq!(shell.detail().state().alone_last, DetailView::Device);
    let back = run(&mut shell, "view.switch_detail_view", json!({}));
    assert_eq!(shows(&back), (false, true));
}

#[test]
fn hiding_the_detail_area_brings_the_same_views_back() {
    // Spec: detail-views R6.
    let mut shell = Shell::headless();
    run(&mut shell, "view.show_detail_views", json!({}));
    let hidden = run(&mut shell, "view.toggle_detail_area", json!({}));
    assert_eq!(shows(&hidden), (false, false));
    assert_eq!(hidden.hidden, [DetailView::Clip, DetailView::Device]);
    assert_eq!(
        shell.dynamic_label("view.toggle_detail_area").as_deref(),
        Some("Show Clip and Device View")
    );
    let shown = run(&mut shell, "view.toggle_detail_area", json!({}));
    assert_eq!(shows(&shown), (true, true));
    assert!(shown.hidden.is_empty());
    assert_eq!(
        shell.dynamic_label("view.toggle_detail_area").as_deref(),
        Some("Hide Clip and Device View")
    );

    // One view hides and comes back alone, with its own label.
    run(&mut shell, "view.toggle_clip_view", json!({}));
    assert_eq!(
        shell.dynamic_label("view.toggle_detail_area").as_deref(),
        Some("Hide Device View")
    );
    run(&mut shell, "view.toggle_detail_area", json!({}));
    assert_eq!(
        shell.dynamic_label("view.toggle_detail_area").as_deref(),
        Some("Show Device View")
    );
    run(&mut shell, "view.toggle_detail_area", json!({}));
    assert_eq!(shows(&shell.detail().state()), (false, true));

    // Anything else done to the views forgets what was hidden; with neither
    // showing and nothing hidden, the Device View comes.
    run(&mut shell, "view.toggle_detail_area", json!({}));
    run(&mut shell, "view.toggle_clip_view", json!({}));
    run(&mut shell, "view.toggle_clip_view", json!({}));
    assert!(shell.detail().state().hidden.is_empty());
    let shown = run(&mut shell, "view.toggle_detail_area", json!({}));
    assert_eq!(shows(&shown), (false, true));
}

#[test]
fn expanding_the_clip_view_goes_back_to_the_height_it_had() {
    // Spec: detail-views R10, R11.
    let mut shell = Shell::headless();
    assert!(!shell.can_invoke("view.expand_clip_view", &json!({})));
    assert!(shell.run("view.expand_clip_view", json!({})).is_err());
    run(&mut shell, "view.toggle_clip_view", json!({}));
    assert!(shell.can_invoke("view.expand_clip_view", &json!({})));
    let sized = run(
        &mut shell,
        "view.set_detail_height",
        json!({ "points": 300 }),
    );
    assert_eq!(sized.clip_height, 300.0);
    let expanded = run(&mut shell, "view.expand_clip_view", json!({}));
    assert!(expanded.expanded);
    assert_eq!(expanded.clip_height, 300.0, "the height it will go back to");
    assert_eq!(shell.switch_state("view.expand_clip_view"), Some(true));
    let back = run(&mut shell, "view.expand_clip_view", json!({}));
    assert!(!back.expanded);
    assert_eq!(back.clip_height, 300.0);

    // Dragging the edge ends the expansion at the dragged height, and so
    // does hiding the Clip View.
    run(&mut shell, "view.expand_clip_view", json!({}));
    let dragged = run(
        &mut shell,
        "view.set_detail_height",
        json!({ "points": 150 }),
    );
    assert!(!dragged.expanded);
    assert_eq!(dragged.clip_height, 150.0);
    run(&mut shell, "view.expand_clip_view", json!({}));
    let hidden = run(&mut shell, "view.toggle_clip_view", json!({}));
    assert!(!hidden.expanded);
    assert!(!shell.can_invoke("view.expand_clip_view", &json!({})));
}

#[test]
fn the_clip_views_height_keeps_to_its_limits() {
    // Spec: detail-views R10.
    let mut shell = Shell::headless();
    let small = run(
        &mut shell,
        "view.set_detail_height",
        json!({ "points": 20 }),
    );
    assert_eq!(small.clip_height, 100.0, "never less than the smallest");
    assert!(
        shell
            .run("view.set_detail_height", json!({ "points": "tall" }))
            .is_err()
    );
}

#[test]
fn the_focus_commands_show_their_view_and_set_the_keys_contexts() {
    // Spec: detail-views R14, R15.
    let mut shell = Shell::headless();
    let state = run(&mut shell, "focus.clip_view", json!({}));
    assert_eq!(shows(&state), (true, true), "the Clip View is shown first");
    assert_eq!(state.focus, DetailView::Clip);
    assert_eq!(shell.focused_area(), Area::Detail);
    let contexts = shell.active_contexts();
    assert!(contexts.contains(&Context::ClipView), "{contexts:?}");
    assert!(!contexts.contains(&Context::DeviceView), "{contexts:?}");

    let state = run(&mut shell, "focus.device_view", json!({}));
    assert_eq!(state.focus, DetailView::Device);
    let contexts = shell.active_contexts();
    assert!(contexts.contains(&Context::DeviceView), "{contexts:?}");
    assert!(!contexts.contains(&Context::ClipView), "{contexts:?}");
    assert!(!contexts.contains(&Context::NoteEditor), "{contexts:?}");

    // A view that shows alone has the focus whichever had it before.
    run(&mut shell, "view.toggle_device_view", json!({}));
    assert!(shell.active_contexts().contains(&Context::ClipView));
    run(&mut shell, "view.toggle_device_view", json!({}));
    run(&mut shell, "view.toggle_clip_view", json!({}));
    assert!(shell.active_contexts().contains(&Context::DeviceView));

    // Hidden, the Device View is shown first.
    run(&mut shell, "view.toggle_device_view", json!({}));
    let state = run(&mut shell, "focus.device_view", json!({}));
    assert_eq!(shows(&state), (false, true));
}

#[test]
fn no_detail_command_changes_the_set_or_adds_an_undo_step() {
    // Spec: detail-views R16, R17.
    let mut shell = Shell::headless();
    let steps = shell.editor().undo_history().count();
    for id in [
        "view.toggle_clip_view",
        "view.toggle_device_view",
        "view.show_detail_views",
        "view.switch_detail_view",
        "view.toggle_detail_area",
        "view.expand_clip_view",
        "focus.clip_view",
        "focus.device_view",
    ] {
        shell.run(id, json!({})).unwrap_or_default();
        assert!(!shell.is_dirty(), "{id} marked the set as changed");
    }
    shell
        .run("view.set_detail_height", json!({ "points": 180 }))
        .unwrap();
    assert!(!shell.is_dirty());
    assert_eq!(shell.editor().undo_history().count(), steps);
}

#[test]
fn view_state_reports_the_detail_area() {
    // Spec: detail-views R17; app-shell R24.
    let mut shell = Shell::headless();
    run(&mut shell, "view.show_detail_views", json!({}));
    run(
        &mut shell,
        "view.set_detail_height",
        json!({ "points": 260 }),
    );
    let state = shell.run("view.state", json!({})).unwrap();
    assert_eq!(state["detail"]["clip_view"], true);
    assert_eq!(state["detail"]["device_view"], true);
    assert_eq!(state["detail"]["clip_height"], 260.0);
    assert_eq!(state["detail"]["focus"], "device");
    assert_eq!(state["clip_view"]["visible"], true);
}

#[test]
fn the_menu_items_follow_the_detail_area() {
    // Spec: detail-views R6, R11. Expand Clip View is enabled whichever view
    // has the focus, as long as the Clip View shows.
    let mut shell = Shell::headless();
    run(&mut shell, "view.show_detail_views", json!({}));
    run(&mut shell, "focus.device_view", json!({}));
    assert!(shell.can_invoke("view.expand_clip_view", &json!({})));
    assert!(shell.can_invoke("focus.clip_view", &json!({})));
    assert!(shell.can_invoke("focus.device_view", &json!({})));
}
