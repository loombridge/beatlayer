//! Impulse-response slots in a device's node (WP-8.30; the impulse responses
//! spec, R1, R4 to R7, R15): a response read from a file convolves the
//! signal, the node swaps and clears it with a crossfade, and none of it
//! allocates on the audio thread.

use std::path::{Path, PathBuf};

use bl_core::time::SampleRate;
use bl_devices::impulse::{self, LoadError};
use bl_devices::testing;
use bl_engine::{Collector, Node, NodeConfig};
use bl_media::{BitDepth, Dither, MediaStore};
use bl_model::ContentHash;

use crate::common;

const BLOCK: usize = 512;

/// A scratch folder, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-impulse-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes `channels` as a float WAV file at `rate`.
fn write(path: &Path, channels: &[Vec<f32>], rate: u32) {
    let frames = channels[0].len();
    let interleaved: Vec<f32> = (0..frames)
        .flat_map(|i| channels.iter().map(move |c| c[i]))
        .collect();
    let bytes = bl_media::wav_bytes(
        &interleaved,
        channels.len() as u16,
        rate,
        BitDepth::Float32,
        Dither::None,
    )
    .unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A delta at `at`, `frames` long.
fn delta(at: usize, level: f32, frames: usize) -> Vec<f32> {
    let mut samples = vec![0.0; frames];
    samples[at] = level;
    samples
}

/// The content hash of `path` in `store`.
fn hash_of(store: &MediaStore, path: &Path) -> ContentHash {
    ContentHash(*store.import(path).unwrap().as_bytes())
}

/// A convolution test device's node holding the response in `file`, run
/// until its first response has faded in.
fn node_with(file: Option<&Path>) -> (Box<dyn Node>, Collector, MediaStore, bl_model::Device) {
    let registry = testing::registry();
    let mut device = common::device(&testing::convolution_type(), &[]);
    let store = MediaStore::new();
    if let Some(file) = file {
        device.set_media(0, Some(hash_of(&store, file)));
    }
    let node = common::node(&registry, &device, BLOCK);
    (node, Collector::new(), store, device)
}

/// Sends `node` what the compiler would for `device`.
fn send(node: &mut dyn Node, collector: &Collector, store: &MediaStore, device: &bl_model::Device) {
    let file = |media: ContentHash| {
        store
            .entry(bl_media::ContentHash::from_bytes(media.0))
            .map(|entry| entry.path)
    };
    let config = NodeConfig {
        sample_rate: SampleRate::HZ_48000,
        max_frames: 64,
        channels: 2,
        round_trip_latency: 0,
    };
    let data = impulse::data(1)(device, &config, &file);
    let gc = collector.handle();
    let sent: Vec<_> = data.items.into_iter().map(|item| item(&gc)).collect();
    for data in sent {
        bl_rt::rt_test!({ node.receive_data(data) });
    }
}

/// Runs `frames` of an input that is silent but for a unit impulse at
/// `impulse`, `BLOCK` frames at a time.
fn run(node: &mut dyn Node, frames: usize, impulse: usize) -> Vec<f32> {
    let mut input = vec![0.0; frames];
    input[impulse] = 1.0;
    let input = vec![input.clone(), input];
    common::run_blocks(node, &input, BLOCK, &[], &Default::default()).remove(0)
}

// Spec: impulse-responses R1, R4, R15
#[test]
fn a_slot_convolves_with_its_response_and_an_empty_slot_gives_silence() {
    let scratch = Scratch::new("delay");
    let file = scratch.0.join("Delta.wav");
    // Its level is normalised away (R7): the delta comes out at 1.
    write(&file, &[delta(100, 0.25, 600)], 48_000);
    let (mut node, collector, store, device) = node_with(Some(&file));
    send(node.as_mut(), &collector, &store, &device);
    let out = run(node.as_mut(), 16 * BLOCK, 6000);
    let (at, peak) = out
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap();
    assert_eq!(at, 6100, "the response's predelay is kept (R6)");
    assert!((peak - 1.0).abs() < 0.01, "normalised to energy 1: {peak}");

    let (mut node, ..) = node_with(None);
    let out = run(node.as_mut(), 8 * BLOCK, 1000);
    assert!(out.iter().all(|&s| s == 0.0), "no response, no sound");
}

// Spec: impulse-responses R4
#[test]
fn a_new_response_replaces_the_old_with_a_crossfade_and_clearing_goes_silent() {
    let scratch = Scratch::new("swap");
    let (a, b) = (scratch.0.join("A.wav"), scratch.0.join("B.wav"));
    write(&a, &[delta(10, 1.0, 100)], 48_000);
    write(&b, &[delta(300, 1.0, 400)], 48_000);
    let (mut node, collector, store, mut device) = node_with(Some(&a));
    send(node.as_mut(), &collector, &store, &device);
    run(node.as_mut(), 16 * BLOCK, 0);

    device.set_media(0, Some(hash_of(&store, &b)));
    send(node.as_mut(), &collector, &store, &device);
    let out = run(node.as_mut(), 32 * BLOCK, 10_000);
    let at = out
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;
    assert_eq!(
        at, 10_300,
        "the second response plays once the fade is over"
    );

    device.set_media(0, None);
    send(node.as_mut(), &collector, &store, &device);
    let out = run(node.as_mut(), 32 * BLOCK, 10_000);
    assert!(
        out[8 * BLOCK..].iter().all(|s| s.abs() < 1e-4),
        "a cleared slot gives silence"
    );
}

/// White noise from a seeded generator, scaled to `level`.
fn noise(frames: usize, level: f32, mut seed: u32) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1 << 23) as f32 - 1.0) * level
        })
        .collect()
}

