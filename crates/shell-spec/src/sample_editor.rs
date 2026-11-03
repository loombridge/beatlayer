//! The Sample Editor's view commands in the command registry (WP-4.10): the
//! warp markers and transients selected in the Clip View's editor for an
//! audio clip, its insert marker and time selection, and what part of the
//! file it shows. The sample editor spec holds the behaviour.
//!
//! These change only what the Clip View shows, never the set, so every one
//! is in the read class and none is an undo step. The edits the editor makes
//! are the document's own `clip.*` warp commands, which agents run too.
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them
//! by ID.

use bl_commands::Context::{ClipView, NoteEditor, NoteSelection, SampleEditor};
use bl_commands::{Binding, CommandSpec, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::ClipId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::notes::NoteSelectMode;

/// `clip_view.sample_editor`: what the Sample Editor shows and has selected.
pub const STATE: &str = "clip_view.sample_editor";
/// `clip_view.select_warp_markers`: select warp markers.
pub const SELECT_WARP_MARKERS: &str = "clip_view.select_warp_markers";
/// `clip_view.select_transients`: select transients.
pub const SELECT_TRANSIENTS: &str = "clip_view.select_transients";
/// `clip_view.set_insert_marker`: place the Sample Editor's insert marker.
pub const SET_INSERT_MARKER: &str = "clip_view.set_insert_marker";
/// `clip_view.select_time`: make a time selection in the Sample Editor.
pub const SELECT_TIME: &str = "clip_view.select_time";
/// `clip_view.sample_view`: zoom and scroll the Sample Editor.
pub const SAMPLE_VIEW: &str = "clip_view.sample_view";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    STATE,
    SELECT_WARP_MARKERS,
    SELECT_TRANSIENTS,
    SET_INSERT_MARKER,
    SELECT_TIME,
    SAMPLE_VIEW,
];

/// Whether `id` is one of the Sample Editor's view commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `clip_view.select_warp_markers`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectWarpMarkersArgs {
    /// The audio clip. Left out, the one the Sample Editor shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The markers, by their place in the clip's warp map, the first being
    /// 0. Empty with `replace` clears the selection.
    pub markers: Vec<u32>,
    /// How to change the selection; `replace` if left out.
    #[serde(default)]
    pub mode: NoteSelectMode,
    /// Whether the clip's Start marker is selected too, as clicking it in
    /// the marker strip does; Shift with the arrow keys then moves the
    /// clip's region with it. With `toggle` it switches the Start marker's
    /// selection. Not selected if left out.
    #[serde(default)]
    pub start_marker: bool,
}

/// Arguments of `clip_view.select_transients`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectTransientsArgs {
    /// The audio clip. Left out, the one the Sample Editor shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The transients, by their times in the file in seconds, as
    /// `clip.transients` lists them. Empty with `replace` clears the
    /// selection.
    pub times: Vec<f64>,
    /// How to change the selection; `replace` if left out.
    #[serde(default)]
    pub mode: NoteSelectMode,
}

/// Arguments of `clip_view.set_insert_marker`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetInsertMarkerArgs {
    /// The audio clip. Left out, the one the Sample Editor shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// Where to put the marker, as a time in the file in seconds. Give this
    /// or `beat`; give neither to take the marker away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<f64>,
    /// Where to put the marker, as a beat of a warped clip's contents in
    /// ticks from its 1.1.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat: Option<Ticks>,
}

/// Arguments of `clip_view.select_time`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectTimeArgs {
    /// The audio clip. Left out, the one the Sample Editor shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// Where the selection starts. Give neither `start` nor `end` to clear
    /// the selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    /// Where the selection ends, after `start`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    /// Whether `start` and `end` are ticks of a warped clip's contents
    /// instead of seconds in the file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ticks: bool,
}

