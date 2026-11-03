//! A user wave's partials (the Operator spec, R7, R27): the levels an
//! oscillator's drawn wave is built from, kept on the device so they are
//! saved with the set and read back by agents.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::ids::DeviceId;

/// The oscillators a device's user waves are drawn for: Operator's A to D.
pub const OSCILLATORS: u8 = 4;

/// How many partials a user wave holds: its first 16, 32 or 64.
pub const COUNTS: [usize; 3] = [16, 32, 64];

/// One oscillator's drawn partials.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Partials {
    /// The oscillator, from 0 for A.
    pub osc: u8,
    /// The levels of the first partials, each from 0 to 1; there are 16, 32
    /// or 64 of them.
    pub levels: Vec<f32>,
}

/// Whether `partials` are a user wave a device can hold: each oscillator
/// once, a count of 16, 32 or 64, and levels from 0 to 1.
///
/// # Errors
///
/// The first rule that fails.
pub fn check_partials(partials: &[Partials]) -> Result<(), ModelError> {
    let mut seen = [false; OSCILLATORS as usize];
    for p in partials {
        if p.osc >= OSCILLATORS || std::mem::replace(&mut seen[usize::from(p.osc)], true) {
            return Err(ModelError::InvalidInput(
                "partials are drawn for oscillators A to D, one list each",
            ));
        }
        if !COUNTS.contains(&p.levels.len()) {
            return Err(ModelError::InvalidInput(
                "a user wave has 16, 32 or 64 partials",
            ));
        }
        if p.levels.iter().any(|l| !(0.0..=1.0).contains(l)) {
            return Err(ModelError::InvalidInput(
                "a partial's level is from 0 to 1",
            ));
        }
    }
    Ok(())
}

impl crate::Document {
    /// Sets oscillator `osc`'s drawn partials on `device` (the Operator
    /// spec, R27).
    pub(crate) fn set_partials(
        &mut self,
        device: DeviceId,
        osc: u8,
        levels: &[f32],
    ) -> Result<(), ModelError> {
        let mut partials = self.device_ref(device)?.partials.clone();
        partials.retain(|p| p.osc != osc);
        partials.push(Partials {
            osc,
            levels: levels.to_vec(),
        });
        partials.sort_by_key(|p| p.osc);
        check_partials(&partials)?;
        if partials != self.device_ref(device)?.partials {
            self.device_mut(device)?.partials = partials;
        }
        Ok(())
    }
}
