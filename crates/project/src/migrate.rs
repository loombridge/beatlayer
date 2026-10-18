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

use bl_core::time::{TempoMap, Ticks};
use serde_json::{Map, Value};

use crate::error::LoadError;

/// The schema version this build writes, and the newest it can read.
pub const SCHEMA_VERSION: u32 = 6;

/// The oldest schema version this build can read, through [`MIGRATIONS`].
pub const OLDEST_SCHEMA_VERSION: u32 = SCHEMA_VERSION - MIGRATIONS.len() as u32;

// Schema versions count from 1, so there's at most one migration fewer
// than the current version.
const _: () = assert!(OLDEST_SCHEMA_VERSION >= 1);

/// Every migration, oldest first: the one at index `i` reads version
/// [`OLDEST_SCHEMA_VERSION`]` + i`.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        from: 1,
        summary: "an audio clip's offset into its file becomes part of its Start marker, as Live \
                  moves the marker when a clip is split or its left edge trimmed",
        apply: audio_offsets_to_start_markers,
    },
    Migration {
        from: 2,
        summary: "the document gains the Session View's scenes and the clips in their slots; a \
                  set saved without scenes opens with the default eight, whose IDs the \
                  document's loader hands out after the set's largest (the session view spec, \
                  R14), so the saved JSON itself needs nothing",
        apply: Ok,
    },
    Migration {
        from: 3,
        summary: "the set gains the global launch quantisation and its enabled switch, and each \
                  track's removed Clip Stop buttons; a set saved before them opens with a new \
                  set's quantisation and with a stop button in every slot (the launch \
                  quantisation spec, R1 and R8), so the saved JSON itself needs nothing",
        apply: Ok,
    },
    Migration {
        from: 4,
        summary: "a clip envelope's values are modulation (a share of the parameter's travel, or \
                  of a level's position) rather than parameter units added to the value; a set \
                  saved before it keeps the old values in the JSON, and the loader's caller \
                  turns them with the controls' values and the devices' descriptions \
                  (`SetInfo::before_relative_modulation`; the modulation spec, R5)",
        apply: Ok,
    },
    Migration {
        from: 5,
        summary: "a device may hold a multisampler's layers, and the set may save what each \
                  multisampler's views show; a set saved before them opens with no layers and \
                  the views a new device has (the multisampler spec, R1 and R2), so the saved \
                  JSON itself needs nothing",
        apply: Ok,
    },
];

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

/// Version 1 to 2 (WP-1.4b): an audio clip trimmed or split in version 1
/// kept an `offset` in ticks, how far into its contents it started, which
/// played its file that much later than its Start marker. Version 2 moves
/// the Start marker instead, as Live does (the audio clips spec, R16): by
/// the time from where the contents began to the clip's start under the
/// set's tempo map, which plays the same audio at the same moments. A MIDI
/// clip keeps its offset.
fn audio_offsets_to_start_markers(mut value: Value) -> Result<Value, String> {
    let document = value
        .get_mut("document")
        .and_then(Value::as_object_mut)
        .ok_or("the set has no document")?;
    let tempo = document
        .get("settings")
        .and_then(|settings| settings.get("tempo"))
        .cloned()
        .ok_or("the document has no tempo map")?;
    let tempo: TempoMap =
        serde_json::from_value(tempo).map_err(|e| format!("the tempo map doesn't read: {e}"))?;
    for list in ["tracks", "returns"] {
        if let Some(tracks) = document.get_mut(list).and_then(Value::as_array_mut) {
            for track in tracks {
                fold_track(track, &tempo)?;
            }
        }
    }
    if let Some(main) = document.get_mut("main") {
        fold_track(main, &tempo)?;
    }
    Ok(value)
}

/// Folds the offsets of `track`'s audio clips, and its children's, into
/// their Start markers.
fn fold_track(track: &mut Value, tempo: &TempoMap) -> Result<(), String> {
    if let Some(clips) = track.get_mut("clips").and_then(Value::as_array_mut) {
        for clip in clips.iter_mut().filter_map(Value::as_object_mut) {
            fold_clip(clip, tempo)?;
        }
    }
    if let Some(children) = track.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            fold_track(child, tempo)?;
        }
    }
    Ok(())
}

