//! What a device's display shows, as data (the device framework spec, R35
//! to R38): curves and spectra over an axis, markers on them, and single
//! readings, so the cell draws them, a screen reader says them, and an agent
//! reads them. A device type works them out from its parameters' values and
//! the display data its node publishes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;

/// A curve or a spectrum: values at evenly spaced places along an axis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DisplaySeries {
    /// What it is, such as `Spectrum`; the cell draws it by this name.
    pub name: String,
    /// What the values are in, such as `dB`.
    pub unit: String,
    /// What the axis is in, such as `Hz`.
    pub x_unit: String,
    /// The axis's first and last place.
    pub x_range: [f32; 2],
    /// Whether the axis is logarithmic.
    pub log_x: bool,
    /// The values the cell shows between its bottom and its top.
    pub y_range: [f32; 2],
    /// Whether it's a part of another series, such as one band's response
    /// within the curve's sum: drawn faint.
    #[serde(default)]
    pub detail: bool,
    /// The values, from the first place to the last.
    pub values: Vec<f32>,
}

impl DisplaySeries {
    /// The place on the axis of the value at `index`.
    #[must_use]
    pub fn x_at(&self, index: usize) -> f32 {
        let last = self.values.len().saturating_sub(1).max(1) as f32;
        let t = index as f32 / last;
        let [from, to] = self.x_range;
        if self.log_x {
            from * (to / from).powf(t)
        } else {
            from + (to - from) * t
        }
    }

    /// The place and value of its highest value, if it has any.
    #[must_use]
    pub fn peak(&self) -> Option<(f32, f32)> {
        let (index, value) = self
            .values
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        Some((self.x_at(index), value))
    }
}

/// A point on the display: a band's dot on an EQ's curve, or the level now
/// on a compressor's transfer curve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayMarker {
    /// What it is, such as `A1`.
    pub name: String,
    /// The series whose axes it sits on.
    pub series: String,
    /// Its place along the axis, in the series' `x_unit`.
    pub x: f32,
    /// Its value, in the series' `unit`.
    pub y: f32,
    /// How it reads, such as `A1 Peak 1.00 kHz 3.0 dB`.
    pub text: String,
}

/// A single reading, such as a gain reduction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayReading {
    /// What it is, such as `Gain Reduction`.
    pub name: String,
    /// Its value now, in `unit`.
    pub value: f32,
    /// What it's in, such as `dB`.
    pub unit: String,
    /// How it reads, such as `-4.3 dB`.
    pub text: String,
}

/// The grid of a device's spectrogram, as its type's description gives it
/// (R46): fixed for the type, never by the size of the cell or window it is
/// drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpectrogramGrid {
    /// The columns over time, the newest on the right.
    pub columns: usize,
    /// The rows over frequency, the lowest at the bottom.
    pub rows: usize,
    /// The lowest and highest frequency, in Hz, at the middle of the first
    /// and last row.
    pub frequencies: [f32; 2],
    /// Whether the rows are spaced logarithmically in frequency (linearly
    /// if not).
    pub log_frequency: bool,
    /// The levels in dB that the colour ramp runs between: the floor, drawn
    /// as its first colour, and the ceiling, as its last.
    pub levels: [f32; 2],
}

impl SpectrogramGrid {
    /// The most columns or rows a grid may have.
    pub const MAX_SIDE: usize = 1_024;

    /// The frequency in Hz at the middle of `row`.
    #[must_use]
    pub fn frequency_at(&self, row: usize) -> f32 {
        let t = row as f32 / self.rows.saturating_sub(1).max(1) as f32;
        let [from, to] = self.frequencies;
        if self.log_frequency {
            from * (to / from).powf(t)
        } else {
            from + (to - from) * t
        }
    }

    /// The first broken rule, as a description's check words it.
    pub(crate) fn check(&self) -> Result<(), String> {
        let [low, high] = self.frequencies;
        let [floor, ceiling] = self.levels;
        let side = 1..=Self::MAX_SIDE;
        if !side.contains(&self.columns) || !side.contains(&self.rows) {
            return Err(format!(
                "a spectrogram has 1 to {} columns and rows",
                Self::MAX_SIDE
            ));
        }
        if !(low.is_finite() && high.is_finite() && 0.0 < low && low < high) {
            return Err("a spectrogram's frequencies rise from above 0 Hz".to_owned());
        }
        if !(floor.is_finite() && ceiling.is_finite() && floor < ceiling) {
            return Err("a spectrogram's floor is below its ceiling".to_owned());
        }
        Ok(())
    }
}

// Every value in a grid is finite (`check`), so equality is an equivalence.
impl Eq for SpectrogramGrid {}

/// A spectrogram: magnitudes in dB over time and frequency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DisplaySpectrogram {
    /// What it is, such as `Spectrogram`.
    pub name: String,
    /// Its grid, from the type's description.
    #[serde(flatten)]
    pub grid: SpectrogramGrid,
    /// The magnitudes in dB, a column after another from the oldest to the
    /// newest, each from its lowest row to its highest.
    pub values: Vec<f32>,
}

