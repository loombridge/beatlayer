//! The agent loop against a scripted provider and a real document: a turn's
//! tool calls run as the registry commands they name and undo together,
//! refusals go back to the model, cancel stops between calls, and the call
//! cap ends a runaway turn.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::Mutex;

use bl_assistant::agent::{Agent, Commands, Stopped};
use bl_assistant::message::{Event, Message, Part, Reply, Request, Role, Stop};
use bl_assistant::provider::{Error, Provider};
use bl_assistant::tools::ToolCall;
use bl_commands::{Permission, Registry};
use bl_model::{ChangeSet, EditError, Editor};
use serde_json::{Value, json};

/// A provider that answers with a script: one reply per call, recording what
/// it was sent.
struct Script {
    replies: Mutex<VecDeque<Reply>>,
    seen: Mutex<Vec<Request>>,
}

impl Script {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> usize {
        self.seen.lock().expect("the script's lock").len()
    }
}

impl Provider for Script {
    fn stream(&self, request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error> {
        self.seen
            .lock()
            .expect("the script's lock")
            .push(request.clone());
        let reply = self
            .replies
            .lock()
            .expect("the script's lock")
            .pop_front()
            .expect("the script ran out of replies");
        if !reply.text.is_empty() {
            on_event(Event::Text(reply.text.clone()));
        }
        for call in &reply.tool_calls {
            on_event(Event::ToolCall(call.clone()));
        }
        Ok(reply)
    }
}

/// The loop's commands over a real document: `permit` refuses what a
/// headless agent API refuses, and `run` runs the command on the editor.
struct DocumentCommands {
    editor: Editor,
}

impl Commands for DocumentCommands {
    fn begin_turn(&mut self, label: &str) {
        self.editor.begin(label.to_owned());
    }

    fn end_turn(&mut self) {
        self.editor.commit().expect("the loop opened a turn");
    }

    fn permit(&mut self, command: &str, _args: &Value, class: Permission) -> Result<(), String> {
        if class.asks_first() {
            Err(format!(
                "`{command}` is {class}, and there is no one to ask."
            ))
        } else {
            Ok(())
        }
    }

    fn run(&mut self, command: &str, args: Value) -> Result<Value, String> {
        bl_model::registry::run(&mut self.editor, command, args)
            .map(|changes| json!({ "changed": !changes.is_empty() }))
            .map_err(|error| error.to_string())
    }
}

/// The loop offering every document command.
fn agent() -> Agent {
    let mut registry = Registry::new();
    bl_model::registry::register(&mut registry).expect("the document's commands");
    Agent::new(registry.iter())
}

/// A tool call.
fn call(id: &str, tool: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: tool.to_owned(),
        arguments: args,
    }
}

/// A reply that calls `calls` and nothing else.
fn calls(calls: Vec<ToolCall>) -> Reply {
    Reply {
        text: String::new(),
        tool_calls: calls,
        stop: Stop::End,
    }
}

/// The model's final answer.
fn answer(text: &str) -> Reply {
    Reply {
        text: text.to_owned(),
        tool_calls: Vec::new(),
        stop: Stop::End,
    }
}

/// A request holding one prompt.
fn request(prompt: &str) -> Request {
    let mut request = Request::new("test-model");
    request.messages.push(Message::user(prompt));
    request
}

/// The Main track's ID, which every test's commands act on.
fn main_id(editor: &Editor) -> u64 {
    editor.document().main_id().id().get()
}

