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
            "recombine",
            "span",
            "strum",
            "time_warp",
            "velocity_shaper",
            "test_transpose",
            "test_shift",
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
fn two_tools_chained_by_id_read_back_as_a_chain() {
    // Spec: midi-tools R27, R28, R33. A second transformation applied to the
    // notes the first wrote joins its chain; `midi_tool.state` lists each
    // step, and a change of the first with `step` runs the second again.
    let mut client = app();
    let clip = bass(&mut client, 3);
    run(
        &mut client,
        "arrangement.select_clips",
        json!({ "clips": [clip] }),
    );
    run(
        &mut client,
        "clip_view.select_notes",
        json!({ "notes": [0, 1, 2] }),
    );
    run(
        &mut client,
        "midi_tool.apply",
        json!({ "tool": "test_transpose", "values": { "degrees": 2 }, "seed": 5 }),
    );
    let result = run(
        &mut client,
        "midi_tool.apply",
        json!({ "tool": "test_shift", "values": { "sixteenths": 4 }, "seed": 6 }),
    );
    assert_eq!(result["run"]["tool"], "test_shift");
    let state = run(&mut client, "midi_tool.state", json!({ "clip": clip }));
    let chain = state["application"]["chain"].as_array().unwrap();
    let steps: Vec<(&str, u64)> = chain
        .iter()
        .map(|s| (s["tool"].as_str().unwrap(), s["seed"].as_u64().unwrap()))
        .collect();
    assert_eq!(steps, [("test_transpose", 5), ("test_shift", 6)]);
    assert_eq!(chain[0]["values"]["degrees"], 2);
    assert_eq!(chain[1]["values"]["sixteenths"], 4);
    assert_eq!(keys(&mut client, clip), [62, 63, 64]);
    run(
        &mut client,
        "midi_tool.set",
        json!({ "tool": "test_transpose", "step": 0, "values": { "degrees": 5 } }),
    );
    assert_eq!(keys(&mut client, clip), [65, 66, 67], "the chain ran again");
    let state = run(&mut client, "midi_tool.state", json!({ "clip": clip }));
    assert_eq!(state["application"]["chain"][0]["values"]["degrees"], 5);
    assert_eq!(state["application"]["chain"][1]["seed"], 6);
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

/// A clip of these notes, `[key, start, length, velocity]` each, on the first
/// MIDI track.
fn clip_with(client: &mut Client<InProcess>, name: &str, notes: &[[u64; 4]]) -> u64 {
    let track = call(client, "query", json!({ "path": "track \"1 MIDI\"" }))["id"]
        .as_u64()
        .unwrap();
    let notes: Vec<Value> = notes
        .iter()
        .map(|[key, start, length, velocity]| {
            json!({ "key": key, "velocity": velocity, "start": start, "length": length })
        })
        .collect();
    let made = call(
        client,
        "run",
        json!({ "commands": [{ "command": "clip.create", "args": {
            "track": track,
            "clip": { "id": { "$new": name }, "name": name, "start": 0,
                      "length": 4 * 4 * BEAT, "mute": false,
                      "data": { "midi": { "notes": notes } } } } }] }),
    );
    made["ids"][name].as_u64().unwrap()
}

/// The clip's notes as `[key, start, length, velocity]`, in order of start.
fn notes_of(client: &mut Client<InProcess>, clip: u64) -> Vec<[u64; 4]> {
    let full = call(client, "query", json!({ "id": clip, "detail": "full" }));
    let mut notes: Vec<[u64; 4]> = full["saved"]["data"]["midi"]["notes"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("the clip's notes: {full}"))
        .iter()
        .map(|n| ["key", "start", "length", "velocity"].map(|field| n[field].as_u64().unwrap()))
        .collect();
    notes.sort_unstable_by_key(|n| (n[1], n[0]));
    notes
}

fn steps(client: &mut Client<InProcess>) -> u64 {
    call(client, "query", json!({ "path": "song" }))["undo"]["current"]
        .as_u64()
        .unwrap()
}

#[test]
fn each_transformation_applies_by_id_with_a_seed_as_one_undo_step() {
    // Spec: midi-transformations R23 to R35; midi-tools R20, R22, R23. Each
    // tool by ID with values and a seed: the notes read back, the same seed
    // gives the same notes, and one undo step puts them all back.
    type Notes = Vec<[u64; 4]>;
    let cases: [(&str, Value, Notes, Notes); 5] = [
        (
            "recombine",
            json!({ "dimension": "pitch", "shuffle": true, "mirror": true, "rotation_steps": 1 }),
            vec![
                [60, 0, BEAT, 100],
                [62, BEAT, BEAT, 100],
                [64, 2 * BEAT, BEAT, 100],
            ],
            // Pinned to the seed: the application's draw is Beatlayer's own
            // generator, the same on every platform.
            Vec::new(),
        ),
        (
            "span",
            json!({ "mode": "staccato" }),
            vec![
                [60, 0, 3 * BEAT, 100],
                [62, BEAT, BEAT, 100],
                [64, 3 * BEAT, BEAT, 100],
            ],
            vec![
                [60, 0, BEAT / 2, 100],
                [62, BEAT, BEAT / 2, 100],
                [64, 3 * BEAT, BEAT / 2, 100],
            ],
        ),
        (
            "strum",
            json!({ "strum_low": 100 }),
            vec![[60, 0, BEAT, 100], [64, 0, BEAT, 100], [67, 0, BEAT, 100]],
            // The grid step is the editor's, so the strum's size isn't pinned.
            Vec::new(),
        ),
        (
            "time_warp",
            json!({ "breakpoint_1_speed": 200, "breakpoint_2_speed": 200,
                    "preserve_time_range": false }),
            vec![
                [60, 0, BEAT, 100],
                [62, 2 * BEAT, BEAT, 100],
                [64, 4 * BEAT, BEAT, 100],
            ],
            vec![
                [60, 0, BEAT, 100],
                [62, BEAT, BEAT, 100],
                [64, 2 * BEAT, BEAT, 100],
            ],
        ),
        (
            "velocity_shaper",
            json!({ "envelope": [{ "x": 0.0, "y": 0.0 }, { "x": 1.0, "y": 1.0 }],
                    "minimum_velocity": 1, "maximum_velocity": 101 }),
            vec![
                [60, 0, BEAT, 90],
                [62, 2 * BEAT, BEAT, 90],
                [64, 4 * BEAT, BEAT, 90],
                [65, 6 * BEAT, BEAT, 90],
            ],
            vec![
                [60, 0, BEAT, 1],
                [62, 2 * BEAT, BEAT, 26],
                [64, 4 * BEAT, BEAT, 51],
                [65, 6 * BEAT, BEAT, 76],
            ],
        ),
    ];
    let mut client = app();
    for (tool, values, given, expected) in cases {
        let clip = clip_with(&mut client, tool, &given);
        let before = steps(&mut client);
        let range = json!({ "start": 0, "end": 8 * BEAT });
        let apply = |client: &mut Client<InProcess>| {
            run(
                client,
                "midi_tool.apply",
                json!({ "clip": clip, "tool": tool, "notes": (0..given.len()).collect::<Vec<_>>(),
                        "range": range, "values": values, "seed": 11 }),
            )
        };
        let result = apply(&mut client);
        assert_eq!(result["run"]["seed"], 11, "{tool}");
        assert_eq!(result["run"]["wrote"], given.len(), "{tool}");
        let written = notes_of(&mut client, clip);
        assert_ne!(written, given, "{tool} changed the notes");
        if !expected.is_empty() {
            assert_eq!(written, expected, "{tool}");
        }
        assert_eq!(steps(&mut client), before + 1, "{tool}: one undo step");
        // One step puts them all back, and the same seed writes the same.
        call(
            &mut client,
            "run",
            json!({ "commands": [{ "command": "edit.go_to_step", "args": { "index": before } }] }),
        );
        assert_eq!(notes_of(&mut client, clip), given, "{tool}: undone");
        apply(&mut client);
        assert_eq!(
            notes_of(&mut client, clip),
            written,
            "{tool}: the same seed"
        );
        // The settings read back as set.
        let state = run(&mut client, "midi_tool.state", json!({ "clip": clip }));
        let kept = state["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["tool"] == tool)
            .unwrap();
        for (name, value) in values.as_object().unwrap() {
            assert_eq!(&kept["values"][name], value, "{tool} {name}");
        }
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
