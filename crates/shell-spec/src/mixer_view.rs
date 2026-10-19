//! The Mixer view's view commands in the command registry (WP-1.9):
//! showing and hiding the mixer in the Session and Arrangement Views and
//! its sections, scrolling its columns, and moving the keyboard focus to it.
//!
//! These change only what the window shows, never the set, so every one is
//! in the read class. The mixer's values (volume, pan, the activator, Solo,
//! Arm) are `bl-model`'s `mixer.*` commands. [`register`] adds the specs,
//! which are pure data, so `cargo xtask` can check them and list them in the
//! command reference; `bl-shell` runs them by ID. The behaviour they follow
//! is in the mixer view spec.

use bl_commands::Context::{Arrangement, Global};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `mixer_view.state`: what the mixers show.
pub const STATE: &str = "mixer_view.state";
/// `mixer_view.toggle`: show or hide the mixer (View ▸ Mixer).
pub const TOGGLE: &str = "mixer_view.toggle";
/// `mixer_view.in_out`: show or hide the In/Out section.
pub const IN_OUT: &str = "mixer_view.in_out";
/// `mixer_view.sends`: show or hide the Sends section.
pub const SENDS: &str = "mixer_view.sends";
/// `mixer_view.volume`: show or hide the mixer strips (Volume).
pub const VOLUME: &str = "mixer_view.volume";
/// `mixer_view.crossfader`: show or hide the Crossfader section.
pub const CROSSFADER: &str = "mixer_view.crossfader";
/// `mixer_view.scene_tempo`: show or hide the Main column's scene tempo and
/// time signature fields.
pub const SCENE_TEMPO: &str = "mixer_view.scene_tempo";
/// `mixer_view.return_tracks`: show or hide the return tracks' columns.
pub const RETURN_TRACKS: &str = "mixer_view.return_tracks";
/// `mixer_view.track_options`: show or hide the tracks' Track Options
/// sections.
pub const TRACK_OPTIONS: &str = "mixer_view.track_options";
/// `mixer_view.scroll`: scroll a mixer's columns sideways.
pub const SCROLL: &str = "mixer_view.scroll";
/// `mixer_view.focus`: move the keyboard focus to the Arrangement's mixer.
pub const FOCUS: &str = "mixer_view.focus";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    STATE,
    TOGGLE,
    IN_OUT,
    SENDS,
    VOLUME,
    CROSSFADER,
    SCENE_TEMPO,
    RETURN_TRACKS,
    TRACK_OPTIONS,
    SCROLL,
    FOCUS,
];

/// Whether `id` is one of the Mixer view's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// A track column's width in the mixer at 100%, in points: the Session
/// View's default (layout.md, L-SES-001). The mixer scrolls by columns of
/// this width.
pub const COLUMN_WIDTH: f32 = 94.0;

/// Which view's mixer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(description = "Which view's mixer: the Session View's or the Arrangement View's.")]
pub enum MixerOf {
    /// The Session View's mixer, under the clip slots.
    Session,
    /// The Arrangement View's mixer, under the track lanes.
    Arrangement,
}

/// One view's mixer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewMixerState {
    /// Whether the mixer shows in that view.
    pub shown: bool,
    /// How far its track columns are scrolled sideways, in points at 100%.
    pub scroll: f64,
}

/// What `mixer_view.state` and the other view commands return.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MixerViewState {
    /// The Session View's mixer.
    pub session: ViewMixerState,
    /// The Arrangement View's mixer.
    pub arrangement: ViewMixerState,
    /// Whether the In/Out section shows, in both mixers.
    pub in_out: bool,
    /// Whether the Sends section shows.
    pub sends: bool,
    /// Whether the mixer strips (volume, pan, the meter, the activator,
    /// Solo, Arm) show.
    pub volume: bool,
    /// Whether the Crossfader section shows: the tracks' A/B assign buttons
    /// and Main's crossfader. Hidden in a new set.
    pub crossfader: bool,
    /// Whether the Session View's Main column shows the scenes' tempo and
    /// time signature fields (View > Scene Tempo and Time Signature). Hidden
    /// in a new set.
    pub scene_tempo: bool,
    /// Whether the return tracks' columns show.
    pub return_tracks: bool,
    /// Whether the tracks' Track Options sections show (track delay and
    /// Keep Monitoring Latency). Shown only while this shows; hidden in a
    /// new set.
    pub track_options: bool,
}

/// Arguments of `mixer_view.toggle`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToggleArgs {
    /// Which view's mixer; the view in front if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<MixerOf>,
    /// Show it (true) or hide it (false); switches it if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<bool>,
}

/// Arguments of the section commands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SectionArgs {
    /// Show it (true) or hide it (false); switches it if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<bool>,
}

/// Arguments of `mixer_view.scroll`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrollArgs {
    /// Which view's mixer; the view in front if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<MixerOf>,
    /// Points to scroll sideways at 100%: positive towards the last track.
    pub x: f64,
}

/// Adds the Mixer view's commands to a registry.
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

/// A view command: read class, returning the mixers' state.
fn view(id: &'static str, label: &'static str, description: &'static str) -> CommandSpec {
    CommandSpec::new(id, label)
        .description(description)
        .permission(Permission::Read)
        .result::<MixerViewState>()
}

