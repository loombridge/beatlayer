//! The benchmark scene: deterministic data, the same shape in both UI spikes.
//!
//! Everything comes from one SplitMix64 stream seeded with [`SEED`], so two
//! runs (and two toolkits) draw the same tracks, clips, and waveforms. The
//! algorithm is written out in `docs/spikes/ui-egui.md` so the GPUI spike can
//! check it generates the same thing.

use std::f64::consts::TAU;

/// Seed for every random choice in the scene.
pub const SEED: u64 = 0x0008_B1A7_5EED;

pub const TRACKS: usize = 200;
pub const CLIPS: usize = 5_000;
pub const BARS: u32 = 512;
pub const BEATS_PER_BAR: u32 = 4;
pub const BPM: u32 = 120;
pub const SAMPLE_RATE: u32 = 48_000;
pub const SAMPLES_PER_PEAK: u32 = 256;
/// 60 / 120 × 48,000.
pub const SAMPLES_PER_BEAT: u32 = SAMPLE_RATE * 60 / BPM;
/// Level 0 is 256 samples per peak; each level above merges four peaks.
pub const MIP_LEVELS: usize = 6;
pub const MIP_FACTOR: u32 = 4;
pub const BROWSER_FILES: usize = 100_000;

/// The size of a scene. [`Config::BENCH`] is the benchmark; tests use smaller ones.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub tracks: usize,
    pub clips_per_track: usize,
    pub bars: u32,
    pub files: usize,
}

impl Config {
    pub const BENCH: Self = Self {
        tracks: TRACKS,
        clips_per_track: CLIPS / TRACKS,
        bars: BARS,
        files: BROWSER_FILES,
    };

    pub fn total_beats(&self) -> u32 {
        self.bars * BEATS_PER_BAR
    }
}

/// SplitMix64: tiny, fast, and easy to reproduce in any language.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
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

    /// A uniform integer in `0..n`.
    pub fn below(&mut self, n: u32) -> u32 {
        (((self.next_u64() >> 32) * u64::from(n)) >> 32) as u32
    }

    /// A uniform float in `[0, 1)` with 53 bits of precision.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A run of min/max peaks inside [`Scene::peaks`], counted in peaks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeakRun {
    pub offset: u32,
    pub len: u32,
}

/// The synthetic signal behind one clip's waveform.
#[derive(Clone, Copy, Debug)]
pub struct Signal {
    /// Three sine partials: frequency in Hz, amplitude, and phase in radians.
    pub partials: [(f64, f64, f64); 3],
    /// Per-beat exponential decay rate, per second.
    pub decay: f64,
}

#[derive(Clone, Debug)]
pub struct Clip {
    pub name: String,
    pub start_beat: u32,
    pub len_beats: u32,
    pub color: [u8; 3],
    pub signal: Signal,
    /// One run per mip level; level 0 is 256 samples per peak.
    pub levels: [PeakRun; MIP_LEVELS],
}

impl Clip {
    pub fn end_beat(&self) -> u32 {
        self.start_beat + self.len_beats
    }

