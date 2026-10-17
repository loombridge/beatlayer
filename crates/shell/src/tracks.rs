//! Track commands that run on the shell (WP-2.19b): Duplicate Track Without
//! Clips (IMP-12).
//!
//! A duplicate is made from the document's own commands, inside one
//! transaction, so it's one undo step and each part of it is checked as any
//! edit is.

use bl_model::{Command, DeviceId, Document, EditError, Editor, Output, Track, TrackId, TrackKind};

use crate::routing::{return_letter, track_label};

/// The tracks a duplicate copies: those given, less Main and any inside
/// another one given (its group's copy holds its copy), in display order.
#[must_use]
pub fn duplicated(doc: &Document, tracks: &[TrackId]) -> Vec<TrackId> {
    let inside_another = |id: TrackId| {
        let mut parent = doc.parent(id);
        while let Some(group) = parent {
            if tracks.contains(&group) {
                return true;
            }
            parent = doc.parent(group);
        }
        false
    };
    doc.track_order()
        .into_iter()
        .filter(|&id| {
            tracks.contains(&id)
                && doc.track(id).is_some_and(|t| t.kind() != TrackKind::Main)
                && !inside_another(id)
        })
        .collect()
}

/// Duplicates `tracks` without their clips, as one undo step named `label`:
/// each copy straight after its track, with its name, mixer, sends,
/// routing, and devices (new IDs, the same settings), and a group's copy
/// holding copies of its tracks. Returns the copies, in display order.
///
/// # Errors
///
/// If nothing can be duplicated, or a command is refused; the set is then
/// unchanged.
pub fn duplicate_without_clips(
    editor: &mut Editor,
    tracks: &[TrackId],
    label: &'static str,
) -> Result<Vec<TrackId>, String> {
    let originals = duplicated(editor.document(), tracks);
    if originals.is_empty() {
        return Err("there's no track to duplicate".to_owned());
    }
    let mut copies = Vec::new();
    editor
        .transact(label, |editor| {
            let mut renames = Vec::new();
            // The last first, so each original's place stays where it was.
            for &original in originals.iter().rev() {
                let doc = editor.document();
                let parent = doc.parent(original);
                let kind = doc.track(original).map(Track::kind);
                let siblings: Vec<TrackId> = match (kind, parent) {
                    (Some(TrackKind::Return), _) => doc.returns().collect(),
                    (_, Some(group)) => doc
                        .track(group)
                        .map(|g| g.children().collect())
                        .unwrap_or_default(),
                    (_, None) => doc.top_level().collect(),
                };
                let index = siblings
                    .iter()
                    .position(|&t| t == original)
                    .map_or(0, |i| i + 1);
                let copy = copy_track(editor, original, parent, index, &mut renames)?;
                copies.push(copy);
            }
            for (copy, original) in renames {
                rename_like(editor, copy, original)?;
            }
            Ok::<_, EditError>(())
        })
        .map_err(|e| e.to_string())?;
    let order = editor.document().track_order();
    copies.sort_by_key(|copy| order.iter().position(|t| t == copy));
    Ok(copies)
}

/// Copies `original` to `index` in `parent`'s list (or the returns), with
/// its settings and devices, and a group's tracks inside it; notes each
/// copy for renaming.
fn copy_track(
    editor: &mut Editor,
    original: TrackId,
    parent: Option<TrackId>,
    index: usize,
    renames: &mut Vec<(TrackId, TrackId)>,
) -> Result<TrackId, EditError> {
    let source = editor
        .document()
        .track(original)
        .cloned()
        .expect("the original exists");
    let id: TrackId = editor.new_id();
    editor.apply(Command::CreateTrack {
        id,
        kind: source.kind(),
        name: source.name().to_owned(),
        parent: if source.kind() == TrackKind::Return {
            None
        } else {
            parent
        },
        index,
    })?;
    renames.push((id, original));
    let mixer = *source.mixer();
    editor.apply(Command::SetVolume {
        track: id,
        volume: mixer.volume,
    })?;
    editor.apply(Command::SetPan {
        track: id,
        pan: mixer.pan,
    })?;
    if source.kind().has_mute_and_solo() {
        editor.apply(Command::SetMute {
            track: id,
            mute: mixer.mute,
        })?;
        editor.apply(Command::SetSolo {
            track: id,
            solo: mixer.solo,
        })?;
    }
    if source.kind().has_arm() {
        editor.apply(Command::SetArm {
            track: id,
            arm: mixer.arm,
        })?;
    }
    if source.kind() == TrackKind::Return {
        editor.apply(Command::SetSendTap {
            track: id,
            tap: mixer.tap,
        })?;
    }
    for (to, level) in source.sends() {
        editor.apply(Command::SetSend {
            track: id,
            to,
            level,
        })?;
    }
    for (index, device) in source.devices().enumerate() {
        // New IDs for the device and everything inside its racks.
        let mut copy = device.clone();
        copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
        copy.floating = device.floating;
        editor.apply(Command::InsertDevice {
            track: id,
            index,
            device: copy,
        })?;
    }
    let children: Vec<TrackId> = source.children().collect();
    for (index, child) in children.into_iter().enumerate() {
        copy_track(editor, child, Some(id), index, renames)?;
    }
    // The routing last, once the devices and the tracks inside are there.
    let routing = source.routing().clone();
    if source.kind().holds_clips() {
        editor.apply(Command::SetInput {
            track: id,
            input: routing.input,
            channel: routing.channel,
        })?;
        editor.apply(Command::SetMonitor {
            track: id,
            monitor: routing.monitor,
        })?;
    }
    if routing.output != Output::Default {
        editor.apply(Command::SetOutput {
            track: id,
            output: routing.output,
        })?;
    }
    if source.kind() == TrackKind::Midi {
        editor.apply(Command::SetMidiOutput {
            track: id,
            to: routing.midi_output,
        })?;
    }
    Ok(id)
}

