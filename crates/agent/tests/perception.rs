//! Perception through the agent API (WP-1.16): a scripted client builds a
//! set from fixtures with known answers (a 1 kHz sine at −23 dBFS, a pink
//! noise burst, click tracks at known tempos, cadences in known keys),
//! previews it with the `preview` tool, and checks `analyze`'s numbers
//! against the reference values within the tolerances the agent
//! perception spec states. The fixtures are generated here
//! (`bl_analysis::signals`) and written as WAV files the browser adds, so
//! they play through the engine as any clip does.
//!
//! View captures need the app's window: headless the tool says so, and the
//! plumbing that hands a capture to the host and the image back is tested
//! with a stand-in host.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bl_agent::client::{Client, InProcess, ToolResult};
use bl_agent::{Agent, Captured, Outcome};
use bl_analysis::signals::{self, A_MINOR_CADENCE, C_MAJOR_CADENCE};
use bl_media::{AudioWriter, BitDepth, Dither, EncodeSpec, FileType};
use bl_shell::Shell;
use bl_shell::perception_commands::View;
use serde_json::{Value, json};

const RATE: u32 = 48_000;

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-agent-perception-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }

    /// Writes mono `samples` as a 32-bit float WAV file at 48 kHz.
    fn wav(&self, name: &str, samples: &[f32]) -> PathBuf {
        let path = self.0.join(format!("{name}.wav"));
        write_wav(&path, samples);
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_wav(path: &Path, samples: &[f32]) {
    let mut writer = AudioWriter::create(
        path,
        EncodeSpec {
            file_type: FileType::Wav,
            bit_depth: BitDepth::Float32,
            dither: Dither::None,
            sample_rate: RATE,
            channels: 1,
        },
    )
    .unwrap();
    writer.write(samples).unwrap();
    writer.finish().unwrap();
}

/// An agent on a headless shell, allowed to add files (external) without a
/// person to ask.
fn agent() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("listener").unwrap();
    client
}

fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{tool}: {error}"))
}

/// Adds the files to the Arrangement at 1.1.1, each on a track of its own,
/// and returns the tracks' IDs.
fn add(client: &mut Client<InProcess>, files: &[&Path]) -> Vec<u64> {
    let paths: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let result = call(
        client,
        "run",
        json!({ "commands": [{ "command": "browser.add_to_arrangement",
                               "args": { "paths": paths, "at": 0, "spread": true } }] }),
    );
    result["results"][0]["result"]["created_tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_u64().unwrap())
        .collect()
}

/// Previews the whole set, or `args`, and returns the result and the WAV
/// file's bytes.
fn preview(client: &mut Client<InProcess>, args: Value) -> (Value, Vec<u8>) {
    let result: ToolResult = client.call("preview", args).unwrap();
    let wav = result.attachment("audio/wav").expect("the preview's audio");
    (result.ok().unwrap(), wav)
}

fn analyze(client: &mut Client<InProcess>, args: Value) -> Value {
    call(client, "analyze", args)
}

fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("{value} isn't a number"))
}

/// Checks a measurement against its reference within a tolerance, and
/// prints it for the acceptance evidence.
fn within(what: &str, measured: f64, reference: f64, tolerance: f64) {
    println!("{what}: measured {measured}, reference {reference} ± {tolerance}");
    assert!(
        (measured - reference).abs() <= tolerance,
        "{what}: {measured} isn't within {tolerance} of {reference}"
    );
}

