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

#[test]
fn each_settings_command_reads_back_from_settings_state() {
    let mut client = client();
    let before = settings_state(&mut client);
    assert_eq!(before["theme"], "dark");
    assert_eq!(before["create_fades_on_clip_edges"], true);
    assert_eq!(before["warp_recorded_audio"], true);
    assert_eq!(before["auto_hide_plug_in_windows"], true);
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
        json!({ "hidden": [], "show_hidden": false })
    );
    assert_eq!(before["display_input"]["outline_view_in_focus"], true);
    assert_eq!(before["display_input"]["zoom_percent"], 100);
    assert_eq!(
        before["audio"]["device"]["driver_error_compensation_ms"],
        0.0
    );

    run(
        &mut client,
        "settings.set_theme",
        json!({ "theme": "light" }),
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
        "plugin.settings",
        json!({ "hidden": ["vst3/example"], "show_hidden": true, "format_preference": "clap" }),
    );
    run(
        &mut client,
        "settings.set_outline_view_in_focus",
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
    assert_eq!(state["theme"], "light");
    assert_eq!(state["create_fades_on_clip_edges"], false);
    assert_eq!(state["warp_recorded_audio"], false);
    assert_eq!(state["auto_hide_plug_in_windows"], false);
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
        json!({ "hidden": ["vst3/example"], "show_hidden": true, "format_preference": "clap" })
    );
    assert_eq!(
        state["display_input"],
        json!({
            "zoom_percent": 150,
            "outline_view_in_focus": false,
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
