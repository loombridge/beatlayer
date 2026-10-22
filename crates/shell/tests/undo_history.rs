//! The Undo History (WP-6.28): the panel's switch, which is view state
//! and no undo step, and the jump through the history, which is a
//! registered command the agent API can run.

use bl_assistant::message::{Event, Reply, Request, Stop};
use bl_assistant::provider::{Error, Provider};
use bl_assistant::tools::ToolCall;
use bl_model::TrackId;
use bl_shell::Shell;
use bl_shell::assistant::{Decision, PromptRequest, Prompter, run_turn};
use bl_shell::commands::{VIEW_STATE, VIEW_TOGGLE_UNDO_HISTORY};
use serde_json::json;

/// A shell with a MIDI track, and the edits that make the history.
fn shell() -> Shell {
    let mut shell = Shell::headless();
    let track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": "midi", "name": "Keys", "index": 0 }),
        )
        .unwrap();
    shell
        .run("mixer.set_volume", json!({ "track": track, "volume": 0.5 }))
        .unwrap();
    shell
        .run("mixer.set_pan", json!({ "track": track, "pan": 0.5 }))
        .unwrap();
    shell
}

#[test]
fn the_panels_switch_is_view_state_and_no_undo_step() {
    // Spec: undo-history R1, R3. Whether the panel shows isn't saved with
    // the set and isn't an undo step.
    let mut shell = shell();
    let steps = shell.editor().undo_history().len();
    assert!(!shell.undo_history_open());

    let result = shell.run(VIEW_TOGGLE_UNDO_HISTORY, json!({})).unwrap();
    assert_eq!(result["open"], true);
    assert_eq!(result["steps"], steps);
    assert_eq!(result["current"], steps);
    assert!(shell.undo_history_open());
    // The switch is no undo step.
    assert_eq!(shell.editor().undo_history().len(), steps);

    // `view.state` reports whether it shows.
    let state = shell.run(VIEW_STATE, json!({})).unwrap();
    assert_eq!(state["undo_history"], true);

    // Running it again hides it.
    shell.run(VIEW_TOGGLE_UNDO_HISTORY, json!({})).unwrap();
    assert!(!shell.undo_history_open());
    let state = shell.run(VIEW_STATE, json!({})).unwrap();
    assert_eq!(state["undo_history"], false);
    assert_eq!(shell.editor().undo_history().len(), steps);
}

#[test]
fn a_jump_returns_the_set_to_its_point_and_is_no_step() {
    // Spec: undo-history R6, R7. The jump undoes or redoes as many steps
    // as it takes, and is no step of its own.
    let mut shell = shell();
    let track = shell.editor().document().top_level().next().unwrap();
    let steps = shell.editor().undo_history().len();
    assert_eq!(steps, 3);

    // Back to the step before the newest one.
    shell.run("edit.go_to_step", json!({ "index": 2 })).unwrap();
    assert_eq!(
        shell.editor().document().track(track).unwrap().mixer().pan,
        0.0
    );
    assert_eq!(
        shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .mixer()
            .volume,
        0.5
    );
    assert_eq!(shell.editor().history().len(), steps);
    assert_eq!(shell.editor().current_index(), 2);

    // The step above it is greyed, and can be redone.
    let rows = shell.undo_history_rows();
    assert!(rows[0].undone);
    assert_eq!(rows[0].label, "Change pan");
    assert!(!rows[1].undone);

    // Forward again gives the set back.
    shell.run("edit.go_to_step", json!({ "index": 3 })).unwrap();
    assert_eq!(
        shell.editor().document().track(track).unwrap().mixer().pan,
        0.5
    );
    assert_eq!(shell.editor().history().len(), steps);

    // A row the history doesn't hold is refused.
    assert!(shell.run("edit.go_to_step", json!({ "index": 9 })).is_err());
}

#[test]
fn a_new_edit_after_a_jump_discards_the_greyed_steps() {
    // Spec: undo-history R7.
    let mut shell = shell();
    let track = shell.editor().document().top_level().next().unwrap();
    shell.run("edit.go_to_step", json!({ "index": 1 })).unwrap();
    assert!(shell.editor().can_redo());

    shell
        .run("mixer.set_mute", json!({ "track": track, "mute": true }))
        .unwrap();
    assert!(!shell.editor().can_redo());
    assert_eq!(shell.editor().history().len(), 2);
}

#[test]
fn an_agents_step_says_who_made_it() {
    // Spec: undo-history R9. The editor records who made each step.
    let mut shell = shell();
    let track = shell.editor().document().top_level().next().unwrap();
    shell
        .run_labelled(
            bl_model::Maker::Agent("editor".to_owned()),
            "editor: Mute track",
            "mixer.set_mute",
            json!({ "track": track, "mute": true }),
        )
        .unwrap();
    let rows = shell.undo_history_rows();
    assert_eq!(rows[0].maker, bl_model::Maker::Agent("editor".to_owned()));
    assert_eq!(rows[0].label, "editor: Mute track");
    // The person's own step says so.
    assert_eq!(rows[1].maker, bl_model::Maker::Person);
}

/// A provider answering with a fixed script.
struct Script(Vec<Reply>);

impl Provider for Script {
    fn stream(&self, _request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error> {
        let mut replies = self.0.clone();
        let reply = replies.remove(0);
        on_event(Event::Text(reply.text.clone()));
        Ok(reply)
    }
}

/// A prompter that never asks: the commands the test runs don't need it.
struct Quiet;

impl Prompter for Quiet {
    fn ask(&mut self, request: &PromptRequest) -> Decision {
        panic!("no prompt was expected: {request:?}")
    }
}

#[test]
fn an_assistants_turn_is_one_row_marked_with_the_assistant() {
    // Spec: undo-history R9. The editor records who made each step, and
    // the assistant's turn is the assistant's.
    let mut shell = shell();
    let track: TrackId = shell.editor().document().top_level().next().unwrap();
    let provider = Script(vec![
        Reply {
            text: "Muting the track.".to_owned(),
            tool_calls: vec![ToolCall {
                id: "c1".to_owned(),
                name: "mixer_set_mute".to_owned(),
                arguments: json!({ "track": track, "mute": true }),
            }],
            stop: Stop::End,
        },
        Reply {
            text: "Muted.".to_owned(),
            tool_calls: vec![],
            stop: Stop::End,
        },
    ]);
    run_turn(&mut shell, &provider, &mut Quiet, "Mute the track.").unwrap();

    let rows = shell.undo_history_rows();
    assert_eq!(rows[0].maker, bl_model::Maker::Assistant);
    assert_eq!(rows[0].label, "Assistant");
    // The turn is one step, and the person's own steps are still theirs.
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[1].maker, bl_model::Maker::Person);
}
