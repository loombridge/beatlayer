//! The warp-mode interface (the warp modes spec, R1 to R8) and Re-Pitch
//! (R9 and R10).
//!
//! The clip player finds, from the warp map under the tempo map, the file
//! position each song frame plays ([`WarpTable`](crate::WarpTable)); a mode
//! turns those positions into audio. It is given the positions of a run of
//! frames and a [`Window`] of the file around them, which the player has read
//! contiguously and padded with silence outside the file, and writes the
//! frames. Every mode's output depends only on the positions and the file,
//! never on how the player split its blocks (R6). A mode that keeps state
//! (grains, segments, a stretcher) makes it with the player's clip list, off
//! the audio thread, and `render` allocates and locks nothing (R5).

use crate::beats::Plan;
use crate::interp::Interpolation;

/// A planar block of frames: channel `c` is `data[c * stride..]`.
#[derive(Clone, Copy, Debug)]
pub struct Window<'a> {
    /// The frames, channel after channel, `stride` apart.
    pub data: &'a [f32],
    /// The distance between channels' starts, in frames.
    pub stride: usize,
    /// How many channels.
    pub channels: usize,
    /// How many frames each channel holds.
    pub frames: usize,
    /// The file frame the window's first frame is. Negative where the window
    /// reaches before the file starts.
    pub first: i64,
}

impl Window<'_> {
    /// Channel `c`'s frames.
    #[must_use]
    pub fn channel(&self, c: usize) -> &[f32] {
        &self.data[c * self.stride..c * self.stride + self.frames]
    }
}

/// What a mode renders: the file position of each song frame of a run, and
/// how the clip is read.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    /// The file position, in frames and perhaps fractional, each output
    /// frame plays. Never NaN; in increasing order.
    pub positions: &'a [f64],
    /// The song frame the run's first output frame plays on.
    pub first: i64,
    /// File frames per song frame over the run.
    pub speed: f64,
    /// Whether the run reads the file frame for frame: the speed is exactly 1
    /// and the pitch is unchanged, so each song frame plays the file frame
    /// nearest its position and the clip is neutral (R4).
    pub neutral: bool,
    /// The HiQ switch (R12).
    pub high_quality: bool,
    /// A Beats clip's segments and envelope (R14 to R17); empty in every
    /// other mode.
    pub beats: &'a Plan,
    /// The pitch ratio (R20). 1.0 where untransposed.
    pub pitch: f64,
}

/// The frames of the run a window must hold besides what the mode reaches
/// for: a block, with room.
const RUN_FRAMES: usize = 8_192;

/// The most file frames any mode reads around a run at `rate` (R18, R19):
/// Texture's largest Grain Size reaches each way, through the widest
/// interpolator, with the run's own frames. The player reads a clip through a
/// window this wide, and a clip's source keeps this much ready. A mode whose
/// reach can exceed it must widen it here.
#[must_use]
pub fn window_frames(rate: f64) -> usize {
    let grain = (crate::texture::MAX_TEXTURE_GRAIN_MS / 1000.0 * rate).ceil() as usize;
    let interp = Interpolation::of(true).reach(f64::INFINITY).1;
    RUN_FRAMES + 2 * (2 * grain + interp + 64)
}

/// A warp mode.
pub trait WarpMode {
    /// How many file frames before and after the frame a position falls in
    /// the mode reads, for a run at `speed`: the player makes them ready in
    /// the [`Window`].
    fn reach(&self, request: &Request<'_>) -> (usize, usize);

    /// Writes the run's frames to `out` (planar, as the window is): frame
    /// `i` of every channel plays `request.positions[i]`.
    fn render(
        &mut self,
        request: &Request<'_>,
        window: &Window<'_>,
        out: &mut [f32],
        stride: usize,
    );

    /// Starts afresh, as at a locate, a launch or a jump (R7).
    fn reset(&mut self) {}
}

/// Re-Pitch: reads the file at the clip's speed and changes nothing else, so
/// pitch and tempo move together (R9). It has no state and no controls.
#[derive(Clone, Copy, Debug, Default)]
pub struct RePitch;

impl WarpMode for RePitch {
    fn reach(&self, request: &Request<'_>) -> (usize, usize) {
        if request.neutral {
            (0, 0)
        } else {
            Interpolation::of(request.high_quality).reach(request.speed)
        }
    }

