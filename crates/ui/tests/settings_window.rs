//! The Settings window, headless: its page chooser in Live's order, and an
//! Audio page whose device and buffer changes apply to the running stream
//! without a restart, by mouse, keyboard, or assistive technology.
//!
//! Spec: app-shell R5, R6, R7.

mod common;

use bl_shell::settings::{SettingsPage, ThemeChoice};
use common::{Harness, Window};
use egui::accesskit::{Action, Role};
use egui::{Id, Key, Modifiers};
use serde_json::json;

#[test]
fn with_live_s_title_bar_drawn_the_page_chooser_sits_where_live_s_does() {
    // Spec: app-shell R14.
    // D22, L-SET-001, L-SET-002: a 562 x 617 window, the first page row 25
    // points tall from y 34, the chooser 140 points wide.
    let mut h = Harness::with_title_bar(Window::Settings);
    h.shell.invoke("app.settings", json!({})).unwrap();
    h.shell.take_effects();
    assert_eq!(h.size, egui::vec2(562.0, 617.0));
    let nodes = Harness::nodes(&h.settle());
    for (index, page) in SettingsPage::ALL.iter().enumerate() {
        let node = nodes
            .iter()
            .find(|n| n.role == Role::Tab && n.label == page.label())
            .unwrap();
        let b = node.bounds.unwrap();
        let top = 34.0 + 25.0 * index as f64;
        assert!(
            (b.y0 - top).abs() < 0.5 && (b.y1 - top - 25.0).abs() < 0.5,
            "{}: {b:?}",
            page.label()
        );
        assert!(b.x0.abs() < 0.5 && (b.x1 - 140.0).abs() < 0.5);
    }
}

fn open_settings() -> Harness {
    let mut h = Harness::manual(Window::Settings);
    h.shell.invoke("app.settings", json!({})).unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    h
}

/// Waits, driving the frames, for the stream to deliver buffers of `frames`.
fn wait_for_buffer(h: &mut Harness, frames: u32) -> Option<u32> {
    h.wait(&format!("buffers of {frames} samples"), |h| {
        h.shell.audio().status().buffer_size == Some(frames)
    });
    h.shell.audio().status().buffer_size
}

#[test]
fn the_page_chooser_lists_every_page_in_order_with_audio_first_shown() {
    let mut h = open_settings();
    let nodes = Harness::nodes(&h.frame(vec![]));
    // The tabs in tree order: a walk from the tab list.
    let list = nodes
        .iter()
        .find(|n| n.role == Role::TabList)
        .expect("a tab list");
    let mut tabs: Vec<&str> = Vec::new();
    let mut stack = vec![list.id];
    while let Some(id) = stack.pop() {
        let node = nodes.iter().find(|n| n.id == id).unwrap();
        if node.role == Role::Tab {
            tabs.push(node.label.as_str());
        }
        stack.extend(node.children.iter().rev());
    }
    let expected: Vec<&str> = SettingsPage::ALL.iter().map(|p| p.label()).collect();
    assert_eq!(tabs, expected);
    assert_eq!(h.shell.settings_page(), SettingsPage::Audio);
    for name in [
        "Driver Type",
        "Audio Input Device",
        "Audio Output Device",
        "Output Config",
        "In/Out Sample Rate",
        "Buffer Size",
        "Test Tone",
    ] {
        let node = Harness::node(&nodes, name);
        assert!(!node.disabled, "{name}");
    }
    assert_eq!(Harness::node(&nodes, "Driver Type").value, "Null");
    assert_eq!(Harness::node(&nodes, "Driver Type").role, Role::ComboBox);
    assert_eq!(
        Harness::node(&nodes, "Audio Output Device").value,
        "System Default (Null device)"
    );
    // D23: Live's tone controls, at the level and frequency of a new
    // install.
    assert_eq!(Harness::node(&nodes, "Tone Volume").value, "-36 dB");
    assert_eq!(Harness::node(&nodes, "Tone Volume").role, Role::SpinButton);
    assert_eq!(Harness::node(&nodes, "Tone Frequency").value, "440 Hz");
}

#[test]
fn a_buffer_change_from_the_keyboard_applies_without_a_restart() {
    // Spec: app-shell R6, with S-SET-003 (Up and Down change the focused
    // option).
    let mut h = open_settings();
    assert_eq!(wait_for_buffer(&mut h, 256), Some(256));
    let buffer = Id::new(("chooser", "buffer"));
    h.accesskit(buffer, Action::Focus);
    h.settle();
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(buffer));
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(h.shell.preferences().audio.buffer_size, Some(16));
    assert_eq!(
        wait_for_buffer(&mut h, 16),
        Some(16),
        "the running stream took it"
    );
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(wait_for_buffer(&mut h, 128), Some(128));
    assert_eq!(
        h.ctx.memory(|m| m.focused()),
        Some(buffer),
        "Up and Down didn't move the focus"
    );
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Buffer Size").value, "128 Samples");
    assert!(
        Harness::node(&nodes, "Running At")
            .value
            .starts_with("128 samples at 48 kHz")
    );
    assert!(h.shell.audio().is_running());
}

