//! `bl-cli export` with a plug-in device (WP-2.6b; the offline plug-in
//! renders spec): with no audio device, the export makes the device's
//! plug-in from the saved set and renders through it, so the file holds the
//! plug-in's output, not its input passed through.

use std::path::{Path, PathBuf};
use std::process::Command;

use bl_shell::commands::SET_SAVE_AS;
use bl_shell::plugin_commands::{self as plugin_ids, PluginLoaded};
use bl_shell::{Shell, browser_commands};
use bl_test_clap::GAIN;
use serde_json::json;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-cli-export-plugins-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of one second at 48 kHz: a ramp.
fn wav(path: &Path) {
    let frames = 48_000u32;
    let data = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let sample = ((i % 100) as i16 - 50) * 200;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// Exports `set` with `bl-cli export` as a 32-bit float WAV file, and
/// returns its left channel.
fn bl_cli_export(set: &Path, output: &Path) -> Vec<f32> {
    let run = Command::new(env!("CARGO_BIN_EXE_bl-cli"))
        .args(["export", set.to_str().unwrap(), output.to_str().unwrap()])
        .args(["--bit-depth", "32", "--bars", "1"])
        .output()
        .expect("bl-cli runs");
    assert!(
        run.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    bl_media::decode_file(output)
        .unwrap()
        .channel(0)
        .unwrap()
        .to_vec()
}

// Spec: offline-plugin-renders R1
#[test]
fn bl_cli_export_renders_through_the_sets_plugin() {
    let scratch = Scratch::new("gain");
    let file = scratch.0.join("Ramp.wav");
    wav(&file);
    let mut shell = Shell::headless();
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 0 }),
        )
        .unwrap();
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.clips().next().is_some())
        .map(bl_model::Track::id)
        .expect("the clip's track");
    // The same set saved twice: without the plug-in, and with it at a gain
    // of 0.5.
    let dry_set = shell
        .run(SET_SAVE_AS, json!({ "path": scratch.0.join("Dry.blset") }))
        .unwrap()["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let loaded: PluginLoaded = serde_json::from_value(
        shell
            .run(
                plugin_ids::LOAD,
                json!({ "path": bl_test_clap::bundle(), "track": track }),
            )
            .unwrap(),
    )
    .unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": loaded.device, "param": GAIN, "value": 0.5 }),
        )
        .unwrap();
    let wet_set = shell
        .run(SET_SAVE_AS, json!({ "path": scratch.0.join("Wet.blset") }))
        .unwrap()["path"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(shell);

    let dry = bl_cli_export(Path::new(&dry_set), &scratch.0.join("dry.wav"));
    let wet = bl_cli_export(Path::new(&wet_set), &scratch.0.join("wet.wav"));
    assert_eq!(dry.len(), wet.len());
    assert!(dry.iter().any(|s| *s != 0.0), "the clip plays");
    for (n, (out, input)) in wet.iter().zip(&dry).enumerate() {
        assert_eq!(out.to_bits(), (input * 0.5).to_bits(), "frame {n}");
    }
}
