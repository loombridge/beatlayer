//! Floating device windows in the shell (WP-2.5b, IMP-14; the device chains
//! spec, R26 to R30): Float in the device's menu and `device.set_floating`
//! float and dock a device as one undo step each, its place in the chain a
//! strip; its window's frame is saved with the set but is no undo step,
//! and never smaller than the docked size; closing the window docks it;
//! a set reopens with the same devices floating in the same places. The
//! devices are the framework's test devices.

use bl_devices::testing;
use bl_model::{DeviceId, TrackId, WindowFrame};
use bl_shell::device_commands::{
    DeviceAdded, FloatingWindow, FloatingWindows, TITLE_BAR_HEIGHT, frame_width, window_min_size,
};
use bl_shell::{Area, Shell, WindowKind};
use serde_json::{Value, json};

fn shell() -> (Shell, TrackId) {
    let mut shell = Shell::headless();
    shell.set_device_types(testing::device_types());
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    (shell, audio)
}

fn add(shell: &mut Shell, device: &Value) -> DeviceId {
    let added: DeviceAdded = serde_json::from_value(
        shell
            .invoke("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn add_type(shell: &mut Shell, t: &bl_engine::DeviceType) -> DeviceId {
    add(
        shell,
        &json!({ "kind": t.kind, "name": t.name, "role": t.role }),
    )
}

fn windows(shell: &mut Shell) -> Vec<FloatingWindow> {
    let reply = shell.run("device.windows", json!({})).unwrap();
    let windows: FloatingWindows = serde_json::from_value(reply).unwrap();
    assert_eq!(windows, shell.floating_windows());
    windows.windows
}

fn history(shell: &Shell) -> Vec<String> {
    shell.editor().undo_history().map(str::to_owned).collect()
}

fn floating(shell: &Shell, device: DeviceId) -> bool {
    shell.editor().document().device(device).unwrap().floating
}

// Spec: device-chains R26, R32
#[test]
fn float_and_dock_are_one_undo_step_each_and_leave_a_strip_in_the_chain() {
    let (mut shell, audio) = shell();
    let utility = add_type(&mut shell, &testing::utility_type());
    let docked = frame_width(&shell.device_view_state().devices[0]);
    assert_eq!(shell.switch_state("device.set_floating"), Some(false));

    // Float in the device's menu acts on the selected device.
    assert!(shell.can_invoke("device.set_floating", &json!({})));
    shell.invoke("device.set_floating", json!({})).unwrap();
    assert!(floating(&shell, utility));
    assert_eq!(history(&shell)[0], "Float Device");
    assert_eq!(shell.switch_state("device.set_floating"), Some(true));
    let frame = &shell.device_view_state().devices[0];
    assert!(frame.floating);
    assert_eq!(frame_width(frame), TITLE_BAR_HEIGHT, "a strip in the chain");

    let open = windows(&mut shell);
    assert_eq!(open.len(), 1);
    let window = &open[0];
    assert_eq!((window.device, window.track), (utility, audio));
    // Live titles a plug-in's window with the device, then the track.
    assert_eq!(window.title, "Framework Test Utility/3-Audio");
    assert_eq!(window.frame, None, "the app places a new window");
    assert_eq!(
        (window.min_width, window.min_height),
        (docked, TITLE_BAR_HEIGHT + 158.0)
    );
    assert!(!window.view.folded);

    // Choosing Float again docks it.
    shell.invoke("device.set_floating", json!({})).unwrap();
    assert!(!floating(&shell, utility));
    assert_eq!(history(&shell)[..2], ["Dock Device", "Float Device"]);
    assert!(windows(&mut shell).is_empty());
    assert_eq!(frame_width(&shell.device_view_state().devices[0]), docked);

    // Undo floats it again, and undo again docks it.
    shell.invoke("edit.undo", json!({})).unwrap();
    assert!(floating(&shell, utility));
    shell.invoke("edit.undo", json!({})).unwrap();
    assert!(!floating(&shell, utility));
}

// Spec: device-chains R27
#[test]
fn closing_a_floating_window_docks_the_device() {
    let (mut shell, _) = shell();
    let utility = add_type(&mut shell, &testing::utility_type());
    shell
        .run(
            "device.set_floating",
            json!({ "device": utility, "floating": true }),
        )
        .unwrap();
    // Close Window (Cmd-W) with the device's window in front.
    shell.set_focused_window(WindowKind::Device(utility));
    assert!(shell.can_invoke("window.close", &json!({})));
    shell.invoke("window.close", json!({})).unwrap();
    assert!(!floating(&shell, utility));
    assert_eq!(history(&shell)[0], "Dock Device");
    assert_eq!(shell.focused_window(), WindowKind::Main);
    assert!(!shell.can_invoke("window.close", &json!({})));
}

// Spec: device-chains R28
#[test]
fn moving_or_resizing_a_window_is_no_undo_step_and_never_goes_below_the_docked_size() {
    let (mut shell, _) = shell();
    let scope = add_type(&mut shell, &testing::scope_type());
    let utility = add_type(&mut shell, &testing::utility_type());
    for device in [scope, utility] {
        shell
            .run(
                "device.set_floating",
                json!({ "device": device, "floating": true }),
            )
            .unwrap();
    }
    let steps = history(&shell);
    let dirty = shell.is_dirty();
    let frame = |w: f64, h: f64| WindowFrame {
        x: 100.0,
        y: 80.0,
        width: w,
        height: h,
    };
    // The scope has a display that grows; its window does too.
    shell
        .run(
            "device.set_window",
            json!({ "device": scope, "frame": frame(900.0, 400.0) }),
        )
        .unwrap();
    assert_eq!(history(&shell), steps, "moving a window is no undo step");
    assert_eq!(shell.is_dirty(), dirty);
    let open = windows(&mut shell);
    let scope_window = open.iter().find(|w| w.device == scope).unwrap();
    assert!(scope_window.grows_across && scope_window.grows_down);
    assert_eq!(scope_window.frame, Some(frame(900.0, 400.0)));
    // Smaller than the docked size, it's held at the docked size.
    shell
        .run(
            "device.set_window",
            json!({ "device": scope, "frame": frame(10.0, 10.0) }),
        )
        .unwrap();
    let (w, h) = window_min_size(&scope_window.view);
    assert_eq!(
        windows(&mut shell)[0].frame,
        Some(frame(f64::from(w), f64::from(h)))
    );
    // A device without a display that grows stays at its docked size.
    shell
        .run(
            "device.set_window",
            json!({ "device": utility, "frame": frame(900.0, 400.0) }),
        )
        .unwrap();
    let utility_window = windows(&mut shell)
        .into_iter()
        .find(|w| w.device == utility)
        .unwrap();
    assert!(!utility_window.grows_across && !utility_window.grows_down);
    assert_eq!(
        utility_window.frame,
        Some(frame(
            f64::from(utility_window.min_width),
            f64::from(utility_window.min_height)
        ))
    );
    // A docked device has no window to move, and a broken frame is refused.
    shell
        .run(
            "device.set_floating",
            json!({ "device": utility, "floating": false }),
        )
        .unwrap();
    assert!(
        shell
            .run(
                "device.set_window",
                json!({ "device": utility, "frame": frame(300.0, 300.0) }),
            )
            .is_err()
    );
    assert!(
        shell
            .run(
                "device.set_window",
                json!({ "device": scope, "frame": frame(-1.0, 300.0) }),
            )
            .is_err()
    );
    // Undoing the dock brings the window back where it was.
    let before = windows(&mut shell)[0].frame;
    shell.invoke("edit.undo", json!({})).unwrap();
    let back = windows(&mut shell);
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].frame, before);
}

