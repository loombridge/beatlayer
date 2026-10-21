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

/// An agent reads the EQ Eight's spectrum and the Compressor's gain reduction
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
                device(1, "EQ Eight", bl_devices::eq::EQ),
                device(2, "Compressor", bl_devices::compressor::COMPRESSOR),
                { "command": "device.set_param",
                  "args": { "device": {"$ref": "Tone"}, "param": 0, "value": 1000.0 } },
                { "command": "device.set_param",
                  "args": { "device": {"$ref": "Tone"}, "param": 1, "value": -6.0 } },
                { "command": "device.set_param",
                  "args": { "device": {"$ref": "Compressor"}, "param": 0, "value": -30.0 } },
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

/// The loudest sample the master played since the last call.
fn peak_since(host: &mut EngineHost) -> f32 {
    let peak = host.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    host.samples.clear();
    peak
}

// Spec: channel-eq R1, three-band-eq R1, gate R7, bus-compressor R10,
// device-framework R14 (WP-5.21): an agent adds the channel EQ, the
// three-band EQ, the gate and the bus compressor by their types, sets them
// by ID, reads them back, and keys the gate and the bus compressor from
// another track, in a running engine, at no heap use on the audio thread.
#[test]
fn an_agent_adds_the_eqs_and_dynamics_sets_them_by_id_and_keys_them_from_another_track() {
    use bl_devices::{bus_compressor, channel_eq, gate, three_band_eq};

    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    let mut host = EngineHost::with_registry(256, registry);
    let insert = |track: &str, index: u32, name: &str, device_type: &str| {
        json!({ "command": "device.insert",
                "args": { "track": {"$ref": track}, "index": index,
                          "device": { "id": {"$new": format!("{track} {name}")}, "name": name,
                                      "kind": { "native": { "device_type": device_type } },
                                      "enabled": true, "params": [] } } })
    };
    let set_param = |track: &str, name: &str, param: u32, value: f64| {
        json!({ "command": "device.set_param",
                "args": { "device": {"$ref": format!("{track} {name}")}, "param": param, "value": value } })
    };
    let tone = bl_engine::nodes::TEST_TONE;
    let mut commands = Vec::new();
    for track in ["Key", "Gated", "Bussed", "Equalled"] {
        commands.push(json!({ "command": "track.create",
            "args": { "id": {"$new": track}, "kind": "audio", "name": track, "index": 0 } }));
        commands.push(insert(track, 0, "Tone", tone));
    }
    // The key: 1 kHz at -6 dB. The gated tone is quiet, below the gate's
    // threshold; the bussed one is loud, above the compressor's.
    commands.extend([
        set_param("Key", "Tone", 0, 1_000.0),
        set_param("Key", "Tone", 1, -6.0),
        set_param("Gated", "Tone", 1, -50.0),
        set_param("Bussed", "Tone", 1, -20.0),
        set_param("Equalled", "Tone", 1, -20.0),
    ]);
    host.call(
        "run",
        json!({ "label": "Add tracks", "commands": commands }),
    );
    let track_id = |host: &mut EngineHost, name: &str| {
        let document = host.client.transport().shell.editor().document();
        document.tracks().find(|t| t.name() == name).unwrap().id()
    };
    // The four devices are added by their types, as the browser adds them,
    // each in a call of its own.
    for (track, device_type) in [
        ("Gated", gate::gate_type()),
        ("Bussed", bus_compressor::bus_compressor_type()),
        ("Equalled", channel_eq::channel_eq_type()),
        ("Equalled", three_band_eq::three_band_eq_type()),
    ] {
        let track = track_id(&mut host, track);
        host.call(
            "run",
            json!({ "label": "Add", "commands": [{
            "command": "device.add",
            "args": { "track": track,
                      "device": { "kind": device_type.kind, "name": device_type.name,
                                  "role": device_type.role,
                                  "sidechain": device_type.sidechain } } }] }),
        );
    }
    let ids = {
        let document = host.client.transport().shell.editor().document();
        let device = |track_name: &str, index: usize| {
            let track = document.tracks().find(|t| t.name() == track_name).unwrap();
            track.devices().nth(index).unwrap().id
        };
        [
            device("Gated", 1),
            device("Bussed", 1),
            device("Equalled", 1),
            device("Equalled", 2),
        ]
    };
    let tracks = ["Key", "Gated", "Bussed", "Equalled"].map(|name| track_id(&mut host, name));
    let [shutter, bond, strip, triad] = ids;
    let [key, gated, bussed, equalled] = tracks;
    let set = |host: &mut EngineHost, device, param: bl_model::ParamId, value: f64| {
        host.call(
            "run",
            json!({ "label": "Set", "commands": [
            { "command": "device.set_param",
              "args": { "device": device, "param": param.0, "value": value } } ] }),
        );
    };
    set(&mut host, shutter, gate::THRESHOLD, -30.0);
    set(&mut host, bond, bus_compressor::THRESHOLD, -30.0);
    set(&mut host, bond, bus_compressor::RATIO, 20.0);
    let mute = |host: &mut EngineHost, track, mute: bool| {
        host.call(
            "run",
            json!({ "label": "Mute", "commands": [
            { "command": "mixer.set_mute", "args": { "track": track, "mute": mute } } ] }),
        );
    };
    let listen_to_key = |host: &mut EngineHost, device, track| {
        host.call(
            "run",
            json!({ "label": "Key", "commands": [
            { "command": "device.set_sidechain_input",
              "args": { "device": device,
                        "input": { "type": "track", "track": track, "tap": "post_fx" } } },
            { "command": "device.set_sidechain_enabled",
              "args": { "device": device, "enabled": true } } ] }),
        );
    };
    // Only the key's own track is silent, and one other track plays at a time.
    for track in [key, bussed, equalled] {
        mute(&mut host, track, true);
    }
    let mut clock = 0;
    // Half a second to settle every change, then half a second to measure.
    let mut play = |host: &mut EngineHost| {
        host.play_to(clock + 24_000);
        host.samples.clear();
        clock += 48_000;
        host.play_to(clock);
        peak_since(host)
    };

    // The gate shuts on its own quiet input; the key track opens it; Listen
    // plays the key itself.
    assert!(
        play(&mut host) < 1e-6,
        "shut: the quiet tone is under the threshold"
    );
    listen_to_key(&mut host, shutter, key);
    let opened = play(&mut host);
    assert!((0.002..0.0045).contains(&opened), "open: {opened}");
    host.call("run", json!({ "label": "Listen", "commands": [
        { "command": "device.set_sidechain_listen", "args": { "device": shutter, "listen": true } } ] }));
    let heard = play(&mut host);
    assert!(
        (0.45..0.52).contains(&heard),
        "Listen plays the key: {heard}"
    );

    // The bus compressor squeezes the loud tone, and squeezes it harder
    // while the key track drives it.
    mute(&mut host, gated, true);
    mute(&mut host, bussed, false);
    let alone = play(&mut host);
    assert!(
        alone > 0.02 && alone < 0.06,
        "compressing its own input: {alone}"
    );
    listen_to_key(&mut host, bond, key);
    let keyed = play(&mut host);
    assert!(
        keyed < 0.5 * alone,
        "ducked by the key: {keyed} against {alone}"
    );

    // The EQs pass the tone flat; their parameters are set by ID and read
    // back with how they read.
    mute(&mut host, bussed, true);
    mute(&mut host, equalled, false);
    let flat = play(&mut host);
    assert!((flat - 0.1).abs() < 0.005, "flat: {flat}");
    host.call(
        "run",
        json!({ "label": "Set", "commands": [
        { "command": "device.set_param",
          "args": { "device": strip, "param": channel_eq::LOW.0, "value": -9.0 } },
        { "command": "device.set_param",
          "args": { "device": triad, "param": three_band_eq::FREQ_LOW.0, "value": 400.0 } },
        { "command": "device.set_param",
          "args": { "device": triad, "param": three_band_eq::SLOPE.0, "value": 1.0 } } ] }),
    );
    let read = |host: &mut EngineHost, device, name: &str| {
        let reply = host.call(
            "run",
            json!({ "label": "Read", "commands": [
            { "command": "device.parameters", "args": { "device": device } } ] }),
        );
        let params = reply["results"][0]["result"]["params"].clone();
        let param = params
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .cloned();
        param.unwrap_or_else(|| panic!("no {name} in {params}"))
    };
    assert_eq!(read(&mut host, strip, "Low")["value"], json!(-9.0));
    assert_eq!(read(&mut host, triad, "FreqLo")["value"], json!(400.0));
    let slope = read(&mut host, triad, "Slope");
    assert_eq!(
        (slope["value"].clone(), slope["text"].clone()),
        (json!(1.0), json!("48 dB"))
    );
    assert_eq!(read(&mut host, shutter, "Threshold")["value"], json!(-30.0));
    assert_eq!(
        read(&mut host, bond, "Oversampling")["menu_option"],
        json!(true)
    );
    assert_eq!(host.offline.heap_operations(), 0);
}

// Spec: drum-bus R1, device-framework R14 (WP-5.22b): an agent adds Drum Buss
// by its type, sets its parameters by ID and reads them back.
#[test]
fn an_agent_adds_drum_buss_sets_it_by_id_and_reads_it_back() {
    use bl_devices::drum_bus::{BOOM, DRUM_BUSS, FREQ, TYPE};

    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    let mut host = EngineHost::with_registry(256, registry);
    host.call(
        "run",
        json!({ "label": "Build", "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Drums"}, "kind": "audio", "name": "Drums", "index": 0 } },
            { "command": "device.insert",
              "args": { "track": {"$ref": "Drums"}, "index": 0,
                        "device": { "id": {"$new": "Drums buss"}, "name": "Drum Buss",
                                    "kind": { "native": { "device_type": DRUM_BUSS } },
                                    "enabled": true, "params": [] } } } ] }),
    );
    let device = json!({ "$ref": "Drums buss" });
    host.call(
        "run",
        json!({ "label": "Set", "commands": [
            { "command": "device.set_param",
              "args": { "device": device, "param": TYPE.0, "value": 2.0 } },
            { "command": "device.set_param",
              "args": { "device": device, "param": FREQ.0, "value": 90.0 } },
            { "command": "device.set_param",
              "args": { "device": device, "param": BOOM.0, "value": 50.0 } } ] }),
    );
    let reply = host.call(
        "run",
        json!({ "label": "Read", "commands": [
            { "command": "device.parameters", "args": { "device": device } } ] }),
    );
    let params = reply["results"][0]["result"]["params"].clone();
    assert_eq!(params.as_array().unwrap().len(), 14);
    let find = |name: &str| {
        params
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no {name} in {params}"))
    };
    assert_eq!(find("Type")["text"], json!("Hard"));
    assert_eq!(find("Freq")["value"], json!(90.0));
    assert_eq!(find("Boom")["value"], json!(50.0));
    assert_eq!(find("Output")["text"], json!("0.00 dB"));
    assert_eq!(host.offline.heap_operations(), 0);
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
            .starts_with("tracks:\n  items[11]{clips,delay.unit,delay.value,devices,frozen,group,id,keep_monitoring_latency,kind,mixer.arm,mixer.mute,mixer.pan,mixer.pan_mode,mixer.solo,mixer.split_left,mixer.split_right,mixer.volume,mixer.volume_db,name,tracks_inside}:"),
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
            ..LaunchSettings::default()
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
                "name": "Sampler", "role": "instrument" } } },
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

// Spec: racks R36. An agent zones the chains of an instrument rack, moves
// its chain selector, and reads the zones, the selector and the editor shown
// back: by command ID, one undo step each.
#[test]
fn an_agent_zones_a_rack_and_reads_it_back() {
    let mut client = Client::new(InProcess::new(Agent::headless(false), Shell::headless()));
    client.initialize("zones").unwrap();
    let tracks = default_tracks(&mut client);
    let (midi, audio) = (tracks[0], tracks[2]);
    let built = call(
        &mut client,
        "run",
        json!({ "label": "Build a rack", "commands": [
            { "command": "device.insert", "args": { "track": midi, "index": 0, "device": {
                "id": {"$new": "rack"}, "name": "Layers", "enabled": true, "params": [],
                "role": "instrument",
                "kind": { "rack": { "kind": "instrument" } },
                "chains": [
                    { "id": {"$new": "low"}, "name": "Low", "devices": [] },
                    { "id": {"$new": "high"}, "name": "High", "devices": [] },
                ],
            } } },
        ] }),
    );
    let (rack, low, high) = (
        built["ids"]["rack"].as_u64().unwrap(),
        built["ids"]["low"].as_u64().unwrap(),
        built["ids"]["high"].as_u64().unwrap(),
    );
    let zones = |client: &mut Client<_>, chain: u64| {
        call(client, "query", json!({ "id": chain }))["zones"].clone()
    };
    // A new chain passes everything.
    assert_eq!(
        zones(&mut client, low)["key"],
        json!({ "low": 0, "high": 127, "fade_low": 0, "fade_high": 0 })
    );

    let ran = call(
        &mut client,
        "run",
        json!({ "label": "Split the keys", "commands": [
            { "command": "chain.set_zone", "args": { "chain": low, "kind": "key",
                "zone": { "low": 0, "high": 59, "fade_high": 12 } } },
            { "command": "chain.set_zone", "args": { "chain": high, "kind": "key",
                "zone": { "low": 48, "high": 127, "fade_low": 12 } } },
            { "command": "chain.set_zone", "args": { "chain": high, "kind": "select",
                "zone": { "low": 1, "high": 4 } } },
            { "command": "device.set_param", "args": { "device": rack, "param": 0, "value": 1 } },
            { "command": "device.set_param", "args": { "device": rack, "param": 1, "value": 1 } },
        ] }),
    );
    assert_eq!(ran["ran"], 5);
    let one = |client: &mut Client<_>, command: &str, args: Value| {
        call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )["results"][0]["result"]
            .clone()
    };
    one(
        &mut client,
        "rack.set_zone_editor",
        json!({ "rack": rack, "editor": "key" }),
    );
    one(
        &mut client,
        "rack.set_auto_select",
        json!({ "rack": rack, "on": true }),
    );
    assert_eq!(
        zones(&mut client, low)["key"],
        json!({ "low": 0, "high": 59, "fade_low": 0, "fade_high": 12 })
    );
    assert_eq!(
        zones(&mut client, high)["select"],
        json!({ "low": 1, "high": 4, "fade_low": 0, "fade_high": 0 })
    );
    // The selector and Chain Selector Filters MIDI Ctrl are the rack's
    // parameters; the editor and Auto Select are in the Device View's state.
    let parameters = one(&mut client, "device.parameters", json!({ "device": rack }));
    let value = |name: &str| {
        parameters["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("no {name} in {parameters}"))["value"]
            .clone()
    };
    assert_eq!(value("Chain Selector"), json!(1.0));
    assert_eq!(value("Chain Selector Filters MIDI Ctrl"), json!(1.0));
    one(
        &mut client,
        "arrangement.select_tracks",
        json!({ "tracks": [midi] }),
    );
    let state = one(&mut client, "device_view.state", json!({})).to_string();
    assert!(state.contains("\"zone_editor\":\"key\""), "{state}");
    assert!(state.contains("\"auto_select\":true"), "{state}");

    // A zone the rack doesn't give its chains is refused with a reason.
    let built = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "device.insert", "args": { "track": audio, "index": 0, "device": {
                "id": {"$new": "effects"}, "name": "Layers", "enabled": true, "params": [],
                "kind": { "rack": { "kind": "audio_effect" } },
                "chains": [{ "id": {"$new": "wet"}, "name": "Wet", "devices": [] }],
            } } },
        ] }),
    );
    let wet = built["ids"]["wet"].as_u64().unwrap();
    let refused = client
        .call(
            "run",
            json!({ "commands": [
                { "command": "chain.set_zone", "args": { "chain": wet, "kind": "key",
                    "zone": { "low": 0, "high": 59 } } },
            ] }),
        )
        .expect("the call reaches the server");
    assert!(
        refused.ok().is_err(),
        "an audio effect rack's chain has no key zone"
    );
}

