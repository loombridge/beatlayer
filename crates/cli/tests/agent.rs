//! The agent API end to end (WP-1.15): a scripted MCP client drives
//! `bl-cli mcp` over stdio, as an agent client does, and builds projects
//! headless. Each is checked against the same project built by hand, by
//! its document and by its render.
//!
//! - The M1 demo (the build plan's exit demo, its agent version): eight
//!   stems arranged and mixed, saved, and reopened identically. Its audio
//!   null test waits on clip playback, the transport, and the mixer
//!   (WP-1.4, 1.5, 1.6): until they're in, its clips are silent, and the
//!   loop and export steps are reported as pending.
//! - A golden fixture rebuilt through the agent API renders bit for bit as
//!   the hand-built fixture, which is null-tested against its reference.
//! - Scheduled commands fire on their exact frames through the engine, and
//!   the render matches the golden fixture that schedules the same changes.

bl_rt::install_trap!();

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess, Stdio, Transport};
use bl_cli::golden::{self, Render, SAMPLE_RATE};
use bl_cli::render::{Settings, interleave, render};
use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::{EngineConfig, Offline, Registry, TransportCommand};
use bl_model::{
    AudioData, Clip, ClipData, Command as Edit, ContentHash, Document, Editor, TrackId, TrackKind,
};
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
            "bl-cli-agent-{name}-{}-{nanos}",
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

/// `bl-cli mcp` with these arguments, as a scripted client's server.
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

