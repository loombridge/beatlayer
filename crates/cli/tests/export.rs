//! Exporting audio (WP-1.12) against the golden renders: an export runs the
//! same engine code a golden test renders through, so an export of a golden
//! fixture nulls against its reference; and the rest of the export spec's
//! rules, from the range to cancelling.
//!
//! Each export's engine blocks run under the allocation trap, and an export
//! that touched the heap inside a block fails.
//!
//! With `BL_TEST_DEPTH=full` (pushes to main, manual CI runs, and the nightly
//! run), every golden fixture exports as FLAC, WAV, and AIFF; with `quick`,
//! the default for local runs and pull requests, each fixture exports as one
//! of the three in turn, so every fixture and every format is still checked.

bl_rt::install_trap!();

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bl_cli::golden::{
    self, Entry, Fixture, Manifest, Render, SAMPLE_RATE, THRESHOLD_DBFS, fixture, fixtures,
    platform, read_flac,
};
use bl_cli::render::{self, Cue, Settings, interleave, render_with, to_i24};
use bl_core::time::{SampleRate, Ticks};
use bl_engine::{MediaLibrary, MediaMap};
use bl_media::{BitDepth, Dither, FileType};
use bl_model::{AudioData, Clip, ClipData, Command as Edit, ContentHash, Editor, TrackKind};
use bl_project::{Media, MediaPath, MediaRef, Project};
use bl_shell::export::{self, ExportError, Job, Length, Report};

