//! The …Time commands (WP-6.24b): Insert Silence, Cut, Copy, Paste,
//! Duplicate and Delete Time. They act on a span of time on every track at
//! once, through the document's `InsertTime` and `DeleteTime`, and carry
//! the clips, envelopes, tempo, time-signature markers and locators over
//! the span on a time clipboard. The behaviour is in the time editing spec,
//! R14 to R24.

use std::collections::BTreeMap;

use bl_core::time::{Bpm, Ticks, TimeSignatureChange};
use bl_model::{
    AutomationTarget, Breakpoint, Clip, Command, Document, Editor, Locator, SongSection,
    TakeLaneId, TrackId,
};
use bl_shell_spec::arrangement::Lane;
use bl_shell_spec::editing::{
    self as ids, FragmentaryBarArgs, PasteTimeArgs, TimeClipboardState, TimeSpanArgs,
};
use serde_json::Value;

use super::editing::{
    Clipboard, EditFailure, EnvelopeMaterial, Material, apply, envelopes_over, fits, invalid,
    paste_plan, place, place_on_lane, place_on_main, refused, result, settled, to_value,
    unavailable, write_envelopes,
};
use super::placing::piece;
use super::{ArrangementView, parse};

/// One track's share of a copied span, with positions counted from the
/// span's start.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackTime {
    /// The track it came from.
    pub track: TrackId,
    /// Its place in the set's track order, counted from 0.
    pub place: usize,
    /// The clips of its main lane, or parts of them.
    pub clips: Vec<Clip>,
    /// The clips of each of its take lanes, or parts of them, with the lane
    /// and its place among the track's take lanes, counted from 0.
    pub lanes: Vec<(TakeLaneId, usize, Vec<Clip>)>,
    /// Each parameter's shape over the span.
    pub envelopes: Vec<(AutomationTarget, Vec<Breakpoint>)>,
}

/// What Copy Time takes from a span: every track's clips and envelopes, the
/// tempo, the time-signature markers and the locators over it (time editing
/// R18).
#[derive(Clone, Debug, PartialEq)]
pub struct TimeMaterial {
    /// Whether it was copied in another set, so its tracks and take lanes are
    /// found by their places, not their IDs (the set exchange spec, R6).
    pub foreign: bool,
    /// How long the span is.
    pub length: Ticks,
    /// The tracks that held anything over it.
    pub tracks: Vec<TrackTime>,
    /// The tempo over it (`bl_model::tempo_shape`).
    pub tempo: Vec<(Ticks, Bpm)>,
    /// The time-signature markers inside it.
    pub time_signatures: Vec<TimeSignatureChange>,
    /// The locators inside it.
    pub locators: Vec<Locator>,
    /// The sections inside it, cut to it (the arranger track spec, R13).
    pub sections: Vec<SongSection>,
}

impl TimeMaterial {
    /// Every clip it holds.
    pub(super) fn clips(&self) -> impl Iterator<Item = &Clip> {
        self.tracks.iter().flat_map(|track| {
            track
                .clips
                .iter()
                .chain(track.lanes.iter().flat_map(|(_, _, clips)| clips))
        })
    }

    /// Makes it a paste into another set (`Clipboard::leave_set`).
    pub(super) fn leave_set(&mut self) {
        self.foreign = true;
        for track in &mut self.tracks {
            track
                .clips
                .iter_mut()
                .chain(track.lanes.iter_mut().flat_map(|(_, _, clips)| clips))
                .for_each(super::editing::leave_set_clip);
            track
                .envelopes
                .retain(|(target, _)| !super::editing::names_an_object_of_its_set(target));
        }
    }

    /// What it holds, as `arrangement.copy_time` reports it.
    #[must_use]
    pub fn state(&self) -> TimeClipboardState {
        TimeClipboardState {
            clips: self
                .tracks
                .iter()
                .map(|t| {
                    t.clips.len()
                        + t.lanes
                            .iter()
                            .map(|(_, _, clips)| clips.len())
                            .sum::<usize>()
                })
                .sum(),
            tracks: self.tracks.len(),
            envelopes: self.tracks.iter().map(|t| t.envelopes.len()).sum(),
            tempo_breakpoints: self.tempo.len(),
            time_signatures: self.time_signatures.len(),
            locators: self.locators.len(),
            length: self.length,
        }
    }
}

