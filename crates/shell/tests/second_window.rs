//! The second window, driven headless the way the menus, the keymap and
//! agents drive it (WP-6.29; the second-window spec R1 to R15 and the
//! interface-zoom spec R11 to R15): every command by ID, the shell's state
//! after each, and what the set keeps.

use bl_shell::audio::AudioHost;
use bl_shell::commands::{DetailView, ViewState, WindowName};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Area, Effect, MainView, Shell, WindowKind};
use serde_json::{Value, json};

const TOGGLE: &str = "view.toggle_second_window";

fn open() -> Shell {
    let mut shell = Shell::headless();
    shell.run(TOGGLE, json!({})).unwrap();
    shell
}

fn view_state(shell: &mut Shell) -> ViewState {
    serde_json::from_value(shell.run("view.state", json!({})).unwrap()).unwrap()
}

fn shows(shell: &Shell, window: WindowKind) -> (bool, bool) {
    let detail = shell.detail_of(window);
    (detail.clip_shown(), detail.device_shown())
}

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct TestDir(std::path::PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("bl-shell-second-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell that keeps its preferences in `dir`, as the app does.
fn launch(dir: &TestDir) -> Shell {
    Shell::new(
        Preferences::load(&dir.0).unwrap(),
        Some(dir.0.clone()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

#[test]
fn the_command_opens_and_closes_the_second_window_and_the_menu_item_is_ticked() {
    // Spec: second-window R1, R15.
    let mut shell = Shell::headless();
    assert!(!shell.second_window_open());
    assert_eq!(shell.switch_state(TOGGLE), Some(false));
    let result = shell.run(TOGGLE, json!({})).unwrap();
    assert_eq!(result["open"], json!(true));
    assert!(shell.second_window_open());
    assert_eq!(shell.switch_state(TOGGLE), Some(true));
    assert_eq!(shell.focused_window(), WindowKind::Second);
    let result = shell.run(TOGGLE, json!({})).unwrap();
    assert_eq!(result["open"], json!(false));
    assert!(!shell.second_window_open());
    assert_eq!(shell.switch_state(TOGGLE), Some(false));
    assert_eq!(shell.focused_window(), WindowKind::Main);
    // It is the command Cmd+Shift+W runs and View ▸ Second Window shows.
    let spec = shell.registry().get(TOGGLE).unwrap();
    assert_eq!(spec.permission, Some(bl_commands::Permission::Read));
    assert!(
        spec.bindings.iter().any(|b| b.shortcut == "S-VIEW-002"),
        "{:?}",
        spec.bindings
    );
}

#[test]
fn close_window_closes_the_second_window_with_it_in_front() {
    // Spec: second-window R1; app-shell R16.
    let mut shell = open();
    assert!(shell.can_invoke("window.close", &json!({})));
    shell.run("window.close", json!({})).unwrap();
    assert!(!shell.second_window_open());
    assert!(
        !shell.can_invoke("window.close", &json!({})),
        "with only the main window open it is disabled"
    );
    // With the main window in front, Close Window leaves the second open.
    let mut shell = open();
    shell.set_focused_window(WindowKind::Main);
    assert!(!shell.can_invoke("window.close", &json!({})));
}

#[test]
fn the_second_window_shows_the_view_the_main_window_does_not() {
    // Spec: second-window R4.
    let mut shell = open();
    assert_eq!(shell.main_view(), MainView::Arrangement);
    assert_eq!(shell.main_view_of(WindowKind::Second), MainView::Session);
    let state = view_state(&mut shell);
    assert!(state.second_window.open);
    assert_eq!(state.window.main_view, "arrangement");
    assert_eq!(state.second_window.main_view, "session");
    // Tab, the selector and the command swap them.
    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    assert_eq!(shell.main_view(), MainView::Session);
    assert_eq!(
        shell.main_view_of(WindowKind::Second),
        MainView::Arrangement
    );
    assert_eq!(
        shell.window_showing(MainView::Arrangement),
        WindowKind::Second
    );
    let state = view_state(&mut shell);
    assert_eq!(state.window.main_view, "session");
    assert_eq!(state.second_window.main_view, "arrangement");
    // Closed, the second window is the one that would show the other.
    shell.run(TOGGLE, json!({})).unwrap();
    assert_eq!(
        view_state(&mut shell).second_window.main_view,
        "arrangement"
    );
    assert_eq!(
        shell.main_view(),
        MainView::Session,
        "the main window keeps its view"
    );
}

#[test]
fn switching_views_changes_no_set_and_adds_no_undo_step() {
    // Spec: second-window R14.
    let mut shell = Shell::headless();
    shell.run(TOGGLE, json!({})).unwrap();
    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    shell.run(TOGGLE, json!({})).unwrap();
    assert!(!shell.is_dirty());
    assert!(!shell.editor().can_undo());
}

#[test]
fn navigate_gives_the_focus_to_the_window_that_shows_the_view() {
    // Spec: second-window R11.
    let mut shell = open();
    shell.set_focused_window(WindowKind::Main);
    shell.take_effects();
    // The Session View is in the second window.
    shell.run("session.focus", json!({})).unwrap();
    assert_eq!(shell.focused_window(), WindowKind::Second);
    assert_eq!(shell.focused_area_of(WindowKind::Second), Area::MainView);
    assert_eq!(shell.main_view(), MainView::Arrangement, "no view changed");
    assert_eq!(
        shell.take_effects(),
        [Effect::FocusSecondArea(Area::MainView)]
    );
    shell.run("focus.arrangement", json!({})).unwrap();
    assert_eq!(shell.focused_window(), WindowKind::Main);
    assert_eq!(shell.main_view(), MainView::Arrangement);
    assert_eq!(shell.take_effects(), [Effect::FocusArea(Area::MainView)]);
    // With one window the view is shown first, as before.
    shell.run(TOGGLE, json!({})).unwrap();
    shell.run("session.focus", json!({})).unwrap();
    assert_eq!(shell.main_view(), MainView::Session);
}

#[test]
fn a_view_shown_by_its_toggle_opens_in_the_window_with_the_focus() {
    // Spec: second-window R7; detail-views R1.
    let mut shell = open();
    // The second window opens with neither detail view; the Device View
    // stays in the main window.
    assert_eq!(shows(&shell, WindowKind::Main), (false, true));
    assert_eq!(shows(&shell, WindowKind::Second), (false, false));
    assert_eq!(shell.acting_window(), WindowKind::Second);
    shell.run("view.toggle_clip_view", json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Second), (true, false));
    assert_eq!(shows(&shell, WindowKind::Main), (false, true));
    // The Device View leaves the main window for the second one.
    shell.run("view.toggle_device_view", json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Second), (true, true));
    assert_eq!(shows(&shell, WindowKind::Main), (false, false));
    // The main window takes it back when it has the focus.
    shell.set_focused_window(WindowKind::Main);
    assert_eq!(shell.acting_window(), WindowKind::Main);
    shell.run("view.toggle_device_view", json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Main), (false, true));
    assert_eq!(shows(&shell, WindowKind::Second), (true, false));
    assert!(
        shell.clip_view().visible(),
        "the Clip View shows in a window"
    );
    // The selector's shape: `window` names the window whichever has the
    // focus.
    shell
        .run("view.toggle_clip_view", json!({ "window": "main" }))
        .unwrap();
    assert_eq!(shows(&shell, WindowKind::Main), (true, true));
    assert_eq!(shows(&shell, WindowKind::Second), (false, false));
    assert!(shell.clip_view().visible());
    shell
        .run("view.toggle_clip_view", json!({ "window": "second" }))
        .unwrap();
    assert_eq!(shows(&shell, WindowKind::Second), (true, false));
    // No window argument names a closed window.
    shell.run(TOGGLE, json!({})).unwrap();
    assert!(
        shell
            .run("view.toggle_clip_view", json!({ "window": "second" }))
            .is_err()
    );
}

#[test]
fn the_clip_view_keeps_its_height_and_expansion_when_it_moves() {
    // Spec: second-window R7.
    let mut shell = open();
    shell.set_focused_window(WindowKind::Main);
    shell.run("view.toggle_clip_view", json!({})).unwrap();
    shell
        .run("view.set_detail_height", json!({ "points": 300 }))
        .unwrap();
    shell.run("view.switch_detail_view", json!({})).unwrap();
    // Both views moved to the second window, the Clip View as tall.
    assert_eq!(shows(&shell, WindowKind::Second), (true, true));
    assert_eq!(shell.detail_of(WindowKind::Second).clip_height(), 300.0);
    shell
        .run("view.expand_clip_view", json!({ "window": "second" }))
        .unwrap();
    shell.set_focused_window(WindowKind::Second);
    shell.run("view.switch_detail_view", json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Main), (true, true));
    assert!(shell.detail_of(WindowKind::Main).expanded());
}

#[test]
fn f12_moves_what_the_window_in_front_shows_to_the_other_window() {
    // Spec: second-window R8.
    let mut shell = open();
    shell.set_focused_window(WindowKind::Main);
    // The main window shows the Device View: it moves.
    shell.run("view.switch_detail_view", json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Main), (false, false));
    assert_eq!(shows(&shell, WindowKind::Second), (false, true));
    // The window in front shows neither: the other window's come to it.
    shell.run("view.switch_detail_view", json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Main), (false, true));
    assert_eq!(shows(&shell, WindowKind::Second), (false, false));
    // One view in each window: the front window's moves, joining the other.
    shell
        .run("view.toggle_clip_view", json!({ "window": "second" }))
        .unwrap();
    assert_eq!(shows(&shell, WindowKind::Second), (true, false));
    shell.run("view.switch_detail_view", json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Main), (false, false));
    assert_eq!(shows(&shell, WindowKind::Second), (true, true));
    // Both views stacked move together.
    shell.set_focused_window(WindowKind::Second);
    shell.run("view.switch_detail_view", json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Main), (true, true));
    assert_eq!(shows(&shell, WindowKind::Second), (false, false));
    // Nothing marks the set changed or adds an undo step.
    assert!(!shell.is_dirty());
    assert!(!shell.editor().can_undo());
}

