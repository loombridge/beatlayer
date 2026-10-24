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
pub const SCHEMA_VERSION: u32 = 11;

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
    Migration {
        from: 6,
        summary: "a native device saved under the name Beatlayer gave it before it took the name \
                  of the Live device it matches opens under the new name; a device the user \
                  renamed keeps its name",
        apply: rename_devices,
    },
    Migration {
        from: 7,
        summary: "a take lane has an ID and holds clips, with clip IDs, in place of the recorded \
                  passes it held, and a track may name the take lane it auditions; each saved \
                  pass becomes a clip under a new ID, and a lane gets one (the comping spec, \
                  R3)",
        apply: passes_become_clips,
    },
    Migration {
        from: 8,
        summary: "a note's pressure is an envelope of breakpoints, beside its new Pitch and Slide \
                  envelopes (the MPE spec, R1 and R2); each pressure point a set saved before \
                  it holds becomes a breakpoint, with one a tick before each later point \
                  holding the value before it, so the pressure plays the steps it played",
        apply: |mut value| {
            pressure_to_breakpoints(&mut value);
            Ok(value)
        },
    },
    Migration {
        from: 9,
        summary: "Beats' Transient Loop Mode defaults to Loop Back-and-Forth, as Live's does, \
                  where it defaulted to Loop Forward and left Loop Forward out of the file; each \
                  audio clip, and each device that warps, that doesn't name its mode now names \
                  Loop Forward, so it plays as it did (the warp modes spec, R2)",
        apply: |mut value| {
            pin_loop_forward(&mut value);
            Ok(value)
        },
    },
    Migration {
        from: 10,
        summary: "tracks, clips and scenes gain colours; a set saved before them opens with each \
                  track at swatch `id mod 14` of the palette's top row, each clip in its \
                  track's colour and every scene with none, as such sets were drawn (the themes \
                  spec, R21), so the saved JSON itself needs nothing",
        apply: Ok,
    },
];

/// Gives every audio clip's and device's warp settings in the saved JSON
/// `value` that name no Transient Loop Mode the one they had before it
/// defaulted to Loop Back-and-Forth (the warp modes spec, R2). A set file and
/// a clip file of the schemas before it hold the old form.
pub(crate) fn pin_loop_forward(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields.iter_mut() {
                match (key.as_str(), child) {
                    // A clip's `warp` is a switch; a device's holds its settings.
                    ("audio", Value::Object(audio)) if audio.contains_key("media") => {
                        pin_settings(audio, "warp_settings");
                    }
                    ("warp", Value::Object(warp)) => pin_settings(warp, "settings"),
                    (_, child) => pin_loop_forward(child),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(pin_loop_forward),
        _ => {}
    }
}

fn pin_settings(owner: &mut Map<String, Value>, key: &str) {
    let mode = owner
        .entry(key)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .and_then(|settings| {
            settings
                .entry("beats")
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
        });
    if let Some(beats) = mode {
        beats
            .entry("transient_loop_mode")
            .or_insert_with(|| Value::from("loop_forward"));
    }
}

/// Turns every note's pressure points in the saved JSON `value` into
/// breakpoints (the MPE spec, R2): each point stays, and a point after another
/// is preceded one tick before it by a breakpoint at the value before it, so
/// the envelope plays the steps the points were. A set file and a clip file
/// of the schemas before it hold the old form.
pub(crate) fn pressure_to_breakpoints(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            if let Some(notes) = fields
                .get_mut("midi")
                .and_then(|midi| midi.get_mut("notes"))
                .and_then(Value::as_array_mut)
            {
                notes.iter_mut().for_each(steps_of_pressure);
            }
            fields.values_mut().for_each(pressure_to_breakpoints);
        }
        Value::Array(items) => items.iter_mut().for_each(pressure_to_breakpoints),
        _ => {}
    }
}

