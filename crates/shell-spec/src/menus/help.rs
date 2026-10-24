//! The Help menu.

use super::Action::{Omitted, Pending, Shortcut as Key};
use super::{Action, IMP_16, IMP_16_SEPARATOR, Row, SEPARATOR};

pub(super) const HELP: &[Row] = &[
    (IMP_16, "Search Commands…", Key(&["S-MENU-007"])),
    (IMP_16_SEPARATOR, "", Action::Separator),
    ("M-HELP-001", "Learn View", Key(&["S-VIEW-018"])),
    (
        "M-HELP-002",
        "Pack Overview",
        Omitted("Ableton's pack service (D21); see M-VIEW-006"),
    ),
    SEPARATOR,
    ("M-HELP-003", "Load Demo Set", Pending(None)),
    ("M-HELP-004", "Read the Beatlayer Manual...", Pending(None)),
    (
        "M-HELP-005",
        "Learn Live at ableton.com...",
        Omitted("Ableton's website (D21); Beatlayer's own site goes in M-HELP-006 and M-HELP-007"),
    ),
    SEPARATOR,
    ("M-HELP-006", "Join the User Forum...", Pending(None)),
    ("M-HELP-007", "Get Support", Pending(None)),
    (
        "M-HELP-008",
        "User Account and Licenses...",
        Omitted("Ableton's account (D21); Beatlayer is open source and has no licence keys"),
    ),
    (
        "M-HELP-009",
        "Purchase License...",
        Omitted("Ableton's store (D21); Beatlayer is free"),
    ),
    SEPARATOR,
    ("M-HELP-010", "Check for Updates...", Pending(None)),
];
