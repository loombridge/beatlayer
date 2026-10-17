//! The channel EQ, the three-band EQ, the gate and the bus compressor in the
//! shell (WP-5.21; the channel-eq, three-band-eq, gate and bus-compressor
//! specs, R1): a set with each saved and reopened keeps every parameter,
//! including the bus compressor's Oversampling menu option and the gate's and
//! compressor's sidechain. Driven through `Shell::run`, as an agent drives
//! it, on bl-project's real set files.

use std::path::PathBuf;

use bl_devices::{bus_compressor_type, channel_eq_type, gate_type, three_band_eq_type};
use bl_engine::DeviceType;
use bl_model::{DeviceId, DeviceKind, Input, Tap, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::device_commands::DeviceAdded;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Spec: channel-eq R1, three-band-eq R1, gate R1, bus-compressor R1
#[test]
fn a_set_with_the_four_devices_reopens_with_every_parameter() {
    let scratch = Scratch(std::env::temp_dir().join(format!(
        "bl-eqs-dynamics-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )));
    std::fs::create_dir_all(&scratch.0).unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let tracks: Vec<_> = shell
        .editor()
        .document()
        .tracks()
        .filter(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .collect();
    let (track, key) = (tracks[0], tracks[1]);
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();

    // Each device is added by its type, every parameter moved off its
    // default, and the two with sidechains keyed from the other track.
    let types: [DeviceType; 4] = [
        channel_eq_type(),
        three_band_eq_type(),
        gate_type(),
        bus_compressor_type(),
    ];
    let mut expected: Vec<(DeviceId, Vec<(u32, f64)>)> = Vec::new();
    for device_type in &types {
        let added: DeviceAdded = serde_json::from_value(
            shell
                .run(
                    "device.add",
                    json!({ "device": {
                        "kind": device_type.kind, "name": device_type.name,
                        "role": device_type.role, "sidechain": device_type.sidechain } }),
                )
                .unwrap(),
        )
        .unwrap();
        let mut values = Vec::new();
        for param in &device_type.description.params {
            let to = if param.default == param.max() {
                param.min()
            } else {
                param.max()
            };
            shell
                .run(
                    "device.set_param",
                    json!({ "device": added.device, "param": param.id.0, "value": to }),
                )
                .unwrap();
            values.push((param.id.0, to));
        }
        if device_type.sidechain {
            shell
                .run(
                    "device.set_sidechain_input",
                    json!({ "device": added.device,
                            "input": Input::Track { track: key, tap: Tap::PostFx } }),
                )
                .unwrap();
            shell
                .run(
                    "device.set_sidechain_enabled",
                    json!({ "device": added.device, "enabled": true }),
                )
                .unwrap();
        }
        expected.push((added.device, values));
    }

    let set = scratch.0.join("Song.blset");
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run("set.save_as", json!({ "path": set.to_string_lossy() }))
            .unwrap(),
    )
    .unwrap();
    let set = PathBuf::from(saved.path.expect("the set was saved"));
    shell
        .run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();

    for (device, values) in &expected {
        let (name, kept) = {
            let reopened = shell
                .editor()
                .document()
                .device(*device)
                .expect("the device");
            assert!(matches!(&reopened.kind, DeviceKind::Native { .. }));
            (reopened.name.clone(), reopened.params.clone())
        };
        for (param, value) in values {
            let kept = kept.get(&bl_model::ParamId(*param)).copied();
            assert_eq!(kept, Some(*value), "{name}: parameter {param}");
        }
        let read = shell
            .run("device.parameters", json!({ "device": device }))
            .unwrap();
        assert_eq!(
            read["params"].as_array().unwrap().len(),
            values.len(),
            "{name}: every parameter reads back"
        );
    }
    for device in expected.iter().map(|(d, _)| *d) {
        let reopened = shell.editor().document().device(device).unwrap();
        if let Some(sidechain) = &reopened.sidechain {
            assert!(sidechain.enabled);
            assert_eq!(
                sidechain.input,
                Input::Track {
                    track: key,
                    tap: Tap::PostFx
                }
            );
        }
    }
}
