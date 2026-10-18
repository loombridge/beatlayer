//! Follow actions (WP-3.10): what a Session clip or a scene does by itself
//! after it has played for a while.
//!
//! A clip's or a scene's [`FollowActions`] are saved with it and each edit
//! is one undo step. The engine reads them when the clip or the scene
//! launches and carries them out; the rules are in the follow actions spec.

use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;

/// What a follow action does when its time comes (the Clip View's action
/// list, in its order).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FollowAction {
    /// Nothing: the clip plays on as it was.
    NoAction,
    /// Stops the clip.
    Stop,
    /// Launches the clip again from its start.
    PlayAgain,
    /// Launches the clip before this one in its group.
    Previous,
    /// Launches the clip after this one in its group.
    #[default]
    Next,
    /// Launches the first clip of the group.
    First,
    /// Launches the last clip of the group.
    Last,
    /// Launches any clip of the group, this one included.
    Any,
    /// Launches any clip of the group but this one.
    Other,
    /// Launches the clip at a slot number the action names.
    Jump,
}

impl FollowAction {
    /// Every action, in the chooser's order.
    pub const ALL: [Self; 10] = [
        Self::NoAction,
        Self::Stop,
        Self::PlayAgain,
        Self::Previous,
        Self::Next,
        Self::First,
        Self::Last,
        Self::Any,
        Self::Other,
        Self::Jump,
    ];

    /// The action's name in the chooser.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoAction => "No Action",
            Self::Stop => "Stop",
            Self::PlayAgain => "Play Again",
            Self::Previous => "Previous",
            Self::Next => "Next",
            Self::First => "First",
            Self::Last => "Last",
            Self::Any => "Any",
            Self::Other => "Other",
            Self::Jump => "Jump",
        }
    }
}

/// The longest a Linked multiplier goes (observed: 999999x is accepted).
pub const FOLLOW_MULTIPLIER_MAX: u32 = 999_999;
/// The shortest an Unlinked follow time is: 0.0.1, a sixteenth.
pub const FOLLOW_TIME_MIN: Ticks = Ticks::SIXTEENTH;
/// The longest an Unlinked follow time is, in 4/4 bars.
pub const FOLLOW_TIME_MAX: Ticks = Ticks(Ticks::WHOLE.0 * 999);

fn yes() -> bool {
    true
}
fn one() -> u32 {
    1
}
fn hundred() -> u8 {
    100
}
fn next() -> FollowAction {
    FollowAction::Next
}
fn bar() -> Ticks {
    Ticks::WHOLE
}
fn is_yes(value: &bool) -> bool {
    *value
}

/// A clip's or a scene's follow actions: the Follow Action switch, actions
/// A and B with the chance that splits them, and the follow time.
///
/// On a scene, `linked` is the Longest button: the time is the multiplier
/// times the longest clip of the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FollowActions {
    /// The Follow Action switch. Off, nothing runs; off is the default.
    #[serde(default)]
    pub enabled: bool,
    /// Action A, which runs with the chance `chance_a`. Next by default.
    #[serde(default = "next")]
    pub action_a: FollowAction,
    /// Action B, which runs the rest of the time. No Action by default.
    #[serde(default)]
    pub action_b: FollowAction,
    /// The slot (scene) number action A's Jump goes to, counted from 1; 0
    /// leaves it empty, and a jump to an empty target stops the clip.
    #[serde(default = "one")]
    pub jump_a: u32,
    /// The slot (scene) number action B's Jump goes to, as `jump_a`.
    #[serde(default = "one")]
    pub jump_b: u32,
    /// The chance of action A in whole percent, 0 to 100. Action B has the
    /// rest, so the two always add up to 100.
    #[serde(default = "hundred")]
    pub chance_a: u8,
    /// Linked: the follow time is a multiple of the clip's length (the
    /// multiplier counts loops, and a clip whose Loop switch is off follows
    /// at its end). Unlinked: `time`. On by default.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub linked: bool,
    /// The Linked multiplier, a whole number from 1.
    #[serde(default = "one")]
    pub multiplier: u32,
    /// The Unlinked follow time in ticks (bars.beats.sixteenths), from a
    /// sixteenth; one 4/4 bar by default.
    #[serde(default = "bar")]
    pub time: Ticks,
}

impl Default for FollowActions {
    fn default() -> Self {
        Self {
            enabled: false,
            action_a: FollowAction::Next,
            action_b: FollowAction::NoAction,
            jump_a: 1,
            jump_b: 1,
            chance_a: 100,
            linked: true,
            multiplier: 1,
            time: Ticks::WHOLE,
        }
    }
}