/// Spec: agent-perception R1, R2, R4, R5, R6.
#[test]
fn a_sine_at_minus_23_dbfs_previews_and_reads_minus_23_lufs() {
    let dir = TempDir::new("sine");
    let sine = signals::sine(1_000.0, signals::level(-23.0), 20.0, RATE, 1, 0.0);
    let file = dir.wav("Sine", &sine);
    let mut client = agent();
    add(&mut client, &[&file]);

    let (result, wav) = preview(&mut client, json!({}));
    // The preview is the whole set: 20 s, 16-bit stereo at the engine's
    // rate, as a WAV file.
    let reader = hound::WavReader::new(std::io::Cursor::new(&wav)).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.spec().sample_rate, RATE);
    assert_eq!(reader.spec().bits_per_sample, 16);
    assert_eq!(reader.duration(), 20 * RATE);
    assert_eq!(result["bytes"], wav.len());
    assert_eq!(result["rendered"]["range"], "set");
    assert_eq!(result["rendered"]["truncated"], false);
    assert_eq!(result["rendered"]["start_position"], "1.1.1");
    assert_eq!(
        result["uri"],
        format!("beatlayer://preview/{}.wav", result["id"])
    );

    let analysis = analyze(&mut client, json!({ "preview": result["id"] }));
    let loudness = &analysis["loudness"];
    within(
        "1 kHz sine at −23 dBFS: integrated loudness (LUFS)",
        number(&loudness["integrated_lufs"]),
        -23.0,
        0.1,
    );
    within(
        "1 kHz sine at −23 dBFS: short-term max (LUFS)",
        number(&loudness["short_term_max_lufs"]),
        -23.0,
        0.1,
    );
    within(
        "1 kHz sine at −23 dBFS: loudness range (LU)",
        number(&loudness["range_lu"]),
        0.0,
        0.1,
    );
    within(
        "1 kHz sine at −23 dBFS: true peak (dBTP)",
        number(&loudness["true_peak_dbtp"]),
        -23.0,
        0.2,
    );
    let levels = &analysis["levels"];
    within(
        "1 kHz sine at −23 dBFS: sample peak (dBFS)",
        number(&levels["sample_peak_dbfs"]),
        -23.0,
        0.05,
    );
    within(
        "1 kHz sine at −23 dBFS: RMS (dBFS)",
        number(&levels["rms_dbfs"]),
        -26.01,
        0.05,
    );
    let bands = analysis["spectrum"]["bands"].as_array().unwrap();
    assert_eq!(bands.len(), 10);
    let band = |hz: f64| {
        bands
            .iter()
            .find(|b| number(&b["centre_hz"]) == hz)
            .unwrap()["level_db"]
            .as_f64()
            .unwrap_or(-300.0)
    };
    within(
        "1 kHz sine at −23 dBFS: the 1 kHz octave band (dB)",
        band(1_000.0),
        -26.01,
        0.1,
    );
    for other in [
        31.5, 63.0, 125.0, 250.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
    ] {
        assert!(band(other) < -86.0, "{other} Hz: {}", band(other));
    }
    assert!(
        analysis["summary"]
            .as_str()
            .unwrap()
            .contains("integrated -23.0 LUFS"),
        "{}",
        analysis["summary"]
    );
}

/// Spec: agent-perception R5.
#[test]
fn a_pink_noise_burst_has_a_flat_octave_spectrum() {
    let dir = TempDir::new("pink");
    let noise = signals::pink_noise(-20.0, 10.0, RATE, 11);
    let file = dir.wav("Pink", &noise);
    let mut client = agent();
    add(&mut client, &[&file]);
    let (result, _) = preview(&mut client, json!({}));
    let analysis = analyze(
        &mut client,
        json!({ "preview": result["id"], "measure": ["spectrum", "levels"] }),
    );
    assert!(analysis["loudness"].is_null() && analysis["key"].is_null());
    let rms = number(&analysis["levels"]["rms_dbfs"]);
    within("pink noise at −20 dBFS RMS: RMS (dBFS)", rms, -20.0, 0.05);
    // Equal power in each octave: the ten bands from 22 Hz share the
    // noise's power from 20 Hz, about 10.1 dB under its RMS level each.
    let reference =
        rms + 10.0 * (f64::log2(24_000.0 / 22.1) / f64::log2(24_000.0 / 20.0) / 10.0).log10();
    for band in analysis["spectrum"]["bands"].as_array().unwrap() {
        let hz = number(&band["centre_hz"]);
        let tolerance = if hz < 50.0 { 1.5 } else { 1.0 };
        within(
            &format!("pink noise: the {hz} Hz octave band (dB)"),
            number(&band["level_db"]),
            reference,
            tolerance,
        );
    }
}