fn specs() -> Vec<CommandSpec> {
    vec![
        view(
            STATE,
            "Mixer View State",
            "Reports what the mixers show: whether the Session View's and the Arrangement \
             View's mixers are shown and how far each is scrolled, and which sections (In/Out, \
             Sends, the volume strips, the return tracks) show. Track values (volume, pan, the \
             activator, Solo, Arm, routing, sends, the crossfader assignment) are in the set; \
             read them with the query tool: a track with `detail: \"full\"` returns them as \
             the set saves them. Changes nothing.",
        )
        .context(Global)
        .args::<NoArgs>(),
        view(
            TOGGLE,
            "Mixer",
            "Shows or hides the mixer, as View > Mixer and the Status Bar's mixer toggle do: in \
             the Session View under the clip slots, in the Arrangement View under the track \
             lanes, which it splits. Acts on the view in front unless `view` names one; switches \
             unless `shown` says which. Each view keeps its own: a new set shows the Session's \
             and hides the Arrangement's. Changes only the view, and isn't saved with the set.",
        )
        .context(Global)
        .binding("S-VIEW-014")
        .args::<ToggleArgs>(),
        view(
            IN_OUT,
            "Mixer In/Out",
            "Shows or hides the mixers' In/Out section (input and output routing and \
             monitoring), as View > Mixer Controls > In/Out does. Switches unless `shown` says \
             which. Changes only the view.",
        )
        .context(Global)
        .binding("S-VIEW-012")
        .args::<SectionArgs>(),
        view(
            SENDS,
            "Mixer Sends",
            "Shows or hides the mixers' Sends section (a send knob per return track), as View > \
             Mixer Controls > Sends does. Switches unless `shown` says which. Changes only the \
             view.",
        )
        .context(Global)
        .binding("S-VIEW-013")
        .args::<SectionArgs>(),
        view(
            VOLUME,
            "Mixer Volume",
            "Shows or hides the mixers' strips (the peak level, pan, the activator, Solo, Arm, \
             the volume fader, and the meter), as View > Mixer Controls > Volume does. Switches unless \
             `shown` says which. Changes only the view; the Arrangement's track headers keep \
             their controls.",
        )
        .context(Global)
        .args::<SectionArgs>(),
        view(
            CROSSFADER,
            "Mixer Crossfader",
            "Shows or hides the mixers' Crossfader section, as View > Mixer Controls > \
             Crossfader does: under the strips, a track's A and B assign buttons and, in \
             Main's column, the crossfader itself. Hidden in a new set. Switches unless \
             `shown` says which. Changes only the view; the assign and the position are in \
             the set (`mixer.set_crossfade_assign`, `mixer.set_crossfader`).",
        )
        .context(Global)
        .args::<SectionArgs>(),
        view(
            SCENE_TEMPO,
            "Scene Tempo and Time Signature",
            "Shows or hides the Session View's scene tempo and time signature fields, at the \
             left of the Main track's column, as View > Scene Tempo and Time Signature does. \
             Hidden in a new set. Switches unless `shown` says which. Changes only the view; a \
             scene's own tempo and signature are in the set (`scene.set_tempo`, \
             `scene.set_time_signature`).",
        )
        .context(Global)
        .args::<SectionArgs>(),
        view(
            RETURN_TRACKS,
            "Return Tracks",
            "Shows or hides the return tracks' columns in the mixers, as View > Mixer Controls > \
             Return Tracks does. Switches unless `shown` says which. Changes only the view; the \
             returns keep playing.",
        )
        .context(Global)
        .binding(Binding::new("S-TRACK-011"))
        .args::<SectionArgs>(),
        view(
            TRACK_OPTIONS,
            "Track Options",
            "Shows or hides the tracks' Track Options sections, as View > Mixer Controls > \
             Track Options does: under each strip, the track delay (with its ms/samples unit \
             switch) and Keep Monitoring Latency. Hidden in a new set; shown only while this \
             shows. Switches unless `shown` says which. Changes only the view; the delay and \
             the Keep Monitoring Latency switch are in the set (`mixer.set_track_delay`, \
             `mixer.set_keep_monitoring_latency`).",
        )
        .context(Global)
        .args::<SectionArgs>(),
        view(
            SCROLL,
            "Scroll Mixer",
            "Scrolls a mixer's track columns sideways by `x` points (positive towards the last \
             track), as the wheel over the mixer does. The return tracks and Main stay put. It \
             can't scroll before the first track or past the last. Acts on the view in front \
             unless `view` names one. Changes only the view.",
        )
        .context(Global)
        .args::<ScrollArgs>(),
        view(
            FOCUS,
            "Move Focus to Mixer",
            "Moves the keyboard focus to the Arrangement View's mixer, as Navigate > Mixer \
             (Option-Shift-M) does, showing it if it's hidden: to the selected track's column, \
             or the first track's. Left and Right then move the track selection. Changes only \
             the view.",
        )
        .context(Arrangement)
        .binding("S-ARR-035")
        .args::<NoArgs>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for id in IDS {
            let spec = registry.get(id).unwrap_or_else(|| panic!("{id}"));
            assert_eq!(spec.permission, Some(Permission::Read), "{id}");
            assert!(spec.args.is_some() && spec.result.is_some(), "{id}");
            assert!(!spec.description.is_empty(), "{id}");
        }
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
    }
}
