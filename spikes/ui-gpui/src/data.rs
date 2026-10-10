//! The benchmark scene's data, generated deterministically from a fixed seed.
//!
//! The generator is specified in `docs/spikes/ui-gpui.md` ("The scene's data")
//! so another spike can reproduce it. Nothing here depends on GPUI.

use std::ops::Range;
use std::sync::Arc;

/// The seed every run uses.
pub const SEED: u64 = 0x0BEA_71A7_E202_6A08;
pub const TRACKS: usize = 200;
pub const CLIPS_PER_TRACK: usize = 25;
pub const CLIPS: usize = TRACKS * CLIPS_PER_TRACK;
pub const BARS: u32 = 512;
pub const BPM: f64 = 120.0;
pub const BEATS_PER_BAR: u32 = 4;
pub const SAMPLE_RATE: u32 = 48_000;
pub const SAMPLES_PER_PEAK: u32 = 256;
/// 4 beats at 120 BPM is 2 seconds, so 96,000 samples at 48 kHz.
pub const SAMPLES_PER_BAR: u64 = (SAMPLE_RATE as u64) * (BEATS_PER_BAR as u64) * 60 / (BPM as u64);
pub const SAMPLES_PER_BEAT: u64 = SAMPLES_PER_BAR / BEATS_PER_BAR as u64;
/// Mip levels of the peak pyramid: 256, 1,024, 4,096, 16,384, and 65,536
/// samples per peak.
pub const PEAK_LEVELS: usize = 5;
pub const BROWSER_FILES: usize = 100_000;

/// SplitMix64 (Steele, Lea, and Flood, 2014): tiny, fast, and easy to
/// reproduce in another program.
#[derive(Clone, Debug)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform value in [0, 1) with 53 bits of precision.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// A uniform integer in [0, n), by multiply-shift (no modulo bias worth
    /// worrying about at these sizes).
    pub fn below(&mut self, n: u64) -> u64 {
        ((self.next_u64() as u128 * n as u128) >> 64) as u64
    }
}

const TRACK_WORDS: &[&str] = &[
    "Drums", "Kick", "Snare", "Hats", "Perc", "Bass", "Sub", "Keys", "Piano", "Pad", "Lead",
    "Pluck", "Strings", "Brass", "Vox", "Choir", "FX", "Guitar", "Organ", "Arp",
];

const CLIP_WORDS: &[&str] = &[
    "Groove", "Fill", "Loop", "Break", "Riff", "Hook", "Verse", "Chorus", "Bridge", "Intro",
    "Outro", "Drop", "Build", "Phrase", "Motif", "Take", "Comp", "Stem", "Swell", "Hit",
];

const BROWSER_DIRS: &[&str] = &[
    "Drums",
    "Bass",
    "Synth",
    "Vocals",
    "Guitar",
    "Keys",
    "FX",
    "Loops",
    "One-shots",
    "Textures",
];

const BROWSER_WORDS: &[&str] = &[
    "warm", "dusty", "bright", "dark", "tight", "wide", "deep", "crisp", "lofi", "glassy",
    "punchy", "airy", "gritty", "smooth", "metallic", "wooden", "analog", "digital", "tape",
    "vinyl",
];

/// Track colours, as 0xRRGGBB.
pub const PALETTE: &[u32] = &[
    0xE5_6B_6F, 0xE9_8A_4B, 0xE8_C1_4F, 0x9C_CC_65, 0x4F_BF_8F, 0x4F_B3_C9, 0x5F_8F_E8, 0x8C_7A_E6,
    0xC0_74_D6, 0xE0_6F_A8,
];

#[derive(Clone, Debug)]
pub struct Track {
    pub name: Arc<str>,
    pub color: u32,
    pub mute: bool,
    pub solo: bool,
    /// The knob, 0 to 1.
    pub volume: f32,
}

/// The synthetic signal a clip's peaks come from.
#[derive(Clone, Copy, Debug)]
pub struct Voice {
    /// Three partials: frequency in Hz, amplitude, and starting phase.
    pub partials: [(f64, f64, f64); 3],
    /// Decay time constant of the per-beat envelope, in seconds.
    pub decay: f64,
}

#[derive(Clone, Debug)]
pub struct Clip {
    pub track: usize,
    /// Position in bars from the song start. Whole bars when generated; a
    /// drag snaps to a sixteenth of a bar.
    pub start: f64,
    pub bars: u32,
    pub name: Arc<str>,
    pub voice: Voice,
}

impl Clip {
    pub fn end(&self) -> f64 {
        self.start + self.bars as f64
    }
}

