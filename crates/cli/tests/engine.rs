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
    // Dropouts are wall-clock results, which other processes on a busy
    // machine can cause: they're judged at full depth (pushes to main and
    // the nightly run). Every run judges the rest.
    let full_depth = std::env::var("BL_TEST_DEPTH").as_deref() == Ok("full");
    let failures: Vec<&str> = errors
        .lines()
        .filter(|line| line.starts_with("error: "))
        .collect();
    let only_dropouts =
        !failures.is_empty() && failures.iter().all(|line| line.ends_with(" dropouts"));
    assert!(
        output.status.success() || (!full_depth && only_dropouts),
        "{text}\n{errors}"
    );
    assert!(
        text.contains("device:      Null device (null:null)"),
        "{text}"
    );
    if full_depth {
        assert!(text.contains("dropouts:        0 ("), "{text}");
    }
    assert!(
        text.contains("heap operations: 0 on the audio thread (allocation trap on)"),
        "a debug build installs the trap: {text}"
    );
    assert!(text.contains("0 programs out of order"), "{text}");
    assert!(
        text.contains("counter node:    instance 1 counted"),
        "{text}"
    );
    // The engine folds edits that arrive faster than it takes programs into
    // the next one, so a slow machine swaps fewer programs. Every recompile
    // asked for still shows up as a program (the first isn't one of them),
    // an edit synced without one, or an edit folded into a later program,
    // give or take the few in flight when the run ends.
    let line = |prefix: &str| -> Vec<&str> {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(prefix))
            .unwrap_or_else(|| panic!("a `{prefix}` line: {text}"))
            .split_whitespace()
            .collect()
    };
    let number = |words: &[&str], at: usize| -> u64 { words[at].parse().expect("a number") };
    let swapped = number(&line("programs:"), 0);
    let mixer = line("mixer edits:");
    let synced = number(&mixer, 0);
    let folded = number(
        &mixer,
        mixer.iter().position(|w| *w == "folded").unwrap() - 1,
    );
    assert!(
        swapped.saturating_sub(1) + synced + folded >= 150,
        "swapped {swapped}, synced {synced}, folded {folded} of 200 recompiles: {text}"
    );
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
