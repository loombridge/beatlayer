//! `bl-cli devices` and `bl-cli tone` against the null device, which runs on
//! any machine, including CI runners without sound hardware.

use std::process::{Command, Output};

fn bl_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bl-cli"))
        .args(args)
        .output()
        .expect("bl-cli runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn devices_lists_the_null_device_with_rates_and_buffer_sizes() {
    let output = bl_cli(&["devices", "--backend", "null"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Null (null"), "{text}");
    assert!(
        text.contains("[0] Null device (default output, default input)"),
        "{text}"
    );
    assert!(text.contains("id:           null:null"), "{text}");
    assert!(text.contains("44100, 48000"), "{text}");
    assert!(text.contains("(running at 48000)"), "{text}");
    assert!(
        text.contains("buffer sizes: 16 to 8192 frames, default 256"),
        "{text}"
    );
}

#[test]
fn devices_lists_every_available_backend() {
    let output = bl_cli(&["devices"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Null (null"));
}

#[test]
fn tone_plays_on_the_null_device_without_dropouts() {
    let output = bl_cli(&[
        "tone",
        "--backend",
        "null",
        "--rate",
        "48000",
        "--buffer",
        "2048",
        "--seconds",
        "1",
        "--level",
        "-60",
    ]);
    let text = stdout(&output);
    assert!(output.status.success(), "{text}\n{}", stderr(&output));
    assert!(text.contains("440 Hz sine at -60.0 dBFS for 1 s"), "{text}");
    assert!(text.contains("sample rate: 48000 Hz"), "{text}");
    assert!(
        text.contains("buffer:      requested 2048 frames, backend reports 2048 frames"),
        "{text}"
    );
    assert!(text.contains("device buffers:  2048 frames"), "{text}");
    assert!(text.contains("dropouts:        0 ("), "{text}");
    assert!(text.contains("peak level:      -60.0 dBFS"), "{text}");
}

#[test]
fn tone_verbose_reports_how_the_audio_thread_runs() {
    let output = bl_cli(&[
        "tone",
        "--backend",
        "null",
        "--buffer",
        "256",
        "--seconds",
        "0.3",
        "--level",
        "-60",
        "--verbose",
    ]);
    let text = stdout(&output);
    assert!(output.status.success(), "{text}\n{}", stderr(&output));
    let report = text
        .split_once("\nAudio thread\n")
        .map(|(_, report)| report)
        .unwrap_or_else(|| panic!("no audio thread report:\n{text}"));
    assert!(report.contains("  priority:        "), "{report}");
    assert!(
        report.contains("  denormals:       flushed to zero in every block"),
        "{report}"
    );
    assert!(
        report.contains("  allocation trap: on, 0 heap operations in audio blocks"),
        "a debug build installs the trap: {report}"
    );
    assert!(report.contains("  set-ups:         1"), "{report}");
    assert!(
        text.contains("[rt log] info: audio thread set up (set-up 1 of this stream)"),
        "the audio thread's log reaches the terminal: {text}"
    );
    if cfg!(target_os = "macos") {
        assert!(
            report.contains("real-time, Mach time-constraint policy (period 5.333 ms"),
            "{report}"
        );
        assert!(
            report
                .contains("  workgroup:       joined the work interval \"Beatlayer null device\""),
            "{report}"
        );
        assert!(
            report.contains("name = Beatlayer null device"),
            "the system's own description names it: {report}"
        );
    }
    if cfg!(windows) {
        assert!(report.contains("MMCSS task \"Pro Audio\""), "{report}");
    }
}

#[test]
fn tone_without_verbose_keeps_its_report_short() {
    let output = bl_cli(&[
        "tone",
        "--backend",
        "null",
        "--seconds",
        "0.1",
        "--level",
        "-60",
    ]);
    let text = stdout(&output);
    assert!(output.status.success(), "{text}");
    assert!(!text.contains("Audio thread"), "{text}");
    assert!(!text.contains("[rt log]"), "{text}");
}

#[test]
fn tone_counts_deliberate_overruns_as_dropouts() {
    let output = bl_cli(&[
        "tone",
        "--backend",
        "null",
        "--buffer",
        "256",
        "--seconds",
        "1",
        "--glitch-every",
        "0.25",
        "--level",
        "-60",
    ]);
    let text = stdout(&output);
    assert!(!output.status.success(), "{text}");
    assert!(stderr(&output).contains("dropouts"), "{}", stderr(&output));
    let late: u64 = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("dropouts:"))
        .and_then(|rest| rest.split("late callbacks ").nth(1))
        .and_then(|rest| rest.trim_end_matches(')').parse().ok())
        .expect("a dropouts line");
    assert!(late >= 3, "{text}");
}

#[test]
fn tone_takes_the_backend_from_a_device_id() {
    let output = bl_cli(&[
        "tone",
        "--device",
        "null:null",
        "--buffer",
        "4096",
        "--seconds",
        "0.2",
        "--level=-60",
    ]);
    let text = stdout(&output);
    assert!(output.status.success(), "{text}\n{}", stderr(&output));
    assert!(
        text.contains("device:      Null device (null:null)"),
        "{text}"
    );
}

#[test]
fn tone_refuses_levels_above_full_scale() {
    let output = bl_cli(&["tone", "--backend", "null", "--level", "3"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--level must be 0 dBFS or lower"));
}

#[test]
fn tone_reports_a_buffer_the_device_cannot_take() {
    let output = bl_cli(&[
        "tone",
        "--backend",
        "null",
        "--buffer",
        "4",
        "--level",
        "-60",
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("16 to 8192 frames, not 4"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn tone_reports_an_unknown_device() {
    let output = bl_cli(&["tone", "--device", "null:other", "--level", "-60"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no device `null:other`"),
        "{}",
        stderr(&output)
    );
}
