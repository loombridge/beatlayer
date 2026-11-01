//! Video clips through the shell (WP-9.25, the video spec R12, R13): the
//! movie a clip names, read once and kept, `video.frame_at`, and the cues a
//! movie's markers give the Sample Editor.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use bl_media::ContentHash;
use bl_model::{Clip, ClipData, ClipId};
use bl_shell_spec::sample_editor::Cue;
use bl_shell_spec::video::{FRAME_AT, FrameAt, FrameAtArgs};
use bl_video::{Movie, Rational};
use serde_json::Value;

use super::*;

/// The movies the set's video clips name, by the content hash of their
/// files, each read once. A file that can't be read is asked for again, in
/// case it has been relinked.
#[derive(Default)]
pub(super) struct Movies(Mutex<HashMap<ContentHash, Arc<Movie>>>);

impl Shell {
    /// The movie a clip plays, if it is a video clip whose file can be read.
    #[must_use]
    pub fn clip_movie(&self, clip: ClipId) -> Option<Arc<Movie>> {
        let Some(ClipData::Audio(audio)) = self.editor.document().clip(clip).map(|c| &c.data)
        else {
            return None;
        };
        audio
            .video
            .then(|| self.movie_of(ContentHash::from_bytes(audio.media.0)))
            .flatten()
    }

    fn movie_of(&self, hash: ContentHash) -> Option<Arc<Movie>> {
        let mut movies = self.movies.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(movie) = movies.get(&hash) {
            return Some(Arc::clone(movie));
        }
        let movie = Arc::new(Movie::open(self.media.entry(hash)?.path).ok()?);
        movies.insert(hash, Arc::clone(&movie));
        Some(movie)
    }

    /// A video clip's cues (R12): the markers its movie holds, at their
    /// times in the movie. Empty for any other clip.
    #[must_use]
    pub fn clip_cues(&self, clip: ClipId) -> Vec<Cue> {
        self.clip_movie(clip).map_or_else(Vec::new, |movie| {
            movie
                .markers()
                .iter()
                .map(|marker| Cue {
                    time: marker.time.seconds(),
                    name: marker.name.clone(),
                })
                .collect()
        })
    }

    /// Puts a video clip's cues in `clip_view.sample_editor`'s result.
    pub(super) fn add_cues(&self, state: &mut Value) {
        let Some(clip) = state
            .get("clip")
            .and_then(|clip| serde_json::from_value::<ClipId>(clip.clone()).ok())
        else {
            return;
        };
        if let Ok(cues) = serde_json::to_value(self.clip_cues(clip)) {
            state["cues"] = cues;
        }
    }

    /// Runs `video.frame_at` (R13).
    pub(super) fn run_video(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        debug_assert_eq!(id, FRAME_AT);
        let FrameAtArgs {
            at,
            movie_time: asked,
            clip,
        } = Self::args(id, args)?;
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let invalid = |message: &str| ShellError::InvalidArgs {
            command: id.to_owned(),
            message: message.to_owned(),
        };
        let doc = self.editor.document();
        let video_clip = |clip: ClipId| {
            doc.clip(clip)
                .filter(|c| matches!(&c.data, ClipData::Audio(audio) if audio.video))
                .ok_or_else(|| failed(format!("clip {} isn't a video clip", clip.0)))
        };
        let found: Option<&Clip> = match (at, asked, clip) {
            (Some(_), Some(_), _) | (None, None, _) => {
                return Err(invalid("give `at` or `movie_time`, not both or neither"));
            }
            (None, Some(_), None) => return Err(invalid("`movie_time` needs a `clip`")),
            (_, _, Some(clip)) => Some(video_clip(clip)?),
            (Some(at), None, None) => doc.video_clip_at(at),
        };
        let Some(found) = found else {
            return Ok(to_value(&FrameAt::default()));
        };
        let movie = self
            .clip_movie(found.id)
            .ok_or_else(|| failed("the clip's movie can't be read".to_owned()))?;
        let movie_time = match (at, asked) {
            (Some(at), _) => {
                found.movie_time_at(at, &doc.tempo_map(), movie.duration().seconds())
            }
            (None, time) => time.filter(|seconds| seconds.is_finite() && *seconds >= 0.0),
        };
        // Nanoseconds, so a time between two frames' starts, which is what a
        // warp map gives, lands on the frame it is in.
        let frame = movie_time
            .and_then(|seconds| {
                movie.frame_at(Rational::new((seconds * 1e9).round() as i128, 1_000_000_000))
            })
            .and_then(|frame| Some((frame, movie.video()?.frame_start(frame)?)));
        Ok(to_value(&FrameAt {
            clip: Some(found.id),
            movie_time,
            frame: frame.map(|(frame, _)| frame as u32),
            frame_time: frame.map(|(_, start)| start.seconds()),
        }))
    }
}
