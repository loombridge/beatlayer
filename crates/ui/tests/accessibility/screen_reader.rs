//! What the accessibility tree gives a screen reader, and how it follows the focus.

use crate::accessibility::helpers::*;
use crate::common::Harness;
use bl_shell::mixer_view::MixerOf;
use bl_ui::arrangement::benchmark;
use bl_ui::arrangement::headers::title_id;
use bl_ui::main_window::main_view_id;
use bl_ui::mixer::{Place, column_id, fader_id, pan_id, send_id, volume_field_id};
use egui::accesskit::{self, Role};
use egui::{Id, Key, Modifiers};
use serde_json::json;
use std::collections::BTreeMap;

// Spec: accessibility R1.
#[test]
fn screen_readers_read_track_names_clip_names_and_fader_values() {
    let (mut h, tracks, clip) = set();
    let output = h.settle();
    let nodes = tree(&output);
    let node = |id: Id| nodes.get(&id.accesskit_id()).cloned().unwrap();

    // Tracks: each header's title is a row header named for the track.
    let names: Vec<String> = Harness::nodes(&output)
        .into_iter()
        .filter(|n| n.role == Role::RowHeader)
        .map(|n| n.label)
        .collect();
    for name in [
        "1 MIDI", "2 MIDI", "3 Audio", "4 Audio", "A Return", "B Return", "Main",
    ] {
        assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
    }
    let title = node(title_id(tracks[2]));
    assert_eq!(title.role(), Role::RowHeader);
    assert_eq!(title.label(), Some("3 Audio"));
    assert_eq!(title.description(), Some("Audio track 3"));

    // Clips: a list item named for the clip, with where it plays.
    let kick = node(bl_ui::arrangement::clip_id(clip));
    assert_eq!(kick.role(), Role::ListItem);
    assert_eq!(kick.label(), Some("Kick"));
    assert_eq!(kick.value(), Some("3.1.1 to 7.1.1"));
    assert_eq!(kick.is_selected(), Some(false));

    // Faders: sliders whose value is in dB, as the Status Bar shows it.
    let volume = node(volume_field_id(Place::Header, tracks[2]));
    assert_eq!(volume.role(), Role::Slider);
    assert_eq!(volume.label(), Some("Track Volume"));
    assert_eq!(volume.value(), Some("0.0 dB"));
    h.shell
        .run(
            "mixer.set_volume",
            json!({ "track": tracks[2], "volume": 0.5 }),
        )
        .unwrap();
    let nodes = tree(&h.settle());
    let volume = &nodes[&volume_field_id(Place::Header, tracks[2]).accesskit_id()];
    assert_eq!(volume.value(), Some("-6.0 dB"));
    assert!((volume.numeric_value().unwrap() + 6.02).abs() < 0.01);
    h.shell
        .run(
            "mixer.set_volume",
            json!({ "track": tracks[2], "volume": 0.0 }),
        )
        .unwrap();
    let nodes = tree(&h.settle());
    let volume = &nodes[&volume_field_id(Place::Header, tracks[2]).accesskit_id()];
    assert_eq!(volume.value(), Some("-inf dB"));
    let pan = &nodes[&pan_id(Place::Header, tracks[2]).accesskit_id()];
    assert_eq!((pan.label(), pan.value()), (Some("Track Pan"), Some("C")));

    // The Session View's mixer: a group per track, its fader in dB.
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    let nodes = tree(&h.settle());
    let column = &nodes[&column_id(MixerOf::Session, tracks[3]).accesskit_id()];
    assert_eq!(
        (column.role(), column.label()),
        (Role::Group, Some("4 Audio"))
    );
    let fader = &nodes[&fader_id(Place::Mixer(MixerOf::Session), tracks[3]).accesskit_id()];
    assert_eq!(
        (fader.role(), fader.label(), fader.value()),
        (Role::Slider, Some("Track Volume"), Some("0.0 dB"))
    );
    let fader = &nodes[&fader_id(Place::Mixer(MixerOf::Session), tracks[2]).accesskit_id()];
    assert_eq!(fader.value(), Some("-inf dB"));
}

