//! The Arrangement's locators (WP-6.24): adding, deleting, moving,
//! renaming and describing them, looping to the next one, the song start
//! time, and the jumps between them.
//!
//! The locators are part of the set, so the commands that change them are
//! edits, and each is one undo step: the document's own commands run inside
//! one transaction. The jumps change nothing in the set, and they need the
//! audio engine, so they are refused without one.
//!
//! The behaviour is in the time editing spec.

use bl_core::time::Ticks;
use bl_model::{
    Command, Document, Editor, LaunchQuantization, LaunchSettings, Locator, LocatorId, SongStart,
};
use bl_shell_spec::arrangement::ArrangementState;
use bl_shell_spec::editing::SelectMode;
use bl_shell_spec::locator::{
    self as ids, AddLocatorArgs, DeleteLocatorArgs, JumpArgs, LocatorResult, LoopToNextArgs,
    MoveLocatorArgs, RenameLocatorArgs, SelectLocatorArgs, SetInfoTextArgs, SetStartTimeArgs,
};
use serde::Serialize;
use serde_json::Value;

use crate::audio::AudioHost;

use super::editing::{EditFailure, invalid, refused, unavailable};
use super::{ArrangementView, parse};

/// The name of the undo step a locator command makes, such as "Add
/// Locator".
#[must_use]
pub fn label(id: &str) -> &'static str {
    match id {
        ids::ADD => "Add Locator",
        ids::DELETE => "Delete Locator",
        ids::MOVE => "Move Locator",
        ids::RENAME => "Rename Locator",
        ids::SET_INFO_TEXT => "Edit Info Text",
        ids::LOOP_TO_NEXT => "Loop to Next Locator",
        ids::SET_START_TIME => "Set Song Start Time",
        _ => "Select",
    }
}

