//! The agent server's settings (WP-1.15): switched on and off, agents
//! allowed and forgotten, autonomous mode, and the token, each through its
//! registered command and kept with the preferences. And the editing hook
//! the agent API uses for batches, dry runs, and checkpoints.

use bl_model::{TrackId, registry as model};
use bl_shell::agent::{AgentSettings, DEFAULT_PORT};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError};
use serde_json::json;

fn shell_saving_to(dir: &std::path::Path) -> Shell {
    let prefs = Preferences::load(dir).unwrap();
    Shell::new(
        prefs.clone(),
        Some(dir.to_owned()),
        AudioHost::null(prefs.audio),
        Box::new(ProjectFiles::new()),
    )
}

#[test]
fn the_server_starts_off_and_switching_it_on_makes_a_token_that_is_kept() {
    let dir = std::env::temp_dir().join(format!("bl-shell-agent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut shell = shell_saving_to(&dir);
    assert_eq!(shell.agent_settings(), &AgentSettings::default());
    assert!(!shell.agent_settings().server);

    let state = shell
        .run("agent.set_server", json!({ "on": true }))
        .unwrap();
    assert_eq!(state["server"], true);
    assert_eq!(state["url"], format!("http://127.0.0.1:{DEFAULT_PORT}/mcp"));
    assert!(
        state.get("token").is_none(),
        "results never carry the token"
    );
    let token = shell.agent_settings().token.clone();
    assert_eq!(token.len(), 64);

    // Off and on again keeps the token; a new port moves the address.
    shell
        .run("agent.set_server", json!({ "on": false }))
        .unwrap();
    let state = shell
        .run("agent.set_server", json!({ "on": true, "port": 40_000 }))
        .unwrap();
    assert_eq!(state["url"], "http://127.0.0.1:40000/mcp");
    assert_eq!(shell.agent_settings().token, token);
    assert!(matches!(
        shell.run("agent.set_server", json!({ "port": 80 })),
        Err(ShellError::InvalidArgs { .. })
    ));

    // A new token replaces the old one.
    shell.run("agent.new_token", json!({})).unwrap();
    let new = shell.agent_settings().token.clone();
    assert_ne!(new, token);

    // The next start reads them back.
    let again = shell_saving_to(&dir);
    assert!(again.agent_settings().server);
    assert_eq!(again.agent_settings().port, 40_000);
    assert_eq!(again.agent_settings().token, new);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn agents_are_allowed_once_made_autonomous_and_forgotten() {
    let mut shell = Shell::headless();
    assert!(matches!(
        shell.run(
            "agent.set_autonomous",
            json!({ "client": "claude-code", "on": true })
        ),
        Err(ShellError::Unavailable { .. })
    ));
    shell
        .run("agent.approve_client", json!({ "client": "claude-code" }))
        .unwrap();
    // Allowing it again changes nothing.
    let state = shell
        .run("agent.approve_client", json!({ "client": "claude-code" }))
        .unwrap();
    assert_eq!(
        state["clients"],
        json!([{ "name": "claude-code", "autonomous": false }])
    );
    assert!(!shell.agent_settings().is_autonomous("claude-code"));
    shell
        .run(
            "agent.set_autonomous",
            json!({ "client": "claude-code", "on": true }),
        )
        .unwrap();
    assert!(shell.agent_settings().is_autonomous("claude-code"));
    assert!(!shell.agent_settings().is_autonomous("someone-else"));
    shell
        .run("agent.forget_client", json!({ "client": "claude-code" }))
        .unwrap();
    assert!(shell.agent_settings().client("claude-code").is_none());
    assert!(matches!(
        shell.run("agent.forget_client", json!({ "client": "claude-code" })),
        Err(ShellError::Unavailable { .. })
    ));
    assert!(matches!(
        shell.run("agent.approve_client", json!({ "client": " " })),
        Err(ShellError::InvalidArgs { .. })
    ));
}

#[test]
fn an_edit_marks_the_set_changed_only_if_the_document_changed() {
    let mut shell = Shell::headless();
    assert!(!shell.is_dirty());

    // A new ID, and a batch tried and cancelled, change nothing.
    let id: TrackId = shell.new_id();
    shell.edit(|editor| {
        editor.begin("Dry run");
        model::run(
            editor,
            "track.create",
            json!({ "id": id, "kind": "audio", "name": "Tried", "index": 0 }),
        )
        .unwrap();
        editor.cancel().unwrap();
    });
    assert!(!shell.is_dirty());
    assert!(!shell.editor().can_undo());

    // A batch committed is one undo step, and the set has changed.
    let revision = shell.revision();
    shell
        .edit(|editor| {
            editor.transact("Agent: two tracks", |editor| {
                for (name, index) in [("A", 0), ("B", 1)] {
                    let id: TrackId = editor.new_id();
                    model::run(
                        editor,
                        "track.create",
                        json!({ "id": id, "kind": "midi", "name": name, "index": index }),
                    )?;
                }
                Ok::<_, model::RunError>(())
            })
        })
        .unwrap();
    assert!(shell.is_dirty());
    assert_ne!(shell.revision(), revision);
    assert_eq!(
        shell.editor().undo_history().collect::<Vec<_>>(),
        ["Agent: two tracks"]
    );
}