/// Min/max peaks for one clip, as a pyramid. `levels[0]` has one peak per
/// 256 samples; each level above it has a quarter as many.
#[derive(Clone, Debug, Default)]
pub struct Peaks {
    pub levels: Vec<Vec<[i16; 2]>>,
}

impl Peaks {
    pub fn samples_per_peak(level: usize) -> u64 {
        (SAMPLES_PER_PEAK as u64) << (2 * level)
    }

    /// The coarsest level whose peaks are no wider than `samples_per_column`,
    /// so each drawn column covers at least one whole peak.
    pub fn level_for(samples_per_column: f64) -> usize {
        let mut level = 0;
        while level + 1 < PEAK_LEVELS
            && Self::samples_per_peak(level + 1) as f64 <= samples_per_column
        {
            level += 1;
        }
        level
    }

    /// The min and max over a range of samples, read from `level`. Returns
    /// `None` when the range is outside the clip.
    pub fn min_max(&self, level: usize, samples: Range<u64>) -> Option<(i16, i16)> {
        let peaks = &self.levels[level];
        let spp = Self::samples_per_peak(level);
        let first = (samples.start / spp) as usize;
        let last = (samples.end.div_ceil(spp) as usize).min(peaks.len());
        if first >= last {
            return None;
        }
        let mut lo = i16::MAX;
        let mut hi = i16::MIN;
        for p in &peaks[first..last] {
            lo = lo.min(p[0]);
            hi = hi.max(p[1]);
        }
        Some((lo, hi))
    }
}

pub struct Project {
    pub tracks: Vec<Track>,
    pub clips: Vec<Clip>,
    /// Indexed like `clips`. Peaks belong to the clip's media, so a drag
    /// never touches them.
    pub peaks: Vec<Peaks>,
    /// For each track, its clip indices sorted by start.
    pub lanes: Vec<Vec<usize>>,
}

impl Project {
    /// Generates the scene. Peaks are computed on every available core; the
    /// result doesn't depend on the thread count.
    pub fn generate(seed: u64) -> Self {
        let mut rng = SplitMix64::new(seed);
        let tracks: Vec<Track> = (0..TRACKS)
            .map(|t| Track {
                name: format!(
                    "{} {}",
                    t + 1,
                    TRACK_WORDS[rng.below(TRACK_WORDS.len() as u64) as usize]
                )
                .into(),
                color: PALETTE[rng.below(PALETTE.len() as u64) as usize],
                mute: false,
                solo: false,
                volume: 0.75,
            })
            .collect();

        // Each track gets 25 clips, one per slot of 512 / 25 bars, so clips on a
        // track never overlap and spread over the whole song.
        let slot = BARS as f64 / CLIPS_PER_TRACK as f64;
        let mut clips = Vec::with_capacity(CLIPS);
        for track in 0..TRACKS {
            for k in 0..CLIPS_PER_TRACK {
                let bars = 1 + rng.below(8) as u32;
                let start = (k as f64 * slot + rng.next_f64() * (slot - bars as f64)).floor();
                let name: Arc<str> = format!(
                    "{} {}",
                    CLIP_WORDS[rng.below(CLIP_WORDS.len() as u64) as usize],
                    clips.len() + 1
                )
                .into();
                let mut partials = [(0.0, 0.0, 0.0); 3];
                for p in &mut partials {
                    // 55 Hz to 880 Hz, log-uniform.
                    let freq = 55.0 * 2f64.powf(rng.next_f64() * 4.0);
                    let amp = 0.2 + 0.8 * rng.next_f64();
                    let phase = rng.next_f64() * std::f64::consts::TAU;
                    *p = (freq, amp, phase);
                }
                let decay = 0.05 + 0.4 * rng.next_f64();
                clips.push(Clip {
                    track,
                    start,
                    bars,
                    name,
                    voice: Voice { partials, decay },
                });
            }
        }

        let peaks = compute_all_peaks(&clips);
        let mut project = Self {
            tracks,
            clips,
            peaks,
            lanes: vec![Vec::new(); TRACKS],
        };
        project.rebuild_lanes();
        project
    }

    pub fn rebuild_lanes(&mut self) {
        for lane in &mut self.lanes {
            lane.clear();
        }
        for (i, clip) in self.clips.iter().enumerate() {
            self.lanes[clip.track].push(i);
        }
        for lane in &mut self.lanes {
            lane.sort_by(|&a, &b| self.clips[a].start.total_cmp(&self.clips[b].start));
        }
    }

