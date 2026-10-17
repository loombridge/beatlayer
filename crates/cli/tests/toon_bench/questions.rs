//! The benchmark's questions and how each answer is computed from its
//! fixture. `questions.json` beside the fixtures holds the answers as the
//! keys; `check_keys` recomputes them.

use std::collections::BTreeSet;
use std::fs;

use serde_json::{Value, json};

use crate::{fixture, fixtures_dir};

/// A quarter note and a 4/4 bar, in ticks.
const QUARTER: u64 = 705_600;
const BAR: u64 = 4 * QUARTER;

pub struct Question {
    pub id: &'static str,
    /// The fixture the question is about.
    pub fixture: &'static str,
    pub text: &'static str,
    /// The answer, computed from the fixture.
    answer: fn(&Value) -> String,
}

impl Question {
    /// The answer key: the answer computed from the fixture.
    pub fn key(&self) -> String {
        (self.answer)(&fixture(self.fixture))
    }
}

/// The note list of a clip fixture.
fn notes(v: &Value) -> &[Value] {
    v["content"]["notes"]["items"].as_array().expect("notes")
}

fn tracks(v: &Value) -> &[Value] {
    v["tracks"]["items"].as_array().expect("tracks")
}

fn rows<'a>(v: &'a Value, path: &str) -> &'a [Value] {
    v.pointer(path).and_then(Value::as_array).expect(path)
}

fn n(v: &Value, field: &str) -> u64 {
    v[field].as_u64().unwrap_or_else(|| panic!("{field}"))
}

fn f(v: &Value, field: &str) -> f64 {
    v[field].as_f64().unwrap_or_else(|| panic!("{field}"))
}

fn s<'a>(v: &'a Value, field: &str) -> &'a str {
    v[field].as_str().unwrap_or_else(|| panic!("{field}"))
}

fn count<T>(items: impl IntoIterator<Item = T>, keep: impl Fn(&T) -> bool) -> String {
    items
        .into_iter()
        .filter(|item| keep(item))
        .count()
        .to_string()
}

fn list<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    names.into_iter().collect::<Vec<_>>().join(", ")
}

fn keys(notes: &[Value]) -> BTreeSet<u64> {
    notes.iter().map(|note| n(note, "key")).collect()
}

