//! The M5 exit demo, rehearsed headless (T353).
//!
//! The build plan's demo is a Drum Rack beat from generated samples, with a
//! choke group and a return chain, played through an Audio Effect Rack of
//! three chains with macros mapped across them and an LFO on a filter; two
//! macro variations stored and the second launched during playback; exported,
//! and reopened identically. This test does it through the agent API on a
//! headless shell, so CI proves every step before a person sits down to it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_devices::auto_filter::{AUTO_FILTER, FREQ as FILTER_FREQ, RES as FILTER_RES};
use bl_devices::delay::{DELAY, DRY_WET as DELAY_DRY_WET, FEEDBACK as DELAY_FEEDBACK};
use bl_devices::reverb::REVERB;
use bl_devices::saturator::{DRIVE as SATURATOR_DRIVE, DRY_WET as SATURATOR_DRY_WET, SATURATOR};
use bl_engine::{EngineConfig, MediaMap, Offline, Registry, TransportCommand};
use bl_model::modulator::WAVE_MOD;
use bl_model::{ClipData, ContentHash, MidiData, Note};
use bl_shell::Shell;
use serde_json::{Value, json};

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-cli-m5-demo-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether the tests run at full depth: `BL_TEST_DEPTH=full`, as on pushes
/// to main and in the nightly run.
fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Ok("quick") | Err(_) => false,
        Ok(other) => panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full"),
    }
}

const RATE: u32 = 48_000;
/// A 4/4 bar, a beat and a sixteenth step, in ticks.
const BAR: i64 = 2_822_400;
const STEP: i64 = BAR / 16;
/// The demo's tempo is 120 BPM, so a bar is two seconds.
const BAR_FRAMES: u64 = 96_000;

/// The generated one-shots the pads hold, with the note each sits on (General
/// MIDI's kick, snare, closed and open hat).
const PADS: [(&str, u8); 4] = [
    ("kick.wav", 36),
    ("snare.wav", 38),
    ("hat-closed.wav", 42),
    ("hat-open.wav", 46),
];

fn drum_audio(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/drums")
        .join(name)
}

/// The one-bar pattern as `(pad note, steps)`, in sixteenths. The open hat on
/// step 6 is cut by the closed hat on step 8; the kick on step 6 has died away
/// by the time the choke check listens, and nothing else sounds before step 10.
const PATTERN: [(u8, &[i64]); 4] = [
    (36, &[0, 6, 10]),
    (38, &[4, 12]),
    (42, &[0, 2, 4, 8, 10, 12, 14]),
    (46, &[6]),
];

/// The macros of the effect rack, by number.
const TONE: u32 = 1;
const SPACE: u32 = 2;
const FEEDBACK: u32 = 3;
const BLEND: u32 = 4;

/// The macros' values in the two variations (index 0 and 1), by macro.
const VARIATIONS: [[f64; 4]; 2] = [[0.25, 0.2, 0.3, 0.5], [0.8, 0.7, 0.6, 0.9]];

/// The ids the demo's batches make, by name.
struct Ids {
    rack: Value,
    /// The Audio Effect Rack's chains.
    filter_chain: Value,
    saturation_chain: Value,
    delay_chain: Value,
    filter: Value,
    saturator: Value,
    delay: Value,
    lfo: Value,
}

struct Demo {
    client: Client<InProcess>,
    folder: PathBuf,
    media: Arc<MediaMap>,
}

/// The channels of a render.
type Channels = Vec<Vec<f32>>;

impl Demo {
    fn new(folder: &Path) -> Self {
        let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
        client.initialize("m5-demo").unwrap();
        let mut media = MediaMap::new();
        for (file, _) in PADS {
            let path = drum_audio(file);
            let hash = ContentHash(*bl_media::ContentHash::of_file(&path).unwrap().as_bytes());
            let decoded = bl_media::Decoder::open(&path)
                .and_then(bl_media::Decoder::decode_to_end)
                .expect("the one-shot decodes");
            media.insert(hash, Arc::new(decoded));
        }
        Self {
            client,
            folder: folder.to_owned(),
            media: Arc::new(media),
        }
    }

