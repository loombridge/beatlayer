//! The IDs a command that creates an object needs, which only the project
//! can issue. Rather than ask for one first, a caller (an agent, or the
//! built-in assistant) writes `{"$new": "name"}` where the ID goes, and
//! `{"$ref": "name"}` wherever a later command refers to it.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// Replaces `{"$new": name}` and `{"$ref": name}` in `value`, issuing new
/// IDs with `issue`. Every `$new` is replaced first, so a `$ref` may name
/// an ID made anywhere in the same arguments.
///
/// # Errors
///
/// Why a placeholder can't be resolved: a name given twice, a `$ref` to a
/// name no `$new` made, or a placeholder whose name isn't a string.
pub fn resolve_placeholders(
    value: &mut Value,
    names: &mut BTreeMap<String, u64>,
    issue: &mut dyn FnMut() -> u64,
) -> Result<(), String> {
    replace(value, names, issue, "$new")?;
    replace(value, names, issue, "$ref")
}

/// Replaces the placeholders with the key `kind` (`$new` or `$ref`).
fn replace(
    value: &mut Value,
    names: &mut BTreeMap<String, u64>,
    issue: &mut dyn FnMut() -> u64,
    kind: &str,
) -> Result<(), String> {
    match value {
        Value::Object(object) if object.len() == 1 && object.contains_key(kind) => {
            let Some(Value::String(name)) = object.get(kind) else {
                return Err(format!("`{kind}` takes a name, as a string"));
            };
            let id = if kind == "$new" {
                if names.contains_key(name) {
                    return Err(format!(
                        "`{name}` already names an ID; give each new object its own name"
                    ));
                }
                let id = issue();
                names.insert(name.clone(), id);
                id
            } else {
                *names.get(name).ok_or_else(|| {
                    format!("`{name}` names no ID; make one with {{\"$new\": \"{name}\"}} first")
                })?
            };
            *value = json!(id);
            Ok(())
        }
        Value::Object(object) => object
            .values_mut()
            .try_for_each(|inner| replace(inner, names, issue, kind)),
        Value::Array(items) => items
            .iter_mut()
            .try_for_each(|inner| replace(inner, names, issue, kind)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_issue_ids_and_refer_back() {
        let mut next = 100;
        let mut issue = || {
            next += 1;
            next
        };
        let mut names = BTreeMap::new();
        // A reference may come before the ID it names in the arguments.
        let mut args = json!({
            "after": {"$ref": "bass"},
            "id": {"$new": "bass"},
            "list": [{"$new": "eq"}],
            "name": "$new",
        });
        resolve_placeholders(&mut args, &mut names, &mut issue).unwrap();
        assert_eq!(
            args,
            json!({
                "after": 101,
                "id": 101,
                "list": [102],
                "name": "$new",
            })
        );
        let mut again = json!({"id": {"$new": "bass"}});
        assert!(resolve_placeholders(&mut again, &mut names, &mut issue).is_err());
        let mut unknown = json!([{"$ref": "drums"}]);
        assert!(resolve_placeholders(&mut unknown, &mut names, &mut issue).is_err());
        let mut wrong = json!({"$new": 3});
        assert!(resolve_placeholders(&mut wrong, &mut names, &mut issue).is_err());
    }
}
