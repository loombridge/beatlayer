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
use bl_model::{ChainId, DeviceId, TrackId};
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
/// `mixer_view.return_tracks_scroll`: scroll the return tracks with the
/// tracks (IMP-01).
pub const RETURN_TRACKS_SCROLL: &str = "mixer_view.return_tracks_scroll";
/// `mixer_view.track_options`: show or hide the tracks' Track Options
/// sections.
pub const TRACK_OPTIONS: &str = "mixer_view.track_options";
/// `mixer_view.performance_impact`: show or hide the Performance Impact
/// section.
pub const PERFORMANCE_IMPACT: &str = "mixer_view.performance_impact";
/// `mixer_view.scroll`: scroll a mixer's columns sideways.
pub const SCROLL: &str = "mixer_view.scroll";
/// `mixer_view.focus`: move the keyboard focus to the Arrangement's mixer.
pub const FOCUS: &str = "mixer_view.focus";
/// `mixer_view.fold_chains`: fold a rack's chains out as strips in the
/// mixers, or fold them back in (racks spec, R27).
pub const FOLD_CHAINS: &str = "mixer_view.fold_chains";

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
    RETURN_TRACKS_SCROLL,
    TRACK_OPTIONS,
    PERFORMANCE_IMPACT,
    SCROLL,
    FOCUS,
    FOLD_CHAINS,
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

/// One track's gain reduction, as `mixer_view.state` reports it (IMP-25,
/// the mixer view spec R40).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrackGainReduction {
    /// The track.
    pub track: TrackId,
    /// The most reduction any of the track's compressors or limiters
    /// reported for the last block, in dB, as a compressor's Gain Reduction
    /// reads it: zero when none of them reduced.
    pub reduction: f32,
}

/// What `mixer_view.state` and the other view commands return.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    /// Whether the Crossfader section shows: the tracks' A and B assign buttons
    /// and Main's crossfader. Hidden in a new set.
    pub crossfader: bool,
    /// Whether the Session View's Main column shows the scenes' tempo and
    /// time signature fields (View > Scene Tempo and Time Signature). Hidden
    /// in a new set.
    pub scene_tempo: bool,
    /// Whether the return tracks' columns show.
    pub return_tracks: bool,
    /// Whether the return tracks scroll with the tracks (IMP-01, the
    /// mixer view spec R21): they come after the drop area and only Main
    /// stays pinned. Off in a new install, so the returns stay pinned, as
    /// in Live.
    pub return_tracks_scroll: bool,
    /// Whether the tracks' Track Options sections show (track delay and
    /// Keep Monitoring Latency). Shown only while this shows; hidden in a
    /// new set.
    pub track_options: bool,
    /// Whether the Performance Impact section shows: under each strip, the
    /// six segments that light up from the left with the track's share of
    /// the set's processing time. Hidden in a new set.
    pub performance_impact: bool,
    /// Each track's Performance Impact, as the engine last published it:
    /// the track, how many of its six segments are lit, and its share of
    /// the set's processing time (the mixer spec, R31).
    #[serde(default)]
    pub performance_impact_values: Vec<TrackImpact>,
    /// Each track's gain reduction, in the order the tracks run (IMP-25,
    /// R40); empty when no engine has published meters.
    #[serde(default)]
    pub gain_reduction: Vec<TrackGainReduction>,
    /// The racks whose chains fold out in the mixers (racks spec, R27): the
    /// track each is on, the rack, and whether its chains are folded out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub racks: Vec<RackFold>,
    /// The chains folded out as strips, in the mixers' column order (racks
    /// spec, R27).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chains: Vec<ChainStrip>,
}

/// One track's Performance Impact: the six segments the mixer draws under
/// its strip, and the share of the set's processing time they show (the
/// mixer spec, R31).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrackImpact {
    /// The track.
    pub track: TrackId,
    /// How many of the six segments are lit, from 0 to 6.
    pub segments: u8,
    /// The track's share of the set's processing time, from 0 to 1.
    pub share: f32,
}

/// How many of the six Performance Impact segments a track's share of the
/// set's processing time lights, from the left: the share of the whole set
/// a sixth at a time, rounded up, so a track with any of it lights one
/// (the mixer spec, R31).
#[must_use]
pub fn impact_segments(share: f32) -> u8 {
    if !share.is_finite() || share <= 0.0 {
        return 0;
    }
    ((share * 6.0).ceil() as u8).min(6)
}

