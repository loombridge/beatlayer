//! The setups the reachability walk's paths run before pressing a key.

use crate::common::{Harness, command};
use bl_model::ClipId;
use bl_shell::Shell;
use bl_shell::mixer_view::MixerOf;
use bl_ui::mixer::column_id;
use egui::{Key, Modifiers};
use serde_json::json;

/// A second theme for the Theme chooser to step to (themes R2).
pub(crate) fn a_second_theme(h: &mut Harness) {
    h.shell
        .set_themes(vec!["Beatlayer".to_owned(), "Other".to_owned()]);
}

/// The Shortcuts page, narrowed to Duplicate Track Without Clips, with a
/// key of the person's own on it (the custom shortcuts spec, R8).
pub(crate) fn a_bound_shortcut(h: &mut Harness) {
    let (platform, keys) = if cfg!(target_os = "macos") {
        ("mac", "Cmd+Option+D")
    } else {
        ("win", "Ctrl+Alt+D")
    };
    h.shell
        .run(
            "keymap.bind",
            json!({
                "command": "track.duplicate_without_clips",
                "keys": [keys],
                "platform": platform,
                "context": "global",
            }),
        )
        .unwrap();
    // The search field, narrowed to the command.
    h.click(egui::Pos2::new(400.0, 20.0));
    h.frame(vec![egui::Event::Text(
        "Duplicate Track Without Clips".to_owned(),
    )]);
}

/// A Session clip on the first MIDI track, selected, with the Clip View on
/// its Envelopes tab showing Pan (WP-3.16).
pub(crate) fn envelopes_tab(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().next().unwrap();
    let scene = h.shell.editor().document().scenes().next().unwrap().id;
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    h.shell
        .run("session.select", json!({ "track": track, "scene": scene }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell
        .run("view.clip_tab", json!({ "tab": "envelopes" }))
        .unwrap();
    h.shell
        .run(
            "view.clip_envelope",
            json!({ "target": { "type": "mixer", "param": "pan" } }),
        )
        .unwrap();
    h.settle();
}

/// As [`envelopes_tab`], with a breakpoint on Pan's envelope.
pub(crate) fn envelopes_tab_with_an_envelope(h: &mut Harness) {
    envelopes_tab(h);
    let track = h.shell.editor().document().top_level().next().unwrap();
    let scene = h.shell.editor().document().scenes().next().unwrap().id;
    let clip = h.shell.editor().document().slot(track, scene).unwrap().id;
    h.shell
        .run(
            "clip.set_envelope_breakpoint",
            json!({
                "clip": clip,
                "target": { "type": "mixer", "param": "pan" },
                "at": 0,
                "value": 0.25
            }),
        )
        .unwrap();
    h.settle();
}

/// A MIDI input port for the Tempo & MIDI page's table to list (WP-2.2).
pub(crate) fn a_midi_port(h: &mut Harness) {
    let test = bl_midi_io::TestBackend::new();
    test.add_port(bl_midi_io::Direction::Input, "Keys");
    h.shell.use_midi_backend(Box::new(test));
}

/// The Arrangement View has the keyboard.
pub(crate) fn arrangement_view(h: &mut Harness) {
    h.key(Key::Num2, Modifiers::ALT);
}

/// A locator is selected, so the Set Locator button reads Delete Locator.
pub(crate) fn a_locator(h: &mut Harness) {
    h.shell
        .run("locator.add", json!({ "at": 2_822_400 }))
        .unwrap();
}

pub(crate) fn arrangement_track_3(h: &mut Harness) {
    h.key(Key::Num2, Modifiers::ALT);
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
}

pub(crate) fn arrangement_track_1(h: &mut Harness) {
    h.key(Key::Num2, Modifiers::ALT);
    let track = h.shell.editor().document().top_level().next().unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
}

pub(crate) fn session_main(h: &mut Harness) {
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    let main = h.shell.editor().document().main_id();
    h.ctx
        .memory_mut(|m| m.request_focus(column_id(MixerOf::Session, main)));
    h.settle();
}

/// The shell runs audio on a four-channel null device, so the In/Out
/// choosers list two output pairs and four inputs, as an interface does
/// (WP-2.13).
pub(crate) fn four_channel_interface(h: &mut Harness) {
    use bl_audio_io::{BackendKind, NullBackend, NullOptions};
    use bl_shell::audio::{AudioHost, AudioSettings};
    use bl_shell::files::ProjectFiles;
    use bl_shell::settings::Preferences;

    let backend = NullBackend::with_options(NullOptions {
        dropout_after: None,
        channels: 4,
        ..NullOptions::default()
    });
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(move |_| Ok(Box::new(backend.clone()))),
        AudioSettings::default(),
    );
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        audio,
        Box::new(ProjectFiles::new()),
    );
    shell.start_audio();
    h.shell = shell;
}

