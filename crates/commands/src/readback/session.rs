//! The Session View: slots, scenes and launching.

use super::Object::{Clip, Scene, Song, Track};
use super::{Readback, query, read};

/// The readback of this area's commands, one line each, so a command's entry is easy to find.
#[rustfmt::skip]
pub(super) const ROWS: &[(&str, Readback)] = &[
    ("scene.create", query(Song, "scenes")),
    ("scene.delete", query(Song, "scenes")),
    ("scene.duplicate", query(Song, "scenes")),
    ("scene.move", query(Song, "scenes")),
    ("scene.rename", query(Scene, "name")),
    ("scene.set_follow_actions", query(Scene, "follow")),
    ("scene.set_tempo", query(Scene, "tempo")),
    ("scene.set_time_signature", query(Scene, "signature")),
    ("session.capture_and_insert_scene", query(Song, "scenes")),
    ("session.copy_clip_to_slot", query(Track, "slots")),
    ("session.copy_to_arrangement", query(Track, "clips")),
    ("session.copy_to_session", query(Track, "slots")),
    ("session.create_follow_action_chain", query(Clip, "launch.follow")),
    ("session.cut", query(Track, "slots")),
    ("session.deactivate", query(Clip, "mute")),
    ("session.delete", query(Track, "slots")),
    ("session.drop_files", query(Track, "slots")),
    ("session.duplicate", query(Track, "slots")),
    ("session.duplicate_scene", query(Song, "scenes")),
    ("session.insert_empty_midi_clip", query(Track, "slots")),
    ("session.insert_scene", query(Song, "scenes")),
    ("session.overdub", read("transport.state", "recording.session_slots")),
    ("session.paste", query(Track, "slots")),
    ("session.record", read("transport.state", "recording.session_slots")),
    ("session.record_slot", read("transport.state", "recording.session_slots")),
    ("session.toggle_follow_actions", query(Clip, "launch.follow.enabled")),
    ("slot.set_stop_button", query(Track, "removed_stop_buttons")),
];