fn steps_of_pressure(note: &mut Value) {
    let Some(points) = note.get_mut("pressure").and_then(Value::as_array_mut) else {
        return;
    };
    let breakpoint = |at: i64, value: f64| {
        let mut point = Map::new();
        point.insert("at".to_owned(), Value::from(at));
        point.insert("value".to_owned(), Value::from(value));
        Value::Object(point)
    };
    let mut steps: Vec<Value> = Vec::with_capacity(points.len() * 2 + 1);
    let mut before: Option<(i64, f64)> = None;
    // A note played no pressure before its first point; an envelope holds
    // its first value back to the note's start, so it starts from none.
    if points
        .first()
        .and_then(|point| point.get("at"))
        .and_then(Value::as_i64)
        .is_some_and(|at| at > 0)
    {
        steps.push(breakpoint(0, 0.0));
        before = Some((0, 0.0));
    }
    for point in points.iter() {
        let (Some(at), Some(value)) = (
            point.get("at").and_then(Value::as_i64),
            point.get("value").and_then(Value::as_f64),
        ) else {
            // Not a pressure point as the old form had it: leave it for the
            // loader to refuse.
            return;
        };
        if let Some((was, held)) = before
            && at - 1 > was
        {
            steps.push(breakpoint(at - 1, held));
        }
        steps.push(breakpoint(at, value));
        before = Some((at, value));
    }
    *points = steps;
}

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

/// The native devices that took Live's names (T197), as (device type, name
/// before, name now). The device types are the stable IDs and did not change.
const RENAMED_DEVICES: &[(&str, &str, &str)] = &[
    ("beatlayer.contour", "Contour", "EQ Eight"),
    ("beatlayer.squeeze", "Squeeze", "Compressor"),
    ("bl.device.reprise", "Reprise", "Delay"),
    ("beatlayer.cascade", "Cascade", "Drift"),
    ("beatlayer.snapshot", "Snapshot", "Simpler"),
    ("beatlayer.strike", "Strike", "Drum Sampler"),
    ("beatlayer.mosaic", "Mosaic", "Sampler"),
    ("beatlayer.sweep", "Sweep", "Auto Filter"),
    ("beatlayer.triband", "Triband", "Multiband Dynamics"),
    ("beatlayer.degrade", "Degrade", "Erosion"),
    ("beatlayer.crush", "Crush", "Redux"),
    ("beatlayer.wax", "Wax", "Vinyl Distortion"),
    ("beatlayer.singe", "Singe", "Saturator"),
    ("beatlayer.grit", "Grit", "Overdrive"),
    ("beatlayer.valve", "Valve", "Dynamic Tube"),
    ("bl.device.trident", "Trident", "Filter Delay"),
    ("bl.device.scatter", "Scatter", "Grain Delay"),
    ("bl.device.plumb", "Plumb", "Align Delay"),
    ("bl.device.stutter", "Stutter", "Beat Repeat"),
    ("beatlayer.strip", "Strip", "Channel EQ"),
    ("beatlayer.triad", "Triad", "EQ Three"),
    ("beatlayer.shutter", "Shutter", "Gate"),
    ("beatlayer.bond", "Bond", "Glue Compressor"),
    ("beatlayer.wave_mod", "Wave Mod", "LFO"),
    ("beatlayer.curve_mod", "Curve Mod", "Shaper"),
    ("beatlayer.brickwall", "Brickwall", "Limiter"),
    ("beatlayer.rotunda", "Rotunda", "Reverb"),
    ("beatlayer.kitbag", "Kitbag", "Impulse"),
    ("beatlayer.dice", "Dice", "Random"),
    ("beatlayer.hold", "Hold", "Note Length"),
    ("beatlayer.lift", "Lift", "Pitch"),
    ("beatlayer.remap", "Remap", "Scale"),
    ("beatlayer.stack", "Stack", "Chord"),
    ("beatlayer.touch", "Touch", "Velocity"),
    ("beatlayer.veer", "Veer", "Shifter"),
    ("beatlayer.level_mod", "Level Mod", "Envelope Follower"),
    ("beatlayer.gate_mod", "Gate Mod", "Envelope MIDI"),
    ("beatlayer.strike_mod", "Strike Mod", "Shaper MIDI"),
    ("beatlayer.play_mod", "Play Mod", "Expression Control"),
];

/// The name now for a device of `device_type` called `name` before T197, or
/// `None` if `name` isn't the old name of that type.
pub(crate) fn renamed_device(device_type: &str, name: &str) -> Option<&'static str> {
    RENAMED_DEVICES
        .iter()
        .find(|(ty, old, _)| *ty == device_type && *old == name)
        .map(|(_, _, new)| *new)
}

