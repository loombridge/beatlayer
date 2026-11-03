//! The Video Window through the shell (WP-9.26, the video spec R16 to R23):
//! whether it shows, where it is kept, the frame it draws, and the commands
//! that show and place it. The window itself is the host's.

use std::sync::{Arc, Mutex, PoisonError};

use bl_model::{Clip, ClipData, ClipId, WindowFrame};
use bl_shell_spec::video::{
    FrameAt, SET_WINDOW, SetWindowArgs, TOGGLE_WINDOW, WINDOW_STATE, WindowState,
};
use bl_video::{Frame, Movie, Picture, Rational, VideoThread};
use serde_json::Value;

use super::video::movie_frame;
use super::*;

/// The Video Window's state. Which frame it shows is worked out when it
/// draws, from the playhead, so nothing here is saved with a set.
#[derive(Default)]
pub(super) struct VideoWindow {
    /// Whether the person (or a drop) asked for it. It shows only while the
    /// set has a video clip.
    open: bool,
    full_screen: bool,
    /// While a video clip's warp marker is dragged: the clip and the
    /// marker's time in its movie (R19).
    scrub: Option<(ClipId, f64)>,
    decoding: Mutex<Decoding>,
}

/// The video thread of the movie shown, and what the window last drew.
#[derive(Default)]
struct Decoding {
    /// The movie shown and its thread; `None` for the thread when no
    /// decoder takes the movie.
    thread: Option<(Arc<Movie>, Option<VideoThread>)>,
    asked: Option<Rational>,
    last: Option<Arc<Frame>>,
}

/// What the Video Window draws now (R18, R19, R23).
#[derive(Clone, Debug, Default)]
pub struct VideoView {
    /// The video clip whose movie shows, or `None` where none plays: black.
    pub clip: Option<ClipId>,
    /// That clip's name.
    pub name: String,
    /// The time in its movie, in seconds.
    pub movie_time: Option<f64>,
    /// The index of the frame shown then, and when it starts in the movie.
    pub frame: Option<u32>,
    /// When that frame starts to show on the movie's timeline, in seconds.
    pub frame_time: Option<f64>,
    /// The picture, or `None` for black. While the frame wanted isn't
    /// decoded yet this is the last picture the window had.
    pub picture: Option<Arc<Frame>>,
    /// Whether the frame wanted isn't decoded yet, so the window draws again
    /// soon.
    pub pending: bool,
}

/// Stops a video thread without making the caller wait for the frame it is
/// decoding (R23).
fn retire(old: Option<(Arc<Movie>, Option<VideoThread>)>) {
    if let Some((_, Some(thread))) = old {
        std::thread::spawn(move || drop(thread));
    }
}

impl Shell {
    /// Whether the Video Window shows (R16): it was asked for, and the set
    /// has a video clip in the Arrangement.
    #[must_use]
    pub fn video_window_shown(&self) -> bool {
        self.video_window.open && self.editor.document().has_video_clip()
    }

    /// Whether the window is in full screen.
    #[must_use]
    pub fn video_window_full_screen(&self) -> bool {
        self.video_window.full_screen && self.video_window_shown()
    }

    /// Where the window was last, and how big (R17), kept in the
    /// preferences.
    #[must_use]
    pub fn video_window_frame(&self) -> Option<WindowFrame> {
        self.prefs.video_window
    }

    /// Records where the window sits and how big it is, as the host sees it,
    /// for the next time it opens and the next launch.
    pub fn set_video_window_frame(&mut self, frame: WindowFrame) {
        if frame.is_valid() && self.prefs.video_window != Some(frame) {
            self.prefs.video_window = Some(frame);
        }
    }

    /// Records that the window entered or left full screen by other means.
    pub fn set_video_full_screen_state(&mut self, on: bool) {
        if self.video_window.full_screen != on {
            self.video_window.full_screen = on;
            self.touch();
        }
    }

    /// Shows the window for a movie just dropped in the Arrangement (R16).
    pub(super) fn show_video_window_for(&mut self, added: &[ClipId]) {
        let doc = self.editor.document();
        let video = |id: &ClipId| {
            doc.clip(*id)
                .is_some_and(|c| matches!(&c.data, ClipData::Audio(audio) if audio.video))
        };
        if added.iter().any(video) && !self.video_window_shown() {
            self.video_window.open = true;
            self.video_window.full_screen = false;
        }
    }

