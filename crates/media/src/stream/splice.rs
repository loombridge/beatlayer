//! The fades that hide a jump: a crossfade from the audio that was playing
//! to the new position, and a fade-in where audio resumes after silence.

use core::ops::Range;
use std::f64::consts::PI;

use super::planar::PlanarMut;

/// The default length of a jump's fades: 4 ms, the length of the short fade
/// Live puts on clip edges to stop them clicking (the disk streaming spec).
pub const DEFAULT_FADE: std::time::Duration = std::time::Duration::from_millis(4);

/// The fade length in frames at a sample rate: [`DEFAULT_FADE`], at least
/// one frame.
#[must_use]
pub fn fade_frames(sample_rate: u32) -> usize {
    let frames = u128::from(sample_rate) * DEFAULT_FADE.as_micros() / 1_000_000;
    (frames as usize).max(1)
}

/// A jump's fades, for one source. Everything is allocated when it's made;
/// using it allocates nothing.
#[derive(Debug)]
pub(crate) struct Splice {
    channels: usize,
    /// The gain of incoming audio at each frame of a fade, rising from near
    /// 0 to near 1 along a raised cosine, whose slope is zero at both ends.
    /// Outgoing audio takes one minus it, so the two gains sum to one and a
    /// crossfade between correlated signals neither dips nor bulges.
    ramp: Box<[f32]>,
    /// The audio fading out, planar, `channels` × the fade's length.
    outgoing: Box<[f32]>,
    /// Where the next jump's outgoing audio is captured, while the current
    /// one may still be fading out of `outgoing`.
    pub(crate) capture: Box<[f32]>,
    /// Frames of the fade-out already played; the fade's length when nothing
    /// is fading out.
    out_done: usize,
    /// Frames of the fade-in already played, while one runs.
    in_done: Option<usize>,
}

impl Splice {
    pub(crate) fn new(channels: usize, len: usize) -> Self {
        let len = len.max(1);
        let ramp = (0..len)
            .map(|i| (0.5 - 0.5 * (PI * (i as f64 + 0.5) / len as f64).cos()) as f32)
            .collect();
        Self {
            channels,
            ramp,
            outgoing: vec![0.0; channels * len].into_boxed_slice(),
            capture: vec![0.0; channels * len].into_boxed_slice(),
            out_done: len,
            in_done: None,
        }
    }

    /// The fade's length in frames.
    pub(crate) fn len(&self) -> usize {
        self.ramp.len()
    }

    /// Whether a fade is running.
    pub(crate) fn fading(&self) -> bool {
        self.in_done.is_some() || self.out_done < self.len()
    }

    /// Stops any fade: what plays next starts at full level, with nothing
    /// fading out under it.
    pub(crate) fn reset(&mut self) {
        self.out_done = self.len();
        self.in_done = None;
    }

    /// Starts a crossfade: what was captured in `capture` fades out, and the
    /// audio from the new position fades in.
    pub(crate) fn begin_crossfade(&mut self) {
        core::mem::swap(&mut self.outgoing, &mut self.capture);
        self.out_done = 0;
        self.in_done = Some(0);
    }

    /// Audio stopped for want of data: whatever comes next fades in.
    pub(crate) fn restart_fade_in(&mut self) {
        self.in_done = Some(0);
    }

    /// Applies the fade-in, if one runs, to `frames` of the first `channels`
    /// channels of `out`, which hold the source's audio.
    pub(crate) fn fade_in(
        &mut self,
        out: &mut PlanarMut<'_>,
        channels: usize,
        frames: Range<usize>,
    ) {
        let Some(done) = self.in_done else {
            return;
        };
        let len = self.len();
        let count = frames.len().min(len - done);
        let ramp = &self.ramp[done..done + count];
        for c in 0..channels {
            let span = out.span_mut(c, frames.start..frames.start + count);
            for (sample, gain) in span.iter_mut().zip(ramp) {
                *sample *= gain;
            }
        }
        let done = done + count;
        self.in_done = (done < len).then_some(done);
    }

    /// Adds what's left of the fade-out to the first `channels` channels of
    /// `out`, from its first frame.
    pub(crate) fn fade_out(&mut self, out: &mut PlanarMut<'_>, channels: usize) {
        let len = self.len();
        let done = self.out_done;
        let count = out.frames().min(len - done);
        if count == 0 {
            return;
        }
        for c in 0..channels.min(self.channels) {
            let old = &self.outgoing[c * len + done..c * len + done + count];
            let ramp = &self.ramp[done..done + count];
            let span = out.span_mut(c, 0..count);
            for ((sample, old), gain) in span.iter_mut().zip(old).zip(ramp) {
                *sample += old * (1.0 - gain);
            }
        }
        self.out_done = done + count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_ms_at_common_rates() {
        assert_eq!(fade_frames(48_000), 192);
        assert_eq!(fade_frames(44_100), 176);
        assert_eq!(fade_frames(96_000), 384);
        assert_eq!(fade_frames(1), 1);
    }

    #[test]
    fn the_gains_rise_smoothly_and_sum_to_one() {
        let splice = Splice::new(1, 192);
        let ramp = &splice.ramp;
        assert!(ramp[0] > 0.0 && ramp[0] < 1e-4);
        assert!(ramp[191] < 1.0 && ramp[191] > 1.0 - 1e-4);
        assert!(ramp.windows(2).all(|w| w[1] > w[0]));
        // Symmetric: the fade-out mirrors the fade-in, so they sum to one.
        for i in 0..192 {
            assert!((ramp[i] + ramp[191 - i] - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn a_crossfade_between_equal_signals_is_flat() {
        let len = 16;
        let mut splice = Splice::new(2, len);
        splice.capture.fill(0.5);
        splice.begin_crossfade();
        let mut data = vec![0.5f32; 2 * 40];
        let mut out = PlanarMut::new(&mut data, 2, 40, 40);
        splice.fade_in(&mut out, 2, 0..40);
        splice.fade_out(&mut out, 2);
        for c in 0..2 {
            assert!(out.channel(c).iter().all(|s| (s - 0.5).abs() < 1e-6));
        }
        assert!(!splice.fading());
    }
}