/// Spec: agent-perception R5, R6.
#[test]
fn click_tracks_read_their_tempo_and_their_clicks_where_they_are() {
    let dir = TempDir::new("clicks");
    for bpm in [120.0, 97.0] {
        let (clicks, starts) = signals::click_track(bpm, 0.25, 12.0, RATE);
        let file = dir.wav(&format!("Clicks {bpm}"), &clicks);
        let mut client = agent();
        add(&mut client, &[&file]);
        let (result, _) = preview(&mut client, json!({}));
        let analysis = analyze(
            &mut client,
            json!({ "preview": result["id"], "measure": ["tempo", "transients"] }),
        );
        within(
            &format!("click track at {bpm} BPM: tempo (BPM)"),
            number(&analysis["tempo"]["bpm"]),
            bpm,
            0.5,
        );
        let transients = &analysis["transients"];
        assert_eq!(transients["count"], starts.len(), "{transients}");
        let mut worst: f64 = 0.0;
        for (found, start) in transients["times_seconds"]
            .as_array()
            .unwrap()
            .iter()
            .zip(&starts)
        {
            worst = worst.max((number(found) - start).abs());
        }
        within(
            &format!("click track at {bpm} BPM: largest transient error (s)"),
            worst,
            0.0,
            0.005,
        );
        // The first click is a quarter of a second in: at the set's 120
        // BPM, the third sixteenth of bar 1.
        assert_eq!(transients["positions"][0], "1.1.3");
    }
}

/// Spec: agent-perception R5.
#[test]
fn cadences_are_heard_in_their_keys() {
    let dir = TempDir::new("keys");
    for (chords, key) in [
        (&C_MAJOR_CADENCE[..], "C major"),
        (&A_MINOR_CADENCE[..], "A minor"),
    ] {
        let tones = signals::progression(chords, 1.0, RATE);
        let file = dir.wav(key, &tones);
        let mut client = agent();
        add(&mut client, &[&file]);
        let analysis = analyze(&mut client, json!({ "measure": ["key"] }));
        println!("cadence in {key}: {}", analysis["key"]);
        assert_eq!(analysis["key"]["key"], key, "{}", analysis["key"]);
        assert!(analysis["preview"].is_null(), "a range rendered for it");
    }
}

/// Spec: agent-perception R3.
#[test]
fn one_track_previews_as_it_sounds_soloed() {
    let dir = TempDir::new("solo");
    let sine = dir.wav(
        "Sine",
        &signals::sine(1_000.0, signals::level(-23.0), 5.0, RATE, 1, 0.0),
    );
    let noise = dir.wav("Noise", &signals::pink_noise(-20.0, 5.0, RATE, 3));
    let mut client = agent();
    let tracks = add(&mut client, &[&sine, &noise]);
    let (alone, _) = preview(&mut client, json!({ "track": tracks[0] }));
    assert_eq!(alone["rendered"]["track"]["name"], "Sine");
    let alone = analyze(&mut client, json!({ "preview": alone["id"] }));
    within(
        "the sine track soloed in a mix with noise: integrated loudness (LUFS)",
        number(&alone["loudness"]["integrated_lufs"]),
        -23.0,
        0.1,
    );
    let mix = analyze(&mut client, json!({}));
    assert!(number(&mix["loudness"]["integrated_lufs"]) > -22.0, "{mix}");
    // The set itself is unchanged: nothing is soloed.
    let set = call(&mut client, "query", json!({ "path": "tracks" }));
    for track in set["tracks"]["items"].as_array().unwrap() {
        assert_ne!(track["mixer"]["solo"], true, "{track}");
    }
    // The Main track is the whole mix, and an unknown track is refused.
    let main = call(&mut client, "query", json!({ "path": "track main" }));
    let main = client
        .call("preview", json!({ "track": main["id"] }))
        .unwrap();
    assert!(main.is_error && main.text.contains("whole mix"), "{main:?}");
    let unknown = client.call("preview", json!({ "track": 999_999 })).unwrap();
    assert!(unknown.is_error, "{unknown:?}");
}

