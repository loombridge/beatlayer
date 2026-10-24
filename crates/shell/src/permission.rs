//! A call's permission class, decided from its arguments against the
//! document as it is (WP-1.15b; the agent permissions spec).
//!
//! Most commands have one class whatever their arguments: `track.delete`
//! is always destructive and `track.rename` always an edit. But in the
//! Arrangement a clip placed over part of another replaces the part it
//! covers, as in Live (the arrangement editing spec, R6), so moving,
//! resizing, nudging, duplicating, or pasting clips, dropping files from the
//! browser, or importing a MIDI file, can delete or shorten clips the call
//! doesn't name. Such a call is destructive as well as its registered class;
//! the same call into
//! empty space isn't. Likewise `clip.set_data` that would leave a MIDI
//! clip fewer notes than it has deletes notes (WP-1.15c). [`assess`] decides
//! which, and the agent API asks the person first when the answer is
//! destructive. The edit itself is the same either way: permissions only
//! decide who must agree to it.

use std::collections::BTreeSet;

use bl_commands::Permission;
use bl_core::time::Ticks;
use bl_model::{ClipData, ClipId, Document, Editor, TrackId, TrackKind};
use bl_shell_spec::audio_editing as audio_ids;
use bl_shell_spec::audio_editing::CropArgs;
use bl_shell_spec::bounce::{self as bounce_ids, PasteBouncedAudioArgs};
use bl_shell_spec::browser::{self as browser_ids, AddToArrangementArgs};
use bl_shell_spec::comping::{self as comping_ids, MoveCompEdgeArgs};
use bl_shell_spec::editing as edit_ids;
use bl_shell_spec::midi_files::{self as midi_ids, MidiImportArgs};
use bl_shell_spec::midi_tools as tool_ids;
use bl_shell_spec::session_view as session_ids;
use serde_json::Value;

use crate::arrangement::ArrangementView;
use crate::arrangement::editing::{self, Clipboard};

/// `clip.set_data`: replaces what a clip plays, for a MIDI clip its whole
/// list of notes.
pub const SET_CLIP_DATA: &str = "clip.set_data";

/// The commands whose class can rise with their arguments: each is
/// destructive as well as its registered class when it would delete or
/// shorten clips it doesn't act on, or, for `clip.set_data`, leave a MIDI
/// clip fewer notes than it has.
pub const RAISED_BY_ARGUMENTS: &[&str] = &[
    tool_ids::APPLY,
    tool_ids::SET,
    tool_ids::SET_AUTO,
    tool_ids::RESEED,
    tool_ids::RANDOMIZE,
    tool_ids::ADD_CHORD,
    tool_ids::DELETE_CHORD,
    audio_ids::CROP,
    edit_ids::MOVE_CLIPS,
    edit_ids::RESIZE_CLIPS,
    edit_ids::NUDGE,
    edit_ids::DUPLICATE,
    edit_ids::PASTE,
    session_ids::PASTE,
    session_ids::DUPLICATE,
    session_ids::COPY_TO_ARRANGEMENT,
    session_ids::COPY_TO_SESSION,
    browser_ids::ADD_TO_ARRANGEMENT,
    browser_ids::LOAD_SELECTED,
    midi_ids::IMPORT,
    SET_CLIP_DATA,
    bounce_ids::PASTE_BOUNCED_AUDIO,
    comping_ids::COPY_TO_MAIN_LANE,
    comping_ids::REPLACE_WITH_TAKE,
    comping_ids::MOVE_COMP_EDGE,
];

/// Whether `id`'s class can rise with its arguments.
#[must_use]
pub fn raised_by_arguments(id: &str) -> bool {
    RAISED_BY_ARGUMENTS.contains(&id)
}

/// What one call of a command needs, decided from its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallPermission {
    /// The command's registered class.
    pub registered: Permission,
    /// The clips the call would delete or shorten without acting on them:
    /// those its clips would be placed over. Empty for a call that leaves
    /// every other clip as it is.
    pub replaces: Vec<ClipId>,
    /// Whether `replaces` lists clips the call may replace rather than ones
    /// it will: a drop from the browser, whose clips' lengths come from
    /// files that aren't read until the person allows it.
    pub may: bool,
    /// The MIDI clip the call would leave fewer notes than it has
    /// (`clip.set_data`, and `clip.crop` of a MIDI clip), if it would.
    pub fewer_notes: Option<FewerNotes>,
    /// The clip a crop would leave holding less than it does
    /// (`clip.crop`), if it would: audio outside the region it keeps.
    pub cropped: Option<ClipId>,
}

