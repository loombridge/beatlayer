//! Veer, the native Shifter, in the shell (WP-5.27; the
//! pitch-and-frequency-shifter spec, R13 and R14, and the routing spec,
//! R34): its MIDI source chooser lists the MIDI tracks as Live's choosers
//! list tracks, choosing is one undo step, and a set with the device saves
//! and reopens with every parameter and the source. Driven through
//! `Shell::run`, as an agent drives it.

bl_rt::install_trap!();

use std::path::PathBuf;

use bl_devices::shifter::{SHIFTER, shifter_type};
use bl_model::{DeviceId, DeviceKind, DeviceRole, TrackId, TrackKind};
use bl_shell::audio::AudioHost;
use bl_shell::device_commands::{DeviceAdded, DeviceParameters};
use bl_shell::files::ProjectFiles;
use bl_shell::routing::{self, Chooser};
use bl_shell::settings::Preferences;
use bl_shell::{Area, Shell};
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-shifter-{name}-{}-{}",
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

fn shell_in(scratch: &Scratch) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

/// Adds Veer to `track`, as `device.add` does with its type's flags.
fn add_veer(shell: &mut Shell, track: TrackId) -> DeviceId {
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": {
                    "kind": DeviceKind::Native { device_type: SHIFTER.to_owned() },
                    "name": "Veer",
                    "role": DeviceRole::AudioEffect,
                    "midi_sidechain": shifter_type().midi_sidechain,
                } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn choose(shell: &mut Shell, chooser: &Chooser, label: &str) {
    let run = chooser
        .run_for(label)
        .unwrap_or_else(|| panic!("{label} isn't a live entry of {:?}", chooser.labels()))
        .clone();
    shell.invoke(&run.id, run.args).unwrap();
}

fn tracks(shell: &Shell, kind: TrackKind) -> Vec<TrackId> {
    shell
        .editor()
        .document()
        .tracks()
        .filter(|t| t.kind() == kind)
        .map(bl_model::Track::id)
        .collect()
}

// Spec: routing R34
#[test]
fn the_midi_source_chooser_lists_the_midi_tracks_and_choosing_is_one_undo_step() {
    let mut shell = Shell::headless();
    let audio = tracks(&shell, TrackKind::Audio)[0];
    let midi = tracks(&shell, TrackKind::Midi);
    let device = add_veer(&mut shell, audio);
    let state = |shell: &Shell| {
        shell.device_view_state().devices[0]
            .midi_sidechain
            .clone()
            .map(|section| section.source)
    };

    // A new device: No Input; the MIDI tracks, then No Input.
    assert_eq!(state(&shell).as_deref(), Some("No Input"));
    let chooser = routing::midi_sidechain_source(shell.editor().document(), device).unwrap();
    assert_eq!(chooser.labels(), ["1-MIDI", "2-MIDI", "─", "No Input"]);
    assert_eq!(chooser.text, "No Input");

    choose(&mut shell, &chooser, "2-MIDI");
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Change \"Routing\"")
    );
    assert_eq!(state(&shell).as_deref(), Some("2-MIDI"));
    let doc = shell.editor().document();
    assert_eq!(
        doc.device(device)
            .unwrap()
            .midi_sidechain
            .clone()
            .unwrap()
            .source,
        Some(midi[1])
    );
    let chooser = routing::midi_sidechain_source(doc, device).unwrap();
    assert_eq!(chooser.text, "2-MIDI");
    choose(&mut shell, &chooser, "No Input");
    assert_eq!(state(&shell).as_deref(), Some("No Input"));

    // The strip's toggle shows the section, as a sidechain's does.
    assert!(
        !shell.device_view_state().devices[0]
            .midi_sidechain
            .clone()
            .unwrap()
            .shown
    );
    shell.invoke("device.show_sidechain", json!({})).unwrap();
    assert!(
        shell.device_view_state().devices[0]
            .midi_sidechain
            .clone()
            .unwrap()
            .shown
    );

    // A device without a MIDI sidechain has no chooser, and no source in its
    // frame.
    let sweep: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": {
                    "kind": DeviceKind::Native { device_type: "beatlayer.utility".to_owned() },
                    "name": "Utility",
                    "role": DeviceRole::AudioEffect,
                } }),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(routing::midi_sidechain_source(shell.editor().document(), sweep.device).is_none());
}

// Spec: pitch-and-frequency-shifter R14
#[test]
fn a_set_with_veer_reopens_with_every_parameter_and_its_midi_source() {
    let scratch = Scratch::new("reopen");
    let mut shell = shell_in(&scratch);
    let audio = tracks(&shell, TrackKind::Audio)[0];
    let keys = tracks(&shell, TrackKind::Midi)[0];
    let device = add_veer(&mut shell, audio);
    shell
        .run(
            "device.set_midi_sidechain_source",
            json!({ "device": device, "track": keys }),
        )
        .unwrap();

    // Every parameter moved off its default, to its range's other end.
    let mut wanted = Vec::new();
    for param in &shifter_type().description.params {
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
    assert_eq!(wanted.len(), 34);

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

    let read: DeviceParameters = serde_json::from_value(
        shell
            .run("device.parameters", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    for (id, to) in wanted {
        let state = read
            .params
            .iter()
            .find(|p| p.descriptor.id == id)
            .unwrap_or_else(|| panic!("parameter {id:?} is read back"));
        assert_eq!(state.value, to, "{} reopened", state.descriptor.name);
    }
    let reopened = shell.editor().document().device(device).unwrap();
    assert_eq!(
        reopened.midi_sidechain.clone().unwrap().source,
        Some(keys),
        "the MIDI source is saved with the set"
    );
}
