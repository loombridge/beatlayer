//! Video clips as an agent reads them (WP-9.25): `video.frame_at`, and the
//! Video Window (WP-9.26): `view.toggle_video_window`, `video.window_state`
//! and `video.set_window`. The video spec, R13 and R16 to R23, holds the
//! behaviour; a video clip itself is an audio clip whose saved data has
//! `video` set, which `clip` reads back.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{ClipId, WindowFrame};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `video.frame_at`: the frame a video clip plays at a song position.
pub const FRAME_AT: &str = "video.frame_at";

/// `view.toggle_video_window`: View ▸ Video Window (M-VIEW-029, S-VIEW-009).
pub const TOGGLE_WINDOW: &str = "view.toggle_video_window";

/// `video.window_state`: whether the Video Window shows, where, and the
/// frame it shows.
pub const WINDOW_STATE: &str = "video.window_state";

/// `video.set_window`: shows, hides and places the Video Window.
pub const SET_WINDOW: &str = "video.set_window";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[FRAME_AT, TOGGLE_WINDOW, WINDOW_STATE, SET_WINDOW];

/// Whether `id` is one of the video commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `video.frame_at`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameAtArgs {
    /// The song position, in ticks. Give this or `movie_time`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Ticks>,
    /// A time in the movie, in seconds, as a warp marker's `time` is: the
    /// frame there, whichever song position plays it. Give this or `at`; it
    /// needs `clip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movie_time: Option<f64>,
    /// The video clip. Left out with `at`, the one whose picture shows
    /// there: the playing video clip on the bottom-most track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
}

/// What `video.frame_at` returns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FrameAt {
    /// The video clip, or `null` where none plays: the picture is black.
    pub clip: Option<ClipId>,
    /// The time in the movie, in seconds, the clip plays at `at`, through
    /// its warp map, or the `movie_time` asked for; `null` outside the clip
    /// or before the movie.
    pub movie_time: Option<f64>,
    /// The index of the movie's frame shown then, counting from 0 in the
    /// order they show; `null` where the movie has no picture: black.
    pub frame: Option<u32>,
    /// When that frame starts to show on the movie's timeline, in seconds.
    pub frame_time: Option<f64>,
}

/// Arguments of `video.set_window`. Left out, a field changes nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetWindowArgs {
    /// Show (`true`) or hide the window. Showing needs a video clip in the
    /// Arrangement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<bool>,
    /// Where the window is and how big: the left and top of its frame and
    /// the size of its picture, in points. Kept in the preferences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<WindowFrame>,
    /// Put the window in full screen on the screen it is on, or bring it
    /// back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_screen: Option<bool>,
    /// Set the window's size to the size of the picture of the movie it
    /// shows, one point to a pixel, keeping its place.
    #[serde(default)]
    pub original_size: bool,
}

/// What `video.window_state` and `video.set_window` return.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WindowState {
    /// Whether the window shows.
    pub shown: bool,
    /// Whether the set has a video clip in the Arrangement, which the window
    /// needs to show.
    pub enabled: bool,
    /// Where the window is and how big, once it has been placed; the same in
    /// every set.
    pub frame: Option<WindowFrame>,
    /// Whether the window is in full screen.
    pub full_screen: bool,
    /// The frame the window shows now: `clip` is `null` where it is black
    /// for want of a video clip at the playhead.
    pub showing: FrameAt,
}

/// Adds the video commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if its ID is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(FRAME_AT, "Video Frame At")
            .description(
                "Gives the frame a video clip plays at a song position `at`, in ticks: the \
                 `movie_time` in seconds that the clip's warp map gives for it, the `frame` \
                 index shown then (the last frame at or before the time) and when it starts, \
                 `frame_time`. Without `clip` it asks the video clip whose picture shows at \
                 `at`, the playing one on the bottom-most track; with none playing every field \
                 but `clip` is `null` and the picture is black. Moving a warp marker changes \
                 which frame plays when. With `movie_time` and a `clip` instead of `at` it \
                 gives the frame at that time in the movie, as for a warp marker being dragged. \
                 Fails if the clip isn't a video clip or its movie can't be read. Changes \
                 nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<FrameAtArgs>()
            .result::<FrameAt>(),
    )?;
    registry.register(
        CommandSpec::new(TOGGLE_WINDOW, "Video Window")
            .description(
                "Shows the Video Window, or hides it while it shows, as View ▸ Video \
                 Window does. The window floats above the main window and shows the frame at \
                 the playhead, or the frame under a warp marker being dragged. Fails while the \
                 set has no video clip in the Arrangement and the window is hidden. Returns \
                 what `video.window_state` does.",
            )
            .permission(Permission::Read)
            .context(Global)
            .binding("S-VIEW-009")
            .args::<NoArgs>()
            .result::<WindowState>(),
    )?;
    registry.register(
        CommandSpec::new(WINDOW_STATE, "Video Window State")
            .description(
                "Reads the Video Window: whether it shows, whether the set has a video clip \
                 to show, where it is and how big (kept in the preferences, the same in every \
                 set), whether it is in full screen, and the frame it shows now, as \
                 `video.frame_at` gives it. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<WindowState>(),
    )?;
    registry.register(
        CommandSpec::new(SET_WINDOW, "Set Video Window")
            .description(
                "Shows or hides the Video Window (`shown`), places and sizes it (`frame`), puts \
                 it in full screen or brings it back (`full_screen`, on the screen it is on) \
                 and sets its size to the movie's own (`original_size`). A view command: it \
                 changes no set and is no undo step. Showing needs a video clip in the \
                 Arrangement. Returns what `video.window_state` does.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<SetWindowArgs>()
            .result::<WindowState>(),
    )
}
