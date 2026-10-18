//! Exporting through the agent API (WP-1.12): a scripted MCP client
//! drives `bl-cli mcp`, builds a set from generated stems, and exports it,
//! and the file nulls against the set's render through the golden harness's
//! engine. Without `--autonomous`, the export, which writes a file outside
//! the project, is refused, as there's no one to ask.

bl_rt::install_trap!();

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use bl_agent::client::{Client, Stdio, Transport};
use bl_cli::golden::{BLOCK, SAMPLE_RATE};
use bl_cli::render::{Cue, Settings, interleave, media_store, render_with, to_i24};
use bl_project::ProjectFolder;
use serde_json::{Value, json};

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-cli-agent-export-{name}-{}-{nanos}",
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

/// Writes a short sine as a 16-bit WAV file: a stand-in stem. Test audio is
/// generated, never committed.
fn write_stem(path: &Path, hz: f64, seconds: f64) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..(seconds * 48_000.0) as u32 {
        let sample = (std::f64::consts::TAU * hz * f64::from(n) / 48_000.0).sin() * 0.2;
        writer
            .write_sample((sample * f64::from(i16::MAX)) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
}

/// Spec: export R15.
#[test]
fn an_agent_exports_the_set_it_built_and_the_file_nulls_against_its_render() {
    let dir = TempDir::new("built");
    let stems: Vec<PathBuf> = [(110.0, 2.0), (330.0, 1.5), (660.0, 3.0)]
        .iter()
        .enumerate()
        .map(|(index, (hz, seconds))| {
            let path = dir.0.join(format!("Stem {index}.wav"));
            write_stem(&path, *hz, *seconds);
            path
        })
        .collect();

    let mut client = bl_cli_mcp(&["--autonomous"]);
    // Drop the stems in, each on a track of its own, from bar 2.
    let paths: Vec<String> = stems
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "browser.add_to_arrangement",
                               "args": { "paths": paths, "at": 2_822_400, "spread": true } }] }),
    );
    // Turn the middle one down.
    let tracks = call(&mut client, "query", json!({ "path": "tracks" }));
    let middle = tracks["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|track| track["name"] == "Stem 1")
        .expect("the stem's track")["id"]
        .clone();
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "mixer.set_volume",
                               "args": { "track": middle, "volume": 0.5 } }] }),
    );

    // Export it: the whole set, as 24-bit WAV, with no dither, so it can
    // null to the bit.
    let file = dir.0.join("Agent Mix.wav");
    let exported = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "export.audio",
                               "args": { "path": file.to_string_lossy(), "bit_depth": 24,
                                         "sample_rate": 48000, "dither": "none" } }] }),
    );
    let result = &exported["results"][0]["result"];
    assert_eq!(result["range"], "set");
    assert_eq!(result["start"], 0);
    assert_eq!(result["file_type"], "wav");
    assert!(result["missing_media"].as_array().unwrap().is_empty());
    eprintln!(
        "agent export: {} frames, {:.0} times real time",
        result["frames"],
        result["speed"].as_f64().unwrap()
    );
    // The set is unchanged by it: nothing to undo but the two edits.
    let history = call(&mut client, "query", json!({ "path": "history" }));
    assert_eq!(history["undo"]["items"].as_array().unwrap().len(), 2);

    // Save the set, to render it here the way the golden harness renders.
    let saved = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as",
                               "args": { "path": dir.0.join("Agent.blset").to_string_lossy() } }] }),
    );
    let set = PathBuf::from(saved["results"][0]["result"]["path"].as_str().unwrap());
    assert!(client.into_transport().finish().unwrap().success());

    let (project, _) = bl_project::load(&set).unwrap();
    let document = &project.document;
    let end = bl_shell::export::song_end(document).unwrap();
    assert_eq!(result["end"], end.get());
    let frames = document
        .settings()
        .tempo
        .ticks_to_frames(end, SAMPLE_RATE)
        .get() as u64;
    let mut samples = Vec::new();
    let summary = render_with(
        document,
        media_store(&project, ProjectFolder::of_set(&set).root()),
        &[],
        &[Cue::PLAY],
        &Settings {
            sample_rate: SAMPLE_RATE,
            block: BLOCK,
            frames,
        },
        |output| interleave(output, &mut samples),
    );
    assert!(summary.missing_media.is_empty());
    assert!(summary.peak > 0.1, "the stems play");
    let expected: Vec<i32> = samples.iter().map(|&s| to_i24(s)).collect();
    let mut reader = hound::WavReader::open(&file).unwrap();
    assert_eq!(reader.spec().bits_per_sample, 24);
    let written: Vec<i32> = reader.samples::<i32>().map(Result::unwrap).collect();
    assert_eq!(written.len(), expected.len());
    assert!(written == expected, "the export nulls against the render");
}

