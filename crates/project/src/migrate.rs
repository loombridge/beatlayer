//! Schema versions and the migrations between them.
//!
//! Every set file carries the schema version it was written in. When a
//! change to the saved form would stop an older file from loading as it is,
//! the version goes up by one and a [`Migration`] from the old version is
//! added to [`MIGRATIONS`]. A migration is a pure function from the whole
//! saved JSON of one version to the JSON of the next; loading an old file runs
//! every migration from its version up to [`SCHEMA_VERSION`] in turn, then
//! reads the result as the current version.
//!
//! Each past version keeps a fixture file in `tests/fixtures/` that must load
//! for ever, and a merged migration is never edited without the reason in the
//! pull request.

use serde_json::Value;

use crate::error::LoadError;

/// The schema version this build writes, and the newest it can read.
pub const SCHEMA_VERSION: u32 = 1;

/// The oldest schema version this build can read, through [`MIGRATIONS`].
pub const OLDEST_SCHEMA_VERSION: u32 = SCHEMA_VERSION - MIGRATIONS.len() as u32;

// Schema versions count from 1, so there's at most one migration fewer
// than the current version.
const _: () = assert!(OLDEST_SCHEMA_VERSION >= 1);

/// Every migration, oldest first: the one at index `i` reads version
/// [`OLDEST_SCHEMA_VERSION`]` + i`.
///
/// Version 1 is the first, so there are none yet.
pub const MIGRATIONS: &[Migration] = &[];

/// One step from a schema version to the next.
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    /// The version it reads. It writes `from + 1`.
    pub from: u32,
    /// What changed between the two versions, in a sentence.
    pub summary: &'static str,
    /// The migration itself: the whole saved JSON of version `from` in, the
    /// JSON of version `from + 1` out, or a message saying why it can't be
    /// migrated.
    ///
    /// It must be a pure function of its input. It needn't touch the
    /// `schema` field, which the loader sets after each step.
    pub apply: fn(Value) -> Result<Value, String>,
}

/// Runs the migrations in `chain` from version `from` up to
/// [`SCHEMA_VERSION`].
pub(crate) fn run(mut value: Value, from: u32, chain: &[Migration]) -> Result<Value, LoadError> {
    let mut version = from;
    while version < SCHEMA_VERSION {
        let step = chain
            .iter()
            .find(|step| step.from == version)
            .ok_or(LoadError::TooOld {
                found: from,
                oldest: OLDEST_SCHEMA_VERSION,
            })?;
        value = (step.apply)(value).map_err(|message| LoadError::Migration {
            from: version,
            message,
        })?;
        version += 1;
        match &mut value {
            Value::Object(fields) => {
                fields.insert("schema".to_owned(), Value::from(version));
            }
            _ => {
                return Err(LoadError::Migration {
                    from: version - 1,
                    message: "the migration didn't return a JSON object".to_owned(),
                });
            }
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn migrations_form_one_chain_up_to_the_current_version() {
        for (offset, step) in (0..).zip(MIGRATIONS) {
            assert_eq!(
                step.from,
                OLDEST_SCHEMA_VERSION + offset,
                "migration {offset} ({}) is out of order",
                step.summary
            );
            assert!(!step.summary.is_empty());
        }
    }

    #[test]
    fn current_files_need_no_migration() {
        let value = json!({"schema": SCHEMA_VERSION, "x": 1});
        assert_eq!(run(value.clone(), SCHEMA_VERSION, &[]).unwrap(), value);
    }

    #[test]
    fn a_gap_in_the_chain_is_reported() {
        let error = run(json!({}), 0, &[]).unwrap_err();
        assert!(
            matches!(error, LoadError::TooOld { found: 0, oldest } if oldest == OLDEST_SCHEMA_VERSION),
            "{error:?}"
        );
    }

    #[test]
    fn each_step_sets_the_schema_and_failures_name_their_version() {
        const STEPS: &[Migration] = &[Migration {
            from: 0,
            summary: "adds a field",
            apply: |mut value| {
                value["added"] = json!(true);
                Ok(value)
            },
        }];
        let migrated = run(json!({"schema": 0}), 0, STEPS).unwrap();
        assert_eq!(migrated, json!({"schema": 1, "added": true}));

        const FAILS: &[Migration] = &[Migration {
            from: 0,
            summary: "fails",
            apply: |_| Err("no".to_owned()),
        }];
        assert!(matches!(
            run(json!({}), 0, FAILS).unwrap_err(),
            LoadError::Migration { from: 0, message } if message == "no"
        ));

        const NOT_AN_OBJECT: &[Migration] = &[Migration {
            from: 0,
            summary: "returns a number",
            apply: |_| Ok(json!(3)),
        }];
        assert!(matches!(
            run(json!({}), 0, NOT_AN_OBJECT).unwrap_err(),
            LoadError::Migration { from: 0, .. }
        ));
    }
}