impl DisplaySpectrogram {
    /// A spectrogram named `name` of `grid`, from the `data` its node
    /// published (one value per cell, as `values` holds them), or nothing
    /// if `data` isn't the grid's size.
    #[must_use]
    pub fn from_data(name: &str, grid: SpectrogramGrid, data: &[f32]) -> Option<Self> {
        (data.len() == grid.columns * grid.rows).then(|| Self {
            name: name.to_owned(),
            grid,
            values: data.to_vec(),
        })
    }

    /// The column at `index`, oldest first, or nothing past the last.
    #[must_use]
    pub fn column(&self, index: usize) -> Option<&[f32]> {
        self.values.chunks_exact(self.grid.rows).nth(index)
    }

    /// The frequency and level of the loudest row of the newest column.
    #[must_use]
    pub fn peak(&self) -> Option<(f32, f32)> {
        let (row, level) = self
            .column(self.grid.columns.checked_sub(1)?)?
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        Some((self.grid.frequency_at(row), level))
    }
}

/// A device's display now, as `device.display` reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceDisplay {
    /// The device.
    pub device: DeviceId,
    /// Whether the device's engine has published its data: `false` while no
    /// engine runs or before its first block, when only what its parameters
    /// give is here.
    pub live: bool,
    /// The view the display is showing, for a device whose display has
    /// views.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// Its curves and spectra.
    pub series: Vec<DisplaySeries>,
    /// Its spectrograms.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spectrograms: Vec<DisplaySpectrogram>,
    /// Its markers.
    pub markers: Vec<DisplayMarker>,
    /// Its single readings.
    pub readings: Vec<DisplayReading>,
}

impl DeviceDisplay {
    /// The series called `name`.
    #[must_use]
    pub fn series(&self, name: &str) -> Option<&DisplaySeries> {
        self.series.iter().find(|s| s.name == name)
    }

    /// The reading called `name`.
    #[must_use]
    pub fn reading(&self, name: &str) -> Option<&DisplayReading> {
        self.readings.iter().find(|r| r.name == name)
    }

    /// The display as a screen reader says it: each reading, the highest
    /// point of each spectrum, and each marker.
    #[must_use]
    pub fn text(&self) -> String {
        let mut parts: Vec<String> = self
            .readings
            .iter()
            .map(|r| format!("{} {}", r.name, r.text))
            .collect();
        for series in self.series.iter().filter(|s| s.name == "Spectrum") {
            if let Some((x, y)) = series.peak() {
                parts.push(format!(
                    "Spectrum peak {x:.0} {} {y:.1} {}",
                    series.x_unit, series.unit
                ));
            }
        }
        for spectrogram in &self.spectrograms {
            if let Some((hz, db)) = spectrogram.peak() {
                parts.push(format!("{} peak {hz:.0} Hz {db:.1} dB", spectrogram.name));
            }
        }
        parts.extend(self.markers.iter().map(|m| m.text.clone()));
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRID: SpectrogramGrid = SpectrogramGrid {
        columns: 3,
        rows: 4,
        frequencies: [100.0, 800.0],
        log_frequency: true,
        levels: [-80.0, 0.0],
    };

    // Spec: device-framework R46
    #[test]
    fn a_spectrogram_has_the_grid_its_description_gives_and_data_of_no_other_size() {
        let mut data = vec![-80.0; 12];
        data[9 + 2] = -6.0;
        let shown = DisplaySpectrogram::from_data("Spectrogram", GRID, &data).unwrap();
        assert_eq!(shown.column(2).unwrap(), [-80.0, -80.0, -80.0, -6.0]);
        assert!(shown.column(3).is_none());
        assert!(DisplaySpectrogram::from_data("Spectrogram", GRID, &data[1..]).is_none());
        // The rows are an octave apart on a log axis from 100 to 800 Hz.
        let hz: Vec<f32> = (0..4).map(|r| GRID.frequency_at(r)).collect();
        assert!(
            hz.iter()
                .zip([100.0, 200.0, 400.0, 800.0])
                .all(|(a, b)| (a - b).abs() < 0.01)
        );
        // The newest column's loudest row is what a screen reader hears.
        let (peak_hz, level) = shown.peak().unwrap();
        assert!((peak_hz - 800.0).abs() < 0.01 && level == -6.0);
    }

    // Spec: device-framework R46
    #[test]
    fn a_grid_must_have_columns_rows_a_frequency_span_and_a_level_span() {
        assert_eq!(GRID.check(), Ok(()));
        let broken = [
            SpectrogramGrid { columns: 0, ..GRID },
            SpectrogramGrid {
                rows: SpectrogramGrid::MAX_SIDE + 1,
                ..GRID
            },
            SpectrogramGrid {
                frequencies: [0.0, 800.0],
                ..GRID
            },
            SpectrogramGrid {
                frequencies: [800.0, 100.0],
                ..GRID
            },
            SpectrogramGrid {
                levels: [0.0, -80.0],
                ..GRID
            },
            SpectrogramGrid {
                levels: [f32::NAN, 0.0],
                ..GRID
            },
        ];
        for grid in broken {
            assert!(grid.check().is_err(), "{grid:?}");
        }
    }
}