    /// Runs `commands` as one undo step the agent names `label`.
    fn batch(&mut self, label: &str, commands: Vec<Value>) -> Value {
        self.client
            .call("run", json!({ "label": label, "commands": commands }))
            .expect("the call reaches the agent")
            .ok()
            .unwrap_or_else(|error| panic!("{label} failed: {error}"))
    }

    /// Runs one command, and returns its result.
    fn run(&mut self, command: &str, args: Value) -> Value {
        let reply = self
            .client
            .call(
                "run",
                json!({ "commands": [{ "command": command, "args": args }] }),
            )
            .expect("the call reaches the agent")
            .ok()
            .unwrap_or_else(|error| panic!("{command} failed: {error}"));
        reply["results"][0]["result"].clone()
    }

    fn query(&mut self, args: Value) -> Value {
        self.client
            .call("query", args)
            .expect("the call reaches the agent")
            .ok()
            .unwrap_or_else(|error| panic!("query failed: {error}"))
    }

    fn shell(&mut self) -> &mut Shell {
        &mut self.client.transport().shell
    }

    fn document(&mut self) -> Value {
        serde_json::to_value(self.shell().editor().document()).unwrap()
    }

    /// A parameter of a device, by its ID.
    fn set_param(&mut self, device: &Value, param: u32, value: f64) {
        self.run(
            "device.set_param",
            json!({ "device": device, "param": param, "value": value }),
        );
    }

    /// Macro `number` of `rack`, moved to `value`.
    fn set_macro(&mut self, rack: &Value, number: u32, value: f64) {
        self.set_param(rack, 99 + number, value);
    }

    /// An engine playing the set from its start, as the app's would after a
    /// sync: the registry holds the native devices and the pads' samples.
    fn engine(&mut self, block: usize) -> Offline {
        let mut registry = Registry::new();
        bl_devices::register(&mut registry);
        let registry = registry.with_media(self.media.clone());
        let mut offline = Offline::new(EngineConfig::new(SampleRate::HZ_48000, block), registry);
        let unhosted = offline.sync(self.shell().editor().document());
        assert!(unhosted.is_empty(), "devices with no node: {unhosted:?}");
        assert!(
            offline.missing_media().is_empty(),
            "{:?}",
            offline.missing_media()
        );
        offline.sender().transport(TransportCommand::Play);
        offline
    }

    /// Plays `frames` more frames of `offline` into `out`.
    fn play(offline: &mut Offline, frames: u64, out: &mut Channels) {
        let mut done = 0;
        while done < frames {
            let n = (offline.config().max_frames as u64).min(frames - done) as usize;
            let block = offline.process(n);
            for channel in 0..block.channels() {
                if out.len() <= channel {
                    out.push(Vec::new());
                }
                out[channel].extend_from_slice(block.channel(channel));
            }
            done += n as u64;
        }
    }

    /// The set played from its start for `frames` frames.
    fn render(&mut self, frames: u64) -> Channels {
        let mut offline = self.engine(480);
        let mut out = Vec::new();
        Self::play(&mut offline, frames, &mut out);
        assert_eq!(
            offline.heap_operations(),
            0,
            "the engine stays off the heap"
        );
        out
    }

    /// The set played from its start for `frames` frames, with variation
    /// `index` of `rack` launched, through the agent API, as the song
    /// reaches frame `at`: the launch changes the document, the engine is
    /// synced as the app's loop does after a command, and the changes reach
    /// it on the next block's first frame.
    fn render_launching(&mut self, frames: u64, at: u64, rack: &Value, index: u32) -> Channels {
        assert_eq!(at % 480, 0, "the launch falls between blocks");
        let mut offline = self.engine(480);
        let mut out = Vec::new();
        Self::play(&mut offline, at, &mut out);
        self.run(
            "rack.launch_variation",
            json!({ "rack": rack, "index": index }),
        );
        offline.sync(self.shell().editor().document());
        Self::play(&mut offline, frames - at, &mut out);
        assert_eq!(
            offline.heap_operations(),
            0,
            "the engine stays off the heap"
        );
        out
    }

