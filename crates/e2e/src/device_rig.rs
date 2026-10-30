//! Rigs for the device sweeps: every native device type, and each kind of
//! rack, on a track that gives it something to work on, set through the
//! agent API and rendered offline.
//!
//! | Role | Track | What it plays |
//! |---|---|---|
//! | Instrument | MIDI | One C3 note at each of [`VELOCITIES`], a fifth of a second apart |
//! | MIDI effect | MIDI | The same notes, through the effect and then Cascade, its Vel > Vol at its maximum |
//! | Audio effect | Audio | The fixture sine for a quarter second, then the drum loop |
//!
//! A rack is a [`Subject`] like a native type, with a chain holding a
//! simple device: Cascade in an instrument or drum rack, Pitch (followed
//! by Cascade) in a MIDI effect rack, Utility in an audio effect rack. Its
//! parameters are the chain selector, its switch and the 16 macros.
//!
//! The rules the sweep judges by live here too, each printed with the
//! sweep's summary: which parameters are themselves level controls
//! ([`is_level_control`]), which scale with velocity
//! ([`is_velocity_sensitivity`]), and which device types are random by
//! design ([`random_by_design`]).

use bl_devices::kitbag::KITBAG;
use bl_devices::lift::LIFT;
use bl_devices::multisampler::MULTISAMPLER;
use bl_devices::sampler::SAMPLER;
use bl_devices::utility::UTILITY;
use bl_devices::{DRUM_SAMPLER, POLYSYNTH};
use bl_model::{DeviceRole, ParamDescriptor, ParamKind, ParamUnit};
use serde_json::{Value, json};

use crate::audio::Audio;
use crate::fixture::{self, VELOCITIES};
use crate::host::{ExportSpec, Host, HostOptions};
use crate::{BEAT, Driver};

/// The key every rig's notes play: C3, which the samplers' first slots and
/// pads answer.
pub const KEY: u8 = 60;

/// How far apart the rig's notes start, in ticks: a fifth of a second at
/// 120 BPM.
pub const NOTE_SPACING: i64 = BEAT * 2 / 5;

/// How long each note is held, in ticks: a tenth of a second.
pub const NOTE_LENGTH: i64 = BEAT / 5;

/// How much of the song a render takes, in ticks: one second at 120 BPM,
/// which holds every note and the drum loop's first beats.
pub const RENDER_LENGTH: i64 = 2 * BEAT;

/// Where the audio rig's drum loop starts, in ticks: straight after the
/// quarter second of sine.
pub const LOOP_START: i64 = BEAT / 2;

/// The level a silent render is measured at, in dBFS, so a device whose
/// default is silent still has a level to compare with.
pub const FLOOR_DB: f64 = -80.0;

/// How far above the default setting any setting may raise the level, in
/// dB, unless the parameter is itself a level control.
pub const LEVEL_HEADROOM_DB: f64 = 18.0;

/// What the sweep tests: a native device type, or a kind of rack.
#[derive(Clone, Debug)]
pub struct Subject {
    /// Its key in the known-failures list: the native type's ID, or
    /// `rack:<kind>` for a rack.
    pub key: String,
    /// Its own name, such as `Utility`.
    pub name: String,
    /// Where it goes in a chain.
    pub role: DeviceRole,
    /// Its kind, as `device.insert` takes it.
    pub kind: Value,
    /// Its parameters' descriptors.
    pub params: Vec<ParamDescriptor>,
}

impl Subject {
    /// Whether it's a rack.
    #[must_use]
    pub fn is_rack(&self) -> bool {
        self.key.starts_with("rack:")
    }

    /// Whether it's an instrument (an instrument or drum rack included).
    #[must_use]
    pub fn is_instrument(&self) -> bool {
        self.role == DeviceRole::Instrument
    }

    /// The key for one of its parameters: `<subject>/<name>`, with the
    /// parameter's number after a `#` where two of its parameters share a
    /// name.
    #[must_use]
    pub fn param_key(&self, param: &ParamDescriptor) -> String {
        let shared = self.params.iter().filter(|p| p.name == param.name).count() > 1;
        if shared {
            format!("{}/{} #{}", self.key, param.name, param.id.0)
        } else {
            format!("{}/{}", self.key, param.name)
        }
    }

