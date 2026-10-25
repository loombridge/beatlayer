//! State the set doesn't save, read back through the agent API: every
//! `transport.set_*`, `settings.set_*` and `plugin.settings` command changes
//! something that `transport.state` or `settings.state` then reports, so an
//! agent that can set a value can read it back.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_shell::Shell;
use serde_json::{Value, json};

const BAR: i64 = 2_822_400;

fn client() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    client
}

/// Runs `command` with `args` through the agent API and returns its result.
fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    let result = client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{command} failed: {error}"));
    result["results"][0]["result"].clone()
}

fn transport_state(client: &mut Client<InProcess>) -> Value {
    run(client, "transport.state", json!({}))
}

fn settings_state(client: &mut Client<InProcess>) -> Value {
    run(client, "settings.state", json!({}))
}

#[test]
fn each_transport_setting_reads_back_from_transport_state() {
    let mut client = client();
    let before = transport_state(&mut client);
    for field in [
        "loop_on",
        "arrangement_record",
        "take_running",
        "counting_in",
        "punch_in",
        "punch_out",
        "midi_overdub",
        "automation_arm",
    ] {
        assert_eq!(before[field], false, "{field} starts off");
    }
    assert_eq!(before["recording"], Value::Null);
    assert_eq!(before["last_recording"], Value::Null);

    run(
        &mut client,
        "transport.set_loop",
        json!({ "start": BAR, "length": 2 * BAR }),
    );
    run(
        &mut client,
        "transport.set_loop_enabled",
        json!({ "enabled": true }),
    );
    run(
        &mut client,
        "transport.set_punch_in",
        json!({ "enabled": true }),
    );
    run(
        &mut client,
        "transport.set_punch_out",
        json!({ "enabled": true }),
    );
    run(
        &mut client,
        "transport.set_midi_overdub",
        json!({ "enabled": true }),
    );
    run(
        &mut client,
        "transport.set_metronome_enabled",
        json!({ "enabled": true }),
    );
    run(
        &mut client,
        "transport.set_metronome_count_in",
        json!({ "count_in": "2_bars" }),
    );
    run(
        &mut client,
        "transport.set_metronome_sound",
        json!({ "sound": "wood" }),
    );
    run(
        &mut client,
        "transport.set_metronome_rhythm",
        json!({ "rhythm": "1/8" }),
    );
    run(
        &mut client,
        "transport.set_metronome_only_while_recording",
        json!({ "only_while_recording": true }),
    );
    run(
        &mut client,
        "arrangement.automation_arm",
        json!({ "on": true }),
    );

    let state = transport_state(&mut client);
    assert_eq!(state["loop_start"], BAR);
    assert_eq!(state["loop_length"], 2 * BAR);
    assert_eq!(state["loop_on"], true);
    assert_eq!(state["punch_in"], true);
    assert_eq!(state["punch_out"], true);
    assert_eq!(state["midi_overdub"], true);
    assert_eq!(state["automation_arm"], true);
    assert_eq!(
        state["metronome"],
        json!({
            "enabled": true,
            "count_in": "2_bars",
            "sound": "wood",
            "rhythm": "1/8",
            "only_while_recording": true,
        })
    );

    // And back: each switch reads off again.
    run(
        &mut client,
        "transport.set_punch_in",
        json!({ "enabled": false }),
    );
    run(
        &mut client,
        "transport.set_midi_overdub",
        json!({ "enabled": false }),
    );
    let state = transport_state(&mut client);
    assert_eq!(state["punch_in"], false);
    assert_eq!(state["midi_overdub"], false);
    assert_eq!(state["punch_out"], true, "the other switch is untouched");
}

