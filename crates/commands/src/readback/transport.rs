//! The transport.

use super::{Readback, read};

/// The readback of this area's commands, one line each, so a command's entry is easy to find.
#[rustfmt::skip]
pub(super) const ROWS: &[(&str, Readback)] = &[
    ("link.set_enabled", read("link.state", "enabled")),
    ("link.set_start_stop_sync", read("link.state", "start_stop_sync")),
    ("transport.set_external_sync", read("transport.state", "external_sync")),
    ("transport.set_loop", read("transport.state", "loop_start")),
    ("transport.set_loop_enabled", read("transport.state", "loop_on")),
    ("transport.set_metronome_count_in", read("transport.state", "metronome.count_in")),
    ("transport.set_metronome_enabled", read("transport.state", "metronome.enabled")),
    ("transport.set_metronome_only_while_recording", read("transport.state", "metronome.only_while_recording")),
    ("transport.set_metronome_rhythm", read("transport.state", "metronome.rhythm")),
    ("transport.set_metronome_sound", read("transport.state", "metronome.sound")),
    ("transport.set_midi_overdub", read("transport.state", "midi_overdub")),
    ("transport.set_punch_in", read("transport.state", "punch_in")),
    ("transport.set_punch_out", read("transport.state", "punch_out")),
    ("transport.set_record_quantization", read("transport.state", "record_quantization")),
    ("transport.set_recording", read("transport.state", "recording")),
    ("transport.tap_tempo", read("transport.state", "tap_tempo.tempo")),
];
