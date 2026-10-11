//! `bl-cli export`: a set's Main output exported to a WAV, AIFF, or FLAC
//! file, faster than real time, with no audio device (WP-1.12).
//!
//! It opens the set in a headless shell and runs `export.audio` there, the
//! command an agent runs, so the command line, the agent API, and the app
//! export alike. The behaviour is in the export spec.

use std::path::PathBuf;
use std::process::ExitCode;

use bl_core::time::Ticks;
use bl_shell::export_commands::{self as export_ids, Dither, ExportResult, FileType};
use bl_shell::{Shell, commands as ids};
use serde_json::{Value, json};

use crate::Outcome;

#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// The set to export: a `.blset` file, or a project folder holding one
    /// set.
    pub project: PathBuf,

    /// The file to write. Its extension gives the file type: `.wav`,
    /// `.aif` or `.aiff`, or `.flac`.
    pub output: PathBuf,

    /// Bits per sample: 16 or 24 (integers), or 32 (floating point; WAV
    /// and AIFF only).
    #[arg(long, value_parser = ["16", "24", "32"])]
    pub bit_depth: String,

    /// The file's sample rate in hertz. Below the engine's (48 kHz), the
    /// set renders at the engine's rate and is converted down.
    #[arg(long, default_value_t = 48_000)]
    pub rate: u32,

    /// The file type, if the extension doesn't say or to check it.
    #[arg(long = "type", value_enum)]
    pub file_type: Option<Type>,

    /// The dither below 32 bits.
    #[arg(long, value_enum, default_value_t = DitherArg::Triangular)]
    pub dither: DitherArg,

    /// Bring the highest peak to full scale (0 dBFS).
    #[arg(long)]
    pub normalize: bool,

    /// Where the range starts: bars, beats, and sixteenths, such as 5.1.1,
    /// or ticks. Defaults to 1.1.1.
    #[arg(long, allow_negative_numbers = true)]
    pub start: Option<String>,

    /// How long the range is, in bars of the set's time signatures, from
    /// the start. Defaults to the end of the set's last clip.
    #[arg(long, conflicts_with = "length")]
    pub bars: Option<u32>,

    /// How long the range is, in ticks (705,600 to the quarter note).
    #[arg(long)]
    pub length: Option<i64>,

    /// Replace OUTPUT if it exists.
    #[arg(long)]
    pub replace: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Type {
    Wav,
    Aiff,
    Flac,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum DitherArg {
    None,
    Rectangular,
    Triangular,
}

/// The arguments of `export.audio` these command-line arguments ask for.
fn command_args(args: &ExportArgs, shell: &Shell) -> Result<Value, String> {
    let start = args.start.as_deref().map(|text| {
        text.parse::<i64>()
            .map_or_else(|_| json!(text), |ticks| json!(ticks))
    });
    let length = match (args.length, args.bars) {
        (Some(ticks), _) => Some(Ticks(ticks)),
        (None, Some(0)) => return Err("--bars must be at least 1".into()),
        (None, Some(bars)) => {
            let signatures = &shell.editor().document().settings().time_signatures;
            let from = match &start {
                Some(position) => {
                    serde_json::from_value::<bl_shell::editing_commands::Position>(position.clone())
                        .map_err(|error| error.to_string())?
                        .resolve(signatures)?
                }
                None => Ticks::ZERO,
            };
            // So many bars on from the start, by the bars' own lengths.
            let (bar, _) = signatures.bar_at(from);
            Some(signatures.bar_start(bar + i64::from(bars)) - signatures.bar_start(bar))
        }
        (None, None) => None,
    };
    let mut command = json!({
        "path": args.output.to_string_lossy(),
        "bit_depth": args.bit_depth.parse::<u8>().map_err(|error| error.to_string())?,
        "sample_rate": args.rate,
        "dither": match args.dither {
            DitherArg::None => Dither::None,
            DitherArg::Rectangular => Dither::Rectangular,
            DitherArg::Triangular => Dither::Triangular,
        },
        "normalize": args.normalize,
        "replace": args.replace,
    });
    if let Some(file_type) = args.file_type {
        command["file_type"] = json!(match file_type {
            Type::Wav => FileType::Wav,
            Type::Aiff => FileType::Aiff,
            Type::Flac => FileType::Flac,
        });
    }
    if let Some(start) = start {
        command["start"] = start;
    }
    if let Some(length) = length {
        command["length"] = json!(length);
    }
    Ok(command)
}

/// Runs `bl-cli export`.
///
/// # Errors
///
/// A message if the set doesn't open or the export is refused or fails.
pub fn run(args: &ExportArgs) -> Outcome {
    let mut shell = Shell::headless();
    shell
        .run(
            ids::SET_OPEN,
            json!({ "path": args.project.to_string_lossy() }),
        )
        .map_err(|error| format!("can't open {}: {error}", args.project.display()))?;
    let command = command_args(args, &shell)?;
    let result = shell
        .run(export_ids::AUDIO, command)
        .map_err(|error| error.to_string())?;
    let result: ExportResult = serde_json::from_value(result).map_err(|error| error.to_string())?;
    let signatures = &shell.editor().document().settings().time_signatures;
    println!("set:       {}", args.project.display());
    println!(
        "range:     {} to {} ({:?})",
        signatures.to_bbs(result.start),
        signatures.to_bbs(result.end),
        result.range
    );
    println!(
        "length:    {:.3} s: {} frames at {} Hz, {} channels",
        result.seconds, result.frames, result.sample_rate, result.channels
    );
    println!(
        "format:    {} {}-bit{}{}",
        result.file_type.name(),
        result.bit_depth,
        if result.bit_depth == 32 { " float" } else { "" },
        match result.dither {
            Dither::None => String::new(),
            other => format!(", {other:?} dither").to_lowercase(),
        }
    );
    if result.render_rate != result.sample_rate {
        println!(
            "rendered:  at {} Hz, converted to {} Hz",
            result.render_rate, result.sample_rate
        );
    }
    println!(
        "took:      {:.3} s, {:.0} times real time",
        result.took_seconds, result.speed
    );
    match result.peak_dbfs {
        Some(peak) => println!("peak:      {peak:.2} dBFS"),
        None => println!("peak:      silence"),
    }
    if args.normalize {
        println!("normalize: {:+.2} dB", result.gain_db);
    }
    println!("wrote:     {}", result.path);
    if !result.missing_media.is_empty() {
        eprintln!(
            "warning: {} audio file{} missing or changed, so {} clips are silent: {}",
            result.missing_media.len(),
            if result.missing_media.len() == 1 {
                " is"
            } else {
                "s are"
            },
            if result.missing_media.len() == 1 {
                "its"
            } else {
                "their"
            },
            result.missing_media.join(", ")
        );
    }
    Ok(ExitCode::SUCCESS)
}