// Spec: recording R17. An agent sets Record Quantization, switches it off
// and back on with `on`, and reads the value and the last one other than none
// from `transport.state`.
#[test]
fn record_quantization_reads_back_from_transport_state() {
    let mut client = client();
    let state = transport_state(&mut client);
    assert_eq!(state["record_quantization"], "none");
    run(
        &mut client,
        "transport.set_record_quantization",
        json!({ "quantization": "1/8t" }),
    );
    let state = transport_state(&mut client);
    assert_eq!(state["record_quantization"], "1/8t");
    assert_eq!(state["record_quantization_last"], "1/8t");
    run(
        &mut client,
        "transport.set_record_quantization",
        json!({ "on": false }),
    );
    let state = transport_state(&mut client);
    assert_eq!(state["record_quantization"], "none");
    assert_eq!(state["record_quantization_last"], "1/8t");
    run(
        &mut client,
        "transport.set_record_quantization",
        json!({ "on": true }),
    );
    let state = transport_state(&mut client);
    assert_eq!(state["record_quantization"], "1/8t");
}

#[test]
fn each_settings_command_reads_back_from_settings_state() {
    let mut client = client();
    let before = settings_state(&mut client);
    // Spec: themes R1 to R7, R12. What a new install has.
    assert_eq!(
        before["theme_colors"],
        json!({
            "theme": "Beatlayer",
            "tone": "neutral",
            "high_contrast": false,
            "grid_line_intensity": 100,
            "brightness": 0,
            "color_intensity": 100,
            "color_hue": 0,
            "auto_assign_track_colours": true,
            "default_track_colour": 12,
            "reduced_automatic_colours": false,
            "clip_colour": "track-color",
            "appearance": "dark",
            "drawn_appearance": "dark",
            "themes": ["Beatlayer"],
        })
    );
    assert_eq!(before["create_fades_on_clip_edges"], true);
    // Spec: automation R31. On in a new install.
    assert_eq!(before["snap_automation_to_grid"], true);
    assert_eq!(before["warp_recorded_audio"], true);
    assert_eq!(before["auto_hide_plug_in_windows"], true);
    // Spec: warping R29. What a new install has.
    assert_eq!(
        before["warp"],
        json!({
            "loop_warp_short_samples": "auto",
            "auto_warp_long_samples": true,
            "default_warp_mode": "beats",
        })
    );
    assert_eq!(
        before["launch"],
        json!({
            "default_mode": "trigger",
            "default_quantization": null,
            "select_on_launch": true,
            "select_next_scene_on_launch": true,
            "start_recording_on_scene_launch": false,
        })
    );
    assert_eq!(
        before["plug_ins"],
        json!({
            "hidden": [],
            "show_hidden": false,
            "use_audio_units_v2": false,
            "use_audio_units_v3": false,
            "vst3_system_folders": false,
        })
    );
    assert_eq!(before["display_input"]["outline_view_in_focus"], true);
    assert_eq!(before["display_input"]["permanent_scrub_areas"], true);
    assert_eq!(before["display_input"]["zoom_percent"], 100);
    assert_eq!(before["automation_record_mode"], "touch");
    assert_eq!(
        transport_state(&mut client)["automation_record_mode"],
        "touch"
    );
    assert_eq!(
        before["audio"]["device"]["driver_error_compensation_ms"],
        0.0
    );

    // `light`, which earlier agents send, sets the Appearance (R12).
    run(
        &mut client,
        "settings.set_theme",
        json!({ "theme": "light" }),
    );
    run(
        &mut client,
        "settings.set_theme",
        json!({ "theme": "Beatlayer" }),
    );
    run(
        &mut client,
        "settings.set_appearance",
        json!({ "appearance": "follow-system" }),
    );
    run(&mut client, "settings.set_tone", json!({ "tone": "warm" }));
    run(
        &mut client,
        "settings.set_high_contrast",
        json!({ "on": true }),
    );
    run(
        &mut client,
        "settings.set_grid_line_intensity",
        json!({ "percent": 150 }),
    );
    run(
        &mut client,
        "settings.set_brightness",
        json!({ "value": -20 }),
    );
    run(
        &mut client,
        "settings.set_color_intensity",
        json!({ "percent": 50 }),
    );
    run(
        &mut client,
        "settings.set_color_hue",
        json!({ "degrees": -90 }),
    );
    // Spec: themes R23, R24.
    run(
        &mut client,
        "settings.set_auto_assign_track_colours",
        json!({ "on": false }),
    );
    run(
        &mut client,
        "settings.set_default_track_colour",
        json!({ "colour": 33 }),
    );
    run(
        &mut client,
        "settings.set_reduced_automatic_colours",
        json!({ "on": true }),
    );
    run(
        &mut client,
        "settings.set_clip_colour",
        json!({ "clip_colour": "random" }),
    );
    run(
        &mut client,
        "settings.set_create_fades_on_clip_edges",
        json!({ "on": false }),
    );
    run(
        &mut client,
        "settings.set_warp_recorded_audio",
        json!({ "on": false }),
    );
    run(
        &mut client,
        "settings.set_loop_warp_short_samples",
        json!({ "mode": "warped_one_shot" }),
    );
    run(
        &mut client,
        "settings.set_auto_warp_long_samples",
        json!({ "on": false }),
    );
    run(
        &mut client,
        "settings.set_default_warp_mode",
        json!({ "mode": "complex_pro" }),
    );
    run(
        &mut client,
        "settings.set_default_launch_mode",
        json!({ "mode": "gate" }),
    );
    run(
        &mut client,
        "settings.set_default_launch_quantization",
        json!({ "quantization": "1_bar" }),
    );
    run(
        &mut client,
        "settings.set_select_on_launch",
        json!({ "on": false }),
    );
    run(
        &mut client,
        "settings.set_select_next_scene_on_launch",
        json!({ "on": false }),
    );
    run(
        &mut client,
        "settings.set_start_recording_on_scene_launch",
        json!({ "on": true }),
    );
    run(
        &mut client,
        "settings.set_auto_hide_plug_in_windows",
        json!({ "on": false }),
    );
    run(
        &mut client,
        "settings.set_automation_record_mode",
        json!({ "mode": "latch" }),
    );
    run(
        &mut client,
        "settings.set_snap_automation_to_grid",
        json!({ "on": false }),
    );
    run(
        &mut client,
        "plugin.settings",
        json!({
            "hidden": ["vst3/example"],
            "show_hidden": true,
            "format_preference": "clap",
            "use_audio_units_v2": true,
            "use_audio_units_v3": true,
            "vst3_system_folders": true,
        }),
    );
    run(
        &mut client,
        "settings.set_outline_view_in_focus",
        json!({ "on": false }),
    );
    run(
        &mut client,
        "settings.set_permanent_scrub_areas",
        json!({ "on": false }),
    );
    run(&mut client, "view.set_zoom", json!({ "percent": 150 }));
    run(&mut client, "focus.toggle_tab_navigation", json!({}));
    run(&mut client, "focus.toggle_wrap_navigation", json!({}));
    run(
        &mut client,
        "audio.configure",
        json!({ "driver_error_compensation_ms": 12.345 }),
    );
    run(
        &mut client,
        "audio.set_test_tone",
        json!({ "volume_db": -20.0, "frequency_hz": 880.0 }),
    );
    run(
        &mut client,
        "midi.toggle_computer_keyboard",
        json!({ "on": true }),
    );

    let state = settings_state(&mut client);
    assert_eq!(
        state["theme_colors"],
        json!({
            "theme": "Beatlayer",
            "tone": "warm",
            "high_contrast": true,
            "grid_line_intensity": 150,
            "brightness": -20,
            "color_intensity": 50,
            "color_hue": -90,
            "auto_assign_track_colours": false,
            "default_track_colour": 33,
            "reduced_automatic_colours": true,
            "clip_colour": "random",
            "appearance": "follow-system",
            "drawn_appearance": "dark",
            "themes": ["Beatlayer"],
        })
    );
    assert_eq!(state["create_fades_on_clip_edges"], false);
    assert_eq!(state["snap_automation_to_grid"], false);
    assert_eq!(state["warp_recorded_audio"], false);
    assert_eq!(state["auto_hide_plug_in_windows"], false);
    assert_eq!(
        state["warp"],
        json!({
            "loop_warp_short_samples": "warped_one_shot",
            "auto_warp_long_samples": false,
            "default_warp_mode": "complex_pro",
        })
    );
    assert_eq!(
        state["launch"],
        json!({
            "default_mode": "gate",
            "default_quantization": "1_bar",
            "select_on_launch": false,
            "select_next_scene_on_launch": false,
            "start_recording_on_scene_launch": true,
        })
    );
    assert_eq!(
        state["plug_ins"],
        json!({
            "hidden": ["vst3/example"],
            "show_hidden": true,
            "format_preference": "clap",
            "use_audio_units_v2": true,
            "use_audio_units_v3": true,
            "vst3_system_folders": true,
        })
    );
    assert_eq!(
        state["display_input"],
        json!({
            "zoom_percent": 150,
            "outline_view_in_focus": false,
            "permanent_scrub_areas": false,
            "use_tab_key_to_move_focus": true,
            "wrap_tab_navigation": false,
        })
    );
    assert_eq!(
        state["audio"]["device"]["driver_error_compensation_ms"],
        12.35
    );
    assert_eq!(state["audio"]["tone"]["volume_db"], -20.0);
    assert_eq!(state["audio"]["tone"]["frequency_hz"], 880.0);
    assert_eq!(state["tempo_midi"]["keyboard"]["on"], true);
    assert_eq!(state["automation_record_mode"], "latch");
    assert_eq!(
        transport_state(&mut client)["automation_record_mode"],
        "latch"
    );
}