#[test]
fn closing_the_second_window_brings_its_views_back_to_the_main_window() {
    // Spec: second-window R9.
    let mut shell = open();
    shell.run("view.show_detail_views", json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Second), (true, true));
    assert_eq!(shows(&shell, WindowKind::Main), (false, false));
    shell.run(TOGGLE, json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Main), (true, true));
    assert!(shell.clip_view().visible());
    // It opens again with neither view.
    shell.run(TOGGLE, json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Second), (false, false));
    assert_eq!(shows(&shell, WindowKind::Main), (true, true));
}

#[test]
fn navigate_to_a_detail_view_goes_to_the_window_that_shows_it() {
    // Spec: second-window R11; detail-views R14.
    let mut shell = open();
    shell.set_focused_window(WindowKind::Main);
    shell
        .run("view.toggle_clip_view", json!({ "window": "second" }))
        .unwrap();
    shell.set_focused_window(WindowKind::Main);
    shell.take_effects();
    shell.run("focus.clip_view", json!({})).unwrap();
    assert_eq!(shell.focused_window(), WindowKind::Second);
    assert_eq!(shell.focused_area_of(WindowKind::Second), Area::Detail);
    assert_eq!(
        shell.take_effects(),
        [Effect::FocusSecondArea(Area::Detail)]
    );
    shell.run("focus.device_view", json!({})).unwrap();
    assert_eq!(shell.focused_window(), WindowKind::Main);
    assert_eq!(shell.take_effects(), [Effect::FocusArea(Area::Detail)]);
    // Shown nowhere: it shows in the window with the focus.
    shell.run("view.toggle_device_view", json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Main), (false, false));
    shell.set_focused_window(WindowKind::Second);
    shell.run("focus.device_view", json!({})).unwrap();
    assert_eq!(shows(&shell, WindowKind::Second), (true, true));
    assert_eq!(
        shell.detail_of(WindowKind::Second).focus(),
        DetailView::Device
    );
}

