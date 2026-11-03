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
            return Err(ModelError::InvalidInput("a partial's level is from 0 to 1"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Device, DeviceKind, Editor, TrackKind};

    fn operator(editor: &mut Editor) -> DeviceId {
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let id = editor.new_id();
        let kind = DeviceKind::Native {
            device_type: "test.operator".to_owned(),
        };
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: Device::new(id, "", kind),
            })
            .unwrap();
        id
    }

    // Spec: operator R27
    #[test]
    fn partials_are_16_32_or_64_levels_from_0_to_1_one_list_for_each_oscillator() {
        let list = |osc, len, level| Partials {
            osc,
            levels: vec![level; len],
        };
        assert!(check_partials(&[list(0, 16, 1.0), list(3, 64, 0.0)]).is_ok());
        for broken in [
            vec![list(0, 20, 0.5)],
            vec![list(0, 16, 1.5)],
            vec![list(0, 16, -0.1)],
            vec![list(0, 16, f32::NAN)],
            vec![list(4, 16, 0.5)],
            vec![list(1, 16, 0.5), list(1, 32, 0.5)],
        ] {
            assert!(check_partials(&broken).is_err());
        }
    }

    // Spec: operator R27
    #[test]
    fn setting_partials_is_one_undo_step_that_the_set_saves() {
        let mut editor = Editor::new();
        let device = operator(&mut editor);
        let levels = vec![0.25f32; 32];
        editor
            .apply(Command::SetPartials {
                device,
                osc: 2,
                levels: levels.clone(),
            })
            .unwrap();
        let saved = serde_json::to_value(editor.document().device(device).unwrap()).unwrap();
        assert_eq!(
            saved["partials"],
            serde_json::json!([{ "osc": 2, "levels": levels }])
        );
        let loaded: Device = serde_json::from_value(saved).unwrap();
        assert_eq!(
            loaded.partials,
            editor.document().device(device).unwrap().partials
        );
        // A bad list is refused and changes nothing.
        assert!(
            editor
                .apply(Command::SetPartials {
                    device,
                    osc: 2,
                    levels: vec![2.0; 16],
                })
                .is_err()
        );
        assert_eq!(editor.document().device(device).unwrap().partials.len(), 1);
        editor.undo().unwrap();
        assert!(
            editor
                .document()
                .device(device)
                .unwrap()
                .partials
                .is_empty()
        );
        // A device without any leaves the field out of the saved set.
        let bare = serde_json::to_value(editor.document().device(device).unwrap()).unwrap();
        assert!(bare.get("partials").is_none());
    }
}