    /// The device as `device.insert` takes it, its ID the new name `name`;
    /// a rack with its one chain.
    #[must_use]
    pub fn device(&self, name: &str) -> Value {
        let role = role_name(self.role);
        let mut device = json!({ "id": { "$new": name }, "name": self.name, "enabled": true,
                                 "params": [], "role": role, "kind": self.kind });
        if let Some(kind) = self.key.strip_prefix("rack:") {
            let chain = format!("{name} chain");
            let inner = format!("{name} inner");
            let mut chain = match kind {
                "audio_effect" => json!({ "id": { "$new": chain }, "name": "Chain",
                    "devices": [fixture::native(&inner, UTILITY, "audio_effect")] }),
                "midi_effect" => json!({ "id": { "$new": chain }, "name": "Chain",
                    "devices": [fixture::native(&inner, LIFT, "midi_effect")] }),
                _ => json!({ "id": { "$new": chain }, "name": "Chain",
                    "devices": [fixture::native(&inner, POLYSYNTH, "instrument")] }),
            };
            if kind == "drum" {
                // Every note reaches the pad's chain as it came.
                chain["receive"] = json!("all_notes");
            }
            device["chains"] = json!([chain]);
        }
        device
    }

    /// Its weight in the sweep, for sharing subjects out between tests:
    /// about how many renders it takes.
    #[must_use]
    pub fn weight(&self) -> usize {
        self.params.len() * 2 + 8
    }
}

/// A role as the agent API names it.
#[must_use]
pub fn role_name(role: DeviceRole) -> &'static str {
    match role {
        DeviceRole::Instrument => "instrument",
        DeviceRole::MidiEffect => "midi_effect",
        DeviceRole::AudioEffect => "audio_effect",
    }
}

/// Every subject: each native device type `bl_devices::device_types()`
/// lists, then the four kinds of rack.
#[must_use]
pub fn subjects() -> Vec<Subject> {
    let mut subjects: Vec<Subject> = bl_devices::device_types()
        .into_iter()
        .filter_map(|t| {
            let bl_model::DeviceKind::Native { device_type } = &t.kind else {
                return None;
            };
            Some(Subject {
                key: device_type.clone(),
                name: t.name.clone(),
                role: t.role,
                kind: serde_json::to_value(&t.kind).expect("a kind serialises"),
                params: t.description.params.clone(),
            })
        })
        .collect();
    for (kind, name, role) in [
        ("instrument", "Instrument Rack", DeviceRole::Instrument),
        ("drum", "Drum Rack", DeviceRole::Instrument),
        ("midi_effect", "MIDI Effect Rack", DeviceRole::MidiEffect),
        ("audio_effect", "Audio Effect Rack", DeviceRole::AudioEffect),
    ] {
        subjects.push(Subject {
            key: format!("rack:{kind}"),
            name: name.to_owned(),
            role,
            kind: json!({ "rack": { "kind": kind } }),
            params: bl_model::rack_description().params.clone(),
        });
    }
    subjects
}

/// Shares `subjects` out between `shards` tests by weight, heaviest first,
/// each to the lightest shard so far, and gives shard `index`'s, in the
/// order they were listed.
#[must_use]
pub fn shard(subjects: Vec<Subject>, index: usize, shards: usize) -> Vec<Subject> {
    let mut order: Vec<usize> = (0..subjects.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(subjects[i].weight()), i));
    let mut load = vec![0usize; shards];
    let mut owner = vec![0usize; subjects.len()];
    for i in order {
        let lightest = (0..shards)
            .min_by_key(|&s| (load[s], s))
            .expect("at least one shard");
        load[lightest] += subjects[i].weight();
        owner[i] = lightest;
    }
    subjects
        .into_iter()
        .enumerate()
        .filter(|(i, _)| owner[*i] == index)
        .map(|(_, subject)| subject)
        .collect()
}

/// The words in a parameter's name that make it a level control.
const LEVEL_WORDS: [&str; 8] = [
    "gain", "level", "volume", "drive", "output", "makeup", "boost", "trim",
];