/// Spec: agent-perception R2.
#[test]
fn previews_stop_at_30_seconds_and_analyses_at_ten_minutes() {
    let dir = TempDir::new("long");
    let file = dir.wav(
        "Long",
        &signals::sine(440.0, signals::level(-12.0), 40.0, RATE, 1, 0.0),
    );
    let mut client = agent();
    add(&mut client, &[&file]);
    let (result, wav) = preview(&mut client, json!({ "sample_rate": 22_050 }));
    assert_eq!(result["rendered"]["truncated"], true);
    assert_eq!(result["rendered"]["seconds"], 30.0);
    // 30 s at 22,050 Hz from 1.1.1 at 120 BPM ends on bar 16.
    assert_eq!(result["rendered"]["end_position"], "16.1.1");
    let reader = hound::WavReader::new(std::io::Cursor::new(&wav)).unwrap();
    assert_eq!(reader.spec().sample_rate, 22_050);
    assert_eq!(reader.duration(), 30 * 22_050);
    let whole = analyze(&mut client, json!({ "measure": ["levels"] }));
    assert_eq!(whole["rendered"]["truncated"], false);
    within(
        "a 40 s range analysed whole: length (s)",
        number(&whole["rendered"]["seconds"]),
        40.0,
        1e-9,
    );
}

