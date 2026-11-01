//! Tempo Follower: the song's tempo following incoming audio
//! (`docs/specs/tempo-follower.md`, R1 to R5, R19).

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::Channel;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `tempo_follower.state`: whether Follow is on and what the tracker gives.
pub const STATE: &str = "tempo_follower.state";
/// `tempo_follower.set_enabled`: switches Follow.
pub const SET_ENABLED: &str = "tempo_follower.set_enabled";
/// `settings.set_show_tempo_follower_toggle`: Show Tempo Follower Toggle.
pub const SET_SHOW_TOGGLE: &str = "settings.set_show_tempo_follower_toggle";
/// `settings.set_tempo_follower_input`: Tempo & MIDI's Input Channel.
pub const SET_INPUT: &str = "settings.set_tempo_follower_input";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[STATE, SET_ENABLED, SET_SHOW_TOGGLE, SET_INPUT];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `tempo_follower.set_enabled` and
/// `settings.set_show_tempo_follower_toggle`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowArgs {
    /// Whether it's on; left out, it switches to the other state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// What `tempo_follower.set_enabled` and
/// `settings.set_show_tempo_follower_toggle` return.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FollowSwitched {
    /// Whether it's on now.
    pub on: bool,
}

/// Arguments of `settings.set_tempo_follower_input`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetInputArgs {
    /// The audio input to listen to: a single channel or a stereo pair,
    /// counted from 1.
    pub channel: Channel,
}

/// What `settings.set_tempo_follower_input` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputSet {
    /// The input channel now, as the Input Channel chooser shows it.
    pub label: String,
    /// The input channel now.
    pub channel: Channel,
}

/// What `tempo_follower.state` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TempoFollowerState {
    /// Whether Follow is on: the tracker listens and the song's tempo follows
    /// it.
    pub on: bool,
    /// Whether the Control Bar shows the Follow toggle (Show Tempo Follower
    /// Toggle).
    pub show_toggle: bool,
    /// The audio input it listens to (Input Channel).
    pub input: Channel,
    /// The input channel as the Input Channel chooser shows it.
    pub input_label: String,
    /// Whether Follow can run: the audio device's input can be reached.
    pub can_run: bool,
    /// Why it can't, when it can't.
    pub unavailable: Option<String>,
    /// The tempo the song follows now, in BPM; null while Follow is off or
    /// the tracker hasn't been confident yet.
    pub tempo: Option<f64>,
    /// The tracker's confidence, from 0 to 1; 0 while Follow is off.
    pub confidence: f64,
    /// The beat's phase the tracker hears, from 0 to 1; 0 while Follow is
    /// off.
    pub phase: f64,
    /// How many blocks of input the copy dropped because the tracker fell
    /// behind, since Follow went on.
    pub blocks_dropped: u64,
}

/// Adds the Tempo Follower commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(STATE, "Tempo Follower Status")
            .description(
                "Reports Tempo Follower: whether Follow is on, whether the Control Bar shows \
                 its toggle, the input channel it listens to, whether it can run and, if not, \
                 why, the tempo the song follows, the tracker's confidence and beat phase, and \
                 how many blocks of input were dropped. Changes nothing. Switch it with \
                 `tempo_follower.set_enabled`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<TempoFollowerState>(),
    )?;
    registry.register(
        CommandSpec::new(SET_ENABLED, "Tempo Follower")
            .description(
                "Switches Follow, as the Control Bar's toggle does: on, Beatlayer listens to \
                 the audio input channel and the song's tempo follows what it hears, once it is \
                 sure of it. The followed tempo becomes the set's tempo, with no undo step of \
                 its own, and takes a tempo envelope's place as a hand move does. EXT is \
                 switched off and disabled while it's on, MIDI clock out goes on, Link takes \
                 the tempo, and Tap Tempo and Phase Nudge are refused. Refused, with the \
                 reason, while the input can't be reached, and `on` left out switches to the \
                 other state. Follow starts off and isn't kept: it isn't part of the set. \
                 `tempo_follower.state` reports it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<FollowArgs>()
            .result::<FollowSwitched>(),
    )?;
    registry.register(
        CommandSpec::new(SET_SHOW_TOGGLE, "Show Tempo Follower Toggle")
            .description(
                "Sets Show Tempo Follower Toggle, on Settings ▸ Tempo & MIDI, to `on`, or to \
                 the other state if it's left out. While it's on, the Control Bar shows the \
                 Follow toggle in its tempo group; hiding it switches Follow off. It starts \
                 off, is kept for the next start, and isn't part of the set or an undo step. \
                 `tempo_follower.state` reports it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<FollowArgs>()
            .result::<FollowSwitched>(),
    )?;
    registry.register(
        CommandSpec::new(SET_INPUT, "Tempo Follower Input Channel")
            .description(
                "Sets Input Channel, on Settings ▸ Tempo & MIDI: the audio input Tempo \
                 Follower listens to, a single channel (`{ \"type\": \"mono\", \"channel\": 1 \
                 }`) or a stereo pair (`{ \"type\": \"pair\", \"channel\": 1 }` is 1/2, \
                 summed to mono), counted from 1. It starts as channel 1, is kept for the next \
                 start, and isn't part of the set or an undo step. A channel the audio device \
                 doesn't have is kept, and Follow is off while it can't be reached. \
                 `tempo_follower.state` reports it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SetInputArgs>()
            .result::<InputSet>(),
    )?;
    Ok(())
}
