//! Times peak builds, for the WP-1.2 acceptance figure (an hour of audio in
//! under 5 seconds on an M1).
//!
//! ```text
//! cargo run --release -p bl-media --example peaks -- [OPTIONS] FILE...
//! cargo run --release -p bl-media --example peaks -- --write-wav OUT.wav [--seconds S]
//! ```
//!
//! Options:
//!
//! - `--threads N`: decoding threads; 1 decodes in order. All cores by default.
//! - `--segment FRAMES`: the shortest parallel segment.
//! - `--runs N`: builds per file; the best time is reported. 3 by default.
//! - `--verify`: also builds each file in order on one thread and checks the
//!   peaks are identical.
//! - `--cache`: also times the whole background path: importing the file
//!   into a media store (which hashes it), building through a `PeakCache`
//!   that saves the peak file, and loading that file in a new cache.
//!
//! `--write-wav` writes a test file: 48 kHz stereo 24-bit, filtered noise
//! and a 110 Hz sine under a slow swell, an hour long unless `--seconds` says
//! otherwise. Other formats come from it through an encoder, for example
//! `ffmpeg -i hour.wav -c:a flac hour.flac`, `-c:a libmp3lame -b:a 320k
//! hour.mp3`, `-c:a aac -b:a 256k hour.m4a`, or `-c:a vorbis -strict
//! experimental -q:a 6 hour.ogg`.
//!
//! Builds read the file through the operating system's cache, so after the
//! first run the time is decoding and peak-finding, not the disk.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use bl_media::peaks::{BuildOptions, BuildProgress};
use bl_media::{MediaStore, PeakCache, PeakCacheOptions, Peaks, probe};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    options: BuildOptions,
    runs: usize,
    verify: bool,
    cache: bool,
    files: Vec<PathBuf>,
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut parsed = Args {
        options: BuildOptions::default(),
        runs: 3,
        verify: false,
        cache: false,
        files: Vec::new(),
    };
    let mut write_wav = None;
    let mut seconds = 3_600.0;
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        let number = |text: String| text.parse::<u64>().map_err(|e| e.to_string());
        match arg.as_str() {
            "--threads" => {
                parsed.options.threads = NonZeroUsize::new(number(value("--threads")?)? as usize);
            }
            "--segment" => parsed.options.segment_frames = number(value("--segment")?)?,
            "--runs" => parsed.runs = number(value("--runs")?)?.max(1) as usize,
            "--verify" => parsed.verify = true,
            "--cache" => parsed.cache = true,
            "--write-wav" => write_wav = Some(PathBuf::from(value("--write-wav")?)),
            "--seconds" => {
                seconds = value("--seconds")?
                    .parse()
                    .map_err(|e: std::num::ParseFloatError| e.to_string())?;
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            file => parsed.files.push(PathBuf::from(file)),
        }
    }
    if let Some(path) = write_wav {
        return write_test_wav(&path, seconds).map_err(|e| e.to_string());
    }
    if parsed.files.is_empty() {
        return Err("name a file to build peaks for".into());
    }

    let threads = parsed.options.threads.map_or_else(
        || {
            let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
            format!("all {cores} cores")
        },
        |n| format!("{n} thread(s)"),
    );
    println!(
        "{threads}, segments of at least {} frames, best of {}",
        parsed.options.segment_frames, parsed.runs
    );
    for file in &parsed.files {
        time_file(file, &parsed).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    Ok(())
}

