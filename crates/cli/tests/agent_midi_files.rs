//! Importing and exporting Standard MIDI Files through the agent API
//! (WP-2.21): a scripted MCP client drives `bl-cli mcp`, imports a file the
//! test wrote, and exports the clip back out. Every note's pitch, start,
//! length, and velocity survives the round trip. Without `--autonomous`, the
//! import, which reads a file outside the project, is refused.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use bl_agent::client::{Client, Stdio, Transport};
use bl_core::time::{Bpm, Ticks, TimeSignature};
use bl_interop::midi;
use bl_model::Note;
use serde_json::{Value, json};

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-cli-agent-midi-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn bl_cli_mcp(args: &[&str]) -> Client<Stdio> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bl-cli"));
    command.arg("mcp").args(args);
    let mut client = Client::new(Stdio::spawn(&mut command).expect("bl-cli mcp starts"));
    client
        .initialize("scripted-client")
        .expect("it initialises");
    client
}

fn call(client: &mut Client<impl Transport>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .expect("the call reaches the server")
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

/// Spec: midi-clips R15, R16.
#[test]
fn an_agent_imports_and_exports_midi_files() {
    let dir = TempDir::new("round");
    let notes = vec![
        Note::new(36, Ticks::ZERO, Ticks::from_quarters(1), 100),
        Note::new(43, Ticks::from_quarters(2), Ticks::from_quarters(1), 72),
    ];
    let source = dir.0.join("Source.mid");
    midi::write(
        &source,
        "Bass",
        &notes,
        Bpm::new(100.0).unwrap(),
        TimeSignature::FOUR_FOUR,
    )
    .unwrap();

    let mut client = bl_cli_mcp(&["--autonomous"]);
    let imported = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "midi.import_file",
                               "args": { "path": source.to_string_lossy(), "import_tempo": true } }] }),
    );
    let result = &imported["results"][0]["result"];
    assert_eq!(result["created_tracks"].as_array().unwrap().len(), 1);
    let clip = result["clips"][0]["clip"].clone();
    // The import brought the file's tempo into the song.
    assert_eq!(result["clips"][0]["name"], "Bass");

    let out = dir.0.join("Out.mid");
    let exported = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "midi.export_clip",
                               "args": { "clip": clip, "path": out.to_string_lossy() } }] }),
    );
    let result = &exported["results"][0]["result"];
    assert_eq!(result["notes"], 2);
    assert!(client.into_transport().finish().unwrap().success());
    assert!(out.exists());

    // The file the agent wrote holds the same notes, at the same positions,
    // with the same velocities.
    let read = midi::read(&out).unwrap();
    assert_eq!(read.tracks.len(), 1);
    let mut back = read.tracks[0].notes.clone();
    back.sort_by_key(|note| (note.start, note.key));
    let mut expected = notes.clone();
    expected.sort_by_key(|note| (note.start, note.key));
    assert_eq!(back, expected);
}

/// Spec: midi-clips R16. A file outside the project is external.
#[test]
fn without_autonomous_a_midi_file_import_is_refused() {
    let dir = TempDir::new("refused");
    let source = dir.0.join("Source.mid");
    midi::write(
        &source,
        "Bass",
        &[Note::new(60, Ticks::ZERO, Ticks::from_quarters(1), 90)],
        Bpm::new(120.0).unwrap(),
        TimeSignature::FOUR_FOUR,
    )
    .unwrap();

    let mut client = bl_cli_mcp(&[]);
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "midi.import_file",
                                   "args": { "path": source.to_string_lossy() } }] }),
        )
        .unwrap();
    assert!(refused.is_error);
    assert!(
        refused.text.contains("`midi.import_file` is external"),
        "{}",
        refused.text
    );
}