/// The rule [`is_level_control`] follows, in words, for the sweep to print.
pub const LEVEL_RULE: &str = "the reference is the default setting's level, or for an effect \
    the level without it (switched off) where that's louder, since passing the input through is \
    never a boost; a parameter is itself a level control, so may raise the level past \
    the headroom, when it's a gain (ParamKind::Gain), or its unit is decibels, or a word of its \
    name is gain, level, volume, drive, output, makeup, boost or trim; never when its name holds \
    Vel, since a velocity sensitivity scales the level, it doesn't boost it";

/// Whether `param` is itself a gain, level or drive control (see
/// [`LEVEL_RULE`]).
#[must_use]
pub fn is_level_control(param: &ParamDescriptor) -> bool {
    if param.name.contains("Vel") {
        return false;
    }
    if param.is_gain() {
        return true;
    }
    if let ParamKind::Continuous { unit, .. } | ParamKind::Integer { unit, .. } = &param.kind
        && *unit == ParamUnit::Decibels
    {
        return true;
    }
    let name = param.name.to_lowercase();
    name.split(|c: char| !c.is_alphanumeric())
        .any(|word| LEVEL_WORDS.contains(&word))
}

/// The rule [`is_velocity_sensitivity`] follows, in words.
pub const VELOCITY_RULE: &str = "an instrument's velocity sensitivity for its level is a \
    continuous parameter whose name holds Vel and names the volume too (Vol), or is Velocity \
    alone (Simpler's, which scales the volume); at its maximum, the softest note is no louder \
    (within 0.5 dB) than with it at 0. Velocity to pitch, filter, time or pan is left out: it \
    changes the level by changing the sound";

/// Whether `param` is an instrument's velocity sensitivity for its level
/// (see [`VELOCITY_RULE`]).
#[must_use]
pub fn is_velocity_sensitivity(param: &ParamDescriptor) -> bool {
    matches!(param.kind, ParamKind::Continuous { .. })
        && param.name.contains("Vel")
        && (param.name.contains("Vol") || param.name == "Velocity")
}

/// Why the device type `key` renders differently each time, if it's random
/// by design; `None` for every device that should repeat its render.
#[must_use]
pub fn random_by_design(key: &str) -> Option<&'static str> {
    let _ = key;
    None
}

/// The values the sweep sets a parameter to, each with a label: its
/// minimum and maximum, and at full depth its default, 25 % and 75 % of
/// its range too (on a step for a stepped parameter).
#[must_use]
pub fn points(param: &ParamDescriptor, full: bool) -> Vec<(&'static str, f64)> {
    let (min, max) = (param.min(), param.max());
    let mut points = vec![("minimum", min), ("maximum", max)];
    if full {
        points.extend([
            ("default", param.default),
            ("25 %", param.clamp(min + (max - min) * 0.25)),
            ("75 %", param.clamp(min + (max - min) * 0.75)),
        ]);
    }
    points
}

/// A value for `param` other than its default, for the reopen check: 75 %
/// of its range, or 25 % where that's the default, or the other end of a
/// switch.
#[must_use]
pub fn other_value(param: &ParamDescriptor) -> f64 {
    let (min, max) = (param.min(), param.max());
    for share in [0.75, 0.25, 1.0, 0.0] {
        let value = param.clamp(min + (max - min) * share);
        if value != param.default {
            return value;
        }
    }
    param.default
}

/// One subject on its track in a host of its own, ready to set and render.
pub struct Rig {
    /// The host it runs in.
    pub host: Host,
    /// What it tests.
    pub subject: Subject,
    /// The track it's on.
    pub track: u64,
    /// The device.
    pub device: u64,
}

impl std::fmt::Debug for Rig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rig")
            .field("subject", &self.subject.key)
            .field("track", &self.track)
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