/// What the span `start..end` holds on every track, from `start`.
pub(super) fn time_material(doc: &Document, start: Ticks, end: Ticks) -> TimeMaterial {
    let tempo = &doc.settings().tempo;
    let parts = |clips: &mut dyn Iterator<Item = &Clip>| -> Vec<Clip> {
        let mut parts: Vec<Clip> = clips
            .filter(|c| c.start < end && c.end() > start)
            .map(|c| {
                let part = piece(c, c.start.max(start), c.end().min(end), tempo);
                Clip {
                    start: part.start - start,
                    ..part
                }
            })
            .collect();
        parts.sort_by_key(|c| (c.start, c.id));
        parts
    };
    let order = doc.track_order();
    let tracks = doc
        .tracks()
        .map(|track| TrackTime {
            track: track.id(),
            place: order.iter().position(|&t| t == track.id()).unwrap_or(0),
            clips: parts(&mut track.clips()),
            lanes: track
                .take_lanes()
                .enumerate()
                .map(|(place, lane)| (lane.id, place, parts(&mut lane.clips.iter())))
                .filter(|(_, _, clips)| !clips.is_empty())
                .collect(),
            envelopes: envelopes_over(doc, track.id(), start, end),
        })
        .filter(|t| !(t.clips.is_empty() && t.lanes.is_empty() && t.envelopes.is_empty()))
        .collect();
    let settings = doc.settings();
    TimeMaterial {
        foreign: false,
        length: end - start,
        tracks,
        tempo: bl_model::tempo_shape(&settings.tempo, start, end),
        time_signatures: settings
            .time_signatures
            .changes()
            .filter(|c| c.at >= start && c.at < end)
            .map(|c| TimeSignatureChange {
                at: c.at - start,
                ..c
            })
            .collect(),
        locators: settings
            .locators
            .iter()
            .filter(|l| l.position >= start && l.position < end)
            .map(|l| Locator {
                position: l.position - start,
                ..l.clone()
            })
            .collect(),
        sections: doc
            .sections()
            .iter()
            .filter(|s| s.start < end && s.end() > start)
            .map(|s| SongSection {
                start: s.start.max(start) - start,
                length: s.end().min(end) - s.start.max(start),
                ..s.clone()
            })
            .collect(),
    }
}

/// Opens `material.length` of time at `at` and puts the material in it
/// (time editing R19, R20). The sections it carries take new IDs.
pub(super) fn paste_material(
    editor: &mut Editor,
    material: &TimeMaterial,
    at: Ticks,
) -> Result<(), EditFailure> {
    apply(
        editor,
        Command::InsertTime {
            at,
            length: material.length,
            clips: Vec::new(),
        },
    )?;
    let mut envelopes = Vec::new();
    let mut landed = BTreeMap::new();
    for held in &material.tracks {
        let doc = editor.document();
        // Material from another set finds its tracks and take lanes by place.
        let destination = if material.foreign {
            doc.track_order().get(held.place).copied()
        } else {
            Some(held.track)
        };
        let Some(destination) = destination else {
            continue;
        };
        let Some(track) = doc.track(destination) else {
            continue;
        };
        // Clips that don't fit the track now there are left out.
        let shift = |clips: &[Clip]| -> Vec<Clip> {
            clips
                .iter()
                .filter(|c| fits(doc, destination, &c.data))
                .map(|c| Clip {
                    start: c.start + at,
                    ..c.clone()
                })
                .collect()
        };
        let lanes: Vec<(TakeLaneId, Vec<Clip>)> = held
            .lanes
            .iter()
            .filter_map(|(lane, place, clips)| {
                let lane = if material.foreign {
                    track.take_lanes().nth(*place)?.id
                } else {
                    track.take_lane(*lane)?.id
                };
                Some((lane, shift(clips)))
            })
            .collect();
        // A parameter whose device is gone since the copy gets nothing.
        let shapes = held
            .envelopes
            .iter()
            .filter(|(target, _)| doc.check_automation_target(destination, *target).is_ok())
            .cloned()
            .collect();
        let main = shift(&held.clips);
        if !main.is_empty() {
            place_on_main(editor, vec![(destination, main)])?;
        }
        for (lane, clips) in lanes {
            place_on_lane(editor, destination, lane, clips)?;
        }
        landed.insert(envelopes.len(), destination);
        envelopes.push(EnvelopeMaterial {
            source: destination,
            distance: envelopes.len(),
            start: Ticks::ZERO,
            length: material.length,
            shapes,
        });
    }
    write_envelopes(editor, &envelopes, &landed, at)?;

    let end = at + material.length;
    let tempo = bl_model::write_tempo_shape(
        &editor.document().settings().tempo,
        at,
        end,
        &material.tempo,
    );
    if tempo != editor.document().settings().tempo {
        apply(editor, Command::SetTempo { tempo })?;
    }
    for change in &material.time_signatures {
        apply(
            editor,
            Command::SetTimeSignatureChange {
                at: at + change.at,
                signature: change.signature,
            },
        )?;
    }
    // A marker the opened time pushed to its end that only restates the
    // signature in force there (Delete Time put it at the span's start)
    // would start a short bar, so it goes.
    let signatures = &editor.document().settings().time_signatures;
    if !material.time_signatures.is_empty()
        && signatures.changes().any(|c| c.at == end)
        && signatures.signature_at(end) == signatures.signature_at(end - Ticks(1))
    {
        apply(editor, Command::DeleteTimeSignatureChange { at: end })?;
    }
    for locator in &material.locators {
        let id = editor.new_id();
        let position = at + locator.position;
        let locator = Locator {
            id,
            position,
            ..locator.clone()
        };
        apply(editor, Command::AddLocator { locator })?;
    }
    paste_sections(editor, material, at)
}

