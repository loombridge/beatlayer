//! The command-line flags of the `beatlayer` binary. None of these opens a
//! window, so they run in CI and on packaged builds.

use std::process::Command;

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_beatlayer"))
        .args(args)
        .output()
        .expect("the beatlayer binary runs");
    (
        output.status.code(),
        String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
    )
}

#[test]
fn version_prints_the_name_and_version() {
    let expected = format!("beatlayer {}\n", env!("CARGO_PKG_VERSION"));
    for flag in ["--version", "-V"] {
        let (code, stdout, _) = run(&[flag]);
        assert_eq!(code, Some(0), "{flag} exits successfully");
        assert_eq!(stdout, expected, "{flag}");
    }
}

#[test]
fn help_lists_the_flags() {
    let (code, stdout, _) = run(&["--help"]);
    assert_eq!(code, Some(0));
    for flag in [
        "--version",
        "--check",
        "--config-dir",
        "--screenshot",
        "[set file]",
    ] {
        assert!(stdout.contains(flag), "{flag}: {stdout}");
    }
}

#[test]
fn check_loads_everything_the_window_needs() {
    let (code, stdout, _) = run(&["--check"]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(stdout.contains("0 problems"), "{stdout}");
    assert!(stdout.contains("keymap of 343 shortcuts"), "{stdout}");
    assert!(stdout.contains("9 menus"), "{stdout}");
}

#[test]
fn unknown_options_are_refused() {
    let (code, _, stderr) = run(&["--fly"]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("unknown option --fly"), "{stderr}");
}
