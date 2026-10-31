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

use crate::beats::{Mix, Plan};
use crate::interp::{Interpolation, Taps};

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
    /// A frame count that runs on by one per output frame while the clip
    /// plays on without a jump: the song frame the run's first output frame
    /// plays on, unless the song follows an outside tempo (the transport
    /// spec, R24), when the song frames run on faster or slower than the
    /// frames do and [`song`](Self::song) holds them.
    pub first: i64,
    /// File frames per output frame over the run.
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
    /// The Transient Envelope this run plays (R16), 0 to 100: the plan's own
    /// unless the clip's setting is moving.
    pub transient_envelope: f64,
    /// The pitch ratio (R20). 1.0 where untransposed.
    pub pitch: f64,
    /// The clip whose frames these are, by its ID: a mode that keeps state
    /// uses it to start afresh when another clip takes its instance over,
    /// since one instance may serve several clips that never sound at once
    /// (R5, R7).
    pub clip: u64,
    /// Song frames per output frame: 1 unless the song follows an outside
    /// tempo.
    pub step: f64,
    /// The song frame the run's first output frame plays on, unrounded; NaN
    /// where `first` is it.
    pub song: f64,
}

impl Request<'_> {
    /// The song frame output frame `i` of the run plays on.
    #[must_use]
    pub fn song_frame(&self, i: usize) -> i64 {
        if self.song.is_nan() {
            self.first + i as i64
        } else {
            (self.song + i as f64 * self.step).round() as i64
        }
    }
}

/// The frames of the run a window must hold besides what the mode reaches
/// for: a block, with room.
const RUN_FRAMES: usize = 8_192;

/// The most file frames a grain mode (Tones, Texture) reads around a run at
/// `rate` (R18, R19): Texture's largest Grain Size and Flux reach each way,
/// through the widest interpolator, with the run's own frames. The player
/// reads such a clip through a window this wide, and its source keeps this
/// much ready.
#[must_use]
pub fn grain_window_frames(rate: f64) -> usize {
    RUN_FRAMES + 2 * crate::texture::max_reach(rate)
}

/// The most file frames a frequency-domain mode (Complex, Complex Pro) reads
/// around a run at `rate` (R21, R24): the stretcher's own read-ahead either
/// side, with the run's own frames. Kept apart from
/// [`grain_window_frames`] so a grain mode's window pays only for its own
/// reach.
#[must_use]
pub fn stretch_window_frames(rate: f64) -> usize {
    RUN_FRAMES + 2 * crate::complex::max_reach(rate)
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
/// the player's table, which [`Plan::passes`](crate::beats::Plan::passes)
/// gives, and the mode shapes each frame's level by the plan's joins and the
/// transient envelope (R15, R16). It keeps no state: a frame's output depends
/// only on the song.
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
        self.shape(request, window.channels, out, stride, |mix| mix.level);
    }
}

impl Beats {
    /// Renders the pass fading out under each frame of the run: the request's
    /// positions are where each frame's outgoing pass reads (any position
    /// where none does), and the frames with none are silent (R15).
    pub fn render_outgoing(
        self,
        request: &Request<'_>,
        window: &Window<'_>,
        out: &mut [f32],
        stride: usize,
    ) {
        read(request, window, out, stride);
        self.shape(request, window.channels, out, stride, |mix| {
            mix.outgoing.map_or(0.0, |(_, level)| level)
        });
    }

