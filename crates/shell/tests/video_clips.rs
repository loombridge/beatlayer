//! Video clips through the shell (WP-9.25): dropping a movie, the Session
//! View, the edits that make an audio clip, the picture through the warp
//! map, and the movie's markers. The behaviour is in the video spec, R5 to
//! R14.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{AudioData, ClipData, ClipId, Track, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell_spec::video::FrameAt;
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-video-clips-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// A copy of a generated test movie, which the tests then check wasn't
    /// written to.
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

fn new_shell(scratch: &Scratch) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(scratch.path("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

/// The default set's audio tracks, top to bottom.
fn audio_tracks(shell: &Shell) -> Vec<TrackId> {
    shell
        .editor()
        .document()
        .tracks()
        .filter(|track| track.kind() == TrackKind::Audio)
        .map(Track::id)
        .collect()
}

fn audio_track(shell: &Shell) -> TrackId {
    audio_tracks(shell)[0]
}

/// Drops `path` in the Arrangement on `track` at `at`, and returns the clip.
fn drop_in(shell: &mut Shell, track: TrackId, path: &Path, at: i64) -> ClipId {
    let added = shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [path.display().to_string()], "track": track, "at": at }),
        )
        .unwrap();
    serde_json::from_value(added["clips"][0]["clip"].clone()).unwrap()
}

fn audio(shell: &Shell, clip: ClipId) -> AudioData {
    match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => panic!("an audio clip"),
    }
}

fn frame_at(shell: &mut Shell, args: Value) -> FrameAt {
    serde_json::from_value(shell.run("video.frame_at", args).unwrap()).unwrap()
}

// Spec: video R5, R6, R9.
#[test]
fn a_dropped_movie_is_a_video_clip_that_saves_and_reopens() {
    let scratch = Scratch::new("drop");
    let movie = scratch.movie("mjpeg-24f.mov");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let clip = drop_in(&mut shell, track, &movie, 0);

    let data = audio(&shell, clip);
    assert!(data.video && data.warp && data.tempo_leader);
    // The movie's second, at the set's 120 BPM, is two quarters.
    let length = shell.editor().document().clip(clip).unwrap().length;
    assert!((length - Ticks::QUARTER * 2).get().abs() <= 1, "{length:?}");
    assert!(shell.editor().document().any_tempo_leader());

    let saved = scratch.path("Movie.blset");
    let result = shell
        .run("set.save_as", json!({ "path": saved.display().to_string() }))
        .unwrap();
    let saved = PathBuf::from(result["path"].as_str().unwrap());
    let mut again = new_shell(&scratch);
    again
        .run("set.open", json!({ "path": saved.display().to_string() }))
        .unwrap();
    let ClipData::Audio(reopened) = &again.editor().document().clip(clip).unwrap().data else {
        panic!("an audio clip");
    };
    assert!(reopened.video && reopened.tempo_leader);
    // The query tool's clip holds it too.
    assert!(
        serde_json::to_string(&again.editor().document().clip(clip).unwrap())
            .unwrap()
            .contains("\"video\":true")
    );
}

// Spec: video R5.
#[test]
fn a_movie_dropped_in_the_session_view_is_an_audio_clip() {
    let scratch = Scratch::new("session");
    let movie = scratch.movie("mjpeg-24f.mov");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let scene = shell.editor().document().scenes().next().unwrap().id;
    shell
        .run(
            "session.drop_files",
            json!({ "paths": [movie.display().to_string()], "track": track, "scene": scene }),
        )
        .unwrap();
    let held = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .slot(scene)
        .expect("a clip in the slot")
        .clone();
    let ClipData::Audio(data) = &held.data else {
        panic!("an audio clip");
    };
    assert!(!data.video && !data.tempo_leader);
}

// Spec: video R8.
#[test]
fn consolidate_reverse_and_crop_make_an_audio_clip_and_leave_the_movie() {
    let scratch = Scratch::new("edits");
    let movie = scratch.movie("mjpeg-24f.mov");
    let before = std::fs::read(&movie).unwrap();
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let clip = drop_in(&mut shell, track, &movie, 0);
    let whole = Ticks::QUARTER * 2;
    let end = shell.editor().document().clip(clip).unwrap().length;
    assert!((end - whole).get().abs() <= 1);

    shell.run("clip.reverse", json!({ "clip": clip })).unwrap();
    let reversed = audio(&shell, clip);
    assert!(!reversed.video);
    let reversed_media = reversed.media;

    shell.run("clip.crop", json!({ "clip": clip })).unwrap();
    assert!(!audio(&shell, clip).video);

    let other = drop_in(&mut shell, track, &movie, Ticks::QUARTER.get() * 8);
    assert!(audio(&shell, other).video);
    let result = shell
        .run(
            "arrangement.consolidate",
            json!({
                "tracks": [track],
                "start": Ticks::QUARTER.get() * 8,
                "end": Ticks::QUARTER.get() * 10,
            }),
        )
        .unwrap();
    let created: Vec<ClipId> = serde_json::from_value(result["created"].clone()).unwrap();
    let consolidated = audio(&shell, created[0]);
    assert!(!consolidated.video);
    assert_ne!(consolidated.media, reversed_media);
    assert_eq!(std::fs::read(&movie).unwrap(), before, "the movie is intact");
}