// Spec: racks R27, R28.
#[test]
fn an_agent_folds_a_racks_chains_out_and_reads_the_strips_back() {
    let mut client = Client::new(InProcess::new(Agent::headless(false), Shell::headless()));
    client.initialize("racks").unwrap();
    let midi = default_tracks(&mut client)[0];
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        let out = call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        );
        out["results"][0]["result"].clone()
    };
    // An instrument rack of two chains on the MIDI track.
    let built = call(
        &mut client,
        "run",
        json!({ "label": "Build a rack", "commands": [
            { "command": "device.insert", "args": { "track": midi, "index": 0, "device": {
                "id": {"$new": "rack"}, "name": "Instrument Rack", "enabled": true, "params": [],
                "role": "instrument", "kind": { "rack": { "kind": "instrument" } },
                "chains": [
                    { "id": {"$new": "keys"}, "name": "Keys", "devices": [] },
                    { "id": {"$new": "bass"}, "name": "Bass", "devices": [] },
                ],
            } } },
        ] }),
    );
    let ids = &built["ids"];
    let rack = ids["rack"].as_u64().unwrap();
    let keys = ids["keys"].as_u64().unwrap();
    // The chains are folded out as strips, and the state reports them.
    let folded = run(
        &mut client,
        "mixer_view.fold_chains",
        json!({ "rack": rack }),
    );
    let strips = folded["chains"].as_array().unwrap();
    assert_eq!(strips.len(), 2);
    assert_eq!(strips[0]["chain"].as_u64(), Some(keys));
    assert_eq!(strips[0]["name"], "Keys");
    assert_eq!(strips[0]["rack"].as_u64(), Some(rack));
    assert_eq!(strips[0]["track"].as_u64(), Some(midi));
    // The chain's mixer is set by ID, and reads back in the query.
    run(
        &mut client,
        "chain.set_gain",
        json!({ "chain": keys, "volume": 0.5 }),
    );
    let chain = call(
        &mut client,
        "query",
        json!({ "id": keys, "detail": "full" }),
    );
    assert_eq!(chain["mixer"]["volume"], 0.5);
    // And the strip shows the new value.
    let state = run(&mut client, "mixer_view.state", json!({}));
    assert_eq!(state["chains"][0]["volume"], 0.5);
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

// Spec: racks R19 to R26. An agent runs what a person does in a rack's frame
// by command ID: the views, the chain selection, solo, the drop area's new
// chain, a chain dropped on the track, and the edit keys' commands, and reads
// each result back from `device_view.state` and the query tool.
#[test]
fn an_agent_drives_a_racks_frame_by_command_id() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("rack-view").unwrap();
    let audio = default_tracks(&mut client)[2];
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        let out = call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        );
        out["results"][0]["result"].clone()
    };
    run(
        &mut client,
        "arrangement.select_tracks",
        json!({ "tracks": [audio] }),
    );
    let through = json!({
        "id": {"$new": "x"}, "name": "Through", "enabled": true, "params": [],
        "kind": { "native": { "device_type": "test.through" } },
    });
    let built = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.insert", "args": { "track": audio, "index": 0, "device": {
            "id": {"$new": "rack"}, "name": "Layers", "enabled": true, "params": [],
            "kind": { "rack": { "kind": "audio_effect" } },
            "chains": [
                { "id": {"$new": "a"}, "name": "A", "devices": [] },
                { "id": {"$new": "b"}, "name": "B", "devices": [through] },
                { "id": {"$new": "c"}, "name": "C", "devices": [] },
            ],
        } } }] }),
    );
    let id = |name: &str| {
        built["ids"][name]
            .as_u64()
            .unwrap_or_else(|| panic!("no `{name}`"))
    };
    let (rack, a, b, c) = (id("rack"), id("a"), id("b"), id("c"));
    let frame = |client: &mut Client<InProcess>| {
        run(client, "device_view.state", json!({}))["devices"][0]["rack"].clone()
    };

    // Views: hidden and shown, read back; not an undo step.
    let before = call(&mut client, "query", json!({ "path": "history" }));
    run(
        &mut client,
        "rack.set_view",
        json!({ "rack": rack, "view": "chain_list", "shown": false }),
    );
    assert_eq!(frame(&mut client)["views"]["chain_list"], json!(false));
    run(
        &mut client,
        "rack.set_view",
        json!({ "rack": rack, "view": "chain_list" }),
    );
    assert_eq!(frame(&mut client)["views"]["chain_list"], json!(true));
    assert_eq!(
        call(&mut client, "query", json!({ "path": "history" })),
        before
    );

    // Selection, as a click, a range, and a step.
    run(&mut client, "chain.select", json!({ "chain": a }));
    run(
        &mut client,
        "chain.select",
        json!({ "chain": c, "mode": "range" }),
    );
    assert_eq!(frame(&mut client)["selected_chains"], json!([a, b, c]));
    assert_eq!(frame(&mut client)["note"], json!("3 chains selected"));
    run(&mut client, "chain.select", json!({ "mode": "previous" }));
    assert_eq!(frame(&mut client)["selected_chains"], json!([a]));

    // Solo is read back from the chain.
    run(&mut client, "chain.solo", json!({ "chain": b }));
    run(
        &mut client,
        "chain.solo",
        json!({ "chain": c, "add": true }),
    );
    let rack_now = call(&mut client, "query", json!({ "id": rack }));
    let solos: Vec<bool> = rack_now["chains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|chain| chain["mixer"]["solo"] == json!(true))
        .collect();
    assert_eq!(solos, [false, true, true]);
    run(&mut client, "chain.hot_swap", json!({ "chain": a }));
    assert_eq!(frame(&mut client)["chains"][0]["hot_swap"], json!(true));

    // The drop area's new chain, and a chain dropped on the track.
    let created = run(&mut client, "chain.create", json!({ "rack": rack }));
    assert_eq!(created["chains"].as_array().unwrap().len(), 1);
    let moved = run(
        &mut client,
        "chain.drop_on_track",
        json!({ "chain": b, "track": audio }),
    );
    assert!(
        moved["devices"].as_array().unwrap().len() >= 2,
        "the device joined the track's chain"
    );
    assert_eq!(
        call(&mut client, "query", json!({ "id": rack }))["chains"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    // The edit keys' commands act on the selection.
    run(&mut client, "chain.select", json!({ "chain": a }));
    run(&mut client, "chain.duplicate", json!({}));
    assert_eq!(
        call(&mut client, "query", json!({ "id": rack }))["chains"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    run(&mut client, "chain.delete", json!({}));
    assert_eq!(
        call(&mut client, "query", json!({ "id": rack }))["chains"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

// Spec: racks R42 to R47 (WP-5.8): an agent saves a rack and a chain as
// presets, loads both back, and hot-swaps a device with the rack preset, by
// command ID, and reads each result back from the track's devices and
// `device_view.state`.
#[test]
fn an_agent_saves_and_loads_a_rack_preset() {
    let dir = TempDir::new("rack-presets");
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("rack-presets").unwrap();
    let audio = default_tracks(&mut client)[2];
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        let out = call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        );
        out["results"][0]["result"].clone()
    };
    run(
        &mut client,
        "arrangement.select_tracks",
        json!({ "tracks": [audio] }),
    );
    let through = json!({
        "id": {"$new": "x"}, "name": "Through", "enabled": true, "params": [],
        "kind": { "native": { "device_type": "test.through" } },
    });
    let built = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.insert", "args": { "track": audio, "index": 0, "device": {
            "id": {"$new": "rack"}, "name": "Layers", "enabled": true, "params": [],
            "kind": { "rack": { "kind": "audio_effect" } },
            "chains": [
                { "id": {"$new": "a"}, "name": "Wide", "devices": [through] },
                { "id": {"$new": "b"}, "name": "Narrow", "devices": [] },
            ],
        } } }] }),
    );
    let id = |name: &str| built["ids"][name].as_u64().unwrap();
    let (rack, a) = (id("rack"), id("a"));
    let devices = |client: &mut Client<InProcess>| -> Vec<Value> {
        call(client, "query", json!({ "id": audio }))["devices"]
            .as_array()
            .unwrap()
            .clone()
    };

    // Saved: the file is named, the set is unchanged.
    let rack_file = dir.0.join("Layers.blpreset");
    let chain_file = dir.0.join("Wide.blpreset");
    let history = call(&mut client, "query", json!({ "path": "history" }));
    let saved = run(
        &mut client,
        "device.save_preset",
        json!({ "device": rack, "path": rack_file }),
    );
    assert_eq!(saved["name"], "Layers");
    let saved = run(
        &mut client,
        "chain.save_preset",
        json!({ "chain": a, "path": chain_file }),
    );
    assert_eq!(saved["name"], "Wide");
    assert!(rack_file.is_file() && chain_file.is_file());
    assert_eq!(
        call(&mut client, "query", json!({ "path": "history" })),
        history
    );

    // Loaded back: a second rack with the same chains, new IDs, and a chain
    // preset added to the first as a third chain.
    let added = run(
        &mut client,
        "device.add_preset",
        json!({ "path": rack_file, "track": audio }),
    );
    let copy = added["device"].as_u64().unwrap();
    assert_ne!(copy, rack);
    let copy_chains = call(&mut client, "query", json!({ "id": copy }))["chains"].clone();
    let names: Vec<&str> = copy_chains
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Wide", "Narrow"]);
    let loaded = run(
        &mut client,
        "chain.load_preset",
        json!({ "path": chain_file, "rack": rack }),
    );
    assert_eq!(loaded["chains"].as_array().unwrap().len(), 1);
    let chains = call(&mut client, "query", json!({ "id": rack }))["chains"].clone();
    assert_eq!(chains.as_array().unwrap().len(), 3);
    assert_eq!(chains[2]["name"], "Wide");

    // Hot-swapped: the first rack is replaced by the preset's, and the
    // swap stays on for the replacement.
    run(&mut client, "device.hot_swap", json!({ "device": rack }));
    let swapped = run(
        &mut client,
        "device.replace",
        json!({ "preset": rack_file }),
    );
    let new = swapped["device"].as_u64().unwrap();
    assert_ne!(new, rack);
    let now: Vec<u64> = devices(&mut client)
        .iter()
        .map(|d| d["id"].as_u64().unwrap())
        .collect();
    assert_eq!(now, [new, copy]);
    let state = run(&mut client, "device_view.state", json!({}));
    assert_eq!(state["hot_swap"]["device"], json!(new));
    assert_eq!(state["hot_swap"]["label"], json!("Swapping Audio Effect"));

    // Newer or damaged presets are refused and change nothing.
    let damaged = dir.0.join("Damaged.blpreset");
    let text = fs::read_to_string(&rack_file).unwrap();
    fs::write(&damaged, &text[..text.len() / 2]).unwrap();
    let before = devices(&mut client).len();
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "device.add_preset", "args": { "path": damaged } }] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    assert_eq!(devices(&mut client).len(), before);
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

