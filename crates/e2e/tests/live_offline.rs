//! Live against offline: a set played on the hand-paced null device and
//! captured, against the same range exported offline. The engine renders
//! both, at different block sizes (the device's and the export's), so
//! where nothing is random they must be the same to the bit: the export
//! spec's R14 and each device's "the same at any block size" rule promise
//! it. A case that differs is either listed in `known-failures.toml` under
//! suite `live_offline`, key the case's name, property "live matches
//! offline", or fails.
//!
//! The cases: each warp mode on the drum loop and on a melody stretched
//! from 110 to 120 BPM (the remix case), MIDI through Cascade and Snapshot,
//! every native MIDI effect before Cascade, returns and sends with a
//! sidechain, tempo automation, a tempo-synced delay under a tempo ramp, a
//! group, clip envelopes, a Wave modulator, and the rich set. Full depth
//! adds more tempos and a start mid-song.
//!
//! The helpers at the top (a test signal written as a WAV file, tracks made
//! and filled through the agent API) serve the delay-compensation and
//! export-format families too.

use std::path::{Path, PathBuf};

use bl_devices::auto_filter::{AUTO_FILTER, FREQ as FILTER_FREQ};
use bl_devices::compressor::{COMPRESSOR, THRESHOLD as COMPRESSOR_THRESHOLD};
use bl_devices::delay::DELAY;
use bl_devices::reverb::REVERB;
use bl_devices::saturator::SATURATOR;
use bl_e2e::audio::{Audio, aligned_residual_db, best_alignment, level_difference_db, overlap};
use bl_e2e::fixture::{self, ARPEGGIATOR, POLYSYNTH, SAMPLER, native};
use bl_e2e::{BAR, BEAT, Driver, ExportSpec, Host, Known, Ratchet, SIXTEENTH, full_depth};
use bl_model::modulator::WAVE_MOD;
use bl_model::{DeviceKind, DeviceRole};
use serde_json::{Value, json};

/// The suite's name in `known-failures.toml`.
pub(crate) const SUITE: &str = "live_offline";

/// The property every live-against-offline case checks.
const MATCHES: &str = "live matches offline";

/// The engine's rate on the null device.
pub(crate) const RATE: u32 = 48_000;

/// How much longer than the exported range the live capture runs, in
/// milliseconds, so the capture covers the whole range.
const MARGIN_MS: u64 = 200;

// ---------------------------------------------------------------------------
// Signals and files
// ---------------------------------------------------------------------------

