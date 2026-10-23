//! The interface zoom's commands and preference (WP-1.17), driven headless
//! the way the View menu, the keymap, the Settings window, and agents drive
//! them. The layout's scaling is tested in `crates/ui/tests/zoom.rs`.

use bl_commands::Permission;
use bl_shell::audio::AudioHost;
use bl_shell::commands as ids;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::{PREFERENCES_FILE, Preferences, ZOOM_STEPS};
use bl_shell::{Shell, ShellError};
use serde_json::json;

/// A preferences folder of its own under the system's temporary folder,
/// removed when dropped.
struct TestDir(std::path::PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bl-shell-zoom-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell that keeps its preferences in `dir`, as the app does, starting
/// from whatever is saved there.
fn launch(dir: &TestDir) -> Shell {
    Shell::new(
        Preferences::load(&dir.0).unwrap(),
        Some(dir.0.clone()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

#[test]
fn zoom_in_and_out_walk_every_step_and_stop_at_the_ends() {
    // Spec: interface-zoom R3.
    let mut shell = Shell::headless();
    assert_eq!(shell.zoom_percent(), 100, "a new install is at 100%");
    let mut seen = vec![shell.zoom_percent()];
    while shell.can_invoke(ids::VIEW_ZOOM_IN, &json!({})) {
        shell.invoke(ids::VIEW_ZOOM_IN, json!({})).unwrap();
        seen.push(shell.zoom_percent());
    }
    assert_eq!(shell.zoom_percent(), 200);
    assert!(
        !shell.can_invoke(ids::VIEW_ZOOM_IN, &json!({})),
        "Zoom In is disabled at 200%"
    );
    // An agent's Zoom In at the top changes nothing and says where it is.
    let result = shell.run(ids::VIEW_ZOOM_IN, json!({})).unwrap();
    assert!((result["zoom"].as_f64().unwrap() - 2.0).abs() < 1e-6);
    while shell.can_invoke(ids::VIEW_ZOOM_OUT, &json!({})) {
        shell.invoke(ids::VIEW_ZOOM_OUT, json!({})).unwrap();
        seen.push(shell.zoom_percent());
    }
    assert_eq!(shell.zoom_percent(), 50);
    assert!(
        !shell.can_invoke(ids::VIEW_ZOOM_OUT, &json!({})),
        "Zoom Out is disabled at 50%"
    );
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen, ZOOM_STEPS, "every step, and nothing else, on the way");

    shell.run(ids::VIEW_ZOOM_RESET, json!({})).unwrap();
    assert_eq!(shell.zoom_percent(), 100);
}

#[test]
fn set_zoom_takes_any_whole_percentage_in_range() {
    // Spec: interface-zoom R4, R5.
    let mut shell = Shell::headless();
    let result = shell
        .run(ids::VIEW_SET_ZOOM, json!({ "percent": 125 }))
        .unwrap();
    assert!((result["zoom"].as_f64().unwrap() - 1.25).abs() < 1e-6);
    assert_eq!(shell.zoom_percent(), 125);
    // Between steps, Zoom In and Out go to the nearest step their way.
    shell.run(ids::VIEW_ZOOM_IN, json!({})).unwrap();
    assert_eq!(shell.zoom_percent(), 130);
    shell
        .run(ids::VIEW_SET_ZOOM, json!({ "percent": 125 }))
        .unwrap();
    shell.run(ids::VIEW_ZOOM_OUT, json!({})).unwrap();
    assert_eq!(shell.zoom_percent(), 120);

    // The schema says 50 to 200, so a person's invocation outside it isn't
    // offered; a value that gets through anyway is brought within it.
    let schema = shell
        .registry()
        .get(ids::VIEW_SET_ZOOM)
        .unwrap()
        .args
        .clone()
        .unwrap();
    assert!(bl_commands::validate(&schema, &json!({ "percent": 150 })).is_ok());
    for bad in [
        json!({ "percent": 49 }),
        json!({ "percent": 201 }),
        json!({}),
    ] {
        assert!(bl_commands::validate(&schema, &bad).is_err(), "{bad}");
        assert!(!shell.can_invoke(ids::VIEW_SET_ZOOM, &bad), "{bad}");
    }
    shell
        .run(ids::VIEW_SET_ZOOM, json!({ "percent": 400 }))
        .unwrap();
    assert_eq!(shell.zoom_percent(), 200);
    shell
        .run(ids::VIEW_SET_ZOOM, json!({ "percent": 10 }))
        .unwrap();
    assert_eq!(shell.zoom_percent(), 50);
    assert!(matches!(
        shell.run(ids::VIEW_SET_ZOOM, json!({ "percent": 1.5 })),
        Err(ShellError::InvalidArgs { .. })
    ));
    assert!(matches!(
        shell.run(
            ids::VIEW_SET_ZOOM,
            json!({ "percent": 150, "screen": "main" })
        ),
        Err(ShellError::InvalidArgs { .. })
    ));
}

#[test]
fn zoom_commands_are_edit_class_preferences_outside_the_set() {
    // Spec: interface-zoom R5. They write the preferences file and are
    // trivially changed back: edit, by the architecture's classes.
    let mut shell = Shell::headless();
    for id in [
        ids::VIEW_ZOOM_IN,
        ids::VIEW_ZOOM_OUT,
        ids::VIEW_ZOOM_RESET,
        ids::VIEW_SET_ZOOM,
    ] {
        let spec = shell.registry().get(id).unwrap();
        assert_eq!(spec.permission, Some(Permission::Edit), "{id}");
        assert!(spec.result.is_some() && spec.args.is_some(), "{id}");
    }
    // Neither the set nor its undo history changes.
    shell
        .run(ids::VIEW_SET_ZOOM, json!({ "percent": 150 }))
        .unwrap();
    shell.run(ids::VIEW_ZOOM_IN, json!({})).unwrap();
    assert!(!shell.is_dirty());
    assert!(!shell.editor().can_undo());
}

#[test]
fn the_zoom_is_kept_between_launches() {
    // Spec: interface-zoom R6.
    let dir = TestDir::new("kept");
    let mut first = launch(&dir);
    first
        .invoke(ids::VIEW_SET_ZOOM, json!({ "percent": 150 }))
        .unwrap();
    first.invoke(ids::VIEW_ZOOM_IN, json!({})).unwrap();
    drop(first);
    let mut second = launch(&dir);
    assert_eq!(second.zoom_percent(), 160, "the next start has the zoom");

    // Back to 100%: the file no longer holds a zoom, as on a new install.
    second.invoke(ids::VIEW_ZOOM_RESET, json!({})).unwrap();
    assert_eq!(Preferences::load(&dir.0).unwrap().zoom, None);
    assert_eq!(launch(&dir).zoom_percent(), 100);
}

#[test]
fn a_zoom_out_of_range_in_the_preferences_file_starts_in_range() {
    // Spec: interface-zoom R6.
    let dir = TestDir::new("range");
    std::fs::create_dir_all(&dir.0).unwrap();
    for (text, percent) in [
        (r#"{ "zoom": 5.0 }"#, 200),
        (r#"{ "zoom": 0.1 }"#, 50),
        (r#"{ "zoom": 1.234 }"#, 123),
        (r#"{ "zoom": -3 }"#, 50),
    ] {
        std::fs::write(dir.0.join(PREFERENCES_FILE), text).unwrap();
        assert_eq!(launch(&dir).zoom_percent(), percent, "{text}");
    }
}