/// Spec: export R15, R23 (WP-2.26). An agent exports the set as MP3 by
/// command, and the file is 320 kbps CBR at the export's rate.
#[test]
fn an_agent_exports_an_mp3_by_command() {
    let dir = TempDir::new("mp3");
    let stem = dir.0.join("Stem.wav");
    write_stem(&stem, 440.0, 1.0);

    let mut client = bl_cli_mcp(&["--autonomous"]);
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "browser.add_to_arrangement",
                               "args": { "paths": [stem.to_string_lossy()], "at": 0 } }] }),
    );

    let file = dir.0.join("Agent Mix.mp3");
    let exported = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "export.audio",
                               "args": { "path": file.to_string_lossy(), "bit_depth": 16,
                                         "sample_rate": 48000, "file_type": "mp3" } }] }),
    );
    let result = &exported["results"][0]["result"];
    assert_eq!(result["file_type"], "mp3");
    assert_eq!(result["sample_rate"], 48_000);
    assert_eq!(result["dither"], "none", "MP3 has no dither");
    assert!(client.into_transport().finish().unwrap().success());

    // The file is an MPEG-1 Layer III stream at 320 kbps, and decodes to
    // the frames the command reports.
    let bytes = fs::read(&file).unwrap();
    assert_eq!(bytes[0], 0xff, "a frame sync");
    assert_eq!((bytes[1] >> 3) & 3, 3, "MPEG-1");
    assert_eq!((bytes[1] >> 1) & 3, 1, "Layer III");
    assert_eq!(bytes[2] >> 4, 14, "the 320 kbps bit rate index");
    let decoder = bl_media::Decoder::open(&file).unwrap();
    assert_eq!(decoder.info().sample_rate, 48_000);
    let decoded = decoder.decode_to_end().unwrap();
    assert_eq!(decoded.frames() as u64, result["frames"].as_u64().unwrap());
    assert!(
        decoded.frames() > 40_000,
        "about a second: {}",
        decoded.frames()
    );
}

/// Exports the set as a 32-bit float WAV file by command, and returns its
/// left channel.
fn export_float(client: &mut Client<impl Transport>, path: &Path) -> Vec<f32> {
    call(
        client,
        "run",
        json!({ "commands": [{ "command": "export.audio",
                               "args": { "path": path.to_string_lossy(), "bit_depth": 32,
                                         "sample_rate": 48000 } }] }),
    );
    bl_media::decode_file(path)
        .unwrap()
        .channel(0)
        .unwrap()
        .to_vec()
}

