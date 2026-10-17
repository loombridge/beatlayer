//! The eight-slot drum sampler through the agent API (WP-5.17): a scripted
//! MCP client drives `bl-cli mcp`, adds the drum sampler, selects slots,
//! loads samples into them, changes a slot's parameters and clears a slot,
//! each by command ID, and reads every effect back from the query tool and
//! `device.parameters` (D17).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use bl_agent::client::{Client, Stdio, Transport};
use serde_json::{Value, json};

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-cli-agent-kit-{name}-{}-{nanos}",
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

/// Runs one command and returns its result.
fn run(client: &mut Client<impl Transport>, command: &str, args: Value) -> Value {
    let done = call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    );
    done["results"][0]["result"].clone()
}

/// Writes a short sine as a 16-bit WAV file of `frames` frames.
fn write_hit(path: &Path, frames: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..frames {
        let sample = (std::f64::consts::TAU * 200.0 * f64::from(n) / 48_000.0).sin() * 0.2;
        writer
            .write_sample((sample * f64::from(i16::MAX)) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
}

/// What the query tool saves of the device: its media list.
fn saved_media(client: &mut Client<impl Transport>, device: &Value) -> Value {
    let found = call(client, "query", json!({ "id": device, "detail": "full" }));
    // A list reads back as a page of items; a device with none has no field.
    found["saved"]["media"]
        .get("items")
        .cloned()
        .unwrap_or(Value::Null)
}

// Spec: eight-slot-drum-sampler R19
#[test]
fn an_agent_loads_selects_and_clears_slots_by_command_id_and_reads_them_back() {
    let dir = TempDir::new("slots");
    let (kick, snare) = (dir.0.join("Kick.wav"), dir.0.join("Snare.wav"));
    write_hit(&kick, 4_800);
    write_hit(&snare, 6_000);
    let mut client = bl_cli_mcp(&["--autonomous"]);

    // The default set's MIDI track takes the drum sampler.
    let tracks = call(&mut client, "query", json!({ "path": "tracks" }));
    let midi = tracks["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "midi")
        .expect("a MIDI track")["id"]
        .clone();
    run(
        &mut client,
        "arrangement.select_tracks",
        json!({ "tracks": [midi] }),
    );
    let added = run(
        &mut client,
        "device.add",
        json!({ "device": {
            "kind": { "native": { "device_type": "beatlayer.kitbag" } },
            "name": "Kitbag",
            "role": "instrument",
        } }),
    );
    let device = added["device"].clone();
    assert_eq!(saved_media(&mut client, &device), Value::Null, "none yet");

    // Slot 3 is selected; the sample with no slot goes into it.
    let selected = run(
        &mut client,
        "device.select_slot",
        json!({ "device": device, "slot": 2 }),
    );
    let panel = selected["devices"][0]["panel"].clone();
    assert_eq!(panel["selected_slot"], 2, "{selected}");
    let kicked = run(
        &mut client,
        "device.load_sample",
        json!({ "device": device, "path": kick.to_string_lossy() }),
    );
    assert_eq!(kicked["slot"], 2);
    let snared = run(
        &mut client,
        "device.load_sample",
        json!({ "device": device, "path": snare.to_string_lossy(), "slot": 5 }),
    );
    assert_eq!(snared["slot"], 5);
    let media = saved_media(&mut client, &device);
    assert_eq!(
        media,
        json!([null, null, kicked["media"], null, null, snared["media"]]),
        "each sample is in its slot"
    );

    // A slot's parameters are its own: Slot 3's Pan is parameter 2 × 32 + 17.
    let parameters = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let params = parameters["params"].as_array().unwrap();
    assert_eq!(params.len(), 8 * 24 + 4);
    let pan = params
        .iter()
        .find(|p| p["name"] == "Slot 3 Pan")
        .expect("a parameter for slot 3's pan");
    assert_eq!(pan["id"], 2 * 32 + 17);
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": 2 * 32 + 17, "value": -30.0 }),
    );
    let parameters = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let value_of = |name: &str| {
        parameters["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .map(|p| p["value"].clone())
            .unwrap()
    };
    assert_eq!(value_of("Slot 3 Pan"), json!(-30.0));
    assert_eq!(value_of("Slot 4 Pan"), json!(0.0));

    // Clearing slot 3 leaves slot 6.
    run(
        &mut client,
        "device.set_sample",
        json!({ "device": device, "media": null, "slot": 2 }),
    );
    assert_eq!(
        saved_media(&mut client, &device),
        json!([null, null, null, null, null, snared["media"]])
    );
    // A slot the device doesn't have is refused, and nothing changes.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "device.select_slot",
                                   "args": { "device": device, "slot": 8 } }] }),
        )
        .expect("the call reaches the server")
        .ok();
    let message = refused.expect_err("slot 8 is refused");
    assert!(message.contains("no slot 8"), "{message}");
    assert!(client.into_transport().finish().unwrap().success());
}
