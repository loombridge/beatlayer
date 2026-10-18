//! The agent API end to end (WP-1.15): a scripted MCP client drives
//! `bl-cli mcp` over stdio, as an agent client does, and builds projects
//! headless. Each is checked against the same project built by hand, by
//! its document and by its render.
//!
//! - The M1 demo (the build plan's exit demo, its agent version): eight
//!   stems arranged, a section looped through the transport, mixed, saved,
//!   and reopened identically, and rendered as the hand-built demo renders.
//!   The clips play the stems (WP-1.4) through the mixer (WP-1.6), so the
//!   two renders are an audible null test.
//! - A golden fixture rebuilt through the agent API renders bit for bit as
//!   the hand-built fixture, which is null-tested against its reference.
//! - Scheduled commands fire on their exact frames through the engine, and
//!   the render matches the golden fixture that schedules the same changes.
//! - An agent plays, locates, continues, and stops the transport (WP-1.5)
//!   headless through the engine's queue, with scheduled transport commands
//!   among them, and every frame plays where the transport's rules put it.

bl_rt::install_trap!();

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bl_agent::client::{Client, InProcess, Stdio, Transport};
use bl_agent::{Agent, EngineMessage};
use bl_cli::golden::{self, Render, SAMPLE_RATE};
use bl_cli::render::{Cue, Settings, interleave, render_with};
use bl_core::time::{Bpm, Frames, TempoMap, Ticks};
use bl_engine::nodes::{BEAT_TONE, BeatTone};
use bl_engine::{
    EngineConfig, MediaMap, Offline, Registry, Report, SessionCommand, SlotState, TransportCommand,
};
use bl_model::{
    AudioData, Clip, ClipData, Command as Edit, ContentHash, Document, Editor, LaunchQuantization,
    LaunchSettings, TrackId, TrackKind,
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
        "loop": document.settings().arrangement_loop,
        "tracks": tracks,
    })
}

/// Renders a document from its start, as `bl-cli render` does.
fn render_document(document: &Document, media: MediaMap, frames: u64) -> Render {
    let settings = Settings {
        sample_rate: SAMPLE_RATE,
        block: golden::BLOCK,
        frames,
    };
    let mut samples = Vec::new();
    let summary = render_with(
        document,
        Arc::new(media),
        &[],
        &[Cue::PLAY],
        &settings,
        |output| interleave(output, &mut samples),
    );
    Render {
        sample_rate: SAMPLE_RATE.hz(),
        channels: summary.channels,
        samples,
        scheduled: 0,
        summary,
    }
}

/// Where `set.save_as` saved the set, from `run`'s result: a new set goes
/// into a new `<name> Project` folder (WP-1.7b).
fn saved_path(result: &Value) -> PathBuf {
    PathBuf::from(
        result["results"][0]["result"]["path"]
            .as_str()
            .expect("the save says where the set went"),
    )
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

/// The stems in `dir` decoded, by content hash, as the app's media library
/// serves them to the clip players (WP-1.4).
fn stem_media(dir: &Path) -> MediaMap {
    let mut media = MediaMap::new();
    for (name, ..) in STEMS {
        let path = dir.join(format!("{name}.wav"));
        let hash = ContentHash(*bl_media::ContentHash::of_file(&path).unwrap().as_bytes());
        let decoded = bl_media::Decoder::open(&path)
            .and_then(bl_media::Decoder::decode_to_end)
            .expect("the stem decodes");
        media.insert(hash, Arc::new(decoded));
    }
    media
}

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
            ClipData::Audio(AudioData::new(*hash)),
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
        .apply(Edit::SetLoop {
            start: Ticks(BAR * 4),
            length: Ticks(BAR * 8),
        })
        .unwrap();
    editor
        .apply(Edit::SetLoopEnabled { enabled: true })
        .unwrap();
    editor
}

#[test]
fn a_scripted_client_builds_the_m1_demo_headless_and_it_reopens_as_the_hand_built_one() {
    let dir = TempDir::new("m1-demo");
    let hashes = stems(&dir.0);

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

    // Loop a section, bars 5 to 12, through the transport (WP-1.5). The
    // loop belongs to the set, so it's saved with it. Playing it needs an
    // engine, which `bl-cli mcp` doesn't run, and it says so.
    call(
        &mut client,
        "transport",
        json!({ "action": "set_loop", "args": { "start": BAR * 4, "length": BAR * 8 } }),
    );
    call(
        &mut client,
        "transport",
        json!({ "action": "set_loop_enabled", "args": { "enabled": true } }),
    );
    let played = client
        .call("transport", json!({ "action": "play" }))
        .unwrap();
    assert!(
        played.is_error && played.text.contains("no engine is attached"),
        "{}",
        played.text
    );

    // Save (external: a new file outside any project open now). A new set
    // goes into a new project folder, and the result says where.
    let saved = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as",
                               "args": { "path": dir.0.join("M1 Demo.blset").to_string_lossy() } }] }),
    );
    assert_eq!(saved["results"][0]["result"]["name"], "M1 Demo");
    let set = saved_path(&saved);
    assert!(set.ends_with("M1 Demo Project/M1 Demo.blset"), "{set:?}");
    let built = call(
        &mut client,
        "query",
        json!({ "path": "tracks", "limit": 500 }),
    );
    let history = call(&mut client, "query", json!({ "path": "history" }));
    assert_eq!(
        history["undo"]["items"],
        json!([
            "scripted-client: Arrangement Loop On/Off",
            "scripted-client: Set Loop Brace",
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
    let transport = call(&mut reopened, "transport", json!({ "action": "status" }));
    assert_eq!(transport["loop"]["enabled"], true);
    assert_eq!(transport["loop"]["start"]["bbs"], "5.1.1");
    assert_eq!(transport["loop"]["end"]["bbs"], "13.1.1");
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
    let agent_render = render_document(&project.document, stem_media(&dir.0), frames);
    let hand_render = render_document(hand_built.document(), stem_media(&dir.0), frames);
    assert!(
        agent_render.problems().is_empty(),
        "{:?}",
        agent_render.problems()
    );
    assert!(agent_render.same_bits(&hand_render));
    assert!(
        agent_render.summary.peak_dbfs().is_finite(),
        "the clips play their stems, so the demo isn't silent"
    );
    eprintln!(
        "M1 demo renders: {} frames each, bit-identical, peak {:.1} dBFS",
        agent_render.frames(),
        agent_render.summary.peak_dbfs()
    );
}

#[test]
fn a_golden_fixture_built_through_bl_cli_mcp_renders_bit_for_bit_as_the_hand_built_one() {
    let dir = TempDir::new("fixture");
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
    let saved = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as",
                               "args": { "path": dir.0.join("Group and Return.blset").to_string_lossy() } }] }),
    );
    let set = saved_path(&saved);
    assert!(client.into_transport().finish().unwrap().success());

    let fixture = golden::fixture("group-and-return").unwrap();
    let scene = fixture.scene();
    let (project, _) = bl_project::load(&set).unwrap();
    assert_eq!(canonical(&project.document), canonical(scene.document()));
    let built = render_document(&project.document, MediaMap::new(), scene.frames);
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
            EngineMessage::Transport(command) => offline.sender().transport(command),
            EngineMessage::Session(command) => offline.sender().session(command),
            EngineMessage::Schedule(command) => offline.sender().schedule(command),
            EngineMessage::Cancel(id) => offline.sender().cancel(id),
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

