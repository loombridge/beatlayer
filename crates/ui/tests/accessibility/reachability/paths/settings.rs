//! Commands in this area that have no key and no menu item, and how each is reached.

use crate::accessibility::reachability::setups::*;
use crate::accessibility::reachability::*;
use egui::Key;

pub(crate) fn entries() -> Vec<(&'static str, Path)> {
    use Path::Exception;
    use Reason::{AgentOnly, Equivalent, LaterView, Query};
    vec![
        // The mappings' commands (WP-7.1) and the map modes that make them
        // (WP-7.3).
        ("remote.mappings", Exception(Query)),
        ("remote.state", Exception(Query)),
        ("remote.targets", Exception(Query)),
        (
            "remote.select",
            Exception(Equivalent(
                "a click on a tinted control in a map mode, or Enter with the control focused",
            )),
        ),
        (
            "remote.map",
            Exception(Equivalent(
                "the next key in Key Map Mode, or the next message in MIDI Map Mode, with a \
                 control selected",
            )),
        ),
        (
            "remote.unmap",
            Exception(Equivalent(
                "Delete or Backspace with a control selected in a map mode",
            )),
        ),
        (
            "remote.set_range",
            Exception(Equivalent("the Mapping Browser's Min and Max boxes")),
        ),
        (
            "remote.invert_range",
            Exception(Equivalent("the Mapping Browser's Invert Range")),
        ),
        (
            "remote.set_mode",
            Exception(Equivalent("the Status Bar's mode chooser")),
        ),
        (
            "remote.input",
            Exception(Equivalent(
                "a mapped key, which the window sends as this command on its press and \
                 release (the remote mapping spec, R13)",
            )),
        ),
        (
            "song.set_remote_mappings",
            Exception(AgentOnly(
                "replaces the whole list; the remote.* commands change one",
            )),
        ),
        // The Control Surfaces table (WP-7.5): its choosers step with Up and
        // Down, and Dump is a button.
        ("control_surface.list", Exception(Query)),
        ("control_surface.surfaces", Exception(Query)),
        (
            "control_surface.set",
            settings(
                "tempo-midi",
                a_midi_port,
                "Control Surface 1",
                Key::ArrowDown,
            ),
        ),
        (
            "control_surface.dump",
            settings("tempo-midi", a_surface_row, "Dump 1", Key::Enter),
        ),
        // The Automation Arm button's context menu (the automation spec,
        // R18), opened from the focused button as the tempo field's is.
        (
            "settings.set_automation_record_mode",
            Exception(Equivalent(
                "the Automation Arm button's context menu (the Context Menu key or \
                 Shift-F10 with the button focused)",
            )),
        ),
        // The Settings window's controls (Tab, then Enter or Up and Down).
        (
            "agent.set_server",
            settings("agents", nothing, "Agent Server", Key::Enter),
        ),
        (
            "agent.new_token",
            settings("agents", server_on, "New Token", Key::Enter),
        ),
        (
            "agent.set_autonomous",
            settings("agents", an_agent, "Autonomous: Tester", Key::Enter),
        ),
        (
            "agent.forget_client",
            settings("agents", an_agent, "Forget Tester", Key::Enter),
        ),
        (
            "agent.new_client_token",
            settings("agents", server_and_agent, "New Token: Tester", Key::Enter),
        ),
        // The built-in assistant's settings on the Agents page (WP-2.23,
        // part 4).
        (
            "assistant.set_provider",
            settings("agents", nothing, "Assistant Provider", Key::ArrowDown),
        ),
        (
            "assistant.set_base_url",
            Exception(Equivalent(
                "the Base URL field, typed into and then committed with Enter or by leaving \
                 it: it sends its text once, so a typed space isn't trimmed away",
            )),
        ),
        (
            "assistant.set_model",
            Exception(Equivalent(
                "the Model field, typed into and then committed with Enter or by leaving \
                 it: it sends its text once, so a typed space isn't trimmed away",
            )),
        ),
        (
            "assistant.set_key",
            Exception(Equivalent(
                "the API key field, typed into and then stored with Set: the key has no one \
                 key of its own, since it changes the keychain",
            )),
        ),
        (
            "assistant.forget_key",
            settings(
                "agents",
                assistant_key_set,
                "Forget Assistant API Key",
                Key::Enter,
            ),
        ),
        (
            "assistant.toggle_activity_log",
            settings("agents", nothing, "Open Assistant Activity Log", Key::Enter),
        ),
        // The built-in assistant's panel (WP-2.23, part 3; D24): the pane's
        // controls, reached from the Control Bar with Tab.
        (
            "assistant.toggle_floating",
            main(assistant_open, nothing, "Float Assistant", Key::Enter),
        ),
        (
            "assistant.stop",
            main(assistant_working, nothing, "Stop Assistant", Key::Enter),
        ),
        (
            "assistant.set_panel_width",
            main(assistant_open, nothing, "Assistant Width", Key::ArrowLeft),
        ),
        (
            "assistant.send",
            Exception(Equivalent(
                "the prompt field, typed into and then sent with Enter, or with the Send button                  after it: sending needs a message, so there is no one key",
            )),
        ),
        (
            "audio.configure",
            settings("audio", nothing, "Buffer Size", Key::ArrowDown),
        ),
        (
            "audio.set_test_tone",
            settings("audio", nothing, "Test Tone", Key::Enter),
        ),
        (
            "settings.set_theme",
            settings("theme-colors", a_second_theme, "Theme", Key::ArrowDown),
        ),
        (
            "settings.set_appearance",
            settings("theme-colors", nothing, "Appearance", Key::ArrowDown),
        ),
        (
            "settings.set_tone",
            settings("theme-colors", nothing, "Tone", Key::ArrowDown),
        ),
        (
            "settings.set_high_contrast",
            settings("theme-colors", nothing, "High Contrast", Key::Enter),
        ),
        (
            "settings.set_grid_line_intensity",
            settings("theme-colors", nothing, "Grid Line Intensity", Key::ArrowUp),
        ),
        (
            "settings.set_brightness",
            settings("theme-colors", nothing, "Brightness", Key::ArrowUp),
        ),
        (
            "settings.set_color_intensity",
            settings("theme-colors", nothing, "Color Intensity", Key::ArrowUp),
        ),
        (
            "settings.set_color_hue",
            settings("theme-colors", nothing, "Color Hue", Key::ArrowUp),
        ),
        // The Track and Clip Colors rows (the themes spec, R23): Default Track
        // Color waits for Auto-Assign Track Colors to be off.
        (
            "settings.set_auto_assign_track_colours",
            settings(
                "theme-colors",
                nothing,
                "Auto-Assign Track Colors",
                Key::Enter,
            ),
        ),
        (
            "settings.set_default_track_colour",
            settings(
                "theme-colors",
                auto_assign_off,
                "Default Track Color",
                Key::ArrowDown,
            ),
        ),
        (
            "settings.set_reduced_automatic_colours",
            settings(
                "theme-colors",
                nothing,
                "Reduced Automatic Colors",
                Key::Enter,
            ),
        ),
        (
            "settings.set_clip_colour",
            settings("theme-colors", nothing, "Clip Color", Key::ArrowDown),
        ),
        (
            "settings.set_create_fades_on_clip_edges",
            settings(
                "record-warp-launch",
                nothing,
                "Create Fades on Clip Edges",
                Key::Enter,
            ),
        ),
        (
            "settings.set_temporary_folder",
            settings("file-folder", nothing, "Browse", Key::Enter),
        ),
        (
            "settings.set_takeover_mode",
            settings("tempo-midi", nothing, "Takeover Mode", Key::ArrowDown),
        ),
        (
            "settings.set_collect_files_on_export",
            settings(
                "library",
                nothing,
                "Collect Files on Export",
                Key::ArrowDown,
            ),
        ),
        (
            "settings.set_user_library",
            settings("library", nothing, "Browse", Key::Enter),
        ),
        (
            "settings.set_warp_recorded_audio",
            settings(
                "record-warp-launch",
                nothing,
                "Warp recorded audio",
                Key::Enter,
            ),
        ),
        (
            "settings.set_loop_warp_short_samples",
            settings(
                "record-warp-launch",
                nothing,
                "Loop/Warp Short Samples",
                Key::ArrowUp,
            ),
        ),
        (
            "settings.set_auto_warp_long_samples",
            settings(
                "record-warp-launch",
                nothing,
                "Auto-Warp Long Samples",
                Key::Enter,
            ),
        ),
        (
            "settings.set_default_warp_mode",
            settings(
                "record-warp-launch",
                nothing,
                "Default Warp Mode",
                Key::ArrowDown,
            ),
        ),
        (
            "settings.set_default_launch_mode",
            settings(
                "record-warp-launch",
                nothing,
                "Default Launch Mode",
                Key::ArrowDown,
            ),
        ),
        (
            "settings.set_default_launch_quantization",
            settings(
                "record-warp-launch",
                nothing,
                "Default Launch Quantization",
                Key::ArrowDown,
            ),
        ),
        (
            "settings.set_select_on_launch",
            settings(
                "record-warp-launch",
                nothing,
                "Select on Launch",
                Key::Enter,
            ),
        ),
        (
            "settings.set_select_next_scene_on_launch",
            settings(
                "record-warp-launch",
                nothing,
                "Select Next Scene on Launch",
                Key::Enter,
            ),
        ),
        (
            "settings.set_start_recording_on_scene_launch",
            settings(
                "record-warp-launch",
                nothing,
                "Start Recording on Scene Launch",
                Key::Enter,
            ),
        ),
        (
            "settings.set_record_session_automation_in",
            settings(
                "record-warp-launch",
                nothing,
                "Record Session automation in",
                Key::ArrowDown,
            ),
        ),
        (
            "settings.set_auto_hide_plug_in_windows",
            settings("plug-ins", nothing, "Auto-Hide Plug-In Windows", Key::Enter),
        ),
        (
            "settings.set_outline_view_in_focus",
            settings(
                "display-input",
                nothing,
                "Outline View in Focus",
                Key::Enter,
            ),
        ),
        (
            "settings.set_permanent_scrub_areas",
            settings(
                "display-input",
                nothing,
                "Permanent Scrub Areas",
                Key::Enter,
            ),
        ),
        // The Shortcuts page (the custom shortcuts spec, R8 and R10): the
        // Remove and Reset buttons.
        (
            "keymap.unbind",
            settings(
                "shortcuts",
                a_bound_shortcut,
                "Remove Duplicate Track Without Clips's keys",
                Key::Enter,
            ),
        ),
        (
            "keymap.reset",
            settings(
                "shortcuts",
                a_bound_shortcut,
                "Reset Duplicate Track Without Clips's keys",
                Key::Enter,
            ),
        ),
        (
            "keymap.bind",
            Exception(Equivalent(
                "the Shortcuts page's keys cell takes the key press that becomes the shortcut, so \
                 the command's keyboard path is the shortcut itself (the custom shortcuts spec, \
                 R8)",
            )),
        ),
        ("settings.state", Exception(Query)),
        ("audio.status", Exception(Query)),
        ("audio.devices", Exception(Query)),
        ("keymap.state", Exception(Query)),
        // Agent perception (WP-1.16): how an agent hears and sees the set,
        // which a person does with their own ears and eyes.
        (
            "perception.preview",
            Exception(AgentOnly("an agent's audio preview of a range or track")),
        ),
        (
            "perception.analyze",
            Exception(AgentOnly("an agent's analysis of the set's audio as text")),
        ),
        (
            "perception.capture_view",
            Exception(AgentOnly("an agent's capture of a view as an image")),
        ),
        (
            "agent.approve_client",
            Exception(Equivalent(
                "the system dialog that asks when an agent first connects, Allow",
            )),
        ),
        (
            "settings.set_sample_editor",
            Exception(LaterView(
                "the File & Folder page's Sample Editor Browse button, the second Browse on the \
                 page (WP-4.10b)",
            )),
        ),
        (
            "settings.set_continuous_save",
            settings("file-folder", nothing, "Save Continuously", Key::Enter),
        ),
    ]
}

/// Auto-Assign Track Colors off, which enables Default Track Color.
fn auto_assign_off(h: &mut crate::common::Harness) {
    h.shell
        .run(
            "settings.set_auto_assign_track_colours",
            serde_json::json!({ "on": false }),
        )
        .unwrap();
}
