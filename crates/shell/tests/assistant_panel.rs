//! The built-in assistant's panel (WP-2.23, part 3; D24), headless: its
//! conversation fills as a scripted turn goes, Stop ends a turn between two
//! calls, the pane opens and closes from its command, its width stays within
//! limits and persists, and so does floating it into a window of its own.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bl_assistant::agent::Stopped;
use bl_assistant::message::{Event, Reply, Request, Stop};
use bl_assistant::provider::{Error, Provider};
use bl_assistant::tools::ToolCall;
use bl_shell::Shell;
use bl_shell::assistant::{
    Decision, PANEL_DEFAULT_WIDTH, PANEL_MAX_WIDTH, PANEL_MIN_WIDTH, PromptRequest, Prompter, Row,
    run_turn,
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
    let dir = std::env::temp_dir().join(format!("bl-shell-panel-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A provider that answers with a script; `on_stream` runs as each reply
/// starts, standing in for the person pressing Stop at that moment.
struct Script {
    replies: Mutex<VecDeque<Reply>>,
    on_stream: Box<dyn Fn(usize) + Send + Sync>,
    streamed: Mutex<usize>,
}

impl Script {
    fn new(replies: Vec<Reply>) -> Self {
        Self::with(replies, |_| {})
    }

    fn with(replies: Vec<Reply>, on_stream: impl Fn(usize) + Send + Sync + 'static) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            on_stream: Box::new(on_stream),
            streamed: Mutex::new(0),
        }
    }
}

impl Provider for Script {
    fn stream(&self, _request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error> {
        let n = {
            let mut streamed = self.streamed.lock().unwrap();
            *streamed += 1;
            *streamed
        };
        (self.on_stream)(n);
        let reply = self
            .replies
            .lock()
            .unwrap()
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

/// A prompter for turns that never need one.
struct Quiet;

impl Prompter for Quiet {
    fn ask(&mut self, request: &PromptRequest) -> Decision {
        panic!("no prompt was expected: {request:?}")
    }
}

fn say(text: &str, calls: Vec<ToolCall>) -> Reply {
    Reply {
        text: text.to_owned(),
        stop: Stop::End,
        tool_calls: calls,
    }
}

fn new_track(shell: &mut Shell, call: &str) -> ToolCall {
    let track: bl_model::TrackId = shell.new_id();
    ToolCall {
        id: call.to_owned(),
        name: "track_create".to_owned(),
        arguments: json!({ "id": track, "kind": "audio", "name": "New", "index": 0 }),
    }
}

fn track_count(shell: &Shell) -> usize {
    shell.editor().document().tracks().count()
}

#[test]
fn a_scripted_turns_messages_and_tool_calls_appear_in_order() {
    let mut shell = Shell::headless();
    let tracks = track_count(&shell);
    shell
        .run("assistant.send", json!({ "text": "Add a track" }))
        .unwrap();
    assert_eq!(
        shell.assistant_rows(),
        vec![Row::You("Add a track".to_owned())],
        "the message is in the conversation at once"
    );
    let text = shell.take_assistant_prompt().expect("the host takes it");
    assert_eq!(shell.take_assistant_prompt(), None, "once");

    let provider = Script::new(vec![
        say("Adding a track.", vec![new_track(&mut shell, "c1")]),
        say("Done.", vec![]),
    ]);
    run_turn(&mut shell, &provider, &mut Quiet, &text).unwrap();

    let rows = shell.assistant_rows();
    assert_eq!(rows.len(), 4, "{rows:?}");
    assert_eq!(rows[0], Row::You("Add a track".to_owned()));
    assert_eq!(rows[1], Row::Assistant("Adding a track.".to_owned()));
    match &rows[2] {
        Row::Call {
            name,
            command,
            result,
            ..
        } => {
            assert_eq!(name, "track_create");
            assert_eq!(command.as_deref(), Some("track.create"));
            assert!(matches!(result, Some(Ok(_))), "{result:?}");
        }
        other => panic!("a tool call row, not {other:?}"),
    }
    assert_eq!(rows[3], Row::Assistant("Done.".to_owned()));
    assert!(!shell.assistant_running());
    assert_eq!(track_count(&shell), tracks + 1);
}

#[test]
fn a_refused_call_shows_why_it_didnt_run() {
    let mut shell = Shell::headless();
    let provider = Script::new(vec![
        say(
            "",
            vec![ToolCall {
                id: "c1".to_owned(),
                name: "assistant_set_model".to_owned(),
                arguments: json!({ "model": "other" }),
            }],
        ),
        say("I can't change that.", vec![]),
    ]);
    run_turn(&mut shell, &provider, &mut Quiet, "Change your model").unwrap();
    let rows = shell.assistant_rows();
    assert!(
        matches!(&rows[1], Row::Call { result: Some(Err(why)), .. } if why.contains("person's own")),
        "{rows:?}"
    );
}

#[test]
fn stop_ends_the_turn_between_calls_and_keeps_what_ran() {
    let mut shell = Shell::headless();
    let tracks = track_count(&shell);
    let flag = shell.assistant_stop_flag();
    // The person presses Stop while the second reply streams in.
    let provider = Script::with(
        vec![
            say("One.", vec![new_track(&mut shell, "c1")]),
            say("Two.", vec![new_track(&mut shell, "c2")]),
        ],
        move |n| {
            if n == 2 {
                flag.store(true, Ordering::SeqCst);
            }
        },
    );
    let turn = run_turn(&mut shell, &provider, &mut Quiet, "Add tracks").unwrap();

    assert_eq!(turn.stopped, Some(Stopped::Cancelled));
    assert_eq!(track_count(&shell), tracks + 1, "the first call stays done");
    let rows = shell.assistant_rows();
    assert!(
        matches!(
            &rows[2],
            Row::Call {
                result: Some(Ok(_)),
                ..
            }
        ),
        "{rows:?}"
    );
    assert!(
        matches!(
            &rows[4],
            Row::Call {
                result: Some(Err(_)),
                ..
            }
        ),
        "the call that didn't run isn't left running: {rows:?}"
    );
    assert_eq!(rows.last(), Some(&Row::Notice("Stopped.".to_owned())));
    assert!(!shell.assistant_running());
}

#[test]
fn the_stop_command_sets_the_flag_only_while_a_turn_runs() {
    let mut shell = Shell::headless();
    let flag = shell.assistant_stop_flag();
    let state = shell.run("assistant.stop", json!({})).unwrap();
    assert_eq!(state["running"], false);
    assert!(!flag.load(Ordering::SeqCst), "nothing to stop");

    // While a turn runs (the provider sees the flag a command would set).
    let seen = Arc::new(AtomicBool::new(false));
    let probe = Arc::clone(&seen);
    let flag_again = shell.assistant_stop_flag();
    let provider = Script::with(vec![say("Hi.", vec![])], move |_| {
        probe.store(flag_again.load(Ordering::SeqCst), Ordering::SeqCst);
    });
    run_turn(&mut shell, &provider, &mut Quiet, "Hello").unwrap();
    assert!(!seen.load(Ordering::SeqCst), "a new turn starts unstopped");
}

#[test]
fn stop_before_the_turn_starts_withdraws_the_message() {
    let mut shell = Shell::headless();
    shell
        .run("assistant.send", json!({ "text": "Add a track" }))
        .unwrap();
    assert!(shell.assistant_running(), "at work from the message on");
    let state = shell.run("assistant.stop", json!({})).unwrap();
    assert_eq!(state["running"], false);
    assert_eq!(shell.take_assistant_prompt(), None, "nothing left to run");
    assert_eq!(
        shell.assistant_rows().last(),
        Some(&Row::Notice("Stopped.".to_owned()))
    );
}

#[test]
fn a_message_is_refused_when_empty_or_while_one_waits() {
    let mut shell = Shell::headless();
    assert!(
        shell
            .run("assistant.send", json!({ "text": "  " }))
            .is_err()
    );
    shell
        .run("assistant.send", json!({ "text": "First" }))
        .unwrap();
    assert!(
        shell
            .run("assistant.send", json!({ "text": "Second" }))
            .is_err(),
        "the first hasn't been taken"
    );
}

#[test]
fn the_pane_opens_and_closes_from_its_command() {
    let mut shell = Shell::headless();
    assert!(
        !shell.assistant_panel_state().open,
        "a new session starts closed"
    );
    assert_eq!(shell.assistant_docked_width(), None);

    let state = shell.run("view.toggle_assistant", json!({})).unwrap();
    assert_eq!(state["open"], true);
    assert_eq!(shell.assistant_docked_width(), Some(PANEL_DEFAULT_WIDTH));
    assert!(!shell.assistant_window_open());
    assert_eq!(shell.switch_state("view.toggle_assistant"), Some(true));

    let state = shell.run("view.toggle_assistant", json!({})).unwrap();
    assert_eq!(state["open"], false);
    assert_eq!(shell.assistant_docked_width(), None);
}

#[test]
fn the_width_stays_within_limits_and_persists() {
    let dir = temp_dir("width");
    let mut shell = shell_saving_to(&dir);
    shell.run("view.toggle_assistant", json!({})).unwrap();

    let state = shell
        .run("assistant.set_panel_width", json!({ "width": 400.0 }))
        .unwrap();
    assert_eq!(state["width"], 400.0);
    shell
        .run("assistant.set_panel_width", json!({ "width": 5000.0 }))
        .unwrap();
    assert_eq!(shell.assistant_panel_state().width, PANEL_MAX_WIDTH);
    shell
        .run("assistant.set_panel_width", json!({ "width": 10.0 }))
        .unwrap();
    assert_eq!(shell.assistant_panel_state().width, PANEL_MIN_WIDTH);
    shell
        .run("assistant.set_panel_width", json!({ "width": 410.0 }))
        .unwrap();

    let again = shell_saving_to(&dir);
    assert_eq!(again.assistant_panel_state().width, 410.0);
    assert!(
        !again.assistant_panel_state().open,
        "the pane starts closed"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn floating_and_docking_switch_and_persist() {
    let dir = temp_dir("float");
    let mut shell = shell_saving_to(&dir);
    shell.run("view.toggle_assistant", json!({})).unwrap();
    assert!(shell.assistant_docked_width().is_some());

    shell.run("assistant.toggle_floating", json!({})).unwrap();
    assert!(shell.assistant_panel_state().floating);
    assert!(
        shell.assistant_window_open(),
        "open and floating is a window"
    );
    assert_eq!(shell.assistant_docked_width(), None, "the pane makes room");

    // The choice is remembered across starts; the pane starts closed.
    let mut again = shell_saving_to(&dir);
    assert!(again.assistant_panel_state().floating);
    again.run("view.toggle_assistant", json!({})).unwrap();
    assert!(again.assistant_window_open());

    again.run("assistant.toggle_floating", json!({})).unwrap();
    assert!(!again.assistant_window_open());
    assert!(again.assistant_docked_width().is_some());
    assert!(!shell_saving_to(&dir).assistant_panel_state().floating);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_panels_commands_are_the_persons_own() {
    for id in [
        "view.toggle_assistant",
        "assistant.set_panel_width",
        "assistant.toggle_floating",
        "assistant.send",
        "assistant.stop",
    ] {
        assert!(
            bl_shell::assistant::handles(id),
            "{id}: the agent API refuses it"
        );
    }
}

#[test]
fn the_view_menu_has_an_assistant_item_that_runs_the_toggle() {
    use bl_shell::menus::{Action, MenuPlatform, menu_bar};
    for platform in [MenuPlatform::Mac, MenuPlatform::Other] {
        let bar = menu_bar(platform);
        let item = bar
            .iter()
            .flat_map(bl_shell::menus::Menu::walk)
            .find(|item| item.row == "D24")
            .expect("the View menu has the assistant's item");
        assert_eq!(item.label, "Assistant");
        assert!(matches!(
            item.action,
            Action::Command("view.toggle_assistant")
        ));
    }
    let mut shell = Shell::headless();
    assert!(shell.can_invoke("view.toggle_assistant", &json!({})));
    shell.invoke("view.toggle_assistant", json!({})).unwrap();
    assert!(shell.assistant_panel_state().open);
}