    /// Notes the time a warp marker being dragged has in its video clip's
    /// movie, for the window to show (R19), and forgets it when the drag
    /// ends. `invocation` is the drag's step.
    pub(super) fn follow_warp_marker(
        &mut self,
        phase: GesturePhase,
        invocation: Option<&Invocation>,
    ) {
        let scrub = match (phase, invocation) {
            (GesturePhase::Update, Some(invocation)) => {
                self.dragged_marker(invocation).or(self.video_window.scrub)
            }
            (GesturePhase::Update, None) => self.video_window.scrub,
            _ => None,
        };
        if self.video_window.scrub != scrub {
            self.video_window.scrub = scrub;
            self.touch();
        }
    }

    /// The video clip and marker time a warp marker command moves.
    fn dragged_marker(&self, invocation: &Invocation) -> Option<(ClipId, f64)> {
        let args = &invocation.args;
        let (clip, time) = match invocation.id.as_str() {
            "clip.insert_warp_marker" => (
                serde_json::from_value(args["clips"][0].clone()).ok()?,
                args["time"].as_f64(),
            ),
            "clip.move_warp_markers" | "clip.shift_warp_marker" => {
                let clip: ClipId = serde_json::from_value(args["clips"][0].clone()).ok()?;
                let place = args["markers"][0]
                    .as_u64()
                    .or_else(|| args["marker"].as_u64())?;
                let marker = match &self.editor.document().clip(clip)?.data {
                    ClipData::Audio(audio) => audio.warp_markers.get(place as usize)?.time,
                    ClipData::Midi(_) => return None,
                };
                (clip, Some(marker))
            }
            _ => return None,
        };
        self.clip_movie(clip)?;
        Some((clip, time?))
    }

    /// What the window draws now: the frame at the playhead within one frame
    /// of the bottom-most track's playing video clip (R18), or the frame
    /// under a warp marker being dragged (R19). It asks the video thread for
    /// the frame and takes what has been decoded: it never waits (R23).
    #[must_use]
    pub fn video_view(&self) -> VideoView {
        let doc = self.editor.document();
        let scrubbed = self
            .video_window
            .scrub
            .and_then(|(id, time)| Some((doc.clip(id)?, Some(time))));
        let (clip, time): (&Clip, Option<f64>) = match scrubbed {
            Some(found) => found,
            None => match doc.video_clip_at(self.arrangement.transport().position) {
                Some(clip) => (clip, None),
                None => return self.black(),
            },
        };
        let Some(movie) = self.clip_movie(clip.id) else {
            return self.black();
        };
        let movie_time = time.or_else(|| {
            clip.movie_time_at(
                self.arrangement.transport().position,
                &doc.tempo_map(),
                movie.duration().seconds(),
            )
        });
        let mut view = VideoView {
            clip: Some(clip.id),
            name: clip.name.clone(),
            movie_time,
            ..VideoView::default()
        };
        let wanted = movie_time.and_then(|seconds| {
            let (frame, at) = movie_frame(&movie, seconds)?;
            Some((frame, at, movie.video()?.frame_start(frame)?))
        });
        let Some((frame, at, start)) = wanted else {
            // Before the movie, or a movie without a picture: black.
            self.decoding().last = None;
            return view;
        };
        view.frame = Some(frame as u32);
        view.frame_time = Some(start.seconds());

        let mut decoding = self.decoding();
        if !decoding
            .thread
            .as_ref()
            .is_some_and(|(shown, _)| Arc::ptr_eq(shown, &movie))
        {
            let spawned = VideoThread::spawn(Arc::clone(&movie)).ok();
            decoding.asked = None;
            let old = decoding.thread.replace((Arc::clone(&movie), spawned));
            retire(old);
        }
        let Some((_, Some(thread))) = &decoding.thread else {
            return view;
        };
        if decoding.asked != Some(at) {
            thread.request(at);
        }
        let picture = thread.picture_at(at);
        decoding.asked = Some(at);
        match picture {
            Picture::Frame(frame) => decoding.last = Some(frame),
            Picture::Black => decoding.last = None,
            Picture::Pending => view.pending = true,
        }
        view.picture.clone_from(&decoding.last);
        view
    }

    fn decoding(&self) -> std::sync::MutexGuard<'_, Decoding> {
        self.video_window
            .decoding
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Black: no video clip plays at the playhead (R18).
    fn black(&self) -> VideoView {
        self.decoding().last = None;
        VideoView::default()
    }

