//! The M2 exit demo's song half, rehearsed headless (T108).
//!
//! The build plan's demo is a three-minute song made from scratch in the
//! app: a MIDI keyboard, the sampler and the synth, a third-party CLAP, VST3
//! and (on macOS) Audio Unit plug-in, a recorded vocal, filter automation,
//! and sidechain compression through a send; exported, reopened identically,
//! and listened to. This test makes the same song through the agent API on a
//! headless shell, so CI proves every step before a person sits down to it.
//!
//! - The keyboard is the test MIDI backend: its notes enter at the input
//!   stage while Arrangement Record runs, not through `note.add`.
//! - The vocal is the voice-like phrase of `tests/fixtures/gen/gen_song.py`,
//!   played into the null device's loopback and recorded from its input, so
//!   the recording latency compensation puts it back on its frames.
//! - One filter sweep is drawn by command; the other is recorded with
//!   Automation Arm while a take runs.
//! - Sidechain compression, two ways: a compressor on the bass track keyed from
//!   the kick, as the manual's dance music example has it, and the pad sending
//!   to a return whose compressor is keyed from the kick, as the build plan
//!   words it. Both listen to the kick through the group its track sits in (a
//!   MIDI track is not one of a sidechain's sources).
//! - The song is exported, saved, closed and reopened, and exported again.
//!
//! At `BL_TEST_DEPTH=full` the song is three minutes long; at quick depth it
//! is its first 16 bars.

bl_rt::install_trap!();

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_audio_io::{BackendKind, NullBackend, NullOptions};
use bl_core::time::Ticks;
use bl_devices::compressor::{ATTACK, COMPRESSOR, KNEE, RATIO, RELEASE, THRESHOLD};
use bl_devices::polysynth::{FREQ, POLYSYNTH};
use bl_devices::sampler::{MODE, SAMPLER, SLICE_BY, SLICE_REGIONS};
use bl_midi_io::{Direction, TestBackend};
use bl_model::{
    AutomationTarget, Clip, ClipData, DeviceId, MidiData, Note, ParamId, TrackId, TrackKind,
};
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings, EngineTransport};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-cli-m2-song-{name}-{}-{nanos}",
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
/// A 4/4 bar and a beat, in ticks.
const BAR: i64 = 2_822_400;
const BEAT: i64 = BAR / 4;
/// The song's tempo is 120 BPM, so a bar is two seconds.
const BAR_FRAMES: i64 = 96_000;

/// The song frame a position in ticks falls on.
fn frame(ticks: i64) -> i64 {
    (ticks * BAR_FRAMES + BAR / 2) / BAR
}

/// The folder of the generated audio the song plays.
fn song_audio(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/song")
        .join(name)
}

fn note(key: u8, start: i64, length: i64, velocity: u8) -> Note {
    Note::new(key, Ticks(start), Ticks(length), velocity)
}

/// The kick: four on the floor.
fn kick_notes(bars: i64) -> Vec<Note> {
    (0..bars * 4)
        .map(|beat| note(60, beat * BEAT, BEAT / 2, 110))
        .collect()
}

/// The kit's four slices play from C3: a snare on two and four, hats on the
/// off-beats, a clap on the last beat of every second bar. It comes in on
/// bar 3.
fn perc_notes(bars: i64) -> Vec<Note> {
    let mut notes = Vec::new();
    for bar in 2..bars {
        let start = bar * BAR;
        for beat in [1, 3] {
            notes.push(note(60, start + beat * BEAT, BEAT / 4, 100));
        }
        for beat in 0..4 {
            notes.push(note(61, start + beat * BEAT + BEAT / 2, BEAT / 4, 80));
        }
        if bar % 2 == 1 {
            notes.push(note(63, start + 3 * BEAT, BEAT / 4, 90));
        }
    }
    notes
}

/// A legato bassline of quarter notes on the roots of Am, F, C and G, so
/// there is always a bass under the kick.
fn bass_notes(bars: i64) -> Vec<Note> {
    const ROOTS: [u8; 4] = [33, 29, 36, 31];
    let eighth = BEAT / 2;
    let mut notes = Vec::new();
    for bar in 0..bars {
        for at in [0, 2, 4, 6] {
            notes.push(note(
                ROOTS[bar as usize % 4],
                bar * BAR + at * eighth,
                eighth * 2,
                100,
            ));
        }
    }
    notes
}

/// The chords, a bar each.
fn pad_notes(bars: i64) -> Vec<Note> {
    const CHORDS: [[u8; 3]; 4] = [[57, 60, 64], [53, 57, 60], [60, 64, 67], [55, 59, 62]];
    let mut notes = Vec::new();
    for bar in 0..bars {
        for key in CHORDS[bar as usize % 4] {
            notes.push(note(key, bar * BAR, BAR, 90));
        }
    }
    notes
}

/// What the keyboard plays: E and G, a quarter note each from the second
/// beat of the take.
fn keyboard_key(index: usize) -> u8 {
    if index.is_multiple_of(2) { 64 } else { 67 }
}

/// A MIDI clip for `clip.create`, its notes counted from its start.
fn midi_clip(name: &str, start: i64, length: i64, notes: Vec<Note>) -> Value {
    json!({
        "id": { "$new": format!("{name} clip") },
        "name": name,
        "start": start,
        "length": length,
        "mute": false,
        "data": serde_json::to_value(ClipData::Midi(MidiData::new(notes))).unwrap(),
    })
}

/// Where the song's takes are, in bars (counted from 0).
const KEYS_BAR: i64 = 4;
const VOCAL_BAR: i64 = 12;

/// The song's session: the agent API over a shell whose engine plays on a
/// four-channel null device with a loopback, the keyboard's test port, and
/// the folder it all lives in.
struct Song {
    client: Client<InProcess>,
    keyboard: TestBackend,
    folder: PathBuf,
}