#[test]
fn keys_run_what_the_keymap_binds_for_the_window_in_front() {
    // Spec: second-window R10. The contexts are those of the focused area
    // of the window in front.
    use bl_commands::Context;
    let mut shell = open();
    assert_eq!(shell.focused_window(), WindowKind::Second);
    // The Session View is in the second window.
    assert!(shell.active_contexts().contains(&Context::Session));
    shell.set_focused_window(WindowKind::Main);
    assert!(shell.active_contexts().contains(&Context::Arrangement));
    assert!(!shell.active_contexts().contains(&Context::Session));
    // A click gives a window its area the focus.
    shell.set_focused_area_in(WindowKind::Second, Area::Detail);
    assert_eq!(shell.focused_area_of(WindowKind::Second), Area::Detail);
    assert_eq!(shell.focused_area_of(WindowKind::Main), Area::MainView);
    shell.set_focused_window(WindowKind::Second);
    assert!(!shell.active_contexts().contains(&Context::Session));
}

#[test]
fn the_info_view_and_the_undo_history_stay_in_the_main_window() {
    // Spec: second-window R12; info-view R8; undo-history R1.
    let mut shell = open();
    assert!(shell.info_view_of(WindowKind::Main));
    assert!(!shell.info_view_of(WindowKind::Second));
    shell.run("view.toggle_info_view", json!({})).unwrap();
    assert!(
        !shell.info_view_visible(),
        "with the second window in front"
    );
    assert!(!shell.info_view_of(WindowKind::Second));
    shell.run("view.toggle_undo_history", json!({})).unwrap();
    assert!(view_state(&mut shell).undo_history);
    assert_eq!(shell.acting_window(), WindowKind::Second, "the focus stays");
}

