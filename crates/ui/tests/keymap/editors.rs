//! The Sample Editor and note editor keys.

use bl_commands::Context;
use bl_ui::keymap::{Keymap, Mods, Platform, Status};
use egui::Key;

use crate::keymap::helpers::*;

// Spec: warping R49, R50. Cmd+U (S-SAMPLE-001) and Cmd+Shift+U (S-SAMPLE-002)
// are bound in the Sample Editor to clip.quantize_audio and clip_view.quantize_settings.
#[test]
fn quantize_and_quantize_settings_are_bound_in_the_sample_editor() {
    let keymap = Keymap::live12();
    let contexts = [Context::SampleEditor, Context::Global];
    for (platform, u_mods, shift_u_mods) in [
        (
            Platform::Mac,
            mods(true, false, false, false),
            mods(true, false, false, true),
        ),
        (
            Platform::Other,
            mods(false, true, false, false),
            mods(false, true, false, true),
        ),
    ] {
        let quantize = keymap
            .resolve(platform, Key::U, u_mods, &contexts)
            .map(|hit| hit.command.id.clone());
        assert_eq!(
            quantize.as_deref(),
            Some("clip.quantize_audio"),
            "{platform:?}"
        );
        let settings = keymap
            .resolve(platform, Key::U, shift_u_mods, &contexts)
            .map(|hit| hit.command.id.clone());
        assert_eq!(
            settings.as_deref(),
            Some("clip_view.quantize_settings"),
            "{platform:?}"
        );
    }
    let entry_001 = keymap.entry_for("S-SAMPLE-001").unwrap();
    assert_eq!(entry_001.status, Status::Bound);
    let entry_002 = keymap.entry_for("S-SAMPLE-002").unwrap();
    assert_eq!(entry_002.status, Status::Bound);
}

// Spec: midi-transformations R21. Cmd+Shift+U (Ctrl+Shift+U on Windows;
// S-NOTE-012) shows the Transform panel with Quantize chosen, in the note
// editor, beside Cmd+U (S-NOTE-011), which quantises.
#[test]
fn quantize_and_quantize_settings_are_bound_in_the_note_editor() {
    let keymap = Keymap::live12();
    let contexts = [Context::NoteEditor, Context::Global];
    for (platform, u_mods, shift_u_mods) in [
        (
            Platform::Mac,
            mods(true, false, false, false),
            mods(true, false, false, true),
        ),
        (
            Platform::Other,
            mods(false, true, false, false),
            mods(false, true, false, true),
        ),
    ] {
        let resolve = |modifiers| {
            keymap
                .resolve(platform, Key::U, modifiers, &contexts)
                .map(|hit| hit.command.id.clone())
        };
        assert_eq!(
            resolve(u_mods).as_deref(),
            Some("note.quantize"),
            "{platform:?}"
        );
        assert_eq!(
            resolve(shift_u_mods).as_deref(),
            Some("clip_view.quantize_settings"),
            "{platform:?}"
        );
    }
    assert_eq!(
        keymap.entry_for("S-NOTE-012").unwrap().status,
        Status::Bound
    );
}

// Spec: find-and-select-notes R1, R13. Cmd+Shift+F (Ctrl+Shift+F on Windows;
// S-NOTE-037) shows the note filters and Cmd+Shift+A (S-NOTE-034) inverts the
// selection, in the note editor.
#[test]
fn the_note_filters_and_invert_selection_are_bound_in_the_note_editor() {
    let keymap = Keymap::live12();
    let contexts = [Context::NoteEditor, Context::Global];
    for (platform, shift) in [
        (Platform::Mac, mods(true, false, false, true)),
        (Platform::Other, mods(false, true, false, true)),
    ] {
        let filters = keymap
            .resolve(platform, Key::F, shift, &contexts)
            .expect("Shift with Cmd or Ctrl and F is bound");
        assert_eq!(filters.command.id, "clip_view.note_filters", "{platform:?}");
        assert_eq!(filters.command.args["toggle"], true);
        let invert = keymap
            .resolve(platform, Key::A, shift, &contexts)
            .expect("and so is A");
        assert_eq!(
            invert.command.id, "clip_view.invert_selection",
            "{platform:?}"
        );
    }
    for id in ["S-NOTE-034", "S-NOTE-037"] {
        assert_eq!(keymap.entry_for(id).unwrap().status, Status::Bound, "{id}");
    }
}

