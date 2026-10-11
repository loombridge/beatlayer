//! The transport as the app shell shows it (WP-1.18): the query that reads
//! what the Control Bar's transport section shows, and the IDs of the
//! transport's commands the menus and the Control Bar run.
//!
//! The transport's own commands (play, stop, locate, and the rest) are
//! `bl-engine`'s (`bl_engine::transport`), and the loop's are the
//! document's (`bl_model::registry`); `bl-shell` registers and runs them
//! all. This crate holds only their IDs, as data, so the menus can name
//! them without linking the engine. The rules are in
//! `docs/specs/control-bar-transport.md`.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `transport.state`: what the transport and the Control Bar's transport
/// section show.
pub const STATE: &str = "transport.state";

/// `transport.play` (`bl-engine`): the Control Bar's Play button.
pub const PLAY: &str = "transport.play";
/// `transport.stop` (`bl-engine`): the Control Bar's Stop button.
pub const STOP: &str = "transport.stop";
/// `transport.play_stop` (`bl-engine`): the space bar, and the Playback
/// menu's Play item, which reads Stop while the song plays.
pub const PLAY_STOP: &str = "transport.play_stop";
/// `transport.locate` (`bl-engine`): the Arrangement Position fields.
pub const LOCATE: &str = "transport.locate";
/// `transport.return_to_start` (`bl-engine`): the Playback menu's Return
/// Play Position to 1.1.1.
pub const RETURN_TO_START: &str = "transport.return_to_start";
/// `transport.set_loop` (`bl-model`): the Loop Start and Loop Length
/// fields.
pub const SET_LOOP: &str = "transport.set_loop";
/// `transport.set_loop_enabled` (`bl-model`): the Arrangement Loop switch.
pub const SET_LOOP_ENABLED: &str = "transport.set_loop_enabled";
/// `song.set_tempo` (`bl-model`): the Tempo field.
pub const SET_TEMPO: &str = "song.set_tempo";
/// `song.set_time_signatures` (`bl-model`): the time-signature fields.
pub const SET_TIME_SIGNATURES: &str = "song.set_time_signatures";

/// Every command this module registers.
pub const IDS: &[&str] = &[STATE];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// What `transport.state` returns: the transport and the Control Bar's
/// transport section, as they are now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TransportState {
    /// Whether the audio engine runs on a device, so the transport can
    /// play. While it doesn't, the transport's commands are refused.
    pub engine_running: bool,
    /// Whether the song is playing.
    pub playing: bool,
    /// The play position (the playhead), in ticks from 1.1.1 (705,600 to
    /// the quarter note): where the song plays now, or where playback
    /// stopped, which is where `transport.continue` resumes.
    pub position: Ticks,
    /// The insert marker, where `transport.play` starts, in ticks.
    pub insert_marker: Ticks,
    /// What the Control Bar's Arrangement Position fields show, as
    /// bar.beat.sixteenth by the song's time signatures: the play position
    /// while the song plays, and the insert marker while it's stopped.
    pub arrangement_position: String,
    /// Where the Arrangement loop brace starts, in ticks.
    pub loop_start: Ticks,
    /// How long the loop brace is, in ticks.
    pub loop_length: Ticks,
    /// Whether the Arrangement Loop switch is on.
    pub loop_on: bool,
    /// The loop brace's start as the Loop Start fields show it, as
    /// bar.beat.sixteenth.
    pub loop_start_position: String,
    /// The brace's length as the Loop Length fields show it: bars, beats,
    /// and sixteenths counted from 0, such as `4.0.0` for four bars.
    pub loop_length_bars: String,
    /// The tempo where the Arrangement Position fields are, in quarter
    /// notes per minute, as the Tempo field shows it.
    pub tempo: f64,
    /// Whether the tempo is one constant tempo, so the Tempo field can set
    /// it. With tempo changes along the song, change it with
    /// `song.set_tempo`'s full map instead.
    pub tempo_editable: bool,
    /// The time signature there, such as `4/4`, as the time-signature fields
    /// show it.
    pub time_signature: String,
    /// Whether the song has one time signature throughout, so the
    /// time-signature fields can set it.
    pub time_signature_editable: bool,
}

/// Adds the transport query to a registry.
///
/// # Errors
///
/// [`RegisterError`] if its ID is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(STATE, "Transport Status")
            .description(
                "Reports what the Control Bar's transport section shows: whether the song \
                 plays, the play position and the insert marker (in ticks from 1.1.1, and the \
                 Arrangement Position fields' bar.beat.sixteenth), the Arrangement loop's start, \
                 length, and switch, the tempo, and the time signature, and whether the audio \
                 engine runs so the transport can play. Changes nothing. Use it to check where \
                 the song is before and after `transport.play`, `transport.stop`, \
                 `transport.locate`, or `transport.set_loop`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<TransportState>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_registers_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
        assert!(handles(STATE));
        assert!(!handles(PLAY), "the engine's");
    }
}