/// The document as a person sees it, without IDs: settings, then every
/// track in order with its group, mixer, devices, and clips by start.
fn canonical(document: &Document) -> Value {
    let tracks: Vec<Value> = document
        .tracks()
        .map(|track| {
            let mut clips: Vec<&Clip> = track.clips().collect();
            clips.sort_by_key(|clip| (clip.start, clip.name.clone()));
            json!({
                "kind": track.kind(),
                "name": track.name(),
                "group": document.parent(track.id()).map(|g| document.track(g).unwrap().name()),
                "mixer": track.mixer(),
                "devices": track.devices().map(|d| json!({
                    "name": d.name, "kind": d.kind, "enabled": d.enabled,
                    "params": d.params.iter().map(|(p, v)| json!([p, v])).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "clips": clips.iter().map(|c| json!({
                    "name": c.name, "start": c.start, "length": c.length, "mute": c.mute,
                    "data": c.data,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "tempo": document.settings().tempo,
        "time_signatures": document.settings().time_signatures,
        "tracks": tracks,
    })
}

/// Renders a document from its start, as `bl-cli render` does.
fn render_document(document: &Document, frames: u64) -> Render {
    let settings = Settings {
        sample_rate: SAMPLE_RATE,
        block: golden::BLOCK,
        frames,
    };
    let mut samples = Vec::new();
    let summary = render(document, &[], &settings, |output| {
        interleave(output, &mut samples);
    });
    Render {
        sample_rate: SAMPLE_RATE.hz(),
        channels: summary.channels,
        samples,
        scheduled: 0,
        summary,
    }
}

/// The IDs of the default set's tracks, from the agent's overview.
fn default_tracks(client: &mut Client<impl Transport>) -> Vec<u64> {
    let tracks = call(client, "query", json!({ "path": "tracks" }));
    tracks["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["kind"] != "main")
        .map(|t| t["id"].as_u64().unwrap())
        .collect()
}

/// Writes a short sine as a 16-bit WAV file: a stand-in stem. Test audio is
/// generated, never committed.
fn write_stem(path: &Path, hz: f64) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..48_000 {
        let sample = (std::f64::consts::TAU * hz * f64::from(n) / 48_000.0).sin() * 0.1;
        writer
            .write_sample((sample * f64::from(i16::MAX)) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
}

/// The M1 demo's stems: name, test frequency, first bar, bars long, volume,
/// and pan.
const STEMS: &[(&str, f64, i64, i64, f64, f64)] = &[
    ("Kick", 55.0, 1, 16, 1.0, 0.0),
    ("Snare", 180.0, 5, 12, 0.8, 0.0),
    ("Hats", 6000.0, 5, 12, 0.5, 0.3),
    ("Bass", 82.4, 1, 16, 0.9, 0.0),
    ("Keys", 261.6, 9, 8, 0.6, -0.4),
    ("Pad", 196.0, 1, 16, 0.4, 0.0),
    ("Lead", 523.3, 9, 8, 0.7, 0.2),
    ("Vocal", 440.0, 13, 4, 1.0, 0.0),
];

const BAR: i64 = 2_822_400;

/// The stems' content hashes, as importing them would record.
fn stems(dir: &Path) -> Vec<ContentHash> {
    STEMS
        .iter()
        .map(|(name, hz, ..)| {
            let path = dir.join(format!("{name}.wav"));
            write_stem(&path, *hz);
            let hash = bl_media::ContentHash::of_file(&path).unwrap();
            ContentHash(*hash.as_bytes())
        })
        .collect()
}

/// The M1 demo built by hand, through the model's commands, from the
/// factory default set as the agent starts from.
fn hand_built_m1_demo(hashes: &[ContentHash]) -> Editor {
    let mut editor = bl_shell::files::factory_default_set();
    let defaults: Vec<TrackId> = editor.document().top_level().collect();
    for track in defaults {
        editor.apply(Edit::DeleteTrack { track }).unwrap();
    }
    editor
        .apply(Edit::SetTempo {
            tempo: TempoMap::constant(Bpm::new(124.0).unwrap()),
        })
        .unwrap();
    for (index, ((name, _, bar, bars, volume, pan), hash)) in STEMS.iter().zip(hashes).enumerate() {
        let track: TrackId = editor.new_id();
        editor
            .apply(Edit::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: (*name).to_owned(),
                parent: None,
                index,
            })
            .unwrap();
        let mut clip = Clip::new(
            editor.new_id(),
            Ticks(BAR * (bar - 1)),
            Ticks(BAR * bars),
            ClipData::Audio(AudioData { media: *hash }),
        );
        clip.name = (*name).to_owned();
        editor.apply(Edit::CreateClip { track, clip }).unwrap();
        editor
            .apply(Edit::SetVolume {
                track,
                volume: *volume,
            })
            .unwrap();
        editor.apply(Edit::SetPan { track, pan: *pan }).unwrap();
    }
    editor
}

#[test]
fn a_scripted_client_builds_the_m1_demo_headless_and_it_reopens_as_the_hand_built_one() {
    let dir = TempDir::new("m1-demo");
    let hashes = stems(&dir.0);
    let set = dir.0.join("M1 Demo Project").join("M1 Demo.blset");

    let mut client = bl_cli_mcp(&["--autonomous"]);
    let guide = client.read_resource(bl_agent::guide::URI).unwrap();
    assert!(guide.contains("$new"));
    call(
        &mut client,
        "checkpoint",
        json!({ "action": "save", "name": "factory default" }),
    );

    // Clear the default set's audio and MIDI tracks (destructive: allowed
    // because the server runs with --autonomous).
    let defaults = default_tracks(&mut client);
    let deletes: Vec<Value> = defaults
        .iter()
        .take(4)
        .map(|track| json!({ "command": "track.delete", "args": { "track": track } }))
        .collect();
    let cleared = call(
        &mut client,
        "run",
        json!({ "label": "Clear the default tracks", "commands": deletes }),
    );
    assert_eq!(cleared["ran"], 4);

    // Drag the eight stems in: a track and a clip each, one undo step.
    let mut import = vec![json!({
        "command": "song.set_tempo",
        "args": { "tempo": TempoMap::constant(Bpm::new(124.0).unwrap()) },
    })];
    for (index, ((name, _, bar, bars, ..), hash)) in STEMS.iter().zip(&hashes).enumerate() {
        import.push(json!({
            "command": "track.create",
            "args": { "id": {"$new": name}, "kind": "audio", "name": name, "index": index },
        }));
        import.push(json!({
            "command": "clip.create",
            "args": { "track": {"$ref": name},
                      "clip": { "id": {"$new": format!("{name} clip")}, "name": name,
                                "start": BAR * (bar - 1), "length": BAR * bars, "mute": false,
                                "data": { "audio": { "media": hash.to_string() } } } },
        }));
    }
    let imported = call(
        &mut client,
        "run",
        json!({ "label": "Import eight stems", "commands": import }),
    );
    assert_eq!(imported["ran"], 17);
    assert_eq!(imported["undo_step"], "scripted-client: Import eight stems");

    // Mix with the faders, after checking the mix with a dry run.
    let mix: Vec<Value> = STEMS
        .iter()
        .flat_map(|(name, _, _, _, volume, pan)| {
            [
                json!({ "command": "mixer.set_volume",
                        "args": { "track": {"$ref": name}, "volume": volume } }),
                json!({ "command": "mixer.set_pan", "args": { "track": {"$ref": name}, "pan": pan } }),
            ]
        })
        .collect();
    let checked = call(
        &mut client,
        "run",
        json!({ "dry_run": true, "commands": mix.clone() }),
    );
    assert_eq!(checked["valid"], true);
    call(
        &mut client,
        "run",
        json!({ "label": "Mix", "commands": mix }),
    );

    // Loop a section: the transport (WP-1.5) isn't in yet.
    let looped = client
        .call("transport", json!({ "action": "loop", "args": {} }))
        .unwrap();
    assert!(
        looped.is_error && looped.text.contains("WP-1.5"),
        "{}",
        looped.text
    );

    // Save (external: a new file outside any project open now).
    let saved = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as",
                               "args": { "path": set.to_string_lossy() } }] }),
    );
    assert_eq!(saved["results"][0]["result"]["name"], "M1 Demo");
    let built = call(
        &mut client,
        "query",
        json!({ "path": "tracks", "limit": 500 }),
    );
    let history = call(&mut client, "query", json!({ "path": "history" }));
    assert_eq!(
        history["undo"]["items"],
        json!([
            "scripted-client: Mix",
            "scripted-client: Import eight stems",
            "scripted-client: Clear the default tracks",
        ])
    );
    // Quit.
    assert!(client.into_transport().finish().unwrap().success());

    // Reopen, in a new session, and find it as it was.
    let mut reopened = bl_cli_mcp(&["--set", &set.to_string_lossy()]);
    let overview = call(&mut reopened, "query", json!({}));
    assert_eq!(overview["set"]["name"], "M1 Demo");
    assert_eq!(overview["set"]["unsaved_changes"], false);
    assert_eq!(overview["tempo"]["bpm_at_start"], 124.0);
    let again = call(
        &mut reopened,
        "query",
        json!({ "path": "tracks", "limit": 500 }),
    );
    assert_eq!(again, built);
    let vocal = call(
        &mut reopened,
        "query",
        json!({ "path": "track \"Vocal\" / clip #1" }),
    );
    assert_eq!(vocal["start"]["bbs"], "13.1.1");
    assert_eq!(vocal["content"]["media"], hashes[7].to_string());
    drop(reopened);

    // The saved set is the hand-built demo, track for track and clip for
    // clip.
    let (project, _) = bl_project::load(&set).unwrap();
    let hand_built = hand_built_m1_demo(&hashes);
    assert_eq!(
        canonical(&project.document),
        canonical(hand_built.document())
    );

    // And they render alike. Sixteen bars at 124 BPM.
    let frames = (16.0 * 4.0 * 60.0 / 124.0 * 48_000.0) as u64;
    let agent_render = render_document(&project.document, frames);
    let hand_render = render_document(hand_built.document(), frames);
    assert!(
        agent_render.problems().is_empty(),
        "{:?}",
        agent_render.problems()
    );
    assert!(agent_render.same_bits(&hand_render));
    eprintln!(
        "M1 demo renders: {} frames each, bit-identical, peak {:.1} dBFS (audio clips play from \
         WP-1.4; until then the render is silent and the audible null test is pending)",
        agent_render.frames(),
        agent_render.summary.peak_dbfs()
    );
}

#[test]
fn a_golden_fixture_built_through_bl_cli_mcp_renders_bit_for_bit_as_the_hand_built_one() {
    let dir = TempDir::new("fixture");
    let set = dir.0.join("Group and Return.blset");
    let mut client = bl_cli_mcp(&["--autonomous"]);

    let deletes: Vec<Value> = default_tracks(&mut client)
        .into_iter()
        .map(|track| json!({ "command": "track.delete", "args": { "track": track } }))
        .collect();
    call(&mut client, "run", json!({ "commands": deletes }));

    // The `group-and-return` fixture, command for command.
    let track = |name: &str, kind: &str, parent: Option<&str>, index: usize| {
        json!({ "command": "track.create",
                "args": { "id": {"$new": name}, "kind": kind, "name": name,
                          "parent": parent.map(|p| json!({"$ref": p})), "index": index } })
    };
    let tone = |track: &str, hz: f64, dbfs: f64| {
        json!({ "command": "device.insert",
                "args": { "track": {"$ref": track}, "index": 0,
                          "device": { "id": {"$new": format!("{track} tone")}, "name": "Test Tone",
                                      "kind": { "native": { "device_type": "beatlayer.test_tone" } },
                                      "enabled": true, "params": [[0, hz], [1, dbfs]] } } })
    };
    let volume = |track: &str, volume: f64| json!({ "command": "mixer.set_volume", "args": { "track": {"$ref": track}, "volume": volume } });
    let main = call(&mut client, "query", json!({ "path": "track main" }))["id"].clone();
    let commands = vec![
        track("Group", "group", None, 0),
        volume("Group", 0.5),
        track("A", "midi", Some("Group"), 0),
        tone("A", 262.0, -24.0),
        track("B", "midi", Some("Group"), 1),
        tone("B", 392.0, -24.0),
        volume("B", 0.7),
        track("Inner", "group", Some("Group"), 2),
        volume("Inner", 0.8),
        track("C", "audio", Some("Inner"), 0),
        tone("C", 523.25, -24.0),
        track("Beside", "midi", None, 1),
        tone("Beside", 196.0, -30.0),
        track("Return", "return", None, 0),
        tone("Return", 660.0, -30.0),
        volume("Return", 0.5),
        json!({ "command": "mixer.set_volume", "args": { "track": main, "volume": 0.8 } }),
    ];
    call(
        &mut client,
        "run",
        json!({ "label": "Build group-and-return", "commands": commands }),
    );
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as", "args": { "path": set.to_string_lossy() } }] }),
    );
    assert!(client.into_transport().finish().unwrap().success());

    let fixture = golden::fixture("group-and-return").unwrap();
    let scene = fixture.scene();
    let (project, _) = bl_project::load(&set).unwrap();
    assert_eq!(canonical(&project.document), canonical(scene.document()));
    let built = render_document(&project.document, scene.frames);
    let reference = fixture.render();
    assert!(built.problems().is_empty(), "{:?}", built.problems());
    assert!(built.summary.peak > 0.0, "the fixture plays tones");
    assert!(
        built.same_bits(&reference),
        "the agent's set renders as the hand-built fixture"
    );
}

#[test]
fn scheduled_commands_fire_on_their_exact_frames_and_render_as_the_golden_fixture() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("scheduler").unwrap();
    let deletes: Vec<Value> = default_tracks(&mut client)
        .into_iter()
        .map(|track| json!({ "command": "track.delete", "args": { "track": track } }))
        .collect();
    call(&mut client, "run", json!({ "commands": deletes }));

    // The `scheduled-commands` fixture's song: 4/4 then 7/8 at 180 BPM.
    let fixture = golden::fixture("scheduled-commands").unwrap();
    let scene = fixture.scene();
    let settings = scene.document().settings();
    let tone = |track: &str, hz: f64, dbfs: f64| {
        json!({ "command": "device.insert",
                "args": { "track": {"$ref": track}, "index": 0,
                          "device": { "id": {"$new": format!("{track} tone")}, "name": "Test Tone",
                                      "kind": { "native": { "device_type": "beatlayer.test_tone" } },
                                      "enabled": true, "params": [[0, hz], [1, dbfs]] } } })
    };
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "song.set_tempo", "args": { "tempo": settings.tempo } },
            { "command": "song.set_time_signatures",
              "args": { "time_signatures": settings.time_signatures } },
            { "command": "track.create",
              "args": { "id": {"$new": "Lead"}, "kind": "midi", "name": "Lead", "index": 0 } },
            tone("Lead", 440.0, -24.0),
            { "command": "track.create",
              "args": { "id": {"$new": "Pad"}, "kind": "midi", "name": "Pad", "index": 1 } },
            tone("Pad", 220.0, -30.0),
        ] }),
    );
    // Its changes, scheduled at the same bars and beats through the agent
    // API as registered commands.
    let lead = json!({"$ref": "Lead"});
    let pad = json!({"$ref": "Pad"});
    let lead_tone = json!({"$ref": "Lead tone"});
    let schedule = [
        (
            "1.3.1",
            "mixer.set_mute",
            json!({ "track": lead, "mute": true }),
        ),
        (
            "1.4.1",
            "mixer.set_mute",
            json!({ "track": lead, "mute": false }),
        ),
        (
            "1.4.3",
            "mixer.set_volume",
            json!({ "track": pad, "volume": 0.5 }),
        ),
        (
            "2.1.1",
            "device.set_param",
            json!({ "device": lead_tone, "param": 0, "value": 660.0 }),
        ),
        (
            "2.4.1",
            "mixer.set_volume",
            json!({ "track": pad, "volume": 0.25 }),
        ),
        (
            "2.5.2",
            "device.set_param",
            json!({ "device": lead_tone, "param": 1, "value": -36.0 }),
        ),
        (
            "3.1.1",
            "device.set_param",
            json!({ "device": lead_tone, "param": 0, "value": 495.0 }),
        ),
        (
            "3.3.2",
            "mixer.set_volume",
            json!({ "track": lead, "volume": 0.5 }),
        ),
        (
            "3.7.2",
            "mixer.set_mute",
            json!({ "track": pad, "mute": true }),
        ),
    ];
    for (at, command, args) in &schedule {
        let added = call(
            &mut client,
            "schedule",
            json!({ "action": "add", "at": at, "command": command, "args": args }),
        );
        assert_eq!(added["sample_accurate"], true, "{at}");
    }

    // Play the song through the engine on this thread, as the app's audio
    // thread would, handing the engine's reports back to the agent API
    // between blocks as the main thread would.
    let transport = client.transport();
    let mut offline = Offline::new(
        EngineConfig::new(SAMPLE_RATE, golden::BLOCK),
        Registry::new(),
    );
    assert!(offline.sync(transport.shell.editor().document()).is_empty());
    offline.sender().transport(TransportCommand::Play);
    for message in transport.agent.take_engine_messages() {
        match message {
            bl_agent::EngineMessage::Schedule(command) => offline.sender().schedule(command),
            bl_agent::EngineMessage::Cancel(id) => offline.sender().cancel(id),
        }
    }
    let mut samples = Vec::new();
    let mut fired = Vec::new();
    let mut done = 0;
    while done < scene.frames {
        let block = (golden::BLOCK as u64).min(scene.frames - done);
        interleave(&offline.process(block as usize), &mut samples);
        done += block;
        for report in offline.reports() {
            if let bl_engine::Report::Fired { position, .. } = report {
                fired.push(position.get());
            }
            transport.agent.on_report(&mut transport.shell, report);
        }
        offline.sync(transport.shell.editor().document());
    }
    assert_eq!(
        offline.heap_operations(),
        0,
        "the engine's blocks stay off the heap"
    );

    // Each fired on the frame its bar and beat falls on.
    let expected: Vec<i64> = schedule
        .iter()
        .map(|(at, ..)| {
            let ticks = settings.time_signatures.parse_position(at).unwrap();
            settings.tempo.ticks_to_frames(ticks, SAMPLE_RATE).get()
        })
        .collect();
    assert_eq!(fired, expected);

    // The render is the golden fixture's, to the bit.
    let reference = fixture.render();
    assert_eq!(samples.len(), reference.samples.len());
    assert!(
        samples
            .iter()
            .zip(&reference.samples)
            .all(|(a, b)| a.to_bits() == b.to_bits()),
        "the scheduled commands play as the fixture's"
    );

    // And the document followed: each fired command is an undo step with
    // the agent's name and the position.
    let list = call(&mut client, "schedule", json!({ "action": "list" }));
    assert!(
        list["scheduled"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["state"] == "fired")
    );
    let shell = &client.transport().shell;
    let history: Vec<&str> = shell.editor().undo_history().collect();
    assert_eq!(history[0], "scheduler: Activate/Deactivate Track at 3.7.2");
    assert_eq!(history[8], "scheduler: Activate/Deactivate Track at 1.3.1");
    let document = shell.editor().document();
    let pad = document.tracks().find(|t| t.name() == "Pad").unwrap();
    assert!(pad.mixer().mute);
    assert_eq!(pad.mixer().volume, 0.25);
}

