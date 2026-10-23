//! The Complex and Complex Pro warp modes (the warp modes spec, R21 to R24):
//! Signalsmith Stretch, the frequency-domain stretcher that changes speed and
//! pitch independently.
//!
//! The stretcher is driven the way its own documentation asks. The player
//! hands this mode a run of song frames with the file position each plays
//! ([`Request`]) and a window of the file around them ([`Window`]); the mode
//! feeds the file to the stretcher at the run's speed and asks for the run's
//! frames out, so the audio lands where the warp map puts it (R24), and the
//! output depends only on the song, the media and the frame playback began on
//! (R6). A start, a locate or a launch primes the stretcher with the audio
//! before the point and drops the stretcher's own latency, so the clip is at
//! its level from its first frame and its reads add no latency (R7, R24).
//!
//! Complex uses the library's own default configuration and never corrects
//! formants; Complex Pro uses a finer configuration and, when the pitch
//! moves, holds the formants by Formants and places the formant analysis by
//! Envelope (R22, R23). Neither is neutral, so both always run the stretcher
//! (R21).

use crate::mode::{Request, WarpMode, Window};
use crate::signalsmith::Stretcher;

/// Formants' default: 100 %, so a transposed Complex Pro clip keeps its
/// formants (R2).
pub const DEFAULT_FORMANTS: f64 = 100.0;
/// Formants' range (R2).
pub const MIN_FORMANTS: f64 = 0.0;
/// Formants' range (R2).
pub const MAX_FORMANTS: f64 = 100.0;

/// Envelope's default (R2).
pub const DEFAULT_ENVELOPE: u32 = 128;
/// Envelope's range (R2).
pub const MIN_ENVELOPE: u32 = 8;
/// Envelope's range (R2).
pub const MAX_ENVELOPE: u32 = 512;

/// Complex's spectral block, in seconds. A longer block than the library's
/// own default (120 ms): the measurements in the tests below choose it, since
/// a transposed clip's pitch is only within the spec's cent at the finer
/// frequency resolution a 240 ms block gives (R21, R35).
const DEFAULT_BLOCK_SECONDS: f64 = 0.24;
/// Complex's interval between spectral blocks: finer than the library's
/// default (30 ms) for the same reason.
const DEFAULT_INTERVAL_SECONDS: f64 = 0.01;
/// Complex Pro's spectral block: longer than Complex's, so the formant
/// analysis it adds has finer frequency resolution still.
const PRO_BLOCK_SECONDS: f64 = 0.36;
/// Complex Pro's interval between spectral blocks.
const PRO_INTERVAL_SECONDS: f64 = 0.01;

/// The most file frames per song frame a window is sized for: a song at four
/// times a clip's own tempo.
const MAX_RATE: f64 = 4.0;

/// The frames the pre-roll and the lead add beyond the latencies, so no
/// rounding reaches past what [`Complex::reach`] asked the window to hold.
const MARGIN: usize = 32;

/// The frames a prime's dropped output is written into, a chunk at a time so
/// this stays small.
const SCRATCH_FRAMES: usize = 2_048;

/// Half of Complex Pro's spectral block, in seconds: the larger of the two
/// modes' latencies, which is what a window must be sized for.
const WIDEST_HALF: f64 = PRO_BLOCK_SECONDS / 2.0;

/// How far before a position a Complex read reaches at `rate` file frames per
/// song frame, at `sample_rate`: the audio a prime seeks with, so a locate or a
/// launch starts at the right level (R7, R24).
#[must_use]
fn seek_reach(sample_rate: f64, rate: f64) -> usize {
    let half = sample_rate * WIDEST_HALF;
    ((half * (1.0 + rate.max(0.0))).ceil() as usize).max(1) + MARGIN
}

/// The most file frames any Complex read reaches each way, at `sample_rate`:
/// what a window around a run must hold, and what a clip's source must keep
/// ready. Kept in step with [`Complex::reach`], which no setting widens.
#[must_use]
pub fn max_reach(sample_rate: f64) -> usize {
    seek_reach(sample_rate, MAX_RATE)
}

/// The frames of channel `channel` at file frames `first..first + frames`,
/// where the window holds them. `None` where the window doesn't.
fn slice<'a>(
    window: &'a Window<'_>,
    channel: usize,
    first: i64,
    frames: usize,
) -> Option<&'a [f32]> {
    let data = window.channel(channel.min(window.channels - 1));
    let offset = first - window.first;
    let offset = usize::try_from(offset).ok()?;
    data.get(offset..offset + frames)
}