/// What `clip_view.sample_view` changes about the part of the file shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "How to change the Sample Editor's view: `selection` zooms to the time \
                   selection (Z), `back` goes to the view before the last zoom (X), `whole` \
                   fits the whole file across the editor (W), `height` switches fitting the \
                   waveform to the editor's height (H), `span` shows the span `start` to `end`, \
                   `scroll` shows it without remembering the view before, and `page` does the \
                   same without pausing Follow."
)]
pub enum SampleViewChange {
    /// Zoom to the time selection.
    Selection,
    /// Go back to the view before the last zoom.
    Back,
    /// Fit the whole file across the editor.
    Whole,
    /// Switch fitting the waveform to the editor's height.
    Height,
    /// Show a span.
    Span,
    /// Show a span without remembering the view before, as scrolling does.
    Scroll,
    /// Show a span without remembering the view before or pausing Follow, as
    /// Follow turning the page does.
    Page,
}

/// Arguments of `clip_view.sample_view`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SampleViewArgs {
    /// The audio clip. Left out, the one the Sample Editor shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// What to change.
    pub show: Option<SampleViewChange>,
    /// With `span`, where the view starts: seconds in the file, or ticks of
    /// a warped clip's contents with `ticks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    /// With `span`, where the view ends, after `start`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    /// Whether `start` and `end` are ticks of a warped clip's contents
    /// instead of seconds in the file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ticks: bool,
}

/// A point in the file, as the Sample Editor reports it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SamplePoint {
    /// Seconds into the file.
    pub time: f64,
    /// The beat of a warped clip's contents it plays on, in ticks from the
    /// clip's 1.1.1; `null` for a clip that isn't warped.
    pub beat: Option<Ticks>,
}

/// A cue the Sample Editor shows (the video spec, R12): a marker embedded in
/// a video clip's movie.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Cue {
    /// Where it is in the movie, in seconds.
    pub time: f64,
    /// The name the movie gives it.
    pub name: String,
}

/// What the Sample Editor shows and has selected, for the clip it shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SampleEditorState {
    /// The audio clip the editor shows; `null` when it shows none.
    pub clip: Option<ClipId>,
    /// Whether the clip is warped, so the editor runs along its beats
    /// rather than along time in the file.
    pub warped: bool,
    /// The selected warp markers, by their place in the clip's warp map.
    pub markers: Vec<u32>,
    /// Whether the clip's Start marker is selected.
    pub start_marker: bool,
    /// The selected transients, by their times in the file in seconds.
    pub transients: Vec<f64>,
    /// The insert marker, if there is one.
    pub insert_marker: Option<SamplePoint>,
    /// The time selection's start and end, if there is one.
    pub time_selection: Option<(SamplePoint, SamplePoint)>,
    /// The span shown, in seconds in the file, or in ticks of the clip's
    /// contents when it is warped (`warped`); `null` shows the whole file.
    pub view: Option<(f64, f64)>,
    /// Whether the waveform is fitted to the editor's height.
    pub fit_height: bool,
    /// The markers embedded in a video clip's movie, shown as cues that
    /// can't be moved (the video spec, R12); none for any other clip.
    #[serde(default)]
    pub cues: Vec<Cue>,
}

/// Adds the Sample Editor's view commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs() {
        registry.register(spec)?;
    }
    Ok(())
}

fn view(id: &'static str, label: &'static str, description: &'static str) -> CommandSpec {
    CommandSpec::new(id, label)
        .description(description)
        .permission(Permission::Read)
        .contexts([ClipView, SampleEditor])
        .result::<SampleEditorState>()
}

