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
        parts.extend(self.markers.iter().map(|m| m.text.clone()));
        parts.join(", ")
    }
}
