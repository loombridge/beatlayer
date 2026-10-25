//! The File menu.

use super::Action::{Command as Cmd, Pending, Submenu};
use super::{IMP_05, IMP_29, Row, SEPARATOR};
use crate::browser::ADD_FOLDER as BROWSER_ADD_FOLDER;
use crate::commands::{
    SET_COLLECT_ALL, SET_NEW, SET_OPEN, SET_SAVE, SET_SAVE_AS, SET_SAVE_AS_DEFAULT,
    SET_SAVE_AS_TEMPLATE, VIEW_TOGGLE_PLUGIN_LIST, VIEW_TOGGLE_SAVE_HISTORY, WINDOW_CLOSE,
};
use crate::export::OPEN_DIALOG as EXPORT_OPEN_DIALOG;
use crate::file_manager::{SET_SAVE_COPY, VIEW_TOGGLE_FILE_MANAGER};
use crate::set_exchange::INSTALL_PACK;

pub(super) const FILE: &[Row] = &[
    ("M-FILE-001", "New Beatlayer Set", Cmd(SET_NEW)),
    ("M-FILE-002", "Open Beatlayer Set...", Cmd(SET_OPEN)),
    ("M-FILE-003", "Open Recent Set", Pending(None)),
    ("M-FILE-004", "Close Window", Cmd(WINDOW_CLOSE)),
    SEPARATOR,
    ("M-FILE-005", "Install Pack...", Cmd(INSTALL_PACK)),
    (
        "M-FILE-006",
        "Add Folder to Browser...",
        Cmd(BROWSER_ADD_FOLDER),
    ),
    SEPARATOR,
    ("M-FILE-007", "Manage Files", Cmd(VIEW_TOGGLE_FILE_MANAGER)),
    (IMP_29, "Manage Plug-Ins", Cmd(VIEW_TOGGLE_PLUGIN_LIST)),
    ("M-FILE-008", "Open Beatlayer Set Backups", Submenu),
    (IMP_05, "Save History", Cmd(VIEW_TOGGLE_SAVE_HISTORY)),
    SEPARATOR,
    ("M-FILE-009", "Save Beatlayer Set", Cmd(SET_SAVE)),
    ("M-FILE-010", "Save Beatlayer Set As...", Cmd(SET_SAVE_AS)),
    ("M-FILE-011", "Save a Copy...", Cmd(SET_SAVE_COPY)),
    ("M-FILE-012", "Collect All and Save", Cmd(SET_COLLECT_ALL)),
    (
        "M-FILE-013",
        "Save Beatlayer Set As Template...",
        Cmd(SET_SAVE_AS_TEMPLATE),
    ),
    (
        "M-FILE-014",
        "Save Beatlayer Set As Default Set...",
        Cmd(SET_SAVE_AS_DEFAULT),
    ),
    SEPARATOR,
    (
        "M-FILE-015",
        "Export Audio/Video...",
        Cmd(EXPORT_OPEN_DIALOG),
    ),
    (
        "M-FILE-016",
        "Export MIDI Clip...",
        Pending(Some("S-FILE-008")),
    ),
];