fn specs() -> Vec<CommandSpec> {
    vec![
        view(
            STATE,
            "Sample Editor State",
            "Reports what the Clip View's Sample Editor shows for the audio clip it shows: \
             whether the clip is warped, the selected warp markers (by their place in the \
             clip's warp map), whether the Start marker is selected, the selected transients \
             (by their times in the file), the insert marker, the time selection, the span \
             shown, and a video clip's `cues`. Changes nothing.",
        )
        .args::<bl_commands::NoArgs>(),
        view(
            SELECT_WARP_MARKERS,
            "Select Warp Markers",
            "Selects warp markers of an audio clip in the Sample Editor, as clicking one does: \
             `replace` selects only these, `toggle` adds or removes each (Cmd-click, Ctrl-click \
             on Windows). A marker is named by its place in the clip's warp map \
             (`data.audio.warp_markers`), the first being 0. With `start_marker` the clip's \
             Start marker is selected too. The marker edits (`clip.move_warp_markers`, \
             `clip.delete_warp_markers`) take their markers by place and act on these when the \
             app runs them from a key. Selecting is never an undo step and changes nothing in \
             the set.",
        )
        .args::<SelectWarpMarkersArgs>(),
        view(
            SELECT_TRANSIENTS,
            "Select Transients",
            "Selects transients of an audio clip in the Sample Editor, as clicking a \
             transient's pseudo-warp marker does: `replace` selects only these, `toggle` adds \
             or removes each. A transient is named by its time in the file in seconds, as \
             `clip.transients` lists it. Delete Transient acts on the selected transients. \
             Selecting is never an undo step and changes nothing in the set.",
        )
        .args::<SelectTransientsArgs>(),
        view(
            SET_INSERT_MARKER,
            "Set Insert Marker",
            "Puts the editor's insert marker, the point the editor's commands act at, at a \
             `time` in the file in seconds or at a `beat` of a warped clip's contents in ticks \
             from its 1.1.1; with neither it is taken away. Clicking the waveform runs this, on \
             the nearest grid line while snapping is on. In the MIDI Note Editor the marker is a \
             `beat` of the clip's contents in ticks from their beginning, and it stays inside \
             the clip's span. Never an undo step, and changes nothing in the set.",
        )
        .contexts([ClipView, SampleEditor, NoteEditor, NoteSelection])
        .args::<SetInsertMarkerArgs>(),
        view(
            SELECT_TIME,
            "Select Time",
            "Makes a time selection in the editor from `start` to `end`, in seconds in the \
             file or, with `ticks`, in ticks of a warped clip's contents; with neither the \
             selection is cleared. Dragging across the waveform runs this. In the MIDI Note \
             Editor `start` and `end` are ticks of the clip's contents, and the selection is \
             drawn across the note area and the lanes. Never an undo step, and changes nothing \
             in the set.",
        )
        .contexts([ClipView, SampleEditor, NoteEditor, NoteSelection])
        .args::<SelectTimeArgs>(),
        view(
            SAMPLE_VIEW,
            "Zoom Sample Editor",
            "Changes the part of the file the Sample Editor shows: `selection` zooms to the time \
             selection (Z, remembering the view before), `back` goes back to the view before \
             the last zoom (X, a step at a time), `whole` fits the whole file across the editor \
             (W), `height` switches fitting the waveform to the editor's height (H), and \
             `span` shows `start` to `end`, in seconds in the file or, with `ticks`, in ticks \
             of a warped clip's contents, and `scroll` does the same without remembering the \
             view before, as dragging the ruler or the wheel does (scrolling pauses Follow \
             until playback starts or stops); `page` is Follow turning the page, which \
             doesn't pause it. Changes only the view.",
        )
        .binding(Binding::new("S-SAMPLE-007").arg("show", "selection"))
        .binding(Binding::new("S-SAMPLE-008").arg("show", "back"))
        .binding(Binding::new("S-SAMPLE-013").arg("show", "whole"))
        .binding(Binding::new("S-SAMPLE-014").arg("show", "height"))
        .args::<SampleViewArgs>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        for id in IDS {
            assert!(registry.contains(id), "{id}");
            // Selecting and zooming change only the view.
            assert_eq!(registry.get(id).unwrap().permission, Some(Permission::Read));
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
    }
}
