//! The keymap test (the architecture, "The keymap"; build plan, "Every UI
//! package"): every shortcut in the UI parity reference is in the default
//! keymap with the reference's keys, and is bound to a registered command,
//! handled by the platform or the focused control, or marked with the
//! milestone (and for M1 the package) that binds it. The shortcuts of the
//! views the shell builds are bound.

use std::path::PathBuf;

use bl_commands::{Context, Registry};
use bl_ui::keymap::{Keymap, Mods, Platform, Status, parse_binding};
use egui::Key;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A shortcut from the shortcut reference.
#[derive(Debug, serde::Deserialize)]
struct Reference {
    id: String,
    context: String,
    kind: String,
    trigger: String,
    milestone: String,
    mac: Vec<String>,
    win: Vec<String>,
}

#[derive(serde::Deserialize)]
struct ReferenceFile {
    shortcut: Vec<Reference>,
}

/// The reference's shortcuts, or `None` in a checkout without the planning
/// documents (they aren't published yet), where the test skips.
fn reference() -> Option<Vec<Reference>> {
    let docs = repo().join("docs");
    if !docs.exists() {
        eprintln!(
            "skipped: the UI parity reference isn't published yet, and this checkout doesn't have it"
        );
        return None;
    }
    let text = std::fs::read_to_string(docs.join("ui-parity/shortcuts.toml")).unwrap();
    let file: ReferenceFile = toml::from_str(&text).unwrap();
    Some(file.shortcut)
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    bl_model::registry::register(&mut registry).unwrap();
    bl_shell::register(&mut registry).unwrap();
    // The transport's commands (WP-1.5), which the playback keys bind.
    bl_engine::transport::register(&mut registry).unwrap();
    // The Session View's launching and stopping (WP-3.2), which its grid
    // runs and its keys reach.
    bl_engine::session::register(&mut registry).unwrap();
    registry
}

/// The shortcuts of the views and windows the shell (WP-1.7) builds, which
/// must be bound.
const SHELL: &[&str] = &[
    "S-FILE-001",
    "S-FILE-002",
    "S-FILE-003",
    "S-FILE-004",
    "S-FILE-005",
    "S-FILE-006",
    "S-FILE-009",
    "S-VIEW-001",
    "S-VIEW-003",
    "S-VIEW-010",
    "S-BRW-002",
    "S-VIEW-019",
    "S-VIEW-020",
    "S-VIEW-021",
    "S-VIEW-004",
    "S-VIEW-005",
    "S-VIEW-015",
    "S-VIEW-016",
    "S-VIEW-022",
    "S-NOTE-030",
    "S-FOCUS-004",
    "S-FOCUS-005",
    "S-FOCUS-001",
    "S-FOCUS-003",
    "S-FOCUS-011",
    "S-FOCUS-012",
    "S-ZOOM-001",
    "S-ZOOM-002",
    "S-ENGINE-001",
    "S-SET-001",
    "S-SET-002",
    "S-SET-005",
    "S-SET-006",
    "S-SET-007",
    "S-EDIT-006",
    "S-EDIT-007",
    "S-TRACK-001",
];

/// Everything wrong with a keymap against the reference and the registry.
fn problems(keymap: &Keymap, reference: &[Reference], registry: &Registry) -> Vec<String> {
    let mut problems = Vec::new();
    let entries = keymap.entries();
    if entries.len() != reference.len() {
        problems.push(format!(
            "the keymap has {} shortcuts and the reference {}",
            entries.len(),
            reference.len()
        ));
    }
    for (entry, source) in entries.iter().zip(reference) {
        let id = &source.id;
        if entry.id != *id
            || entry.context != source.context
            || entry.kind != source.kind
            || entry.milestone != source.milestone
            || entry.mac != source.mac
            || entry.win != source.win
            || format!("{:?}", entry.trigger).to_lowercase() != source.trigger
        {
            problems.push(format!("{id}: differs from the reference"));
            continue;
        }
        match &entry.status {
            Status::Bound => {
                for command in &entry.commands {
                    let Some(spec) = registry.get(&command.id) else {
                        problems.push(format!("{id}: runs {}, which isn't registered", command.id));
                        continue;
                    };
                    if !spec.bindings.iter().any(|b| b.shortcut == *id) {
                        problems.push(format!("{id}: {} doesn't bind it", command.id));
                    }
                    if command.contexts.is_empty() {
                        problems.push(format!("{id}: {} runs in no context", command.id));
                    }
                }
                if entry.kind == "key" {
                    for (platform, bindings) in
                        [(Platform::Mac, &entry.mac), (Platform::Other, &entry.win)]
                    {
                        for binding in bindings {
                            if parse_binding(binding, platform).is_none() {
                                problems.push(format!("{id}: `{binding}` can't be matched"));
                            }
                        }
                    }
                }
            }
            Status::System(how) | Status::Widget(how) => {
                if how.is_empty() {
                    problems.push(format!("{id}: says nothing about who handles it"));
                }
            }
            Status::Pending { milestone, package } => {
                if *milestone != source.milestone {
                    problems.push(format!(
                        "{id}: pending for {milestone}, not {}",
                        source.milestone
                    ));
                }
                match (source.milestone.as_str(), package.as_deref()) {
                    ("M1", None) => problems.push(format!("{id}: an M1 shortcut with no package")),
                    ("M1", Some("WP-1.7")) => {
                        problems.push(format!("{id}: the shell's own, but unbound"));
                    }
                    _ => {}
                }
            }
        }
    }
    for id in SHELL {
        if keymap
            .entry_for(id)
            .is_none_or(|e| e.status != Status::Bound)
        {
            problems.push(format!(
                "{id}: the shell builds its view, but it isn't bound"
            ));
        }
    }
    problems
}

