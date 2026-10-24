//! A tool described as data: its name, kind and parameters (the MIDI Tools
//! spec, R5).

use bl_core::time::Ticks;

use crate::registry::ToolError;

/// What a tool does with notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    /// Takes notes and changes them.
    Transformation,
    /// Writes new notes.
    Generator,
}

/// A tool: its ID, its name, its kind and its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDescriptor {
    /// The ID: the name in lower case with words joined by underscores
    /// (`arpeggiate`, `time_warp`).
    pub id: String,
    /// The name the selector shows.
    pub name: String,
    /// Whether it changes notes or writes them.
    pub kind: ToolKind,
    /// The parameters, in the order the panel shows them.
    pub parameters: Vec<ParamSpec>,
}

impl ToolDescriptor {
    /// Every parameter's default, in order.
    #[must_use]
    pub fn defaults(&self) -> Vec<ParamValue> {
        self.parameters
            .iter()
            .map(|spec| spec.default.clone())
            .collect()
    }

    /// Whether `values` are one value per parameter, each of its kind and
    /// inside its range.
    ///
    /// # Errors
    ///
    /// [`ToolError::Values`] saying which value is wrong.
    pub fn check(&self, values: &[ParamValue]) -> Result<(), ToolError> {
        if values.len() != self.parameters.len() {
            return Err(ToolError::Values(format!(
                "{} takes {} values, not {}",
                self.name,
                self.parameters.len(),
                values.len()
            )));
        }
        for (spec, value) in self.parameters.iter().zip(values) {
            spec.check(value).map_err(ToolError::Values)?;
        }
        Ok(())
    }

    /// The parameter with this ID, and its place.
    #[must_use]
    pub fn parameter(&self, id: &str) -> Option<(usize, &ParamSpec)> {
        self.parameters
            .iter()
            .enumerate()
            .find(|(_, spec)| spec.id == id)
    }
}

/// One parameter of a tool.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamSpec {
    /// The ID, in lower case with words joined by underscores.
    pub id: String,
    /// The name the panel shows.
    pub name: String,
    /// The kind of value, with its range.
    pub kind: ParamKind,
    /// The value a new set starts with and Reset restores.
    pub default: ParamValue,
    /// The unit the value is shown with, such as "%" or "st".
    pub unit: Option<String>,
    /// Whether the parameter counts scale degrees where the clip has a
    /// scale (the MIDI Tools spec, R8).
    pub scale_aware: bool,
    /// What the Info View shows while the pointer is over the control.
    pub description: String,
}

impl ParamSpec {
    /// A parameter with no unit that isn't scale-aware.
    #[must_use]
    pub fn new(
        id: &str,
        name: &str,
        kind: ParamKind,
        default: ParamValue,
        description: &str,
    ) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            kind,
            default,
            unit: None,
            scale_aware: false,
            description: description.to_owned(),
        }
    }

    /// This parameter, shown with a unit.
    #[must_use]
    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = Some(unit.to_owned());
        self
    }

    /// This parameter, counting scale degrees.
    #[must_use]
    pub fn scale_aware(mut self) -> Self {
        self.scale_aware = true;
        self
    }

    /// Whether `value` is of this parameter's kind and inside its range.
    ///
    /// # Errors
    ///
    /// Why it isn't.
    pub fn check(&self, value: &ParamValue) -> Result<(), String> {
        let wrong = |what: &str| Err(format!("`{}` takes {what}", self.id));
        match (&self.kind, value) {
            (ParamKind::Number { min, max, .. }, ParamValue::Number(n)) => {
                if n.is_finite() && (*min..=*max).contains(n) {
                    Ok(())
                } else {
                    wrong(&format!("a number from {min} to {max}"))
                }
            }
            (ParamKind::Whole { min, max }, ParamValue::Whole(n)) => {
                if (*min..=*max).contains(n) {
                    Ok(())
                } else {
                    wrong(&format!("a whole number from {min} to {max}"))
                }
            }
            (ParamKind::Choice { options }, ParamValue::Choice(n)) => {
                if *n < options.len() {
                    Ok(())
                } else {
                    wrong("one of its options")
                }
            }
            (ParamKind::Switch, ParamValue::Switch(_)) => Ok(()),
            (ParamKind::Pitch, ParamValue::Pitch(key)) => {
                if *key <= 127 {
                    Ok(())
                } else {
                    wrong("a key from 0 to 127")
                }
            }
            (ParamKind::PitchRange, ParamValue::PitchRange { low, high }) => {
                if low <= high && *high <= 127 {
                    Ok(())
                } else {
                    wrong("a low key at or below a high key, from 0 to 127")
                }
            }
            (ParamKind::Division, ParamValue::Division(n)) => {
                if *n < DIVISION_NAMES.len() {
                    Ok(())
                } else {
                    wrong("one of the note divisions")
                }
            }
            (ParamKind::Breakpoints { min, max }, ParamValue::Breakpoints(points)) => {
                let ordered = points.windows(2).all(|pair| pair[0].x < pair[1].x);
                let inside = points.iter().all(|p| {
                    (0.0..=1.0).contains(&p.x) && p.y.is_finite() && (*min..=*max).contains(&p.y)
                });
                if ordered && inside {
                    Ok(())
                } else {
                    wrong(&format!(
                        "points in order of x from 0 to 1, with y from {min} to {max}"
                    ))
                }
            }
            (ParamKind::Levels { count, min, max }, ParamValue::Levels(levels)) => {
                if levels.len() == *count
                    && levels
                        .iter()
                        .all(|l| l.is_finite() && (*min..=*max).contains(l))
                {
                    Ok(())
                } else {
                    wrong(&format!("{count} levels from {min} to {max}"))
                }
            }
            (ParamKind::Toggles { count }, ParamValue::Toggles(toggles)) => {
                if toggles.len() == *count {
                    Ok(())
                } else {
                    wrong(&format!("{count} toggles"))
                }
            }
            (ParamKind::Structured, ParamValue::Structured(_)) => Ok(()),
            _ => wrong("a value of its own kind"),
        }
    }
}