/// The app's main thread with an engine, played on this thread: after
/// each tool call it syncs the document and puts what the agent API asked
/// of the engine on the engine's queue, as it would the UI's commands; it
/// plays blocks as the audio thread would; and between blocks it hands the
/// engine's reports and state back to the agent API.
struct EngineHost {
    client: Client<InProcess>,
    offline: Offline,
    block: usize,
    /// Frames played: the engine clock.
    clock: u64,
    samples: Vec<f32>,
    /// Each transport command sent, with the clock frame it was sent at,
    /// the start of the block it acts in.
    sent: Vec<(u64, TransportCommand)>,
    fired: Vec<(u64, i64)>,
}

impl EngineHost {
    fn new(block: usize) -> Self {
        Self::with_registry(block, Registry::new())
    }

    fn with_registry(block: usize, registry: Registry) -> Self {
        // Not autonomous: the transport's commands are `read`, so they run
        // without asking anyway.
        let mut client = Client::new(InProcess::new(Agent::headless(false), Shell::headless()));
        client.initialize("player").unwrap();
        client.transport().agent.set_engine_attached(true);
        Self {
            client,
            offline: Offline::new(EngineConfig::new(SAMPLE_RATE, block), registry),
            block,
            clock: 0,
            samples: Vec::new(),
            sent: Vec::new(),
            fired: Vec::new(),
        }
    }

    fn call(&mut self, tool: &str, args: Value) -> Value {
        let result = call(&mut self.client, tool, args);
        let transport = self.client.transport();
        self.offline.sync(transport.shell.editor().document());
        for message in transport.agent.take_engine_messages() {
            match message {
                EngineMessage::Transport(command) => {
                    self.sent.push((self.clock, command));
                    self.offline.sender().transport(command);
                }
                EngineMessage::Session(command) => self.offline.sender().session(command),
                EngineMessage::Schedule(command) => self.offline.sender().schedule(command),
                EngineMessage::Cancel(id) => self.offline.sender().cancel(id),
            }
        }
        result
    }

    /// Plays until the engine clock reaches `clock`.
    fn play_to(&mut self, clock: u64) {
        while self.clock < clock {
            let frames = (self.block as u64).min(clock - self.clock);
            interleave(&self.offline.process(frames as usize), &mut self.samples);
            self.clock += frames;
            let transport = self.client.transport();
            for report in self.offline.reports() {
                if let Report::Fired {
                    position, clock, ..
                } = report
                {
                    self.fired.push((clock, position.get()));
                }
                transport.agent.on_report(&mut transport.shell, report);
            }
            transport.agent.on_engine_state(self.offline.state());
            self.offline.sync(transport.shell.editor().document());
        }
    }

    fn status(&mut self) -> Value {
        self.call("transport", json!({ "action": "status" }))
    }

    /// Hands the display data the devices published to the shell, as the
    /// app's poll of the engine does.
    fn publish_displays(&mut self) {
        let mut data = std::collections::BTreeMap::new();
        self.offline.displays().read_all(&mut data);
        let shell = &mut self.client.transport().shell;
        for (device, data) in data {
            shell.set_device_display_data(device, &data.values);
        }
    }
}

