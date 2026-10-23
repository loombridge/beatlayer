//! The M2 exit demo, part 1 (T107): the built-in assistant builds a 16-bar
//! track from a prompt, and every change it made can be undone.
//!
//! Every transcript in `tests/fixtures/assistant/` is replayed, with no
//! model, through the loop the app runs (`bl-cli assistant`'s
//! `run_headless`) from the factory default set. Each must leave a 16-bar
//! track at the tempo the kept prompt asks for, and one undo must return
//! the document exactly to the factory set, and redo bring the track back.
//! A transcript recorded from a local or a cloud model is added by
//! `bl-cli assistant --record <file> --model <model> ... "$(cat prompt.txt)"`
//! and needs nothing else.
//!
//! `scripted.json` is the transcript of a script, not a model: the test
//! `record_the_scripted_transcript` records it through the same `--record`
//! path, so the replay and its checks run in CI before a model's transcript
//! is kept. Run it again (`cargo test -p bl-cli --test cli_tests -- m2_assistant::
//! --ignored`) when a command's result changes and a replay fails with a
//! drift diff.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use bl_assistant::keychain::{Keychain, MemoryKeychain};
use bl_assistant::message::{Event, Part, Reply, Request, Stop};
use bl_assistant::provider::{Error, OpenAi, Provider};
use bl_assistant::tools::{ToolCall, ToolSet};
use bl_cli::assistant::{Recording, Replay, Transcript, provider, run_headless};
use bl_devices::polysynth::POLYSYNTH;
use bl_devices::sampler::SAMPLER;
use bl_model::{Document, TrackKind};
use bl_shell::Shell;
use serde_json::{Value, json};

/// A quarter note, a bar of 4/4 and the track's length, in ticks.
const QUARTER: u64 = 705_600;
const BAR: u64 = 4 * QUARTER;
const BARS: u64 = 16;

/// The tempo the kept prompt asks for.
const TEMPO: f64 = 96.0;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/assistant")
}

/// The prompt kept beside the transcripts.
fn prompt() -> String {
    let text = fs::read_to_string(fixtures().join("prompt.txt")).expect("the kept prompt");
    text.trim().to_owned()
}

/// Every transcript kept, in name order.
fn transcripts() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(fixtures())
        .expect("the fixtures folder")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    found.sort();
    found
}

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-cli-m2-assistant-{name}-{}-{nanos}",
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

/// The document as the set file holds it, for comparing two states exactly.
fn serialized(shell: &Shell) -> Value {
    serde_json::to_value(shell.editor().document()).expect("a document serializes")
}

/// The turn's last undo step is undone, or redone, by command.
fn undo_redo(shell: &mut Shell, command: &str) {
    shell
        .run(command, json!({}))
        .unwrap_or_else(|error| panic!("{command} failed: {error}"));
}

// What the replay checks of the result.

/// Whether a device is a native instrument.
fn is_native_instrument(device: &bl_model::Device) -> bool {
    matches!(
        &device.kind,
        bl_model::DeviceKind::Native { device_type }
            if device_type == POLYSYNTH || device_type == SAMPLER
    )
}

/// The MIDI tracks that carry clips, as `(track, notes)`.
fn played_tracks(document: &Document) -> Vec<(&bl_model::Track, usize)> {
    document
        .tracks()
        .filter(|track| track.kind() == TrackKind::Midi)
        .filter_map(|track| {
            let mut notes = 0;
            let mut clips = 0;
            for clip in track.clips() {
                clips += 1;
                if let bl_model::ClipData::Midi(midi) = &clip.data {
                    notes += midi.notes.len();
                }
            }
            (clips > 0).then_some((track, notes))
        })
        .collect()
}