/// A MIDI clip a call would leave fewer notes than it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FewerNotes {
    /// The clip.
    pub clip: ClipId,
    /// How many notes it has.
    pub has: usize,
    /// How many it would have.
    pub leaves: usize,
}

impl CallPermission {
    /// A call whose class is the command's registered class.
    #[must_use]
    pub fn registered(class: Permission) -> Self {
        Self {
            registered: class,
            replaces: Vec::new(),
            may: false,
            fewer_notes: None,
            cropped: None,
        }
    }

    /// Whether its arguments make the call destructive whatever its
    /// registered class: it replaces other clips, leaves a clip fewer
    /// notes, or crops audio away.
    fn deletes_material(&self) -> bool {
        !self.replaces.is_empty() || self.fewer_notes.is_some() || self.cropped.is_some()
    }

    /// Whether the call deletes or shortens other material, or deletes
    /// notes, so it's destructive.
    #[must_use]
    pub fn is_destructive(&self) -> bool {
        self.registered == Permission::Destructive || self.deletes_material()
    }

    /// Every class the call needs, least reach first: its registered class,
    /// and destructive when it replaces other clips or deletes notes.
    #[must_use]
    pub fn classes(&self) -> Vec<Permission> {
        let mut classes = vec![self.registered];
        if self.deletes_material() && !classes.contains(&Permission::Destructive) {
            classes.push(Permission::Destructive);
        }
        classes.sort_unstable();
        classes
    }

    /// The call's most far-reaching class, which decides whether the
    /// person is asked first.
    #[must_use]
    pub fn class(&self) -> Permission {
        self.classes().into_iter().max().unwrap_or(self.registered)
    }
}

/// What the shell knows that a call's targets can depend on.
pub(crate) struct Scene<'a> {
    pub(crate) editor: &'a mut Editor,
    pub(crate) view: &'a ArrangementView,
    pub(crate) clipboard: &'a Option<Clipboard>,
    /// The Session View's selection, which its clipboard commands act on.
    pub(crate) session: &'a crate::session_view::SessionView,
    pub(crate) insert_marker: Ticks,
    pub(crate) selected_track: Option<TrackId>,
    /// The clip the shell's Clip View shows, which a call naming no clip
    /// acts on (`clip.crop`).
    pub(crate) shown_clip: Option<ClipId>,
    /// The Clip View, whose MIDI Tools a tool's call is tried against.
    pub(crate) clip_view: &'a crate::clip_view::ClipView,
}

/// Decides what calling `id` with `args` needs, against the document as it
/// is. An Arrangement edit, or `clip.set_data`, is tried in a transaction
/// that's then cancelled, so nothing changes; a drop from the browser or a
/// MIDI file import is judged without reading its files. A call that would
/// fail needs only its registered class: it changes nothing.
pub(crate) fn assess(
    id: &str,
    args: &Value,
    registered: Permission,
    scene: Scene<'_>,
) -> CallPermission {
    if !raised_by_arguments(id) {
        return CallPermission::registered(registered);
    }
    match id {
        browser_ids::ADD_TO_ARRANGEMENT => {
            let Ok(drop) = serde_json::from_value::<AddToArrangementArgs>(args.clone()) else {
                return CallPermission::registered(registered);
            };
            let at = drop.at.unwrap_or(scene.insert_marker);
            dropped(scene.editor.document(), drop.track, at, registered)
        }
        browser_ids::LOAD_SELECTED => {
            // Spec: browser R14. The selected track if it holds audio,
            // otherwise a new one, at the insert marker.
            let doc = scene.editor.document();
            // A selected clip file may be a MIDI clip's, so a MIDI track counts
            // too (clip files R10): this errs towards destructive.
            let track = scene.selected_track.filter(|track| {
                doc.track(*track)
                    .is_some_and(|t| matches!(t.kind(), TrackKind::Audio | TrackKind::Midi))
            });
            dropped(doc, track, scene.insert_marker, registered)
        }
        midi_ids::IMPORT => {
            let Ok(import) = serde_json::from_value::<MidiImportArgs>(args.clone()) else {
                return CallPermission::registered(registered);
            };
            let at = import.at.unwrap_or(scene.insert_marker);
            dropped(scene.editor.document(), import.track, at, registered)
        }
        audio_ids::CROP => cropping(args, registered, scene),
        SET_CLIP_DATA => notes_set(args, registered, scene.editor),
        tool_ids::APPLY
        | tool_ids::SET
        | tool_ids::SET_AUTO
        | tool_ids::RESEED
        | tool_ids::RANDOMIZE
        | tool_ids::ADD_CHORD
        | tool_ids::DELETE_CHORD => tool_notes(id, args, registered, scene),
        bounce_ids::PASTE_BOUNCED_AUDIO => paste_bounced(args, registered, scene),
        _ => tried(id, args, registered, scene),
    }
}

