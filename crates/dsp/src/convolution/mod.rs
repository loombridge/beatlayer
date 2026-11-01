//! Partitioned convolution (WP-8.29): a [`Convolver`] plays a prepared
//! impulse response ([`PreparedIr`]) over a stereo signal with no latency,
//! for mono, stereo and true-stereo responses of up to 30 seconds, and swaps
//! one response for another with a crossfade.
//!
//! # Design
//!
//! The method is non-uniform partitioned convolution after W. G. Gardner,
//! "Efficient convolution without input-output delay", JAES 43(3), 1995,
//! with the scheduling of F. Wefers, "Partitioned convolution algorithms for
//! real-time auralization", thesis, RWTH Aachen, 2014. The response is cut
//! into a head and three stages of uniform partitions:
//!
//! | Part | Block `B` | Taps it covers | Transform |
//! |---|---|---|---|
//! | head | 1 | 0 to 63 | direct form |
//! | stage 0 | 64 | 64 to 1,023 | 128 points, one tick |
//! | stage 1 | 512 | 1,024 to 4,095 | 1,024 points, 8 ticks |
//! | stage 2 | 2,048 | 4,096 to the end | 4,096 points, 32 ticks |
//!
//! A *tick* is 64 frames. The convolver counts frames itself and does its
//! frequency-domain work on every 64th, so what it outputs never depends on
//! how the host cuts its blocks. A stage's input spectra go into a frequency
//! delay line shared by every response, and each partition of the response
//! multiplies one of them. The stage's output for a block of `B` frames is
//! ready `2 B` frames after the block starts (`B` for the transform's own
//! window, `B` for the block itself), which is why each stage starts at `2 B`
//! taps (stage 0 at `B`, as its work is done within the tick it finishes
//! in): the head and the smaller stages have already covered the taps before
//! it. The first output frame carries the response's first tap.
//!
//! Each stage's transform is a four-step FFT ([`four_step`]), run a few of its
//! small transforms on each tick instead of all at once: the forward
//! transform of a stage's newest block in the first half of its window, the
//! inverse transform in the second, so every tick of a stage does the same
//! work. A stage's multiply-accumulate is spread in the same way, a slice of
//! the bins on every tick, a block ahead. [`Convolver::sub_ffts`] counts the
//! small transforms, which the tests hold to the stage's share.
//!
//! # Swapping
//!
//! [`Convolver::swap`] starts a second *voice*, which reads the same input
//! spectra with the new response's partitions. Its stages start part-way
//! through a block, so it is exact only from the first block whose work it
//! has done from the start: up to about `2.5 B` frames of the largest stage
//! it uses after the swap (a little over 100 ms at 48 kHz for a response
//! longer than 4,096 frames). Until then the old response keeps playing; the
//! crossfade, linear in amplitude over the time the caller gives, then runs
//! between two exact outputs, and the old response goes to
//! [`take_retired`](Convolver::take_retired) when it ends, for the caller to
//! send to the drop thread. A swap called while another is running waits
//! behind it, and replaces any swap already waiting.
//!
//! # Limits
//!
//! [`prepare`] refuses a response longer than 30 seconds at the engine's
//! rate (the file's own length is checked by the caller against
//! [`MAX_SECONDS`] before resampling, as the impulse-responses spec says),
//! and one whose prepared form passes [`MAX_PREPARED_BYTES`], 128 MiB: that
//! is 30 seconds of true stereo at up to 96 kHz, or of stereo at up to
//! 192 kHz. A convolver's own memory is set by the longest response it will
//! be given ([`Convolver::new`]): two frequency delay lines the size of one
//! of that response's channels.
//!
//! Every buffer is made in [`prepare`] and [`Convolver::new`], off the audio
//! thread; processing, resetting and swapping allocate nothing.

mod four_step;

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use rustfft::FftPlanner;
use rustfft::num_complex::Complex32;

use four_step::FourStep;

