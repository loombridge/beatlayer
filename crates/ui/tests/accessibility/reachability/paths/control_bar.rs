//! Commands in this area that have no key and no menu item, and how each is reached.

use crate::accessibility::reachability::setups::*;
use crate::accessibility::reachability::*;
use egui::Key;

pub(crate) fn entries() -> Vec<(&'static str, Path)> {
    use Path::Exception;
    use Reason::{Equivalent, LaterView, Live, Query};
    vec![
        (
            "transport.play",
            main(nothing, control_bar, "Play", Key::Enter),
        ),
        (
            "transport.stop",
            main(nothing, control_bar, "Stop", Key::Enter),
        ),
        (
            "transport.tap_tempo",
            main(nothing, control_bar, "Tap Tempo", Key::Enter),
        ),
        (
            "transport.resync_external_hardware",
            main(
                the_resync_button_shown,
                control_bar,
                "Resync External Hardware",
                Key::Enter,
            ),
        ),
        (
            "transport.set_loop_enabled",
            main(nothing, control_bar, "Arrangement Loop", Key::Enter),
        ),
        (
            "transport.set_punch_in",
            main(nothing, control_bar, "Punch-In", Key::Enter),
        ),
        (
            "transport.set_punch_out",
            main(nothing, control_bar, "Punch-Out", Key::Enter),
        ),
        (
            "transport.set_metronome_count_in",
            Exception(METRONOME_MENU),
        ),
        ("transport.set_metronome_sound", Exception(METRONOME_MENU)),
        ("transport.set_metronome_rhythm", Exception(METRONOME_MENU)),
        (
            "transport.play_selection",
            Exception(Equivalent(
                "an agent's explicit form of Space and Option-Space with a time selection, \
                 which the keys run from the selection (transport R35, R36)",
            )),
        ),
        (
            "transport.phase_nudge",
            Exception(Live(
                "Live binds no key to Phase Nudge Down or Up, which nudge while the mouse holds \
                 them; a key or MIDI mapping makes each a momentary control (transport R28)",
            )),
        ),
        // The Control Bar's scale controls (scales R9, R12, R14): the
        // switch changes the selected clips' scales, or the set's current
        // scale when no clip is selected.
        (
            "song.set_scale",
            main(nothing, control_bar, "Scale Mode", Key::Enter),
        ),
        // Tuning systems (WP-6.9): the browser's Tunings label and the
        // Tuning section, which load, change and remove the tuning, come with
        // WP-6.10; until then agents reach these.
        ("tuning.load", Exception(TUNING_SECTION)),
        ("tuning.remove", Exception(TUNING_SECTION)),
        ("tuning.set", Exception(TUNING_SECTION)),
        ("tuning.set_range", Exception(TUNING_SECTION)),
        ("tuning.set_reference", Exception(TUNING_SECTION)),
        ("tuning.state", Exception(Query)),
        // The Groove Pool (WP-6.20): its fields and buttons are in the
        // browser column's Tab order, the Control Bar's field has Up and
        // Down, and the pool's own keys select, rename and delete a groove.
        (
            "groove.add",
            Exception(Reason::AgentOnly(
                "adds a groove from nothing: a person's pool grows by loading a groove file \
                 and by extracting a groove from a clip",
            )),
        ),
        (
            "groove.commit",
            main(a_groove_pool, pool_focused, "Commit Groove", Key::Enter),
        ),
        (
            "groove.delete",
            main(a_groove_pool, pool_row_selected, "Groove Pool", Key::Delete),
        ),
        (
            "groove.hot_swap",
            main(a_groove_pool, pool_focused, "Hot-Swap Groove", Key::Enter),
        ),
        (
            "groove.load",
            Exception(Equivalent(
                "Enter on a groove in the browser's Grooves label (S-BRW-004, \
                 `browser.load_selected`), which loads it into the pool or replaces the groove \
                 being hot-swapped (grooves R32, R33)",
            )),
        ),
        ("groove.pool", Exception(Query)),
        (
            "groove.rename",
            Exception(Equivalent(
                "Cmd+R (Ctrl+R) with the Groove Pool focused and a groove selected, which \
                 types the new name in its row (grooves R26, S-EDIT-008)",
            )),
        ),
        (
            "groove.save",
            main(a_groove_pool, pool_focused, "Save Groove", Key::Enter),
        ),
        (
            "groove.set",
            main(a_groove_pool, pool_focused, "Quantize", Key::ArrowUp),
        ),
        (
            "groove.set_auto_load",
            main(a_groove_pool, pool_focused, "Auto Load Groove", Key::Enter),
        ),
        (
            "groove.set_global_amount",
            main(nothing, control_bar, "Global Amount", Key::ArrowUp),
        ),
        (
            "groove.to_clip",
            Exception(Live(
                "dragging a groove from the browser or the pool onto a MIDI track or the drop \
                 area; the manual gives it no keys (manual 14.2)",
            )),
        ),
        ("transport.stop_recording", Exception(RECORDING)),
        // Tempo breakpoints (WP-2.20): the arrangement's tempo envelope isn't
        // drawn yet, so agents reach these through the agent API until it is.
        (
            "song.set_tempo_breakpoint",
            Exception(LaterView("the arrangement's tempo envelope (WP-2.20)")),
        ),
        (
            "song.move_tempo_breakpoint",
            Exception(LaterView("the arrangement's tempo envelope (WP-2.20)")),
        ),
        (
            "song.delete_tempo_breakpoint",
            Exception(LaterView("the arrangement's tempo envelope (WP-2.20)")),
        ),
        (
            "song.set_time_signature_change",
            Exception(Equivalent(
                "the Control Bar's signature fields, which change the marker in force, and \
                 Cmd-R, which types a selected marker's signature (the time editing spec, R31 \
                 and R33)",
            )),
        ),
        (
            "song.move_time_signature_change",
            Exception(Equivalent(
                "arrangement.move_time_signature_change, which the arrow keys and a drag run \
                 (the time editing spec, R32)",
            )),
        ),
        (
            "song.delete_time_signature_change",
            Exception(Equivalent(
                "the Delete key while a time-signature marker is selected, which the \
                 Arrangement's keyboard runs directly (the time editing spec, R32)",
            )),
        ),
        // The time-signature lane (WP-6.61).
        (
            "arrangement.select_time_signature_change",
            Exception(Equivalent(
                "clicking a marker selects it, and Tab reaches the markers in the Arrangement's \
                 Tab order (the time editing spec, R31)",
            )),
        ),
        (
            "arrangement.edit_time_signature",
            Exception(Equivalent(
                "Cmd-R while a marker is selected, which the Arrangement's keyboard runs \
                 directly (the time editing spec, R31)",
            )),
        ),
        (
            "arrangement.move_time_signature_change",
            Exception(Equivalent(
                "the arrow keys move the selected marker one grid step, and dragging one moves \
                 it (the time editing spec, R32)",
            )),
        ),
        (
            "arrangement.delete_fragmentary_bar_time",
            Exception(Unobserved(
                "the short bar's context menu, which the pointer opens; Live's keyboard path \
                 for it isn't observed yet (the time editing spec, R34)",
            )),
        ),
        (
            "arrangement.complete_fragmentary_bar",
            Exception(Unobserved(
                "the short bar's context menu, which the pointer opens; Live's keyboard path \
                 for it isn't observed yet (the time editing spec, R34)",
            )),
        ),
        // The Arrangement's locator buttons (time-editing R3, R4, R6).
        (
            "locator.add",
            main(nothing, arrangement_view, "Set Locator", Key::Enter),
        ),
        (
            "locator.delete",
            main(a_locator, arrangement_view, "Delete Locator", Key::Enter),
        ),
        // The jumps between the locators need the audio engine, so the
        // buttons that run them are named instead (time-editing R6, R13).
        (
            "locator.jump",
            Exception(Equivalent(
                "clicking a locator jumps to it, and the locators are in the Arrangement's Tab \
                 order, where Enter jumps to the one reached (the time editing spec, R5 and R12)",
            )),
        ),
        (
            "locator.jump_previous",
            Exception(Equivalent(
                "the Previous Locator button, which the keyboard reaches with Tab; the jump \
                 needs the audio engine (the time editing spec, R6 and R13)",
            )),
        ),
        (
            "locator.jump_next",
            Exception(Equivalent(
                "the Next Locator button, which the keyboard reaches with Tab; the jump needs \
                 the audio engine (the time editing spec, R6 and R13)",
            )),
        ),
        (
            "locator.select",
            Exception(Equivalent(
                "clicking a locator selects it, and Tab reaches the locators in the \
                 Arrangement's Tab order (the time editing spec, R5 and R12)",
            )),
        ),
        (
            "locator.move",
            Exception(Equivalent(
                "the arrow keys move the selected locators one grid step, and dragging one \
                 moves it (the time editing spec, R7)",
            )),
        ),
        (
            "locator.loop_to_next",
            Exception(Equivalent(
                "a locator's context menu, which the pointer opens (the time editing spec, R9)",
            )),
        ),
        (
            "song.set_start_time",
            Exception(Equivalent(
                "a locator's and the loop brace's context menus, which the pointer open (the \
                 time editing spec, R10)",
            )),
        ),
        (
            "locator.rename",
            Exception(Equivalent(
                "Cmd-R and Edit ▸ Rename while a locator is selected, which the Arrangement's \
                 keyboard runs directly (the time editing spec, R8)",
            )),
        ),
        (
            "locator.set_info_text",
            Exception(Equivalent(
                "Edit ▸ Edit Info Text (M-EDIT-025) while a locator is selected: it opens the Info \
                 View's field for it (the Info View spec, R10)",
            )),
        ),
        // The document's own locator commands (the time editing spec, R1 to
        // R13): the shell's locator.* commands run them, and agents reach
        // them through the agent API.
        (
            "song.add_locator",
            Exception(Equivalent(
                "the shell's locator.add, which the Set Locator button, Create ▸ Add Locator and \
                 a click in the scrub area run (the time editing spec, R3 and R13)",
            )),
        ),
        (
            "song.delete_locator",
            Exception(Equivalent(
                "the shell's locator.delete, which the Delete Locator button and the Delete key \
                 run (the time editing spec, R4 and R13)",
            )),
        ),
        (
            "song.insert_time",
            Exception(Equivalent(
                "arrangement.insert_silence, which Cmd-I and Create ▸ Insert Silence run on the \
                 time selection (the time editing spec, R17 and R24)",
            )),
        ),
        (
            "song.delete_time",
            Exception(Equivalent(
                "arrangement.delete_time, which Cmd-Shift-Delete and Edit ▸ Delete Time run on \
                 the time selection (the time editing spec, R15 and R24)",
            )),
        ),
        (
            "song.move_locator",
            Exception(Equivalent(
                "the shell's locator.move, which dragging a locator and the arrow keys run (the \
                 time editing spec, R7 and R13)",
            )),
        ),
        (
            "song.rename_locator",
            Exception(Equivalent(
                "the shell's locator.rename, which Cmd-R and Edit ▸ Rename run (the time editing \
                 spec, R8 and R13)",
            )),
        ),
        (
            "song.set_locator_info_text",
            Exception(Equivalent(
                "the shell's locator.set_info_text, which Edit ▸ Edit Info Text runs (the time \
                 editing spec, R8 and R13)",
            )),
        ),
        (
            "song.loop_to_next_locator",
            Exception(Equivalent(
                "the shell's locator.loop_to_next, which a locator's context menu runs (the time \
                 editing spec, R9 and R13)",
            )),
        ),
        (
            "song.set_start",
            Exception(Equivalent(
                "the shell's song.set_start_time, which a locator's and the loop brace's context \
                 menus run (the time editing spec, R10 and R13)",
            )),
        ),
        ("transport.state", Exception(Query)),
        ("link.state", Exception(Query)),
        (
            "link.set_enabled",
            main(the_link_toggle_shown, control_bar, "Link", Key::Enter),
        ),
        (
            "link.set_show_toggle",
            settings("link", nothing, "Show Link Toggle", Key::ArrowUp),
        ),
        ("tempo_follower.state", Exception(Query)),
        (
            "tempo_follower.set_enabled",
            main(
                the_follow_toggle_shown,
                control_bar,
                "Tempo Follower",
                Key::Enter,
            ),
        ),
        (
            "settings.set_show_tempo_follower_toggle",
            settings(
                "tempo-midi",
                nothing,
                "Show Tempo Follower Toggle",
                Key::ArrowUp,
            ),
        ),
        (
            "settings.set_tempo_follower_input",
            settings(
                "tempo-midi",
                the_follow_toggle_shown,
                "Input Channel",
                Key::ArrowDown,
            ),
        ),
        // Link Audio (the Link Audio spec, R2): the Link page's section.
        (
            "link.set_audio",
            settings("link", nothing, "Link Audio", Key::Enter),
        ),
        (
            "link.set_audio_latency",
            settings("link", nothing, "Latency", Key::ArrowRight),
        ),
        (
            "link.set_audio_name",
            Exception(Equivalent(
                "the Name field, typed into and then committed with Enter or by leaving it: \
                 it sends its text once, so a typed space isn't trimmed away",
            )),
        ),
        (
            "link.set_sync_to_incoming_audio",
            settings("link", nothing, "Sync to Incoming Audio", Key::Enter),
        ),
        ("song.scales", Exception(Query)),
        (
            "song.unfollow_tempo_automation",
            Exception(Equivalent(
                "the tempo field's context menu (the Context Menu key or \
                 Shift-F10 with the field focused)",
            )),
        ),
    ]
}