    /// Exports the set from its start for `ticks` as a 32-bit WAV file, and
    /// returns its channels.
    fn export(&mut self, name: &str, ticks: i64) -> Channels {
        let path = self.folder.join(name);
        let result = self.run(
            "export.audio",
            json!({ "path": path, "bit_depth": 32, "sample_rate": RATE, "dither": "none",
                    "start": 0, "length": ticks, "replace": true }),
        );
        assert!(
            result["missing_media"].as_array().unwrap().is_empty(),
            "{result}"
        );
        let audio = bl_media::decode_file(&path).expect("the export decodes");
        (0..audio.channel_count())
            .map(|channel| audio.channel(channel).unwrap().to_vec())
            .collect()
    }
}

fn rms(samples: &[f32]) -> f64 {
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len().max(1) as f64).sqrt()
}

/// The first frame where the two renders differ, in any channel.
fn first_difference(a: &Channels, b: &Channels) -> Option<usize> {
    a.iter()
        .zip(b)
        .filter_map(|(x, y)| {
            x.iter()
                .zip(y)
                .position(|(p, q)| p.to_bits() != q.to_bits())
        })
        .min()
}

/// The spectral centroid in hertz of the octave bands, each weighted by its
/// level in dB above -100 dBFS rather than by its power, so a filter
/// opening shows even when the low end carries nearly all of the power.
fn centroid(samples: &[f32]) -> f64 {
    let audio = bl_analysis::Audio::new(samples, 1, RATE).unwrap();
    let bands = bl_analysis::spectrum::octave_bands(&audio);
    let weights: Vec<f64> = bands
        .iter()
        .map(|band| bl_analysis::power_db(band.power).map_or(0.0, |db| (db + 100.0).max(0.0)))
        .collect();
    let total: f64 = weights.iter().sum();
    assert!(total > 0.0, "the window is silent");
    bands
        .iter()
        .zip(&weights)
        .map(|(band, weight)| band.centre * weight)
        .sum::<f64>()
        / total
}

fn native(name: &str, device_type: &str) -> Value {
    json!({ "id": {"$new": name}, "name": name, "enabled": true, "params": [],
            "role": "audio_effect", "kind": { "native": { "device_type": device_type } } })
}

