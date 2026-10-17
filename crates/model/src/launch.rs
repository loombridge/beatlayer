//! The global launch quantisation (WP-3.2): the grid a clip or scene launch
//! waits for, and the steps the Control Bar's Quantization Menu offers.
//!
//! The value and its switch are saved with the set like the view settings
//! (the project holds them, not the document), so changing them is no undo
//! step. The rules are in the launch quantisation spec; the menu's steps and
//! its default were observed in Live 12.4.3.

use bl_core::time::{Ticks, TimeSignatureMap};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One step of the launch quantisation grid: how long a slot waits after a
/// launch or a stop is asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchQuantization {
    /// No quantisation: a launch or a stop takes effect at once (the
    /// menu's "None", S-GQ-005).
    None,
    /// Eight bars.
    #[serde(rename = "8_bars")]
    Bars8,
    /// Four bars.
    #[serde(rename = "4_bars")]
    Bars4,
    /// Two bars.
    #[serde(rename = "2_bars")]
    Bars2,
    /// One bar, the observed default.
    #[default]
    #[serde(rename = "1_bar")]
    Bar,
    /// A half note.
    Half,
    /// A half-note triplet.
    HalfTriplet,
    /// A quarter note.
    Quarter,
    /// A quarter-note triplet.
    QuarterTriplet,
    /// An eighth note.
    Eighth,
    /// An eighth-note triplet.
    EighthTriplet,
    /// A sixteenth note.
    Sixteenth,
    /// A sixteenth-note triplet.
    SixteenthTriplet,
    /// A thirty-second note.
    ThirtySecond,
}

impl LaunchQuantization {
    /// Every step, in the menu's order: the largest first, as the observed
    /// menu lists them.
    pub const ALL: [Self; 14] = [
        Self::None,
        Self::Bars8,
        Self::Bars4,
        Self::Bars2,
        Self::Bar,
        Self::Half,
        Self::HalfTriplet,
        Self::Quarter,
        Self::QuarterTriplet,
        Self::Eighth,
        Self::EighthTriplet,
        Self::Sixteenth,
        Self::SixteenthTriplet,
        Self::ThirtySecond,
    ];

    /// The step's name in the menu.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Bars8 => "8 Bars",
            Self::Bars4 => "4 Bars",
            Self::Bars2 => "2 Bars",
            Self::Bar => "1 Bar",
            Self::Half => "1/2",
            Self::HalfTriplet => "1/2T",
            Self::Quarter => "1/4",
            Self::QuarterTriplet => "1/4T",
            Self::Eighth => "1/8",
            Self::EighthTriplet => "1/8T",
            Self::Sixteenth => "1/16",
            Self::SixteenthTriplet => "1/16T",
            Self::ThirtySecond => "1/32",
        }
    }

    /// The step the menu moves to when the person makes quantisation
    /// smaller (S-SES-024, M-OPT-011). The smallest step stays.
    #[must_use]
    pub fn smaller(self) -> Self {
        let at = self.position();
        Self::ALL[(at + 1).min(Self::ALL.len() - 1)]
    }

    /// The step the menu moves to when the person makes quantisation larger
    /// (S-SES-025, M-OPT-012). The largest step stays.
    #[must_use]
    pub fn larger(self) -> Self {
        Self::ALL[self.position().saturating_sub(1)]
    }

    /// The same note value as a triplet, or this step when it has no
    /// triplet: the bar steps and None don't (S-SES-026, M-OPT-013).
    #[must_use]
    pub fn triplet(self) -> Self {
        match self {
            Self::Half => Self::HalfTriplet,
            Self::HalfTriplet => Self::Half,
            Self::Quarter => Self::QuarterTriplet,
            Self::QuarterTriplet => Self::Quarter,
            Self::Eighth => Self::EighthTriplet,
            Self::EighthTriplet => Self::Eighth,
            Self::Sixteenth => Self::SixteenthTriplet,
            Self::SixteenthTriplet => Self::Sixteenth,
            other => other,
        }
    }

    /// How many bars one step is, for the steps measured in bars.
    #[must_use]
    pub fn bars(self) -> Option<i64> {
        match self {
            Self::Bars8 => Some(8),
            Self::Bars4 => Some(4),
            Self::Bars2 => Some(2),
            Self::Bar => Some(1),
            _ => None,
        }
    }

    /// How long one step is, in ticks, with `bar` the length of a bar of
    /// the signature it falls in; `None` for no quantisation.
    #[must_use]
    pub fn ticks(self, bar: Ticks) -> Option<Ticks> {
        if let Some(bars) = self.bars() {
            return Some(bar * bars);
        }
        match self {
            Self::None => None,
            Self::Half => Some(Ticks::WHOLE / 2),
            Self::HalfTriplet => Some(Ticks::WHOLE / 3),
            Self::Quarter => Some(Ticks::QUARTER),
            Self::QuarterTriplet => Some(Ticks::QUARTER * 2 / 3),
            Self::Eighth => Some(Ticks::QUARTER / 2),
            Self::EighthTriplet => Some(Ticks::QUARTER / 3),
            Self::Sixteenth => Some(Ticks::SIXTEENTH),
            Self::SixteenthTriplet => Some(Ticks::QUARTER / 6),
            Self::ThirtySecond => Some(Ticks::SIXTEENTH / 2),
            Self::Bars8 | Self::Bars4 | Self::Bars2 | Self::Bar => unreachable!("a bar step"),
        }
    }

    /// The tick of the first grid line strictly after `from`: the line a
    /// launch or a stop asked for at `from` waits for, and `from` itself
    /// with no quantisation. Bars are counted from the start of the
    /// arrangement, so the "1 Bar" grid has a line at 1.1.1 and at every bar
    /// after it.
    #[must_use]
    pub fn next_line(self, from: Ticks, signatures: &TimeSignatureMap) -> Ticks {
        let from = from.max(Ticks::ZERO);
        if let Some(bars) = self.bars() {
            // The next bar whose index counts a whole number of steps from
            // the start of the arrangement.
            let (index, _) = signatures.bar_at(from);
            let mut bar = index + 1;
            let into = bar.rem_euclid(bars);
            bar += if into == 0 { 0 } else { bars - into };
            return signatures.bar_start(bar);
        }
        let bar = signatures.signature_at(from).bar_length();
        let Some(unit) = self.ticks(bar) else {
            return from;
        };
        let into = from.rem_euclid(unit);
        from + if into == Ticks::ZERO {
            unit
        } else {
            unit - into
        }
    }

    /// This step's place in the menu.
    fn position(self) -> usize {
        Self::ALL
            .iter()
            .position(|&step| step == self)
            .unwrap_or_default()
    }
}