// Spec: auto-filter R15, device-framework R14 (WP-5.20): an agent inserts
// Auto Filter by its type, sets its parameters by ID, including the two menu
// options, and reads every one back with its descriptor.
#[test]
fn an_agent_inserts_sweep_by_its_type_and_sets_and_reads_its_parameters_by_id() {
    use bl_devices::auto_filter::{
        AUTO_FILTER, FORMANT, FREQ, HI_QUALITY, LFO_AMOUNT, MONO_SIDECHAIN, TYPE,
    };

    let mut shell = Shell::headless();
    shell.set_device_types(bl_devices::device_types());
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("sweep-agent").unwrap();
    let track = default_tracks(&mut client)[2];
    let set = |param: bl_model::ParamId, value: f64| {
        json!({ "command": "device.set_param",
                "args": { "device": {"$ref": "Auto Filter"}, "param": param, "value": value } })
    };
    let reply = call(
        &mut client,
        "run",
        json!({ "label": "Add Auto Filter", "commands": [
            { "command": "device.insert",
              "args": { "track": track, "index": 0,
                        "device": { "id": {"$new": "Auto Filter"}, "name": "Auto Filter",
                                    "kind": { "native": { "device_type": AUTO_FILTER } },
                                    "enabled": true, "params": [] } } },
            set(TYPE, 9.0),
            set(FORMANT, 50.0),
            set(FREQ, 440.0),
            set(LFO_AMOUNT, 25.0),
            set(HI_QUALITY, 1.0),
        ] }),
    );
    let device = reply["ids"]["Auto Filter"].clone();
    let read = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.parameters", "args": { "device": device } }] }),
    );
    let params = read["results"][0]["result"]["params"].as_array().unwrap();
    assert_eq!(params.len(), 42, "every parameter is listed");
    let get = |id: bl_model::ParamId| {
        params
            .iter()
            .find(|p| p["id"] == json!(id.0))
            .unwrap_or_else(|| panic!("parameter {id:?}"))
            .clone()
    };
    let values: Vec<_> = [TYPE, FORMANT, FREQ, LFO_AMOUNT, HI_QUALITY]
        .iter()
        .map(|id| get(*id)["value"].clone())
        .collect();
    assert_eq!(
        values,
        [
            json!(9.0),
            json!(50.0),
            json!(440.0),
            json!(25.0),
            json!(1.0)
        ]
    );
    // The menu options are marked, and Mono Sidechain starts on.
    assert_eq!(get(HI_QUALITY)["menu_option"], json!(true));
    assert_eq!(get(MONO_SIDECHAIN)["menu_option"], json!(true));
    assert_eq!(get(MONO_SIDECHAIN)["value"], json!(1.0));
    assert!(params.iter().all(|p| p["automatable"] == json!(true)));
}

// Spec: pitch-and-frequency-shifter R13, routing R33 and R35 (WP-5.27): an
// agent inserts Shifter by its type with its MIDI sidechain, sets its
// parameters by ID, points its MIDI source at a MIDI track and reads both
// back; the source goes when that track does.
#[test]
fn an_agent_inserts_veer_sets_its_parameters_and_points_its_midi_sidechain_at_a_track() {
    use bl_devices::shifter::{COARSE, MODE, PITCH_SOURCE, SHIFT, SHIFTER};

    let mut shell = Shell::headless();
    shell.set_device_types(bl_devices::device_types());
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("veer-agent").unwrap();
    let tracks = default_tracks(&mut client);
    let (keys, audio) = (tracks[0], tracks[2]);
    let reply = call(
        &mut client,
        "run",
        json!({ "label": "Add Shifter", "commands": [
            { "command": "device.add",
              "args": { "track": audio,
                        "device": { "kind": { "native": { "device_type": SHIFTER } },
                                    "name": "Shifter", "role": "audio_effect",
                                    "midi_sidechain": true } } },
        ] }),
    );
    let device = reply["results"][0]["result"]["device"].clone();
    let set = |param: bl_model::ParamId, value: f64| {
        json!({ "command": "device.set_param",
                "args": { "device": device, "param": param, "value": value } })
    };
    call(
        &mut client,
        "run",
        json!({ "label": "Set Shifter", "commands": [
            set(MODE, 1.0),
            set(SHIFT, 123.0),
            set(COARSE, -7.0),
            set(PITCH_SOURCE, 1.0),
            { "command": "device.set_midi_sidechain_source",
              "args": { "device": device, "track": keys } },
        ] }),
    );
    let read = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.parameters", "args": { "device": device } }] }),
    );
    let params = read["results"][0]["result"]["params"].as_array().unwrap();
    assert_eq!(params.len(), 34, "every parameter is listed");
    let value = |id: bl_model::ParamId| {
        params
            .iter()
            .find(|p| p["id"] == json!(id.0))
            .unwrap_or_else(|| panic!("parameter {id:?}"))["value"]
            .clone()
    };
    assert_eq!(
        [MODE, SHIFT, COARSE, PITCH_SOURCE].map(value),
        [json!(1.0), json!(123.0), json!(-7.0), json!(1.0)]
    );
    assert!(params.iter().all(|p| p["automatable"] == json!(true)));

    // The source reads back from the device, and goes with its track.
    let found = call(
        &mut client,
        "query",
        json!({ "path": format!("device {}", device.as_u64().unwrap()), "detail": "full" }),
    );
    assert_eq!(found["saved"]["midi_sidechain"]["source"], json!(keys));
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "track.delete", "args": { "track": keys } }] }),
    );
    let found = call(
        &mut client,
        "query",
        json!({ "path": format!("device {}", device.as_u64().unwrap()), "detail": "full" }),
    );
    assert!(
        found["saved"]["midi_sidechain"].get("source").is_none(),
        "{found}"
    );
    // An audio track isn't a source.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "device.set_midi_sidechain_source",
                                    "args": { "device": device, "track": audio } }] }),
        )
        .unwrap();
    assert!(refused.ok().is_err());
}

// Spec: one-shot-drum-sampler R22 and R23 (WP-5.18): an agent adds Drum Sampler,
// reads its parameters with its two menu options, switches one by ID, and
// swaps it to Simpler, by command ID.
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
    assert_eq!(mode["value"], json!(1.0), "Simpler in One-Shot mode");
}

// Spec: beat-repeat R14, device-framework R14 (WP-5.26): an agent inserts
// Beat Repeat by its type, sets its parameters by ID and reads every one back
// with its descriptor.
#[test]
fn an_agent_inserts_stutter_by_its_type_and_sets_and_reads_its_parameters_by_id() {
    use bl_devices::beat_repeat::{BEAT_REPEAT, CHANCE, GRID, INTERVAL, MODE, REPEAT};

    let mut shell = Shell::headless();
    shell.set_device_types(bl_devices::device_types());
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("stutter-agent").unwrap();
    let track = default_tracks(&mut client)[2];
    let set = |param: bl_model::ParamId, value: f64| {
        json!({ "command": "device.set_param",
                "args": { "device": {"$ref": "Beat Repeat"}, "param": param, "value": value } })
    };
    let reply = call(
        &mut client,
        "run",
        json!({ "label": "Add Beat Repeat", "commands": [
            { "command": "device.insert",
              "args": { "track": track, "index": 0,
                        "device": { "id": {"$new": "Beat Repeat"}, "name": "Beat Repeat",
                                    "kind": { "native": { "device_type": BEAT_REPEAT } },
                                    "enabled": true, "params": [] } } },
            set(INTERVAL, 3.0),
            set(CHANCE, 25.0),
            set(GRID, 6.0),
            set(MODE, 2.0),
            set(REPEAT, 1.0),
        ] }),
    );
    let device = reply["ids"]["Beat Repeat"].clone();
    let read = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.parameters", "args": { "device": device } }] }),
    );
    let params = read["results"][0]["result"]["params"].as_array().unwrap();
    assert_eq!(params.len(), 17, "every parameter is listed");
    let get = |id: bl_model::ParamId| {
        params
            .iter()
            .find(|p| p["id"] == json!(id.0))
            .unwrap_or_else(|| panic!("parameter {id:?}"))
            .clone()
    };
    let values: Vec<_> = [INTERVAL, CHANCE, GRID, MODE, REPEAT]
        .iter()
        .map(|id| get(*id)["value"].clone())
        .collect();
    assert_eq!(
        values,
        [json!(3.0), json!(25.0), json!(6.0), json!(2.0), json!(1.0)]
    );
    assert!(params.iter().all(|p| p["automatable"] == json!(true)));
}

// Spec: filter-delay R9, grain-delay R8, alignment-delay R8 (WP-5.25): an
// agent inserts the three delays by command ID, sets parameters by ID,
// reads them back with their descriptors, saves the set and reopens it
// with every value, and moves Grain Delay's X–Y axis as one undo step.
#[test]
fn an_agent_inserts_sets_and_reads_the_three_delays_by_command_id() {
    use bl_devices::alignment_delay::{self, ALIGNMENT_DELAY};
    use bl_devices::filter_delay::{self, FILTER_DELAY};
    use bl_devices::grain_delay::{self, GRAIN_DELAY};
    use bl_model::{DeviceId, ParamId};

    let dir = TempDir::new("delays");
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("delays").unwrap();
    let track = default_tracks(&mut client)[2];
    let kinds = [
        ("trident", FILTER_DELAY),
        ("scatter", GRAIN_DELAY),
        ("plumb", ALIGNMENT_DELAY),
    ];
    let inserts: Vec<Value> = kinds
        .iter()
        .enumerate()
        .map(|(index, (name, kind))| {
            json!({ "command": "device.insert", "args": { "track": track, "index": index, "device": {
                "id": {"$new": name}, "name": name, "enabled": true, "params": [],
                "kind": { "native": { "device_type": kind } },
            } } })
        })
        .collect();
    let inserted = call(&mut client, "run", json!({ "commands": inserts }));
    let id = |name: &str| inserted["ids"][name].as_u64().unwrap();

    // Parameters by ID, one value in each device that isn't its default.
    let sets = [
        (
            "trident",
            filter_delay::line_param(1, filter_delay::FREQ),
            2_500.0,
        ),
        ("trident", filter_delay::FEEDBACK, 35.0),
        ("scatter", grain_delay::PITCH, 12.0),
        ("scatter", grain_delay::SPRAY, 120.0),
        ("plumb", alignment_delay::MODE, 2.0),
        (
            "plumb",
            alignment_delay::side_param(0, alignment_delay::METRES),
            3.43,
        ),
    ];
    let commands: Vec<Value> = sets
        .iter()
        .map(|(name, param, value)| {
            json!({ "command": "device.set_param",
                    "args": { "device": id(name), "param": param.0, "value": value } })
        })
        .collect();
    call(&mut client, "run", json!({ "commands": commands }));

    let read = |client: &mut Client<InProcess>, name: &str| {
        let read = call(
            client,
            "run",
            json!({ "commands": [{ "command": "device.parameters", "args": { "device": id(name) } }] }),
        );
        read["results"][0]["result"]["params"].clone()
    };
    let value_of = |params: &Value, param: ParamId| {
        params
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == param.0)
            .unwrap_or_else(|| panic!("parameter {}", param.0))["value"]
            .clone()
    };
    for (name, param, value) in &sets {
        let params = read(&mut client, name);
        assert_eq!(
            value_of(&params, *param),
            json!(value),
            "{name} {}",
            param.0
        );
    }
    assert_eq!(read(&mut client, "trident").as_array().unwrap().len(), 32);
    assert_eq!(read(&mut client, "scatter").as_array().unwrap().len(), 12);
    assert_eq!(read(&mut client, "plumb").as_array().unwrap().len(), 14);

    // The X–Y axis is a parameter: one undo step puts it back.
    let axis =
        |client: &mut Client<InProcess>| value_of(&read(client, "scatter"), grain_delay::X_AXIS);
    assert_eq!(axis(&mut client), json!(0.0), "Delay Time to start");
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.set_param", "args": {
            "device": id("scatter"), "param": grain_delay::X_AXIS.0, "value": 4.0 } }] }),
    );
    assert_eq!(axis(&mut client), json!(4.0), "Spray");
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "edit.undo", "args": {} }] }),
    );
    assert_eq!(axis(&mut client), json!(0.0), "one undo step");
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.set_param", "args": {
            "device": id("scatter"), "param": grain_delay::X_AXIS.0, "value": 4.0 } }] }),
    );

    // Saved and reopened, every value comes back.
    let saved = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as",
                               "args": { "path": dir.0.join("Delays.blset").to_string_lossy() } }] }),
    );
    let (project, _) = bl_project::load(saved_path(&saved)).unwrap();
    let reopened = |name: &str, param: ParamId| {
        let device = project
            .document
            .device(DeviceId(bl_core::id::Id::new(id(name)).unwrap()))
            .unwrap_or_else(|| panic!("{name} is in the set"));
        device.params.get(&param).copied()
    };
    for (name, param, value) in &sets {
        assert_eq!(reopened(name, *param), Some(*value), "{name} {}", param.0);
    }
    assert_eq!(reopened("scatter", grain_delay::X_AXIS), Some(4.0));
}

// Spec: digital-degradation R9, bit-crusher R10, vinyl-distortion R8
// (WP-5.24): an agent adds Erosion, Redux and Vinyl Distortion, sets a parameter of each
// by ID, reads them back, and finds them as they were when the set is saved
// and reopened.
#[test]
fn an_agent_sets_the_lo_fi_effects_by_id_and_finds_them_after_a_save_and_reopen() {
    use bl_devices::{crusher, degrade, wax};

    let dir = TempDir::new("lo-fi");
    let mut client = bl_cli_mcp(&["--autonomous"]);
    let track = default_tracks(&mut client)[2];
    call(
        &mut client,
        "run",
        json!({ "commands": [{
            "command": "arrangement.select_tracks",
            "args": { "tracks": [track] },
        }] }),
    );
    // Each device, with one parameter changed from its default.
    let devices = [
        (degrade::degrade_type(), degrade::STEREO_WIDTH, 80.0),
        (crusher::crusher_type(), crusher::BITS, 5.0),
        (wax::wax_type(), wax::CRACKLE_DENSITY, 40.0),
    ];
    let mut ids = Vec::new();
    for (device_type, param, value) in &devices {
        let added = call(
            &mut client,
            "run",
            json!({ "commands": [{
                "command": "device.add",
                "args": { "track": track, "device": {
                    "kind": device_type.kind, "name": device_type.name, "role": device_type.role,
                } },
            }] }),
        );
        let device = added["results"][0]["result"]["device"].clone();
        call(
            &mut client,
            "run",
            json!({ "commands": [{
                "command": "device.set_param",
                "args": { "device": device, "param": param, "value": value },
            }] }),
        );
        ids.push(device);
    }
    let read = |client: &mut Client<Stdio>, device: &Value| {
        let read = call(
            client,
            "run",
            json!({ "commands": [{ "command": "device.parameters", "args": { "device": device } }] }),
        );
        read["results"][0]["result"]["params"].clone()
    };
    let before: Vec<Value> = ids.iter().map(|id| read(&mut client, id)).collect();
    for (params, (device_type, param, value)) in before.iter().zip(&devices) {
        let params = params.as_array().unwrap();
        assert_eq!(params.len(), device_type.description.params.len());
        let changed = params.iter().find(|p| p["id"] == json!(param)).unwrap();
        assert_eq!(changed["value"], json!(value), "{}", device_type.name);
        assert!(params.iter().all(|p| p["automatable"] == json!(true)));
    }

    let saved = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as",
                               "args": { "path": dir.0.join("Lo-Fi.blset").to_string_lossy() } }] }),
    );
    let set = saved_path(&saved);
    assert!(client.into_transport().finish().unwrap().success());

    let mut reopened = bl_cli_mcp(&["--set", &set.to_string_lossy()]);
    for (id, params) in ids.iter().zip(&before) {
        assert_eq!(&read(&mut reopened, id), params);
    }
}