// Spec: piano-roll R23 to R31 (WP-6.53).
#[test]
fn the_note_editors_keys_are_all_handled() {
    // Every key the note editor reads, and every key the Edit menu's
    // clipboard items run, is bound to a command or read by the note
    // editor itself.
    let keymap = Keymap::live12();
    for id in [
        "S-NOTE-001",
        "S-NOTE-002",
        "S-NOTE-017",
        "S-NOTE-018",
        "S-NOTE-019",
        "S-NOTE-020",
        "S-NOTE-042",
        "S-NOTE-043",
        "S-NOTE-045",
        "S-NOTE-046",
        // WP-6.5: Step Input Mode, and the arrows that step-record.
        "S-NOTE-044",
        "S-NOTE-048",
        "S-NOTE-049",
        // WP-6.1: the value keys, the typed velocity, the Cmd-drag, and
        // deactivating.
        "S-NOTE-016",
        "S-NOTE-026",
        "S-NOTE-027",
        "S-NOTE-028",
        "S-NOTE-029",
        "S-NOTE-041",
        // WP-6.2: Group Notes and Ungroup Notes.
        "S-NOTE-031",
        "S-NOTE-032",
        "S-NOTE-050",
        "S-NOTE-051",
        "S-A11Y-004",
        "S-A11Y-005",
        "S-A11Y-006",
        "S-A11Y-007",
        "S-A11Y-008",
        "S-A11Y-009",
        "S-A11Y-010",
        "S-A11Y-011",
        "S-EDIT-001",
        "S-EDIT-002",
        "S-EDIT-003",
        "S-EDIT-004",
        "S-EDIT-005",
        "S-EDIT-009",
    ] {
        let entry = keymap
            .entry_for(id)
            .unwrap_or_else(|| panic!("{id} is in the keymap"));
        assert!(
            matches!(entry.status, Status::Bound | Status::Widget(_)),
            "{id} is handled: {:?}",
            entry.status
        );
    }
}

// Spec: step-recording R4, R6, R9. Cmd-Shift-B (Ctrl-Shift-B) runs Step Input
// Mode in the note editor (S-NOTE-044; the Options menu's item reaches it from
// anywhere), and Right and Left are the note editor's own while it
// step-records.
#[test]
fn cmd_shift_b_runs_step_input_mode_and_the_arrows_step_in_the_note_editor() {
    let keymap = Keymap::live12();
    let entry = keymap.entry_for("S-NOTE-044").unwrap();
    assert_eq!(entry.status, Status::Bound);
    for (platform, mods) in [
        (Platform::Mac, mods(true, false, false, true)),
        (Platform::Other, mods(false, true, false, true)),
    ] {
        let hit = keymap
            .resolve(
                platform,
                Key::B,
                mods,
                &[Context::NoteEditor, Context::Global],
            )
            .expect("Cmd-Shift-B is bound in the note editor");
        assert_eq!(hit.command.id, "clip_view.step_input_mode", "{platform:?}");
    }
    for id in ["S-NOTE-048", "S-NOTE-049"] {
        let entry = keymap.entry_for(id).unwrap();
        assert!(matches!(entry.status, Status::Widget(_)), "{id}");
    }
}

// Spec: note-editing R22, R23. Cmd-G and Cmd-Shift-G group and ungroup the
// selected notes (S-NOTE-031, S-NOTE-032); with devices or tracks in front
// they are those windows' own.
#[test]
fn cmd_g_and_cmd_shift_g_group_and_ungroup_notes_while_notes_are_selected() {
    let keymap = Keymap::live12();
    let notes = [Context::NoteEditor, Context::NoteSelection, Context::Global];
    for (platform, group, ungroup) in [
        (
            Platform::Mac,
            mods(true, false, false, false),
            mods(true, false, false, true),
        ),
        (
            Platform::Other,
            mods(false, true, false, false),
            mods(false, true, false, true),
        ),
    ] {
        let hit = |mods, contexts: &[Context]| {
            keymap
                .resolve(platform, Key::G, mods, contexts)
                .map(|hit| hit.command.id.clone())
        };
        assert_eq!(
            hit(group, &notes).as_deref(),
            Some("note.group"),
            "{platform:?}"
        );
        assert_eq!(
            hit(ungroup, &notes).as_deref(),
            Some("note.ungroup"),
            "{platform:?}"
        );
        // With no notes selected, the keys are not the notes'.
        let none = [Context::NoteEditor, Context::Global];
        assert_ne!(
            hit(group, &none).as_deref(),
            Some("note.group"),
            "{platform:?}"
        );
    }
}