impl Song {
    fn new(folder: &Path) -> Self {
        // A device that reports 400 frames of input latency and 600 of
        // output latency, and loops its output back to its input 1,480
        // frames later; Driver Error Compensation covers the other 480.
        let (backend, _control) = NullBackend::with_devices(
            NullOptions {
                channels: 4,
                ..NullOptions::default()
            },
            &[("null", "Null")],
        );
        let backend = backend.with_latency(400, 600).with_loopback(1_480);
        let audio = AudioHost::with_backends(
            vec![BackendKind::Null],
            Box::new(move |_| Ok(Box::new(backend.clone()))),
            AudioSettings::default(),
        );
        let mut shell = Shell::new(
            Preferences::default(),
            None,
            audio,
            Box::new(ProjectFiles::new()),
        );
        let keyboard = TestBackend::new();
        keyboard.add_port(Direction::Input, "Test Keys");
        shell.use_midi_backend(Box::new(keyboard.clone()));
        shell
            .run("audio.configure", json!({ "input_device": "null:null" }))
            .unwrap();
        shell.start_audio();
        assert!(shell.audio().engine_running(), "{:?}", shell.audio());
        // The app's agent API, so transport commands reach the engine; the
        // person allows the agent and every command that asks first.
        let mut client = Client::new(InProcess::new(Agent::for_app(), shell));
        client.transport().answer = Box::new(|_| true);
        client.initialize("m2-song").unwrap();
        Self {
            client,
            keyboard,
            folder: folder.to_owned(),
        }
    }

    fn shell(&mut self) -> &mut Shell {
        &mut self.client.transport().shell
    }

    /// What the app's event loop does after every event: the agent's
    /// transport and scheduled commands go to the engine, its reports come
    /// back, and the shell polls the engine.
    fn poll(&mut self) -> EngineTransport {
        let host = self.client.transport();
        host.agent.sync_engine(&mut host.shell);
        while host.agent.resume(&mut host.shell).is_some() {}
        host.shell.poll_engine()
    }

    /// Runs `commands` as one undo step the agent names `label`.
    fn run_all(&mut self, label: &str, commands: Vec<Value>) -> Value {
        let reply = self
            .client
            .call("run", json!({ "label": label, "commands": commands }))
            .expect("the call reaches the agent")
            .ok()
            .unwrap_or_else(|error| panic!("{label} failed: {error}"));
        self.poll();
        reply
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
        self.poll();
        reply["results"][0]["result"].clone()
    }

    /// The track called `name`.
    fn track(&mut self, name: &str) -> TrackId {
        self.shell()
            .editor()
            .document()
            .tracks()
            .find(|track| track.name() == name)
            .map(bl_model::Track::id)
            .unwrap_or_else(|| panic!("no track called {name}"))
    }

    fn clips(&mut self, track: TrackId) -> Vec<Clip> {
        let mut clips: Vec<Clip> = self
            .shell()
            .editor()
            .document()
            .track(track)
            .expect("the track")
            .clips()
            .cloned()
            .collect();
        clips.sort_by_key(|clip| clip.start);
        clips
    }

    /// Adds a track at the end of its list.
    fn create_track(&mut self, name: &str, kind: &str) -> TrackId {
        let index = {
            let document = self.shell().editor().document();
            if kind == "return" {
                document.returns().len()
            } else {
                document.top_level().len()
            }
        };
        self.run(
            "track.create",
            json!({ "id": { "$new": name }, "kind": kind, "name": name, "index": index }),
        );
        self.track(name)
    }

    /// Adds a MIDI or audio track inside a group of its own, `<name> Group`,
    /// and returns the track and its group. A track alone can't be exported
    /// or used as a sidechain's source if it is a MIDI track; its group can.
    fn create_grouped(&mut self, name: &str, kind: &str) -> (TrackId, TrackId) {
        let group = self.create_track(&format!("{name} Group"), "group");
        self.run(
            "track.create",
            json!({ "id": { "$new": name }, "kind": kind, "name": name, "index": 0,
                    "parent": group }),
        );
        (self.track(name), group)
    }

    /// Adds a native device to `track`.
    fn add_device(
        &mut self,
        track: TrackId,
        device_type: &str,
        name: &str,
        role: &str,
        sidechain: bool,
    ) -> DeviceId {
        let added = self.run(
            "device.add",
            json!({
                "track": track,
                "device": { "kind": { "native": { "device_type": device_type } },
                            "name": name, "role": role, "sidechain": sidechain },
            }),
        );
        serde_json::from_value(added["device"].clone()).expect("the new device")
    }

    /// Sets a compressor to turn its track down hard while `source` plays
    /// loud, with its sidechain listening to `source` after its devices.
    fn duck_under(&mut self, device: DeviceId, source: TrackId) {
        for (param, value) in [
            (THRESHOLD, DUCK_THRESHOLD),
            (RATIO, 20.0),
            (ATTACK, 0.5),
            (RELEASE, 120.0),
            (KNEE, 0.0),
        ] {
            self.set_param(device, param, value);
        }
        self.run(
            "device.set_sidechain_input",
            json!({ "device": device,
                    "input": { "type": "track", "track": source, "tap": "post_fx" } }),
        );
        self.run(
            "device.set_sidechain_enabled",
            json!({ "device": device, "enabled": true }),
        );
    }

    fn set_param(&mut self, device: DeviceId, param: ParamId, value: f64) {
        self.run(
            "device.set_param",
            json!({ "device": device, "param": param, "value": value }),
        );
    }

    /// Loads a plug-in on `track`, and returns its device.
    fn load_plugin(&mut self, path: &Path, track: TrackId) -> DeviceId {
        let loaded = self.run("plugin.load", json!({ "path": path, "track": track }));
        serde_json::from_value(loaded["device"].clone()).expect("the new device")
    }