// Spec: video R10, R11, R13.
#[test]
fn a_warp_marker_moves_which_frame_plays_when() {
    let scratch = Scratch::new("frames");
    let movie = scratch.movie("mjpeg-24f.mov");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let start = Ticks::QUARTER.get() * 4;
    let clip = drop_in(&mut shell, track, &movie, start);

    // A quarter at 120 BPM is half a second; the clip leads, so the movie
    // plays at its own speed. 0.25 s into 30000/1001 frames a second is
    // frame 7.
    let at = start + Ticks::QUARTER.get() / 2;
    let before = frame_at(&mut shell, json!({ "at": at }));
    assert_eq!(before.clip, Some(clip));
    assert_eq!(before.frame, Some(7));
    assert!((before.movie_time.unwrap() - 0.25).abs() < 1e-6);
    // Before the clip, black.
    let none = frame_at(&mut shell, json!({ "at": 0 }));
    assert_eq!((none.clip, none.frame, none.movie_time), (None, None, None));

    // A marker there, whose time in the movie moves 0.2 s later: frame 13.
    shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [clip], "beat": Ticks::QUARTER.get() / 2 }),
        )
        .unwrap();
    shell
        .run(
            "clip.shift_warp_marker",
            json!({ "clips": [clip], "marker": 1, "by": 0.2 }),
        )
        .unwrap();
    let after = frame_at(&mut shell, json!({ "at": at, "clip": clip }));
    assert_eq!(after.frame, Some(13));
    assert!((after.movie_time.unwrap() - 0.45).abs() < 1e-6);

    // An audio clip isn't one.
    shell
        .run("clip.reverse", json!({ "clip": clip }))
        .unwrap();
    assert!(
        shell
            .run("video.frame_at", json!({ "at": at, "clip": clip }))
            .is_err()
    );
}

// Spec: video R10.
#[test]
fn the_bottom_most_leader_leads_and_its_picture_shows() {
    let scratch = Scratch::new("bottom");
    let movie = scratch.movie("mjpeg-24f.mov");
    let mut shell = new_shell(&scratch);
    let (upper, lower) = {
        let tracks = audio_tracks(&shell);
        (tracks[0], tracks[1])
    };
    let a = drop_in(&mut shell, upper, &movie, 0);
    let b = drop_in(&mut shell, lower, &movie, 0);
    // Warp the lower one at another tempo through its tempo: it leads there.
    shell
        .run("clip.set_segment_bpm", json!({ "clip": b, "bpm": 60.0 }))
        .unwrap();
    let shown = frame_at(&mut shell, json!({ "at": Ticks::QUARTER.get() }));
    assert_eq!(shown.clip, Some(b));
    // The song's tempo over the clips is the lower leader's.
    let bpm = shell
        .editor()
        .document()
        .tempo_map()
        .bpm_at(Ticks::QUARTER)
        .get();
    assert!((bpm - 60.0).abs() < 1e-9, "{bpm}");
    let _ = a;
}

// Spec: video R12.
#[test]
fn the_movies_markers_are_cues() {
    let scratch = Scratch::new("cues");
    let movie = scratch.movie("mjpeg-chapters.mov");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let clip = drop_in(&mut shell, track, &movie, 0);
    let cues = shell.clip_cues(clip);
    assert_eq!(
        cues.iter().map(|c| (c.time, c.name.as_str())).collect::<Vec<_>>(),
        [(0.0, "Intro"), (0.5, "Drop")]
    );
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let state = shell.run("clip_view.sample_editor", json!({})).unwrap();
    assert_eq!(state["cues"][1]["name"], "Drop");
    // An audio clip has none.
    let other = drop_in(&mut shell, track, &scratch.movie("mjpeg-24f.mov"), 100_000_000);
    shell.run("clip.reverse", json!({ "clip": other })).unwrap();
    assert!(shell.clip_cues(other).is_empty());
}

fn hash_of(path: &Path) -> String {
    bl_media::ContentHash::of_file(path)
        .unwrap()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// Spec: video R14.
#[test]
fn a_video_clips_movie_is_a_file_the_file_manager_lists_and_replaces() {
    let scratch = Scratch::new("files");
    let movie = scratch.movie("mjpeg-24f.mov");
    let other = scratch.movie("mjpeg-chapters.mov");
    let sound = scratch.movie("mjpeg-24f.wav");
    let mut shell = new_shell(&scratch);
    let track = audio_track(&shell);
    let clip = drop_in(&mut shell, track, &movie, 0);
    shell
        .run(
            "set.save_as",
            json!({ "path": scratch.path("Song.blset").display().to_string() }),
        )
        .unwrap();

    // The movie is listed, with the clip that uses it.
    let state = shell.run("file_manager.state", json!({})).unwrap();
    let row = state["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["name"] == "mjpeg-24f.mov")
        .expect("the movie is listed");
    assert!(!row["used_by"].as_array().unwrap().is_empty(), "{row}");

    // Another movie in its place keeps the clip a video clip; a sound makes
    // it an audio clip.
    shell
        .run(
            "file_manager.replace_file",
            json!({ "hash": hash_of(&movie), "path": other.display().to_string() }),
        )
        .unwrap();
    assert!(audio(&shell, clip).video);
    shell
        .run(
            "file_manager.replace_file",
            json!({ "hash": hash_of(&other), "path": sound.display().to_string() }),
        )
        .unwrap();
    assert!(!audio(&shell, clip).video);
}
