//! Loops: the span of a sample that repeats, so a sustained sound can hold
//! past the note that made it.
//!
//! The autosampler (the multisampler spec, R71) finds one in each note it
//! samples. A loop's two ends sit on rising zero crossings of the left
//! channel, so the wrap from the end back to the start is silent, and the
//! 10 ms before the end matches the 10 ms before the start, so the loop holds
//! the pitch rather than jumping. A sample whose best pair of ends doesn't
//! match within −40 dB has no loop, and the autosampler says so.

/// A loop in a sample: the span that repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loop {
    /// Where the loop begins, in frames.
    pub start: u64,
    /// Where it ends, in frames: the sound goes on from here to the start.
    pub end: u64,
}

/// The window compared at each end, in milliseconds.
const WINDOW_MS: f64 = 10.0;

/// How close the two windows must be, in decibels, for a loop to be found.
const MATCH_DB: f64 = -40.0;

/// The most ends one sample's search looks at.
const MAX_ENDS: usize = 256;

/// The rate the windows are compared at, in hertz: above the pitch of any
/// sound a sample holds, so the comparison sees the waveform and not only
/// its shape.
const COMPARE_RATE: u32 = 24_000;

/// Finds the loop in `left`, the left channel of a note's sample, or `None`
/// when it has none.
///
/// `note_frames` is how many frames the note was held for: the loop lies
/// between a quarter of it and nine tenths of it, inside the held part.
#[must_use]
pub fn find_loop(left: &[f32], rate: u32, note_frames: u64) -> Option<Loop> {
    let window = ((WINDOW_MS * f64::from(rate)) / 1000.0).round().max(1.0) as usize;
    let low = note_frames / 4;
    let high = note_frames * 9 / 10;
    if high <= low || low as usize + window >= left.len() {
        return None;
    }
    let ends = crossings(left, low, high);
    if ends.len() < 2 {
        return None;
    }
    let stride = (rate / COMPARE_RATE).max(1) as usize;
    let mut best: Option<(f64, Loop)> = None;
    for (index, &start) in ends.iter().enumerate() {
        let start = start as usize;
        if start < window {
            continue;
        }
        for &end in &ends[index + 1..] {
            let end = end as usize;
            if end < window {
                continue;
            }
            let score = difference(left, start, end, window, stride);
            if best.as_ref().is_none_or(|(worst, _)| score < *worst) {
                best = Some((
                    score,
                    Loop {
                        start: start as u64,
                        end: end as u64,
                    },
                ));
            }
        }
    }
    let (score, found) = best?;
    (score <= MATCH_DB).then_some(found)
}

/// The frames in `low..=high` where the signal crosses zero upwards.
fn crossings(left: &[f32], low: u64, high: u64) -> Vec<u64> {
    let low = low as usize;
    let high = (high as usize).min(left.len().saturating_sub(1));
    let mut found: Vec<u64> = Vec::new();
    for frame in (low + 1)..=high {
        if left[frame - 1] <= 0.0 && left[frame] > 0.0 {
            found.push(frame as u64);
        }
    }
    // A bright sound crosses zero thousands of times a second: the search
    // looks at a few of them, spread evenly.
    if found.len() > MAX_ENDS {
        let step = found.len().div_ceil(MAX_ENDS);
        found = found.into_iter().step_by(step).take(MAX_ENDS).collect();
    }
    found
}

/// How far the 10 ms before `end` is from the 10 ms before `start`, in
/// decibels below the louder of the two.
fn difference(left: &[f32], start: usize, end: usize, window: usize, stride: usize) -> f64 {
    let mut difference = 0.0;
    let mut before_start = 0.0;
    let mut before_end = 0.0;
    let mut count = 0.0;
    for offset in (0..window).step_by(stride) {
        let a = f64::from(left[start - window + offset]);
        let b = f64::from(left[end - window + offset]);
        difference += (b - a) * (b - a);
        before_start += a * a;
        before_end += b * b;
        count += 1.0;
    }
    if count == 0.0 {
        return f64::INFINITY;
    }
    let level = (before_start.max(before_end) / count).sqrt();
    if level <= 0.0 {
        return f64::INFINITY;
    }
    20.0 * ((difference / count).sqrt() / level).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine of `hz` cycles a second, `seconds` long, at `rate`.
    fn tone(hz: f32, seconds: f64, rate: u32) -> Vec<f32> {
        let frames = (f64::from(rate) * seconds) as usize;
        (0..frames)
            .map(|frame| {
                let t = frame as f64 / f64::from(rate);
                (2.0 * std::f64::consts::PI * f64::from(hz) * t).sin() as f32
            })
            .collect()
    }

    #[test]
    fn a_sustained_tone_gets_a_loop_without_a_click() {
        let rate = 48_000;
        let hz = 220.0;
        let samples = tone(hz, 2.0, rate);
        let found = find_loop(&samples, rate, samples.len() as u64).expect("a tone holds");
        // The loop lies inside the held part, between a quarter of it and
        // nine tenths of it.
        assert!(found.start >= samples.len() as u64 / 4);
        assert!(found.end <= samples.len() as u64 * 9 / 10);
        assert!(found.end > found.start);
        // Its length is a whole number of the tone's periods, so the loop
        // holds the pitch.
        let period = f64::from(rate) / f64::from(hz);
        let length = (found.end - found.start) as f64;
        let periods = length / period;
        assert!(
            (periods - periods.round()).abs() < 0.05,
            "the loop holds {periods} periods"
        );
        // Both ends are silent crossings, so the wrap from the end back to
        // the start doesn't click.
        let wrap = (samples[found.end as usize - 1] - samples[found.start as usize]).abs();
        assert!(wrap < 0.05, "the wrap jumps {wrap}");
    }

    #[test]
    fn a_note_that_fades_away_has_no_loop() {
        let rate = 48_000;
        let mut samples = tone(220.0, 2.0, rate);
        let length = samples.len() as f32;
        for (frame, sample) in samples.iter_mut().enumerate() {
            let fade = 1.0 - frame as f32 / length;
            *sample *= fade;
        }
        assert!(find_loop(&samples, rate, samples.len() as u64).is_none());
    }

    #[test]
    fn silence_has_no_loop() {
        let rate = 48_000;
        let samples = vec![0.0f32; 96_000];
        assert!(find_loop(&samples, rate, 96_000).is_none());
    }
}