/// Puts the sections the material carries where it landed, leaving out the
/// parts that fall where a section already is (the arranger track spec, R13,
/// R16).
fn paste_sections(
    editor: &mut Editor,
    material: &TimeMaterial,
    at: Ticks,
) -> Result<(), EditFailure> {
    for section in &material.sections {
        let (start, end) = (at + section.start, at + section.start + section.length);
        let held: Vec<(Ticks, Ticks)> = editor
            .document()
            .sections()
            .iter()
            .map(|other| (other.start, other.end()))
            .collect();
        let (mut free, mut from) = (Vec::new(), start);
        for (other_start, other_end) in held {
            if other_end <= from || other_start >= end {
                continue;
            }
            if other_start > from {
                free.push((from, other_start));
            }
            from = from.max(other_end);
        }
        if from < end {
            free.push((from, end));
        }
        for (from, to) in free {
            let piece = SongSection {
                id: editor.new_id(),
                start: from,
                length: to - from,
                ..section.clone()
            };
            apply(editor, Command::AddSection { section: piece })?;
        }
    }
    Ok(())
}

/// The span the arguments name, or the time selection.
pub(super) fn span(
    doc: &Document,
    view: &ArrangementView,
    args: TimeSpanArgs,
) -> Result<(Ticks, Ticks), EditFailure> {
    let signatures = &doc.settings().time_signatures;
    let start = match args.start {
        Some(start) => start.resolve(signatures).map_err(invalid)?,
        None => view
            .time_selection
            .map(|(start, _)| start)
            .ok_or_else(|| unavailable("no time is selected"))?,
    };
    let end = match args.end {
        Some(end) => end.resolve(signatures).map_err(invalid)?,
        None => view
            .time_selection
            .map(|(_, end)| end)
            .ok_or_else(|| unavailable("no time is selected"))?,
    };
    if start.is_negative() || end <= start {
        return Err(invalid(
            "the span must start at 1.1.1 or later and end after it starts",
        ));
    }
    Ok((start, end))
}

/// Selects `start..end` on the lanes already selected, with no clips.
pub(super) fn select_time(view: &mut ArrangementView, start: Ticks, end: Ticks) {
    let lanes = if view.time_selection_lanes.is_empty() {
        view.selected
            .iter()
            .map(|&track| Lane::Track { track })
            .collect()
    } else {
        view.time_selection_lanes.clone()
    };
    view.select_span_on(lanes, start, end);
}

/// The insert marker at `at` with no time selected, as Delete Time leaves it
/// (R15).
pub(super) fn leave_marker(view: &mut ArrangementView, at: Ticks) {
    view.selected_clips.clear();
    view.time_selection = None;
    view.time_selection_lanes.clear();
    view.insert_marker = at;
}

/// Whether a person can run time command `id` now (R22): there's a span to
/// act on, or, for Paste Time, something on the clipboard.
#[must_use]
pub fn can_run(
    id: &str,
    args: &Value,
    view: &ArrangementView,
    clipboard: Option<&Clipboard>,
) -> bool {
    if id == ids::PASTE_TIME {
        return match clipboard {
            Some(Clipboard::Time(_)) => true,
            Some(Clipboard::Material(material)) => !material.lanes.is_empty(),
            _ => false,
        };
    }
    args.get("start").is_some() || view.time_selection.is_some()
}