impl Rig {
    /// `subject` on a track of its own in a new host, with notes or audio
    /// to play (see the module's documentation). The host's set holds that
    /// track and Main, with nothing on Main.
    ///
    /// # Panics
    ///
    /// If any step of building it fails, naming it.
    #[must_use]
    pub fn new(subject: &Subject) -> Self {
        let mut host = Host::with_options(HostOptions {
            window: false,
            ..HostOptions::default()
        });
        let tracks = host.query(json!({ "path": "tracks", "limit": 500 }));
        let deletes: Vec<Value> = tracks["tracks"]["items"]
            .as_array()
            .unwrap_or_else(|| panic!("no tracks in {tracks}"))
            .iter()
            .filter(|t| t["kind"] != "main")
            .map(|t| json!({ "command": "track.delete", "args": { "track": t["id"] } }))
            .collect();
        host.batch_ok("Clear the default set", deletes);
        let audio = subject.role == DeviceRole::AudioEffect;
        let made = host.batch_ok(
            "Make the track",
            vec![
                json!({ "command": "track.create", "args": { "id": { "$new": "Rig" },
                "kind": if audio { "audio" } else { "midi" }, "name": "Rig", "index": 0 } }),
            ],
        );
        let track = made["ids"]["Rig"]
            .as_u64()
            .unwrap_or_else(|| panic!("no track in {made}"));
        if audio {
            for (path, at) in [
                (staged(&host, &fixture::media("sine-48k-s24-2ch.wav")), 0),
                (staged(&host, &fixture::drums("drum-loop.wav")), LOOP_START),
            ] {
                host.run_ok(
                    "browser.add_to_arrangement",
                    json!({ "paths": [path], "track": track, "at": at }),
                );
            }
        } else {
            let notes = VELOCITIES
                .iter()
                .enumerate()
                .map(|(i, &velocity)| (KEY, i as i64 * NOTE_SPACING, NOTE_LENGTH, velocity));
            host.run_ok(
                "clip.create",
                json!({ "track": track, "clip": fixture::midi_clip("Notes", 0, RENDER_LENGTH,
                        fixture::midi_data(notes)) }),
            );
        }
        let mut inserts = vec![json!({ "command": "device.insert",
            "args": { "track": track, "index": 0, "device": subject.device("subject") } })];
        if subject.role == DeviceRole::MidiEffect {
            inserts.push(
                json!({ "command": "device.insert", "args": { "track": track,
                "index": 1, "device": fixture::native("synth", POLYSYNTH, "instrument") } }),
            );
        }
        let reply = host.batch_ok("Insert the device", inserts);
        let device = reply["ids"]["subject"]
            .as_u64()
            .unwrap_or_else(|| panic!("no device in {reply}"));
        if subject.role == DeviceRole::MidiEffect {
            // Cascade's level follows velocity, so an effect that changes
            // only velocities is heard.
            let synth = reply["ids"]["synth"]
                .as_u64()
                .unwrap_or_else(|| panic!("no Cascade in {reply}"));
            let vel_vol = bl_devices::device_types()
                .into_iter()
                .find(|t| {
                    matches!(&t.kind, bl_model::DeviceKind::Native { device_type }
                        if device_type == POLYSYNTH)
                })
                .and_then(|t| {
                    t.description
                        .params
                        .into_iter()
                        .find(|p| p.name == "Vel > Vol")
                })
                .expect("Cascade has Vel > Vol");
            host.run_ok(
                "device.set_param",
                json!({ "device": synth, "param": vel_vol.id, "value": vel_vol.max() }),
            );
        }
        match subject.key.as_str() {
            SAMPLER | KITBAG | DRUM_SAMPLER => {
                host.run_ok(
                    "device.load_sample",
                    json!({ "device": device, "path": staged(&host, &fixture::drums("kick.wav")) }),
                );
            }
            MULTISAMPLER => {
                host.run_ok(
                    "device.add_layers",
                    json!({ "device": device, "paths": [staged(&host, &fixture::drums("kick.wav"))] }),
                );
            }
            _ => {}
        }
        host.settle();
        Self {
            host,
            subject: subject.clone(),
            track,
            device,
        }
    }

    /// Renders [`RENDER_LENGTH`] from the song's start offline, through the
    /// agent API's `export.audio`, as a 32-bit float file it decodes and
    /// then deletes.
    ///
    /// # Panics
    ///
    /// If the export fails or its file doesn't decode.
    pub fn render(&mut self) -> Audio {
        let (_, path) = self
            .host
            .export_file(0, RENDER_LENGTH, &ExportSpec::default())
            .unwrap_or_else(|error| panic!("{}: export.audio failed: {error}", self.subject.key));
        let audio = Audio::decode(&path).unwrap_or_else(|error| panic!("{error}"));
        let _ = std::fs::remove_file(&path);
        audio
    }