    /// Polls the engine as the app's loop does until the playhead reaches
    /// `at`, and returns where it was.
    fn wait_for(&mut self, at: i64) -> i64 {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let position = self.poll().position.get();
            if position >= at {
                return position;
            }
            assert!(Instant::now() < until, "the playhead never reached {at}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Polls the engine until `done` holds of the shell.
    fn wait_until(&mut self, what: &str, done: impl Fn(&Shell) -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            self.poll();
            if done(self.shell()) {
                return;
            }
            assert!(Instant::now() < until, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Lets the engine run for `millis`, polling as the app does.
    fn pass_time(&mut self, millis: u64) {
        let until = Instant::now() + Duration::from_millis(millis);
        while Instant::now() < until {
            self.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Exports the range from `start` for `length` ticks as a 32-bit WAV
    /// file, of Main or one track, and returns its channels.
    fn export(
        &mut self,
        name: &str,
        track: Option<TrackId>,
        start: i64,
        length: i64,
    ) -> Vec<Vec<f32>> {
        let path = self.folder.join(name);
        let mut args = json!({
            "path": path, "bit_depth": 32, "sample_rate": RATE,
            "start": start, "length": length, "replace": true,
        });
        if let Some(track) = track {
            args["rendered_track"] = json!({ "track": track });
        }
        self.run("export.audio", args);
        read_wav(&path)
    }
}

/// The channels of a WAV file.
fn read_wav(path: &Path) -> Vec<Vec<f32>> {
    let audio = bl_media::decode_file(path).expect("the export decodes");
    (0..audio.channel_count())
        .map(|channel| audio.channel(channel).unwrap().to_vec())
        .collect()
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
    assert!(total > 0.0, "the sweep is silent");
    bands
        .iter()
        .zip(&weights)
        .map(|(band, weight)| band.centre * weight)
        .sum::<f64>()
        / total
}

fn rms(samples: &[f32]) -> f64 {
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len().max(1) as f64).sqrt()
}

/// What the keyboard's take left in the set: the notes the test sent, each
/// with the playhead before and after it went out, and the filter moves.
struct KeysTake {
    clip: Clip,
    /// The note-ons: key, velocity, and the playhead (in ticks) before and
    /// after the message was sent.
    ons: Vec<(u8, u8, i64, i64)>,
    /// The note-offs, likewise.
    offs: Vec<(u8, i64, i64)>,
    /// The cutoff moves: value and the playhead before and after.
    cuts: Vec<(f64, i64, i64)>,
    synth: DeviceId,
    /// The group the keyboard's track is in.
    group: TrackId,
}

/// The level in dB above which the kick turns the return down.
const DUCK_THRESHOLD: f64 = -36.0;

/// The cutoffs the take sweeps through, in hertz, one at the start of each
/// two beats from the second beat.
const CUTS: [f64; 4] = [400.0, 1_200.0, 3_000.0, 7_000.0];

impl Song {
    /// Saves the set into the song's folder, which recording needs, clears
    /// the factory set's tracks, and sets the compensation that makes up
    /// the loopback's delay.
    fn start(&mut self) {
        let set = self.folder.join("M2 Song.blset");
        self.run("set.save_as", json!({ "path": set }));
        self.run(
            "audio.configure",
            json!({ "driver_error_compensation_ms": 10.0 }),
        );
        self.run(
            "settings.set_create_fades_on_clip_edges",
            json!({ "on": false }),
        );
        let tempo = self.shell().editor().document().settings().tempo.clone();
        assert_eq!(
            tempo.bpm_at(Ticks::ZERO).get(),
            120.0,
            "the factory set's tempo"
        );
        let defaults: Vec<TrackId> = self
            .shell()
            .editor()
            .document()
            .tracks()
            .filter(|track| track.kind() != TrackKind::Main)
            .map(bl_model::Track::id)
            .collect();
        let deletes = defaults
            .iter()
            .map(|track| json!({ "command": "track.delete", "args": { "track": track } }))
            .collect();
        self.run_all("Clear the factory tracks", deletes);
    }

    /// Plays the keyboard's part while Arrangement Record runs, with
    /// Automation Arm moving the synth's filter, and returns what came out.
    fn record_keys(&mut self) -> KeysTake {
        let (keys, group) = self.create_grouped("Keys", "midi");
        let synth = self.add_device(keys, POLYSYNTH, "Drift", "instrument", false);
        self.run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Test Keys", "track": true }),
        );
        self.run(
            "routing.set_input",
            json!({ "track": keys, "input": { "type": "port", "name": "Test Keys" } }),
        );
        self.run("mixer.set_arm", json!({ "track": keys, "arm": true }));
        self.run("arrangement.automation_arm", json!({ "on": true }));
        self.pass_time(300);

        // One take of two bars from bar 5. The keyboard plays a quarter
        // note on each beat from the second, and the cutoff moves on the
        // second, fourth, sixth and eighth beats.
        let start = KEYS_BAR * BAR;
        self.run("transport.locate", json!({ "position": start }));
        self.run("transport.set_recording", json!({ "recording": true }));
        self.run("transport.play", json!({}));
        let mut take = Vec::new();
        for index in 0..7 {
            let on = start + (index as i64 + 1) * BEAT;
            take.push((on, Action::On(keyboard_key(index))));
            take.push((on + BEAT * 3 / 4, Action::Off(keyboard_key(index))));
        }
        for (index, cut) in CUTS.iter().enumerate() {
            take.push((start + (index as i64 * 2 + 1) * BEAT, Action::Cut(*cut)));
        }
        // A move goes in before the note that starts on its beat.
        take.sort_by_key(|(at, action)| (*at, !matches!(action, Action::Cut(_))));
        let (mut ons, mut offs, mut cuts) = (Vec::new(), Vec::new(), Vec::new());
        for (at, action) in take {
            let before = self.wait_for(at);
            match action {
                Action::On(key) => {
                    assert!(
                        self.keyboard
                            .send("Test Keys", Instant::now(), &[0x90, key, 100])
                    );
                    let after = self.poll().position.get();
                    ons.push((key, 100, before, after));
                }
                Action::Off(key) => {
                    assert!(
                        self.keyboard
                            .send("Test Keys", Instant::now(), &[0x80, key, 64])
                    );
                    let after = self.poll().position.get();
                    offs.push((key, before, after));
                }
                Action::Cut(value) => {
                    // A person's drag reaches the shell directly, which is
                    // where Automation Arm notes the move; an agent's edit
                    // is an undo step of its own and isn't recorded.
                    self.shell()
                        .run(
                            "device.set_param",
                            json!({ "device": synth, "param": FREQ, "value": value }),
                        )
                        .unwrap();
                    let after = self.poll().position.get();
                    cuts.push((value, before, after));
                }
            }
        }
        self.wait_for(start + 8 * BEAT);
        self.run("transport.stop", json!({}));
        self.run("arrangement.automation_arm", json!({ "on": false }));
        self.wait_until("the keyboard's clip and the filter's envelope", |shell| {
            let document = shell.editor().document();
            document.track(keys).is_some_and(|track| {
                track.clips().len() == 1
                    && track
                        .automation_for(&AutomationTarget::Device {
                            device: synth,
                            param: FREQ,
                        })
                        .is_some()
            })
        });
        KeysTake {
            clip: self.clips(keys).remove(0),
            ons,
            offs,
            cuts,
            synth,
            group,
        }
    }
}

impl Song {
    /// Plays the voice phrase twice from bar 13 through the loopback, and
    /// records it on an armed audio track from the device's input. Returns
    /// the recorded clip, the recorded file's samples, and Main's samples
    /// over the same two bars, rendered offline before the take.
    fn record_vocal(&mut self) -> (Clip, Vec<f32>, Vec<f32>) {
        let singer = self.create_track("Singer", "audio");
        let vocal = self.create_track("Vocal", "audio");
        let voice = song_audio("voice.wav");
        for bar in [VOCAL_BAR, VOCAL_BAR + 1] {
            self.run(
                "browser.add_to_arrangement",
                json!({ "paths": [voice], "track": singer, "at": bar * BAR }),
            );
        }
        self.run(
            "routing.set_input",
            json!({ "track": vocal, "input": { "type": "external",
                                               "channel": { "type": "mono", "channel": 1 } } }),
        );
        self.run("mixer.set_arm", json!({ "track": vocal, "arm": true }));
        // It doesn't hear itself: the loopback would feed it back.
        self.run(
            "routing.set_monitor",
            json!({ "track": vocal, "monitor": "off" }),
        );

        // What Main plays over those two bars, offline.
        let reference = self.export("reference.wav", None, VOCAL_BAR * BAR, 2 * BAR);
        // Let the clips' streams cue before the take starts.
        self.pass_time(300);
        self.run("transport.locate", json!({ "position": VOCAL_BAR * BAR }));
        self.run("transport.set_recording", json!({ "recording": true }));
        self.run("transport.play", json!({}));
        self.wait_for((VOCAL_BAR + 2) * BAR);
        self.run("transport.stop", json!({}));
        self.wait_until("the vocal's clip", |shell| {
            shell
                .editor()
                .document()
                .track(vocal)
                .is_some_and(|track| track.clips().len() == 1)
        });
        let clip = self.clips(vocal).remove(0);
        let recorded = self
            .folder
            .join("M2 Song Project")
            .join("Samples")
            .join("Recorded");
        let files: Vec<PathBuf> = std::fs::read_dir(&recorded)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|e| e == "wav"))
            .collect();
        assert_eq!(
            files.len(),
            1,
            "one file for the one armed audio track: {files:?}"
        );
        let file = read_wav(&files[0]).remove(0);
        // The singer has done its part.
        self.run("track.delete", json!({ "track": singer }));
        (clip, file, reference.into_iter().next().unwrap())
    }
}

/// The tracks and devices of the finished song that the checks reach.
struct Parts {
    /// The groups the bass and the keyboard's track sit in: what the sweeps
    /// export, since a MIDI track can't be exported alone.
    bass: TrackId,
    keys: TrackId,
    /// Every track that makes a sound of its own, as it can be exported:
    /// MIDI tracks only through their groups.
    sounding: Vec<TrackId>,
    /// The return the pad sends to, and the compressors on it and on the
    /// bass track.
    duck: TrackId,
    squeeze: DeviceId,
    squeeze_bass: DeviceId,
    riff_clap: TrackId,
    riff_vst3: TrackId,
    clap: DeviceId,
    vst3: DeviceId,
}

impl Song {
    /// Builds the rest of the song around the two takes, `bars` bars long,
    /// by command: the sampler's drums, the synths, the plug-in parts, the
    /// return the pad and the bass send to, the drawn filter sweep, and the
    /// takes copied on through the song.
    fn arrange(&mut self, bars: i64, keys: &KeysTake, vocal: &Clip) -> Parts {
        let end = bars * BAR;
        // A MIDI track isn't one of the sidechain's sources, so the kick sits
        // in a group, whose sound the compressor listens to.
        let (kick, kick_group) = self.create_grouped("Kick", "midi");
        let (perc, perc_group) = self.create_grouped("Perc", "midi");
        let (bass, bass_group) = self.create_grouped("Bass", "midi");
        let (pad, pad_group) = self.create_grouped("Pad", "midi");
        let riff_clap = self.create_track("Riff CLAP", "audio");
        let riff_vst3 = self.create_track("Riff VST3", "audio");
        let duck = self.create_track("Duck", "return");
        let keys_track = self.track("Keys");
        let vocal_track = self.track("Vocal");

        // The drums are the sampler: the kick on its own track as a
        // one-shot, and the kit's snare, hats and clap as four slices of
        // one file.
        let snapshot = self.add_device(kick, SAMPLER, "Simpler", "instrument", false);
        self.run(
            "device.load_sample",
            json!({ "device": snapshot, "path": song_audio("kick.wav") }),
        );
        self.set_param(snapshot, MODE, 1.0);
        let kit = self.add_device(perc, SAMPLER, "Simpler", "instrument", false);
        self.run(
            "device.load_sample",
            json!({ "device": kit, "path": song_audio("kit.wav") }),
        );
        self.set_param(kit, MODE, 2.0);
        self.set_param(kit, SLICE_BY, 2.0);
        self.set_param(kit, SLICE_REGIONS, 4.0);
        let bass_synth = self.add_device(bass, POLYSYNTH, "Drift", "instrument", false);
        self.add_device(pad, POLYSYNTH, "Drift", "instrument", false);

        // Two compressors duck under the kick, each listening to the kick's
        // group. The bass's sits on the bass track, as the manual's dance
        // music example has it (29.9.2.2); the pad's sits on a return the pad
        // sends to.
        let squeeze_bass = self.add_device(bass, COMPRESSOR, "Compressor", "audio_effect", true);
        self.duck_under(squeeze_bass, kick_group);
        let squeeze = self.add_device(duck, COMPRESSOR, "Compressor", "audio_effect", true);
        self.duck_under(squeeze, kick_group);
        let mut mix = Vec::new();
        for (track, volume) in [
            (kick, 0.35),
            (perc, 0.15),
            (bass, 0.08),
            (pad, 0.08),
            (keys_track, 0.15),
            (riff_clap, 0.2),
            (riff_vst3, 0.2),
            (vocal_track, 0.4),
        ] {
            mix.push(json!({ "command": "mixer.set_volume",
                             "args": { "track": track, "volume": volume } }));
        }
        mix.push(json!({ "command": "mixer.set_send",
                         "args": { "track": pad, "to": duck, "level": 0.5 } }));
        self.run_all("Mix", mix);

        // The audio riff: one file, one bar a clip, from bar 9 on the
        // CLAP track and from bar 13 on the VST3 track.
        let pluck = song_audio("pluck.wav");
        self.run(
            "browser.add_to_arrangement",
            json!({ "paths": [pluck], "track": riff_clap, "at": 8 * BAR }),
        );
        self.run(
            "browser.add_to_arrangement",
            json!({ "paths": [pluck], "track": riff_vst3, "at": 12 * BAR }),
        );
        let riff_data = |song: &mut Self, track: TrackId| {
            serde_json::to_value(&song.clips(track)[0].data).unwrap()
        };
        let clap_data = riff_data(self, riff_clap);
        let vst3_data = riff_data(self, riff_vst3);

        let mut commands = Vec::new();
        let mut add = |track: TrackId, clip: Value| {
            commands.push(json!({ "command": "clip.create",
                                  "args": { "track": track, "clip": clip } }));
        };
        add(kick, midi_clip("Kick", 0, end, kick_notes(bars)));
        add(perc, midi_clip("Perc", 0, end, perc_notes(bars)));
        add(bass, midi_clip("Bass", 0, end, bass_notes(bars)));
        add(pad, midi_clip("Pad", 0, end, pad_notes(bars)));
        for bar in 9..bars {
            let audio = |name: &str, data: &Value| {
                json!({ "id": { "$new": format!("{name} {bar}") }, "name": name,
                        "start": bar * BAR, "length": BAR, "mute": false, "data": data })
            };
            add(riff_clap, audio("Riff CLAP", &clap_data));
            if bar >= 13 {
                add(riff_vst3, audio("Riff VST3", &vst3_data));
            }
        }
        // The takes carry on: the keyboard's two bars every eight, the
        // vocal's every sixteen.
        let ClipData::Midi(played) = &keys.clip.data else {
            panic!("the keyboard's take is a MIDI clip");
        };
        let notes: Vec<Note> = played.notes.iter().cloned().collect();
        for bar in (KEYS_BAR + 8..bars - 1).step_by(8) {
            add(
                keys_track,
                midi_clip(&format!("Keys {bar}"), bar * BAR, 2 * BAR, notes.clone()),
            );
        }
        for bar in (VOCAL_BAR + 16..bars - 1).step_by(16) {
            add(
                vocal_track,
                json!({ "id": { "$new": format!("Vocal {bar}") }, "name": "Vocal",
                        "start": bar * BAR, "length": vocal.length.get(), "mute": false,
                        "data": serde_json::to_value(&vocal.data).unwrap() }),
            );
        }
        // The bass's filter opens over two bars and closes over two.
        let mut sweep = Vec::new();
        for bar in (0..=bars).step_by(2) {
            let value = if bar % 4 == 0 { 250.0 } else { 5_000.0 };
            sweep.push(json!({ "command": "automation.set_breakpoint",
                               "args": { "track": bass,
                                         "target": { "type": "device", "device": bass_synth,
                                                     "param": FREQ },
                                         "at": bar * BAR, "value": value } }));
        }
        commands.extend(sweep);
        self.run_all("Arrange the song", commands);

        // The plug-in parts.
        let clap = self.load_plugin(bl_test_clap::bundle(), riff_clap);
        let vst3 = self.load_plugin(bl_test_vst3::bundle(), riff_vst3);
        Parts {
            bass: bass_group,
            keys: keys.group,
            sounding: vec![
                kick_group,
                perc_group,
                bass_group,
                pad_group,
                keys.group,
                riff_clap,
                riff_vst3,
                vocal_track,
                duck,
            ],
            duck,
            squeeze,
            squeeze_bass,
            riff_clap,
            riff_vst3,
            clap,
            vst3,
        }
    }
}

impl Song {
    /// Measures how far a compressor turns its track down. `source` is a
    /// track or group that can be exported alone and carries the
    /// compressor's signal. The sound the compressor ducks is exported with
    /// it switched off. The ducking itself is read from Main instead: when
    /// the kick's group is soloed out, as an export of one track does, the
    /// children it mixes are silenced and the key with them. Main with the
    /// threshold as set, less Main with the threshold at 0 dB (same latency,
    /// nothing compressed), is the change in the track's signal. Returns the
    /// signal without the compressor, and with it, for the first four bars.
    fn duck_renders(&mut self, device: DeviceId, source: TrackId) -> (Vec<f32>, Vec<f32>) {
        self.run(
            "device.set_enabled",
            json!({ "device": device, "enabled": false }),
        );
        let plain = self.export("duck open.wav", Some(source), 0, 4 * BAR);
        self.run(
            "device.set_enabled",
            json!({ "device": device, "enabled": true }),
        );
        let ducked = self.export("main keyed.wav", None, 0, 4 * BAR);
        self.set_param(device, THRESHOLD, 0.0);
        let level = self.export("main level.wav", None, 0, 4 * BAR);
        self.set_param(device, THRESHOLD, DUCK_THRESHOLD);
        let plain = plain.into_iter().next().unwrap();
        let with: Vec<f32> = plain
            .iter()
            .zip(&ducked[0])
            .zip(&level[0])
            .map(|((r, a), b)| r + (a - b))
            .collect();
        (plain, with)
    }
}

/// One thing the take does.
#[derive(Clone, Copy)]
enum Action {
    On(u8),
    Off(u8),
    Cut(f64),
}

/// The most a message the test sends can land after the playhead the test
/// read after it: a few blocks, in ticks.
const SLACK: i64 = 2_048 * BAR / BAR_FRAMES;

impl Song {
    /// Checks what the keyboard's take left in the set: each note on its
    /// frame, the clip over the take, and the filter's moves where they
    /// were made.
    fn check_keys(&mut self, take: &KeysTake, keys: TrackId) {
        // Spec: recording R6, R7. The clip covers the whole take, from bar 5
        // for two bars and no more than a block over.
        let start = KEYS_BAR * BAR;
        assert_eq!(take.clip.start.get(), start);
        let over = take.clip.length.get() - 2 * BAR;
        assert!((0..=SLACK).contains(&over), "the take is {over} ticks over");
        let ClipData::Midi(data) = &take.clip.data else {
            panic!("the keyboard's take is a MIDI clip");
        };
        let mut played: Vec<Note> = data.notes.iter().cloned().collect();
        played.sort_by_key(|note| note.start);
        assert_eq!(
            played.len(),
            take.ons.len(),
            "one note for each key pressed"
        );
        // Spec: midi-io R12, R15. A note lands on the frame its message was
        // stamped for: between the playhead the test read before sending it
        // and the one it read after, to within the block the engine places
        // it in.
        for (index, note) in played.iter().enumerate() {
            let (key, velocity, before, after) = take.ons[index];
            let (off_key, off_before, off_after) = take.offs[index];
            assert_eq!((note.key, note.velocity, off_key), (key, velocity, key));
            let at = start + note.start.get();
            assert!(
                (before..=after + SLACK).contains(&at),
                "note {index} landed at {at}, sent between {before} and {after}"
            );
            let length = note.length.get();
            assert!(
                (off_before - after - SLACK..=off_after - before + SLACK).contains(&length),
                "note {index} lasts {length}, released between {off_before} and {off_after}"
            );
        }
        // Spec: automation R17. The filter's moves are in its envelope at
        // the playhead they were made at, in one undo step.
        let document = self.shell().editor().document();
        let envelope = document
            .track(keys)
            .unwrap()
            .automation_for(&AutomationTarget::Device {
                device: take.synth,
                param: FREQ,
            })
            .expect("the cutoff's envelope")
            .envelope
            .clone();
        let points: Vec<_> = envelope.points().collect();
        assert_eq!(points.len(), take.cuts.len());
        for (point, (value, before, after)) in points.iter().zip(&take.cuts) {
            assert_eq!(point.value, *value);
            assert!(
                (*before..=*after).contains(&point.at.get()),
                "a move made between {before} and {after} is at {:?}",
                point.at
            );
        }
        let history: Vec<&str> = self.shell().editor().undo_history().collect();
        assert_eq!(
            history
                .iter()
                .filter(|label| **label == "Record Automation")
                .count(),
            1
        );
        assert_eq!(
            history
                .iter()
                .filter(|label| **label == "Recorded Clip")
                .count(),
            1
        );
    }

    /// Checks the recorded vocal against Main's offline render of the same
    /// two bars: the loopback carried exactly that, so with the device's
    /// latency and the compensation added up to the loop's delay, every
    /// frame of the file is the render's frame.
    fn check_vocal(&mut self, clip: &Clip, file: &[f32], reference: &[f32]) {
        // Spec: recording R4, R10.
        assert_eq!(clip.start.get(), VOCAL_BAR * BAR);
        assert!(
            reference.iter().any(|sample| sample.abs() > 0.5),
            "the render has the voice in it"
        );
        let delay = 1_480;
        assert!(
            file.len() as i64 > 2 * BAR_FRAMES - delay - 2_048 && file.len() <= reference.len(),
            "the file holds the take less the delay: {} frames",
            file.len()
        );
        let wrong = (0..file.len()).find(|&i| file[i].to_bits() != reference[i].to_bits());
        assert_eq!(wrong, None, "the first frame the recording gets wrong");
        let tempo = self.shell().editor().document().settings().tempo.clone();
        let rate = bl_core::time::SampleRate::new(RATE).unwrap();
        let length = tempo.ticks_to_frames(clip.start + clip.length, rate).get()
            - tempo.ticks_to_frames(clip.start, rate).get();
        assert!(
            (length - file.len() as i64).abs() <= 2,
            "the clip holds the file: {length} frames against {}",
            file.len()
        );
    }

    /// Checks that both filter sweeps show in the render: the drawn one on
    /// the bass over four bars (open for two, closing for two), and the
    /// recorded one on the keyboard's synth, a step to each move.
    fn check_sweeps(&mut self, parts: &Parts, take: &KeysTake) {
        // Spec: automation R3. Playback follows the drawn envelope: over
        // four bars the filter opens for two and closes for two.
        let bass = self
            .export("bass.wav", Some(parts.bass), 0, 4 * BAR)
            .remove(0);
        let halves: Vec<f64> = (0..8)
            .map(|half| centroid(&bass[half * 48_000..(half + 1) * 48_000]))
            .collect();
        assert!(
            halves[0] < halves[1] && halves[1] < halves[2] && halves[2] < halves[3],
            "the filter opens over two bars: {halves:.0?}"
        );
        assert!(
            halves[4] > halves[5] && halves[5] > halves[6] && halves[6] > halves[7],
            "and closes over the next two: {halves:.0?}"
        );
        assert!(
            halves[3] > halves[0] * 1.4 && halves[4] > halves[7] * 1.4,
            "the sweep is wide: {halves:.0?}"
        );
        // Spec: automation R17. Playback follows the recorded envelope, the
        // synth's cutoff stepping between the points made in the take.
        let start = KEYS_BAR * BAR;
        let played = self
            .export("keys.wav", Some(parts.keys), start, 2 * BAR)
            .remove(0);
        let mut edges: Vec<usize> = take
            .cuts
            .iter()
            .map(|(_, at, _)| frame(*at - start) as usize)
            .collect();
        edges.push(frame(8 * BEAT) as usize);
        let steps: Vec<f64> = edges
            .windows(2)
            .map(|pair| centroid(&played[pair[0]..pair[1]]))
            .collect();
        assert!(
            steps.windows(2).all(|pair| pair[1] > pair[0] * 1.05),
            "the filter opens at each move: {steps:.0?}"
        );
        assert!(steps[3] > steps[0] * 1.5, "{steps:.0?}");
    }

    /// Checks one sidechain: the signal the compressor works on turns down
    /// at each kick, and has come back up before the next. `what` names it.
    fn check_duck(&mut self, what: &str, device: DeviceId, source: TrackId) {
        // Spec: routing R24, compressor R12.
        let (plain, ducked) = self.duck_renders(device, source);
        let db = |a: &[f32], b: &[f32]| 20.0 * (rms(a) / rms(b)).log10();
        let mut measured = 0;
        for beat in 1..16i64 {
            let kick = beat * BEAT_FRAMES;
            let after = (kick + 240) as usize..(kick + 2_160) as usize;
            let before = (kick - 2_400) as usize..(kick - 480) as usize;
            // Where the track is silent there is nothing to turn down.
            if rms(&plain[after.clone()]) < 1e-3 || rms(&plain[before.clone()]) < 1e-3 {
                continue;
            }
            measured += 1;
            let reduction = db(&ducked[after.clone()], &plain[after]);
            let recovered = db(&ducked[before.clone()], &plain[before]);
            assert!(
                reduction < -12.0,
                "{what} is {reduction:.1} dB at the kick on beat {beat}"
            );
            assert!(
                recovered > -3.0,
                "and {recovered:.1} dB before the next kick on beat {beat}"
            );
        }
        assert!(
            measured >= 12,
            "{what}: only {measured} kicks had a sound under them"
        );
    }
}

/// A beat in frames.
const BEAT_FRAMES: i64 = BAR_FRAMES / 4;

/// The plug-ins the song uses besides the native devices (and, on macOS,
/// Apple's AUDelay).
enum Plugins {
    /// The test CLAP and VST3 plug-ins, which stand in for third-party
    /// ones.
    Test,
    /// The free plug-ins a person has installed on this machine.
    Local(Local),
}

/// Where the free plug-ins are, from the scanner.
struct Local {
    surge: PathBuf,
    surge_id: String,
    dexed: PathBuf,
    dexed_id: String,
    airwindows: PathBuf,
    airwindows_id: String,
}

impl Song {
    /// Looks for Surge XT (CLAP), Dexed (VST3) and an Airwindows effect with
    /// the scanner, and says which are missing.
    fn find_local(&mut self) -> Result<Local, String> {
        // VST3 is scanned only with its Plug-In Sources switch on (the
        // plug-in scanning spec, R12), and Dexed is wanted as a VST3.
        self.run("plugin.settings", json!({ "vst3_system_folders": true }));
        let scanned = self.run("plugin.scan", json!({}));
        let found: Vec<Value> = scanned["plugins"].as_array().cloned().unwrap_or_default();
        // A plug-in is Airwindows's if its name or its vendor says so.
        let find = |format: &str, name: &str, role: Option<&str>| -> Option<(PathBuf, String)> {
            found.iter().find_map(|plugin| {
                let says = |field: &str| {
                    plugin[field]
                        .as_str()
                        .is_some_and(|text| text.to_lowercase().contains(name))
                };
                let matches = plugin["format"] == format
                    && (says("name") || says("vendor"))
                    && role.is_none_or(|role| plugin["role"] == role);
                matches.then(|| {
                    (
                        PathBuf::from(plugin["path"].as_str().unwrap()),
                        plugin["plugin_id"].as_str().unwrap().to_owned(),
                    )
                })
            })
        };
        let surge = find("clap", "surge xt", None);
        let dexed = find("vst3", "dexed", None);
        let airwindows = find("clap", "airwindows", Some("audio_effect"))
            .or_else(|| find("vst3", "airwindows", Some("audio_effect")));
        let mut missing = Vec::new();
        for (what, plugin) in [
            ("Surge XT (CLAP)", &surge),
            ("Dexed (VST3)", &dexed),
            ("an Airwindows effect (CLAP or VST3)", &airwindows),
        ] {
            if plugin.is_none() {
                missing.push(what);
            }
        }
        if !missing.is_empty() {
            return Err(format!("not installed: {}", missing.join(", ")));
        }
        let (surge, surge_id) = surge.unwrap();
        let (dexed, dexed_id) = dexed.unwrap();
        let (airwindows, airwindows_id) = airwindows.unwrap();
        Ok(Local {
            surge,
            surge_id,
            dexed,
            dexed_id,
            airwindows,
            airwindows_id,
        })
    }

    /// Adds the free plug-ins to the song: Surge XT plays the pad's chords,
    /// Dexed plays the bass line an octave up, and an Airwindows effect
    /// goes on the VST3 riff.
    fn local_parts(&mut self, local: &Local, bars: i64, parts: &Parts) {
        for (name, path, id, notes) in [
            ("Surge XT", &local.surge, &local.surge_id, pad_notes(bars)),
            ("Dexed", &local.dexed, &local.dexed_id, bass_notes(bars)),
        ] {
            let track = self.create_track(name, "midi");
            self.run(
                "plugin.load",
                json!({ "path": path, "plugin": id, "track": track }),
            );
            self.run_all(
                name,
                vec![json!({ "command": "clip.create",
                             "args": { "track": track,
                                       "clip": midi_clip(name, 0, bars * BAR, notes) } })],
            );
        }
        self.run(
            "plugin.load",
            json!({ "path": local.airwindows, "plugin": local.airwindows_id,
                    "track": parts.riff_vst3 }),
        );
    }

    /// `plugin.info` for `device`, once its plug-in has started.
    fn plugin_info(&mut self, device: DeviceId) -> Value {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            self.poll();
            let reply = self
                .client
                .call(
                    "run",
                    json!({ "commands": [{ "command": "plugin.info",
                                           "args": { "device": device } }] }),
                )
                .expect("the call reaches the agent");
            if let Ok(info) = reply.ok() {
                return info["results"][0]["result"].clone();
            }
            assert!(Instant::now() < until, "the plug-in never started");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Sets a plug-in's parameter and polls until the plug-in has it: the
    /// change reaches the live plug-in a moment after the set, and an export
    /// renders through that plug-in.
    fn set_plugin_param(&mut self, device: DeviceId, param: ParamId, value: f64) {
        self.set_param(device, param, value);
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            self.poll();
            let info = self.plugin_info(device);
            if info["values"]
                .as_array()
                .is_some_and(|values| values.contains(&json!([param, value])))
            {
                return;
            }
            assert!(Instant::now() < until, "the plug-in never took {value}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Checks that a CLAP plug-in's device is in the render: the track's
    /// signal with it, against its signal with the device switched off,
    /// scaled by the gain the plug-in is set to. (Exports render CLAP
    /// devices only; a VST3 or Audio Unit device passes its input through
    /// in one.)
    fn check_clap_in_export(&mut self, track: TrackId, device: DeviceId, param: u32, bar: i64) {
        // Spec: offline-plugin-renders R1.
        self.set_plugin_param(device, ParamId(param), 0.5);
        let through = self
            .export("through.wav", Some(track), bar * BAR, BAR)
            .remove(0);
        self.run(
            "device.set_enabled",
            json!({ "device": device, "enabled": false }),
        );
        let past = self
            .export("past.wav", Some(track), bar * BAR, BAR)
            .remove(0);
        self.run(
            "device.set_enabled",
            json!({ "device": device, "enabled": true }),
        );
        self.set_plugin_param(device, ParamId(param), 1.0);
        assert!(past.iter().any(|sample| *sample != 0.0), "the riff plays");
        for (n, (out, input)) in through.iter().zip(&past).enumerate() {
            assert_eq!(out.to_bits(), (input * 0.5).to_bits(), "frame {n}");
        }
    }

    /// The loudest peak the track's meter shows while the song plays from
    /// `bar` for `millis`.
    fn playing_peak(&mut self, track: TrackId, bar: i64, millis: u64) -> f32 {
        self.run("transport.locate", json!({ "position": bar * BAR }));
        self.run("transport.play", json!({}));
        let mut peak = 0.0f32;
        let until = Instant::now() + Duration::from_millis(millis);
        while Instant::now() < until {
            self.poll();
            if let Some(meter) = self.shell().meters().iter().find(|m| m.track == track) {
                peak = peak.max(meter.peak[0]);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        self.run("transport.stop", json!({}));
        self.pass_time(100);
        peak
    }

    /// Checks that the VST3 plug-in plays in the engine: the riff's meter
    /// halves with the plug-in's gain, and the plug-in runs without a
    /// failure.
    fn check_vst3_plays(&mut self, track: TrackId, device: DeviceId) {
        // Spec: vst3-hosting R1.
        let info = self.plugin_info(device);
        assert_eq!(info["active"], true, "{info}");
        assert_eq!(info["failure"], Value::Null, "{info}");
        self.set_plugin_param(device, ParamId(bl_test_vst3::GAIN), 0.5);
        self.pass_time(300);
        let half = self.playing_peak(track, 12, 2_500);
        self.set_plugin_param(device, ParamId(bl_test_vst3::GAIN), 1.0);
        self.pass_time(300);
        let full = self.playing_peak(track, 12, 2_500);
        assert!(full > 0.05, "the riff plays: {full}");
        let ratio = f64::from(half / full);
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "the plug-in's gain halves the riff: {half} against {full}"
        );
    }

    /// Exports every sounding track over bars 13 to 16, one at a time, and
    /// checks that none is silent: every part plays there.
    fn check_every_track_sounds(&mut self, parts: &Parts) {
        for (index, track) in parts.sounding.clone().into_iter().enumerate() {
            let name = self
                .shell()
                .editor()
                .document()
                .track(track)
                .unwrap()
                .name()
                .to_owned();
            let samples = self.export(
                &format!("track {index}.wav"),
                Some(track),
                12 * BAR,
                4 * BAR,
            );
            let peak = samples
                .iter()
                .flatten()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            assert!(peak > 0.01, "{name} is silent over bars 13 to 16");
        }
    }

    /// The document as the set file holds it, to compare.
    fn document(&mut self) -> Value {
        serde_json::to_value(self.shell().editor().document()).unwrap()
    }

    /// Saves, closes, and reopens the set; the reopened set is the saved
    /// one, and a second export of it nulls against `first` to within
    /// `tolerance_db` (`None` for bit for bit).
    fn export_save_and_reopen(&mut self, bars: i64, tolerance_db: Option<f64>) {
        let first = self.export("song first.wav", None, 0, bars * BAR);
        // The listening check: `BL_M2_SONG_OUT=/some/file.wav` keeps this
        // render, which the temporary folder would otherwise take with it.
        if let Ok(keep) = std::env::var("BL_M2_SONG_OUT") {
            std::fs::copy(self.folder.join("song first.wav"), keep).expect("the render is kept");
        }
        assert!(
            first.iter().flatten().any(|sample| sample.abs() > 0.1),
            "the song is not silent"
        );
        self.run("set.save", json!({}));
        let saved = self.document();
        let set = self.folder.join("M2 Song Project").join("M2 Song.blset");
        let (project, _) = bl_project::load(&set).expect("the saved set loads");
        assert_eq!(
            serde_json::to_value(&project.document).unwrap(),
            saved,
            "the file holds the document"
        );
        self.run("set.new", json!({}));
        assert_ne!(self.document(), saved, "closed");
        self.run("set.open", json!({ "path": set }));
        assert_eq!(self.document(), saved, "the reopened set is the saved one");
        let second = self.export("song second.wav", None, 0, bars * BAR);
        assert_eq!(first.len(), second.len());
        let mut worst = 0.0f64;
        let mut differing = 0usize;
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.len(), b.len());
            for (x, y) in a.iter().zip(b) {
                if x.to_bits() != y.to_bits() {
                    differing += 1;
                    worst = worst.max(f64::from((x - y).abs()));
                }
            }
        }
        match tolerance_db {
            None => assert_eq!(differing, 0, "the second export nulls; worst {worst:e}"),
            Some(db) => assert!(
                worst <= 10f64.powf(db / 20.0),
                "{differing} samples differ, by up to {worst:e}"
            ),
        }
    }
}

/// Makes the song with `plugins`, and checks every step.
fn rehearse(plugins: &Plugins, song: &mut Song) {
    let bars = if full_depth() { 90 } else { 16 };
    song.start();
    let take = song.record_keys();
    let keys = song.track("Keys");
    song.check_keys(&take, keys);
    let (vocal, file, reference) = song.record_vocal();
    song.check_vocal(&vocal, &file, &reference);

    // Neither take allocated on the audio thread.
    let stats = song.shell().audio().stats().expect("a stream is open");
    assert!(stats.blocks > 0);
    assert_eq!(stats.heap_operations, 0, "{stats:?}");

    let parts = song.arrange(bars, &take, &vocal);
    song.check_clap_in_export(parts.riff_clap, parts.clap, bl_test_clap::GAIN, 8);
    song.check_vst3_plays(parts.riff_vst3, parts.vst3);
    let tolerance = match plugins {
        Plugins::Test => None,
        // Third-party plug-ins may add noise of their own, or keep state
        // between renders; the second export has to null to -80 dBFS.
        Plugins::Local(local) => {
            song.local_parts(local, bars, &parts);
            Some(-80.0)
        }
    };
    #[cfg(target_os = "macos")]
    {
        // Apple's AUDelay, on the VST3 riff's track after the plug-in.
        let loaded = song.run(
            "plugin.load_au",
            json!({ "type": "aufx", "subtype": "dely", "manufacturer": "appl",
                    "track": parts.riff_vst3 }),
        );
        let device = serde_json::from_value(loaded["device"].clone()).unwrap();
        let info = song.plugin_info(device);
        assert_eq!(info["active"], true, "{info}");
        assert_eq!(info["failure"], Value::Null, "{info}");
    }
    song.check_sweeps(&parts, &take);
    song.check_duck("the bass", parts.squeeze_bass, parts.bass);
    song.check_duck("the pad's return", parts.squeeze, parts.duck);
    song.check_every_track_sounds(&parts);
    song.export_save_and_reopen(bars, tolerance);
}

#[test]
fn the_song_is_played_recorded_arranged_exported_and_reopened() {
    let dir = TempDir::new("song");
    let mut song = Song::new(&dir.0);
    rehearse(&Plugins::Test, &mut song);
}

/// The same song with Surge XT, Dexed and an Airwindows effect in place of
/// the test plug-ins' company. It runs where they are installed; where they
/// aren't it says what's missing and passes.
#[test]
fn the_song_with_the_free_plugins_a_person_has_installed() {
    let dir = TempDir::new("local");
    let mut song = Song::new(&dir.0);
    match song.find_local() {
        Ok(local) => rehearse(&Plugins::Local(local), &mut song),
        Err(missing) => eprintln!("skipped: {missing}; install them to run the local variant"),
    }
}