#[test]
fn a_three_call_turn_runs_every_command_and_undoes_as_one_step() {
    let agent = agent().with_label("Trim the Main track");
    let mut commands = DocumentCommands {
        editor: Editor::new(),
    };
    let main = main_id(&commands.editor);
    let before = commands.editor.document().clone();
    let script = Script::new(vec![
        calls(vec![
            call(
                "c1",
                "mixer_set_volume",
                json!({"track": main, "volume": 0.5}),
            ),
            call("c2", "mixer_set_pan", json!({"track": main, "pan": 0.25})),
            call("c3", "transport_set_loop_enabled", json!({"enabled": true})),
        ]),
        answer("Trimmed the Main track."),
    ]);
    let mut request = request("Trim the Main track.");

    let turn = agent
        .run(&script, &mut request, &mut commands, &|| false, &mut |_| {})
        .expect("a finished turn");

    assert_eq!(turn.stopped, None);
    assert_eq!(turn.steps.len(), 3);
    assert!(
        turn.steps.iter().all(|step| step.result.is_ok()),
        "{:?}",
        turn.steps
    );
    assert_eq!(turn.reply.text, "Trimmed the Main track.");
    assert!(!ChangeSet::between(&before, commands.editor.document()).is_empty());

    // The three commands are one undo step, named for the turn: one undo
    // puts the set back, and there's nothing before it.
    assert_eq!(
        commands.editor.undo_history().next(),
        Some("Trim the Main track")
    );
    commands.editor.undo().expect("one undo step");
    assert!(ChangeSet::between(&before, commands.editor.document()).is_empty());
    assert_eq!(commands.editor.undo(), Err(EditError::NothingToUndo));

    // The conversation grew by the model's calls, the results, and its
    // answer, so the next turn carries the whole exchange.
    assert_eq!(request.messages.len(), 4);
    assert_eq!(request.messages[1].role, Role::Assistant);
    assert_eq!(request.messages[1].parts.len(), 3);
    let results = &request.messages[2];
    assert_eq!(results.role, Role::User);
    let contents: Vec<&str> = results
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::Result(result) => Some(result.content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(contents.len(), 3);
}

#[test]
fn a_forbidden_command_is_refused_and_the_refusal_goes_back_to_the_model() {
    let agent = agent();
    let mut commands = DocumentCommands {
        editor: Editor::new(),
    };
    let main = main_id(&commands.editor);
    let before = commands.editor.document().clone();
    let script = Script::new(vec![
        calls(vec![call("c1", "track_delete", json!({"track": main}))]),
        answer("I couldn't delete it."),
    ]);
    let mut request = request("Delete the Main track.");

    let turn = agent
        .run(&script, &mut request, &mut commands, &|| false, &mut |_| {})
        .expect("a finished turn");

    assert_eq!(turn.steps.len(), 1);
    let refusal = turn.steps[0]
        .result
        .as_ref()
        .expect_err("a destructive call is refused");
    assert!(refusal.contains("track.delete"), "{refusal}");
    assert!(refusal.contains("destructive"), "{refusal}");

    // The refusal is the tool result the model reads next turn.
    let result = request.messages[2]
        .parts
        .iter()
        .find_map(|part| match part {
            Part::Result(result) => Some(result),
            _ => None,
        })
        .expect("the refusal as a tool result");
    assert_eq!(result.content, *refusal);

    // Nothing changed and no undo step was recorded.
    assert!(ChangeSet::between(&before, commands.editor.document()).is_empty());
    assert_eq!(commands.editor.undo(), Err(EditError::NothingToUndo));
}

#[test]
fn cancel_stops_the_turn_between_calls_and_keeps_what_ran() {
    let agent = agent();
    let mut commands = DocumentCommands {
        editor: Editor::new(),
    };
    let main = main_id(&commands.editor);
    let before = commands.editor.document().clone();
    let script = Script::new(vec![calls(vec![
        call(
            "c1",
            "mixer_set_volume",
            json!({"track": main, "volume": 0.5}),
        ),
        call("c2", "mixer_set_pan", json!({"track": main, "pan": 0.25})),
        call("c3", "transport_set_loop_enabled", json!({"enabled": true})),
    ])]);
    let mut request = request("Trim the Main track.");

    // Cancel after the first call: the check runs before each call.
    let checks = Cell::new(0);
    let cancel = || {
        checks.set(checks.get() + 1);
        checks.get() > 1
    };
    let turn = agent
        .run(&script, &mut request, &mut commands, &cancel, &mut |_| {})
        .expect("a cancelled turn");

    assert_eq!(turn.stopped, Some(Stopped::Cancelled));
    assert_eq!(turn.steps.len(), 1);
    assert_eq!(script.requests(), 1, "no further model turn after cancel");
    // Every call the model made has a result, run or not, so the next turn
    // is well formed for a provider that pairs calls with results.
    assert_eq!(request.messages[2].parts.len(), 3);

    // The one call that ran is kept, and is one undo step of its own.
    assert!(!ChangeSet::between(&before, commands.editor.document()).is_empty());
    commands.editor.undo().expect("one undo step");
    assert!(ChangeSet::between(&before, commands.editor.document()).is_empty());
}

#[test]
fn the_cap_ends_a_runaway_loop() {
    let agent = agent().with_max_calls(2);
    let mut commands = DocumentCommands {
        editor: Editor::new(),
    };
    let main = main_id(&commands.editor);
    // A model that keeps calling: ten turns of one call each, more than the
    // cap, so the loop has to stop it.
    let script = Script::new(
        (0..10)
            .map(|index| {
                calls(vec![call(
                    &format!("c{index}"),
                    "mixer_set_volume",
                    json!({"track": main, "volume": 0.5}),
                )])
            })
            .collect(),
    );
    let mut request = request("Set the volume.");

    let turn = agent
        .run(&script, &mut request, &mut commands, &|| false, &mut |_| {})
        .expect("a capped turn");

    assert_eq!(turn.stopped, Some(Stopped::Cap));
    assert_eq!(turn.steps.len(), 2, "the cap stops further calls");
    assert_eq!(
        script.requests(),
        3,
        "the loop stopped rather than running on"
    );
}