// Spec: device-chains R29
#[test]
fn a_set_reopens_with_the_same_devices_floating_in_the_same_places() {
    let (mut shell, _) = shell();
    let scope = add_type(&mut shell, &testing::scope_type());
    let utility = add_type(&mut shell, &testing::utility_type());
    shell
        .run(
            "device.set_floating",
            json!({ "device": scope, "floating": true }),
        )
        .unwrap();
    let frame = WindowFrame {
        x: 240.0,
        y: 160.0,
        width: 640.0,
        height: 300.0,
    };
    shell
        .run(
            "device.set_window",
            json!({ "device": scope, "frame": frame }),
        )
        .unwrap();
    let dir = std::env::temp_dir().join(format!(
        "bl-shell-floating-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": dir.join("Floats.blset").to_string_lossy() }),
        )
        .unwrap();
    let path = saved["path"].as_str().unwrap().to_owned();
    let expected = windows(&mut shell);

    shell.run("set.new", json!({})).unwrap();
    assert!(windows(&mut shell).is_empty());
    shell.run("set.open", json!({ "path": path })).unwrap();
    let reopened = windows(&mut shell);
    assert_eq!(reopened.len(), 1);
    assert_eq!(reopened[0].device, scope);
    assert_eq!(reopened[0].frame, Some(frame));
    assert_eq!(reopened[0].title, expected[0].title);
    assert!(!floating(&shell, utility));
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: device-chains R30
#[test]
fn plug_ins_dont_float_and_copies_go_in_docked() {
    let (mut shell, _) = shell();
    let plugin = add(
        &mut shell,
        &json!({
            "kind": { "plugin": { "format": "clap", "plugin_id": "org.example.reverb" } },
            "name": "Reverb",
        }),
    );
    assert!(!shell.can_invoke("device.set_floating", &json!({})));
    assert!(
        shell
            .run(
                "device.set_floating",
                json!({ "device": plugin, "floating": true }),
            )
            .is_err()
    );
    let utility = add_type(&mut shell, &testing::utility_type());
    shell
        .run(
            "device.set_floating",
            json!({ "device": utility, "floating": true }),
        )
        .unwrap();
    let copy: DeviceAdded = serde_json::from_value(
        shell
            .run("device.duplicate", json!({ "device": utility }))
            .unwrap(),
    )
    .unwrap();
    assert!(!floating(&shell, copy.device));
    shell
        .run("device.copy", json!({ "device": utility }))
        .unwrap();
    let pasted: DeviceAdded =
        serde_json::from_value(shell.run("device.paste", json!({})).unwrap()).unwrap();
    assert!(!floating(&shell, pasted.device));
    assert_eq!(windows(&mut shell).len(), 1);
}

