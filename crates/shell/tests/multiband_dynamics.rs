//! Multiband Dynamics, the native multiband dynamics, in the shell (WP-5.22; the
//! multiband-dynamics spec, R17): a set with the device saves and reopens
//! with every parameter, and `device.parameters` reads them back. Driven
//! through `Shell::run`, as an agent drives it, on bl-project's real set
//! files.

bl_rt::install_trap!();

use std::path::PathBuf;

use bl_devices::multiband_dynamics::{MULTIBAND_DYNAMICS, multiband_dynamics_type};
use bl_model::{DeviceId, DeviceKind, DeviceRole, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::device_commands::{DeviceAdded, DeviceParameters};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-multiband-dynamics-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn parameters(shell: &mut Shell, device: DeviceId) -> DeviceParameters {
    serde_json::from_value(
        shell
            .run("device.parameters", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

// Spec: multiband-dynamics R17
#[test]
fn a_set_with_triband_reopens_with_every_parameter() {
    let scratch = Scratch::new("reopen");
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let audio = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track");
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({
                    "device": {
                        "kind": DeviceKind::Native { device_type: MULTIBAND_DYNAMICS.to_owned() },
                        "name": "Multiband Dynamics",
                        "role": DeviceRole::AudioEffect,
                    }
                }),
            )
            .unwrap(),
    )
    .unwrap();
    let device = added.device;

    // Every parameter moved off its default, to its range's other end.
    let description = multiband_dynamics_type().description;
    let mut wanted = Vec::new();
    for param in &description.params {
        let to = if param.default == param.max() {
            param.min()
        } else {
            param.max()
        };
        shell
            .run(
                "device.set_param",
                json!({ "device": device, "param": param.id, "value": to }),
            )
            .unwrap();
        wanted.push((param.id, to));
    }
    assert_eq!(wanted.len(), 45);

    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run(
                "set.save_as",
                json!({ "path": scratch.0.join("Song.blset").to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap();
    let path = saved.path.expect("the set was saved");
    shell.run("set.open", json!({ "path": path })).unwrap();

    let read = parameters(&mut shell, device);
    for (id, to) in wanted {
        let state = read
            .params
            .iter()
            .find(|p| p.descriptor.id == id)
            .unwrap_or_else(|| panic!("parameter {id:?} is read back"));
        assert_eq!(state.value, to, "{} reopened", state.descriptor.name);
    }
}
