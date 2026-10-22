//! Crash recovery through the agent API (WP-6.62; the crash-recovery spec,
//! R26): an agent lists the sets a crash left, recovers one by its ID, and
//! the song is the one the crashed session had made.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_model::{Command, TrackKind};
use bl_project::autosave::Autosave;
use bl_project::{Media, Project};
use bl_shell::Shell;
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-agent-recovery-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, relative: &str) -> std::path::PathBuf {
        self.0.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs `command` with `args` through the agent API and returns its result.
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

/// The song the agent reads through the query tool.
fn song(client: &mut Client<InProcess>) -> Value {
    client
        .call("query", json!({}))
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("the query failed: {error}"))
}

#[test]
fn save_continuously_leaves_a_set_with_a_file_clean() {
    // Spec: crash-recovery R25.
    let scratch = Scratch::new("continuous");
    let recovery = scratch.path("Recovery");
    let set = scratch.path("Song Project").join("Song.blset");
    let mut file = bl_project::SetFile::new(&set);
    file.save(&Project::new()).unwrap();
    let mut shell = Shell::headless();
    shell.use_recovery(recovery).unwrap();
    shell
        .run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("test-agent").unwrap();
    // An edit leaves the set with unsaved changes.
    run(
        &mut client,
        "track.create",
        json!({ "id": {"$new": "T"}, "kind": "midi", "name": "T", "index": 0 }),
    );
    assert_eq!(song(&mut client)["set"]["unsaved_changes"], true);
    // With Save Continuously on, it has none.
    run(
        &mut client,
        "settings.set_continuous_save",
        json!({ "on": true }),
    );
    assert_eq!(song(&mut client)["set"]["unsaved_changes"], false);
}

#[test]
fn an_agent_recovers_a_set_a_crash_left() {
    // Spec: crash-recovery R2, R3, R26.
    let scratch = Scratch::new("recover");
    let recovery = scratch.path("Recovery");
    let set = scratch.path("Song Project").join("Song.blset");
    let mut file = bl_project::SetFile::new(&set);
    file.save(&Project::new()).unwrap();
    // A session that made an edit and stopped without closing its set.
    {
        let (file, _) = bl_project::SetFile::open(&set).unwrap();
        let mut autosave = Autosave::start(&recovery).unwrap();
        autosave.begin(Some(&file)).unwrap();
        let mut editor = Project::new().editor();
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind: TrackKind::Midi,
                name: "Recovered Track".to_owned(),
                parent: None,
                index: 0,
            })
            .unwrap();
        autosave.edited(&Project::from_editor(&editor, Media::new()), "Create Track");
        autosave.flush().unwrap();
        drop(autosave);
    }
    let mut shell = Shell::headless();
    shell.use_recovery(recovery).unwrap();
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("test-agent").unwrap();

    // The set is listed.
    let listed = run(&mut client, "set.list_recoverable", json!({}));
    let sets = listed["sets"].as_array().expect("the sets are listed");
    assert_eq!(sets.len(), 1, "{listed}");
    let id = sets[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(sets[0]["name"], "Song");
    assert!(sets[0]["edits"].as_u64().unwrap() >= 1);

    // Recovering it opens the song as it was after its last edit.
    let recovered = run(&mut client, "set.recover", json!({ "id": id }));
    assert_eq!(recovered["name"], "Song");
    let song = song(&mut client);
    let names: Vec<&str> = song["tracks"]["items"]
        .as_array()
        .expect("the tracks are listed")
        .iter()
        .filter_map(|track| track["name"].as_str())
        .collect();
    assert!(
        names.contains(&"Recovered Track"),
        "the song is the one that was journalled: {names:?}"
    );
}
