//! The agent server's settings (WP-1.15): switched on and off, agents
//! allowed and forgotten, autonomous mode, and the token, each through its
//! registered command and kept with the preferences, the tokens in the
//! keychain and never in the preferences' file. And the editing hook
//! the agent API uses for batches, dry runs, and checkpoints.

use std::sync::Arc;

use bl_assistant::keychain::{Keychain, KeychainError, MemoryKeychain};
use bl_model::{TrackId, registry as model};
use bl_shell::agent::{AgentSettings, DEFAULT_PORT, KEYCHAIN_ACCOUNT};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError};
use serde_json::json;

/// A shell keeping its preferences in `dir` and its agent tokens in
/// `keys`, as the app keeps them in the OS keychain.
fn shell_saving_to(dir: &std::path::Path, keys: &Arc<MemoryKeychain>) -> Shell {
    let prefs = Preferences::load(dir).unwrap();
    let mut shell = Shell::new(
        prefs.clone(),
        Some(dir.to_owned()),
        AudioHost::null(prefs.audio),
        Box::new(ProjectFiles::new()),
    );
    shell.use_agent_keychain(Arc::clone(keys) as Arc<dyn Keychain>);
    shell
}

/// The preferences' file as written.
fn file(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join(bl_shell::settings::PREFERENCES_FILE)).unwrap()
}

/// The tokens the keychain holds, as its entry's text.
fn stored(keys: &MemoryKeychain) -> String {
    keys.read(KEYCHAIN_ACCOUNT).unwrap().unwrap_or_default()
}