    /// Sets the device's parameter `param` to `value` with `device.set_param`.
    ///
    /// # Errors
    ///
    /// The command's error.
    pub fn set(&mut self, param: &ParamDescriptor, value: f64) -> Result<(), String> {
        self.host
            .run(
                "device.set_param",
                json!({ "device": self.device, "param": param.id, "value": value }),
            )
            .map(|_| ())
    }

    /// The device's parameter `param` as the query tool reads it: the value
    /// set, or its default when none is.
    ///
    /// # Errors
    ///
    /// The tool's error, or a reply without a value.
    pub fn read(&mut self, param: &ParamDescriptor) -> Result<f64, String> {
        let reply = self.host.tool(
            "query",
            json!({ "path": format!("device {} / param {}", self.device, param.id.0) }),
        )?;
        if reply["set"] == false {
            return Ok(param.default);
        }
        reply["value"]
            .as_f64()
            .ok_or_else(|| format!("no value in {reply}"))
    }

    /// Switches the device on or off with `device.set_enabled`.
    ///
    /// # Panics
    ///
    /// If the command fails.
    pub fn set_enabled(&mut self, on: bool) {
        self.host.run_ok(
            "device.set_enabled",
            json!({ "device": self.device, "enabled": on }),
        );
    }

    /// Every parameter's value as `device.parameters` reports it, by
    /// number.
    ///
    /// # Errors
    ///
    /// The command's error, or a reply that doesn't parse.
    pub fn read_all(&mut self) -> Result<std::collections::BTreeMap<u32, f64>, String> {
        let reply = self
            .host
            .run("device.parameters", json!({ "device": self.device }))?;
        reply["params"]
            .as_array()
            .ok_or_else(|| format!("no params in {reply}"))?
            .iter()
            .map(|p| {
                let id = p["id"].as_u64().and_then(|n| u32::try_from(n).ok());
                let value = p["value"].as_f64();
                id.zip(value).ok_or_else(|| format!("a bad parameter: {p}"))
            })
            .collect()
    }

    /// Each note's level in `audio`, in dBFS (at least [`FLOOR_DB`]): the
    /// RMS from the note's start to the next's, softest first, as
    /// [`VELOCITIES`] are.
    #[must_use]
    pub fn note_levels(audio: &Audio) -> Vec<f64> {
        let spacing = frames(audio.rate, NOTE_SPACING);
        (0..VELOCITIES.len())
            .map(|i| level(&audio.slice(i * spacing, (i + 1) * spacing)))
            .collect()
    }
}

/// A copy of the fixture at `path` in the host's temporary folder, for
/// handing to Beatlayer: a command never touches the repository's own
/// files (importing writes beside the file it reads).
///
/// # Panics
///
/// If the copy fails.
#[must_use]
pub fn staged(host: &Host, path: &str) -> String {
    let source = std::path::Path::new(path);
    let name = source.file_name().expect("a fixture has a file name");
    let copy = host.dir().join("fixtures").join(name);
    if !copy.exists() {
        std::fs::create_dir_all(copy.parent().expect("a parent folder"))
            .expect("a folder for the fixtures");
        std::fs::copy(source, &copy).unwrap_or_else(|error| panic!("copying {path}: {error}"));
    }
    copy.to_string_lossy().into_owned()
}

/// `ticks` at 120 BPM as frames at `rate`.
#[must_use]
pub fn frames(rate: u32, ticks: i64) -> usize {
    let seconds = ticks as f64 / BEAT as f64 * 0.5;
    (seconds * f64::from(rate)).round() as usize
}

/// `audio`'s RMS level in dBFS, at least [`FLOOR_DB`].
#[must_use]
pub fn level(audio: &Audio) -> f64 {
    let rms = audio.rms_dbfs();
    if rms.is_nan() { rms } else { rms.max(FLOOR_DB) }
}