/// Spec: agent-perception R4.
#[test]
fn older_clients_get_the_audio_as_an_embedded_resource_and_any_client_can_read_it() {
    let dir = TempDir::new("resource");
    let file = dir.wav("Sine", &signals::sine(1_000.0, 0.1, 1.0, RATE, 1, 0.0));
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize_with("older", "2024-11-05").unwrap();
    add(&mut client, &[&file]);
    let result = client.call("preview", json!({})).unwrap();
    assert_eq!(
        result.content[1]["type"], "resource",
        "{:?}",
        result.content
    );
    let uri = result.content[1]["resource"]["uri"]
        .as_str()
        .unwrap()
        .to_owned();
    let embedded = result.attachment("audio/wav").unwrap();
    let (mime, read) = client.read_blob(&uri).unwrap();
    assert_eq!(mime, "audio/wav");
    assert_eq!(read, embedded);
    let listed = client.request("resources/list", json!({})).unwrap();
    assert!(
        listed["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["uri"] == uri),
        "{listed}"
    );

    // A newer client gets an audio block.
    let mut newer = agent();
    let file = dir.wav("Sine 2", &signals::sine(1_000.0, 0.1, 1.0, RATE, 1, 0.0));
    add(&mut newer, &[&file]);
    let result = newer.call("preview", json!({})).unwrap();
    assert_eq!(result.content[1]["type"], "audio");
    assert_eq!(result.content[1]["mimeType"], "audio/wav");
}

/// Spec: agent-perception R7.
#[test]
fn a_preview_renders_while_the_engine_plays_and_playback_carries_on() {
    let dir = TempDir::new("playing");
    let file = dir.wav(
        "Sine",
        &signals::sine(220.0, signals::level(-12.0), 20.0, RATE, 1, 0.0),
    );
    let mut shell = Shell::headless();
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("listener").unwrap();
    add(&mut client, &[&file]);
    let pump = |client: &mut Client<InProcess>| {
        let host = client.transport();
        host.agent.sync_engine(&mut host.shell);
    };
    pump(&mut client);
    call(&mut client, "transport", json!({ "action": "play" }));
    let until = Instant::now() + Duration::from_secs(10);
    let position = |client: &mut Client<InProcess>| {
        let state = client.transport().shell.audio_mut().transport_state();
        (state.playing, state.position)
    };
    loop {
        pump(&mut client);
        if position(&mut client).0 {
            break;
        }
        assert!(Instant::now() < until, "the song never started");
        std::thread::sleep(Duration::from_millis(5));
    }
    let before = position(&mut client).1;
    let dropouts = |client: &mut Client<InProcess>| {
        client
            .transport()
            .shell
            .audio()
            .stats()
            .map_or(0, |stats| stats.dropouts)
    };
    let dropouts_before = dropouts(&mut client);
    let (result, _) = preview(&mut client, json!({}));
    assert_eq!(result["rendered"]["seconds"], 20.0);
    std::thread::sleep(Duration::from_millis(50));
    pump(&mut client);
    let (playing, after) = position(&mut client);
    assert!(playing, "playback stopped");
    assert!(
        after > before,
        "the playhead didn't move: {before:?} then {after:?}"
    );
    // The device misses blocks while the preview renders only if its thread
    // has no real-time priority (such as a Linux CI runner without rtprio
    // limits or rtkit); on a thread that really runs with real-time priority,
    // no dropouts are expected.
    let realtime = client
        .transport()
        .shell
        .audio()
        .stats()
        .and_then(|stats| stats.audio_thread)
        .is_some_and(|report| report.priority.is_realtime());
    if realtime {
        assert_eq!(
            dropouts(&mut client),
            dropouts_before,
            "the device dropped out"
        );
    }
}

/// Spec: agent-perception R9.
#[test]
fn headless_a_view_capture_is_unavailable_and_says_why() {
    let mut client = agent();
    let result = client
        .call("capture_view", json!({ "view": "arrangement" }))
        .unwrap();
    assert!(result.is_error);
    assert!(result.text.contains("bl-cli mcp"), "{}", result.text);
    // Through `run`, the command says the tool takes it.
    let run = client
        .call(
            "run",
            json!({ "commands": [{ "command": "perception.capture_view",
                                   "args": { "view": "window" } }] }),
        )
        .unwrap();
    assert!(run.is_error && run.text.contains("capture_view"), "{run:?}");
    // A size out of range is refused before anything is asked of a host.
    let big = client
        .call(
            "capture_view",
            json!({ "view": "window", "max_size": 10_000 }),
        )
        .unwrap();
    assert!(big.is_error, "{big:?}");
}

/// Spec: agent-perception R8, R9.
#[test]
fn in_the_app_a_capture_waits_for_the_host_and_returns_its_image() {
    let mut shell = Shell::headless();
    let mut agent = Agent::headless(false);
    agent.set_view_capture(true);
    let session = agent.open_session(None);
    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                       "params": { "protocolVersion": "2025-11-25", "capabilities": {},
                                   "clientInfo": { "name": "looker" } } });
    assert!(matches!(
        agent.handle(&mut shell, &session, init),
        Outcome::Reply(Some(_))
    ));
    let capture = |id: u64, view: &str| {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": "capture_view", "arguments": { "view": view } } })
    };
    // The call waits for the host, which takes the request.
    let Outcome::Wait(ticket) = agent.handle(&mut shell, &session, capture(2, "mixer")) else {
        panic!("the capture didn't wait for the host");
    };
    let requests = agent.take_capture_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].ticket, ticket);
    assert_eq!(requests[0].view, View::Mixer);
    assert_eq!(requests[0].max_size, 1568);
    assert!(agent.take_capture_requests().is_empty());
    assert!(agent.resume(&mut shell).is_none(), "nothing answered yet");

    // The host answers with the image; the reply carries it.
    let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3];
    agent.finish_capture(
        ticket,
        Ok(Captured {
            png: png.clone(),
            width: 1200,
            height: 300,
            pixels_per_point: 1.5,
        }),
    );
    assert!(agent.holds_calls());
    let (answered, Outcome::Reply(Some(reply))) = agent.resume(&mut shell).unwrap() else {
        panic!("no reply");
    };
    assert_eq!(answered, ticket);
    assert_eq!(reply["id"], 2);
    let result = &reply["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["view"], "mixer");
    assert_eq!(result["structuredContent"]["width"], 1200);
    assert_eq!(result["content"][1]["type"], "image");
    assert_eq!(result["content"][1]["mimeType"], "image/png");
    assert_eq!(
        bl_agent::base64::decode(result["content"][1]["data"].as_str().unwrap()).unwrap(),
        png
    );

    // A view that isn't on screen comes back as the host's error.
    let Outcome::Wait(ticket) = agent.handle(&mut shell, &session, capture(3, "settings")) else {
        panic!("the capture didn't wait");
    };
    agent.take_capture_requests();
    agent.finish_capture(ticket, Err("the Settings window isn't open".to_owned()));
    let (_, Outcome::Reply(Some(reply))) = agent.resume(&mut shell).unwrap() else {
        panic!("no reply");
    };
    assert_eq!(reply["result"]["isError"], true);

    // When the window goes, waiting captures fail and new ones are refused.
    let Outcome::Wait(_) = agent.handle(&mut shell, &session, capture(4, "window")) else {
        panic!("the capture didn't wait");
    };
    agent.set_view_capture(false);
    let (_, Outcome::Reply(Some(reply))) = agent.resume(&mut shell).unwrap() else {
        panic!("no reply");
    };
    assert_eq!(reply["result"]["isError"], true);
    assert!(agent.take_capture_requests().is_empty());
    let Outcome::Reply(Some(reply)) = agent.handle(&mut shell, &session, capture(5, "window"))
    else {
        panic!("a capture without a window waited");
    };
    assert_eq!(reply["result"]["isError"], true);
}

