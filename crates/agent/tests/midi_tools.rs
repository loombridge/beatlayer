//! The MIDI Tools through the agent API (WP-6.11; the MIDI Tools spec, R23,
//! R24): `midi_tool.apply` by ID with values and a seed, read back through
//! the query tool and `midi_tool.state`, and a dry run of an apply that
//! leaves fewer notes. The rules are in the MIDI Tools spec.

use bl_agent::client::{Client, InProcess};
use bl_agent::{Agent, QuestionKind};
use bl_shell::Shell;
use serde_json::{Value, json};

const BEAT: u64 = 705_600;

fn app() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::for_app(), Shell::headless()));
    client.transport().answer = Box::new(|q| matches!(q.kind, QuestionKind::Connect { .. }));
    client.initialize("editor").unwrap();
    client
}

fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|e| panic!("{tool}: {e}"))
}

/// Runs one command and reads its result.
fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    )["results"][0]["result"]
        .clone()
}

/// A clip called Bass of `count` notes a beat apart on the first MIDI track.
fn bass(client: &mut Client<InProcess>, count: u64) -> u64 {
    clip_of(client, count, |n| (n * BEAT, BEAT))
}

/// A clip called Bass of `count` short notes off the grid, with room between.
fn loose(client: &mut Client<InProcess>, count: u64) -> u64 {
    clip_of(client, count, |n| (n * BEAT + 1000, BEAT / 2))
}

fn clip_of(client: &mut Client<InProcess>, count: u64, place: impl Fn(u64) -> (u64, u64)) -> u64 {
    let track = call(client, "query", json!({ "path": "track \"1 MIDI\"" }))["id"]
        .as_u64()
        .unwrap();
    let notes: Vec<Value> = (0..count)
        .map(|n| {
            let (start, length) = place(n);
            json!({ "key": 60 + n, "velocity": 100, "start": start, "length": length })
        })
        .collect();
    let made = call(
        client,
        "run",
        json!({ "commands": [{ "command": "clip.create", "args": {
            "track": track,
            "clip": { "id": { "$new": "Bass" }, "name": "Bass", "start": 0,
                      "length": 4 * 4 * BEAT, "mute": false,
                      "data": { "midi": { "notes": notes } } } } }] }),
    );
    made["ids"]["Bass"].as_u64().unwrap()
}

/// Each note's start, key and length, in order.
fn notes(client: &mut Client<InProcess>, clip: u64) -> Vec<(u64, u64, u64)> {
    let full = call(client, "query", json!({ "id": clip, "detail": "full" }));
    let mut notes: Vec<(u64, u64, u64)> = full["saved"]["data"]["midi"]["notes"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("the clip's notes: {full}"))
        .iter()
        .map(|n| {
            (
                n["start"].as_u64().unwrap(),
                n["key"].as_u64().unwrap(),
                n["length"].as_u64().unwrap(),
            )
        })
        .collect();
    notes.sort_unstable();
    notes
}

fn keys(client: &mut Client<InProcess>, clip: u64) -> Vec<u64> {
    let full = call(client, "query", json!({ "id": clip, "detail": "full" }));
    let mut notes: Vec<(u64, u64)> = full["saved"]["data"]["midi"]["notes"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("the clip's notes: {full}"))
        .iter()
        .map(|n| (n["start"].as_u64().unwrap(), n["key"].as_u64().unwrap()))
        .collect();
    notes.sort_unstable();
    notes.into_iter().map(|(_, key)| key).collect()
}