/// A Complex or Complex Pro warp mode: one Signalsmith Stretch instance with
/// the settings and the bookkeeping that keep its output on the warp map.
#[derive(Debug)]
pub struct Complex {
    stretcher: Stretcher,
    channels: usize,
    sample_rate: f64,
    /// Whether this is Complex Pro, which alone has Formants and Envelope.
    pro: bool,
    formants: f64,
    envelope: u32,
    /// The song frame the last run ended at, so a run that doesn't follow it
    /// (a start, a locate, a jump) primes afresh (R7).
    ended: Option<i64>,
    /// The clip the last run was for, so a run of another clip taking this
    /// instance over primes afresh too: a pool's stretcher serves several of
    /// the track's clips that never sound at once (R5, R7).
    clip: u64,
    /// The file frame the input has been fed up to.
    fed: i64,
    /// How many times the stretcher has been primed, for the loop-wrap test
    /// (R7).
    #[cfg(test)]
    primes: u64,
    /// The room a prime's dropped output is written into, in frames per
    /// channel: made once, so no prime allocates (R5).
    scratch: Vec<f32>,
    scratch_frames: usize,
}

impl Complex {
    /// The file frames the input is fed ahead of the audio it makes, at
    /// `rate`: the frames a prime drops, in file frames, so a run's feed and a
    /// prime's agree (R6, R24).
    fn lead_frames(&self, rate: f64) -> usize {
        (rate.max(0.0) * self.discard_frames(rate) as f64).round() as usize
    }

    /// How far beyond a run's last position this mode's reads reach at `rate`:
    /// the lead, and the frame the run's own speed adds, with room.
    fn after_frames(&self, rate: f64) -> usize {
        self.lead_frames(rate) + rate.max(0.0).ceil() as usize + MARGIN
    }

    /// The audio a prime seeks with, at `rate`: the audio the stretcher reads
    /// ahead of the point it lands on (R7, R24).
    fn pre_roll_frames(&self, rate: f64) -> usize {
        self.stretcher.output_seek_length(rate) + MARGIN
    }

    /// How many output frames a prime drops at `rate`: the library's output
    /// latency, and its input latency in the output frames it feeds while
    /// dropping them (R24).
    fn discard_frames(&self, rate: f64) -> usize {
        let input = self.stretcher.input_latency() as f64;
        let output = self.stretcher.output_latency() as f64;
        (output + input / rate.max(1e-6)).round().max(0.0) as usize + 2
    }

    /// Makes a stretcher for `channels` channels at `sample_rate` Hz, for
    /// Complex or for Complex Pro. Called with a clip list, off the audio
    /// thread (R5).
    #[must_use]
    pub fn new(channels: usize, sample_rate: f64, pro: bool) -> Self {
        let (block, interval) = if pro {
            (PRO_BLOCK_SECONDS, PRO_INTERVAL_SECONDS)
        } else {
            (DEFAULT_BLOCK_SECONDS, DEFAULT_INTERVAL_SECONDS)
        };
        let block_samples = (sample_rate * block).round() as usize;
        let interval_samples = (sample_rate * interval).round() as usize;
        let mut stretcher = Stretcher::new(channels.max(1), block_samples, interval_samples, false);
        // Grow the library's own scratch here, off the audio thread, so no
        // render or prime allocates later (R5).
        stretcher.warm();
        Self {
            stretcher,
            channels: channels.max(1),
            sample_rate,
            pro,
            formants: DEFAULT_FORMANTS,
            envelope: DEFAULT_ENVELOPE,
            ended: None,
            clip: 0,
            fed: 0,
            #[cfg(test)]
            primes: 0,
            scratch: vec![0.0; channels.max(1) * SCRATCH_FRAMES],
            scratch_frames: SCRATCH_FRAMES,
        }
    }

    /// The spectral block this mode configured, for the measurements that
    /// chose it.
    #[must_use]
    pub fn block_samples(&self) -> usize {
        self.stretcher.block_samples()
    }

    /// The interval between this mode's spectral blocks.
    #[must_use]
    pub fn interval_samples(&self) -> usize {
        self.stretcher.interval_samples()
    }

    /// Sets Formants, in per cent (R22). Complex ignores it.
    pub fn set_formants(&mut self, formants: f64) {
        self.formants = formants.clamp(MIN_FORMANTS, MAX_FORMANTS);
    }

    /// Sets Envelope (R23). Complex ignores it.
    pub fn set_envelope(&mut self, envelope: u32) {
        self.envelope = envelope.clamp(MIN_ENVELOPE, MAX_ENVELOPE);
    }