/// Version 6 to 7 (T197): the devices took the names of the Live devices
/// they match. A device saved with its type's old name, as its title or its
/// own name, gets the new one wherever it sits in the set (a track's chain, a
/// rack's chain); one the user renamed keeps its name. The IDs are unchanged.
fn rename_devices(mut value: Value) -> Result<Value, String> {
    fn walk(value: &mut Value) {
        match value {
            Value::Object(map) => {
                let device_type = map
                    .get("kind")
                    .and_then(|kind| kind.get("native"))
                    .and_then(|native| native.get("device_type"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(device_type) = device_type {
                    for key in ["name", "own_name"] {
                        let new = map
                            .get(key)
                            .and_then(Value::as_str)
                            .and_then(|name| renamed_device(&device_type, name));
                        if let Some(new) = new {
                            map.insert(key.to_owned(), Value::from(new));
                        }
                    }
                }
                map.values_mut().for_each(walk);
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut value);
    Ok(value)
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

/// Version 7 to 8 (WP-6.21): a take lane held recorded passes, which had no
/// ID, and had no ID itself. Version 8's lane has an ID and holds full
/// clips. Each lane and each pass is given the next ID after both the set's
/// allocator and its largest ID (the loader allows an allocator behind the
/// document), in saved order, and the allocator moves past them, so older
/// sets play as they did.
fn passes_become_clips(mut value: Value) -> Result<Value, String> {
    fn largest_id(value: &Value) -> u64 {
        match value {
            Value::Object(map) => map
                .iter()
                .map(|(key, v)| match (key.as_str(), v.as_u64()) {
                    ("id", Some(id)) => id,
                    _ => largest_id(v),
                })
                .max()
                .unwrap_or(0),
            Value::Array(items) => items.iter().map(largest_id).max().unwrap_or(0),
            _ => 0,
        }
    }
    let mut next = value
        .get("ids")
        .and_then(Value::as_u64)
        .ok_or("the set has no ID allocator")?;
    if let Some(document) = value.get("document") {
        next = next.max(largest_id(document).saturating_add(1));
    }
    let document = value
        .get_mut("document")
        .and_then(Value::as_object_mut)
        .ok_or("the set has no document")?;
    for list in ["tracks", "returns"] {
        if let Some(tracks) = document.get_mut(list).and_then(Value::as_array_mut) {
            for track in tracks {
                give_ids(track, &mut next)?;
            }
        }
    }
    if let Some(main) = document.get_mut("main") {
        give_ids(main, &mut next)?;
    }
    value["ids"] = Value::from(next);
    Ok(value)
}

/// Gives the take lanes of `track`, and of its children, and their passes
/// new IDs from `next`.
fn give_ids(track: &mut Value, next: &mut u64) -> Result<(), String> {
    let mut issue = || {
        let id = *next;
        *next += 1;
        Value::from(id)
    };
    if let Some(lanes) = track.get_mut("take_lanes").and_then(Value::as_array_mut) {
        for lane in lanes.iter_mut().filter_map(Value::as_object_mut) {
            lane.insert("id".to_owned(), issue());
            let passes = lane
                .get_mut("clips")
                .and_then(Value::as_array_mut)
                .ok_or("a take lane's passes aren't a list")?;
            for pass in passes.iter_mut().filter_map(Value::as_object_mut) {
                pass.insert("id".to_owned(), issue());
                pass.insert("mute".to_owned(), Value::Bool(false));
                // A pass's data was the audio's fields; a clip's is tagged.
                let data = pass.remove("data").ok_or("a pass has no data")?;
                pass.insert("data".to_owned(), serde_json::json!({ "audio": data }));
            }
        }
    }
    if let Some(children) = track.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            give_ids(child, next)?;
        }
    }
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

    // Spec: mpe R2
    #[test]
    fn pressure_points_become_steps() {
        let mut value = json!({
            "tracks": [{ "clips": [{ "data": { "midi": { "notes": [
                { "key": 60, "pressure": [
                    { "at": 0, "value": 90 },
                    { "at": 500, "value": 20 },
                    { "at": 501, "value": 30 },
                ] },
                { "key": 62 },
                { "key": 64, "pressure": [{ "at": 100, "value": 64 }] },
            ] } } }] }]
        });
        pressure_to_breakpoints(&mut value);
        let notes = &value["tracks"][0]["clips"][0]["data"]["midi"]["notes"];
        assert_eq!(
            notes[0]["pressure"],
            json!([
                { "at": 0, "value": 90.0 },
                { "at": 499, "value": 90.0 },
                { "at": 500, "value": 20.0 },
                { "at": 501, "value": 30.0 },
            ]),
            "a point a tick after another needs no step before it"
        );
        assert!(notes[1].get("pressure").is_none());
        assert_eq!(
            notes[2]["pressure"],
            json!([
                { "at": 0, "value": 0.0 },
                { "at": 99, "value": 0.0 },
                { "at": 100, "value": 64.0 },
            ]),
            "no pressure before the first point, as it played"
        );
    }
}
