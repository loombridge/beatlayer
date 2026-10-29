//! Sync: what `sync.state` reports of MIDI clock (`docs/specs/midi-clock.md`,
//! R12, R25). WP-7.11 adds the sending half; following a clock adds its own
//! fields to the same result.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `sync.state`: the state of MIDI clock.
pub const STATE: &str = "sync.state";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[STATE];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// What `sync.state` returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SyncState {
    /// How many output ports have Sync on, so MIDI clock goes to them while
    /// the song plays.
    pub output_ports: u32,
    /// Whether MIDI clock is going out now: the song plays, past any count-in,
    /// and some output has Sync on.
    pub sending: bool,
    /// Whether the lower sync lamp is lit: a quarter note's clock left for a
    /// port in the last 100 ms.
    pub output_lamp: bool,
    /// Timing Clocks handed to the MIDI output thread since the audio engine
    /// started, one for each port.
    pub clocks_sent: u64,
    /// Timing Clocks stamped for a moment already past when the engine wrote
    /// them.
    pub clocks_late: u64,
    /// Sync messages dropped because the queue to the MIDI output thread was
    /// full.
    pub clocks_dropped: u64,
}

/// Adds the sync commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(STATE, "Sync Status")
            .description(
                "Reports the state of MIDI clock: how many output ports have Sync on, whether \
                 clock is going out now (the song plays, past any count-in), whether the lower \
                 sync lamp beside the Control Bar's tempo is lit, and how many Timing Clocks \
                 the audio engine has sent, stamped late and dropped since it started. Changes \
                 nothing. Set an output's Sync switch and its options with `midi.set_port`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<SyncState>(),
    )
}