/// Whether the tests run at full depth: `BL_TEST_DEPTH=full`, as on pushes
/// to main and the nightly run. Unset or `quick`, the default for local runs
/// and pull requests, they run fewer cases or rounds of the same checks.
fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Err(_) | Ok("" | "quick") => false,
        Ok(other) => panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full"),
    }
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-cli-export-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }

    /// The names in the folder.
    fn names(&self) -> BTreeSet<String> {
        fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An export job at the golden references' rate, from 1.1.1, `frames`
/// long, without dither.
fn job(path: PathBuf, file_type: FileType, bit_depth: BitDepth, frames: u64) -> Job {
    Job {
        path,
        file_type,
        bit_depth,
        dither: Dither::None,
        sample_rate: SAMPLE_RATE.hz(),
        engine_rate: SAMPLE_RATE.hz(),
        normalize: false,
        start: Ticks::ZERO,
        length: Length::Frames(frames),
        replace: false,
        block: export::BLOCK,
    }
}

fn media_of(fixture: &Fixture) -> Arc<dyn MediaLibrary> {
    Arc::new(fixture.scene().media)
}

/// Runs an export that must succeed, and checks the engine stayed off the
/// heap.
fn run(document: &bl_model::Document, media: Arc<dyn MediaLibrary>, job: &Job) -> Report {
    let report = export::export(
        document,
        media,
        None,
        job,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_or_else(|error| panic!("{}: {error}", job.path.display()));
    assert_eq!(report.heap_operations, 0, "the real-time rules");
    report
}

/// Interleaved samples of a file, as 32-bit floats, through Beatlayer's own
/// decoder (the one importing uses).
fn decode(path: &Path) -> (u32, usize, Vec<f32>) {
    let decoder = bl_media::Decoder::open(path).unwrap();
    let rate = decoder.info().sample_rate;
    let data = decoder.decode_to_end().unwrap();
    let channels = data.channel_count();
    let mut samples = Vec::with_capacity(data.frames() * channels);
    for frame in 0..data.frames() {
        samples.extend((0..channels).map(|c| data.channel(c).unwrap()[frame]));
    }
    (rate, channels, samples)
}

/// A file's samples as 24-bit integers, as the references hold them.
fn ints_24(path: &Path) -> Vec<i32> {
    decode(path).2.iter().map(|&s| to_i24(s)).collect()
}

/// How many 24-bit samples differ from the reference, and the largest
/// difference in dBFS.
fn difference(ours: &[i32], reference: &[i32]) -> (usize, f64) {
    assert_eq!(ours.len(), reference.len(), "the same length");
    let mut differing = 0;
    let mut largest = 0i64;
    for (&a, &b) in ours.iter().zip(reference) {
        let d = (i64::from(a) - i64::from(b)).abs();
        if d != 0 {
            differing += 1;
            largest = largest.max(d);
        }
    }
    (differing, 20.0 * (largest as f64 / 8_388_608.0).log10())
}

/// Whether a fixture plays as a saved set plays: from the start, nothing
/// scheduled, and (since an export plays straight through) its loop off.
fn exports_as_rendered(fixture: &Fixture) -> bool {
    let scene = fixture.scene();
    scene.plays_as_saved() && !scene.document().settings().arrangement_loop.enabled
}

/// Spec: export R2, R4, R12.
#[test]
fn every_fixture_exports_as_its_golden_reference_to_the_bit() {
    let temp = TempDir::new("golden");
    let dir = golden_dir();
    let manifest = Manifest::load(&dir).expect("the manifest reads");
    let mut exported = 0;
    for fixture in fixtures() {
        if !exports_as_rendered(fixture) {
            continue;
        }
        let scene = fixture.scene();
        let entry: &Entry = &manifest.references[fixture.name];
        let reference = read_flac(&fixture.reference_path(&dir)).expect("the reference reads");
        let exact = entry.made_on == platform();

        let formats = [FileType::Flac, FileType::Wav, FileType::Aiff];
        let formats = if full_depth() {
            &formats[..]
        } else {
            std::slice::from_ref(&formats[exported % formats.len()])
        };
        for &file_type in formats {
            let path = temp
                .0
                .join(format!("{}.{}", fixture.name, file_type.extension()));
            let report = run(
                scene.document(),
                media_of(fixture),
                &job(path.clone(), file_type, BitDepth::Int24, scene.frames),
            );
            assert_eq!(report.frames, scene.frames);
            assert_eq!(report.channels, 2);
            assert!(report.missing_media.is_empty());
            let ours = if file_type == FileType::Flac {
                // An independent decoder, as the golden harness reads its
                // references.
                read_flac(&path).unwrap().samples
            } else {
                ints_24(&path)
            };
            let (differing, residual) = difference(&ours, &reference.samples);
            println!(
                "{} as 24-bit {file_type:?}: {differing} samples differ ({residual:.1} dBFS)",
                fixture.name
            );
            if exact {
                assert_eq!(differing, 0, "{} as {file_type:?}", fixture.name);
            } else {
                assert!(residual < THRESHOLD_DBFS, "{}", fixture.name);
            }
        }

        // As 32-bit float, the file holds the render the reference was made
        // from: its hash is the manifest's.
        let path = temp.0.join(format!("{}-float.wav", fixture.name));
        run(
            scene.document(),
            media_of(fixture),
            &job(path.clone(), FileType::Wav, BitDepth::Float32, scene.frames),
        );
        let (rate, channels, samples) = decode(&path);
        let render = Render {
            sample_rate: rate,
            channels,
            samples,
            scheduled: 0,
            summary: render::Summary::default(),
        };
        if exact {
            assert_eq!(render.hash(), entry.render_hash, "{}", fixture.name);
        }
        assert!(render.same_bits(&fixture.render()), "{}", fixture.name);
        exported += 1;
    }
    // Every fixture a saved set can hold, but the loop's.
    assert_eq!(exported, 11);
    assert!(
        !temp.names().iter().any(|name| name.starts_with('.')),
        "no hidden files left: {:?}",
        temp.names()
    );
}

/// Renders `document` from its start with `media`, as the golden harness
/// does, at `rate`.
fn render_at(
    document: &bl_model::Document,
    media: Arc<dyn MediaLibrary>,
    cues: &[Cue],
    rate: SampleRate,
    frames: u64,
) -> Vec<f32> {
    let mut samples = Vec::new();
    let summary = render_with(
        document,
        media,
        &[],
        cues,
        &Settings {
            sample_rate: rate,
            block: golden::BLOCK,
            frames,
        },
        |output| interleave(output, &mut samples),
    );
    assert_eq!(summary.heap_operations, 0);
    samples
}

fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|s| s.to_bits()).collect()
}

