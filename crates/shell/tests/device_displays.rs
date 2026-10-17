//! Device displays in the shell (WP-5.43; the device framework spec, R35 to
//! R38, the eq spec, R12 to R14, the compressor spec, R14 to R16): a device's
//! node publishes its display data after each block, under the allocation
//! trap, and the shell reads the newest; `device.display` reports what the
//! display shows, and the chosen view is view state.

bl_rt::install_trap!();

use std::collections::BTreeMap;

use bl_core::time::SampleRate;
use bl_devices::compressor::{COMPRESSOR, THRESHOLD};
use bl_devices::eq::{ANALYSER_BANDS, EQ};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::{Collector, Compiler, DisplayData, EngineConfig, Registry, channel};
use bl_model::{
    Command, Device, DeviceDisplay, DeviceId, DeviceKind, Editor, ParamId, TrackId, TrackKind,
};
use bl_shell::Shell;
use bl_shell::device_commands::DeviceAdded;
use serde_json::{Value, json};

const RATE: SampleRate = SampleRate::HZ_48000;
const MAX_FRAMES: usize = 4096;

fn insert(editor: &mut Editor, track: TrackId, kind: &str, params: &[(ParamId, f64)]) -> DeviceId {
    let id = editor.new_id();
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
    let index = editor.document().track(track).unwrap().devices().len();
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device,
        })
        .unwrap();
    id
}

// Spec: device-framework R35
#[test]
fn the_contours_spectrum_and_the_squeezes_gain_reduction_are_published_after_every_block() {
    for block in [1usize, 64, 512, 4096] {
        let config = EngineConfig::new(RATE, MAX_FRAMES);
        let mut registry = Registry::new();
        bl_devices::register(&mut registry);
        let collector = Collector::new();
        let mut compiler = Compiler::new(config, registry, collector.handle());
        let displays = compiler.displays();
        let (mut sender, mut processor, _monitor) = channel(&config);

        let mut editor = Editor::new();
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        insert(
            &mut editor,
            id,
            TEST_TONE,
            &[(test_tone::FREQUENCY, 1000.0), (test_tone::LEVEL, -6.0)],
        );
        let contour = insert(&mut editor, id, EQ, &[]);
        let squeeze = insert(&mut editor, id, COMPRESSOR, &[(THRESHOLD, -30.0)]);
        let update = compiler.sync(editor.document());
        sender.send_update(update);
        assert!(sender.flush());

        let mut read = BTreeMap::new();
        let mut published = 0;
        // Eight windows of the analyser, in blocks of this size.
        for _ in 0..(8 * 1024 / block).max(1) {
            bl_rt::rt_test!({ processor.process(block) });
            published += 1;
            displays.read_all(&mut read);
            for device in [contour, squeeze] {
                let data: &DisplayData = &read[&device];
                assert_eq!(
                    data.published, published,
                    "block size {block}: the display of {device} is published after every block"
                );
            }
        }

        // The spectrum has its peak in the band that holds 1 kHz, and the
        // squeezed tone is turned down.
        let spectrum = &read[&contour].values;
        assert_eq!(spectrum.len(), ANALYSER_BANDS);
        let loudest = spectrum
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        let centre = 20.0 * (20_000.0f64 / 20.0).powf((loudest as f64 + 0.5) / 64.0);
        assert!(
            (centre / 1000.0).ln().abs() < 0.2,
            "block size {block}: the loudest band is at {centre:.0} Hz"
        );
        let gain_reduction = read[&squeeze].values[0];
        assert!(
            gain_reduction < -3.0,
            "block size {block}: a -6 dBFS tone over a -30 dB threshold is turned down: {gain_reduction}"
        );
    }
}