/// Spec: warping R1, R4 to R11, R52. An agent warps a clip, edits its warp
/// map and reads every change back in the query tool's `clip`, each command
/// run by ID and one undo step.
#[test]
fn an_agent_warps_a_clip_and_reads_it_back() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Loop"}, "kind": "audio", "name": "Loop", "index": 0 } },
        ] }),
    );
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.name() == "Loop")
        .expect("the track was made")
        .id();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "clip.create",
              "args": { "track": track, "clip": {
                  "id": {"$new": "Break"}, "name": "Break", "start": 0, "length": 2 * BAR,
                  "mute": false, "data": { "audio": { "media": "ab".repeat(32) } } } } },
        ] }),
    );
    let clip = client
        .transport()
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .next()
        .expect("the clip was made")
        .id;
    let steps =
        |client: &mut Client<InProcess>| client.transport().shell.editor().undo_history().len();
    let audio = |client: &mut Client<InProcess>| {
        call(client, "query", json!({ "id": clip, "detail": "full" }))["saved"]["data"]["audio"]
            .clone()
    };
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        let before = steps(client);
        call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        );
        assert_eq!(steps(client), before + 1, "{command} is one undo step");
        audio(client)
    };

    let unwarped = audio(&mut client);
    assert!(unwarped.get("warp").is_none() && unwarped.get("warp_markers").is_none());

    // Warp on: one marker at the Start marker, at the set's tempo.
    let warped = run(
        &mut client,
        "clip.set_warp",
        json!({ "clip": clip, "warp": true }),
    );
    assert_eq!(warped["warp"], true);
    assert_eq!(
        warped["warp_markers"]["items"],
        json!([{ "time": 0.0, "beat": 0 }])
    );
    assert_eq!(warped["tempo"], 120.0);

    // Insert, move and shift, delete.
    let inserted = run(
        &mut client,
        "clip.insert_warp_marker",
        json!({ "clips": [clip], "beat": BAR }),
    );
    assert_eq!(
        inserted["warp_markers"]["items"],
        json!([{ "time": 0.0, "beat": 0 }, { "time": 2.0, "beat": BAR }])
    );
    let moved = run(
        &mut client,
        "clip.move_warp_markers",
        json!({ "clips": [clip], "markers": [1], "by": BAR }),
    );
    assert_eq!(
        moved["warp_markers"]["items"][1],
        json!({ "time": 2.0, "beat": 2 * BAR })
    );
    assert_eq!(
        moved["tempo"], 240.0,
        "the last marker's segment is the clip's tempo"
    );
    let shifted = run(
        &mut client,
        "clip.shift_warp_marker",
        json!({ "clips": [clip], "marker": 1, "by": 0.5 }),
    );
    assert_eq!(
        shifted["warp_markers"]["items"][1],
        json!({ "time": 2.5, "beat": 2 * BAR })
    );

    // The BPM field and ×2.
    let slower = run(
        &mut client,
        "clip.set_segment_bpm",
        json!({ "clip": clip, "marker": 0, "bpm": 60.0 }),
    );
    // 60 BPM for 2.5 s: two and a half quarter notes.
    assert_eq!(
        slower["warp_markers"]["items"][1]["beat"],
        2 * 705_600 + 705_600 / 2
    );
    let doubled = run(
        &mut client,
        "clip.scale_tempo",
        json!({ "clip": clip, "factor": 2.0 }),
    );
    assert_eq!(doubled["tempo"], 480.0);
    assert_eq!(doubled["warp_markers"]["items"][1]["beat"], 5 * 705_600);
    let one = run(
        &mut client,
        "clip.delete_warp_markers",
        json!({ "clips": [clip], "markers": [1] }),
    );
    assert_eq!(one["warp_markers"]["total"], 1);

    // A warped clip's positions are ticks, in the form the clip shows.
    let region = run(
        &mut client,
        "clip.set_region",
        json!({ "clip": clip, "start": 705_600, "end": 4 * 705_600, "ticks": true }),
    );
    assert_eq!(
        (region["start"].clone(), region["end"].clone()),
        (json!(705_600.0), json!(2_822_400.0))
    );
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "clip.set_region",
                "args": { "clip": clip, "start": 1.5, "end": null } }] }),
        )
        .unwrap();
    assert!(refused.ok().is_err(), "seconds on a warped clip");

    // Warp off keeps the map and turns the markers back to seconds.
    let off = run(
        &mut client,
        "clip.set_warp",
        json!({ "clip": clip, "warp": false }),
    );
    assert!(off.get("warp").is_none());
    assert_eq!(off["warp_markers"]["total"], 1);
    assert!(
        off["start"].as_f64().unwrap() < 10.0,
        "seconds again: {off}"
    );
}

/// An agent imports an SFZ instrument by path, then reads its layers, its
/// device settings and what the import left unread back (WP-5.14; the
/// multisampler spec, R47 and R53).
#[test]
fn an_agent_imports_an_sfz_by_path() {
    let sfz = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/sfz/Sine Bank.sfz")
        .to_string_lossy()
        .into_owned();
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    let imported = call(
        &mut client,
        "run",
        json!({ "label": "Import the kit", "commands": [
            { "command": "device.import_sfz", "args": { "path": sfz } },
        ] }),
    );
    let result = &imported["results"][0]["result"];
    assert_eq!(result["layers"].as_array().unwrap().len(), 5);
    assert_eq!(result["left_out"][0]["sample"], "missing.wav");
    assert_eq!(result["unread"][0]["name"], "group");
    let device = result["device"].clone();
    // The layers read back, in the order a round robin takes them, with the
    // zones the file gave.
    let found = call(
        &mut client,
        "query",
        json!({ "path": format!("device {}", device) }),
    );
    let layers = &found["layers"];
    assert_eq!(layers["count"], 5);
    let items = layers["list"]["items"].as_array().unwrap();
    let names: Vec<&str> = items.iter().map(|l| l["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["low_soft", "low_hard", "mid_a", "mid_b", "high"]);
    assert_eq!(items[0]["key"]["low"], 36);
    assert_eq!(items[0]["velocity"]["high"], 63);
    assert_eq!(items[2]["key"]["fade_high"], 3);
    assert_eq!(items[4]["sustain"], "loop");
    // Round Robin is on in Forward mode, and Vel > Vol is the file's 0.
    let parameters = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "device.parameters", "args": { "device": device } },
        ] }),
    );
    let params = parameters["results"][0]["result"]["params"]
        .as_array()
        .unwrap()
        .clone();
    let value = |name: &str| {
        params
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("no {name}"))["value"]
            .as_f64()
            .unwrap()
    };
    assert_eq!(value("Round Robin"), 1.0);
    assert_eq!(value("Vel > Vol"), 0.0);
    assert_eq!(value("Env Release"), 50.0);
}

/// Spec: warp-modes R1 to R3, R12; warping R14. An agent sets a clip's warp
/// mode and controls and its RAM and HiQ switches by ID and reads each back
/// in the query tool's `clip`; a loop can't be switched on for an unwarped
/// clip, and unwarping a looped one switches the loop off.
#[test]
fn an_agent_sets_the_warp_mode_and_its_switches_and_reads_them_back() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Loop"}, "kind": "audio", "name": "Loop", "index": 0 } },
        ] }),
    );
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.name() == "Loop")
        .expect("the track was made")
        .id();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "clip.create",
              "args": { "track": track, "clip": {
                  "id": {"$new": "Break"}, "name": "Break", "start": 0, "length": 2 * BAR,
                  "mute": false, "data": { "audio": { "media": "ab".repeat(32) } } } } },
        ] }),
    );
    let clip = client
        .transport()
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .next()
        .expect("the clip was made")
        .id;
    let steps =
        |client: &mut Client<InProcess>| client.transport().shell.editor().undo_history().len();
    let audio = |client: &mut Client<InProcess>| {
        call(client, "query", json!({ "id": clip, "detail": "full" }))["saved"]["data"]["audio"]
            .clone()
    };
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        let before = steps(client);
        call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        );
        assert_eq!(steps(client), before + 1, "{command} is one undo step");
        audio(client)
    };
    let refused = |client: &mut Client<InProcess>, command: &str, args: Value| {
        let before = steps(client);
        let result = client
            .call(
                "run",
                json!({ "commands": [{ "command": command, "args": args }] }),
            )
            .unwrap();
        assert!(result.ok().is_err(), "{command} {args}");
        assert_eq!(steps(client), before, "a refused {command} changes nothing");
    };

    // A new clip has the table's defaults, which are not saved.
    let new = audio(&mut client);
    for key in [
        "warp_mode",
        "warp_settings",
        "ram",
        "high_quality",
        "transpose",
        "detune",
    ] {
        assert!(new.get(key).is_none(), "{key} in a new clip: {new}");
    }
    let mode = run(
        &mut client,
        "clip.set_warp_mode",
        json!({ "clip": clip, "mode": "re_pitch" }),
    );
    assert_eq!(mode["warp_mode"], "re_pitch");
    let settings = run(
        &mut client,
        "clip.set_warp_settings",
        json!({ "clip": clip,
                "beats": { "preserve": "eighth", "transient_loop_mode": "loop_off",
                           "transient_envelope": 40 },
                "texture": { "flux": 10.0 },
                "complex_pro": { "formants": 20.0, "envelope": 200 } }),
    );
    assert_eq!(
        settings["warp_settings"],
        json!({
            "beats": { "preserve": "eighth", "transient_loop_mode": "loop_off",
                       "transient_envelope": 40 },
            "texture": { "flux": 10.0 },
            "complex_pro": { "formants": 20.0, "envelope": 200 },
        })
    );
    // A setting out of range changes nothing, and so does naming none.
    refused(
        &mut client,
        "clip.set_warp_settings",
        json!({ "clip": clip, "tones": { "grain_size": 101.0 } }),
    );
    refused(
        &mut client,
        "clip.set_warp_settings",
        json!({ "clip": clip }),
    );
    let ram = run(
        &mut client,
        "clip.set_ram_mode",
        json!({ "clip": clip, "ram": true }),
    );
    assert_eq!(ram["ram"], true);
    let hiq = run(
        &mut client,
        "clip.set_high_quality",
        json!({ "clip": clip, "high_quality": false }),
    );
    assert_eq!(hiq["high_quality"], false);
    // Spec: warp-modes R3. The clip's pitch, in semitones and in cents.
    let pitch = run(
        &mut client,
        "clip.set_pitch",
        json!({ "clip": clip, "transpose": 7, "detune": -25.0 }),
    );
    assert_eq!(pitch["transpose"], 7);
    assert_eq!(pitch["detune"], -25.0);
    // A value out of range changes nothing, and so does naming neither.
    refused(
        &mut client,
        "clip.set_pitch",
        json!({ "clip": clip, "transpose": 49 }),
    );
    refused(
        &mut client,
        "clip.set_pitch",
        json!({ "clip": clip, "detune": 50.5 }),
    );
    refused(&mut client, "clip.set_pitch", json!({ "clip": clip }));
    // Each reads back as it was set after the others.
    let all = audio(&mut client);
    assert_eq!(all["warp_mode"], "re_pitch");
    assert_eq!(all["ram"], true);
    assert_eq!(all["transpose"], 7);
    assert_eq!(all["detune"], -25.0);

    // Only a warped clip loops.
    refused(
        &mut client,
        "clip.set_loop_enabled",
        json!({ "clip": clip, "enabled": true }),
    );
    run(
        &mut client,
        "clip.set_warp",
        json!({ "clip": clip, "warp": true }),
    );
    let looped = run(
        &mut client,
        "clip.set_loop_enabled",
        json!({ "clip": clip, "enabled": true }),
    );
    assert_eq!(looped["looping"], true);
    let unwarped = run(
        &mut client,
        "clip.set_warp",
        json!({ "clip": clip, "warp": false }),
    );
    assert!(
        unwarped.get("looping").is_none(),
        "unwarping switches the loop off"
    );
}