/// A drop of files on `track` (or a new track, for `None`) at `at`. Its
/// clips last as long as their files, which aren't read yet, and each lasts
/// at least a tick, so any clip on the track that ends after `at` may be
/// covered.
fn dropped(
    doc: &Document,
    track: Option<TrackId>,
    at: Ticks,
    registered: Permission,
) -> CallPermission {
    let replaces = track
        .and_then(|track| doc.track(track))
        .map(|track| {
            track
                .clips()
                .filter(|clip| clip.end() > at)
                .map(|clip| clip.id)
                .collect()
        })
        .unwrap_or_default();
    CallPermission {
        registered,
        replaces,
        may: true,
        fewer_notes: None,
        cropped: None,
    }
}

/// `clip.crop`, decided from its arguments against the document: destructive
/// when it would remove anything (the audio clip editing spec, R18) — notes a
/// MIDI clip drops, or audio outside the region it keeps.
fn cropping(args: &Value, registered: Permission, scene: Scene<'_>) -> CallPermission {
    let unchanged = CallPermission::registered(registered);
    let Ok(args) = serde_json::from_value::<CropArgs>(args.clone()) else {
        return unchanged;
    };
    let doc = scene.editor.document();
    // The span and the clip are resolved as the shell resolves them (R16):
    // the call's, or the Arrangement's time selection and the clip it
    // covers. A span that can't be read, or no clip at all, leaves the
    // registered class as it is.
    let Ok(span) = crate::shell::audio_editing::crop_span(doc, scene.view, scene.shown_clip, &args)
    else {
        return unchanged;
    };
    let Some(clip) =
        crate::shell::audio_editing::crop_clip(doc, scene.view, scene.shown_clip, span, &args)
            .and_then(|id| doc.clip(id))
    else {
        return unchanged;
    };
    let removes = crate::shell::audio_editing::crop_removes(clip, span);
    match (&clip.data, removes) {
        (ClipData::Midi(midi), true) => {
            let (from, to) = match span {
                Some((start, end)) => (
                    midi.start + (start - clip.start),
                    midi.start + (end - clip.start),
                ),
                None => crate::shell::audio_editing::midi_region(clip, midi),
            };
            let leaves = midi
                .notes
                .iter()
                .filter(|note| note.start >= from && note.start < to)
                .count();
            CallPermission {
                fewer_notes: Some(FewerNotes {
                    clip: clip.id,
                    has: midi.notes.len(),
                    leaves,
                }),
                ..unchanged
            }
        }
        (ClipData::Audio(_), true) => CallPermission {
            cropped: Some(clip.id),
            ..unchanged
        },
        _ => unchanged,
    }
}

/// `clip.set_data`, tried and cancelled: destructive when it would leave a
/// MIDI clip fewer notes than it has. Adding notes, keeping their count, or
/// replacing an audio clip's file keeps its registered class.
fn notes_set(args: &Value, registered: Permission, editor: &mut Editor) -> CallPermission {
    let unchanged = CallPermission::registered(registered);
    let Some(clip) = args
        .get("clip")
        .and_then(|clip| serde_json::from_value::<ClipId>(clip.clone()).ok())
    else {
        return unchanged;
    };
    let notes = |doc: &Document| match doc.clip(clip).map(|clip| &clip.data) {
        Some(ClipData::Midi(midi)) => Some(midi.notes.len()),
        _ => None,
    };
    let Some(has) = notes(editor.document()) else {
        return unchanged;
    };
    editor.begin("Check a call's permission");
    let ran = bl_model::registry::run(editor, SET_CLIP_DATA, args.clone()).is_ok();
    let leaves = notes(editor.document());
    editor.cancel().ok();
    match leaves {
        Some(leaves) if ran && leaves < has => CallPermission {
            fewer_notes: Some(FewerNotes { clip, has, leaves }),
            ..unchanged
        },
        _ => unchanged,
    }
}