/// Gives `copy` its original's name, numbered for its own place where the
/// original's name starts with the original's number or letter (Live
/// duplicates "4 Audio" as "5 Audio", observed); any other name as it is.
fn rename_like(editor: &mut Editor, copy: TrackId, original: TrackId) -> Result<(), EditError> {
    let doc = editor.document();
    let Some(name) = doc.track(original).map(|t| t.name().to_owned()) else {
        return Ok(());
    };
    let Some((head, rest)) = name.split_once(' ') else {
        return Ok(());
    };
    let own = |track: TrackId| {
        if doc.track(track).map(Track::kind) == Some(TrackKind::Return) {
            let index = doc.returns().position(|r| r == track).unwrap_or(0);
            return_letter(index)
        } else {
            track_label(doc, track)
                .split_once('-')
                .map(|(number, _)| number.to_owned())
                .unwrap_or_default()
        }
    };
    if head != own(original) {
        return Ok(());
    }
    let renamed = format!("{} {rest}", own(copy));
    if renamed != name {
        editor.apply(Command::RenameTrack {
            track: copy,
            name: renamed,
        })?;
    }
    Ok(())
}

/// The tracks from `tracks` that can be grouped, in display order:
/// tracks whose kind satisfies `can_be_grouped()` and that exist in the document,
/// excluding Main and Return tracks, and excluding tracks that are already inside
/// another track in `tracks`.
#[must_use]
pub fn groupable(doc: &Document, tracks: &[TrackId]) -> Vec<TrackId> {
    let inside_another = |id: TrackId| {
        let mut parent = doc.parent(id);
        while let Some(group) = parent {
            if tracks.contains(&group) {
                return true;
            }
            parent = doc.parent(group);
        }
        false
    };
    doc.track_order()
        .into_iter()
        .filter(|&id| {
            tracks.contains(&id)
                && doc.track(id).is_some_and(|t| t.kind().can_be_grouped())
                && !inside_another(id)
        })
        .collect()
}

/// Groups `tracks` into a new group track named "Group" as one undo step named `label`.
/// Inserts the group track where the first track was, moves each track into it in order,
/// and returns the new group track ID.
///
/// # Errors
///
/// If no tracks can be grouped, or if a command fails; the set remains unchanged.
pub fn group_tracks(
    editor: &mut Editor,
    tracks: &[TrackId],
    label: &'static str,
) -> Result<TrackId, String> {
    let members = groupable(editor.document(), tracks);
    if members.is_empty() {
        return Err("there's no track to group".to_owned());
    }
    let first = members[0];
    let doc = editor.document();
    let parent = doc.parent(first);
    let siblings: Vec<TrackId> = match parent {
        Some(group) => doc
            .track(group)
            .map(|g| g.children().collect())
            .unwrap_or_default(),
        None => doc.top_level().collect(),
    };
    let insert_index = siblings
        .iter()
        .position(|&t| t == first)
        .unwrap_or(siblings.len());

    let group_id: TrackId = editor.new_id();

    editor
        .transact(label, |editor| {
            editor.apply(Command::CreateTrack {
                id: group_id,
                kind: TrackKind::Group,
                name: "Group".to_owned(),
                parent,
                index: insert_index,
            })?;
            for (i, &member) in members.iter().enumerate() {
                editor.apply(Command::MoveTrack {
                    track: member,
                    parent: Some(group_id),
                    index: i,
                })?;
            }
            Ok::<_, EditError>(())
        })
        .map_err(|e| e.to_string())?;

    Ok(group_id)
}