/// An agent reads the Contour's spectrum and the Squeeze's gain reduction
/// by their device IDs, as the engine published them after the last block,
/// at any block size (the device framework spec, R26 and R28).
#[test]
fn an_agent_reads_a_contours_spectrum_and_a_squeezes_gain_reduction_by_device_id() {
    for block in [1, 64, 512, 4_096] {
        let mut registry = Registry::new();
        bl_devices::register(&mut registry);
        let mut host = EngineHost::with_registry(block, registry);
        let device = |index: u32, name: &str, device_type: &str| {
            json!({ "command": "device.insert",
                    "args": { "track": {"$ref": "Track"}, "index": index,
                              "device": { "id": {"$new": name}, "name": name,
                                          "kind": { "native": { "device_type": device_type } },
                                          "enabled": true, "params": [] } } })
        };
        host.call(
            "run",
            json!({ "label": "Add devices", "commands": [
                { "command": "track.create",
                  "args": { "id": {"$new": "Track"}, "kind": "audio", "name": "Track", "index": 0 } },
                device(0, "Tone", bl_engine::nodes::TEST_TONE),
                device(1, "Contour", bl_devices::eq::EQ),
                device(2, "Squeeze", bl_devices::compressor::COMPRESSOR),
                { "command": "device.set_param",
                  "args": { "device": {"$ref": "Tone"}, "param": 0, "value": 1000.0 } },
                { "command": "device.set_param",
                  "args": { "device": {"$ref": "Tone"}, "param": 1, "value": -6.0 } },
                { "command": "device.set_param",
                  "args": { "device": {"$ref": "Squeeze"}, "param": 0, "value": -30.0 } },
            ] }),
        );
        let ids: Vec<_> = {
            let document = host.client.transport().shell.editor().document();
            let track = document.tracks().find(|t| t.name() == "Track").unwrap();
            track.devices().map(|d| d.id).collect()
        };
        let (contour, squeeze) = (ids[1], ids[2]);
        host.play_to(16_384);
        host.publish_displays();
        let read = |host: &mut EngineHost, device| {
            let reply = host.call(
                "run",
                json!({ "label": "Read", "commands": [
                    { "command": "device.display", "args": { "device": device } },
                ] }),
            );
            reply["results"][0]["result"].clone()
        };

        let contour = read(&mut host, contour);
        assert_eq!(contour["live"], true);
        let series = |name: &str| {
            contour["series"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["name"] == name)
                .unwrap_or_else(|| panic!("block {block}: no {name} in {contour}"))
                .clone()
        };
        let spectrum: Vec<f64> = series("Spectrum")["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let loudest = spectrum
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        let centre = 20.0 * (1000.0f64).powf((loudest as f64 + 0.5) / 64.0);
        assert!(
            (centre / 1000.0).ln().abs() < 0.2,
            "block {block}: the 1 kHz tone is read at {centre:.0} Hz"
        );

        let squeeze = read(&mut host, squeeze);
        let reduction = squeeze["readings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "Gain Reduction")
            .unwrap_or_else(|| panic!("block {block}: no reduction in {squeeze}"));
        assert!(
            reduction["value"].as_f64().unwrap() < -1.0,
            "block {block}: {reduction}"
        );
        assert_eq!(squeeze["view"], "Transfer Curve");
        assert_eq!(host.offline.heap_operations(), 0, "block {block}");
    }
}

/// The transport after the transport spec, one frame at a time,
/// apart from the engine's code: what each frame of the engine clock plays.
struct TransportModel {
    tempo: TempoMap,
    playing: bool,
    position: i64,
    insert_marker: Ticks,
    /// Scheduled transport commands: the song frame each fires on, once,
    /// when the playing song reaches it.
    scheduled: Vec<(i64, TransportCommand)>,
}

impl TransportModel {
    fn frames(&self, ticks: Ticks) -> i64 {
        self.tempo.ticks_to_frames(ticks, SAMPLE_RATE).get()
    }

    fn apply(&mut self, command: TransportCommand) {
        match command {
            TransportCommand::Play => {
                self.position = self.frames(self.insert_marker);
                self.playing = true;
            }
            TransportCommand::Stop if self.playing => self.playing = false,
            TransportCommand::Stop => {
                self.insert_marker = Ticks::ZERO;
                self.position = 0;
            }
            TransportCommand::Continue => self.playing = true,
            TransportCommand::Locate(ticks) => {
                self.insert_marker = ticks;
                if self.playing {
                    self.position = self.frames(ticks);
                }
            }
            other => panic!("the model has no {other:?}"),
        }
    }

    /// The song frame this clock frame plays, if it plays one.
    fn step(&mut self) -> Option<i64> {
        while self.playing
            && let Some(due) = self
                .scheduled
                .iter()
                .position(|(frame, _)| *frame == self.position)
        {
            let (_, command) = self.scheduled.remove(due);
            self.apply(command);
        }
        if !self.playing {
            return None;
        }
        self.position += 1;
        Some(self.position - 1)
    }
}

/// An agent starts, locates, and stops the transport headless, through the
/// engine's queue, with transport commands scheduled at bars and beats
/// among its moves. Every frame is the beat tone at the song position the
/// transport's rules give it, or silence, at any block size, and the
/// scheduled ones act on their exact samples.
#[test]
fn an_agent_plays_locates_and_stops_the_transport_on_exact_samples() {
    const BAR_FRAMES: i64 = 96_000; // 4/4 at 120 BPM and 48 kHz
    let tempo = TempoMap::default();
    let ticks_at = |frames: i64| tempo.frames_to_ticks(Frames(frames), SAMPLE_RATE).get();
    let start = Ticks(BAR + 12_345); // off the beat: its sine doesn't start at 0
    let mut renders = Vec::new();
    for block in [61, 1_000, 4_096] {
        let mut host = EngineHost::new(block);
        host.call(
            "run",
            json!({ "label": "Add a beat", "commands": [
                { "command": "track.create",
                  "args": { "id": {"$new": "Beat"}, "kind": "midi", "name": "Beat", "index": 0 } },
                // At its default level, −60 dBFS.
                { "command": "device.insert",
                  "args": { "track": {"$ref": "Beat"}, "index": 0,
                            "device": { "id": {"$new": "tone"}, "name": "Beat Tone",
                                        "kind": { "native": { "device_type": BEAT_TONE } },
                                        "enabled": true, "params": [] } } },
            ] }),
        );
        let status = host.status();
        assert_eq!(status["engine_plays_here"], true);

        // Set the insert marker, and play from it.
        host.call(
            "transport",
            json!({ "action": "locate", "args": { "position": start.get() } }),
        );
        host.play_to(10_007);
        let status = host.status();
        assert_eq!(status["playing"], false);
        assert_eq!(status["insert_marker"]["ticks"], start.get());
        assert_eq!(status["position"]["bbs"], "1.1.1");
        // At 3.1.1, jump to 5.1.1; at 5.3.1, stop.
        for (at, command, args) in [
            ("3.1.1", "transport.locate", json!({ "position": BAR * 4 })),
            ("5.3.1", "transport.stop", json!({})),
        ] {
            let added = host.call(
                "schedule",
                json!({ "action": "add", "at": at, "command": command, "args": args }),
            );
            assert_eq!(added["sample_accurate"], true, "{at}");
        }
        host.call("transport", json!({ "action": "play" }));
        host.play_to(60_000);
        let status = host.status();
        assert_eq!(status["playing"], true);
        let first = tempo.ticks_to_frames(start, SAMPLE_RATE).get();
        assert_eq!(
            status["position"]["ticks"],
            ticks_at(first + 60_000 - 10_007)
        );

        // Locate while playing: playback jumps back to 2.2.1. The song
        // reaches 3.1.1 72,000 frames later and jumps to 5.1.1, and reaches
        // 5.3.1 48,000 after that, and stops.
        host.call("transport", json!({ "action": "locate", "at": "2.2.1" }));
        host.play_to(190_000);
        let status = host.status();
        assert_eq!(status["playing"], false);
        assert_eq!(status["position"]["bbs"], "5.3.1");
        assert_eq!(status["insert_marker"]["bbs"], "5.1.1");
        assert_eq!(
            host.fired,
            [
                (60_000 + 72_000, BAR_FRAMES * 2),
                (60_000 + 72_000 + 48_000, BAR_FRAMES * 4 + 48_000),
            ],
            "block {block}: each scheduled command fires on its exact frame"
        );

        // Continue from where it stopped, stop, and stop again to return to
        // the start.
        host.call("transport", json!({ "action": "continue" }));
        host.play_to(200_000);
        host.call("transport", json!({ "action": "stop" }));
        host.play_to(205_000);
        let status = host.status();
        assert_eq!(status["playing"], false);
        assert_eq!(
            status["position"]["ticks"],
            ticks_at(BAR_FRAMES * 4 + 48_000 + 10_000)
        );
        host.call("transport", json!({ "action": "stop" }));
        host.play_to(210_000);
        let status = host.status();
        assert_eq!(status["position"]["bbs"], "1.1.1");
        assert_eq!(status["insert_marker"]["bbs"], "1.1.1");
        assert_eq!(
            host.offline.heap_operations(),
            0,
            "the engine's blocks stay off the heap"
        );

        // The scheduled commands are spent, and the transport left no undo
        // step: only the agent's edit is in the history.
        let list = host.call("schedule", json!({ "action": "list" }));
        let states: Vec<&Value> = list["scheduled"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| &e["state"])
            .collect();
        assert_eq!(states, [&json!("fired"), &json!("fired")]);
        let history = host.call("query", json!({ "path": "history" }));
        assert_eq!(history["undo"]["items"], json!(["player: Add a beat"]));

        // Every frame, against the model: the agent's commands act at the
        // start of the block after the call that sent them, and the
        // scheduled ones on their song frames.
        let document = host.client.transport().shell.editor().document();
        let device = document
            .tracks()
            .flat_map(|track| track.devices())
            .next()
            .unwrap();
        let tone = BeatTone::new(device);
        let mut model = TransportModel {
            tempo: tempo.clone(),
            playing: false,
            position: 0,
            insert_marker: Ticks::ZERO,
            scheduled: vec![
                (BAR_FRAMES * 2, TransportCommand::Locate(Ticks(BAR * 4))),
                (BAR_FRAMES * 4 + 48_000, TransportCommand::Stop),
            ],
        };
        let mut sent = host.sent.clone();
        sent.reverse();
        let mut played = 0;
        for frame in 0..host.clock {
            while sent.last().is_some_and(|(at, _)| *at == frame) {
                model.apply(sent.pop().unwrap().1);
            }
            let expected = model.step().map_or(0.0, |position| {
                played += 1;
                tone.sample_at(&model.tempo, Frames(position), SAMPLE_RATE)
            });
            for channel in 0..2 {
                let sample = host.samples[frame as usize * 2 + channel];
                assert_eq!(
                    sample.to_bits(),
                    expected.to_bits(),
                    "block {block}: frame {frame}, channel {channel}: {sample} where the model \
                     has {expected}"
                );
            }
        }
        assert!(sent.is_empty());
        assert!(model.scheduled.is_empty(), "both scheduled commands fired");
        // 10,007 to 60,000, 60,000 to 180,000, and 190,000 to 200,000.
        assert_eq!(played, 49_993 + 120_000 + 10_000, "block {block}");
        renders.push(host.samples);
    }
    assert!(
        renders.windows(2).all(|pair| pair[0]
            .iter()
            .zip(&pair[1])
            .all(|(a, b)| a.to_bits() == b.to_bits())),
        "every block size plays the same samples"
    );
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
fn with_result_format_toon_a_long_table_is_toon_and_anything_else_stays_compact_json() {
    let mut json = bl_cli_mcp(&[]);
    let mut toon = bl_cli_mcp(&["--result-format", "toon"]);

    // The factory set lists 7 tracks, short of the 8 rows a table needs.
    let short = toon.call("query", json!({ "path": "tracks" })).unwrap();
    assert!(short.text.starts_with("{\"tracks\":"), "{}", short.text);

    // Four more tracks make a table of 11 rows.
    let add: Vec<Value> = (0..4)
        .map(|i| {
            json!({ "command": "track.create", "args": {
                "id": { "$new": format!("t{i}") }, "kind": "midi", "index": 4 + i,
                "name": format!("Added {i}") } })
        })
        .collect();
    for client in [&mut json, &mut toon] {
        call(client, "run", json!({ "commands": add }));
    }
    let query = json!({ "path": "tracks" });
    let plain = json.call("query", query.clone()).unwrap();
    let long = toon.call("query", query).unwrap();
    assert!(plain.text.starts_with("{\"tracks\":"), "{}", plain.text);
    assert!(
        long.text
            .starts_with("tracks:\n  items[11]{clips,devices,group,id,kind,mixer.arm,"),
        "{}",
        long.text
    );
    assert!(long.text.len() < plain.text.len());
    // `structuredContent` is the same JSON either way.
    assert_eq!(long.structured, plain.structured);
    assert_eq!(long.structured.as_ref().unwrap()["tracks"]["total"], 11);

    // A result with no table stays compact JSON.
    let nested = toon
        .call("describe_command", json!({ "command": "clip.create" }))
        .unwrap();
    assert!(nested.text.starts_with("{\"access\":"), "{}", nested.text);
}

#[test]
fn the_assistant_settings_are_the_person_s_own_through_the_agent_api() {
    // Spec: agent-permissions R9, by analogy. Even an autonomous session
    // may not change the built-in assistant's settings or its key.
    let mut client = bl_cli_mcp(&["--autonomous"]);
    let model = client
        .call(
            "run",
            json!({ "commands": [{ "command": "assistant.set_model",
                                   "args": { "model": "gpt-4o" } }] }),
        )
        .unwrap();
    assert!(model.is_error, "{}", model.text);
    assert!(model.text.contains("only a person"), "{}", model.text);
    assert!(model.text.contains("assistant.set_model"), "{}", model.text);

    let key = client
        .call(
            "run",
            json!({ "commands": [{ "command": "assistant.set_key",
                                   "args": { "key": "sk-not-stored" } }] }),
        )
        .unwrap();
    assert!(key.is_error, "{}", key.text);
    assert!(key.text.contains("only a person"), "{}", key.text);
    assert!(client.into_transport().finish().unwrap().success());
}

/// An agent sets the Temporary Folder by command ID and reads it back from
/// `settings.state` (WP-4.20).
#[test]
fn an_agent_sets_the_temporary_folder_by_command_id_and_reads_it_back() {
    // Spec: audio-clip-editing R5
    let dir = TempDir::new("temporary-folder");
    let chosen = dir.0.join("Scratch");
    let mut client = bl_cli_mcp(&["--autonomous"]);
    let set = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "settings.set_temporary_folder",
                               "args": { "path": chosen } }] }),
    );
    assert_eq!(set["results"][0]["result"]["path"], json!(chosen), "{set}");
    let state = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "settings.state", "args": {} }] }),
    );
    assert_eq!(
        state["results"][0]["result"]["file_folder"]["temporary_folder"],
        json!(chosen),
        "{state}"
    );
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

