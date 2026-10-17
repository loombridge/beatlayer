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

/// A clip on a track of a headless shell, saved as a set without a
/// plug-in, and again with the plug-in `load` puts on the clip's track:
/// the two sets' paths.
fn dry_and_wet_sets(
    scratch: &Scratch,
    load: impl FnOnce(&mut Shell, bl_model::TrackId),
) -> (String, String) {
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
    let save = |shell: &mut Shell, name: &str| {
        shell
            .run(SET_SAVE_AS, json!({ "path": scratch.0.join(name) }))
            .unwrap()["path"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let dry = save(&mut shell, "Dry.blset");
    load(&mut shell, track);
    let wet = save(&mut shell, "Wet.blset");
    (dry, wet)
}

/// Runs a command that loads a plug-in, then sets parameters of its device.
fn load_plugin(shell: &mut Shell, command: &str, args: serde_json::Value, params: &[(u32, f64)]) {
    let loaded: PluginLoaded = serde_json::from_value(shell.run(command, args).unwrap()).unwrap();
    for (param, value) in params {
        shell
            .run(
                "device.set_param",
                json!({ "device": loaded.device, "param": param, "value": value }),
            )
            .unwrap();
    }
}

// Spec: offline-plugin-renders R1
#[test]
fn bl_cli_export_renders_through_the_sets_plugin() {
    let scratch = Scratch::new("gain");
    // The same set saved twice: without the plug-in, and with it at a gain
    // of 0.5.
    let (dry_set, wet_set) = dry_and_wet_sets(&scratch, |shell, track| {
        load_plugin(
            shell,
            plugin_ids::LOAD,
            json!({ "path": bl_test_clap::bundle(), "track": track }),
            &[(GAIN, 0.5)],
        );
    });

    let dry = bl_cli_export(Path::new(&dry_set), &scratch.0.join("dry.wav"));
    let wet = bl_cli_export(Path::new(&wet_set), &scratch.0.join("wet.wav"));
    assert_eq!(dry.len(), wet.len());
    assert!(dry.iter().any(|s| *s != 0.0), "the clip plays");
    for (n, (out, input)) in wet.iter().zip(&dry).enumerate() {
        assert_eq!(out.to_bits(), (input * 0.5).to_bits(), "frame {n}");
    }
}

// Spec: offline-plugin-renders R1, R3, R11. With no audio device, the
// export makes the VST3 plug-in from the saved set and renders through it.
#[test]
fn bl_cli_export_renders_through_the_sets_vst3_plugin() {
    let scratch = Scratch::new("vst3");
    let (dry_set, wet_set) = dry_and_wet_sets(&scratch, |shell, track| {
        load_plugin(
            shell,
            plugin_ids::LOAD,
            json!({ "path": bl_test_vst3::bundle(), "track": track }),
            &[(bl_test_vst3::GAIN, 0.5)],
        );
    });

    let dry = bl_cli_export(Path::new(&dry_set), &scratch.0.join("dry.wav"));
    let wet = bl_cli_export(Path::new(&wet_set), &scratch.0.join("wet.wav"));
    assert_eq!(dry.len(), wet.len());
    assert!(dry.iter().any(|s| *s != 0.0), "the clip plays");
    for (n, (out, input)) in wet.iter().zip(&dry).enumerate() {
        assert_eq!(out.to_bits(), (input * 0.5).to_bits(), "frame {n}");
    }
}

// Spec: offline-plugin-renders R1, R3, R5, R11. The same with Apple's
// AUDelay, fully wet at 10 ms: not the clip passed through, and the same
// export after export.
#[cfg(target_os = "macos")]
#[test]
fn bl_cli_export_renders_through_the_sets_audio_unit() {
    let scratch = Scratch::new("au");
    let (dry_set, wet_set) = dry_and_wet_sets(&scratch, |shell, track| {
        load_plugin(
            shell,
            plugin_ids::LOAD_AU,
            json!({ "type": "aufx", "subtype": "dely", "manufacturer": "appl",
                    "track": track }),
            &[(0, 100.0), (1, 0.01)],
        );
    });

    let dry = bl_cli_export(Path::new(&dry_set), &scratch.0.join("dry.wav"));
    let wet = bl_cli_export(Path::new(&wet_set), &scratch.0.join("wet.wav"));
    let again = bl_cli_export(Path::new(&wet_set), &scratch.0.join("again.wav"));
    assert_eq!(dry.len(), wet.len());
    assert!(wet.iter().any(|s| *s != 0.0), "the unit's echo plays");
    assert!(
        wet.iter().zip(&dry).any(|(out, input)| out != input),
        "not passed through"
    );
    assert!(
        wet.iter()
            .zip(&again)
            .all(|(a, b)| a.to_bits() == b.to_bits()),
        "the same every time"
    );
}

/// Loads a CLAP and a VST3 plug-in, each at a gain of 0.5.
fn load_clap_and_vst3(shell: &mut Shell, track: bl_model::TrackId) {
    load_plugin(
        shell,
        plugin_ids::LOAD,
        json!({ "path": bl_test_clap::bundle(), "track": track }),
        &[(GAIN, 0.5)],
    );
    load_plugin(
        shell,
        plugin_ids::LOAD,
        json!({ "path": bl_test_vst3::bundle(), "track": track }),
        &[(bl_test_vst3::GAIN, 0.5)],
    );
}

// Spec: offline-plugin-renders R1, R3 (one loan across formats). A set with
// a CLAP and a VST3 device renders both: the clip at a quarter of its level.
#[test]
fn bl_cli_export_renders_a_set_with_a_clap_and_a_vst3_plugin() {
    let scratch = Scratch::new("clap-vst3");
    let (dry_set, wet_set) = dry_and_wet_sets(&scratch, load_clap_and_vst3);

    let dry = bl_cli_export(Path::new(&dry_set), &scratch.0.join("dry.wav"));
    let wet = bl_cli_export(Path::new(&wet_set), &scratch.0.join("wet.wav"));
    assert!(dry.iter().any(|s| *s != 0.0), "the clip plays");
    for (n, (out, input)) in wet.iter().zip(&dry).enumerate() {
        assert_eq!(out.to_bits(), (input * 0.25).to_bits(), "frame {n}");
    }
}

// Spec: offline-plugin-renders R1, R3, R11. A set with a CLAP, a VST3 and an
// Audio Unit device renders all three in one export: the unit's echo of the
// clip at a quarter of its level, as the unit alone gives it.
#[cfg(target_os = "macos")]
#[test]
fn bl_cli_export_renders_a_set_with_all_three_formats() {
    let scratch = Scratch::new("all-three");
    let load_au = |shell: &mut Shell, track| {
        load_plugin(
            shell,
            plugin_ids::LOAD_AU,
            json!({ "type": "aufx", "subtype": "dely", "manufacturer": "appl",
                    "track": track }),
            &[(0, 100.0), (1, 0.01)],
        );
    };
    let (_, unit_set) = dry_and_wet_sets(&scratch, load_au);
    let unit = bl_cli_export(Path::new(&unit_set), &scratch.0.join("unit.wav"));
    let scratch_all = Scratch::new("all-three-sets");
    let (_, all_set) = dry_and_wet_sets(&scratch_all, |shell, track| {
        load_clap_and_vst3(shell, track);
        load_au(shell, track);
    });
    let all = bl_cli_export(Path::new(&all_set), &scratch.0.join("all.wav"));
    assert!(unit.iter().any(|s| *s != 0.0), "the unit's echo plays");
    assert_eq!(all.len(), unit.len());
    for (n, (out, alone)) in all.iter().zip(&unit).enumerate() {
        assert!(
            (out - alone * 0.25).abs() <= 1e-6,
            "frame {n}: {out} against {alone}"
        );
    }
}
