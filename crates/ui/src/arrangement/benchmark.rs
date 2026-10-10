//! The Arrangement View's benchmark scene (ADR-001's, WP-0.8b's): 200
//! tracks of 25 audio clips each, 5,000 clips with waveforms, spread over
//! 512 bars, from one seeded random stream so every run draws the same
//! thing. The headless tests and `examples/arrangement_bench.rs` share it.
//!
//! Clips play media files the scene makes up: `media` short signals whose
//! peaks are computed from their samples by `bl-media`, exactly as a peak
//! file holds them, and kept in memory ([`MemoryPeaks`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_core::time::Ticks;
use bl_media::{Peaks, SampleData};
use bl_model::{AudioData, Clip, ClipData, Command, ContentHash, Editor, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::SetStore;
use bl_shell::settings::Preferences;
use serde_json::json;

use super::peaks::MemoryPeaks;

/// The scene's seed, as the UI spikes used.
pub const SEED: u64 = 0x0008_B1A7_5EED;

/// The sample rate of the scene's media.
pub const SAMPLE_RATE: u32 = 48_000;

/// SplitMix64: a small, fast, seeded random stream.
#[derive(Clone, Debug)]
pub struct Random(u64);

impl Random {
    /// A stream from a seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number from 0 up to `n`.
    pub fn below(&mut self, n: u64) -> u64 {
        ((self.next_u64() >> 32) * n) >> 32
    }

    /// A number from 0 up to 1.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

const TRACK_WORDS: [&str; 12] = [
    "Kick", "Snare", "Hats", "Bass", "Pad", "Lead", "Keys", "Vox", "Perc", "Strings", "FX", "Choir",
];
const CLIP_WORDS: [&str; 16] = [
    "Loop", "Fill", "Hit", "Riff", "Phrase", "Take", "Groove", "Swell", "Break", "Verse", "Chorus",
    "Drop", "Intro", "Outro", "Chop", "Stab",
];

/// What the scene holds.
pub struct BenchmarkScene {
    /// The set.
    pub editor: Editor,
    /// Its media's peaks.
    pub peaks: MemoryPeaks,
    /// Its tracks, top to bottom.
    pub tracks: Vec<TrackId>,
    /// How many clips it has.
    pub clips: usize,
}

/// The made-up media file `index`'s content hash.
#[must_use]
pub fn media_hash(index: usize) -> ContentHash {
    let hash =
        bl_media::ContentHash::of_bytes(format!("beatlayer benchmark media {index}").as_bytes());
    ContentHash(*hash.as_bytes())
}

/// One made-up media file: `seconds` of stereo, three decaying partials
/// hit on every beat at 120 BPM under a slow swell, as the UI spikes drew.
#[must_use]
pub fn media_samples(random: &mut Random, seconds: f64) -> SampleData {
    let frames = (seconds * f64::from(SAMPLE_RATE)) as usize;
    let partials: Vec<(f64, f64, f64)> = (0..3)
        .map(|_| {
            (
                40.0 * 2f64.powf(4.0 * random.unit()),
                0.2 + 0.8 * random.unit(),
                std::f64::consts::TAU * random.unit(),
            )
        })
        .collect();
    let total: f64 = partials.iter().map(|p| p.1).sum();
    let decay = 3.0 + 12.0 * random.unit();
    let rate = f64::from(SAMPLE_RATE);
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for n in 0..frames {
        let t = n as f64 / rate;
        let beat = (n % (SAMPLE_RATE as usize / 2)) as f64 / rate;
        let envelope = (-decay * beat).exp()
            * (0.55 + 0.45 * (std::f64::consts::PI * n as f64 / frames as f64).sin());
        let sum: f64 = partials
            .iter()
            .map(|(f, a, p)| a * (std::f64::consts::TAU * f * t + p).sin())
            .sum();
        let value = (0.9 * envelope * sum / total) as f32;
        left.push(value);
        right.push(value * 0.8);
    }
    SampleData::new(SAMPLE_RATE, vec![left, right]).expect("two channels of equal length")
}

/// Builds the scene: `tracks` audio tracks of `clips_per_track` clips from
/// 1 to 8 bars long over 512 bars of 4/4 at 120 BPM, playing `media`
/// made-up files of 16 seconds (8 bars) each.
///
/// # Panics
///
/// If a command fails, which would be a bug.
#[must_use]
pub fn scene(tracks: usize, clips_per_track: usize, media: usize) -> BenchmarkScene {
    let mut random = Random::new(SEED);
    let mut peaks = MemoryPeaks::new();
    let mut media_random = Random::new(SEED ^ 0x0EED_DA7A);
    for index in 0..media.max(1) {
        let samples = media_samples(&mut media_random, 16.0);
        let built = Peaks::from_samples(&samples).expect("16 seconds fits a peak file");
        peaks.insert(media_hash(index), Arc::new(built));
    }
    let mut editor = Editor::new();
    let mut ids = Vec::with_capacity(tracks);
    let mut clip_count = 0;
    for t in 0..tracks {
        let id: TrackId = editor.new_id();
        let word = TRACK_WORDS[random.below(12) as usize];
        editor
            .apply(Command::CreateTrack {
                id,
                kind: TrackKind::Audio,
                name: format!("{} {word}", t + 1),
                parent: None,
                index: t,
            })
            .expect("a new track");
        ids.push(id);
        let lengths: Vec<i64> = (0..clips_per_track)
            .map(|_| 1 + random.below(8) as i64)
            .collect();
        let used: i64 = lengths.iter().sum();
        let free = (512 - used).max(0) as f64;
        let weights: Vec<f64> = (0..=clips_per_track).map(|_| random.unit()).collect();
        let weight_sum: f64 = weights.iter().sum::<f64>().max(1e-9);
        let mut bar = 0i64;
        for (c, length) in lengths.iter().enumerate() {
            bar += (free * weights[c] / weight_sum).floor() as i64;
            let clip_id = editor.new_id();
            let media_index = random.below(media.max(1) as u64) as usize;
            let mut clip = Clip::new(
                clip_id,
                Ticks::WHOLE * bar,
                Ticks::WHOLE * *length,
                ClipData::Audio(AudioData {
                    media: media_hash(media_index),
                }),
            );
            clip.name = CLIP_WORDS[random.below(16) as usize].to_owned();
            editor
                .apply(Command::CreateClip { track: id, clip })
                .expect("a new clip");
            clip_count += 1;
            bar += length;
        }
    }
    editor
        .clear_history()
        .expect("no transaction is open while building the scene");
    BenchmarkScene {
        editor,
        peaks,
        tracks: ids,
        clips: clip_count,
    }
}

/// A set store that opens one set held in memory, so a headless shell can
/// show the scene without files.
pub struct SceneStore {
    editor: Option<Editor>,
}

impl SceneStore {
    /// A store that opens `editor`.
    #[must_use]
    pub fn new(editor: Editor) -> Self {
        Self {
            editor: Some(editor),
        }
    }
}

impl SetStore for SceneStore {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let editor = self.editor.take().ok_or("the scene was opened already")?;
        Ok((editor, path.to_owned()))
    }

    fn save(&mut self, _path: &Path, _editor: &Editor) -> Result<Vec<String>, String> {
        Err("the benchmark scene isn't saved".to_owned())
    }

    fn close(&mut self) {}
}

/// A headless shell (null audio, no preferences saved) showing `editor`'s
/// set in the Arrangement View.
///
/// # Panics
///
/// If the shell can't open it, which would be a bug.
#[must_use]
pub fn shell_with(editor: Editor) -> Shell {
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(SceneStore::new(editor)),
    );
    shell
        .run(
            bl_shell::commands::SET_OPEN,
            json!({ "path": "Benchmark.blset" }),
        )
        .expect("the scene opens");
    shell
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scene_is_the_same_every_time() {
        let a = scene(3, 4, 2);
        let b = scene(3, 4, 2);
        assert_eq!(a.clips, 12);
        let starts = |s: &BenchmarkScene| -> Vec<(i64, i64, String)> {
            let doc = s.editor.document();
            s.tracks
                .iter()
                .flat_map(|t| doc.track(*t).unwrap().clips())
                .map(|c| (c.start.get(), c.length.get(), c.name.clone()))
                .collect()
        };
        assert_eq!(starts(&a), starts(&b));
        assert_eq!(a.peaks.len(), 2);
        // Clips never overlap on a track and fit in 512 bars.
        let doc = a.editor.document();
        for track in &a.tracks {
            let mut clips: Vec<_> = doc.track(*track).unwrap().clips().collect();
            clips.sort_by_key(|c| c.start);
            for pair in clips.windows(2) {
                assert!(pair[0].end() <= pair[1].start);
            }
            assert!(clips.last().unwrap().end() <= Ticks::WHOLE * 512);
        }
    }
}