impl Demo {
    /// Clears the default set, sets 120 BPM, makes the Drums track with an
    /// empty Drum Rack and the one-bar pattern, looped over `bars` bars, and
    /// puts the four one-shots on their pads. Returns the track, the rack and
    /// the pads' chains.
    fn beat(&mut self, bars: i64) -> (Value, Value, [Value; 4]) {
        let tracks = self.query(json!({ "path": "tracks" }));
        let deletes: Vec<Value> = tracks["tracks"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["kind"] != "main")
            .map(|t| json!({ "command": "track.delete", "args": { "track": t["id"] } }))
            .collect();
        self.batch("Clear the default tracks", deletes);
        let mut midi = MidiData::new(PATTERN.iter().flat_map(|&(key, steps)| {
            steps
                .iter()
                .map(move |step| Note::new(key, Ticks(step * STEP), Ticks(STEP), 100))
        }));
        midi.looping = true;
        midi.loop_start = Ticks(0);
        midi.loop_end = Some(Ticks(BAR));
        let built = self.batch(
            "Program the beat",
            vec![
                json!({ "command": "song.set_tempo", "args": { "tempo": serde_json::to_value(
                    TempoMap::constant(Bpm::new(120.0).unwrap())).unwrap() } }),
                json!({ "command": "track.create",
                        "args": { "id": {"$new": "Drums"}, "kind": "midi", "name": "Drums", "index": 0 } }),
                json!({ "command": "mixer.set_volume",
                        "args": { "track": {"$ref": "Drums"}, "volume": 0.7 } }),
                json!({ "command": "device.insert", "args": { "track": {"$ref": "Drums"}, "index": 0,
                    "device": { "id": {"$new": "kit"}, "name": "Drum Rack", "enabled": true,
                                "params": [], "role": "instrument",
                                "kind": { "rack": { "kind": "drum" } }, "chains": [] } } }),
                json!({ "command": "clip.create", "args": { "track": {"$ref": "Drums"}, "clip": {
                    "id": {"$new": "Beat"}, "name": "Beat", "start": 0, "length": bars * BAR,
                    "mute": false, "data": serde_json::to_value(ClipData::Midi(midi)).unwrap() } } }),
            ],
        );
        let (track, kit) = (built["ids"]["Drums"].clone(), built["ids"]["kit"].clone());
        let chains = PADS.map(|(file, note)| {
            let dropped = self.run(
                "pad.drop",
                json!({ "rack": kit, "note": note,
                        "samples": [drum_audio(file).to_string_lossy()] }),
            );
            dropped["chains"][0].clone()
        });
        // A drum hit plays to its end whatever the length of the note that
        // struck it: each pad's Simpler is set to One-Shot, whose Trigger
        // (on by default) ignores the note's end.
        let rack = self.query(json!({ "id": kit, "detail": "full" }));
        for chain in rack["chains"].as_array().unwrap() {
            let simpler = chain["devices"][0]["id"].clone();
            self.set_param(&simpler, bl_devices::sampler::MODE.0, 1.0);
        }
        (track, kit, chains)
    }
}

/// The frame a step of the bar falls on.
const fn step_frame(step: u64) -> u64 {
    step * BAR_FRAMES / 16
}

impl Demo {
    /// Spec: drum-racks R4. The open hat on step 6 rings until the closed hat
    /// on step 8 cuts it: with both hats in one choke group the render is the
    /// ungrouped one to the bit up to the closed hat's frame, and silent after
    /// the closed hat ends; without the group the open hat rings on.
    fn check_choke(&mut self, closed: &Value, open: &Value) {
        let bar = BAR_FRAMES;
        let ungrouped = self.render(bar);
        for chain in [closed, open] {
            self.run("chain.set_choke", json!({ "chain": chain, "choke": 1 }));
        }
        let grouped = self.render(bar);
        let cut = step_frame(8) as usize;
        let first = first_difference(&ungrouped, &grouped).expect("the choke changes the render");
        eprintln!("choke: the renders first differ at frame {first}; the closed hat is at {cut}");
        assert!(
            first >= cut && first <= cut + 8,
            "the open hat is cut at the closed hat's frame"
        );
        // The closed hat is 60 ms long; the open hat would ring until frame
        // 36,000 + 24,000. Nothing else sounds in between.
        let (from, to) = (cut + 3_360, step_frame(10) as usize - 1_000);
        let ringing = rms(&ungrouped[0][from..to]);
        let choked = rms(&grouped[0][from..to]);
        eprintln!(
            "choke: after the closed hat, rms {ringing:.5} ungrouped and {choked:.7} grouped"
        );
        assert!(
            ringing > 0.005,
            "the open hat rings on without the group: {ringing}"
        );
        assert!(choked < ringing * 0.001, "the group silences it: {choked}");
        // The closed hat itself sounds.
        assert!(rms(&grouped[0][cut..cut + 2_000]) > 0.01);
    }

