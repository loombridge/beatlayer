//! The rich set: one set that holds a little of everything the test
//! families exercise, built through the agent API as an agent builds a set,
//! so building it is itself a test.
//!
//! | Track | Kind | What it holds |
//! |---|---|---|
//! | Keys | MIDI | Cascade, then Auto Filter with a Wave modulator on its resonance; a two-bar chord clip at five velocities with a pan clip envelope; a Session clip in scene 1; volume and cutoff automation; a send to Verb |
//! | Bass | MIDI | Arpeggiator, then Snapshot holding the kick sample, then Compressor sidechained from Loop; a two-bar clip; a Session clip in scene 2; a send to Verb |
//! | Band | Group | An Audio Effect Rack of one chain: Saturator, Utility |
//! | Loop | Audio, in Band | The drum loop from bar 1; the same clip in scene 1; a send to Echo |
//! | Tone | Audio, in Band | The 48 kHz sine from bar 2 |
//! | Verb | Return | Rotunda (reverb) |
//! | Echo | Return | Reprise (delay) |
//! | Main | Main | Brickwall (limiter) |
//!
//! The song: a scene `Drop` added at the top of the Session's scenes, two
//! locators (bar 1 `Intro`, bar 3 `Verse`), a tempo change from 120 to 128
//! BPM at bar 5, a change to 3/4 at bar 6, and the loop brace over bars 1
//! and 2, switched off so playback runs straight through. Everything that
//! sounds sits in the first two bars, before the tempo and signature
//! change, so a test that plays them knows a bar is two seconds.
//!
//! It stays small, a dozen tracks at most and clips of a bar or two, since
//! test families build it many times.

use std::collections::BTreeMap;

use bl_core::time::Ticks;
use bl_devices::auto_filter::{AUTO_FILTER, FREQ as FILTER_FREQ, RES as FILTER_RES};
use bl_devices::compressor::COMPRESSOR;
use bl_devices::delay::DELAY;
use bl_devices::limiter::LIMITER;
use bl_devices::reverb::REVERB;
use bl_devices::saturator::SATURATOR;
use bl_model::modulator::WAVE_MOD;
use bl_model::{ClipData, MidiData, Note};
use serde_json::{Value, json};

use crate::{BAR, BEAT, Driver, SIXTEENTH};

/// The native Arpeggiator.
pub use bl_devices::arpeggiator::ARPEGGIATOR;
/// Cascade, the native polysynth.
pub use bl_devices::polysynth::POLYSYNTH;
/// Snapshot, the native sampler.
pub use bl_devices::sampler::SAMPLER;
/// The native Utility.
pub use bl_devices::utility::UTILITY;

/// The velocities the Keys clip's chords are played at, softest first.
pub const VELOCITIES: [u8; 5] = [20, 50, 80, 110, 127];

/// The clips of one kind of track, by view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clips {
    /// Audio clips.
    pub audio: Vec<u64>,
    /// MIDI clips.
    pub midi: Vec<u64>,
}

/// What [`build`] made, by ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ids {
    /// Every track by name, Main (`"Main"`) included.
    pub tracks: BTreeMap<String, u64>,
    /// The MIDI tracks, in order.
    pub midi_tracks: Vec<u64>,
    /// The audio tracks, in order.
    pub audio_tracks: Vec<u64>,
    /// The group tracks.
    pub groups: Vec<u64>,
    /// The return tracks, in order.
    pub returns: Vec<u64>,
    /// The Main track.
    pub main: u64,
    /// Every clip by name: `Chords`, `Kick`, `Loop`, `Tone` in the
    /// Arrangement, and `Chords slot`, `Kick slot`, `Loop slot` in the
    /// Session.
    pub clips: BTreeMap<String, u64>,
    /// The Arrangement's clips, by kind.
    pub arrangement: Clips,
    /// The Session's clips, by kind.
    pub session: Clips,
    /// Every device by its type ID (a rack by `rack:<kind>`), in the order
    /// they were made.
    pub devices: BTreeMap<String, Vec<u64>>,
    /// The Band rack's chains.
    pub chains: Vec<u64>,
    /// Every scene, top to bottom.
    pub scenes: Vec<u64>,
    /// The locators, in order.
    pub locators: Vec<u64>,
}