/// Spec: export R3.
#[test]
fn a_range_plays_straight_through_from_its_start_without_the_loop() {
    let temp = TempDir::new("range");
    // A range in the middle of a song with clips is the song's render from
    // the range's first frame: every clip plays from where the range finds
    // it, on the exact sample.
    // (The test tone the other fixtures play runs on its own clock, so it
    // isn't positional.)
    for name in [
        "clip-mid-bar",
        "overlapping-clips",
        "clip-tempo-change",
        "clip-fades",
    ] {
        let fixture = fixture(name).unwrap();
        let scene = fixture.scene();
        let whole = render_at(
            scene.document(),
            media_of(fixture),
            &[Cue::PLAY],
            SAMPLE_RATE,
            scene.frames,
        );
        let start = Ticks::QUARTER * 3;
        let first = scene
            .document()
            .settings()
            .tempo
            .ticks_to_frames(start, SAMPLE_RATE)
            .get() as usize;
        let frames = scene.frames - first as u64;
        let path = temp.0.join(format!("{name}.wav"));
        let mut job = job(path.clone(), FileType::Wav, BitDepth::Float32, frames);
        job.start = start;
        run(scene.document(), media_of(fixture), &job);
        let (_, _, samples) = decode(&path);
        assert_eq!(
            bits(&samples),
            bits(&whole[first * 2..]),
            "{name} from frame {first}"
        );
    }

    // With the Arrangement loop on, the export plays on past the loop's
    // end, as the song renders with the loop off.
    let fixture = fixture("loop-wraps").unwrap();
    let scene = fixture.scene();
    assert!(scene.document().settings().arrangement_loop.enabled);
    let mut unlooped = Editor::from_document(scene.document().clone(), scene.editor.ids());
    unlooped
        .apply(Edit::SetLoopEnabled { enabled: false })
        .unwrap();
    let expected = render_at(
        unlooped.document(),
        media_of(fixture),
        &[Cue::PLAY],
        SAMPLE_RATE,
        scene.frames,
    );
    let looped = render_at(
        scene.document(),
        media_of(fixture),
        &[Cue::PLAY],
        SAMPLE_RATE,
        scene.frames,
    );
    assert_ne!(
        bits(&expected),
        bits(&looped),
        "the loop changes the render"
    );
    let path = temp.0.join("loop-wraps.wav");
    run(
        scene.document(),
        media_of(fixture),
        &job(path.clone(), FileType::Wav, BitDepth::Float32, scene.frames),
    );
    assert_eq!(bits(&decode(&path).2), bits(&expected));

    // A range in ticks ends on the frame its end falls on.
    let path = temp.0.join("two-beats.wav");
    let mut two_beats = job(path, FileType::Wav, BitDepth::Float32, 0);
    two_beats.start = Ticks::QUARTER;
    two_beats.length = Length::To(Ticks::QUARTER * 3);
    let report = run(scene.document(), media_of(fixture), &two_beats);
    let tempo = &scene.document().settings().tempo;
    let frames = tempo.ticks_to_frames(Ticks::QUARTER * 3, SAMPLE_RATE).get()
        - tempo.ticks_to_frames(Ticks::QUARTER, SAMPLE_RATE).get();
    assert_eq!(report.frames, frames as u64);
}

/// Spec: export R6.
#[test]
fn a_lower_rate_is_converted_from_the_engines_and_a_higher_one_rendered_at() {
    let temp = TempDir::new("rates");
    let fixture = fixture("clip-tempo-change").unwrap();
    let scene = fixture.scene();
    let golden = fixture.render();

    // 44.1 kHz from a 48 kHz engine: the 48 kHz render, converted.
    let path = temp.0.join("down.wav");
    let mut down = job(path.clone(), FileType::Wav, BitDepth::Float32, scene.frames);
    down.sample_rate = 44_100;
    let report = run(scene.document(), media_of(fixture), &down);
    assert_eq!(report.render_rate, 48_000);
    assert_eq!(
        report.frames,
        bl_media::converted_len(scene.frames as usize, 48_000, 44_100) as u64
    );
    let left: Vec<f32> = golden.samples.iter().step_by(2).copied().collect();
    let right: Vec<f32> = golden.samples.iter().skip(1).step_by(2).copied().collect();
    let converted = bl_media::convert_sample_rate(
        &bl_media::SampleData::new(48_000, vec![left, right]).unwrap(),
        44_100,
    )
    .unwrap();
    let (rate, _, samples) = decode(&path);
    assert_eq!(rate, 44_100);
    for (n, frame) in samples.chunks(2).enumerate() {
        assert_eq!(
            frame[0].to_bits(),
            converted.channel(0).unwrap()[n].to_bits()
        );
        assert_eq!(
            frame[1].to_bits(),
            converted.channel(1).unwrap()[n].to_bits()
        );
    }

    // 96 kHz from a 48 kHz engine: rendered at 96 kHz, every clip's file
    // converted to it.
    let path = temp.0.join("up.wav");
    let mut up = job(
        path.clone(),
        FileType::Wav,
        BitDepth::Float32,
        scene.frames * 2,
    );
    up.sample_rate = 96_000;
    let report = run(scene.document(), media_of(fixture), &up);
    assert_eq!(report.render_rate, 96_000);
    let expected = render_at(
        scene.document(),
        media_of(fixture),
        &[Cue::PLAY],
        SampleRate::HZ_96000,
        scene.frames * 2,
    );
    let (rate, _, samples) = decode(&path);
    assert_eq!(rate, 96_000);
    assert_eq!(bits(&samples), bits(&expected));
    // Same rate as the engine's: no conversion.
    assert_eq!(
        run(
            scene.document(),
            media_of(fixture),
            &job(
                temp.0.join("same.wav"),
                FileType::Wav,
                BitDepth::Int16,
                4800
            )
        )
        .render_rate,
        48_000
    );
}

