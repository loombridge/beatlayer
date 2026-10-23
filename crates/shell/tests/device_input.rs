//! WP-2.13's acceptance with the test backend: the null device runs duplex,
//! and a test tone it carries on one input channel reaches only the track
//! set to that Ext. In channel (the routing spec).
//!
//! The app opens the input alongside the output when the settings name an
//! input device; the engine hands the device's input to the tracks whose
//! Audio From is Ext. In, which the tracks' meters show. The stream's
//! counters show the whole path allocates nothing: every block, input
//! included, runs under the allocation trap.

use std::time::{Duration, Instant};

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions};
use bl_model::TrackId;
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A shell on a four-channel null device, with its input open, and the
/// control over the device.
fn duplex_shell() -> (Shell, NullControl) {
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            dropout_after: None,
            channels: 4,
            ..NullOptions::default()
        },
        &[("null", "Null")],
    );
    let devices = backend.clone();
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(move |_| Ok(Box::new(devices.clone()))),
        AudioSettings::default(),
    );
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        audio,
        Box::new(ProjectFiles::new()),
    );
    shell
        .run("audio.configure", json!({ "input_device": "null:null" }))
        .unwrap();
    shell.start_audio();
    (shell, control)
}

/// An audio track whose Audio From is Ext. In `channel`, monitoring.
fn ext_in_track(shell: &mut Shell, channel: u16) -> TrackId {
    shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    let track = shell.editor().document().top_level().last().unwrap();
    shell
        .run(
            "routing.set_input",
            json!({
                "track": track,
                "input": { "type": "external", "channel": { "type": "mono", "channel": channel } },
            }),
        )
        .unwrap();
    shell
        .run(
            "routing.set_monitor",
            json!({ "track": track, "monitor": "in" }),
        )
        .unwrap();
    track
}

/// A track's peak meter as the engine last published it.
fn peak(shell: &mut Shell, track: TrackId) -> f32 {
    let mut meters = Vec::new();
    shell.audio_mut().meters_into(&mut meters);
    meters
        .iter()
        .find(|meter| meter.track == track)
        .map_or(0.0, |meter| meter.peak[0].max(meter.peak[1]))
}

/// Polls `track`'s meter until it rises past `want`, up to 5 seconds, and
/// returns the highest peak seen.
fn wait_for_peak(shell: &mut Shell, track: TrackId, want: f32) -> f32 {
    let until = Instant::now() + Duration::from_secs(5);
    let mut seen: f32 = 0.0;
    while Instant::now() < until {
        seen = seen.max(peak(shell, track));
        if seen >= want {
            return seen;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    seen
}

#[test]
fn a_tone_on_input_channel_three_reaches_only_the_track_set_to_ext_in_three() {
    let (mut shell, control) = duplex_shell();
    // The settings named an input device, so the stream runs duplex: four
    // input channels alongside the four outputs.
    let status = shell.audio().status();
    assert_eq!(status.input_channels, 4, "the interface's inputs");
    assert_eq!(status.output_channels, 4);

    let third = ext_in_track(&mut shell, 3);
    let first = ext_in_track(&mut shell, 1);
    control.set_input_tone(2, 300.0, 0.5);

    let heard = wait_for_peak(&mut shell, third, 0.4);
    assert!(
        heard > 0.4,
        "the track set to Ext. In 3 hears channel 3: {heard}"
    );
    assert_eq!(
        peak(&mut shell, first),
        0.0,
        "the track set to Ext. In 1 hears nothing"
    );
    // The whole path — the input stream, the engine, the callback — stays
    // off the heap.
    let stats = shell.audio().stats().expect("a stream is open");
    assert!(stats.blocks > 0);
    assert_eq!(stats.heap_operations, 0, "{stats:?}");

    // The tone on channel 1 instead: the other way round.
    control.set_input_tone(2, 0.0, 0.0);
    control.set_input_tone(0, 300.0, 0.5);
    let heard = wait_for_peak(&mut shell, first, 0.4);
    assert!(heard > 0.4, "the track set to Ext. In 1 hears channel 1");
}
