//! Auto Filter, the native Auto Filter, in the shell (WP-5.20; the auto-filter
//! spec, R16): a set with the device saves and reopens with every
//! parameter, and `device.parameters` reads them back. Driven through
//! `Shell::run`, as an agent drives it, on bl-project's real set files.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bl_core::time::SampleRate;
use bl_devices::auto_filter::{
    AUTO_FILTER, CLIP, FREQ, LFO_AMOUNT, LFO_HZ, TYPE, auto_filter_type,
};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{Collector, Compiler, DisplayData, EngineConfig, Registry, channel};
use bl_model::{
    Command, Device, DeviceDisplay, DeviceId, DeviceKind, DeviceRole, Editor, TrackKind,
};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::device_commands::{DeviceAdded, DeviceParameters};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-auto-filter-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn parameters(shell: &mut Shell, device: DeviceId) -> DeviceParameters {
    serde_json::from_value(
        shell
            .run("device.parameters", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

// Spec: auto-filter R16
#[test]
fn a_set_with_sweep_reopens_with_every_parameter() {
    let scratch = Scratch::new("reopen");
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let audio = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track");
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({
                    "device": {
                        "kind": DeviceKind::Native { device_type: AUTO_FILTER.to_owned() },
                        "name": "Auto Filter",
                        "role": DeviceRole::AudioEffect,
                    }
                }),
            )
            .unwrap(),
    )
    .unwrap();
    let device = added.device;

    // Every parameter moved off its default, to its range's other end.
    let description = auto_filter_type().description;
    let mut wanted = Vec::new();
    for param in &description.params {
        let to = if param.default == param.max() {
            param.min()
        } else {
            param.max()
        };
        shell
            .run(
                "device.set_param",
                json!({ "device": device, "param": param.id, "value": to }),
            )
            .unwrap();
        wanted.push((param.id, to));
    }
    assert_eq!(wanted.len(), 42);

    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run(
                "set.save_as",
                json!({ "path": scratch.0.join("Song.blset").to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap();
    let path = saved.path.expect("the set was saved");
    shell.run("set.open", json!({ "path": path })).unwrap();

    let read = parameters(&mut shell, device);
    for (id, to) in wanted {
        let state = read
            .params
            .iter()
            .find(|p| p.descriptor.id == id)
            .unwrap_or_else(|| panic!("parameter {id:?} is read back"));
        assert_eq!(state.value, to, "{} reopened", state.descriptor.name);
    }
}

// Spec: auto-filter R13
#[test]
fn sweep_publishes_its_cutoffs_and_clip_light_after_every_block() {
    for block in [1usize, 64, 512, 4096] {
        let config = EngineConfig::new(SampleRate::HZ_48000, 4096);
        let mut registry = Registry::new();
        bl_devices::register(&mut registry);
        let collector = Collector::new();
        let mut compiler = Compiler::new(config, registry, collector.handle());
        let displays = compiler.displays();
        let (mut sender, mut processor, _monitor) = channel(&config);

        let mut editor = Editor::new();
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let native = |kind: &str, params: &[(bl_model::ParamId, f64)], id| {
            let mut device = Device::new(
                id,
                "",
                DeviceKind::Native {
                    device_type: kind.to_owned(),
                },
            );
            for &(param, value) in params {
                device.params.insert(param, value);
            }
            device
        };
        let tone = native(
            TEST_TONE,
            &[(test_tone::FREQUENCY, 1_000.0), (test_tone::LEVEL, -6.0)],
            editor.new_id(),
        );
        let sweep_id = editor.new_id();
        let sweep = native(
            AUTO_FILTER,
            &[(CLIP, 1.0), (LFO_AMOUNT, 100.0), (LFO_HZ, 10.0)],
            sweep_id,
        );
        for (index, device) in [tone, sweep].into_iter().enumerate() {
            editor
                .apply(Command::InsertDevice {
                    track,
                    index,
                    device,
                })
                .unwrap();
        }
        sender.send_update(compiler.sync(editor.document()));
        assert!(sender.flush());

        let mut read = BTreeMap::new();
        let mut published = 0;
        let mut cutoffs = Vec::new();
        for _ in 0..(4096 / block).max(1) * 4 {
            bl_rt::rt_test!({ processor.process(block) });
            published += 1;
            displays.read_all(&mut read);
            let data: &DisplayData = &read[&sweep_id];
            assert_eq!(data.published, published, "block size {block}");
            assert_eq!(data.values.len(), 3);
            cutoffs.push(data.values[0]);
        }
        // A 10 Hz LFO at 100 % moves the 1 kHz cutoff ±4 octaves.
        let (low, high) = cutoffs
            .iter()
            .fold((f32::MAX, 0.0f32), |(lo, hi), c| (lo.min(*c), hi.max(*c)));
        assert!(low >= 1_000.0 / 16.5 && high <= 16_500.0, "{low} {high}");
        if block >= 64 {
            assert!(low < 300.0 && high > 3_000.0, "{low} {high}");
        }
    }
}

fn display(shell: &mut Shell, device: DeviceId) -> DeviceDisplay {
    serde_json::from_value(
        shell
            .invoke("device.display", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

// Spec: auto-filter R13
#[test]
fn an_agent_reads_the_filters_curves_from_its_parameters_and_the_newest_data() {
    let mut shell = Shell::headless();
    let types = bl_devices::device_types();
    let sweep = types
        .iter()
        .find(
            |t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == AUTO_FILTER),
        )
        .unwrap();
    shell.set_device_types(types.clone());
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Detail);
    let added: DeviceAdded = serde_json::from_value(
        shell
            .invoke(
                "device.add",
                json!({ "device": { "kind": sweep.kind, "name": sweep.name, "role": sweep.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    let device = added.device;

    // Without an engine: the low-pass at 1 kHz, flat below it and well down
    // above it, and nothing about the left and right channels.
    let idle = display(&mut shell, device);
    assert!(!idle.live && idle.series("Left Curve").is_none());
    let curve = idle.series("Filter Curve").unwrap();
    assert!(curve.log_x && curve.values.len() == 96);
    assert!(
        curve.values[0].abs() < 0.5 && curve.values[95] < -24.0,
        "{:?}",
        curve.values
    );

    // The cutoffs the engine published: left at 2 kHz, right at 500 Hz,
    // and the Clip light on.
    shell.set_device_display_data(device, &[2_000.0, 500.0, 1.0]);
    let live = display(&mut shell, device);
    assert!(live.live);
    assert_eq!(live.reading("Left Cutoff").unwrap().value, 2_000.0);
    assert_eq!(live.reading("Right Cutoff").unwrap().text, "500 Hz");
    assert_eq!(live.reading("Clip").unwrap().text, "Clipping");
    let left = live.series("Left Curve").unwrap();
    let right = live.series("Right Curve").unwrap();
    // The left curve is open further up than the right's.
    assert!(left.values[60] > right.values[60] + 6.0);
    assert_eq!(live.markers.len(), 2);

    // The type changes the curve: a high-pass is down at the bottom.
    shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": TYPE, "value": 1.0 }),
        )
        .unwrap();
    shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": FREQ, "value": 5_000.0 }),
        )
        .unwrap();
    let high = display(&mut shell, device);
    let curve = high.series("Filter Curve").unwrap();
    assert!(curve.values[0] < -24.0 && curve.values[95].abs() < 0.5);
}