/// A 16-bar track at `TEMPO`: at least three MIDI tracks, each with a native
/// instrument and notes; clips that cover bars 1 to 16 and nothing beyond.
fn assert_a_16_bar_track(document: &Document) {
    let tempo = serde_json::to_value(&document.settings().tempo).expect("a tempo map");
    let segments = tempo.as_array().expect("a tempo map is a list");
    assert_eq!(segments.len(), 1, "one tempo: {tempo}");
    assert_eq!(segments[0]["bpm"], json!(TEMPO), "the tempo: {tempo}");

    let played = played_tracks(document);
    assert!(
        played.len() >= 3,
        "{} MIDI tracks with clips, wanted at least three",
        played.len()
    );
    for (track, notes) in &played {
        assert!(
            track.devices().any(is_native_instrument),
            "`{}` has no native instrument",
            track.name()
        );
        assert!(*notes > 0, "`{}` has no notes", track.name());
    }

    let mut spans: Vec<(i64, i64)> = played
        .iter()
        .flat_map(|(track, _)| track.clips())
        .map(|clip| {
            let start = clip.start.get();
            (start, start + clip.length.get())
        })
        .collect();
    spans.sort_unstable();
    assert_eq!(spans[0].0, 0, "the clips begin at bar 1");
    let mut reached = 0;
    for (start, end) in spans {
        assert!(start <= reached, "a gap before {start} (reached {reached})");
        reached = reached.max(end);
    }
    assert_eq!(
        reached,
        (BARS * BAR) as i64,
        "the clips end at the end of bar 16"
    );
}

/// Whether the tests run at full depth: `BL_TEST_DEPTH=full`, as on pushes
/// to main and the nightly run. Unset or `quick`, the default for local runs
/// and pull requests, they render the first two bars and the last of each
/// track rather than all sixteen, which takes a minute in a debug build.
fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Err(_) | Ok("" | "quick") => false,
        Ok(other) => panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full"),
    }
}

/// Renders each played track alone and checks none is silent in any bar of
/// what was rendered.
fn assert_no_track_is_silent(shell: &mut Shell) {
    // Each range is its first bar and its length in bars.
    let ranges: &[(u64, u64)] = if full_depth() {
        &[(1, BARS)]
    } else {
        &[(1, 2), (BARS, 1)]
    };
    let dir = TempDir::new("render");
    let tracks: Vec<(u64, String)> = played_tracks(shell.editor().document())
        .iter()
        .map(|(track, _)| (track.id().0.get(), track.name().to_owned()))
        .collect();
    for (id, name) in tracks {
        for &(first, bars) in ranges {
            let path = dir.0.join(format!("{id}-{first}.wav"));
            shell
                .run(
                    "export.audio",
                    json!({
                        "path": path.to_string_lossy(),
                        "bit_depth": 16,
                        "sample_rate": 44100,
                        "rendered_track": { "track": id },
                        "start": first.to_string(),
                        "length": bars * BAR,
                    }),
                )
                .unwrap_or_else(|error| panic!("rendering `{name}` failed: {error}"));
            let mut reader = hound::WavReader::open(&path).expect("the render");
            let spec = reader.spec();
            let samples: Vec<i16> = reader.samples::<i16>().map(Result::unwrap).collect();
            // A bar of 4 beats at the tempo, in samples of all channels.
            let bar = (4.0 * 60.0 / TEMPO * f64::from(spec.sample_rate)) as usize
                * usize::from(spec.channels);
            for index in 0..bars as usize {
                let window = &samples
                    [(index * bar).min(samples.len())..((index + 1) * bar).min(samples.len())];
                let peak = window.iter().map(|sample| sample.unsigned_abs()).max();
                assert!(
                    peak > Some(100),
                    "`{name}` is silent in bar {}",
                    first as usize + index
                );
            }
        }
    }
}

/// Replays one kept transcript from the factory default set and checks it.
fn replay_and_check(path: &Path) {
    let name = path.file_name().unwrap().to_string_lossy();
    let transcript = Transcript::read(path).unwrap_or_else(|error| panic!("{name}: {error}"));
    let mut shell = Shell::headless();
    let factory = serialized(&shell);

    // Every command the model called is registered.
    let tools = ToolSet::new(shell.registry().iter());
    let missing: Vec<&str> = transcript
        .exchanges
        .iter()
        .flat_map(|exchange| &exchange.response.tool_calls)
        .map(|call| call.name.as_str())
        .filter(|tool| tools.get(tool).is_none())
        .collect();
    assert!(missing.is_empty(), "{name}: unregistered tools {missing:?}");

    let replay = Replay::new(transcript.clone());
    let turn = run_headless(&mut shell, &replay, &transcript.model, false, &prompt())
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    replay
        .finished()
        .unwrap_or_else(|why| panic!("{name}: {why}"));
    assert!(turn.stopped.is_none(), "{name}: the turn was cut short");

    assert_a_16_bar_track(shell.editor().document());
    assert_no_track_is_silent(&mut shell);

    // One undo: exactly the factory set. Redo: the track again.
    let built = serialized(&shell);
    assert_ne!(built, factory, "{name}: the turn changed nothing");
    undo_redo(&mut shell, "edit.undo");
    assert_eq!(serialized(&shell), factory, "{name}: one undo isn't enough");
    undo_redo(&mut shell, "edit.redo");
    assert_eq!(serialized(&shell), built, "{name}: redo lost the track");
}

