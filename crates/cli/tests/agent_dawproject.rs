//! Importing a DAWproject file through the agent API (WP-6.36): a scripted
//! MCP client drives `bl-cli mcp`, opens a file as a set, and reads the set's
//! name and the import report back. Without `--autonomous`, the import, which
//! reads a file outside the project, is refused.

use std::path::PathBuf;
use std::process::Command;

use bl_agent::client::{Client, Stdio, Transport};
use serde_json::{Value, json};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/dawproject")
        .join(name)
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

/// Spec: dawproject R10, R12, R88. The import returns the set's name and
/// the report, `set.import_report` gives the report again, and the query
/// tool's `song` reads the name back.
#[test]
fn an_agent_imports_a_dawproject_file_and_reads_its_report() {
    let path = fixture("bitwig-shaped.dawproject");
    let mut client = bl_cli_mcp(&["--autonomous"]);
    let imported = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.import_dawproject",
                               "args": { "path": path.to_string_lossy() } }] }),
    );
    let result = &imported["results"][0]["result"];
    assert_eq!(result["name"], "bitwig-shaped");
    let entries = result["report"]["entries"].as_array().unwrap();
    assert!(
        entries.iter().any(|entry| {
            entry["what"] == "a CLAP plug-in" && entry["did"] == "kept as a missing plug-in"
        }),
        "{entries:?}"
    );

    let again = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.import_report", "args": {} }] }),
    );
    assert_eq!(&again["results"][0]["result"], &result["report"]);

    let song = call(&mut client, "query", json!({ "path": "song" }));
    assert_eq!(song["set"]["name"], "bitwig-shaped", "{song}");
    assert!(client.into_transport().finish().unwrap().success());
}

/// Spec: dawproject R10. A file outside the project is external.
#[test]
fn without_autonomous_a_dawproject_import_is_refused() {
    let path = fixture("minimal.dawproject");
    let mut client = bl_cli_mcp(&[]);
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "set.import_dawproject",
                                   "args": { "path": path.to_string_lossy() } }] }),
        )
        .unwrap();
    assert!(refused.is_error);
    assert!(
        refused.text.contains("`set.import_dawproject` is external"),
        "{}",
        refused.text
    );
}

/// Spec: dawproject R44 to R62, R88. After `set.import_dawproject` of a file
/// with devices and automation, the query tool reads them back.
#[test]
fn an_agent_reads_the_imported_devices_and_automation() {
    let path = fixture("cubase-shaped.dawproject");
    let mut client = bl_cli_mcp(&["--autonomous"]);
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.import_dawproject",
                               "args": { "path": path.to_string_lossy() } }] }),
    );
    let query = |client: &mut Client<Stdio>, path: &str| {
        call(client, "query", json!({ "path": path, "detail": "full" }))
    };
    let vocals = query(&mut client, "track \"Vocals\"");
    let devices: Vec<(&str, u64)> = vocals["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["name"].as_str().unwrap(),
                d["params_set"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        devices,
        [
            ("EQ", 10),
            ("Compressor", 6),
            ("Gate", 4),
            ("Limiter", 3),
            ("Missing Insert", 0)
        ]
    );
    // The plug-in that isn't installed keeps its ID and its state.
    assert_eq!(
        vocals["devices"][4]["kind"]["plugin"]["plugin_id"],
        "00000000-0000-0000-0000-0000000000AA"
    );
    let automation = query(&mut client, "track \"Vocals\" / automation");
    let driven: Vec<(String, u64)> = automation["automation"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["parameter"].as_str().unwrap().to_owned(),
                a["points"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        driven,
        [
            ("volume".to_owned(), 2),
            ("pan".to_owned(), 3),
            ("mute".to_owned(), 3),
            ("device 20 param 0".to_owned(), 4),
        ]
    );
    assert!(client.into_transport().finish().unwrap().success());
}