fn fold_clip(clip: &mut Map<String, Value>, tempo: &TempoMap) -> Result<(), String> {
    let Some(offset) = clip.get("offset") else {
        return Ok(());
    };
    let offset = offset
        .as_i64()
        .ok_or("a clip's offset isn't a whole number")?;
    let start = clip
        .get("start")
        .and_then(Value::as_i64)
        .ok_or("a clip's start isn't a whole number")?;
    let Some(audio) = clip
        .get_mut("data")
        .and_then(|data| data.get_mut("audio"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    let marker = match audio.get("start") {
        None => 0.0,
        Some(marker) => marker
            .as_f64()
            .ok_or("an audio clip's Start marker isn't a number")?,
    };
    let contents = Ticks(start.saturating_sub(offset));
    let mut marker =
        marker + (tempo.ticks_to_seconds(Ticks(start)) - tempo.ticks_to_seconds(contents));
    // A looped clip's marker wraps back inside its loop brace, as a trim
    // does in version 2; a brace to the file's end has no known length.
    let looping = audio.get("looping").and_then(Value::as_bool) == Some(true);
    let loop_start = audio
        .get("loop_start")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if let (true, Some(loop_end)) = (looping, audio.get("loop_end").and_then(Value::as_f64)) {
        let length = loop_end - loop_start;
        if length > 0.0 && marker >= loop_end {
            let into = (marker - loop_end).rem_euclid(length);
            marker = loop_start + if into < length { into } else { 0.0 };
        }
    }
    if marker == 0.0 {
        audio.remove("start");
    } else {
        audio.insert(
            "start".to_owned(),
            serde_json::Number::from_f64(marker)
                .map(Value::Number)
                .ok_or("an audio clip's Start marker isn't finite")?,
        );
    }
    clip.remove("offset");
    Ok(())
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

    // Spec: audio-clips R16
    #[test]
    fn audio_offsets_become_start_markers_and_midi_offsets_stay() {
        let tick = |quarters: i64| Ticks::QUARTER.get() * quarters;
        let audio = |offset: Option<i64>, marker: Option<f64>| {
            let mut data = json!({ "media": "07".repeat(32) });
            if let Some(marker) = marker {
                data["start"] = json!(marker);
            }
            let mut clip = json!({ "id": 9, "name": "", "start": tick(6), "length": tick(4),
                                   "mute": false, "data": { "audio": data } });
            if let Some(offset) = offset {
                clip["offset"] = json!(offset);
            }
            clip
        };
        let set = |clips: Vec<Value>| {
            json!({ "schema": 1, "document": {
                "settings": { "tempo": [{ "kind": "constant", "start": 0, "bpm": 120.0 }] },
                "tracks": [{ "id": 2, "children": [{ "id": 3, "clips": clips }] }],
                "returns": [],
                "main": { "id": 1 },
            } })
        };
        let midi = json!({ "id": 10, "name": "", "start": tick(2), "length": tick(4),
                           "offset": tick(1), "mute": false, "data": { "midi": { "notes": [] } } });
        let before = set(vec![
            audio(Some(tick(2)), None),
            audio(Some(tick(1)), Some(0.25)),
            audio(None, Some(0.75)),
            midi.clone(),
        ]);
        let after = audio_offsets_to_start_markers(before).unwrap();
        let clips = &after["document"]["tracks"][0]["children"][0]["clips"];
        // Two quarters at 120 BPM is a second, one is half a second.
        assert_eq!(clips[0], audio(None, Some(1.0)));
        assert_eq!(clips[1], audio(None, Some(0.75)));
        assert_eq!(clips[2], audio(None, Some(0.75)));
        assert_eq!(clips[3], midi);
        // A looped clip's marker wraps inside its brace: three quarters (1.5
        // seconds) in, past a brace from 0.25 to 1.25, is 0.5.
        let mut looped = audio(Some(tick(3)), None);
        looped["data"]["audio"]["looping"] = json!(true);
        looped["data"]["audio"]["loop_start"] = json!(0.25);
        looped["data"]["audio"]["loop_end"] = json!(1.25);
        let after = audio_offsets_to_start_markers(set(vec![looped.clone()])).unwrap();
        let mut expected = looped;
        expected.as_object_mut().unwrap().remove("offset");
        expected["data"]["audio"]["start"] = json!(0.5);
        assert_eq!(
            after["document"]["tracks"][0]["children"][0]["clips"][0],
            expected
        );
        // A file the migration can't read says why.
        assert!(audio_offsets_to_start_markers(json!({})).is_err());
        let bad = json!({ "document": { "settings": { "tempo": "fast" } } });
        assert!(
            audio_offsets_to_start_markers(bad)
                .unwrap_err()
                .contains("tempo")
        );
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
        // An invented step from version 0, then steps that change nothing
        // up to the current version.
        let mut steps = vec![Migration {
            from: 0,
            summary: "adds a field",
            apply: |mut value| {
                value["added"] = json!(true);
                Ok(value)
            },
        }];
        steps.extend((1..SCHEMA_VERSION).map(|from| Migration {
            from,
            summary: "changes nothing",
            apply: Ok,
        }));
        let migrated = run(json!({"schema": 0}), 0, &steps).unwrap();
        assert_eq!(migrated, json!({"schema": SCHEMA_VERSION, "added": true}));

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