/// How a Session clip's launch button behaves (the clip's Launch Mode, and
/// the Default Launch Mode setting).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    /// Pressing starts the clip; releasing does nothing. The observed
    /// default.
    #[default]
    Trigger,
    /// The clip plays while the button is held.
    Gate,
    /// Pressing starts the clip and the next press stops it.
    Toggle,
    /// The clip retriggers while the button is held.
    Repeat,
}

impl LaunchMode {
    /// Every mode, in the Launch panel's order.
    pub const ALL: [Self; 4] = [Self::Trigger, Self::Gate, Self::Toggle, Self::Repeat];

    /// The mode's name in the chooser.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Trigger => "Trigger",
            Self::Gate => "Gate",
            Self::Toggle => "Toggle",
            Self::Repeat => "Repeat",
        }
    }
}

/// A Session clip's own launch settings (the Clip View's Launch panel,
/// WP-3.4). They are saved with the clip, so they're part of the set and
/// each change is one undo step; a clip without them has the defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClipLaunch {
    /// Legato: the clip takes over the play position of the clip playing on
    /// its track when it launches.
    #[serde(default)]
    pub legato: bool,
    /// The clip's own launch quantisation; `null` (left out) follows the
    /// global one, which Live's chooser calls "Global".
    #[serde(default)]
    pub quantization: Option<LaunchQuantization>,
    /// How much the velocity of a note that launches the clip changes its
    /// volume, from 0 (none, the default) to 1 (100 %).
    #[serde(default)]
    pub velocity_amount: f64,
    /// What a press and a release of the clip's launch button do (WP-3.3).
    #[serde(default)]
    pub mode: LaunchMode,
}