    pub fn len_samples(&self) -> u64 {
        u64::from(self.len_beats) * u64::from(SAMPLES_PER_BEAT)
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    pub name: String,
    pub color: [u8; 3],
    /// Clip ids, sorted by start beat.
    pub clips: Vec<u32>,
    pub muted: bool,
    pub soloed: bool,
    /// The knob's value, 0 to 1.
    pub gain: f32,
}

pub struct Scene {
    pub config: Config,
    pub tracks: Vec<Track>,
    pub clips: Vec<Clip>,
    /// Interleaved `[min, max]` pairs, each an `i8` stored as its bits.
    pub peaks: Vec<u8>,
    pub files: Vec<String>,
}

const PALETTE: [[u8; 3]; 12] = [
    [0xE0, 0x6C, 0x4C],
    [0xE8, 0xA2, 0x3C],
    [0xD9, 0xCB, 0x4A],
    [0x9B, 0xC9, 0x4E],
    [0x52, 0xB7, 0x6A],
    [0x3F, 0xB5, 0xA8],
    [0x45, 0xA4, 0xD8],
    [0x5E, 0x7F, 0xE0],
    [0x8C, 0x6C, 0xE0],
    [0xC2, 0x63, 0xC9],
    [0xE0, 0x5E, 0x9A],
    [0xA8, 0x8B, 0x6E],
];

const TRACK_WORDS: [&str; 12] = [
    "Drums", "Bass", "Keys", "Pad", "Lead", "Vox", "Perc", "Gtr", "Strings", "Brass", "FX", "Sub",
];

const CLIP_WORDS: [&str; 16] = [
    "Kick", "Snare", "Hats", "Bass", "Pad", "Lead", "Vox", "Perc", "Chord", "Arp", "Riser", "Sub",
    "Keys", "Gtr", "Str", "Fill",
];

const FILE_ADJECTIVES: [&str; 16] = [
    "Dusty", "Bright", "Warm", "Dark", "Crisp", "Wide", "Tight", "Loose", "Deep", "Airy", "Gritty",
    "Smooth", "Punchy", "Soft", "Hard", "Lo-fi",
];

const FILE_NOUNS: [&str; 16] = [
    "Kick", "Snare", "Clap", "Hat", "Ride", "Tom", "Bass", "Pluck", "Pad", "Stab", "Chord",
    "Vocal", "Loop", "Fill", "Noise", "Texture",
];

impl Scene {
    /// Generates the scene. Peaks are computed on every core; the result
    /// doesn't depend on the number of threads.
    pub fn generate(config: Config) -> Self {
        let mut scene = Self::layout(config);
        scene.compute_peaks();
        scene
    }

    /// Tracks, clips, names, and file names, without peak data.
    pub fn layout(config: Config) -> Self {
        let mut rng = Rng::new(SEED);
        let mut tracks = Vec::with_capacity(config.tracks);
        let mut clips = Vec::with_capacity(config.tracks * config.clips_per_track);

        for t in 0..config.tracks {
            let word = TRACK_WORDS[rng.below(TRACK_WORDS.len() as u32) as usize];
            let color = PALETTE[rng.below(PALETTE.len() as u32) as usize];
            let name = format!("{word} {}", t + 1);

            // Lengths of 1 to 8 bars, then the free bars shared out as gaps
            // before each clip (the last share is left after the last clip).
            let n = config.clips_per_track;
            let lengths: Vec<u32> = (0..n).map(|_| 1 + rng.below(8)).collect();
            let used: u32 = lengths.iter().sum();
            let free = config.bars.saturating_sub(used);
            let weights: Vec<f64> = (0..=n).map(|_| rng.unit()).collect();
            let weight_sum: f64 = weights.iter().sum();

            let mut bar = 0u32;
            let mut ids = Vec::with_capacity(n);
            for (i, &len_bars) in lengths.iter().enumerate() {
                bar += (f64::from(free) * weights[i] / weight_sum).floor() as u32;
                let clip_word = CLIP_WORDS[rng.below(CLIP_WORDS.len() as u32) as usize];
                let clip_color = PALETTE[rng.below(PALETTE.len() as u32) as usize];
                let signal = Signal::random(&mut rng);
                ids.push(clips.len() as u32);
                clips.push(Clip {
                    name: format!("{clip_word} {}", clips.len() + 1),
                    start_beat: bar * BEATS_PER_BAR,
                    len_beats: len_bars * BEATS_PER_BAR,
                    color: clip_color,
                    signal,
                    levels: [PeakRun::default(); MIP_LEVELS],
                });
                bar += len_bars;
            }

            tracks.push(Track {
                name,
                color,
                clips: ids,
                muted: false,
                soloed: false,
                gain: 0.7,
            });
        }

        let mut file_rng = Rng::new(SEED ^ 0xF11E_5EED);
        let files = (0..config.files)
            .map(|i| {
                let adjective = FILE_ADJECTIVES[file_rng.below(16) as usize];
                let noun = FILE_NOUNS[file_rng.below(16) as usize];
                format!("{adjective} {noun} {:05}.wav", i + 1)
            })
            .collect();

        Self {
            config,
            tracks,
            clips,
            peaks: Vec::new(),
            files,
        }
    }

