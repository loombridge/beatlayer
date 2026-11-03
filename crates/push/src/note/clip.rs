//! What Note Mode reads of the set: the selected track and what it is, the
//! clip the steps edit, its notes and where it plays
//! (`docs/specs/push-2-notes.md`, R1, R20 and R28).

use bl_control::ReadModel;
use bl_core::time::Ticks;
use bl_engine::SlotState;
use bl_model::{
    Clip, ClipData, ClipId, DeviceRole, MidiData, Note, RackKind, SceneId, TrackId, TrackKind,
};

use super::Kind;

/// The track the pads play: the first selected one.
pub fn selected_track(model: &ReadModel) -> Option<TrackId> {
    model
        .selection
        .tracks
        .first()
        .copied()
        .filter(|track| model.document.track(*track).is_some())
}

/// The scene the selection names: the selected scene, or the scene of the
/// first selected slot.
pub fn selected_scene(model: &ReadModel) -> Option<SceneId> {
    model
        .selection
        .scenes
        .first()
        .copied()
        .or_else(|| model.selection.slots.first().map(|(_, scene)| *scene))
}

/// What the pads are for on `track` (R1).
pub fn classify(model: &ReadModel, track: TrackId) -> Kind {
    let Some(t) = model.document.track(track) else {
        return Kind::Silent;
    };
    if t.kind() != TrackKind::Midi {
        return Kind::Silent;
    }
    let Some(instrument) = t.devices().find(|d| d.role == DeviceRole::Instrument) else {
        return Kind::Melodic;
    };
    if instrument.rack_kind() == Some(RackKind::Drum) {
        let drum = model.drum.filter(|d| d.rack == instrument.id);
        return Kind::Rack {
            rack: instrument.id,
            first: drum.map_or(36, |d| d.first),
            selected: drum.and_then(|d| d.selected),
            filled: drum.map_or(0, |d| d.filled),
        };
    }
    if let Some(view) = crate::simpler::View::of(model, instrument.id)
        && view.mode() == crate::simpler::Mode::Slice
    {
        // Each slice's key is a pad: the pads past the last are empty (R75).
        let filled = (0..view.slice_count()).fold(0u128, |all, slice| {
            all | 1 << (crate::simpler::FIRST_KEY as usize + slice)
        });
        return Kind::Slices {
            device: instrument.id,
            filled,
        };
    }
    Kind::Melodic
}

/// The MIDI clip in `track`'s slot at `scene`, if the slot holds one.
pub fn midi_clip(model: &ReadModel, track: TrackId, scene: SceneId) -> Option<ClipId> {
    let clip = model.document.slot(track, scene)?;
    matches!(clip.data, ClipData::Midi(_)).then_some(clip.id)
}

/// Whether `track`'s slot at `scene` holds a clip.
pub fn slot_holds(model: &ReadModel, track: TrackId, scene: SceneId) -> bool {
    model.document.slot(track, scene).is_some()
}

/// The clip `id` and its MIDI data.
pub fn midi(model: &ReadModel, id: ClipId) -> Option<(&Clip, &MidiData)> {
    let clip = model.document.clip(id)?;
    match &clip.data {
        ClipData::Midi(midi) => Some((clip, midi)),
        ClipData::Audio(_) => None,
    }
}

/// The clip's notes in the order the note commands count them: by start,
/// then pitch.
pub fn ordered(midi: &MidiData) -> Vec<&Note> {
    let mut notes: Vec<&Note> = midi.notes.iter().collect();
    notes.sort_by_key(|note| (note.start, note.key));
    notes
}

/// The places, in the commands' order, of the notes of pitch `key` that
/// start in `from` to `to`.
pub fn places(midi: &MidiData, key: u8, from: Ticks, to: Ticks) -> Vec<u32> {
    ordered(midi)
        .into_iter()
        .enumerate()
        .filter(|(_, note)| note.key == key && note.start >= from && note.start < to)
        .map(|(place, _)| place as u32)
        .collect()
}

/// The places of the notes of every pitch that start in `from` to `to`.
pub fn places_in(midi: &MidiData, from: Ticks, to: Ticks) -> Vec<u32> {
    ordered(midi)
        .into_iter()
        .enumerate()
        .filter(|(_, note)| note.start >= from && note.start < to)
        .map(|(place, _)| place as u32)
        .collect()
}

/// The part of the clip that plays and repeats: its loop brace while the loop
/// is on, else between its markers (R26, R29).
pub fn region(clip: &Clip, midi: &MidiData) -> (Ticks, Ticks) {
    if midi.looping {
        (midi.loop_start, midi.loop_end.unwrap_or(clip.length))
    } else {
        (midi.start, midi.end.unwrap_or(clip.length))
    }
}

/// Where in its contents the clip plays now, if it is playing: the time it
/// has played, brought into its loop (R23, R28).
pub fn playhead(
    model: &ReadModel,
    track: TrackId,
    scene: SceneId,
    clip: &Clip,
    midi: &MidiData,
) -> Option<Ticks> {
    model.slots.iter().find(|status| {
        status.track == track && status.scene == scene && status.state == SlotState::Playing
    })?;
    let since = model.clip_time.iter().find(|(t, _)| *t == track)?.1;
    let (from, to) = region(clip, midi);
    let at = midi.start + since;
    if to <= from {
        return Some(at);
    }
    Some(if at >= to {
        let span = to.get() - from.get();
        Ticks(from.get() + (at.get() - from.get()).rem_euclid(span))
    } else {
        at
    })
}
