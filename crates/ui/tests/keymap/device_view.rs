//! The Device View and rack keys: Float, fades, Group, chain visibility, hot-swap.

use bl_commands::Context;
use bl_ui::keymap::{Keymap, Platform, Status};
use egui::Key;

use crate::keymap::helpers::*;

// Spec: device-chains R26. Float's key is scoped by focus: with the
// Device View (or a floating device's window) focused it floats or docks
// the selected device; with the Arrangement focused the same keys are
// Create Fade/Crossfade's (S-ARR-008).
#[test]
fn float_s_key_reaches_float_in_the_device_view_and_fades_in_the_arrangement() {
    let keymap = Keymap::live12();
    let devices = [Context::DeviceView, Context::Global];
    let arrangement = [Context::Arrangement, Context::Global];
    let hit = |platform, m, contexts: &[Context]| {
        keymap
            .resolve(platform, Key::F, m, contexts)
            .map(|hit| hit.command.id.clone())
    };
    let ctrl_alt = mods(false, true, true, false);
    let option = mods(false, false, true, false);
    let cmd_option = mods(true, false, true, false);
    for (platform, float, fade) in [
        (Platform::Other, ctrl_alt, ctrl_alt),
        (Platform::Mac, option, cmd_option),
    ] {
        assert_eq!(
            hit(platform, float, &devices).as_deref(),
            Some("device.set_floating"),
            "{platform:?}: Float in the Device View"
        );
        assert_ne!(
            hit(platform, fade, &arrangement).as_deref(),
            Some("device.set_floating"),
            "{platform:?}: not Float in the Arrangement"
        );
        // The same keys are the fade shortcut's in the Arrangement.
        let fades = keymap.entry_for("S-ARR-008").unwrap();
        assert!(
            fades
                .chords(platform)
                .iter()
                .any(|chord| chord.matches(Key::F, fade)),
            "{platform:?}"
        );
        assert_eq!(fades.context, "arrangement");
    }
    // Cmd-Option-F on macOS is the fades', not Float's.
    assert_eq!(hit(Platform::Mac, cmd_option, &devices), None);
}

// Spec: audio-clips R27 to R29. The fade keys are bound: Cmd-Option-F and
// Cmd-Option-Delete (Ctrl-Alt-F and Ctrl-Alt-Backspace) run Create Fade and
// Delete Fades in the Arrangement, and F held shows the fade handles, which
// the Arrangement View reads itself (S-ARR-010, S-ENV-004).
#[test]
fn the_fade_keys_are_bound() {
    let keymap = Keymap::live12();
    let command_of = |id: &str| {
        let entry = keymap.entry_for(id).unwrap();
        assert_eq!(entry.context, "arrangement", "{id}");
        match &entry.status {
            Status::Bound => Some(entry.commands[0].id.clone()),
            Status::Widget(handler) => {
                assert!(handler.contains("fade handles"), "{id}: {handler}");
                None
            }
            other => panic!("{id} is {other:?}"),
        }
    };
    assert_eq!(
        command_of("S-ARR-008").as_deref(),
        Some("arrangement.create_fade")
    );
    assert_eq!(
        command_of("S-ARR-009").as_deref(),
        Some("arrangement.delete_fades")
    );
    assert_eq!(command_of("S-ARR-010"), None);
    assert_eq!(command_of("S-ENV-004"), None);
    // The keys resolve to them in the Arrangement.
    let arrangement = [Context::Arrangement, Context::Global];
    for (platform, create, delete, delete_key) in [
        (
            Platform::Mac,
            mods(true, false, true, false),
            mods(true, false, true, false),
            Key::Delete,
        ),
        (
            Platform::Other,
            mods(false, true, true, false),
            mods(false, true, true, false),
            Key::Backspace,
        ),
    ] {
        let hit = |key, m| {
            keymap
                .resolve(platform, key, m, &arrangement)
                .map(|hit| hit.command.id.clone())
        };
        assert_eq!(
            hit(Key::F, create).as_deref(),
            Some("arrangement.create_fade"),
            "{platform:?}"
        );
        assert_eq!(
            hit(delete_key, delete).as_deref(),
            Some("arrangement.delete_fades"),
            "{platform:?}"
        );
    }
}

