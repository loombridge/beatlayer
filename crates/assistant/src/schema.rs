//! The registry's command schemas for the providers' tool formats.
//!
//! A command's argument schema is self-contained: the definitions it refers
//! to sit under `$defs` in the same schema. OpenAI and Anthropic take it as
//! it is. Gemini's API takes the OpenAPI Schema subset of JSON Schema, which
//! has no references, so [`for_gemini`] resolves them and drops the keywords
//! the subset doesn't take.

use serde_json::{Map, Value};

/// The schema for Gemini: every `$ref` replaced by the definition it names,
/// `$defs` and `$schema` dropped, and `additionalProperties` removed
/// wherever it appears.
pub(crate) fn for_gemini(schema: &Value) -> Value {
    let mut schema = schema.clone();
    let defs = schema.as_object_mut().and_then(|object| {
        object.remove("$schema");
        object.remove("$defs")
    });
    resolve(&mut schema, defs.as_ref());
    strip_additional_properties(&mut schema);
    schema
}

/// Replaces every `$ref` in `value` with the `$defs` entry it names, and any
/// keys beside the reference after it. A reference to a type already being
/// expanded (a recursive type) leaves anything allowed, so the expansion
/// ends.
fn resolve(value: &mut Value, defs: Option<&Value>) {
    fn expand(name: &str, defs: &Value, open: &mut Vec<String>) -> Value {
        if open.iter().any(|seen| seen == name) {
            return Value::Object(Map::new());
        }
        // The registry's schemas are self-contained, so a reference always
        // finds its definition; one that does not leaves anything allowed.
        let Some(definition) = defs.get(name) else {
            return Value::Object(Map::new());
        };
        open.push(name.to_owned());
        let mut expanded = definition.clone();
        resolve_with(&mut expanded, defs, open);
        open.pop();
        expanded
    }

    fn resolve_with(value: &mut Value, defs: &Value, open: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                if let Some(name) = object
                    .get("$ref")
                    .and_then(Value::as_str)
                    .map(|reference| reference.rsplit('/').next().unwrap_or(reference).to_owned())
                {
                    let mut expanded = expand(&name, defs, open);
                    if let Value::Object(target) = &mut expanded {
                        for (key, sibling) in object.iter().filter(|(key, _)| *key != "$ref") {
                            target.insert(key.clone(), sibling.clone());
                        }
                    }
                    *value = expanded;
                } else {
                    for child in object.values_mut() {
                        resolve_with(child, defs, open);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    resolve_with(item, defs, open);
                }
            }
            _ => {}
        }
    }

    if let Some(defs) = defs {
        resolve_with(value, defs, &mut Vec::new());
    }
}

/// Removes every `additionalProperties`: Gemini's subset names a map's values
/// with `additionalProperties`, which its API refuses.
fn strip_additional_properties(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("additionalProperties");
            for child in object.values_mut() {
                strip_additional_properties(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_additional_properties(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_reference_becomes_the_definition_it_names() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$ref": "#/$defs/Track",
            "$defs": {
                "Track": {
                    "type": "object",
                    "properties": {
                        "id": { "$ref": "#/$defs/TrackId" },
                        "name": { "type": "string" }
                    }
                },
                "TrackId": { "type": "integer" }
            }
        });
        assert_eq!(
            for_gemini(&schema),
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "name": { "type": "string" }
                }
            })
        );
    }

    #[test]
    fn keys_beside_a_reference_come_after_the_definition() {
        let schema = json!({
            "$defs": { "Count": { "type": "integer", "description": "A count." } },
            "properties": { "count": { "$ref": "#/$defs/Count", "description": "How many." } }
        });
        assert_eq!(
            for_gemini(&schema)["properties"]["count"],
            json!({ "type": "integer", "description": "How many." })
        );
    }

    #[test]
    fn a_recursive_type_ends_as_anything_allowed() {
        let schema = json!({
            "$ref": "#/$defs/Node",
            "$defs": {
                "Node": {
                    "type": "object",
                    "properties": { "child": { "$ref": "#/$defs/Node" } }
                }
            }
        });
        assert_eq!(for_gemini(&schema)["properties"]["child"], json!({}));
    }

    #[test]
    fn additional_properties_are_dropped_wherever_they_are() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "values": { "type": "object", "additionalProperties": { "type": "number" } }
            }
        });
        assert_eq!(
            for_gemini(&schema),
            json!({
                "type": "object",
                "properties": { "values": { "type": "object" } }
            })
        );
    }
}