#[test]
fn every_kept_transcript_builds_a_16_bar_track_that_one_undo_removes() {
    let kept = transcripts();
    assert!(!kept.is_empty(), "no transcript is kept");
    for path in kept {
        replay_and_check(&path);
    }
}

#[test]
fn the_kept_prompt_asks_for_what_the_test_checks() {
    let prompt = prompt();
    assert!(prompt.contains(&format!("{TEMPO} BPM")), "{prompt}");
    assert!(prompt.contains("16-bar"), "{prompt}");
}

// Drift.

fn scripted() -> Transcript {
    Transcript::read(&fixtures().join("scripted.json")).expect("the scripted transcript")
}

#[test]
fn a_changed_tool_result_fails_the_replay_with_a_diff() {
    let mut transcript = scripted();
    let result = transcript
        .exchanges
        .iter_mut()
        .flat_map(|exchange| &mut exchange.messages)
        .flat_map(|message| &mut message.parts)
        .find_map(|part| match part {
            Part::Result(result) => Some(result),
            _ => None,
        })
        .expect("a recorded tool result");
    result.content.push_str(" (edited)");

    let mut shell = Shell::headless();
    let replay = Replay::new(transcript.clone());
    let error = run_headless(&mut shell, &replay, &transcript.model, false, &prompt())
        .expect_err("the replay should drift");
    let message = error.to_string();
    assert!(
        message.contains("drifted from the transcript at request 2"),
        "{message}"
    );
    assert!(
        message.contains("recorded:") && message.contains("(edited)"),
        "{message}"
    );
    assert!(message.contains("sent:"), "{message}");
}

#[test]
fn a_changed_prompt_fails_the_replay_at_the_first_request() {
    let transcript = scripted();
    let mut shell = Shell::headless();
    let replay = Replay::new(transcript.clone());
    let error = run_headless(
        &mut shell,
        &replay,
        &transcript.model,
        false,
        "Make a beat.",
    )
    .expect_err("the replay should drift");
    assert!(error.to_string().contains("at request 1"), "{error}");
}

// Permissions: no one to ask.

/// A provider that answers with a script, recording what it was sent.
struct Script {
    replies: Mutex<VecDeque<Reply>>,
    seen: Mutex<Vec<Request>>,
}

