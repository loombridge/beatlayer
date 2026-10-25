//! Snap Automation to Grid in the shell (WP-6.57; the automation spec, R31):
//! the preference is on in a new install, kept between launches, set by its
//! command and ticked in the Options menu. Driven through `Shell::run` and
//! `Shell::invoke`, as an agent and a person drive them.

use bl_shell::Shell;
use bl_shell::settings::Preferences;
use serde_json::json;

const COMMAND: &str = "settings.set_snap_automation_to_grid";

// Spec: automation R31. On in a new install and for preferences saved before
// it existed, kept between launches.
#[test]
fn the_preference_is_on_in_a_new_install_and_kept() {
    assert!(Preferences::default().snap_automation_to_grid.0);
    let older: Preferences = serde_json::from_str("{}").unwrap();
    assert!(
        older.snap_automation_to_grid.0,
        "preferences from before it"
    );
    let off = serde_json::to_value(Preferences {
        snap_automation_to_grid: bl_shell::settings::OnByDefault(false),
        ..Preferences::default()
    })
    .unwrap();
    assert_eq!(off["snap_automation_to_grid"], false);
    let kept: Preferences = serde_json::from_value(off).unwrap();
    assert!(!kept.snap_automation_to_grid.0);
}

// Spec: automation R31 (M-OPT-015). The command sets it, `settings.state`
// reads it back, and the menu item switches it and shows it ticked.
#[test]
fn the_command_sets_it_and_the_menu_item_ticks_with_it() {
    let mut shell = Shell::headless();
    let read = |shell: &mut Shell| shell.run("settings.state", json!({})).unwrap();
    assert_eq!(read(&mut shell)["snap_automation_to_grid"], true);
    assert!(shell.snap_automation_to_grid());
    assert_eq!(shell.switch_state(COMMAND), Some(true));

    shell.run(COMMAND, json!({ "on": false })).unwrap();
    assert_eq!(read(&mut shell)["snap_automation_to_grid"], false);
    assert_eq!(shell.switch_state(COMMAND), Some(false));

    // The menu item runs the command with no argument: it switches.
    shell.invoke(COMMAND, json!({})).unwrap();
    assert!(shell.snap_automation_to_grid());
    assert_eq!(shell.switch_state(COMMAND), Some(true));
}

// Spec: automation R31. The preference isn't part of the set and isn't an
// undo step.
#[test]
fn switching_it_is_no_undo_step() {
    let mut shell = Shell::headless();
    let steps = shell.editor().undo_history().len();
    shell.run(COMMAND, json!({ "on": false })).unwrap();
    assert_eq!(shell.editor().undo_history().len(), steps);
}
