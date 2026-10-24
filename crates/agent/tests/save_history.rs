//! The save history through the agent API (WP-6.39; the crash-recovery spec,
//! R20): an agent lists the history, `set.restore` asks the person first as a
//! destructive command, and the restore reads back through the query tool's
//! `song` as the undo step it is.

use bl_agent::client::{Client, InProcess};
use bl_agent::{Agent, QuestionKind};
use bl_project::{Project, SetFile};
use bl_shell::Shell;
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{command} failed: {error}"))["results"][0]["result"]
        .clone()
}

fn track_names(client: &mut Client<InProcess>) -> Vec<String> {
    let song = client.call("query", json!({})).unwrap().ok().unwrap();
    song["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|track| track["name"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn set_restore_asks_first_and_reads_back_as_an_undo_step() {
    // Spec: crash-recovery R20.
    let scratch = Scratch(std::env::temp_dir().join(format!(
        "bl-agent-save-history-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )));
    let set = scratch.0.join("Song Project/Song.blset");
    SetFile::new(&set).save(&Project::new()).unwrap();
    let mut shell = Shell::headless();
    shell.use_recovery(scratch.0.join("Recovery")).unwrap();
    shell
        .run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();
    shell
        .run("settings.set_continuous_save", json!({ "on": true }))
        .unwrap();
    // The person allows the agent to connect, and declines anything else.
    let mut client = Client::new(InProcess::new(Agent::for_app(), shell));
    client.transport().answer = Box::new(|q| matches!(q.kind, QuestionKind::Connect { .. }));
    client.initialize("editor").unwrap();

    for name in ["One", "Two"] {
        run(
            &mut client,
            "track.create",
            json!({ "id": {"$new": name}, "kind": "midi", "name": name, "index": 0 }),
        );
        client.transport().shell.flush_recovery().unwrap();
    }
    let history = run(&mut client, "set.save_history", json!({}));
    assert_eq!(history["continuous"], true);
    let kinds: Vec<&str> = history["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["save_point", "save_point", "saved"], "{history}");
    // The save point after the first edit.
    let first = history["rows"][1]["id"].as_str().unwrap().to_owned();
    let args = json!({ "id": first });

    // It is destructive, so a declined restore changes nothing.
    let dry = client
        .call(
            "run",
            json!({ "commands": [{ "command": "set.restore", "args": args }], "dry_run": true }),
        )
        .unwrap()
        .ok()
        .unwrap();
    assert_eq!(dry["would"], "ask the person first", "{dry}");
    assert_eq!(dry["permission"], "destructive", "{dry}");
    let before = track_names(&mut client);
    let declined = client
        .call(
            "run",
            json!({ "commands": [{ "command": "set.restore", "args": args }] }),
        )
        .unwrap();
    assert!(
        declined.is_error && declined.text.contains("declined"),
        "{declined:?}"
    );
    assert_eq!(track_names(&mut client), before);

    // Allowed, it restores the song as one undo step the query tool names.
    client.transport().answer = Box::new(|_| true);
    run(&mut client, "set.restore", args);
    assert_eq!(track_names(&mut client).len(), before.len() - 1);
    let song = client.call("query", json!({})).unwrap().ok().unwrap();
    assert_eq!(song["undo"]["next_undo"], "Restore Save Point", "{song}");
}