    /// Blocks until the video thread has decoded what the window last asked
    /// of it. For tests, which drive time themselves: the window never
    /// waits.
    pub fn wait_for_video(&self) {
        if let Some((_, Some(thread))) = &self.decoding().thread {
            thread.wait_idle();
        }
    }

    fn video_window_state(&self) -> WindowState {
        let shown = self.video_window_shown();
        let view = if shown {
            self.video_view()
        } else {
            VideoView::default()
        };
        WindowState {
            shown,
            enabled: self.editor.document().has_video_clip(),
            frame: self.prefs.video_window,
            full_screen: self.video_window_full_screen(),
            showing: FrameAt {
                clip: view.clip,
                movie_time: view.movie_time,
                frame: view.frame,
                frame_time: view.frame_time,
            },
        }
    }

    fn show_video_window(&mut self, id: &str) -> Result<(), ShellError> {
        if self.video_window_shown() {
            return Ok(());
        }
        if !self.editor.document().has_video_clip() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "the set has no video clip in the Arrangement".to_owned(),
            });
        }
        self.video_window.open = true;
        self.video_window.full_screen = false;
        Ok(())
    }

    fn hide_video_window(&mut self) {
        self.video_window.open = false;
        self.video_window.full_screen = false;
        // The thread and its frames go with the window.
        let old = std::mem::take(&mut *self.decoding()).thread;
        retire(old);
    }

    /// Runs `view.toggle_video_window`, `video.window_state` and
    /// `video.set_window` (R16, R17, R20, R21).
    pub(super) fn run_video_window(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            WINDOW_STATE => {
                Self::args::<NoArgs>(id, args)?;
            }
            TOGGLE_WINDOW => {
                Self::args::<NoArgs>(id, args)?;
                if self.video_window_shown() {
                    self.hide_video_window();
                } else {
                    self.show_video_window(id)?;
                }
                self.touch();
            }
            _ => {
                debug_assert_eq!(id, SET_WINDOW);
                let SetWindowArgs {
                    shown,
                    frame,
                    full_screen,
                    original_size,
                } = Self::args(id, args)?;
                let invalid = |message: &str| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: message.to_owned(),
                };
                if frame.is_some_and(|frame| !frame.is_valid()) {
                    return Err(invalid("`frame` needs finite numbers and a size above 0"));
                }
                match shown {
                    Some(true) => self.show_video_window(id)?,
                    Some(false) => self.hide_video_window(),
                    None => {}
                }
                let mut frame = frame;
                if original_size {
                    let movie = self
                        .window_clip()
                        .and_then(|clip| self.clip_movie(clip))
                        .and_then(|movie| {
                            let track = movie.video()?;
                            Some((f64::from(track.width()), f64::from(track.height())))
                        })
                        .ok_or_else(|| ShellError::Failed {
                            command: id.to_owned(),
                            message: "no movie with a picture to take the size of".to_owned(),
                        })?;
                    let at = frame.or(self.prefs.video_window);
                    frame = Some(WindowFrame {
                        x: at.map_or(0.0, |f| f.x),
                        y: at.map_or(0.0, |f| f.y),
                        width: movie.0,
                        height: movie.1,
                    });
                }
                if let Some(frame) = frame {
                    self.prefs.video_window = Some(frame);
                    self.effects.push(Effect::PlaceVideoWindow(frame));
                    self.save_preferences();
                }
                if let Some(on) = full_screen {
                    if on && !self.video_window_shown() {
                        return Err(ShellError::Unavailable {
                            command: id.to_owned(),
                            reason: "the Video Window isn't showing".to_owned(),
                        });
                    }
                    if self.video_window.full_screen != on {
                        self.video_window.full_screen = on;
                        self.effects.push(Effect::SetVideoFullScreen(on));
                    }
                }
                self.touch();
            }
        }
        Ok(to_value(&self.video_window_state()))
    }

    /// The video clip the window shows or would: the one at the playhead,
    /// else the first in the Arrangement.
    fn window_clip(&self) -> Option<ClipId> {
        let doc = self.editor.document();
        doc.video_clip_at(self.arrangement.transport().position)
            .or_else(|| {
                doc.track_order()
                    .into_iter()
                    .filter_map(|id| doc.track(id))
                    .flat_map(|track| track.clips())
                    .find(|clip| matches!(&clip.data, ClipData::Audio(audio) if audio.video))
            })
            .map(|clip| clip.id)
    }
}