/// An agent launches a slot's clip, stops its track, launches the whole
/// scene, and stops every clip, by command ID through the engine's queue; the
/// engine's published slot states follow each command (WP-3.2).
#[test]
fn an_agent_launches_and_stops_session_clips_by_command_id() {
    // Spec: launch-quantisation R9
    let block = 512u64;
    let mut host = EngineHost::new(block as usize);
    // A MIDI track with an empty clip in the first scene's slot.
    host.call(
        "run",
        json!({ "label": "Add a slot", "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Slots"}, "kind": "midi", "name": "Slots", "index": 0 } },
        ] }),
    );
    let (track, scene) = {
        let document = host.client.transport().shell.editor().document();
        let track = document
            .tracks()
            .find(|track| track.name() == "Slots")
            .expect("the track was made")
            .id();
        let scene = document.scenes().next().expect("a new set has scenes").id;
        (track, scene)
    };
    host.call(
        "run",
        json!({ "commands": [
            { "command": "session.insert_empty_midi_clip",
              "args": { "track": track, "scene": scene } },
        ] }),
    );
    // The set's quantisation off, as a set saved with it off: the launches
    // and stops land at once, so a block's frames show each one.
    host.offline
        .sender()
        .session(SessionCommand::SetQuantization(LaunchSettings {
            quantization: LaunchQuantization::None,
            enabled: true,
        }));

    // Launch the slot's clip by ID. With the transport stopped the launch
    // starts it, and the slot plays from the Arrangement position.
    let launched = host.call(
        "run",
        json!({ "commands": [
            { "command": "session.launch", "args": { "track": track, "scene": scene } },
        ] }),
    );
    assert_eq!(launched["results"][0]["command"], "session.launch");
    host.play_to(2 * block);
    assert_eq!(host.status()["playing"], true);
    let states = host.offline.state().session.clone();
    assert_eq!(states.len(), 1, "{states:?}");
    assert_eq!(states[0].track, track);
    assert_eq!(states[0].scene, scene);
    assert_eq!(states[0].state, SlotState::Playing);

    // Stop its track by ID: its clip stops, and so does the slot.
    host.call(
        "run",
        json!({ "commands": [
            { "command": "session.stop_track", "args": { "track": track } },
        ] }),
    );
    host.play_to(4 * block);
    assert!(host.offline.state().session.is_empty());

    // Launch the scene by ID: its clips start together.
    host.call(
        "run",
        json!({ "commands": [
            { "command": "session.launch_scene", "args": { "scene": scene } },
        ] }),
    );
    host.play_to(6 * block);
    let states = host.offline.state().session.clone();
    assert_eq!(states.len(), 1, "{states:?}");
    assert_eq!(states[0].state, SlotState::Playing);

    // Stop All Clips by ID: every track stops.
    host.call(
        "run",
        json!({ "commands": [{ "command": "session.stop_all" }] }),
    );
    host.play_to(8 * block);
    assert!(host.offline.state().session.is_empty());

    // Spec: session-view R21 to R23. The clip played and Stop All Clips
    // pressed every stop button, so the tracks stay taken over from the
    // Arrangement until a return, by ID.
    assert!(host.offline.state().session_over.contains(&track));
    host.call(
        "run",
        json!({ "commands": [{ "command": "session.back_to_arrangement" }] }),
    );
    host.play_to(10 * block);
    assert!(host.offline.state().session_over.is_empty());
    host.call(
        "run",
        json!({ "commands": [
            { "command": "session.launch", "args": { "track": track, "scene": scene } },
        ] }),
    );
    host.play_to(12 * block);
    assert_eq!(host.offline.state().session_over, [track]);
    host.call(
        "run",
        json!({ "commands": [
            { "command": "session.back_to_arrangement_track", "args": { "track": track } },
        ] }),
    );
    host.play_to(14 * block);
    assert!(host.offline.state().session_over.is_empty());
    assert!(host.offline.state().session.is_empty(), "its clip stopped");

    // Spec: launch-quantisation R18, R22. Set the clip to Gate by ID, press
    // and release its launch button by ID: with quantisation off, it plays
    // while pressed and stops at the release.
    let clip = host
        .client
        .transport()
        .shell
        .editor()
        .document()
        .track(track)
        .and_then(|track| track.slot(scene))
        .expect("the slot holds the clip")
        .id;
    host.call(
        "run",
        json!({ "commands": [
            { "command": "clip.set_launch_mode", "args": { "clip": clip, "mode": "gate" } },
        ] }),
    );
    host.call(
        "run",
        json!({ "commands": [
            { "command": "session.launch", "args": { "track": track, "scene": scene } },
        ] }),
    );
    host.play_to(16 * block);
    assert_eq!(host.offline.state().session[0].state, SlotState::Playing);
    host.call(
        "run",
        json!({ "commands": [
            { "command": "session.release", "args": { "track": track, "scene": scene } },
        ] }),
    );
    host.play_to(18 * block);
    assert!(host.offline.state().session.is_empty());
    assert_eq!(
        host.offline.heap_operations(),
        0,
        "the engine's blocks stay off the heap"
    );
}

