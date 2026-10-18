//! The external instrument in the shell (WP-5.34, `docs/specs/external-
//! devices.md` R9, R15, R16): its device type, its choosers listing the open
//! interface and the output ports, `device.set_external` and its readback,
//! and what offline renders say of it. The audio is in the engine's tests.

use std::path::PathBuf;

use bl_devices::hardware_instrument_type;
use bl_model::{Channel, DeviceId, DeviceKind, DeviceRole, HardwareLatency, TrackKind};
use bl_shell::device_commands::DeviceAdded;
use bl_shell::export_commands::{self as export_ids, ExportResult};
use bl_shell::routing::{self, Interface};
use bl_shell::{Shell, browser_commands, perception_commands};
use serde_json::{Value, json};

fn add_hardware(shell: &mut Shell) -> DeviceId {
    let first = shell.editor().document().top_level().next().unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [first] }))
        .unwrap();
    let t = hardware_instrument_type();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

// Spec: external-devices R9
#[test]
fn the_external_instrument_is_described_and_its_choosers_list_the_interface() {
    // It is an instrument of its own name with a Gain from -inf to +24 dB
    // at 0 dB, and works with hardware.
    let t = hardware_instrument_type();
    assert_eq!(t.name, "Hardware Instrument");
    assert_eq!(t.role, DeviceRole::Instrument);
    assert!(t.external && !t.sidechain);
    let gain = t
        .description
        .params
        .iter()
        .find(|p| p.name == "Gain")
        .unwrap();
    assert_eq!(gain.default, 1.0, "0 dB");
    assert!(matches!(
        &t.kind,
        DeviceKind::Native { device_type } if device_type == "beatlayer.hardware_instrument"
    ));

    // On a MIDI track, with no output, channel 1, the first pair, 0 ms.
    let mut shell = Shell::headless();
    let device = add_hardware(&mut shell);
    let document = shell.editor().document();
    let track = document.device_track(device).unwrap();
    assert_eq!(document.track(track).unwrap().kind(), TrackKind::Midi);
    let external = document.device(device).unwrap().external.clone().unwrap();
    assert_eq!(external, bl_model::External::default());
    assert_eq!(external.port, None);
    assert_eq!(
        (external.channel, external.audio_from),
        (1, Channel::Pair(1))
    );

    // The choosers list what the interface and the MIDI system offer: the
    // ports, No Output, Configure…; the single input channels, then the
    // pairs, Configure….
    let interface = Interface {
        inputs: 4,
        outputs: 4,
        midi_outputs: vec!["Synth".to_owned()],
        ..Interface::default()
    };
    let choosers = routing::external_choosers(shell.editor().document(), device, &interface)
        .expect("an external device");
    assert_eq!(
        choosers.midi_to.labels(),
        ["Synth", "No Output", "─", "Configure…"]
    );
    assert_eq!(choosers.midi_to.text, "No Output");
    assert_eq!(choosers.channel.labels().len(), 16);
    assert_eq!(choosers.channel.text, "1");
    assert_eq!(
        choosers.audio_from.labels(),
        ["1", "2", "3", "4", "1/2", "3/4", "─", "Configure…"]
    );
    assert_eq!(choosers.audio_from.text, "1/2");

    // Each entry runs the registered command, one undo step, and the
    // settings read back.
    for (chooser, label) in [
        (&choosers.midi_to, "Synth"),
        (&choosers.channel, "9"),
        (&choosers.audio_from, "3/4"),
    ] {
        let run = chooser.run_for(label).unwrap().clone();
        let steps = shell.editor().undo_history().len();
        shell.invoke(&run.id, run.args).unwrap();
        assert_eq!(shell.editor().undo_history().len(), steps + 1);
    }
    let external = shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .external
        .clone()
        .unwrap();
    assert_eq!(external.port.as_deref(), Some("Synth"));
    assert_eq!(
        (external.channel, external.audio_from),
        (9, Channel::Pair(3))
    );
    let choosers =
        routing::external_choosers(shell.editor().document(), device, &interface).unwrap();
    assert_eq!(choosers.midi_to.text, "Synth");
    assert_eq!(choosers.audio_from.text, "3/4");

    // A port that goes away stays, dimmed.
    let gone = Interface {
        midi_outputs: Vec::new(),
        ..interface
    };
    let choosers = routing::external_choosers(shell.editor().document(), device, &gone).unwrap();
    assert_eq!(choosers.midi_to.labels()[0], "Synth");
    assert!(choosers.midi_to.run_for("Synth").is_none());

    // Hardware Latency, in either unit, is refused out of range.
    for (value, ok) in [
        (json!({ "unit": "milliseconds", "value": 1000.0 }), true),
        (json!({ "unit": "milliseconds", "value": 1000.5 }), false),
        (json!({ "unit": "samples", "value": 192_000 }), true),
        (json!({ "unit": "samples", "value": 192_001 }), false),
    ] {
        let result = shell.run(
            "device.set_external",
            json!({ "device": device, "hardware_latency": value }),
        );
        assert_eq!(result.is_ok(), ok, "{value}");
    }
    assert_eq!(
        shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .external
            .as_ref()
            .unwrap()
            .hardware_latency,
        HardwareLatency::Samples { value: 192_000 }
    );
}

/// A one-second mono WAV of a quiet ramp.
fn wav(path: &PathBuf) {
    let frames = 48_000u32;
    let data = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    for field in [16u32, 1 | 1 << 16, 48_000, 96_000, 2 | 16 << 16] {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        bytes.extend_from_slice(&(((i % 100) as i16 - 50) * 20).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

// Spec: external-devices R16, R27
#[test]
fn an_offline_render_silences_the_external_devices_and_names_them() {
    let folder = std::env::temp_dir().join(format!("bl-external-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Tone.wav");
    wav(&file);

    let mut shell = Shell::headless();
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 0 }),
        )
        .unwrap();
    let device = add_hardware(&mut shell);
    let name = format!("Hardware Instrument ({device})");

    // An export sends nothing to hardware and brings back silence; the
    // result names the device.
    let result: Value = shell
        .run(
            export_ids::AUDIO,
            json!({ "path": folder.join("out.wav").to_string_lossy(), "bit_depth": 24,
                    "sample_rate": 48000 }),
        )
        .unwrap();
    let result: ExportResult = serde_json::from_value(result).unwrap();
    assert_eq!(result.silenced_external_devices, std::slice::from_ref(&name));
    assert!(result.peak_dbfs.is_some(), "the clip still plays");

    // So does a preview, in its notes.
    let preview = shell.run(perception_commands::PREVIEW, json!({})).unwrap();
    let notes = preview["rendered"]["notes"].as_array().unwrap();
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().is_some_and(|n| n.starts_with(&name))),
        "{notes:?}"
    );

    // A switched-off device is named in neither.
    shell
        .run(
            "device.set_enabled",
            json!({ "device": device, "enabled": false }),
        )
        .unwrap();
    let result = shell
        .run(
            export_ids::AUDIO,
            json!({ "path": folder.join("again.wav").to_string_lossy(), "bit_depth": 24,
                    "sample_rate": 48000 }),
        )
        .unwrap();
    assert!(result.get("silenced_external_devices").is_none());
    let _ = std::fs::remove_dir_all(&folder);
}
