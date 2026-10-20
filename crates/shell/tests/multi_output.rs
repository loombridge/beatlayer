//! WP-5.45 through the shell: the test CLAP plug-in's audio output port past
//! its main one appears in another track's Audio From, and what it carries
//! reaches Main (the routing spec).
//!
//! The plug-in offers its extra output only where `BL_TEST_CLAP_OUTPUTS`
//! says so, and nextest runs each test in a process of its own.

bl_rt::install_trap!();

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use bl_core::time::Ticks;
use bl_model::{DeviceId, DeviceOutput, Input, Tap};
use bl_shell::export::{self, Length, Render};
use bl_shell::plugin_commands::{self as plugin_ids, PluginLoaded};
use bl_shell::routing::{InOut, in_out};
use bl_shell::{Shell, browser_commands};
use bl_test_clap::{GAIN, OUTPUTS_VAR, PLUGIN_NAME};
use serde_json::json;

/// Frames rendered: half a second at 48 kHz.
const FRAMES: u64 = 24_000;

/// What the plug-in's extra output carries, as a share of its main output
/// (the test plug-in's own).
const EXTRA: f32 = 0.25;

/// The clip's loudest sample: the ramp's 10,000 of 32,768.
const PEAK: f32 = 10_000.0 / 32_768.0;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-multi-output-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of one second at 48 kHz: a ramp that peaks
/// at [`PEAK`].
fn wav(path: &Path) {
    let frames = 48_000u32;
    let mut bytes = Vec::new();
    let data = frames * 2;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let sample = ((i % 100) as i16 - 50) * 200;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// Polls the shell, as the app's frames do, until the device's plug-in runs.
fn running(shell: &mut Shell, device: DeviceId) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        shell.poll();
        if shell.plugins().status(device).is_some_and(|s| s.active) {
            return;
        }
        assert!(Instant::now() < until, "the plug-in never ran");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Renders the set as exports do, and returns Main's first channel.
fn render(shell: &mut Shell) -> Vec<f32> {
    let spec = Render {
        start: Ticks::ZERO,
        length: Length::Frames(FRAMES),
        sample_rate: 48_000,
        engine_rate: 48_000,
        block: export::BLOCK,
    };
    let mut samples = Vec::new();
    let rendered = export::render(
        shell.editor().document(),
        shell.media_store(),
        shell.plugin_hosts(),
        &spec,
        60.0,
        &AtomicBool::new(false),
        &mut |_| {},
        &mut |block| {
            samples.extend_from_slice(block);
            Ok(())
        },
        None,
    )
    .unwrap();
    assert_eq!(rendered.heap_operations, 0, "the real-time rules");
    assert!(rendered.unhosted.is_empty(), "the plug-in renders");
    samples
}

/// The loudest sample.
fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
}

// Spec: routing R2, R12, R16. The plug-in's second stereo output is listed in
// another track's Audio From, and taking it carries that output's signal
// alone.
#[test]
fn a_plugins_second_output_reaches_another_track() {
    // SAFETY: the plug-in reads this when it is asked for its audio ports,
    // and this test runs in a process of its own.
    unsafe { std::env::set_var(OUTPUTS_VAR, "1") };
    let scratch = Scratch::new("second-output");
    let mut shell = Shell::headless();
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let file = scratch.path("Ramp.wav");
    wav(&file);
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 0 }),
        )
        .unwrap();
    let source = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.clips().next().is_some())
        .map(bl_model::Track::id)
        .expect("the clip's track");
    let loaded: PluginLoaded = serde_json::from_value(
        shell
            .run(
                plugin_ids::LOAD,
                json!({ "path": bl_test_clap::bundle(), "track": source }),
            )
            .unwrap(),
    )
    .unwrap();
    shell
        .run(
            "device.set_param",
            json!({ "device": loaded.device, "param": GAIN, "value": 1.0 }),
        )
        .unwrap();
    running(&mut shell, loaded.device);

    shell
        .invoke("track.create", json!({ "kind": "audio", "name": "Tap" }))
        .unwrap();
    let tapper = shell
        .editor()
        .document()
        .top_level()
        .last()
        .expect("the new track");
    // The channel chooser lists the plug-in's extra output once the
    // instrument's track is chosen, named by the device and the port.
    shell
        .run(
            "routing.set_input",
            json!({ "track": tapper, "input": { "type": "track", "track": source } }),
        )
        .unwrap();
    let section = in_out(shell.editor().document(), tapper, &shell.interface())
        .expect("the track's In/Out section");
    let channel = match section {
        InOut::Clips { input_channel, .. } => input_channel,
        _ => panic!("an audio track's section"),
    };
    let label = format!("{PLUGIN_NAME} | Output 2");
    assert!(
        channel.labels().contains(&label.as_str()),
        "the chooser lists it: {:?}",
        channel.labels()
    );
    // Taking it runs a command, which is the route.
    let run = channel.run_for(&label).expect("the entry runs a command");
    shell.run(&run.id, run.args.clone()).expect("it runs");
    assert_eq!(
        shell
            .editor()
            .document()
            .track(tapper)
            .unwrap()
            .routing()
            .input,
        Input::Track {
            track: source,
            tap: Tap::PostMixer,
            chain: None,
            output: Some(DeviceOutput {
                device: loaded.device,
                output: 0,
            }),
        }
    );

    // Main carries the second output alone: the ramp at a quarter of its
    // level. The instrument's own track plays nowhere, so nothing else is in
    // it.
    shell
        .run(
            "routing.set_monitor",
            json!({ "track": tapper, "monitor": "in" }),
        )
        .unwrap();
    shell
        .run(
            "routing.set_output",
            json!({ "track": source, "output": { "type": "sends_only" } }),
        )
        .unwrap();
    let samples = render(&mut shell);
    let heard = peak(&samples);
    assert!(
        (heard - PEAK * EXTRA).abs() < 1e-4,
        "the route carries the second output: peak {heard}, expected {}",
        PEAK * EXTRA
    );
}
