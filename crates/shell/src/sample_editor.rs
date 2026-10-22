//! The Sample Editor's view state (WP-4.10, the sample editor spec): what the
//! Clip View's editor for an audio clip has selected and shows. The warp
//! markers and transients selected, the insert marker and time selection, and
//! the part of the file in view belong to the shell, not the window, so a
//! key, a menu item, and an agent reach them as a click does. None of it is
//! saved with the set or an undo step.
//!
//! A warp marker is named by its place in the clip's warp map. Edits that
//! insert or delete markers move the places of those after them, so
//! [`SampleView::sync`] follows the markers across each change of the set,
//! and a selected marker stays selected through moves, insertions before it,
//! and the deletion of others.

use bl_core::time::Ticks;
use bl_model::{AudioData, ClipData, ClipId, Document, TRANSIENT_MATCH, WarpMarker};
use bl_shell_spec::notes::NoteSelectMode;
use bl_shell_spec::sample_editor::{
    self as ids, SampleEditorState, SamplePoint, SampleViewArgs, SampleViewChange, SelectTimeArgs,
    SelectTransientsArgs, SelectWarpMarkersArgs, SetInsertMarkerArgs,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::arrangement::ArrangementView;
use crate::arrangement::editing::EditFailure;
use crate::clip_view::ClipView;

/// How many zooms `clip_view.sample_view` remembers to go back through.
const BACK_STEPS: usize = 32;

/// What the Sample Editor has selected and shows for one audio clip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SampleView {
    /// The clip the state belongs to; showing another clip starts afresh.
    clip: Option<ClipId>,
    /// Whether the clip was warped when the view was last synced: the
    /// editor's axis, and so the span in view, change with it.
    warped: bool,
    /// The selected warp markers, by place, in order.
    markers: Vec<u32>,
    /// The clip's warp markers as the selection last saw them.
    seen: Vec<WarpMarker>,
    /// Whether the Start marker is selected.
    start_marker: bool,
    /// The selected transients, by time in the file, in order.
    transients: Vec<f64>,
    /// The insert marker, as a time in the file in seconds.
    insert: Option<f64>,
    /// The time selection, as times in the file in seconds.
    selection: Option<(f64, f64)>,
    /// The span in view, in ticks of a warped clip's contents or seconds in
    /// an unwarped clip's file; `None` shows the whole file.
    view: Option<(f64, f64)>,
    /// The views before each zoom, newest last.
    back: Vec<Option<(f64, f64)>>,
    /// Whether the waveform is fitted to the editor's height.
    fit_height: bool,
    /// Whether the editor has stopped following playback until the song
    /// starts or stops (an edit or a scroll by hand pauses it).
    follow_paused: bool,
}

fn invalid(message: impl Into<String>) -> EditFailure {
    EditFailure::InvalidArgs(message.into())
}

fn unavailable(reason: impl Into<String>) -> EditFailure {
    EditFailure::Unavailable(reason.into())
}

