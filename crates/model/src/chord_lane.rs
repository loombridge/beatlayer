//! The chord lane (WP-6.48; the chord lane spec, R2): regions of the song,
//! each with a scale and a transposition, which the MIDI tracks that follow
//! the lane play their Arrangement clips through.
//!
//! The regions are saved in the set's settings, so every edit is an undo
//! step. A set saved before the lane has none and loads with none.

use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::ids::ChordRegionId;
use crate::scale::Scale;

/// The most a region transposes by, up or down (the chord lane spec, R2).
pub const MAX_TRANSPOSE: i8 = 24;

/// One stretch of the song and what it does to the tracks that follow the
/// lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChordRegion {
    /// The region's ID.
    pub id: ChordRegionId,
    /// Where it starts, on the song's beats: 1.1.1 or later.
    pub start: Ticks,
    /// Where it ends, which it doesn't include: after its start.
    pub end: Ticks,
    /// Its scale: the current scale of the following tracks' devices while
    /// the song plays through it. A scale with `enabled` false is no scale,
    /// and leaves the clips' own.
    #[serde(default, skip_serializing_if = "Scale::is_default")]
    pub scale: Scale,
    /// How far it moves the notes of the following tracks: degrees of each
    /// clip's scale where the clip has Scale Mode on, semitones where it is
    /// off. From -24 to 24.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub transpose: i8,
}

fn is_zero(value: &i8) -> bool {
    *value == 0
}

impl ChordRegion {
    /// A region over `start..end` with no scale and no transposition.
    #[must_use]
    pub fn new(id: ChordRegionId, start: Ticks, end: Ticks) -> Self {
        Self {
            id,
            start,
            end,
            scale: Scale::default(),
            transpose: 0,
        }
    }

    /// Whether the region is one a set may hold: from 1.1.1 or later, longer
    /// than nothing, and transposing by no more than 24.
    pub(crate) fn check(&self) -> Result<(), ModelError> {
        if self.start < Ticks::ZERO {
            return Err(ModelError::NegativePosition(self.start));
        }
        if self.end <= self.start {
            return Err(ModelError::InvalidInput(
                "a chord region must end after it starts",
            ));
        }
        check_transpose(self.transpose)
    }

    /// What the lane writes in the region: its scale and its transposition,
    /// such as "D Dorian +2", or "No Scale +0" (the chord lane spec, R2).
    #[must_use]
    pub fn name(&self) -> String {
        let scale = if self.scale.enabled {
            format!(
                "{} {}",
                Scale::root_name(self.scale.root),
                self.scale.name.label()
            )
        } else {
            "No Scale".to_owned()
        };
        format!("{scale} {:+}", self.transpose)
    }
}

/// Refuses a transposition outside -24 to 24.
pub(crate) fn check_transpose(transpose: i8) -> Result<(), ModelError> {
    if transpose.abs() > MAX_TRANSPOSE {
        return Err(ModelError::InvalidValue {
            what: "transposition",
            value: f64::from(transpose),
        });
    }
    Ok(())
}

/// Checks the regions a saved set holds: each one a set may hold, and none
/// overlapping another.
pub(crate) fn check_all(regions: &[ChordRegion]) -> Result<(), ModelError> {
    for region in regions {
        region.check()?;
    }
    let mut sorted: Vec<&ChordRegion> = regions.iter().collect();
    sorted.sort_by_key(|region| region.start);
    for pair in sorted.windows(2) {
        if pair[1].start < pair[0].end {
            return Err(ModelError::ChangeInUse {
                what: "chord region",
                at: pair[1].start,
            });
        }
    }
    Ok(())
}

/// Whether `start..end` overlaps a region other than `except`.
pub(crate) fn overlaps(
    regions: &[ChordRegion],
    except: Option<ChordRegionId>,
    start: Ticks,
    end: Ticks,
) -> Option<Ticks> {
    regions
        .iter()
        .find(|other| Some(other.id) != except && other.start < end && start < other.end)
        .map(|other| other.start.max(start))
}