/// A Test Gain on audio track 3, the selected track, with no device
/// selected (WP-2.4).
pub(crate) fn a_device(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_engine::testing::gain_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// As [`a_rack`], with its chain select editor shown (WP-5.3).
pub(crate) fn a_rack_with_the_zone_editor(h: &mut Harness) {
    a_rack(h);
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    let rack = h
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .devices()
        .last()
        .unwrap()
        .id;
    h.shell
        .run(
            "rack.set_zone_editor",
            json!({ "rack": rack, "editor": "select" }),
        )
        .unwrap();
}

/// The test plug-in on audio track 3, the selected track, once it offers
/// its editor, with no device selected (WP-2.10).
pub(crate) fn a_plugin(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let loaded = h
        .shell
        .run(
            "plugin.load",
            json!({ "path": bl_test_clap::bundle(), "track": track }),
        )
        .unwrap();
    let device: bl_model::DeviceId = serde_json::from_value(loaded["device"].clone()).unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !h.shell.plugin_info(device).is_some_and(|i| i.has_window) {
        assert!(
            std::time::Instant::now() < until,
            "the plug-in never offered its editor"
        );
        h.shell.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// An audio effect rack with two chains, Pad and Lead, on audio track 3, the
/// selected track, with no device selected (WP-5.2).
pub(crate) fn a_rack(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "audio_effect" } },
                "name": "Rack",
                "role": "audio_effect",
            } }),
        )
        .unwrap();
    let rack: bl_model::DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    for (index, name) in ["Pad", "Lead"].into_iter().enumerate() {
        let id: bl_model::ChainId = h.shell.new_id();
        h.shell
            .run(
                "chain.add",
                json!({ "rack": rack, "index": index, "chain": { "id": id, "name": name, "devices": [] } }),
            )
            .unwrap();
    }
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// A drum rack on the first MIDI track, the selected track, with a pad on C1
/// (a chain named Gain), a return chain named Reverb, and the rack's
/// Input/Output, Send and Return switches on, no device selected (WP-5.6).
pub(crate) fn a_drum_rack(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().next().unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "drum" } },
                "name": "Drum Rack",
                "role": "instrument",
            } }),
        )
        .unwrap();
    let rack: bl_model::DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    let t = bl_engine::testing::gain_type();
    let effect = json!({ "kind": t.kind, "name": t.name, "role": t.role });
    h.shell
        .run(
            "pad.drop",
            json!({ "rack": rack, "note": 36, "device": effect }),
        )
        .unwrap();
    h.shell
        .run(
            "pad.rename",
            json!({ "rack": rack, "note": 36, "name": "Gain" }),
        )
        .unwrap();
    let made = h
        .shell
        .run(
            "chain.create",
            json!({ "rack": rack, "device": effect, "returns": true }),
        )
        .unwrap();
    h.shell
        .run(
            "chain.rename",
            json!({ "chain": made["chain"], "name": "Reverb" }),
        )
        .unwrap();
    for view in ["input_output", "send", "returns"] {
        h.shell
            .run(
                "rack.set_view",
                json!({ "rack": rack, "view": view, "shown": true }),
            )
            .unwrap();
    }
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// The eight-slot drum sampler on the third track, the selected track, no
/// device selected (WP-5.17).
pub(crate) fn a_drum_sampler(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_devices::kitbag_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// The Test Sidechain on audio track 3, the selected track, its sidechain
/// section shown, no device selected (WP-2.19b).
pub(crate) fn a_sidechained_device(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_engine::testing::key_type();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": t.kind, "name": t.name, "role": t.role, "sidechain": true,
            } }),
        )
        .unwrap();
    h.shell
        .run(
            "device.show_sidechain",
            json!({ "device": added["device"], "shown": true }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// The Test MIDI Sidechain on audio track 3, the selected track, its
/// sidechain section shown, no device selected (WP-5.27).
pub(crate) fn a_midi_sidechained_device(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_engine::testing::hear_type();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": t.kind, "name": t.name, "role": t.role, "midi_sidechain": true,
            } }),
        )
        .unwrap();
    h.shell
        .run(
            "device.show_sidechain",
            json!({ "device": added["device"], "shown": true }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// An External Instrument on a new MIDI track, its section showing, no
/// device selected (WP-5.34).
pub(crate) fn a_hardware_device(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_devices::hardware_instrument_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// An LFO and a test utility on the selected audio track (WP-5.10),
/// with the LFO's slot 1 mapped to the utility's gain when `mapped`
/// and its Map button lit when `lit`.
pub(crate) fn modulators_on_track(h: &mut Harness, mapped: bool, lit: bool) {
    let mut types = bl_devices::testing::device_types();
    types.extend(bl_devices::device_types());
    h.shell.set_device_types(types);
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let add = |h: &mut Harness, t: &bl_engine::DeviceType| -> serde_json::Value {
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap()["device"]
            .clone()
    };
    let utility = add(h, &bl_devices::testing::utility_type());
    let wave = add(h, &bl_devices::modulator::wave_mod_type());
    if mapped {
        h.shell
            .run(
                "modulator.map",
                json!({ "device": wave, "slot": 0,
                        "target": { "type": "device", "device": utility, "param": 0 } }),
            )
            .unwrap();
    }
    if lit {
        h.shell
            .run("modulator.arm_map", json!({ "device": wave, "slot": 0 }))
            .unwrap();
    }
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

pub(crate) fn a_wave_mod(h: &mut Harness) {
    modulators_on_track(h, false, false);
}

pub(crate) fn a_mapped_wave_mod(h: &mut Harness) {
    modulators_on_track(h, true, false);
}

pub(crate) fn a_lit_map_button(h: &mut Harness) {
    modulators_on_track(h, false, true);
}

/// A Shaper on the selected audio track.
pub(crate) fn a_curve_mod(h: &mut Harness) {
    h.shell.set_device_types(bl_devices::device_types());
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_devices::modulator::curve_mod_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// A Compressor, whose display has views, on the selected audio track.
pub(crate) fn a_squeeze(h: &mut Harness) {
    h.shell.set_device_types(bl_devices::device_types());
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let types = bl_devices::device_types();
    let t = types
        .iter()
        .find(|t| t.name == "Compressor")
        .expect("the compressor is offered");
    h.shell
        .run(
            "device.add",
            json!({ "device": {
                "kind": t.kind, "name": t.name, "role": t.role, "sidechain": t.sidechain,
            } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// The framework's test effects on audio track 3, the selected track, no
/// device selected: a device with an expanded part, so it has the expand
/// toggle (WP-5.42).
pub(crate) fn an_expandable_device(h: &mut Harness) {
    h.shell
        .set_device_types(bl_devices::testing::device_types());
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = bl_devices::testing::effects_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

/// A multisampler with a layer on a MIDI track, selected, its zone editor
/// open (WP-5.13).
pub(crate) fn a_multisampler(h: &mut Harness) {
    h.shell.set_device_types(bl_devices::device_types());
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let t = bl_devices::multisampler_type();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    let dir = std::env::temp_dir().join(format!("bl-access-multi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // The second layer's name is its own: the first one names the device's
    // title.
    let files: Vec<String> = ["Alpha", "Tone"]
        .iter()
        .map(|name| {
            let file = dir.join(format!("{name}.wav"));
            std::fs::write(
                &file,
                bl_media::wav_bytes(
                    &[0.5; 4800],
                    1,
                    48_000,
                    bl_media::BitDepth::Int16,
                    bl_media::Dither::None,
                )
                .unwrap(),
            )
            .unwrap();
            file.to_string_lossy().into_owned()
        })
        .collect();
    h.shell
        .run(
            "device.add_layers",
            json!({ "device": added["device"], "paths": files }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": added["device"] }))
        .unwrap();
    h.shell
        .run(
            "device.set_multisampler_view",
            json!({ "device": added["device"], "zone_editor": true }),
        )
        .unwrap();
}

/// The keyboard in the Device View, as a click there leaves it.
pub(crate) fn device_view(h: &mut Harness) {
    h.shell.set_focused_area(bl_shell::Area::Detail);
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
}

/// The search field, open on its Commands scope.
pub(crate) fn search_commands(h: &mut Harness) {
    h.shell
        .run("search.open", json!({ "scope": "commands" }))
        .unwrap();
}

/// The search field, open on its Tracks scope.
pub(crate) fn search_tracks(h: &mut Harness) {
    h.shell
        .run("search.open", json!({ "scope": "tracks" }))
        .unwrap();
}

/// The list of the set's plug-ins, open in the detail area, with the
/// keyboard in it (IMP-29, WP-6.41).
/// The Clip View showing, as Cmd+Option+3 shows it.
pub(crate) fn clip_view_shown(h: &mut Harness) {
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
}

/// The keyboard focus in the detail area, off any control.
pub(crate) fn browser_area(h: &mut Harness) {
    h.shell.set_focused_area(bl_shell::Area::Browser);
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
}

pub(crate) fn detail_area(h: &mut Harness) {
    h.shell.set_focused_area(bl_shell::Area::Detail);
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
}

/// A MIDI clip in the first Session slot, shown in the Clip View with the
/// Transform panel showing.
pub(crate) fn midi_tools_shown(h: &mut Harness) {
    let track = h.shell.editor().document().top_level().next().unwrap();
    let scene = h.shell.editor().document().scenes().next().unwrap().id;
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    h.shell
        .run("session.select", json!({ "track": track, "scene": scene }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    // The test tool, whose controls these checks reach.
    h.shell
        .run("midi_tool.choose", json!({ "tool": "test_transpose" }))
        .unwrap();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "transform" }))
        .unwrap();
}

/// A MIDI clip in the first Session slot, shown in the Clip View on its MPE
/// tab (WP-6.18).
pub(crate) fn mpe_tab_shown(h: &mut Harness) {
    midi_tools_shown(h);
    h.shell
        .run("view.clip_tab", json!({ "tab": "mpe" }))
        .unwrap();
}

/// The same, with the shown tool's degrees off their default, so Reset can
/// be pressed.
pub(crate) fn midi_tools_changed(h: &mut Harness) {
    midi_tools_shown(h);
    h.shell
        .run(
            "midi_tool.set_auto",
            json!({ "tool": "test_transpose", "auto": false }),
        )
        .unwrap();
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "test_transpose", "values": { "degrees": 3 } }),
        )
        .unwrap();
    h.shell
        .run(
            "midi_tool.set_auto",
            json!({ "tool": "test_transpose", "auto": true }),
        )
        .unwrap();
}

/// The same, with LFO chosen and its Random wave, which has a Reseed button.
pub(crate) fn midi_tools_lfo_random(h: &mut Harness) {
    midi_tools_shown(h);
    h.shell
        .run("midi_tool.choose", json!({ "tool": "lfo" }))
        .unwrap();
    h.shell
        .run(
            "midi_tool.set",
            json!({ "tool": "lfo", "values": { "shape_type": "random" } }),
        )
        .unwrap();
}

/// The same, with the Generate panel showing Euclidean, whose Pattern tab
/// has the randomise button.
pub(crate) fn euclidean_shown(h: &mut Harness) {
    midi_tools_shown(h);
    h.shell
        .run("midi_tool.choose", json!({ "tool": "euclidean" }))
        .unwrap();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "generate" }))
        .unwrap();
}

/// The same, with the Generate panel showing Stacks and two chords in its
/// progression, so Delete Chord is enabled.
pub(crate) fn stacks_shown(h: &mut Harness) {
    midi_tools_shown(h);
    h.shell
        .run("midi_tool.choose", json!({ "tool": "stacks" }))
        .unwrap();
    h.shell
        .run(
            "midi_tool.set_auto",
            json!({ "tool": "stacks", "auto": false }),
        )
        .unwrap();
    h.shell.run("midi_tool.add_chord", json!({})).unwrap();
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "generate" }))
        .unwrap();
}

pub(crate) fn plugin_list(h: &mut Harness) {
    h.shell
        .run("view.toggle_plugin_list", json!({}))
        .expect("the list opens");
    h.shell.set_focused_area(bl_shell::Area::Detail);
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
}

/// A Simpler on the first MIDI track, that track selected, no device selected
/// (WP-5.15b): the Warp controls show in the Device View.
pub(crate) fn a_snapshot(h: &mut Harness) {
    h.shell.set_device_types(bl_devices::device_types());
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let t = bl_devices::sampler_type();
    h.shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
}

pub(crate) fn control_bar(h: &mut Harness) {
    h.key(Key::Num0, Modifiers::ALT);
}

/// A MIDI clip on the first MIDI track, selected, so the Control Bar's
/// scale controls change the clip's scale (the scales spec, R12).
pub(crate) fn a_clip_selected(h: &mut Harness) {
    let track = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == bl_model::TrackKind::Midi)
        .expect("the default set has a MIDI track")
        .id();
    let clip: ClipId = h.shell.new_id();
    h.shell
        .run(
            "clip.create",
            json!({
                "track": track,
                "clip": {
                    "id": clip,
                    "name": "",
                    "start": 0,
                    "length": 705_600,
                    "offset": 0,
                    "mute": false,
                    "data": { "midi": { "notes": [] } },
                },
            }),
        )
        .unwrap();
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
}

/// The assistant's pane open.
pub(crate) fn assistant_open(h: &mut Harness) {
    h.shell.run("view.toggle_assistant", json!({})).unwrap();
}

/// The pane open with a message sent and no turn taking it, so Stop shows.
pub(crate) fn assistant_working(h: &mut Harness) {
    assistant_open(h);
    h.shell
        .run("assistant.send", json!({ "text": "Hello" }))
        .unwrap();
}

pub(crate) fn browser_search(h: &mut Harness) {
    h.key(Key::F, command());
}

pub(crate) fn server_on(h: &mut Harness) {
    h.shell
        .run("agent.set_server", json!({ "on": true }))
        .unwrap();
}

pub(crate) fn an_agent(h: &mut Harness) {
    h.shell
        .run("agent.approve_client", json!({ "client": "Tester" }))
        .unwrap();
}

pub(crate) fn server_and_agent(h: &mut Harness) {
    server_on(h);
    an_agent(h);
}

/// An assistant API key stored in the headless keychain, so the Agents
/// page shows its Forget button.
pub(crate) fn assistant_key_set(h: &mut Harness) {
    h.shell
        .run("assistant.set_key", json!({ "key": "sk-headless-test" }))
        .unwrap();
}

/// A session that stopped without closing its set, so the recovery prompt
/// offers it (WP-6.62, the crash-recovery spec, R22).
pub(crate) fn crashed_session(h: &mut Harness) {
    let dir = std::env::temp_dir().join(format!("bl-a11y-recovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut autosave = bl_project::autosave::Autosave::start(&dir).unwrap();
    autosave.begin(None).unwrap();
    autosave.edited(&bl_project::Project::new(), "Move Clip");
    // The session ends as a crash would: its lock goes with it.
    drop(autosave);
    h.shell.use_recovery(dir).unwrap();
}