// Spec: note-editing R14, R15, R18. 0 deactivates the selected notes through
// the keymap (S-NOTE-041); the keys that read a number or a direction are the
// note editor's, and Cmd-Up and Cmd-Down with the Chord Selector Pad focused
// (S-NOTE-052) are the Chord Selector Pad's.
#[test]
fn zero_deactivates_the_selected_notes_and_the_value_keys_are_the_editors() {
    let keymap = Keymap::live12();
    let contexts = [Context::NoteEditor, Context::NoteSelection, Context::Global];
    let none = mods(false, false, false, false);
    let hit = keymap
        .resolve(Platform::Mac, Key::Num0, none, &contexts)
        .expect("0 is bound while notes are selected");
    assert_eq!(hit.command.id, "note.deactivate");
    // With no notes selected, 0 doesn't reach it.
    let hit = keymap.resolve(
        Platform::Mac,
        Key::Num0,
        none,
        &[Context::NoteEditor, Context::Global],
    );
    assert!(
        hit.is_none_or(|hit| hit.command.id != "note.deactivate"),
        "no notes, no deactivating"
    );
    for id in ["S-NOTE-027", "S-NOTE-028", "S-NOTE-029"] {
        let entry = keymap.entry_for(id).unwrap();
        assert!(matches!(entry.status, Status::Widget(_)), "{id}");
    }
    let entry = keymap.entry_for("S-NOTE-052").unwrap();
    assert!(
        matches!(entry.status, Status::Widget(_)),
        "the Chord Selector Pad handles its own keys: {:?}",
        entry.status
    );
}

// Spec: multi-clip-editing R11, R13. N is Focus Mode's latch (S-NOTE-040) in
// the note editor, and Cmd-D duplicates the loop (S-LOOP-012) when the loop
// brace or loop bars are selected, ahead of the notes' Duplicate.
#[test]
fn n_switches_focus_mode_and_cmd_d_duplicates_the_loop_of_selected_bars() {
    let keymap = Keymap::live12();
    for id in ["S-NOTE-040", "S-LOOP-012"] {
        let entry = keymap.entry_for(id).unwrap();
        assert_eq!(entry.status, Status::Bound, "{id}");
    }
    let none = mods(false, false, false, false);
    let hit = keymap
        .resolve(
            Platform::Mac,
            Key::N,
            none,
            &[Context::NoteEditor, Context::Global],
        )
        .expect("N is bound in the note editor");
    assert_eq!(hit.command.id, "clip_view.focus_mode");
    assert_eq!(hit.entry.trigger, bl_ui::keymap::Trigger::Latch);
    let command = mods(true, false, false, false);
    let contexts = [
        Context::LoopBrace,
        Context::ClipView,
        Context::NoteEditor,
        Context::NoteSelection,
        Context::Global,
    ];
    let hit = keymap
        .resolve(Platform::Mac, Key::D, command, &contexts)
        .expect("Cmd-D is bound");
    assert_eq!(hit.command.id, "clip.duplicate_loop");
    // Without a selected brace or bar it still duplicates the notes.
    let hit = keymap
        .resolve(Platform::Mac, Key::D, command, &contexts[1..])
        .expect("Cmd-D is bound");
    assert_eq!(hit.command.id, "note.duplicate");
}

#[test]
fn the_filter_and_history_shortcuts_reach_their_commands() {
    // Spec: browser-tags R18, R30.
    let keymap = Keymap::live12();
    for (shortcut, mac, win, command) in [
        (
            "S-BRW-013",
            "Cmd+Option+G",
            "Ctrl+Alt+G",
            "browser.set_filter_view",
        ),
        (
            "S-BRW-011",
            "Cmd+LeftBracket",
            "Ctrl+LeftBracket",
            "browser.back",
        ),
        (
            "S-BRW-012",
            "Cmd+RightBracket",
            "Ctrl+RightBracket",
            "browser.forward",
        ),
    ] {
        let entry = keymap.entry_for(shortcut).expect("the shortcut");
        assert_eq!(entry.status, Status::Bound, "{shortcut}");
        assert_eq!(entry.mac, vec![mac.to_owned()]);
        assert_eq!(entry.win, vec![win.to_owned()]);
        assert!(
            entry.commands.iter().any(|c| c.id == command),
            "{shortcut}: {:?}",
            entry.commands
        );
    }
    // Rename (Cmd-R) renames the chosen label while the sidebar has the
    // keyboard (browser-tags R25).
    let rename = keymap.entry_for("S-EDIT-008").expect("the shortcut");
    assert!(
        rename
            .commands
            .iter()
            .any(|c| c.id == "browser.rename_label"),
        "{:?}",
        rename.commands
    );
}