// Spec: multisampler R63. An agent edits the samples of Simpler and of a
// multisampler by command ID, reverts and converts them, and reads each
// result back through the query tool's `device` object.
#[test]
fn an_agent_edits_a_sample_and_reads_the_edits_back() {
    let dir = TempDir::new("sample-edits");
    let file = dir.0.join("Stem.wav");
    write_stem(&file, 220.0);
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    // Commands that need the shell run one to a call.
    let one = |client: &mut Client<InProcess>, command: &str, args: Value| -> Value {
        call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )["results"][0]["result"]
            .clone()
    };
    let refused = |client: &mut Client<InProcess>, command: &str, args: Value| -> bool {
        client
            .call(
                "run",
                json!({ "commands": [{ "command": command, "args": args }] }),
            )
            .unwrap()
            .ok()
            .is_err()
    };
    let add = |client: &mut Client<InProcess>, name: &str, device_type: &str| {
        one(
            client,
            "track.create",
            json!({ "id": {"$new": name}, "kind": "midi", "name": name, "index": 0 }),
        );
        let track = {
            let document = client.transport().shell.editor().document();
            document.tracks().find(|t| t.name() == name).unwrap().id()
        };
        one(
            client,
            "arrangement.select_tracks",
            json!({ "tracks": [track] }),
        );
        one(
            client,
            "device.add",
            json!({ "device": {
                "kind": { "native": { "device_type": device_type } },
                "name": name, "role": "instrument" } }),
        );
        let document = client.transport().shell.editor().document();
        document.track(track).unwrap().devices().next().unwrap().id
    };
    let snapshot = add(&mut client, "Snap", "beatlayer.snapshot");
    let mosaic = add(&mut client, "Sampler", "beatlayer.mosaic");
    one(
        &mut client,
        "device.load_sample",
        json!({ "device": snapshot, "path": file.to_string_lossy() }),
    );
    one(
        &mut client,
        "device.add_layers",
        json!({ "device": mosaic, "paths": [file.to_string_lossy()] }),
    );
    let device = |client: &mut Client<InProcess>, id: bl_model::DeviceId| -> Value {
        call(
            client,
            "query",
            json!({ "path": format!("device {}", id.0.get()), "detail": "full" }),
        )
    };
    let layer = client
        .transport()
        .shell
        .editor()
        .document()
        .device(mosaic)
        .unwrap()
        .layers
        .list[0]
        .id;

    // Edits by ID: Simpler's list reads back under `saved.sample_edits`, the
    // layer's under its own `edits`.
    let cut = one(
        &mut client,
        "device.edit_sample",
        json!({ "device": snapshot, "kind": "cut", "range": { "from": 0, "to": 12_000 } }),
    );
    assert_eq!(cut["length"], 36_000);
    one(
        &mut client,
        "device.edit_sample",
        json!({ "device": snapshot, "kind": "fade_out" }),
    );
    let trim = one(
        &mut client,
        "device.edit_sample",
        json!({ "device": mosaic, "layer": layer, "kind": "trim",
                "range": { "from": 6_000, "to": 30_000 } }),
    );
    assert_eq!(trim["length"], 24_000);
    let read = device(&mut client, snapshot);
    let edits = read["saved"]["sample_edits"]["items"].as_array().unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0]["kind"], "cut");
    let read = device(&mut client, mosaic);
    let item = &read["layers"]["list"]["items"][0];
    assert_eq!(item["edits"][0]["kind"], "trim");
    assert_eq!(item["length"], 24_000);
    assert_eq!(item["end"], 24_000, "the layer's markers followed the trim");

    // A selection past the sample is refused and changes nothing; so does
    // Crop in a set that has never been saved.
    assert!(refused(
        &mut client,
        "device.edit_sample",
        json!({ "device": snapshot, "kind": "trim", "range": { "from": 0, "to": 90_000 } }),
    ));
    assert!(refused(
        &mut client,
        "device.crop_sample",
        json!({ "device": snapshot })
    ));
    assert_eq!(
        device(&mut client, snapshot)["saved"]["sample_edits"]["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Auditioning is a read and changes nothing.
    let before = client.transport().shell.editor().document().clone();
    one(
        &mut client,
        "device.audition_sample",
        json!({ "device": snapshot, "from": 0.5 }),
    );
    one(
        &mut client,
        "device.audition_sample",
        json!({ "device": snapshot, "down": false }),
    );
    assert_eq!(*client.transport().shell.editor().document(), before);

    // Revert clears both lists; the conversion keeps the sample.
    one(
        &mut client,
        "device.edit_sample",
        json!({ "device": snapshot, "revert": true }),
    );
    one(
        &mut client,
        "device.edit_sample",
        json!({ "device": mosaic, "layer": layer, "revert": true }),
    );
    assert!(
        device(&mut client, snapshot)["saved"]
            .get("sample_edits")
            .is_none()
    );
    assert_eq!(
        device(&mut client, mosaic)["layers"]["list"]["items"][0]["length"],
        48_000
    );
    let converted = one(
        &mut client,
        "device.convert",
        json!({ "device": snapshot, "to": "multisampler" }),
    );
    let new: u64 = converted["device"].as_u64().unwrap();
    let read = call(
        &mut client,
        "query",
        json!({ "path": format!("device {new}") }),
    );
    assert_eq!(
        read["layers"]["count"], 1,
        "Simpler is a multisampler of one layer now"
    );
}

/// Spec: multisampler R64, R69, R70, R73. An agent autosamples a plug-in
/// instrument by ID: the command waits for the samples and returns them,
/// and the new track's multisampler and its layers read back.
#[test]
fn an_agent_autosamples_a_plugin_into_a_multisampler() {
    let dir = TempDir::new("autosample");
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("autosample").unwrap();
    let one = |client: &mut Client<InProcess>, command: &str, args: Value| -> Value {
        call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )["results"][0]["result"]
            .clone()
    };
    // The samples go in the project's folder, so the set is saved first.
    let set = dir.0.join("Song.blset");
    one(
        &mut client,
        "set.save_as",
        json!({ "path": set.to_string_lossy() }),
    );
    // The kit's sine example, a plug-in instrument, on a MIDI track.
    one(
        &mut client,
        "track.create",
        json!({ "id": {"$new": "Inst"}, "kind": "midi", "name": "Inst", "index": 0 }),
    );
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.name() == "Inst")
        .unwrap()
        .id();
    let sine = {
        let types = client.transport().shell.kit().device_types();
        types
            .iter()
            .find(|kind| kind.name == "Example Sine")
            .expect("the sine example")
            .clone()
    };
    let added = one(
        &mut client,
        "device.add",
        json!({ "device": { "kind": sine.kind, "name": sine.name, "role": sine.role },
                "track": track }),
    );
    let device = added["device"].as_u64().expect("the instrument");
    // The command runs by ID, waits for the samples and returns them.
    let result = one(
        &mut client,
        "device.autosample",
        json!({
            "device": device, "name": "Sine", "lowest": 60, "highest": 60,
            "note_length": 0.1, "release_tail": 0.05,
        }),
    );
    let files = result["files"].as_array().expect("the samples");
    assert_eq!(files.len(), 1, "one note at one velocity: {result}");
    assert_eq!(files[0]["note"], 60);
    assert_eq!(files[0]["velocity"], 127);
    let file = Path::new(files[0]["path"].as_str().unwrap());
    assert!(file.exists(), "the sample is written: {file:?}");
    assert_eq!(
        file.parent().unwrap(),
        dir.0.join("Song Project/Samples/Processed/Autosample/Sine"),
        "in the project's folder, under the name"
    );
    // The new track holds the multisampler, and its layers read back.
    let new_track = result["track"].as_u64().expect("the new track");
    let multisampler = result["device"].as_u64().expect("the multisampler");
    let read = call(
        &mut client,
        "query",
        json!({ "id": new_track, "detail": "full" }),
    );
    assert_eq!(read["name"], "Sine");
    assert_eq!(read["kind"], "midi");
    assert_eq!(read["devices"][0]["id"], json!(multisampler));
    let read = call(
        &mut client,
        "query",
        json!({ "id": multisampler, "detail": "full" }),
    );
    let layers = read["layers"]["list"]["items"].as_array().unwrap();
    assert_eq!(layers.len(), 1, "one layer per sample");
    assert_eq!(layers[0]["root_key"], 60);
    assert_eq!(
        (
            layers[0]["key"]["low"].as_u64(),
            layers[0]["key"]["high"].as_u64()
        ),
        (Some(0), Some(127))
    );
    assert_eq!(layers[0]["sustain"], "off", "no loop was asked for");
}

/// Spec: sampler R18, R20. An agent sets Simpler's Warp controls and its
/// manual slices by command ID, as the Warp controls and the display's
/// gestures do, and reads them back.
#[test]
fn an_agent_sets_the_warp_controls_and_manual_slices_and_reads_them_back() {
    let dir = TempDir::new("warp-slices");
    let file = dir.0.join("Loop.wav");
    write_stem(&file, 220.0);
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    let one = |client: &mut Client<InProcess>, command: &str, args: Value| -> Value {
        call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )["results"][0]["result"]
            .clone()
    };
    one(
        &mut client,
        "track.create",
        json!({ "id": {"$new": "Snap"}, "kind": "midi", "name": "Snap", "index": 0 }),
    );
    let track = {
        let document = client.transport().shell.editor().document();
        document.tracks().find(|t| t.name() == "Snap").unwrap().id()
    };
    one(
        &mut client,
        "arrangement.select_tracks",
        json!({ "tracks": [track] }),
    );
    one(
        &mut client,
        "device.add",
        json!({ "device": {
            "kind": { "native": { "device_type": "beatlayer.snapshot" } },
            "name": "Snap", "role": "instrument" } }),
    );
    let snapshot = client
        .transport()
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .devices()
        .next()
        .unwrap()
        .id;
    one(
        &mut client,
        "device.load_sample",
        json!({ "device": snapshot, "path": file.to_string_lossy() }),
    );

    // The Warp controls: on, Beats, its Preserve and Transient Envelope, the
    // sample's tempo, and ×2.
    one(
        &mut client,
        "device.set_warp",
        json!({ "device": snapshot, "warp": true }),
    );
    one(
        &mut client,
        "device.set_warp_mode",
        json!({ "device": snapshot, "mode": "tones" }),
    );
    one(
        &mut client,
        "device.set_warp_settings",
        json!({ "device": snapshot, "beats": { "preserve": "quarter", "transient_envelope": 40 } }),
    );
    one(
        &mut client,
        "device.set_sample_bpm",
        json!({ "device": snapshot, "bpm": 90.0 }),
    );
    one(
        &mut client,
        "device.scale_tempo",
        json!({ "device": snapshot, "factor": 2.0 }),
    );
    // The manual slices: two added, one moved, one deleted.
    one(
        &mut client,
        "device.add_slice",
        json!({ "device": snapshot, "frame": 12_000 }),
    );
    one(
        &mut client,
        "device.add_slice",
        json!({ "device": snapshot, "frame": 30_000 }),
    );
    one(
        &mut client,
        "device.move_slice",
        json!({ "device": snapshot, "from": 30_000, "to": 36_000 }),
    );
    one(
        &mut client,
        "device.delete_slice",
        json!({ "device": snapshot, "frame": 12_000 }),
    );

    let saved = call(
        &mut client,
        "query",
        json!({ "path": format!("device {}", snapshot.0.get()), "detail": "full" }),
    )["saved"]
        .clone();
    assert_eq!(saved["warp"]["warp"], true);
    assert_eq!(saved["warp"]["mode"], "tones");
    assert_eq!(saved["warp"]["bpm"], 180.0);
    assert_eq!(saved["warp"]["settings"]["beats"]["preserve"], "quarter");
    assert_eq!(saved["warp"]["settings"]["beats"]["transient_envelope"], 40);
    assert_eq!(saved["slices"]["items"], json!([36_000]));
}

/// Spec: sample-editor R21. An agent selects, makes, moves and deletes warp
/// markers and transients by command ID, as the Sample Editor's gestures
/// and keys do, and reads the editor's selection and view back.
#[test]
fn an_agent_edits_warp_markers_as_the_sample_editor_does() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Loop"}, "kind": "audio", "name": "Loop", "index": 0 } },
        ] }),
    );
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.name() == "Loop")
        .expect("the track was made")
        .id();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "clip.create",
              "args": { "track": track, "clip": {
                  "id": {"$new": "Break"}, "name": "Break", "start": 0, "length": 2 * BAR,
                  "mute": false, "data": { "audio": { "media": "ab".repeat(32) } } } } },
        ] }),
    );
    let clip = client
        .transport()
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .next()
        .expect("the clip was made")
        .id;
    let steps =
        |client: &mut Client<InProcess>| client.transport().shell.editor().undo_history().len();
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )["results"][0]["result"]
            .clone()
    };
    let markers = |client: &mut Client<InProcess>| {
        call(client, "query", json!({ "id": clip, "detail": "full" }))["saved"]["data"]["audio"]
            ["warp_markers"]["items"]
            .clone()
    };
    run(
        &mut client,
        "clip.set_warp",
        json!({ "clip": clip, "warp": true }),
    );
    run(
        &mut client,
        "arrangement.select_clips",
        json!({ "clips": [clip] }),
    );
    // The editor shows the clip: the insert marker puts a marker where the
    // key would, and the selection reads back.
    let state = run(&mut client, "clip_view.sample_editor", json!({}));
    assert_eq!(state["clip"], json!(clip));
    run(
        &mut client,
        "clip_view.set_insert_marker",
        json!({ "beat": BAR }),
    );
    let before = steps(&mut client);
    run(
        &mut client,
        "clip.insert_warp_marker",
        json!({ "clips": [clip], "beat": BAR }),
    );
    assert_eq!(steps(&mut client), before + 1);
    run(
        &mut client,
        "clip_view.select_warp_markers",
        json!({ "markers": [1] }),
    );
    let state = run(&mut client, "clip_view.sample_editor", json!({}));
    assert_eq!(state["markers"], json!([1]));
    assert_eq!(state["insert_marker"]["beat"], BAR);
    // Moving, shifting and deleting a marker, and a transient put in and
    // taken out, each one undo step.
    run(
        &mut client,
        "clip.move_warp_markers",
        json!({ "clips": [clip], "markers": [1], "by": BAR / 2 }),
    );
    assert_eq!(markers(&mut client)[1]["beat"], BAR + BAR / 2);
    run(
        &mut client,
        "clip.insert_transient",
        json!({ "clip": clip, "time": 1.0 }),
    );
    run(
        &mut client,
        "clip_view.select_transients",
        json!({ "times": [1.0] }),
    );
    assert_eq!(
        run(&mut client, "clip_view.sample_editor", json!({}))["transients"],
        json!([1.0])
    );
    run(
        &mut client,
        "clip.delete_transients",
        json!({ "clip": clip, "times": [1.0] }),
    );
    run(
        &mut client,
        "clip.delete_warp_markers",
        json!({ "clips": [clip], "markers": [1] }),
    );
    assert_eq!(markers(&mut client).as_array().unwrap().len(), 1);
    // Zooming to a time selection reads back as the view.
    run(
        &mut client,
        "clip_view.select_time",
        json!({ "start": 0.5, "end": 1.5 }),
    );
    run(
        &mut client,
        "clip_view.sample_view",
        json!({ "show": "selection" }),
    );
    let state = run(&mut client, "clip_view.sample_editor", json!({}));
    // The view is the selection in ticks of the clip's contents: 1.5 s is
    // three times 0.5 s on the map's one speed.
    let view = state["view"].as_array().unwrap();
    let (from, to) = (view[0].as_f64().unwrap(), view[1].as_f64().unwrap());
    assert!(from > 0.0 && (to - 3.0 * from).abs() < 2.0, "{view:?}");
}