// Spec: impulse-responses R7
#[test]
fn two_responses_of_different_energy_play_within_a_decibel_on_steady_noise() {
    let scratch = Scratch::new("level");
    let loud = scratch.0.join("Loud.wav");
    let quiet = scratch.0.join("Quiet.wav");
    write(&loud, &[noise(2000, 0.9, 1)], 48_000);
    let decaying: Vec<f32> = noise(40_000, 0.01, 2)
        .iter()
        .enumerate()
        .map(|(i, s)| s * (-(i as f32) / 6000.0).exp())
        .collect();
    write(&quiet, &[decaying], 48_000);

    let level = |file: &Path| {
        let (mut node, collector, store, device) = node_with(Some(file));
        send(node.as_mut(), &collector, &store, &device);
        let input = vec![noise(96_000, 0.5, 7), noise(96_000, 0.5, 8)];
        let out = common::run_blocks(node.as_mut(), &input, BLOCK, &[], &Default::default());
        let tail = &out[0][48_000..];
        (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt()
    };
    let db = 20.0 * (level(&loud) / level(&quiet)).log10();
    assert!(db.abs() < 1.0, "{db:.2} dB apart");
}

// Spec: impulse-responses R3
#[test]
fn a_three_channel_file_and_a_file_over_thirty_seconds_are_refused() {
    let scratch = Scratch::new("refused");
    let three = scratch.0.join("Three.wav");
    write(
        &three,
        &[delta(0, 1.0, 100), delta(1, 1.0, 100), delta(2, 1.0, 100)],
        48_000,
    );
    assert_eq!(
        impulse::load(&three, 48_000).err(),
        Some(LoadError::Channels(3))
    );
    let long = scratch.0.join("Long.wav");
    write(&long, &[delta(0, 1.0, 31 * 8000)], 8000);
    let refused = impulse::load(&long, 48_000).err();
    assert_eq!(refused, Some(LoadError::TooLong));
    assert!(refused.unwrap().to_string().contains("30 seconds"));
}

// Spec: impulse-responses R5, R6
#[test]
fn a_response_is_resampled_to_the_engine_and_trimmed_after_its_last_audible_frame() {
    let scratch = Scratch::new("trim");
    let file = scratch.0.join("Padded.wav");
    // A burst at 44.1 kHz, then 20,000 frames of padding.
    let mut samples = noise(4410, 0.5, 3);
    samples.extend(vec![0.0; 20_000]);
    write(&file, &[samples], 44_100);
    let loaded = impulse::load(&file, 48_000).unwrap();
    assert_eq!(loaded.file_frames, 24_410);
    assert_eq!(loaded.ir.rate(), 48_000);
    let burst = 4410 * 48_000 / 44_100;
    // The resampler's own ringing past the burst stays above -90 dB for a
    // few hundred frames.
    assert!(
        (burst - 200..burst + 600).contains(&loaded.frames),
        "the padding is dropped: {} frames of about {burst}",
        loaded.frames
    );
}

// Spec: impulse-responses R7
#[test]
fn a_unit_impulse_at_frame_zero_is_not_changed_by_normalising() {
    let scratch = Scratch::new("unit");
    let file = scratch.0.join("Unit.wav");
    write(&file, &[delta(0, 1.0, 64)], 48_000);
    let loaded = impulse::load(&file, 48_000).unwrap();
    assert!(loaded.gain_db.abs() < 0.01, "{} dB", loaded.gain_db);
    assert_eq!(loaded.frames, 1);
}