    /// Spec: drum-racks R6, R7. The snare's send reaches the return chain:
    /// with the send up the render differs from the one without, from the
    /// snare's frame, and from nothing before it.
    fn check_return(&mut self, kit: &Value, snare: &Value) {
        let added = self.run(
            "chain.create",
            json!({ "rack": kit, "returns": true,
                    "device": { "name": "Reverb", "role": "audio_effect",
                                "kind": { "native": { "device_type": REVERB } } } }),
        );
        let ret = added["chain"].clone();
        let bar = BAR_FRAMES;
        let without = self.render(bar);
        self.run(
            "chain.set_send",
            json!({ "chain": snare, "to": ret, "level": 0.8 }),
        );
        let with = self.render(bar);
        let first = first_difference(&without, &with).expect("the send changes the render");
        let hit = step_frame(4) as usize;
        eprintln!("return: the renders first differ at frame {first}; the snare is at {hit}");
        // The reverb's predelay is 10 ms, so its first sound is a little
        // after the snare's.
        assert!(
            first >= hit && first <= hit + 2_400,
            "only the snare sends: {first}"
        );
        let diff: Vec<f32> = with[0]
            .iter()
            .zip(&without[0])
            .map(|(a, b)| a - b)
            .collect();
        let tail = rms(&diff[step_frame(5) as usize..step_frame(8) as usize]);
        eprintln!("return: the reverb's rms after the snare {tail:.5}");
        assert!(tail > 0.002, "the return chain's reverb sounds: {tail}");
        // It reads back.
        let rack = self.query(json!({ "id": kit, "detail": "full" }));
        assert_eq!(rack["returns"].as_array().unwrap().len(), 1);
        assert_eq!(rack["returns"][0]["id"], ret);
    }
}

impl Demo {
    /// An Audio Effect Rack after the Drum Rack, of three chains: Auto Filter
    /// with an LFO beside it, Saturator, and Delay. Four macros map across
    /// them, each mapping with a range of its own, and the LFO, synced to the
    /// song's tempo, modulates the filter's cutoff. Returns the ids.
    fn effect_rack(&mut self, track: &Value) -> Ids {
        let built = self.batch(
            "Add the effect rack",
            vec![
                json!({ "command": "device.insert", "args": { "track": track, "index": 1,
                "device": { "id": {"$new": "rack"}, "name": "Audio Effect Rack", "enabled": true,
                            "params": [], "role": "audio_effect",
                            "kind": { "rack": { "kind": "audio_effect" } },
                            "chains": [
                                { "id": {"$new": "filter chain"}, "name": "Filter", "devices": [
                                    native("filter", AUTO_FILTER), native("lfo", WAVE_MOD) ] },
                                { "id": {"$new": "saturation chain"}, "name": "Saturation",
                                  "devices": [ native("saturator", SATURATOR) ] },
                                { "id": {"$new": "delay chain"}, "name": "Delay",
                                  "devices": [ native("delay", DELAY) ] },
                            ] } } }),
            ],
        );
        let id = |name: &str| built["ids"][name].clone();
        let ids = Ids {
            rack: id("rack"),
            filter_chain: id("filter chain"),
            saturation_chain: id("saturation chain"),
            delay_chain: id("delay chain"),
            filter: id("filter"),
            saturator: id("saturator"),
            delay: id("delay"),
            lfo: id("lfo"),
        };
        let param = |device: &Value, param: bl_model::ParamId| json!({ "type": "device", "device": device, "param": param.0 });
        // Spec: macros R4, R7. Tone opens the filter and drives the saturator;
        // Space raises the delay's mix and the filter's resonance; Feedback
        // and Blend each map one parameter.
        let mappings = [
            (TONE, param(&ids.filter, FILTER_FREQ), 0.3, 0.9),
            (TONE, param(&ids.saturator, SATURATOR_DRIVE), 0.0, 0.5),
            (SPACE, param(&ids.delay, DELAY_DRY_WET), 0.0, 0.6),
            (SPACE, param(&ids.filter, FILTER_RES), 0.15, 0.55),
            (FEEDBACK, param(&ids.delay, DELAY_FEEDBACK), 0.1, 0.7),
            (BLEND, param(&ids.saturator, SATURATOR_DRY_WET), 0.3, 1.0),
        ];
        let mut commands = Vec::new();
        for (number, target, min, max) in &mappings {
            commands.push(json!({ "command": "rack.map_macro",
                "args": { "rack": ids.rack, "macro_number": number, "target": target } }));
            commands.push(json!({ "command": "rack.set_mapping_range",
                "args": { "rack": ids.rack, "target": target, "min": min, "max": max } }));
        }
        for (number, name) in [
            (TONE, "Tone"),
            (SPACE, "Space"),
            (FEEDBACK, "Feedback"),
            (BLEND, "Blend"),
        ] {
            commands.push(json!({ "command": "rack.rename_macro",
                "args": { "rack": ids.rack, "macro_number": number, "name": name } }));
        }
        // The chains add up, so each is turned down to leave headroom.
        for chain in [&ids.filter_chain, &ids.saturation_chain, &ids.delay_chain] {
            commands.push(json!({ "command": "chain.set_gain",
                "args": { "chain": chain, "volume": 0.35 } }));
        }
        self.batch("Map the macros", commands);
        let read = self.run("rack.macros", json!({ "rack": ids.rack }));
        for (number, devices) in [
            (TONE, ["filter", "saturator"]),
            (SPACE, ["delay", "filter"]),
        ] {
            let mappings = &read["macros"][number as usize - 1]["mappings"];
            let named: Vec<&str> = mappings
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["device"].as_str().unwrap())
                .collect();
            assert_eq!(
                named, devices,
                "macro {number} maps parameters in two chains"
            );
        }
        let tone = &read["macros"][0]["mappings"];
        assert_eq!(tone[0]["min_position"], 0.3);
        assert_eq!(tone[1]["max_position"], 0.5);
        // Spec: modulation R22. The LFO is synced: a sine over two bars.
        self.set_param(&ids.lfo, 2, 1.0);
        self.set_param(&ids.lfo, 5, 6.0);
        self.set_param(&ids.lfo, 67, 70.0);
        self.run(
            "modulator.map",
            json!({ "device": ids.lfo, "slot": 0, "target": param(&ids.filter, FILTER_FREQ) }),
        );
        let lfo = self.query(json!({ "id": ids.lfo }));
        assert_eq!(
            lfo["mappings"][0]["target"]["param"], FILTER_FREQ.0,
            "{lfo}"
        );
        ids
    }
}