/// Spec: agent-perception R7, WP-2.28.
/// In the app, a preview of a 50-track set defers and renders on a worker thread
/// while the shell runs commands on the main thread, keeps the main thread within
/// its frame budget, and matches a synchronous preview sample for sample.
#[test]
fn in_the_app_a_preview_defers_and_renders_on_a_worker_thread_while_the_shell_runs_commands() {
    let dir = TempDir::new("fifty-tracks-app");
    let file = dir.wav(
        "Tone",
        &signals::sine(440.0, signals::level(-12.0), 30.0, RATE, 1, 0.0),
    );
    let mut shell = Shell::headless();
    let mut agent = Agent::headless(true);
    agent.set_view_capture(true);
    let session = agent.open_session(None);
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "listener" }
        }
    });
    assert!(matches!(
        agent.handle(&mut shell, &session, init),
        Outcome::Reply(Some(_))
    ));

    // Delete initial default tracks so only our 50 audio tracks exist.
    let initial_tracks: Vec<bl_model::TrackId> = shell.editor().document().top_level().collect();
    for id in initial_tracks {
        let _ = shell.run("track.delete", json!({ "track": id }));
    }

    // Place the 30-second tone clip onto 50 separate tracks using spread: true.
    let paths = vec![file.to_str().unwrap().to_owned(); 50];
    let add_files = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "run", "arguments": {
            "commands": [{
                "command": "browser.add_to_arrangement",
                "args": { "paths": paths, "spread": true }
            }]
        }}
    });
    assert!(matches!(
        agent.handle(&mut shell, &session, add_files),
        Outcome::Reply(Some(_))
    ));
    let tracks_with_audio = shell
        .editor()
        .document()
        .tracks()
        .filter(|t| t.clips().len() > 0)
        .count();
    assert_eq!(tracks_with_audio, 50);

    // Reference synchronous preview run directly on the shell (row 2.28 acceptance).
    let sync_result = shell
        .run(bl_shell::perception_commands::PREVIEW, json!({}))
        .expect("synchronous preview succeeds");
    let sync_id = sync_result["id"].as_u64().expect("has preview id");
    let sync_samples = shell
        .previews()
        .get(sync_id)
        .expect("kept in shell")
        .samples
        .clone();

    // Call preview via agent API. In the app without CLAP devices, it defers (Outcome::Wait).
    let preview_call = json!({
        "jsonrpc": "2.0", "id": 200, "method": "tools/call",
        "params": { "name": "preview", "arguments": {} }
    });
    let init_start = Instant::now();
    let Outcome::Wait(ticket) = agent.handle(&mut shell, &session, preview_call) else {
        panic!("preview in the app didn't defer");
    };
    let mut requests = agent.take_render_requests();
    let preview_init_time = init_start.elapsed();
    assert_eq!(requests.len(), 1);
    let request = requests.pop().unwrap();
    assert_eq!(request.ticket, ticket);

    // Spawn the worker thread with name "bl-preview-worker".
    let (tx, rx) = std::sync::mpsc::channel();
    let worker_done = Arc::new(AtomicBool::new(false));
    let done_flag = Arc::clone(&worker_done);
    let worker = std::thread::Builder::new()
        .name("bl-preview-worker".to_owned())
        .spawn(move || {
            let result = request.execute();
            done_flag.store(true, Ordering::Relaxed);
            tx.send(result).unwrap();
        })
        .unwrap();

    // While the worker renders off-thread, run a main-thread loop measuring frame times.
    let mut max_frame_time = Duration::ZERO;
    let mut frame_count = 0;
    while !worker_done.load(Ordering::Relaxed) {
        let frame_start = Instant::now();
        let query_call = json!({
            "jsonrpc": "2.0", "id": 1000 + frame_count, "method": "tools/call",
            "params": { "name": "query", "arguments": { "target": "tracks" } }
        });
        assert!(matches!(
            agent.handle(&mut shell, &session, query_call),
            Outcome::Reply(Some(_))
        ));
        let frame_time = frame_start.elapsed();
        max_frame_time = max_frame_time.max(frame_time);
        frame_count += 1;
        std::thread::sleep(Duration::from_millis(1));
    }

    // While waiting for outcome receipt, run a concurrent edit command on the main thread.
    let edit_call = json!({
        "jsonrpc": "2.0", "id": 201, "method": "tools/call",
        "params": { "name": "run", "arguments": {
            "commands": [{ "command": "track.create", "args": { "kind": "midi", "name": "Concurrent" } }]
        }}
    });
    assert!(matches!(
        agent.handle(&mut shell, &session, edit_call),
        Outcome::Reply(Some(_))
    ));

    // Wait for worker to finish.
    let outcome = rx.recv().expect("worker finished");
    worker.join().unwrap();

    // Finish render and measure main-thread finish time.
    let finish_start = Instant::now();
    agent.finish_render(&mut shell, ticket, outcome);
    let finish_time = finish_start.elapsed();

    // Assert the structure: while the render is in flight (before finish_render),
    // the main thread handles several loop iterations and commands. That's what a stall would break.
    assert!(
        frame_count > 10,
        "main thread ran {frame_count} loop iterations while render was in flight"
    );

    // Wall-clock check to catch a stall (a main-thread render of this 50-track set takes seconds).
    // A 250 ms ceiling catches a regression where rendering moves back to the main thread,
    // while tolerating CI runner scheduling jitter across parallel tests.
    const STALL_LIMIT: Duration = Duration::from_millis(250);
    println!(
        "WP-2.28 frame times: preview initiation {preview_init_time:?}, main loop max frame {max_frame_time:?} ({frame_count} iterations), finish_render {finish_time:?}"
    );
    assert!(
        preview_init_time < STALL_LIMIT,
        "preview initiation ({preview_init_time:?}) exceeded stall limit {STALL_LIMIT:?}"
    );
    assert!(
        max_frame_time < STALL_LIMIT,
        "main-thread loop frame ({max_frame_time:?}) exceeded stall limit {STALL_LIMIT:?}"
    );
    assert!(
        finish_time < STALL_LIMIT,
        "finish_render ({finish_time:?}) exceeded stall limit {STALL_LIMIT:?}"
    );

    let (answered, Outcome::Reply(Some(reply))) = agent.resume(&mut shell).unwrap() else {
        panic!("no reply for finished render");
    };
    assert_eq!(answered, ticket);
    assert_eq!(reply["id"], 200);
    let result = &reply["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["rendered"]["seconds"], 30.0);
    assert_eq!(result["structuredContent"]["rendered"]["truncated"], false);
    let preview_id = result["structuredContent"]["id"].as_u64().unwrap();

    // The preview is kept in shell.previews.
    let kept_preview = shell.previews().get(preview_id).expect("kept in shell");
    let worker_samples = &kept_preview.samples;

    // Verify sample-for-sample equality against the synchronous preview of the same 50-track set.
    assert_eq!(worker_samples.len(), sync_samples.len());
    assert_eq!(worker_samples, &sync_samples);

    // Perception analyze still works on the kept preview.
    let analyze_call = json!({
        "jsonrpc": "2.0", "id": 202, "method": "tools/call",
        "params": { "name": "analyze", "arguments": { "preview": preview_id, "measure": ["levels"] } }
    });
    let Outcome::Reply(Some(analysis_reply)) = agent.handle(&mut shell, &session, analyze_call)
    else {
        panic!("analyze of kept preview didn't reply immediately");
    };
    assert_eq!(analysis_reply["result"]["isError"], false);
    assert!(
        analysis_reply["result"]["structuredContent"]["levels"]["sample_peak_dbfs"].is_number()
    );
}

