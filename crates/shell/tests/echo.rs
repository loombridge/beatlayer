//! Echo in the shell (WP-8.37; the echo spec, R8, R20, R23): its node
//! publishes the lines' times after every block, `device.display` reports
//! the tunnel and the filters' response by device ID, and a set with Echo
//! saves and reopens with every parameter and its tab.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bl_core::time::SampleRate;
use bl_devices::echo::{self, ECHO, TABS, echo_type, line_param};
use bl_engine::{Collector, Compiler, DisplayData, EngineConfig, Registry, channel};
use bl_model::{
    Command, Device, DeviceDisplay, DeviceId, DeviceKind, Editor, ParamId, TrackId, TrackKind,
};
use bl_shell::audio::AudioHost;
use bl_shell::device_commands::DeviceAdded;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::Shell;
use serde_json::json;

const RATE: SampleRate = SampleRate::HZ_48000;
const MAX_FRAMES: usize = 4096;

// Spec: echo R8
#[test]
fn the_lines_times_are_published_after_every_block_at_the_tempo() {
    for block in [1usize, 64, 512, 4096] {
        let config = EngineConfig::new(RATE, MAX_FRAMES);
        let mut registry = Registry::new();
        bl_devices::register(&mut registry);
        let collector = Collector::new();
        let mut compiler = Compiler::new(config, registry, collector.handle());
        let displays = compiler.displays();
        let (mut sender, mut processor, _monitor) = channel(&config);
        let mut editor = Editor::new();
        let track: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let device: DeviceId = editor.new_id();
        let mut echo = Device::new(
            device,
            "",
            DeviceKind::Native {
                device_type: ECHO.to_owned(),
            },
        );
        // A quarter note on the left and 100 ms free on the right.
        for (param, value) in [
            (line_param(0, echo::NOTES), 4.0),
            (line_param(1, echo::SYNC), 0.0),
            (line_param(1, echo::TIME), 100.0),
        ] {
            echo.params.insert(param, value);
        }
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: echo,
            })
            .unwrap();
        sender.send_update(compiler.sync(editor.document()));
        assert!(sender.flush());
        let mut read = BTreeMap::new();
        for published in 1..=3u64 {
            bl_rt::rt_test!({ processor.process(block) });
            displays.read_all(&mut read);
            let data: &DisplayData = &read[&device];
            assert_eq!(data.published, published, "block size {block}");
            // 120 BPM: a quarter note is 500 ms.
            assert_eq!(data.values, [500.0, 100.0], "block size {block}");
        }
    }
}

fn add(shell: &mut Shell) -> DeviceId {
    shell.set_device_types(bl_devices::device_types());
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Detail);
    let t = echo_type();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .invoke(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn display(shell: &mut Shell, device: DeviceId) -> DeviceDisplay {
    serde_json::from_value(
        shell
            .invoke("device.display", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

// Spec: echo R8
#[test]
fn an_agent_reads_the_tunnel_and_the_filter_view_by_device_id() {
    let mut shell = Shell::headless();
    let device = add(&mut shell);
    let idle = display(&mut shell, device);
    assert!(!idle.live);
    assert_eq!(idle.view.as_deref(), Some("Tunnel"));
    assert!(idle.markers.iter().any(|m| m.name == "Left echo 1"));
    // The newest data the engine published replaces the default tempo's times.
    shell.set_device_display_data(device, &[300.0, 450.0]);
    let live = display(&mut shell, device);
    assert!(live.live);
    let first = live.markers.iter().find(|m| m.name == "Right echo 1").unwrap();
    assert_eq!(first.x, 450.0);
    // The second view is the filters' response, and a view it doesn't have
    // is refused. Choosing a view is no undo step.
    let steps = shell.editor().undo_history().len();
    shell
        .invoke(
            "device.set_display_view",
            json!({ "device": device, "view": "Filter" }),
        )
        .unwrap();
    let filter = display(&mut shell, device);
    assert_eq!(filter.view.as_deref(), Some("Filter"));
    assert!(filter.series("Filter").is_some());
    assert!(filter.markers.iter().all(|m| m.handle.is_some()));
    assert!(
        shell
            .invoke(
                "device.set_display_view",
                json!({ "device": device, "view": "Spectrum" }),
            )
            .is_err()
    );
    assert_eq!(shell.editor().undo_history().len(), steps);
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-echo-{}-{}",
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

fn shown(shell: &Shell, device: DeviceId) -> String {
    let frame = shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap();
    frame
        .panel
        .unwrap()
        .pages
        .into_iter()
        .find(|p| p.node == TABS)
        .unwrap()
        .shown
}

// Spec: echo R23
#[test]
fn a_set_with_echo_saves_and_reopens_with_every_parameter_and_its_tab() {
    let scratch = Scratch::new();
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let device = add(&mut shell);
    // Every parameter off its default, to the other end of its range.
    let mut wanted: Vec<(ParamId, f64)> = Vec::new();
    for param in &echo_type().description.params {
        let to = if param.default == param.max() {
            param.min()
        } else {
            param.max()
        };
        shell
            .invoke(
                "device.set_param",
                json!({ "device": device, "param": param.id, "value": to }),
            )
            .unwrap();
        wanted.push((param.id, to));
    }
    shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": TABS, "page": "Character" }),
        )
        .unwrap();
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run(
                "set.save_as",
                json!({ "path": scratch.0.join("Echo.blset").to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap();
    let path = saved.path.expect("the set was saved");
    // Another tab after the save is not what it keeps.
    shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": TABS, "page": "Modulation" }),
        )
        .unwrap();
    shell.run("set.open", json!({ "path": path })).unwrap();
    let audio = shell.editor().document().device_track(device).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    let reopened = shell.editor().document().device(device).unwrap().clone();
    for (param, value) in wanted {
        assert_eq!(reopened.params[&param], value, "{param:?} reopened");
    }
    assert_eq!(shown(&shell, device), "Character");
}
