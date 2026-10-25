//! Commands in this area that have no key and no menu item, and how each is reached.

use crate::accessibility::reachability::*;

pub(crate) fn entries() -> Vec<(&'static str, Path)> {
    use Path::Exception;
    use Reason::{Equivalent, LaterView, Query, Unobserved};
    vec![
        // The Session View's grid (WP-3.7): the pointer's selection, drags,
        // drops, and the key the view reads itself.
        ("session.state", Exception(Query)),
        // The scene's colour palette comes with the Session View's context
        // menus (the themes spec, R16).
        (
            "scene.set_colour",
            Exception(LaterView(
                "the scene's context menu and its colour palette, which come with the Session \
                 View's context menus (WP-6.56)",
            )),
        ),
        (
            "session.scroll",
            Exception(Equivalent(
                "the wheel; the arrows and Page Up and Page Down bring the scenes into view",
            )),
        ),
        (
            "session.delete",
            Exception(Equivalent(
                "the Delete key with the grid focused; the slot and scene context menus arrive \
                 with the Session View's menus",
            )),
        ),
        (
            "session.copy_clip_to_slot",
            Exception(Equivalent(
                "Option-dragging (Ctrl-dragging) a clip to another slot; dragging moves it \
                 (`clip.move_to_slot`)",
            )),
        ),
        (
            "session.copy_to_arrangement",
            Exception(Equivalent(
                "dragging clips onto the Arrangement View or into the other window; Copy in \
                 the Session View and Paste in the Arrangement (`arrangement.paste`) do the \
                 same from the keyboard",
            )),
        ),
        (
            "session.copy_to_session",
            Exception(Equivalent(
                "dragging clips onto the Session View or into the other window; Copy in the \
                 Arrangement and Paste in the Session View (`session.paste`) do the same from \
                 the keyboard",
            )),
        ),
        (
            "session.drop_files",
            Exception(Equivalent(
                "dropping files from the browser, Finder, or Explorer on a slot",
            )),
        ),
        (
            "session.duplicate_scene",
            Exception(Equivalent(
                "the scene's context menu, Edit ▸ Duplicate (the menu arrives with a later \
                 package)",
            )),
        ),
        // The Session View's launching (WP-3.2): the launch button on a slot
        // with a clip, the scene launch buttons, and the status row arrive
        // with the launching UI (WP-3.8); the commands are the engine's.
        (
            "session.launch",
            Exception(LaterView(
                "the Session View's launch buttons and status row (WP-3.8)",
            )),
        ),
        (
            "session.release",
            Exception(LaterView(
                "the Session View's launch buttons, which send the release (WP-3.8)",
            )),
        ),
        (
            "session.launch_scene",
            Exception(LaterView(
                "the Session View's scene launch buttons (WP-3.8)",
            )),
        ),
        (
            "session.stop_track",
            Exception(LaterView(
                "the Session View's stop buttons and status row (WP-3.8)",
            )),
        ),
        (
            "session.stop_all",
            Exception(LaterView(
                "the Session View's Stop All Clips button (WP-3.8)",
            )),
        ),
        (
            "session.back_to_arrangement_track",
            Exception(LaterView(
                "the Arrangement's per-track Back to Arrangement buttons (WP-3.8)",
            )),
        ),
        // Session recording (WP-3.11): Session Record is on a shortcut and the
        // Playback menu; the slots' record buttons arrive with the launching
        // UI.
        (
            "session.record_slot",
            Exception(LaterView("the Session View's Clip Record buttons (WP-3.8)")),
        ),
        (
            "session.overdub",
            Exception(Equivalent(
                "Session Record (`session.record`) overdubs the MIDI clip that plays",
            )),
        ),
        (
            "session.stop_recording",
            Exception(Equivalent(
                "Session Record again (`session.record`) ends the recording",
            )),
        ),
        // The Session View's scenes and slots (WP-3.1): the grid, its
        // context menus, and their keys are WP-3.7's.
        (
            "scene.create",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "scene.delete",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "scene.move",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "scene.duplicate",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "scene.rename",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "scene.set_tempo",
            Exception(Unobserved(
                "the scene's Tempo field, which a click focuses and Up, Down, digits, and Delete edit; its place in Live's Tab order",
            )),
        ),
        (
            "scene.set_follow_actions",
            Exception(Unobserved(
                "the follow controls in the Scene View, which Tab reaches; their place in Live's Tab order",
            )),
        ),
        (
            "session.set_follow_actions_enabled",
            Exception(Unobserved(
                "the switch in Main's status row, which Tab reaches with the grid's buttons; its place in Live's Tab order",
            )),
        ),
        (
            "scene.set_time_signature",
            Exception(Unobserved(
                "the scene's Signature fields, which a click focuses and Up, Down, digits, and Delete edit; their place in Live's Tab order",
            )),
        ),
    ]
}
