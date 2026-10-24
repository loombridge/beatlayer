//! Stacks' chord banks (the MIDI generators spec, R24 to R27): the file
//! format Beatlayer reads and writes, and the bank built into Beatlayer.
//!
//! A bank file (`.blstacks`) is UTF-8 JSON:
//!
//! ```json
//! {"format":"beatlayer-stacks","schema":1,"name":"My Chords",
//!  "chords":[{"name":"Triad","degrees":[0,2,4]},
//!            {"name":"Major","semitones":[0,4,7]}]}
//! ```

use std::fmt;
use std::sync::OnceLock;

use serde_json::Value;

/// A bank file's extension.
pub const BANK_EXTENSION: &str = "blstacks";
/// The `format` field every bank file holds.
pub const BANK_FORMAT: &str = "beatlayer-stacks";
/// The schema this build writes and reads.
pub const BANK_SCHEMA: u64 = 1;
/// The largest bank file read, in bytes (R25).
pub const MAX_BANK_BYTES: usize = 1 << 20;
/// The most entries a bank holds (R24).
pub const MAX_ENTRIES: usize = 256;
/// The most notes a chord entry holds, and the fewest.
pub const MAX_CHORD_NOTES: usize = 8;
const MIN_CHORD_NOTES: usize = 2;
/// The greatest interval, in semitones or degrees.
const MAX_INTERVAL: u64 = 48;
/// The most characters an entry's name holds.
const MAX_NAME: usize = 64;

/// One chord of a bank: a name and the intervals of its notes above the
/// root, in semitones or in degrees of a scale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordEntry {
    /// The chord's name.
    pub name: String,
    /// Whether `intervals` are degrees of a scale rather than semitones.
    pub degrees: bool,
    /// The intervals: 2 to 8 rising whole numbers, the first 0.
    pub intervals: Vec<u8>,
}

impl ChordEntry {
    fn new(name: &str, degrees: bool, intervals: &[u8]) -> Self {
        Self {
            name: name.to_owned(),
            degrees,
            intervals: intervals.to_vec(),
        }
    }
}

/// A bank of chords.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordBank {
    /// The bank's name.
    pub name: String,
    /// Its chords, 1 to [`MAX_ENTRIES`], in order.
    pub chords: Vec<ChordEntry>,
}

/// Why a bank file didn't load: what is wrong and where (R25).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BankError {
    /// Where, such as `chords[3] "Sus"`; empty for the whole file.
    pub place: String,
    /// What is wrong.
    pub reason: String,
}

impl fmt::Display for BankError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.place.is_empty() {
            f.write_str(&self.reason)
        } else {
            write!(f, "{}: {}", self.place, self.reason)
        }
    }
}

impl std::error::Error for BankError {}

fn broken(place: &str, reason: impl Into<String>) -> BankError {
    BankError {
        place: place.to_owned(),
        reason: reason.into(),
    }
}

/// The built-in bank "Beatlayer Chords" (R27).
#[must_use]
pub fn built_in() -> &'static ChordBank {
    static BANK: OnceLock<ChordBank> = OnceLock::new();
    BANK.get_or_init(|| {
        const DEGREES: [(&str, &[u8]); 9] = [
            ("Triad", &[0, 2, 4]),
            ("Seventh", &[0, 2, 4, 6]),
            ("Ninth", &[0, 2, 4, 6, 8]),
            ("Sixth", &[0, 2, 4, 5]),
            ("Add Nine", &[0, 2, 4, 8]),
            ("Sus2", &[0, 1, 4]),
            ("Sus4", &[0, 3, 4]),
            ("Open Triad", &[0, 4, 9]),
            ("Fifth", &[0, 4]),
        ];
        const SEMITONES: [(&str, &[u8]); 18] = [
            ("Major", &[0, 4, 7]),
            ("Minor", &[0, 3, 7]),
            ("Diminished", &[0, 3, 6]),
            ("Augmented", &[0, 4, 8]),
            ("Suspended 2", &[0, 2, 7]),
            ("Suspended 4", &[0, 5, 7]),
            ("Major 7", &[0, 4, 7, 11]),
            ("Minor 7", &[0, 3, 7, 10]),
            ("Dominant 7", &[0, 4, 7, 10]),
            ("Half-Diminished 7", &[0, 3, 6, 10]),
            ("Diminished 7", &[0, 3, 6, 9]),
            ("Minor-Major 7", &[0, 3, 7, 11]),
            ("Major 6", &[0, 4, 7, 9]),
            ("Minor 6", &[0, 3, 7, 9]),
            ("Major 9", &[0, 4, 7, 11, 14]),
            ("Minor 9", &[0, 3, 7, 10, 14]),
            ("Dominant 9", &[0, 4, 7, 10, 14]),
            ("Power", &[0, 7]),
        ];
        ChordBank {
            name: "Beatlayer Chords".to_owned(),
            chords: DEGREES
                .iter()
                .map(|(name, intervals)| ChordEntry::new(name, true, intervals))
                .chain(
                    SEMITONES
                        .iter()
                        .map(|(name, intervals)| ChordEntry::new(name, false, intervals)),
                )
                .collect(),
        }
    })
}

