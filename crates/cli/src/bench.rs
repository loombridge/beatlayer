//! `bl-cli bench`: timed runs of the on-device features, as JSON.
//!
//! `bench stems` separates a generated mix in both modes on whatever
//! provider loads, and reports how long the model took to compile and each
//! mode to run (stem separation spec R36, R41). The first compile is
//! reported apart, and left out of the times.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use bl_analysis::stems::generated;
use bl_analysis::stems::onnx::{Demucs, Options};
use bl_analysis::stems::{Quality, separate};
use serde_json::json;

use crate::Outcome;

#[derive(Debug, clap::Args)]
pub struct BenchArgs {
    #[command(subcommand)]
    pub what: Bench,
}

#[derive(Debug, clap::Subcommand)]
pub enum Bench {
    /// Separate a generated 3-minute mix into stems in High Speed and High
    /// Quality, and print JSON with the platform, the provider, the compile
    /// time and each mode's time.
    ///
    /// The mix is made from a seed, so it is the same on every computer.
    /// The model's first compile on a computer is slow and reported as
    /// `compile_seconds`; it is left out of the modes' times, and a later
    /// run finds it cached (`compile_seconds` is then null).
    Stems(StemsArgs),
}

#[derive(Debug, clap::Args)]
pub struct StemsArgs {
    /// ONNX Runtime's shared library.
    #[arg(long)]
    pub runtime: PathBuf,

    /// The model: the split model for the GPU, or the whole graph.
    #[arg(long)]
    pub model: PathBuf,

    /// Where the GPU's compiled model is cached. Delete it to measure the
    /// first compile again.
    #[arg(long, default_value_os_t = std::env::temp_dir().join("beatlayer-bench-stems"))]
    pub cache: PathBuf,

    /// The mix's length in seconds.
    #[arg(long, default_value_t = 180.0)]
    pub seconds: f64,

    /// The seed the mix is generated from.
    #[arg(long, default_value_t = 1)]
    pub seed: u32,

    /// Run on the CPU even where a GPU provider is available.
    #[arg(long)]
    pub cpu: bool,
}

pub fn run(args: &BenchArgs) -> Outcome {
    match &args.what {
        Bench::Stems(args) => stems(args),
    }
}

/// The speed target of R35, in seconds, for a 3-minute mix in High Speed.
const TARGET_SECONDS: f64 = 10.0;

fn stems(args: &StemsArgs) -> Outcome {
    let options = Options {
        threads: std::thread::available_parallelism().map_or(4, std::num::NonZero::get),
        accelerate: !args.cpu,
        cache: Some(args.cache.clone()),
    };
    let mut network = Demucs::load(&args.runtime, &args.model, &options, &mut |_| {})?;
    let report = network.report().clone();
    let generated = generated::mix(args.seconds, args.seed);
    let [left, right] = &generated.mix;
    let mut time = |quality| {
        let started = Instant::now();
        separate(left, right, &mut network, quality, &mut |_| true)
            .map(|_| started.elapsed().as_secs_f64())
            .map_err(|e| e.to_string())
    };
    let high_speed = time(Quality::HighSpeed)?;
    let high_quality = time(Quality::HighQuality)?;
    let round = |x: f64| (x * 100.0).round() / 100.0;
    let out = json!({
        "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        "provider": report.provider.name(),
        "fallback": report.fallback,
        "compile_seconds": report.compiled.map(|c| round(c.as_secs_f64())),
        "load_seconds": round(report.load.as_secs_f64()),
        "mix_seconds": args.seconds,
        "seed": args.seed,
        "high_speed_seconds": round(high_speed),
        "high_quality_seconds": round(high_quality),
        "target_seconds": TARGET_SECONDS,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
    );
    Ok(ExitCode::SUCCESS)
}