fn num(x: f64) -> String {
    let text = format!("{x:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

pub fn all() -> Vec<Question> {
    let q = |id, fixture, text, answer| Question {
        id,
        fixture,
        text,
        answer,
    };
    vec![
        q(
            "overview-tracks",
            "overview",
            "How many tracks does the set have?",
            |v| v["counts"]["tracks"].to_string(),
        ),
        q(
            "overview-tempo",
            "overview",
            "What is the tempo at the start of the set, in BPM?",
            |v| num(f(&v["tempo"], "bpm_at_start")),
        ),
        q(
            "overview-clip-tracks",
            "overview",
            "How many tracks hold at least one clip?",
            |v| count(rows(v, "/tracks/items"), |t| n(t, "clips") > 0),
        ),
        q(
            "overview-id-18",
            "overview",
            "What is the name of the track whose id is 18?",
            |v| {
                let t = rows(v, "/tracks/items").iter().find(|t| n(t, "id") == 18);
                s(t.expect("track 18"), "name").to_owned()
            },
        ),
        q(
            "overview-undo",
            "overview",
            "How many undo steps does the set have?",
            |v| v["undo"]["steps"].to_string(),
        ),
        q(
            "tracks-with-devices",
            "tracks",
            "List the names of the tracks that have a device, in the order they are listed.",
            |v| {
                list(
                    tracks(v)
                        .iter()
                        .filter(|t| n(t, "devices") > 0)
                        .map(|t| s(t, "name")),
                )
            },
        ),
        q(
            "tracks-muted",
            "tracks",
            "How many tracks are muted?",
            |v| count(tracks(v), |t| t["mixer"]["mute"] == true),
        ),
        q(
            "tracks-lead-id",
            "tracks",
            "What is the id of the track named Lead?",
            |v| {
                let t = tracks(v).iter().find(|t| s(t, "name") == "Lead");
                n(t.expect("Lead"), "id").to_string()
            },
        ),
        q(
            "tracks-quietest",
            "tracks",
            "Which track has the lowest volume_db? Give its name.",
            |v| {
                let t = tracks(v).iter().min_by(|a, b| {
                    f(&a["mixer"], "volume_db").total_cmp(&f(&b["mixer"], "volume_db"))
                });
                s(t.expect("a track"), "name").to_owned()
            },
        ),
        q(
            "tracks-volume-sum",
            "tracks",
            "What is the sum of the volume of all tracks, to two decimals?",
            |v| num(tracks(v).iter().map(|t| f(&t["mixer"], "volume")).sum()),
        ),
        q(
            "tracks-left",
            "tracks",
            "How many tracks are panned to the left (pan below 0)?",
            |v| count(tracks(v), |t| f(&t["mixer"], "pan") < 0.0),
        ),
        q(
            "tracks-bare-midi",
            "tracks",
            "How many MIDI tracks have no device?",
            |v| {
                count(tracks(v), |t| {
                    s(t, "kind") == "midi" && n(t, "devices") == 0
                })
            },
        ),
        q(
            "drums-total",
            "notes_drums",
            "How many notes does the clip hold?",
            |v| notes(v).len().to_string(),
        ),
        q(
            "drums-key-42",
            "notes_drums",
            "How many notes have key 42?",
            |v| count(notes(v), |x| n(x, "key") == 42),
        ),
        q(
            "drums-key-36",
            "notes_drums",
            "How many notes have key 36?",
            |v| count(notes(v), |x| n(x, "key") == 36),
        ),
        q(
            "drums-max-velocity",
            "notes_drums",
            "What is the highest velocity of any note?",
            |v| {
                notes(v)
                    .iter()
                    .map(|x| n(x, "velocity"))
                    .max()
                    .unwrap_or(0)
                    .to_string()
            },
        ),
        q(
            "drums-first-snare",
            "notes_drums",
            "At which start, in ticks, is the first note with key 38?",
            |v| {
                let t = notes(v).iter().find(|x| n(x, "key") == 38);
                n(t.expect("a snare"), "start").to_string()
            },
        ),
        q(
            "drums-late-hats",
            "notes_drums",
            "How many notes with key 42 start in bar 5 or later? A bar is 2822400 ticks, and bar 1 starts at tick 0.",
            |v| count(notes(v), |x| n(x, "key") == 42 && n(x, "start") / BAR >= 4),
        ),
        q(
            "drums-kick-length",
            "notes_drums",
            "What is the sum of the length, in ticks, of all notes with key 36?",
            |v| {
                notes(v)
                    .iter()
                    .filter(|x| n(x, "key") == 36)
                    .map(|x| n(x, "length"))
                    .sum::<u64>()
                    .to_string()
            },
        ),
        q(
            "drums-off-beat",
            "notes_drums",
            "How many notes with key 42 start off the beat, that is at a tick that is not a multiple of 705600?",
            |v| {
                count(notes(v), |x| {
                    n(x, "key") == 42 && !n(x, "start").is_multiple_of(QUARTER)
                })
            },
        ),
        q(
            "chords-total",
            "notes_chords",
            "How many notes does the clip hold?",
            |v| notes(v).len().to_string(),
        ),
        q(
            "chords-keys",
            "notes_chords",
            "List the distinct keys of the notes, lowest first.",
            |v| {
                keys(notes(v))
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        ),
        q(
            "chords-key-60",
            "notes_chords",
            "How many notes have key 60?",
            |v| count(notes(v), |x| n(x, "key") == 60),
        ),
        q(
            "chords-at-zero",
            "notes_chords",
            "How many notes start at tick 0?",
            |v| count(notes(v), |x| n(x, "start") == 0),
        ),
        q(
            "chords-above-60",
            "notes_chords",
            "How many notes have a key above 60?",
            |v| count(notes(v), |x| n(x, "key") > 60),
        ),
        q(
            "chords-lowest",
            "notes_chords",
            "What is the lowest key of any note?",
            |v| keys(notes(v)).first().expect("a note").to_string(),
        ),
        q(
            "lead-total",
            "notes_lead",
            "How many notes does the clip hold?",
            |v| notes(v).len().to_string(),
        ),
        q(
            "lead-highest",
            "notes_lead",
            "What is the highest key of any note?",
            |v| keys(notes(v)).last().expect("a note").to_string(),
        ),
        q(
            "lead-key-69",
            "notes_lead",
            "How many notes have key 69?",
            |v| count(notes(v), |x| n(x, "key") == 69),
        ),
        q(
            "lead-commonest",
            "notes_lead",
            "Which key is played by the most notes? If several tie, give the lowest.",
            |v| {
                let tally = |key: &u64| notes(v).iter().filter(|x| n(x, "key") == *key).count();
                keys(notes(v))
                    .into_iter()
                    .max_by_key(|k| (tally(k), std::cmp::Reverse(*k)))
                    .expect("a note")
                    .to_string()
            },
        ),
        q(
            "lead-first-four-bars",
            "notes_lead",
            "How many notes start in bars 1 to 4? A bar is 2822400 ticks, and bar 1 starts at tick 0.",
            |v| count(notes(v), |x| n(x, "start") < 4 * BAR),
        ),
        q(
            "lead-velocity-sum",
            "notes_lead",
            "What is the sum of the velocity of all notes?",
            |v| {
                notes(v)
                    .iter()
                    .map(|x| n(x, "velocity"))
                    .sum::<u64>()
                    .to_string()
            },
        ),
        q(
            "commands-total",
            "commands_clip",
            "How many commands are listed?",
            |v| rows(v, "/commands").len().to_string(),
        ),
        q(
            "commands-destructive",
            "commands_clip",
            "How many commands have the permission destructive?",
            |v| {
                count(rows(v, "/commands"), |c| {
                    s(c, "permission") == "destructive"
                })
            },
        ),
        q(
            "commands-refused",
            "commands_clip",
            "Which commands are refused to agents? Give their ids, in the order they are listed.",
            |v| {
                list(
                    rows(v, "/commands")
                        .iter()
                        .filter(|c| s(c, "access") == "refused to agents")
                        .map(|c| s(c, "command")),
                )
            },
        ),
        q(
            "commands-label",
            "commands_clip",
            "What is the label of the command clip.set_gain?",
            |v| {
                let c = rows(v, "/commands")
                    .iter()
                    .find(|c| s(c, "command") == "clip.set_gain");
                s(c.expect("clip.set_gain"), "label").to_owned()
            },
        ),
        q(
            "commands-set-prefix",
            "commands_clip",
            "How many command ids start with clip.set_?",
            |v| {
                count(rows(v, "/commands"), |c| {
                    s(c, "command").starts_with("clip.set_")
                })
            },
        ),
        q(
            "describe-permission",
            "describe_clip_create",
            "What is the permission class of the command?",
            |v| s(v, "permission").to_owned(),
        ),
        q(
            "describe-properties",
            "describe_clip_create",
            "What are the names of the properties of the command's arguments, alphabetically?",
            |v| {
                let mut names: Vec<&str> = v["args"]["properties"]
                    .as_object()
                    .expect("properties")
                    .keys()
                    .map(String::as_str)
                    .collect();
                names.sort_unstable();
                list(names)
            },
        ),
        q(
            "describe-defs",
            "describe_clip_create",
            "How many definitions does the argument schema hold under $defs?",
            |v| {
                v["args"]["$defs"]
                    .as_object()
                    .expect("$defs")
                    .len()
                    .to_string()
            },
        ),
        q(
            "describe-required",
            "describe_clip_create",
            "Which arguments are required? Give their names alphabetically.",
            |v| {
                let mut names: Vec<&str> = v["args"]["required"]
                    .as_array()
                    .expect("required")
                    .iter()
                    .map(|x| x.as_str().expect("a name"))
                    .collect();
                names.sort_unstable();
                list(names)
            },
        ),
        q(
            "analysis-key",
            "analysis",
            "Which key does the analysis find?",
            |v| s(&v["key"], "key").to_owned(),
        ),
        q(
            "analysis-transients",
            "analysis",
            "How many transients does the analysis count?",
            |v| v["transients"]["count"].to_string(),
        ),
        q(
            "analysis-loudest-band",
            "analysis",
            "What is the centre frequency, in Hz, of the spectrum band with the highest level_db?",
            |v| {
                let band = rows(v, "/spectrum/bands")
                    .iter()
                    .max_by(|a, b| f(a, "level_db").total_cmp(&f(b, "level_db")));
                num(f(band.expect("a band"), "centre_hz"))
            },
        ),
        q(
            "analysis-short-term",
            "analysis",
            "How many short-term loudness readings are listed?",
            |v| rows(v, "/loudness/short_term").len().to_string(),
        ),
    ]
}

/// Whether two answers are the same: compared in lower case, ignoring a
/// closing full stop, quotes and markdown marks, and spaces round commas;
/// numbers compare by value.
pub fn same(key: &str, given: &str) -> bool {
    fn clean(text: &str) -> String {
        let text = text
            .trim()
            .trim_matches(|c: char| matches!(c, '`' | '*' | '"' | '\'' | '.' | ' '))
            .to_lowercase();
        let parts: Vec<String> = text
            .split(',')
            .map(|part| {
                let part = part.trim();
                part.parse::<f64>().map_or_else(|_| part.to_owned(), num)
            })
            .collect();
        parts.join(",")
    }
    clean(key) == clean(given)
}

/// The keys as `questions.json` holds them.
fn keys_file() -> Vec<Value> {
    all()
        .iter()
        .map(|q| {
            json!({
                "id": q.id,
                "fixture": q.fixture,
                "question": q.text,
                "answer": (q.answer)(&fixture(q.fixture)),
            })
        })
        .collect()
}

pub fn write_keys() {
    let mut text = serde_json::to_string_pretty(&keys_file()).expect("JSON");
    text.push('\n');
    fs::write(fixtures_dir().join("questions.json"), text).expect("the keys are written");
}

pub fn check_keys() {
    let text = fs::read_to_string(fixtures_dir().join("questions.json")).expect("the keys");
    let kept: Vec<Value> = serde_json::from_str(&text).expect("the keys are JSON");
    let now = keys_file();
    assert!(now.len() >= 40, "{} questions", now.len());
    let ids: BTreeSet<&str> = all().iter().map(|q| q.id).collect();
    assert_eq!(ids.len(), now.len(), "a question id is used twice");
    assert_eq!(
        kept, now,
        "the answer keys no longer match the fixtures: run `cargo test -p bl-cli --test toon_bench -- --ignored write_the_fixtures_and_keys`"
    );
}