    /// Scales each frame of the run by the level `level` picks from what the
    /// plan says sounds there. At 100 every level is 1, so the run is left
    /// untouched, to the bit.
    fn shape(
        self,
        request: &Request<'_>,
        channels: usize,
        out: &mut [f32],
        stride: usize,
        level: impl Fn(Mix) -> f64,
    ) {
        if request.beats.is_empty() {
            return;
        }
        for c in 0..channels {
            let channel = &mut out[c * stride..c * stride + request.positions.len()];
            for (i, frame) in channel.iter_mut().enumerate() {
                let mix = request
                    .beats
                    .mix(request.song_frame(i), request.transient_envelope);
                let level = level(mix);
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
    if request.neutral {
        for (i, &position) in request.positions.iter().enumerate() {
            // Spec: warp-modes R4. The nearest frame, untouched.
            let at = (position.round() as i64 - window.first) as usize;
            for c in 0..window.channels {
                out[c * stride + i] = window.channel(c)[at];
            }
        }
        return;
    }
    let interpolation = Interpolation::of(request.high_quality);
    let mut taps = Taps::new();
    for (i, &position) in request.positions.iter().enumerate() {
        let floor = position.floor();
        let at = (floor as i64 - window.first) as usize;
        // The weights depend on the position alone: work them out once for
        // every channel.
        interpolation.weights(at, position - floor, request.speed, &mut taps);
        for c in 0..window.channels {
            out[c * stride + i] = taps.dot(window.channel(c));
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
        let data = ramp(400);
        let window = Window {
            data: &data,
            stride: 400,
            channels: 1,
            frames: 400,
            first: 10,
        };
        // Twice as fast: every other frame, at whole positions.
        let positions: Vec<f64> = (0..20).map(|i| 100.0 + 2.0 * f64::from(i)).collect();
        let request = Request {
            positions: &positions,
            first: 0,
            speed: 2.0,
            neutral: false,
            high_quality: false,
            beats: &Plan::default(),
            transient_envelope: 100.0,
            pitch: 1.0,
            clip: 1,
            step: 1.0,
            song: f64::NAN,
        };
        let mut out = vec![0.0; 20];
        RePitch.render(&request, &window, &mut out, 20);
        let want: Vec<f32> = positions.iter().map(|&p| (p - 10.0) as f32).collect();
        // A read at twice the speed goes through the interpolator's filter
        // even on whole frames (R12), which leaves a ramp's frames within a
        // hundredth.
        for (got, want) in out.iter().zip(&want) {
            assert!((got - want).abs() < 0.01, "{out:?} against {want:?}");
        }
        // Between frames, a ramp is found exactly by either interpolator.
        let positions: Vec<f64> = (0..20).map(|i| 100.25 + 0.5 * f64::from(i)).collect();
        for high_quality in [false, true] {
            let request = Request {
                positions: &positions,
                first: 0,
                speed: 0.5,
                neutral: false,
                high_quality,
                beats: &Plan::default(),
                transient_envelope: 100.0,
                pitch: 1.0,
                clip: 1,
                step: 1.0,
                song: f64::NAN,
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
            transient_envelope: 100.0,
            pitch: 1.0,
            clip: 1,
            step: 1.0,
            song: f64::NAN,
        };
        assert_eq!(RePitch.reach(&request), (0, 0));
        let mut out = vec![0.0; 10];
        RePitch.render(&request, &window, &mut out, 10);
        assert_eq!(out, (20..30).map(|i| i as f32).collect::<Vec<_>>());
    }

    /// Two different channels: a sine, and a ramp with noise from a fixed seed.
    fn stereo(frames: usize) -> Vec<f32> {
        let mut seed = 12_345_u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
        };
        let left = (0..frames).map(|i| (i as f32 * 0.0123 * std::f32::consts::TAU).sin());
        let right: Vec<f32> = (0..frames)
            .map(|i| i as f32 / frames as f32 + 0.1 * noise())
            .collect();
        left.chain(right).collect()
    }

    fn run<'a>(
        positions: &'a [f64],
        plan: &'a Plan,
        speed: f64,
        high_quality: bool,
    ) -> Request<'a> {
        Request {
            positions,
            first: 0,
            speed,
            neutral: false,
            high_quality,
            beats: plan,
            transient_envelope: 100.0,
            pitch: 1.0,
            clip: 1,
            step: 1.0,
            song: f64::NAN,
        }
    }

    // Spec: warp-modes R12. Guards the weights being worked out once for every
    // channel: each channel reads as if it had been read alone.
    #[test]
    fn read_gives_each_channel_what_the_interpolator_gives_it_alone() {
        let data = stereo(4_000);
        let window = Window {
            data: &data,
            stride: 4_000,
            channels: 2,
            frames: 4_000,
            first: 0,
        };
        for speed in [0.5, 1.5, 4.0] {
            let positions: Vec<f64> = (0..200).map(|i| 1_000.3 + speed * f64::from(i)).collect();
            for high_quality in [false, true] {
                let mut out = vec![0.0; 2 * 200];
                read(
                    &run(&positions, &Plan::default(), speed, high_quality),
                    &window,
                    &mut out,
                    200,
                );
                let interpolation = Interpolation::of(high_quality);
                for c in 0..2 {
                    for (i, &position) in positions.iter().enumerate() {
                        let at = position.floor();
                        let want = interpolation.sample(
                            window.channel(c),
                            at as usize,
                            position - at,
                            speed,
                        );
                        assert_eq!(out[c * 200 + i].to_bits(), want.to_bits());
                    }
                }
            }
        }
    }

    // Spec: warp-modes R82
    #[test]
    fn read_computes_kernel_weights_once_per_frame() {
        let data = stereo(4_000);
        let window = Window {
            data: &data,
            stride: 4_000,
            channels: 2,
            frames: 4_000,
            first: 0,
        };
        let positions: Vec<f64> = (0..256).map(|i| 1_000.5 + 1.5 * f64::from(i)).collect();
        let mut out = vec![0.0; 2 * 256];
        crate::interp::KERNEL_EVALS.with(|evals| evals.set(0));
        read(
            &run(&positions, &Plan::default(), 1.5, true),
            &window,
            &mut out,
            256,
        );
        assert_eq!(crate::interp::KERNEL_EVALS.with(std::cell::Cell::get), 256);
    }
}