/// Runs time command `id` with `args`, as `editing::run` does the others.
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
pub fn run(
    id: &str,
    args: Value,
    editor: &mut Editor,
    view: &mut ArrangementView,
    clipboard: &mut Option<Clipboard>,
    edge_fades: bool,
) -> Result<Value, EditFailure> {
    let before = editor.document().clone();
    let doc = &before;
    let label = super::editing::label(id, &args);
    match id {
        ids::COPY_TIME => {
            let (start, end) = span(doc, view, parse(args)?)?;
            let copied = time_material(doc, start, end);
            let state = copied.state();
            *clipboard = Some(Clipboard::Time(copied));
            Ok(to_value(&state))
        }
        ids::INSERT_SILENCE => {
            let (start, end) = span(doc, view, parse(args)?)?;
            let (_, recorded) = settled(editor, label, (doc, edge_fades), |editor| {
                apply(
                    editor,
                    Command::InsertTime {
                        at: start,
                        length: end - start,
                        clips: Vec::new(),
                    },
                )
            })?;
            select_time(view, start, end);
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::DELETE_TIME | ids::CUT_TIME => {
            let (start, end) = span(doc, view, parse(args)?)?;
            let copied = (id == ids::CUT_TIME).then(|| time_material(doc, start, end));
            let (_, recorded) = settled(editor, label, (doc, edge_fades), |editor| {
                apply(
                    editor,
                    Command::DeleteTime {
                        start,
                        end,
                        clips: Vec::new(),
                    },
                )
            })?;
            if let Some(copied) = copied {
                *clipboard = Some(Clipboard::Time(copied));
            }
            leave_marker(view, start);
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::DUPLICATE_TIME => {
            let (start, end) = span(doc, view, parse(args)?)?;
            let copied = time_material(doc, start, end);
            let (_, recorded) = settled(editor, label, (doc, edge_fades), |editor| {
                paste_material(editor, &copied, end)
            })?;
            select_time(view, end, end + copied.length);
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::PASTE_TIME => {
            let PasteTimeArgs { at } = parse(args)?;
            let at = match at {
                Some(at) => at
                    .resolve(&doc.settings().time_signatures)
                    .map_err(invalid)?,
                None => view
                    .time_selection
                    .map_or(view.insert_marker, |(start, _)| start),
            };
            if at.is_negative() {
                return Err(invalid("`at` must be 1.1.1 or later"));
            }
            let (length, recorded) = match clipboard.as_ref() {
                Some(Clipboard::Time(copied)) => {
                    let copied = copied.clone();
                    let (_, recorded) = settled(editor, label, (doc, edge_fades), |editor| {
                        paste_material(editor, &copied, at)
                    })?;
                    (copied.length, recorded)
                }
                Some(Clipboard::Material(copied)) if !copied.lanes.is_empty() => {
                    let copied = copied.clone();
                    let (_, recorded) = settled(editor, label, (doc, edge_fades), |editor| {
                        paste_clips(editor, view, &copied, at)
                    })?;
                    (copied.length, recorded)
                }
                _ => return Err(unavailable("there's nothing on the clipboard to paste")),
            };
            select_time(view, at, at + length);
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        ids::DELETE_FRAGMENTARY_BAR_TIME | ids::COMPLETE_FRAGMENTARY_BAR => {
            let FragmentaryBarArgs { at } = parse(args)?;
            let signatures = &doc.settings().time_signatures;
            let at = at.resolve(signatures).map_err(invalid)?;
            let bar = signatures
                .fragmentary_bars()
                .find(|bar| (bar.start..bar.end).contains(&at))
                .ok_or_else(|| invalid("no time-signature change cuts a bar short there"))?;
            let (_, recorded) = settled(editor, label, (doc, edge_fades), |editor| {
                if id == ids::DELETE_FRAGMENTARY_BAR_TIME {
                    return apply(
                        editor,
                        Command::DeleteTime {
                            start: bar.start,
                            end: bar.end,
                            clips: Vec::new(),
                        },
                    );
                }
                let length = bar.missing();
                apply(
                    editor,
                    Command::InsertTime {
                        at: bar.start,
                        length,
                        clips: Vec::new(),
                    },
                )?;
                // Opening time at a marker pushes the marker later too; the
                // marker that starts the short bar stays where it is.
                if bar.start > Ticks::ZERO
                    && signatures.changes().any(|change| change.at == bar.start)
                {
                    apply(
                        editor,
                        Command::MoveTimeSignatureChange {
                            at: bar.start + length,
                            to: bar.start,
                        },
                    )?;
                }
                Ok(())
            })?;
            if id == ids::DELETE_FRAGMENTARY_BAR_TIME {
                leave_marker(view, bar.start);
            }
            Ok(to_value(&result(doc, editor, view, recorded)))
        }
        other => Err(refused(format!("`{other}` isn't a time command"))),
    }
}

/// Opens time for clips copied with Copy or Cut and puts them where Paste
/// puts them (time editing R19).
fn paste_clips(
    editor: &mut Editor,
    view: &ArrangementView,
    copied: &Material,
    at: Ticks,
) -> Result<(), EditFailure> {
    let plan = paste_plan(editor.document(), view, copied, at, (None, None))?;
    apply(
        editor,
        Command::InsertTime {
            at,
            length: copied.length,
            clips: Vec::new(),
        },
    )?;
    place(editor, plan.placements)?;
    write_envelopes(editor, &copied.envelopes, &plan.landed, at)?;
    Ok(())
}