#[test]
fn each_window_has_its_own_browser_and_full_screen() {
    // Spec: second-window R5, R13.
    let mut shell = open();
    assert!(shell.browser_visible());
    assert!(!shell.browser_of(WindowKind::Second), "it opens without");
    shell.run("view.toggle_browser", json!({})).unwrap();
    assert!(shell.browser_of(WindowKind::Second));
    assert!(shell.browser_visible(), "the main window's is its own");
    shell
        .run("view.toggle_browser", json!({ "window": "main" }))
        .unwrap();
    assert!(!shell.browser_visible());
    assert!(shell.browser_of(WindowKind::Second));
    shell.take_effects();
    shell.run("view.full_screen", json!({})).unwrap();
    assert_eq!(shell.take_effects(), [Effect::SetSecondFullScreen(true)]);
    assert!(shell.full_screen_of(WindowKind::Second));
    assert!(!shell.full_screen());
    shell.set_focused_window(WindowKind::Main);
    shell.run("view.full_screen", json!({})).unwrap();
    assert_eq!(shell.take_effects(), [Effect::SetFullScreen(true)]);
    assert!(shell.full_screen() && shell.full_screen_of(WindowKind::Second));
    let state = view_state(&mut shell);
    assert!(state.window.full_screen && state.second_window.full_screen);
    assert!(state.second_window.browser && !state.window.browser);
}

#[test]
fn view_state_reads_back_every_command() {
    // Spec: second-window R15.
    let mut shell = Shell::headless();
    let state = view_state(&mut shell);
    assert!(!state.second_window.open);
    assert_eq!(state.focused_window, WindowName::Main);
    let steps: [(&str, Value); 10] = [
        (TOGGLE, json!({})),
        ("view.toggle_clip_view", json!({})),
        ("view.toggle_browser", json!({})),
        ("view.set_zoom", json!({ "percent": 140 })),
        ("view.set_detail_height", json!({ "points": 180 })),
        ("view.expand_clip_view", json!({})),
        ("view.full_screen", json!({})),
        ("view.toggle_session_arrangement", json!({})),
        ("view.switch_detail_view", json!({})),
        ("view.toggle_detail_area", json!({})),
    ];
    for (id, args) in steps {
        let result = shell.run(id, args).unwrap();
        let state = view_state(&mut shell);
        assert_eq!(state.focused_window, WindowName::Second, "{id}");
        if id == TOGGLE {
            assert_eq!(serde_json::to_value(&state.second_window).unwrap(), result);
        }
        assert_eq!(
            state.second_window.detail,
            shell.detail_of(WindowKind::Second).state(),
            "{id}"
        );
        assert_eq!(state.detail, shell.detail().state(), "{id}");
    }
    let state = view_state(&mut shell);
    assert!(state.second_window.open && state.second_window.browser);
    assert!(state.second_window.full_screen);
    assert!((state.second_window.zoom - 1.4).abs() < 1e-6);
    assert!(
        (state.window.zoom - 1.0).abs() < 1e-6,
        "the main window's own"
    );
    shell.set_focused_window(WindowKind::Main);
    assert_eq!(view_state(&mut shell).focused_window, WindowName::Main);
}

