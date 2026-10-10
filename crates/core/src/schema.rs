//! JSON Schemas for the core types, for the command registry (WP-0.14,
//! decision D17).
//!
//! Commands carry these types, and every command publishes a JSON Schema of
//! its arguments so an agent knows what to send. Each schema here describes
//! exactly what the type's serde form accepts, including the checks its
//! deserialiser makes (a tempo from 1 to 1,000 BPM, a denominator of 1, 2,
//! 4, 8, or 16), and says what the numbers mean, since an agent can't read
//! the Rust docs.
//!
//! The impls are written by hand rather than derived because most of these
//! types serialise through a different form (`try_from`, `into`, or a
//! private field), which a derive would describe wrongly.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

use crate::id::Id;
use crate::time::{
    Bpm, Frames, SampleRate, TICKS_PER_QUARTER, TempoMap, TempoSegment, Ticks, TimeSignature,
    TimeSignatureChange, TimeSignatureMap,
};

/// Implements `schema_name` and `schema_id` from the type's path.
macro_rules! named {
    ($name:literal, $path:literal) => {
        fn schema_name() -> Cow<'static, str> {
            Cow::Borrowed($name)
        }

        fn schema_id() -> Cow<'static, str> {
            Cow::Borrowed($path)
        }
    };
}

impl JsonSchema for Id {
    named!("Id", "bl_core::id::Id");

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "An object's ID: a positive integer, unique within the project and never reused.",
            "type": "integer",
            "format": "uint64",
            "minimum": 1
        })
    }
}

impl JsonSchema for Ticks {
    named!("Ticks", "bl_core::time::Ticks");

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let description = format!(
            "A musical position or length in ticks: {} to the quarter note, so a sixteenth note \
             is {}, a beat in 4/4 is {}, and a 4/4 bar is {}. Position 0 is the start of the \
             arrangement (bar 1, beat 1); positions before it are negative.",
            thousands(TICKS_PER_QUARTER),
            thousands(TICKS_PER_QUARTER / 4),
            thousands(TICKS_PER_QUARTER),
            thousands(TICKS_PER_QUARTER * 4),
        );
        json_schema!({
            "description": description,
            "type": "integer",
            "format": "int64"
        })
    }
}

impl JsonSchema for Frames {
    named!("Frames", "bl_core::time::Frames");

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A position or length on a device's sample clock, in frames (one sample on every channel). Frame 0 is tick 0.",
            "type": "integer",
            "format": "int64"
        })
    }
}

impl JsonSchema for SampleRate {
    named!("SampleRate", "bl_core::time::SampleRate");

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A sample rate in hertz, such as 48000.",
            "type": "integer",
            "format": "uint32",
            "minimum": 1
        })
    }
}

impl JsonSchema for Bpm {
    named!("Bpm", "bl_core::time::Bpm");

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A tempo in quarter notes per minute, whatever the time signature.",
            "type": "number",
            "minimum": Bpm::MIN.get(),
            "maximum": Bpm::MAX.get()
        })
    }
}

impl JsonSchema for TempoSegment {
    named!("TempoSegment", "bl_core::time::TempoSegment");

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let ticks = generator.subschema_for::<Ticks>();
        let bpm = generator.subschema_for::<Bpm>();
        json_schema!({
            "description": "One piece of a tempo map. It runs from its start to the next segment's start; the last one runs on for ever.",
            "oneOf": [
                {
                    "description": "A steady tempo.",
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "const": "constant" },
                        "start": ticks,
                        "bpm": bpm
                    },
                    "required": ["kind", "start", "bpm"]
                },
                {
                    "description": "A tempo that changes in a straight line against musical position, from `from` at `start` to `to` at the next segment's start. The next segment may start at a different tempo, which makes a jump.",
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "const": "ramp" },
                        "start": ticks,
                        "from": bpm,
                        "to": bpm
                    },
                    "required": ["kind", "start", "from", "to"]
                }
            ]
        })
    }
}

impl JsonSchema for TempoMap {
    named!("TempoMap", "bl_core::time::TempoMap");

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let segment = generator.subschema_for::<TempoSegment>();
        json_schema!({
            "description": "The song's tempo over time, as segments in order of position. The first starts at tick 0, each starts after the one before it, and the last is constant. A song at one tempo is a single constant segment.",
            "type": "array",
            "items": segment,
            "minItems": 1
        })
    }
}