/// A MIDI Tool's call, tried and cancelled against a copy of the Clip View:
/// destructive when it would leave the clip fewer notes than it has, as
/// `clip.set_data` is (the MIDI Tools spec, R24).
fn tool_notes(id: &str, args: &Value, registered: Permission, scene: Scene<'_>) -> CallPermission {
    let unchanged = CallPermission::registered(registered);
    let Scene {
        editor,
        view,
        clip_view,
        shown_clip,
        ..
    } = scene;
    let clip = args
        .get("clip")
        .and_then(|clip| serde_json::from_value::<ClipId>(clip.clone()).ok())
        .or_else(|| clip_view.shown_clip(editor.document(), view))
        .or(shown_clip);
    let Some(clip) = clip else {
        return unchanged;
    };
    let notes = |doc: &Document| match doc.clip(clip).map(|clip| &clip.data) {
        Some(ClipData::Midi(midi)) => Some(midi.notes.len()),
        _ => None,
    };
    let Some(has) = notes(editor.document()) else {
        return unchanged;
    };
    let mut tried = clip_view.clone();
    editor.begin("Check a call's permission");
    let ran = crate::midi_tools::run(id, args.clone(), editor, &mut tried, view).is_ok();
    let leaves = notes(editor.document());
    editor.cancel().ok();
    match leaves {
        Some(leaves) if ran && leaves < has => CallPermission {
            fewer_notes: Some(FewerNotes { clip, has, leaves }),
            ..unchanged
        },
        _ => unchanged,
    }
}

/// `track.paste_bounced_audio`: destructive when the pasted audio would
/// replace or shorten a clip it doesn't act on (the freeze and bounce spec,
/// R26; the agent permissions spec, R1).
fn paste_bounced(args: &Value, registered: Permission, scene: Scene<'_>) -> CallPermission {
    let unchanged = CallPermission::registered(registered);
    let Ok(args) = serde_json::from_value::<PasteBouncedAudioArgs>(args.clone()) else {
        return unchanged;
    };
    let Some(clipboard) = scene.clipboard.as_ref().and_then(Clipboard::material) else {
        return unchanged;
    };
    if clipboard.lanes.is_empty() {
        return unchanged;
    }
    let doc = scene.editor.document();
    // A Session slot: the clip it holds is the one replaced.
    if let Some(slot) = args.scene {
        let track = args.track.or(scene.selected_track);
        let Some(track) = track else {
            return unchanged;
        };
        let replaces = doc
            .slot(track, slot)
            .map(|clip| vec![clip.id])
            .unwrap_or_default();
        return CallPermission {
            registered,
            replaces,
            may: false,
            fewer_notes: None,
            cropped: None,
        };
    }
    // A take lane: the audio lands on the lane at its place in each track.
    let lane_place = args.lane.and_then(|lane| {
        doc.track(doc.take_lane_track(lane)?)?
            .take_lanes()
            .position(|held| held.id == lane)
    });
    let track = args
        .lane
        .and_then(|lane| doc.take_lane_track(lane))
        .or(args.track)
        .or(scene.selected_track)
        .or_else(|| clipboard.source.first().copied());
    let Some(track) = track else {
        return unchanged;
    };
    let at = match &args.at {
        Some(at) => match at.resolve(&doc.settings().time_signatures) {
            Ok(at) => at,
            Err(_) => return unchanged,
        },
        None => scene
            .view
            .time_selection()
            .map_or(scene.insert_marker, |(start, _)| start),
    };
    let end = at + clipboard.length;
    // Every destination the paste lands on, not only the first: the paste
    // puts each source's audio on its own track, so a clip on a later
    // destination is replaced too.
    let mut replaces: Vec<ClipId> = Vec::new();
    let lanes = editing::lanes(doc);
    let Some(base) = lanes.iter().position(|id| *id == track) else {
        return unchanged;
    };
    let mut distances: Vec<usize> = clipboard.lanes.iter().map(|lane| lane.distance).collect();
    distances.dedup();
    for distance in distances {
        let Some(destination) = lanes.get(base + distance) else {
            return unchanged;
        };
        if let Some(held) = doc.track(*destination) {
            let on_lane: Vec<&bl_model::Clip> = match lane_place {
                Some(place) => held
                    .take_lanes()
                    .nth(place)
                    .map_or_else(Vec::new, |lane| lane.clips.iter().collect()),
                None => held.clips().collect(),
            };
            replaces.extend(
                on_lane
                    .into_iter()
                    .filter(|clip| clip.start < end && clip.end() > at)
                    .map(|clip| clip.id),
            );
        }
    }
    CallPermission {
        registered,
        replaces,
        may: false,
        fewer_notes: None,
        cropped: None,
    }
}