/// The kind of a parameter's value, with its range.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamKind {
    /// A number from `min` to `max`, shown with `decimals` decimals.
    Number {
        /// The least value.
        min: f64,
        /// The greatest value.
        max: f64,
        /// How many decimals the control shows.
        decimals: u8,
    },
    /// A whole number from `min` to `max`.
    Whole {
        /// The least value.
        min: i64,
        /// The greatest value.
        max: i64,
    },
    /// One of a list of options.
    Choice {
        /// The options, in order.
        options: Vec<ChoiceOption>,
    },
    /// On or off.
    Switch,
    /// A key, 0 to 127.
    Pitch,
    /// A low key and a high key.
    PitchRange,
    /// One of the note divisions ([`DIVISION_NAMES`]).
    Division,
    /// Points `x` from 0 to 1 and `y` from `min` to `max`, in order of `x`:
    /// a display's data.
    Breakpoints {
        /// The least `y`.
        min: f64,
        /// The greatest `y`.
        max: f64,
    },
    /// `count` levels from `min` to `max`: a display's data.
    Levels {
        /// How many levels.
        count: usize,
        /// The least level.
        min: f64,
        /// The greatest level.
        max: f64,
    },
    /// `count` switches: a display's data.
    Toggles {
        /// How many.
        count: usize,
    },
    /// A structured value of the tool's own, as text the tool reads (Stacks'
    /// progression, T255).
    Structured,
}

/// One option of a [`ParamKind::Choice`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceOption {
    /// The ID, in lower case with words joined by underscores.
    pub id: String,
    /// The name the chooser shows.
    pub name: String,
}

impl ChoiceOption {
    /// An option.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
        }
    }
}

/// A point of a [`ParamKind::Breakpoints`] value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Breakpoint {
    /// Where, from 0 to 1.
    pub x: f64,
    /// The value.
    pub y: f64,
}

/// A parameter's value.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    /// A [`ParamKind::Number`].
    Number(f64),
    /// A [`ParamKind::Whole`].
    Whole(i64),
    /// A [`ParamKind::Choice`]: the option's place in the list.
    Choice(usize),
    /// A [`ParamKind::Switch`].
    Switch(bool),
    /// A [`ParamKind::Pitch`].
    Pitch(u8),
    /// A [`ParamKind::PitchRange`].
    PitchRange {
        /// The low key.
        low: u8,
        /// The high key.
        high: u8,
    },
    /// A [`ParamKind::Division`]: the division's place in
    /// [`DIVISION_NAMES`].
    Division(usize),
    /// A [`ParamKind::Breakpoints`].
    Breakpoints(Vec<Breakpoint>),
    /// A [`ParamKind::Levels`].
    Levels(Vec<f64>),
    /// A [`ParamKind::Toggles`].
    Toggles(Vec<bool>),
    /// A [`ParamKind::Structured`].
    Structured(String),
}

/// The note divisions a [`ParamKind::Division`] offers, in order: 1 is a
/// whole note, 1/n a whole note divided by n, a T value a triplet (the MIDI
/// transformations spec, R1).
pub const DIVISION_NAMES: [&str; 13] = [
    "1", "1/2", "1/2T", "1/4", "1/4T", "1/8", "1/8T", "1/16", "1/16T", "1/32", "1/32T", "1/64",
    "1/128",
];

/// How long the division at `index` of [`DIVISION_NAMES`] is: a whole note
/// divided by n, and two-thirds of that for a triplet.
///
/// # Panics
///
/// If `index` is past the list.
#[must_use]
pub fn division_ticks(index: usize) -> Ticks {
    const DIVIDERS: [i64; 13] = [1, 2, 2, 4, 4, 8, 8, 16, 16, 32, 32, 64, 128];
    let plain = Ticks::WHOLE.0 / DIVIDERS[index];
    // A whole note divides into every one of these without a remainder, and
    // by three for the triplets, so the ticks are exact.
    Ticks(if DIVISION_NAMES[index].ends_with('T') {
        plain * 2 / 3
    } else {
        plain
    })
}