/// Spec: agent-perception R7, WP-2.28.
/// If the window closes (view_capture becomes false) while a render is in flight,
/// finish_render does not add the preview to shell.previews.
#[test]
fn deferred_render_not_added_to_previews_if_window_closed_before_finish() {
    let mut shell = Shell::headless();
    let mut agent = Agent::headless(true);
    agent.set_view_capture(true);
    let session = agent.open_session(None);
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "listener" }
        }
    });
    assert!(matches!(
        agent.handle(&mut shell, &session, init),
        Outcome::Reply(Some(_))
    ));

    let preview_call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "preview", "arguments": { "bit_depth": 32 } }
    });
    let Outcome::Wait(ticket) = agent.handle(&mut shell, &session, preview_call) else {
        panic!("preview didn't defer");
    };

    let mut requests = agent.take_render_requests();
    let request = requests.pop().unwrap();
    assert_eq!(request.ticket, ticket);

    let initial_previews: Vec<u64> = shell.previews().ids().collect();

    // Window closes before render completes.
    agent.set_view_capture(false);

    // Worker completes the render off-thread.
    let outcome = request.execute();

    // Host reports finished render.
    agent.finish_render(&mut shell, ticket, outcome);

    // Previews must remain unchanged: no preview was added.
    let final_previews: Vec<u64> = shell.previews().ids().collect();
    assert_eq!(initial_previews, final_previews);
}