#[test]
fn midi_tool_apply_by_id_with_values_and_a_seed_reads_back() {
    // Spec: midi-tools R23. The tool, its values and the seed in one call;
    // the notes read back in the query tool's clip, the settings and the
    // application in `midi_tool.state`.
    let mut client = app();
    let clip = bass(&mut client, 3);
    let result = run(
        &mut client,
        "midi_tool.apply",
        json!({ "clip": clip, "tool": "test_transpose", "notes": [0, 1, 2],
                "values": { "degrees": 2, "spread": 10 }, "seed": 5 }),
    );
    assert_eq!(result["run"]["seed"], 5);
    assert_eq!(result["run"]["wrote"], 3);
    assert_eq!(keys(&mut client, clip), [62, 63, 64]);
    let state = run(&mut client, "midi_tool.state", json!({ "clip": clip }));
    assert_eq!(state["application"]["tool"], "test_transpose");
    assert_eq!(state["application"]["seed"], 5);
    assert_eq!(state["application"]["input_notes"], 3);
    let settings = state["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool"] == "test_transpose")
        .unwrap();
    assert_eq!(
        settings["values"]["degrees"], 2,
        "the values are kept as its settings"
    );
    assert_eq!(settings["at_defaults"], false);
    // The list says what to pass.
    let list = run(&mut client, "midi_tool.list", json!({}));
    let ids: Vec<&str> = list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "arpeggiate",
            "chop",
            "connect",
            "ornament",
            "quantize",
            "test_transpose",
            "rhythm",
            "seed",
            "shape",
            "euclidean",
            "test_fill"
        ]
    );
    // The same seed on the same notes gives the same result: apply again
    // from the application's input with a reseed to the same seed.
    let again = run(
        &mut client,
        "midi_tool.reseed",
        json!({ "clip": clip, "seed": 5 }),
    );
    assert_eq!(again["run"]["seed"], 5);
    assert_eq!(keys(&mut client, clip), [62, 63, 64]);
}

#[test]
fn an_apply_that_leaves_fewer_notes_reports_fewer_notes_and_asks_first() {
    // Spec: midi-tools R24, agent-permissions R11. A generator whose note
    // replaces the three notes it overlaps.
    let mut client = app();
    let track = call(&mut client, "query", json!({ "path": "track \"1 MIDI\"" }))["id"]
        .as_u64()
        .unwrap();
    let made = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "clip.create", "args": {
            "track": track,
            "clip": { "id": { "$new": "Keys" }, "name": "Keys", "start": 0,
                      "length": 4 * 4 * BEAT, "mute": false,
                      "data": { "midi": { "notes": [
                        { "key": 60, "velocity": 100, "start": 0, "length": BEAT },
                        { "key": 60, "velocity": 100, "start": 10, "length": BEAT },
                        { "key": 60, "velocity": 100, "start": 20, "length": BEAT }] } } } } }] }),
    );
    let clip = made["ids"]["Keys"].as_u64().unwrap();
    let apply = json!({ "clip": clip, "tool": "test_fill",
                        "values": { "division": "1" }, "range": { "start": 0, "end": BEAT } });
    let dry = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "midi_tool.apply", "args": apply }], "dry_run": true }),
    );
    assert_eq!(dry["results"][0]["permission"], "edit and destructive");
    assert_eq!(
        dry["results"][0]["fewer_notes"],
        json!({ "clip": clip, "has": 3, "leaves": 1 })
    );
    // Run for real, the person declines: the question names the clip, and
    // nothing changes.
    let result = client
        .call(
            "run",
            json!({ "commands": [{ "command": "midi_tool.apply", "args": apply }] }),
        )
        .unwrap();
    assert!(
        result.is_error && result.text.contains("declined"),
        "{}",
        result.text
    );
    let asked = client.transport().asked.last().unwrap().clone();
    assert!(
        asked.contains("wants to run destructive command:")
            && asked.contains("Apply MIDI Tool (destructive):")
            && asked.contains("clip \u{201c}Keys\u{201d}"),
        "{asked}"
    );
    assert_eq!(keys(&mut client, clip).len(), 3);
    // `set` and `set_auto` are judged the same way, and described as raised.
    for command in [
        "midi_tool.apply",
        "midi_tool.set",
        "midi_tool.set_auto",
        "midi_tool.reseed",
        "midi_tool.randomize",
    ] {
        let described = call(
            &mut client,
            "describe_command",
            json!({ "command": command }),
        );
        assert!(
            described["raised_by_arguments"]
                .as_str()
                .unwrap()
                .contains("fewer notes"),
            "{command}: {described}"
        );
    }
    // The commands that only change settings or view state stay as they are.
    let described = call(
        &mut client,
        "describe_command",
        json!({ "command": "midi_tool.choose" }),
    );
    assert!(described.get("raised_by_arguments").is_none());
}