/// An agent builds a multisample by command ID, headless, and reads every
/// edit back through the query tool and the document (WP-5.13; the
/// multisampler spec, R30).
#[test]
fn an_agent_builds_a_multisample_by_command_and_reads_it_back() {
    let dir = TempDir::new("multisample");
    let files: Vec<String> = ["Low", "Mid", "High"]
        .iter()
        .map(|name| {
            let path = dir.0.join(format!("{name}.wav"));
            write_stem(&path, 220.0);
            path.to_string_lossy().into_owned()
        })
        .collect();
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    call(
        &mut client,
        "run",
        json!({ "label": "Add a track", "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Keys"}, "kind": "midi", "name": "Keys", "index": 0 } },
        ] }),
    );
    let track = {
        let document = client.transport().shell.editor().document();
        document.tracks().find(|t| t.name() == "Keys").unwrap().id()
    };
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "arrangement.select_tracks", "args": { "tracks": [track] } },
        ] }),
    );
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "device.add", "args": { "device": {
                "kind": { "native": { "device_type": "beatlayer.mosaic" } },
                "name": "Mosaic", "role": "instrument" } } },
        ] }),
    );
    let device = {
        let document = client.transport().shell.editor().document();
        document.track(track).unwrap().devices().next().unwrap().id
    };
    let layers = |client: &mut Client<InProcess>| -> Value {
        let found = call(
            client,
            "query",
            json!({ "path": format!("device {}", device.0.get()) }),
        );
        found["layers"].clone()
    };
    let added = call(
        &mut client,
        "run",
        json!({ "label": "Add the samples", "commands": [
            { "command": "device.add_layers", "args": { "device": device, "paths": files } },
        ] }),
    );
    assert_eq!(added["results"][0]["command"], "device.add_layers");
    let read = layers(&mut client);
    assert_eq!(read["count"], 3);
    let ids: Vec<u64> = read["list"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["id"].as_u64().unwrap())
        .collect();
    // Each edit, by ID, one at a time.
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "device.set_layer_zone", "args": { "device": device, "kind": "key",
                "zones": [{ "layer": ids[0], "zone": { "low": 0, "high": 40, "fade_high": 6 } }] } },
            { "command": "device.update_layers", "args": { "device": device, "layers": [ids[1]],
                "settings": { "root_key": 64, "sustain": "loop" } } },
            { "command": "device.rename_layer", "args": { "device": device, "layer": ids[2], "name": "Top" } },
            { "command": "device.distribute_layers", "args": { "device": device, "layers": [],
                "how": "around_root_key", "kind": "key" } },
            { "command": "device.set_layer_options", "args": { "device": device, "snap": true } },
        ] }),
    );
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "device.set_multisampler_view", "args": { "device": device,
                "tab": "filter_global", "zone_editor": true } },
        ] }),
    );
    let read = layers(&mut client);
    assert_eq!(read["snap"], true);
    let items = read["list"]["items"].as_array().unwrap();
    assert_eq!(items[2]["name"], "Top");
    assert_eq!(items[1]["root_key"], 64);
    assert_eq!(items[1]["sustain"], "loop");
    // Around their root keys (60, 64, 60): the two C3 layers share the lower
    // zone and the E3 layer takes the upper.
    let zones: Vec<(u64, u64)> = items
        .iter()
        .map(|l| {
            (
                l["key"]["low"].as_u64().unwrap(),
                l["key"]["high"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(zones, [(0, 30), (63, 127), (31, 62)]);
    // The views read back in the Device View's state.
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.select", "args": { "device": device } }] }),
    );
    let state = call(
        &mut client,
        "run",
        json!({ "commands": [
        { "command": "device_view.state" }] }),
    );
    let frame = state["results"][0]["result"]["devices"][0]["multisampler"].clone();
    assert_eq!(frame["tab"], "filter_global");
    assert_eq!(frame["layers"], 3);
    // Undo takes the last edit back.
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "edit.undo" }] }),
    );
    assert_eq!(layers(&mut client)["count"], 3);
}