/// Runs locator command `id` with `args`.
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
pub fn run(
    id: &str,
    args: Value,
    editor: &mut Editor,
    view: &mut ArrangementView,
    launch: &LaunchSettings,
    mut engine: Option<&mut AudioHost>,
) -> Result<Value, EditFailure> {
    match id {
        ids::ADD => {
            let AddLocatorArgs { at } = parse(args)?;
            let doc = editor.document();
            let position = match at {
                Some(position) => position
                    .resolve(&doc.settings().time_signatures)
                    .map_err(invalid)?,
                None => placement(doc, view, launch, engine.as_deref_mut()),
            };
            // Two locators never share a position: adding one where another
            // is leaves that one as it is, and selects it.
            if let Some(existing) = doc
                .locators()
                .iter()
                .find(|locator| locator.position == position)
            {
                view.select_locators(vec![existing.id]);
                return Ok(to_value(&result(view, false)));
            }
            let id = editor.new_id();
            let command = Command::AddLocator {
                locator: Locator::new(id, position, String::new()),
            };
            apply(editor, label(ids::ADD), |editor| {
                editor.apply(command.clone())?;
                Ok(())
            })?;
            view.select_locators(vec![id]);
            Ok(to_value(&result(view, true)))
        }
        ids::DELETE => {
            let DeleteLocatorArgs { locators } = parse(args)?;
            let targets = locators.unwrap_or_else(|| view.selected_locators().to_vec());
            if targets.is_empty() {
                return Err(unavailable("no locator is selected"));
            }
            apply(editor, label(ids::DELETE), |editor| {
                for locator in &targets {
                    editor.apply(Command::DeleteLocator { locator: *locator })?;
                }
                Ok(())
            })?;
            view.select_locators(Vec::new());
            Ok(to_value(&result(view, true)))
        }
        ids::MOVE => {
            let MoveLocatorArgs { locator, to } = parse(args)?;
            let doc = editor.document();
            let targets = match locator {
                Some(locator) => vec![locator],
                None => view.selected_locators().to_vec(),
            };
            if targets.is_empty() {
                return Err(unavailable("no locator is selected"));
            }
            let to = to
                .ok_or_else(|| invalid("`to` says where the locator goes"))?
                .resolve(&doc.settings().time_signatures)
                .map_err(invalid)?
                .max(Ticks::ZERO);
            let primary = targets[0];
            let current = doc
                .locator(primary)
                .ok_or_else(|| invalid(format!("no locator has the ID {}", primary.0)))?
                .position;
            let delta = to.get() - current.get();
            if delta == 0 {
                return Ok(to_value(&result(view, false)));
            }
            // Every other selected locator moves by the same distance.
            let mut moves = vec![(primary, to)];
            for other in view.selected_locators() {
                if *other == primary {
                    continue;
                }
                let Some(locator) = doc.locator(*other) else {
                    continue;
                };
                moves.push((*other, Ticks(locator.position.get() + delta)));
            }
            apply(editor, label(ids::MOVE), |editor| {
                move_locators(editor, &moves)
            })?;
            Ok(to_value(&result(view, true)))
        }
        ids::RENAME => {
            let RenameLocatorArgs { locators, name } = parse(args)?;
            let targets = locators.unwrap_or_else(|| view.selected_locators().to_vec());
            if targets.is_empty() {
                return Err(unavailable("no locator is selected"));
            }
            apply(editor, label(ids::RENAME), |editor| {
                for locator in &targets {
                    editor.apply(Command::RenameLocator {
                        locator: *locator,
                        name: name.clone(),
                    })?;
                }
                Ok(())
            })?;
            Ok(to_value(&result(view, true)))
        }
        ids::SET_INFO_TEXT => {
            let SetInfoTextArgs { locator, info_text } = parse(args)?;
            let doc = editor.document();
            let target = match locator {
                Some(locator) => locator,
                None => *view
                    .selected_locators()
                    .first()
                    .ok_or_else(|| unavailable("no locator is selected"))?,
            };
            if doc.locator(target).is_none() {
                return Err(invalid(format!("no locator has the ID {}", target.0)));
            }
            let command = Command::SetLocatorInfoText {
                locator: target,
                info_text: info_text.clone(),
            };
            apply(editor, label(ids::SET_INFO_TEXT), |editor| {
                editor.apply(command.clone())?;
                Ok(())
            })?;
            Ok(to_value(&result(view, true)))
        }
        ids::LOOP_TO_NEXT => {
            let LoopToNextArgs { locator } = parse(args)?;
            let target = match locator {
                Some(locator) => locator,
                None => *view
                    .selected_locators()
                    .first()
                    .ok_or_else(|| unavailable("no locator is selected"))?,
            };
            let command = Command::LoopToNextLocator { locator: target };
            apply(editor, label(ids::LOOP_TO_NEXT), |editor| {
                editor.apply(command.clone())?;
                Ok(())
            })?;
            Ok(to_value(&result(view, true)))
        }
        ids::SELECT => {
            let SelectLocatorArgs { locators, mode } = parse(args)?;
            let doc = editor.document();
            for locator in &locators {
                if doc.locator(*locator).is_none() {
                    return Err(invalid(format!("no locator has the ID {}", locator.0)));
                }
            }
            let mut selected = view.selected_locators().to_vec();
            match mode {
                SelectMode::Replace => {
                    selected = locators;
                    // A click on a locator clears the clip and time
                    // selections (the time editing spec, R5).
                    view.clear_clip_selection();
                }
                SelectMode::Add => {
                    for locator in locators {
                        if !selected.contains(&locator) {
                            selected.push(locator);
                        }
                    }
                }
                SelectMode::Toggle => {
                    for locator in locators {
                        if let Some(at) = selected.iter().position(|other| *other == locator) {
                            selected.remove(at);
                        } else {
                            selected.push(locator);
                        }
                    }
                }
                SelectMode::Extend => {
                    let mut all = selected.clone();
                    all.extend(locators.iter().copied());
                    all.sort_by_key(|locator| {
                        doc.locator(*locator)
                            .map_or(0, |locator| locator.position.get())
                    });
                    selected = all;
                }
            }
            view.select_locators(selected);
            Ok(to_value(&view.state(doc)))
        }
        ids::JUMP => {
            let JumpArgs { locator } = parse(args)?;
            let doc = editor.document();
            let target = match locator {
                Some(locator) => locator,
                None => *view
                    .selected_locators()
                    .first()
                    .ok_or_else(|| unavailable("no locator is selected"))?,
            };
            let position = doc
                .locator(target)
                .ok_or_else(|| invalid(format!("no locator has the ID {}", target.0)))?
                .position;
            jump(engine, view, position)?;
            Ok(to_value(&ArrangementState {
                selected_locators: view.selected_locators().to_vec(),
                ..view.state(doc)
            }))
        }
        ids::JUMP_PREVIOUS | ids::JUMP_NEXT => {
            let doc = editor.document();
            let later = id == ids::JUMP_NEXT;
            // The position the jump is measured from: the playhead while the
            // song plays, the insert marker while it's stopped (the time
            // editing spec, R6).
            let position = match engine.as_deref_mut() {
                Some(engine) => {
                    let transport = engine.transport_state();
                    if transport.playing {
                        transport.position
                    } else {
                        transport.insert_marker
                    }
                }
                None => Ticks::ZERO,
            };
            let target = if later {
                doc.locators()
                    .iter()
                    .find(|locator| locator.position > position)
                    .map(|locator| locator.position)
            } else {
                doc.locators()
                    .iter()
                    .rev()
                    .find(|locator| locator.position < position)
                    .map(|locator| locator.position)
            };
            let target = match target {
                Some(position) => position,
                // With no locator before it, Previous jumps to 1.1.1; with
                // none after it, Next jumps to the Arrangement's end, the
                // end of its last clip on any track, or 1.1.1 when it has
                // none (the time editing spec, R6).
                None if later => doc
                    .tracks()
                    .flat_map(|track| track.clips().map(bl_model::Clip::end))
                    .max()
                    .unwrap_or(Ticks::ZERO),
                None => Ticks::ZERO,
            };
            jump(engine, view, target)?;
            Ok(to_value(&view.state(doc)))
        }
        ids::SET_START_TIME => {
            let SetStartTimeArgs {
                locator,
                loop_brace,
                clear,
            } = parse(args)?;
            let doc = editor.document();
            let start = if clear {
                // Choosing it where it's ticked clears it (the time editing
                // spec, R10).
                None
            } else if loop_brace {
                Some(SongStart::LoopBrace)
            } else {
                let locator = match locator {
                    Some(locator) => locator,
                    None => *view
                        .selected_locators()
                        .first()
                        .ok_or_else(|| unavailable("no locator is selected"))?,
                };
                if doc.locator(locator).is_none() {
                    return Err(invalid(format!("no locator has the ID {}", locator.0)));
                }
                // With no locator given, the song starts at the selected
                // one, or stops starting there if it already does.
                if doc.settings().song_start == Some(SongStart::Locator(locator)) {
                    None
                } else {
                    Some(SongStart::Locator(locator))
                }
            };
            let command = Command::SetSongStart { start };
            apply(editor, label(ids::SET_START_TIME), |editor| {
                editor.apply(command.clone())?;
                Ok(())
            })?;
            Ok(to_value(&result(view, true)))
        }
        other => Err(invalid(format!("`{other}` isn't a locator command"))),
    }
}