/// Spec: export R8.
#[test]
fn normalize_brings_the_peak_to_full_scale() {
    let temp = TempDir::new("normalize");
    let fixture = fixture("looped-clip").unwrap();
    let scene = fixture.scene();
    let peak = fixture.render().summary.peak;
    assert!(peak < 0.9, "the fixture is quieter than full scale");

    let path = temp.0.join("float.aif");
    let mut float = job(
        path.clone(),
        FileType::Aiff,
        BitDepth::Float32,
        scene.frames,
    );
    float.normalize = true;
    let report = run(scene.document(), media_of(fixture), &float);
    assert!(
        (report.gain - 1.0 / f64::from(peak)).abs() < 1e-12,
        "gain {} for a peak of {peak}",
        report.gain
    );
    let (_, _, samples) = decode(&path);
    let top = samples.iter().fold(0f32, |top, s| top.max(s.abs()));
    assert!((top - 1.0).abs() <= f32::EPSILON, "peak {top}");
    // The shape is the render's, scaled.
    let golden = fixture.render();
    for (ours, theirs) in samples.iter().zip(&golden.samples) {
        assert!((f64::from(*ours) - f64::from(*theirs) * report.gain).abs() < 1e-6);
    }

    for (bit_depth, top_step) in [(BitDepth::Int16, 32_767), (BitDepth::Int24, 8_388_607)] {
        let path = temp.0.join(format!("{}.wav", bit_depth.bits()));
        let mut ints = job(path.clone(), FileType::Wav, bit_depth, scene.frames);
        ints.normalize = true;
        ints.dither = Dither::Triangular;
        run(scene.document(), media_of(fixture), &ints);
        let mut reader = hound::WavReader::open(&path).unwrap();
        let top = reader
            .samples::<i32>()
            .map(|s| s.unwrap().abs())
            .max()
            .unwrap();
        assert!(
            top == top_step || top == top_step + 1,
            "{} bits: the largest step, {top}",
            bit_depth.bits()
        );
    }
    assert!(!temp.names().iter().any(|name| name.starts_with('.')));
}

/// The M1 demo's shape: eight stems of `seconds`, at levels a mix would
/// use, in memory.
fn eight_stems(seconds: f64) -> (Editor, MediaMap) {
    let mut editor = Editor::new();
    let mut media = MediaMap::new();
    let frames = (seconds * 48_000.0) as usize;
    for (index, hz) in [55.0, 180.0, 6000.0, 82.4, 261.6, 196.0, 523.3, 440.0]
        .into_iter()
        .enumerate()
    {
        let mut hash = [0u8; 32];
        hash[0] = index as u8 + 1;
        let tone: Vec<f32> = (0..frames)
            .map(|n| (0.1 * (std::f64::consts::TAU * hz * n as f64 / 48_000.0).sin()) as f32)
            .collect();
        media.insert(
            ContentHash(hash),
            Arc::new(bl_media::SampleData::new(48_000, vec![tone.clone(), tone]).unwrap()),
        );
        let track = editor.new_id();
        editor
            .apply(Edit::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: format!("Stem {index}"),
                parent: None,
                index,
            })
            .unwrap();
        // At 120 BPM, a quarter note is half a second.
        let length = Ticks((seconds * 2.0 * Ticks::QUARTER.0 as f64) as i64);
        let clip = Clip::new(
            editor.new_id(),
            Ticks::ZERO,
            length,
            ClipData::Audio(AudioData::new(ContentHash(hash))),
        );
        editor.apply(Edit::CreateClip { track, clip }).unwrap();
    }
    (editor, media)
}