// Spec: note-editing R35 to R41, R47 (WP-6.3). Join, Fit to Time Range and
// Split Clip are bound in the note editor, ahead of the Arrangement's keys
// they share (Consolidate on Cmd-J); the keys that run differently with
// what is selected or held down are the note editor's own.
#[test]
fn the_note_operations_keys_reach_their_commands() {
    let keymap = Keymap::live12();
    let contexts = [
        Context::NoteSelection,
        Context::NoteEditor,
        Context::ClipView,
        Context::Global,
    ];
    for (platform, command, command_alt, command_shift) in [
        (
            Platform::Mac,
            mods(true, false, false, false),
            mods(true, false, true, false),
            mods(true, false, false, true),
        ),
        (
            Platform::Other,
            mods(false, true, false, false),
            mods(false, true, true, false),
            mods(false, true, false, true),
        ),
    ] {
        for (key, mods, expected) in [
            (Key::J, command, "note.join"),
            (Key::J, command_alt, "note.fit_to_time"),
            (Key::E, command_shift, "note.split_clip"),
        ] {
            let hit = keymap
                .resolve(platform, key, mods, &contexts)
                .map(|hit| hit.command.id.clone());
            assert_eq!(hit.as_deref(), Some(expected), "{platform:?} {key:?}");
        }
    }
    for (id, status) in [
        ("S-NOTE-007", "bound"),
        ("S-NOTE-008", "bound"),
        ("S-NOTE-036", "bound"),
    ] {
        let entry = keymap.entry_for(id).unwrap();
        assert_eq!(entry.status, Status::Bound, "{id} is {status}");
    }
    for id in [
        "S-NOTE-003",
        "S-NOTE-004",
        "S-NOTE-005",
        "S-NOTE-006",
        "S-NOTE-047",
    ] {
        let entry = keymap.entry_for(id).unwrap();
        assert!(
            matches!(entry.status, Status::Widget(_)),
            "{id} is read by the note editor: {:?}",
            entry.status
        );
    }
    // Cmd-E isn't taken by anything wider in the note editor.
    let plain = keymap.resolve(
        Platform::Mac,
        Key::E,
        mods(true, false, false, false),
        &contexts,
    );
    assert!(plain.is_none(), "the note editor reads Cmd-E itself");
}

// Spec: mpe R22, R27, R29, R30, R31. The MPE tab's keys reach its commands:
// Option+Shift+3 and Option+Tab the tabs, Cmd+4 the tab's own grid, B Draw
// Mode, Z the zoom, and Cmd+A the breakpoints of an envelope that has the
// keyboard, where without one it selects the notes.
#[test]
fn the_mpe_tabs_keys_reach_its_commands() {
    let keymap = Keymap::live12();
    let tab = [Context::ClipView, Context::NoteEditor, Context::Global];
    for platform in [Platform::Mac, Platform::Other] {
        let command = |key, mods, contexts: &[Context]| {
            keymap.resolve(platform, key, mods, contexts).map(|hit| {
                (
                    hit.command.id.clone(),
                    hit.command.args.get("tab").cloned(),
                    hit.command.args.get("all").cloned(),
                )
            })
        };
        let (cmd, alt, ctrl_or_cmd) = match platform {
            Platform::Mac => (
                mods(true, false, false, false),
                mods(false, false, true, false),
                mods(true, false, false, false),
            ),
            Platform::Other => (
                mods(false, true, false, false),
                mods(false, false, true, false),
                mods(false, true, false, false),
            ),
        };
        let option_shift = Mods {
            alt: true,
            shift: true,
            ..Mods::default()
        };
        let next = match platform {
            Platform::Mac => alt,
            Platform::Other => mods(false, true, false, false),
        };
        let (id, tab_arg, _) = command(Key::Num3, option_shift, &tab).expect("Option+Shift+3");
        assert_eq!(
            (id.as_str(), tab_arg),
            ("view.clip_tab", Some("mpe".into()))
        );
        // Option+Tab cycles from any of the three tabs, the Envelopes
        // tab's contexts among them.
        for contexts in [
            &tab[..],
            &[Context::ClipView, Context::Envelope, Context::Global],
        ] {
            let (id, ..) = command(Key::Tab, next, contexts).expect("Option+Tab");
            assert_eq!(id, "view.clip_tab", "{platform:?} {contexts:?}");
        }
        assert_eq!(command(Key::Num4, cmd, &tab).unwrap().0, "view.mpe_grid");
        // In the Arrangement the same keys are Snap to Grid.
        assert_eq!(
            command(Key::Num4, cmd, &[Context::Arrangement, Context::Global])
                .unwrap()
                .0,
            "arrangement.snap_to_grid"
        );
        assert_eq!(
            command(Key::B, Mods::default(), &tab).unwrap().0,
            "view.draw_mode"
        );
        // Spec: mpe R31, piano-roll R32. Z zooms to the time selection and,
        // in the MPE tab, the rows its Pitch values reach.
        assert_eq!(
            command(Key::Z, Mods::default(), &tab).unwrap().0,
            "clip_view.note_view"
        );
        let (id, _, all) = command(Key::A, ctrl_or_cmd, &tab).expect("Cmd+A");
        assert_eq!(
            (id.as_str(), all),
            ("clip_view.select_expression_breakpoints", Some(true.into())),
            "{platform:?}"
        );
        // Without an envelope with the keyboard the shell refuses it, and
        // Cmd+A selects the notes.
        let notes = keymap
            .resolve_with(platform, Key::A, ctrl_or_cmd, &tab, |hit| {
                hit.command.id != "clip_view.select_expression_breakpoints"
            })
            .expect("Cmd+A without an envelope");
        assert_eq!(notes.command.id, "clip_view.select_notes", "{platform:?}");
    }
}