impl Demo {
    /// Switches the effect rack's chains on and off, `on` of them on.
    fn only_chains(&mut self, ids: &Ids, on: &[&Value]) {
        for chain in [&ids.filter_chain, &ids.saturation_chain, &ids.delay_chain] {
            let enabled = on.contains(&chain);
            self.run(
                "chain.set_enabled",
                json!({ "chain": chain, "enabled": enabled }),
            );
        }
    }

    /// Spec: modulation R20, R22. The tempo-synced LFO sweeps the filter's
    /// cutoff: on the filter chain alone, over two bars of a beat that
    /// repeats every bar, each eighth of a bar's spectral centroid moves from
    /// one bar to the next with the LFO on, and hardly moves with it off.
    fn check_sweep(&mut self, ids: &Ids) {
        self.only_chains(ids, &[&ids.filter_chain]);
        let frames = 2 * BAR_FRAMES;
        let drift = |render: &Channels| -> (f64, Vec<f64>) {
            let window = (BAR_FRAMES / 8) as usize;
            let centroids: Vec<f64> = (0..16)
                .map(|w| centroid(&render[0][w * window..(w + 1) * window]))
                .collect();
            let drift = (0..8)
                .map(|w| (centroids[w + 8] / centroids[w]).ln().abs())
                .sum::<f64>()
                / 8.0;
            (drift, centroids)
        };
        let swept = self.render(frames);
        self.run(
            "device.set_enabled",
            json!({ "device": ids.lfo, "enabled": false }),
        );
        let still = self.render(frames);
        self.run(
            "device.set_enabled",
            json!({ "device": ids.lfo, "enabled": true }),
        );
        let (with, c_with) = drift(&swept);
        let (without, c_without) = drift(&still);
        eprintln!("sweep: centroids with the LFO {c_with:.0?}");
        eprintln!("sweep: centroids without it {c_without:.0?}");
        eprintln!(
            "sweep: mean octave drift between bars {with:.3} with the LFO, {without:.3} without"
        );
        assert!(with > 3.0 * without + 0.05, "the LFO sweeps the cutoff");
        self.only_chains(
            ids,
            &[&ids.filter_chain, &ids.saturation_chain, &ids.delay_chain],
        );
    }

