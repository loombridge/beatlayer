//! Golden tests: every fixture rendered offline and null-tested against its
//! reference in `tests/golden/`, bit-exact on the platform that made the
//! reference and below −96 dBFS elsewhere. The engine runs each block under
//! the allocation trap, and a render that touches the heap fails.
//!
//! Remake references with `cargo xtask golden --update`, never from here.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use bl_cli::golden::{
    self, BITS, Entry, Manifest, Render, SAMPLE_RATE, THRESHOLD_DBFS, fixture, fixtures, null_test,
    platform, read_flac,
};
use bl_cli::render::Summary;
use bl_project::{Media, MediaPath, MediaRef, Project};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

fn bl_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bl-cli"))
        .args(args)
        .output()
        .expect("bl-cli runs")
}

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-cli-golden-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn every_fixture_nulls_against_its_reference() {
    let dir = golden_dir();
    let manifest = Manifest::load(&dir).expect("the manifest reads");
    let mut failed = Vec::new();
    for fixture in fixtures() {
        let null = golden::check_fixture(fixture, &dir, &manifest);
        println!("{null}");
        if !null.passed() {
            failed.push(null.to_string());
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn every_fixture_renders_bit_identically_twice() {
    for fixture in fixtures() {
        let first = fixture.render();
        let second = fixture.render();
        assert!(first.problems().is_empty(), "{:?}", first.problems());
        assert!(
            first.samples.iter().any(|&s| s != 0.0),
            "{} is silent",
            fixture.name
        );
        assert!(
            first.same_bits(&second),
            "{} rendered differently",
            fixture.name
        );
        assert_eq!(first.hash(), second.hash());
        println!(
            "{}: {} twice, the same to the bit",
            fixture.name,
            first.hash()
        );
    }
}

// Spec: mixer R20.
#[test]
fn the_block_size_changes_nothing() {
    for fixture in fixtures() {
        let reference = fixture.render();
        for block in [100, 4096] {
            let other = fixture.render_in_blocks(block);
            assert!(other.problems().is_empty(), "{:?}", other.problems());
            assert!(
                reference.same_bits(&other),
                "{} in blocks of {block} differs from blocks of {}",
                fixture.name,
                golden::BLOCK
            );
        }
    }
}

// WP-5.37: the multicore scheduler. A pool of workers running the tasks
// renders every fixture the same to the bit, at every block size, with
// every worker's blocks under the allocation trap too.
#[test]
fn every_fixture_renders_bit_identically_with_workers() {
    for fixture in fixtures() {
        let reference = fixture.render();
        for block in [1, 100, 512, 4096] {
            let other = fixture.render_with_workers(block, 3);
            assert!(other.problems().is_empty(), "{:?}", other.problems());
            assert!(
                reference.same_bits(&other),
                "{} in blocks of {block} on three workers differs from one thread",
                fixture.name,
            );
        }
    }
}

// WP-5.37: and so does the capacity target's song, 128 tracks with an EQ and
// a compressor each.
#[test]
fn a_128_track_set_renders_bit_identically_with_workers() {
    use std::sync::Arc;

    use bl_cli::render::{Cue, Script, Settings, interleave, render_launching_with_workers};
    use bl_core::time::SampleRate;
    use bl_engine::MediaMap;

    let (editor, tracks) = bl_cli::capacity::song(128);
    assert_eq!(tracks.len(), 128);
    for block in [1, 100, 512, 4096] {
        // A fifth of a second; a block a frame makes a block per frame, so
        // fewer of those.
        let frames = if block == 1 { 1_000 } else { 9_600 };
        let settings = Settings {
            sample_rate: SampleRate::HZ_48000,
            block,
            frames,
        };
        let render = |workers| {
            let mut samples = Vec::new();
            let script = Script {
                schedule: &[],
                cues: &[Cue::PLAY],
                launches: &[],
                device_channels: 0,
            };
            let summary = render_launching_with_workers(
                editor.document(),
                Arc::new(MediaMap::new()),
                &script,
                &settings,
                workers,
                |output| interleave(output, &mut samples),
            );
            assert_eq!(summary.heap_operations, 0);
            samples.iter().map(|s| s.to_bits()).collect::<Vec<_>>()
        };
        let alone = render(0);
        assert!(alone.iter().any(|&bits| bits != 0), "the song is silent");
        assert_eq!(render(3), alone, "blocks of {block}, three workers");
        assert_eq!(render(7), alone, "blocks of {block}, seven workers");
    }
}

/// The reference platform needs every bit; elsewhere, anything below −96
/// dBFS passes, so a change has to be larger to fail there.
#[test]
fn changing_one_sample_fails_the_null_test() {
    let fixture = fixture("single-tone").expect("the fixture exists");
    let dir = golden_dir();
    let manifest = Manifest::load(&dir).expect("the manifest reads");
    let entry = &manifest.references[fixture.name];
    let reference = read_flac(&fixture.reference_path(&dir)).expect("the reference reads");
    let render = fixture.render();
    let here = entry.made_on == platform();
    assert!(null_test(fixture.name, &render, &reference, entry).passed());

    // One sample of the right channel, half a second in, changed three ways.
    let index = 24_000 * 2 + 1;
    let changed = |change: &dyn Fn(f32) -> f32| {
        let mut changed = render.clone();
        changed.samples[index] = change(changed.samples[index]);
        null_test(fixture.name, &changed, &reference, entry)
    };

    // By its last bit, below what 24 bits hold: only the hash sees it.
    let last_bit = changed(&|s| f32::from_bits(s.to_bits() ^ 1));
    println!("last bit of one sample flipped: {last_bit}");
    assert_eq!(last_bit.differing, 0);
    assert!(!last_bit.same_hash);
    assert_eq!(last_bit.passed(), !here);

    // By one 24-bit step: −138.5 dBFS.
    let one_step = changed(&|s| s + 1.0 / 8_388_608.0);
    println!("one sample up by one 24-bit step: {one_step}");
    assert_eq!(one_step.differing, 1);
    assert_eq!(one_step.worst, Some((24_000, 1)));
    assert!((one_step.residual_dbfs() + 138.5).abs() < 0.1);
    assert_eq!(one_step.passed(), !here);

    // By −60 dBFS: fails everywhere.
    let loud = changed(&|s| s + 0.001);
    println!("one sample up by 0.001 (-60 dBFS): {loud}");
    assert!(!loud.passed());
    assert!(loud.residual_dbfs() > THRESHOLD_DBFS);
    assert!(loud.to_string().contains("FAILED"));
}

#[test]
fn a_render_with_a_problem_fails_whatever_the_reference_says() {
    let fixture = fixture("single-tone").expect("the fixture exists");
    let dir = golden_dir();
    let manifest = Manifest::load(&dir).expect("the manifest reads");
    let entry = &manifest.references[fixture.name];
    let reference = read_flac(&fixture.reference_path(&dir)).expect("the reference reads");

    let mut short = fixture.render();
    short.samples.truncate(short.samples.len() - 2);
    let null = null_test(fixture.name, &short, &reference, entry);
    assert!(!null.passed());
    assert!(
        null.to_string().contains("the render has 95999 frames"),
        "{null}"
    );

    let mut unfired = fixture.render();
    unfired.scheduled = 1;
    let null = null_test(fixture.name, &unfired, &reference, entry);
    assert!(!null.passed());
    assert!(
        null.to_string().contains("0 of 1 scheduled commands fired"),
        "{null}"
    );
}

#[test]
fn the_references_are_small_and_each_belongs_to_a_fixture() {
    let dir = golden_dir();
    let manifest = Manifest::load(&dir).expect("the manifest reads");
    for fixture in fixtures() {
        let path = fixture.reference_path(&dir);
        let size = fs::metadata(&path).expect("the reference exists").len();
        println!("{}: {size} bytes", path.display());
        assert!(size < 256 * 1024, "{} is {size} bytes", path.display());
        let entry = &manifest.references[fixture.name];
        assert_eq!(entry.sample_rate, SAMPLE_RATE.hz());
        assert_eq!(entry.bits, BITS);
        assert_eq!(entry.frames as u64, fixture.scene().frames);
    }
    assert_eq!(golden::stale(&dir), Ok(Vec::new()));
}

/// Every reference opens through Beatlayer's own importer and gives the
/// samples the test reader does: a reference is a file Beatlayer wrote, and
/// anything Beatlayer writes it must read.
#[test]
fn every_reference_reads_through_beatlayers_own_decoder() {
    let dir = golden_dir();
    for fixture in fixtures() {
        let path = fixture.reference_path(&dir);
        let reference = golden::read_flac(&path).expect("the test reader opens it");
        let decoded = bl_media::decode_file(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert_eq!(
            decoded.channel_count(),
            reference.channels,
            "{}",
            path.display()
        );
        let scale = f64::from(1u32 << (reference.bits - 1));
        for (index, sample) in reference.samples.iter().enumerate() {
            let channel = decoded
                .channel(index % reference.channels)
                .expect("the channel exists");
            let value = channel[index / reference.channels];
            assert_eq!(
                f64::from(value),
                f64::from(*sample) / scale,
                "{} sample {index}",
                path.display()
            );
        }
    }
}

/// Saves a fixture's song as a set in `dir`, with the audio files its clips
/// play in `dir/Samples/Imported/` and listed in the set's media table.
fn save_set(dir: &Path, name: &str) -> PathBuf {
    let scene = fixture(name).expect("the fixture exists").scene();
    let mut media = Media::new();
    let samples = dir.join("Samples").join("Imported");
    fs::create_dir_all(&samples).expect("the samples folder");
    for file in &scene.files {
        fs::write(samples.join(&file.name), &file.bytes).expect("the audio file saves");
        media.insert(MediaRef {
            hash: file.hash,
            path: MediaPath::new(format!("Samples/Imported/{}", file.name)).unwrap(),
            size: file.bytes.len() as u64,
            reversed_from: None,
        });
    }
    let project = Project::from_editor(&scene.editor, media);
    let path = dir.join(format!("{name}.blset"));
    fs::write(
        &path,
        bl_project::encode(&project, 1).expect("the set encodes"),
    )
    .expect("the set saves");
    path
}

/// `bl-cli render` on a saved set writes the render the harness makes, so
/// it nulls against the same reference. Only fixtures that play from the
/// start with nothing scheduled: a set holds no scheduled commands or
/// transport moves. (It does hold its Arrangement loop, which
/// `loop-wraps` plays, and its mixer: sends, Pre/Post switches, solo, and
/// Solo in Place, which `pan-and-sends`, `solo-in-place`, and
/// `solo-a-return` play. And its clips, whose audio files `bl-cli render`
/// finds through the set's media table, beside the set, as in a project
/// folder.)
#[test]
fn bl_cli_render_writes_what_the_harness_renders() {
    let temp = TempDir::new("render");
    let dir = golden_dir();
    let manifest = Manifest::load(&dir).expect("the manifest reads");
    let mut rendered = 0;
    for fixture in fixtures() {
        let scene = fixture.scene();
        if !scene.plays_as_saved() {
            continue;
        }
        let set = save_set(&temp.0, fixture.name);
        let wav = temp.0.join(format!("{}.wav", fixture.name));
        let seconds = (scene.frames as f64 / SAMPLE_RATE.as_f64()).to_string();
        let output = bl_cli(&[
            "render",
            set.to_str().unwrap(),
            wav.to_str().unwrap(),
            "--seconds",
            &seconds,
        ]);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{text}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            text.contains("heap operations: 0 in the engine's blocks (allocation trap on)"),
            "a debug build installs the trap: {text}"
        );

        let mut reader = hound::WavReader::open(&wav).expect("the WAV reads");
        let spec = reader.spec();
        assert_eq!(spec.sample_format, hound::SampleFormat::Float);
        assert_eq!(spec.bits_per_sample, 32);
        let samples: Vec<f32> = reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .expect("the samples read");
        let render = Render {
            sample_rate: spec.sample_rate,
            channels: usize::from(spec.channels),
            samples,
            scheduled: 0,
            summary: Summary::default(),
        };
        let reference = read_flac(&fixture.reference_path(&dir)).expect("the reference reads");
        let entry: &Entry = &manifest.references[fixture.name];
        let null = null_test(fixture.name, &render, &reference, entry);
        println!("bl-cli render, {null}");
        assert!(null.passed(), "{null}");
        assert!(render.same_bits(&fixture.render()), "{}", fixture.name);
        rendered += 1;
    }
    assert_eq!(rendered, 69);
    assert!(!fs::read_dir(&temp.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial")
    }));
}

#[test]
fn bl_cli_render_writes_24_bit_samples_as_the_references_hold_them() {
    let temp = TempDir::new("render-i24");
    let set = save_set(&temp.0, "single-tone");
    let wav = temp.0.join("single-tone.wav");
    let output = bl_cli(&[
        "render",
        set.to_str().unwrap(),
        wav.to_str().unwrap(),
        "--seconds",
        "2",
        "--format",
        "i24",
        "--block",
        "333",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut reader = hound::WavReader::open(&wav).expect("the WAV reads");
    assert_eq!(reader.spec().bits_per_sample, 24);
    let samples: Vec<i32> = reader
        .samples::<i32>()
        .collect::<Result<_, _>>()
        .expect("the samples read");
    let fixture = fixture("single-tone").expect("the fixture exists");
    let reference = read_flac(&fixture.reference_path(&golden_dir())).expect("the reference reads");
    assert_eq!(samples, reference.samples);
}

/// A set with clips renders to the end of its last clip unless told
/// otherwise; with its audio files gone, it still renders, its clips silent,
/// and says which files are missing (the audio clips spec, R14).
#[test]
fn bl_cli_render_plays_clips_to_their_end_and_names_missing_files() {
    let temp = TempDir::new("render-clips");
    let set = save_set(&temp.0, "clip-fades");
    let set = set.to_str().unwrap();
    let wav = temp.0.join("clips.wav");
    let wav = wav.to_str().unwrap();
    let output = bl_cli(&["render", set, wav]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    // The last clip ends on 2.3.1, three seconds in at 120 BPM.
    assert!(
        text.contains("3.000 s: 144000 frames at 48000 Hz"),
        "{text}"
    );
    assert!(text.contains("peak:      -20.6 dBFS"), "{text}");

    fs::remove_dir_all(temp.0.join("Samples")).unwrap();
    let output = bl_cli(&["render", set, wav]);
    let text = String::from_utf8_lossy(&output.stdout);
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{text}\n{errors}");
    assert!(text.contains("peak:      -inf dBFS"), "{text}");
    let hash = fixture("clip-fades").unwrap().scene().files[0].hash;
    assert!(
        errors.contains(&format!(
            "warning: 1 audio file is missing or changed, so its clips play silence: {hash}"
        )),
        "{errors}"
    );
}

#[test]
fn bl_cli_render_takes_a_length_in_bars_and_refuses_what_it_cannot_do() {
    let temp = TempDir::new("render-args");
    let set = save_set(&temp.0, "single-tone");
    let set = set.to_str().unwrap();
    let wav = temp.0.join("out.wav");
    let wav = wav.to_str().unwrap();

    // Two bars of 4/4 at 120 BPM: four seconds.
    let output = bl_cli(&["render", set, wav, "--bars", "2"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains("4.000 s: 192000 frames at 48000 Hz, 2 channels"),
        "{text}"
    );
    assert!(text.contains("peak:      -24.0 dBFS"), "{text}");

    for (args, message) in [
        (
            vec!["render", set, "out.aiff"],
            "OUTPUT must be a .wav file",
        ),
        (vec!["render", set, wav, "--rate", "0"], "--rate must be"),
        (vec!["render", set, wav, "--block", "0"], "--block must be"),
        (
            vec!["render", set, wav, "--seconds", "-1"],
            "--seconds must be",
        ),
        (
            vec!["render", "missing.blset", wav],
            "can't open missing.blset",
        ),
    ] {
        let output = bl_cli(&args);
        let errors = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}");
        assert!(errors.contains(message), "{args:?}: {errors}");
    }
}

#[test]
fn bl_cli_golden_checks_the_fixtures_it_is_given() {
    let dir = golden_dir();
    let output = bl_cli(&["golden", "--dir", dir.to_str().unwrap(), "single-tone"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("1 fixtures: 1 passed, 0 failed"), "{text}");

    let output = bl_cli(&["golden", "--dir", dir.to_str().unwrap(), "no-such-fixture"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("there's no fixture called"));
}

/// `--update` writes references into an empty folder, then leaves them
/// alone while the renders don't change, and they null against themselves.
#[test]
fn bl_cli_golden_update_makes_references_and_then_leaves_them_alone() {
    let temp = TempDir::new("update");
    let dir = temp.0.to_str().unwrap();
    let output = bl_cli(&["golden", "--dir", dir, "--update", "single-tone"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("single-tone: reference written"), "{text}");

    let output = bl_cli(&["golden", "--dir", dir, "--update", "single-tone"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("single-tone: unchanged"), "{text}");
    assert!(text.contains("bit-exact"), "{text}");

    // The new reference holds the same samples as the committed one.
    let fixture = fixture("single-tone").expect("the fixture exists");
    assert_eq!(
        read_flac(&fixture.reference_path(&temp.0)),
        read_flac(&fixture.reference_path(&golden_dir()))
    );
}

// Spec: cv-tools R12, R13, R17. The CV fixture's reference holds the device
// channels after Main's two, with the voltages the phrase writes on their
// frames, the same at any block size.
#[test]
fn the_cv_fixture_writes_its_voltages_on_the_device_channels() {
    let fixture = fixture("cv-instrument-and-triggers").expect("the fixture exists");
    let render = fixture.render();
    assert_eq!(render.channels, 8, "Main's two and six device channels");
    let at = |frame: usize, channel: usize| render.samples[frame * 8 + channel];
    // The first key, 60, is 0 V on Pitch (channel 3), the gate (4) rises on
    // its frame at 5 V, and the first trigger (6) pulses for 10 ms.
    assert_eq!(at(0, 2), 0.0);
    assert_eq!(at(0, 3), 0.5);
    assert_eq!(at(0, 5), 0.5);
    assert_eq!(at(479, 5), 0.5);
    assert_eq!(at(480, 5), 0.0, "the pulse ends after 480 frames");
    // Key 67 plays Pitch at 7/12 V by its frame, once the glide has run.
    let pitch = at(14 * 6_000 + 3_000 + 200, 2);
    assert!((pitch - 0.7 / 12.0).abs() < 1e-5, "{pitch}");
    for block in [1, 64, 4096] {
        assert!(
            fixture.render_in_blocks(block).same_bits(&render),
            "blocks of {block}"
        );
    }
}

// Spec: cv-tools R22. The clock fixture's device channel pulses on every
// sixteenth note of the song, on the frames the tempo map gives them, through
// the ramps up and down, and the same at any block size.
#[test]
fn the_cv_clock_fixture_pulses_on_the_tempo_maps_frames() {
    use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
    let fixture = fixture("cv-clock-out-tempo-ramp").expect("the fixture exists");
    let render = fixture.render();
    assert_eq!(render.channels, 3, "Main's two and the clock's channel");
    let clock: Vec<f32> = render.samples.chunks(3).map(|frame| frame[2]).collect();
    let rises: Vec<usize> = (0..clock.len())
        .filter(|&at| clock[at] > 0.0 && (at == 0 || clock[at - 1] == 0.0))
        .collect();
    let beats = |n: i64| Ticks::QUARTER * n;
    let bpm = |n: f64| Bpm::new(n).unwrap();
    let tempo = TempoMap::from_breakpoints([
        (beats(0), bpm(90.0)),
        (beats(4), bpm(150.0)),
        (beats(8), bpm(90.0)),
        (beats(12), bpm(90.0)),
    ])
    .unwrap();
    for (k, &rise) in rises.iter().enumerate() {
        let want = tempo
            .ticks_to_frames(Ticks(k as i64 * Ticks::QUARTER.0 / 4), SampleRate::HZ_48000)
            .get();
        assert_eq!(rise as i64, want, "pulse {k}");
    }
    assert!(rises.len() > 30, "{} pulses", rises.len());
    assert!(clock.iter().all(|&s| s == 0.0 || s == 0.5), "5 V");
    for block in [1, 64, 4096] {
        assert!(
            fixture.render_in_blocks(block).same_bits(&render),
            "blocks of {block}"
        );
    }
}

// Spec: cv-tools R28. The Utility fixture's track in Main and its CV output
// are one signal, clamped to ±10 V and centred on the volt of offset, the
// same at any block size.
#[test]
fn the_cv_utility_fixture_puts_one_signal_on_the_track_and_its_cv_output() {
    let fixture = fixture("cv-utility-track").expect("the fixture exists");
    let render = fixture.render();
    assert_eq!(render.channels, 3);
    let cv: Vec<f32> = render.samples.chunks(3).map(|frame| frame[2]).collect();
    let main: Vec<f32> = render.samples.chunks(3).map(|frame| frame[0]).collect();
    assert!(cv.iter().all(|s| s.abs() <= 1.0));
    let mean = cv.iter().sum::<f32>() / cv.len() as f32;
    assert!((mean - 0.1).abs() < 0.01, "a volt of offset: {mean}");
    // The track reaches Main through its own mixer: the same shape, scaled
    // by one constant.
    let ratios: Vec<f32> = cv
        .iter()
        .zip(&main)
        .filter(|(c, _)| c.abs() > 0.05)
        .map(|(c, m)| m / c)
        .collect();
    assert!(ratios.len() > 1000);
    assert!(
        ratios.iter().all(|r| (r - ratios[0]).abs() < 1e-4),
        "Main is the CV output through the mixer"
    );
    for block in [1, 64, 4096] {
        assert!(
            fixture.render_in_blocks(block).same_bits(&render),
            "blocks of {block}"
        );
    }
}

// Spec: cv-tools R32, R33. The modulators' fixture writes each device's
// voltage on its own channel, worked out on every frame: CV LFO between its
// Min and Max, CV Shaper on the four levels of its staircase, CV Envelope
// Follower between 0 V and 5 V, the same at any block size.
#[test]
fn the_cv_modulators_fixture_writes_each_signal_on_its_channel() {
    let fixture = fixture("cv-modulators").expect("the fixture exists");
    let render = fixture.render();
    assert_eq!(render.channels, 5, "Main's two and the three devices'");
    let channel = |at: usize| -> Vec<f32> { render.samples.chunks(5).map(|frame| frame[at]).collect() };
    // The triangle with Jitter, from −3 V to +4 V.
    let wave = channel(2);
    let (low, high) = wave
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), &s| (l.min(s), h.max(s)));
    assert!(low >= -0.3 - 1e-5 && high <= 0.4 + 1e-5, "{low} to {high}");
    assert!(low < -0.2 && high > 0.3, "it sweeps the span: {low} to {high}");
    // The staircase: four levels of 5 V, a third of the way apart.
    let stairs = channel(3);
    for want in [0.0f32, 0.5 / 3.0, 1.0 / 3.0, 0.5] {
        assert!(
            stairs.iter().filter(|&&s| (s - want).abs() < 1e-5).count() > 1_000,
            "the staircase holds {want}"
        );
    }
    // The follower: 0 to 5 V.
    let follower = channel(4);
    assert!(follower.iter().all(|&s| (0.0..=0.5).contains(&s)));
    assert!(follower.iter().any(|&s| s > 0.2), "it follows the slow tone");
    for block in [1, 64, 4096] {
        assert!(
            fixture.render_in_blocks(block).same_bits(&render),
            "blocks of {block}"
        );
    }
}