/// A rack whose chains fold out in the mixers as strips (racks spec, R27):
/// the track it is on, the rack itself, and whether its chains are folded
/// out now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RackFold {
    /// The track the rack is on.
    pub track: TrackId,
    /// The rack.
    pub rack: DeviceId,
    /// The rack's name.
    pub name: String,
    /// Whether its chains are folded out as strips now.
    pub folded: bool,
}

/// A chain folded out as a strip in the mixers (racks spec, R27): the
/// chain, the rack it belongs to, and the mixer controls the chain list
/// shows for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChainStrip {
    /// The track the rack is on.
    pub track: TrackId,
    /// The rack the chain belongs to.
    pub rack: DeviceId,
    /// The rack's name.
    pub rack_name: String,
    /// The chain.
    pub chain: ChainId,
    /// The chain's name.
    pub name: String,
    /// The chain's Volume as linear gain: 1 is 0 dB.
    pub volume: f64,
    /// The chain's Pan, from −1 to 1.
    pub pan: f64,
    /// Whether the chain has Volume and Pan: all but a MIDI effect rack's
    /// chains do.
    pub has_mixer: bool,
    /// Whether the chain's activator is on.
    pub enabled: bool,
    /// Whether the chain is soloed.
    pub solo: bool,
    /// The chain's colour, a swatch of the palette of 5 rows of 14, if it
    /// has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<u8>,
    /// The rack inside this chain, if it has one whose chains fold out too
    /// (racks spec, R27): a fold button on this strip shows its chains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested: Option<RackFold>,
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

/// Arguments of `mixer_view.return_tracks_scroll`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrollReturnsArgs {
    /// Scroll the return tracks with the tracks (true) or pin them (false);
    /// switches it if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
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

/// Arguments of `mixer_view.fold_chains`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FoldChainsArgs {
    /// The rack whose chains fold out.
    pub rack: DeviceId,
    /// Show them (true) or hide them (false); switches them if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<bool>,
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
             Sends, the volume strips, the return tracks, the Performance Impact) show. The \
             Performance Impact values are the engine's last, one entry per track: how many of \
             the six segments are lit and the track's share of the set's processing time. \
             Track values (volume, pan, the activator, Solo, Arm, routing, sends, the \
             crossfader assignment) are in the set; read them with the query tool: a track with \
             `detail: \"full\"` returns them as the set saves them. Changes nothing.",
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
            RETURN_TRACKS_SCROLL,
            "Return Tracks Scroll with Tracks",
            "Scrolls the return tracks with the other tracks, in the Session View's grid, in the \
             Arrangement View's lanes and in both mixers, as View > Mixer Controls > Return Tracks \
             Scroll with Tracks does: the returns come after the drop area and only Main stays \
             pinned at the right. Off in a new install, so the returns stay pinned, as in Live. It \
             is an application setting, kept between launches and not saved with the set. \
             Switches unless `on` says which. Changes only the view.",
        )
        .context(Global)
        .args::<ScrollReturnsArgs>(),
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
            PERFORMANCE_IMPACT,
            "Performance Impact",
            "Shows or hides the mixers' Performance Impact section, as View > Mixer Controls > \
             Performance Impact does: under each strip, the six segments that light up from the \
             left with the track's share of the set's processing time. Hidden in a new set. \
             Switches unless `shown` says which. Changes only the view; the shares themselves \
             are in `mixer_view.state`, as `performance_impact_values`.",
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
        view(
            FOLD_CHAINS,
            "Fold Rack Chains",
            "Folds a rack's chains out in the mixers as strips after the track's, as the fold \
             button in the track's title bar does, or folds them back in. `rack` is the rack: a \
             rack of more than one chain, on a track or inside another rack's chain. A track's \
             fold button is for the instrument or drum rack it holds; a chain's is for the rack \
             inside it. A rack of one chain has no chains to fold out. Switches unless `shown` \
             says which. Changes only the view, and isn't saved with the set; the chains it \
             shows read back in `mixer_view.state`.",
        )
        .context(Global)
        .args::<FoldChainsArgs>(),
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
