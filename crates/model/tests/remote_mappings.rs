//! The set's MIDI and key mappings (WP-7.1; the remote mapping spec, R1 to
//! R6): what a set saves, what a load drops, and what a deletion takes with
//! it in the same undo step.

use bl_model::{
    ControlKind, Command, Device, DeviceKind, DeviceRole, Document, Editor, MixerParam,
    ParamId, RemoteMapping, RemoteMode, RemoteSource, RemoteTarget, SceneId, TrackId, TrackKind,
};
use serde_json::json;

fn track(editor: &mut Editor, kind: TrackKind) -> TrackId {
    let id: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id,
            kind,
            name: "Track".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    id
}

fn scene(editor: &Editor) -> SceneId {
    editor.document().scenes().next().unwrap().id
}

fn note(key: u8) -> RemoteSource {
    RemoteSource::Note { channel: 1, key }
}

fn map(source: RemoteSource, target: RemoteTarget, range: Option<(f64, f64)>) -> RemoteMapping {
    let kind = target.natural_kind().unwrap_or(ControlKind::Continuous);
    RemoteMapping::new(source, target, kind, range, None).unwrap()
}

fn set(editor: &mut Editor, mappings: Vec<RemoteMapping>) {
    editor
        .apply(Command::SetRemoteMappings { mappings })
        .unwrap();
}

fn mappings(editor: &Editor) -> &[RemoteMapping] {
    &editor.document().settings().remote
}

// Spec: remote-mapping R1 to R4, R6. Every source kind and every kind of
// control comes back from a saved set as it was made.
#[test]
fn every_source_and_every_kind_of_control_round_trips_through_a_saved_set() {
    let mut editor = Editor::new();
    let audio = track(&mut editor, TrackKind::Audio);
    let ret = track(&mut editor, TrackKind::Return);
    let scene = scene(&editor);
    let device: bl_model::DeviceId = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track: audio,
            index: 0,
            device: Device::new(
                device,
                "Device".to_owned(),
                DeviceKind::Native {
                    device_type: "test.effect".to_owned(),
                },
            )
            .with_role(DeviceRole::AudioEffect),
        })
        .unwrap();
    let relative = RemoteMode::Relative {
        encoding: bl_model::RelativeEncoding::TwosComplement,
        accelerated: true,
    };
    let list = vec![
        map(note(36), RemoteTarget::Slot { track: audio, scene }, None),
        map(note(37), RemoteTarget::SceneLaunch { scene }, None),
        map(note(38), RemoteTarget::Solo { track: audio }, None),
        map(
            RemoteSource::Key { key: "A".to_owned() },
            RemoteTarget::Arm { track: audio },
            None,
        ),
        map(note(39), RemoteTarget::DeviceOn { device }, Some((20.0, 100.0))),
        map(note(40), RemoteTarget::CrossfadeAssign { track: audio }, None),
        RemoteMapping::new(
            RemoteSource::Cc { channel: 2, controller: 7 },
            RemoteTarget::Mixer { track: audio, param: MixerParam::Volume },
            ControlKind::Continuous,
            Some((0.0, 1.0)),
            Some(relative),
        )
        .unwrap(),
        RemoteMapping::new(
            RemoteSource::Cc14 { channel: 1, controller: 10 },
            RemoteTarget::Send { track: audio, to: ret },
            ControlKind::Continuous,
            Some((0.0, 1.0)),
            None,
        )
        .unwrap(),
        map(
            RemoteSource::PitchBend { channel: 3 },
            RemoteTarget::Crossfader,
            Some((0.0, 1.0)),
        ),
        RemoteMapping::new(
            RemoteSource::NoteRange { channel: 1, low: 48, high: 60, root: Some(55) },
            RemoteTarget::Device { device, param: ParamId(2) },
            ControlKind::Radio { options: 4 },
            None,
            None,
        )
        .unwrap(),
        map(note(41), RemoteTarget::TrackStop { track: audio }, None),
        map(note(42), RemoteTarget::StopAll, None),
    ];
    set(&mut editor, list.clone());

    let saved = serde_json::to_value(editor.document()).unwrap();
    let loaded: Document = serde_json::from_value(saved).unwrap();
    assert_eq!(loaded.settings().remote, list);
    assert_eq!(loaded.remote_dropped(), 0);
    assert_eq!(&loaded, editor.document());
}

// Spec: remote-mapping R6. A set saved before mappings loads with none, and
// a set with none writes none.
#[test]
fn a_set_with_no_mappings_writes_none_and_one_saved_before_them_loads_with_none() {
    let editor = Editor::new();
    let saved = serde_json::to_value(editor.document()).unwrap();
    assert!(saved["settings"].get("remote").is_none());
    let loaded: Document = serde_json::from_value(saved).unwrap();
    assert!(loaded.settings().remote.is_empty());
}

