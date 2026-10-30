//! The Playback menu.

use super::Action::{Command as Cmd, Pending, Shortcut as Key, Submenu};
use super::{Row, SEPARATOR};

pub(super) const PLAYBACK: &[Row] = &[
    ("M-PLAY-001", "Play", Key(&["S-TRANS-001"])),
    ("M-PLAY-002", "Continue Playback", Key(&["S-TRANS-002"])),
    ("M-PLAY-003", "Play From Insert Marker", Key(&["S-ARR-032"])),
    (
        "M-PLAY-004",
        "Return Play Position to 1.1.1",
        Cmd(crate::transport::RETURN_TO_START),
    ),
    (
        "M-PLAY-005",
        "Move Insert Marker To Playhead",
        Key(&["S-ARR-033"]),
    ),
    ("M-PLAY-006", "Options", Submenu),
    ("M-PLAY-006a", "Follow", Key(&["S-ZOOM-005"])),
    ("M-PLAY-006b", "Start Transport With Record", Pending(None)),
    (
        "M-PLAY-006c",
        "Start Transport With Tap Tempo",
        Cmd(crate::commands::SETTINGS_SET_START_TRANSPORT_WITH_TAP_TEMPO),
    ),
    (
        "M-PLAY-006d",
        "Link Start Stop Sync",
        Cmd(crate::link::SET_START_STOP_SYNC),
    ),
    SEPARATOR,
    ("M-PLAY-007", "Record to Arrangement", Key(&["S-TRANS-006"])),
    (
        "M-PLAY-008",
        "Arm Recording to Arrangement",
        Key(&["S-TRANS-007"]),
    ),
    ("M-PLAY-009", "Record to Session", Key(&["S-TRANS-008"])),
    SEPARATOR,
    ("M-PLAY-010", "Metronome", Key(&["S-TRANS-011"])),
    (
        "M-PLAY-011",
        "Hear Metronome Only While Recording",
        Cmd(crate::transport::SET_METRONOME_ONLY_WHILE_RECORDING),
    ),
    SEPARATOR,
    ("M-PLAY-012", "Back to Arrangement", Key(&["S-TRANS-009"])),
];
