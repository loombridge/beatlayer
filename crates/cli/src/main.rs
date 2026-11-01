//! `bl-cli`: render, inspect, convert, and benchmark projects without a UI,
//! and serve the agent API (`bl-cli mcp`).

// Debug builds count heap use on audio threads (the real-time rules allow
// none); `bl-cli tone`, `render`, and `golden` fail if they find any.
#[cfg(debug_assertions)]
bl_rt::install_trap!();

mod device_changes;
mod devices;
mod engine_smoke;
mod export;
mod mcp;
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
    /// Play a small song through the engine while recompiling it, and count
    /// dropouts and swaps.
    ///
    /// Two test tones and a counter node, on the null device unless another
    /// is named. Exits with status 1 if the stream fails, any dropout is
    /// detected, the audio thread touches the heap (in a debug build), or
    /// the counter node loses its state in a swap.
    EngineSmoke(engine_smoke::EngineSmokeArgs),
    /// Play a quiet tone through the engine and report what happens to the
    /// device: unplugging, sample-rate changes, sleep and wake.
    ///
    /// On a real device it plays for --seconds while someone changes the
    /// device, printing each message and recovery. With --simulate it puts
    /// the null backend's simulated interface through each change in turn
    /// and checks the engine recovers. Exits with status 1 if a check fails,
    /// the device isn't running at the end, or (in a debug build) the audio
    /// thread touches the heap.
    DeviceChanges(device_changes::DeviceChangesArgs),
    /// Render a set to a WAV file, faster than real time, with no audio
    /// device.
    ///
    /// The song plays from its start through the same engine code a device
    /// runs. The same set renders to the same samples every time. Exits with
    /// status 1 if a sample isn't finite or (in a debug build) the engine
    /// touches the heap inside a block.
    Render(bl_cli::render::RenderArgs),
    /// Export a set's Main output to a WAV, AIFF, or FLAC file, faster
    /// than real time, with no audio device.
    ///
    /// Runs `export.audio`, the command agents run, on a headless shell:
    /// the range (the whole set unless --start, --bars, or --length say
    /// otherwise) plays straight through the same engine code a device
    /// runs, then is converted, normalised, dithered, and encoded as asked.
    /// The file appears only when it's complete; an existing one is
    /// replaced only with --replace.
    Export(export::ExportArgs),
    /// Render the golden fixtures and null-test them against their
    /// references in tests/golden.
    ///
    /// A render must be bit-exact on the platform that made its reference,
    /// and below −96 dBFS elsewhere. Exits with status 1 if any fails. With
    /// --update, remakes the references whose renders changed: say why in
    /// the pull request.
    Golden(bl_cli::golden::GoldenArgs),
    /// Measure how accurately Convert Melody and Convert Harmony find notes.
    ///
    /// Seeded phrases with known notes are played through Drift and Simpler,
    /// clean, with a room and with noise, transcribed, and scored by
    /// mir_eval's note definitions. Prints a table of precision, recall and
    /// F-measure, and exits with status 1 if an onset F-measure is under its
    /// target. `BL_TRANSCRIBE_CORPUS` names the folder of an open labelled
    /// set to report on as well.
    TranscribeBench(bl_cli::transcribe_bench::BenchArgs),
    /// Serve the agent API (MCP) on standard input and output, with no
    /// window.
    ///
    /// An agent client starts it and sends JSON-RPC messages, one per line.
    /// Commands run on a headless shell holding the factory default set, or
    /// the set --set names. Destructive and external commands are refused
    /// unless --autonomous is given, because there's no one to ask.
    Mcp(mcp::McpArgs),
    /// Run one turn of the built-in assistant, with no window.
    ///
    /// The same loop the app's panel runs, against the factory default set
    /// or the set --set names: the model's tool calls run as commands, and
    /// the whole turn is one undo step. Destructive and external commands
    /// are refused, and the refusal is told to the model, unless
    /// --autonomous is given. A cloud provider's key is read from the OS
    /// keychain, where the Agents page stores it, and from nowhere else.
    /// --record writes the exchange as a JSON transcript.
    Assistant(bl_cli::assistant::AssistantArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let outcome = match &cli.command {
        Command::Devices(args) => devices::run(args),
        Command::Tone(args) => tone::run(args),
        Command::EngineSmoke(args) => engine_smoke::run(args),
        Command::DeviceChanges(args) => device_changes::run(args),
        Command::Render(args) => bl_cli::render::run(args),
        Command::Export(args) => export::run(args),
        Command::Golden(args) => bl_cli::golden::run(args),
        Command::TranscribeBench(args) => bl_cli::transcribe_bench::run(args),
        Command::Mcp(args) => mcp::run(args),
        Command::Assistant(args) => bl_cli::assistant::run(args),
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

    #[test]
    fn the_assistant_takes_no_api_key() {
        // The key comes from the OS keychain alone: no option carries it.
        for flag in ["--api-key", "--key", "--token"] {
            let parsed =
                Cli::try_parse_from(["bl-cli", "assistant", "--model", "m", flag, "sk-1", "Hi."]);
            assert!(parsed.is_err(), "{flag} is accepted");
        }
        let help = Cli::command()
            .find_subcommand_mut("assistant")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(!help.to_lowercase().contains("api-key"), "{help}");
    }
}
