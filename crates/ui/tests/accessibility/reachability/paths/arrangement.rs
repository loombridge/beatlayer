//! Commands in this area that have no key and no menu item, and how each is reached.

use crate::accessibility::reachability::setups::*;
use crate::accessibility::reachability::*;
use egui::Key;

pub(crate) fn entries() -> Vec<(&'static str, Path)> {
    use Path::Exception;
    use Reason::{Equivalent, Gap, LaterView, Live, Query};
    vec![
        // The Control Bar's transport (control-bar-transport R11).
        ("automation.copy", Exception(ENVELOPE_EDIT)),
        ("automation.cut", Exception(ENVELOPE_EDIT)),
        ("automation.delete_range", Exception(ENVELOPE_EDIT)),
        ("automation.duplicate", Exception(ENVELOPE_EDIT)),
        ("automation.move_breakpoints", Exception(ENVELOPE_EDIT)),
        ("automation.insert_shape", Exception(ENVELOPE_SHAPES)),
        ("automation.paste", Exception(ENVELOPE_EDIT)),
        ("automation.skew", Exception(HANDLES)),
        ("automation.stretch_time", Exception(HANDLES)),
        ("automation.stretch_values", Exception(HANDLES)),
        ("automation.select_segment", Exception(ENVELOPE_EDIT)),
        ("automation.select_breakpoint", Exception(SELECTING)),
        ("clip.select_envelope_breakpoint", Exception(SELECTING)),
        ("automation.set_envelope", Exception(ENVELOPE_EDIT)),
        (
            "track.set_tuning_bypass",
            Exception(LaterView(
                "the In/Out section's row for it, which the mixers' section has no room for \
                 yet; the Arrangement's headers show it where they have the height (WP-6.9)",
            )),
        ),
        // Automation (WP-2.12): the Control Bar's Re-Enable Automation
        // button, and the arrangement's automation lanes, which aren't
        // drawn yet.
        (
            "automation.re_enable",
            main(nothing, control_bar, "Re-Enable Automation", Key::Enter),
        ),
        (
            "arrangement.automation_arm",
            main(nothing, control_bar, "Automation Arm", Key::Enter),
        ),
        ("automation.set_breakpoint", Exception(LANE_KEYS)),
        ("automation.set_curve", Exception(CURVING)),
        ("automation.move_breakpoint", Exception(LANE_KEYS)),
        ("automation.delete_breakpoint", Exception(LANE_KEYS)),
        ("automation.delete", Exception(LANE_KEYS)),
        (
            "automation.re_enable_for",
            Exception(LaterView("the arrangement's automation lanes (WP-2.12)")),
        ),
        (
            "view.set_zoom",
            settings("display-input", nothing, "Zoom", Key::ArrowUp),
        ),
        // Queries.
        ("arrangement.state", Exception(Query)),
        ("view.state", Exception(Query)),
        // Agents' forms of gestures and dialogs.
        (
            "arrangement.move_clips",
            Exception(Equivalent(
                "a drag; the keyboard moves clips with Left and Right \
                 (`arrangement.nudge`), or cut and paste",
            )),
        ),
        (
            "arrangement.select_clips",
            Exception(Equivalent(
                "a click; the keyboard selects with Shift and the arrows \
                 (`arrangement.select_time`) and Cmd-A (Ctrl-A)",
            )),
        ),
        (
            "arrangement.scroll",
            Exception(Equivalent(
                "the wheel; the view follows the tracks the keys select and the playhead \
                 (Follow), and the zoom keys change what it shows",
            )),
        ),
        (
            "arrangement.show_range",
            Exception(Equivalent(
                "an agent's view move; a person zooms with Z, X, and the zoom keys",
            )),
        ),
        (
            "arrangement.show_track",
            Exception(Equivalent(
                "an agent's view move; the view follows the track the keys select",
            )),
        ),
        (
            "edit.go_to_step",
            Exception(Equivalent(
                "the Undo History panel's rows: click one, or move to it with the arrow keys \
                 and press Enter (undo history R6, R8)",
            )),
        ),
        (
            "track.add_take_lane",
            Exception(Equivalent(
                "Insert Take Lane (Shift+Option+T), which adds an empty take lane; recording makes the others",
            )),
        ),
        (
            "take_lane.comp_sources",
            Exception(LaterView(
                "the take lanes' source highlights (WP-6.22b, comping R33)",
            )),
        ),
        (
            "take_lane.create_clip",
            Exception(Equivalent(
                "moving, copying and pasting clips onto a take lane in the Arrangement (comping R27)",
            )),
        ),
        (
            "take_lane.delete",
            Exception(Equivalent(
                "Delete with take lane headers selected, which runs arrangement.delete (comping R7)",
            )),
        ),
        (
            "take_lane.delete_all",
            Exception(Equivalent(
                "Delete All Take Lanes in a track header's or a take lane header's menu (comping R29)",
            )),
        ),
        (
            "take_lane.delete_unused",
            Exception(Equivalent(
                "Delete All Unused Take Lanes in a track header's or a take lane header's menu (comping R29)",
            )),
        ),
        (
            "take_lane.move_comp_edge",
            Exception(LaterView(
                "dragging a source highlight's edge (WP-6.22b, comping R34)",
            )),
        ),
        (
            "take_lane.rename",
            Exception(Equivalent(
                "Cmd+R with a take lane header selected, and Rename in its menu, type the name in the header (comping R28)",
            )),
        ),
        (
            "take_lane.set_audition",
            Exception(Equivalent(
                "the Audition Take Lane switch in a take lane's header, and T (comping R14, R30)",
            )),
        ),
        (
            "arrangement.select_take_lanes",
            Exception(Equivalent(
                "clicking a take lane's header, or Up and Down through the headers (comping R26)",
            )),
        ),
        // The freeze's document-level commands (WP-4.14): `track.freeze` and
        // `track.unfreeze` are the ones a person and an agent run, from the
        // menus and Cmd-Option-Shift-F, and they apply these.
        (
            "track.set_frozen",
            Exception(Equivalent(
                "track.freeze, from the Edit menu, the track and clip menus, and Cmd-Option-Shift-F (freeze-and-bounce R13)",
            )),
        ),
        (
            "track.clear_frozen",
            Exception(Equivalent(
                "track.unfreeze, the same item read as Unfreeze Track (freeze-and-bounce R10, R13)",
            )),
        ),
        // The bounces' document-level commands (WP-4.15): the three bounce
        // commands are the ones a person and an agent run, from the menus
        // and the keys, and they apply these.
        (
            "track.set_bounced",
            Exception(Equivalent(
                "track.bounce_in_place, from the Edit menu and the track and clip menus (freeze-and-bounce R21, R26)",
            )),
        ),
        (
            "track.create_bounced_track",
            Exception(Equivalent(
                "track.bounce_to_new_track, from the Edit menu, the clip and slot menus, and Cmd-B (freeze-and-bounce R22, R26)",
            )),
        ),
        (
            "track.unfreeze",
            Exception(Equivalent(
                "the Freeze Track item, which reads Unfreeze Track and unfreezes a selection that is frozen (freeze-and-bounce R1, R10)",
            )),
        ),
        ("view.lane", Exception(NOTE_EDITOR)),
        ("view.lanes", Exception(NOTE_FOOTER)),
        // The list of the set's plug-ins (IMP-29, WP-6.41): Enter on its
        // Show Only Missing switch.
        (
            "view.set_plugin_list_only_missing",
            main(nothing, plugin_list, "Show Only Missing", Key::Enter),
        ),
        // The detail views (WP-6.27): Option-click on a view's toggle shows
        // both, and the keys of the two toggles do the same one after the
        // other; the Clip View's top edge is a splitter the keyboard reaches
        // and the arrow keys move.
        (
            "view.show_detail_views",
            Exception(Equivalent(
                "Option-click on a view's toggle; Cmd+Option+3 and Cmd+Option+4 show both views in turn",
            )),
        ),
        // The browser's right edge is a splitter the keyboard reaches and
        // the arrow keys move (browser R29).
        (
            "view.set_browser_width",
            main(nothing, browser_area, "Browser Width", Key::ArrowRight),
        ),
        (
            "view.set_detail_height",
            main(
                clip_view_shown,
                detail_area,
                "Clip View Height",
                Key::ArrowUp,
            ),
        ),
        ("track.set_devices_enabled", Exception(TRACK_MENU)),
        (
            "arrangement.set_crossfade",
            Exception(Live(
                "Live doesn't support editing audio clip fades with a screen reader (manual \
                 41.4.1.3); Create Fade (S-ARR-008) makes a crossfade from the keyboard",
            )),
        ),
        (
            "arrangement.delete_crossfade",
            Exception(Live(
                "Live doesn't support editing audio clip fades with a screen reader (manual \
                 41.4.1.3); Delete Fades (S-ARR-009) deletes a crossfade from the keyboard",
            )),
        ),
        (
            "arrangement.select_fade_handle",
            Exception(Live(
                "a fade handle is a pointer target Live doesn't expose to a screen reader \
                 (manual 41.4.1.3)",
            )),
        ),
        // Gaps.
        (
            "track.move",
            Exception(Gap(
                "moving tracks with Cmd and the arrows (S-TRACK-012), not bound yet",
            )),
        ),
    ]
}