impl Script {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Provider for Script {
    fn stream(&self, request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error> {
        self.seen.lock().unwrap().push(request.clone());
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("the script ran out of replies");
        if !reply.text.is_empty() {
            on_event(Event::Text(reply.text.clone()));
        }
        for call in &reply.tool_calls {
            on_event(Event::ToolCall(call.clone()));
        }
        Ok(reply)
    }
}

fn call(id: &str, tool: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: tool.to_owned(),
        arguments,
    }
}

fn calls(calls: Vec<ToolCall>) -> Reply {
    Reply {
        text: String::new(),
        tool_calls: calls,
        stop: Stop::End,
    }
}

fn answer(text: &str) -> Reply {
    Reply {
        text: text.to_owned(),
        tool_calls: Vec::new(),
        stop: Stop::End,
    }
}

/// What the model was told by the last request: the results it holds.
fn last_results(script: &Script) -> Vec<String> {
    let seen = script.seen.lock().unwrap();
    let last = seen.last().expect("a request");
    last.messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            Part::Result(result) => Some(result.content.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_destructive_call_is_refused_without_autonomous_and_the_model_is_told() {
    let delete = |id: &str| calls(vec![call(id, "track_delete", json!({ "track": 2 }))]);
    for autonomous in [false, true] {
        let mut shell = Shell::headless();
        let tracks = shell.editor().document().tracks().count();
        let script = Script::new(vec![delete("c1"), answer("Done.")]);

        run_headless(&mut shell, &script, "m", autonomous, "Delete track 1.").unwrap();

        let results = last_results(&script);
        let remaining = shell.editor().document().tracks().count();
        if autonomous {
            assert_eq!(remaining, tracks - 1, "autonomous: the track went");
            assert!(!results[0].contains("declined"), "{results:?}");
        } else {
            assert_eq!(remaining, tracks, "refused: nothing changed");
            assert!(results[0].contains("declined"), "{results:?}");
        }
    }
}

// The key comes from the keychain alone.

/// A server that answers each request with the next of `bodies` as an SSE
/// stream, and hands back every request's headers.
struct Mock {
    url: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    fn new(bodies: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for body in bodies {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let (mut headers, mut length) = (String::new(), 0);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    headers.push_str(&line.to_ascii_lowercase());
                }
                let mut sent = vec![0; length];
                reader.read_exact(&mut sent).unwrap();
                log.lock().unwrap().push(headers);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { url, seen }
    }
}

/// An OpenAI-style stream that makes `calls` (id, tool, arguments), or says
/// `text`.
fn stream(text: &str, calls: &[(&str, &str, Value)]) -> String {
    let tool_calls: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(index, (id, name, arguments))| {
            json!({ "index": index, "id": id,
                "function": { "name": name, "arguments": arguments.to_string() } })
        })
        .collect();
    let mut delta = json!({ "content": text });
    if !tool_calls.is_empty() {
        delta["tool_calls"] = json!(tool_calls);
    }
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({ "choices": [{ "delta": delta, "finish_reason": "stop" }] })
    )
}

/// Wraps a keychain and lists the accounts read from it.
struct Reads {
    inner: MemoryKeychain,
    accounts: Mutex<Vec<String>>,
}

impl Keychain for Reads {
    fn read(&self, account: &str) -> Result<Option<String>, bl_assistant::keychain::KeychainError> {
        self.accounts.lock().unwrap().push(account.to_owned());
        self.inner.read(account)
    }
    fn write(&self, account: &str, key: &str) -> Result<(), bl_assistant::keychain::KeychainError> {
        self.inner.write(account, key)
    }
    fn delete(&self, account: &str) -> Result<(), bl_assistant::keychain::KeychainError> {
        self.inner.delete(account)
    }
}

#[test]
fn the_key_is_read_from_the_keychain_and_sent_as_the_providers_header() {
    let mock = Mock::new(vec![stream("Hello.", &[])]);
    let keychain = Arc::new(Reads {
        inner: MemoryKeychain::new(),
        accounts: Mutex::default(),
    });
    keychain
        .write(OpenAi::ACCOUNT, "sk-from-the-keychain")
        .unwrap();
    let api = provider(
        bl_shell::assistant::AssistantProvider::OpenAi,
        &mock.url,
        Arc::clone(&keychain) as Arc<dyn Keychain>,
    );
    let mut shell = Shell::headless();

    let turn = run_headless(&mut shell, api.as_ref(), "m", false, "Hi.").unwrap();

    assert_eq!(turn.reply.text, "Hello.");
    assert_eq!(*keychain.accounts.lock().unwrap(), [OpenAi::ACCOUNT]);
    let seen = mock.seen.lock().unwrap();
    assert!(
        seen[0].contains("authorization: bearer sk-from-the-keychain"),
        "{}",
        seen[0]
    );
}

#[test]
fn the_command_takes_no_key_and_the_binary_records_and_refuses() {
    // No option carries a key (the unit test in main.rs checks the flags);
    // run the binary against a mock, with and without --autonomous.
    for autonomous in [false, true] {
        let dir = TempDir::new("binary");
        let record = dir.0.join("turn.json");
        let mock = Mock::new(vec![
            stream("", &[("c1", "track_delete", json!({ "track": 2 }))]),
            stream("Done.", &[]),
        ]);
        let mut command = Command::new(env!("CARGO_BIN_EXE_bl-cli"));
        command
            .args(["assistant", "--model", "m", "--base-url", &mock.url])
            .arg("--record")
            .arg(&record);
        if autonomous {
            command.arg("--autonomous");
        }
        let output = command
            .arg("Delete track 1.")
            .output()
            .expect("bl-cli runs");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "Done.");

        let transcript = Transcript::read(&record).unwrap();
        assert_eq!(transcript.exchanges.len(), 2);
        let result = match &transcript.exchanges[1].messages[0].parts[0] {
            Part::Result(result) => result.content.clone(),
            other => panic!("expected a result, got {other:?}"),
        };
        assert_eq!(result.contains("declined"), !autonomous, "{result}");
    }
}