fn time_file(file: &Path, args: &Args) -> Result<(), String> {
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let info = probe(file).map_err(|e| e.to_string())?;
    let mut best = f64::INFINITY;
    let mut peaks = None;
    for _ in 0..args.runs {
        let start = Instant::now();
        let built = Peaks::build_file(file, &args.options, &BuildProgress::new())
            .map_err(|e| e.to_string())?;
        best = best.min(start.elapsed().as_secs_f64());
        peaks = Some(built);
    }
    let peaks = peaks.ok_or("no runs")?;
    let duration = peaks.frames() as f64 / f64::from(peaks.sample_rate());
    println!(
        "{name}: {:?} {:?}, {} Hz, {} ch, {:.1} s; {:.3} s ({:.0}x real time), {} levels, {} bytes of peaks",
        info.container,
        info.codec,
        peaks.sample_rate(),
        peaks.channels(),
        duration,
        best,
        duration / best,
        peaks.level_count(),
        peaks.byte_size(),
    );

    if args.verify {
        let start = Instant::now();
        let in_order = Peaks::build_file(
            file,
            &BuildOptions::single_threaded(),
            &BuildProgress::new(),
        )
        .map_err(|e| e.to_string())?;
        let same = in_order == peaks;
        println!(
            "  verify: in order on one thread, {:.3} s; identical to the parallel build: {}",
            start.elapsed().as_secs_f64(),
            if same { "yes" } else { "NO" }
        );
        if !same {
            return Err("the parallel build differs from the in-order build".into());
        }
    }

    if args.cache {
        time_cache(file, &peaks)?;
    }
    Ok(())
}

/// Imports the file, builds its peaks through a cache that saves them, then
/// loads them in a fresh cache, timing each step.
fn time_cache(file: &Path, expected: &Peaks) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("bl-peaks-example-{}", std::process::id()));
    let store = Arc::new(MediaStore::new());
    let start = Instant::now();
    let hash = store.import(file).map_err(|e| e.to_string())?;
    let imported = start.elapsed().as_secs_f64();

    let options = PeakCacheOptions::in_dir(&dir);
    let cache = PeakCache::new(Arc::clone(&store), options.clone()).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let built = cache.wait(hash).map_err(|e| e.to_string())?;
    let building = start.elapsed().as_secs_f64();
    drop(cache);

    let cache = PeakCache::new(Arc::clone(&store), options).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let loaded = cache.wait(hash).map_err(|e| e.to_string())?;
    let loading = start.elapsed().as_secs_f64();
    let file_size = cache
        .file_path(hash)
        .and_then(|p| std::fs::metadata(p).ok())
        .map_or(0, |m| m.len());
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);

    println!(
        "  cache: import and hash {imported:.3} s; background build and save {building:.3} s; \
         load from disk in a new cache {loading:.3} s ({file_size} bytes); same peaks: {}",
        if *built == *expected && *loaded == *expected {
            "yes"
        } else {
            "NO"
        }
    );
    Ok(())
}

/// Writes 48 kHz stereo 24-bit test audio: low-passed noise plus a 110 Hz
/// sine, under an eight-second swell.
fn write_test_wav(path: &Path, seconds: f64) -> std::io::Result<()> {
    const RATE: u32 = 48_000;
    let frames = (seconds * f64::from(RATE)) as u64;
    let data_len = frames * 6;
    let mut out = BufWriter::with_capacity(1 << 20, File::create(path)?);
    out.write_all(b"RIFF")?;
    out.write_all(&(u32::try_from(36 + data_len).unwrap_or(u32::MAX)).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?;
    out.write_all(&2u16.to_le_bytes())?;
    out.write_all(&RATE.to_le_bytes())?;
    out.write_all(&(RATE * 6).to_le_bytes())?;
    out.write_all(&6u16.to_le_bytes())?;
    out.write_all(&24u16.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&(u32::try_from(data_len).unwrap_or(u32::MAX)).to_le_bytes())?;

    let mut rng: u64 = 0x5EED;
    let mut filtered = [0.0f64; 2];
    let step = std::f64::consts::TAU * 110.0 / f64::from(RATE);
    for n in 0..frames {
        let t = n as f64 / f64::from(RATE);
        let swell = 0.4 + 0.4 * (std::f64::consts::TAU * t / 8.0).sin();
        let sine = 0.3 * (step * n as f64).sin();
        for f in &mut filtered {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let white = (rng >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
            *f += 0.05 * (white - *f);
            let v = ((*f * 4.0 + sine) * swell).clamp(-1.0, 1.0);
            let s = (v * 8_388_607.0) as i32;
            out.write_all(&s.to_le_bytes()[..3])?;
        }
    }
    out.flush()
}
