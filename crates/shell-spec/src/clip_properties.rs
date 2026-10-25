//! The Clip View's Main Properties panel commands (WP-6.55): which panel the
//! left group shows, scrolling the editor to a marker, and the Set buttons.
//! The panel's other controls run the document's own `clip.*` commands,
//! which agents run too. The behaviour is in the clip properties spec, R1 to
//! R12.
//!
//! The Set buttons read where the playing clip has reached, which only the
//! shell knows, so the shell runs these. [`register`] adds their specs, which
//! are pure data, so `cargo xtask` can check them and list them in the
//! command reference; `bl-shell` runs them by ID.

use bl_commands::Context::{ClipView, Global};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::ClipId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `clip_view.show_properties`: choose the panel the left group shows.
pub const SHOW_PROPERTIES: &str = "clip_view.show_properties";

/// `clip_view.show_marker`: scroll the editor to a marker.
pub const SHOW_MARKER: &str = "clip_view.show_marker";

/// `clip.set_marker_at_playhead`: the Set buttons.
pub const SET_MARKER_AT_PLAYHEAD: &str = "clip.set_marker_at_playhead";

/// `focus.clip_panels`: Navigate ▸ Clip Panels.
pub const FOCUS_PANELS: &str = "focus.clip_panels";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    SHOW_PROPERTIES,
    SHOW_MARKER,
    SET_MARKER_AT_PLAYHEAD,
    FOCUS_PANELS,
];

/// Whether `id` is one of the Main Properties panel's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// The panels of the Clip View's left group (Spec: clip-properties R1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Properties {
    /// Main Properties.
    #[default]
    Main,
    /// Launch Properties, a Session clip's.
    Launch,
}

/// The four markers the panel's Show and Set buttons name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Marker {
    /// The Start marker.
    Start,
    /// The End marker.
    End,
    /// The loop brace's start: the Position field.
    LoopPosition,
    /// The loop brace's end: the Length field's marker.
    LoopLength,
}

impl Marker {
    /// The marker's name on the panel's buttons.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::End => "End",
            Self::LoopPosition => "Position",
            Self::LoopLength => "Length",
        }
    }
}

/// Arguments of `clip_view.show_properties`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowPropertiesArgs {
    /// The panel to show.
    pub panel: Properties,
}

/// What `clip_view.show_properties` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PropertiesShown {
    /// The panel the left group shows now: Main Properties for an Arrangement
    /// clip, whatever was asked.
    pub panel: Properties,
}

/// Arguments of `clip_view.show_marker` and `clip.set_marker_at_playhead`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkerArgs {
    /// The clip. Left out, the one the Clip View shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// The marker.
    pub marker: Marker,
}

/// What `clip_view.show_marker` and `clip.set_marker_at_playhead` return.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerAt {
    /// The clip.
    pub clip: ClipId,
    /// The marker.
    pub marker: Marker,
    /// Where the marker is now: ticks of the clip's contents for a MIDI or
    /// warped audio clip, seconds into the file for an unwarped one.
    pub position: f64,
}

/// Adds the Main Properties panel's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(SHOW_PROPERTIES, "Show Clip View Properties Panel")
            .description(
                "Shows Main Properties or Launch Properties in the Clip View's left group, as \
                 the panel selector row's buttons do. Launch Properties is a Session clip's: an \
                 Arrangement clip always shows Main Properties. View state: Main Properties \
                 when the app starts, no undo step, not saved with the set.",
            )
            .permission(Permission::Read)
            .context(ClipView)
            .args::<ShowPropertiesArgs>()
            .result::<PropertiesShown>(),
    )?;
    registry.register(
        CommandSpec::new(SHOW_MARKER, "Show Clip Marker")
            .description(
                "Scrolls the Clip View's editor so that a marker of the clip is in view, as \
                 the Show buttons in the Main Properties panel do: `start`, `end`, \
                 `loop_position` (the loop brace's start) or `loop_length` (its end). The \
                 Sample Editor of an audio clip scrolls; a MIDI clip's note editor shows its \
                 whole length already and stays as it is. The result names where the marker \
                 is. View state: no undo step, nothing changes in the set.",
            )
            .permission(Permission::Read)
            .context(ClipView)
            .args::<MarkerArgs>()
            .result::<MarkerAt>(),
    )?;
    registry.register(
        CommandSpec::new(SET_MARKER_AT_PLAYHEAD, "Set Clip Marker at Playhead")
            .description(
                "Moves a marker of a playing clip to where the clip has reached, as the Set \
                 buttons in the Main Properties panel do: `start`, `end`, `loop_position` or \
                 `loop_length`, on the nearest step of the global launch quantisation at or \
                 before the playhead (exactly where it is with quantisation None). Setting the \
                 loop's position or length switches the loop on; with the loop off, \
                 `loop_length` moves the whole brace so it ends there, keeping its length. \
                 Refused while the clip isn't playing or a marker would cross another. One \
                 undo step. Read the result back in the query tool's `clip` `data`.",
            )
            .permission(Permission::Edit)
            .context(ClipView)
            .args::<MarkerArgs>()
            .result::<MarkerAt>(),
    )?;
    registry.register(
        CommandSpec::new(FOCUS_PANELS, "Clip Panels")
            .description(
                "Moves the keyboard focus to the Clip View's panels, onto the first control of \
                 the panel the left group shows, showing the Clip View first if it is hidden: \
                 Navigate \u{25b8} Clip Panels. Tab then moves through the panel's controls in \
                 order. Changes nothing in the set and isn't an undo step.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-FOCUS-010")
            .args::<NoArgs>()
            .result::<()>(),
    )?;
    Ok(())
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
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
        let class = |id: &str| registry.get(id).unwrap().permission;
        assert_eq!(class(SHOW_PROPERTIES), Some(Permission::Read));
        assert_eq!(class(SHOW_MARKER), Some(Permission::Read));
        assert_eq!(class(SET_MARKER_AT_PLAYHEAD), Some(Permission::Edit));
        assert_eq!(class(FOCUS_PANELS), Some(Permission::Read));
    }
}