    /// Clip indices on `track` that overlap the bar range, in start order.
    /// Clips on a lane don't overlap, so their ends are sorted too.
    pub fn clips_in(&self, track: usize, bars: Range<f64>) -> impl Iterator<Item = usize> + '_ {
        let lane = &self.lanes[track];
        let first = lane.partition_point(|&c| self.clips[c].end() <= bars.start);
        lane[first..]
            .iter()
            .copied()
            .take_while(move |&c| self.clips[c].start < bars.end)
    }

    /// The topmost clip under a point, given in bars and a track index.
    pub fn hit_test(&self, track: usize, bar: f64) -> Option<usize> {
        if track >= self.tracks.len() {
            return None;
        }
        self.clips_in(track, bar..bar + f64::EPSILON).last()
    }

    /// Moves a clip, as a drag ends. Returns false when the target overlaps
    /// another clip on the destination lane, in which case nothing changes.
    pub fn move_clip(&mut self, clip: usize, track: usize, start: f64) -> bool {
        let bars = self.clips[clip].bars as f64;
        let start = start.clamp(0.0, BARS as f64 - bars);
        let track = track.min(self.tracks.len() - 1);
        let overlaps = self
            .clips_in(track, start..start + bars)
            .any(|other| other != clip);
        if overlaps {
            return false;
        }
        self.clips[clip].track = track;
        self.clips[clip].start = start;
        self.rebuild_lanes();
        true
    }
}

/// Computes every clip's peak pyramid, spreading clips over the cores.
pub fn compute_all_peaks(clips: &[Clip]) -> Vec<Peaks> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = clips.len().div_ceil(threads).max(1);
    let mut out = vec![Peaks::default(); clips.len()];
    std::thread::scope(|scope| {
        for (clips, out) in clips.chunks(chunk).zip(out.chunks_mut(chunk)) {
            scope.spawn(move || {
                for (clip, peaks) in clips.iter().zip(out.iter_mut()) {
                    *peaks = compute_peaks(clip);
                }
            });
        }
    });
    out
}

/// Synthesises a clip's signal at 48 kHz and reduces it to min/max peaks.
///
/// The signal is three sines under an envelope that restarts on every beat:
/// a 5 ms linear attack, then an exponential decay. Each 256-sample window
/// starts from exactly computed phases and envelope, then advances them by
/// rotation and multiplication, so rounding error never accumulates past
/// one window.
pub fn compute_peaks(clip: &Clip) -> Peaks {
    let samples = clip.bars as u64 * SAMPLES_PER_BAR;
    let windows = samples.div_ceil(SAMPLES_PER_PEAK as u64) as usize;
    let sr = SAMPLE_RATE as f64;
    let attack = (0.005 * sr) as u64;
    let norm: f64 = clip.voice.partials.iter().map(|p| p.1).sum();
    let decay_per_sample = (-1.0 / (clip.voice.decay * sr)).exp() as f32;
    let steps: Vec<(f32, f32)> = clip
        .voice
        .partials
        .iter()
        .map(|&(f, _, _)| {
            let w = std::f64::consts::TAU * f / sr;
            (w.cos() as f32, w.sin() as f32)
        })
        .collect();

    let mut level0 = Vec::with_capacity(windows);
    for w in 0..windows as u64 {
        let n0 = w * SAMPLES_PER_PEAK as u64;
        let n1 = (n0 + SAMPLES_PER_PEAK as u64).min(samples);
        // Exact phasors and envelope at the window start.
        let mut ph = [(0f32, 0f32); 3];
        for (i, &(f, a, phase)) in clip.voice.partials.iter().enumerate() {
            let angle = std::f64::consts::TAU * f * n0 as f64 / sr + phase;
            let a = (a / norm) as f32;
            ph[i] = (a * angle.cos() as f32, a * angle.sin() as f32);
        }
        let beat_pos = n0 % SAMPLES_PER_BEAT;
        let mut env = envelope(beat_pos, attack, clip.voice.decay, sr);
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for n in n0..n1 {
            let pos = n % SAMPLES_PER_BEAT;
            if pos <= attack {
                env = envelope(pos, attack, clip.voice.decay, sr);
            } else {
                env *= decay_per_sample;
            }
            let s = (ph[0].1 + ph[1].1 + ph[2].1) * env;
            lo = lo.min(s);
            hi = hi.max(s);
            for (p, &(c, s)) in ph.iter_mut().zip(&steps) {
                *p = (p.0 * c - p.1 * s, p.0 * s + p.1 * c);
            }
        }
        level0.push([to_i16(lo), to_i16(hi)]);
    }

    let mut levels = vec![level0];
    while levels.len() < PEAK_LEVELS {
        let below = levels.last().expect("level 0 exists");
        let next: Vec<[i16; 2]> = below
            .chunks(4)
            .map(|c| {
                c.iter().fold([i16::MAX, i16::MIN], |acc, p| {
                    [acc[0].min(p[0]), acc[1].max(p[1])]
                })
            })
            .collect();
        levels.push(next);
    }
    Peaks { levels }
}