#[test]
fn a_set_keeps_the_main_view_and_the_second_window() {
    // Spec: second-window R14.
    let dir = TestDir::new("keeps");
    let mut shell = Shell::headless();
    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    shell.run(TOGGLE, json!({})).unwrap();
    assert!(!shell.is_dirty(), "switching marks nothing as changed");
    // The store puts the set in a project folder; the result says where.
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": dir.0.join("Kept.blset").display().to_string() }),
        )
        .unwrap();
    let path = std::path::PathBuf::from(saved["path"].as_str().unwrap());
    // Another set, with the other views, replaces it.
    shell.run("set.new", json!({})).unwrap();
    assert_eq!(
        shell.main_view(),
        MainView::Arrangement,
        "a new set opens in the Arrangement"
    );
    assert!(!shell.second_window_open());
    shell
        .run("set.open", json!({ "path": path.display().to_string() }))
        .unwrap();
    assert_eq!(shell.main_view(), MainView::Session);
    assert!(shell.second_window_open());
    assert_eq!(
        shell.main_view_of(WindowKind::Second),
        MainView::Arrangement
    );
    assert!(!shell.is_dirty());
    assert!(!shell.editor().can_undo());
    // A change made after the open is kept by the next save.
    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    shell.run(TOGGLE, json!({})).unwrap();
    assert!(!shell.is_dirty());
    shell.run("set.save", json!({})).unwrap();
    shell.run("set.new", json!({})).unwrap();
    shell
        .run("set.open", json!({ "path": path.display().to_string() }))
        .unwrap();
    assert_eq!(shell.main_view(), MainView::Arrangement);
    assert!(!shell.second_window_open());
}

