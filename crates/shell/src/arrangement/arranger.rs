//! The arranger track (WP-6.45): sections of the song, made, renamed,
//! coloured, resized, moved, copied, duplicated, removed and deleted.
//!
//! The sections are part of the set, so the commands that change them are
//! edits, each one undo step. Moving, copying, duplicating and deleting a
//! section are the …Time operations over its span, composed inside one
//! transaction from the same functions the time commands use, so a section
//! carries exactly what Cut Time and Paste Time carry. The time clipboard is
//! left as it was. The behaviour is in the arranger track spec.

use bl_core::time::Ticks;
use bl_model::{Command, Document, Editor, SongSection, SectionId, next_section_colour};
use bl_shell_spec::arranger::{
    self as ids, AddSectionArgs, ArrangerResult, CopySectionArgs, MoveSectionArgs, Neighbour,
    RenameSectionArgs, ResizeSectionArgs, SectionArgs, SelectSectionArgs, SetSectionColourArgs,
};
use bl_shell_spec::arrangement::SwitchArgs;
use bl_shell_spec::editing::{SelectMode, TimeSpanArgs};
use serde_json::Value;

use super::editing::{EditFailure, apply, invalid, settled, to_value, transaction, unavailable};
use super::time_commands::{leave_marker, paste_material, span, time_material};
use super::{ArrangementView, parse};

/// The name of the undo step an arranger command makes, such as "Add
/// Section".
#[must_use]
pub fn label(id: &str) -> &'static str {
    match id {
        ids::ADD_SECTION | ids::SECTIONS_FROM_LOCATORS => "Add Section",
        ids::RENAME_SECTION => "Rename Section",
        ids::SET_SECTION_COLOUR => "Set Section Colour",
        ids::RESIZE_SECTION => "Resize Section",
        ids::MOVE_SECTION => "Move Section",
        ids::COPY_SECTION => "Copy Section",
        ids::DUPLICATE_SECTION => "Duplicate Section",
        ids::REMOVE_SECTION => "Remove Section",
        ids::DELETE_SECTION => "Delete Section",
        _ => "Select Section",
    }
}