/// Spec: agent-perception R7, WP-2.28.
/// In the app, a range analyze defers and finishes through the worker thread.
#[test]
fn in_the_app_a_range_analyze_defers_and_finishes() {
    let mut shell = Shell::headless();
    let mut agent = Agent::headless(true);
    agent.set_view_capture(true);
    let session = agent.open_session(None);
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "listener" }
        }
    });
    assert!(matches!(
        agent.handle(&mut shell, &session, init),
        Outcome::Reply(Some(_))
    ));

    let analyze_call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "analyze", "arguments": { "start": 0, "length": 960 } }
    });
    let Outcome::Wait(ticket) = agent.handle(&mut shell, &session, analyze_call) else {
        panic!("range analyze in the app didn't defer");
    };

    let mut requests = agent.take_render_requests();
    assert_eq!(requests.len(), 1);
    let request = requests.pop().unwrap();
    assert_eq!(request.ticket, ticket);

    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("bl-preview-worker".to_owned())
        .spawn(move || {
            let result = request.execute();
            tx.send(result).unwrap();
        })
        .unwrap();

    let outcome = rx.recv().expect("worker finished");
    worker.join().unwrap();

    agent.finish_render(&mut shell, ticket, outcome);
    let (answered, Outcome::Reply(Some(reply))) = agent.resume(&mut shell).unwrap() else {
        panic!("no reply for finished range analyze");
    };
    assert_eq!(answered, ticket);
    assert_eq!(reply["result"]["isError"], false);
    assert!(reply["result"]["structuredContent"]["loudness"].is_object());
}