impl JsonSchema for TimeSignature {
    named!("TimeSignature", "bl_core::time::TimeSignature");

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A time signature: `numerator` beats to the bar, each 1/`denominator` of a whole note. 4/4 is numerator 4, denominator 4.",
            "type": "object",
            "properties": {
                "numerator": {
                    "description": "Beats to the bar.",
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 99
                },
                "denominator": {
                    "description": "The note value of a beat, as a fraction of a whole note.",
                    "type": "integer",
                    "enum": [1, 2, 4, 8, 16]
                }
            },
            "required": ["numerator", "denominator"]
        })
    }
}

impl JsonSchema for TimeSignatureChange {
    named!("TimeSignatureChange", "bl_core::time::TimeSignatureChange");

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let ticks = generator.subschema_for::<Ticks>();
        let signature = generator.subschema_for::<TimeSignature>();
        json_schema!({
            "description": "A time signature that takes effect at a position. A new bar always starts there.",
            "type": "object",
            "properties": {
                "at": ticks,
                "signature": signature
            },
            "required": ["at", "signature"]
        })
    }
}

impl JsonSchema for TimeSignatureMap {
    named!("TimeSignatureMap", "bl_core::time::TimeSignatureMap");

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let change = generator.subschema_for::<TimeSignatureChange>();
        json_schema!({
            "description": "The song's time signatures over time, as changes in order of position. The first is at tick 0 and positions increase. Each change starts a new bar; one that comes before the previous bar is full cuts that bar short.",
            "type": "array",
            "items": change,
            "minItems": 1
        })
    }
}

/// A whole number with thousands separators, as the docs write numbers.
fn thousands(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    if value < 0 {
        out.insert(0, '-');
    }
    out
}

#[cfg(test)]
mod tests {
    use schemars::schema_for;
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn numbers_get_separators() {
        assert_eq!(thousands(705_600), "705,600");
        assert_eq!(thousands(2_822_400), "2,822,400");
        assert_eq!(thousands(176_400), "176,400");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(-1_000), "-1,000");
    }

    /// The schemas name the same fields and tags the serde forms use.
    #[test]
    fn schemas_match_serde_forms() {
        let ramp = TempoMap::new([
            TempoSegment::Ramp {
                start: Ticks::ZERO,
                from: Bpm::new(90.0).unwrap(),
                to: Bpm::new(120.0).unwrap(),
            },
            TempoSegment::Constant {
                start: Ticks::WHOLE,
                bpm: Bpm::new(120.0).unwrap(),
            },
        ])
        .unwrap();
        let json = serde_json::to_value(&ramp).unwrap();
        let schema = serde_json::to_value(schema_for!(TempoMap)).unwrap();
        let variants = &schema["$defs"]["TempoSegment"]["oneOf"];
        // The map holds a ramp, then a constant; the schema lists constant first.
        for (segment, variant) in json.as_array().unwrap().iter().zip([1, 0]) {
            let variant = &variants[variant];
            assert_eq!(segment["kind"], variant["properties"]["kind"]["const"]);
            assert_eq!(keys(segment), keys(&variant["properties"]));
        }

        let signature = TimeSignature::new(7, 8).unwrap();
        let json = serde_json::to_value(TimeSignatureMap::constant(signature)).unwrap();
        assert_eq!(
            json,
            json!([{ "at": 0, "signature": { "numerator": 7, "denominator": 8 } }])
        );
        let schema = serde_json::to_value(schema_for!(TimeSignatureMap)).unwrap();
        assert_eq!(
            keys(&json[0]),
            keys(&schema["$defs"]["TimeSignatureChange"]["properties"])
        );
        assert_eq!(
            keys(&json[0]["signature"]),
            keys(&schema["$defs"]["TimeSignature"]["properties"])
        );
    }

    #[test]
    fn schemas_state_the_limits() {
        let bpm = serde_json::to_value(schema_for!(Bpm)).unwrap();
        assert_eq!(bpm["minimum"], json!(1.0));
        assert_eq!(bpm["maximum"], json!(1000.0));
        let ticks = serde_json::to_value(schema_for!(Ticks)).unwrap();
        assert!(
            ticks["description"]
                .as_str()
                .unwrap()
                .contains("705,600 to the quarter note")
        );
    }

    fn keys(object: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = object
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }
}
