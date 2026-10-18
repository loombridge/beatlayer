//! The built-in assistant's settings, permission prompts and activity log
//! (WP-2.23, part 4): the settings persist except the API key, which goes
//! only to the keychain; a consent-class call waits for the prompt and a
//! denial reaches the model; every command is recorded; and a command only
//! a person runs is refused.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use bl_assistant::agent::{Agent, Turn};
use bl_assistant::keychain::{Keychain, MemoryKeychain};
use bl_assistant::message::{Event, Reply, Request, Stop};
use bl_assistant::provider::{Error, Provider};
use bl_assistant::tools::ToolCall;
use bl_shell::Shell;
use bl_shell::assistant::{
    AssistantCommands, AssistantProvider, Decision, Outcome, PromptRequest, Prompter,
};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

fn shell_saving_to(dir: &Path) -> Shell {
    let prefs = Preferences::load(dir).unwrap();
    Shell::new(
        prefs.clone(),
        Some(dir.to_owned()),
        AudioHost::null(prefs.audio),
        Box::new(ProjectFiles::new()),
    )
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("bl-shell-assistant-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_assistant_settings_persist_and_only_the_key_goes_to_the_keychain() {
    let dir = temp_dir("settings");
    let keys: Arc<MemoryKeychain> = Arc::new(MemoryKeychain::new());
    let mut shell = shell_saving_to(&dir);
    shell.use_keychain(Arc::clone(&keys) as Arc<dyn Keychain>);
    assert_eq!(
        shell.assistant_settings().provider,
        AssistantProvider::default()
    );

    shell
        .run("assistant.set_provider", json!({ "provider": "anthropic" }))
        .unwrap();
    shell
        .run(
            "assistant.set_base_url",
            json!({ "base_url": "http://localhost:11434/v1" }),
        )
        .unwrap();
    shell
        .run(
            "assistant.set_model",
            json!({ "model": "claude-sonnet-4-5" }),
        )
        .unwrap();
    assert_eq!(
        shell.assistant_settings().provider,
        AssistantProvider::Anthropic
    );
    assert_eq!(shell.assistant_settings().model, "claude-sonnet-4-5");

    // The key goes to the keychain for the current provider, not the file.
    let state = shell
        .run("assistant.set_key", json!({ "key": "sk-test-secret" }))
        .unwrap();
    assert_eq!(state["key_set"], true);
    assert!(state.get("key").is_none(), "results never carry the key");
    assert_eq!(
        keys.read("anthropic").unwrap().as_deref(),
        Some("sk-test-secret")
    );

    let file = std::fs::read_to_string(dir.join("preferences.json")).unwrap();
    assert!(!file.contains("sk-test-secret"), "the key is never written");
    assert!(file.contains("claude-sonnet-4-5"), "{file}");

    // The next start reads the settings back, and the key's presence from
    // the same keychain.
    let mut again = shell_saving_to(&dir);
    again.use_keychain(Arc::clone(&keys) as Arc<dyn Keychain>);
    assert_eq!(
        again.assistant_settings().provider,
        AssistantProvider::Anthropic
    );
    assert_eq!(
        again.assistant_settings().base_url,
        "http://localhost:11434/v1"
    );
    assert_eq!(again.assistant_settings().model, "claude-sonnet-4-5");
    assert!(again.assistant_key_set());

    // Forgetting removes it from the keychain.
    let state = again.run("assistant.forget_key", json!({})).unwrap();
    assert_eq!(state["key_set"], false);
    assert_eq!(keys.read("anthropic").unwrap(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A provider that answers with a script, recording what it was sent.
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

    fn seen(&self) -> Vec<Request> {
        self.seen.lock().expect("the script's lock").clone()
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

/// A prompter that answers from a script, recording the questions.
struct ScriptedPrompter {
    answers: VecDeque<Decision>,
    asked: Vec<PromptRequest>,
}

impl ScriptedPrompter {
    fn new(answers: Vec<Decision>) -> Self {
        Self {
            answers: answers.into(),
            asked: Vec::new(),
        }
    }
}

impl Prompter for ScriptedPrompter {
    fn ask(&mut self, request: &PromptRequest) -> Decision {
        self.asked.push(request.clone());
        self.answers
            .pop_front()
            .expect("the prompter ran out of answers")
    }
}

fn call(id: &str, tool: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: tool.to_owned(),
        arguments: args,
    }
}

fn calls(calls: Vec<ToolCall>) -> Reply {
    Reply {
        text: String::new(),
        tool_calls: calls,
        stop: Stop::End,
    }
}

fn answer(text: &str) -> Reply {
    Reply {
        text: text.to_owned(),
        tool_calls: Vec::new(),
        stop: Stop::End,
    }
}

/// Runs one turn of the loop through the shell's assistant host.
fn one_turn(shell: &mut Shell, provider: &dyn Provider, prompter: &mut dyn Prompter) -> Turn {
    let agent = Agent::new(shell.registry().iter());
    let mut request = Request::new("test-model");
    let mut commands = AssistantCommands::new(shell, prompter);
    agent
        .run(
            provider,
            &mut request,
            &mut commands,
            &|| false,
            &mut |_| {},
        )
        .unwrap()
}

fn a_track(shell: &Shell) -> bl_model::TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .next()
        .expect("the default set has a track")
        .id()
}

#[test]
fn a_consent_class_call_waits_for_the_prompt_and_a_denial_reaches_the_model() {
    let mut shell = Shell::headless();
    let track = a_track(&shell);
    let provider = Script::new(vec![
        calls(vec![call("c1", "track_delete", json!({ "track": track }))]),
        answer("I couldn't delete the track."),
    ]);
    let mut prompter = ScriptedPrompter::new(vec![Decision::Deny]);

    let turn = one_turn(&mut shell, &provider, &mut prompter);

    assert_eq!(prompter.asked.len(), 1, "the call waited for the prompt");
    assert_eq!(prompter.asked[0].command, "track.delete");
    assert!(prompter.asked[0].class.asks_first());
    // The denial is the call's result, in the conversation the provider saw
    // after it.
    let seen = provider.seen();
    let results = &seen[1].messages;
    let denial = results.iter().any(|message| {
        message.parts.iter().any(|part| {
            matches!(
                part,
                bl_assistant::message::Part::Result(result) if result.content.contains("declined")
            )
        })
    });
    assert!(denial, "{results:?}");
    // A declined call changes nothing.
    assert!(shell.editor().document().track(track).is_some());
    // It's recorded in the activity log as denied.
    assert_eq!(shell.assistant_log().len(), 1);
    assert_eq!(shell.assistant_log()[0].command, "track.delete");
    assert!(matches!(
        shell.assistant_log()[0].outcome,
        Outcome::Denied(_)
    ));
    assert_eq!(turn.reply.text, "I couldn't delete the track.");
}

#[test]
fn an_allowed_call_runs_records_its_result_and_always_stops_asking() {
    let mut shell = Shell::headless();
    let track = a_track(&shell);
    let provider = Script::new(vec![
        calls(vec![call("c1", "track_delete", json!({ "track": track }))]),
        answer("Deleted it."),
    ]);
    let mut prompter = ScriptedPrompter::new(vec![Decision::Always]);

    one_turn(&mut shell, &provider, &mut prompter);

    assert_eq!(prompter.asked.len(), 1);
    assert!(shell.editor().document().track(track).is_none(), "it ran");
    assert_eq!(shell.assistant_log().len(), 1);
    assert!(matches!(shell.assistant_log()[0].outcome, Outcome::Ran(_)));

    // A later call of the same command doesn't ask again: the prompter here
    // has no answer to give.
    let provider = Script::new(vec![
        calls(vec![call("c2", "track_delete", json!({ "track": track }))]),
        answer("Done."),
    ]);
    let mut empty = ScriptedPrompter::new(Vec::new());
    one_turn(&mut shell, &provider, &mut empty);
    assert!(
        empty.asked.is_empty(),
        "always-allowed commands don't ask again"
    );
}

#[test]
fn an_edit_call_runs_without_a_prompt_and_is_recorded() {
    let mut shell = Shell::headless();
    let track = a_track(&shell);
    let provider = Script::new(vec![
        calls(vec![call(
            "c1",
            "track_rename",
            json!({ "track": track, "name": "Assistant Track" }),
        )]),
        answer("Renamed the track."),
    ]);
    let mut prompter = ScriptedPrompter::new(Vec::new());

    one_turn(&mut shell, &provider, &mut prompter);

    assert!(prompter.asked.is_empty(), "an edit doesn't ask first");
    assert_eq!(
        shell
            .editor()
            .document()
            .track(track)
            .map(bl_model::Track::name),
        Some("Assistant Track")
    );
    assert_eq!(shell.assistant_log().len(), 1);
    assert!(matches!(shell.assistant_log()[0].outcome, Outcome::Ran(_)));
}

#[test]
fn a_new_id_placeholder_is_issued_and_a_later_call_refers_to_it() {
    let mut shell = Shell::headless();
    let before = shell.editor().document().tracks().count();
    let provider = Script::new(vec![
        calls(vec![
            call(
                "c1",
                "track_create",
                json!({ "id": { "$new": "lead" }, "kind": "midi", "name": "Lead", "index": 0 }),
            ),
            call(
                "c2",
                "track_rename",
                json!({ "track": { "$ref": "lead" }, "name": "Lead synth" }),
            ),
            call(
                "c3",
                "track_rename",
                json!({ "track": { "$ref": "nobody" }, "name": "x" }),
            ),
        ]),
        answer("Made a lead track."),
    ]);
    let mut prompter = ScriptedPrompter::new(Vec::new());

    let turn = one_turn(&mut shell, &provider, &mut prompter);

    assert!(prompter.asked.is_empty(), "creating a track doesn't ask");
    let document = shell.editor().document();
    assert_eq!(document.tracks().count(), before + 1);
    assert!(document.tracks().any(|track| track.name() == "Lead synth"));
    // A reference to a name nothing made is the call's result, not a crash.
    let refused = turn.steps[2].result.clone().unwrap_err();
    assert!(refused.contains("nobody"), "{refused}");
}

#[test]
fn a_command_only_a_person_runs_is_refused_and_recorded() {
    let mut shell = Shell::headless();
    let provider = Script::new(vec![
        calls(vec![call(
            "c1",
            "assistant_set_model",
            json!({ "model": "gpt-4o" }),
        )]),
        answer("I can't change that."),
    ]);
    let mut prompter = ScriptedPrompter::new(Vec::new());

    one_turn(&mut shell, &provider, &mut prompter);

    assert!(prompter.asked.is_empty(), "it's refused, not asked about");
    assert_eq!(shell.assistant_settings().model, "", "nothing changed");
    assert_eq!(shell.assistant_log().len(), 1);
    assert!(matches!(
        shell.assistant_log()[0].outcome,
        Outcome::Denied(_)
    ));
}

#[test]
fn the_settings_commands_are_the_person_s_own() {
    // The agent API refuses every assistant command, whatever the agent's
    // permissions (agent-permissions R9, by analogy).
    let shell = Shell::headless();
    assert!(bl_shell::assistant::handles("assistant.set_model"));
    assert!(bl_shell::assistant::handles("assistant.set_key"));
    assert!(!bl_shell::assistant::handles("track.create"));
    // A shell command the assistant may run isn't a settings one.
    assert!(matches!(
        shell
            .registry()
            .get("assistant.set_model")
            .map(|s| s.permission),
        Some(Some(bl_commands::Permission::Edit))
    ));
    assert!(matches!(
        shell
            .registry()
            .get("assistant.set_key")
            .map(|s| s.permission),
        Some(Some(bl_commands::Permission::External))
    ));
}