#[test]
fn every_shortcut_is_bound_or_marked_with_the_milestone_that_binds_it() {
    let keymap = Keymap::live12();
    let Some(reference) = reference() else {
        return;
    };
    let problems = problems(&keymap, &reference, &registry());
    assert!(problems.is_empty(), "{problems:#?}");
    let bound = keymap
        .entries()
        .iter()
        .filter(|e| e.status == Status::Bound)
        .count();
    assert!(bound >= SHELL.len(), "{bound} bound");
}

#[test]
fn the_test_fails_when_a_shortcut_is_neither_bound_nor_marked() {
    // Unbind Save and take away its package: the test must notice.
    let text = bl_ui::keymap::LIVE12.replace(
        "status = \"bound\"\ncommands = [\n  { id = \"set.save\", contexts = [\"global\"] },\n]",
        "status = \"pending\"",
    );
    assert_ne!(text, bl_ui::keymap::LIVE12, "the replacement applied");
    let keymap = Keymap::parse(&text).unwrap();
    let Some(reference) = reference() else {
        return;
    };
    let problems = problems(&keymap, &reference, &registry());
    assert!(
        problems
            .iter()
            .any(|p| p.contains("S-FILE-003: an M1 shortcut with no package")),
        "{problems:#?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("S-FILE-003: the shell builds"))
    );
}

fn mods(cmd: bool, ctrl: bool, alt: bool, shift: bool) -> Mods {
    Mods {
        ctrl,
        alt,
        shift,
        cmd,
    }
}

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