#[test]
fn settings_state_reports_secrets_only_as_stored_or_not() {
    let mut client = client();
    let state = settings_state(&mut client);
    assert_eq!(state["agents"]["token_stored"], false);
    assert_eq!(state["assistant"]["key_set"], false);
    let text = state.to_string();
    for forbidden in ["token\":\"", "\"key\""] {
        assert!(!text.contains(forbidden), "{forbidden} in {text}");
    }
}

#[test]
fn view_state_reads_back_the_view_toggles() {
    let mut client = client();
    let before = run(&mut client, "view.state", json!({}));
    assert_eq!(before["window"]["main_view"], "arrangement");
    assert_eq!(before["window"]["browser"], true);
    assert_eq!(before["clip_view"]["visible"], false);
    assert_eq!(before["clip_view"]["draw_mode"], false);

    run(&mut client, "view.toggle_session_arrangement", json!({}));
    run(&mut client, "view.toggle_browser", json!({}));
    run(&mut client, "view.toggle_info_view", json!({}));
    run(&mut client, "view.toggle_clip_view", json!({}));
    run(&mut client, "view.draw_mode", json!({}));
    let state = run(&mut client, "view.state", json!({}));
    assert_eq!(state["window"]["main_view"], "session");
    assert_eq!(state["window"]["browser"], false);
    assert_eq!(
        state["window"]["info_view"],
        !before["window"]["info_view"].as_bool().unwrap()
    );
    assert_eq!(state["clip_view"]["visible"], true);
    assert_eq!(state["clip_view"]["draw_mode"], true);
}

