//! The Signalsmith Stretch library behind Beatlayer's own C wrapper (the warp
//! modes spec, R21): the frequency-domain stretcher the Complex and Complex
//! Pro modes are built on.
//!
//! The library is C++ (MIT, vendored under `vendor/signalsmith-stretch`, and
//! compiled by `build.rs`); [`Stretcher`] is the only thing in Beatlayer that
//! touches it. A `Stretcher` is made with its clip list, off the audio thread
//! (R5), is sized for a fixed number of channels, and after that
//! [`Stretcher::process`] allocates nothing: the pointer arrays the library
//! indexes are made once, by [`Stretcher::new`].

use std::ffi::c_int;

/// The C++ object, opaque here.
#[repr(C)]
struct Raw {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn bl_stretch_new(
        channels: c_int,
        block_samples: c_int,
        interval_samples: c_int,
        split_computation: bool,
    ) -> *mut Raw;
    fn bl_stretch_free(stretch: *mut Raw);
    fn bl_stretch_block_samples(stretch: *const Raw) -> c_int;
    fn bl_stretch_interval_samples(stretch: *const Raw) -> c_int;
    fn bl_stretch_input_latency(stretch: *const Raw) -> c_int;
    fn bl_stretch_output_latency(stretch: *const Raw) -> c_int;
    fn bl_stretch_reset(stretch: *mut Raw);
    fn bl_stretch_set_transpose_factor(stretch: *mut Raw, multiplier: f64, tonality_limit: f64);
    fn bl_stretch_set_formant_factor(stretch: *mut Raw, multiplier: f64, compensate_pitch: bool);
    fn bl_stretch_set_formant_base(stretch: *mut Raw, base_freq: f64);
    fn bl_stretch_seek(
        stretch: *mut Raw,
        inputs: *const *const f32,
        input_samples: c_int,
        playback_rate: f64,
    );
    fn bl_stretch_process(
        stretch: *mut Raw,
        inputs: *const *const f32,
        input_samples: c_int,
        outputs: *const *mut f32,
        output_samples: c_int,
    );
}

/// The tonality limit the pitch shift uses: the library's own default, half
/// the sample rate (its `README` passes `8000/sampleRate` only to keep more
/// of the timbre; R21 gives no reason to deviate).
const TONALITY_LIMIT: f64 = 0.5;

/// A Signalsmith Stretch instance: `channels` interleaved-free channels at
/// `sample_rate` Hz.
pub struct Stretcher {
    raw: *mut Raw,
    channels: usize,
    /// The input and output pointer arrays the library reads, made once so a
    /// call to [`process`](Self::process) allocates nothing (R5).
    in_ptrs: Vec<*const f32>,
    out_ptrs: Vec<*mut f32>,
}

// SAFETY: the C++ object holds no thread-affine state. The engine makes a
// `Stretcher` with a track's clip list and uses it on that track's player,
// never on two threads at once.
unsafe impl Send for Stretcher {}

impl Stretcher {
    /// A stretcher for `channels` channels, in spectral blocks of
    /// `block_samples` at `interval_samples` apart.
    #[must_use]
    pub fn new(
        channels: usize,
        block_samples: usize,
        interval_samples: usize,
        split_computation: bool,
    ) -> Self {
        // SAFETY: the counts come from the caller and the wrapper checks its
        // own configuration, returning null if the library refuses it.
        let raw = unsafe {
            bl_stretch_new(
                channels as c_int,
                block_samples as c_int,
                interval_samples as c_int,
                split_computation,
            )
        };
        assert!(!raw.is_null(), "the stretcher couldn't be configured");
        Self {
            raw,
            channels,
            in_ptrs: vec![std::ptr::null(); channels],
            out_ptrs: vec![std::ptr::null_mut(); channels],
        }
    }

    /// The frames in one spectral block.
    #[must_use]
    pub fn block_samples(&self) -> usize {
        // SAFETY: `self.raw` is a live handle for as long as `self` is.
        unsafe { bl_stretch_block_samples(self.raw) as usize }
    }

    /// The frames between the starts of two spectral blocks.
    #[must_use]
    pub fn interval_samples(&self) -> usize {
        // SAFETY: `self.raw` is a live handle for as long as `self` is.
        unsafe { bl_stretch_interval_samples(self.raw) as usize }
    }

    /// How far ahead of the output's own time the input is read, in frames.
    #[must_use]
    pub fn input_latency(&self) -> usize {
        // SAFETY: `self.raw` is a live handle for as long as `self` is.
        unsafe { bl_stretch_input_latency(self.raw) as usize }
    }

