//! The TOON trial's benchmark: does a model read agent API results as
//! well in TOON as in compact JSON, and how many tokens does each cost?
//!
//! `tests/fixtures/toon-bench/` holds the results of the agent API's tools on
//! the 16-bar set the M2 demo's scripted transcript builds, one JSON file
//! each, and `questions.json`, about 40 questions about them with exact
//! answers. `questions.rs` computes every answer from the fixtures.
//!
//! In CI:
//!
//! - `the_answer_keys_match_the_fixtures` recomputes each answer from its
//!   fixture and compares it with `questions.json`, so changing a fixture
//!   without its keys fails.
//! - `the_fixtures_have_the_shape_the_tools_return_now` rebuilds the set,
//!   calls the tools and compares each result's shape (its keys and types,
//!   not its values) with the fixture, so the benchmark keeps measuring what
//!   the agent API sends. When it fails, run `write_the_fixtures_and_keys`.
//!
//! By hand (`--ignored`): `write_the_fixtures_and_keys` remakes the fixtures
//! and the keys, and `run_the_benchmark` asks a model every question with
//! the data in each format; `runner.rs` says how to point it at a server.

bl_rt::install_trap!();

mod questions;
mod runner;

use std::fs;
use std::path::{Path, PathBuf};

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_cli::assistant::{Replay, Transcript, run_headless};
use bl_shell::Shell;
use serde_json::{Value, json};

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/toon-bench")
}

/// One call of a tool whose result is a fixture.
pub struct Call {
    pub name: &'static str,
    pub tool: &'static str,
    args: fn() -> Value,
}

/// The tools' calls the benchmark reads.
pub const CALLS: [Call; 8] = [
    Call {
        name: "overview",
        tool: "query",
        args: || json!({}),
    },
    Call {
        name: "tracks",
        tool: "query",
        args: || json!({ "path": "tracks" }),
    },
    Call {
        name: "notes_drums",
        tool: "query",
        args: || notes("Drums"),
    },
    Call {
        name: "notes_chords",
        tool: "query",
        args: || notes("Chords"),
    },
    Call {
        name: "notes_lead",
        tool: "query",
        args: || notes("Lead"),
    },
    Call {
        name: "commands_clip",
        tool: "list_commands",
        args: || json!({ "area": "clip" }),
    },
    Call {
        name: "describe_clip_create",
        tool: "describe_command",
        args: || json!({ "command": "clip.create" }),
    },
    Call {
        name: "analysis",
        tool: "analyze",
        args: || json!({}),
    },
];

/// A track's only clip, with its notes.
fn notes(track: &str) -> Value {
    json!({ "path": format!("track \"{track}\" / clip #1"), "detail": "full", "limit": 500 })
}

/// What the agent API returns for each call, on the set the M2 demo's
/// scripted transcript builds from the factory default set.
fn current_results() -> Vec<(&'static str, Value)> {
    let transcript = Transcript::read(&fixtures_dir().join("../assistant/scripted.json"))
        .expect("the scripted transcript");
    let prompt = fs::read_to_string(fixtures_dir().join("../assistant/prompt.txt"))
        .expect("the kept prompt");
    let mut shell = Shell::headless();
    run_headless(
        &mut shell,
        &Replay::new(transcript.clone()),
        &transcript.model,
        false,
        prompt.trim(),
    )
    .expect("the scripted turn replays");
    let mut client = Client::new(InProcess::new(Agent::headless(false), shell));
    client.initialize("toon-bench").expect("it initialises");
    // A rough mix, so the tracks differ: the Drums, Bass, Chords and Lead
    // tracks are 16 to 19, and "4 Audio" is 5.
    let mix = [
        ("mixer.set_volume", json!({ "track": 16, "volume": 0.8 })),
        ("mixer.set_volume", json!({ "track": 17, "volume": 0.7 })),
        ("mixer.set_pan", json!({ "track": 17, "pan": -0.2 })),
        ("mixer.set_volume", json!({ "track": 18, "volume": 0.6 })),
        ("mixer.set_pan", json!({ "track": 18, "pan": 0.35 })),
        ("mixer.set_volume", json!({ "track": 19, "volume": 0.55 })),
        ("mixer.set_pan", json!({ "track": 19, "pan": 0.1 })),
        ("mixer.set_mute", json!({ "track": 5, "mute": true })),
    ];
    let commands: Vec<Value> = mix
        .into_iter()
        .map(|(command, args)| json!({ "command": command, "args": args }))
        .collect();
    client
        .call("run", json!({ "label": "Mix", "commands": commands }))
        .and_then(bl_agent::client::ToolResult::ok)
        .expect("the mix runs");
    CALLS
        .iter()
        .map(|call| {
            let result = client
                .call(call.tool, (call.args)())
                .and_then(bl_agent::client::ToolResult::ok)
                .unwrap_or_else(|error| panic!("{}: {error}", call.name));
            (call.name, result)
        })
        .collect()
}

/// The fixture `name`.
pub fn fixture(name: &str) -> Value {
    let path = fixtures_dir().join(format!("{name}.json"));
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("couldn't read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn write_json(path: &Path, value: &Value) {
    let mut text = serde_json::to_string(value).expect("JSON");
    text.push('\n');
    fs::write(path, text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}

/// A value's keys and types, with a list as the shape of its first item: what
/// has to stay the same for a fixture to be what the tool returns.
fn shape(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), shape(v))).collect())
        }
        Value::Array(items) => json!([items.first().map_or(Value::Null, shape)]),
        Value::Number(_) => json!("number"),
        Value::String(_) => json!("string"),
        Value::Bool(_) => json!("bool"),
        Value::Null => json!("null"),
    }
}

#[test]
fn the_fixtures_have_the_shape_the_tools_return_now() {
    for (name, now) in current_results() {
        assert_eq!(
            shape(&fixture(name)),
            shape(&now),
            "{name} has changed shape: run `cargo test -p bl-cli --test toon_bench -- --ignored write_the_fixtures_and_keys`"
        );
    }
}

#[test]
#[ignore = "remakes the fixtures and the answer keys"]
fn write_the_fixtures_and_keys() {
    for (name, result) in current_results() {
        write_json(&fixtures_dir().join(format!("{name}.json")), &result);
    }
    questions::write_keys();
}

#[test]
fn the_answer_keys_match_the_fixtures() {
    questions::check_keys();
}