/// Warping R49, R50 (WP-4.12): an agent quantises a warped clip's transients
/// by command ID, one undo step, and reads the markers back in the query
/// tool's `clip`; 0 percent changes nothing and records no step.
#[test]
fn an_agent_quantises_a_warped_clip_by_command_id() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Loop"}, "kind": "audio", "name": "Loop", "index": 0 } },
        ] }),
    );
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.name() == "Loop")
        .expect("the track was made")
        .id();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "clip.create",
              "args": { "track": track, "clip": {
                  "id": {"$new": "Break"}, "name": "Break", "start": 0, "length": 2 * BAR,
                  "mute": false, "data": { "audio": { "media": "ab".repeat(32) } } } } },
        ] }),
    );
    let clip = client
        .transport()
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .next()
        .expect("the clip was made")
        .id;
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )["results"][0]["result"]
            .clone()
    };
    let markers = |client: &mut Client<InProcess>| {
        call(client, "query", json!({ "id": clip, "detail": "full" }))["saved"]["data"]["audio"]
            ["warp_markers"]["items"]
            .clone()
    };
    let steps =
        |client: &mut Client<InProcess>| client.transport().shell.editor().undo_history().len();
    run(
        &mut client,
        "clip.set_warp",
        json!({ "clip": clip, "warp": true }),
    );
    // Two late hits at 120 BPM, quantised at 100 percent onto the sixteenths
    // they are nearest (0.5 s and 1 s), in one undo step.
    let before = steps(&mut client);
    run(
        &mut client,
        "clip.quantize_audio",
        json!({ "clip": clip, "amount": 100, "grid": Ticks::SIXTEENTH.get(),
                "transients": [0.52, 1.03] }),
    );
    assert_eq!(steps(&mut client), before + 1);
    assert_eq!(
        markers(&mut client),
        json!([
            { "time": 0.0, "beat": 0 },
            { "time": 0.52, "beat": 4 * Ticks::SIXTEENTH.get() },
            { "time": 1.03, "beat": 8 * Ticks::SIXTEENTH.get() },
        ])
    );
    // 0 percent changes nothing and records no step.
    run(
        &mut client,
        "clip.quantize_audio",
        json!({ "clip": clip, "amount": 0, "grid": Ticks::SIXTEENTH.get(),
                "transients": [0.52, 1.03] }),
    );
    assert_eq!(markers(&mut client).as_array().unwrap().len(), 3);
    assert_eq!(steps(&mut client), before + 1);
}

/// Warping R29 to R41 (WP-4.9): an agent drops a generated 40-second track
/// at 128 BPM, reads the warp settings, auto-warps it from its first click,
/// fixes its downbeat, and warps a loop, every command run by ID, one undo
/// step each, every effect read back in the query tool's `clip`.
#[test]
fn an_agent_auto_warps_and_fixes_a_downbeat_by_command_id() {
    let dir = TempDir::new("auto-warp");
    let (samples, starts) = bl_analysis::signals::click_track(128.0, 0.37, 40.0, 48_000);
    let path = dir.0.join("Track.wav");
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for sample in &samples {
            writer
                .write_sample((sample * 0.9 * f32::from(i16::MAX)) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();
    }
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        let before = client.transport().shell.editor().undo_history().len();
        let result = call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        );
        let after = client.transport().shell.editor().undo_history().len();
        (result, after - before)
    };

    // The settings read back under `warp`.
    run(
        &mut client,
        "settings.set_loop_warp_short_samples",
        json!({ "mode": "unwarped_one_shot" }),
    );
    let state = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "settings.state" }] }),
    );
    let warp = &state["results"][0]["result"]["warp"];
    assert_eq!(warp["loop_warp_short_samples"], "unwarped_one_shot");
    assert_eq!(warp["auto_warp_long_samples"], true);
    assert_eq!(warp["default_warp_mode"], "beats");

    let (added, _) = run(
        &mut client,
        "browser.add_to_arrangement",
        json!({ "paths": [path.display().to_string()] }),
    );
    let clip = added["results"][0]["result"]["clips"][0]["clip"]
        .as_u64()
        .unwrap();
    let media = match &client
        .transport()
        .shell
        .editor()
        .document()
        .clip(serde_json::from_value(json!(clip)).unwrap())
        .unwrap()
        .data
    {
        ClipData::Audio(audio) => audio.media,
        ClipData::Midi(_) => unreachable!("an audio clip"),
    };
    client.transport().shell.wait_analysis(media).unwrap();
    let audio = |client: &mut Client<InProcess>| {
        call(client, "query", json!({ "id": clip, "detail": "full" }))["saved"]["data"]["audio"]
            .clone()
    };
    let markers = |audio: &Value| audio["warp_markers"]["items"].as_array().unwrap().clone();

    let (_, steps) = run(
        &mut client,
        "clip.set_warp",
        json!({ "clip": clip, "warp": true }),
    );
    assert_eq!(steps, 1);
    // Warp From Here, from the first click: a marker on every bar.
    let (_, steps) = run(
        &mut client,
        "clip.warp_from_here",
        json!({ "clip": clip, "time": starts[0], "method": "auto" }),
    );
    assert_eq!(steps, 1, "one undo step");
    let warped = audio(&mut client);
    let bars = markers(&warped);
    assert!(bars.len() > 20, "{} markers", bars.len());
    assert!((warped["tempo"].as_f64().unwrap() - 128.0).abs() < 0.5);
    // The markers: the file's start, the first click, then each later bar.
    for (bar, marker) in bars[2..].iter().enumerate() {
        let time = marker["time"].as_f64().unwrap();
        assert!(
            (time - starts[4 * (bar + 1)]).abs() < 0.001,
            "bar {bar}: {time}"
        );
    }

    // The third bar's downbeat becomes 1.1.1.
    let (_, steps) = run(
        &mut client,
        "clip.set_1_1_1_here",
        json!({ "clip": clip, "marker": 3 }),
    );
    assert_eq!(steps, 1);
    let fixed = audio(&mut client);
    assert_eq!(markers(&fixed)[3]["beat"], 0);
    assert_eq!(markers(&fixed).len(), bars.len());
    assert_eq!(fixed["tempo"], warped["tempo"]);

    // Warp as a loop, and a selection of it.
    let (_, steps) = run(&mut client, "clip.warp_as_loop", json!({ "clip": clip }));
    assert_eq!(steps, 1);
    let looped = audio(&mut client);
    assert_eq!(looped["looping"], true);
    assert_eq!(markers(&looped).len(), 2);
    let (_, steps) = run(
        &mut client,
        "clip.warp_selection_as_loop",
        json!({ "clip": clip, "from": starts[0], "to": starts[8] }),
    );
    assert_eq!(steps, 1);
    // Two bars at 128 BPM, taken as a loop of 2 bars at the set's 120.
    let selection = markers(&audio(&mut client));
    assert_eq!(selection[1]["beat"], 2 * BAR);

    // Save Default Clip (R42) writes beside the sample and is no undo step;
    // `clip.defaults` reads it back.
    let selected = audio(&mut client);
    let (saved, steps) = run(&mut client, "clip.save_default", json!({ "clip": clip }));
    assert_eq!(steps, 0);
    let file = saved["results"][0]["result"]["file"].as_str().unwrap();
    assert_eq!(Path::new(file).parent(), path.parent());
    assert!(Path::new(file).is_file());
    let (read, steps) = run(&mut client, "clip.defaults", json!({ "clip": clip }));
    assert_eq!(steps, 0);
    let defaults = &read["results"][0]["result"]["defaults"];
    assert_eq!(defaults["warp"], true);
    assert_eq!(defaults["tempo"], selected["tempo"]);
    assert_eq!(defaults["warp_markers"].as_array().unwrap().len(), 2);
}

// Spec: external-devices R2, R15, R28
#[test]
fn an_agent_routes_an_external_instrument_and_reads_it_back() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("hardware-agent").unwrap();
    let tracks = default_tracks(&mut client);
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        let result = call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        );
        result["results"][0].clone()
    };

    // A MIDI track's MIDI To, with a port and a channel.
    run(
        &mut client,
        "routing.set_midi_output",
        json!({ "track": tracks[0], "to": { "name": "Synth", "channel": 3 } }),
    );
    let track = call(
        &mut client,
        "query",
        json!({ "id": tracks[0], "detail": "full" }),
    );
    assert_eq!(
        track["saved"]["routing"]["midi_output"],
        json!({ "name": "Synth", "channel": 3 })
    );

    // The external instrument: added, routed, and its Hardware Latency set
    // in samples and then in milliseconds, each read back.
    let hardware = bl_devices::hardware_instrument_type();
    run(
        &mut client,
        "arrangement.select_tracks",
        json!({ "tracks": [tracks[0]] }),
    );
    let added = run(
        &mut client,
        "device.add",
        json!({ "device": { "kind": hardware.kind, "name": hardware.name,
                            "role": hardware.role } }),
    );
    let device = added["result"]["device"].clone();
    let external = |client: &mut Client<InProcess>| {
        call(client, "query", json!({ "id": device, "detail": "full" }))["saved"]["external"]
            .clone()
    };
    assert_eq!(external(&mut client), json!({}), "a new device's defaults");
    run(
        &mut client,
        "device.set_external",
        json!({ "device": device, "midi_to": { "type": "port", "name": "Synth" },
                "channel": 4, "audio_from": { "type": "pair", "channel": 3 } }),
    );
    run(
        &mut client,
        "device.set_external",
        json!({ "device": device, "hardware_latency": { "unit": "samples", "value": 600 } }),
    );
    assert_eq!(
        external(&mut client),
        json!({ "port": "Synth", "channel": 4, "audio_from": { "type": "pair", "channel": 3 },
                "hardware_latency": { "unit": "samples", "value": 600 } })
    );
    run(
        &mut client,
        "device.set_external",
        json!({ "device": device, "hardware_latency": { "unit": "milliseconds", "value": 12.5 } }),
    );
    assert_eq!(
        external(&mut client)["hardware_latency"],
        json!({ "unit": "milliseconds", "value": 12.5 })
    );

    // A bad request changes nothing, and each change is one undo step.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "device.set_external",
                "args": { "device": device, "channel": 17 } }] }),
        )
        .expect("the call reaches the server")
        .ok();
    assert!(refused.is_err(), "channel 17 is refused");
    assert_eq!(external(&mut client)["channel"], json!(4));
    let history = call(&mut client, "query", json!({ "path": "history" }));
    assert_eq!(
        history["undo"]["items"],
        json!([
            "hardware-agent: Set External Device",
            "hardware-agent: Set External Device",
            "hardware-agent: Set External Device",
            "hardware-agent: Add Device",
            "hardware-agent: Set MIDI Output",
        ])
    );
}

// Spec: saturator R1, R13 and R15, overdrive R1, R7 and R9, tube-distortion
// R1, R10 and R12 (WP-5.23): an agent adds Saturator, Overdrive and Dynamic Tube, reads their
// parameters, sets some by ID, shows Saturator's expanded part, reads each
// display's curve, and finds the devices and values again when the saved
// set is opened.
#[test]
fn an_agent_adds_sets_and_reads_the_saturation_devices_by_id_and_reopens_them() {
    use bl_devices::{overdrive, saturator, tube};

    let dir = TempDir::new("saturation");
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("saturation-agent").unwrap();
    let track = default_tracks(&mut client)[2];
    call(
        &mut client,
        "run",
        json!({ "commands": [{
            "command": "arrangement.select_tracks",
            "args": { "tracks": [track] },
        }] }),
    );
    let mut added = Vec::new();
    for device_type in [
        saturator::saturator_type(),
        overdrive::overdrive_type(),
        tube::tube_type(),
    ] {
        let reply = call(
            &mut client,
            "run",
            json!({ "commands": [{
                "command": "device.add",
                "args": {
                    "track": track,
                    "device": {
                        "kind": device_type.kind,
                        "name": device_type.name,
                        "role": device_type.role,
                    },
                },
            }] }),
        );
        added.push(reply["results"][0]["result"]["device"].clone());
    }
    let [singe, grit, valve] = [&added[0], &added[1], &added[2]];

    let parameters = |client: &mut Client<InProcess>, device: &Value| {
        let read = call(
            client,
            "run",
            json!({ "commands": [{ "command": "device.parameters", "args": { "device": device } }] }),
        );
        read["results"][0]["result"]["params"].clone()
    };
    let value_of = |params: &Value, name: &str| {
        params
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("no {name}"))["value"]
            .clone()
    };
    assert_eq!(parameters(&mut client, singe).as_array().unwrap().len(), 19);
    assert_eq!(parameters(&mut client, grit).as_array().unwrap().len(), 6);
    assert_eq!(parameters(&mut client, valve).as_array().unwrap().len(), 9);

    let set = |client: &mut Client<InProcess>, device: &Value, param: u32, value: f64| {
        call(
            client,
            "run",
            json!({ "commands": [{
                "command": "device.set_param",
                "args": { "device": device, "param": param, "value": value },
            }] }),
        );
    };
    set(&mut client, singe, saturator::CURVE.0, 7.0);
    set(&mut client, singe, saturator::HI_QUALITY.0, 1.0);
    set(&mut client, grit, overdrive::DRIVE.0, 80.0);
    set(&mut client, valve, tube::MODEL.0, 2.0);
    set(&mut client, valve, tube::ENVELOPE.0, -40.0);
    let params = parameters(&mut client, singe);
    assert_eq!(value_of(&params, "Curve"), json!(7.0));
    assert_eq!(value_of(&params, "Hi-Quality"), json!(1.0));
    assert_eq!(
        value_of(&parameters(&mut client, grit), "Drive"),
        json!(80.0)
    );

    // Saturator has an expanded part; Overdrive and Dynamic Tube have none.
    let expand = |client: &mut Client<InProcess>, device: &Value| {
        call(
            client,
            "run",
            json!({ "commands": [{ "command": "device.expand", "args": { "device": device } }] }),
        )
    };
    let shown = expand(&mut client, singe);
    assert_eq!(
        shown["results"][0]["result"]["devices"][0]["expanded"],
        json!(true)
    );

    // Each display holds its curve, with no engine publishing.
    for (device, name) in [
        (singe, "Shaper Curve"),
        (grit, "Drive Curve"),
        (valve, "Tube Curve"),
    ] {
        let read = call(
            &mut client,
            "run",
            json!({ "commands": [{ "command": "device.display", "args": { "device": device } }] }),
        );
        let display = &read["results"][0]["result"];
        assert_eq!(display["live"], json!(false));
        let curve = display["series"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("no {name} in {display}"));
        assert!(curve["values"].as_array().unwrap().len() > 50);
    }

    // The saved set reopens with the three devices and their values.
    let saved = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as",
                               "args": { "path": dir.0.join("Saturation.blset").to_string_lossy() } }] }),
    );
    let (project, _) = bl_project::load(saved_path(&saved)).unwrap();
    let devices: Vec<_> = project
        .document
        .tracks()
        .flat_map(|t| t.devices().cloned().collect::<Vec<_>>())
        .collect();
    let find = |kind: &str| {
        devices
            .iter()
            .find(|d| {
                matches!(&d.kind, bl_model::DeviceKind::Native { device_type } if device_type == kind)
            })
            .unwrap_or_else(|| panic!("no {kind} in the reopened set"))
    };
    assert_eq!(find(saturator::SATURATOR).params[&saturator::CURVE], 7.0);
    assert_eq!(
        find(saturator::SATURATOR).params[&saturator::HI_QUALITY],
        1.0
    );
    assert_eq!(find(overdrive::OVERDRIVE).params[&overdrive::DRIVE], 80.0);
    assert_eq!(find(tube::TUBE).params[&tube::MODEL], 2.0);
    assert_eq!(find(tube::TUBE).params[&tube::ENVELOPE], -40.0);
}