/// The crossfade a swap takes unless its caller chooses otherwise, in seconds.
pub const DEFAULT_FADE: f32 = 0.05;
/// The longest response, in seconds at the engine's rate.
pub const MAX_SECONDS: u32 = 30;
/// The largest a response may be once prepared, in bytes.
pub const MAX_PREPARED_BYTES: usize = 128 << 20;
/// The frames between two bursts of frequency-domain work.
pub const TICK: usize = 64;
/// The direct-form taps at the head of a response.
pub const HEAD: usize = 64;
/// The least time a crossfade takes, in frames.
const MIN_FADE: u64 = 1;
/// The retired responses a convolver holds for its caller to collect.
const RETIRED: usize = 4;
/// The frames of input a convolver remembers: the largest stage's frame.
const HISTORY: usize = 4096;

/// One stage of uniform partitions.
struct StageDef {
    /// The partition and block size, in frames.
    b: usize,
    /// The first tap the stage covers.
    off: usize,
    /// The four-step shape of the stage's `2 b`-point transform.
    r: usize,
    c: usize,
}

impl StageDef {
    /// The bins of a half spectrum.
    const fn bins(&self) -> usize {
        self.b + 1
    }

    /// The ticks a block of the stage spans.
    const fn ticks(&self) -> usize {
        self.b / TICK
    }
}

const STAGES: [StageDef; 3] = [
    StageDef {
        b: 64,
        off: 64,
        r: 16,
        c: 8,
    },
    StageDef {
        b: 512,
        off: 1024,
        r: 32,
        c: 32,
    },
    StageDef {
        b: 2048,
        off: 4096,
        r: 64,
        c: 64,
    },
];

/// The partitions each stage needs for a response of `frames` frames.
fn partitions(frames: usize) -> [usize; 3] {
    std::array::from_fn(|k| {
        let start = STAGES[k].off;
        let end = STAGES.get(k + 1).map_or(usize::MAX, |next| next.off);
        if frames > start {
            (frames.min(end) - start).div_ceil(STAGES[k].b)
        } else {
            0
        }
    })
}

/// The channels of a response and what they connect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One channel, applied to each input channel and heard on both outputs.
    Mono,
    /// Two channels: left to left and right to right.
    Stereo,
    /// Four channels, left to left, left to right, right to left and right
    /// to right.
    TrueStereo,
}

impl Layout {
    /// The channels a response of this layout has.
    #[must_use]
    pub fn channels(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            Self::TrueStereo => 4,
        }
    }

    /// The (input channel, response channel) pairs that sum into output `o`.
    fn paths(self, o: usize) -> &'static [(usize, usize)] {
        match (self, o) {
            (Self::Mono, 0) => &[(0, 0)],
            (Self::Mono, _) => &[(1, 0)],
            (Self::Stereo, 0) => &[(0, 0)],
            (Self::Stereo, _) => &[(1, 1)],
            (Self::TrueStereo, 0) => &[(0, 0), (1, 2)],
            (Self::TrueStereo, _) => &[(0, 1), (1, 3)],
        }
    }
}

/// Why [`prepare`] refused a response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrepareError {
    /// The response has no frames, or no channels, or channels of different lengths.
    Empty,
    /// A response has one, two or four channels.
    Channels(usize),
    /// The engine's rate is zero.
    Rate,
    /// The response is longer than [`MAX_SECONDS`].
    TooLong {
        /// Its length in frames.
        frames: usize,
        /// The most frames at this rate.
        max: usize,
    },
    /// The prepared response would pass [`MAX_PREPARED_BYTES`].
    TooLarge {
        /// What it would take.
        bytes: usize,
    },
}

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the impulse response is empty"),
            Self::Channels(n) => write!(
                f,
                "an impulse response has one, two or four channels, not {n}"
            ),
            Self::Rate => f.write_str("the sample rate is zero"),
            Self::TooLong { frames, max } => write!(
                f,
                "the impulse response is {frames} frames, longer than the {MAX_SECONDS} seconds ({max} frames) the engine takes"
            ),
            Self::TooLarge { bytes } => write!(
                f,
                "the impulse response would take {} MiB prepared, more than the {} MiB allowed",
                bytes >> 20,
                MAX_PREPARED_BYTES >> 20
            ),
        }
    }
}

