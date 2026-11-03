//! Create Video through the shell (WP-9.27; the export spec, R38 and R46 to
//! R52, the video spec, R24 and R25): the movie `export.audio` writes beside
//! the audio, frame by frame from a Motion JPEG export, its sound against the
//! audio file's, its range with a video clip selected, and what is left
//! behind when it fails.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_core::time::Ticks;
use bl_model::{ClipId, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::export_commands::{ExportResult, ExportStatus};
use bl_video::{Frame, FrameSource, Movie, MovieAudio, Picture, Rational};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();
const FRAME: i128 = 1001;
const TIME_SCALE: i128 = 30000;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-export-video-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A copy of a generated test movie.
    fn movie(&self, name: &str) -> PathBuf {
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/video")
            .join(name);
        let to = self.path(name);
        std::fs::copy(from, &to).unwrap();
        to
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn audio_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(|track| track.id())
        .unwrap()
}

/// A shell with the 24-frame movie dropped on an audio track at `at`.
fn shell_with_a_movie(scratch: &Scratch, at: i64) -> (Shell, ClipId) {
    let movie = scratch.movie("mjpeg-24f.mov");
    let mut shell = Shell::headless();
    let track = audio_track(&shell);
    let added = shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [text(&movie)], "track": track, "at": at }),
        )
        .unwrap();
    let clip = serde_json::from_value(added["clips"][0]["clip"].clone()).unwrap();
    (shell, clip)
}

fn export(shell: &mut Shell, args: Value) -> Result<ExportResult, bl_shell::ShellError> {
    shell
        .run("export.audio", args)
        .map(|result| serde_json::from_value(result).unwrap())
}

/// The index a test movie's frame shows in its top row of cells, or `None`
/// for a black picture.
fn index_of(frame: &Frame) -> Option<u32> {
    let luma = |x, y| {
        let [r, g, b, _] = frame.pixel(x, y).unwrap();
        (u32::from(r) + u32::from(g) + u32::from(b)) / 3
    };
    if frame
        .rgba()
        .chunks(4)
        .all(|p| p[0] < 24 && p[1] < 24 && p[2] < 24)
    {
        return None;
    }
    Some(
        (0..8)
            .map(|bit| u32::from(luma(8 * bit + 4, 4) > 128) << bit)
            .sum(),
    )
}

/// The picture at each frame of a movie, as `index_of` reads it.
fn indices(movie: &Arc<Movie>, frames: usize) -> Vec<Option<u32>> {
    let rate = movie.video().unwrap();
    assert_eq!(rate.frame_start(1), Some(Rational::new(FRAME, TIME_SCALE)));
    let mut source = FrameSource::new(Arc::clone(movie)).unwrap();
    (0..frames as i128)
        .map(|n| {
            // The middle of the frame, so rounding can't decide it.
            let t = Rational::new(2 * n * FRAME + FRAME, 2 * TIME_SCALE);
            match source.picture_at(t) {
                Picture::Frame(frame) => index_of(&frame),
                Picture::Black | Picture::Pending => None,
            }
        })
        .collect()
}

// Spec: export R46, R47, R49, R51, R52; video R24.
#[test]
fn a_motion_jpeg_export_has_the_arrangements_frame_at_each_frame_time_and_its_sound_nulls() {
    let scratch = Scratch::new("frames");
    let (mut shell, _) = shell_with_a_movie(&scratch, 0);
    let path = scratch.path("Mix.wav");
    let result = export(
        &mut shell,
        json!({
            "path": text(&path),
            "bit_depth": 24,
            "sample_rate": 48000,
            "video": { "encoder": "motion_jpeg" },
        }),
    )
    .unwrap();

    // The movie sits beside the audio, named as it is with .mov.
    let movie_path = scratch.path("Mix.mov");
    assert_eq!(
        result.movie.as_deref().map(Path::new),
        Some(std::fs::canonicalize(&movie_path).unwrap().as_path())
    );
    assert_eq!(
        scratch.names(),
        ["Mix.mov", "Mix.wav", "mjpeg-24f.mov"],
        "no hidden file is left"
    );

    // Frame k is the Arrangement's picture at its time: the movie's own
    // frames, then black past its end. One second of audio is 30 frames of
    // 30000/1001 a second.
    let movie = Arc::new(Movie::open(&movie_path).unwrap());
    assert_eq!(movie.video().unwrap().frame_count(), 30);
    let expected: Vec<Option<u32>> = (0..30)
        .map(|k| if k < 24 { Some(k) } else { None })
        .collect();
    assert_eq!(indices(&movie, 30), expected);

    // The sound is the rendered audio, sample for sample.
    let wav = bl_media::decode_file(&path).unwrap();
    let mut sound = MovieAudio::open(&movie).unwrap();
    assert_eq!(sound.sample_rate(), 48_000);
    let decoded = sound.decode_all().unwrap();
    assert_eq!(decoded.frames(), wav.frames());
    for channel in 0..2 {
        assert!(
            decoded.channel(channel).unwrap() == wav.channel(channel).unwrap(),
            "channel {channel} differs"
        );
    }
}

