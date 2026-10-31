//! The built-in assistant's settings on the Agents page (WP-2.23, part 4),
//! headless: the provider, base URL, model, the masked API key entry, and
//! the activity log, every control reachable by a screen reader and each
//! change a registered command. The key is stored in the keychain and never
//! echoed back.

use crate::common;
use std::collections::VecDeque;
use std::sync::Mutex;

use bl_assistant::agent::Agent;
use bl_assistant::message::{Event, Reply, Request, Stop};
use bl_assistant::provider::{Error, Provider};
use bl_assistant::tools::ToolCall;
use bl_shell::assistant::{
    AssistantCommands, AssistantProvider, Decision, PromptRequest, Prompter,
};
use bl_shell::settings::SettingsPage;
use common::{Harness, Window};
use egui::accesskit::{Action, ActionRequest, Role, TreeId};
use egui::{Id, Key, Modifiers};
use serde_json::json;

/// A provider answering with a fixed script, for filling the activity log.
struct Script(Mutex<VecDeque<Reply>>);

impl Script {
    fn new(replies: Vec<Reply>) -> Self {
        Self(Mutex::new(replies.into()))
    }
}

impl Provider for Script {
    fn stream(&self, _request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error> {
        let reply = self
            .0
            .lock()
            .expect("the script's lock")
            .pop_front()
            .expect("the script ran out of replies");
        for call in &reply.tool_calls {
            on_event(Event::ToolCall(call.clone()));
        }
        Ok(reply)
    }
}

/// A prompter for turns that never need one.
struct Quiet;

impl Prompter for Quiet {
    fn ask(&mut self, request: &PromptRequest) -> Decision {
        panic!("no prompt was expected: {request:?}")
    }
}

fn open_agents() -> Harness {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": "agents" }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    assert_eq!(h.shell.settings_page(), SettingsPage::Agents);
    h
}

/// Clicks the node labelled `label`, as a screen reader does.
fn press(h: &mut Harness, label: &str) {
    let nodes = Harness::nodes(&h.frame(vec![]));
    let id = Harness::node(&nodes, label).id;
    h.frame(vec![egui::Event::AccessKitActionRequest(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: id,
        data: None,
    })]);
    h.settle();
}

#[test]
fn the_assistant_settings_show_on_the_agents_page_and_are_reachable_by_a_screen_reader() {
    let mut h = open_agents();
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(
        Harness::node(&nodes, "Assistant Provider").role,
        Role::ComboBox,
        "the provider is a chooser"
    );
    assert_eq!(
        Harness::node(&nodes, "Assistant Base URL").role,
        Role::TextInput
    );
    assert_eq!(
        Harness::node(&nodes, "Assistant Model").role,
        Role::TextInput
    );
    assert_eq!(
        Harness::node(&nodes, "Assistant API Key").role,
        Role::PasswordInput,
        "the key is typed into a masked field"
    );
    assert_eq!(
        Harness::node(&nodes, "Set Assistant API Key").role,
        Role::Button
    );
    assert_eq!(
        Harness::node(&nodes, "Open Assistant Activity Log").role,
        Role::Button
    );
    assert_eq!(Harness::node(&nodes, "Key").value, "None stored");

    // Choosing a provider is a registered command.
    let provider = Id::new(("chooser", "assistant-provider"));
    h.accesskit(provider, Action::Focus);
    h.settle();
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(
        h.shell.assistant_settings().provider,
        AssistantProvider::Anthropic
    );
}

#[test]
fn a_stored_key_is_reported_without_being_echoed_and_can_be_forgotten() {
    let mut h = open_agents();
    h.shell
        .run("assistant.set_key", json!({ "key": "sk-test-secret" }))
        .unwrap();
    h.shell
        .run(
            "assistant.set_model",
            json!({ "model": "claude-sonnet-4-5" }),
        )
        .unwrap();
    h.settle();

    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Key").value, "Stored in the keychain");
    assert_eq!(
        Harness::node(&nodes, "Assistant Model").value,
        "claude-sonnet-4-5"
    );
    assert_eq!(
        Harness::node(&nodes, "Forget Assistant API Key").role,
        Role::Button
    );

    // The field never holds the key: it's the person's, masked, and cleared.
    assert_ne!(
        Harness::node(&nodes, "Assistant API Key").value,
        "sk-test-secret",
        "the key is never echoed into the field"
    );

    press(&mut h, "Forget Assistant API Key");
    assert!(!h.shell.assistant_key_set(), "Forget removes the key");
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Key").value, "None stored");
}

#[test]
fn the_activity_log_opens_shows_what_ran_and_closes() {
    let mut h = open_agents();
    press(&mut h, "Open Assistant Activity Log");
    assert!(h.shell.assistant_log_open());
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(
        nodes
            .iter()
            .any(|n| n.value.contains("hasn't run a command")),
        "the empty log says so"
    );

    // Run one turn so the log records a command, then draw it.
    let track = h
        .shell
        .editor()
        .document()
        .tracks()
        .next()
        .expect("the default set has a track")
        .id();
    let script = Script::new(vec![
        Reply {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "c1".to_owned(),
                name: "run".to_owned(),
                arguments: json!({
                    "command": "track.rename",
                    "args": { "track": track, "name": "From the Log" },
                }),
            }],
            stop: Stop::End,
        },
        Reply {
            text: "Renamed it.".to_owned(),
            tool_calls: Vec::new(),
            stop: Stop::End,
        },
    ]);
    let mut prompter = Quiet;
    {
        let agent = Agent::new();
        let mut request = Request::new("test-model");
        let mut commands = AssistantCommands::new(&mut h.shell, &mut prompter);
        agent
            .run(&script, &mut request, &mut commands, &|| false, &mut |_| {})
            .unwrap();
    }
    assert_eq!(h.shell.assistant_log().len(), 1);

    h.settle();
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(
        nodes
            .iter()
            .any(|n| n.value.contains("track.rename") && n.value.contains("ran")),
        "the log lists each command with its result: {nodes:?}"
    );

    press(&mut h, "Open Assistant Activity Log");
    assert!(!h.shell.assistant_log_open());
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(
        !nodes.iter().any(|n| n.label == "Activity Log"),
        "closing hides the log"
    );
}

#[test]
fn the_model_is_sent_when_the_field_loses_focus_so_a_typed_space_stays() {
    let mut h = open_agents();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let b = Harness::node(&nodes, "Assistant Model").bounds.unwrap();
    h.click(egui::pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    for text in ["my", " ", "model"] {
        h.frame(vec![egui::Event::Text(text.to_owned())]);
        assert_eq!(
            h.shell.assistant_settings().model,
            "",
            "nothing is sent while typing"
        );
    }
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(h.shell.assistant_settings().model, "my model");
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Assistant Model").value, "my model");
}

#[test]
fn clicking_into_the_model_field_keeps_the_stored_model() {
    let mut h = open_agents();
    h.shell
        .invoke("assistant.set_model", json!({ "model": "gpt-4" }))
        .unwrap();
    h.settle();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let b = Harness::node(&nodes, "Assistant Model").bounds.unwrap();
    h.click(egui::pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.frame(vec![]);
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Assistant Model").value, "gpt-4");
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(h.shell.assistant_settings().model, "gpt-4");
}
