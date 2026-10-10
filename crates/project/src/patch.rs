//! Differences between two JSON values, for the journal.
//!
//! A journal entry after the first in a run holds the difference between
//! the saved form of the song before an edit and after it, rather than the
//! whole song, so a long session journals a few hundred bytes per edit
//! instead of the whole set. [`diff`] finds the difference and [`apply`]
//! applies it; `apply(diff(a, b), a)` gives `b` exactly, numbers to the bit.
//!
//! The difference is structural: objects by key, arrays by the run they
//! keep at each end and what changed between, anything else replaced. It
//! knows nothing about the document model, so a patched value is checked
//! by the set loader like any other.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// How one JSON value becomes another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Patch {
    /// The value is replaced by this one.
    Replace(Value),
    /// An object keeps its other keys: `set` patches or adds these keys
    /// (an added key's patch is a `replace`), and `remove` deletes these.
    Object {
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        set: BTreeMap<String, Patch>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remove: Vec<String>,
    },
    /// An array keeps `keep_start` elements at its start and `keep_end` at
    /// its end, and the elements between change as `edit` says.
    Array {
        keep_start: usize,
        keep_end: usize,
        edit: ArrayEdit,
    },
}

/// What happens to the elements of an array between the runs it keeps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ArrayEdit {
    /// The same number of elements, each patched in place (`null` for one
    /// that doesn't change).
    Each(Vec<Option<Patch>>),
    /// The elements are replaced by these, however many there were.
    Splice(Vec<Value>),
}

/// The patch that turns `from` into `to`, or `None` if they're the same.
pub(crate) fn diff(from: &Value, to: &Value) -> Option<Patch> {
    match (from, to) {
        (Value::Object(a), Value::Object(b)) => diff_objects(a, b),
        (Value::Array(a), Value::Array(b)) => diff_arrays(a, b),
        _ if same(from, to) => None,
        _ => Some(Patch::Replace(to.clone())),
    }
}

fn diff_objects(a: &Map<String, Value>, b: &Map<String, Value>) -> Option<Patch> {
    let mut set = BTreeMap::new();
    for (key, new) in b {
        let patch = match a.get(key) {
            Some(old) => diff(old, new),
            None => Some(Patch::Replace(new.clone())),
        };
        if let Some(patch) = patch {
            set.insert(key.clone(), patch);
        }
    }
    let remove: Vec<String> = a
        .keys()
        .filter(|key| !b.contains_key(*key))
        .cloned()
        .collect();
    if set.is_empty() && remove.is_empty() {
        None
    } else {
        Some(Patch::Object { set, remove })
    }
}

fn diff_arrays(a: &[Value], b: &[Value]) -> Option<Patch> {
    let keep_start = a.iter().zip(b).take_while(|(x, y)| same(x, y)).count();
    let shorter = a.len().min(b.len()) - keep_start;
    let keep_end = a[keep_start..]
        .iter()
        .rev()
        .zip(b[keep_start..].iter().rev())
        .take(shorter)
        .take_while(|(x, y)| same(x, y))
        .count();
    let old = &a[keep_start..a.len() - keep_end];
    let new = &b[keep_start..b.len() - keep_end];
    if old.is_empty() && new.is_empty() {
        return None;
    }
    let edit = if old.len() == new.len() {
        ArrayEdit::Each(old.iter().zip(new).map(|(x, y)| diff(x, y)).collect())
    } else {
        ArrayEdit::Splice(new.to_vec())
    };
    Some(Patch::Array {
        keep_start,
        keep_end,
        edit,
    })
}

/// Whether two values are the same, comparing numbers written as floats by
/// their bits, so `0.0` and `-0.0` differ, as they do in the saved song.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if x.is_f64() || y.is_f64() {
                x.is_f64() == y.is_f64()
                    && x.as_f64().map(f64::to_bits) == y.as_f64().map(f64::to_bits)
            } else {
                x == y
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, x)| y.get(key).is_some_and(|y| same(x, y)))
        }
        _ => a == b,
    }
}

