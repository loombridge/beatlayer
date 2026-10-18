//! A device's sample slots (WP-5.17; the eight-slot drum sampler spec, R17):
//! the media list a set saves, a set saved before slots existed, and
//! `SetDeviceSample` with its slot.

use bl_core::id::Id;
use bl_model::{
    Command, ContentHash, Device, DeviceId, DeviceKind, Editor, MAX_MEDIA_SLOTS, TrackKind,
};
use serde_json::{Value, json};

fn hash(n: u8) -> ContentHash {
    ContentHash([n; 32])
}

fn device() -> Device {
    Device::new(
        DeviceId(Id::new(7).expect("7 is an ID")),
        "Impulse",
        DeviceKind::Native {
            device_type: "beatlayer.kitbag".to_owned(),
        },
    )
}

/// The device as a saved set holds it, with `media` as given.
fn saved_with(media: Option<Value>) -> Result<Device, serde_json::Error> {
    let mut saved = serde_json::to_value(device()).unwrap();
    match media {
        Some(media) => saved["media"] = media,
        None => {
            saved.as_object_mut().unwrap().remove("media");
        }
    }
    serde_json::from_value(saved)
}

// Spec: eight-slot-drum-sampler R17
#[test]
fn a_set_saved_before_sample_slots_loads_its_one_sample_as_slot_zero() {
    let one = saved_with(Some(json!(hash(3).to_string()))).unwrap();
    assert_eq!(one.media, [Some(hash(3))]);
    assert_eq!(one.media_at(0), Some(hash(3)));
    assert_eq!(saved_with(Some(Value::Null)).unwrap().media, []);
    assert_eq!(saved_with(None).unwrap().media, [], "no media, none");
    assert!(saved_with(Some(json!(5))).is_err(), "not a hash");
}

// Spec: eight-slot-drum-sampler R17
#[test]
fn the_slots_are_saved_as_a_list_with_gaps_and_a_device_with_none_saves_nothing() {
    let mut d = device();
    assert!(
        serde_json::to_value(&d).unwrap().get("media").is_none(),
        "left out of the saved set"
    );
    d.set_media(0, Some(hash(1)));
    d.set_media(3, Some(hash(4)));
    let saved = serde_json::to_value(&d).unwrap();
    assert_eq!(
        saved["media"],
        json!([hash(1).to_string(), null, null, hash(4).to_string()])
    );
    let reloaded: Device = serde_json::from_value(saved).unwrap();
    assert_eq!(reloaded, d);
    // Clearing the last slot trims the list; clearing an empty one does
    // nothing.
    d.set_media(3, None);
    assert_eq!(d.media, [Some(hash(1))]);
    d.set_media(5, None);
    assert_eq!(d.media, [Some(hash(1))]);
    d.set_media(0, None);
    assert!(d.media.is_empty());
}

// Spec: eight-slot-drum-sampler R17
#[test]
fn set_device_sample_sets_one_slot_as_one_undo_step() {
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
    let id = editor.new_id();
    let mut kit = device();
    kit.id = id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: kit,
        })
        .unwrap();
    let steps = editor.undo_history().len();
    let set = |editor: &mut Editor, media, slot| {
        editor
            .apply(Command::SetDeviceSample {
                device: id,
                media,
                slot,
            })
            .map(|_| ())
    };
    set(&mut editor, Some(hash(1)), 0).unwrap();
    set(&mut editor, Some(hash(2)), 2).unwrap();
    let media = |editor: &Editor| editor.document().device(id).unwrap().media.clone();
    assert_eq!(media(&editor), [Some(hash(1)), None, Some(hash(2))]);
    assert_eq!(editor.undo_history().len(), steps + 2, "a step for each");
    // The same sample again changes nothing and isn't a step.
    set(&mut editor, Some(hash(2)), 2).unwrap();
    assert_eq!(media(&editor), [Some(hash(1)), None, Some(hash(2))]);
    editor.undo().unwrap();
    assert_eq!(media(&editor), [Some(hash(1))]);
    // The slot is bounded.
    let top = u8::try_from(MAX_MEDIA_SLOTS).unwrap();
    assert!(set(&mut editor, Some(hash(9)), top - 1).is_ok());
    assert!(set(&mut editor, Some(hash(9)), top).is_err());
    assert_eq!(
        editor.document().device(id).unwrap().media.len(),
        MAX_MEDIA_SLOTS
    );
    // A command from before slots existed names no slot: it means slot 0.
    let old: Command = serde_json::from_value(json!({
        "op": "set_device_sample", "device": id, "media": hash(5).to_string()
    }))
    .unwrap();
    editor.apply(old).unwrap();
    assert_eq!(
        editor.document().device(id).unwrap().media_at(0),
        Some(hash(5))
    );
}