// The scripted model.

/// The notes of one bar-by-bar part: `(key, start, length, velocity)`.
type Notes = Vec<(u8, u64, u64, u8)>;

fn drums() -> Notes {
    let mut notes = Vec::new();
    for bar in 0..BARS {
        for beat in 0..4 {
            let start = bar * BAR + beat * QUARTER;
            notes.push((42, start, QUARTER / 4, 70));
            match beat {
                0 | 2 => notes.push((36, start, QUARTER / 2, 110)),
                _ => notes.push((38, start, QUARTER / 2, 100)),
            }
        }
    }
    notes
}

fn bass() -> Notes {
    (0..BARS)
        .flat_map(|bar| {
            [
                (33, bar * BAR, 2 * QUARTER, 100),
                (40, bar * BAR + 2 * QUARTER, 2 * QUARTER, 100),
            ]
        })
        .collect()
}

fn chords() -> Notes {
    // Am, F, C and G, a bar each, over and over: Drift's default patch
    // dies away within a few seconds, so a held chord would fall silent.
    let chords: [[u8; 3]; 4] = [[57, 60, 64], [53, 57, 60], [48, 52, 55], [55, 59, 62]];
    (0..BARS)
        .flat_map(|bar| {
            chords[(bar % 4) as usize]
                .iter()
                .map(move |key| (*key, bar * BAR, BAR, 80))
        })
        .collect()
}

fn lead() -> Notes {
    let melody = [69, 71, 72, 74, 76, 74, 72, 71];
    (0..BARS * 4)
        .map(|beat| (melody[(beat % 8) as usize], beat * QUARTER, QUARTER, 95))
        .collect()
}

fn midi_clip(name: &str, notes: &Notes) -> Value {
    let notes: Vec<Value> = notes
        .iter()
        .map(|(key, start, length, velocity)| {
            json!({ "key": key, "start": start, "length": length, "velocity": velocity })
        })
        .collect();
    json!({
        "id": { "$new": format!("{name}-clip") },
        "name": name,
        "start": 0,
        "length": BARS * BAR,
        "mute": false,
        "data": { "midi": { "notes": notes } },
    })
}

/// What a model that did the prompt's bidding would call, three replies of
/// parallel calls and an answer.
fn scripted_replies() -> Vec<Reply> {
    let parts: [(&str, Notes); 4] = [
        ("Drums", drums()),
        ("Bass", bass()),
        ("Chords", chords()),
        ("Lead", lead()),
    ];
    let mut tracks = vec![call(
        "call_1",
        "song_set_tempo",
        json!({ "tempo": [{ "kind": "constant", "start": 0, "bpm": TEMPO }] }),
    )];
    let mut devices = Vec::new();
    let mut clips = Vec::new();
    for (index, (name, notes)) in parts.iter().enumerate() {
        let key = name.to_lowercase();
        tracks.push(call(
            &format!("call_track_{key}"),
            "track_create",
            json!({ "id": { "$new": key }, "kind": "midi", "name": name, "index": 4 + index }),
        ));
        devices.push(call(
            &format!("call_device_{key}"),
            "device_add",
            json!({
                "device": {
                    "kind": { "native": { "device_type": POLYSYNTH } },
                    "name": "Drift",
                    "role": "instrument",
                },
                "track": { "$ref": key },
            }),
        ));
        clips.push(call(
            &format!("call_clip_{key}"),
            "clip_create",
            json!({ "track": { "$ref": key }, "clip": midi_clip(&key, notes) }),
        ));
    }
    vec![
        calls(tracks),
        calls(devices),
        calls(clips),
        answer(
            "I set the tempo to 96 BPM and made four MIDI tracks, Drums, Bass, Chords and Lead, \
             each with a Drift and a 16-bar clip. It is all one undo step.",
        ),
    ]
}

/// Records the scripted model's turn from the factory set to
/// `scripted.json`, through the recording `--record` uses.
#[test]
#[ignore = "rewrites tests/fixtures/assistant/scripted.json"]
fn record_the_scripted_transcript() {
    let mut shell = Shell::headless();
    let script = Script::new(scripted_replies());
    let recording = Recording::new(&script);
    run_headless(&mut shell, &recording, "scripted", false, &prompt()).unwrap();
    recording
        .transcript("open-ai", "")
        .write(&fixtures().join("scripted.json"))
        .unwrap();
}