impl ClipLaunch {
    /// Whether these are the defaults, so a clip's saved form leaves them
    /// out.
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The global launch quantisation and its enabled switch (L-CTRL-010,
/// M-OPT-014): saved with the set, changed by the Options items and their
/// shortcuts, and no undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct LaunchSettings {
    /// The step launches and stops wait for.
    pub quantization: LaunchQuantization,
    /// Whether launches and stops are quantised at all. Off, they happen at
    /// once, whatever the step says.
    pub enabled: bool,
}

impl Default for LaunchSettings {
    fn default() -> Self {
        Self {
            quantization: LaunchQuantization::default(),
            enabled: true,
        }
    }
}

impl LaunchSettings {
    /// The tick of the grid line a launch or a stop asked for at `from`
    /// waits for, and `from` itself while quantisation is off.
    #[must_use]
    pub fn next_line(self, from: Ticks, signatures: &TimeSignatureMap) -> Ticks {
        if !self.enabled {
            return from.max(Ticks::ZERO);
        }
        self.quantization.next_line(from, signatures)
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::{TimeSignature, TimeSignatureChange};

    use super::*;

    fn four_four() -> TimeSignatureMap {
        TimeSignatureMap::default()
    }

    #[test]
    fn the_menu_moves_one_step_at_a_time_and_stops_at_its_ends() {
        // Spec: launch-quantisation R5
        assert_eq!(LaunchQuantization::Bar.smaller(), LaunchQuantization::Half);
        assert_eq!(LaunchQuantization::Bar.larger(), LaunchQuantization::Bars2);
        assert_eq!(
            LaunchQuantization::None.smaller(),
            LaunchQuantization::Bars8
        );
        assert_eq!(LaunchQuantization::None.larger(), LaunchQuantization::None);
        assert_eq!(
            LaunchQuantization::ThirtySecond.smaller(),
            LaunchQuantization::ThirtySecond
        );
        assert_eq!(
            LaunchQuantization::HalfTriplet.triplet(),
            LaunchQuantization::Half
        );
        assert_eq!(
            LaunchQuantization::Sixteenth.triplet(),
            LaunchQuantization::SixteenthTriplet
        );
        assert_eq!(
            LaunchQuantization::Bar.triplet(),
            LaunchQuantization::Bar,
            "a bar step has no triplet"
        );
    }

    #[test]
    fn a_line_falls_every_step_strictly_after_the_position() {
        // Spec: launch-quantisation R2
        let sigs = four_four();
        let bar = Ticks::WHOLE;
        // 3.2.1: a bar and a beat in.
        let at = bar * 2 + Ticks::QUARTER;
        assert_eq!(
            LaunchQuantization::Bar.next_line(at, &sigs),
            bar * 3,
            "the next bar line"
        );
        assert_eq!(
            LaunchQuantization::Bars2.next_line(at, &sigs),
            bar * 4,
            "the next two-bar line"
        );
        assert_eq!(
            LaunchQuantization::Half.next_line(at, &sigs),
            bar * 2 + Ticks::QUARTER * 2,
            "the next half note"
        );
        assert_eq!(
            LaunchQuantization::SixteenthTriplet.next_line(at, &sigs),
            at + Ticks::QUARTER / 6,
            "a sixteenth triplet after the position"
        );
        // Exactly on a line waits for the next one.
        assert_eq!(LaunchQuantization::Bar.next_line(bar * 3, &sigs), bar * 4);
        assert_eq!(
            LaunchQuantization::None.next_line(at, &sigs),
            at,
            "no quantisation takes effect at once"
        );
    }

    #[test]
    fn bar_lines_follow_a_changing_meter() {
        // Spec: launch-quantisation R2
        let sigs = TimeSignatureMap::new([
            TimeSignatureChange {
                at: Ticks::ZERO,
                signature: TimeSignature::FOUR_FOUR,
            },
            TimeSignatureChange {
                at: Ticks::WHOLE * 2,
                signature: TimeSignature::new(3, 4).unwrap(),
            },
        ])
        .unwrap();
        // A 4/4 bar is four quarters; the 3/4 bars after 3.1.1 are three.
        assert_eq!(
            LaunchQuantization::Bar.next_line(Ticks::WHOLE + Ticks::QUARTER, &sigs),
            Ticks::WHOLE * 2,
            "the first 3/4 bar starts at 3.1.1"
        );
        assert_eq!(
            LaunchQuantization::Bar.next_line(Ticks::WHOLE * 2, &sigs),
            Ticks::WHOLE * 2 + Ticks::QUARTER * 3,
            "the next 3/4 bar"
        );
    }

    #[test]
    fn the_switch_off_takes_effect_at_once() {
        // Spec: launch-quantisation R3
        let sigs = four_four();
        let settings = LaunchSettings::default();
        assert!(settings.enabled);
        assert_eq!(settings.quantization, LaunchQuantization::Bar);
        let off = LaunchSettings {
            enabled: false,
            ..settings
        };
        assert_eq!(off.next_line(Ticks::QUARTER, &sigs), Ticks::QUARTER);
    }
}