/// Spec: export R15, offline-plugin-renders R1, R3, R11 (WP-4.21). An agent
/// loads a VST3 plug-in and, on macOS, an Audio Unit onto the track it built,
/// and `export.audio` renders through them by ID: the VST3 plug-in at a
/// gain of 0.5 halves the clip exactly, and the unit's echo isn't the clip
/// passed through.
#[test]
fn an_agent_exports_a_set_with_a_vst3_plugin_and_an_audio_unit_by_command() {
    let dir = TempDir::new("plugins");
    let stem = dir.0.join("Stem.wav");
    write_stem(&stem, 440.0, 1.0);

    let mut client = bl_cli_mcp(&["--autonomous"]);
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "browser.add_to_arrangement",
                               "args": { "paths": [stem.to_string_lossy()], "at": 0 } }] }),
    );
    let tracks = call(&mut client, "query", json!({ "path": "tracks" }));
    let track = tracks["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|track| track["name"] == "Stem")
        .expect("the stem's track")["id"]
        .clone();
    let dry = export_float(&mut client, &dir.0.join("dry.wav"));
    assert!(dry.iter().any(|s| *s != 0.0), "the stem plays");

    let loaded = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "plugin.load",
                               "args": { "path": bl_test_vst3::bundle(), "track": track } }] }),
    );
    let device = loaded["results"][0]["result"]["device"].clone();
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.set_param",
                               "args": { "device": device, "param": bl_test_vst3::GAIN,
                                         "value": 0.5 } }] }),
    );
    let halved = export_float(&mut client, &dir.0.join("halved.wav"));
    assert_eq!(halved.len(), dry.len());
    for (n, (out, input)) in halved.iter().zip(&dry).enumerate() {
        assert_eq!(out.to_bits(), (input * 0.5).to_bits(), "frame {n}");
    }

    #[cfg(target_os = "macos")]
    {
        let loaded = call(
            &mut client,
            "run",
            json!({ "commands": [{ "command": "plugin.load_au",
                                   "args": { "type": "aufx", "subtype": "dely",
                                             "manufacturer": "appl", "track": track } }] }),
        );
        let device = loaded["results"][0]["result"]["device"].clone();
        for (param, value) in [(0, 100.0), (1, 0.01)] {
            call(
                &mut client,
                "run",
                json!({ "commands": [{ "command": "device.set_param",
                                       "args": { "device": device, "param": param,
                                                 "value": value } }] }),
            );
        }
        let both = export_float(&mut client, &dir.0.join("both.wav"));
        let again = export_float(&mut client, &dir.0.join("again.wav"));
        assert_eq!(both.len(), dry.len());
        assert!(both.iter().any(|s| *s != 0.0), "the unit's echo plays");
        assert!(
            both.iter()
                .zip(&halved)
                .any(|(out, vst3_only)| out != vst3_only),
            "the unit isn't passed through"
        );
        assert!(
            both.iter()
                .zip(&again)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "the same every time"
        );
    }
    assert!(client.into_transport().finish().unwrap().success());
}

/// Spec: export R15, R24 (WP-4.17). An agent exports a range of a stem
/// through a reverb as a loop, by command: the file is the range long, and
/// holds the tail the same export without `render_as_loop` leaves out.
#[test]
fn an_agent_exports_a_loop_by_command() {
    let dir = TempDir::new("loop");
    let stem = dir.0.join("Stem.wav");
    write_stem(&stem, 440.0, 1.0);

    let mut client = bl_cli_mcp(&["--autonomous"]);
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "browser.add_to_arrangement",
                               "args": { "paths": [stem.to_string_lossy()], "at": 0 } }] }),
    );
    let tracks = call(&mut client, "query", json!({ "path": "tracks" }));
    let track = tracks["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|track| track["name"] == "Stem")
        .expect("the stem's track")["id"]
        .clone();
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.add",
                               "args": { "track": track,
                                         "device": { "kind": { "native": { "device_type": "beatlayer.rotunda" } },
                                                     "name": "Reverb", "role": "audio_effect" } } }] }),
    );

    // The first half second, 705,600 ticks to the quarter note.
    let export = |client: &mut Client<Stdio>, name: &str, looped: bool| {
        let file = dir.0.join(name);
        let exported = call(
            client,
            "run",
            json!({ "commands": [{ "command": "export.audio",
                                   "args": { "path": file.to_string_lossy(), "bit_depth": 32,
                                             "sample_rate": 48000, "start": 0, "length": 705_600,
                                             "render_as_loop": looped } }] }),
        );
        assert_eq!(exported["results"][0]["result"]["frames"], 24_000);
        fs::read(file).unwrap()
    };
    let plain = export(&mut client, "plain.wav", false);
    let looped = export(&mut client, "loop.wav", true);
    assert!(client.into_transport().finish().unwrap().success());
    assert_eq!(plain.len(), looped.len());
    assert_ne!(plain, looped, "the tail is folded in");
}

/// Spec: export R15.
#[test]
fn without_autonomous_an_export_is_refused_and_writes_nothing() {
    let dir = TempDir::new("refused");
    let mut client = bl_cli_mcp(&[]);
    let file = dir.0.join("Mix.wav");
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "export.audio",
                                   "args": { "path": file.to_string_lossy(), "bit_depth": 16,
                                             "sample_rate": 48000 } }] }),
        )
        .unwrap();
    assert!(refused.is_error);
    assert!(
        refused.text.contains("`export.audio` is external"),
        "{}",
        refused.text
    );
    assert!(refused.text.contains("--autonomous"), "{}", refused.text);
    assert!(!file.exists());
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0, "nothing written");
}