/// Applies a patch to a value in place.
///
/// # Errors
///
/// A message if the patch doesn't fit the value: a key it changes or
/// removes is missing, or an array is shorter than the runs it keeps. The
/// value may be partly patched; callers discard it.
pub(crate) fn apply(value: &mut Value, patch: &Patch) -> Result<(), String> {
    match patch {
        Patch::Replace(new) => {
            *value = new.clone();
            Ok(())
        }
        Patch::Object { set, remove } => {
            let Value::Object(map) = value else {
                return Err("an object patch for a value that isn't an object".to_owned());
            };
            for key in remove {
                if map.remove(key).is_none() {
                    return Err(format!("the patch removes `{key}`, which isn't there"));
                }
            }
            for (key, patch) in set {
                match (map.get_mut(key), patch) {
                    (Some(old), patch) => apply(old, patch)?,
                    (None, Patch::Replace(new)) => {
                        map.insert(key.clone(), new.clone());
                    }
                    (None, _) => {
                        return Err(format!("the patch changes `{key}`, which isn't there"));
                    }
                }
            }
            Ok(())
        }
        Patch::Array {
            keep_start,
            keep_end,
            edit,
        } => {
            let Value::Array(items) = value else {
                return Err("an array patch for a value that isn't an array".to_owned());
            };
            let kept = keep_start
                .checked_add(*keep_end)
                .filter(|kept| *kept <= items.len())
                .ok_or_else(|| {
                    format!(
                        "the patch keeps {keep_start} and {keep_end} elements of an array of {}",
                        items.len()
                    )
                })?;
            let middle = *keep_start..*keep_start + (items.len() - kept);
            match edit {
                ArrayEdit::Each(patches) => {
                    if patches.len() != middle.len() {
                        return Err(format!(
                            "the patch changes {} elements in place where there are {}",
                            patches.len(),
                            middle.len()
                        ));
                    }
                    for (item, patch) in items[middle].iter_mut().zip(patches) {
                        if let Some(patch) = patch {
                            apply(item, patch)?;
                        }
                    }
                }
                ArrayEdit::Splice(new) => {
                    items.splice(middle, new.iter().cloned());
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    fn round_trip(a: &Value, b: &Value) -> Option<Patch> {
        let patch = diff(a, b);
        let mut patched = a.clone();
        if let Some(patch) = &patch {
            // Through JSON, as the journal stores it.
            let stored: Patch =
                serde_json::from_str(&serde_json::to_string(patch).unwrap()).unwrap();
            assert_eq!(&stored, patch);
            apply(&mut patched, &stored).unwrap();
        }
        assert!(same(&patched, b), "{patched} != {b}");
        patch
    }

    #[test]
    fn equal_values_need_no_patch() {
        let value = json!({"a": [1, 2.5, {"b": null}], "c": "d"});
        assert_eq!(diff(&value, &value.clone()), None);
    }

    #[test]
    fn small_edits_make_small_patches() {
        let tracks: Vec<Value> = (0..100)
            .map(|n| json!({"id": n, "name": format!("Track {n}"), "volume": 1.0}))
            .collect();
        let before = json!({"tracks": tracks});

        // Renaming one track touches one element.
        let mut renamed = before.clone();
        renamed["tracks"][40]["name"] = json!("Bass");
        let patch = round_trip(&before, &renamed).unwrap();
        let text = serde_json::to_string(&patch).unwrap();
        assert!(text.len() < 160, "{text}");
        assert!(text.contains(r#""keep_start":40"#), "{text}");

        // Inserting a track splices one element in.
        let mut inserted = before.clone();
        inserted["tracks"]
            .as_array_mut()
            .unwrap()
            .insert(10, json!({"id": 1000, "name": "New", "volume": 1.0}));
        let text = serde_json::to_string(&round_trip(&before, &inserted).unwrap()).unwrap();
        assert!(text.len() < 150, "{text}");

        // Deleting one splices nothing in.
        let mut deleted = before.clone();
        deleted["tracks"].as_array_mut().unwrap().remove(99);
        let text = serde_json::to_string(&round_trip(&before, &deleted).unwrap()).unwrap();
        assert!(text.contains(r#""splice":[]"#), "{text}");
    }

    #[test]
    fn keys_are_added_and_removed() {
        let before = json!({"a": 1, "b": {"c": 2}});
        let after = json!({"b": {"c": 2, "d": [1]}, "e": true});
        round_trip(&before, &after).unwrap();
    }

    #[test]
    fn signed_zeros_and_number_kinds_differ() {
        round_trip(&json!({"pan": 0.0}), &json!({"pan": -0.0})).unwrap();
        round_trip(&json!([1]), &json!([1.0])).unwrap();
        assert!(diff(&json!(5e-324), &json!(5e-324)).is_none());
    }

    #[test]
    fn patches_that_dont_fit_are_refused() {
        let mut value = json!({"a": [1, 2]});
        let cases = [
            Patch::Object {
                set: BTreeMap::new(),
                remove: vec!["missing".to_owned()],
            },
            Patch::Object {
                set: BTreeMap::from([(
                    "missing".to_owned(),
                    Patch::Object {
                        set: BTreeMap::new(),
                        remove: Vec::new(),
                    },
                )]),
                remove: Vec::new(),
            },
            Patch::Array {
                keep_start: 0,
                keep_end: 0,
                edit: ArrayEdit::Splice(Vec::new()),
            },
        ];
        for patch in &cases {
            assert!(apply(&mut value, patch).is_err(), "{patch:?}");
        }
        let too_long = Patch::Object {
            set: BTreeMap::from([(
                "a".to_owned(),
                Patch::Array {
                    keep_start: 2,
                    keep_end: 1,
                    edit: ArrayEdit::Splice(Vec::new()),
                },
            )]),
            remove: Vec::new(),
        };
        assert!(apply(&mut value, &too_long).is_err());
        let wrong_count = Patch::Object {
            set: BTreeMap::from([(
                "a".to_owned(),
                Patch::Array {
                    keep_start: 0,
                    keep_end: 0,
                    edit: ArrayEdit::Each(vec![None]),
                },
            )]),
            remove: Vec::new(),
        };
        assert!(apply(&mut value, &wrong_count).is_err());
        let overflow = Patch::Array {
            keep_start: usize::MAX,
            keep_end: 1,
            edit: ArrayEdit::Splice(Vec::new()),
        };
        assert!(apply(&mut json!([]), &overflow).is_err());
    }

    fn json_value() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            (-5i64..5).prop_map(Value::from),
            prop::sample::select(vec![0.0, -0.0, 0.5, 1.0, -1.0, 5e-324]).prop_map(Value::from),
            "[ab]{0,2}".prop_map(Value::from),
        ];
        leaf.prop_recursive(4, 48, 6, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
                prop::collection::btree_map("[abc]", inner, 0..4)
                    .prop_map(|map| Value::Object(map.into_iter().collect())),
            ]
        })
    }

    /// A value and a few random edits of it, so the two share structure as
    /// a song before and after an edit does.
    fn related() -> impl Strategy<Value = (Value, Value)> {
        (
            json_value(),
            prop::collection::vec((any::<prop::sample::Index>(), json_value()), 0..4),
        )
            .prop_map(|(a, edits)| {
                let mut b = a.clone();
                for (at, new) in edits {
                    edit_somewhere(&mut b, at, new);
                }
                (a, b)
            })
    }

    fn edit_somewhere(value: &mut Value, at: prop::sample::Index, new: Value) {
        match value {
            Value::Array(items) if !items.is_empty() => {
                let i = at.index(items.len() + 1);
                if i == items.len() {
                    items.insert(at.index(items.len()), new);
                } else {
                    edit_somewhere(&mut items[i], at, new);
                }
            }
            Value::Object(map) if !map.is_empty() => {
                let keys: Vec<String> = map.keys().cloned().collect();
                let key = &keys[at.index(keys.len())];
                if at.index(3) == 0 {
                    map.remove(key);
                } else {
                    edit_somewhere(map.get_mut(key).unwrap(), at, new);
                }
            }
            _ => *value = new,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2_000))]

        #[test]
        fn any_patch_round_trips((a, b) in related()) {
            round_trip(&a, &b);
            round_trip(&b, &a);
        }

        #[test]
        fn unrelated_values_round_trip(a in json_value(), b in json_value()) {
            round_trip(&a, &b);
        }
    }
}
