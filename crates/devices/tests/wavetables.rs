//! Wavetables for Wavetable (WP-8.8; the wavetable spec, R1 to R7, R9, R10):
//! the factory generator, its categories and choosers, user tables from
//! files, and the worker that builds them off the audio thread.

use std::path::{Path, PathBuf};

use bl_devices::wavetables::{
    CATEGORIES, Source, factory, factory_tables, folder_files, hash, loader, step, tables_in, user,
};
use lspk_wavetable::{FRAME_SIZE, Interpolation, MAX_FRAMES, increment};
use realfft::RealFftPlanner;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-wavetables-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    repository().join("tests/fixtures/wavetables").join(name)
}

/// Spec: wavetable R1: every table is in one of the seven categories, has a
/// name of its own, and draws 1 to 256 frames.
#[test]
fn every_factory_table_is_in_a_category_and_draws_up_to_256_frames() {
    let all = factory_tables();
    assert!(!all.is_empty());
    for table in &all {
        assert!(CATEGORIES.contains(&table.category), "{}", table.name);
        assert!((1..=MAX_FRAMES).contains(&table.frames), "{}", table.name);
        assert_eq!(table.source().frame_count(), table.frames);
        assert_eq!(table.source().interpolation(), table.interpolation);
    }
    for category in CATEGORIES {
        assert!(!tables_in(category).is_empty(), "{category} is empty");
    }
    let mut names: Vec<_> = all.iter().map(|t| (t.category, t.name)).collect();
    names.dedup();
    assert_eq!(names.len(), all.len(), "a name is listed twice");
}

/// Every table's hash on every platform: the golden list of R2. CI runs
/// this on macOS, Windows and Linux, so a table that comes out differently
/// anywhere fails there.
const HASHES: [(&str, &str, u64); 26] = [
    ("Shape Morphs", "Saw to Square", 11608712095048825021),
    ("Shape Morphs", "Sine Folds", 14514606166583674044),
    ("Shape Morphs", "Sine to Saw", 16177784718492493135),
    ("Shape Morphs", "Square to Triangle", 563968543669637516),
    ("Additive", "Falling Bells", 11864589619428350086),
    ("Additive", "Harmonic Ladder", 2982913063844283159),
    ("Additive", "Octave Stack", 2394074861533486276),
    ("Additive", "Odd Rise", 10508322783469047265),
    ("Pulse and Sync", "Pulse Width Sweep", 4539946891562153908),
    ("Pulse and Sync", "Sync Saw Sweep", 17801580448034074585),
    ("Pulse and Sync", "Sync Sine Sweep", 1783319249019011761),
    ("Pulse and Sync", "Twin Pulses", 42895614029773452),
    ("FM", "Feedback Bloom", 2226540163186759427),
    ("FM", "Index Sweep", 13083609646793228672),
    ("FM", "Ratio Steps", 6309806211248076840),
    ("Voices", "Back Vowels", 13043706237645202304),
    ("Voices", "Formant Glide", 3123570359971868425),
    ("Voices", "Front Vowels", 15096477686540367343),
    ("Voices", "Vowel Walk", 5747656191458757965),
    ("Bit and Step", "Bit Crush Sweep", 16829840268700936773),
    ("Bit and Step", "Hold Squeeze", 265690352598888286),
    ("Bit and Step", "Stair Rise", 17388704099849182627),
    ("Grain and Noise", "Grit Drift", 6635583296054947239),
    ("Grain and Noise", "Noise Smear", 3439205135174657267),
    ("Grain and Noise", "Rough Sine", 12077705223164925999),
    ("Grain and Noise", "Scatter Pulses", 14957158758916802257),
];

/// Spec: wavetable R1, R2: the generator makes every table bit for bit the
/// same on every run, and on every platform.
#[test]
fn the_generator_makes_the_same_tables_every_run_and_everywhere() {
    let hashes = |_: ()| -> Vec<(&str, &str, u64)> {
        factory_tables()
            .iter()
            .map(|t| (t.category, t.name, hash(&t.source())))
            .collect()
    };
    let first = hashes(());
    assert_eq!(first, hashes(()), "a second run differs from the first");
    assert_eq!(first, HASHES, "a table differs from the golden hashes");
}