// Spec: piano-roll R32 to R38 (WP-6.54). Z, X, W and H, Option-Shift-F and
// Cmd-F9 to Cmd-F12 are bound to commands; the keys with two directions or a
// pointer (Page keys, Plus and Minus, the region move, Cmd-click) are read
// by the note editor itself.
#[test]
fn the_note_editors_view_keys_and_clip_marker_keys_are_all_handled() {
    let keymap = Keymap::live12();
    let contexts = [Context::ClipView, Context::NoteEditor, Context::Global];
    let none = mods(false, false, false, false);
    for (key, show) in [
        (Key::Z, "selection"),
        (Key::X, "back"),
        (Key::W, "whole"),
        (Key::H, "height"),
    ] {
        let hit = keymap
            .resolve(Platform::Mac, key, none, &contexts)
            .unwrap_or_else(|| panic!("{key:?} is bound in the note editor"));
        assert_eq!(hit.command.id, "clip_view.note_view", "{key:?}");
        assert_eq!(hit.command.args["show"], show, "{key:?}");
    }
    // Option-Shift-F (Alt-Shift-F on Windows) follows playback.
    let hit = keymap
        .resolve(
            Platform::Mac,
            Key::F,
            mods(false, false, true, true),
            &contexts,
        )
        .expect("Option-Shift-F");
    assert_eq!(hit.command.id, "arrangement.follow");
    // Cmd-F9 to Cmd-F12 set the clip's markers (Ctrl on Windows).
    for (platform, command) in [
        (Platform::Mac, mods(true, false, false, false)),
        (Platform::Other, mods(false, true, false, false)),
    ] {
        for (key, marker) in [
            (Key::F9, "start"),
            (Key::F10, "loop_start"),
            (Key::F11, "loop_end"),
            (Key::F12, "end"),
        ] {
            let hit = keymap
                .resolve(platform, key, command, &contexts)
                .unwrap_or_else(|| panic!("{key:?} sets a marker on {platform:?}"));
            assert_eq!(hit.command.id, "clip_view.set_marker", "{key:?}");
            assert_eq!(hit.command.args["marker"], marker, "{key:?}");
        }
    }
    for id in [
        "S-NOTE-009",
        "S-NOTE-010",
        "S-NOTE-021",
        "S-NOTE-023",
        "S-NOTE-024",
        "S-LOOP-001",
        "S-LOOP-002",
        "S-LOOP-003",
        "S-LOOP-004",
    ] {
        assert_eq!(keymap.entry_for(id).unwrap().status, Status::Bound, "{id}");
    }
    for id in [
        "S-NOTE-013",
        "S-NOTE-014",
        "S-NOTE-015",
        "S-NOTE-022",
        "S-NOTE-025",
        "S-LOOP-005",
        "S-LOOP-006",
    ] {
        assert!(
            matches!(keymap.entry_for(id).unwrap().status, Status::Widget(_)),
            "{id} is read by the note editor"
        );
    }
}