/// Spec: export R9.
#[test]
fn an_export_runs_faster_than_real_time() {
    let temp = TempDir::new("speed");
    let (editor, media) = eight_stems(30.0);
    let path = temp.0.join("stems.wav");
    let mut stems = job(path, FileType::Wav, BitDepth::Int24, 0);
    stems.length = Length::To(export::song_end(editor.document()).unwrap());
    stems.dither = Dither::Triangular;
    let report = run(editor.document(), Arc::new(media), &stems);
    let speed = report.speed(48_000);
    println!(
        "eight 30-second stems exported in {:.3} s: {speed:.1} times real time ({} build)",
        report.took.as_secs_f64(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    assert_eq!(report.frames, 30 * 48_000);
    assert!(speed > 1.0, "{speed:.2} times real time");
}

/// Spec: export R10.
#[test]
fn cancelling_leaves_nothing_behind() {
    let fixture = fixture("clip-fades").unwrap();
    let scene = fixture.scene();
    for normalize in [false, true] {
        for (file_type, bit_depth) in [
            (FileType::Wav, BitDepth::Int24),
            (FileType::Flac, BitDepth::Int16),
            (FileType::Aiff, BitDepth::Float32),
        ] {
            let temp = TempDir::new("cancel");
            // A file already there, which the cancelled export would have
            // replaced: it stays as it was.
            let kept = temp.0.join(format!("kept.{}", file_type.extension()));
            fs::write(&kept, b"the file that was there").unwrap();
            for path in [
                temp.0.join(format!("new.{}", file_type.extension())),
                kept.clone(),
            ] {
                let mut job = job(path.clone(), file_type, bit_depth, scene.frames);
                job.normalize = normalize;
                job.replace = true;
                let cancel = AtomicBool::new(false);
                let mut reached = 0.0;
                let result = export::export(
                    scene.document(),
                    media_of(fixture),
                    None,
                    &job,
                    &cancel,
                    &mut |done| {
                        reached = done;
                        // Halfway through what's written.
                        if done >= 0.4 {
                            cancel.store(true, Ordering::Relaxed);
                        }
                    },
                );
                assert_eq!(result, Err(ExportError::Cancelled));
                assert!(reached > 0.0 && reached < 1.0, "{reached}");
            }
            assert_eq!(
                temp.names(),
                BTreeSet::from([format!("kept.{}", file_type.extension())]),
                "{file_type:?}, normalize {normalize}"
            );
            assert_eq!(fs::read(&kept).unwrap(), b"the file that was there");
        }
    }
}

/// Spec: export R10.
#[test]
fn a_failure_leaves_nothing_behind() {
    let fixture = fixture("single-tone").unwrap();
    let scene = fixture.scene();
    let temp = TempDir::new("failure");
    // Something in the way of the partial file: the export renders, then
    // can't write, and leaves the folder as it found it.
    let path = temp.0.join("mix.wav");
    fs::create_dir(export::hidden_beside(&path, "partial")).unwrap();
    let before = temp.names();
    for normalize in [false, true] {
        let mut job = job(path.clone(), FileType::Wav, BitDepth::Int24, scene.frames);
        job.normalize = normalize;
        let result = export::export(
            scene.document(),
            media_of(fixture),
            None,
            &job,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(matches!(result, Err(ExportError::Failed(_))), "{result:?}");
        assert_eq!(temp.names(), before);
    }
    // A folder that isn't there is refused before anything renders.
    let result = export::export(
        scene.document(),
        media_of(fixture),
        None,
        &job(
            temp.0.join("nowhere/mix.wav"),
            FileType::Wav,
            BitDepth::Int24,
            10,
        ),
        &AtomicBool::new(false),
        &mut |_| panic!("nothing renders"),
    );
    assert!(matches!(result, Err(ExportError::Invalid(_))), "{result:?}");
}

/// Spec: export R7, R14.
#[test]
fn the_same_export_is_the_same_bytes_at_any_block_size() {
    let fixture = fixture("clip-mid-bar").unwrap();
    let scene = fixture.scene();
    let temp = TempDir::new("blocks");
    for (file_type, dither) in [
        (FileType::Flac, Dither::Triangular),
        (FileType::Wav, Dither::Rectangular),
    ] {
        let mut files = Vec::new();
        for (n, block) in [512, 512, 61, 4096].into_iter().enumerate() {
            let path = temp.0.join(format!("{n}.{}", file_type.extension()));
            let mut job = job(path.clone(), file_type, BitDepth::Int16, scene.frames);
            job.dither = dither;
            job.block = block;
            run(scene.document(), media_of(fixture), &job);
            files.push(fs::read(&path).unwrap());
        }
        assert!(
            files.windows(2).all(|pair| pair[0] == pair[1]),
            "{file_type:?}"
        );
        // Dither changes the file, by at most a step either way.
        let path = temp.0.join(format!("plain.{}", file_type.extension()));
        run(
            scene.document(),
            media_of(fixture),
            &job(path.clone(), file_type, BitDepth::Int16, scene.frames),
        );
        assert_ne!(fs::read(&path).unwrap(), files[0]);
        let plain = decode(&path).2;
        let dithered = decode(&temp.0.join(format!("0.{}", file_type.extension()))).2;
        let step = 1.0 / 32_768.0;
        let reach = if dither == Dither::Triangular {
            2.0
        } else {
            1.0
        };
        for (a, b) in plain.iter().zip(&dithered) {
            assert!((a - b).abs() <= reach * step + 1e-9, "{a} against {b}");
        }
    }
}

fn bl_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bl-cli"))
        .args(args)
        .output()
        .expect("bl-cli runs")
}

/// Saves a fixture's song as a set in `dir`, with the audio files its clips
/// play in `dir/Samples/Imported/`, listed in the set's media table.
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
        });
    }
    let project = Project::from_editor(&scene.editor, media);
    let path = dir.join(format!("{name}.blset"));
    fs::write(&path, bl_project::encode(&project, 1).unwrap()).unwrap();
    path
}