// Spec: racks R17. An agent builds a nested rack by command ID, edits its
// chains and groups and ungroups devices, and reads each result back
// through the query tool's `device` object, chains and their IDs included.
#[test]
fn an_agent_builds_and_reads_back_a_nested_rack() {
    let mut client = Client::new(InProcess::new(Agent::headless(false), Shell::headless()));
    client.initialize("racks").unwrap();
    let audio = default_tracks(&mut client)[2];
    let native = |name: &str| {
        json!({
            "id": {"$new": name}, "name": name, "enabled": true, "params": [],
            "kind": { "native": { "device_type": "test.through" } },
        })
    };

    // A rack of two chains, the second holding a device and a rack of its
    // own, and the chains' mixers, in one call.
    let built = call(
        &mut client,
        "run",
        json!({ "label": "Build a rack", "commands": [
            { "command": "device.insert", "args": { "track": audio, "index": 0, "device": {
                "id": {"$new": "outer"}, "name": "Layers", "enabled": true, "params": [],
                "kind": { "rack": { "kind": "audio_effect" } },
                "chains": [
                    { "id": {"$new": "dry"}, "name": "Dry", "devices": [] },
                    { "id": {"$new": "wet"}, "name": "Wet", "devices": [
                        native("Delay"),
                        {
                            "id": {"$new": "inner"}, "name": "Inner", "enabled": true, "params": [],
                            "kind": { "rack": { "kind": "audio_effect" } },
                            "chains": [{ "id": {"$new": "deep"}, "name": "Deep", "devices": [] }],
                        },
                    ] },
                ],
            } } },
            { "command": "chain.set_gain", "args": { "chain": {"$ref": "wet"}, "volume": 0.5 } },
            { "command": "chain.set_pan", "args": { "chain": {"$ref": "wet"}, "pan": -0.5 } },
            { "command": "chain.set_colour", "args": { "chain": {"$ref": "dry"}, "colour": 7 } },
            { "command": "chain.set_solo", "args": { "chain": {"$ref": "dry"}, "solo": true } },
        ] }),
    );
    assert_eq!(built["ran"], 5);
    let ids = &built["ids"];
    let id = |name: &str| {
        ids[name]
            .as_u64()
            .unwrap_or_else(|| panic!("no `{name}`: {ids}"))
    };
    let (outer, dry, wet, inner, deep, delay) = (
        id("outer"),
        id("dry"),
        id("wet"),
        id("inner"),
        id("deep"),
        id("Delay"),
    );

    // The rack reads back with its chains, their mixers and devices, racks
    // inside racks included.
    let rack = call(
        &mut client,
        "query",
        json!({ "id": outer, "detail": "full" }),
    );
    let chains = rack["chains"].as_array().unwrap();
    assert_eq!(chains.len(), 2);
    assert_eq!(chains[0]["id"].as_u64(), Some(dry));
    assert_eq!(chains[0]["name"], "Dry");
    assert_eq!(chains[0]["colour"], 7);
    assert_eq!(chains[0]["mixer"]["solo"], true);
    assert_eq!(chains[1]["mixer"]["volume"], 0.5);
    assert_eq!(chains[1]["mixer"]["pan"], -0.5);
    let nested = &chains[1]["devices"][1];
    assert_eq!(nested["id"].as_u64(), Some(inner));
    assert_eq!(nested["chains"][0]["id"].as_u64(), Some(deep));
    assert_eq!(
        rack["saved"]["chains"]["total"], 2,
        "the saved form pages chains"
    );

    // A chain is read by its ID, with the rack and track it belongs to, and
    // so is a device inside a rack.
    let chain = call(&mut client, "query", json!({ "id": deep }));
    assert_eq!(chain["name"], "Deep");
    assert_eq!(chain["rack"].as_u64(), Some(inner));
    assert_eq!(chain["track"].as_u64(), Some(audio));
    let device = call(&mut client, "query", json!({ "id": delay }));
    assert_eq!(device["track"].as_u64(), Some(audio));

    // Group and Ungroup by ID: the delay and the inner rack go in a rack of
    // their own, in the wet chain, and come out again.
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "device.group", "args": { "devices": [delay, inner] } },
        ] }),
    );
    let wet_chain = call(&mut client, "query", json!({ "id": wet }));
    let devices = wet_chain["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1);
    let group = devices[0]["id"].as_u64().unwrap();
    assert_eq!(
        devices[0]["chains"][0]["devices"].as_array().unwrap().len(),
        2
    );
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.ungroup", "args": { "device": group } }] }),
    );
    let wet_chain = call(&mut client, "query", json!({ "id": wet }));
    let order: Vec<u64> = wet_chain["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_u64().unwrap())
        .collect();
    assert_eq!(order, [delay, inner]);

    // Deleting a chain is destructive: an agent that isn't autonomous is
    // refused, and the chain stays.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "chain.remove", "args": { "chain": dry } }] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    assert!(refused.text.contains("destructive"), "{}", refused.text);
    let rack = call(&mut client, "query", json!({ "id": outer }));
    assert_eq!(rack["chains"].as_array().unwrap().len(), 2);

    // Undo takes each step back whole.
    let history = call(&mut client, "query", json!({ "path": "history" }));
    assert_eq!(history["undo"]["items"][0], "racks: Ungroup");
}