// Spec: export R38.
#[test]
fn the_video_section_is_enabled_only_with_a_video_clip_in_the_range_and_pcm_on() {
    let scratch = Scratch::new("enabled");
    let disabled = |shell: &mut Shell| -> Vec<(String, String)> {
        let status: ExportStatus =
            serde_json::from_value(shell.run("export.open_dialog", json!({})).unwrap()).unwrap();
        status
            .dialog
            .unwrap()
            .disabled
            .into_iter()
            .filter(|d| {
                ["Create Video", "Video Encoder", "Encoder Settings"].contains(&d.control.as_str())
            })
            .map(|d| (d.control, d.reason))
            .collect()
    };

    // A set without a video clip.
    let mut plain = Shell::headless();
    let wav = scratch.path("Tone.wav");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/video/mjpeg-24f.wav"),
        &wav,
    )
    .unwrap();
    plain
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [text(&wav)], "at": 0 }),
        )
        .unwrap();
    let reasons = disabled(&mut plain);
    assert_eq!(reasons.len(), 3);
    assert!(
        reasons.iter().all(|(_, why)| why.contains("no video clip")),
        "{reasons:?}"
    );

    // With one, and the default of PCM on: enabled.
    let (mut shell, _) = shell_with_a_movie(&scratch, 0);
    assert!(disabled(&mut shell).is_empty());
    // Past the clip, the range holds none.
    shell
        .run("export.set_choices", json!({ "start": 8 * BAR }))
        .unwrap();
    assert_eq!(disabled(&mut shell).len(), 3);
    shell.run("export.close_dialog", json!({})).unwrap();

    // With Encode PCM off, the movie has no PCM file to take its sound from.
    shell.run("export.open_dialog", json!({})).unwrap();
    shell
        .run(
            "export.set_choices",
            json!({ "encode_pcm": false, "encode_mp3": true }),
        )
        .unwrap();
    let reasons = disabled(&mut shell);
    assert_eq!(reasons.len(), 3);
    assert!(
        reasons.iter().all(|(_, why)| why.contains("Encode PCM")),
        "{reasons:?}"
    );
}

// Spec: export R38, R52.
#[test]
fn an_export_without_a_video_clip_or_a_pcm_file_refuses_video_and_writes_nothing() {
    let scratch = Scratch::new("refused");
    let (mut shell, _) = shell_with_a_movie(&scratch, 0);
    let before = scratch.names();

    let mp3 = export(
        &mut shell,
        json!({ "path": text(&scratch.path("Mix.mp3")), "video": {} }),
    )
    .unwrap_err()
    .to_string();
    assert!(mp3.contains("PCM"), "{mp3}");

    // A range past the movie holds no video clip.
    let past = export(
        &mut shell,
        json!({
            "path": text(&scratch.path("Mix.wav")),
            "start": 8 * BAR,
            "length": BAR,
            "video": {},
        }),
    )
    .unwrap_err()
    .to_string();
    assert!(
        past.contains("no video clip") || past.contains("has none"),
        "{past}"
    );

    let bad = export(
        &mut shell,
        json!({
            "path": text(&scratch.path("Mix.wav")),
            "video": { "settings": { "quality": 0 } },
        }),
    )
    .unwrap_err()
    .to_string();
    assert!(bad.contains("quality"), "{bad}");
    assert_eq!(scratch.names(), before);
}

// Spec: export R51.
#[test]
fn a_movie_that_cannot_be_written_leaves_no_file_behind() {
    let scratch = Scratch::new("failure");
    let (mut shell, _) = shell_with_a_movie(&scratch, 0);
    // The movie's hidden file can't be made where a folder stands.
    std::fs::create_dir(scratch.path(".Mix.mov.partial")).unwrap();
    let before = scratch.names();
    let error = export(
        &mut shell,
        json!({ "path": text(&scratch.path("Mix.wav")), "video": { "encoder": "motion_jpeg" } }),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("movie"), "{error}");
    assert_eq!(
        scratch.names(),
        before,
        "neither file, nor an audio partial"
    );
}

// Spec: export R51.
#[test]
fn an_existing_movie_is_refused_unless_replaced() {
    let scratch = Scratch::new("exists");
    let (mut shell, _) = shell_with_a_movie(&scratch, 0);
    std::fs::write(scratch.path("Mix.mov"), b"old").unwrap();
    let args = |replace: bool| {
        json!({
            "path": text(&scratch.path("Mix.wav")),
            "replace": replace,
            "video": { "encoder": "motion_jpeg" },
        })
    };
    let error = export(&mut shell, args(false)).unwrap_err().to_string();
    assert!(error.contains("Mix.mov"), "{error}");
    assert!(!scratch.path("Mix.wav").exists());
    assert_eq!(std::fs::read(scratch.path("Mix.mov")).unwrap(), b"old");
    export(&mut shell, args(true)).unwrap();
    assert!(Movie::open(scratch.path("Mix.mov")).is_ok());
}