    /// Spec: macros R26, R27, R30. Stores two variations (the Blend macro
    /// excluded from both), and leaves the rack on the first.
    fn store_variations(&mut self, ids: &Ids) {
        self.run(
            "rack.set_macro_variation_excluded",
            json!({ "rack": ids.rack, "macro_number": BLEND, "excluded": true }),
        );
        for values in VARIATIONS {
            for (number, value) in (1..).zip(values) {
                self.set_macro(&ids.rack, number, value);
            }
            self.run("rack.store_variation", json!({ "rack": ids.rack }));
        }
        self.set_macro(&ids.rack, BLEND, 0.6);
        self.run(
            "rack.launch_variation",
            json!({ "rack": ids.rack, "index": 0 }),
        );
    }

    /// Spec: macros R27, R33. A launch moves every macro that isn't excluded
    /// on one frame: on each chain alone, the render with the second variation
    /// launched at a bar line is the render without the launch to the bit up
    /// to that frame, and differs from it from that frame on.
    fn check_launch_frames(&mut self, ids: &Ids) {
        let at = BAR_FRAMES;
        let frames = at + BAR_FRAMES / 4;
        for (name, chain) in [
            ("Filter", &ids.filter_chain),
            ("Saturation", &ids.saturation_chain),
            ("Delay", &ids.delay_chain),
        ] {
            self.only_chains(ids, &[chain]);
            self.run(
                "rack.launch_variation",
                json!({ "rack": ids.rack, "index": 0 }),
            );
            let kept = self.render(frames);
            let launched = self.render_launching(frames, at, &ids.rack, 1);
            let first = first_difference(&kept, &launched).expect("the launch changes the render");
            eprintln!(
                "launch: the {name} chain first differs at frame {first}; the launch is at {at}"
            );
            assert!(
                first >= at as usize && first <= at as usize + 8,
                "{name}: {first}"
            );
        }
        self.only_chains(
            ids,
            &[&ids.filter_chain, &ids.saturation_chain, &ids.delay_chain],
        );
        self.run(
            "rack.launch_variation",
            json!({ "rack": ids.rack, "index": 0 }),
        );
    }
}

/// The macros' values, by macro.
fn macro_values(read: &Value) -> Vec<f64> {
    read["macros"].as_array().unwrap()[..4]
        .iter()
        .map(|m| m["value"].as_f64().unwrap())
        .collect()
}

/// Whether two renders are the same to the bit; `worst` is the largest
/// difference between them.
fn compare(a: &Channels, b: &Channels) -> (usize, f64) {
    assert_eq!(a.len(), b.len());
    let mut differing = 0;
    let mut worst = 0.0f64;
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.len(), y.len());
        for (p, q) in x.iter().zip(y) {
            if p.to_bits() != q.to_bits() {
                differing += 1;
                worst = worst.max(f64::from((p - q).abs()));
            }
        }
    }
    (differing, worst)
}