/// Spec: wavetable R1: no factory table is a file in the repository.
#[test]
fn no_table_is_stored_as_audio() {
    fn audio_files(folder: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                audio_files(&path, found);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| ["wav", "aif", "aiff", "flac", "ogg", "mp3"].contains(&e))
            {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    audio_files(&repository().join("assets"), &mut found);
    assert_eq!(found, Vec::<PathBuf>::new(), "assets/ holds audio");
    found.clear();
    audio_files(&repository().join("tests/fixtures"), &mut found);
    let slug = |name: &str| name.to_lowercase().replace(' ', "-");
    for table in factory_tables() {
        let slug = slug(table.name);
        assert!(
            found
                .iter()
                .all(|f| !f.file_stem().unwrap().to_string_lossy().contains(&slug)),
            "a fixture holds {}",
            table.name
        );
    }
}

/// Spec: wavetable R4: every frame of every table, read at the top key,
/// folds back nothing above −80 dB.
#[test]
fn every_frame_is_band_limited_at_the_top_key() {
    const LENGTH: usize = 8192;
    let rate = 48_000.0;
    // Key 127: only its fundamental sits below Nyquist, so whatever else
    // there is has folded back.
    let hz = 12_543.85;
    let step = increment(hz, rate);
    // Blackman-Harris, whose sidelobes sit 92 dB down.
    let window: Vec<f32> = (0..LENGTH)
        .map(|n| {
            let x = std::f32::consts::TAU * n as f32 / LENGTH as f32;
            0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos()
        })
        .collect();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(LENGTH);
    let power = |signal: &mut Vec<f32>| {
        let mut bins = fft.make_output_vec();
        fft.process(signal, &mut bins).unwrap();
        let fundamental = (hz / rate * LENGTH as f64).round() as usize;
        let (mut near, mut far) = (0.0f32, 0.0f32);
        for (bin, c) in bins.iter().enumerate() {
            if bin.abs_diff(fundamental) <= 6 {
                near += c.norm_sqr();
            } else {
                far += c.norm_sqr();
            }
        }
        (near, far)
    };
    // A full-scale sine's power: the floor for a frame with no fundamental
    // left at this key is 120 dB below it.
    let reference = {
        let mut sine: Vec<f32> = (0..LENGTH)
            .map(|n| 0.9 * (std::f32::consts::TAU * hz as f32 / rate as f32 * n as f32).sin())
            .zip(&window)
            .map(|(x, w)| x * w)
            .collect();
        power(&mut sine).0
    };
    for table in factory_tables() {
        let shared = table.shared();
        let level = shared.level_for(step);
        for frame in 0..shared.frame_count() {
            let wave = shared.wave(level, frame);
            let mut phase = 0u32;
            let mut signal: Vec<f32> = window
                .iter()
                .map(|w| {
                    let x = wave.read(phase) * w;
                    phase = phase.wrapping_add(step);
                    x
                })
                .collect();
            let (played, folded) = power(&mut signal);
            // As the kit measures aliasing: against what the key plays.
            let played = played.max(reference * 1e-4);
            assert!(
                folded < played * 1e-8,
                "{} frame {frame} folds back {:.1} dB",
                table.name,
                10.0 * (folded / played).log10()
            );
        }
    }
}

/// Spec: wavetable R3, R9: a factory table is built once into the kit's
/// cache and shared, and plays with its own interpolation.
#[test]
fn a_factory_table_is_shared_and_keeps_its_interpolation() {
    let vowels = factory("Voices", "Vowel Walk").unwrap();
    let (a, b) = (vowels.shared(), vowels.shared());
    assert!(a.ptr_eq(&b));
    assert_eq!(a.interpolation(), Interpolation::Spectral);
    let steps = factory("FM", "Ratio Steps").unwrap().shared();
    assert_eq!(steps.interpolation(), Interpolation::Crossfade);
}

/// Spec: wavetable R5: the choosers list the categories, the tables of a
/// category in name order, and the arrows step across categories.
#[test]
fn the_arrows_step_through_the_tables_across_categories() {
    let all = factory_tables();
    for category in CATEGORIES {
        let names: Vec<_> = tables_in(category).iter().map(|t| t.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "{category} isn't in name order");
    }
    let order: Vec<_> = CATEGORIES
        .iter()
        .flat_map(|c| tables_in(c))
        .map(|t| (t.category, t.name))
        .collect();
    assert_eq!(
        order,
        all.iter().map(|t| (t.category, t.name)).collect::<Vec<_>>()
    );

    let last_shape = tables_in("Shape Morphs").pop().unwrap();
    let next = step(last_shape.category, last_shape.name, 1).unwrap();
    assert_eq!(
        (next.category, next.name),
        (CATEGORIES[1], tables_in(CATEGORIES[1])[0].name)
    );
    let back = step(next.category, next.name, -1).unwrap();
    assert_eq!(back.name, last_shape.name);
    // The ends hold.
    let (first, last) = (all[0], *all.last().unwrap());
    assert_eq!(
        step(first.category, first.name, -1).unwrap().name,
        first.name
    );
    assert_eq!(step(last.category, last.name, 1).unwrap().name, last.name);
    assert!(step("Additive", "No Such Table", 1).is_none());
}

fn correlation(frame: &[f32], harmonic: usize) -> f32 {
    let (mut dot, mut a, mut b) = (0.0, 0.0, 0.0);
    for (n, x) in frame.iter().enumerate() {
        let y = (std::f32::consts::TAU * harmonic as f32 * n as f32 / frame.len() as f32).sin();
        dot += x * y;
        a += x * x;
        b += y * y;
    }
    dot / (a * b).sqrt()
}

/// Spec: wavetable R6: a WAV with a `clm ` chunk is cut by its frame size,
/// each frame resampled to 2,048 samples.
#[test]
fn a_wav_with_a_clm_chunk_is_cut_by_its_frame_size() {
    let table = user::read(&fixture("clm-4-frames.wav"), false).unwrap();
    assert_eq!(table.frame_count(), 4);
    for (i, frame) in table.frames().iter().enumerate() {
        assert_eq!(frame.len(), FRAME_SIZE);
        assert!(correlation(&frame[..], i + 1) > 0.99, "frame {i}");
    }
    // R9: a user table crossfades.
    assert_eq!(table.interpolation(), Interpolation::Crossfade);
}

/// Spec: wavetable R6: any other file is cut into frames of 2,048 samples
/// from its start, a last part shorter than a frame dropped.
#[test]
fn a_plain_wav_or_aiff_is_cut_into_frames_of_2048_samples() {
    let wav = user::read(&fixture("plain-3-frames.wav"), true).unwrap();
    assert_eq!(wav.frame_count(), 3);
    let aiff = user::read(&fixture("plain-2-frames.aif"), false).unwrap();
    assert_eq!(
        aiff.frame_count(),
        2,
        "the part of a third frame is dropped"
    );
    for (i, frame) in aiff.frames().iter().enumerate() {
        assert!(correlation(&frame[..], i + 1) > 0.99, "frame {i}");
    }
}

/// Spec: wavetable R6: a file shorter than a frame fills one.
#[test]
fn a_short_file_is_resampled_into_one_frame() {
    let table = user::read(&fixture("short-300.wav"), false).unwrap();
    assert_eq!(table.frame_count(), 1);
    assert!(correlation(&table.frames()[0][..], 1) > 0.99);
}

/// Spec: wavetable R6: a file makes at most 256 frames.
#[test]
fn a_long_file_makes_256_frames() {
    let scratch = Scratch::new("long");
    let samples = vec![0.25f32; 300 * FRAME_SIZE];
    let bytes = bl_media::wav_bytes(
        &samples,
        1,
        48_000,
        bl_media::BitDepth::Float32,
        bl_media::Dither::None,
    )
    .unwrap();
    let path = scratch.0.join("long.wav");
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(user::read(&path, true).unwrap().frame_count(), MAX_FRAMES);
}

/// Spec: wavetable R6: with Raw off each frame loses its DC offset and its
/// ends meet; with Raw on the frames are used as they are.
#[test]
fn raw_skips_the_clean_up() {
    let mean = |frame: &[f32]| frame.iter().sum::<f32>() / frame.len() as f32;
    let raw = user::read(&fixture("plain-3-frames.wav"), true).unwrap();
    let clean = user::read(&fixture("plain-3-frames.wav"), false).unwrap();
    for (raw, clean) in raw.frames().iter().zip(clean.frames()) {
        assert!((mean(&raw[..]) - 0.2).abs() < 1e-3, "Raw keeps the DC");
        assert!(mean(&clean[..]).abs() < 1e-3, "the clean-up removes it");
        // The sample after the last is the first.
        assert!((clean[FRAME_SIZE - 1] - clean[0]).abs() < 1e-3);
        assert!((raw[FRAME_SIZE - 1] - raw[0]).abs() > 1e-3);
    }
}

/// Spec: wavetable R6, R8: a file that can't be read, or has no audio, makes
/// no table.
#[test]
fn an_unreadable_file_or_one_with_no_audio_makes_no_table() {
    assert!(user::read(&fixture("empty.wav"), false).is_err());
    assert!(user::read(&fixture("missing.wav"), false).is_err());
    let scratch = Scratch::new("garbage");
    let path = scratch.0.join("garbage.wav");
    std::fs::write(&path, b"not audio").unwrap();
    assert!(user::read(&path, false).is_err());
}

/// Spec: wavetable R7: after a user table loads, the choosers browse the
/// file's folder: its WAV and AIFF files in name order.
#[test]
fn the_choosers_browse_the_files_folder() {
    let files = folder_files(&fixture("short-300.wav"));
    let names: Vec<_> = files
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "clm-4-frames.wav",
            "empty.wav",
            "plain-2-frames.aif",
            "plain-3-frames.wav",
            "short-300.wav"
        ]
    );
}

/// Spec: wavetable R3, R10: a table is built on a worker thread, and the
/// audio thread collects it without allocating or locking; a table that
/// can't be built leaves the previous one in place.
#[test]
fn a_table_is_built_off_the_audio_thread() {
    let (loader, mut ready) = loader();
    loader.request(
        1,
        Source::Factory {
            category: "Additive".into(),
            name: "Odd Rise".into(),
        },
    );
    loader.request(
        2,
        Source::File {
            path: fixture("missing.wav"),
            raw: false,
        },
    );
    let mut built = None;
    for _ in 0..50_000_000u32 {
        // The audio thread's side, under the allocation trap.
        bl_rt::rt_test!({ built = ready.poll() });
        if built.is_some() {
            break;
        }
        std::thread::yield_now();
    }
    let built = built.expect("the worker finished");
    assert_eq!(built.oscillator, 1);
    assert_eq!(built.table.frame_count(), 32);
    let expected = factory("Additive", "Odd Rise").unwrap().shared();
    assert!(built.table.ptr_eq(&expected), "shared through the cache");
    assert!(ready.poll().is_none(), "the missing file built nothing");
}
