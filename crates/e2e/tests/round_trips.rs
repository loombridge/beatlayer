//! Round trips: what goes out of Beatlayer and comes back in is what went
//! out. The rich set after many kinds of edits saved and reopened; a
//! DAWproject export imported again; a MIDI file and a clip file written
//! and read back; tracks, clips and devices copied, pasted and duplicated,
//! with new IDs and the references between the copies remapped; crash
//! recovery's journal replayed; and every export format decoded again.
//!
//! Each case is checked through the known-failures ratchet: the key is the
//! case, the property `round trip keeps it`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use bl_e2e::fixture::{self, POLYSYNTH, UTILITY};
use bl_e2e::{
    Audio, BAR, BEAT, Driver, ExportSpec, Host, Ids, Known, TempDir, document_diff, full_depth,
};
use serde_json::{Value, json};

/// The ratchet's suite.
const SUITE: &str = "background";

/// The property every case checks.
const KEEPS_IT: &str = "round trip keeps it";

/// Checks each `(case, outcome)` against the known failures.
fn ratchet(outcomes: Vec<(&str, Result<(), String>)>) {
    let mut ratchet = Known::load().ratchet(SUITE);
    for (key, outcome) in outcomes {
        if let Err(error) = &outcome {
            eprintln!("{key}: {error}");
        }
        ratchet.check(key, KEEPS_IT, outcome);
    }
    ratchet.finish();
}

/// The open set as JSON, as its saved form serialises it.
fn document(host: &mut Host) -> Value {
    serde_json::to_value(host.document()).expect("the document serialises")
}

/// The first object in `value` whose `id` is `id`: IDs are unique across
/// every kind of object in a set.
fn object(value: &Value, id: u64) -> Option<&Value> {
    match value {
        Value::Object(fields) => {
            if fields.get("id").and_then(Value::as_u64) == Some(id) {
                return Some(value);
            }
            fields.values().find_map(|v| object(v, id))
        }
        Value::Array(items) => items.iter().find_map(|v| object(v, id)),
        _ => None,
    }
}