// Spec: multiband-dynamics R18, device-framework R14 (WP-5.22): an agent
// inserts Multiband Dynamics by its type, sets its parameters by ID, a band's among
// them, and reads every one back with its descriptor.
#[test]
fn an_agent_inserts_triband_by_its_type_and_sets_and_reads_its_parameters_by_id() {
    use bl_devices::multiband_dynamics::{
        ABOVE_RATIO, HIGH, HIGH_FREQ, LOW_ON, MID, MULTIBAND_DYNAMICS, SOLO, band_param,
    };

    let mut shell = Shell::headless();
    shell.set_device_types(bl_devices::device_types());
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("triband-agent").unwrap();
    let track = default_tracks(&mut client)[2];
    let set = |param: bl_model::ParamId, value: f64| {
        json!({ "command": "device.set_param",
                "args": { "device": {"$ref": "Multiband Dynamics"}, "param": param, "value": value } })
    };
    let reply = call(
        &mut client,
        "run",
        json!({ "label": "Add Multiband Dynamics", "commands": [
            { "command": "device.insert",
              "args": { "track": track, "index": 0,
                        "device": { "id": {"$new": "Multiband Dynamics"}, "name": "Multiband Dynamics",
                                    "kind": { "native": { "device_type": MULTIBAND_DYNAMICS } },
                                    "enabled": true, "params": [] } } },
            set(LOW_ON, 0.0),
            set(HIGH_FREQ, 3_000.0),
            set(band_param(MID, ABOVE_RATIO), 4.0),
            set(band_param(HIGH, SOLO), 1.0),
        ] }),
    );
    let device = reply["ids"]["Multiband Dynamics"].clone();
    let read = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.parameters", "args": { "device": device } }] }),
    );
    let params = read["results"][0]["result"]["params"].as_array().unwrap();
    assert_eq!(params.len(), 45, "every parameter is listed");
    let get = |id: bl_model::ParamId| {
        params
            .iter()
            .find(|p| p["id"] == json!(id.0))
            .unwrap_or_else(|| panic!("parameter {id:?}"))
            .clone()
    };
    let values: Vec<_> = [
        LOW_ON,
        HIGH_FREQ,
        band_param(MID, ABOVE_RATIO),
        band_param(HIGH, SOLO),
    ]
    .iter()
    .map(|id| get(*id)["value"].clone())
    .collect();
    assert_eq!(values, [json!(0.0), json!(3_000.0), json!(4.0), json!(1.0)]);
    assert_eq!(
        get(band_param(MID, ABOVE_RATIO))["name"],
        json!("Mid Above Ratio")
    );
    assert!(params.iter().all(|p| p["automatable"] == json!(true)));
}

// Spec: auto-pan-tremolo R16. An agent inserts Auto Pan-Tremolo by type and
// sets and reads its parameters by ID.
#[test]
fn an_agent_inserts_auto_pan_tremolo_by_type_and_sets_and_reads_parameters() {
    use bl_devices::auto_pan_tremolo::{AMOUNT, AUTO_PAN_TREMOLO, HARMONIC, MODE, RATE_HZ};

    let mut shell = Shell::headless();
    shell.set_device_types(bl_devices::device_types());
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("auto-pan-agent").unwrap();
    let track = default_tracks(&mut client)[2];
    let set = |param: bl_model::ParamId, value: f64| {
        json!({ "command": "device.set_param",
                "args": { "device": {"$ref": "Auto Pan-Tremolo"}, "param": param, "value": value } })
    };
    let reply = call(
        &mut client,
        "run",
        json!({ "label": "Add Auto Pan-Tremolo", "commands": [
            { "command": "device.insert",
              "args": { "track": track, "index": 0,
                        "device": { "id": {"$new": "Auto Pan-Tremolo"}, "name": "Auto Pan-Tremolo",
                                    "kind": { "native": { "device_type": AUTO_PAN_TREMOLO } },
                                    "enabled": true, "params": [] } } },
            set(MODE, 1.0),
            set(RATE_HZ, 5.0),
            set(AMOUNT, 50.0),
            set(HARMONIC, 1.0),
        ] }),
    );
    let device = reply["ids"]["Auto Pan-Tremolo"].clone();
    let read = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.parameters", "args": { "device": device } }] }),
    );
    let params = read["results"][0]["result"]["params"].as_array().unwrap();
    assert_eq!(params.len(), 20, "every parameter is listed");
    let get = |id: bl_model::ParamId| {
        params
            .iter()
            .find(|p| p["id"] == json!(id.0))
            .unwrap_or_else(|| panic!("parameter {id:?}"))
            .clone()
    };
    let values: Vec<_> = [MODE, RATE_HZ, AMOUNT, HARMONIC]
        .iter()
        .map(|id| get(*id)["value"].clone())
        .collect();
    assert_eq!(values, [json!(1.0), json!(5.0), json!(50.0), json!(1.0)]);
    assert_eq!(get(MODE)["name"], json!("Mode"));
    assert_eq!(get(RATE_HZ)["name"], json!("Rate"));
    assert!(params.iter().all(|p| p["automatable"] == json!(true)));
}

// Spec: drum-racks R27. An agent builds a kit by command ID: a drum rack
// with pads, a choke group, a send to a return chain and a drop on a pad,
// and reads each chain's Receive, Play, Choke and sends and the rack's
// returns back through the query tool's `device` object.
#[test]
fn an_agent_builds_a_kit_and_reads_it_back() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("kit").unwrap();
    let midi = default_tracks(&mut client)[0];
    let effect = json!({
        "id": {"$new": "fx"}, "name": "Through", "enabled": true, "params": [],
        "kind": { "native": { "device_type": "test.through" } },
    });
    let built = call(
        &mut client,
        "run",
        json!({ "label": "Build a kit", "commands": [
            { "command": "device.insert", "args": { "track": midi, "index": 0, "device": {
                "id": {"$new": "kit"}, "name": "Drums", "enabled": true, "params": [],
                "role": "instrument", "kind": { "rack": { "kind": "drum" } },
                "chains": [
                    { "id": {"$new": "kick"}, "name": "Kick", "devices": [] },
                    { "id": {"$new": "hat"}, "name": "Hat", "devices": [],
                      "receive": { "note": 42 } },
                    { "id": {"$new": "room"}, "name": "Room", "devices": [effect],
                      "is_return": true },
                ],
            } } },
            { "command": "chain.set_play", "args": { "chain": {"$ref": "kick"}, "play": 62 } },
            { "command": "chain.set_choke", "args": { "chain": {"$ref": "hat"}, "choke": 1 } },
            { "command": "chain.set_receive", "args": { "chain": {"$ref": "hat"},
                                                         "receive": { "note": 46 } } },
            { "command": "chain.set_send", "args": { "chain": {"$ref": "kick"},
                                                       "to": {"$ref": "room"}, "level": 0.5 } },
        ] }),
    );
    assert_eq!(built["ran"], 5);
    let id = |name: &str| built["ids"][name].as_u64().unwrap();
    let rack = call(
        &mut client,
        "query",
        json!({ "id": id("kit"), "detail": "full" }),
    );
    let chains = rack["chains"].as_array().unwrap();
    assert_eq!(
        chains.len(),
        2,
        "the drum chains; the return chain is `returns`"
    );
    assert_eq!(chains[0]["receive"], json!({ "note": 36 }));
    assert_eq!(chains[0]["play"], 62);
    assert_eq!(chains[0]["choke"], json!(null));
    assert_eq!(chains[0]["sends"][0]["to"].as_u64(), Some(id("room")));
    assert_eq!(chains[0]["sends"][0]["level"], 0.5);
    assert_eq!(chains[1]["receive"], json!({ "note": 46 }));
    assert_eq!(chains[1]["choke"], 1);
    let returns = rack["returns"].as_array().unwrap();
    assert_eq!(returns.len(), 1);
    assert_eq!(returns[0]["id"].as_u64(), Some(id("room")));
    assert_eq!(returns[0]["audio_to"], json!(null));
    // The pad view and the pads' state read back from the Device View.
    let run_one = |client: &mut Client<InProcess>, command: &str, args: Value| {
        let out = call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        );
        out["results"][0]["result"].clone()
    };
    run_one(
        &mut client,
        "arrangement.select_tracks",
        json!({ "tracks": [midi] }),
    );
    run_one(
        &mut client,
        "pad.select",
        json!({ "rack": id("kit"), "note": 36 }),
    );
    run_one(
        &mut client,
        "pad.set_mute",
        json!({ "rack": id("kit"), "note": 36 }),
    );
    let state = run_one(&mut client, "device_view.state", json!({}));
    let frame = &state["devices"][0]["rack"];
    assert_eq!(frame["pads"]["first"], 36);
    assert_eq!(frame["pads"]["selected"], 36);
    assert_eq!(frame["pads"]["pads"][0]["mute"], "on");
    assert_eq!(
        frame["chains"][2]["output"]["audio_to_label"],
        "Rack Output"
    );
    // A pad takes a device by command ID, one undo step.
    let dropped = run_one(
        &mut client,
        "pad.drop",
        json!({ "rack": id("kit"), "note": 36,
                "device": { "kind": { "native": { "device_type": "test.through" } },
                            "name": "Through", "role": "audio_effect" } }),
    );
    assert_eq!(dropped["notes"], json!([36]));
    let history = call(&mut client, "query", json!({ "path": "history" }));
    assert_eq!(history["undo"]["items"][0], "kit: Drop on Pad");
}

