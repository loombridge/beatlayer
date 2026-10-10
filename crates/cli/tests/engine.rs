//! `bl-cli engine-smoke` against the null device, which runs on any machine,
//! including CI runners without sound hardware.

use std::process::{Command, Output};

fn bl_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bl-cli"))
        .args(args)
        .output()
        .expect("bl-cli runs")
}

#[test]
fn engine_smoke_recompiles_during_playback_without_dropouts() {
    let output = bl_cli(&[
        "engine-smoke",
        "--buffer",
        "256",
        "--seconds",
        "1",
        "--recompiles",
        "200",
    ]);
    let text = String::from_utf8_lossy(&output.stdout);
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{text}\n{errors}");
    assert!(
        text.contains("device:      Null device (null:null)"),
        "{text}"
    );
    assert!(text.contains("dropouts:        0 ("), "{text}");
    assert!(
        text.contains("heap operations: 0 on the audio thread (allocation trap on)"),
        "a debug build installs the trap: {text}"
    );
    assert!(text.contains("0 programs out of order"), "{text}");
    assert!(
        text.contains("counter node:    instance 1 counted"),
        "{text}"
    );
    // A program per edit, give or take the last few in flight.
    let swapped: u64 = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("programs:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .expect("a programs line");
    assert!(swapped > 100, "{text}");
    // Two tones at −60 dBFS through faders at −6 dB.
    let peak: f64 = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("peak level:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .expect("a peak level line");
    assert!((-61.0..=-60.0).contains(&peak), "{text}");
}

#[test]
fn engine_smoke_refuses_levels_above_full_scale() {
    let output = bl_cli(&["engine-smoke", "--level", "3"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--level must be 0 dBFS or lower"));
}