#[test]
fn keys_resolve_to_commands_in_their_contexts() {
    // Spec: app-shell R16.
    let keymap = Keymap::live12();
    let main = [Context::Arrangement, Context::Global];
    let resolve = |platform, key, m, contexts: &[Context]| {
        keymap
            .resolve(platform, key, m, contexts)
            .map(|hit| hit.command.id.clone())
    };
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::S,
            mods(true, false, false, false),
            &main
        )
        .as_deref(),
        Some("set.save")
    );
    assert_eq!(
        resolve(
            Platform::Other,
            Key::S,
            mods(false, true, false, false),
            &main
        )
        .as_deref(),
        Some("set.save")
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::S,
            mods(false, true, false, false),
            &main
        ),
        None,
        "Ctrl+S isn't Save on macOS"
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Comma,
            mods(true, false, false, false),
            &main
        )
        .as_deref(),
        Some("app.settings")
    );
    assert_eq!(
        resolve(
            Platform::Other,
            Key::Y,
            mods(false, true, false, false),
            &main
        )
        .as_deref(),
        Some("edit.redo")
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Equals,
            mods(true, false, false, false),
            &main
        )
        .as_deref(),
        Some("view.zoom_in")
    );
    assert_eq!(
        resolve(Platform::Mac, Key::B, mods(true, false, true, false), &main).as_deref(),
        Some("view.toggle_browser")
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Num0,
            mods(false, false, true, false),
            &main
        )
        .as_deref(),
        Some("focus.control_bar")
    );
    // Tab switches views unless Tab moves the focus.
    assert_eq!(
        resolve(Platform::Mac, Key::Tab, Mods::default(), &main).as_deref(),
        Some("view.toggle_session_arrangement")
    );
    let tab_focus = [Context::Arrangement, Context::TabFocus, Context::Global];
    assert_eq!(
        resolve(Platform::Mac, Key::Tab, Mods::default(), &tab_focus).as_deref(),
        Some("focus.next")
    );
    // The same keys run different commands in different places: in the
    // Arrangement, Delete deletes the selection, clips or time (WP-1.8b).
    assert_eq!(
        resolve(Platform::Mac, Key::Delete, Mods::default(), &main).as_deref(),
        Some("arrangement.delete")
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Delete,
            Mods::default(),
            &[Context::DeviceView, Context::Global]
        )
        .as_deref(),
        Some("device.remove")
    );
    let settings = [Context::Settings, Context::Dialog, Context::Global];
    assert_eq!(
        resolve(Platform::Mac, Key::Escape, Mods::default(), &settings).as_deref(),
        Some("window.close")
    );
    assert_eq!(
        resolve(Platform::Mac, Key::Tab, Mods::default(), &settings).as_deref(),
        Some("focus.next")
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Tab,
            mods(false, false, true, false),
            &settings
        )
        .as_deref(),
        Some("settings.next_page")
    );
    assert_eq!(
        resolve(
            Platform::Other,
            Key::Tab,
            mods(false, true, false, false),
            &settings
        )
        .as_deref(),
        Some("settings.next_page")
    );
    // Esc closes nothing in the main window: in the Arrangement it goes to
    // the highlighted track's title bar (S-ARR-034, WP-1.8a).
    assert_eq!(
        resolve(Platform::Mac, Key::Escape, Mods::default(), &main).as_deref(),
        Some("arrangement.focus_title_bar"),
        "Esc closes nothing in the main window"
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Escape,
            Mods::default(),
            &[Context::Global]
        ),
        None,
        "Esc closes nothing in the main window"
    );
    // Cmd/Ctrl-W closes the frontmost window (S-FILE-009); with only the
    // main window open the shell disables it.
    for (platform, m) in [
        (Platform::Mac, mods(true, false, false, false)),
        (Platform::Other, mods(false, true, false, false)),
    ] {
        assert_eq!(
            resolve(platform, Key::W, m, &settings).as_deref(),
            Some("window.close")
        );
    }
}

#[test]
fn platform_items_and_corrected_keys_follow_the_reference() {
    let keymap = Keymap::live12();
    // macOS's standard menus provide Hide Others and Emoji & Symbols with
    // their keys.
    for id in ["S-FILE-010", "S-MENU-006"] {
        let entry = keymap.entry_for(id).unwrap();
        assert!(matches!(entry.status, Status::System(_)), "{id}");
    }
    // The video window's keys as Live 12.4.3's View menu shows them, not
    // the manual's (S-VIEW-009, corrected in WP-0.13b). It comes in M9.
    let video = keymap.entry_for("S-VIEW-009").unwrap();
    assert_eq!(video.mac, ["Cmd+Option+Shift+V"]);
    assert_eq!(video.win, ["Ctrl+Alt+Shift+V"]);
    assert!(matches!(video.status, Status::Pending { .. }));
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

// Spec: note-editing R14, R15, R18. 0 deactivates the selected notes through
// the keymap (S-NOTE-041); the keys that read a number or a direction are the
// note editor's, and Cmd-Up and Cmd-Down with the Chord Selector Pad focused
// (S-NOTE-052) wait for Stacks.
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
        matches!(entry.status, Status::Pending { .. }),
        "the Chord Selector Pad's keys wait for Stacks: {:?}",
        entry.status
    );
}