impl ChordBank {
    /// The bank a file's bytes hold (R24, R25). Fields the format doesn't
    /// name are ignored.
    ///
    /// # Errors
    ///
    /// [`BankError`] saying what is wrong and where; nothing is loaded.
    pub fn parse(bytes: &[u8]) -> Result<Self, BankError> {
        if bytes.len() > MAX_BANK_BYTES {
            return Err(broken(
                "",
                format!(
                    "the file is {} bytes; a chord bank holds at most {MAX_BANK_BYTES}",
                    bytes.len()
                ),
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|error| {
            broken(
                &format!("line {}, column {}", error.line(), error.column()),
                format!("not JSON: {error}"),
            )
        })?;
        let Some(object) = value.as_object() else {
            return Err(broken("", "the file isn't a JSON object"));
        };
        if object.get("format").and_then(Value::as_str) != Some(BANK_FORMAT) {
            return Err(broken(
                "format",
                format!("must be \"{BANK_FORMAT}\" for a chord bank"),
            ));
        }
        match object.get("schema").and_then(Value::as_u64) {
            Some(BANK_SCHEMA) => {}
            Some(newer) if newer > BANK_SCHEMA => {
                return Err(broken(
                    "schema",
                    format!("{newer} is newer than the {BANK_SCHEMA} this Beatlayer reads"),
                ));
            }
            _ => return Err(broken("schema", format!("must be {BANK_SCHEMA}"))),
        }
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| broken("name", "must be text"))?;
        let list = object
            .get("chords")
            .and_then(Value::as_array)
            .ok_or_else(|| broken("chords", "must be a list"))?;
        if list.is_empty() || list.len() > MAX_ENTRIES {
            return Err(broken(
                "chords",
                format!(
                    "holds {} entries; a bank holds 1 to {MAX_ENTRIES}",
                    list.len()
                ),
            ));
        }
        let chords = list
            .iter()
            .enumerate()
            .map(|(at, entry)| entry_of(at, entry))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            name: name.to_owned(),
            chords,
        })
    }

    /// The bank as a file holds it (R24): the fields in the order the format
    /// lists them, one entry to a line.
    #[must_use]
    pub fn write(&self) -> String {
        let text = |text: &str| Value::from(text).to_string();
        let list = |intervals: &[u8]| {
            intervals
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let entries: Vec<String> = self
            .chords
            .iter()
            .map(|entry| {
                format!(
                    "    {{\"name\": {}, \"{}\": [{}]}}",
                    text(&entry.name),
                    if entry.degrees {
                        "degrees"
                    } else {
                        "semitones"
                    },
                    list(&entry.intervals)
                )
            })
            .collect();
        format!(
            "{{\n  \"format\": \"{BANK_FORMAT}\",\n  \"schema\": {BANK_SCHEMA},\n  \"name\": {},\n  \
             \"chords\": [\n{}\n  ]\n}}\n",
            text(&self.name),
            entries.join(",\n")
        )
    }

    /// Whether this is the built-in bank.
    #[must_use]
    pub fn is_built_in(&self) -> bool {
        self == built_in()
    }
}

/// The entry at place `at` of the list (R24).
fn entry_of(at: usize, entry: &Value) -> Result<ChordEntry, BankError> {
    let named = entry
        .get("name")
        .and_then(Value::as_str)
        .map_or_else(String::new, |name| format!(" \"{name}\""));
    let place = format!("chords[{at}]{named}");
    let Some(object) = entry.as_object() else {
        return Err(broken(&place, "must be an object"));
    };
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| broken(&place, "has no name"))?;
    let length = name.chars().count();
    if !(1..=MAX_NAME).contains(&length) {
        return Err(broken(
            &place,
            format!("its name has {length} characters; it takes 1 to {MAX_NAME}"),
        ));
    }
    let (degrees, list) = match (object.get("semitones"), object.get("degrees")) {
        (Some(list), None) => (false, list),
        (None, Some(list)) => (true, list),
        (Some(_), Some(_)) => return Err(broken(&place, "has both semitones and degrees")),
        (None, None) => return Err(broken(&place, "has neither semitones nor degrees")),
    };
    let field = if degrees { "degrees" } else { "semitones" };
    let Some(list) = list.as_array() else {
        return Err(broken(&place, format!("{field} must be a list")));
    };
    if !(MIN_CHORD_NOTES..=MAX_CHORD_NOTES).contains(&list.len()) {
        return Err(broken(
            &place,
            format!(
                "{field} holds {} numbers; a chord takes {MIN_CHORD_NOTES} to {MAX_CHORD_NOTES}",
                list.len()
            ),
        ));
    }
    let mut intervals: Vec<u8> = Vec::with_capacity(list.len());
    for (n, number) in list.iter().enumerate() {
        let value = number
            .as_u64()
            .filter(|value| *value <= MAX_INTERVAL)
            .and_then(|value| u8::try_from(value).ok())
            .ok_or_else(|| {
                broken(
                    &place,
                    format!("{field}[{n}] must be a whole number from 0 to {MAX_INTERVAL}"),
                )
            })?;
        match intervals.last() {
            None if value != 0 => {
                return Err(broken(&place, format!("{field}[0] must be 0")));
            }
            Some(last) if value <= *last => {
                return Err(broken(
                    &place,
                    format!("{field}[{n}] must be larger than the one before"),
                ));
            }
            _ => intervals.push(value),
        }
    }
    Ok(ChordEntry {
        name: name.to_owned(),
        degrees,
        intervals,
    })
}