    /// Computes every clip's peaks and mip levels from its signal.
    pub fn compute_peaks(&mut self) {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let chunk = self.clips.len().div_ceil(threads).max(1);

        let per_chunk: Vec<Vec<Vec<[i8; 2]>>> = std::thread::scope(|s| {
            let handles: Vec<_> = self
                .clips
                .chunks(chunk)
                .map(|clips| {
                    s.spawn(move || {
                        clips
                            .iter()
                            .map(|c| level0_peaks(&c.signal, c.len_samples()))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("peak thread panicked"))
                .collect()
        });

        let mut peaks: Vec<u8> = Vec::new();
        let mut clip_index = 0;
        for chunk in per_chunk {
            for level0 in chunk {
                let clip = &mut self.clips[clip_index];
                let mut level = level0;
                for k in 0..MIP_LEVELS {
                    clip.levels[k] = PeakRun {
                        offset: (peaks.len() / 2) as u32,
                        len: level.len() as u32,
                    };
                    for [lo, hi] in &level {
                        peaks.push(*lo as u8);
                        peaks.push(*hi as u8);
                    }
                    level = reduce(&level);
                }
                clip_index += 1;
            }
        }
        // Pad to whole u32 words for the GPU storage buffer.
        while !peaks.len().is_multiple_of(4) {
            peaks.push(0);
        }
        self.peaks = peaks;
    }

    /// The min and max of one peak, in -1 to 1.
    pub fn peak(&self, index: u32) -> (f32, f32) {
        let i = index as usize * 2;
        (
            f32::from(self.peaks[i] as i8) / 127.0,
            f32::from(self.peaks[i + 1] as i8) / 127.0,
        )
    }

    /// A 64-bit fingerprint of the layout and peaks, for comparing spikes.
    pub fn fingerprint(&self) -> u64 {
        let mut h: u64 = 0xCBF2_9CE4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for b in bytes {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01B3);
            }
        };
        for track in &self.tracks {
            eat(track.name.as_bytes());
            for &id in &track.clips {
                let clip = &self.clips[id as usize];
                eat(clip.name.as_bytes());
                eat(&clip.start_beat.to_le_bytes());
                eat(&clip.len_beats.to_le_bytes());
            }
        }
        eat(&self.peaks);
        h
    }

    /// Moves a clip to a track and start beat, keeping the track's list sorted.
    pub fn move_clip(&mut self, clip: u32, from_track: usize, to_track: usize, start_beat: u32) {
        self.tracks[from_track].clips.retain(|&c| c != clip);
        self.clips[clip as usize].start_beat = start_beat;
        let clips = &self.clips;
        let list = &mut self.tracks[to_track].clips;
        let at = list.partition_point(|&c| clips[c as usize].start_beat <= start_beat);
        list.insert(at, clip);
    }

    /// The longest clip, in beats, which bounds how far back a visibility
    /// search must look.
    pub fn max_clip_beats(&self) -> u32 {
        8 * BEATS_PER_BAR
    }
}

impl Signal {
    fn random(rng: &mut Rng) -> Self {
        let mut partials = [(0.0, 0.0, 0.0); 3];
        let mut amp_sum = 0.0;
        for p in &mut partials {
            let freq = 40.0 * 2f64.powf(4.0 * rng.unit()); // 40 to 640 Hz
            let amp = 0.2 + 0.8 * rng.unit();
            let phase = TAU * rng.unit();
            *p = (freq, amp, phase);
            amp_sum += amp;
        }
        for p in &mut partials {
            p.1 *= 0.9 / amp_sum;
        }
        let decay = 3.0 + 12.0 * rng.unit();
        Self { partials, decay }
    }
}

/// Level-0 peaks for a signal of `len` samples: the min and max of every
/// 256-sample window, computed from every sample.
///
/// The signal is `env(n) × Σ aₖ sin(2π fₖ n / 48000 + φₖ)`, where
/// `env(n) = exp(−decay × (n mod 24000) / 48000) × (0.55 + 0.45 sin(π n / len))`.
/// Phases are recomputed exactly at the start of every window, so the
/// per-sample rotation never drifts; the slow swell term is held per window.
pub fn level0_peaks(signal: &Signal, len: u64) -> Vec<[i8; 2]> {
    let windows = len.div_ceil(u64::from(SAMPLES_PER_PEAK)) as usize;
    let mut out = Vec::with_capacity(windows);
    let sr = f64::from(SAMPLE_RATE);
    let spb = u64::from(SAMPLES_PER_BEAT);
    let step = (-signal.decay / sr).exp() as f32;
    let rot: [(f32, f32); 3] = signal
        .partials
        .map(|(f, _, _)| ((TAU * f / sr).cos() as f32, (TAU * f / sr).sin() as f32));

    for w in 0..windows {
        let n0 = w as u64 * u64::from(SAMPLES_PER_PEAK);
        let n1 = (n0 + u64::from(SAMPLES_PER_PEAK)).min(len);
        let swell = (0.55 + 0.45 * (std::f64::consts::PI * n0 as f64 / len as f64).sin()) as f32;

        // Phasor state for each partial at the window start: (cos, sin, amp).
        let mut ph: [(f32, f32, f32); 3] = signal.partials.map(|(f, a, phi)| {
            let theta = (TAU * f * n0 as f64 / sr + phi) % TAU;
            (theta.cos() as f32, theta.sin() as f32, a as f32)
        });
        let mut env = (-signal.decay * (n0 % spb) as f64 / sr).exp() as f32;
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for n in n0..n1 {
            if n % spb == 0 {
                env = 1.0;
            }
            let s = ph[0].1 * ph[0].2 + ph[1].1 * ph[1].2 + ph[2].1 * ph[2].2;
            let v = s * env * swell;
            lo = lo.min(v);
            hi = hi.max(v);
            for (k, p) in ph.iter_mut().enumerate() {
                let (c, s) = (p.0, p.1);
                p.0 = c * rot[k].0 - s * rot[k].1;
                p.1 = s * rot[k].0 + c * rot[k].1;
            }
            env *= step;
        }
        out.push([quantise(lo), quantise(hi)]);
    }
    out
}

fn quantise(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}

/// Merges groups of four peaks into one.
pub fn reduce(level: &[[i8; 2]]) -> Vec<[i8; 2]> {
    level
        .chunks(MIP_FACTOR as usize)
        .map(|g| {
            let lo = g.iter().map(|p| p[0]).min().unwrap_or(0);
            let hi = g.iter().map(|p| p[1]).max().unwrap_or(0);
            [lo, hi]
        })
        .collect()
}

/// A synthetic meter level, 0 to 1, for track `track` at time `t` seconds.
pub fn meter_level(track: usize, t: f64) -> f32 {
    let mut rng = Rng::new(SEED ^ (track as u64).wrapping_mul(0x9E37_79B9));
    let wobble = 0.3 + 1.7 * rng.unit();
    let phase = TAU * rng.unit();
    let offset = rng.unit();
    let beat = (t * f64::from(BPM) / 60.0 + offset).fract();
    let hit = (-beat * 5.0).exp();
    let level = 0.2 + 0.65 * hit * (0.65 + 0.35 * (TAU * wobble * t + phase).sin());
    level.clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: Config = Config {
        tracks: 6,
        clips_per_track: 5,
        bars: 64,
        files: 100,
    };

    #[test]
    fn bench_layout_matches_the_brief() {
        let scene = Scene::layout(Config::BENCH);
        assert_eq!(scene.tracks.len(), 200);
        assert_eq!(scene.clips.len(), 5_000);
        assert_eq!(scene.files.len(), 100_000);
        for track in &scene.tracks {
            let mut end = 0;
            for &id in &track.clips {
                let clip = &scene.clips[id as usize];
                assert!(clip.start_beat >= end, "clips overlap on {}", track.name);
                assert!((4..=32).contains(&clip.len_beats));
                end = clip.end_beat();
            }
            assert!(end <= BARS * BEATS_PER_BAR);
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let a = Scene::layout(Config::BENCH);
        let b = Scene::layout(Config::BENCH);
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.files[99_999], b.files[99_999]);
    }

    #[test]
    fn peaks_have_one_entry_per_256_samples_and_valid_mips() {
        let scene = Scene::generate(SMALL);
        for clip in &scene.clips {
            let expected = clip.len_samples().div_ceil(256) as u32;
            assert_eq!(clip.levels[0].len, expected);
            for k in 1..MIP_LEVELS {
                assert_eq!(clip.levels[k].len, clip.levels[k - 1].len.div_ceil(4));
            }
            // Every level-1 peak covers the four level-0 peaks under it.
            let l0 = clip.levels[0];
            let l1 = clip.levels[1];
            for i in 0..l1.len {
                let (lo, hi) = scene.peak(l1.offset + i);
                for j in i * 4..((i + 1) * 4).min(l0.len) {
                    let (a, b) = scene.peak(l0.offset + j);
                    assert!(lo <= a && b <= hi);
                }
            }
        }
        assert_eq!(scene.peaks.len() % 4, 0);
    }

    #[test]
    fn peak_generation_is_deterministic_across_thread_counts() {
        let a = Scene::generate(SMALL);
        // Recompute one clip directly and compare with the threaded result.
        let clip = &a.clips[7];
        let direct = level0_peaks(&clip.signal, clip.len_samples());
        for (i, p) in direct.iter().enumerate() {
            let (lo, hi) = a.peak(clip.levels[0].offset + i as u32);
            assert_eq!((lo, hi), (f32::from(p[0]) / 127.0, f32::from(p[1]) / 127.0));
        }
        assert_eq!(a.fingerprint(), Scene::generate(SMALL).fingerprint());
    }

    #[test]
    fn peaks_follow_the_envelope() {
        // The eight peaks after the second beat starts are louder than the
        // eight before it, because the envelope restarts on every beat.
        let scene = Scene::generate(SMALL);
        let loudness = |clip: &Clip, from: u32| {
            (from..from + 8)
                .map(|i| {
                    let (lo, hi) = scene.peak(clip.levels[0].offset + i);
                    lo.abs().max(hi.abs())
                })
                .fold(0.0, f32::max)
        };
        let beat = SAMPLES_PER_BEAT / SAMPLES_PER_PEAK; // 93.75, rounded down
        for clip in &scene.clips {
            let after = loudness(clip, beat + 1);
            let before = loudness(clip, beat - 8);
            assert!(after > before, "{}: {after} <= {before}", clip.name);
        }
    }

    #[test]
    fn move_clip_keeps_tracks_sorted() {
        let mut scene = Scene::layout(SMALL);
        let id = scene.tracks[0].clips[0];
        scene.move_clip(id, 0, 1, 200);
        assert!(!scene.tracks[0].clips.contains(&id));
        let starts: Vec<u32> = scene.tracks[1]
            .clips
            .iter()
            .map(|&c| scene.clips[c as usize].start_beat)
            .collect();
        assert!(starts.windows(2).all(|w| w[0] <= w[1]));
        assert!(scene.tracks[1].clips.contains(&id));
    }

    #[test]
    fn meter_levels_stay_in_range() {
        for track in 0..200 {
            for i in 0..100 {
                let level = meter_level(track, f64::from(i) * 0.013);
                assert!((0.0..=1.0).contains(&level));
            }
        }
    }
}