#[test]
fn a_saved_default_set_keeps_its_views() {
    // Spec: second-window R14; D20.
    let dir = TestDir::new("default");
    let mut shell = Shell::headless();
    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    shell.run(TOGGLE, json!({})).unwrap();
    let default = dir.0.join("Default.blset");
    shell
        .run(
            "set.save_as_default",
            json!({ "path": default.display().to_string() }),
        )
        .unwrap();
    // A new set starts from it, views and all; so does the next launch.
    shell.run(TOGGLE, json!({})).unwrap();
    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    shell.run("set.new", json!({})).unwrap();
    assert_eq!(shell.main_view(), MainView::Session);
    assert!(shell.second_window_open());
    let prefs = Preferences {
        default_set: Some(default),
        ..Preferences::default()
    };
    let launched = Shell::new(
        prefs,
        None,
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    assert_eq!(launched.main_view(), MainView::Session);
    assert!(launched.second_window_open());
    // The factory default set opens in the Arrangement View with one
    // window.
    let factory = Shell::headless();
    assert_eq!(factory.main_view(), MainView::Arrangement);
    assert!(!factory.second_window_open());
}

#[test]
fn each_window_zooms_on_its_own() {
    // Spec: interface-zoom R11, R13, R14.
    let mut shell = Shell::headless();
    shell
        .run("view.set_zoom", json!({ "percent": 120 }))
        .unwrap();
    shell.run(TOGGLE, json!({})).unwrap();
    // The first time it opens at the main window's zoom.
    assert_eq!(shell.zoom_percent_of(WindowKind::Second), 120);
    // The zoom keys and commands act on the window with the focus.
    shell.run("view.zoom_in", json!({})).unwrap();
    assert_eq!(shell.zoom_percent_of(WindowKind::Second), 130);
    assert_eq!(shell.zoom_percent(), 120);
    shell.set_focused_window(WindowKind::Main);
    shell.run("view.zoom_out", json!({})).unwrap();
    assert_eq!(shell.zoom_percent(), 110);
    assert_eq!(shell.zoom_percent_of(WindowKind::Second), 130);
    // `window` names one whichever has the focus.
    shell
        .run(
            "view.set_zoom",
            json!({ "percent": 75, "window": "second" }),
        )
        .unwrap();
    assert_eq!(shell.zoom_percent_of(WindowKind::Second), 75);
    assert_eq!(shell.zoom_percent(), 110);
    shell
        .run("view.zoom_reset", json!({ "window": "second" }))
        .unwrap();
    assert_eq!(shell.zoom_percent_of(WindowKind::Second), 100);
    assert_eq!(shell.zoom_percent(), 110);
    // The disabled ends are those of the window the zoom acts on.
    shell
        .run(
            "view.set_zoom",
            json!({ "percent": 200, "window": "second" }),
        )
        .unwrap();
    assert!(shell.can_invoke("view.zoom_in", &json!({})));
    assert!(!shell.can_invoke("view.zoom_in", &json!({ "window": "second" })));
    shell.set_focused_window(WindowKind::Second);
    assert!(!shell.can_invoke("view.zoom_in", &json!({})));
    // Zooming changes nothing in the set.
    assert!(!shell.is_dirty());
    assert!(!shell.editor().can_undo());
}

#[test]
fn the_second_window_zoom_row_shows_only_while_it_is_open() {
    // Spec: interface-zoom R12 (the page's state), R4.
    let mut shell = Shell::headless();
    let settings = |shell: &mut Shell| shell.run("settings.state", json!({})).unwrap();
    assert!(
        settings(&mut shell)["display_input"]
            .get("second_window_zoom_percent")
            .is_none()
    );
    shell.run(TOGGLE, json!({})).unwrap();
    shell
        .run(
            "view.set_zoom",
            json!({ "percent": 150, "window": "second" }),
        )
        .unwrap();
    let state = settings(&mut shell);
    assert_eq!(
        state["display_input"]["second_window_zoom_percent"],
        json!(150)
    );
    assert_eq!(state["display_input"]["zoom_percent"], json!(100));
    // The Settings window follows the main window.
    assert_eq!(shell.zoom_percent(), 100);
    shell.run(TOGGLE, json!({})).unwrap();
    assert!(
        settings(&mut shell)["display_input"]
            .get("second_window_zoom_percent")
            .is_none()
    );
}

#[test]
fn zoom_set_while_the_second_window_is_closed_is_the_zoom_it_opens_at() {
    // Spec: interface-zoom R14.
    let mut shell = Shell::headless();
    shell
        .run(
            "view.set_zoom",
            json!({ "percent": 160, "window": "second" }),
        )
        .unwrap();
    assert_eq!(shell.zoom_percent(), 100);
    shell.run(TOGGLE, json!({})).unwrap();
    assert_eq!(shell.zoom_percent_of(WindowKind::Second), 160);
}

#[test]
fn the_second_windows_zoom_and_frame_are_kept_between_launches() {
    // Spec: second-window R1; interface-zoom R11.
    let dir = TestDir::new("frame");
    let mut first = launch(&dir);
    first.run(TOGGLE, json!({})).unwrap();
    first
        .run("view.set_zoom", json!({ "percent": 140 }))
        .unwrap();
    first.set_second_window_frame(bl_model::WindowFrame {
        x: 120.0,
        y: 80.0,
        width: 900.0,
        height: 640.0,
    });
    // Closing keeps it for the next time.
    first.run(TOGGLE, json!({})).unwrap();
    drop(first);
    let second = launch(&dir);
    assert_eq!(second.zoom_percent_of(WindowKind::Second), 140);
    assert_eq!(second.zoom_percent(), 100);
    let frame = second.second_window_frame().unwrap();
    assert_eq!(
        (frame.x, frame.y, frame.width, frame.height),
        (120.0, 80.0, 900.0, 640.0)
    );
    // A frame that isn't a window is not kept.
    let mut shell = Shell::headless();
    shell.set_second_window_frame(bl_model::WindowFrame {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 10.0,
    });
    assert!(shell.second_window_frame().is_none());
}