// Spec: export R50; video R25.
#[test]
fn with_a_video_clip_selected_the_movie_has_the_whole_original_with_its_preroll() {
    let scratch = Scratch::new("selected");
    let (mut shell, clip) = shell_with_a_movie(&scratch, BAR);
    // Trim the clip's head by an eighth of a bar: a quarter of a second of
    // the movie now comes before the clip.
    shell
        .run(
            "arrangement.resize_clips",
            json!({ "clips": [clip], "edge": "start", "by": BAR / 8 }),
        )
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let status: ExportStatus =
        serde_json::from_value(shell.run("export.open_dialog", json!({})).unwrap()).unwrap();
    let dialog = status.dialog.unwrap();
    // The movie's first instant is where the untrimmed clip began, and its
    // end is a second on: two quarters at 120 beats a minute.
    assert!(
        (dialog.start - Ticks(BAR)).get().abs() <= 1,
        "{:?}",
        dialog.start
    );
    assert!(
        (dialog.length - Ticks::QUARTER * 2).get().abs() <= 1,
        "{:?}",
        dialog.length
    );
    shell.run("export.close_dialog", json!({})).unwrap();

    // The same through `export.audio`, from the clip alone.
    let result = export(
        &mut shell,
        json!({
            "path": text(&scratch.path("Mix.wav")),
            "sample_rate": 48000,
            "video": { "encoder": "motion_jpeg", "clip": clip },
        }),
    )
    .unwrap();
    assert!((result.start - Ticks(BAR)).get().abs() <= 1);
    let movie = Arc::new(Movie::open(scratch.path("Mix.mov")).unwrap());
    // The movie is the original's whole length. What comes before the
    // trimmed clip is its pre-roll: the clip isn't playing there, so the
    // picture is black (R49), and the frames from the clip's start are the
    // movie's own.
    let shown = indices(&movie, 24);
    let preroll = 7;
    assert!(shown[..preroll].iter().all(Option::is_none), "{shown:?}");
    assert_eq!(
        shown[preroll..],
        (preroll as u32..24).map(Some).collect::<Vec<_>>()
    );
    assert_eq!(movie.video().unwrap().frame_count(), 30);
}

// Spec: export R48, R52; video R24.
#[test]
fn the_encoders_settings_change_the_rate_and_size_of_the_movie() {
    let scratch = Scratch::new("settings");
    let (mut shell, _) = shell_with_a_movie(&scratch, 0);
    export(
        &mut shell,
        json!({
            "path": text(&scratch.path("Mix.wav")),
            "video": {
                "encoder": "motion_jpeg",
                "settings": { "quality": 50, "frame_rate": 25.0, "size": { "width": 33, "height": 25 } },
            },
        }),
    )
    .unwrap();
    let movie = Movie::open(scratch.path("Mix.mov")).unwrap();
    let video = movie.video().unwrap();
    // Even sizes, and 25 frames for the second of audio.
    assert_eq!((video.width(), video.height()), (32, 24));
    assert_eq!(video.frame_count(), 25);
    assert_eq!(video.frame_start(1), Some(Rational::new(1, 25)));
}

// Spec: export R47, R52.
#[test]
fn the_dialog_lists_the_encoders_and_takes_them_headless() {
    let scratch = Scratch::new("dialog");
    let (mut shell, _) = shell_with_a_movie(&scratch, 0);
    let status: ExportStatus =
        serde_json::from_value(shell.run("export.open_dialog", json!({})).unwrap()).unwrap();
    let dialog = status.dialog.unwrap();
    assert!(
        dialog
            .video_encoders
            .contains(&bl_shell::export_commands::VideoEncoder::MotionJpeg)
    );
    // A new install shows H.264 where it is listed, else Motion JPEG.
    let shown = dialog.choices.video_encoder.unwrap();
    assert!(dialog.video_encoders.contains(&shown));
    assert!(!dialog.choices.create_video);

    shell
        .run(
            "export.set_choices",
            json!({
                "create_video": true,
                "video_encoder": "motion_jpeg",
                "video_settings": { "quality": 70 },
            }),
        )
        .unwrap();
    shell
        .run(
            "export.start",
            json!({ "path": text(&scratch.path("Mix.wav")) }),
        )
        .unwrap();
    while shell.export_running() {
        std::thread::yield_now();
        let _ = shell.run("export.status", json!({}));
    }
    assert!(scratch.path("Mix.mov").exists());
    assert!(scratch.path("Mix.wav").exists());
    // The choices are kept for the next export.
    let again: ExportStatus =
        serde_json::from_value(shell.run("export.open_dialog", json!({})).unwrap()).unwrap();
    let choices = again.dialog.unwrap().choices;
    assert!(choices.create_video);
    assert_eq!(choices.video_settings.quality, Some(70));
}
