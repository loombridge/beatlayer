//! The menu bar against the UI parity reference: every row of
//! `docs/ui-parity/menus.md` is in the model in Live's order, and every item
//! reaches a registered command or is marked pending (D16, D17).

use std::collections::BTreeSet;
use std::path::PathBuf;

use bl_commands::Registry;
use bl_shell::menus::{Action, MenuPlatform, menu_bar};
use bl_shell::{Shell, commands};
use serde_json::json;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Each menu-bar section of menus.md, by heading, with its row IDs in order.
fn reference_menus() -> Vec<(String, Vec<String>)> {
    let text = std::fs::read_to_string(repo().join("docs/ui-parity/menus.md")).unwrap();
    let mut menus: Vec<(String, Vec<String>)> = Vec::new();
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if heading.starts_with("Context menus") {
                break;
            }
            menus.push((heading.to_owned(), Vec::new()));
        } else if let Some(rest) = line.strip_prefix("| M-")
            && let Some((menu, rows)) = menus.last_mut().map(|(m, r)| (m.clone(), r))
        {
            let id = format!("M-{}", rest.split('|').next().unwrap().trim());
            assert!(!menu.is_empty());
            rows.push(id);
        }
    }
    menus.retain(|(_, rows)| !rows.is_empty());
    menus
}

#[test]
fn every_menu_row_is_in_the_model_in_order() {
    let reference = reference_menus();
    let bar = menu_bar(MenuPlatform::Mac);
    assert_eq!(bar.len(), reference.len(), "nine menus");
    for (menu, (heading, rows)) in bar.iter().zip(&reference) {
        let ours: Vec<&str> = menu.walk().map(|item| item.row).collect();
        assert_eq!(
            ours, *rows,
            "menu {} against menus.md's {heading}",
            menu.title
        );
    }
    assert!(
        reference[0].0.starts_with("Live"),
        "the application menu comes first on macOS"
    );
}

#[test]
fn every_item_reaches_a_command_or_is_pending() {
    let mut registry = Registry::new();
    bl_model::registry::register(&mut registry).unwrap();
    commands::register(&mut registry).unwrap();
    let shortcuts = std::fs::read_to_string(repo().join("docs/ui-parity/shortcuts.toml")).unwrap();
    for platform in [MenuPlatform::Mac, MenuPlatform::Other] {
        for menu in menu_bar(platform) {
            for item in menu.walk() {
                match item.action {
                    Action::Command(id) => assert!(registry.contains(id), "{}: {id}", item.row),
                    Action::Shortcut(ids) => {
                        assert!(!ids.is_empty());
                        for id in ids {
                            assert!(
                                shortcuts.contains(&format!("id = \"{id}\"")),
                                "{}: {id} isn't in shortcuts.toml",
                                item.row
                            );
                        }
                    }
                    Action::Pending(Some(id)) => {
                        assert!(shortcuts.contains(&format!("id = \"{id}\"")), "{id}");
                    }
                    Action::Submenu => assert!(!item.items.is_empty(), "{}", item.row),
                    Action::Pending(None) | Action::System(_) | Action::Omitted(_) => {}
                }
                if item.is_shown() {
                    assert!(!item.label.contains("Live"), "{}: {}", item.row, item.label);
                }
            }
        }
    }
}

#[test]
fn every_shell_command_is_on_a_menu_or_a_control() {
    // Commands reached only from the Settings window's controls, the
    // Status Bar, or agents.
    let elsewhere: BTreeSet<&str> = [
        commands::SETTINGS_NEXT_PAGE,
        commands::SETTINGS_PREVIOUS_PAGE,
        commands::SETTINGS_SET_THEME,
        commands::AUDIO_DEVICES,
        commands::AUDIO_STATUS,
        commands::AUDIO_CONFIGURE,
        commands::AUDIO_SET_TEST_TONE,
    ]
    .into();
    let bar = menu_bar(MenuPlatform::Mac);
    let on_menus: BTreeSet<&str> = bar
        .iter()
        .flat_map(|menu| menu.walk().filter_map(|item| item.command()))
        .collect();
    for id in commands::IDS {
        assert!(
            on_menus.contains(id) || elsewhere.contains(id),
            "{id} is on no menu"
        );
    }
}

#[test]
fn menu_items_enable_with_what_can_run_now() {
    let mut shell = Shell::headless();
    assert!(shell.can_invoke("set.new", &json!({})));
    assert!(shell.can_invoke("app.settings", &json!({})));
    assert!(
        !shell.can_invoke("edit.undo", &json!({})),
        "nothing to undo"
    );
    assert!(
        !shell.can_invoke("window.close", &json!({})),
        "no secondary window"
    );
    assert!(
        !shell.can_invoke("track.delete", &json!({})),
        "deleting needs a selected track"
    );
    shell
        .invoke("track.create", json!({ "kind": "midi" }))
        .unwrap();
    assert!(shell.can_invoke("edit.undo", &json!({})));
}