fn add(shell: &mut Shell, kind: &str) -> DeviceId {
    let types = bl_devices::device_types();
    let device_type = types
        .iter()
        .find(|t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == kind))
        .unwrap();
    shell.set_device_types(types.clone());
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Detail);
    let device =
        json!({ "kind": device_type.kind, "name": device_type.name, "role": device_type.role });
    let added: DeviceAdded = serde_json::from_value(
        shell
            .invoke("device.add", json!({ "device": device }))
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

// Spec: device-framework R37
#[test]
fn an_agent_reads_a_display_from_its_parameters_and_from_the_newest_data() {
    let mut shell = Shell::headless();
    let contour = add(&mut shell, EQ);

    // Without an engine only what the parameters give is there: the
    // responses of the bands that are on, and a marker for each.
    let idle = display(&mut shell, contour);
    assert!(!idle.live);
    assert!(idle.series("Spectrum").is_none());
    assert!(idle.markers.is_empty(), "every band starts switched off");
    let flat = idle.series("Response").unwrap();
    assert!(flat.values.iter().all(|v| v.abs() < 1e-3));

    // A peak band at 1 kHz, boosted 12 dB.
    let first = bl_devices::eq::band_param(0, 3, bl_devices::eq::ON);
    for (param, value) in [
        (first, 1.0),
        (
            bl_devices::eq::band_param(0, 3, bl_devices::eq::FREQ),
            1000.0,
        ),
        (
            bl_devices::eq::band_param(0, 3, bl_devices::eq::GAIN_SLOT),
            12.0,
        ),
        (bl_devices::eq::band_param(0, 3, bl_devices::eq::Q), 1.0),
    ] {
        shell
            .invoke(
                "device.set_param",
                json!({ "device": contour, "param": param, "value": value }),
            )
            .unwrap();
    }
    // The newest data the engine published.
    let mut bands = vec![-100.0f32; ANALYSER_BANDS];
    bands[32] = -6.0;
    shell.set_device_display_data(contour, &bands);
    let live = display(&mut shell, contour);
    assert!(live.live);
    let marker = &live.markers[0];
    assert_eq!(
        (marker.name.as_str(), marker.x, marker.y),
        ("A4", 1000.0, 12.0)
    );
    assert!(
        marker.text.starts_with("A4 Peak 1.00 kHz"),
        "{}",
        marker.text
    );
    let sum = live.series("Response").unwrap();
    let (at, boost) = sum.peak().unwrap();
    assert!((boost - 12.0).abs() < 0.3, "a 12 dB boost: {boost}");
    assert!((at / 1000.0).ln().abs() < 0.1, "at 1 kHz: {at}");
    assert_eq!(live.series("A4 Response").unwrap().values, sum.values);
    assert!(live.series("A4 Response").unwrap().detail);
    assert_eq!(live.series("Spectrum").unwrap().values, bands);
    assert!(live.text().contains("Spectrum peak"), "{}", live.text());

    // Analyze off: no spectrum.
    shell
        .invoke(
            "device.set_param",
            json!({ "device": contour, "param": bl_devices::eq::ANALYZE, "value": 0.0 }),
        )
        .unwrap();
    assert!(display(&mut shell, contour).series("Spectrum").is_none());
}

// Spec: device-framework R38, compressor R16
#[test]
fn the_squeezes_view_is_view_state_that_agents_set_and_read() {
    let mut shell = Shell::headless();
    let squeeze = add(&mut shell, COMPRESSOR);
    let frame = |shell: &Shell| {
        shell
            .device_view_state()
            .devices
            .into_iter()
            .find(|d| d.id == squeeze)
            .unwrap()
    };
    assert_eq!(
        frame(&shell).display_view.as_deref(),
        Some("Transfer Curve")
    );
    shell.set_device_display_data(squeeze, &[-4.5, -10.0, -14.0]);
    let before = shell.editor().undo_history().count();
    shell
        .invoke(
            "device.set_display_view",
            json!({ "device": squeeze, "view": "Activity" }),
        )
        .unwrap();
    assert_eq!(frame(&shell).display_view.as_deref(), Some("Activity"));
    assert_eq!(
        shell.editor().undo_history().count(),
        before,
        "a view isn't an undo step"
    );
    let shown = display(&mut shell, squeeze);
    assert_eq!(shown.view.as_deref(), Some("Activity"));
    assert_eq!(shown.reading("Gain Reduction").unwrap().text, "-4.5 dB");
    assert_eq!(shown.reading("Input").unwrap().value, -10.0);
    assert_eq!(shown.markers.len(), 2, "the threshold and the level now");
    assert_eq!(shown.series("Transfer Curve").unwrap().values.len(), 61);
    let refused = shell.invoke(
        "device.set_display_view",
        json!({ "device": squeeze, "view": "Nonsense" }),
    );
    assert!(refused.is_err());
    // A device that draws no display has nothing to report.
    let value: Value = shell
        .invoke("device.display", json!({ "device": squeeze }))
        .unwrap();
    assert!(value["readings"].as_array().is_some());
}