    // Spec: warp-modes R9
    fn render(
        &mut self,
        request: &Request<'_>,
        window: &Window<'_>,
        out: &mut [f32],
        stride: usize,
    ) {
        read(request, window, out, stride);
    }
}

/// Beats (R14 to R17): the clip played as segments. The positions come from
/// the player's table, which [`Plan::stretches`](crate::beats::Plan::stretches)
/// gives, and the mode shapes each segment's level by the transient envelope
/// (R16). It keeps no state: a frame's output depends only on the song.
#[derive(Clone, Copy, Debug, Default)]
pub struct Beats;

impl WarpMode for Beats {
    fn reach(&self, request: &Request<'_>) -> (usize, usize) {
        if request.neutral {
            (0, 0)
        } else {
            Interpolation::of(request.high_quality).reach(request.speed)
        }
    }

    // Spec: warp-modes R14 to R17
    fn render(
        &mut self,
        request: &Request<'_>,
        window: &Window<'_>,
        out: &mut [f32],
        stride: usize,
    ) {
        read(request, window, out, stride);
        if request.beats.is_empty() {
            return;
        }
        // The envelope (R16). At 100 every level is 1, so the run is left
        // untouched, to the bit.
        for c in 0..window.channels {
            let channel = &mut out[c * stride..c * stride + request.positions.len()];
            for (i, frame) in channel.iter_mut().enumerate() {
                let level = request.beats.level(request.first + i as i64);
                if level != 1.0 {
                    *frame = (*frame as f64 * level) as f32;
                }
            }
        }
    }
}

/// Reads the run's positions from the window, into `out`: the nearest file
/// frame where the run is neutral, the interpolator's value otherwise.
pub(crate) fn read(request: &Request<'_>, window: &Window<'_>, out: &mut [f32], stride: usize) {
    let interpolation = Interpolation::of(request.high_quality);
    for c in 0..window.channels {
        let source = window.channel(c);
        let out = &mut out[c * stride..c * stride + request.positions.len()];
        for (frame, &position) in out.iter_mut().zip(request.positions) {
            *frame = if request.neutral {
                // Spec: warp-modes R4. The nearest frame, untouched.
                source[(position.round() as i64 - window.first) as usize]
            } else {
                let floor = position.floor();
                let at = (floor as i64 - window.first) as usize;
                interpolation.sample(source, at, position - floor, request.speed)
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(frames: usize) -> Vec<f32> {
        (0..frames).map(|i| i as f32).collect()
    }

    // Spec: warp-modes R9
    #[test]
    fn re_pitch_reads_the_file_at_the_clips_speed() {
        let data = ramp(200);
        let window = Window {
            data: &data,
            stride: 200,
            channels: 1,
            frames: 200,
            first: 10,
        };
        // Twice as fast: every other frame, at whole positions.
        let positions: Vec<f64> = (0..20).map(|i| 50.0 + 2.0 * f64::from(i)).collect();
        let request = Request {
            positions: &positions,
            first: 0,
            speed: 2.0,
            neutral: false,
            high_quality: false,
            beats: &Plan::default(),
            pitch: 1.0,
        };
        let mut out = vec![0.0; 20];
        RePitch.render(&request, &window, &mut out, 20);
        let want: Vec<f32> = positions.iter().map(|&p| (p - 10.0) as f32).collect();
        assert_eq!(out, want);
        // Between frames, a ramp is found exactly by either interpolator.
        let positions: Vec<f64> = (0..20).map(|i| 50.25 + 0.5 * f64::from(i)).collect();
        for high_quality in [false, true] {
            let request = Request {
                positions: &positions,
                first: 0,
                speed: 0.5,
                neutral: false,
                high_quality,
                beats: &Plan::default(),
                pitch: 1.0,
            };
            RePitch.render(&request, &window, &mut out, 20);
            for (got, p) in out.iter().zip(&positions) {
                assert!((f64::from(*got) - (p - 10.0)).abs() < 1e-3, "{got} {p}");
            }
        }
    }

    // Spec: warp-modes R4
    #[test]
    fn a_neutral_run_plays_the_nearest_frames_untouched() {
        let data = ramp(100);
        let window = Window {
            data: &data,
            stride: 100,
            channels: 1,
            frames: 100,
            first: 0,
        };
        let positions: Vec<f64> = (0..10).map(|i| 20.4 + f64::from(i)).collect();
        let request = Request {
            positions: &positions,
            first: 0,
            speed: 1.0,
            neutral: true,
            high_quality: true,
            beats: &Plan::default(),
            pitch: 1.0,
        };
        assert_eq!(RePitch.reach(&request), (0, 0));
        let mut out = vec![0.0; 10];
        RePitch.render(&request, &window, &mut out, 10);
        assert_eq!(out, (20..30).map(|i| i as f32).collect::<Vec<_>>());
    }
}
