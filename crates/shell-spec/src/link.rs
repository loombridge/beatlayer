//! Link: tempo, phase, and start and stop shared with other apps on the
//! network (`docs/specs/link.md`, R1, R8, R15).

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `link.state`: what Link shares with the session.
pub const STATE: &str = "link.state";
/// `link.set_enabled`: switches Link.
pub const SET_ENABLED: &str = "link.set_enabled";
/// `link.set_start_stop_sync`: switches Start Stop Sync.
pub const SET_START_STOP_SYNC: &str = "link.set_start_stop_sync";

/// `link.set_show_toggle`: shows or hides the Control Bar's Link toggle.
pub const SET_SHOW_TOGGLE: &str = "link.set_show_toggle";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[STATE, SET_ENABLED, SET_START_STOP_SYNC, SET_SHOW_TOGGLE];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `link.set_enabled`, `link.set_start_stop_sync` and
/// `link.set_show_toggle`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    /// Whether it's on; left out, it switches to the other state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// What `link.set_enabled`, `link.set_start_stop_sync` and
/// `link.set_show_toggle` return.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Switched {
    /// Whether it's on now.
    pub on: bool,
}

/// What `link.state` returns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LinkState {
    /// Whether Link is on: the app is in a Link session on its networks.
    pub enabled: bool,
    /// How many other peers are in the session; 0 while Link is off.
    pub peers: u64,
    /// The session's tempo, in quarter notes per minute; null while Link has
    /// never been on.
    pub tempo: Option<f64>,
    /// The quantum, in quarter notes: the global launch quantisation's
    /// length at the play position, one quarter note with None.
    pub quantum: f64,
    /// How far the session is through the quantum now, in quarter notes;
    /// null while Link has never been on.
    pub phase: Option<f64>,
    /// Whether the session's start and stop state is playing.
    pub session_playing: bool,
    /// Whether starts and stops are shared with the peers that share them.
    pub start_stop_sync: bool,
    /// Whether the Control Bar shows the Link toggle and its menu.
    pub show_toggle: bool,
    /// Whether a start waits for the session's phase.
    pub waiting_for_link: bool,
    /// How many seconds until a waiting start begins, once it has been
    /// worked out; null otherwise.
    pub starts_in: Option<f64>,
}

/// Adds the Link commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(STATE, "Link Status")
            .description(
                "Reports Link: whether it is on, how many other peers are in the session, the \
                 session's tempo, the quantum (the global launch quantisation's length at the \
                 play position, in quarter notes) and how far the session is through it, \
                 whether the session plays, whether Start Stop Sync is on, whether the Control \
                 Bar shows the Link toggle, and whether a start waits for the session's phase and how many seconds until it begins. Changes \
                 nothing. Switch Link with `link.set_enabled` and Start Stop Sync with \
                 `link.set_start_stop_sync`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<LinkState>(),
    )?;
    registry.register(
        CommandSpec::new(SET_ENABLED, "Link")
            .description(
                "Switches Link, as the Control Bar's Link toggle does: on, the app joins the \
                 Link session on its networks or starts one, and shares tempo, beat and phase \
                 with the other apps in it. It talks on the network, so it is external. With \
                 no session to join, switching it on changes neither the tempo nor the song \
                 position; where a session exists, the song takes its tempo and phase. While \
                 it is on, a tempo change from a peer becomes the set's tempo and overrides \
                 tempo automation, the count-in doesn't play, and a start waits for the \
                 session's phase. `on` left out switches to the other state. It is a \
                 preference kept between starts, not part of the set. `link.state` reports \
                 it.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<SetArgs>()
            .result::<Switched>(),
    )?;
    registry.register(
        CommandSpec::new(SET_START_STOP_SYNC, "Link Start Stop Sync")
            .description(
                "Switches Start Stop Sync: with it on, the app's starts and stops go to the \
                 Link session and a peer's reach the app, with the peers that have it on too. \
                 Switching it on while the session plays doesn't start the song. `on` left \
                 out switches to the other state. It is a preference kept between starts, \
                 not part of the set. `link.state` reports it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SetArgs>()
            .result::<Switched>(),
    )?;
    registry.register(
        CommandSpec::new(SET_SHOW_TOGGLE, "Show Link Toggle")
            .description(
                "Shows or hides the Control Bar's Link toggle and its menu, as Settings' Link \
                 page's Show Link Toggle does. Hiding it leaves Link as it is. `on` is whether \
                 it shows; left out, it switches to the other state. It is a preference kept \
                 between starts, not part of the set. `link.state` reports it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SetArgs>()
            .result::<Switched>(),
    )
}