/// Every `id` in `value`, in the order a walk meets them.
fn ids_in(value: &Value, out: &mut Vec<u64>) {
    match value {
        Value::Object(fields) => {
            if let Some(id) = fields.get("id").and_then(Value::as_u64) {
                out.push(id);
            }
            for v in fields.values() {
                ids_in(v, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| ids_in(v, out)),
        _ => {}
    }
}

/// The keys whose number names another object: a device or a track.
const REFERENCES: [&str; 2] = ["device", "track"];

/// Every reference in `value`: the numbers under [`REFERENCES`].
fn references_in(value: &Value, out: &mut Vec<u64>) {
    match value {
        Value::Object(fields) => {
            for (key, v) in fields {
                if REFERENCES.contains(&key.as_str())
                    && let Some(id) = v.as_u64()
                {
                    out.push(id);
                }
                references_in(v, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| references_in(v, out)),
        _ => {}
    }
}

/// `value` with each ID replaced by the order the walk met it in, and each
/// reference to one of those IDs likewise, so two copies of one structure
/// read the same. Names are left out: a copy may be renamed.
fn canonical(value: &Value) -> Value {
    let mut order = Vec::new();
    ids_in(value, &mut order);
    let index: BTreeMap<u64, usize> = order.iter().enumerate().map(|(n, id)| (*id, n)).collect();
    fn walk(value: &Value, index: &BTreeMap<u64, usize>) -> Value {
        match value {
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .filter(|(key, _)| key.as_str() != "name")
                    .map(|(key, v)| {
                        let mapped = (key == "id" || REFERENCES.contains(&key.as_str()))
                            .then(|| v.as_u64().and_then(|id| index.get(&id)))
                            .flatten()
                            .map_or_else(|| walk(v, index), |n| json!(format!("#{n}")));
                        (key.clone(), mapped)
                    })
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(|v| walk(v, index)).collect()),
            other => other.clone(),
        }
    }
    walk(value, &index)
}

/// Checks that `copies` copy `originals` one for one: every ID in a copy
/// is new to the set as it was `before`; every reference a copy makes to
/// an object inside the originals points at its copy instead; and with
/// IDs set aside each copy is its original. References to objects outside
/// the originals (a return, a sidechain's source not copied) stay.
fn copied(before: &Value, after: &Value, originals: &[u64], copies: &[u64]) -> Result<(), String> {
    if originals.len() != copies.len() {
        return Err(format!(
            "{} copies of {} objects",
            copies.len(),
            originals.len()
        ));
    }
    let mut existing = Vec::new();
    ids_in(before, &mut existing);
    let existing: BTreeSet<u64> = existing.into_iter().collect();
    let mut inside = BTreeSet::new();
    for &id in originals {
        let original = object(before, id).ok_or_else(|| format!("no object #{id} before"))?;
        let mut ids = Vec::new();
        ids_in(original, &mut ids);
        inside.extend(ids);
    }
    // The originals and their copies side by side, as one structure each,
    // so references between copied objects line up.
    let mut left = Vec::new();
    let mut right = Vec::new();
    for (&original, &copy) in originals.iter().zip(copies) {
        let copy_value = object(after, copy).ok_or_else(|| format!("no copy #{copy}"))?;
        let mut ids = Vec::new();
        ids_in(copy_value, &mut ids);
        if let Some(old) = ids.iter().find(|id| existing.contains(id)) {
            return Err(format!("copy #{copy} reuses the ID #{old}"));
        }
        let mut refs = Vec::new();
        references_in(copy_value, &mut refs);
        if let Some(stale) = refs.iter().find(|id| inside.contains(id)) {
            return Err(format!(
                "copy #{copy} still refers to #{stale}, inside what was copied"
            ));
        }
        left.push(object(before, original).cloned().unwrap_or_default());
        right.push(copy_value.clone());
    }
    let (left, right) = (
        canonical(&Value::Array(left)),
        canonical(&Value::Array(right)),
    );
    if left != right {
        return Err(format!(
            "a copy differs from its original:\n  original {}\n  copy     {}",
            short(&left),
            short(&right)
        ));
    }
    Ok(())
}

/// `value` as text, cut short for a failure message.
fn short(value: &Value) -> String {
    let text = value.to_string();
    if text.len() > 1_500 {
        format!("{}…", &text[..1_500])
    } else {
        text
    }
}

/// A MIDI clip's notes as (key, start, length, velocity), in order.
fn notes(clip: &Value) -> Vec<(u64, u64, u64, u64)> {
    let mut notes: Vec<_> = clip["data"]["midi"]["notes"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|n| {
                    let field = |name: &str| n[name].as_u64().unwrap_or_default();
                    (
                        field("key"),
                        field("start"),
                        field("length"),
                        field("velocity"),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    notes.sort_unstable();
    notes
}

/// The rich set, and the IDs it was built with.
fn rich() -> (Host, Ids) {
    let mut host = Host::new();
    let ids = fixture::build(&mut host);
    (host, ids)
}

/// Runs a command, as an error message on failure.
fn run(host: &mut Host, command: &str, args: Value) -> Result<Value, String> {
    host.run(command, args)
        .map_err(|error| format!("`{command}` failed: {error}"))
}

/// Edits of many kinds to the rich set, as an agent makes them.
fn many_edits(host: &mut Host, ids: &Ids) -> Result<(), String> {
    let keys = ids.track("Keys");
    let chords = ids.clip("Chords");
    let edits = [
        (
            "track.rename",
            json!({ "track": keys, "name": "Chords and more" }),
        ),
        (
            "track.set_colour",
            json!({ "tracks": [ids.track("Bass")], "colour": 9 }),
        ),
        ("mixer.set_pan", json!({ "track": keys, "pan": -0.4 })),
        (
            "mixer.set_send",
            json!({ "track": ids.track("Tone"), "to": ids.track("Verb"), "level": 0.5 }),
        ),
        (
            "device.set_param",
            json!({ "device": ids.device(POLYSYNTH), "param": 27, "value": 0.5 }),
        ),
        (
            "device.rename",
            json!({ "device": ids.device(UTILITY), "name": "Width" }),
        ),
        (
            "note.add",
            json!({ "clip": chords, "key": 72, "start": BEAT, "length": BEAT, "velocity": 90 }),
        ),
        (
            "clip.rename",
            json!({ "clip": ids.clip("Kick"), "name": "Four on the floor" }),
        ),
        (
            "clip.set_loop_enabled",
            json!({ "clip": ids.clip("Chords slot"), "enabled": true }),
        ),
        (
            "clip.set_launch_mode",
            json!({ "clip": ids.clip("Kick slot"), "mode": "gate" }),
        ),
        (
            "clip.set_follow_actions",
            json!({ "clip": ids.clip("Chords slot"), "enabled": true,
            "action_a": "next", "chance_a": 80, "action_b": "stop" }),
        ),
        (
            "clip.set_gain",
            json!({ "clip": ids.clip("Loop"), "gain": 0.5 }),
        ),
        (
            "clip.set_warp_mode",
            json!({ "clip": ids.clip("Loop"), "mode": "complex" }),
        ),
        (
            "scene.rename",
            json!({ "scene": ids.scenes[1], "name": "Breakdown" }),
        ),
        (
            "scene.set_tempo",
            json!({ "scene": ids.scenes[1], "tempo": 96.0 }),
        ),
        (
            "automation.set_breakpoint",
            json!({ "track": ids.track("Bass"),
            "target": { "type": "mixer", "param": "pan" }, "at": BAR, "value": 0.25 }),
        ),
        (
            "song.set_tempo_breakpoint",
            json!({ "at": 8 * BAR, "bpm": 140.0 }),
        ),
        ("transport.set_loop_enabled", json!({ "enabled": true })),
        (
            "session.set_quantization",
            json!({ "quantization": "half" }),
        ),
    ];
    for (command, args) in edits {
        run(host, command, args)?;
    }
    run(
        host,
        "track.duplicate",
        json!({ "tracks": [ids.track("Tone")] }),
    )?;
    run(
        host,
        "device.duplicate",
        json!({ "device": ids.device("rack:audio_effect") }),
    )?;
    run(host, "arrangement.duplicate", json!({ "clips": [chords] }))?;
    Ok(())
}

/// Saves the open set into a fresh project in the host's folder, reopens
/// it, and fails unless the reopened document equals the saved one.
fn saved_and_reopened(host: &mut Host, name: &str) -> Result<(), String> {
    let dir = host.dir().join(name);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    host.save_to(&dir);
    let saved = host.document().clone();
    host.reopen()?;
    let reopened = host.document().clone();
    if saved != reopened {
        return Err(format!(
            "the reopened set differs:\n{}",
            document_diff(&saved, &reopened, 10).join("\n")
        ));
    }
    host.check();
    Ok(())
}

#[test]
fn the_rich_set_after_many_kinds_of_edits_saves_and_reopens_the_same() {
    let (mut host, ids) = rich();
    let outcome =
        many_edits(&mut host, &ids).and_then(|()| saved_and_reopened(&mut host, "edited"));
    ratchet(vec![(
        "the rich set after many kinds of edits, saved and reopened",
        outcome,
    )]);
}

/// What a DAWproject keeps of a set, by names rather than IDs: each track
/// with its kind, group, device types, mixer, sends and clips (their
/// places and notes), the tempo map and the time signatures.
fn exchange_summary(doc: &Value) -> Value {
    fn devices(track: &Value) -> Vec<Value> {
        track["devices"]
            .as_array()
            .map(|items| items.iter().map(|d| d["kind"].clone()).collect())
            .unwrap_or_default()
    }
    fn clips(list: &Value) -> Vec<Value> {
        list.as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|c| {
                        let clip = c.get("clip").unwrap_or(c);
                        json!({ "start": clip["start"], "length": clip["length"],
                                "notes": format!("{:?}", notes(clip)) })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    fn round(value: &Value) -> Value {
        value
            .as_f64()
            .map_or(Value::Null, |v| json!((v * 1_000.0).round() / 1_000.0))
    }
    fn walk(tracks: &Value, parent: &str, names: &BTreeMap<u64, String>, out: &mut Vec<Value>) {
        for track in tracks.as_array().into_iter().flatten() {
            let name = track["name"].as_str().unwrap_or_default();
            let sends: Vec<Value> = track["sends"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|s| {
                            let to = s[0].as_u64().and_then(|id| names.get(&id)).cloned();
                            json!([to, round(&s[1])])
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.push(json!({
                "name": name, "kind": track["kind"], "group": parent,
                "devices": devices(track),
                "volume": round(&track["mixer"]["volume"]), "pan": round(&track["mixer"]["pan"]),
                "sends": sends, "clips": clips(&track["clips"]), "slots": clips(&track["slots"]),
            }));
            walk(&track["children"], name, names, out);
        }
    }
    let mut names = BTreeMap::new();
    let mut all = Vec::new();
    ids_in(doc, &mut all);
    for id in all {
        if let Some(name) = object(doc, id).and_then(|o| o["name"].as_str()) {
            names.insert(id, name.to_owned());
        }
    }
    let mut tracks = Vec::new();
    walk(&doc["tracks"], "", &names, &mut tracks);
    walk(&doc["returns"], "returns", &names, &mut tracks);
    json!({
        "tracks": tracks,
        "main": devices(&doc["main"]),
        "tempo": doc["settings"]["tempo"],
        "time_signatures": doc["settings"]["time_signatures"],
        "scenes": doc["scenes"].as_array().map_or(0, Vec::len),
    })
}

/// Where two summaries differ, field by field.
fn summary_diff(left: &Value, right: &Value, at: &str, out: &mut Vec<String>) {
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
                let null = Value::Null;
                summary_diff(
                    a.get(key).unwrap_or(&null),
                    b.get(key).unwrap_or(&null),
                    &format!("{at}/{key}"),
                    out,
                );
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (n, (x, y)) in a.iter().zip(b).enumerate() {
                summary_diff(x, y, &format!("{at}/{n}"), out);
            }
        }
        _ if left != right => out.push(format!("{at}: {} then {}", short(left), short(right))),
        _ => {}
    }
}

#[test]
fn a_dawproject_export_imported_again_keeps_the_set() {
    // Spec: dawproject R10, R13 to R15, R24, R28, R42, R50. The rich set's
    // tracks, groups, returns, clips, notes, tempo, signatures, scenes and
    // Beatlayer's own devices come back as they went out.
    let (mut host, _ids) = rich();
    let dir = host.dir().to_owned();
    host.save_to(&dir);
    let before = exchange_summary(&document(&mut host));
    eprintln!("the set as a DAWproject keeps it: {}", short(&before));
    let tracks = before["tracks"].as_array().map_or(0, Vec::len);
    assert_eq!(tracks, 7, "the rich set's tracks and returns: {before}");
    let path = dir.join("Rich.dawproject");
    let outcome = run(
        &mut host,
        "set.export_dawproject",
        json!({ "path": path.to_string_lossy() }),
    )
    .and_then(|_| {
        run(
            &mut host,
            "set.import_dawproject",
            json!({ "path": path.to_string_lossy() }),
        )
    })
    .and_then(|_| {
        host.check();
        let after = exchange_summary(&document(&mut host));
        let mut differences = Vec::new();
        summary_diff(&before, &after, "", &mut differences);
        if differences.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "the import differs:\n  {}",
                differences.join("\n  ")
            ))
        }
    });
    ratchet(vec![("DAWproject export then import", outcome)]);
}

#[test]
fn a_midi_file_written_and_read_back_keeps_the_notes() {
    let (mut host, ids) = rich();
    let path = host.dir().join("Chords.mid");
    let original = notes(object(&document(&mut host), ids.clip("Chords")).expect("the chords"));
    assert_eq!(original.len(), 15, "the chords' notes");
    let outcome = (|| {
        run(
            &mut host,
            "midi.export_clip",
            json!({ "clip": ids.clip("Chords"), "path": path.to_string_lossy() }),
        )?;
        let before = document(&mut host);
        let read = run(
            &mut host,
            "midi.import_file",
            json!({ "path": path.to_string_lossy(), "at": 0 }),
        )?;
        let after = document(&mut host);
        let mut old = Vec::new();
        ids_in(&before, &mut old);
        let mut new = Vec::new();
        ids_in(&after, &mut new);
        // The imported clip: the one MIDI clip whose ID is new.
        let clip = new
            .iter()
            .filter(|id| !old.contains(id))
            .filter_map(|id| object(&after, *id))
            .find(|o| o["data"]["midi"].is_object())
            .ok_or_else(|| format!("no new MIDI clip: {read}"))?;
        let back = notes(clip);
        if back != original {
            return Err(format!("{original:?}\n  came back as\n  {back:?}"));
        }
        Ok(())
    })();
    ratchet(vec![("MIDI file export then import", outcome)]);
}

/// The clip placed by `browser.add_to_arrangement`, from its result.
fn placed(result: &Value) -> Result<u64, String> {
    result["clips"][0]["clip"]
        .as_u64()
        .ok_or_else(|| format!("no clip placed: {result}"))
}

#[test]
fn a_clip_file_saved_and_placed_again_keeps_the_clip_and_its_chain() {
    // Spec: clip-files R1, R2, R8, R9.
    let (mut host, ids) = rich();
    let folder = host.dir().join("clips");
    std::fs::create_dir_all(&folder).expect("a folder for the clip file");
    let chords = ids.clip("Chords");
    let keys = ids.track("Keys");
    let mut outcomes = Vec::new();
    let saved = run(
        &mut host,
        "clip.save_file",
        json!({ "clip": chords, "folder": folder.to_string_lossy(), "copy": true }),
    )
    .and_then(|saved| {
        saved["path"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("no path in {saved}"))
    });
    let file = match saved {
        Ok(file) => file,
        Err(error) => {
            ratchet(vec![(".blclip save then load", Err(error))]);
            return;
        }
    };
    // On its own track, after the clip: the clip comes back, envelopes and
    // all, at its new place.
    let before = document(&mut host);
    let on_track = run(
        &mut host,
        "browser.add_to_arrangement",
        json!({ "paths": [file], "track": keys, "at": 4 * BAR }),
    )
    .and_then(|result| placed(&result))
    .and_then(|copy| {
        let after = document(&mut host);
        let mut original = object(&before, chords).cloned().unwrap_or_default();
        let mut back = object(&after, copy).cloned().unwrap_or_default();
        original["start"] = json!(0);
        back["start"] = json!(0);
        if canonical(&original) == canonical(&back) {
            Ok(())
        } else {
            Err(format!(
                "the clip came back changed:\n  {}\n  {}",
                short(&canonical(&original)),
                short(&canonical(&back))
            ))
        }
    });
    outcomes.push((".blclip save then load", on_track));
    // On a new track: its chain comes too.
    let new_track = run(
        &mut host,
        "browser.add_to_arrangement",
        json!({ "paths": [file], "at": 0 }),
    )
    .and_then(|result| placed(&result))
    .and_then(|copy| {
        let after = document(&mut host);
        let track = after["tracks"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| object(t, copy).is_some())
            .ok_or("the placed clip is on no top-level track")?;
        let types = |t: &Value| -> Vec<Value> {
            t["devices"]
                .as_array()
                .map(|d| d.iter().map(|d| d["kind"].clone()).collect())
                .unwrap_or_default()
        };
        let original = object(&after, keys).map(types).unwrap_or_default();
        if types(track) == original {
            Ok(())
        } else {
            Err(format!(
                "the chain came back as {:?}, not {original:?}",
                types(track)
            ))
        }
    });
    outcomes.push((".blclip placed on a new track brings its chain", new_track));
    ratchet(outcomes);
}

#[test]
fn copies_and_duplicates_get_new_ids_and_remap_their_references() {
    // A copy is the original under new IDs, its references between copied
    // objects pointing at the copies.
    let (mut host, ids) = rich();
    let mut outcomes = Vec::new();

    // A track whose Wave modulator is mapped to its own Auto Filter.
    let keys = ids.track("Keys");
    let before = document(&mut host);
    let outcome = run(&mut host, "track.duplicate", json!({ "tracks": [keys] })).and_then(|made| {
        let copy = made["tracks"][0]
            .as_u64()
            .ok_or(format!("no track in {made}"))?;
        copied(&before, &document(&mut host), &[keys], &[copy])
    });
    outcomes.push(("track.duplicate", outcome));

    // Two tracks copied together: the Compressor's sidechain from Loop
    // follows Loop's copy.
    let pair = [ids.track("Bass"), ids.track("Loop")];
    let before = document(&mut host);
    let outcome = run(&mut host, "track.copy", json!({ "tracks": pair }))
        .and_then(|_| {
            run(
                &mut host,
                "track.paste",
                json!({ "track": ids.track("Echo") }),
            )
        })
        .and_then(|made| {
            let copies: Vec<u64> = made["tracks"]
                .as_array()
                .map(|t| t.iter().filter_map(Value::as_u64).collect())
                .unwrap_or_default();
            copied(&before, &document(&mut host), &pair, &copies)
        });
    outcomes.push(("track.copy and track.paste", outcome));

    // A group: its tracks come with it.
    let band = ids.track("Band");
    let before = document(&mut host);
    let outcome = run(&mut host, "track.duplicate", json!({ "tracks": [band] })).and_then(|made| {
        let copy = made["tracks"][0]
            .as_u64()
            .ok_or(format!("no track in {made}"))?;
        copied(&before, &document(&mut host), &[band], &[copy])
    });
    outcomes.push(("track.duplicate of a group", outcome));

    // A rack: new IDs for its chain and devices.
    let rack = ids.device("rack:audio_effect");
    let before = document(&mut host);
    let outcome = run(&mut host, "device.duplicate", json!({ "device": rack })).and_then(|_| {
        let after = document(&mut host);
        let mut old = Vec::new();
        ids_in(&before, &mut old);
        let band_after = object(&after, band).ok_or("Band is gone")?;
        let copy = band_after["devices"]
            .as_array()
            .and_then(|d| {
                d.iter()
                    .filter_map(|d| d["id"].as_u64())
                    .find(|id| !old.contains(id))
            })
            .ok_or("no new device on Band")?;
        copied(&before, &after, &[rack], &[copy])
    });
    outcomes.push(("device.duplicate of a rack", outcome));

    // A device copied to another track.
    let filter = ids.device("beatlayer.sweep");
    let before = document(&mut host);
    let outcome = run(&mut host, "device.copy", json!({ "device": filter }))
        .and_then(|_| {
            run(
                &mut host,
                "device.paste",
                json!({ "track": ids.track("Tone") }),
            )
        })
        .and_then(|_| {
            let after = document(&mut host);
            let mut old = Vec::new();
            ids_in(&before, &mut old);
            let tone = object(&after, ids.track("Tone")).ok_or("Tone is gone")?;
            let copy = tone["devices"]
                .as_array()
                .and_then(|d| {
                    d.iter()
                        .filter_map(|d| d["id"].as_u64())
                        .find(|id| !old.contains(id))
                })
                .ok_or("no new device on Tone")?;
            copied(&before, &after, &[filter], &[copy])
        });
    outcomes.push(("device.copy and device.paste", outcome));

    // A clip with its envelope, duplicated in the Arrangement.
    let chords = ids.clip("Chords");
    let before = document(&mut host);
    let outcome = run(
        &mut host,
        "arrangement.duplicate",
        json!({ "clips": [chords] }),
    )
    .and_then(|_| {
        let after = document(&mut host);
        let mut old = Vec::new();
        ids_in(&before, &mut old);
        let keys_after = object(&after, keys).ok_or("Keys is gone")?;
        let copy = keys_after["clips"]
            .as_array()
            .and_then(|c| {
                c.iter()
                    .filter_map(|c| c["id"].as_u64())
                    .find(|id| !old.contains(id))
            })
            .ok_or("no new clip on Keys")?;
        let start = object(&after, copy).map(|c| c["start"].clone());
        if start != Some(json!(2 * BAR)) {
            return Err(format!(
                "the copy starts at {start:?}, not right after the clip"
            ));
        }
        // Its start differs by design; the rest is the original's.
        let mut moved = before.clone();
        set_field(&mut moved, chords, "start", json!(2 * BAR));
        copied(&moved, &after, &[chords], &[copy])
    });
    outcomes.push(("arrangement.duplicate", outcome));

    host.check();
    ratchet(outcomes);
}

/// Sets `field` of the object with `id` in `value`.
fn set_field(value: &mut Value, id: u64, field: &str, to: Value) {
    match value {
        Value::Object(fields) => {
            if fields.get("id").and_then(Value::as_u64) == Some(id) {
                fields.insert(field.to_owned(), to);
                return;
            }
            for v in fields.values_mut() {
                set_field(v, id, field, to.clone());
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|v| set_field(v, id, field, to.clone())),
        _ => {}
    }
}

/// Builds the rich set with crash recovery on in `recovery`, edits it
/// (saving it first in `saved`, when given), and ends the session as a
/// crash would. Returns the document as the last edit left it.
fn crash(recovery: &Path, saved: Option<&Path>) -> Result<bl_model::Document, String> {
    let mut host = Host::new();
    host.shell_mut().use_recovery(recovery.to_owned())?;
    let ids = fixture::build(&mut host);
    if let Some(folder) = saved {
        host.save_to(folder);
    }
    many_edits(&mut host, &ids)?;
    host.shell_mut().flush_recovery()?;
    let document = host.document().clone();
    // Dropping the shell parks the journal as a crash leaves it.
    drop(host);
    Ok(document)
}

/// Starts again with the same recovery folder and recovers the newest set.
fn recover(recovery: &Path, crashed: &bl_model::Document) -> Result<(), String> {
    let mut host = Host::new();
    host.shell_mut().use_recovery(recovery.to_owned())?;
    let offers = run(&mut host, "set.list_recoverable", json!({}))?;
    if offers.to_string().matches("\"id\"").count() == 0 {
        return Err(format!("nothing is offered: {offers}"));
    }
    run(&mut host, "set.recover", json!({}))?;
    let recovered = host.document().clone();
    if &recovered != crashed {
        return Err(format!(
            "the recovered set differs:\n{}",
            document_diff(crashed, &recovered, 10).join("\n")
        ));
    }
    host.check();
    Ok(())
}

#[test]
fn crash_recovery_replays_the_journal_to_the_same_document() {
    // Spec: crash-recovery R1, R2, R3.
    let mut outcomes = Vec::new();
    let never_saved = TempDir::new("recovery");
    outcomes.push((
        "crash recovery of a set never saved",
        crash(never_saved.path(), None).and_then(|doc| recover(never_saved.path(), &doc)),
    ));
    let saved = TempDir::new("recovery-saved");
    let recovery = saved.path().join("Recovery");
    let project = saved.path().join("Set");
    let made = std::fs::create_dir_all(&recovery)
        .and_then(|()| std::fs::create_dir_all(&project))
        .map_err(|e| e.to_string());
    outcomes.push((
        "crash recovery of a saved set edited after its save",
        made.and_then(|()| crash(&recovery, Some(&project)))
            .and_then(|doc| recover(&recovery, &doc)),
    ));
    ratchet(outcomes);
}

/// The export formats a round trip decodes: extension, bit depth, mono.
fn formats() -> Vec<(&'static str, u8, bool)> {
    let mut formats = vec![
        ("wav", 16, false),
        ("wav", 24, false),
        ("wav", 32, false),
        ("aif", 24, false),
        ("flac", 24, false),
        ("mp3", 16, false),
        ("wav", 24, true),
    ];
    if full_depth() {
        formats.extend([
            ("aif", 16, false),
            ("aif", 32, false),
            ("flac", 16, false),
            ("wav", 16, true),
            ("wav", 32, true),
            ("aif", 24, true),
            ("flac", 24, true),
            ("mp3", 16, true),
        ]);
    }
    formats
}

#[test]
fn every_export_format_decodes_back_to_the_right_length_and_level() {
    // Spec: export R1, R4. Each format decodes to the render's frames and
    // level: lossless formats within a fraction of a decibel, MP3 within
    // one, at the same length give or take its encoder's padding.
    let (mut host, _ids) = rich();
    let reference = host.export(0, 2 * BAR, &ExportSpec::default());
    let mut outcomes = Vec::new();
    let keys: Vec<String> = formats()
        .iter()
        .map(|(ext, bits, mono)| {
            format!(
                "export {ext} {bits}-bit {}",
                if *mono { "mono" } else { "stereo" }
            )
        })
        .collect();
    for ((extension, bit_depth, mono), key) in formats().into_iter().zip(&keys) {
        let spec = ExportSpec {
            extension,
            bit_depth,
            mono,
            ..ExportSpec::default()
        };
        let outcome = host
            .export_file(0, 2 * BAR, &spec)
            .and_then(|(_, path)| Audio::decode(&path))
            .and_then(|audio| {
                let lossy = extension == "mp3";
                let channels = if mono { 1 } else { 2 };
                if audio.channels.len() != channels {
                    return Err(format!("{} channels, not {channels}", audio.channels.len()));
                }
                let frames = audio.frames() as i64 - reference.frames() as i64;
                if frames.abs() > if lossy { 2_400 } else { 0 } {
                    return Err(format!(
                        "{} frames, not {}",
                        audio.frames(),
                        reference.frames()
                    ));
                }
                let expected = if mono {
                    Audio::new(reference.rate, vec![reference.mono()]).rms_dbfs()
                } else {
                    reference.rms_dbfs()
                };
                let level = audio.rms_dbfs() - expected;
                if level.abs() > if lossy { 1.0 } else { 0.1 } {
                    return Err(format!("the level is {level:+.2} dB off the render's"));
                }
                Ok(())
            });
        outcomes.push((key.as_str(), outcome));
    }
    ratchet(outcomes);
}