fn refused(reason: impl Into<String>) -> EditFailure {
    EditFailure::Refused(reason.into())
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, EditFailure> {
    let args = if args.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        args
    };
    serde_json::from_value(args).map_err(|error| invalid(error.to_string()))
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn same_time(a: f64, b: f64) -> bool {
    (a - b).abs() <= TRANSIENT_MATCH
}

/// The audio data of `clip`, or why it has none.
fn audio_of(doc: &Document, clip: ClipId) -> Result<&AudioData, EditFailure> {
    match doc.clip(clip).map(|c| &c.data) {
        Some(ClipData::Audio(audio)) => Ok(audio),
        Some(ClipData::Midi(_)) => Err(refused("the clip isn't an audio clip")),
        None => Err(invalid(format!("no clip has the ID {}", clip.0))),
    }
}

impl SampleView {
    /// The clip the state is for.
    #[must_use]
    pub fn clip(&self) -> Option<ClipId> {
        self.clip
    }

    /// The selected warp markers, by place in the clip's warp map.
    #[must_use]
    pub fn markers(&self) -> &[u32] {
        &self.markers
    }

    /// Whether the Start marker is selected.
    #[must_use]
    pub fn start_marker(&self) -> bool {
        self.start_marker
    }

    /// The selected transients, by time in the file in seconds.
    #[must_use]
    pub fn transients(&self) -> &[f64] {
        &self.transients
    }

    /// The insert marker, as a time in the file in seconds.
    #[must_use]
    pub fn insert_marker(&self) -> Option<f64> {
        self.insert
    }

    /// The time selection's ends, as times in the file in seconds.
    #[must_use]
    pub fn time_selection(&self) -> Option<(f64, f64)> {
        self.selection
    }

    /// The span in view: ticks of a warped clip's contents or seconds in an
    /// unwarped clip's file. `None` shows the whole file.
    #[must_use]
    pub fn span(&self) -> Option<(f64, f64)> {
        self.view
    }

    /// Whether the waveform is fitted to the editor's height.
    #[must_use]
    pub fn fit_height(&self) -> bool {
        self.fit_height
    }

    /// Whether the editor waits to follow playback again.
    #[must_use]
    pub fn follow_paused(&self) -> bool {
        self.follow_paused
    }

    /// Stops the editor following playback until the song starts or stops.
    pub fn pause_follow(&mut self) {
        self.follow_paused = true;
    }

    /// Lets the editor follow playback again.
    pub fn resume_follow(&mut self) {
        self.follow_paused = false;
    }

    /// Starts afresh for `clip`, if it isn't the clip the state is for.
    pub(crate) fn bind_to(&mut self, clip: ClipId, audio: &AudioData) {
        if self.clip != Some(clip) {
            *self = Self {
                clip: Some(clip),
                warped: audio.warp,
                seen: audio.warp_markers.clone(),
                fit_height: self.fit_height,
                ..Self::default()
            };
        }
    }

    /// Follows the set's changes: the selection follows its markers to their
    /// new places, and the span in view is dropped when Warp is switched,
    /// since the axis changes with it. Called whenever the set may have
    /// changed.
    pub fn sync(&mut self, doc: &Document) {
        let Some(clip) = self.clip else {
            return;
        };
        let Ok(audio) = audio_of(doc, clip) else {
            *self = Self {
                fit_height: self.fit_height,
                ..Self::default()
            };
            return;
        };
        if audio.warp != self.warped {
            self.warped = audio.warp;
            self.view = None;
            self.back.clear();
        }
        if audio.warp_markers != self.seen {
            // Moves and shifts keep every marker's place; an insertion or a
            // deletion moves the places after it, so follow each selected
            // marker by what it is.
            if audio.warp_markers.len() != self.seen.len() {
                self.markers = self
                    .markers
                    .iter()
                    .filter_map(|&place| {
                        let marker = self.seen.get(place as usize)?;
                        let at = audio.warp_markers.iter().position(|m| m == marker)?;
                        u32::try_from(at).ok()
                    })
                    .collect();
            }
            self.seen.clone_from(&audio.warp_markers);
        }
        let places = audio.warp_markers.len();
        self.markers.retain(|&place| (place as usize) < places);
    }

    /// The beat of a warped clip's contents that `time` seconds in the file
    /// plays on.
    #[must_use]
    pub fn beat_of(audio: &AudioData, time: f64) -> Option<Ticks> {
        let map = audio.warp.then(|| audio.warp_map().ok()).flatten()?;
        Some(Ticks(map.time_to_beat(time).round() as i64))
    }

    fn point(audio: &AudioData, time: f64) -> SamplePoint {
        SamplePoint {
            time,
            beat: Self::beat_of(audio, time),
        }
    }

    /// What the editor shows, as the commands report it.
    #[must_use]
    pub fn state(&self, doc: &Document, shown: Option<ClipId>) -> SampleEditorState {
        let (Some(clip), true) = (shown, shown == self.clip) else {
            return SampleEditorState {
                clip: shown,
                ..SampleEditorState::default()
            };
        };
        let Ok(audio) = audio_of(doc, clip) else {
            return SampleEditorState::default();
        };
        SampleEditorState {
            clip: Some(clip),
            warped: audio.warp,
            markers: self.markers.clone(),
            start_marker: self.start_marker,
            transients: self.transients.clone(),
            insert_marker: self.insert.map(|time| Self::point(audio, time)),
            time_selection: self
                .selection
                .map(|(from, to)| (Self::point(audio, from), Self::point(audio, to))),
            view: self.view,
            fit_height: self.fit_height,
        }
    }

    fn push_back(&mut self) {
        if self.back.len() == BACK_STEPS {
            self.back.remove(0);
        }
        self.back.push(self.view);
    }
}

/// A selection after `mode` applied `chosen` to `now`, kept in order.
fn select<T: Copy + PartialOrd>(
    now: &[T],
    chosen: &[T],
    mode: NoteSelectMode,
    same: impl Fn(T, T) -> bool,
) -> Vec<T> {
    let mut out: Vec<T> = match mode {
        NoteSelectMode::Replace => Vec::new(),
        NoteSelectMode::Toggle => now.to_vec(),
    };
    for &item in chosen {
        match out.iter().position(|&other| same(other, item)) {
            Some(at) if mode == NoteSelectMode::Toggle => {
                out.remove(at);
            }
            Some(_) => {}
            None => out.push(item),
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// The audio clip a Sample Editor command acts on: the one given, or the
/// shown one.
fn clip_of(
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
    clip: Option<ClipId>,
) -> Result<ClipId, EditFailure> {
    clip.or_else(|| view.shown_audio_clip(doc, arrangement))
        .ok_or_else(|| unavailable("no audio clip is shown in the Clip View"))
}

/// The MIDI clip a command acts on, when the arguments name one or the
/// Clip View shows one: the note editor's own insert marker and time
/// selection (piano-roll R23, R24, R31).
fn midi_target(
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
    args: &serde_json::Value,
) -> Option<ClipId> {
    let named = args
        .get("clip")
        .and_then(|clip| serde_json::from_value::<ClipId>(clip.clone()).ok());
    let clip = named.or_else(|| view.shown_any_clip(doc, arrangement))?;
    doc.clip(clip)
        .is_some_and(|value| matches!(value.data, ClipData::Midi(_)))
        .then_some(clip)
}

/// Runs a Sample Editor command (`clip_view.select_warp_markers` and its
/// siblings), which change only the view.
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
pub fn run(
    id: &str,
    args: Value,
    doc: &Document,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    let report = |view: &ClipView, clip: ClipId| to_value(&view.sample().state(doc, Some(clip)));
    match id {
        ids::STATE => {
            parse::<bl_commands::NoArgs>(args)?;
            let shown = view.shown_audio_clip(doc, arrangement);
            Ok(to_value(&view.sample().state(doc, shown)))
        }
        ids::SELECT_WARP_MARKERS => {
            let SelectWarpMarkersArgs {
                clip,
                markers,
                mode,
                start_marker,
            } = parse(args)?;
            let clip = clip_of(doc, view, arrangement, clip)?;
            let audio = audio_of(doc, clip)?;
            if !markers.is_empty() && !audio.warp {
                return Err(refused(
                    "the clip isn't warped: it has no warp markers to select",
                ));
            }
            if let Some(place) = markers
                .iter()
                .find(|&&place| place as usize >= audio.warp_markers.len())
            {
                return Err(invalid(format!(
                    "clip {} has no warp marker at place {place}",
                    clip.0
                )));
            }
            let sample = view.sample_mut(clip, audio);
            let now = sample.markers.clone();
            sample.markers = select(&now, &markers, mode, |a, b| a == b);
            sample.start_marker = match mode {
                NoteSelectMode::Replace => start_marker,
                NoteSelectMode::Toggle => sample.start_marker ^ start_marker,
            };
            // A click selects one kind of thing.
            if mode == NoteSelectMode::Replace && (!markers.is_empty() || start_marker) {
                sample.transients.clear();
            }
            Ok(report(view, clip))
        }
        ids::SELECT_TRANSIENTS => {
            let SelectTransientsArgs { clip, times, mode } = parse(args)?;
            let clip = clip_of(doc, view, arrangement, clip)?;
            let audio = audio_of(doc, clip)?;
            if times.iter().any(|t| !t.is_finite() || *t < 0.0) {
                return Err(invalid("`times` are seconds in the file: 0 or more"));
            }
            let sample = view.sample_mut(clip, audio);
            let now = sample.transients.clone();
            sample.transients = select(&now, &times, mode, same_time);
            if mode == NoteSelectMode::Replace && !times.is_empty() {
                sample.markers.clear();
                sample.start_marker = false;
            }
            Ok(report(view, clip))
        }
        ids::SET_INSERT_MARKER => {
            // A MIDI clip's editor has an insert marker and a time
            // selection of its own (piano-roll R23, R24, R31).
            if midi_target(doc, view, arrangement, &args).is_some() {
                return crate::clip_view::run_view(id, args, doc, view, arrangement);
            }
            let SetInsertMarkerArgs { clip, time, beat } = parse(args)?;
            let clip = clip_of(doc, view, arrangement, clip)?;
            let audio = audio_of(doc, clip)?;
            let at = match (time, beat) {
                (Some(_), Some(_)) => return Err(invalid("give `time` or `beat`, not both")),
                (Some(time), None) => Some(time),
                (None, Some(beat)) => {
                    let map = audio
                        .warp
                        .then(|| audio.warp_map().ok())
                        .flatten()
                        .ok_or_else(|| refused("the clip isn't warped: give `time`"))?;
                    Some(map.beat_to_time(beat.get() as f64))
                }
                (None, None) => None,
            };
            if let Some(at) = at
                && !(at.is_finite() && at >= 0.0)
            {
                return Err(invalid(
                    "the insert marker lies in the file: `time` is 0 or more",
                ));
            }
            let sample = view.sample_mut(clip, audio);
            sample.insert = at;
            if at.is_some() {
                sample.selection = None;
            }
            Ok(report(view, clip))
        }
        ids::SELECT_TIME => {
            // A MIDI clip's editor has an insert marker and a time
            // selection of its own (piano-roll R23, R24, R31).
            if midi_target(doc, view, arrangement, &args).is_some() {
                return crate::clip_view::run_view(id, args, doc, view, arrangement);
            }
            let SelectTimeArgs {
                clip,
                start,
                end,
                ticks,
            } = parse(args)?;
            let clip = clip_of(doc, view, arrangement, clip)?;
            let audio = audio_of(doc, clip)?;
            let selection = match (start, end) {
                (None, None) => None,
                (Some(start), Some(end)) => {
                    if !(start.is_finite() && end.is_finite() && start < end) {
                        return Err(invalid("`start` must be before `end`"));
                    }
                    let (from, to) = if ticks {
                        let map = audio
                            .warp
                            .then(|| audio.warp_map().ok())
                            .flatten()
                            .ok_or_else(|| refused("the clip isn't warped: give seconds"))?;
                        (map.beat_to_time(start), map.beat_to_time(end))
                    } else {
                        (start, end)
                    };
                    if from < 0.0 {
                        return Err(invalid(
                            "the selection lies in the file: it starts at 0 or later",
                        ));
                    }
                    Some((from, to))
                }
                _ => return Err(invalid("give both `start` and `end`, or neither")),
            };
            let sample = view.sample_mut(clip, audio);
            sample.selection = selection;
            if selection.is_some() {
                sample.insert = None;
            }
            Ok(report(view, clip))
        }
        ids::SAMPLE_VIEW => {
            let SampleViewArgs {
                clip,
                show,
                start,
                end,
                ticks,
            } = parse(args)?;
            let clip = clip_of(doc, view, arrangement, clip)?;
            let audio = audio_of(doc, clip)?;
            let sample = view.sample_mut(clip, audio);
            match show {
                None => {}
                Some(SampleViewChange::Selection) => {
                    let (from, to) = sample
                        .selection
                        .ok_or_else(|| unavailable("make a time selection first"))?;
                    let axis = |time: f64| match SampleView::beat_of(audio, time) {
                        Some(beat) => beat.get() as f64,
                        None => time,
                    };
                    sample.push_back();
                    sample.view = Some((axis(from), axis(to)));
                }
                Some(SampleViewChange::Back) => {
                    sample.view = sample
                        .back
                        .pop()
                        .ok_or_else(|| unavailable("there is no earlier view to go back to"))?;
                }
                Some(SampleViewChange::Whole) => {
                    if sample.view.is_some() {
                        sample.push_back();
                        sample.view = None;
                    }
                }
                Some(SampleViewChange::Height) => sample.fit_height = !sample.fit_height,
                Some(
                    change @ (SampleViewChange::Span
                    | SampleViewChange::Scroll
                    | SampleViewChange::Page),
                ) => {
                    let (Some(start), Some(end)) = (start, end) else {
                        return Err(invalid("`span` and `scroll` need `start` and `end`"));
                    };
                    if !(start.is_finite() && end.is_finite() && start < end) {
                        return Err(invalid("`start` must be before `end`"));
                    }
                    if ticks != audio.warp {
                        return Err(refused(if audio.warp {
                            "the clip is warped: give `start` and `end` in ticks with `ticks`"
                        } else {
                            "the clip isn't warped: give `start` and `end` in seconds"
                        }));
                    }
                    if change == SampleViewChange::Page {
                        sample.view = Some((start, end));
                    } else if change == SampleViewChange::Scroll {
                        // Spec: sample-editor R11. Scrolling by hand pauses
                        // Follow until playback starts or stops.
                        sample.view = Some((start, end));
                        sample.follow_paused = true;
                    } else if sample.view != Some((start, end)) {
                        sample.push_back();
                        sample.view = Some((start, end));
                    }
                }
            }
            Ok(report(view, clip))
        }
        other => Err(invalid(format!("no Sample Editor command is `{other}`"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_adds_and_removes_and_replacing_starts_over() {
        let same = |a: u32, b: u32| a == b;
        let now = [1, 3];
        assert_eq!(select(&now, &[3, 5], NoteSelectMode::Toggle, same), [1, 5]);
        assert_eq!(select(&now, &[2], NoteSelectMode::Replace, same), [2]);
        assert!(select(&now, &[], NoteSelectMode::Replace, same).is_empty());
    }

    #[test]
    fn transients_a_hair_apart_are_one() {
        let now = [1.0];
        let chosen = [1.0 + TRANSIENT_MATCH / 2.0];
        assert!(select(&now, &chosen, NoteSelectMode::Toggle, same_time).is_empty());
    }
}