    /// How far behind that time the output comes out, in frames.
    #[must_use]
    pub fn output_latency(&self) -> usize {
        // SAFETY: `self.raw` is a live handle for as long as `self` is.
        unsafe { bl_stretch_output_latency(self.raw) as usize }
    }

    /// Forgets everything it holds, as at a locate or a launch.
    pub fn reset(&mut self) {
        // SAFETY: `self.raw` is a live handle for as long as `self` is.
        unsafe { bl_stretch_reset(self.raw) }
    }

    /// The pitch shift, as a frequency multiplier (1 leaves the pitch alone).
    pub fn set_transpose_factor(&mut self, multiplier: f64) {
        // SAFETY: `self.raw` is a live handle for as long as `self` is, and
        // a multiplier is a plain number.
        unsafe { bl_stretch_set_transpose_factor(self.raw, multiplier, TONALITY_LIMIT) }
    }

    /// The formant correction: `multiplier` scales the formants and
    /// `compensate_pitch` keeps them in place through the pitch shift.
    pub fn set_formant_factor(&mut self, multiplier: f64, compensate_pitch: bool) {
        // SAFETY: `self.raw` is a live handle for as long as `self` is, and
        // a multiplier is a plain number.
        unsafe { bl_stretch_set_formant_factor(self.raw, multiplier, compensate_pitch) }
    }

    /// The rough fundamental the formant analysis assumes, as a fraction of
    /// the sample rate.
    pub fn set_formant_base(&mut self, base_freq: f64) {
        // SAFETY: `self.raw` is a live handle for as long as `self` is, and
        // a frequency is a plain number.
        unsafe { bl_stretch_set_formant_base(self.raw, base_freq) }
    }

    /// Primes the stretcher with the `input` frames just before playback
    /// starts, so the output begins at the right level (R7).
    pub fn seek(&mut self, input: &[&[f32]], playback_rate: f64) {
        let samples = input.first().map_or(0, |channel| channel.len());
        self.gather_in(input, samples);
        // SAFETY: `self.raw` is a live handle; `in_ptrs` holds one pointer
        // per channel, each to a slice of at least `samples` frames, which
        // `gather_in` has just checked and the library reads no further.
        unsafe {
            bl_stretch_seek(
                self.raw,
                self.in_ptrs.as_ptr(),
                samples as c_int,
                playback_rate,
            )
        }
    }

    /// Consumes `input_frames` frames of each channel of `input` and writes
    /// `out` frames to each channel of `output`.
    pub fn process(&mut self, input: &[&[f32]], input_frames: usize, output: &mut [&mut [f32]]) {
        let out_frames = output.first().map_or(0, |channel| channel.len());
        self.gather_in(input, input_frames);
        self.gather_out(output);
        // SAFETY: `self.raw` is a live handle; `in_ptrs` and `out_ptrs` hold
        // one pointer per channel into slices the caller has lent for this
        // call, and the library touches only the frames asked for.
        unsafe {
            bl_stretch_process(
                self.raw,
                self.in_ptrs.as_ptr(),
                input_frames as c_int,
                self.out_ptrs.as_ptr(),
                out_frames as c_int,
            )
        }
    }

    /// Points the wrapper's input array at `input`'s first `samples` frames.
    fn gather_in(&mut self, input: &[&[f32]], samples: usize) {
        for (pointer, channel) in self.in_ptrs.iter_mut().zip(input) {
            debug_assert!(
                channel.len() >= samples,
                "a channel is shorter than its read"
            );
            *pointer = channel.as_ptr();
        }
    }

    /// Points the wrapper's output array at each channel of `output`.
    fn gather_out(&mut self, output: &mut [&mut [f32]]) {
        for (pointer, channel) in self.out_ptrs.iter_mut().zip(output) {
            *pointer = channel.as_mut_ptr();
        }
    }
}

impl Drop for Stretcher {
    fn drop(&mut self) {
        // SAFETY: the handle was made by `bl_stretch_new` and is freed once,
        // here, because `Stretcher` is never cloned.
        unsafe { bl_stretch_free(self.raw) }
    }
}

impl std::fmt::Debug for Stretcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stretcher")
            .field("channels", &self.channels)
            .field("block_samples", &self.block_samples())
            .field("interval_samples", &self.interval_samples())
            .finish_non_exhaustive()
    }
}
