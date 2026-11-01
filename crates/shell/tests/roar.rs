//! Roar, the native Roar, in the shell (WP-8.38; the roar spec, R18): a set
//! with the device saves and reopens with every parameter, and
//! `device.parameters` reads them back. Driven through `Shell::run`, as an
//! agent drives it, on bl-project's real set files.

use std::path::PathBuf;

use bl_devices::roar::{ROAR, matrix_param, roar_type};
use bl_model::{DeviceId, DeviceKind, DeviceRole, Tap, TrackKind};
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
            "bl-roar-{name}-{}-{}",
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

// Spec: roar R18
#[test]
fn a_set_with_roar_reopens_with_every_parameter() {
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
                        "kind": DeviceKind::Native { device_type: ROAR.to_owned() },
                        "name": "Roar",
                        "role": DeviceRole::AudioEffect,
                    }
                }),
            )
            .unwrap(),
    )
    .unwrap();
    let device = added.device;

    // Every parameter moved off its default, to its range's other end.
    let description = roar_type().description;
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
    assert_eq!(wanted.len(), description.params.len());
    assert!(wanted.len() > 43, "part 2's parameters follow part 1's");

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

/// Adds a Roar to the first audio track as the Device View's browser does.
fn add_roar(shell: &mut Shell) -> DeviceId {
    let audio = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": {
                    "kind": DeviceKind::Native { device_type: ROAR.to_owned() },
                    "name": "Roar",
                    "role": DeviceRole::AudioEffect,
                    "sidechain": true,
                    "midi_sidechain": true,
                } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

// Spec: roar R36; routing R38
#[test]
fn the_device_views_state_reports_both_sidechain_sections() {
    let mut shell = Shell::headless();
    let device = add_roar(&mut shell);
    let doc = shell.editor().document();
    let saved = doc.device(device).unwrap();
    assert!(saved.sidechain.is_some() && saved.midi_sidechain.is_some());
    assert_eq!(
        saved.midi_sidechain.clone().unwrap().tap,
        Some(Tap::PostFx),
        "a new Roar's notes are taken Post FX"
    );
    let frame = shell.device_view_state().devices[0].clone();
    let (audio, midi) = (frame.sidechain.unwrap(), frame.midi_sidechain.unwrap());
    assert!(!audio.shown && !midi.shown);
    assert_eq!(midi.source, "No Input");
    assert_eq!(midi.tap, Some(Tap::PostFx));
    shell.invoke("device.show_sidechain", json!({})).unwrap();
    let frame = shell.device_view_state().devices[0].clone();
    assert!(frame.sidechain.unwrap().shown && frame.midi_sidechain.unwrap().shown);
}

// Spec: roar R35; routing R39
#[test]
fn a_set_with_roar_reopens_with_the_matrix_the_feedback_section_and_both_sidechains() {
    let scratch = Scratch::new("reopen-two");
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let device = add_roar(&mut shell);
    let keys = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    shell
        .run(
            "device.set_midi_sidechain_source",
            json!({ "device": device, "track": keys, "tap": "pre_fx" }),
        )
        .unwrap();
    shell
        .run(
            "device.set_sidechain_enabled",
            json!({ "device": device, "enabled": true }),
        )
        .unwrap();
    for (param, value) in [(matrix_param(1, 3), -42.0), (matrix_param(3, 30), 17.0)] {
        shell
            .run(
                "device.set_param",
                json!({ "device": device, "param": param, "value": value }),
            )
            .unwrap();
    }
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run(
                "set.save_as",
                json!({ "path": scratch.0.join("Song.blset").to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap();
    shell
        .run("set.open", json!({ "path": saved.path.expect("saved") }))
        .unwrap();
    let doc = shell.editor().document();
    let reopened = doc.device(device).unwrap();
    let midi = reopened.midi_sidechain.clone().unwrap();
    assert_eq!((midi.source, midi.tap), (Some(keys), Some(Tap::PreFx)));
    assert!(reopened.sidechain.clone().unwrap().enabled);
    let read = parameters(&mut shell, device);
    for (param, value) in [(matrix_param(1, 3), -42.0), (matrix_param(3, 30), 17.0)] {
        let state = read
            .params
            .iter()
            .find(|p| p.descriptor.id == param)
            .unwrap();
        assert_eq!(state.value, value);
    }
}