// Spec: racks R41 (WP-5.7): an agent takes a rack chain's tap as a track's
// input by command ID, reads it back as the track's input, and sees it go
// to No Input with the chain.
#[test]
fn an_agent_takes_a_chain_tap() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("racks").unwrap();
    let tracks = default_tracks(&mut client);
    let (source, tapper) = (tracks[2], tracks[3]);
    let built = call(
        &mut client,
        "run",
        json!({ "label": "Build a rack", "commands": [
            { "command": "device.insert", "args": { "track": source, "index": 0, "device": {
                "id": {"$new": "rack"}, "name": "Layers", "enabled": true, "params": [],
                "kind": { "rack": { "kind": "audio_effect" } },
                "chains": [{ "id": {"$new": "wet"}, "name": "Wet", "devices": [] }],
            } } },
        ] }),
    );
    let wet = built["ids"]["wet"].as_u64().unwrap();
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "routing.set_input", "args": { "track": tapper,
            "input": { "type": "track", "track": source, "tap": "post_fx", "chain": wet } } }] }),
    );

    let read = |client: &mut Client<InProcess>| {
        call(client, "query", json!({ "id": tapper, "detail": "full" }))["saved"]["routing"]["input"]
            .clone()
    };
    assert_eq!(
        read(&mut client),
        json!({ "type": "track", "track": source, "tap": "post_fx", "chain": wet })
    );
    let history = call(&mut client, "query", json!({ "path": "history" }));
    assert_eq!(history["undo"]["items"][0], "racks: Set Input");

    // A chain that isn't on the track is refused.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "routing.set_input", "args": { "track": tapper,
                "input": { "type": "track", "track": tracks[0], "chain": wet } } }] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);

    // Removing the chain leaves No Input; undo brings the tap back.
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "chain.remove", "args": { "chain": wet } }] }),
    );
    assert_eq!(read(&mut client), json!({ "type": "none" }));
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "edit.undo", "args": {} }] }),
    );
    assert_eq!(
        read(&mut client),
        json!({ "type": "track", "track": source, "tap": "post_fx", "chain": wet })
    );
}

