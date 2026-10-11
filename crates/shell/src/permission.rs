//! A call's permission class, decided from its arguments against the
//! document as it is (WP-1.15b; the agent permissions spec).
//!
//! Most commands have one class whatever their arguments: `track.delete`
//! is always destructive and `track.rename` always an edit. But in the
//! Arrangement a clip placed over part of another replaces the part it
//! covers, as in Live (the arrangement editing spec, R6), so moving,
//! resizing, nudging, duplicating, or pasting clips, and dropping files from
//! the browser, can delete or shorten clips the call doesn't name. Such a
//! call is destructive as well as its registered class; the same call into
//! empty space isn't. [`assess`] decides which, and the agent API asks the
//! person first when the answer is destructive. The edit itself is the
//! same either way: permissions only decide who must agree to it.

use std::collections::BTreeSet;

use bl_commands::Permission;
use bl_core::time::Ticks;
use bl_model::{ClipId, Document, Editor, TrackId, TrackKind};
use bl_shell_spec::browser::{self as browser_ids, AddToArrangementArgs};
use bl_shell_spec::editing as edit_ids;
use serde_json::Value;

use crate::arrangement::ArrangementView;
use crate::arrangement::editing::{self, Clipboard};

/// The commands whose class can rise with their arguments: each is
/// destructive as well as its registered class when it would delete or
/// shorten clips it doesn't act on.
pub const RAISED_BY_ARGUMENTS: &[&str] = &[
    edit_ids::MOVE_CLIPS,
    edit_ids::RESIZE_CLIPS,
    edit_ids::NUDGE,
    edit_ids::DUPLICATE,
    edit_ids::PASTE,
    browser_ids::ADD_TO_ARRANGEMENT,
    browser_ids::LOAD_SELECTED,
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
}

impl CallPermission {
    /// A call whose class is the command's registered class.
    #[must_use]
    pub fn registered(class: Permission) -> Self {
        Self {
            registered: class,
            replaces: Vec::new(),
            may: false,
        }
    }

    /// Whether the call deletes or shortens other material, so it's
    /// destructive.
    #[must_use]
    pub fn is_destructive(&self) -> bool {
        self.registered == Permission::Destructive || !self.replaces.is_empty()
    }

    /// Every class the call needs, least reach first: its registered class,
    /// and destructive when it replaces other clips.
    #[must_use]
    pub fn classes(&self) -> Vec<Permission> {
        let mut classes = vec![self.registered];
        if !self.replaces.is_empty() && !classes.contains(&Permission::Destructive) {
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
    pub(crate) insert_marker: Ticks,
    pub(crate) selected_track: Option<TrackId>,
}

/// Decides what calling `id` with `args` needs, against the document as it
/// is. An Arrangement edit is tried in a transaction that's then cancelled,
/// so nothing changes; a drop from the browser is judged without reading
/// its files. A call that would fail needs only its registered class: it
/// changes nothing.
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
            let track = scene.selected_track.filter(|track| {
                doc.track(*track)
                    .is_some_and(|t| t.kind() == TrackKind::Audio)
            });
            dropped(doc, track, scene.insert_marker, registered)
        }
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
    }
}

/// An Arrangement edit, tried and cancelled.
fn tried(id: &str, args: &Value, registered: Permission, scene: Scene<'_>) -> CallPermission {
    let Scene {
        editor,
        view,
        clipboard,
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
        _ => BTreeSet::new(),
    };
    let before = editor.document().clone();
    let mut view = view.clone();
    let mut clipboard = clipboard.clone();
    editor.begin("Check a call's permission");
    let ran = editing::run(id, args.clone(), editor, &mut view, &mut clipboard).is_ok();
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
    }

    #[test]
    fn only_placing_commands_rise() {
        for id in RAISED_BY_ARGUMENTS {
            assert!(raised_by_arguments(id));
        }
        for id in [
            edit_ids::SPLIT,
            edit_ids::DELETE,
            edit_ids::COPY,
            "clip.create",
        ] {
            assert!(!raised_by_arguments(id), "{id}");
        }
    }
}