impl FollowActions {
    /// Whether these are the defaults, so a saved clip or scene leaves them
    /// out.
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The chance of action B, in whole percent.
    #[must_use]
    pub fn chance_b(&self) -> u8 {
        100 - self.chance_a
    }

    /// Changes the fields `patch` names. A multiplier, a time or a jump
    /// target past its limits is held at the limit, as the fields do;
    /// `targets` is how many slots (scenes) a jump can name.
    ///
    /// # Errors
    ///
    /// [`ModelError::InvalidValue`] if `chance_a` is over 100.
    pub fn apply(&mut self, patch: &FollowPatch, targets: usize) -> Result<(), ModelError> {
        if let Some(chance) = patch.chance_a
            && chance > 100
        {
            return Err(ModelError::InvalidValue {
                what: "the chance of action A, in percent",
                value: f64::from(chance),
            });
        }
        let jump = |target: u32| target.min(u32::try_from(targets).unwrap_or(u32::MAX));
        let next = Self {
            enabled: patch.enabled.unwrap_or(self.enabled),
            action_a: patch.action_a.unwrap_or(self.action_a),
            action_b: patch.action_b.unwrap_or(self.action_b),
            jump_a: patch.jump_a.map_or(self.jump_a, jump),
            jump_b: patch.jump_b.map_or(self.jump_b, jump),
            chance_a: patch.chance_a.unwrap_or(self.chance_a),
            linked: patch.linked.unwrap_or(self.linked),
            multiplier: patch
                .multiplier
                .map_or(self.multiplier, |m| m.clamp(1, FOLLOW_MULTIPLIER_MAX)),
            time: patch
                .time
                .map_or(self.time, |t| t.clamp(FOLLOW_TIME_MIN, FOLLOW_TIME_MAX)),
        };
        *self = next;
        Ok(())
    }
}

/// The fields of [`FollowActions`] a `set_follow_actions` command changes;
/// those left out stay as they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FollowPatch {
    /// The Follow Action switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Action A.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_a: Option<FollowAction>,
    /// Action B.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_b: Option<FollowAction>,
    /// The slot (scene) number action A's Jump goes to: 1 up to the number
    /// of scenes, and 0 for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump_a: Option<u32>,
    /// The slot (scene) number action B's Jump goes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump_b: Option<u32>,
    /// The chance of action A in whole percent, 0 to 100; B has the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chance_a: Option<u8>,
    /// Linked (on a scene, Longest) or Unlinked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
    /// The Linked multiplier, a whole number from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiplier: Option<u32>,
    /// The Unlinked follow time in ticks, from a sixteenth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<Ticks>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_observed_ones() {
        // Spec: follow-actions R1
        let follow = FollowActions::default();
        assert!(!follow.enabled);
        assert_eq!(
            (follow.action_a, follow.action_b),
            (FollowAction::Next, FollowAction::NoAction)
        );
        assert_eq!((follow.chance_a, follow.chance_b()), (100, 0));
        assert!(follow.linked);
        assert_eq!((follow.multiplier, follow.time), (1, Ticks::WHOLE));
        assert!(follow.is_default());
    }

    #[test]
    fn a_patch_changes_what_it_names_and_holds_values_at_their_limits() {
        // Spec: follow-actions R3, R4, R7
        let mut follow = FollowActions::default();
        follow
            .apply(
                &FollowPatch {
                    enabled: Some(true),
                    jump_a: Some(99),
                    jump_b: Some(0),
                    multiplier: Some(0),
                    time: Some(Ticks::ZERO),
                    chance_a: Some(34),
                    ..FollowPatch::default()
                },
                8,
            )
            .unwrap();
        assert!(follow.enabled);
        assert_eq!((follow.jump_a, follow.jump_b), (8, 0));
        assert_eq!(follow.multiplier, 1);
        assert_eq!(follow.time, FOLLOW_TIME_MIN);
        assert_eq!((follow.chance_a, follow.chance_b()), (34, 66));
        follow
            .apply(
                &FollowPatch {
                    multiplier: Some(u32::MAX),
                    ..FollowPatch::default()
                },
                8,
            )
            .unwrap();
        assert_eq!(follow.multiplier, FOLLOW_MULTIPLIER_MAX);
        let before = follow;
        assert!(
            follow
                .apply(
                    &FollowPatch {
                        chance_a: Some(101),
                        enabled: Some(false),
                        ..FollowPatch::default()
                    },
                    8
                )
                .is_err()
        );
        assert_eq!(follow, before, "a refused patch changes nothing");
    }
}
