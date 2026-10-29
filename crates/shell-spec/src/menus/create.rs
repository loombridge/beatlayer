//! The Create menu.

use super::Action::{Command as Cmd, Pending, Shortcut as Key};
use super::{Row, SAMPLE_DELETE, SAMPLE_INSERT, SAMPLE_WARP, SEPARATOR};
use crate::audio_editing::CONSOLIDATE_TO_SCENE;
use crate::capture_midi::CAPTURE as CAPTURE_MIDI;

pub(super) const CREATE: &[Row] = &[
    ("M-CREATE-001", "Insert Audio Track", Key(&["S-TRACK-001"])),
    ("M-CREATE-002", "Insert MIDI Track", Key(&["S-TRACK-002"])),
    ("M-CREATE-003", "Insert Return Track", Key(&["S-TRACK-003"])),
    ("M-CREATE-004", "Insert Take Lane", Key(&["S-COMP-004"])),
    SEPARATOR,
    (
        "M-CREATE-005",
        "Insert Scene",
        Key(&["S-SES-008", "S-ARR-014"]),
    ),
    (
        "M-CREATE-006",
        "Capture and Insert Scene",
        Key(&["S-SES-009"]),
    ),
    (
        "M-CREATE-007",
        "Consolidate Time to New Scene",
        Cmd(CONSOLIDATE_TO_SCENE),
    ),
    SEPARATOR,
    (
        "M-CREATE-008",
        "Insert Empty MIDI Clip",
        Key(&["S-SES-007", "S-ARR-037"]),
    ),
    ("M-CREATE-009", "Capture MIDI", Cmd(CAPTURE_MIDI)),
    SEPARATOR,
    ("M-CREATE-010", "Create Fade", Key(&["S-ARR-008"])),
    // The manual places these in the Create menu while the Sample Editor has
    // focus (the note under menus.md's Create menu); they are disabled
    // elsewhere.
    (SAMPLE_WARP, "Insert Warp Marker", Key(&["S-SAMPLE-009"])),
    (SAMPLE_INSERT, "Insert Transient", Key(&["S-SAMPLE-011"])),
    (SAMPLE_DELETE, "Delete Transient", Key(&["S-SAMPLE-012"])),
    SEPARATOR,
    (
        "M-CREATE-011",
        "Slice to New MIDI Track",
        Cmd(crate::slice::OPEN),
    ),
    (
        "M-CREATE-012",
        "Convert Harmony to New MIDI Track",
        Pending(None),
    ),
    (
        "M-CREATE-013",
        "Convert Melody to New MIDI Track",
        Pending(None),
    ),
    (
        "M-CREATE-014",
        "Convert Drums to New MIDI Track",
        Pending(None),
    ),
    (
        "M-CREATE-015",
        "Separate Stems to New Audio Tracks",
        Cmd(crate::stems::OPEN),
    ),
    SEPARATOR,
    ("M-CREATE-016", "Add Locator", Cmd(crate::locator::ADD)),
    (
        "M-CREATE-017",
        "Add Time Signature Change",
        Cmd(crate::locator::INSERT_SIGNATURE),
    ),
    SEPARATOR,
    ("M-CREATE-018", "Import Audio/MIDI File...", Pending(None)),
];