// Spec: device-chains R34 and R35, device-framework R14 (WP-5.42): an agent
// adds the framework's test effects, shows and hides its expanded part,
// switches its own menu option, and reads all of it back, by command ID.
#[test]
fn an_agent_expands_a_device_and_switches_its_menu_option_by_command_id() {
    use bl_devices::testing::{self, EFFECTS_INVERT};

    let mut shell = Shell::headless();
    shell.set_device_types(testing::device_types());
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("effects-agent").unwrap();
    let track = default_tracks(&mut client)[2];
    call(
        &mut client,
        "run",
        json!({ "commands": [{
            "command": "arrangement.select_tracks",
            "args": { "tracks": [track] },
        }] }),
    );
    let effects = testing::effects_type();
    let added = call(
        &mut client,
        "run",
        json!({ "commands": [{
            "command": "device.add",
            "args": {
                "track": track,
                "device": { "kind": effects.kind, "name": effects.name, "role": effects.role },
            },
        }] }),
    );
    let device = added["results"][0]["result"]["device"].clone();

    // The expanded part: hidden at first, shown and hidden by command, read
    // back from the Device View's state.
    let frame = |client: &mut Client<InProcess>| {
        let state = call(
            client,
            "run",
            json!({ "commands": [{ "command": "device_view.state" }] }),
        );
        state["results"][0]["result"]["devices"][0].clone()
    };
    let hidden = frame(&mut client);
    assert_eq!(
        (hidden["expandable"].clone(), hidden["expanded"].clone()),
        (json!(true), json!(false))
    );
    let shown = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.expand", "args": { "device": device } }] }),
    );
    assert_eq!(
        shown["results"][0]["result"]["devices"][0]["expanded"],
        json!(true)
    );
    assert_eq!(frame(&mut client)["expanded"], json!(true));
    call(
        &mut client,
        "run",
        json!({ "commands": [{
            "command": "device.expand",
            "args": { "device": device, "expanded": false },
        }] }),
    );
    assert_eq!(frame(&mut client)["expanded"], json!(false));

    // The menu option is a parameter: set by ID, read back as one that
    // lives in the menu.
    call(
        &mut client,
        "run",
        json!({ "commands": [{
            "command": "device.set_param",
            "args": { "device": device, "param": EFFECTS_INVERT, "value": 1.0 },
        }] }),
    );
    let read = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.parameters", "args": { "device": device } }] }),
    );
    let params = read["results"][0]["result"]["params"].as_array().unwrap();
    let invert = params.iter().find(|p| p["name"] == "Invert LFO").unwrap();
    assert_eq!(
        (
            invert["value"].clone(),
            invert["menu_option"].clone(),
            invert["automatable"].clone()
        ),
        (json!(1.0), json!(true), json!(true))
    );
}

// Spec: one-shot-drum-sampler R22 and R23 (WP-5.18): an agent adds Strike,
// reads its parameters with its two menu options, switches one by ID, and
// swaps it to Snapshot, by command ID.
#[test]
fn an_agent_switches_strikes_menu_options_and_swaps_it_to_snapshot_by_command_id() {
    use bl_devices::drum_sampler::{ENV_FOLLOWS_PITCH, PER_NOTE_BEND, drum_sampler_type};

    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("strike-agent").unwrap();
    let track = default_tracks(&mut client)[2];
    call(
        &mut client,
        "run",
        json!({ "commands": [{
            "command": "arrangement.select_tracks",
            "args": { "tracks": [track] },
        }] }),
    );
    let strike = drum_sampler_type();
    let added = call(
        &mut client,
        "run",
        json!({ "commands": [{
            "command": "device.add",
            "args": {
                "track": track,
                "device": { "kind": strike.kind, "name": strike.name, "role": strike.role },
            },
        }] }),
    );
    let device = added["results"][0]["result"]["device"].clone();
    let read = |client: &mut Client<InProcess>, device: &serde_json::Value| {
        let read = call(
            client,
            "run",
            json!({ "commands": [{ "command": "device.parameters", "args": { "device": device } }] }),
        );
        read["results"][0]["result"]["params"].clone()
    };
    let option = |params: &serde_json::Value, name: &str| {
        let param = params
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap()
            .clone();
        (param["value"].clone(), param["menu_option"].clone())
    };
    let params = read(&mut client, &device);
    assert_eq!(
        option(&params, "Enable Per-Note Pitch Bend"),
        (json!(1.0), json!(true)),
        "on in a new device"
    );
    assert_eq!(
        option(&params, "Envelope Follows Pitch"),
        (json!(0.0), json!(true))
    );
    call(
        &mut client,
        "run",
        json!({ "commands": [{
            "command": "device.set_param",
            "args": { "device": device, "param": ENV_FOLLOWS_PITCH, "value": 1.0 },
        }, {
            "command": "device.set_param",
            "args": { "device": device, "param": PER_NOTE_BEND, "value": 0.0 },
        }] }),
    );
    let params = read(&mut client, &device);
    assert_eq!(option(&params, "Envelope Follows Pitch").0, json!(1.0));
    assert_eq!(option(&params, "Enable Per-Note Pitch Bend").0, json!(0.0));

    let swapped = call(
        &mut client,
        "run",
        json!({ "commands": [{
            "command": "device.swap_to_snapshot",
            "args": { "device": device },
        }] }),
    );
    let snapshot = swapped["results"][0]["result"]["device"].clone();
    assert_ne!(snapshot, device);
    let params = read(&mut client, &snapshot);
    let mode = params
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Mode")
        .unwrap();
    assert_eq!(mode["value"], json!(1.0), "Snapshot in One-Shot mode");
}
