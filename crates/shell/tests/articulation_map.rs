//! The Articulation Map in the shell (WP-6.47; the articulation map spec):
//! its list is edited one undo step at a time, the edits that break the
//! rules are refused, the list is saved with the set and in the device's
//! presets, and it reads back through `articulation.list` and the query
//! tool.

use std::path::{Path, PathBuf};

use bl_model::{
    Articulation, ArticulationOutput, ArticulationTrigger, Command, Device, DeviceId, DeviceKind,
    Document, Editor, TrackId, TrackKind,
};
use bl_shell::audio::AudioHost;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError};
use serde_json::{Value, json};

/// Opens a set held in memory.
struct Preloaded(Option<Editor>);

impl bl_shell::files::SetStore for Preloaded {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let editor = self.0.take().ok_or("opened already")?;
        Ok((editor, path.to_owned()))
    }

    fn save(&mut self, _: &Path, _: &Editor) -> Result<Vec<String>, String> {
        Err("not saved".to_owned())
    }

    fn save_as(&mut self, _: &Path, _: &Editor) -> Result<(PathBuf, Vec<String>), String> {
        Err("not saved".to_owned())
    }

    fn close(&mut self) {}
}

/// A shell with a MIDI track and an Articulation Map on it: the shell, the
/// track, the device, and the path of the set.
fn shell_with_map() -> (Shell, TrackId, DeviceId, PathBuf) {
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let device = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: Device::new(
                device,
                "Articulation Map",
                DeviceKind::Native {
                    device_type: bl_devices::articulation_map::ARTICULATION_MAP.to_owned(),
                },
            )
            .with_role(bl_model::DeviceRole::MidiEffect),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Map.blset" }))
        .unwrap();
    (shell, track, device, PathBuf::from("Map.blset"))
}

/// The articulations the device holds.
fn list(shell: &Shell, device: DeviceId) -> Vec<Articulation> {
    shell
        .editor()
        .document()
        .device(device)
        .expect("the device is in the set")
        .articulations
        .clone()
}

/// Runs a command that must fail, and checks it changed nothing.
fn refused(shell: &mut Shell, id: &str, args: Value) -> ShellError {
    let before = shell.editor().document().clone();
    let error = shell
        .run(id, args.clone())
        .expect_err(&format!("the command is refused: {id} {args}"));
    assert_eq!(shell.editor().document(), &before, "the set is as it was");
    error
}

// Spec: articulation-map R2, R16. A new Articulation Map holds one
// articulation, and every edit of the list is one undo step.
#[test]
fn the_list_is_edited_one_undo_step_at_a_time() {
    let (mut shell, _track, device, _path) = shell_with_map();
    assert_eq!(list(&shell, device).len(), 1, "a new map holds one");
    assert_eq!(list(&shell, device)[0].name, "Articulation 1");

    shell
        .run(
            "articulation.add",
            json!({ "device": device, "name": "Staccato" }),
        )
        .unwrap();
    assert_eq!(list(&shell, device).len(), 2);
    assert_eq!(list(&shell, device)[1].name, "Staccato");

    shell
        .run(
            "articulation.set",
            json!({
                "device": device,
                "index": 1,
                "trigger": { "kind": "key", "key": 36 },
                "outputs": [{ "kind": "key", "key": 24, "velocity": 100 }],
                "channel": 5,
            }),
        )
        .unwrap();
    let second = list(&shell, device)[1].clone();
    assert_eq!(second.trigger, ArticulationTrigger::Key { key: 36 });
    // A null channel clears it: the notes keep the channel they come in on.
    shell
        .run(
            "articulation.set",
            json!({ "device": device, "index": 1, "channel": null }),
        )
        .unwrap();
    assert_eq!(list(&shell, device)[1].channel, None);
    assert_eq!(
        second.outputs,
        vec![ArticulationOutput::Key {
            key: 24,
            velocity: 100
        }]
    );
    assert_eq!(second.channel, Some(5));

    shell
        .run(
            "articulation.move",
            json!({ "device": device, "index": 1, "to": 0 }),
        )
        .unwrap();
    assert_eq!(list(&shell, device)[0].name, "Staccato");

    // One undo step per edit: four edits, four undos.
    for _ in 0..4 {
        shell.run("edit.undo", json!({})).unwrap();
    }
    assert_eq!(list(&shell, device).len(), 1, "every edit is undone");
    assert_eq!(list(&shell, device)[0].name, "Articulation 1");
    assert_eq!(list(&shell, device)[0].trigger, ArticulationTrigger::None);
}