impl std::error::Error for PrepareError {}

/// One channel of a prepared response.
struct IrChannel {
    head: [f32; HEAD],
    /// Each stage's partitions' half spectra, `bins` to a partition, in order.
    spectra: [Vec<Complex32>; 3],
}

/// An impulse response cut into partitions and transformed, ready for a
/// [`Convolver`] of the same rate. Immutable, so any number of convolvers
/// and threads may share it.
pub struct PreparedIr {
    rate: u32,
    layout: Layout,
    frames: usize,
    parts: [usize; 3],
    channels: Vec<IrChannel>,
}

impl PreparedIr {
    /// The rate the response is at, which a convolver must share.
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The response's channel layout.
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// The response's length in frames.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The memory the prepared response takes, in bytes.
    #[must_use]
    pub fn bytes(&self) -> usize {
        prepared_bytes(self.layout.channels(), self.parts)
    }
}

impl fmt::Debug for PreparedIr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedIr")
            .field("rate", &self.rate)
            .field("layout", &self.layout)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

fn prepared_bytes(channels: usize, parts: [usize; 3]) -> usize {
    let spectra: usize = (0..3)
        .map(|k| parts[k] * STAGES[k].bins() * size_of::<Complex32>())
        .sum();
    channels * (HEAD * size_of::<f32>() + spectra)
}

/// Prepares a response, one slice of frames per channel, at the engine's
/// `rate`: resample it first when its file is at another. Allocates and runs
/// the transforms, so never call it on the audio thread, nor on the
/// compiler thread while a running engine waits for it.
///
/// # Errors
///
/// If the response is empty, has other than one, two or four channels,
/// channels of different lengths, is longer than [`MAX_SECONDS`] at `rate`,
/// or would take more than [`MAX_PREPARED_BYTES`].
pub fn prepare(channels: &[&[f32]], rate: u32) -> Result<Arc<PreparedIr>, PrepareError> {
    let layout = match channels.len() {
        1 => Layout::Mono,
        2 => Layout::Stereo,
        4 => Layout::TrueStereo,
        n => return Err(PrepareError::Channels(n)),
    };
    if rate == 0 {
        return Err(PrepareError::Rate);
    }
    let frames = channels[0].len();
    if frames == 0 || channels.iter().any(|c| c.len() != frames) {
        return Err(PrepareError::Empty);
    }
    // A file of exactly 30 seconds, resampled, can come out a frame longer.
    let max = MAX_SECONDS as usize * rate as usize + 1;
    if frames > max {
        return Err(PrepareError::TooLong { frames, max });
    }
    let parts = partitions(frames);
    let bytes = prepared_bytes(layout.channels(), parts);
    if bytes > MAX_PREPARED_BYTES {
        return Err(PrepareError::TooLarge { bytes });
    }
    let mut planner = FftPlanner::<f32>::new();
    let plans: [_; 3] = std::array::from_fn(|k| planner.plan_fft_forward(2 * STAGES[k].b));
    let mut work = vec![Complex32::new(0.0, 0.0); 2 * STAGES[2].b];
    let channels = channels
        .iter()
        .map(|ir| {
            let mut head = [0.0; HEAD];
            let n = ir.len().min(HEAD);
            head[..n].copy_from_slice(&ir[..n]);
            let spectra = std::array::from_fn(|k| {
                let def = &STAGES[k];
                let (m, bins) = (2 * def.b, def.bins());
                let mut out = Vec::with_capacity(parts[k] * bins);
                for p in 0..parts[k] {
                    let start = def.off + p * def.b;
                    let segment = &ir[start..(start + def.b).min(ir.len())];
                    let buffer = &mut work[..m];
                    buffer.fill(Complex32::new(0.0, 0.0));
                    for (slot, &v) in buffer.iter_mut().zip(segment) {
                        slot.re = v;
                    }
                    plans[k].process(buffer);
                    out.extend_from_slice(&buffer[..bins]);
                }
                out
            });
            IrChannel { head, spectra }
        })
        .collect();
    Ok(Arc::new(PreparedIr {
        rate,
        layout,
        frames,
        parts,
        channels,
    }))
}