#[test]
fn a_drum_rack_beat_plays_through_a_multi_chain_effect_rack_and_is_exported_and_reopened() {
    let dir = TempDir::new("beat");
    let mut demo = Demo::new(&dir.0);
    let bars: i64 = if full_depth() { 8 } else { 2 };

    // The beat: four one-shots on pads, the hats in one choke group, the
    // snare sending to a return chain with a reverb.
    let (track, kit, [_kick, snare, closed, open]) = demo.beat(bars);
    demo.check_choke(&closed, &open);
    demo.check_return(&kit, &snare);

    // The effect rack, its macros and the LFO.
    let ids = demo.effect_rack(&track);
    demo.check_sweep(&ids);
    demo.store_variations(&ids);
    demo.check_launch_frames(&ids);

    // The performance: the beat plays and the second variation is launched
    // at a bar line. The render is the one without the launch to the bit up
    // to that frame, and plainly different after it.
    let launch_bar = if full_depth() { 4 } else { 1 };
    let at = launch_bar * BAR_FRAMES;
    let frames = bars as u64 * BAR_FRAMES;
    let kept = demo.render(frames);
    let before = demo.run("rack.macros", json!({ "rack": ids.rack }));
    assert_eq!(macro_values(&before), [0.25, 0.2, 0.3, 0.6]);
    let played = demo.render_launching(frames, at, &ids.rack, 1);
    let first = first_difference(&kept, &played).expect("the launch changes the render");
    eprintln!("performance: the launch is at frame {at}; the renders first differ at {first}");
    assert!(first >= at as usize && first <= at as usize + 8);
    // The macros now hold the second variation's values, but for the one
    // left out of variations, which kept its own.
    let after = demo.run("rack.macros", json!({ "rack": ids.rack }));
    assert_eq!(macro_values(&after), [0.8, 0.7, 0.6, 0.6]);
    assert_eq!(after["variations"].as_array().unwrap().len(), 2);
    let bar = BAR_FRAMES as usize;
    let (from, to) = (at as usize, at as usize + bar);
    let change: Vec<f32> = played[0][from..to]
        .iter()
        .zip(&kept[0][from..to])
        .map(|(a, b)| a - b)
        .collect();
    let share = rms(&change) / rms(&kept[0][from..to]);
    eprintln!(
        "performance: in the bar after the launch the change is {:.0} % of the level",
        share * 100.0
    );
    assert!(
        share > 0.2,
        "the second variation is plainly different: {share}"
    );

    // The exported file is what the engine plays.
    let ticks = bars * BAR;
    let first_export = demo.export("m5 first.wav", ticks);
    let engine = demo.render(frames);
    let (differing, worst) = compare(&first_export, &engine);
    eprintln!("export against the engine's render: {differing} samples differ, by up to {worst:e}");
    assert_eq!(differing, 0, "the export is the engine's render");
    let peak = first_export
        .iter()
        .flatten()
        .fold(0.0f32, |m, s| m.max(s.abs()));
    eprintln!("export: {} frames, peak {peak:.3}", first_export[0].len());
    assert!(
        peak > 0.1 && peak < 1.0,
        "the beat sounds and doesn't clip: {peak}"
    );
    // The listening check: `BL_M5_DEMO_OUT=/some/file.wav` keeps this
    // render, which the temporary folder would otherwise take with it.
    if let Ok(keep) = std::env::var("BL_M5_DEMO_OUT") {
        std::fs::copy(dir.0.join("m5 first.wav"), keep).expect("the render is kept");
    }

    // Save, close and reopen: the reopened set is the saved one.
    let saved_at = demo.run(
        "set.save_as",
        json!({ "path": dir.0.join("M5 Demo.blset").to_string_lossy() }),
    );
    let set = PathBuf::from(saved_at["path"].as_str().unwrap());
    let saved = demo.document();
    let (project, _) = bl_project::load(&set).expect("the saved set loads");
    assert_eq!(
        serde_json::to_value(&project.document).unwrap(),
        saved,
        "the file holds the document"
    );
    demo.run("set.new", json!({}));
    assert_ne!(demo.document(), saved, "closed");
    demo.run("set.open", json!({ "path": set }));
    assert_eq!(demo.document(), saved, "the reopened set is the saved one");
    // Its macros, variations and mappings came back.
    let reopened = demo.run("rack.macros", json!({ "rack": ids.rack }));
    assert_eq!(reopened, after);

    // And a second export of it nulls against the first.
    let second_export = demo.export("m5 second.wav", ticks);
    let (differing, worst) = compare(&first_export, &second_export);
    eprintln!("second export against the first: {differing} samples differ, by up to {worst:e}");
    assert_eq!(differing, 0, "the second export nulls");
}
