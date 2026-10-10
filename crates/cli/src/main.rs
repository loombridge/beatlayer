//! `bl-cli`: render, inspect, convert, and benchmark projects without a UI.

// Debug builds count heap use on audio threads (the real-time rules allow
// none); `bl-cli tone` fails if it finds any.
#[cfg(debug_assertions)]
bl_rt::install_trap!();

mod devices;
mod tone;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// What a command returns: an exit code, or a message for a failure.
type Outcome = Result<ExitCode, String>;

/// Beatlayer's headless tool.
#[derive(Debug, Parser)]
#[command(name = "bl-cli", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List audio devices with their sample rates and buffer sizes.
    Devices(devices::DevicesArgs),
    /// Play a 440 Hz sine on a device and count dropouts.
    ///
    /// Exits with status 1 if the stream fails, any dropout is detected, or
    /// (in a debug build) the audio thread touches the heap.
    Tone(tone::ToneArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let outcome = match &cli.command {
        Command::Devices(args) => devices::run(args),
        Command::Tone(args) => tone::run(args),
    };
    outcome.unwrap_or_else(|message| {
        eprintln!("error: {message}");
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn negative_levels_parse_without_an_equals_sign() {
        let cli =
            Cli::try_parse_from(["bl-cli", "tone", "--level", "-60", "--seconds", "1"]).unwrap();
        let Command::Tone(args) = cli.command else {
            panic!("expected the tone command");
        };
        assert_eq!(args.level, -60.0);
        assert_eq!(args.seconds, 1.0);
    }
}
