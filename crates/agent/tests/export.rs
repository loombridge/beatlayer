//! Exporting through the agent API in the app (WP-6.71; the export spec,
//! R42, R44): `export.audio` runs on the shell's export thread and its call
//! waits for it, so `export.status` reads the share meanwhile, a second
//! export is refused naming the first, `export.cancel` stops it and the
//! waiting call gets the cancel, and an export that ends gets its result.
//! The host stands in with a loop of `Shell::poll` and
//! `Agent::sync_export`.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bl_agent::{Agent, Outcome};
use bl_core::time::Ticks;
use bl_shell::Shell;
use serde_json::{Value, json};

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-agent-export-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A one-second mono sine as a 16-bit WAV file.
fn write_stem(path: &std::path::Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..48_000u32 {
        let sample = (std::f64::consts::TAU * 220.0 * f64::from(n) / 48_000.0).sin() * 0.2;
        writer
            .write_sample((sample * f64::from(i16::MAX)) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
}

fn message(id: u64, tool: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments } })
}

fn run(id: u64, command: &str, args: Value) -> Value {
    message(
        id,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    )
}

/// The command's result in a reply to `run`, or its error text.
fn result_of(reply: &Value) -> Result<Value, String> {
    let result = &reply["result"];
    if result["isError"] == true {
        return Err(result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned());
    }
    Ok(result["structuredContent"]["results"][0]["result"].clone())
}

fn reply(outcome: Outcome) -> Value {
    let Outcome::Reply(Some(reply)) = outcome else {
        panic!("{outcome:?}");
    };
    reply
}

/// Spec: export R42, R44. The waiting call, the share, the refusal of a
/// second export, and Cancel.
#[test]
fn in_the_app_an_export_call_waits_while_status_reads_the_share_and_cancel_stops_it() {
    let dir = TempDir::new("waits");
    let stem = dir.0.join("Stem.wav");
    write_stem(&stem);
    let mut shell = Shell::headless();
    let mut agent = Agent::headless(true);
    agent.set_view_capture(true);
    let session = agent.open_session(None);
    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                       "params": { "protocolVersion": "2025-11-25", "capabilities": {},
                                   "clientInfo": { "name": "exporter" } } });
    reply(agent.handle(&mut shell, &session, init));
    reply(agent.handle(
        &mut shell,
        &session,
        run(
            2,
            "browser.add_to_arrangement",
            json!({ "paths": [stem.to_string_lossy()], "at": 0 }),
        ),
    ));

    // A day of silence: more than any test waits for.
    let day = 172_800 * Ticks::QUARTER.0;
    let long = dir.0.join("Long.wav");
    let Outcome::Wait(ticket) = agent.handle(
        &mut shell,
        &session,
        run(
            3,
            "export.audio",
            json!({ "path": long.to_string_lossy(), "bit_depth": 16, "sample_rate": 8000,
                    "start": 0, "length": day }),
        ),
    ) else {
        panic!("the export call didn't wait");
    };
    assert!(shell.export_running());

    // The share moves while the call waits.
    let share = loop {
        shell.poll();
        let status = reply(agent.handle(&mut shell, &session, run(4, "export.status", json!({}))));
        let status = result_of(&status).unwrap();
        if let Some(running) = status["export"].as_object()
            && running["progress"].as_f64().unwrap() > 0.0
        {
            assert!(running["files"][0].as_str().unwrap().ends_with("Long.wav"));
            break running["progress"].as_f64().unwrap();
        }
    };
    assert!(share < 1.0);
    assert!(agent.resume(&mut shell).is_none(), "the export goes on");

    // A second export is refused, naming the first.
    let second = reply(agent.handle(
        &mut shell,
        &session,
        run(
            5,
            "export.audio",
            json!({ "path": dir.0.join("Second.wav").to_string_lossy() }),
        ),
    ));
    let refused = result_of(&second).unwrap_err();
    assert!(refused.contains("Long.wav"), "{refused}");

    // Cancel stops it, and the waiting call is told.
    let cancelled = reply(agent.handle(&mut shell, &session, run(6, "export.cancel", json!({}))));
    result_of(&cancelled).unwrap();
    let waiting = loop {
        shell.poll();
        agent.sync_export(&mut shell);
        if let Some(resumed) = agent.resume(&mut shell) {
            break resumed;
        }
        std::thread::yield_now();
    };
    let (answered, Outcome::Reply(Some(waiting))) = waiting else {
        panic!("no reply");
    };
    assert_eq!(answered, ticket);
    assert_eq!(waiting["id"], 3);
    let error = result_of(&waiting).unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    assert_eq!(dir.names(), ["Stem.wav"], "nothing left behind");

    // The next export runs to its end and its call gets the result.
    let done = dir.0.join("Done.wav");
    let Outcome::Wait(ticket) = agent.handle(
        &mut shell,
        &session,
        run(
            7,
            "export.audio",
            json!({ "path": done.to_string_lossy(), "convert_to_mono": true }),
        ),
    ) else {
        panic!("the export call didn't wait");
    };
    let waiting = loop {
        shell.poll();
        agent.sync_export(&mut shell);
        if let Some(resumed) = agent.resume(&mut shell) {
            break resumed;
        }
        std::thread::yield_now();
    };
    let (answered, Outcome::Reply(Some(waiting))) = waiting else {
        panic!("no reply");
    };
    assert_eq!(answered, ticket);
    let result = result_of(&waiting).unwrap();
    assert_eq!(result["channels"], 1, "{result}");
    assert_eq!(result["bit_depth"], 24, "the default");
    assert!(done.is_file());
}
