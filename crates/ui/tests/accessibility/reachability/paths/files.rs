//! Commands in this area that have no key and no menu item, and how each is reached.

use crate::accessibility::reachability::setups::*;
use crate::accessibility::reachability::*;
use egui::Key;

pub(crate) fn entries() -> Vec<(&'static str, Path)> {
    use Path::Exception;
    use Reason::{Equivalent, Query};
    vec![
        ("set.list_recoverable", Exception(Query)),
        ("set.backups", Exception(Query)),
        (
            "set.open_backup",
            Exception(Equivalent("File ▸ Open Beatlayer Set Backups (WP-6.63)")),
        ),
        (
            "set.discard_changes",
            Exception(Equivalent(
                "the save-changes dialog's Don't Save, the system's dialog",
            )),
        ),
        (
            "set.relink_missing",
            Exception(Equivalent(
                "the File Manager's Automatic Search button, which needs the File Manager (M-FILE-007); \
                 it also runs by itself when a set with missing files is opened",
            )),
        ),
        // DAWproject in the app (WP-6.37b).
        (
            "set.import_dawproject",
            Exception(Equivalent(
                "File ▸ Open Beatlayer Set... (a .dawproject file chosen there), the browser's \
                 Enter, and a file dropped on the window",
            )),
        ),
        ("set.import_report", Exception(Query)),
        (
            "set.close_dawproject_report",
            Exception(Equivalent(
                "the DAWproject Import and Export window's Close, which Tab reaches, or Esc",
            )),
        ),
        // Separate Stems' dialog (IMP-11).
        ("stems.status", Exception(Query)),
        (
            "stems.install",
            Exception(Equivalent(
                "the Separate Stems dialog's Download Required Packages Now, which Tab reaches",
            )),
        ),
        (
            "stems.separate",
            Exception(Equivalent(
                "the Separate Stems dialog's Separate, which Tab reaches",
            )),
        ),
        (
            "stems.cancel",
            Exception(Equivalent(
                "the Separate Stems dialog's Cancel, which Tab reaches",
            )),
        ),
        (
            "stems.close",
            Exception(Equivalent("the Separate Stems dialog's Cancel, or Esc")),
        ),
        ("export.realtime.status", Exception(Query)),
        (
            "export.realtime.restart",
            Exception(Equivalent(
                "the real-time render's dialog's Restart, which Tab reaches",
            )),
        ),
        (
            "export.realtime.skip",
            Exception(Equivalent(
                "the real-time render's dialog's Skip, which Tab reaches",
            )),
        ),
        (
            "export.realtime.cancel",
            Exception(Equivalent(
                "the real-time render's dialog's Cancel, which Tab reaches",
            )),
        ),
        (
            "export.realtime.auto_restart",
            Exception(Equivalent(
                "the real-time render's dialog's Auto-Restart on Drop-Outs switch, which Tab \
                 reaches",
            )),
        ),
        // The Export Audio/Video dialog (WP-6.71): the keyboard reaches each
        // of its controls with Tab, Return presses Export, Esc closes it.
        (
            "export.audio",
            Exception(Equivalent(
                "the Export Audio/Video dialog's Export button, which Tab reaches and Return \
                 presses; it asks for the file and runs `export.start`",
            )),
        ),
        (
            "export.start",
            Exception(Equivalent(
                "the Export Audio/Video dialog's Export button, which Tab reaches and Return \
                 presses",
            )),
        ),
        (
            "export.set_choices",
            Exception(Equivalent(
                "the Export Audio/Video dialog's controls, which Tab reaches",
            )),
        ),
        (
            "export.close_dialog",
            Exception(Equivalent(
                "the Export Audio/Video dialog's Cancel, which Tab reaches, or Esc",
            )),
        ),
        (
            "export.cancel",
            Exception(Equivalent(
                "the export's progress window's Cancel, which Tab reaches, or Esc",
            )),
        ),
        (
            "export.status",
            Exception(Equivalent(
                "the dialog and the progress window, which draw what it reads",
            )),
        ),
        (
            "set.recover",
            main(crashed_session, nothing, "Recover", Key::Enter),
        ),
        (
            "set.restore",
            Exception(Equivalent(
                "the Save History panel's rows: select one with the arrow keys and press Return, \
                 or press Restore (crash recovery R16, R19)",
            )),
        ),
        (
            "set.save_history",
            Exception(Equivalent(
                "the Save History panel lists the same rows, which reach assistive technology \
                 (crash recovery R13, R19)",
            )),
        ),
        (
            "set.discard_recovery",
            main(crashed_session, nothing, "Discard", Key::Enter),
        ),
        (
            "set.later_recovery",
            main(crashed_session, nothing, "Later", Key::Enter),
        ),
    ]
}
