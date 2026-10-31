//! The app reacts to its audio device changing (the device changes spec, R1
//! to R5): an unplugged device shows in the Status Bar and the stream reopens
//! when it's back, and a changed rate reopens the stream at the new rate.
//! Driven on the null device paced by hand, with the test clock.
//!
//! Spec: device-changes R1, R2, R3.

use std::time::Duration;

use bl_audio_io::{BackendKind, DeviceId};
use bl_shell::audio::AudioSettings;
use bl_shell::testing;
use serde_json::json;

#[test]
fn unplugging_the_device_stops_the_stream_and_replugging_reopens_it() {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    shell.run("transport.play", json!({})).unwrap();
    testing::advance(&control, &mut shell, 100);
    let before = shell.audio_mut().transport_state();
    assert!(before.playing);

    control.unplug("null");
    control.run_blocks(1);
    shell.poll();

    assert!(!shell.audio().is_running(), "the dead stream is closed");
    let status = shell.audio().status();
    assert!(status.interrupted);
    assert!(status.error.is_some());
    let message = shell.message().expect("the Status Bar says so");
    assert!(message.error);
    assert!(message.text.contains("Null device"), "{}", message.text);
    let stopped = shell.audio_mut().transport_state();
    assert!(!stopped.playing);
    assert!(stopped.position > bl_core::time::Ticks::ZERO);

    // Nothing reopens before a second has passed, or while the device is away.
    control.clock().unwrap().advance(Duration::from_millis(500));
    shell.poll();
    assert!(!shell.audio().is_running());
    control.clock().unwrap().advance(Duration::from_secs(1));
    shell.poll();
    assert!(!shell.audio().is_running(), "the device is still unplugged");

    control.replug("null");
    control.clock().unwrap().advance(Duration::from_secs(1));
    shell.poll();
    assert!(shell.audio().is_running());
    let status = shell.audio().status();
    assert!(!status.interrupted);
    assert!(status.error.is_none());
    assert!(
        shell.message().is_some_and(|m| !m.error),
        "audio is back, and the Status Bar says so"
    );
    let blocks = |shell: &bl_shell::Shell| shell.audio().stats().map_or(0, |s| s.blocks);
    let started = blocks(&shell);
    testing::run_blocks(&control, &mut shell, 4);
    assert!(blocks(&shell) >= started + 4, "the stream runs again");
    let back = shell.audio_mut().transport_state();
    assert!(!back.playing, "a recovery never starts the song");
    assert_eq!(back.position, stopped.position);
}

#[test]
fn a_sample_rate_change_reopens_the_stream_at_the_new_rate() {
    // A device chosen by name: a stream on the default device keeps running
    // through a rate change, as the system converts for it.
    let (mut shell, control) = testing::rig_with(AudioSettings {
        output_device: Some(DeviceId::new(BackendKind::Null, "null").to_string()),
        ..AudioSettings::default()
    });
    shell.start_audio();
    assert_eq!(shell.audio().sample_rate(), Some(48_000));
    testing::run_blocks(&control, &mut shell, 4);

    control.set_sample_rate("null", 44_100);
    control.run_blocks(1);
    shell.poll();

    assert_eq!(shell.audio().sample_rate(), Some(44_100));
    assert!(shell.audio().status().error.is_none());
}

#[test]
fn a_stream_that_stops_calling_back_is_restarted() {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    testing::run_blocks(&control, &mut shell, 4);
    shell.poll();
    assert!(shell.audio().is_running());

    control.clock().unwrap().advance(Duration::from_secs(4));
    shell.poll();
    assert!(shell.audio().is_running(), "it reopens at once");
    assert_eq!(shell.audio().stats().map(|s| s.blocks), Some(0));
}
