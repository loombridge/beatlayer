//! The Beatlayer application.

use std::process::ExitCode;

const USAGE: &str = "\
Usage: beatlayer [options]

Options:
  -V, --version  Print the version and exit
  -h, --help     Print this message and exit
";

fn main() -> ExitCode {
    // Only the flags are handled here. Anything else, such as a project path
    // passed by a file association, is left for the app to open once it can.
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-V" | "--version" => {
                println!("{}", version_line());
                return ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => {}
        }
    }

    println!(
        "Beatlayer {} has nothing to run yet.",
        env!("CARGO_PKG_VERSION")
    );
    ExitCode::SUCCESS
}

/// What `--version` prints: the binary's name and the package version.
/// `cargo xtask package` checks every packaged binary against this.
fn version_line() -> String {
    format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}
