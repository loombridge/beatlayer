//! `bl-cli device-changes --simulate`, which puts the null backend's
//! simulated interface through each device change (WP-1.13) on any machine,
//! including CI runners without sound hardware.

use std::process::{Command, Output};

fn bl_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bl-cli"))
        .args(args)
        .output()
        .expect("bl-cli runs")
}

// Spec: device-changes R1, R2, R3, R4, R5
#[test]
fn the_simulated_interface_recovers_from_every_change() {
    let output = bl_cli(&["device-changes", "--simulate"]);
    let text = String::from_utf8_lossy(&output.stdout);
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{text}\n{errors}");
    for line in [
        "The audio device \"USB Interface\" was disconnected. Audio is off until it's connected again.",
        "The sample rate of \"USB Interface\" changed from 48000 Hz to 44100 Hz.",
        "Audio stopped while the computer slept, and restarts when it wakes.",
        "The audio device \"USB Interface\" stopped responding. Audio is restarting.",
        "Audio is back on \"USB Interface\" at 48000 Hz",
        "Audio is back on \"USB Interface\" at 44100 Hz",
        "unplug and replug:  ok:",
        "sample-rate change: ok:",
        "sleep and wake:     ok:",
        "stall:              ok:",
        "heap operations:    0 on the audio thread, across 5 streams (allocation trap on)",
    ] {
        assert!(text.contains(line), "missing `{line}` in:\n{text}");
    }
}

#[test]
fn a_simulation_takes_no_real_device() {
    let output = bl_cli(&["device-changes", "--simulate", "--device", "speakers"]);
    assert!(!output.status.success());
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(errors.contains("cannot be used with"), "{errors}");
}

#[test]
fn levels_above_full_scale_are_refused() {
    let output = bl_cli(&["device-changes", "--simulate", "--level", "3"]);
    assert!(!output.status.success());
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(
        errors.contains("--level must be 0 dBFS or lower"),
        "{errors}"
    );
}