/// Every detail command, run by ID through the agent API, reads back from
/// `view.state`'s `detail` (the detail views spec, R17).
#[test]
fn view_state_reads_back_the_detail_area() {
    let mut client = client();
    let detail = |client: &mut _| run(client, "view.state", json!({}))["detail"].clone();
    let before = detail(&mut client);
    assert_eq!(before["clip_view"], false);
    assert_eq!(before["device_view"], true);
    assert_eq!(before["alone_last"], "device");
    assert_eq!(before["hidden"], json!([]));
    assert_eq!(before["clip_height"], 225.0);
    assert_eq!(before["expanded"], false);

    for (command, args) in [
        ("view.toggle_clip_view", json!({})),
        ("view.toggle_device_view", json!({})),
        ("view.show_detail_views", json!({})),
        ("view.switch_detail_view", json!({})),
        ("view.toggle_detail_area", json!({})),
        ("view.toggle_detail_area", json!({})),
        ("view.set_detail_height", json!({ "points": 300 })),
        ("view.expand_clip_view", json!({})),
        ("focus.device_view", json!({})),
        ("focus.clip_view", json!({})),
    ] {
        let returned = run(&mut client, command, args);
        assert_eq!(returned, detail(&mut client), "{command} returns the state");
    }
    let after = detail(&mut client);
    assert_eq!(after["clip_view"], true);
    assert_eq!(after["device_view"], true);
    assert_eq!(after["clip_height"], 300.0);
    assert_eq!(after["expanded"], true);
    assert_eq!(after["focus"], "clip");

    let hidden = run(&mut client, "view.toggle_detail_area", json!({}));
    assert_eq!(hidden["hidden"], json!(["clip", "device"]));
    assert_eq!(hidden["clip_view"], false);
}