// Spec: device-chains R31
#[test]
fn with_auto_hide_on_a_window_hides_while_another_track_is_selected() {
    let (mut shell, audio) = shell();
    assert!(shell.auto_hide_plug_in_windows(), "on in a new install");
    let utility = add_type(&mut shell, &testing::utility_type());
    shell
        .run(
            "device.set_floating",
            json!({ "device": utility, "floating": true }),
        )
        .unwrap();
    assert!(windows(&mut shell)[0].shown);
    let other = shell.editor().document().top_level().nth(3).unwrap();
    assert_ne!(other, audio);
    let steps = history(&shell);
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [other] }))
        .unwrap();
    let hidden = windows(&mut shell);
    assert!(!hidden[0].shown, "hidden while another track is selected");
    // Hiding isn't docking, and isn't an undo step.
    assert!(floating(&shell, utility));
    assert_eq!(history(&shell), steps);
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    assert!(windows(&mut shell)[0].shown, "back with its track");

    // Off, the window stays whatever track is selected.
    shell
        .run(
            "settings.set_auto_hide_plug_in_windows",
            json!({ "on": false }),
        )
        .unwrap();
    assert!(!shell.auto_hide_plug_in_windows());
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [other] }))
        .unwrap();
    assert!(windows(&mut shell)[0].shown);
}

// Spec: device-chains R33
#[test]
fn plug_in_windows_hides_and_shows_every_floating_window_without_docking() {
    let (mut shell, _) = shell();
    // Disabled with no device floating, as Live's is with no plug-in window.
    assert!(!shell.can_invoke("view.toggle_plug_in_windows", &json!({})));
    let utility = add_type(&mut shell, &testing::utility_type());
    shell
        .run(
            "device.set_floating",
            json!({ "device": utility, "floating": true }),
        )
        .unwrap();
    assert!(shell.can_invoke("view.toggle_plug_in_windows", &json!({})));
    assert_eq!(
        shell.switch_state("view.toggle_plug_in_windows"),
        Some(true)
    );
    let steps = history(&shell);
    let dirty = shell.is_dirty();

    // Cmd-Option-P (Ctrl-Alt-P) and View ▸ Plug-In Windows.
    let reply = shell
        .invoke("view.toggle_plug_in_windows", json!({}))
        .unwrap();
    assert_eq!(reply, json!({ "on": false }));
    assert!(!windows(&mut shell)[0].shown, "hidden");
    assert!(floating(&shell, utility), "still floating");
    assert_eq!(history(&shell), steps, "no undo step");
    assert_eq!(shell.is_dirty(), dirty, "nothing in the set changed");
    assert_eq!(
        shell.switch_state("view.toggle_plug_in_windows"),
        Some(false)
    );
    shell
        .invoke("view.toggle_plug_in_windows", json!({}))
        .unwrap();
    assert!(windows(&mut shell)[0].shown, "shown again");

    // Floating a device shows them again.
    shell
        .run("view.toggle_plug_in_windows", json!({ "shown": false }))
        .unwrap();
    let scope = add_type(&mut shell, &testing::scope_type());
    shell
        .run(
            "device.set_floating",
            json!({ "device": scope, "floating": true }),
        )
        .unwrap();
    assert!(windows(&mut shell).iter().all(|w| w.shown));
}

// Spec: device-chains R26 (Float's key in a floating window)
#[test]
fn in_a_floating_window_float_s_key_docks_its_device() {
    let (mut shell, _) = shell();
    let utility = add_type(&mut shell, &testing::utility_type());
    let scope = add_type(&mut shell, &testing::scope_type());
    shell
        .run(
            "device.set_floating",
            json!({ "device": utility, "floating": true }),
        )
        .unwrap();
    assert_eq!(shell.devices().selected(), Some(scope));
    // Its window in front selects its device, and the Device View's keys
    // (Option-F, Ctrl-Alt-F) reach it.
    shell.set_focused_window(WindowKind::Device(utility));
    assert_eq!(shell.devices().selected(), Some(utility));
    assert!(
        shell
            .active_contexts()
            .contains(&bl_commands::Context::DeviceView)
    );
    assert!(shell.can_invoke("device.set_floating", &json!({})));
    shell.invoke("device.set_floating", json!({})).unwrap();
    assert!(!floating(&shell, utility));
    assert_eq!(history(&shell)[0], "Dock Device");
}