    /// Applies a pitch ratio to the stretcher, and Complex Pro's formant
    /// correction with it (R22, R23).
    fn tune(&mut self, ratio: f64) {
        let ratio = ratio.max(1e-3);
        self.stretcher.set_transpose_factor(ratio);
        if !self.pro {
            return;
        }
        // Spec: warp-modes R22. Formants at 100 % keeps the formants where
        // they were however far the pitch moves; at 0 % they move with it,
        // which is no formant correction at all.
        let share = self.formants / 100.0;
        if (ratio - 1.0).abs() < 1e-12 || share <= 0.0 {
            // No pitch change, or no correction asked for: the formant stage
            // is off, so Formants and Envelope change nothing (R22, R23).
            self.stretcher.set_formant_factor(1.0, false);
            return;
        }
        self.stretcher
            .set_formant_factor(ratio.powf(1.0 - share), true);
        // Spec: warp-modes R23. Envelope places the fundamental the formant
        // analysis assumes, 44,100 / Envelope Hz, as a fraction of the rate.
        let base = 44_100.0 / f64::from(self.envelope) / self.sample_rate;
        self.stretcher.set_formant_base(base);
    }

    /// The input frames from `self.fed` up to file frame `until`, one channel
    /// an item, and how many: the window holds them, and the iterator builds
    /// no collection, so a render allocates nothing (R5).
    fn inputs<'a>(
        &self,
        window: &'a Window<'_>,
        until: i64,
    ) -> (impl Iterator<Item = &'a [f32]> + 'a, usize) {
        let room = (window.first + window.frames as i64 - self.fed).max(0);
        let need = (until - self.fed).max(0).min(room) as usize;
        let fed = self.fed;
        let channels = self.channels;
        (
            (0..channels).map(move |c| slice(window, c, fed, need.max(1)).unwrap_or(&[])),
            need,
        )
    }

    /// Starts afresh at the run's first frame: seeks the stretcher to the
    /// point, delivering the audio just before it, and drops the stretcher's
    /// own input and output latency, so the run's first frame comes out
    /// aligned to the point (R7, R24).
    fn prime(&mut self, request: &Request<'_>, window: &Window<'_>) {
        #[cfg(test)]
        {
            self.primes += 1;
        }
        let rate = request.speed.max(1e-6);
        let p0 = request.positions[0].round() as i64;
        // `output_seek` resets the stretcher itself, so the settings go on
        // before it.
        self.tune(request.pitch);
        let seek = self.stretcher.output_seek_length(rate).max(1);
        let from = p0 - seek as i64;
        let before = (0..self.channels).map(|c| slice(window, c, from, seek).unwrap_or(&[]));
        self.stretcher.output_seek(before, seek);
        // The dropped output: the seek's pre-computed latency, while the input
        // it needs is fed from the point on.
        let discard = self.discard_frames(rate);
        self.fed = p0;
        let mut produced = 0;
        while produced < discard {
            let n = (discard - produced).min(self.scratch_frames);
            let until = (p0 as f64 + rate * (produced + n) as f64).round() as i64;
            let (inputs, need) = self.inputs(window, until);
            let columns = self
                .scratch
                .chunks_mut(self.scratch_frames)
                .take(self.channels)
                .map(|channel| &mut channel[..n]);
            self.stretcher.process(inputs, need, columns, n);
            self.fed = until;
            produced += n;
        }
    }

    /// Feeds the run's file and writes its `n` frames of output into `out`
    /// (planar, `stride` apart) starting at channel offset 0.
    fn render_run(
        &mut self,
        request: &Request<'_>,
        window: &Window<'_>,
        out: &mut [f32],
        stride: usize,
        n: usize,
    ) {
        let rate = request.speed.max(1e-6);
        let lead = self.lead_frames(rate) as i64;
        let start = request.positions[0].round() as i64 + lead;
        // The run feeds its own span, from the frame its first one plays, ahead
        // by the lead. A run the last didn't leave the input at (a clip loop
        // brace's wrap, which jumps the file back while the song runs on)
        // re-anchors there, so the mode goes on rather than restarting (R7).
        if (start - self.fed).abs() > MARGIN as i64 {
            self.fed = start;
        }
        // Feed up to the position one frame past the run: the input a run takes
        // is then exactly its own span, whatever the block size (R6).
        let until = (request.positions[0] + rate * n as f64).round() as i64 + lead;
        let (inputs, need) = self.inputs(window, until);
        let columns = out
            .chunks_mut(stride)
            .take(self.channels)
            .map(|channel| &mut channel[..n]);
        self.stretcher.process(inputs, need, columns, n);
        self.fed = until;
    }
}