/// `clip_view.arrange_panels` and `clip_view.fold_panels`, run by ID through
/// the agent API, read back from `view.state`'s `clip_view.panels` (the detail
/// views spec, R26).
#[test]
fn view_state_reads_back_the_clip_view_panels() {
    let mut client = client();
    let panels =
        |client: &mut _| run(client, "view.state", json!({}))["clip_view"]["panels"].clone();
    assert_eq!(
        panels(&mut client),
        json!({ "arrangement": "automatic", "in_use": "horizontal", "folded": false })
    );
    let chosen = run(
        &mut client,
        "clip_view.arrange_panels",
        json!({ "arrangement": "vertical" }),
    );
    assert_eq!(chosen, panels(&mut client));
    assert_eq!(chosen["arrangement"], "vertical");
    assert_eq!(chosen["in_use"], "vertical");
    let folded = run(&mut client, "clip_view.fold_panels", json!({}));
    assert_eq!(folded, panels(&mut client));
    assert_eq!(folded["folded"], true);
    let unfolded = run(
        &mut client,
        "clip_view.fold_panels",
        json!({ "folded": false }),
    );
    assert_eq!(unfolded["folded"], false);
}

/// The second window and the commands that act on one window, run by ID
/// through the agent API, read back from `view.state`'s `second_window` and
/// `focused_window` (the second-window spec, R15; interface-zoom R14).
#[test]
fn view_state_reads_back_the_second_window() {
    let mut client = client();
    let state = |client: &mut _| run(client, "view.state", json!({}));
    let before = state(&mut client);
    assert_eq!(before["second_window"]["open"], false);
    assert_eq!(before["focused_window"], "main");

    let opened = run(&mut client, "view.toggle_second_window", json!({}));
    let after = state(&mut client);
    assert_eq!(
        opened, after["second_window"],
        "the command returns the state"
    );
    assert_eq!(after["second_window"]["open"], true);
    assert_eq!(after["second_window"]["main_view"], "session");
    assert_eq!(after["second_window"]["browser"], false);
    assert_eq!(after["second_window"]["detail"]["device_view"], false);
    assert_eq!(after["focused_window"], "second");

    for (command, args) in [
        ("view.toggle_clip_view", json!({})),
        ("view.toggle_device_view", json!({ "window": "second" })),
        ("view.toggle_browser", json!({ "window": "second" })),
        (
            "view.set_detail_height",
            json!({ "points": 150, "window": "second" }),
        ),
        (
            "view.set_zoom",
            json!({ "percent": 130, "window": "second" }),
        ),
        ("view.zoom_in", json!({ "window": "second" })),
        ("view.full_screen", json!({ "window": "second" })),
    ] {
        run(&mut client, command, args);
    }
    let after = state(&mut client);
    let second = &after["second_window"];
    assert_eq!(second["detail"]["clip_view"], true);
    assert_eq!(second["detail"]["device_view"], true);
    assert_eq!(second["detail"]["clip_height"], 150.0);
    assert_eq!(second["browser"], true);
    assert_eq!(second["full_screen"], true);
    assert!((second["zoom"].as_f64().unwrap() - 1.4).abs() < 1e-6);
    assert_eq!(after["window"]["zoom"], 1.0, "the main window's own");
    // The views left the main window.
    assert_eq!(after["detail"]["device_view"], false);
    assert_eq!(after["window"]["full_screen"], false);

    // Swapping the views reads back in both windows.
    run(&mut client, "view.toggle_session_arrangement", json!({}));
    let swapped = state(&mut client);
    assert_eq!(swapped["window"]["main_view"], "session");
    assert_eq!(swapped["second_window"]["main_view"], "arrangement");

    run(&mut client, "view.toggle_second_window", json!({}));
    let closed = state(&mut client);
    assert_eq!(closed["second_window"]["open"], false);
    assert_eq!(closed["focused_window"], "main");
    assert_eq!(closed["detail"]["clip_view"], true, "its views came back");
    assert_eq!(closed["detail"]["device_view"], true);
}

