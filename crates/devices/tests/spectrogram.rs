//! The test spectrogram (WP-8.28; the device framework spec, R46): it
//! publishes a grid of dB from `bl_dsp::stft` under the allocation trap, and
//! the grid's size comes from the type's description.

use std::f64::consts::TAU;

use bl_devices::testing::{
    SPECTROGRAM_FREEZE, SPECTROGRAM_GRID, registry, spectrogram_display, spectrogram_type,
};
use bl_model::{DeviceDescription, DeviceId};

use crate::common::{RATE, device, node, run};

fn sine(hz: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (TAU * hz * i as f64 / RATE.as_f64()).sin() as f32)
        .collect()
}

/// What the device publishes after `input`, `block` frames at a time.
fn published(input: &[f32], block: usize, freeze: f64) -> Vec<f32> {
    let kind = spectrogram_type();
    let device = device(&kind, &[(SPECTROGRAM_FREEZE, freeze)]);
    let mut node = node(&registry(), &device, block);
    let mut reader = node.display().expect("the device has a display");
    for part in input.chunks(block) {
        run(node.as_mut(), part, &[]);
    }
    reader.read().values.clone()
}

fn description() -> DeviceDescription {
    spectrogram_type().description
}

// Spec: device-framework R46
#[test]
fn the_grids_size_comes_from_the_description_and_not_from_anything_the_ui_draws_in() {
    let description = description();
    description.check().expect("a valid description");
    assert_eq!(description.spectrogram, Some(SPECTROGRAM_GRID));
    let values = published(&sine(1_000.0, 20_000), 512, 0.0);
    assert_eq!(
        values.len(),
        SPECTROGRAM_GRID.columns * SPECTROGRAM_GRID.rows
    );
    let id = DeviceId(bl_core::id::Id::new(1).unwrap());
    let shown = spectrogram_display(id, &description, Some(&values));
    let spectrogram = &shown.spectrograms[0];
    assert_eq!(spectrogram.grid, SPECTROGRAM_GRID);
    assert_eq!(spectrogram.values, values);
    assert!(
        spectrogram_display(id, &description, Some(&values[1..]))
            .spectrograms
            .is_empty(),
        "data of another size isn't a spectrogram"
    );
    assert!(!spectrogram_display(id, &description, None).live);
}

// Spec: device-framework R46
#[test]
fn a_tone_lights_its_own_row_in_the_newest_columns_and_the_audio_passes_through() {
    let kind = spectrogram_type();
    let device = device(&kind, &[]);
    let mut node = node(&registry(), &device, 4096);
    let mut reader = node.display().expect("the device has a display");
    let input = sine(1_000.0, 4096);
    for _ in 0..8 {
        let out = run(node.as_mut(), &input, &[]);
        assert_eq!(out[0], input, "the audio isn't changed");
    }
    let grid = SPECTROGRAM_GRID;
    let id = DeviceId(bl_core::id::Id::new(1).unwrap());
    let shown = spectrogram_display(id, &description(), Some(&reader.read().values.clone()));
    let (hz, level) = shown.spectrograms[0].peak().expect("a newest column");
    // Within a row of 1 kHz, and a full-scale sine reads within a Hann
    // window's scalloping of 0 dB.
    let row = (grid.frequencies[1] / grid.frequencies[0]).ln() / (grid.rows - 1) as f32;
    assert!((hz / 1_000.0).ln().abs() < row, "{hz} Hz");
    assert!((-3.0..=0.1).contains(&level), "{level} dB");
    // The oldest column, from before the sound began, is at the floor.
    let first = shown.spectrograms[0].column(0).unwrap();
    assert!(
        first.iter().all(|&v| v < grid.levels[0] + 20.0),
        "{first:?}"
    );
}

// Spec: device-framework R46
#[test]
fn the_grid_is_the_same_to_the_bit_at_any_block_size_and_freeze_holds_it() {
    let input = sine(440.0, 30_000);
    let one = published(&input, 1, 0.0);
    for block in [64, 511, 4096] {
        assert_eq!(one, published(&input, block, 0.0), "block {block}");
    }
    let frozen = published(&input, 512, 1.0);
    assert!(frozen.iter().all(|&v| v == SPECTROGRAM_GRID.levels[0]));
}