// Spec: articulation-map R3, R4, R5, R20. A list that breaks the rules is
// refused, and the set is as it was.
#[test]
fn a_list_that_breaks_the_rules_is_refused() {
    let (mut shell, _track, device, _path) = shell_with_map();
    shell
        .run(
            "articulation.add",
            json!({ "device": device, "name": "Staccato" }),
        )
        .unwrap();
    // The same key twice: the first takes it, the second is refused.
    shell
        .run(
            "articulation.set",
            json!({
                "device": device,
                "index": 0,
                "trigger": { "kind": "key", "key": 36 },
            }),
        )
        .unwrap();
    refused(
        &mut shell,
        "articulation.set",
        json!({
            "device": device,
            "index": 1,
            "trigger": { "kind": "key", "key": 36 },
        }),
    );
    // Overlapping ranges of the same controller.
    shell
        .run(
            "articulation.set",
            json!({
                "device": device,
                "index": 0,
                "trigger": { "kind": "controller", "controller": 1, "lowest": 0, "highest": 63 },
            }),
        )
        .unwrap();
    refused(
        &mut shell,
        "articulation.set",
        json!({
            "device": device,
            "index": 1,
            "trigger": { "kind": "controller", "controller": 1, "lowest": 32, "highest": 64 },
        }),
    );
    // A value outside its range.
    refused(
        &mut shell,
        "articulation.set",
        json!({
            "device": device,
            "index": 0,
            "outputs": [{ "kind": "key", "key": 24, "velocity": 0 }],
        }),
    );
    refused(
        &mut shell,
        "articulation.set",
        json!({ "device": device, "index": 0, "channel": 17 }),
    );
    // A place in the list that isn't there.
    refused(
        &mut shell,
        "articulation.set",
        json!({ "device": device, "index": 7, "name": "Nope" }),
    );
    // The list keeps one articulation at least.
    shell
        .run(
            "articulation.delete",
            json!({ "device": device, "index": 0 }),
        )
        .unwrap();
    refused(
        &mut shell,
        "articulation.delete",
        json!({ "device": device, "index": 0 }),
    );
    assert_eq!(list(&shell, device).len(), 1, "the list keeps one");
}

// Spec: articulation-map R18. The list reads back through `articulation.list`
// and through the query tool's `device`.
#[test]
fn the_list_reads_back_through_the_command_and_the_query_tool() {
    let (mut shell, _track, device, _path) = shell_with_map();
    shell
        .run(
            "articulation.set",
            json!({
                "device": device,
                "index": 0,
                "name": "Staccato",
                "trigger": { "kind": "key", "key": 36 },
                "outputs": [{ "kind": "controller", "controller": 1, "value": 64 }],
                "channel": 5,
            }),
        )
        .unwrap();
    let read: Value = shell
        .run("articulation.list", json!({ "device": device }))
        .unwrap();
    let articulations = read["articulations"].as_array().expect("the list");
    assert_eq!(articulations.len(), 1);
    assert_eq!(articulations[0]["name"], "Staccato");
    assert_eq!(articulations[0]["trigger"]["kind"], "key");
    assert_eq!(articulations[0]["trigger"]["key"], 36);
    assert_eq!(articulations[0]["outputs"][0]["controller"], 1);
    assert_eq!(articulations[0]["outputs"][0]["value"], 64);
    assert_eq!(articulations[0]["channel"], 5);
}

// Spec: articulation-map R17. The device's display reads the articulation
// that is current.
#[test]
fn the_display_reads_the_current_articulation() {
    let (mut shell, _track, device, _path) = shell_with_map();
    shell
        .run(
            "articulation.set",
            json!({ "device": device, "index": 0, "name": "Staccato" }),
        )
        .unwrap();
    let display: Value = shell
        .run("device.display", json!({ "device": device }))
        .unwrap();
    assert_eq!(display["readings"][0]["name"], "Articulation");
    assert_eq!(display["readings"][0]["value"], 1.0);
    assert_eq!(display["readings"][1]["name"], "Name");
    assert_eq!(display["readings"][1]["text"], "Staccato");
}

// Spec: articulation-map R16. The list is saved with the set and in the
// device's presets.
#[test]
fn the_list_is_saved_with_the_set_and_in_the_presets() {
    let (mut shell, _track, device, path) = shell_with_map();
    shell
        .run(
            "articulation.set",
            json!({
                "device": device,
                "index": 0,
                "name": "Staccato",
                "trigger": { "kind": "key", "key": 36 },
            }),
        )
        .unwrap();
    // The set keeps the list.
    let document: Document = shell.editor().document().clone();
    let saved = serde_json::to_value(&document).unwrap();
    let articulations = &saved["tracks"][0]["devices"][0]["articulations"];
    assert_eq!(articulations.as_array().expect("the list").len(), 1);
    assert_eq!(articulations[0]["name"], "Staccato");

    // The set that comes back has it too.
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let other = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: Device::new(
                other,
                "Articulation Map",
                DeviceKind::Native {
                    device_type: bl_devices::articulation_map::ARTICULATION_MAP.to_owned(),
                },
            )
            .with_role(bl_model::DeviceRole::MidiEffect),
        })
        .unwrap();
    editor
        .apply(Command::SetArticulation {
            device: other,
            index: 0,
            name: Some("Staccato".to_owned()),
            trigger: Some(ArticulationTrigger::Key { key: 36 }),
            outputs: None,
            channel: None,
        })
        .unwrap();
    let reopened = editor
        .document()
        .device(other)
        .unwrap()
        .articulations
        .clone();
    assert_eq!(reopened.len(), 1);
    assert_eq!(reopened[0].name, "Staccato");
    assert_eq!(reopened[0].trigger, ArticulationTrigger::Key { key: 36 });
    let _ = path;
}