#[test]
fn a_device_change_from_the_list_applies_without_a_restart() {
    // Spec: app-shell R7. Open the output list and pick the device.
    let mut h = open_settings();
    let output = Id::new(("chooser", "output"));
    h.accesskit(output, Action::Click);
    let nodes = Harness::nodes(&h.settle());
    let choice = nodes
        .iter()
        .find(|n| n.label == "Null device" && n.role == Role::Button)
        .expect("the list offers the null device");
    let id = choice.id;
    h.frame(vec![egui::Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: id,
            data: None,
        },
    )]);
    h.settle();
    assert_eq!(
        h.shell.preferences().audio.output_device.as_deref(),
        Some("null:null")
    );
    let status = h.shell.audio().status();
    assert_eq!(status.output_device.unwrap().id, "null:null");
    assert!(!status.follows_default);
    assert!(h.shell.audio().is_running());
}

#[test]
fn tab_esc_and_page_keys_work_in_the_settings_window() {
    // Spec: app-shell R5 (I-FOCUS-008).
    let mut h = open_settings();
    h.key(Key::Tab, Modifiers::NONE);
    h.settle();
    assert!(
        h.ctx.memory(|m| m.focused()).is_some(),
        "Tab moved the focus onto an option"
    );
    let next_page = if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL
    };
    h.key(Key::Tab, next_page);
    assert_eq!(h.shell.settings_page(), SettingsPage::Link);
    h.key(Key::Tab, next_page | Modifiers::SHIFT);
    h.key(Key::Tab, next_page | Modifiers::SHIFT);
    assert_eq!(h.shell.settings_page(), SettingsPage::ThemeColors);
    h.settle();
    let theme = Id::new(("chooser", "theme"));
    h.accesskit(theme, Action::Focus);
    h.settle();
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(h.shell.theme(), ThemeChoice::Light);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.shell.settings_open(), "Esc closes the Settings window");
}

#[test]
fn the_test_tone_switch_plays_and_stops() {
    // Spec: app-shell R13.
    let mut h = open_settings();
    h.accesskit(Id::new(("switch", "test-tone")), Action::Click);
    h.settle();
    assert!(h.shell.audio().test_tone());
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Test Tone").role, Role::Button);

    // Tone Volume and Tone Frequency step from a screen reader and the
    // keyboard (D23). The null device plays nothing.
    let volume = Id::new(("number-box", "tone-volume"));
    h.accesskit(volume, Action::Decrement);
    h.settle();
    assert_eq!(h.shell.audio().tone().volume_db, -37.0);
    h.accesskit(volume, Action::Focus);
    h.settle();
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.key(Key::ArrowUp, Modifiers::NONE);
    h.settle();
    assert_eq!(h.shell.audio().tone().volume_db, -38.0);
    assert_eq!(h.shell.preferences().tone.volume_db, -38.0, "kept");
    let frequency = Id::new(("number-box", "tone-frequency"));
    h.accesskit(frequency, Action::Increment);
    h.settle();
    assert_eq!(
        h.shell.audio().tone().frequency_hz,
        466.0,
        "a semitone up from 440 Hz"
    );
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Tone Frequency").value, "466 Hz");

    h.key(Key::Escape, Modifiers::NONE);
    assert!(
        !h.shell.audio().test_tone(),
        "closing the window stops the tone"
    );
}

// Spec: warping R29. The Warp/Fades section's first three rows are live, in
// Live's order, each running its command and showing what the shell keeps.
#[test]
fn the_warp_fades_rows_are_live_in_lives_order() {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": "record-warp-launch" }))
        .unwrap();
    h.shell.take_effects();
    h.settle();

    let nodes = Harness::nodes(&h.frame(vec![]));
    let mut tops = Vec::new();
    for label in [
        "Loop/Warp Short Samples",
        "Auto-Warp Long Samples",
        "Default Warp Mode",
    ] {
        let node = Harness::node(&nodes, label);
        assert!(!node.disabled, "{label} is live");
        tops.push(node.bounds.unwrap().y0);
    }
    assert_eq!(
        Harness::node(&nodes, "Loop/Warp Short Samples").value,
        "Auto"
    );
    assert_eq!(Harness::node(&nodes, "Default Warp Mode").value, "Beats");
    assert_eq!(
        Harness::node(&nodes, "Auto-Warp Long Samples").toggled,
        Some(true)
    );
    let fades = Harness::node(&nodes, "Create Fades on Clip Edges");
    tops.push(fades.bounds.unwrap().y0);
    assert!(tops.windows(2).all(|pair| pair[0] < pair[1]), "{tops:?}");

    // A focused chooser steps to the next choice, a switch toggles.
    h.accesskit(
        Id::new(("chooser", "loop-warp-short-samples")),
        Action::Focus,
    );
    h.key(Key::ArrowUp, Modifiers::NONE);
    h.accesskit(Id::new(("switch", "auto-warp-long-samples")), Action::Click);
    h.accesskit(Id::new(("chooser", "default-warp-mode")), Action::Focus);
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    let prefs = h.shell.warp_prefs();
    assert_eq!(prefs.loop_warp_short_samples.label(), "Warped Loop");
    assert!(!prefs.auto_warp_long_samples);
    assert_eq!(prefs.default_warp_mode.label(), "Tones");
    for id in [
        "settings.set_loop_warp_short_samples",
        "settings.set_auto_warp_long_samples",
        "settings.set_default_warp_mode",
    ] {
        assert!(
            h.events.iter().any(|e| matches!(
                e,
                bl_ui::main_window::UiEvent::Invoke(i) if i.id == id
            )),
            "{id}"
        );
    }
}