/// Where the Set Locator button puts a locator: on the line of the global
/// launch quantisation nearest the playhead while the song plays, and at
/// the time selection's start, or the insert marker, while it is stopped
/// (the time editing spec, R3).
fn placement(
    doc: &Document,
    view: &ArrangementView,
    launch: &LaunchSettings,
    engine: Option<&mut AudioHost>,
) -> Ticks {
    let Some(engine) = engine else {
        return view
            .time_selection()
            .map_or(view.insert_marker(), |(start, _)| start)
            .max(Ticks::ZERO);
    };
    let transport = engine.transport_state();
    if transport.playing || transport.recording {
        let signatures = &doc.settings().time_signatures;
        if launch.enabled && launch.quantization != LaunchQuantization::None {
            return launch
                .quantization
                .nearest_line(transport.position, signatures)
                .max(Ticks::ZERO);
        }
        return transport.position.max(Ticks::ZERO);
    }
    view.time_selection()
        .map_or(view.insert_marker(), |(start, _)| start)
        .max(Ticks::ZERO)
}

/// Jumps to `position`: while the song plays, playback jumps there on the
/// next line of the global launch quantisation; while it is stopped, the
/// insert marker moves there (the time editing spec, R5).
fn jump(
    engine: Option<&mut AudioHost>,
    view: &mut ArrangementView,
    position: Ticks,
) -> Result<(), EditFailure> {
    let Some(engine) = engine else {
        return Err(unavailable("the jump needs the audio engine"));
    };
    if let Some(reason) = engine.engine_refusal() {
        return Err(unavailable(reason));
    }
    if engine.transport_state().playing {
        engine
            .send_transport(bl_engine::TransportCommand::QuantizedLocate(position))
            .map_err(unavailable)?;
    } else {
        view.move_insert_marker(position);
        engine.set_insert_marker(position);
    }
    Ok(())
}

/// Moves the locators to their new positions, freeing each place before
/// another takes it, so a group of locators can swap places.
fn move_locators(editor: &mut Editor, moves: &[(LocatorId, Ticks)]) -> Result<(), EditFailure> {
    let mut pending = moves.to_vec();
    while !pending.is_empty() {
        let mut moved = false;
        let mut waiting = Vec::new();
        for (locator, to) in pending {
            let taken = editor
                .document()
                .locators()
                .iter()
                .any(|other| other.id != locator && other.position == to);
            if taken {
                waiting.push((locator, to));
                continue;
            }
            editor.apply(Command::MoveLocator { locator, to })?;
            moved = true;
        }
        if !moved {
            return Err(refused("two locators would end up at the same position"));
        }
        pending = waiting;
    }
    Ok(())
}

/// Runs `edit` as one transaction labelled `label`, so the change is one
/// undo step.
fn apply(
    editor: &mut Editor,
    label: &'static str,
    edit: impl FnOnce(&mut Editor) -> Result<(), EditFailure>,
) -> Result<(), EditFailure> {
    super::editing::transaction(editor, label, edit).map(|_| ())
}

/// What a locator command reports: the locators selected afterwards, and
/// whether it was an undo step.
fn result(view: &ArrangementView, undo_step: bool) -> LocatorResult {
    LocatorResult {
        selected_locators: view.selected_locators().to_vec(),
        undo_step,
    }
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
