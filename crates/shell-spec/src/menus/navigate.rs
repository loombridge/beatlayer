//! The Navigate menu.

use super::Action::{Command as Cmd, Pending, Shortcut as Key};
use super::{IMP_18, Row, SEPARATOR};
use crate::commands::{
    FOCUS_ARRANGEMENT, FOCUS_CLIP_VIEW, FOCUS_CONTROL_BAR, FOCUS_DEVICE_VIEW, FOCUS_NEXT,
    FOCUS_NEXT_NEIGHBOR, FOCUS_PREVIOUS, FOCUS_PREVIOUS_NEIGHBOR, FOCUS_TOGGLE_TAB_NAVIGATION,
    FOCUS_TOGGLE_WRAP_NAVIGATION,
};
use crate::groove::FOCUS_GROOVE_POOL;
use crate::mixer_view::FOCUS as MIXER_FOCUS;

pub(super) const NAVIGATE: &[Row] = &[
    ("M-NAV-001", "Control Bar", Cmd(FOCUS_CONTROL_BAR)),
    ("M-NAV-002", "Session View", Key(&["S-FOCUS-002"])),
    ("M-NAV-003", "Arrangement View", Cmd(FOCUS_ARRANGEMENT)),
    ("M-NAV-004", "Clip View", Cmd(FOCUS_CLIP_VIEW)),
    ("M-NAV-005", "Device View", Cmd(FOCUS_DEVICE_VIEW)),
    ("M-NAV-006", "Browser", Key(&["S-FOCUS-006"])),
    ("M-NAV-007", "Groove Pool", Cmd(FOCUS_GROOVE_POOL)),
    ("M-NAV-008", "Learn View", Key(&["S-FOCUS-008"])),
    ("M-NAV-009", "Tuning", Pending(None)),
    SEPARATOR,
    ("M-NAV-010", "Mixer", Cmd(MIXER_FOCUS)),
    ("M-NAV-011", "Clip Panels", Key(&["S-FOCUS-010"])),
    SEPARATOR,
    ("M-NAV-012", "Previous", Cmd(FOCUS_PREVIOUS)),
    ("M-NAV-013", "Next", Cmd(FOCUS_NEXT)),
    // Enabled whenever there's a neighbour, whatever the Tab setting
    // (observed in Live 12.4.6).
    (
        "M-NAV-014",
        "Previous Neighbor",
        Cmd(FOCUS_PREVIOUS_NEIGHBOR),
    ),
    ("M-NAV-015", "Next Neighbor", Cmd(FOCUS_NEXT_NEIGHBOR)),
    (IMP_18, "Find Track", Key(&["S-FOCUS-015"])),
    SEPARATOR,
    ("M-NAV-016", "Browser History Back", Key(&["S-BRW-011"])),
    ("M-NAV-017", "Browser History Forward", Key(&["S-BRW-012"])),
    SEPARATOR,
    (
        "M-NAV-018",
        "Use Tab Key to Move Focus",
        Cmd(FOCUS_TOGGLE_TAB_NAVIGATION),
    ),
    (
        "M-NAV-019",
        "Wrap Keyboard Navigation",
        Cmd(FOCUS_TOGGLE_WRAP_NAVIGATION),
    ),
];