// Spec: time-editing R21. Insert Silence and the …Time commands have their
// keys in the Arrangement (S-ARR-014 to S-ARR-019), which Shift with Cut,
// Copy, Paste, Duplicate and Delete is (S-EDIT-015).
#[test]
fn the_time_command_keys_reach_their_commands_in_the_arrangement() {
    let keymap = Keymap::live12();
    let arrangement = [Context::Arrangement, Context::Global];
    for (shortcut, command, key) in [
        ("S-ARR-014", "arrangement.insert_silence", Key::I),
        ("S-ARR-015", "arrangement.cut_time", Key::X),
        ("S-ARR-016", "arrangement.copy_time", Key::C),
        ("S-ARR-017", "arrangement.paste_time", Key::V),
        ("S-ARR-018", "arrangement.duplicate_time", Key::D),
        ("S-ARR-019", "arrangement.delete_time", Key::Delete),
    ] {
        let entry = keymap.entry_for(shortcut).unwrap();
        assert_eq!(entry.status, Status::Bound, "{shortcut}");
        assert_eq!(entry.context, "arrangement", "{shortcut}");
        assert_eq!(entry.commands[0].id, command, "{shortcut}");
        let (mac, other) = (
            mods(true, false, false, shortcut != "S-ARR-014"),
            mods(false, true, false, shortcut != "S-ARR-014"),
        );
        for (platform, held) in [(Platform::Mac, mac), (Platform::Other, other)] {
            let hit = keymap.resolve(platform, key, held, &arrangement);
            assert_eq!(
                hit.map(|hit| hit.command.id.clone()).as_deref(),
                Some(command),
                "{shortcut} on {platform:?}"
            );
        }
    }
    match &keymap.entry_for("S-EDIT-015").unwrap().status {
        Status::Widget(handler) => {
            for command in [
                "arrangement.cut_time",
                "arrangement.copy_time",
                "arrangement.paste_time",
                "arrangement.duplicate_time",
                "arrangement.delete_time",
            ] {
                assert!(handler.contains(command), "{handler}");
            }
        }
        other => panic!("S-EDIT-015 is {other:?}"),
    }
}

// Spec: comping R15, R28. Every S-COMP shortcut is bound: the take-lanes
// context's keys run the comp commands, Cmd-D still duplicates time or clips
// on a take lane, and the two with two directions are the Arrangement's own.
#[test]
fn every_take_lane_shortcut_is_bound() {
    let keymap = Keymap::live12();
    for id in [
        "S-COMP-001",
        "S-COMP-002",
        "S-COMP-003",
        "S-COMP-004",
        "S-COMP-005",
        "S-COMP-006",
        "S-COMP-007",
        "S-COMP-008",
    ] {
        let entry = keymap.entry_for(id).unwrap_or_else(|| panic!("{id}"));
        assert!(
            matches!(entry.status, Status::Bound | Status::Widget(_)),
            "{id} is {:?}",
            entry.status
        );
    }
    let lanes = [Context::TakeLanes, Context::Arrangement, Context::Global];
    let arrangement = [Context::Arrangement, Context::Global];
    for platform in [Platform::Mac, Platform::Other] {
        let none = mods(false, false, false, false);
        let (command, command_alt) = match platform {
            Platform::Mac => (
                mods(true, false, false, false),
                mods(true, false, true, false),
            ),
            Platform::Other => (
                mods(false, true, false, false),
                mods(false, true, true, false),
            ),
        };
        let hit = |contexts: &[Context], key, m| {
            keymap
                .resolve(platform, key, m, contexts)
                .map(|hit| (hit.command.id.clone(), hit.command.args.len()))
        };
        assert_eq!(
            hit(&arrangement, Key::U, command_alt)
                .map(|h| h.0)
                .as_deref(),
            Some("arrangement.show_take_lanes"),
            "{platform:?}"
        );
        assert_eq!(
            hit(&lanes, Key::Enter, none).map(|h| h.0).as_deref(),
            Some("take_lane.copy_to_main_lane")
        );
        assert_eq!(
            hit(&lanes, Key::T, none).map(|h| h.0).as_deref(),
            Some("take_lane.audition")
        );
        assert_eq!(
            hit(&lanes, Key::ArrowLeft, none),
            Some(("arrangement.show_take_lanes".to_owned(), 2)),
            "Left fixes `on` and `title_bars`"
        );
        // Cmd-D: the Arrangement's duplicate or the take lanes' own.
        assert!(hit(&lanes, Key::D, command).is_some());
        // The keys with two directions are the view's, in the take-lanes
        // context, and stop wider contexts from running a command for them.
        assert_eq!(hit(&lanes, Key::ArrowUp, command), None);
        assert_eq!(hit(&lanes, Key::ArrowDown, command), None);
    }
}

// Spec: racks R9, R11. Group and Ungroup have their keys in the Device View
// (S-DEV-001, S-DEV-002), and the Edit menu's items reach them there.
#[test]
fn the_group_and_ungroup_keys_reach_their_commands_in_the_device_view() {
    let keymap = Keymap::live12();
    let devices = [Context::DeviceView, Context::Global];
    let hit = |platform, m, contexts: &[Context]| {
        keymap
            .resolve(platform, Key::G, m, contexts)
            .map(|hit| hit.command.id.clone())
    };
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
        assert_eq!(
            hit(platform, group, &devices).as_deref(),
            Some("device.group"),
            "{platform:?}"
        );
        assert_eq!(
            hit(platform, ungroup, &devices).as_deref(),
            Some("device.ungroup"),
            "{platform:?}"
        );
        // Elsewhere the same keys are the Arrangement's (Group Tracks).
        assert_ne!(
            hit(platform, group, &[Context::Arrangement, Context::Global]).as_deref(),
            Some("device.group"),
            "{platform:?}"
        );
    }
}

