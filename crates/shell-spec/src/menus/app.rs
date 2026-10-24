//! The application menu on macOS.

use super::Action::{Command as Cmd, System};
use super::{Row, SEPARATOR, SystemItem};
use crate::commands::{APP_ABOUT, APP_HIDE, APP_QUIT, APP_SETTINGS};

pub(super) const APP: &[Row] = &[
    ("M-APP-001", "About Beatlayer", Cmd(APP_ABOUT)),
    SEPARATOR,
    ("M-APP-002", "Settings...", Cmd(APP_SETTINGS)),
    SEPARATOR,
    ("M-APP-003", "Services", System(SystemItem::Services)),
    SEPARATOR,
    ("M-APP-004", "Hide Beatlayer", Cmd(APP_HIDE)),
    ("M-APP-005", "Hide Others", System(SystemItem::HideOthers)),
    ("M-APP-006", "Show All", System(SystemItem::ShowAll)),
    SEPARATOR,
    ("M-APP-007", "Quit Beatlayer", Cmd(APP_QUIT)),
];