// Spec: modulation R7. A modulated control's value reads as its base, with
// the value it plays, in one wording for a fader, the header's field, a
// send, pan and a device's knob.
#[test]
fn a_modulated_control_reads_its_base_and_played_value() {
    use bl_devices::testing::{self, UTILITY_GAIN};
    use bl_model::{AutomationTarget, MixerParam};
    use bl_shell::mixer_controls::{db_gain, send_text};
    use bl_ui::device_panel::param_control_id;

    let (mut h, tracks, _) = set();
    let t = tracks[2];
    let ret = h.shell.editor().document().returns().next().unwrap();
    h.shell.set_device_types(testing::device_types());
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [t] }))
        .unwrap();
    let utility = testing::utility_type();
    let added: bl_shell::device_commands::DeviceAdded = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": utility.kind, "name": utility.name, "role": utility.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    for (id, args) in [
        (
            "mixer.set_volume",
            json!({ "track": t, "volume": db_gain(-6.0) }),
        ),
        ("mixer.set_pan", json!({ "track": t, "pan": 0.0 })),
        (
            "mixer.set_send",
            json!({ "track": t, "to": ret, "level": 0.5 }),
        ),
        (
            "device.set_param",
            json!({ "device": added.device, "param": UTILITY_GAIN, "value": db_gain(-6.0) }),
        ),
    ] {
        h.shell.run(id, args).unwrap();
    }
    let device = |param| AutomationTarget::Device {
        device: added.device,
        param,
    };
    let mixer = |param| AutomationTarget::Mixer { param };
    let played = [
        (mixer(MixerParam::Volume), db_gain(-6.0), db_gain(-12.0)),
        (mixer(MixerParam::Pan), 0.0, 0.5),
        (AutomationTarget::Send { to: ret }, 0.5, 0.25),
        (device(UTILITY_GAIN), db_gain(-6.0), db_gain(-18.0)),
    ];
    h.shell.set_modulated(
        played
            .iter()
            .map(|&(target, base, played)| {
                let (node, param) = bl_engine::target_node(t, &target).unwrap();
                bl_engine::PlayedParam {
                    node,
                    param,
                    base,
                    played,
                }
            })
            .collect(),
    );
    let value = |nodes: &BTreeMap<accesskit::NodeId, accesskit::Node>, id: Id| {
        nodes[&id.accesskit_id()].value().map(str::to_owned)
    };
    let nodes = tree(&h.settle());
    let send = |level: f64| format!("{} dB", send_text(level));
    // The header's volume field, pan and send, and the Device View's knob.
    assert_eq!(
        value(&nodes, volume_field_id(Place::Header, t)).as_deref(),
        Some("-6.0 dB, modulated to -12.0 dB")
    );
    assert_eq!(
        value(&nodes, pan_id(Place::Header, t)).as_deref(),
        Some("C, modulated to 25R")
    );
    assert_eq!(
        value(&nodes, send_id(Place::Header, t, ret)),
        Some(format!("{}, modulated to {}", send(0.5), send(0.25)))
    );
    assert_eq!(
        value(&nodes, param_control_id(added.device, UTILITY_GAIN)).as_deref(),
        Some("-6.0 dB, modulated to -18.0 dB")
    );
    // The Session View's fader, pan and send say the same.
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    let nodes = tree(&h.settle());
    let strip = Place::Mixer(MixerOf::Session);
    assert_eq!(
        value(&nodes, fader_id(strip, t)).as_deref(),
        Some("-6.0 dB, modulated to -12.0 dB")
    );
    assert_eq!(
        value(&nodes, pan_id(strip, t)).as_deref(),
        Some("C, modulated to 25R")
    );
    assert_eq!(
        value(&nodes, send_id(strip, t, ret)),
        Some(format!("{}, modulated to {}", send(0.5), send(0.25)))
    );
    // A control that plays its base reads as before.
    h.shell.set_modulated(Vec::new());
    let nodes = tree(&h.settle());
    assert_eq!(
        value(&nodes, fader_id(strip, t)).as_deref(),
        Some("-6.0 dB")
    );
}