/// Runs arranger command `id` with `args`.
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
pub fn run(
    id: &str,
    args: Value,
    editor: &mut Editor,
    view: &mut ArrangementView,
    edge_fades: bool,
) -> Result<Value, EditFailure> {
    let label = label(id);
    match id {
        ids::ADD_SECTION => {
            let AddSectionArgs {
                start,
                end,
                name,
                colour,
            } = parse(args)?;
            let id = editor.new_id();
            let doc = editor.document();
            let (start, end) = span(doc, view, TimeSpanArgs { start, end })?;
            let (start, end) = free_span(doc, start, end)?;
            let section = SongSection {
                id,
                start,
                length: end - start,
                name: name.unwrap_or_default(),
                colour: colour.unwrap_or_else(|| next_section_colour(doc.sections())),
            };
            let new = section.id;
            let (_, recorded) = transaction(editor, label, |editor| {
                apply(editor, Command::AddSection { section })
            })?;
            view.select_sections(editor.document(), vec![new]);
            Ok(result(view, recorded))
        }
        ids::SECTIONS_FROM_LOCATORS => {
            parse::<bl_commands::NoArgs>(args)?;
            let arrangement_end = view.arrangement_end(editor.document());
            let locators = editor.document().locators().to_vec();
            let (made, recorded) = transaction(editor, label, |editor| {
                let mut made = Vec::new();
                for (n, locator) in locators.iter().enumerate() {
                    let end = locators
                        .get(n + 1)
                        .map_or(arrangement_end, |next| next.position);
                    let id = editor.new_id();
                    let doc = editor.document();
                    let Ok((start, end)) = free_span(doc, locator.position, end) else {
                        continue;
                    };
                    let section = SongSection {
                        id,
                        start,
                        length: end - start,
                        name: locator.name.clone(),
                        colour: next_section_colour(doc.sections()),
                    };
                    made.push(section.id);
                    apply(editor, Command::AddSection { section })?;
                }
                Ok(made)
            })?;
            if !made.is_empty() {
                view.select_sections(editor.document(), made);
            }
            Ok(result(view, recorded))
        }
        ids::RENAME_SECTION => {
            let RenameSectionArgs { section, name } = parse(args)?;
            let section = chosen(editor.document(), view, section)?.id;
            let name = name.ok_or_else(|| invalid("`name` says the new name"))?;
            // An empty name keeps the old one, as a locator's does.
            let name = (!name.is_empty()).then_some(name);
            edit(editor, view, label, section, name, None, None, None)
        }
        ids::SET_SECTION_COLOUR => {
            let SetSectionColourArgs { section, colour } = parse(args)?;
            let section = chosen(editor.document(), view, section)?.id;
            edit(editor, view, label, section, None, Some(colour), None, None)
        }
        ids::RESIZE_SECTION => {
            let ResizeSectionArgs {
                section,
                start,
                end,
            } = parse(args)?;
            let doc = editor.document();
            let section = chosen(doc, view, section)?.clone();
            let signatures = &doc.settings().time_signatures;
            let start = match start {
                Some(start) => start.resolve(signatures).map_err(invalid)?,
                None => section.start,
            };
            let end = match end {
                Some(end) => end.resolve(signatures).map_err(invalid)?,
                None => section.end(),
            };
            if end <= start {
                return Err(invalid("the section's end must be after its start"));
            }
            edit(
                editor,
                view,
                label,
                section.id,
                None,
                None,
                Some(start),
                Some(end - start),
            )
        }
        ids::MOVE_SECTION => {
            let MoveSectionArgs { section, to, with } = parse(args)?;
            let doc = editor.document();
            let section = chosen(doc, view, section)?.clone();
            let to = match (to, with) {
                (Some(_), Some(_)) => return Err(invalid("give `to` or `with`, not both")),
                (Some(to), None) => to
                    .resolve(&doc.settings().time_signatures)
                    .map_err(invalid)?,
                (None, Some(Neighbour::Earlier)) => doc
                    .sections()
                    .iter()
                    .rev()
                    .find(|other| other.start < section.start)
                    .map(|other| other.start)
                    .ok_or_else(|| unavailable("there is no section before it"))?,
                (None, Some(Neighbour::Later)) => doc
                    .sections()
                    .iter()
                    .find(|other| other.start > section.start)
                    .map(|other| other.end())
                    .ok_or_else(|| unavailable("there is no section after it"))?,
                (None, None) => return Err(invalid("`to` says where the section goes")),
            };
            if to.is_negative() {
                return Err(invalid("`to` must be 1.1.1 or later"));
            }
            // A drop inside the span it came from changes nothing (R8).
            if (section.start..=section.end()).contains(&to) {
                return Ok(result(view, false));
            }
            place(editor, view, label, edge_fades, &section, to, false)
        }
        ids::COPY_SECTION => {
            let CopySectionArgs { section, to } = parse(args)?;
            let doc = editor.document();
            let section = chosen(doc, view, section)?.clone();
            let to = to
                .resolve(&doc.settings().time_signatures)
                .map_err(invalid)?;
            if to.is_negative() {
                return Err(invalid("`to` must be 1.1.1 or later"));
            }
            place(editor, view, label, edge_fades, &section, to, true)
        }
        ids::DUPLICATE_SECTION => {
            let SectionArgs { section } = parse(args)?;
            let section = chosen(editor.document(), view, section)?.clone();
            place(editor, view, label, edge_fades, &section, section.end(), true)
        }
        ids::REMOVE_SECTION => {
            let SectionArgs { section } = parse(args)?;
            let section = chosen(editor.document(), view, section)?.id;
            let (_, recorded) = transaction(editor, label, |editor| {
                apply(editor, Command::RemoveSection { section })
            })?;
            view.forget_missing_sections(editor.document());
            Ok(result(view, recorded))
        }
        ids::DELETE_SECTION => {
            let SectionArgs { section } = parse(args)?;
            let section = chosen(editor.document(), view, section)?.clone();
            let before = editor.document().clone();
            let (_, recorded) = settled(editor, label, (&before, edge_fades), |editor| {
                apply(
                    editor,
                    Command::DeleteTime {
                        start: section.start,
                        end: section.end(),
                        clips: Vec::new(),
                    },
                )
            })?;
            leave_marker(view, section.start);
            view.forget_missing_sections(editor.document());
            Ok(result(view, recorded))
        }
        ids::SELECT_SECTION => {
            let SelectSectionArgs { sections, mode } = parse(args)?;
            let doc = editor.document();
            if let Some(missing) = sections.iter().find(|id| doc.section(**id).is_none()) {
                return Err(invalid(format!("no section has the ID {}", missing.0)));
            }
            let held = view.selected_sections().to_vec();
            let selected = match mode {
                SelectMode::Replace => sections,
                SelectMode::Add => union(held, sections),
                SelectMode::Toggle => {
                    let mut now = held;
                    for id in sections {
                        match now.iter().position(|held| *held == id) {
                            Some(at) => {
                                now.remove(at);
                            }
                            None => now.push(id),
                        }
                    }
                    now
                }
                SelectMode::Extend => extend(doc, &held, &sections),
            };
            view.select_sections(doc, selected);
            Ok(to_value(&view.state(doc)))
        }
        ids::ARRANGER_TRACK => {
            let SwitchArgs { on } = parse(args)?;
            let shown = on.unwrap_or(!editor.document().settings().arranger_track);
            apply(editor, Command::SetArrangerTrack { shown })?;
            Ok(to_value(&view.state(editor.document())))
        }
        other => Err(invalid(format!("`{other}` isn't an arranger command"))),
    }
}