impl Ids {
    /// The track called `name`.
    ///
    /// # Panics
    ///
    /// If the fixture made none.
    #[must_use]
    pub fn track(&self, name: &str) -> u64 {
        *self
            .tracks
            .get(name)
            .unwrap_or_else(|| panic!("the fixture has no track `{name}`"))
    }

    /// The clip called `name`.
    ///
    /// # Panics
    ///
    /// If the fixture made none.
    #[must_use]
    pub fn clip(&self, name: &str) -> u64 {
        *self
            .clips
            .get(name)
            .unwrap_or_else(|| panic!("the fixture has no clip `{name}`"))
    }

    /// The first device of type `device_type` (a rack: `rack:<kind>`).
    ///
    /// # Panics
    ///
    /// If the fixture made none.
    #[must_use]
    pub fn device(&self, device_type: &str) -> u64 {
        self.devices
            .get(device_type)
            .and_then(|ids| ids.first().copied())
            .unwrap_or_else(|| panic!("the fixture has no `{device_type}` device"))
    }

    /// Every device the fixture made, in no particular order.
    pub fn all_devices(&self) -> impl Iterator<Item = u64> + '_ {
        self.devices.values().flatten().copied()
    }
}

/// A file in the repository's `tests/fixtures/media`.
#[must_use]
pub fn media(name: &str) -> String {
    crate::fixtures()
        .join("media")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// A file in the repository's `tests/fixtures/drums`.
#[must_use]
pub fn drums(name: &str) -> String {
    crate::fixtures()
        .join("drums")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// A native device as `device.insert` takes it, its ID the new name `name`.
#[must_use]
pub fn native(name: &str, device_type: &str, role: &str) -> Value {
    json!({ "id": { "$new": name }, "name": name, "enabled": true, "params": [],
            "role": role, "kind": { "native": { "device_type": device_type } } })
}

/// A MIDI clip's data, as `clip.create` takes it, of `notes` (key, start,
/// length, velocity) and unlooped.
#[must_use]
pub fn midi_data(notes: impl IntoIterator<Item = (u8, i64, i64, u8)>) -> Value {
    let data = MidiData::new(notes.into_iter().map(|(key, start, length, velocity)| {
        Note::new(key, Ticks(start), Ticks(length), velocity)
    }));
    serde_json::to_value(ClipData::Midi(data)).expect("MIDI data serialises")
}

/// A MIDI clip as `clip.create` and `clip.create_in_slot` take it.
#[must_use]
pub fn midi_clip(name: &str, start: i64, length: i64, data: Value) -> Value {
    json!({ "id": { "$new": name }, "name": name, "start": start, "length": length,
            "mute": false, "data": data })
}

/// The Keys clip's notes: a C major triad each half bar for two bars, at
/// each of [`VELOCITIES`] in turn, the last two at the loudest.
fn chords() -> Vec<(u8, i64, i64, u8)> {
    let mut notes = Vec::new();
    for half in 0..4i64 {
        let velocity = VELOCITIES[(half as usize).min(VELOCITIES.len() - 1)];
        for key in [60, 64, 67] {
            notes.push((key, half * BAR / 2, BAR / 2 - SIXTEENTH, velocity));
        }
    }
    // One more chord at the loudest velocity, so all five are heard.
    notes.extend([60, 64, 67].map(|key| (key, 3 * BAR / 2 + BEAT, BEAT, VELOCITIES[4])));
    notes
}

/// The Bass clip's notes: a quarter-note pulse on C2 for two bars, at
/// rising velocities.
fn pulse() -> Vec<(u8, i64, i64, u8)> {
    (0..8i64)
        .map(|beat| (36, beat * BEAT, BEAT / 2, 40 + (beat as u8) * 11))
        .collect()
}

/// Every track's ID and kind, top to bottom, Main last.
fn tracks(driver: &mut impl Driver) -> Vec<(u64, String, String)> {
    let read = driver.query(json!({ "path": "tracks", "limit": 500 }));
    read["tracks"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no tracks in {read}"))
        .iter()
        .map(|t| {
            (
                t["id"].as_u64().unwrap_or_default(),
                t["name"].as_str().unwrap_or_default().to_owned(),
                t["kind"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// The ID `$new` name `name` got in a `run` reply.
fn id(reply: &Value, name: &str) -> u64 {
    reply["ids"][name]
        .as_u64()
        .unwrap_or_else(|| panic!("no ID for `{name}` in {reply}"))
}

/// Builds the rich set (see the module's documentation) in place of the
/// open set, through `driver`, and gives what it made by ID.
///
/// # Panics
///
/// If any step fails, naming it.
pub fn build(driver: &mut impl Driver) -> Ids {
    let mut ids = Ids::default();

    // A clean slate: the default set's tracks go, all but Main.
    let deletes: Vec<Value> = tracks(driver)
        .iter()
        .filter(|(_, _, kind)| kind != "main")
        .map(|(id, _, _)| json!({ "command": "track.delete", "args": { "track": id } }))
        .collect();
    driver.batch_ok("Clear the default set", deletes);

    // The tracks.
    let track = |name: &str, kind: &str, index: usize, parent: Option<&str>| {
        let mut args = json!({ "id": { "$new": name }, "kind": kind, "name": name,
                               "index": index });
        if let Some(parent) = parent {
            args["parent"] = json!({ "$ref": parent });
        }
        json!({ "command": "track.create", "args": args })
    };
    let made = driver.batch_ok(
        "Make the tracks",
        vec![
            track("Keys", "midi", 0, None),
            track("Bass", "midi", 1, None),
            track("Band", "group", 2, None),
            track("Loop", "audio", 0, Some("Band")),
            track("Tone", "audio", 1, Some("Band")),
            track("Verb", "return", 0, None),
            track("Echo", "return", 1, None),
        ],
    );
    for name in ["Keys", "Bass", "Band", "Loop", "Tone", "Verb", "Echo"] {
        ids.tracks.insert(name.to_owned(), id(&made, name));
    }
    ids.midi_tracks = vec![ids.track("Keys"), ids.track("Bass")];
    ids.audio_tracks = vec![ids.track("Loop"), ids.track("Tone")];
    ids.groups = vec![ids.track("Band")];
    ids.returns = vec![ids.track("Verb"), ids.track("Echo")];
    ids.main = tracks(driver)
        .iter()
        .find(|(_, _, kind)| kind == "main")
        .map(|(id, _, _)| *id)
        .expect("a Main track");
    ids.tracks.insert("Main".to_owned(), ids.main);

    // The devices, a native one of each family.
    let insert = |track: u64, index: usize, device: Value| {
        json!({ "command": "device.insert",
                "args": { "track": track, "index": index, "device": device } })
    };
    let rack = json!({
        "id": { "$new": "rack" }, "name": "Audio Effect Rack", "enabled": true, "params": [],
        "role": "audio_effect", "kind": { "rack": { "kind": "audio_effect" } },
        "chains": [{ "id": { "$new": "chain" }, "name": "Colour", "devices": [
            native("saturator", SATURATOR, "audio_effect"),
            native("utility", UTILITY, "audio_effect"),
        ] }],
    });
    let devices = driver.batch_ok(
        "Add the devices",
        vec![
            insert(
                ids.track("Keys"),
                0,
                native("cascade", POLYSYNTH, "instrument"),
            ),
            insert(
                ids.track("Keys"),
                1,
                native("filter", AUTO_FILTER, "audio_effect"),
            ),
            insert(
                ids.track("Keys"),
                2,
                native("wave", WAVE_MOD, "audio_effect"),
            ),
            insert(
                ids.track("Bass"),
                0,
                native("arpeggiator", ARPEGGIATOR, "midi_effect"),
            ),
            insert(
                ids.track("Bass"),
                1,
                native("snapshot", SAMPLER, "instrument"),
            ),
            insert(ids.track("Band"), 0, rack),
            insert(
                ids.track("Verb"),
                0,
                native("reverb", REVERB, "audio_effect"),
            ),
            insert(ids.track("Echo"), 0, native("delay", DELAY, "audio_effect")),
            insert(ids.main, 0, native("limiter", LIMITER, "audio_effect")),
        ],
    );
    for (name, device_type) in [
        ("cascade", POLYSYNTH),
        ("filter", AUTO_FILTER),
        ("wave", WAVE_MOD),
        ("arpeggiator", ARPEGGIATOR),
        ("snapshot", SAMPLER),
        ("rack", "rack:audio_effect"),
        ("saturator", SATURATOR),
        ("utility", UTILITY),
        ("reverb", REVERB),
        ("delay", DELAY),
        ("limiter", LIMITER),
    ] {
        ids.devices
            .entry(device_type.to_owned())
            .or_default()
            .push(id(&devices, name));
    }
    ids.chains = vec![id(&devices, "chain")];
    driver.run_ok(
        "device.load_sample",
        json!({ "device": ids.device(SAMPLER), "path": drums("kick.wav") }),
    );
    driver.run_ok(
        "modulator.map",
        json!({ "device": ids.device(WAVE_MOD), "slot": 0,
                "target": { "type": "device", "device": ids.device(AUTO_FILTER),
                            "param": FILTER_RES.0 } }),
    );
    // The compressor as the browser adds it, with its sidechain input,
    // which `device.insert` leaves out unless it's given.
    let compressor = driver.run_ok(
        "device.add",
        json!({ "track": ids.track("Bass"), "index": 2,
                "device": { "kind": { "native": { "device_type": COMPRESSOR } },
                            "name": "Compressor", "role": "audio_effect", "sidechain": true } }),
    );
    ids.devices.insert(
        COMPRESSOR.to_owned(),
        vec![
            compressor["device"]
                .as_u64()
                .unwrap_or_else(|| panic!("no compressor in {compressor}")),
        ],
    );
    driver.run_ok(
        "device.set_sidechain_input",
        json!({ "device": ids.device(COMPRESSOR),
                "input": { "type": "track", "track": ids.track("Loop"), "tap": "post_fx" } }),
    );
    driver.run_ok(
        "device.set_sidechain_enabled",
        json!({ "device": ids.device(COMPRESSOR), "enabled": true }),
    );

    // The Arrangement's clips.
    let midi = driver.batch_ok(
        "Write the parts",
        vec![
            json!({ "command": "clip.create", "args": { "track": ids.track("Keys"),
                    "clip": midi_clip("Chords", 0, 2 * BAR, midi_data(chords())) } }),
            json!({ "command": "clip.create", "args": { "track": ids.track("Bass"),
                    "clip": midi_clip("Kick", 0, 2 * BAR, midi_data(pulse())) } }),
        ],
    );
    for name in ["Chords", "Kick"] {
        let clip = id(&midi, name);
        ids.clips.insert(name.to_owned(), clip);
        ids.arrangement.midi.push(clip);
    }
    for (name, track, path, at) in [
        ("Loop", "Loop", drums("drum-loop.wav"), 0),
        ("Tone", "Tone", media("sine-48k-s24-2ch.wav"), BAR),
    ] {
        let placed = driver.run_ok(
            "browser.add_to_arrangement",
            json!({ "paths": [path], "track": ids.track(track), "at": at }),
        );
        let clip = placed["clips"][0]["clip"]
            .as_u64()
            .unwrap_or_else(|| panic!("no clip placed for {name}: {placed}"));
        ids.clips.insert(name.to_owned(), clip);
        ids.arrangement.audio.push(clip);
    }

    // The Session: a scene of its own at the top, then the slot clips.
    driver.run_ok(
        "scene.create",
        json!({ "id": { "$new": "Drop" }, "index": 0 }),
    );
    ids.scenes = scenes(driver);
    let (first, second) = (ids.scenes[0], ids.scenes[1]);
    driver.run_ok("scene.rename", json!({ "scene": first, "name": "Drop" }));
    let loop_data = driver.query(json!({ "id": ids.clip("Loop"), "detail": "full" }));
    let loop_data = loop_data["saved"]["data"].clone();
    assert!(
        loop_data.is_object(),
        "the Loop clip's data reads back in full"
    );
    let slots = driver.batch_ok(
        "Fill the slots",
        vec![
            json!({ "command": "clip.create_in_slot", "args": {
                "track": ids.track("Keys"), "scene": first,
                "clip": midi_clip("Chords slot", 0, BAR, midi_data(chords().into_iter()
                    .filter(|note| note.1 < BAR))) } }),
            json!({ "command": "clip.create_in_slot", "args": {
                "track": ids.track("Bass"), "scene": second,
                "clip": midi_clip("Kick slot", 0, BAR, midi_data(pulse().into_iter()
                    .filter(|note| note.1 < BAR))) } }),
            json!({ "command": "clip.create_in_slot", "args": {
                "track": ids.track("Loop"), "scene": first,
                "clip": { "id": { "$new": "Loop slot" }, "name": "Loop slot", "start": 0,
                          "length": BAR, "mute": false, "data": loop_data } } }),
        ],
    );
    for (name, audio) in [
        ("Chords slot", false),
        ("Kick slot", false),
        ("Loop slot", true),
    ] {
        let clip = id(&slots, name);
        ids.clips.insert(name.to_owned(), clip);
        if audio {
            ids.session.audio.push(clip);
        } else {
            ids.session.midi.push(clip);
        }
    }

    // The mix, the automation and a clip envelope.
    let keys = ids.track("Keys");
    let filter = ids.device(AUTO_FILTER);
    driver.batch_ok(
        "Mix",
        vec![
            json!({ "command": "mixer.set_send", "args": { "track": keys,
                    "to": ids.track("Verb"), "level": 0.3 } }),
            json!({ "command": "mixer.set_send", "args": { "track": ids.track("Bass"),
                    "to": ids.track("Verb"), "level": 0.1 } }),
            json!({ "command": "mixer.set_send", "args": { "track": ids.track("Loop"),
                    "to": ids.track("Echo"), "level": 0.2 } }),
            json!({ "command": "mixer.set_volume", "args": { "track": ids.track("Band"),
                    "volume": 0.7 } }),
            json!({ "command": "automation.set_breakpoint", "args": { "track": keys,
                    "target": { "type": "mixer", "param": "volume" }, "at": 0, "value": 0.6 } }),
            json!({ "command": "automation.set_breakpoint", "args": { "track": keys,
                    "target": { "type": "mixer", "param": "volume" }, "at": BAR,
                    "value": 0.9 } }),
            json!({ "command": "automation.set_breakpoint", "args": { "track": keys,
                    "target": { "type": "device", "device": filter, "param": FILTER_FREQ.0 },
                    "at": 0, "value": 600.0 } }),
            json!({ "command": "automation.set_breakpoint", "args": { "track": keys,
                    "target": { "type": "device", "device": filter, "param": FILTER_FREQ.0 },
                    "at": 2 * BAR, "value": 6_000.0 } }),
            json!({ "command": "clip.set_envelope_breakpoint", "args": {
                    "clip": ids.clip("Chords"), "target": { "type": "mixer", "param": "pan" },
                    "at": 0, "value": -0.5 } }),
            json!({ "command": "clip.set_envelope_breakpoint", "args": {
                    "clip": ids.clip("Chords"), "target": { "type": "mixer", "param": "pan" },
                    "at": BAR, "value": 0.5 } }),
        ],
    );

    // The song: locators, a tempo and a signature change, the loop brace.
    let song = driver.batch_ok(
        "Lay out the song",
        vec![
            json!({ "command": "song.add_locator", "args": { "locator": {
                    "id": { "$new": "Intro" }, "position": 0, "name": "Intro" } } }),
            json!({ "command": "song.add_locator", "args": { "locator": {
                    "id": { "$new": "Verse" }, "position": 2 * BAR, "name": "Verse" } } }),
            json!({ "command": "song.set_tempo_breakpoint", "args": { "at": 0, "bpm": 120.0 } }),
            json!({ "command": "song.set_tempo_breakpoint",
                    "args": { "at": 4 * BAR, "bpm": 128.0 } }),
            json!({ "command": "song.set_time_signature_change", "args": { "at": 5 * BAR,
                    "signature": { "numerator": 3, "denominator": 4 } } }),
            json!({ "command": "transport.set_loop", "args": { "start": 0, "length": 2 * BAR } }),
        ],
    );
    ids.locators = vec![id(&song, "Intro"), id(&song, "Verse")];
    ids
}

/// Every scene's ID, top to bottom, through the `query` tool.
pub fn scenes(driver: &mut impl Driver) -> Vec<u64> {
    let mut scenes = Vec::new();
    for number in 1.. {
        let Ok(scene) = driver.tool("query", json!({ "path": format!("scene {number}") })) else {
            break;
        };
        scenes.push(
            scene["id"]
                .as_u64()
                .unwrap_or_else(|| panic!("scene {number} has no ID: {scene}")),
        );
    }
    scenes
}