#[test]
fn the_server_starts_off_and_switching_it_on_makes_a_token_that_is_kept() {
    let dir = std::env::temp_dir().join(format!("bl-shell-agent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let keys = Arc::new(MemoryKeychain::new());
    let mut shell = shell_saving_to(&dir, &keys);
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
    assert_eq!(token.expose().len(), 64);

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

    // The keychain has it; the preferences' file doesn't.
    assert!(stored(&keys).contains(new.expose()));
    assert!(!file(&dir).contains(new.expose()));
    assert!(!file(&dir).contains("token"), "{}", file(&dir));

    // The next start reads them back.
    let again = shell_saving_to(&dir, &keys);
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

#[test]
fn each_allowed_agent_gets_a_token_of_its_own_that_can_be_replaced() {
    // Spec: agent-permissions R7, R9.
    let dir = std::env::temp_dir().join(format!("bl-shell-agent-tokens-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let keys = Arc::new(MemoryKeychain::new());
    let mut shell = shell_saving_to(&dir, &keys);
    shell
        .run("agent.set_server", json!({ "on": true }))
        .unwrap();
    for name in ["claude-code", "goose"] {
        let state = shell
            .run("agent.approve_client", json!({ "client": name }))
            .unwrap();
        assert!(
            !state.to_string().contains("token"),
            "results never carry a token: {state}"
        );
    }
    let settings = shell.agent_settings().clone();
    let tokens: Vec<String> = settings
        .clients
        .iter()
        .map(|c| c.token.expose().to_owned())
        .collect();
    assert!(tokens.iter().all(|t| t.len() == 64));
    assert_ne!(tokens[0], tokens[1]);
    assert!(tokens.iter().all(|t| *t != settings.token));
    assert!(
        !format!("{settings:?}").contains(&tokens[0]),
        "debug prints never show a token"
    );

    // Replacing one changes only that agent's, and the result carries
    // neither the old token nor the new one (WP-1.15c: the Settings
    // window's New Token runs this).
    let state = shell
        .run("agent.new_client_token", json!({ "client": "goose" }))
        .unwrap();
    assert_eq!(shell.agent_settings().clients[0].token, tokens[0]);
    assert_ne!(shell.agent_settings().clients[1].token, tokens[1]);
    let new = shell.agent_settings().clients[1].token.expose().to_owned();
    let text = state.to_string();
    assert!(
        !text.contains("token") && !text.contains(&tokens[1]) && !text.contains(&new),
        "{text}"
    );
    assert!(!format!("{:?}", shell.agent_settings()).contains(&new));
    assert!(matches!(
        shell.run("agent.new_client_token", json!({ "client": "nobody" })),
        Err(ShellError::Unavailable { .. })
    ));
    // Allowing an agent again keeps its token.
    shell
        .run("agent.approve_client", json!({ "client": "claude-code" }))
        .unwrap();
    assert_eq!(shell.agent_settings().clients[0].token, tokens[0]);

    // The next start reads them back, from the keychain alone.
    for token in [&tokens[0], &new] {
        assert!(stored(&keys).contains(token.as_str()));
        assert!(!file(&dir).contains(token.as_str()));
    }
    let again = shell_saving_to(&dir, &keys);
    assert_eq!(again.agent_settings(), shell.agent_settings());

    // Forgetting an agent forgets its token.
    let mut shell = again;
    shell
        .run("agent.forget_client", json!({ "client": "claude-code" }))
        .unwrap();
    assert!(!stored(&keys).contains(tokens[0].as_str()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn agents_allowed_before_agents_had_tokens_get_one_and_keep_it() {
    let dir = std::env::temp_dir().join(format!("bl-shell-agent-old-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(bl_shell::settings::PREFERENCES_FILE),
        r#"{ "agent": { "server": true, "token": "x", "clients": [
              { "name": "claude-code", "autonomous": true } ] } }"#,
    )
    .unwrap();
    let keys = Arc::new(MemoryKeychain::new());
    let shell = shell_saving_to(&dir, &keys);
    let token = shell.agent_settings().clients[0].token.clone();
    assert_eq!(token.expose().len(), 64);
    assert!(shell.agent_settings().is_autonomous("claude-code"));
    let again = shell_saving_to(&dir, &keys);
    assert_eq!(again.agent_settings().clients[0].token, token);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_labelled_run_is_one_undo_step_with_that_label() {
    // Spec: agent-permissions R6. The Arrangement's edits and the browser's
    // drops, which the shell runs, can be named for the agent that ran them.
    let mut shell = Shell::headless();
    let track = shell.editor().document().top_level().next().unwrap();
    let id: bl_model::ClipId = shell.new_id();
    shell
        .run_labelled(
            bl_model::Maker::Agent("editor".to_owned()),
            "editor: Create Clip",
            "clip.create",
            json!({ "track": track, "clip": { "id": id, "name": "A", "start": 0,
                    "length": 705_600, "mute": false, "data": { "midi": { "notes": [] } } } }),
        )
        .unwrap();
    shell
        .run_labelled(
            bl_model::Maker::Agent("editor".to_owned()),
            "editor: Move Clips",
            "arrangement.move_clips",
            json!({ "clips": [id], "by": 705_600 }),
        )
        .unwrap();
    // Selecting changes nothing in the set and records nothing.
    shell
        .run_labelled(
            bl_model::Maker::Agent("editor".to_owned()),
            "editor: Select",
            "arrangement.select_clips",
            json!({ "clips": [id] }),
        )
        .unwrap();
    // A failure records nothing and leaves the set as it was.
    assert!(
        shell
            .run_labelled(
                bl_model::Maker::Agent("editor".to_owned()),
                "editor: Move Clips",
                "arrangement.move_clips",
                json!({ "clips": [id], "by": -2_822_400 }),
            )
            .is_err()
    );
    assert_eq!(
        shell.editor().undo_history().collect::<Vec<_>>(),
        ["editor: Move Clips", "editor: Create Clip"]
    );
    assert!(shell.is_dirty());
}

#[test]
fn loading_the_browser_s_files_is_destructive_where_a_clip_is_in_the_way() {
    // Spec: agent-permissions R3. Enter loads onto the selected audio track
    // at the insert marker.
    use bl_commands::Permission;
    let mut shell = Shell::headless();
    let audio = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Audio)
        .unwrap()
        .id();
    let id: bl_model::ClipId = shell.new_id();
    shell
        .run(
            "clip.create",
            json!({ "track": audio, "clip": { "id": id, "name": "Held", "start": 2_822_400,
                    "length": 2_822_400, "mute": false,
                    "data": { "audio": { "media": "11".repeat(32) } } } }),
        )
        .unwrap();
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    let at = |shell: &mut Shell, ticks: i64| {
        shell
            .run("arrangement.set_insert_marker", json!({ "at": ticks }))
            .unwrap();
        shell.call_permission("browser.load_selected", &json!({}))
    };
    let over = at(&mut shell, 0);
    assert_eq!(
        over.classes(),
        [Permission::Destructive, Permission::External]
    );
    assert_eq!(over.replaces, [id]);
    assert!(over.may);
    let after = at(&mut shell, 3 * 2_822_400);
    assert_eq!(after.classes(), [Permission::External]);
}

#[test]
fn clip_set_data_is_destructive_only_when_it_leaves_a_midi_clip_fewer_notes() {
    // Spec: agent-permissions R11. Decided against the set as it is, and
    // nothing changes.
    use bl_commands::Permission;
    let mut shell = Shell::headless();
    let kind = |shell: &Shell, kind: bl_model::TrackKind| {
        shell
            .editor()
            .document()
            .tracks()
            .find(|t| t.kind() == kind)
            .unwrap()
            .id()
    };
    let midi = kind(&shell, bl_model::TrackKind::Midi);
    let audio = kind(&shell, bl_model::TrackKind::Audio);
    let notes = |count: u64| {
        let notes: Vec<_> = (0..count)
            .map(|n| {
                json!({ "key": 60 + n, "velocity": 100, "start": n * 705_600,
                             "length": 705_600 })
            })
            .collect();
        json!({ "midi": { "notes": notes } })
    };
    let bass: bl_model::ClipId = shell.new_id();
    shell
        .run(
            "clip.create",
            json!({ "track": midi, "clip": { "id": bass, "name": "Bass", "start": 0,
                    "length": 2_822_400, "mute": false, "data": notes(3) } }),
        )
        .unwrap();
    let before = shell.editor().document().clone();
    let check = |shell: &mut Shell, data: serde_json::Value| {
        shell.call_permission("clip.set_data", &json!({ "clip": bass, "data": data }))
    };
    let fewer = check(&mut shell, notes(1));
    assert_eq!(fewer.classes(), [Permission::Edit, Permission::Destructive]);
    let thinned = fewer.fewer_notes.unwrap();
    assert_eq!((thinned.clip, thinned.has, thinned.leaves), (bass, 3, 1));
    assert!(fewer.replaces.is_empty());
    for (data, why) in [
        (notes(3), "the same count"),
        (notes(5), "more notes"),
        (
            json!({ "audio": { "media": "11".repeat(32) } }),
            "audio on a MIDI track fails",
        ),
    ] {
        let call = check(&mut shell, data);
        assert_eq!(call.classes(), [Permission::Edit], "{why}");
        assert!(call.fewer_notes.is_none(), "{why}");
    }
    assert_eq!(*shell.editor().document(), before, "nothing changed");
    assert_eq!(
        shell.editor().undo_history().count(),
        1,
        "only the clip's creation is an undo step"
    );

    // An audio clip's file replaced keeps its class.
    let held: bl_model::ClipId = shell.new_id();
    shell
        .run(
            "clip.create",
            json!({ "track": audio, "clip": { "id": held, "name": "Held", "start": 0,
                    "length": 2_822_400, "mute": false,
                    "data": { "audio": { "media": "11".repeat(32) } } } }),
        )
        .unwrap();
    let call = shell.call_permission(
        "clip.set_data",
        &json!({ "clip": held, "data": { "audio": { "media": "22".repeat(32) } } }),
    );
    assert_eq!(call.classes(), [Permission::Edit]);
    // An unknown clip too.
    let call = shell.call_permission(
        "clip.set_data",
        &json!({ "clip": 999_999, "data": notes(0) }),
    );
    assert_eq!(call.classes(), [Permission::Edit]);
}

#[test]
fn tokens_in_an_older_preferences_file_move_to_the_keychain() {
    let dir = std::env::temp_dir().join(format!("bl-shell-agent-move-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let server = "a".repeat(64);
    let client = "b".repeat(64);
    std::fs::write(
        dir.join(bl_shell::settings::PREFERENCES_FILE),
        format!(
            r#"{{ "agent": {{ "server": true, "port": 40001, "token": "{server}", "clients": [
              {{ "name": "claude-code", "autonomous": true, "token": "{client}" }} ] }} }}"#
        ),
    )
    .unwrap();
    let keys = Arc::new(MemoryKeychain::new());
    let shell = shell_saving_to(&dir, &keys);
    // Nothing changes for the person: the same tokens, the same agents.
    assert_eq!(shell.agent_settings().token, server);
    assert_eq!(shell.agent_settings().clients[0].token, client);
    assert!(shell.agent_settings().is_autonomous("claude-code"));
    // They're in the keychain now, and gone from the file, which keeps the
    // rest of its settings.
    assert!(stored(&keys).contains(&server) && stored(&keys).contains(&client));
    let text = file(&dir);
    assert!(!text.contains(&server) && !text.contains(&client), "{text}");
    assert!(
        text.contains("40001") && text.contains("claude-code"),
        "{text}"
    );

    // The next start reads them from the keychain.
    let again = shell_saving_to(&dir, &keys);
    assert_eq!(again.agent_settings(), shell.agent_settings());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A keychain that can't be reached, as on a Linux desktop without a
/// running Secret Service.
struct NoKeychain;

impl Keychain for NoKeychain {
    fn read(&self, _: &str) -> Result<Option<String>, KeychainError> {
        Err(KeychainError::Unavailable("no D-Bus session".to_owned()))
    }

    fn write(&self, _: &str, _: &str) -> Result<(), KeychainError> {
        Err(KeychainError::Unavailable("no D-Bus session".to_owned()))
    }

    fn delete(&self, _: &str) -> Result<(), KeychainError> {
        Err(KeychainError::Unavailable("no D-Bus session".to_owned()))
    }
}

#[test]
fn without_a_keychain_the_tokens_last_the_session_and_never_reach_the_file() {
    let dir = std::env::temp_dir().join(format!("bl-shell-agent-nokeys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut shell = Shell::new(
        Preferences::default(),
        Some(dir.clone()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    shell.use_agent_keychain(Arc::new(NoKeychain));
    assert!(
        shell
            .message()
            .is_some_and(|m| m.text.contains("no D-Bus session"))
    );
    shell
        .run("agent.set_server", json!({ "on": true }))
        .unwrap();
    shell
        .run("agent.approve_client", json!({ "client": "goose" }))
        .unwrap();
    let token = shell.agent_settings().token.clone();
    assert_eq!(
        token.expose().len(),
        64,
        "the server still runs this session"
    );
    assert!(
        shell
            .message()
            .is_some_and(|m| m.text.contains("until Beatlayer quits"))
    );
    let text = file(&dir);
    assert!(
        !text.contains(token.expose()) && !text.contains("token"),
        "{text}"
    );
    assert!(text.contains("goose"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}