// Spec: racks R20. Plus shows the selected rack's chain list and Minus hides
// it (S-DEV-012, S-DEV-013); Minus is Fold (S-DEV-014) where the list isn't
// shown.
#[test]
fn plus_and_minus_show_and_hide_chains_and_minus_folds_otherwise() {
    let keymap = Keymap::live12();
    let devices = [Context::DeviceView, Context::Global];
    for platform in [Platform::Mac, Platform::Other] {
        let plus = keymap
            .resolve(
                platform,
                Key::Plus,
                mods(false, false, false, false),
                &devices,
            )
            .unwrap();
        assert_eq!(plus.entry.id, "S-DEV-012");
        assert_eq!(plus.command.id, "rack.set_view");
        assert_eq!(plus.command.args["view"], "chain_list");
        assert_eq!(plus.command.args["shown"], true);
        // `=` carries Plus on most keyboards.
        assert_eq!(
            keymap
                .resolve(
                    platform,
                    Key::Equals,
                    mods(false, false, false, false),
                    &devices
                )
                .unwrap()
                .entry
                .id,
            "S-DEV-012"
        );
        let minus = |runs: &dyn Fn(&str) -> bool| {
            keymap
                .resolve_with(
                    platform,
                    Key::Minus,
                    mods(false, false, false, false),
                    &devices,
                    |hit| runs(&hit.command.id),
                )
                .map(|hit| (hit.entry.id.clone(), hit.command.id.clone()))
        };
        // While the chain list shows, Hide Chains runs; where it can't, Fold.
        assert_eq!(
            minus(&|_| true),
            Some(("S-DEV-013".to_owned(), "rack.set_view".to_owned()))
        );
        assert_eq!(
            minus(&|id| id == "device.fold"),
            Some(("S-DEV-014".to_owned(), "device.fold".to_owned()))
        );
        // With neither able, the first still decides, so the key is refused.
        assert_eq!(minus(&|_| false).unwrap().0, "S-DEV-013");
    }
}

// Spec: racks R22. S solos the selected chain while a chain list has the
// focus (S-SES-023), and the edit keys act on the chains there.
#[test]
fn s_solos_the_selected_chain() {
    let keymap = Keymap::live12();
    let chains = [Context::RackChains, Context::DeviceView, Context::Global];
    let plain = mods(false, false, false, false);
    for platform in [Platform::Mac, Platform::Other] {
        let hit = |key, m| {
            keymap
                .resolve(platform, key, m, &chains)
                .map(|hit| hit.command.id.clone())
        };
        assert_eq!(hit(Key::S, plain).as_deref(), Some("chain.solo"));
        assert_eq!(hit(Key::Delete, plain).as_deref(), Some("chain.delete"));
        // Without the chain list's focus, S is not the chain's.
        assert_ne!(
            keymap
                .resolve(
                    platform,
                    Key::S,
                    plain,
                    &[Context::DeviceView, Context::Global]
                )
                .map(|hit| hit.command.id.clone())
                .as_deref(),
            Some("chain.solo")
        );
        let command = if platform == Platform::Mac {
            mods(true, false, false, false)
        } else {
            mods(false, true, false, false)
        };
        for (key, id) in [
            (Key::C, "chain.copy"),
            (Key::X, "chain.cut"),
            (Key::V, "chain.paste"),
            (Key::D, "chain.duplicate"),
            (Key::R, "chain.rename"),
        ] {
            assert_eq!(hit(key, command).as_deref(), Some(id), "{key:?}");
        }
    }
}

// Spec: drum-racks R18. D switches between a Drum Rack and its last
// selected pad (S-VIEW-007), wherever the keys are read, on both platforms.
#[test]
fn d_switches_hot_swap_between_the_rack_and_its_pad() {
    let keymap = Keymap::live12();
    for platform in [Platform::Mac, Platform::Other] {
        for contexts in [
            vec![Context::Global],
            vec![Context::DeviceView, Context::Global],
        ] {
            let hit = keymap
                .resolve(
                    platform,
                    Key::D,
                    mods(false, false, false, false),
                    &contexts,
                )
                .map(|hit| hit.command.id.clone());
            assert_eq!(
                hit.as_deref(),
                Some("rack.switch_hot_swap_target"),
                "{platform:?} {contexts:?}"
            );
        }
    }
    let entry = keymap.entry_for("S-VIEW-007").unwrap();
    assert_eq!(entry.status, Status::Bound);
}

// Spec: racks R44. Q switches hot-swap on and off for the selected device
// (S-DEV-008 in the Device View, S-VIEW-006 elsewhere).
#[test]
fn q_toggles_hot_swap() {
    let keymap = Keymap::live12();
    let plain = mods(false, false, false, false);
    for platform in [Platform::Mac, Platform::Other] {
        for contexts in [
            &[Context::DeviceView, Context::Global][..],
            &[Context::Browser, Context::Global][..],
        ] {
            let hit = keymap.resolve(platform, Key::Q, plain, contexts);
            assert_eq!(
                hit.map(|hit| hit.command.id.clone()).as_deref(),
                Some("device.hot_swap"),
                "{platform:?} in {contexts:?}"
            );
        }
    }
}
