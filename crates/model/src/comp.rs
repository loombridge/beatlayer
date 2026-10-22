//! Which take-lane material a track's main lane uses (the comping spec,
//! R20): the one matching that `take_lane.delete_unused`,
//! `take_lane.replace_with_take`, `take_lane.move_comp_edge` and
//! `take_lane.comp_sources` share.

use bl_core::time::{TempoMap, Ticks};

use crate::clip::{AudioData, Clip, ClipData, MidiData, Note};
use crate::ids::{ClipId, TakeLaneId};
use crate::track::Track;

/// How far two Start markers may differ and still be the same point of a
/// file: a cut at a tick and the same cut made from another clip round
/// their seconds differently.
const MARKER_TOLERANCE: f64 = 1e-6;

/// A stretch of a main-lane clip that plays what a take-lane clip plays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The main-lane clip.
    pub clip: ClipId,
    /// The take lane the take-lane clip sits on.
    pub lane: TakeLaneId,
    /// The take-lane clip.
    pub source: ClipId,
    /// Where the stretch starts.
    pub start: Ticks,
    /// Where it ends.
    pub end: Ticks,
}

/// Whether `main` plays what `take` plays over `from..to`, which both must
/// cover: the same data apart from the Start and End markers, the loop brace
/// and switch of an unlooped clip, and the fades; the same clip envelopes;
/// and the same contents at every point, for audio the same point of the same
/// file through the same warp map, for MIDI the same notes at the same places
/// in the song. A deactivated main-lane clip matches nothing.
#[must_use]
pub fn matches(main: &Clip, take: &Clip, from: Ticks, to: Ticks, tempo: &TempoMap) -> bool {
    if main.mute || main.envelopes != take.envelopes {
        return false;
    }
    match (&main.data, &take.data) {
        (ClipData::Audio(a), ClipData::Audio(b)) => same_audio(
            &a.moved_by(main.start, from, tempo),
            &b.moved_by(take.start, from, tempo),
        ),
        (ClipData::Midi(a), ClipData::Midi(b)) => {
            same_notes(a, main.content_start(), b, take.content_start(), from, to)
        }
        _ => false,
    }
}

/// Audio data compared as what it plays from a point on: the markers and the
/// fades of the clip it was cut from don't count, and a Start marker only to
/// within [`MARKER_TOLERANCE`].
fn same_audio(a: &AudioData, b: &AudioData) -> bool {
    let plain = |audio: &AudioData| AudioData {
        start: 0.0,
        end: None,
        loop_start: if audio.looping { audio.loop_start } else { 0.0 },
        loop_end: if audio.looping { audio.loop_end } else { None },
        fade_in: 0.0,
        fade_out: 0.0,
        ..audio.clone()
    };
    (a.start - b.start).abs() <= MARKER_TOLERANCE && plain(a) == plain(b)
}

/// The notes of a MIDI clip that sound in `from..to`, placed in the song and
/// in an order that doesn't depend on how the clip lists them.
fn sounding(data: &MidiData, content_start: Ticks, from: Ticks, to: Ticks) -> Vec<Note> {
    let mut notes: Vec<Note> = data
        .notes
        .iter()
        .map(|note| Note {
            start: note.start + content_start,
            ..note.clone()
        })
        .filter(|note| note.start < to && note.start + note.length > from)
        .collect();
    notes.sort_by_key(|note| (note.start, note.key, note.length));
    notes
}

fn same_notes(
    a: &MidiData,
    a_start: Ticks,
    b: &MidiData,
    b_start: Ticks,
    from: Ticks,
    to: Ticks,
) -> bool {
    sounding(a, a_start, from, to) == sounding(b, b_start, from, to)
}

/// Every stretch of the track's main-lane clips that plays what a take-lane
/// clip plays, in main-lane order and, within a clip, take-lane order.
#[must_use]
pub fn sources(track: &Track, tempo: &TempoMap) -> Vec<Source> {
    let mut mains: Vec<&Clip> = track.clips().collect();
    mains.sort_by_key(|clip| (clip.start, clip.id));
    let mut found = Vec::new();
    for main in mains {
        for lane in track.take_lanes() {
            for take in &lane.clips {
                let (start, end) = (main.start.max(take.start), main.end().min(take.end()));
                if start < end && matches(main, take, start, end, tempo) {
                    found.push(Source {
                        clip: main.id,
                        lane: lane.id,
                        source: take.id,
                        start,
                        end,
                    });
                }
            }
        }
    }
    found
}

/// The track's take lanes none of whose material the main lane uses.
#[must_use]
pub fn unused_lanes(track: &Track, tempo: &TempoMap) -> Vec<TakeLaneId> {
    let used = sources(track, tempo);
    track
        .take_lanes()
        .map(|lane| lane.id)
        .filter(|lane| used.iter().all(|source| source.lane != *lane))
        .collect()
}