// Spec: phaser-flanger R14, chorus-ensemble R9
#[test]
fn an_agent_adds_sets_and_reads_the_modulation_devices_by_id_and_reopens_them() {
    use bl_devices::{chorus_ensemble, phaser_flanger};

    let dir = TempDir::new("modulation");
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("modulation-agent").unwrap();
    let track = default_tracks(&mut client)[2];
    call(
        &mut client,
        "run",
        json!({ "commands": [{
            "command": "arrangement.select_tracks",
            "args": { "tracks": [track] },
        }] }),
    );
    let mut added = Vec::new();
    for device_type in [
        phaser_flanger::phaser_flanger_type(),
        chorus_ensemble::chorus_ensemble_type(),
    ] {
        let reply = call(
            &mut client,
            "run",
            json!({ "commands": [{
                "command": "device.add",
                "args": {
                    "track": track,
                    "device": {
                        "kind": device_type.kind,
                        "name": device_type.name,
                        "role": device_type.role,
                    },
                },
            }] }),
        );
        added.push(reply["results"][0]["result"]["device"].clone());
    }
    let [pf, chorus] = [&added[0], &added[1]];

    let parameters = |client: &mut Client<InProcess>, device: &Value| {
        let read = call(
            client,
            "run",
            json!({ "commands": [{ "command": "device.parameters", "args": { "device": device } }] }),
        );
        read["results"][0]["result"]["params"].clone()
    };
    let value_of = |params: &Value, name: &str| {
        params
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("no {name}"))["value"]
            .clone()
    };

    assert_eq!(parameters(&mut client, pf).as_array().unwrap().len(), 30);
    assert_eq!(
        parameters(&mut client, chorus).as_array().unwrap().len(),
        14
    );

    let set = |client: &mut Client<InProcess>, device: &Value, param: u32, value: f64| {
        call(
            client,
            "run",
            json!({ "commands": [{
                "command": "device.set_param",
                "args": { "device": device, "param": param, "value": value },
            }] }),
        );
    };
    // Every parameter of both devices moved off its default, to its range's
    // other end, and then the two modes set so the displays below are the
    // ones named. What `device.parameters` reads back is what the reopened
    // set has to hold.
    let mut wanted: Vec<(&str, Vec<(bl_model::ParamId, f64)>)> = Vec::new();
    for (device_type, device, kind) in [
        (
            phaser_flanger::phaser_flanger_type(),
            pf,
            phaser_flanger::PHASER_FLANGER,
        ),
        (
            chorus_ensemble::chorus_ensemble_type(),
            chorus,
            chorus_ensemble::CHORUS_ENSEMBLE,
        ),
    ] {
        for param in &device_type.description.params {
            let to = if param.default == param.max() {
                param.min()
            } else {
                param.max()
            };
            set(&mut client, device, param.id.0, to);
        }
        wanted.push((kind, Vec::new()));
    }
    set(&mut client, pf, phaser_flanger::MODE.0, 1.0);
    set(&mut client, chorus, chorus_ensemble::MODE.0, 2.0);
    for ((_, values), (device_type, device)) in wanted.iter_mut().zip([
        (phaser_flanger::phaser_flanger_type(), pf),
        (chorus_ensemble::chorus_ensemble_type(), chorus),
    ]) {
        let read = parameters(&mut client, device);
        *values = device_type
            .description
            .params
            .iter()
            .map(|param| {
                let value = value_of(&read, &param.name).as_f64().expect("a number");
                (param.id, value)
            })
            .collect();
    }

    // Phaser-Flanger has an expanded part.
    let expand = |client: &mut Client<InProcess>, device: &Value| {
        call(
            client,
            "run",
            json!({ "commands": [{ "command": "device.expand", "args": { "device": device } }] }),
        )
    };
    let shown = expand(&mut client, pf);
    assert_eq!(
        shown["results"][0]["result"]["devices"][0]["expanded"],
        json!(true)
    );

    // Each display returns its series.
    for (device, name) in [(pf, "Flanger Delay"), (chorus, "Vibrato LFO")] {
        let read = call(
            &mut client,
            "run",
            json!({ "commands": [{ "command": "device.display", "args": { "device": device } }] }),
        );
        let display = &read["results"][0]["result"];
        assert_eq!(display["live"], json!(false));
        let series = display["series"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("no {name} in {display}"));
        assert_eq!(series["name"], json!(name));
    }

    // Save and reopen.
    let saved = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as",
                               "args": { "path": dir.0.join("Modulation.blset").to_string_lossy() } }] }),
    );
    let (project, _) = bl_project::load(saved_path(&saved)).unwrap();
    let devices: Vec<_> = project
        .document
        .tracks()
        .flat_map(|t| t.devices().cloned().collect::<Vec<_>>())
        .collect();
    let find = |kind: &str| {
        devices
            .iter()
            .find(|d| {
                matches!(&d.kind, bl_model::DeviceKind::Native { device_type } if device_type == kind)
            })
            .unwrap_or_else(|| panic!("no {kind} in the reopened set"))
    };
    for (kind, values) in &wanted {
        let device = find(kind);
        for (param, value) in values {
            assert_eq!(
                device.params[param], *value,
                "{kind}: parameter {param:?} reopened"
            );
        }
    }
}

// Spec: spectrum-analyser R9, tuner R8, oscilloscope R8 (WP-5.28): an agent adds
// Spectrum, Tuner and Oscilloscope, reads their parameters, sets some by ID,
// reads each display, and finds the devices and values again when the saved set
// is opened.
#[test]
fn an_agent_inserts_sets_and_reads_the_measurement_devices_by_id_and_reopens_them() {
    use bl_devices::{oscilloscope, spectrum_analyser, tuner};

    let dir = TempDir::new("measurement");
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("measurement-agent").unwrap();
    let track = default_tracks(&mut client)[2];

    let reply = call(
        &mut client,
        "run",
        json!({ "label": "Add measurement devices", "commands": [
            { "command": "device.insert",
              "args": { "track": track, "index": 0,
                        "device": { "id": {"$new": "Spec"}, "name": "Spectrum",
                                    "kind": { "native": { "device_type": spectrum_analyser::SPECTRUM } },
                                    "enabled": true, "params": [] } } },
            { "command": "device.insert",
              "args": { "track": track, "index": 1,
                        "device": { "id": {"$new": "Tun"}, "name": "Tuner",
                                    "kind": { "native": { "device_type": tuner::TUNER } },
                                    "enabled": true, "params": [] } } },
            { "command": "device.insert",
              "args": { "track": track, "index": 2,
                        "device": { "id": {"$new": "Osc"}, "name": "Oscilloscope",
                                    "kind": { "native": { "device_type": oscilloscope::OSCILLOSCOPE } },
                                    "enabled": true, "params": [] } } },
        ] }),
    );
    let spec = reply["ids"]["Spec"].clone();
    let tun = reply["ids"]["Tun"].clone();
    let osc = reply["ids"]["Osc"].clone();

    // Set parameters by ID
    let mut set = |device: &Value, param: bl_model::ParamId, value: f64| {
        call(
            &mut client,
            "run",
            json!({ "commands": [{
                "command": "device.set_param",
                "args": { "device": device, "param": param, "value": value },
            }] }),
        );
    };

    set(&spec, spectrum_analyser::BLOCK, 2.0);
    set(&spec, spectrum_analyser::REFRESH, 40.0);
    set(&tun, tuner::REFERENCE, 432.0);
    set(&osc, oscilloscope::TIME, 25.0);

    // Read parameters back
    for (device, id_str) in [(&spec, "Spectrum"), (&tun, "Tuner"), (&osc, "Oscilloscope")] {
        let read = call(
            &mut client,
            "run",
            json!({ "commands": [{ "command": "device.parameters", "args": { "device": device } }] }),
        );
        let params = read["results"][0]["result"]["params"].as_array().unwrap();
        assert!(!params.is_empty(), "{id_str} has parameters");
    }

    // Readout / display command
    for device in [&spec, &tun, &osc] {
        let read = call(
            &mut client,
            "run",
            json!({ "commands": [{ "command": "device.display", "args": { "device": device } }] }),
        );
        let display = &read["results"][0]["result"];
        assert_eq!(display["live"], json!(false));
    }

    // Save and reopen
    let saved = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as",
                               "args": { "path": dir.0.join("Measurement.blset").to_string_lossy() } }] }),
    );
    let (project, _) = bl_project::load(saved_path(&saved)).unwrap();
    let devices: Vec<_> = project
        .document
        .tracks()
        .flat_map(|t| t.devices().cloned().collect::<Vec<_>>())
        .collect();
    let find = |kind: &str| {
        devices
            .iter()
            .find(|d| {
                matches!(&d.kind, bl_model::DeviceKind::Native { device_type } if device_type == kind)
            })
            .unwrap_or_else(|| panic!("no {kind} in the reopened set"))
    };

    assert_eq!(
        find(spectrum_analyser::SPECTRUM).params[&spectrum_analyser::BLOCK],
        2.0
    );
    assert_eq!(find(tuner::TUNER).params[&tuner::REFERENCE], 432.0);
    assert_eq!(
        find(oscilloscope::OSCILLOSCOPE).params[&oscilloscope::TIME],
        25.0
    );
}

// Spec: device-chains R39; IMP-25. Deactivate All Devices through the agent
// API: it switches off every top-level device on the tracks it is given, and
// switches them back on when they are all off already. The effect reads back
// through the track's own `devices`.
#[test]
fn deactivate_all_devices_switches_a_tracks_devices_through_the_agent_api() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("devices").unwrap();
    let deletes: Vec<Value> = default_tracks(&mut client)
        .into_iter()
        .map(|track| json!({ "command": "track.delete", "args": { "track": track } }))
        .collect();
    call(&mut client, "run", json!({ "commands": deletes }));
    let tone = |index: usize| {
        json!({ "command": "device.insert",
                "args": { "track": {"$ref": "A"}, "index": index,
                          "device": { "id": {"$new": format!("tone {index}")}, "name": "Test Tone",
                                      "kind": { "native": { "device_type": "beatlayer.test_tone" } },
                                      "enabled": true, "params": [] } } })
    };
    call(
        &mut client,
        "run",
        json!({ "commands": [
            json!({ "command": "track.create",
                    "args": { "id": {"$new": "A"}, "kind": "audio", "name": "A", "index": 0 } }),
            tone(0), tone(1),
        ] }),
    );
    let tracks = call(&mut client, "query", json!({ "path": "tracks" }));
    let track = tracks["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "audio")
        .expect("the new audio track")["id"]
        .clone();
    let devices = |client: &mut Client<InProcess>| -> Vec<Value> {
        let track = call(
            client,
            "query",
            json!({ "path": "track \"A\"", "detail": "full" }),
        );
        track["saved"]["devices"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    assert!(
        devices(&mut client).iter().all(|d| d["enabled"] == true),
        "both devices start on"
    );

    let run_one = |client: &mut Client<InProcess>| {
        call(
            client,
            "run",
            json!({ "commands": [{ "command": "track.set_devices_enabled",
                                   "args": { "tracks": [track] } }] }),
        )
    };
    let result = run_one(&mut client);
    assert_eq!(result["results"][0]["result"]["enabled"], false);
    assert!(
        devices(&mut client).iter().all(|d| d["enabled"] == false),
        "every top-level device switched off"
    );

    // Every device off: the same command turns them back on.
    let result = run_one(&mut client);
    assert_eq!(result["results"][0]["result"]["enabled"], true);
    assert!(
        devices(&mut client).iter().all(|d| d["enabled"] == true),
        "every top-level device back on"
    );
}

// Spec: device-chains R40; IMP-25. Close All Plug-in Windows through the
// agent API: it closes every open editor window at once, changes nothing in
// the set, and is never an undo step.
#[test]
fn close_all_plug_in_windows_closes_the_editors_through_the_agent_api() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("windows").unwrap();
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track");
    let loaded = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "plugin.load",
                               "args": { "path": bl_test_clap::bundle(), "track": track } }] }),
    );
    let device = loaded["results"][0]["result"]["device"].clone();
    let info = |client: &mut Client<InProcess>| -> Value {
        call(
            client,
            "run",
            json!({ "commands": [{ "command": "plugin.info", "args": { "device": device } }] }),
        )["results"][0]["result"]
            .clone()
    };

    client.transport().shell.start_audio();
    // The host activates the plug-in on its main thread while the shell
    // polls; a bounded poll loop drives it without waiting on the clock.
    let mut active = false;
    for _ in 0..10_000 {
        client.transport().shell.poll();
        if info(&mut client)["active"] == true {
            active = true;
            break;
        }
    }
    assert!(active, "the plug-in never activated");
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "plugin.show_window",
                               "args": { "device": device } }] }),
    );
    assert!(
        info(&mut client)["window_open"] == true,
        "the editor is open"
    );

    let result = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "view.close_plug_in_windows", "args": {} }] }),
    );
    assert_eq!(result["results"][0]["result"]["closed"], 1);
    assert!(
        info(&mut client)["window_open"] == false,
        "the editor closed"
    );
}

/// Spec: sample-editor R14, R15, R18. An agent sets every control of the
/// Clip View's Audio Utilities panel by command ID and reads each back in
/// the query tool's `clip`.
#[test]
fn an_agent_sets_the_audio_utilities_panel_and_reads_it_back() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Audio"}, "kind": "audio", "name": "Audio", "index": 0 } },
        ] }),
    );
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.name() == "Audio")
        .expect("the track was made")
        .id();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "clip.create",
              "args": { "track": track, "clip": {
                  "id": {"$new": "Tone"}, "name": "Tone", "start": 0, "length": 4 * BAR,
                  "mute": false, "data": { "audio": { "media": "cd".repeat(32) } } } } },
        ] }),
    );
    let clip = client
        .transport()
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .next()
        .expect("the clip was made")
        .id;
    let audio = |client: &mut Client<InProcess>| {
        call(client, "query", json!({ "id": clip, "detail": "full" }))["saved"]["data"]["audio"]
            .clone()
    };
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        call(
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        );
        audio(client)
    };

    // The Warp switch.
    let warp = run(
        &mut client,
        "clip.set_warp",
        json!({ "clip": clip, "warp": true }),
    );
    assert_eq!(warp["warp"], true);
    // The Warp Mode chooser, and the mode's own controls.
    let mode = run(
        &mut client,
        "clip.set_warp_mode",
        json!({ "clip": clip, "mode": "texture" }),
    );
    assert_eq!(mode["warp_mode"], "texture");
    let settings = run(
        &mut client,
        "clip.set_warp_settings",
        json!({ "clip": clip, "texture": { "grain_size": 40.0, "flux": 30.0 } }),
    );
    assert_eq!(
        settings["warp_settings"],
        json!({ "texture": { "grain_size": 40.0, "flux": 30.0 } })
    );
    // The BPM field, and ×2 and ÷2.
    let bpm = run(
        &mut client,
        "clip.set_segment_bpm",
        json!({ "clip": clip, "bpm": 140.0 }),
    );
    assert_eq!(bpm["tempo"], 140.0);
    let scaled = run(
        &mut client,
        "clip.scale_tempo",
        json!({ "clip": clip, "factor": 2.0 }),
    );
    assert_eq!(scaled["tempo"], 280.0);
    // The Lead/Follow switch.
    let leader = run(
        &mut client,
        "clip.set_tempo_leader",
        json!({ "clip": clip, "leader": true }),
    );
    assert_eq!(leader["tempo_leader"], true);
    // RAM and HiQ.
    let ram = run(
        &mut client,
        "clip.set_ram_mode",
        json!({ "clip": clip, "ram": true }),
    );
    assert_eq!(ram["ram"], true);
    let hiq = run(
        &mut client,
        "clip.set_high_quality",
        json!({ "clip": clip, "high_quality": false }),
    );
    assert_eq!(hiq["high_quality"], false);
    // The Gain field, in decibels: the command takes a linear gain.
    let gain = run(
        &mut client,
        "clip.set_gain",
        json!({ "clip": clip, "gain": 0.5 }),
    );
    assert_eq!(gain["gain"], 0.5);
}
