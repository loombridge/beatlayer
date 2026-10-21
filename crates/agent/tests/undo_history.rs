//! The Undo History through the agent API (WP-6.28): an agent's batch is
//! one row marked with the agent's name, an agent can jump through the
//! history, the jump waits for the person's gesture to end, and the query
//! tool reads the history back.

use bl_agent::client::{Client, InProcess};
use bl_agent::{Agent, Credential, Outcome, QuestionKind};
use bl_shell::Shell;
use serde_json::{Value, json};

/// The app's agent API on a headless shell: the person allows the agent
/// called `editor` to connect.
fn app() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::for_app(), Shell::headless()));
    client.transport().answer = Box::new(|q| matches!(q.kind, QuestionKind::Connect { .. }));
    client.initialize("editor").unwrap();
    client
}

fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|e| panic!("{tool}: {e}"))
}

/// The rows of the history, from the top, as the query tool gives them.
fn rows(client: &mut Client<InProcess>) -> Vec<Value> {
    call(client, "query", json!({ "path": "history" }))["rows"]["items"]
        .as_array()
        .expect("the history's rows")
        .clone()
}

#[test]
fn an_agents_batch_is_one_row_marked_with_the_agents_name() {
    // Spec: undo-history R9, R10. The editor records who made each step,
    // and the query tool reads it back.
    let mut client = app();
    let track = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "track.create",
                               "args": { "id": { "$new": "keys" }, "kind": "midi", "name": "Keys", "index": 0 } }] }),
    );
    assert_eq!(track["ran"], 1);

    let rows = rows(&mut client);
    assert_eq!(rows.len(), 2, "the step and the set's start");
    assert_eq!(rows[0]["label"], "editor: Insert Track");
    assert_eq!(rows[0]["undone"], false);
    assert_eq!(
        rows[0]["made_by"],
        json!({ "kind": "agent", "name": "editor" })
    );
    assert_eq!(rows[0]["index"], 1);
    assert_eq!(rows[1]["label"], "New Beatlayer Set");
    assert_eq!(rows[1]["made_by"], Value::Null);
    assert_eq!(rows[1]["index"], 0);

    // The song's overview counts the steps, and says which row is current.
    let song = call(&mut client, "query", json!({ "path": "song" }));
    assert_eq!(song["undo"]["steps"], 1);
    assert_eq!(song["undo"]["redo_steps"], 0);
    assert_eq!(song["undo"]["current"], 1);
    assert_eq!(song["undo"]["next_undo"], "editor: Insert Track");
}

#[test]
fn an_agent_jumps_through_the_history_and_reads_it_back() {
    // Spec: undo-history R6, R10.
    let mut client = app();
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "track.create",
                               "args": { "id": { "$new": "keys" }, "kind": "midi", "name": "Keys", "index": 0 } }] }),
    );
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "track.create",
                               "args": { "id": { "$new": "bass" }, "kind": "midi", "name": "Bass", "index": 1 } }] }),
    );

    // Back to the set's start: every step is undone.
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "edit.go_to_step", "args": { "index": 0 } }] }),
    );
    let song = call(&mut client, "query", json!({ "path": "song" }));
    assert_eq!(song["undo"]["current"], 0);
    assert_eq!(song["undo"]["steps"], 0);
    assert_eq!(song["undo"]["redo_steps"], 2);
    let rows = rows(&mut client);
    assert!(rows[0]["undone"].as_bool().unwrap());
    assert!(rows[1]["undone"].as_bool().unwrap());

    // And forward again.
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "edit.go_to_step", "args": { "index": 2 } }] }),
    );
    let song = call(&mut client, "query", json!({ "path": "song" }));
    assert_eq!(song["undo"]["current"], 2);
    assert_eq!(song["undo"]["steps"], 2);
    assert_eq!(song["undo"]["redo_steps"], 0);
}

#[test]
fn a_jump_cannot_share_a_batch_with_other_commands() {
    // Spec: undo-history R7. A jump is no step of its own, so it can't go
    // in a batch that records one.
    let mut client = app();
    let failed = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "edit.go_to_step", "args": { "index": 0 } },
                            { "command": "track.create",
                              "args": { "id": { "$new": "keys" }, "kind": "midi",
                                        "name": "Keys", "index": 0 } }] }),
    );
    assert!(
        failed["error"]
            .as_str()
            .unwrap_or_default()
            .contains("`edit.go_to_step` runs on its own"),
        "{failed}"
    );
}

/// The agent API driven as the app's host drives it, without a client:
/// the session's messages go to `Agent::handle`, and calls that wait are
/// resumed with `Agent::resume`.
struct Host {
    agent: Agent,
    shell: Shell,
    session: String,
    next: u64,
}

impl Host {
    fn new() -> Self {
        let mut agent = Agent::for_app();
        let session = agent.open_session_as(None, Credential::Server);
        let mut host = Self {
            agent,
            shell: Shell::headless(),
            session,
            next: 0,
        };
        let outcome = host.send(
            "initialize",
            json!({ "protocolVersion": "2025-11-25", "capabilities": {},
                    "clientInfo": { "name": "editor" } }),
        );
        let Outcome::Ask(question) = outcome else {
            panic!("{outcome:?}")
        };
        host.agent
            .resolve(&mut host.shell, question.id, true)
            .unwrap();
        host
    }

    fn send(&mut self, method: &str, params: Value) -> Outcome {
        self.next += 1;
        let message = json!({ "jsonrpc": "2.0", "id": self.next, "method": method,
                              "params": params });
        self.agent.handle(&mut self.shell, &self.session, message)
    }

    fn tool(&mut self, tool: &str, args: Value) -> Outcome {
        self.send("tools/call", json!({ "name": tool, "arguments": args }))
    }
}

#[test]
fn an_agents_jump_waits_for_the_persons_gesture() {
    // Spec: undo-history R10. An agent's jump waits while a person's
    // gesture is under way, as every agent call does.
    let mut host = Host::new();
    let track: bl_model::TrackId = host.shell.new_id();
    host.shell
        .run(
            "track.create",
            json!({ "id": track, "kind": "midi", "name": "Keys", "index": 0 }),
        )
        .unwrap();
    host.shell.begin_gesture("Change volume");
    let jump = host.tool(
        "run",
        json!({ "commands": [{ "command": "edit.go_to_step", "args": { "index": 0 } }] }),
    );
    let Outcome::Wait(_) = jump else {
        panic!("the jump waits while the drag goes on")
    };
    assert!(host.agent.resume(&mut host.shell).is_none());
    host.shell.end_gesture();

    let mut done = Vec::new();
    while let Some((_ticket, outcome)) = host.agent.resume(&mut host.shell) {
        done.push(outcome);
    }
    assert_eq!(done.len(), 1, "the jump ran after the gesture ended");
    let Outcome::Reply(Some(reply)) = &done[0] else {
        panic!("{done:?}")
    };
    assert_eq!(reply["result"]["structuredContent"]["ran"], 1);
    // The jump undid the step, and recorded none.
    assert_eq!(host.shell.editor().current_index(), 0);
    assert_eq!(host.shell.editor().history().len(), 1);
}