/// Spec: export R1, R11, R15.
#[test]
fn bl_cli_export_writes_what_the_command_does() {
    let temp = TempDir::new("cli");
    let set = save_set(&temp.0, "clip-fades");
    let fixture = fixture("clip-fades").unwrap();
    let scene = fixture.scene();
    let flac = temp.0.join("clip-fades.flac");
    let output = bl_cli(&[
        "export",
        set.to_str().unwrap(),
        flac.to_str().unwrap(),
        "--bit-depth",
        "24",
        "--dither",
        "none",
    ]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("format:    FLAC 24-bit"), "{text}");
    assert!(text.contains("times real time"), "{text}");
    // The whole set: 1.1.1 to the end of its last clip, played through the
    // golden harness's engine and rounded as its references are.
    let end = export::song_end(scene.document()).unwrap();
    let frames = scene
        .document()
        .settings()
        .tempo
        .ticks_to_frames(end, SAMPLE_RATE)
        .get() as u64;
    let expected: Vec<i32> = render_at(
        scene.document(),
        media_of(fixture),
        &[Cue::PLAY],
        SAMPLE_RATE,
        frames,
    )
    .iter()
    .map(|&s| to_i24(s))
    .collect();
    assert_eq!(read_flac(&flac).unwrap().samples, expected);

    // It won't replace the file unless told to.
    let again = bl_cli(&[
        "export",
        set.to_str().unwrap(),
        flac.to_str().unwrap(),
        "--bit-depth",
        "16",
    ]);
    assert!(!again.status.success());
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("exists already"),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    let replaced = bl_cli(&[
        "export",
        set.to_str().unwrap(),
        flac.to_str().unwrap(),
        "--bit-depth",
        "16",
        "--start",
        "1.2",
        "--bars",
        "1",
        "--replace",
        "--rate",
        "44100",
    ]);
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    let reference = read_flac(&flac).unwrap();
    assert_eq!((reference.bits, reference.sample_rate), (16, 44_100));
    // One bar of 4/4 from 1.2.1 at 120 BPM: two seconds, at 44.1 kHz.
    assert_eq!(reference.samples.len() / 2, 88_200);
    assert!(String::from_utf8_lossy(&replaced.stdout).contains("1.2.1 to 2.2.1"));
    // 32-bit FLAC is refused, and leaves nothing.
    let refused = bl_cli(&[
        "export",
        set.to_str().unwrap(),
        temp.0.join("float.flac").to_str().unwrap(),
        "--bit-depth",
        "32",
    ]);
    assert!(!refused.status.success());
    assert!(!temp.0.join("float.flac").exists());
    assert!(!temp.names().iter().any(|name| name.starts_with('.')));
}
