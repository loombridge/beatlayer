//! Video clips as an agent reads them (WP-9.25): `video.frame_at`. The video
//! spec, R13, holds the behaviour; a video clip itself is an audio clip
//! whose saved data has `video` set, which `clip` reads back.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::ClipId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `video.frame_at`: the frame a video clip plays at a song position.
pub const FRAME_AT: &str = "video.frame_at";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[FRAME_AT];

/// Whether `id` is one of the video commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `video.frame_at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameAtArgs {
    /// The song position, in ticks.
    pub at: Ticks,
    /// The video clip. Left out, the one whose picture shows at `at`: the
    /// playing video clip on the bottom-most track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
}

/// What `video.frame_at` returns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FrameAt {
    /// The video clip, or `null` where none plays: the picture is black.
    pub clip: Option<ClipId>,
    /// The time in the movie, in seconds, the clip plays at `at`, through
    /// its warp map; `null` outside the clip or before the movie.
    pub movie_time: Option<f64>,
    /// The index of the movie's frame shown then, counting from 0 in the
    /// order they show; `null` where the movie has no picture: black.
    pub frame: Option<u32>,
    /// When that frame starts to show on the movie's timeline, in seconds.
    pub frame_time: Option<f64>,
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
                 which frame plays when. Fails if the clip isn't a video clip or its movie \
                 can't be read. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<FrameAtArgs>()
            .result::<FrameAt>(),
    )
}
