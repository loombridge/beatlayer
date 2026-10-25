//! The Options menu.

use super::Action::{Choice, Command as Cmd, Omitted, Pending, Shortcut as Key, Submenu};
use super::{Row, SEPARATOR};
use crate::browser::SET_PREVIEW as BROWSER_PREVIEW;
use crate::commands::{AUDIO_TOGGLE_ENGINE, VIEW_DRAW_MODE};
use crate::notes::SET_PREVIEW as NOTE_PREVIEW;

pub(super) const OPTIONS: &[Row] = &[
    ("M-OPT-001", "Accessibility", Submenu),
    ("M-OPT-001a", "Speak Menu Commands", Pending(None)),
    (
        "M-OPT-001b",
        "Speak Minimum and Maximum Sliders Values",
        Pending(None),
    ),
    ("M-OPT-001c", "Speak Time in Seconds", Pending(None)),
    ("M-OPT-001d", "Speak Parameter States", Pending(None)),
    (
        "M-OPT-001e",
        "Speak Extended MIDI Note Description",
        Pending(None),
    ),
    SEPARATOR,
    ("M-OPT-002", "Edit MIDI Map", Key(&["S-MAP-001"])),
    ("M-OPT-003", "Edit Key Map", Key(&["S-MAP-002"])),
    ("M-OPT-004", "Computer MIDI Keyboard", Key(&["S-MAP-003"])),
    ("M-OPT-005", "MIDI Note Editor Preview", Cmd(NOTE_PREVIEW)),
    ("M-OPT-006", "Browser File Preview", Cmd(BROWSER_PREVIEW)),
    SEPARATOR,
    ("M-OPT-007", "External Sync", Pending(None)),
    (
        "M-OPT-008",
        "Delay Compensation",
        Cmd("mixer.set_delay_compensation"),
    ),
    (
        "M-OPT-009",
        "Reduced Latency When Monitoring",
        Cmd("mixer.set_reduced_latency_monitoring"),
    ),
    SEPARATOR,
    ("M-OPT-010", "Time Ruler Format", Submenu),
    ("M-OPT-010a", "Time", Pending(None)),
    ("M-OPT-010b", "24 fps (Film)", Pending(None)),
    ("M-OPT-010c", "25 fps (PAL)", Pending(None)),
    ("M-OPT-010d", "30 fps", Pending(None)),
    (
        "M-OPT-010e",
        "29.97 fps (Drop Frame / NTSC-TV)",
        Pending(None),
    ),
    ("M-OPT-010f", "29.97 fps (Non-Drop / NTSC)", Pending(None)),
    SEPARATOR,
    (
        "M-OPT-011",
        "Smaller Launch Quantization",
        Key(&["S-SES-024", "S-GRID-002"]),
    ),
    (
        "M-OPT-012",
        "Larger Launch Quantization",
        Key(&["S-SES-025", "S-GRID-003"]),
    ),
    (
        "M-OPT-013",
        "Triplet Launch Quantization",
        Key(&["S-SES-026", "S-GRID-004"]),
    ),
    (
        "M-OPT-014",
        "Launch Quantization Enabled",
        Key(&["S-SES-027", "S-GRID-005"]),
    ),
    (
        "M-OPT-015",
        "Snap Automation to Grid",
        Cmd("settings.set_snap_automation_to_grid"),
    ),
    ("M-OPT-016", "Fixed Grid", Key(&["S-GRID-006"])),
    SEPARATOR,
    ("M-OPT-017", "Draw Mode", Cmd(VIEW_DRAW_MODE)),
    ("M-OPT-018", "Step Input Mode", Key(&["S-NOTE-044"])),
    SEPARATOR,
    ("M-OPT-019", "Highlight Scales", Key(&["S-NOTE-035"])),
    ("M-OPT-020", "Chase MIDI Notes", Pending(None)),
    ("M-OPT-021", "MIDI Envelope Auto-Reset", Pending(None)),
    (
        "M-OPT-022",
        "Retune Set On Loading Tuning Systems",
        Cmd("settings.set_retune_on_tuning_load"),
    ),
    SEPARATOR,
    (
        "M-OPT-023",
        "Solo Switches",
        Choice("mixer.set_cue_mode", r#"{"enabled":false}"#),
    ),
    (
        "M-OPT-024",
        "Cue Switches",
        Choice("mixer.set_cue_mode", r#"{"enabled":true}"#),
    ),
    // `bl-model`'s switch, which the shell turns to the other state.
    ("M-OPT-025", "Solo in Place", Cmd("mixer.set_solo_in_place")),
    SEPARATOR,
    (
        "M-OPT-026",
        "Lock Envelopes",
        Cmd("song.set_lock_envelopes"),
    ),
    (
        "M-OPT-027",
        "MIDI Arrangement Overdub",
        Cmd(crate::transport::SET_MIDI_OVERDUB),
    ),
    SEPARATOR,
    (
        "M-OPT-028",
        "Max for Live Developer Mode",
        Omitted("Max for Live is another Ableton product (D21); see M-VIEW-030"),
    ),
    SEPARATOR,
    ("M-OPT-029", "Audio Engine On", Cmd(AUDIO_TOGGLE_ENGINE)),
];