#[test]
fn each_transformation_applies_by_id_with_a_seed_and_reads_back() {
    // Spec: midi-transformations R3 to R19. The five tools of WP-6.12 by ID,
    // with values and a seed, one undo step each, the notes read back in the
    // query tool's clip.
    for (tool, values) in [
        ("arpeggiate", json!({ "style": "down", "steps": 1 })),
        ("chop", json!({ "parts": 4, "gaps": 1 })),
        ("connect", json!({ "spread": 1, "rate": "1/16" })),
        ("ornament", json!({ "mode": "grace_notes" })),
        (
            "quantize",
            json!({ "grid": "1_4", "amount": 100, "end": true }),
        ),
    ] {
        let mut client = app();
        let clip = loose(&mut client, 3);
        let before = notes(&mut client, clip);
        let result = run(
            &mut client,
            "midi_tool.apply",
            json!({ "clip": clip, "tool": tool, "notes": [0, 1, 2], "values": values, "seed": 9 }),
        );
        assert_eq!(result["run"]["tool"], tool);
        assert_eq!(result["run"]["seed"], 9);
        let after = notes(&mut client, clip);
        assert_ne!(after, before, "{tool} changed the notes");
        let state = run(&mut client, "midi_tool.state", json!({ "clip": clip }));
        let settings = state["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["tool"] == tool)
            .unwrap();
        for (name, value) in values.as_object().unwrap() {
            assert_eq!(&settings["values"][name], value, "{tool} {name}");
        }
        // One undo step puts the notes back.
        run(&mut client, "edit.undo", json!({}));
        assert_eq!(notes(&mut client, clip), before, "{tool}: undo");
    }
}

/// A clip called Keys with these notes (key, start, length) on the first MIDI
/// track.
fn keys_clip(client: &mut Client<InProcess>, notes: &[(u64, u64, u64)]) -> u64 {
    let track = call(client, "query", json!({ "path": "track \"1 MIDI\"" }))["id"]
        .as_u64()
        .unwrap();
    let notes: Vec<Value> = notes
        .iter()
        .map(|(key, start, length)| {
            json!({ "key": key, "velocity": 100, "start": start, "length": length })
        })
        .collect();
    let made = call(
        client,
        "run",
        json!({ "commands": [{ "command": "clip.create", "args": {
            "track": track,
            "clip": { "id": { "$new": "Keys" }, "name": "Keys", "start": 0,
                      "length": 4 * 4 * BEAT, "mute": false,
                      "data": { "midi": { "notes": notes } } } } }] }),
    );
    made["ids"]["Keys"].as_u64().unwrap()
}

#[test]
fn each_generator_applies_by_id_with_a_seed_replacing_only_the_notes_it_overlaps() {
    // Spec: midi-generators R4 to R16, midi-tools R12. The four generators of
    // WP-6.14 by ID, with values and a seed into a one-bar range, one undo
    // step each; the same seed writes the same notes.
    let originals = [
        (60, 0, BEAT + 7),
        (60, 2 * BEAT, BEAT + 7),
        (61, 0, BEAT),
        (60, 8 * BEAT, BEAT),
    ];
    for (tool, values, wrote) in [
        (
            "rhythm",
            json!({ "pitch": 60, "steps": 8, "density": 3, "step_duration": "1/8" }),
            Some(3),
        ),
        (
            "seed",
            json!({ "pitch_range": { "low": 60, "high": 61 }, "density": 100, "voices": 3 }),
            None,
        ),
        (
            "shape",
            json!({ "pitch_range": { "low": 60, "high": 61 }, "rate": "1/4" }),
            Some(4),
        ),
        (
            "euclidean",
            json!({ "steps": 8, "density": 3, "division": "1/8", "pitch_1": 60 }),
            Some(3),
        ),
    ] {
        let apply = |client: &mut Client<InProcess>, clip: u64| {
            run(
                client,
                "midi_tool.apply",
                json!({ "clip": clip, "tool": tool, "values": values, "seed": 7,
                        "range": { "start": 0, "end": 4 * BEAT } }),
            )
        };
        let mut client = app();
        let clip = keys_clip(&mut client, &originals);
        let before = notes(&mut client, clip);
        let result = apply(&mut client, clip);
        assert_eq!(result["run"]["tool"], tool);
        assert_eq!(result["run"]["seed"], 7);
        let after = notes(&mut client, clip);
        let kept =
            |(key, start, length): &(u64, u64, u64)| after.contains(&(*start, *key, *length));
        let generated: Vec<_> = after
            .iter()
            .filter(|(start, key, length)| !originals.contains(&(*key, *start, *length)))
            .collect();
        assert!(!generated.is_empty(), "{tool} wrote notes");
        assert_eq!(result["run"]["wrote"], generated.len(), "{tool}");
        if let Some(wrote) = wrote {
            assert_eq!(generated.len(), wrote, "{tool}");
        }
        for original in &originals {
            let (key, start, length) = *original;
            let overlapped = generated
                .iter()
                .any(|(s, k, l)| *k == key && *s < start + length && start < s + l);
            assert_eq!(kept(original), !overlapped, "{tool}: the note {original:?}");
        }
        // One undo step puts the notes back.
        run(&mut client, "edit.undo", json!({}));
        assert_eq!(notes(&mut client, clip), before, "{tool}: undo");
        // The same seed, the same notes.
        let mut other = app();
        let clip = keys_clip(&mut other, &originals);
        apply(&mut other, clip);
        assert_eq!(notes(&mut other, clip), after, "{tool}: the same seed");
    }
}

#[test]
fn the_randomise_button_sets_every_rotation_from_a_seed_and_reads_back() {
    // Spec: midi-generators R16. `midi_tool.randomize` with a seed sets the
    // four Rotations, the same seed the same ones, and with Auto on applies
    // Euclidean as one undo step.
    let rotations = |client: &mut Client<InProcess>| -> Vec<Value> {
        let state = run(client, "midi_tool.state", json!({}));
        let settings = state["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["tool"] == "euclidean")
            .unwrap()
            .clone();
        (1..=4)
            .map(|voice| settings["values"][format!("rotation_{voice}")].clone())
            .collect()
    };
    let mut client = app();
    let clip = keys_clip(&mut client, &[(61, 0, BEAT)]);
    let before = notes(&mut client, clip);
    let result = run(
        &mut client,
        "midi_tool.randomize",
        json!({ "clip": clip, "seed": 3 }),
    );
    assert_eq!(result["run"]["tool"], "euclidean");
    let first = rotations(&mut client);
    assert!(
        first.iter().all(|r| r.as_u64().is_some_and(|r| r < 16)),
        "{first:?}"
    );
    assert!(
        notes(&mut client, clip).len() > before.len(),
        "Auto applied it"
    );
    run(&mut client, "edit.undo", json!({}));
    assert_eq!(notes(&mut client, clip), before, "one undo step");
    run(
        &mut client,
        "midi_tool.randomize",
        json!({ "clip": clip, "seed": 3 }),
    );
    assert_eq!(rotations(&mut client), first, "the same seed");
    run(
        &mut client,
        "midi_tool.randomize",
        json!({ "clip": clip, "seed": 4 }),
    );
    assert_ne!(rotations(&mut client), first, "another seed");
}