/// Every transient edit is one undo step, run by ID, and reads back from
/// `clip.transients` (the warping spec, R23 and R24).
#[test]
fn each_transient_edit_reads_back_from_clip_transients() {
    let folder = std::env::temp_dir().join(format!("bl-agent-transients-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let loop_file = folder.join("drum-loop.wav");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/drums/drum-loop.wav"
        ),
        &loop_file,
    )
    .unwrap();

    let mut client = client();
    let media = client
        .transport()
        .shell
        .media_store()
        .import(&loop_file)
        .unwrap();
    run(
        &mut client,
        "track.create",
        json!({ "id": {"$new": "Drums"}, "kind": "audio", "name": "Drums", "index": 0 }),
    );
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.name() == "Drums")
        .unwrap()
        .id();
    run(
        &mut client,
        "clip.create",
        json!({ "track": track, "clip": {
            "id": {"$new": "Loop"}, "name": "Loop", "start": 0, "length": BAR,
            "mute": false, "data": { "audio": { "media": media.to_string() } } } }),
    );
    let clip = client
        .transport()
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .next()
        .unwrap()
        .id;
    client
        .transport()
        .shell
        .wait_analysis(bl_model::ContentHash(*media.as_bytes()))
        .unwrap();

    let steps =
        |client: &mut Client<InProcess>| client.transport().shell.editor().undo_history().len();
    let listed = |client: &mut Client<InProcess>| {
        let result = run(client, "clip.transients", json!({ "clip": clip }));
        assert_eq!(result["analysis_ready"], true);
        result["transients"].as_array().unwrap().clone()
    };
    let edit = |client: &mut Client<InProcess>, command: &str, args: Value| {
        let before = steps(client);
        run(client, command, args);
        assert_eq!(steps(client), before + 1, "{command} is one undo step");
        listed(client)
    };

    let detected = listed(&mut client);
    assert_eq!(detected.len(), 9);
    let second = detected[1]["time"].as_f64().unwrap();

    let inserted = edit(
        &mut client,
        "clip.insert_transient",
        json!({ "clip": clip, "time": 0.1 }),
    );
    assert_eq!(inserted.len(), 10);
    assert_eq!(
        inserted[1],
        json!({ "time": 0.1, "beat": null, "by_hand": true })
    );

    let deleted = edit(
        &mut client,
        "clip.delete_transients",
        json!({ "clip": clip, "times": [second] }),
    );
    assert!(deleted.iter().all(|t| t["time"].as_f64() != Some(second)));

    let moved = edit(
        &mut client,
        "clip.move_transient",
        json!({ "clip": clip, "from": 0.1, "to": 0.15 }),
    );
    assert_eq!(moved[1]["time"], 0.15);

    let reset = edit(
        &mut client,
        "clip.reset_transients",
        json!({ "clip": clip }),
    );
    assert_eq!(reset, detected);

    let _ = std::fs::remove_dir_all(&folder);
}