fn envelope(beat_pos: u64, attack: u64, decay: f64, sr: f64) -> f32 {
    if beat_pos < attack {
        beat_pos as f32 / attack as f32
    } else {
        (-((beat_pos - attack) as f64) / (decay * sr)).exp() as f32
    }
}

fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

/// The browser's file name at `index`, generated on demand so 100,000 names
/// cost nothing until they're drawn.
pub fn browser_file_name(index: usize) -> String {
    let mut rng = SplitMix64::new(SEED ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let dir = BROWSER_DIRS[rng.below(BROWSER_DIRS.len() as u64) as usize];
    let a = BROWSER_WORDS[rng.below(BROWSER_WORDS.len() as u64) as usize];
    let b = BROWSER_WORDS[rng.below(BROWSER_WORDS.len() as u64) as usize];
    let bpm = 80 + rng.below(80);
    format!("{dir}/{a} {b} {bpm} bpm {index:06}.wav")
}

/// A synthetic meter level for a track at a time, 0 to 1. Deterministic, so a
/// bench run draws the same meters every time.
pub fn meter_level(track: usize, seconds: f64) -> f32 {
    let t = seconds + track as f64 * 0.137;
    let beat = (t * BPM / 60.0).fract();
    let pulse = (-beat * 6.0).exp();
    let wobble = 0.5 + 0.5 * (t * (0.7 + (track % 7) as f64 * 0.31)).sin();
    (0.15 + 0.55 * pulse + 0.3 * wobble * pulse.sqrt()).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_matches_reference_values() {
        // Reference output for seed 0, from the published algorithm.
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }

    #[test]
    fn layout_is_deterministic_and_spread() {
        let a = Project::generate(SEED);
        let b = Project::generate(SEED);
        assert_eq!(a.clips.len(), CLIPS);
        assert_eq!(a.tracks.len(), TRACKS);
        for (x, y) in a.clips.iter().zip(&b.clips) {
            assert_eq!(
                (x.track, x.start, x.bars, &x.name),
                (y.track, y.start, y.bars, &y.name)
            );
        }
        assert_eq!(a.peaks[1234].levels[0], b.peaks[1234].levels[0]);
        for clip in &a.clips {
            assert!((1..=8).contains(&clip.bars));
            assert!(clip.start >= 0.0 && clip.end() <= BARS as f64);
        }
        // No overlaps on any lane.
        for lane in &a.lanes {
            for pair in lane.windows(2) {
                assert!(a.clips[pair[0]].end() <= a.clips[pair[1]].start);
            }
        }
    }

    #[test]
    fn peaks_cover_the_clip_and_stay_in_range() {
        let p = Project::generate(SEED);
        for (clip, peaks) in p.clips.iter().zip(&p.peaks).take(50) {
            let samples = clip.bars as u64 * SAMPLES_PER_BAR;
            assert_eq!(peaks.levels[0].len() as u64, samples.div_ceil(256));
            assert_eq!(peaks.levels.len(), PEAK_LEVELS);
            for pk in &peaks.levels[0] {
                assert!(pk[0] <= pk[1]);
            }
            // The coarse level agrees with level 0 over the whole clip.
            let fine = peaks.min_max(0, 0..samples).unwrap();
            let coarse = peaks.min_max(4, 0..samples).unwrap();
            assert_eq!(fine, coarse);
        }
    }

    #[test]
    fn level_choice_never_skips_detail() {
        assert_eq!(Peaks::level_for(100.0), 0);
        assert_eq!(Peaks::level_for(1023.0), 0);
        assert_eq!(Peaks::level_for(1024.0), 1);
        assert_eq!(Peaks::level_for(1e9), PEAK_LEVELS - 1);
    }

    #[test]
    fn hit_test_and_move() {
        let mut p = Project::generate(SEED);
        let c = p.lanes[3][2];
        let (start, bars) = (p.clips[c].start, p.clips[c].bars as f64);
        assert_eq!(p.hit_test(3, start + bars / 2.0), Some(c));
        // Moving onto another clip is refused.
        let other = p.lanes[4][5];
        assert!(!p.move_clip(c, 4, p.clips[other].start));
        // Moving into the space after the last clip on a lane succeeds.
        let last = *p.lanes[7].last().unwrap();
        let free = p.clips[last].end();
        if free + bars <= BARS as f64 {
            assert!(p.move_clip(c, 7, free));
            assert_eq!(p.clips[c].track, 7);
            assert_eq!(p.hit_test(7, free + 0.5), Some(c));
        }
    }

    #[test]
    fn browser_names_are_stable() {
        assert_eq!(browser_file_name(42), browser_file_name(42));
        assert_ne!(browser_file_name(42), browser_file_name(43));
    }
}