// Spec: remote-mapping R6. A mapping whose control a loaded set lacks is
// dropped and the load counts it.
#[test]
fn a_mapping_to_a_device_the_set_lacks_is_dropped_with_a_count() {
    let mut editor = Editor::new();
    let audio = track(&mut editor, TrackKind::Audio);
    set(
        &mut editor,
        vec![map(note(36), RemoteTarget::Solo { track: audio }, None)],
    );
    let mut saved = serde_json::to_value(editor.document()).unwrap();
    saved["settings"]["remote"] = json!([
        {
            "source": { "type": "note", "channel": 1, "key": 36 },
            "target": { "type": "solo", "track": audio },
            "kind": { "type": "switch" }, "min": 0.0, "max": 1.0
        },
        {
            "source": { "type": "note", "channel": 1, "key": 37 },
            "target": { "type": "device_on", "device": 987_654 },
            "kind": { "type": "ranged_switch" }, "min": 64.0, "max": 127.0
        }
    ]);
    let loaded: Document = serde_json::from_value(saved).unwrap();
    assert_eq!(loaded.settings().remote.len(), 1);
    assert_eq!(loaded.remote_dropped(), 1);
}

// Spec: remote-mapping R5, R6. A source maps one control and a control has
// one mapping of a kind; a mapping that breaks that doesn't load or apply.
#[test]
fn two_mappings_of_one_source_or_one_control_are_refused() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio);
    let b = track(&mut editor, TrackKind::Audio);
    let twice = vec![
        map(note(36), RemoteTarget::Solo { track: a }, None),
        map(note(36), RemoteTarget::Solo { track: b }, None),
    ];
    assert!(editor.apply(Command::SetRemoteMappings { mappings: twice }).is_err());
    let one_control = vec![
        map(note(36), RemoteTarget::Solo { track: a }, None),
        map(note(37), RemoteTarget::Solo { track: a }, None),
    ];
    assert!(editor.apply(Command::SetRemoteMappings { mappings: one_control }).is_err());
    // A key and a note may map the same control.
    let both = vec![
        map(note(36), RemoteTarget::Solo { track: a }, None),
        map(RemoteSource::Key { key: "S".to_owned() }, RemoteTarget::Solo { track: a }, None),
    ];
    set(&mut editor, both);
    assert_eq!(mappings(&editor).len(), 2);
}

// Spec: remote-mapping R6. Deleting a track takes its mappings in the same
// undo step, and undoing brings them back.
#[test]
fn deleting_a_track_a_scene_or_a_device_takes_its_mappings_and_undo_brings_them_back() {
    let mut editor = Editor::new();
    let keep = track(&mut editor, TrackKind::Audio);
    let gone = track(&mut editor, TrackKind::Audio);
    let scene = scene(&editor);
    let device: bl_model::DeviceId = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track: keep,
            index: 0,
            device: Device::new(
                device,
                "Device".to_owned(),
                DeviceKind::Native {
                    device_type: "test.effect".to_owned(),
                },
            )
            .with_role(DeviceRole::AudioEffect),
        })
        .unwrap();
    let list = vec![
        map(note(36), RemoteTarget::Solo { track: keep }, None),
        map(note(37), RemoteTarget::Solo { track: gone }, None),
        map(note(38), RemoteTarget::Slot { track: keep, scene }, None),
        map(note(39), RemoteTarget::DeviceOn { device }, None),
    ];
    set(&mut editor, list.clone());

    editor.apply(Command::DeleteTrack { track: gone }).unwrap();
    assert_eq!(mappings(&editor), &list[0..1].iter().chain(&list[2..]).cloned().collect::<Vec<_>>()[..]);
    editor.undo().unwrap();
    assert_eq!(mappings(&editor), &list[..], "one undo step brings the mapping back");

    editor.apply(Command::RemoveDevice { device }).unwrap();
    assert_eq!(mappings(&editor).len(), 3);
    editor.undo().unwrap();

    editor.apply(Command::DeleteScene { scene }).unwrap();
    assert!(!mappings(&editor).iter().any(|m| matches!(m.target, RemoteTarget::Slot { .. })));
    editor.undo().unwrap();
    assert_eq!(mappings(&editor), &list[..]);
}

// Spec: remote-mapping R6. Making a mapping is one undo step.
#[test]
fn changing_the_mappings_is_one_undo_step() {
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio);
    let steps = editor.undo_history().count();
    set(&mut editor, vec![map(note(36), RemoteTarget::Solo { track: a }, None)]);
    assert_eq!(editor.undo_history().count(), steps + 1);
    editor.undo().unwrap();
    assert!(mappings(&editor).is_empty());
}
