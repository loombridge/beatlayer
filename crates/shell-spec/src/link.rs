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

/// `link.set_audio`: switches Link Audio.
pub const SET_AUDIO: &str = "link.set_audio";
/// `link.set_audio_name`: sets the name other peers see this app by.
pub const SET_AUDIO_NAME: &str = "link.set_audio_name";
/// `link.set_audio_latency`: sets the input latency of incoming audio.
pub const SET_AUDIO_LATENCY: &str = "link.set_audio_latency";
/// `link.set_sync_to_incoming_audio`: switches Sync to Incoming Audio.
pub const SET_SYNC_TO_INCOMING_AUDIO: &str = "link.set_sync_to_incoming_audio";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[
    STATE,
    SET_ENABLED,
    SET_START_STOP_SYNC,
    SET_SHOW_TOGGLE,
    SET_AUDIO,
    SET_AUDIO_NAME,
    SET_AUDIO_LATENCY,
    SET_SYNC_TO_INCOMING_AUDIO,
];

/// The longest Latency, in milliseconds, and the one a new install has
/// (`docs/specs/link-audio.md`, R7).
pub const MAX_LATENCY_MS: f64 = 200.0;
/// See [`MAX_LATENCY_MS`].
pub const DEFAULT_LATENCY_MS: f64 = 20.0;

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `link.set_enabled`, `link.set_start_stop_sync`,
/// `link.set_show_toggle`, `link.set_audio` and
/// `link.set_sync_to_incoming_audio`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    /// Whether it's on; left out, it switches to the other state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// What `link.set_enabled`, `link.set_start_stop_sync`, `link.set_show_toggle`,
/// `link.set_audio` and `link.set_sync_to_incoming_audio` return.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Switched {
    /// Whether it's on now.
    pub on: bool,
}

/// Arguments of `link.set_audio_name`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetAudioNameArgs {
    /// The name other peers see this app by; empty for the computer's name.
    pub name: String,
}

/// What `link.set_audio_name` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioName {
    /// The name in effect now.
    pub name: String,
}

/// Arguments of `link.set_audio_latency`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetAudioLatencyArgs {
    /// The latency, in milliseconds, from 0 to 200; a value outside that
    /// range is brought into it.
    pub ms: f64,
}

/// What `link.set_audio_latency` returns.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AudioLatency {
    /// The latency in effect now, in milliseconds.
    pub ms: f64,
}

/// Whether audio is arriving from a Link Audio peer (`link.state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PeerState {
    /// A track takes one of the peer's channels and audio is arriving.
    Receiving,
    /// The peer is in the session with Link Audio on, and nothing arrives.
    Connected,
}

impl PeerState {
    /// The word the peers list shows.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Receiving => "Receiving",
            Self::Connected => "Connected",
        }
    }
}

/// A peer in the session that has Link Audio on (`link.state`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LinkAudioPeer {
    /// The peer's name.
    pub name: String,
    /// Whether audio is arriving from it.
    pub state: PeerState,
    /// The audio buffered from it, in milliseconds.
    pub buffered_ms: f64,
    /// The channels it offers, in its order.
    pub channels: Vec<String>,
}

/// Link Audio's part of `link.state`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LinkAudioState {
    /// Whether Link Audio is switched on (it streams only while Link is on
    /// too).
    pub enabled: bool,
    /// The name other peers see this app by.
    pub name: String,
    /// The input latency of incoming audio, in milliseconds.
    pub latency_ms: f64,
    /// Whether Beatlayer delays its own playback by the latency.
    pub sync_to_incoming_audio: bool,
    /// The peers with Link Audio on, as the Link page lists them.
    pub peers: Vec<LinkAudioPeer>,
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
    /// Link Audio: its switch, name, latency, Sync to Incoming Audio and the
    /// peers.
    pub audio: LinkAudioState,
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
                 Bar shows the Link toggle, and whether a start waits for the session's phase and how many seconds until it begins. Also \
                 Link Audio: whether it is on, the name other peers see this app by, the \
                 latency in milliseconds, whether Sync to Incoming Audio is on, and the peers \
                 with Link Audio on, each with its name, whether audio is arriving from it, \
                 the audio buffered from it in milliseconds, and its channels. Changes \
                 nothing. Switch Link with `link.set_enabled`, Start Stop Sync with \
                 `link.set_start_stop_sync` and Link Audio with `link.set_audio`.",
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
    )?;
    registry.register(
        CommandSpec::new(SET_AUDIO, "Link Audio")
            .description(
                "Switches Link Audio, as Settings' Link page and the Link menu do: on, with \
                 Link on, every track with an audio output is offered to the other apps in the \
                 session as a channel, and the channels they offer can be an audio track's \
                 input (`routing.set_input` with the `link_audio` input). It sends audio over \
                 the network, so it is external. It does nothing while Link is off, and keeps \
                 its state. `on` left out switches to the other state. It is a preference kept \
                 between starts, not part of the set. `link.state` reports it.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<SetArgs>()
            .result::<Switched>(),
    )?;
    registry.register(
        CommandSpec::new(SET_AUDIO_NAME, "Link Audio Name")
            .description(
                "Sets the name other apps in the Link session see this app by, as Settings' \
                 Link page's Name does. An empty name is the computer's name, which a new \
                 install has. It is a preference kept between starts, not part of the set. \
                 `link.state` reports it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SetAudioNameArgs>()
            .result::<AudioName>(),
    )?;
    registry.register(
        CommandSpec::new(SET_AUDIO_LATENCY, "Link Audio Latency")
            .description(
                "Sets the input latency of audio that arrives from the other apps in the Link \
                 session, in milliseconds from 0 to 200 (20 at first), as Settings' Link page's \
                 Latency does: incoming audio plays on the beat it was sent on, that much \
                 later, and more of it is buffered. It is a preference kept between starts, \
                 not part of the set. `link.state` reports it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SetAudioLatencyArgs>()
            .result::<AudioLatency>(),
    )?;
    registry.register(
        CommandSpec::new(SET_SYNC_TO_INCOMING_AUDIO, "Sync to Incoming Audio")
            .description(
                "Switches Sync to Incoming Audio, as Settings' Link page does: with it on, and \
                 Link Audio on, Beatlayer delays its own playback by the latency so that the \
                 audio it monitors from other apps in the session lines up with its own clips. \
                 Only one of the session's apps should have it on. `on` left out switches to \
                 the other state. It is a preference kept between starts, not part of the set. \
                 `link.state` reports it.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<SetArgs>()
            .result::<Switched>(),
    )
}