fn result(view: &ArrangementView, undo_step: bool) -> Value {
    to_value(&ArrangerResult {
        selected_sections: view.selected_sections().to_vec(),
        undo_step,
    })
}

/// The section the arguments name, or the first selected one.
fn chosen<'a>(
    doc: &'a Document,
    view: &ArrangementView,
    section: Option<SectionId>,
) -> Result<&'a SongSection, EditFailure> {
    let id = match section {
        Some(id) => id,
        None => *view
            .selected_sections()
            .first()
            .ok_or_else(|| unavailable("no section is selected"))?,
    };
    doc.section(id)
        .ok_or_else(|| invalid(format!("no section has the ID {}", id.0)))
}

/// `start..end` cut short where it would reach a section; a start inside a
/// section is refused (R5).
fn free_span(doc: &Document, start: Ticks, end: Ticks) -> Result<(Ticks, Ticks), EditFailure> {
    if start.is_negative() || end <= start {
        return Err(invalid(
            "the span must start at 1.1.1 or later and end after it starts",
        ));
    }
    if doc
        .sections()
        .iter()
        .any(|s| s.start <= start && s.end() > start)
    {
        return Err(unavailable("a section is already there"));
    }
    let next = doc.sections().iter().find(|s| s.start > start);
    Ok((start, next.map_or(end, |s| end.min(s.start))))
}

/// Changes a section's own fields, as one undo step, and selects it.
#[allow(clippy::too_many_arguments)]
fn edit(
    editor: &mut Editor,
    view: &mut ArrangementView,
    label: &'static str,
    section: SectionId,
    name: Option<String>,
    colour: Option<u8>,
    start: Option<Ticks>,
    length: Option<Ticks>,
) -> Result<Value, EditFailure> {
    let (_, recorded) = transaction(editor, label, |editor| {
        apply(
            editor,
            Command::EditSection {
                section,
                name,
                colour,
                start,
                length,
            },
        )
    })?;
    view.select_sections(editor.document(), vec![section]);
    Ok(result(view, recorded))
}

/// Moves `section`'s span and everything in it to `to`, as Cut Time and
/// Paste Time at the drop would, in one undo step (R8); with `copy`, copies
/// it as Copy Time and Paste Time would (R9, R10). The sections the paste
/// makes are new, since an ID can't be used twice, and become the selected
/// ones.
fn place(
    editor: &mut Editor,
    view: &mut ArrangementView,
    label: &'static str,
    edge_fades: bool,
    section: &SongSection,
    to: Ticks,
    copy: bool,
) -> Result<Value, EditFailure> {
    let before = editor.document().clone();
    let material = time_material(&before, section.start, section.end());
    let (_, recorded) = settled(editor, label, (&before, edge_fades), |editor| {
        let mut at = to;
        if !copy {
            apply(
                editor,
                Command::DeleteTime {
                    start: section.start,
                    end: section.end(),
                    clips: Vec::new(),
                },
            )?;
            // What follows the span has closed up by its length.
            if to > section.end() {
                at = to - section.length;
            }
        }
        paste_material(editor, &material, at)
    })?;
    let made = editor
        .document()
        .sections()
        .iter()
        .filter(|s| before.section(s.id).is_none())
        .map(|s| s.id)
        .collect();
    view.select_sections(editor.document(), made);
    Ok(result(view, recorded))
}

fn union(mut held: Vec<SectionId>, more: Vec<SectionId>) -> Vec<SectionId> {
    for id in more {
        if !held.contains(&id) {
            held.push(id);
        }
    }
    held
}

/// The sections from the first selected one to the last given, in time
/// order, with the ones already selected (R6).
fn extend(doc: &Document, held: &[SectionId], given: &[SectionId]) -> Vec<SectionId> {
    let place = |id: &SectionId| doc.sections().iter().position(|s| s.id == *id);
    let Some(from) = held.first().and_then(place) else {
        return given.to_vec();
    };
    let Some(to) = given.last().and_then(place) else {
        return held.to_vec();
    };
    let (low, high) = (from.min(to), from.max(to));
    union(
        held.to_vec(),
        doc.sections()[low..=high].iter().map(|s| s.id).collect(),
    )
}