#[test]
fn the_tags_shortcuts_reach_their_commands() {
    // Spec: browser-tags R10 and R14.
    let keymap = Keymap::live12();
    // Cmd-Shift-E shows and hides the Tag Editor (S-BRW-014).
    let entry = keymap.entry_for("S-BRW-014").expect("the shortcut");
    assert_eq!(entry.status, Status::Bound);
    assert_eq!(entry.mac, vec!["Cmd+Shift+E".to_owned()]);
    assert_eq!(entry.win, vec!["Ctrl+Shift+E".to_owned()]);
    assert!(
        entry
            .commands
            .iter()
            .any(|command| command.id == "browser.show_tag_editor"),
        "{:?}",
        entry.commands
    );
    // Cmd-E moves the keyboard focus to Quick Tags' Add field
    // (S-BRW-015): the browser's own widget reads it.
    let entry = keymap.entry_for("S-BRW-015").expect("the shortcut");
    assert!(
        matches!(entry.status, Status::Widget(_)),
        "{:?}",
        entry.status
    );
    assert_eq!(entry.mac, vec!["Cmd+E".to_owned()]);
    assert_eq!(entry.win, vec!["Ctrl+E".to_owned()]);
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

// Spec: detail-views R1, R2, R5, R6, R11, R14. The detail views' seven
// shortcuts, and Cmd+Option+3, are bound to the commands that carry them
// out, on both platforms, in the contexts the spec gives.
#[test]
fn the_detail_views_shortcuts_reach_their_commands() {
    let keymap = Keymap::live12();
    for (id, command, contexts) in [
        (
            "S-VIEW-004",
            "view.switch_detail_view",
            &[Context::Global][..],
        ),
        (
            "S-VIEW-005",
            "view.show_detail_views",
            &[Context::Global][..],
        ),
        (
            "S-VIEW-015",
            "view.toggle_clip_view",
            &[Context::Global][..],
        ),
        (
            "S-VIEW-016",
            "view.toggle_device_view",
            &[Context::Global][..],
        ),
        (
            "S-VIEW-022",
            "view.toggle_detail_area",
            &[Context::Global][..],
        ),
        (
            "S-NOTE-030",
            "view.expand_clip_view",
            &[Context::ClipView][..],
        ),
        ("S-FOCUS-004", "focus.clip_view", &[Context::Global][..]),
        ("S-FOCUS-005", "focus.device_view", &[Context::Global][..]),
    ] {
        let entry = keymap.entry_for(id).unwrap();
        assert_eq!(entry.status, Status::Bound, "{id}");
        assert_eq!(entry.commands.len(), 1, "{id}");
        assert_eq!(entry.commands[0].id, command, "{id}");
        assert_eq!(entry.commands[0].contexts, contexts, "{id}");
    }
    let plain = mods(false, false, false, false);
    for platform in [Platform::Mac, Platform::Other] {
        // Shift+Tab and F12 switch the views in the global context, which
        // Tab's own setting (the `tab-focus` context) comes before.
        let at = |key, m, contexts: &[Context]| {
            keymap
                .resolve(platform, key, m, contexts)
                .map(|hit| hit.command.id.clone())
        };
        let shift = mods(false, false, false, true);
        assert_eq!(
            at(Key::F12, plain, &[Context::Global]).as_deref(),
            Some("view.switch_detail_view")
        );
        assert_eq!(
            at(Key::Tab, shift, &[Context::Global]).as_deref(),
            Some("view.switch_detail_view")
        );
        assert_eq!(
            at(Key::Tab, shift, &[Context::TabFocus, Context::Global]).as_deref(),
            Some("focus.previous")
        );
        // Expand Clip View is the Clip View's own key.
        let (cmd, ctrl) = match platform {
            Platform::Mac => (true, false),
            Platform::Other => (false, true),
        };
        let expand = mods(cmd, ctrl, true, false);
        assert_eq!(
            at(Key::E, expand, &[Context::ClipView, Context::Global]).as_deref(),
            Some("view.expand_clip_view")
        );
        assert_eq!(
            at(Key::E, expand, &[Context::DeviceView, Context::Global]),
            None
        );
    }
}

// Spec: track-context-menu R16. The whole-track commands have their keys
// on the track headers, and the Arrangement's own edits keep them where
// clips or time are selected.
#[test]
fn the_track_headers_keys_reach_the_track_commands() {
    let keymap = Keymap::live12();
    let headers = [Context::TrackHeaders, Context::Arrangement, Context::Global];
    let arrangement = [Context::Arrangement, Context::Global];
    // The Session View has no title-bar keys (R16: track headers only).
    let session = [Context::Session, Context::Global];
    let resolve = |platform, key, m, contexts: &[Context]| {
        keymap
            .resolve(platform, key, m, contexts)
            .map(|hit| hit.command.id.clone())
    };
    for (platform, cmd) in [
        (Platform::Mac, mods(true, false, false, false)),
        (Platform::Other, mods(false, true, false, false)),
    ] {
        for (key, command, edit) in [
            (Key::X, "track.cut", "arrangement.cut"),
            (Key::C, "track.copy", "arrangement.copy"),
            (Key::V, "track.paste", "arrangement.paste"),
            (Key::D, "track.duplicate", "arrangement.duplicate"),
        ] {
            assert_eq!(
                resolve(platform, key, cmd, &headers).as_deref(),
                Some(command),
                "{platform:?}: {command}"
            );
            assert_eq!(
                resolve(platform, key, cmd, &arrangement).as_deref(),
                Some(edit),
                "{platform:?}: {edit}"
            );
            assert_ne!(
                resolve(platform, key, cmd, &session).as_deref(),
                Some(command),
                "{platform:?}: {command} is not bound in the Session View"
            );
        }
    }
}
