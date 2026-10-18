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
    /// File frames per song frame over the run.
    pub speed: f64,
    /// Whether the run reads the file frame for frame: the speed is exactly 1
    /// and the pitch is unchanged, so each song frame plays the file frame
    /// nearest its position and the clip is neutral (R4).
    pub neutral: bool,
    /// The HiQ switch (R12).
    pub high_quality: bool,
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
            speed: 2.0,
            neutral: false,
            high_quality: false,
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
                speed: 0.5,
                neutral: false,
                high_quality,
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
            speed: 1.0,
            neutral: true,
            high_quality: true,
        };
        assert_eq!(RePitch.reach(&request), (0, 0));
        let mut out = vec![0.0; 10];
        RePitch.render(&request, &window, &mut out, 10);
        assert_eq!(out, (20..30).map(|i| i as f32).collect::<Vec<_>>());
    }
}
