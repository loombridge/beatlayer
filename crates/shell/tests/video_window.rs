//! The Video Window through the shell (WP-9.26): when it shows, the frame
//! it draws at the playhead and under a dragged warp marker, and its place,
//! size and full screen. The behaviour is in the video spec, R16 to R23.

use std::path::{Path, PathBuf};

use bl_audio_io::NullControl;
use bl_core::time::Ticks;
use bl_model::{Track, TrackId, TrackKind};
use bl_shell::testing;
use bl_shell::video::{SET_WINDOW, TOGGLE_WINDOW, WINDOW_STATE, WindowState};
use bl_shell::{Effect, GesturePhase, Invocation, Shell, VideoFrame};
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-video-window-{name}-{}-{}",
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

/// A shell on a null device paced by hand, with preferences kept in the
/// scratch folder: a second shell on it is a relaunch.
fn new_shell(scratch: &Scratch) -> (Shell, NullControl) {
    std::fs::create_dir_all(scratch.path("config")).unwrap();
    let (mut shell, control) = testing::rig_in(&scratch.path("config"));
    shell.start_audio();
    (shell, control)
}

fn audio_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(Track::id)
        .unwrap()
}

/// Drops `path` in the Arrangement on the first audio track at 0.
fn drop_movie(shell: &mut Shell, path: &Path) -> bl_model::ClipId {
    let track = audio_track(shell);
    let added = shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [path.display().to_string()], "track": track, "at": 0 }),
        )
        .unwrap();
    serde_json::from_value(added["clips"][0]["clip"].clone()).unwrap()
}

fn state(shell: &mut Shell) -> WindowState {
    serde_json::from_value(shell.run(WINDOW_STATE, json!({})).unwrap()).unwrap()
}

/// The index the generator's movies show in the top row of cells of a
/// frame.
fn index_of(frame: &VideoFrame) -> u32 {
    (0..8)
        .map(|bit| {
            let [r, g, b, _] = frame.pixel(8 * bit + 4, 4).expect("inside the picture");
            let luma = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
            u32::from(luma > 128) << bit
        })
        .sum()
}