#[test]
fn without_autonomous_bl_cli_mcp_refuses_what_would_ask_and_says_how_to_allow_it() {
    let dir = TempDir::new("refused");
    let set = dir.0.join("Refused.blset");
    let mut client = bl_cli_mcp(&[]);
    let result = client
        .call(
            "run",
            json!({ "commands": [{ "command": "set.save_as", "args": { "path": set.to_string_lossy() } }] }),
        )
        .unwrap();
    assert!(result.is_error);
    assert!(
        result.text.contains("`set.save_as` is external"),
        "{}",
        result.text
    );
    assert!(result.text.contains("--autonomous"), "{}", result.text);
    assert!(!set.exists());
    // Edits still run.
    let renamed = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "track.rename", "args": { "track": 2, "name": "Keys" } }] }),
    );
    assert_eq!(renamed["ran"], 1);
    // Quitting is destructive too.
    let quit = client
        .call("run", json!({ "commands": [{ "command": "app.quit" }] }))
        .unwrap();
    assert!(quit.is_error, "{}", quit.text);
    assert!(client.into_transport().finish().unwrap().success());
}

#[test]
fn an_autonomous_agent_can_quit_bl_cli_mcp() {
    let mut client = bl_cli_mcp(&["--autonomous"]);
    let quit = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "app.quit" }] }),
    );
    assert_eq!(quit["ran"], 1);
    // The server has stopped: the next request finds its output closed.
    assert!(client.request("ping", json!({})).is_err());
    assert!(client.into_transport().finish().unwrap().success());
}