/// An Arrangement edit, tried and cancelled.
fn tried(id: &str, args: &Value, registered: Permission, scene: Scene<'_>) -> CallPermission {
    let Scene {
        editor,
        view,
        clipboard,
        session,
        ..
    } = scene;
    // The clips the call acts on: those it moves or resizes keep their
    // identity, and changing them is the edit itself.
    let acts_on: BTreeSet<ClipId> = match id {
        edit_ids::MOVE_CLIPS if args.get("copy").and_then(Value::as_bool) == Some(true) => {
            BTreeSet::new()
        }
        edit_ids::MOVE_CLIPS | edit_ids::RESIZE_CLIPS | edit_ids::NUDGE => {
            match args.get("clips") {
                Some(clips) => serde_json::from_value::<Vec<ClipId>>(clips.clone())
                    .map(|clips| clips.into_iter().collect())
                    .unwrap_or_default(),
                None => view.selected_clips().iter().copied().collect(),
            }
        }
        // The two clips either side of the boundary a comp edge moves.
        comping_ids::MOVE_COMP_EDGE => {
            let doc = editor.document();
            serde_json::from_value::<MoveCompEdgeArgs>(args.clone())
                .ok()
                .and_then(|args| {
                    let at = args.from.resolve(&doc.settings().time_signatures).ok()?;
                    Some(
                        doc.track(args.track)?
                            .clips()
                            .filter(|clip| clip.start == at || clip.end() == at)
                            .map(|clip| clip.id)
                            .collect(),
                    )
                })
                .unwrap_or_default()
        }
        _ => BTreeSet::new(),
    };
    let before = editor.document().clone();
    let mut view = view.clone();
    let mut clipboard = clipboard.clone();
    editor.begin("Check a call's permission");
    // The fades an edit settles change no clip's place, so the dry run skips them.
    let ran = if comping_ids::handles(id) {
        crate::arrangement::comping::run(id, args.clone(), editor, &mut view, false).is_ok()
    } else if session_ids::handles(id) {
        // Spec: second-window R23. A copy between the views, or a paste in
        // the Session View, replaces the clips it lands on.
        let mut session = session.clone();
        crate::session_clips::run(
            id,
            args.clone(),
            editor,
            &mut session,
            &mut view,
            &mut clipboard,
            false,
        )
        .is_ok()
    } else {
        editing::run(id, args.clone(), editor, &mut view, &mut clipboard, false).is_ok()
    };
    let after = editor.document().clone();
    editor.cancel().ok();
    if !ran {
        return CallPermission::registered(registered);
    }
    let replaces = before
        .clip_ids()
        .filter(|clip| !acts_on.contains(clip))
        .filter(|clip| {
            let was = before.clip(*clip);
            match (was, after.clip(*clip)) {
                (Some(_), None) => true,
                (Some(was), Some(now)) => {
                    now.start != was.start
                        || now.length != was.length
                        || now.offset != was.offset
                        || before.clip_track(*clip) != after.clip_track(*clip)
                }
                _ => false,
            }
        })
        .collect();
    CallPermission {
        registered,
        replaces,
        may: false,
        fewer_notes: None,
        cropped: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_that_replaces_clips_is_destructive_as_well() {
        let call = CallPermission {
            registered: Permission::External,
            replaces: vec![ClipId(bl_core::id::Id::new(4).unwrap())],
            may: true,
            fewer_notes: None,
            cropped: None,
        };
        assert_eq!(
            call.classes(),
            [Permission::Destructive, Permission::External]
        );
        assert_eq!(call.class(), Permission::External);
        assert!(call.is_destructive());
        let edit = CallPermission::registered(Permission::Edit);
        assert_eq!(edit.classes(), [Permission::Edit]);
        assert!(!edit.is_destructive());
        let moved = CallPermission {
            replaces: vec![ClipId(bl_core::id::Id::new(4).unwrap())],
            ..edit
        };
        assert_eq!(moved.class(), Permission::Destructive);
        let thinned = CallPermission {
            fewer_notes: Some(FewerNotes {
                clip: ClipId(bl_core::id::Id::new(4).unwrap()),
                has: 8,
                leaves: 3,
            }),
            ..CallPermission::registered(Permission::Edit)
        };
        assert_eq!(
            thinned.classes(),
            [Permission::Edit, Permission::Destructive]
        );
        assert!(thinned.is_destructive());
    }

    #[test]
    fn only_placing_commands_rise() {
        for id in RAISED_BY_ARGUMENTS {
            assert!(raised_by_arguments(id));
        }
        assert!(raised_by_arguments(SET_CLIP_DATA));
        for id in [
            edit_ids::SPLIT,
            edit_ids::DELETE,
            edit_ids::COPY,
            "clip.create",
            midi_ids::EXPORT,
            "song.set_tempo",
            "song.set_time_signatures",
        ] {
            assert!(!raised_by_arguments(id), "{id}");
        }
    }
}