/// Why [`Convolver::swap`] didn't take a response, which it hands back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapError {
    /// The response is at another rate than the convolver.
    Rate,
    /// The response is longer than the convolver was made for.
    TooLong,
    /// Responses already retired are waiting to be collected; call
    /// [`Convolver::take_retired`].
    Busy,
}

impl fmt::Display for SwapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rate => "the response is at another sample rate than the convolver",
            Self::TooLong => "the response is longer than the convolver was made for",
            Self::Busy => "retired responses are waiting to be collected",
        })
    }
}

impl std::error::Error for SwapError {}

/// The last frames of the input, kept twice over so any recent run is one slice.
struct History {
    buf: Vec<f32>,
    pos: usize,
}

impl History {
    fn new() -> Self {
        Self {
            buf: vec![0.0; 2 * HISTORY],
            pos: 0,
        }
    }

    fn push(&mut self, samples: &[f32]) {
        for &x in samples {
            self.buf[self.pos] = x;
            self.buf[self.pos + HISTORY] = x;
            self.pos += 1;
            if self.pos == HISTORY {
                self.pos = 0;
            }
        }
    }

    /// The last `n` frames, oldest first.
    fn recent(&self, n: usize) -> &[f32] {
        &self.buf[self.pos + HISTORY - n..self.pos + HISTORY]
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

/// A stage's state that every response shares: the input spectra.
struct Stage {
    fft: FourStep,
    /// Per input channel: `depth` half spectra, indexed by block number.
    fdl: [Vec<Complex32>; 2],
    /// The `2 b` frames being transformed, per input channel.
    frame: [Vec<f32>; 2],
    tmp: [Vec<Complex32>; 2],
}

/// A voice's state for one stage.
struct VoiceStage {
    /// Per output channel and block parity: the half spectrum being summed.
    acc: [Vec<Complex32>; 4],
    /// Per output channel and block parity: the block's output.
    res: [Vec<f32>; 4],
    tmp: [Vec<Complex32>; 2],
}

/// One response being played: the old one, or the new one while it is made ready.
struct Voice {
    ir: Option<Arc<PreparedIr>>,
    /// Per stage, the first block this voice computes from the start.
    first: [u64; 3],
    stages: [VoiceStage; 3],
}

/// A crossfade in progress or waiting for its voice to be exact.
#[derive(Clone, Copy)]
struct Fade {
    start: u64,
    len: u64,
}

/// A zero-latency convolver of a stereo signal with a prepared response.
pub struct Convolver {
    rate: u32,
    max_parts: [usize; 3],
    depth: [usize; 3],
    history: [History; 2],
    /// Frames since the last tick.
    fill: usize,
    /// Ticks run.
    events: u64,
    /// Frames processed.
    frames: u64,
    stages: [Stage; 3],
    voices: [Voice; 2],
    active: usize,
    fade: Option<Fade>,
    queued: Option<(Arc<PreparedIr>, u64)>,
    retired: Vec<Arc<PreparedIr>>,
}

impl Convolver {
    /// A convolver at `rate` that takes responses of up to `max_frames`
    /// frames. Allocates; its memory grows with `max_frames`, so give the
    /// longest response it will play, not the largest allowed.
    #[must_use]
    pub fn new(rate: u32, max_frames: usize) -> Self {
        let max_parts = partitions(max_frames);
        let depth = max_parts.map(|p| p.max(1) + 1);
        let zero = |n: usize| vec![Complex32::new(0.0, 0.0); n];
        let stages = std::array::from_fn(|k| {
            let def = &STAGES[k];
            let fft = FourStep::new(def.r, def.c);
            assert!(
                def.ticks() == 1 || fft.steps().is_multiple_of(def.ticks() / 2),
                "a stage's steps divide evenly into half its ticks"
            );
            Stage {
                fft,
                fdl: std::array::from_fn(|_| zero(depth[k] * def.bins())),
                frame: std::array::from_fn(|_| vec![0.0; 2 * def.b]),
                tmp: std::array::from_fn(|_| zero(2 * def.b)),
            }
        });
        let voice = || Voice {
            ir: None,
            first: [0; 3],
            stages: std::array::from_fn(|k| {
                let def = &STAGES[k];
                VoiceStage {
                    acc: std::array::from_fn(|_| zero(def.bins())),
                    res: std::array::from_fn(|_| vec![0.0; def.b]),
                    tmp: std::array::from_fn(|_| zero(2 * def.b)),
                }
            }),
        };
        Self {
            rate,
            max_parts,
            depth,
            history: [History::new(), History::new()],
            fill: 0,
            events: 0,
            frames: 0,
            stages,
            voices: [voice(), voice()],
            active: 0,
            fade: None,
            queued: None,
            retired: Vec::with_capacity(RETIRED),
        }
    }

    /// A convolver that plays `ir` from its first frame, as one does that has
    /// heard silence before it: nothing waits for the response to be exact.
    /// Allocates. Later responses are swapped in as [`swap`](Self::swap) says.
    #[must_use]
    pub fn with_response(ir: Arc<PreparedIr>) -> Self {
        let mut conv = Self::new(ir.rate, ir.frames);
        conv.voices[0].ir = Some(ir);
        conv
    }

    /// The sample rate.
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The frames of latency: none.
    #[must_use]
    pub fn latency(&self) -> usize {
        0
    }

    /// The memory the convolver holds, in bytes.
    #[must_use]
    pub fn bytes(&self) -> usize {
        let fdl: usize = (0..3)
            .map(|k| 2 * self.depth[k] * STAGES[k].bins() * size_of::<Complex32>())
            .sum();
        let voices: usize = (0..3)
            .map(|k| {
                let def = &STAGES[k];
                2 * (4 * def.bins() * size_of::<Complex32>()
                    + 4 * def.b * size_of::<f32>()
                    + 2 * 2 * def.b * size_of::<Complex32>())
            })
            .sum();
        fdl + voices
    }

    /// How many small transforms each stage has run: the unit of its work,
    /// which a stage runs the same number of on every tick.
    #[must_use]
    pub fn sub_ffts(&self) -> [u64; 3] {
        std::array::from_fn(|k| self.stages[k].fft.calls())
    }

    /// The frame the crossfade to a swapped-in response starts at, and its
    /// length, while a swap is under way and its response has been made ready.
    #[must_use]
    pub fn fade(&self) -> Option<(u64, u64)> {
        self.fade.map(|f| (f.start, f.len))
    }

    /// Whether a swap is under way or waiting.
    #[must_use]
    pub fn swapping(&self) -> bool {
        self.fade.is_some() || self.queued.is_some()
    }

    /// The frames processed since creation or the last [`reset`](Self::reset).
    #[must_use]
    pub fn position(&self) -> u64 {
        self.frames
    }

    /// Changes to a response, crossfading over `fade` seconds once the new
    /// response is exact (see the module's notes). Allocates nothing. The first
    /// response a convolver gets fades in from silence.
    ///
    /// # Errors
    ///
    /// If the response can't be played here, or retired responses are waiting
    /// to be collected, with the response handed back so the caller frees it
    /// off the audio thread.
    pub fn swap(
        &mut self,
        ir: Arc<PreparedIr>,
        fade: f32,
    ) -> Result<(), (SwapError, Arc<PreparedIr>)> {
        if ir.rate != self.rate {
            return Err((SwapError::Rate, ir));
        }
        if (0..3).any(|k| ir.parts[k] > self.max_parts[k]) {
            return Err((SwapError::TooLong, ir));
        }
        // Room for the response this call may retire and the one the end of
        // the fade will.
        if self.retired.len() + 2 > RETIRED {
            return Err((SwapError::Busy, ir));
        }
        let len = ((f64::from(fade) * f64::from(self.rate)).round() as u64).max(MIN_FADE);
        if self.fade.is_none() {
            self.start_incoming(ir, len);
        } else if let Some((old, _)) = self.queued.replace((ir, len)) {
            self.retired.push(old);
        }
        Ok(())
    }

    /// A response the convolver has finished with, for the caller to send to
    /// the drop thread. Call it after each block until it gives `None`.
    pub fn take_retired(&mut self) -> Option<Arc<PreparedIr>> {
        self.retired.pop()
    }

    /// Clears the history, as after a jump in the song or a device switched
    /// off: the next frames convolve with silence before them.
    pub fn reset(&mut self) {
        for h in &mut self.history {
            h.clear();
        }
        self.fill = 0;
        self.events = 0;
        self.frames = 0;
        for v in &mut self.voices {
            v.first = [0; 3];
        }
        // A response being made ready starts again from here.
        if self.fade.is_some() {
            let incoming = 1 - self.active;
            if let Some(len) = self.fade.map(|f| f.len) {
                self.arm(incoming, len);
            }
        }
    }

    /// Convolves `left` and `right` in place.
    ///
    /// # Panics
    ///
    /// If the slices differ in length.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), right.len(), "the channels must match");
        let mut done = 0;
        while done < left.len() {
            let m = (TICK - self.fill).min(left.len() - done);
            let range = done..done + m;
            self.history[0].push(&left[range.clone()]);
            self.history[1].push(&right[range.clone()]);
            let mut out = [[0.0; TICK]; 2];
            self.render_chunk(m, &mut out);
            left[range.clone()].copy_from_slice(&out[0][..m]);
            right[range].copy_from_slice(&out[1][..m]);
            self.frames += m as u64;
            self.fill += m;
            done += m;
            if self.fade.is_some_and(|f| self.frames >= f.start + f.len) {
                self.finish_fade();
            }
            if self.fill == TICK {
                self.fill = 0;
                self.event();
            }
        }
    }

    /// The output of the chunk of `m` frames that ends at the frame just pushed.
    fn render_chunk(&self, m: usize, out: &mut [[f32; TICK]; 2]) {
        let n0 = self.frames;
        self.render_voice(self.active, n0, m, out);
        let Some(fade) = self.fade else { return };
        if n0 + m as u64 <= fade.start {
            return;
        }
        let mut next = [[0.0; TICK]; 2];
        self.render_voice(1 - self.active, n0, m, &mut next);
        for (old, new) in out.iter_mut().zip(&next) {
            for (j, (a, &b)) in old[..m].iter_mut().zip(&new[..m]).enumerate() {
                let n = n0 + j as u64;
                let g = if n <= fade.start {
                    0.0
                } else if n >= fade.start + fade.len {
                    1.0
                } else {
                    ((n - fade.start) as f64 / fade.len as f64) as f32
                };
                *a = *a * (1.0 - g) + b * g;
            }
        }
    }

    /// One voice's output for the `m` frames from `n0`: the head's taps, then
    /// each stage's block.
    fn render_voice(&self, v: usize, n0: u64, m: usize, out: &mut [[f32; TICK]; 2]) {
        let voice = &self.voices[v];
        let Some(ir) = &voice.ir else {
            out.iter_mut().for_each(|o| o.fill(0.0));
            return;
        };
        for (o, dst) in out.iter_mut().enumerate() {
            let dst = &mut dst[..m];
            dst.fill(0.0);
            for &(input, channel) in ir.layout.paths(o) {
                let x = self.history[input].recent(HEAD - 1 + m);
                for (t, &h) in ir.channels[channel].head.iter().enumerate() {
                    let src = &x[HEAD - 1 - t..HEAD - 1 - t + m];
                    for (d, &s) in dst.iter_mut().zip(src) {
                        *d += h * s;
                    }
                }
            }
            for (k, def) in STAGES.iter().enumerate() {
                if ir.parts[k] == 0 || n0 < def.off as u64 {
                    continue;
                }
                let rel = n0 - def.off as u64;
                let block = rel / def.b as u64;
                if block < voice.first[k] {
                    continue;
                }
                let at = (rel % def.b as u64) as usize;
                let src = &voice.stages[k].res[o * 2 + (block & 1) as usize][at..at + m];
                for (d, &s) in dst.iter_mut().zip(src) {
                    *d += s;
                }
            }
        }
    }

    /// Starts the response in the idle voice.
    fn start_incoming(&mut self, ir: Arc<PreparedIr>, len: u64) {
        let incoming = 1 - self.active;
        self.voices[incoming].ir = Some(ir);
        self.arm(incoming, len);
    }

    /// Clears voice `v`'s work and works out when its output is exact.
    fn arm(&mut self, v: usize, len: u64) {
        let voice = &mut self.voices[v];
        let Some(ir) = &voice.ir else { return };
        let next = self.events + 1;
        let mut start = self.frames;
        for (k, def) in STAGES.iter().enumerate() {
            let t = def.ticks() as u64;
            // The tick at which block `m`'s work starts: `m + 1` for a stage
            // that works within a tick, else half a window before the
            // block's own.
            voice.first[k] = if t == 1 {
                next.saturating_sub(1)
            } else {
                (next.saturating_sub(t / 2)).div_ceil(t)
            };
            if ir.parts[k] > 0 {
                start = start.max(voice.first[k] * def.b as u64 + def.off as u64);
            }
            for a in &mut voice.stages[k].acc {
                a.fill(Complex32::new(0.0, 0.0));
            }
            for r in &mut voice.stages[k].res {
                r.fill(0.0);
            }
        }
        self.fade = Some(Fade { start, len });
    }

    /// Ends the crossfade: the new voice plays alone and the old response retires.
    fn finish_fade(&mut self) {
        let old = self.active;
        self.active = 1 - old;
        if let Some(ir) = self.voices[old].ir.take() {
            self.retired.push(ir);
        }
        self.fade = None;
        if let Some((ir, len)) = self.queued.take() {
            self.start_incoming(ir, len);
        }
    }

    /// The frequency-domain work of one tick.
    fn event(&mut self) {
        self.events += 1;
        for k in 0..STAGES.len() {
            self.stage_event(k);
        }
    }

    fn stage_event(&mut self, k: usize) {
        let def = &STAGES[k];
        let e = self.events;
        let t = def.ticks() as u64;
        let steps = self.stages[k].fft.steps();
        if t == 1 {
            // The block that has just ended is transformed, multiplied and
            // transformed back within this tick.
            let j = e - 1;
            self.snapshot(k);
            self.forward(k, j, 0..steps);
            for v in 0..2 {
                self.mac(k, v, j, 0..def.bins(), 0..usize::MAX, true);
            }
            self.inverse(k, j, 0..steps);
            return;
        }
        let (w, tau) = (e / t, e % t);
        let half = t / 2;
        let per = steps / half as usize;
        if w >= 1 && tau < half {
            if tau == 0 {
                self.snapshot(k);
            }
            let from = tau as usize * per;
            self.forward(k, w - 1, from..from + per);
        }
        // The block whose other partitions are being summed: the next, in
        // the second half of a window, and the current in the first.
        let target = if tau >= half {
            Some(w)
        } else {
            w.checked_sub(1)
        };
        if let Some(m) = target {
            let slice = ((tau + half) % t) as usize;
            let chunk = def.bins().div_ceil(t as usize);
            let lo = (slice * chunk).min(def.bins());
            let hi = (lo + chunk).min(def.bins());
            for v in 0..2 {
                self.mac(k, v, m, lo..hi, 1..usize::MAX, true);
            }
        }
        if w >= 1 && tau >= half {
            let j = w - 1;
            if tau == half {
                for v in 0..2 {
                    self.mac(k, v, j, 0..def.bins(), 0..1, false);
                }
            }
            let from = (tau - half) as usize * per;
            self.inverse(k, j, from..from + per);
        }
    }

    /// Keeps the last `2 b` frames for the transform of the block just ended.
    fn snapshot(&mut self, k: usize) {
        let n = 2 * STAGES[k].b;
        for (frame, history) in self.stages[k].frame.iter_mut().zip(&self.history) {
            frame.copy_from_slice(history.recent(n));
        }
    }

    /// Steps of the forward transform of block `j`, into the input spectra.
    fn forward(&mut self, k: usize, j: u64, steps: Range<usize>) {
        let bins = STAGES[k].bins();
        let slot = (j % self.depth[k] as u64) as usize;
        let Stage {
            fft,
            fdl,
            frame,
            tmp,
        } = &mut self.stages[k];
        for ch in 0..2 {
            let frame = &frame[ch];
            let spectrum = &mut fdl[ch][slot * bins..(slot + 1) * bins];
            for s in steps.clone() {
                fft.step(
                    false,
                    s,
                    |i| Complex32::new(frame[i], 0.0),
                    &mut tmp[ch],
                    |i, v| {
                        if i < bins {
                            spectrum[i] = v;
                        }
                    },
                );
            }
        }
    }

    /// Sums the partitions `parts` of block `m` into voice `v`'s
    /// accumulators, for the bins in `range`, after clearing them when `clear`.
    fn mac(
        &mut self,
        k: usize,
        v: usize,
        m: u64,
        range: Range<usize>,
        parts: Range<usize>,
        clear: bool,
    ) {
        let bins = STAGES[k].bins();
        let voice = &mut self.voices[v];
        let Some(ir) = &voice.ir else { return };
        if ir.parts[k] == 0 || m < voice.first[k] || range.is_empty() {
            return;
        }
        let parts = parts.start..parts.end.min(ir.parts[k]);
        let depth = self.depth[k] as u64;
        for o in 0..2 {
            let acc = &mut voice.stages[k].acc[o * 2 + (m & 1) as usize][range.clone()];
            if clear {
                acc.fill(Complex32::new(0.0, 0.0));
            }
            for &(input, channel) in ir.layout.paths(o) {
                let spectra = &ir.channels[channel].spectra[k];
                let fdl = &self.stages[k].fdl[input];
                for p in parts.clone() {
                    if m < p as u64 {
                        break;
                    }
                    let slot = ((m - p as u64) % depth) as usize;
                    let x = &fdl[slot * bins + range.start..slot * bins + range.end];
                    let h = &spectra[p * bins + range.start..p * bins + range.end];
                    for ((a, x), h) in acc.iter_mut().zip(x).zip(h) {
                        a.re += x.re * h.re - x.im * h.im;
                        a.im += x.re * h.im + x.im * h.re;
                    }
                }
            }
        }
    }

    /// Steps of the inverse transform of block `j`, for each voice and
    /// output, into the voices' block outputs.
    fn inverse(&mut self, k: usize, j: u64, steps: Range<usize>) {
        let def = &STAGES[k];
        let (b, m) = (def.b, 2 * def.b);
        let scale = 1.0 / m as f32;
        let parity = (j & 1) as usize;
        let fft = &mut self.stages[k].fft;
        for voice in &mut self.voices {
            if voice.ir.is_none() || j < voice.first[k] {
                continue;
            }
            let VoiceStage { acc, res, tmp } = &mut voice.stages[k];
            for o in 0..2 {
                let acc = &acc[o * 2 + parity];
                let res = &mut res[o * 2 + parity];
                // The spectrum is Hermitian, with real DC and Nyquist bins.
                let get = |i: usize| match i {
                    0 => Complex32::new(acc[0].re, 0.0),
                    _ if i == b => Complex32::new(acc[b].re, 0.0),
                    _ if i < b => acc[i],
                    _ => acc[m - i].conj(),
                };
                for s in steps.clone() {
                    fft.step(true, s, get, &mut tmp[o], |i, v| {
                        if (b..m).contains(&i) {
                            res[i - b] = v.re * scale;
                        }
                    });
                }
            }
        }
    }
}

impl fmt::Debug for Convolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Convolver")
            .field("rate", &self.rate)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}