/// Ungroups `group` as one undo step named `label`.
/// Moves all children of `group` to where `group` was, in order, deletes `group`,
/// and returns the former members in order.
///
/// # Errors
///
/// If `group` is not a group track or doesn't exist, or if a command fails.
pub fn ungroup_tracks(
    editor: &mut Editor,
    group: TrackId,
    label: &'static str,
) -> Result<Vec<TrackId>, String> {
    let doc = editor.document();
    let track = doc
        .track(group)
        .ok_or_else(|| "track not found".to_owned())?;
    if track.kind() != TrackKind::Group {
        return Err("selected track is not a group track".to_owned());
    }
    let children: Vec<TrackId> = track.children().collect();
    let parent = doc.parent(group);
    let siblings: Vec<TrackId> = match parent {
        Some(p) => doc
            .track(p)
            .map(|g| g.children().collect())
            .unwrap_or_default(),
        None => doc.top_level().collect(),
    };
    let insert_index = siblings
        .iter()
        .position(|&t| t == group)
        .unwrap_or(siblings.len());

    editor
        .transact(label, |editor| {
            for (i, &child) in children.iter().enumerate() {
                editor.apply(Command::MoveTrack {
                    track: child,
                    parent,
                    index: insert_index + i,
                })?;
            }
            editor.apply(Command::DeleteTrack { track: group })?;
            Ok::<_, EditError>(())
        })
        .map_err(|e| e.to_string())?;

    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(editor: &mut Editor, kind: TrackKind, name: &str, parent: Option<TrackId>) -> TrackId {
        let id = editor.new_id();
        let doc = editor.document();
        let index = match (kind, parent) {
            (TrackKind::Return, _) => doc.returns().len(),
            (_, Some(group)) => doc.track(group).unwrap().children().len(),
            (_, None) => doc.top_level().len(),
        };
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: name.to_owned(),
                parent,
                index,
            })
            .unwrap();
        id
    }

    #[test]
    fn a_duplicate_goes_after_its_track_numbered_for_its_place() {
        let mut editor = Editor::new();
        let one = add(&mut editor, TrackKind::Audio, "1 Audio", None);
        let two = add(&mut editor, TrackKind::Audio, "Kick", None);
        let ret = add(&mut editor, TrackKind::Return, "A Reverb", None);
        let copies = duplicate_without_clips(&mut editor, &[one, two, ret], "Duplicate").unwrap();
        let doc = editor.document();
        let names: Vec<&str> = doc
            .track_order()
            .into_iter()
            .map(|t| doc.track(t).unwrap().name())
            .collect();
        assert_eq!(
            names,
            [
                "1 Audio", "2 Audio", "Kick", "Kick", "A Reverb", "B Reverb", "Main"
            ]
        );
        assert_eq!(copies.len(), 3);
        assert_eq!(editor.undo_history().len(), 4);
        assert_eq!(editor.undo_history().next(), Some("Duplicate"));
    }

    #[test]
    fn group_and_ungroup_tracks_round_trip() {
        let mut editor = Editor::new();
        let one = add(&mut editor, TrackKind::Audio, "1 Audio", None);
        let two = add(&mut editor, TrackKind::Midi, "2 MIDI", None);
        let three = add(&mut editor, TrackKind::Audio, "3 Audio", None);

        let initial_doc = editor.document().clone();

        // Group `two` and `three`
        let group = group_tracks(&mut editor, &[two, three], "Group Tracks").unwrap();
        let doc = editor.document();
        let names: Vec<&str> = doc
            .track_order()
            .into_iter()
            .map(|t| doc.track(t).unwrap().name())
            .collect();
        assert_eq!(names, ["1 Audio", "Group", "2 MIDI", "3 Audio", "Main"]);
        assert_eq!(doc.output_of(two), Some(group));
        assert_eq!(doc.output_of(three), Some(group));
        assert_eq!(doc.output_of(group), Some(doc.main_id()));
        assert_eq!(editor.undo_history().next(), Some("Group Tracks"));

        // Undo grouping
        editor.undo().unwrap();
        assert_eq!(editor.document(), &initial_doc);

        // Redo grouping
        editor.redo().unwrap();
        assert_eq!(
            editor.document().track_order(),
            [one, group, two, three, editor.document().main_id()]
        );

        // Ungroup
        let ungrouped = ungroup_tracks(&mut editor, group, "Ungroup Tracks").unwrap();
        assert_eq!(ungrouped, [two, three]);
        let doc = editor.document();
        assert_eq!(doc.track_order(), [one, two, three, doc.main_id()]);
        assert_eq!(doc.output_of(two), Some(doc.main_id()));
        assert_eq!(doc.output_of(three), Some(doc.main_id()));
        assert_eq!(editor.undo_history().next(), Some("Ungroup Tracks"));

        // Undo ungroup
        editor.undo().unwrap();
        let doc = editor.document();
        assert_eq!(doc.track_order(), [one, group, two, three, doc.main_id()]);
        assert_eq!(doc.output_of(two), Some(group));

        // Redo ungroup
        editor.redo().unwrap();
        assert_eq!(
            editor.document().track_order(),
            [one, two, three, editor.document().main_id()]
        );
    }
}