// Spec: accessibility R2.
#[test]
fn the_screen_reader_follows_the_keyboard_focus() {
    let (mut h, tracks, clip) = set();
    // Option-2 (Alt-2): the Arrangement View has the keyboard.
    let output = press(&mut h, Key::Num2, Modifiers::ALT);
    assert_eq!(focused_id(&h), Some(main_view_id()));
    let (_, pane) = focused(&output).unwrap();
    assert_eq!(
        (pane.role(), pane.label()),
        (Role::Pane, Some("Arrangement View"))
    );
    // Down selects a track: the screen reader reads its title.
    let output = press(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(h.shell.arrangement().selected_tracks(), [tracks[0]]);
    assert_eq!(focused_label(&output), "1 MIDI");
    press(&mut h, Key::ArrowDown, Modifiers::NONE);
    let output = press(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused_label(&output), "3 Audio");
    let (id, title) = focused(&output).unwrap();
    assert_eq!(id, title_id(tracks[2]).accesskit_id());
    assert_eq!(title.role(), Role::RowHeader);

    // A selected clip: the screen reader reads the clip.
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let output = h.settle();
    let (_, kick) = focused(&output).unwrap();
    assert_eq!(kick.label(), Some("Kick"));
    assert_eq!(kick.is_selected(), Some(true));

    // Tab: the locator buttons, then the selected track's title bar, then
    // its controls, each read as the focused control itself (the
    // accessibility spec, R10).
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    tab_navigation_on(&mut h);
    let output = press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(bl_ui::arrangement::set_locator_id()));
    assert_eq!(focused_label(&output), "Set Locator");
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(
        focused_id(&h),
        Some(bl_ui::arrangement::previous_locator_id())
    );
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(bl_ui::arrangement::next_locator_id()));
    press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(
        focused_id(&h),
        Some(bl_ui::arrangement::automation_mode_id())
    );
    let output = press(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused_id(&h), Some(title_id(tracks[2])));
    assert_eq!(focused_label(&output), "3 Audio");
    let output = press(&mut h, Key::Tab, Modifiers::NONE);
    let (_, control) = focused(&output).unwrap();
    assert_eq!(control.label(), Some("Input Type"));
    assert_eq!(control.value(), Some("Ext. In"));
    for _ in 0..5 {
        press(&mut h, Key::Tab, Modifiers::NONE);
    }
    let output = h.settle();
    let (_, control) = focused(&output).unwrap();
    assert_eq!(control.label(), Some("Track Activator"));
    assert_eq!(control.value(), Some("on"));

    // Navigate ▸ Mixer: the column, named for its track.
    let output = press(&mut h, Key::M, Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(
        focused_id(&h),
        Some(column_id(MixerOf::Arrangement, tracks[2]))
    );
    assert_eq!(focused_label(&output), "3 Audio");
}

#[test]
fn the_sample_editors_markers_reach_assistive_technology() {
    // Spec: sample-editor R20 (accessibility R4).
    use bl_ui::arrangement::peaks::MemoryPeaks;
    let (mut h, _tracks, clip) = set();
    let mut peaks = MemoryPeaks::new();
    peaks.insert(
        benchmark::media_hash(0),
        std::sync::Arc::new(
            bl_media::Peaks::from_samples(&benchmark::media_samples(
                &mut benchmark::Random::new(1),
                16.0,
            ))
            .unwrap(),
        ),
    );
    h.main
        .arrangement_mut()
        .set_peak_source(Box::new(peaks), None);
    h.shell
        .run("clip.set_warp", json!({ "clip": clip, "warp": true }))
        .unwrap();
    h.shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [clip], "time": 8.0 }),
        )
        .unwrap();
    h.shell
        .run(
            "clip.insert_transient",
            json!({ "clip": clip, "time": 4.0 }),
        )
        .unwrap();
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell
        .run("clip_view.select_warp_markers", json!({ "markers": [1] }))
        .unwrap();
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let view = Harness::node(&nodes, "Clip View").bounds.unwrap();
    let inside = |node: &crate::common::Node| {
        let b = node.bounds.unwrap();
        b.x0 >= view.x0 - 1.0 && b.x1 <= view.x1 + 1.0 && b.y0 >= view.y0 && b.y1 <= view.y1 + 1.0
    };
    // The waveform is a region naming the clip.
    let wave = nodes
        .iter()
        .find(|n| n.label.starts_with("Waveform of Kick"))
        .expect("the waveform is a region naming the clip");
    assert_eq!(wave.role, Role::Group);
    // Each marker is an item with its name, its value, and whether it is
    // selected.
    let first = Harness::node(&nodes, "Warp Marker 1");
    let second = Harness::node(&nodes, "Warp Marker 2");
    assert_eq!(first.role, Role::ListItem);
    assert_eq!(first.selected, Some(false));
    assert_eq!(second.selected, Some(true));
    assert!(
        second.value.contains("8.000 s in the file"),
        "{}",
        second.value
    );
    assert!(second.value.contains("beat 5.1.1"), "{}", second.value);
    let transient = Harness::node(&nodes, "Transient 1");
    assert!(
        transient.value.contains("4.000 s in the file"),
        "{}",
        transient.value
    );
    for name in ["Start Marker", "End Marker", "Loop Brace"] {
        let node = Harness::node(&nodes, name);
        assert_eq!(node.role, Role::ListItem, "{name}");
        assert!(!node.value.is_empty(), "{name}");
    }
    for node in [
        wave,
        first,
        second,
        transient,
        Harness::node(&nodes, "Start Marker"),
    ] {
        assert!(inside(node), "{} lies outside the Clip View", node.label);
    }
}