/// Starts the song at `from` and runs it for `millis` of test time, then
/// returns where the playhead is.
fn play_from(shell: &mut Shell, control: &NullControl, from: Ticks, millis: u64) -> Ticks {
    shell
        .run("transport.locate", json!({ "position": from }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    testing::advance(control, shell, millis);
    shell.poll_engine().position
}

/// What the window draws now, once the video thread has decoded it: the
/// frame the view names and the index its picture shows.
fn drawn(shell: &Shell) -> (Option<u32>, Option<u32>) {
    let _ = shell.video_view();
    shell.wait_for_video();
    let view = shell.video_view();
    assert!(!view.pending, "decoded after waiting for the thread");
    (view.frame, view.picture.as_deref().map(index_of))
}

/// The song position, in ticks, a third of the way into movie frame `i`:
/// 120 BPM, so a second is two quarters.
fn inside_frame(i: i64) -> Ticks {
    let seconds = (i as f64 + 0.3) * 1001.0 / 30000.0;
    Ticks((seconds * 2.0 * Ticks::QUARTER.get() as f64).round() as i64)
}

// Spec: video R16.
#[test]
fn the_window_needs_a_video_clip_and_a_dropped_movie_shows_it() {
    let scratch = Scratch::new("drop");
    let movie = scratch.movie("mjpeg-24f.mov");
    let (mut shell, _) = new_shell(&scratch);

    // A new set has no video clip: the menu item is disabled and ticked off,
    // and the command refuses.
    assert!(!shell.can_invoke(TOGGLE_WINDOW, &json!({})));
    assert_eq!(shell.switch_state(TOGGLE_WINDOW), Some(false));
    assert!(shell.run(TOGGLE_WINDOW, json!({})).is_err());
    assert!(shell.run(SET_WINDOW, json!({ "shown": true })).is_err());
    let before = state(&mut shell);
    assert!(!before.shown && !before.enabled);

    // Dropping a movie shows the window.
    let clip = drop_movie(&mut shell, &movie);
    assert!(shell.can_invoke(TOGGLE_WINDOW, &json!({})));
    assert_eq!(shell.switch_state(TOGGLE_WINDOW), Some(true));
    let shown = state(&mut shell);
    assert!(shown.shown && shown.enabled);
    assert_eq!(shown.showing.clip, Some(clip));

    // The command hides and shows it; the menu item follows.
    assert!(
        !serde_json::from_value::<WindowState>(shell.run(TOGGLE_WINDOW, json!({})).unwrap())
            .unwrap()
            .shown
    );
    assert_eq!(shell.switch_state(TOGGLE_WINDOW), Some(false));
    assert!(state(&mut shell).enabled);
    shell.run(TOGGLE_WINDOW, json!({})).unwrap();
    assert!(shell.video_window_shown());

    // Deleting the last video clip closes it.
    shell.run("clip.delete", json!({ "clip": clip })).unwrap();
    assert!(!shell.video_window_shown());
    assert!(!state(&mut shell).enabled);
}

// Spec: video R18, R23.
#[test]
fn the_window_draws_the_frame_at_the_playhead_and_black_where_none_plays() {
    let scratch = Scratch::new("playhead");
    let movie = scratch.movie("mjpeg-24f.mov");
    let (mut shell, control) = new_shell(&scratch);
    drop_movie(&mut shell, &movie);

    // During playback the frame drawn is the one at the playhead: the one
    // `video.frame_at` names there, and its picture shows that frame's index.
    let mut seen = std::collections::BTreeSet::new();
    let mut at = play_from(&mut shell, &control, inside_frame(0), 0);
    for step in 0..14 {
        let expected = shell.run("video.frame_at", json!({ "at": at })).unwrap()["frame"]
            .as_u64()
            .map(|frame| frame as u32);
        assert!(expected.is_some(), "step {step}: the clip plays at {at:?}");
        assert_eq!(drawn(&shell), (expected, expected), "step {step} at {at:?}");
        seen.insert(expected);
        testing::advance(&control, &mut shell, 50);
        at = shell.poll_engine().position;
    }
    assert!(
        seen.len() >= 10,
        "playback moved through the movie: {seen:?}"
    );

    // After the clip there is none: black, and nothing to read.
    shell.run("transport.stop", json!({})).unwrap();
    let after = play_from(&mut shell, &control, Ticks::QUARTER * 6, 20);
    assert!(after >= Ticks::QUARTER * 6);
    assert_eq!(drawn(&shell), (None, None));
    let black = shell.video_view();
    assert!(black.clip.is_none() && black.picture.is_none() && !black.pending);
    assert_eq!(state(&mut shell).showing.clip, None);
}

// Spec: video R19.
#[test]
fn a_dragged_warp_marker_shows_the_frame_under_it() {
    let scratch = Scratch::new("scrub");
    let movie = scratch.movie("mjpeg-24f.mov");
    let (mut shell, _) = new_shell(&scratch);
    let clip = drop_movie(&mut shell, &movie);
    assert_eq!(drawn(&shell).0, Some(0));

    // A marker in the middle of the movie, at the time frame 10 starts.
    let time = 10.0 * 1001.0 / 30000.0 + 0.001;
    shell
        .run(
            "clip.insert_warp_marker",
            json!({ "clips": [clip], "time": time }),
        )
        .unwrap();
    // Dragging it, with the playhead still on frame 0, shows frame 10.
    let marker = shell
        .editor()
        .document()
        .clip(clip)
        .and_then(|c| match &c.data {
            bl_model::ClipData::Audio(audio) => audio
                .warp_markers
                .iter()
                .position(|m| (m.time - time).abs() < 1e-9),
            bl_model::ClipData::Midi(_) => None,
        })
        .expect("the marker");
    let beat = Ticks::QUARTER.get() * 3;
    let drag = |by: i64| {
        Invocation::new(
            "clip.move_warp_markers",
            json!({ "clips": [clip], "markers": [marker], "by": by }),
        )
    };
    shell
        .gesture(GesturePhase::Update, Some(&drag(beat / 16)))
        .unwrap();
    let _ = shell.video_view();
    shell.wait_for_video();
    let view = shell.video_view();
    assert_eq!(view.frame, Some(10));
    assert_eq!(view.picture.as_deref().map(index_of), Some(10));
    assert_eq!(state(&mut shell).showing.frame, Some(10));

    // When the drag ends, the playhead's frame comes back.
    shell
        .gesture(GesturePhase::Commit, Some(&drag(beat / 8)))
        .unwrap();
    assert_eq!(drawn(&shell), (Some(0), Some(0)));
}

// Spec: video R17, R20, R21.
#[test]
fn the_window_keeps_its_place_and_size_and_goes_full_screen_and_to_the_movies_size() {
    let scratch = Scratch::new("frame");
    let movie = scratch.movie("mjpeg-24f.mov");
    let (mut shell, _) = new_shell(&scratch);
    drop_movie(&mut shell, &movie);
    shell.take_effects();
    assert_eq!(state(&mut shell).frame, None);

    // Placing it is kept in the preferences, not in the set: a relaunch, and
    // another set, find it there.
    let frame = json!({ "x": 120.0, "y": 80.0, "width": 480.0, "height": 270.0 });
    let result = shell.run(SET_WINDOW, json!({ "frame": frame })).unwrap();
    let kept: WindowState = serde_json::from_value(result).unwrap();
    assert_eq!(
        kept.frame.map(|f| (f.x, f.y, f.width, f.height)),
        Some((120.0, 80.0, 480.0, 270.0))
    );
    assert!(matches!(
        shell.take_effects()[..],
        [Effect::PlaceVideoWindow(_)]
    ));
    assert!(
        shell
            .run(
                SET_WINDOW,
                json!({ "frame": { "x": 0.0, "y": 0.0, "width": 0.0, "height": 9.0 } })
            )
            .is_err(),
        "a window has a size"
    );
    let (mut again, _) = new_shell(&scratch);
    assert_eq!(
        again
            .video_window_frame()
            .map(|f| (f.x, f.y, f.width, f.height)),
        Some((120.0, 80.0, 480.0, 270.0))
    );
    assert!(!again.video_window_shown(), "a new set has no video clip");
    drop_movie(&mut again, &movie);
    assert_eq!(
        state(&mut again).frame.map(|f| f.width),
        Some(480.0),
        "the same in every set"
    );

    // Full screen, and back; the host reports leaving it by other means.
    shell.take_effects();
    let full: WindowState = serde_json::from_value(
        shell
            .run(SET_WINDOW, json!({ "full_screen": true }))
            .unwrap(),
    )
    .unwrap();
    assert!(full.full_screen);
    assert_eq!(shell.take_effects(), [Effect::SetVideoFullScreen(true)]);
    shell.set_video_full_screen_state(false);
    assert!(!state(&mut shell).full_screen);

    // The movie's own size, keeping the place.
    shell
        .run(SET_WINDOW, json!({ "original_size": true }))
        .unwrap();
    let own = state(&mut shell).frame.expect("placed");
    assert_eq!(
        (own.x, own.y, own.width, own.height),
        (120.0, 80.0, 64.0, 48.0)
    );
    assert!(matches!(
        shell.take_effects()[..],
        [Effect::PlaceVideoWindow(_)]
    ));

    // Hiding closes it and leaves full screen.
    shell
        .run(SET_WINDOW, json!({ "full_screen": true }))
        .unwrap();
    let hidden: WindowState =
        serde_json::from_value(shell.run(SET_WINDOW, json!({ "shown": false })).unwrap()).unwrap();
    assert!(!hidden.shown && !hidden.full_screen);
    assert!(
        shell
            .run(SET_WINDOW, json!({ "full_screen": true }))
            .is_err()
    );
}
