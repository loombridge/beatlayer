//! Rendering a set in real time, through hardware (WP-5.35; the external
//! devices spec, R24, R25).
//!
//! While the engine plays a range on the running audio device, the audio
//! thread copies each rendered output into a lock-free ring, at the point an
//! offline render takes it from; a writer thread drains the rings into the
//! files. Nothing on the audio thread allocates, locks, or blocks.
//!
//! The plan names the range in song frames and the outputs to capture: the
//! Main output, or a track's post-fader signal.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use bl_core::time::Frames;
use bl_model::TrackId;
use rtrb::Producer;

/// One output a real-time render captures: the track whose post-fader
/// signal it is, or the Main output.
pub struct RenderOutput {
    /// The track, or `None` for the Main output.
    pub track: Option<TrackId>,
    /// The samples, for the writer thread to drain.
    pub producer: Producer<f32>,
    /// Frames the ring was too full to take, for the shell to report: the
    /// render never loses a frame silently (R25).
    pub dropped: Arc<AtomicU64>,
    /// The frames the ring has taken so far, for the shell to follow the
    /// render's progress with.
    pub captured: Arc<AtomicU64>,
}

/// What a real-time render captures: the range it plays and the outputs it
/// takes.
pub struct RenderPlan {
    /// The song frame the range starts at.
    pub start: Frames,
    /// How many frames the range is.
    pub frames: u64,
    /// The outputs, in the order they are captured.
    pub outputs: Vec<RenderOutput>,
}

impl RenderPlan {
    /// The song frame the range ends at, the first one past it.
    #[must_use]
    pub fn end(&self) -> Frames {
        self.start + Frames(self.frames as i64)
    }
}

impl std::fmt::Debug for RenderPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderPlan")
            .field("start", &self.start)
            .field("frames", &self.frames)
            .field("outputs", &self.outputs.len())
            .finish_non_exhaustive()
    }
}

/// Copies the part of the block the range covers into each output's ring:
/// the Main output, or the track's post-fader signal. Returns whether every
/// output took the whole part; a ring too full for it counts the frames it
/// could not take, and the render is spoiled (R25).
pub(crate) fn capture(
    plan: &mut RenderPlan,
    program: &crate::program::EngineProgram,
    position: Frames,
    frames: usize,
    channels: usize,
) -> bool {
    // The part covers the song's frames from `position` on.
    let from = position.max(plan.start);
    let to = (position + Frames(frames as i64)).min(plan.end());
    if to <= from {
        return true;
    }
    let skip = usize::try_from((from - position).get()).unwrap_or(0);
    let take = usize::try_from((to - from).get()).unwrap_or(0);
    let mut fits = true;
    for output in &mut plan.outputs {
        if output.producer.slots() < take * channels {
            output.dropped.fetch_add(take as u64, Relaxed);
            fits = false;
        }
    }
    if !fits {
        return false;
    }
    for output in &mut plan.outputs {
        let task = match output.track {
            Some(track) => program.task_of(track),
            None => Some(program.output),
        };
        let Some(task) = task else { continue };
        // Spec: external-devices R29. Nothing here allocates: each channel's
        // samples are read one at a time, frame by frame.
        for frame in skip..skip + take {
            for channel in 0..channels {
                let lane = program.task_output(task, channel, frames);
                let _ = output.producer.push(lane[frame]);
            }
        }
        output.captured.fetch_add(take as u64, Relaxed);
    }
    true
}