impl WarpMode for Complex {
    // Spec: warp-modes R6, R7, R24
    fn reach(&self, request: &Request<'_>) -> (usize, usize) {
        let rate = request.speed.max(1e-6);
        (self.pre_roll_frames(rate), self.after_frames(rate))
    }

    // Spec: warp-modes R21 to R24
    fn render(
        &mut self,
        request: &Request<'_>,
        window: &Window<'_>,
        out: &mut [f32],
        stride: usize,
    ) {
        let n = request.positions.len();
        if n == 0 || window.channels == 0 {
            return;
        }
        // Spec: warp-modes R7. One decision, here: a run of another clip, or
        // one that doesn't follow the song frame the last run ended on (the
        // clip's start, a locate, a jump), primes the stretcher afresh. A wrap
        // of the clip's own loop brace keeps the song frame continuous, so it
        // doesn't restart the mode: `render_run` re-anchors the feed at the
        // brace's start and the stretcher goes on.
        if self.clip != request.clip || self.ended != Some(request.first) {
            self.prime(request, window);
        }
        self.tune(request.pitch);
        self.render_run(request, window, out, stride, n);
        self.clip = request.clip;
        self.ended = Some(request.first + n as i64);
    }

    fn reset(&mut self) {
        self.ended = None;
        self.clip = 0;
        self.fed = 0;
    }
}

#[cfg(test)]
mod tests {
    use crate::beats::Plan;

    use super::*;

    /// Renders one run of `n` frames whose file positions start at `at`, for
    /// clip `clip`, through a window of `file` built as the player builds it.
    fn run(mode: &mut Complex, file: &[f32], at: f64, first: i64, n: usize, clip: u64) -> Vec<f32> {
        let empty = Plan::EMPTY;
        let positions: Vec<f64> = (0..n).map(|i| at + i as f64).collect();
        let request = Request {
            positions: &positions,
            first,
            speed: 1.0,
            neutral: false,
            high_quality: true,
            beats: &empty,
            transient_envelope: 100.0,
            pitch: 1.0,
            clip,
        };
        let (before, after) = mode.reach(&request);
        let lo = positions[0].floor() as i64 - before as i64;
        let hi = positions[n - 1].floor() as i64 + after as i64 + 1;
        let len = (hi - lo) as usize;
        let mut data = vec![0.0f32; len];
        for (i, sample) in data.iter_mut().enumerate() {
            let frame = lo + i as i64;
            if frame >= 0 && (frame as usize) < file.len() {
                *sample = file[frame as usize];
            }
        }
        let window = Window {
            data: &data,
            stride: len,
            channels: 1,
            frames: len,
            first: lo,
        };
        let mut out = vec![0.0f32; n];
        mode.render(&request, &window, &mut out, n);
        out
    }

    // Spec: warp-modes R2, R21. The two modes work in the spectral blocks
    // R21 records: Complex in 240 ms blocks 10 ms apart, Complex Pro in
    // 360 ms blocks 10 ms apart.
    #[test]
    fn the_two_modes_take_the_blocks_the_measurements_chose() {
        let complex = Complex::new(1, 48_000.0, false);
        let pro = Complex::new(1, 48_000.0, true);
        assert_eq!(
            (complex.block_samples(), complex.interval_samples()),
            (11_520, 480)
        );
        assert_eq!((pro.block_samples(), pro.interval_samples()), (17_280, 480));
        assert!(pro.block_samples() > complex.block_samples());
    }

    // Spec: warp-modes R7, R24. A start, a locate and a jump prime the mode
    // afresh, but a wrap of the clip's own loop brace doesn't restart it: it
    // goes on with the audio from the brace's start as though it followed the
    // brace's end.
    #[test]
    fn a_loop_brace_wrap_goes_on_without_priming() {
        let file: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut mode = Complex::new(1, 48_000.0, false);
        // The clip plays from file frame 40,000 for 480 song frames.
        run(&mut mode, &file, 40_000.0, 1_000, 480, 7);
        assert_eq!(mode.primes, 1, "the clip's first run primes once");
        // The loop brace wraps: the file jumps back to 10,000 while the song
        // frame runs on.
        run(&mut mode, &file, 10_000.0, 1_480, 480, 7);
        assert_eq!(mode.primes, 1, "the loop brace's wrap primed afresh");
        // A locate jumps the song frame, so the mode starts afresh there.
        run(&mut mode, &file, 30_000.0, 20_000, 480, 7);
        assert_eq!(mode.primes, 2, "a locate must prime");
        // Another clip taking the instance over primes too.
        run(&mut mode, &file, 30_000.0, 20_480, 480, 8);
        assert_eq!(mode.primes, 3, "another clip must prime");
    }
}
