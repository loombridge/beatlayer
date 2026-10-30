//! The velocity curve and the Setup values it comes from
//! (`docs/specs/push-2.md`, R16, R30 and R31).

use crate::sysex;

/// Where the Setup menu keeps its values: Pad Sensitivity, Pad Gain and Pad
/// Dynamics from 0 to 10, the two brightnesses in percent, and Workflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    /// Pad Sensitivity, 0 to 10.
    pub sensitivity: u8,
    /// Pad Gain, 0 to 10.
    pub gain: u8,
    /// Pad Dynamics, 0 to 10.
    pub dynamics: u8,
    /// Display Light, 0 to 100 %.
    pub display_light: u8,
    /// LED Brightness, 0 to 100 %.
    pub led_brightness: u8,
    /// What Duplicate, New and the arrows do in Note Mode (R32).
    pub workflow: Workflow,
}

/// Workflow (R31, R32).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Workflow {
    /// Scene, the default.
    #[default]
    Scene,
    /// Clip.
    Clip,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            sensitivity: 5,
            gain: 5,
            dynamics: 5,
            display_light: 100,
            led_brightness: 100,
            workflow: Workflow::Scene,
        }
    }
}

impl Setup {
    /// This Setup with every value in its range.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            sensitivity: self.sensitivity.min(10),
            gain: self.gain.min(10),
            dynamics: self.dynamics.min(10),
            display_light: self.display_light.min(100),
            led_brightness: self.led_brightness.min(100),
            workflow: self.workflow,
        }
    }

    /// The value Set LED Brightness takes: the percentage of 127, rounded
    /// (R16).
    #[must_use]
    pub fn led_value(&self) -> u8 {
        scale(self.led_brightness, 127)
    }

    /// The value Set Display Brightness takes: the percentage of 255,
    /// rounded (R16).
    #[must_use]
    pub fn display_value(&self) -> u8 {
        scale(self.display_light, 255)
    }

    /// The 128-entry velocity curve these values make (R30).
    #[must_use]
    pub fn curve(&self) -> [u8; 128] {
        curve(self.sensitivity, self.gain, self.dynamics)
    }
}

fn scale(percent: u8, full: u32) -> u8 {
    u8::try_from((u32::from(percent.min(100)) * full + 50) / 100).unwrap_or(u8::MAX)
}

/// The velocity for each force of 32·i grams, entry i, from Pad Sensitivity
/// `s`, Pad Gain `g` and Pad Dynamics `d` (R30).
#[must_use]
pub fn curve(s: u8, g: u8, d: u8) -> [u8; 128] {
    let (s, g, d) = (f64::from(s.min(10)), f64::from(g.min(10)), f64::from(d.min(10)));
    std::array::from_fn(|i| {
        let x = i as f64 / 127.0;
        let x1 = (x * ((s - 5.0) / 2.5).exp2()).min(1.0);
        let x2 = x1.powf(((4.0 - g) / 3.0).exp2());
        let spread = ((7.0 - d) / 3.0).exp2();
        let x3 = 0.5 + (x2 - 0.5).signum() * (2.0 * x2 - 1.0).abs().powf(spread) / 2.0;
        (1.0 + 126.0 * x3).round().clamp(1.0, 127.0) as u8
    })
}

/// The eight Set Pad Velocity Curve Entry commands that send `curve`.
#[must_use]
pub fn messages(curve: &[u8; 128]) -> Vec<Vec<u8>> {
    curve
        .chunks(16)
        .enumerate()
        .map(|(n, entries)| sysex::velocity_curve_entries((n * 16) as u8, entries))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // Spec: push-2 R30. Sensitivity 5, Gain 4 and Dynamics 7 is a straight line.
    #[test]
    fn five_four_seven_is_the_identity() {
        let curve = curve(5, 4, 7);
        for (i, v) in curve.iter().enumerate() {
            assert_eq!(*v, (1.0 + 126.0 * i as f64 / 127.0).round() as u8, "{i}");
        }
    }

    // Spec: push-2 R30. Eight commands of sixteen entries from 0 to 112.
    #[test]
    fn the_curve_goes_out_in_eight_messages() {
        let sent = messages(&curve(5, 5, 5));
        assert_eq!(sent.len(), 8);
        for (n, message) in sent.iter().enumerate() {
            assert!(sysex::allowed(message));
            assert_eq!(message[7], (n * 16) as u8);
            assert_eq!(message.len(), 6 + 1 + 1 + 16 + 1);
        }
    }

    // Spec: push-2 R30. More Sensitivity reaches a high velocity with less force.
    #[test]
    fn sensitivity_raises_the_curve() {
        let (low, high) = (curve(2, 4, 7), curve(8, 4, 7));
        assert!(high[40] > low[40]);
    }

    // Spec: push-2 R16, R31. Percentages of 127 and 255, rounded.
    #[test]
    fn brightness_maps_to_the_device_ranges() {
        let setup = |light, led| Setup {
            display_light: light,
            led_brightness: led,
            ..Setup::default()
        };
        assert_eq!((setup(100, 100).display_value(), setup(100, 100).led_value()), (255, 127));
        assert_eq!((setup(0, 0).display_value(), setup(0, 0).led_value()), (0, 0));
        assert_eq!((setup(50, 50).display_value(), setup(50, 50).led_value()), (128, 64));
    }

    proptest! {
        // Spec: push-2 R30. Monotonic, 1 to 127, for every setting.
        #[test]
        fn every_curve_is_monotonic_and_in_range(s in 0u8..=10, g in 0u8..=10, d in 0u8..=10) {
            let curve = curve(s, g, d);
            prop_assert!(curve.iter().all(|v| (1..=127).contains(v)));
            prop_assert!(curve.windows(2).all(|w| w[0] <= w[1]));
        }
    }
}
