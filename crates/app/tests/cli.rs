//! The command-line flags of the `beatlayer` binary.

use std::process::Command;

fn run(args: &[&str]) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_beatlayer"))
        .args(args)
        .output()
        .expect("the beatlayer binary runs");
    (
        output.status.success(),
        String::from_utf8(output.stdout).expect("stdout is UTF-8"),
    )
}

#[test]
fn version_prints_the_name_and_version() {
    let expected = format!("beatlayer {}\n", env!("CARGO_PKG_VERSION"));
    for flag in ["--version", "-V"] {
        let (ok, stdout) = run(&[flag]);
        assert!(ok, "{flag} exits successfully");
        assert_eq!(stdout, expected, "{flag}");
    }
}

#[test]
fn help_lists_the_version_flag() {
    let (ok, stdout) = run(&["--help"]);
    assert!(ok);
    assert!(stdout.contains("--version"), "{stdout}");
}

#[test]
fn other_arguments_are_left_alone() {
    let (ok, stdout) = run(&["My Song.blset"]);
    assert!(ok);
    assert!(stdout.contains("has nothing to run yet"), "{stdout}");
}
