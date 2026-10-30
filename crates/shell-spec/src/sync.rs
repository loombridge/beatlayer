//! Sync: what `sync.state` reports of MIDI clock (`docs/specs/midi-clock.md`,
//! R12, R25): the sending half (WP-7.11) and the receiving half (WP-7.12).

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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    /// How many input ports have Sync on, so their clock can be followed.
    #[serde(default)]
    pub input_ports: u32,
    /// Whether EXT is available: some input port has Sync on.
    #[serde(default)]
    pub external_sync_available: bool,
    /// Whether EXT is on.
    #[serde(default)]
    pub external_sync: bool,
    /// The input port that leads, while usable clocks arrive from it.
    #[serde(default)]
    pub leading_input: Option<String>,
    /// Whether usable clocks arrive: one came in the last 500 ms.
    #[serde(default)]
    pub receiving: bool,
    /// Whether the upper sync lamp is lit: a quarter note's clock arrived in
    /// the last 100 ms.
    #[serde(default)]
    pub input_lamp: bool,
    /// The smoothed tempo of the incoming clock in BPM, once there is one;
    /// what the Tempo field shows while EXT is on.
    #[serde(default)]
    pub tempo: Option<f64>,
    /// Timing Clocks that arrived from the leading input since the audio
    /// engine started.
    #[serde(default)]
    pub clocks_received: u64,
    /// Timing Clocks set aside as glitches: doubled or missing.
    #[serde(default)]
    pub clocks_set_aside: u64,
    /// Incoming sync messages dropped because the audio thread's list was
    /// full.
    #[serde(default)]
    pub sync_dropped: u64,
    /// The last whole time of an input set to MIDI Timecode, if one came
    /// (the MIDI Timecode spec, R12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timecode: Option<TimecodeReport>,
}

/// A timecode input's last whole time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimecodeReport {
    /// The time, as `hh:mm:ss:ff`.
    pub time: String,
    /// The rate it is counted at.
    pub rate: crate::midi_io::TimecodeRateChoice,
    /// Whether the timecode runs and the song follows it: whole times arrive
    /// in order and the time has reached the offset.
    pub running: bool,
    /// The song position the time gives, in ticks: the time less the offset,
    /// through the tempo map; 0 before the offset.
    pub position_ticks: i64,
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
                "Reports the state of MIDI clock. Sent: how many output ports have Sync on, \
                 whether clock is going out now (the song plays, past any count-in), whether \
                 the lower sync lamp beside the Control Bar's tempo is lit, and how many Timing \
                 Clocks the audio engine has sent, stamped late and dropped since it started. \
                 Received: how many input ports have Sync on, whether EXT is available and on, \
                 the input that leads, whether usable clocks arrive, whether the upper sync \
                 lamp is lit, the smoothed incoming tempo, and how many Timing Clocks arrived \
                 and were set aside as glitches. For an input set to MIDI Timecode: the last whole \
                 time with its rate, whether it runs, and the song position it gives. Changes \
                 nothing. Set a port's Sync switch and \
                 its options with `midi.set_port`, and EXT with `transport.set_external_sync`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<SyncState>(),
    )
}