/// Writes `audio` as a 16-bit PCM WAV file at `path`, rounding each sample
/// to the nearest step: a fixture a test makes for itself.
///
/// # Panics
///
/// If the file can't be written.
pub(crate) fn write_wav(path: &Path, audio: &Audio) {
    let channels = audio.channels.len() as u16;
    let frames = audio.frames() as u32;
    let data_bytes = frames * u32::from(channels) * 2;
    let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&audio.rate.to_le_bytes());
    bytes.extend_from_slice(&(audio.rate * u32::from(channels) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channels * 2).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in audio.interleaved() {
        let step = (f64::from(sample) * 32_767.0)
            .round()
            .clamp(-32_768.0, 32_767.0) as i16;
        bytes.extend_from_slice(&step.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}

/// A tone of `hz` with its first three harmonics, `seconds` long, at peak
/// `amplitude`, with a 5 ms attack and an exponential decay: one note of the
/// melody.
fn note(hz: f64, seconds: f64, amplitude: f64) -> Vec<f32> {
    let frames = (seconds * f64::from(RATE)).round() as usize;
    (0..frames)
        .map(|n| {
            let t = n as f64 / f64::from(RATE);
            let attack = (t / 0.005).min(1.0);
            let decay = (-t * 2.5).exp();
            let wave: f64 = [(1.0, 1.0), (2.0, 0.5), (3.0, 0.25)]
                .iter()
                .map(|(harmonic, gain)| gain * (std::f64::consts::TAU * hz * harmonic * t).sin())
                .sum::<f64>()
                / 1.75;
            (wave * attack * decay * amplitude) as f32
        })
        .collect()
}

/// A bar of a melody at `bpm`, a note a beat (A3, C♯4, E4, A4), stereo, at
/// 48 kHz: pitched material for the stretched-melody cases.
pub(crate) fn melody(bpm: f64) -> Audio {
    let beat = 60.0 / bpm;
    let mut wave = Vec::new();
    for hz in [220.0, 277.18, 329.63, 440.0] {
        wave.extend(note(hz, beat, 0.5));
    }
    Audio::new(RATE, vec![wave.clone(), wave])
}

/// Five inharmonic sines from 210 Hz to 4.9 kHz, `seconds` long, each at
/// `amplitude` peak, on two channels (the right a little different from the
/// left): band-limited, so an oversampler's filters pass it untouched, and
/// with no period short enough to line up at a wrong lag.
pub(crate) fn partials(seconds: f64, amplitude: f64) -> Audio {
    let frames = (seconds * f64::from(RATE)).round() as usize;
    let channel = |detune: f64| -> Vec<f32> {
        (0..frames)
            .map(|n| {
                let t = n as f64 / f64::from(RATE);
                [210.0, 773.0, 1_390.0, 2_917.0, 4_903.0]
                    .iter()
                    .enumerate()
                    .map(|(i, hz)| {
                        (std::f64::consts::TAU * hz * detune * t + i as f64).sin() * amplitude
                    })
                    .sum::<f64>() as f32
            })
            .collect()
    };
    Audio::new(RATE, vec![channel(1.0), channel(1.003)])
}

// ---------------------------------------------------------------------------
// Building sets through the agent API
// ---------------------------------------------------------------------------

/// Deletes every track of the open set but Main, as one undo step.
pub(crate) fn clear(host: &mut Host) {
    let read = host.query(json!({ "path": "tracks", "limit": 500 }));
    let deletes: Vec<Value> = read["tracks"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no tracks in {read}"))
        .iter()
        .filter(|t| t["kind"] != "main")
        .map(|t| json!({ "command": "track.delete", "args": { "track": t["id"] } }))
        .collect();
    host.batch_ok("Clear the set", deletes);
}

/// Makes a track of `kind` called `name` at the top level's `index`, or in
/// group `parent`, and gives its ID.
pub(crate) fn track(
    host: &mut Host,
    name: &str,
    kind: &str,
    index: usize,
    parent: Option<u64>,
) -> u64 {
    let mut args = json!({ "id": { "$new": name }, "kind": kind, "name": name, "index": index });
    if let Some(parent) = parent {
        args["parent"] = json!(parent);
    }
    let reply = host.batch_ok(
        &format!("Make {name}"),
        vec![json!({ "command": "track.create", "args": args })],
    );
    reply["ids"][name]
        .as_u64()
        .unwrap_or_else(|| panic!("no ID for track {name}: {reply}"))
}

/// Inserts native device `device_type` at `index` on `track` with
/// `device.insert`, and gives its ID.
pub(crate) fn insert(
    host: &mut Host,
    track: u64,
    index: usize,
    device_type: &str,
    role: &str,
) -> u64 {
    let name = format!("{device_type} on {track} at {index}");
    let reply = host.batch_ok(
        &format!("Insert {device_type}"),
        vec![
            json!({ "command": "device.insert", "args": { "track": track, "index": index,
                     "device": native(&name, device_type, role) } }),
        ],
    );
    reply["ids"][name.as_str()]
        .as_u64()
        .unwrap_or_else(|| panic!("no ID for {device_type}: {reply}"))
}

/// Sets parameter `param` of `device` to `value`.
pub(crate) fn set_param(host: &mut Host, device: u64, param: u32, value: f64) {
    host.run_ok(
        "device.set_param",
        json!({ "device": device, "param": param, "value": value }),
    );
}

/// Places the audio file at `path` on `track` at `at` ticks, waits until
/// the engine has its media, and gives the clip's ID.
pub(crate) fn place(host: &mut Host, track: u64, path: &Path, at: i64) -> u64 {
    let placed = host.run_ok(
        "browser.add_to_arrangement",
        json!({ "paths": [path.to_string_lossy()], "track": track, "at": at }),
    );
    let clip = placed["clips"][0]["clip"]
        .as_u64()
        .unwrap_or_else(|| panic!("no clip placed: {placed}"));
    host.settle();
    clip
}

/// Writes `audio` to `name` in the host's folder and gives the path.
pub(crate) fn fixture_file(host: &Host, name: &str, audio: &Audio) -> PathBuf {
    let path = host.dir().join(name);
    write_wav(&path, audio);
    path
}

/// Copies the repository fixture at `path` into the host's folder and gives
/// the copy's path, so no command ever touches the repository's own files.
///
/// # Panics
///
/// If the copy fails.
pub(crate) fn copy_fixture(host: &Host, path: &str) -> PathBuf {
    let from = Path::new(path);
    let to = host
        .dir()
        .join(from.file_name().expect("a fixture has a file name"));
    if !to.exists() {
        std::fs::copy(from, &to).unwrap_or_else(|error| panic!("{path}: {error}"));
    }
    to
}

/// Makes a MIDI clip of `notes` (key, start, length, velocity) on `track`
/// from 0, `length` ticks long, and gives its ID.
fn midi_clip(
    host: &mut Host,
    track: u64,
    name: &str,
    length: i64,
    notes: Vec<(u8, i64, i64, u8)>,
) -> u64 {
    let reply = host.batch_ok(
        &format!("Write {name}"),
        vec![json!({ "command": "clip.create", "args": { "track": track,
                     "clip": fixture::midi_clip(name, 0, length, fixture::midi_data(notes)) } })],
    );
    reply["ids"][name]
        .as_u64()
        .unwrap_or_else(|| panic!("no ID for clip {name}: {reply}"))
}

/// Chords and single notes over two bars at rising velocities, with a gap
/// in each bar: something for an instrument to play.
fn phrase() -> Vec<(u8, i64, i64, u8)> {
    let mut notes = Vec::new();
    for (i, root) in [48u8, 53, 55, 50].into_iter().enumerate() {
        let at = i as i64 * BAR / 2;
        let velocity = 40 + 25 * i as u8;
        for interval in [0, 4, 7] {
            notes.push((root + interval, at, BAR / 2 - BEAT, velocity));
        }
        notes.push((root + 24, at + BAR / 4, SIXTEENTH, velocity));
    }
    notes
}

/// A MIDI track called `name` with Cascade and the phrase: the track's ID
/// and the clip's.
fn synth_track(host: &mut Host, name: &str, index: usize, parent: Option<u64>) -> (u64, u64) {
    let track = track(host, name, "midi", index, parent);
    insert(host, track, 0, POLYSYNTH, "instrument");
    let clip = midi_clip(host, track, &format!("{name} phrase"), 2 * BAR, phrase());
    (track, clip)
}

/// The drum loop on a new audio track called `name`: the track's ID and the
/// clip's.
fn loop_track(host: &mut Host, name: &str, index: usize, parent: Option<u64>) -> (u64, u64) {
    let track = track(host, name, "audio", index, parent);
    let path = copy_fixture(host, &fixture::drums("drum-loop.wav"));
    let clip = place(host, track, &path, 0);
    (track, clip)
}

/// Warps `clip` in `mode` as if its file were at `bpm`, so the set's
/// 120 BPM stretches it.
fn warp(host: &mut Host, clip: u64, mode: &str, bpm: f64) {
    host.run_ok("clip.set_warp", json!({ "clip": clip, "warp": true }));
    host.run_ok("clip.set_segment_bpm", json!({ "clip": clip, "bpm": bpm }));
    host.run_ok("clip.set_warp_mode", json!({ "clip": clip, "mode": mode }));
    host.settle();
}

// ---------------------------------------------------------------------------
// The comparison
// ---------------------------------------------------------------------------

/// What a case plays and exports: `length` ticks from `start`, which last
/// `millis` at the case's tempo.
#[derive(Clone, Copy, Debug)]
struct Range {
    start: i64,
    length: i64,
    millis: u64,
}

impl Range {
    /// The first `bars` bars at 120 BPM.
    fn bars(bars: i64) -> Self {
        Self {
            start: 0,
            length: bars * BAR,
            millis: bars as u64 * 2_000,
        }
    }
}

/// A live capture against its export.
#[derive(Debug)]
pub(crate) struct Comparison {
    /// The export.
    pub(crate) offline: Audio,
    /// The live capture over the export's frames, from the frame the song
    /// started.
    pub(crate) live: Audio,
    /// The aligned residual at lag 0: negative infinity when the two are the
    /// same to the bit.
    pub(crate) residual_db: f64,
    /// The first frame where any channel differs, if any does.
    pub(crate) first_difference: Option<usize>,
}

impl Comparison {
    /// Compares a live capture with an export of the same range, frame for
    /// frame from the song's start.
    pub(crate) fn of(offline: Audio, live: &Audio) -> Self {
        let (offline, live) = overlap(&offline, live, 0);
        let residual_db = aligned_residual_db(&offline, &live, 0);
        let first_difference = (0..offline.frames()).find(|&frame| {
            offline
                .channels
                .iter()
                .zip(&live.channels)
                .any(|(a, b)| a[frame].to_bits() != b[frame].to_bits())
        });
        Self {
            offline,
            live,
            residual_db,
            first_difference,
        }
    }

    /// `Ok` when the two are the same to the bit; otherwise what differs.
    pub(crate) fn exact(&self) -> Result<(), String> {
        let Some(frame) = self.first_difference else {
            return Ok(());
        };
        let alignment = best_alignment(&self.offline, &self.live, 2_400, 8_192);
        Err(format!(
            "differs from frame {frame} ({:.3} s): residual {:.1} dB at lag 0, level {:+.2} dB \
             (offline {:.1} dBFS, live {:.1} dBFS), best lag {} at correlation {:.4}",
            frame as f64 / f64::from(RATE),
            self.residual_db,
            level_difference_db(&self.offline, &self.live),
            self.offline.rms_dbfs(),
            self.live.rms_dbfs(),
            alignment.lag,
            alignment.correlation,
        ))
    }
}

/// Plays `range` live, then exports it, and compares them. Both must sound,
/// or the comparison proves nothing.
fn play_and_export(host: &mut Host, range: Range) -> Comparison {
    host.settle();
    let live = host.play_exactly(range.start, range.millis + MARGIN_MS);
    let offline = host.export(range.start, range.length, &ExportSpec::default());
    let comparison = Comparison::of(offline, &live);
    if comparison.live.peak_dbfs() < -60.0 {
        // DIAG (T646, temporary)
        let state = host.shell_mut().audio_mut().transport_state();
        let stats = host.shell().audio().stats();
        let disk = host.shell().audio().disk_stats();
        eprintln!(
            "DIAG silent live: raw {} frames peak {:.1}, state {state:?}, stats {stats:?}, disk {disk:?}",
            live.frames(),
            live.peak_dbfs()
        );
        for k in 0..3 {
            let again = host.play_exactly(range.start, range.millis + MARGIN_MS);
            eprintln!(
                "DIAG replay {k}: {} frames peak {:.1}",
                again.frames(),
                again.peak_dbfs()
            );
        }
    }
    bl_e2e::audio::assert_sounds(&comparison.offline, "the export", -60.0);
    bl_e2e::audio::assert_sounds(&comparison.live, "the live capture", -60.0);
    comparison
}

/// Builds a case in a new host with `build`, compares live with offline over
/// `range`, and checks "live matches offline" for key `name`.
fn case(ratchet: &mut Ratchet, name: &str, range: Range, build: impl FnOnce(&mut Host)) {
    let mut host = Host::new();
    clear(&mut host);
    build(&mut host);
    host.check();
    let comparison = play_and_export(&mut host, range);
    eprintln!(
        "{name}: residual {:.1} dB, first difference {:?}",
        comparison.residual_db, comparison.first_difference
    );
    ratchet.check(name, MATCHES, comparison.exact());
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// The warp modes as `clip.set_warp_mode` names them.
const MODES: [&str; 6] = [
    "beats",
    "tones",
    "texture",
    "re_pitch",
    "complex",
    "complex_pro",
];

/// Warps the drum loop, or the melody, in `mode` from `bpm` to the set's 120
/// BPM: the case.
fn warped_case(ratchet: &mut Ratchet, material: &str, mode: &str, bpm: f64) {
    let name = format!("warp/{mode}/{material} {bpm} to 120 BPM");
    case(ratchet, &name, Range::bars(1), |host| {
        let clip = if material == "drum loop" {
            loop_track(host, "Loop", 0, None).1
        } else {
            let track = track(host, "Melody", "audio", 0, None);
            let path = fixture_file(host, "melody.wav", &melody(bpm));
            place(host, track, &path, 0)
        };
        warp(host, clip, mode, bpm);
        host.play_from(0, 50);
    });
}

// Spec: warp-modes R6. A mode's output never depends on the block size, so
// the live device's blocks and the export's give the same audio, the remix
// case included: a loop at 110 BPM stretched to the set's 120.
#[test]
fn each_warp_mode_plays_live_as_it_exports() {
    // DIAG (T646, temporary): three rounds, to catch a rare silent capture.
    for _ in 0..3 {
        let mut ratchet = Known::load().ratchet(SUITE);
        let tempos: &[f64] = if full_depth() {
            &[110.0, 90.0, 132.0]
        } else {
            &[110.0]
        };
        for &bpm in tempos {
            for mode in MODES {
                warped_case(&mut ratchet, "drum loop", mode, bpm);
                warped_case(&mut ratchet, "melody", mode, bpm);
            }
        }
        ratchet.finish();
    }
}

// Spec: warp-modes R8. A change to a playing clip's mode crossfades from
// the old one over 20 ms; a clip that isn't playing has nothing to fade
// from, so the first start after a change made while stopped plays the new
// mode from its first frame, as the export does. Complex Pro at the clip's
// own tempo isn't neutral (R21), so a fade from Beats shows.
#[test]
fn a_warp_mode_set_while_stopped_starts_clean() {
    let mut ratchet = Known::load().ratchet(SUITE);
    let mut host = Host::new();
    clear(&mut host);
    let (_, clip) = loop_track(&mut host, "Loop", 0, None);
    host.run_ok("clip.set_warp", json!({ "clip": clip, "warp": true }));
    // The engine takes the clip in its first mode before the change, so the
    // change is one the engine sees, not one folded into its first program.
    host.settle();
    host.run_ok(
        "clip.set_warp_mode",
        json!({ "clip": clip, "mode": "complex_pro" }),
    );
    let comparison = play_and_export(&mut host, Range::bars(1));
    let outcome = comparison.exact();
    eprintln!("mode set while stopped: {outcome:?}");
    ratchet.check("warp mode set while stopped/complex_pro", MATCHES, outcome);
    // Once that first start has passed, a second plays as the export does.
    let again = play_and_export(&mut host, Range::bars(1));
    assert_eq!(again.exact(), Ok(()), "the second start");
    ratchet.finish();
}

#[test]
fn instruments_play_live_as_they_export() {
    let mut ratchet = Known::load().ratchet(SUITE);
    case(&mut ratchet, "midi/cascade", Range::bars(2), |host| {
        synth_track(host, "Keys", 0, None);
    });
    case(&mut ratchet, "midi/snapshot", Range::bars(2), |host| {
        let track = track(host, "Drums", "midi", 0, None);
        let snapshot = insert(host, track, 0, SAMPLER, "instrument");
        host.run_ok(
            "device.load_sample",
            json!({ "device": snapshot,
                    "path": copy_fixture(host, &fixture::drums("snare.wav")) }),
        );
        let hits = (0..16i64)
            .map(|i| {
                (
                    60 + (i % 5) as u8,
                    i * BEAT / 2,
                    SIXTEENTH,
                    30 + 6 * i as u8,
                )
            })
            .collect();
        midi_clip(host, track, "Hits", 2 * BAR, hits);
    });
    ratchet.finish();
}

/// Every native MIDI effect's type ID and name, from the device registry.
fn midi_effects() -> Vec<(String, String)> {
    bl_devices::device_types()
        .into_iter()
        .filter(|t| t.role == DeviceRole::MidiEffect)
        .filter_map(|t| match t.kind {
            DeviceKind::Native { device_type } => Some((device_type, t.name)),
            _ => None,
        })
        .collect()
}

// Spec: export R14. Each MIDI effect before Cascade plays live as the export
// renders it: its notes land on the same frames at any block size.
#[test]
fn every_midi_effect_plays_live_as_it_exports() {
    let mut ratchet = Known::load().ratchet(SUITE);
    let effects = midi_effects();
    assert!(
        effects.len() > 5,
        "the registry's MIDI effects: {effects:?}"
    );
    for (device_type, name) in effects {
        case(
            &mut ratchet,
            &format!("midi effect/{name}"),
            Range::bars(2),
            |host| {
                let (track, _) = synth_track(host, "Keys", 0, None);
                insert(host, track, 0, &device_type, "midi_effect");
            },
        );
    }
    ratchet.finish();
}

// Spec: arpeggiator R20 and export R14. The arpeggiator's steps go out on
// their frames whatever the block size: a pulse of eight quarter notes, each
// half a beat long, arpeggiated by the default settings, plays live as it
// exports. The bar-one notes agree and the bar-two notes don't, so the case
// records where the two part.
#[test]
fn the_arpeggiator_steps_on_the_same_frames_live_and_offline() {
    let mut ratchet = Known::load().ratchet(SUITE);
    let mut host = Host::new();
    clear(&mut host);
    let track = track(&mut host, "Pulse", "midi", 0, None);
    insert(&mut host, track, 0, ARPEGGIATOR, "midi_effect");
    insert(&mut host, track, 1, POLYSYNTH, "instrument");
    let pulse = (0..8i64)
        .map(|beat| (36, beat * BEAT, BEAT / 2, 40 + 11 * beat as u8))
        .collect();
    midi_clip(&mut host, track, "Pulse notes", 2 * BAR, pulse);
    let comparison = play_and_export(&mut host, Range::bars(2));
    // The first bar is the same to the bit; what follows tells where the
    // step timing parts.
    let bar = 96_000;
    let first_bar = Comparison::of(comparison.offline.slice(0, bar), &comparison.live);
    eprintln!(
        "arpeggiator: whole {:?}; first bar {:?}",
        comparison.exact(),
        first_bar.exact()
    );
    ratchet.check(
        "arpeggiator pulse/eight quarter notes",
        MATCHES,
        comparison.exact(),
    );
    ratchet.finish();
}

#[test]
fn returns_sends_and_a_sidechain_play_live_as_they_export() {
    let mut ratchet = Known::load().ratchet(SUITE);
    case(
        &mut ratchet,
        "returns, sends and a sidechain",
        Range::bars(2),
        |host| {
            let (loop_track, _) = loop_track(host, "Loop", 0, None);
            let (keys, _) = synth_track(host, "Keys", 1, None);
            let verb = track(host, "Verb", "return", 0, None);
            let echo = track(host, "Echo", "return", 1, None);
            insert(host, verb, 0, REVERB, "audio_effect");
            insert(host, echo, 0, DELAY, "audio_effect");
            for (from, to, level) in [
                (loop_track, echo, 0.5),
                (keys, verb, 0.4),
                (keys, echo, 0.2),
            ] {
                host.run_ok(
                    "mixer.set_send",
                    json!({ "track": from, "to": to, "level": level }),
                );
            }
            // The compressor as the browser adds it, with a sidechain input,
            // listening to the loop and pressing the keys down hard.
            let added = host.run_ok(
                "device.add",
                json!({ "track": keys, "index": 1,
                        "device": { "kind": { "native": { "device_type": COMPRESSOR } },
                                    "name": "Compressor", "role": "audio_effect",
                                    "sidechain": true } }),
            );
            let compressor = added["device"].as_u64().expect("the compressor's ID");
            host.run_ok(
                "device.set_sidechain_input",
                json!({ "device": compressor,
                        "input": { "type": "track", "track": loop_track, "tap": "post_fx" } }),
            );
            host.run_ok(
                "device.set_sidechain_enabled",
                json!({ "device": compressor, "enabled": true }),
            );
            set_param(host, compressor, COMPRESSOR_THRESHOLD.0, -40.0);
        },
    );
    ratchet.finish();
}

#[test]
fn tempo_automation_plays_live_as_it_exports() {
    let mut ratchet = Known::load().ratchet(SUITE);
    // 120 BPM rising to 150 over two bars: the two bars last about 3.6 s.
    let range = Range {
        start: 0,
        length: 2 * BAR,
        millis: 4_000,
    };
    case(&mut ratchet, "tempo automation", range, |host| {
        synth_track(host, "Keys", 0, None);
        let (_, clip) = loop_track(host, "Loop", 1, None);
        host.run_ok("clip.set_warp", json!({ "clip": clip, "warp": true }));
        host.run_ok(
            "clip.set_loop_enabled",
            json!({ "clip": clip, "enabled": true }),
        );
        host.batch_ok(
            "Tempo",
            vec![
                json!({ "command": "song.set_tempo_breakpoint", "args": { "at": 0, "bpm": 120.0 } }),
                json!({ "command": "song.set_tempo_breakpoint",
                        "args": { "at": 2 * BAR, "bpm": 150.0 } }),
            ],
        );
    });
    ratchet.finish();
}

#[test]
fn a_group_plays_live_as_it_exports() {
    let mut ratchet = Known::load().ratchet(SUITE);
    case(&mut ratchet, "group", Range::bars(2), |host| {
        let band = track(host, "Band", "group", 0, None);
        loop_track(host, "Loop", 0, Some(band));
        synth_track(host, "Keys", 1, Some(band));
        insert(host, band, 0, SATURATOR, "audio_effect");
        let filter = insert(host, band, 1, AUTO_FILTER, "audio_effect");
        set_param(host, filter, FILTER_FREQ.0, 2_500.0);
        host.run_ok("mixer.set_volume", json!({ "track": band, "volume": 0.6 }));
        host.run_ok("mixer.set_pan", json!({ "track": band, "pan": -0.3 }));
    });
    ratchet.finish();
}

#[test]
fn clip_envelopes_play_live_as_they_export() {
    let mut ratchet = Known::load().ratchet(SUITE);
    case(&mut ratchet, "clip envelopes", Range::bars(2), |host| {
        let (keys, midi) = synth_track(host, "Keys", 0, None);
        let filter = insert(host, keys, 1, AUTO_FILTER, "audio_effect");
        let (_, audio) = loop_track(host, "Loop", 1, None);
        let breakpoint = |clip: u64, target: Value, at: i64, value: f64| {
            json!({ "command": "clip.set_envelope_breakpoint",
                    "args": { "clip": clip, "target": target, "at": at, "value": value } })
        };
        let volume = json!({ "type": "mixer", "param": "volume" });
        let pan = json!({ "type": "mixer", "param": "pan" });
        let cutoff = json!({ "type": "device", "device": filter, "param": FILTER_FREQ.0 });
        host.batch_ok(
            "Envelopes",
            vec![
                breakpoint(midi, pan.clone(), 0, -0.8),
                breakpoint(midi, pan, 2 * BAR, 0.8),
                breakpoint(midi, cutoff.clone(), 0, -0.6),
                breakpoint(midi, cutoff, BAR, 0.4),
                breakpoint(audio, volume.clone(), 0, 0.1),
                breakpoint(audio, volume, BAR, 1.0),
            ],
        );
    });
    ratchet.finish();
}

#[test]
fn a_wave_modulator_plays_live_as_it_exports() {
    let mut ratchet = Known::load().ratchet(SUITE);
    case(&mut ratchet, "modulator/wave", Range::bars(2), |host| {
        let (keys, _) = synth_track(host, "Keys", 0, None);
        let filter = insert(host, keys, 1, AUTO_FILTER, "audio_effect");
        let wave = insert(host, keys, 2, WAVE_MOD, "audio_effect");
        host.run_ok(
            "modulator.map",
            json!({ "device": wave, "slot": 0,
                    "target": { "type": "device", "device": filter, "param": FILTER_FREQ.0 } }),
        );
    });
    ratchet.finish();
}

// Spec: device-framework S9 and export R14. A tempo-synced delay on a return
// follows a tempo ramp the same at any block size: the drum loop sent to
// Reprise while the tempo rises from 120 BPM to 128 over four bars.
#[test]
fn a_synced_delay_under_a_tempo_ramp_plays_live_as_it_exports() {
    let mut ratchet = Known::load().ratchet(SUITE);
    case(
        &mut ratchet,
        "tempo-synced delay under a tempo ramp",
        Range::bars(1),
        |host| {
            let (loop_track, _) = loop_track(host, "Loop", 0, None);
            let echo = track(host, "Echo", "return", 0, None);
            insert(host, echo, 0, DELAY, "audio_effect");
            host.run_ok(
                "mixer.set_send",
                json!({ "track": loop_track, "to": echo, "level": 0.2 }),
            );
            host.batch_ok(
                "Tempo",
                vec![
                    json!({ "command": "song.set_tempo_breakpoint",
                            "args": { "at": 0, "bpm": 120.0 } }),
                    json!({ "command": "song.set_tempo_breakpoint",
                            "args": { "at": 4 * BAR, "bpm": 128.0 } }),
                ],
            );
        },
    );
    ratchet.finish();
}

// The rich set, every family of device and the mix at once, with its
// Arpeggiator switched off (the Arpeggiator's own case lists its
// difference). Its tempo rises from 120 BPM towards 128 at bar 5, so its
// Reprise on Echo differs as the synced-delay case does.
#[test]
fn the_rich_set_plays_live_as_it_exports() {
    let mut ratchet = Known::load().ratchet(SUITE);
    let ranges = if full_depth() {
        vec![
            ("rich set without the arpeggiator", Range::bars(2)),
            (
                "rich set without the arpeggiator, from bar 2",
                Range {
                    start: BAR,
                    length: BAR,
                    millis: 2_000,
                },
            ),
        ]
    } else {
        vec![("rich set without the arpeggiator", Range::bars(2))]
    };
    for (name, range) in ranges {
        let mut host = Host::new();
        let ids = fixture::build(&mut host);
        host.run_ok(
            "device.set_enabled",
            json!({ "device": ids.device(ARPEGGIATOR), "enabled": false }),
        );
        let comparison = play_and_export(&mut host, range);
        eprintln!("{name}: {:?}", comparison.exact());
        ratchet.check(name, MATCHES, comparison.exact());
    }
    ratchet.finish();
}
