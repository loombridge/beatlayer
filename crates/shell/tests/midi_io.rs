//! MIDI input in the shell (WP-2.2, `docs/specs/midi-io.md`): a port's
//! switches decide whether its notes reach tracks, a MIDI track's MIDI From
//! and channel pick what it takes, the computer MIDI keyboard plays the
//! right notes, and the switches persist. Everything runs against the
//! in-process test backend and a real engine processor driven by hand.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bl_core::clock::TestClock;
use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
use bl_core::time::SampleRate;
use bl_devices::sampler::SAMPLER;
use bl_engine::{Engine, EngineConfig, MediaLibrary, NoInput, OpenedSource, Processor, Registry};
use bl_media::{BitDepth, Dither, SampleData};
use bl_midi_io::{Direction, TestBackend};
use bl_model::{DeviceKind, DeviceRole, Input, TrackId};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::routing::{self, InOut};
use bl_shell::settings::Preferences;
use bl_shell::testing;
use rtrb::Consumer;
use serde_json::{Value, json};

const BLOCK: usize = 256;

/// One block's length, the step the test clock advances by.
fn block() -> Duration {
    Duration::from_nanos(BLOCK as u64 * 1_000_000_000 / 48_000)
}

/// A shell with two input ports and an output, a MIDI track, and an engine
/// processor whose queue is the shell's.
struct Rig {
    shell: Shell,
    test: TestBackend,
    track: TrackId,
    engine: Engine,
    processor: Processor,
    /// The test clock the messages are stamped on and each block starts on,
    /// so a note lands on its frame however busy the machine is (T199).
    clock: TestClock,
}

impl Rig {
    fn new() -> Self {
        Self::with_registry(Registry::new())
    }

    fn with_registry(registry: Registry) -> Self {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Keys A");
        test.add_port(Direction::Input, "Keys B");
        test.add_port(Direction::Output, "Synth");
        let mut shell = Shell::headless();
        shell.use_midi_backend(Box::new(test.clone()));
        let clock = TestClock::new();
        shell.use_test_clock(clock.clone());
        let track = shell.editor().document().top_level().next().unwrap();
        let (engine, mut processor) =
            Engine::start(EngineConfig::new(SampleRate::HZ_48000, BLOCK), registry).unwrap();
        processor.set_midi_input(shell.midi().sender().attach(1024));
        processor.set_midi_output(shell.midi().output().attach(1024));
        let mut rig = Self {
            shell,
            test,
            track,
            engine,
            processor,
            clock,
        };
        rig.sync();
        rig
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    /// Sends the set to the engine, as the shell does after an edit.
    fn sync(&mut self) {
        self.engine.sync(self.shell.editor().document());
        assert!(self.engine.flush(Duration::from_secs(5)));
    }

    fn input(&mut self, input: Value) {
        self.run(
            "routing.set_input",
            json!({ "track": self.track, "input": input }),
        );
        self.sync();
    }

    fn port(&mut self, name: &str, switches: Value) -> Value {
        let mut args = json!({ "direction": "input", "name": name });
        args.as_object_mut()
            .unwrap()
            .extend(switches.as_object().unwrap().clone());
        self.run("midi.set_port", args)
    }

    /// Plays a message from a port, and processes the block it arrives in
    /// and the one after, where it lands.
    fn play(&mut self, port: &str, message: impl AsRef<[u8]>) -> usize {
        self.test.send(port, self.clock.now(), message.as_ref());
        self.blocks_run(2)
    }

    /// Plays a message from a port and runs blocks until the engine has taken
    /// it: after a pause the block clock has drifted from the clock the
    /// message is stamped on, and takes a few blocks to catch up.
    fn play_settled(&mut self, port: &str, message: impl AsRef<[u8]>) {
        let before = self.processor.midi_counters().received;
        self.test.send(port, self.clock.now(), message.as_ref());
        for _ in 0..400 {
            self.blocks_run(1);
            if self.processor.midi_counters().received > before {
                // One more block, where the message lands.
                self.blocks_run(1);
                return;
            }
        }
        panic!("the engine never took the message");
    }

    /// Runs `count` blocks, each starting after the last on the test clock,
    /// and returns how many messages the track's input received over them.
    fn blocks_run(&mut self, count: u32) -> usize {
        let mut received = 0;
        for _ in 0..count {
            let now = self.clock.now();
            self.processor.process_at(BLOCK, &NoInput, now);
            self.clock.advance(block());
            received += self
                .processor
                .midi_input(self.track)
                .map_or(0, |input| input.events.len());
        }
        received
    }
}

fn notes(queue: &mut Consumer<MidiInputEvent>) -> Vec<(u8, u8, u8)> {
    std::iter::from_fn(|| queue.pop().ok())
        .map(|e| {
            assert_eq!(e.source, MidiSource::ComputerKeyboard);
            (e.message.status(), e.message.data1(), e.message.data2())
        })
        .collect()
}

// Spec: midi-io R5, R4
#[test]
fn switching_a_ports_track_off_stops_its_notes_reaching_tracks() {
    let mut rig = Rig::new();
    assert_eq!(rig.play("Keys A", [0x90, 60, 100]), 1, "Track starts on");

    // Sync keeps the port open, so its notes arrive and are turned away.
    rig.port("Keys A", json!({ "track": false, "sync": true }));
    assert!(rig.test.is_open(Direction::Input, "Keys A"));
    assert_eq!(rig.play("Keys A", [0x90, 62, 100]), 0);
    let ports = rig.run("midi.ports", json!({}));
    let keys_a = &ports["ports"][0];
    assert_eq!(keys_a["name"], "Keys A");
    assert_eq!(keys_a["track"], false);

    rig.port("Keys A", json!({ "track": true }));
    assert_eq!(rig.play("Keys A", [0x90, 64, 100]), 1);

    // All switches off closes the port.
    rig.port("Keys A", json!({ "track": false, "sync": false }));
    assert!(!rig.test.is_open(Direction::Input, "Keys A"));
}

// Spec: remote-mapping R7, R8. A message a mapping takes never reaches an
// armed track's input, though the port's Track switch is on; an unmapped one
// on the same port does.
#[test]
fn a_mapped_message_never_reaches_an_armed_tracks_input_and_an_unmapped_one_does() {
    let mut rig = Rig::new();
    let track = rig.track;
    rig.run("mixer.set_arm", json!({ "track": track, "arm": true }));
    rig.sync();
    rig.port("Keys A", json!({ "remote": true }));
    rig.run(
        "remote.map",
        json!({
            "source": { "type": "cc", "channel": 1, "controller": 20 },
            "target": { "type": "mixer", "track": track, "param": "volume" },
        }),
    );
    assert_eq!(
        rig.play("Keys A", [0xB0, 20, 127]),
        0,
        "the mapped controller"
    );
    rig.shell.poll();
    let volume = rig
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .mixer()
        .volume;
    assert!(volume > 1.9, "it moved the volume: {volume}");
    assert_eq!(rig.play("Keys A", [0xB0, 21, 127]), 1, "an unmapped one");
    assert_eq!(rig.play("Keys A", [0x90, 60, 100]), 1, "and a note");
}

// Spec: midi-io R7, R8, R9
#[test]
fn midi_from_one_port_ignores_another_and_a_channel_ignores_the_rest() {
    let mut rig = Rig::new();
    rig.input(json!({ "type": "port", "name": "Keys A" }));
    assert_eq!(rig.play("Keys B", [0x90, 60, 100]), 0);
    assert_eq!(rig.play("Keys A", [0x90, 60, 100]), 1);

    rig.run(
        "routing.set_input",
        json!({ "track": rig.track, "input": { "type": "port", "name": "Keys A" },
                "channel": 2 }),
    );
    rig.sync();
    assert_eq!(
        rig.play("Keys A", [0x90, 60, 100]),
        0,
        "channel 1 is turned away"
    );
    assert_eq!(rig.play("Keys A", [0x91, 60, 100]), 1, "channel 2 is taken");

    // All Ins takes both ports again.
    rig.input(json!({ "type": "all_ins" }));
    assert_eq!(rig.play("Keys B", [0x90, 60, 100]), 1);
}

// Spec: midi-io R7, R23
#[test]
fn the_computer_keyboard_reaches_tracks_set_to_it_or_all_ins_only() {
    let mut rig = Rig::new();
    rig.run("midi.toggle_computer_keyboard", json!({ "on": true }));
    let press = |rig: &mut Rig| {
        rig.run(
            "midi.computer_keyboard_key",
            json!({ "key": "A", "pressed": true }),
        );
        let received = rig.blocks_run(2);
        rig.run(
            "midi.computer_keyboard_key",
            json!({ "key": "A", "pressed": false }),
        );
        received + rig.blocks_run(2)
    };
    // The note-on and the note-off.
    assert_eq!(press(&mut rig), 2, "All Ins takes the keyboard");
    rig.input(json!({ "type": "computer_keyboard" }));
    assert_eq!(press(&mut rig), 2);
    rig.input(json!({ "type": "port", "name": "Keys A" }));
    assert_eq!(press(&mut rig), 0, "a port's track doesn't");
    rig.input(json!({ "type": "none" }));
    assert_eq!(press(&mut rig), 0);
}

// Spec: midi-io R18, R19, R21
#[test]
fn the_keyboards_keys_play_the_right_pitches_and_octave_keys_shift_by_twelve() {
    let mut shell = Shell::headless();
    let mut queue = shell.midi().sender().attach(256);
    let press = |shell: &mut Shell, key: &str| {
        let on = shell
            .run(
                "midi.computer_keyboard_key",
                json!({ "key": key, "pressed": true }),
            )
            .unwrap();
        shell
            .run(
                "midi.computer_keyboard_key",
                json!({ "key": key, "pressed": false }),
            )
            .unwrap();
        on
    };

    // Off: a press is refused, and nothing is sent.
    let refused = shell.run(
        "midi.computer_keyboard_key",
        json!({ "key": "A", "pressed": true }),
    );
    assert!(refused.is_err());
    assert!(notes(&mut queue).is_empty());

    shell
        .run("midi.toggle_computer_keyboard", json!({ "on": true }))
        .unwrap();
    // The home row plays white keys and the row above black keys, from C3.
    for (i, key) in "AWSEDFTGYHUJKOLP".chars().enumerate() {
        let result = press(&mut shell, &key.to_string());
        assert_eq!(result["note"], 60 + i as u64, "{key}");
    }
    let played: Vec<_> = notes(&mut queue);
    assert_eq!(played.len(), 32, "a note-on and a note-off for each");
    assert_eq!(played[0], (0x90, 60, 100), "channel 1, velocity 100");
    assert_eq!(played[1], (0x80, 60, 64));
    assert_eq!(played[30], (0x90, 75, 100));

    // X moves up an octave, Z down; five octaves either way, then it stops.
    let octave = |shell: &mut Shell, steps: i32| {
        shell
            .run("midi.computer_keyboard_octave", json!({ "steps": steps }))
            .unwrap()
    };
    assert_eq!(octave(&mut shell, 1)["lowest_note"], 72);
    assert_eq!(press(&mut shell, "A")["note"], 72);
    assert_eq!(octave(&mut shell, 16)["lowest_note"], 120, "C8 at the top");
    assert_eq!(octave(&mut shell, 1)["octave"], 5);
    // Only the keys whose note passes 127 play nothing.
    assert_eq!(press(&mut shell, "A")["note"], 120);
    assert_eq!(press(&mut shell, "K")["sent"], false, "128 is out of range");
    assert_eq!(
        octave(&mut shell, -16)["lowest_note"],
        0,
        "C-2 at the bottom"
    );
    assert_eq!(octave(&mut shell, -1)["octave"], -5);
    assert_eq!(press(&mut shell, "A")["note"], 0);
    assert_eq!(octave(&mut shell, 5)["lowest_note"], 60);
    notes(&mut queue);

    // C and V step the velocity through 1, 20, 40, 60, 80, 100, 120, 127.
    let velocity = |shell: &mut Shell, steps: i32| {
        shell
            .run("midi.computer_keyboard_velocity", json!({ "steps": steps }))
            .unwrap()["velocity"]
            .as_u64()
            .unwrap()
    };
    let up: Vec<u64> = (0..3).map(|_| velocity(&mut shell, 1)).collect();
    assert_eq!(up, [120, 127, 127]);
    let down: Vec<u64> = (0..8).map(|_| velocity(&mut shell, -1)).collect();
    assert_eq!(down, [120, 100, 80, 60, 40, 20, 1, 1]);
    press(&mut shell, "A");
    assert_eq!(notes(&mut queue)[0], (0x90, 60, 1));
}

// Spec: midi-io R21
#[test]
fn a_note_ends_the_note_it_started_and_switching_the_keyboard_off_ends_every_note() {
    let mut shell = Shell::headless();
    let mut queue = shell.midi().sender().attach(64);
    let key = |shell: &mut Shell, key: &str, pressed: bool| {
        shell
            .run(
                "midi.computer_keyboard_key",
                json!({ "key": key, "pressed": pressed }),
            )
            .unwrap()
    };
    shell
        .run("midi.toggle_computer_keyboard", json!({ "on": true }))
        .unwrap();
    key(&mut shell, "A", true);
    // Holding it doesn't repeat the note.
    assert_eq!(key(&mut shell, "A", true)["sent"], false);
    shell
        .run("midi.computer_keyboard_octave", json!({ "steps": 1 }))
        .unwrap();
    // The note-off goes to the note the key started.
    let off = key(&mut shell, "A", false);
    assert_eq!(off["note"], 60);
    key(&mut shell, "S", true);
    key(&mut shell, "D", true);
    shell
        .run("midi.toggle_computer_keyboard", json!({ "on": false }))
        .unwrap();
    let all: Vec<_> = notes(&mut queue);
    assert_eq!(
        all,
        [
            (0x90, 60, 100),
            (0x80, 60, 64),
            (0x90, 74, 100),
            (0x90, 76, 100),
            (0x80, 74, 64),
            (0x80, 76, 64),
        ]
    );
}

// Spec: midi-io R3, R17
#[test]
fn port_switches_and_the_keyboard_persist_across_restarts() {
    let dir = std::env::temp_dir().join(format!("bl-shell-midi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let start = |dir: &std::path::Path| {
        let prefs = Preferences::load(dir).unwrap();
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Keys A");
        test.add_port(Direction::Output, "Synth");
        let mut shell = Shell::new(
            prefs.clone(),
            Some(dir.to_owned()),
            AudioHost::null(prefs.audio),
            Box::new(ProjectFiles::new()),
        );
        shell.use_midi_backend(Box::new(test.clone()));
        (shell, test)
    };
    let (mut shell, _) = start(&dir);
    let fresh = shell.run("midi.ports", json!({})).unwrap();
    // A port seen for the first time: Track on for an input, nothing for an
    // output; the keyboard is off.
    assert_eq!(fresh["ports"][0]["track"], true);
    assert_eq!(fresh["ports"][1]["track"], false);
    assert_eq!(fresh["keyboard"]["on"], false);

    shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Keys A", "track": false, "remote": true }),
        )
        .unwrap();
    shell
        .run(
            "midi.set_port",
            json!({ "direction": "output", "name": "Synth", "sync": true }),
        )
        .unwrap();
    shell
        .run("midi.toggle_computer_keyboard", json!({}))
        .unwrap();
    drop(shell);

    let (mut shell, test) = start(&dir);
    let back = shell.run("midi.ports", json!({})).unwrap();
    let ports = back["ports"].as_array().unwrap();
    let flags = |p: &Value| (p["track"].clone(), p["sync"].clone(), p["remote"].clone());
    assert_eq!(flags(&ports[0]), (json!(false), json!(false), json!(true)));
    assert_eq!(flags(&ports[1]), (json!(false), json!(true), json!(false)));
    assert_eq!(back["keyboard"]["on"], true);
    assert!(
        test.is_open(Direction::Input, "Keys A"),
        "Remote keeps it open"
    );
    assert!(test.is_open(Direction::Output, "Synth"));
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: midi-io R3, R27. The MPE switch is off for a port seen for the first
// time, set by `midi.set_port`, read back by `midi.ports`, and kept in the
// preferences across a restart; MPE alone opens nothing, and an output has no
// MPE switch.
#[test]
fn the_mpe_switch_persists_across_restarts_and_opens_nothing_by_itself() {
    let dir = std::env::temp_dir().join(format!("bl-shell-mpe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let start = |dir: &std::path::Path| {
        let prefs = Preferences::load(dir).unwrap();
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Roli");
        test.add_port(Direction::Output, "Synth");
        let mut shell = Shell::new(
            prefs.clone(),
            Some(dir.to_owned()),
            AudioHost::null(prefs.audio),
            Box::new(ProjectFiles::new()),
        );
        shell.use_midi_backend(Box::new(test.clone()));
        (shell, test)
    };
    let (mut shell, _) = start(&dir);
    let fresh = shell.run("midi.ports", json!({})).unwrap();
    assert_eq!(fresh["ports"][0]["mpe"], false, "off when first seen");
    let set = shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Roli", "mpe": true }),
        )
        .unwrap();
    assert_eq!(
        (set["mpe"].clone(), set["track"].clone()),
        (json!(true), json!(true))
    );
    // With Track off the port closes though MPE is on: MPE opens nothing.
    let closed = shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Roli", "track": false }),
        )
        .unwrap();
    assert_eq!(
        (closed["mpe"].clone(), closed["open"].clone()),
        (json!(true), json!(false))
    );
    shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Roli", "track": true }),
        )
        .unwrap();
    let refused = shell.run(
        "midi.set_port",
        json!({ "direction": "output", "name": "Synth", "mpe": true }),
    );
    assert!(refused.is_err(), "an output has no MPE switch");
    drop(shell);

    let (mut shell, test) = start(&dir);
    let back = shell.run("midi.ports", json!({})).unwrap();
    assert_eq!(back["ports"][0]["mpe"], true, "kept in the preferences");
    assert!(test.is_open(Direction::Input, "Roli"));
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: midi-io R28. A track whose input is a port with MPE on takes every
// channel: the chooser reads All Channels and can't be changed, and the
// channel it had stays for when MPE is off again.
#[test]
fn a_track_on_an_mpe_port_shows_all_channels_and_keeps_the_channel_it_chose() {
    let mut rig = Rig::new();
    let section = |rig: &Rig| match routing::in_out(
        rig.shell.editor().document(),
        rig.track,
        &rig.shell.interface(),
    )
    .unwrap()
    {
        InOut::Clips { input_channel, .. } => input_channel,
        other => panic!("{other:?}"),
    };
    rig.input(json!({ "type": "port", "name": "Keys A" }));
    let channel = section(&rig).run_for("5").unwrap().clone();
    rig.run(&channel.id, channel.args);
    assert_eq!(section(&rig).text, "5");

    rig.run(
        "midi.set_port",
        json!({ "direction": "input", "name": "Keys A", "mpe": true }),
    );
    let channel = section(&rig);
    assert_eq!(channel.text, "All Channels");
    assert!(!channel.enabled(), "MPE takes every channel");
    let kept = rig.shell.editor().document().track(rig.track).unwrap();
    assert_eq!(
        kept.routing().channel,
        Some(5),
        "the choice stays in the set"
    );

    rig.run(
        "midi.set_port",
        json!({ "direction": "input", "name": "Keys A", "mpe": false }),
    );
    let channel = section(&rig);
    assert_eq!(channel.text, "5");
    assert!(channel.enabled());
}

// Spec: midi-io R6
#[test]
fn a_port_the_computer_doesnt_offer_is_refused() {
    let mut shell = Shell::headless();
    let error = shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Nothing", "track": true }),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("Nothing"), "{error}");
    let ports = shell.run("midi.ports", json!({})).unwrap();
    assert_eq!(ports["ports"], json!([]));
}

// Spec: midi-io R7, R8
#[test]
fn midi_from_lists_the_ports_and_the_channel_chooser_follows_the_input() {
    let mut rig = Rig::new();
    let section = |rig: &Rig| match routing::in_out(
        rig.shell.editor().document(),
        rig.track,
        &rig.shell.interface(),
    )
    .unwrap()
    {
        InOut::Clips {
            input,
            input_channel,
            ..
        } => (input, input_channel),
        other => panic!("{other:?}"),
    };
    let (input, channel) = section(&rig);
    assert_eq!(input.text, "All Ins");
    assert_eq!(
        input.labels()[..5],
        [
            "All Ins",
            "Computer Keyboard",
            "Keys A",
            "Keys B",
            "Configure…"
        ]
    );
    assert_eq!(channel.text, "All Channels");
    assert!(!channel.enabled(), "All Ins takes every channel");

    // Choosing a port by its entry, then a channel by its entry.
    let choose = input.run_for("Keys B").unwrap().clone();
    rig.run(&choose.id, choose.args);
    let (input, channel) = section(&rig);
    assert_eq!(input.text, "Keys B");
    assert!(channel.enabled());
    assert_eq!(channel.labels()[..3], ["All Channels", "1", "2"]);
    assert_eq!(channel.labels().len(), 17);
    let choose = channel.run_for("5").unwrap().clone();
    rig.run(&choose.id, choose.args);
    let (_, channel) = section(&rig);
    assert_eq!(channel.text, "5");
    let track = rig.shell.editor().document().track(rig.track).unwrap();
    assert_eq!(track.routing().channel, Some(5));

    // The track keeps its port's name when the port goes away, and shows it.
    rig.test.remove_port(Direction::Input, "Keys B");
    rig.run("midi.ports", json!({}));
    let (input, _) = section(&rig);
    assert_eq!(input.text, "Keys B");
    assert!(input.labels().contains(&"Keys B"));
    assert_eq!(
        rig.shell
            .editor()
            .document()
            .track(rig.track)
            .unwrap()
            .routing()
            .input,
        Input::Port {
            name: "Keys B".to_owned()
        }
    );
    // And receives again when a port of that name returns (R8).
    rig.test.add_port(Direction::Input, "Keys B");
    rig.run("midi.ports", json!({}));
    rig.sync();
    let message = MidiMessage::note_on(4, 60, 100);
    assert_eq!(rig.play("Keys B", message.bytes()), 1);
}

/// The queue is attached to the engine the audio host starts, so a note
/// played with the audio running doesn't fill a dead queue.
#[test]
fn the_audio_hosts_engine_takes_the_queue() {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    let clock = control.clock().expect("manual pacing");
    let sender = shell.midi().sender().clone();
    for _ in 0..200 {
        sender.send(MidiInputEvent {
            source: MidiSource::ComputerKeyboard,
            time: clock.now(),
            message: MidiMessage::note_on(0, 60, 100),
        });
    }
    testing::run_blocks(&control, &mut shell, 4);
    assert_eq!(sender.stats().snapshot().dropped, 0);
}

/// Every sample the set names is a second of 0.5.
struct Constant;

impl MediaLibrary for Constant {
    fn open(&self, _: bl_model::ContentHash, _: SampleRate, _: u64) -> Option<OpenedSource> {
        None
    }

    fn sample(&self, _: bl_model::ContentHash, rate: SampleRate) -> Option<Arc<SampleData>> {
        SampleData::new(rate.hz(), vec![vec![0.5; rate.hz() as usize]])
            .ok()
            .map(Arc::new)
    }
}

// Spec: midi-io R12
#[test]
fn the_computer_keyboard_plays_snapshot_on_a_monitoring_track() {
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry.set_media(Arc::new(Constant));
    let mut rig = Rig::with_registry(registry);

    // Simpler with a sample, on the MIDI track.
    let folder = std::env::temp_dir().join(format!("bl-midi-play-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Tone.wav");
    let wav = bl_media::wav_bytes(&[0.5; 4800], 1, 48_000, BitDepth::Int16, Dither::None).unwrap();
    std::fs::write(&file, wav).unwrap();
    let track = rig.track;
    rig.run("arrangement.select_tracks", json!({ "tracks": [track] }));
    let added = rig.run(
        "device.add",
        json!({ "device": {
            "kind": DeviceKind::Native { device_type: SAMPLER.to_owned() },
            "name": "Simpler",
            "role": DeviceRole::Instrument,
        } }),
    );
    rig.run(
        "device.load_sample",
        json!({ "device": added["device"], "path": file.to_string_lossy() }),
    );
    std::fs::remove_file(&file).unwrap();
    std::fs::remove_dir(&folder).unwrap();
    rig.run("midi.toggle_computer_keyboard", json!({ "on": true }));
    rig.run(
        "routing.set_monitor",
        json!({ "track": track, "monitor": "in" }),
    );
    rig.sync();

    // Loudest sample of the block's first channel.
    let peak = |rig: &mut Rig| {
        let now = rig.clock.now();
        let out = rig.processor.process_at(BLOCK, &NoInput, now);
        rig.clock.advance(block());
        out.channel(0).iter().fold(0.0_f32, |m, s| m.max(s.abs()))
    };
    assert_eq!(peak(&mut rig), 0.0, "silent before a key");
    rig.run(
        "midi.computer_keyboard_key",
        json!({ "key": "A", "pressed": true }),
    );
    let heard = (0..4).map(|_| peak(&mut rig)).fold(0.0_f32, f32::max);
    assert!(heard > 0.01, "the key plays Simpler: {heard}");
}

// Spec: midi-io R12, midi-transpose R1
#[test]
fn the_computer_keyboard_plays_through_a_midi_effect_into_an_instrument() {
    use bl_devices::lift::{LIFT, PITCH};
    use bl_engine::NoteKind;
    use bl_engine::testing::{self, Probe};

    let probe = Arc::new(Probe::default());
    let mut registry = testing::registry(&probe);
    bl_devices::register(&mut registry);
    let mut rig = Rig::with_registry(registry);
    let track = rig.track;
    rig.run("arrangement.select_tracks", json!({ "tracks": [track] }));
    rig.run(
        "device.add",
        json!({ "device": {
            "kind": DeviceKind::Native { device_type: testing::INSTRUMENT.to_owned() },
            "name": "Test Instrument",
            "role": DeviceRole::Instrument,
        } }),
    );
    let lift = rig.run(
        "device.add",
        json!({ "device": {
            "kind": DeviceKind::Native { device_type: LIFT.to_owned() },
            "name": "Pitch",
            "role": DeviceRole::MidiEffect,
        } }),
    );
    rig.run(
        "device.set_param",
        json!({ "device": lift["device"], "param": PITCH.0, "value": 7.0 }),
    );
    rig.run("midi.toggle_computer_keyboard", json!({ "on": true }));
    rig.run(
        "routing.set_monitor",
        json!({ "track": track, "monitor": "in" }),
    );
    rig.sync();
    rig.blocks_run(1);
    rig.run(
        "midi.computer_keyboard_key",
        json!({ "key": "A", "pressed": true }),
    );
    rig.blocks_run(3);
    rig.run(
        "midi.computer_keyboard_key",
        json!({ "key": "A", "pressed": false }),
    );
    rig.blocks_run(3);
    let heard: Vec<(NoteKind, u8)> = probe
        .recorded_notes()
        .iter()
        .map(|n| (n.event.kind, n.event.key))
        .collect();
    // The computer keyboard's A is middle C; Pitch moves it up a fifth.
    assert_eq!(heard, [(NoteKind::On, 67), (NoteKind::Off, 67)]);
}

// Spec: midi-io R12, device-framework R39. A control change, pitch bend and
// channel pressure from a port reach the instrument through a MIDI effect,
// in order with the notes, and through the same effect switched off.
#[test]
fn a_ports_channel_messages_reach_the_instrument_through_a_midi_effect() {
    use bl_devices::lift::{LIFT, PITCH};
    use bl_engine::NoteKind;
    use bl_engine::testing::{self, Probe};

    let probe = Arc::new(Probe::default());
    let mut registry = testing::registry(&probe);
    bl_devices::register(&mut registry);
    let mut rig = Rig::with_registry(registry);
    let track = rig.track;
    rig.run("arrangement.select_tracks", json!({ "tracks": [track] }));
    let add = |rig: &mut Rig, device_type: &str, role: DeviceRole| {
        rig.run(
            "device.add",
            json!({ "device": {
                "kind": DeviceKind::Native { device_type: device_type.to_owned() },
                "name": device_type,
                "role": role,
            } }),
        )
    };
    add(&mut rig, testing::INSTRUMENT, DeviceRole::Instrument);
    let lift = add(&mut rig, LIFT, DeviceRole::MidiEffect);
    rig.run(
        "device.set_param",
        json!({ "device": lift["device"], "param": PITCH.0, "value": 7.0 }),
    );
    rig.input(json!({ "type": "port", "name": "Keys A" }));
    rig.run(
        "routing.set_monitor",
        json!({ "track": track, "monitor": "in" }),
    );
    rig.sync();
    rig.blocks_run(1);
    let messages: [&[u8]; 4] = [
        &[0x90, 60, 100],
        &[0xB0, 1, 127],
        &[0xE0, 127, 127],
        &[0xD0, 64],
    ];
    for message in messages {
        rig.play("Keys A", message);
    }
    let kinds = |probe: &Probe| -> Vec<NoteKind> {
        probe
            .recorded_notes()
            .iter()
            .map(|n| n.event.kind)
            .collect()
    };
    let expected = [
        NoteKind::On,
        NoteKind::Control {
            controller: 1,
            value: 1.0,
        },
        NoteKind::PitchBend { value: 1.0 },
        NoteKind::ChannelPressure {
            value: 64.0 / 127.0,
        },
    ];
    // Pitch moves the note, and passes the messages as they came.
    assert_eq!(kinds(&probe), expected);

    // Switched off, it ends the note it started and passes all of them.
    rig.run(
        "device.set_enabled",
        json!({ "device": lift["device"], "enabled": false }),
    );
    rig.sync();
    for message in messages {
        rig.play("Keys A", message);
    }
    assert_eq!(kinds(&probe), [&[NoteKind::Off][..], &expected].concat());
}

// Spec: midi-chord R13
#[test]
fn learn_sets_stacks_shifts_from_the_chord_played() {
    use bl_devices::stack::{LEARN, STACK, shift};

    let mut rig = Rig::new();
    let track = rig.track;
    rig.run("arrangement.select_tracks", json!({ "tracks": [track] }));
    let added = rig.run(
        "device.add",
        json!({ "device": {
            "kind": DeviceKind::Native { device_type: STACK.to_owned() },
            "name": "Chord",
            "role": DeviceRole::MidiEffect,
        } }),
    );
    let device = added["device"].clone();
    rig.run("midi.toggle_computer_keyboard", json!({ "on": true }));
    let shifts = |rig: &Rig| -> Vec<f64> {
        let params = &rig
            .shell
            .editor()
            .document()
            .device(serde_json::from_value(device.clone()).unwrap())
            .unwrap()
            .params;
        (0..6)
            .map(|n| params.get(&shift(n)).copied().unwrap_or(-1000.0))
            .collect()
    };
    let key = |rig: &mut Rig, key: &str, pressed: bool| {
        rig.run(
            "midi.computer_keyboard_key",
            json!({ "key": key, "pressed": pressed }),
        );
        rig.shell.poll();
    };

    // Learn off: keys change nothing.
    let before = shifts(&rig);
    rig.shell.poll();
    key(&mut rig, "A", true);
    key(&mut rig, "A", false);
    assert_eq!(shifts(&rig), before);

    // Learn on: A is the root; D and G, held with it, set Shift 1 and 2.
    rig.run(
        "device.set_param",
        json!({ "device": device, "param": LEARN.0, "value": 1.0 }),
    );
    rig.shell.poll();
    key(&mut rig, "A", true);
    key(&mut rig, "G", true);
    key(&mut rig, "H", true);
    let learned = shifts(&rig);
    assert_eq!(learned[0], 7.0, "G is a fifth above A's note");
    assert_eq!(learned[1], 9.0, "H is a sixth above");
    assert_eq!(&learned[2..], [0.0; 4], "the other shifts are cleared");
    assert_ne!(before, learned);
    // More keys held with the chord add shifts.
    key(&mut rig, "J", true);
    assert_eq!(shifts(&rig)[2], 11.0);
    // A new chord starts after every key is up.
    for k in ["A", "G", "H", "J"] {
        key(&mut rig, k, false);
    }
    key(&mut rig, "G", true);
    key(&mut rig, "A", true);
    assert_eq!(shifts(&rig)[0], -7.0, "the first key down is the root");
    // One undo step for the last change.
    let steps = rig.shell.editor().undo_history().count();
    rig.run("edit.undo", json!({}));
    assert_eq!(rig.shell.editor().undo_history().count(), steps - 1);
    assert_eq!(shifts(&rig)[0], 0.0, "back to the chord of the root alone");
    // Switching Learn off stops it.
    rig.run(
        "device.set_param",
        json!({ "device": device, "param": LEARN.0, "value": 0.0 }),
    );
    rig.shell.poll();
    let fixed = shifts(&rig);
    key(&mut rig, "A", false);
    key(&mut rig, "S", true);
    key(&mut rig, "D", true);
    assert_eq!(shifts(&rig), fixed);
}

// Spec: midi-chord R17
#[test]
fn learn_counts_degrees_with_use_current_scale_on() {
    use bl_devices::stack::{LEARN, STACK, USE_SCALE, shift};

    let mut rig = Rig::new();
    let track = rig.track;
    rig.run("arrangement.select_tracks", json!({ "tracks": [track] }));
    let added = rig.run(
        "device.add",
        json!({ "device": {
            "kind": DeviceKind::Native { device_type: STACK.to_owned() },
            "name": "Chord",
            "role": DeviceRole::MidiEffect,
        } }),
    );
    let device = added["device"].clone();
    rig.run(
        "song.set_scale",
        json!({ "enabled": true, "root": 0, "name": "major" }),
    );
    for param in [USE_SCALE, LEARN] {
        rig.run(
            "device.set_param",
            json!({ "device": device, "param": param.0, "value": 1.0 }),
        );
    }
    rig.run("midi.toggle_computer_keyboard", json!({ "on": true }));
    rig.shell.poll();
    for key in ["A", "G", "H", "J"] {
        rig.run(
            "midi.computer_keyboard_key",
            json!({ "key": key, "pressed": true }),
        );
        rig.shell.poll();
    }
    let params = &rig
        .shell
        .editor()
        .document()
        .device(serde_json::from_value(device).unwrap())
        .unwrap()
        .params;
    let learned: Vec<f64> = (0..3).map(|n| params[&shift(n)]).collect();
    // G, A and B are four, five and six degrees above C in C Major.
    assert_eq!(learned, [4.0, 5.0, 6.0]);
}

// Spec: midi-cc-sender R6, R7
#[test]
fn learn_assigns_an_incoming_controller_to_a_dial() {
    use bl_devices::midi_cc::{CC_CONTROL, DIALS, LEARN, dial_cc, dial_value};

    let mut rig = Rig::new();
    let track = rig.track;
    rig.run("arrangement.select_tracks", json!({ "tracks": [track] }));
    let added = rig.run(
        "device.add",
        json!({ "device": {
            "kind": DeviceKind::Native { device_type: CC_CONTROL.to_owned() },
            "name": "CC Control",
            "role": DeviceRole::MidiEffect,
        } }),
    );
    let device: bl_model::DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    rig.input(json!({ "type": "port", "name": "Keys A" }));
    let dial = |rig: &Rig, k: u32| -> (Option<f64>, Option<f64>) {
        let params = &rig.shell.editor().document().device(device).unwrap().params;
        (
            params.get(&dial_cc(k)).copied().filter(|cc| *cc != 0.0),
            params.get(&dial_value(k)).copied(),
        )
    };

    // Learn off: an incoming controller changes nothing.
    rig.play("Keys A", [0xB0, 74, 100]);
    rig.shell.poll();
    assert_eq!(dial(&rig, 0), (None, None));
    assert_eq!(DIALS, 12);

    // Learn on: the controller is handed to Custom B with its value.
    rig.run(
        "device.set_param",
        json!({ "device": device, "param": LEARN.0, "value": 1.0 }),
    );
    rig.shell.poll();
    rig.play("Keys A", [0xB0, 74, 100]);
    rig.shell.poll();
    assert_eq!(dial(&rig, 0), (Some(74.0), Some(100.0)));
    // The next controller goes to Custom C.
    rig.play("Keys A", [0xB0, 7, 50]);
    rig.shell.poll();
    assert_eq!(dial(&rig, 1), (Some(7.0), Some(50.0)));
    // One undo step for the last assignment.
    let steps = rig.shell.editor().undo_history().count();
    rig.run("edit.undo", json!({}));
    assert_eq!(rig.shell.editor().undo_history().count(), steps - 1);
    assert_eq!(dial(&rig, 1), (None, None));

    // A controller the CC Type list leaves out takes no dial: All Notes Off
    // (CC 123) and bank select (CC 0) are channel mode and bank messages.
    rig.play("Keys A", [0xB0, 123, 0]);
    rig.play("Keys A", [0xB0, 0, 5]);
    rig.shell.poll();
    assert_eq!(dial(&rig, 1), (None, None), "the next dial is still free");

    // Switching Learn off stops it.
    rig.run(
        "device.set_param",
        json!({ "device": device, "param": LEARN.0, "value": 0.0 }),
    );
    rig.shell.poll();
    rig.play("Keys A", [0xB0, 10, 20]);
    rig.shell.poll();
    assert_eq!(dial(&rig, 2), (None, None));
}

/// Waits until `done` holds, for up to two seconds: the MIDI output thread
/// hands messages on a thread of its own.
fn wait_for(mut done: impl FnMut() -> bool) -> bool {
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(2) {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    done()
}

// Spec: external-devices R2, R5
#[test]
fn a_track_keeps_a_missing_ports_name_and_sends_again_when_it_returns() {
    let mut rig = Rig::new();
    rig.run(
        "midi.set_port",
        json!({ "direction": "output", "name": "Synth", "track": true }),
    );
    rig.run(
        "routing.set_monitor",
        json!({ "track": rig.track, "monitor": "in" }),
    );
    rig.run(
        "routing.set_midi_output",
        json!({ "track": rig.track, "to": { "name": "Synth", "channel": 2 } }),
    );
    rig.sync();
    // The port is open, with Track on, and the note leaves on channel 2.
    assert!(rig.test.is_open(Direction::Output, "Synth"));
    rig.play_settled("Keys A", [0x90, 60, 100]);
    assert!(wait_for(|| !rig.test.sent("Synth").is_empty()));
    assert_eq!(rig.test.sent("Synth")[0], [0x91, 60, 100]);
    assert_eq!(rig.shell.midi().output().stats().snapshot().sent, 1);

    // The port goes: the track keeps its name and sends nothing.
    rig.test.remove_port(Direction::Output, "Synth");
    rig.shell.refresh_midi();
    assert_eq!(rig.shell.midi().output_names(), Vec::<String>::new());
    rig.play_settled("Keys A", [0x90, 62, 100]);
    assert!(wait_for(|| rig
        .shell
        .midi()
        .output()
        .stats()
        .snapshot()
        .unrouted
        >= 1));
    let routing = rig
        .shell
        .editor()
        .document()
        .track(rig.track)
        .unwrap()
        .routing();
    assert_eq!(
        routing.midi_output,
        Some(bl_model::MidiTo::Port {
            name: "Synth".to_owned(),
            channel: 2,
            mpe: None
        })
    );

    // It returns, with the Track switch it had, and the track sends again.
    rig.test.add_port(Direction::Output, "Synth");
    rig.shell.refresh_midi();
    assert_eq!(rig.shell.midi().output_names(), vec!["Synth".to_owned()]);
    rig.play_settled("Keys A", [0x90, 64, 100]);
    assert!(wait_for(|| !rig.test.sent("Synth").is_empty()));
    assert_eq!(rig.test.sent("Synth")[0], [0x91, 64, 100]);

    // A port whose Track switch is off takes nothing, though it is open
    // for another reason.
    rig.run(
        "midi.set_port",
        json!({ "direction": "output", "name": "Synth", "track": false, "sync": true }),
    );
    let unrouted = rig.shell.midi().output().stats().snapshot().unrouted;
    rig.play_settled("Keys A", [0x90, 65, 100]);
    assert!(wait_for(|| rig
        .shell
        .midi()
        .output()
        .stats()
        .snapshot()
        .unrouted
        > unrouted));
    assert_eq!(rig.test.sent("Synth").len(), 1);
}

/// A shell started on the preferences in `dir`, with an input and two
/// outputs plugged in.
fn start_with_outputs(dir: &std::path::Path) -> (Shell, TestBackend) {
    let prefs = Preferences::load(dir).unwrap();
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Keys A");
    test.add_port(Direction::Output, "Drum machine");
    test.add_port(Direction::Output, "Drone");
    let mut shell = Shell::new(
        prefs.clone(),
        Some(dir.to_owned()),
        AudioHost::null(prefs.audio),
        Box::new(ProjectFiles::new()),
    );
    shell.use_midi_backend(Box::new(test.clone()));
    (shell, test)
}

fn port_named<'a>(ports: &'a Value, name: &str) -> &'a Value {
    ports["ports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|port| port["name"] == name)
        .unwrap()
}

// Spec: midi-clock R1, R2, R10, R25. An output's sync options start at 0 ms,
// Song and Stop and Start, are set by `midi.set_port`, read back by
// `midi.ports`, and kept in the preferences across a restart, as Show Resync
// Button is.
#[test]
fn the_sync_options_and_the_resync_button_persist_across_restarts() {
    let dir = std::env::temp_dir().join(format!("bl-shell-sync-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut shell, _) = start_with_outputs(&dir);
    let fresh = shell.run("midi.ports", json!({})).unwrap();
    let drums = port_named(&fresh, "Drum machine");
    assert_eq!(drums["sync_delay_ms"], 0);
    assert_eq!(drums["clock_type"], "song");
    assert_eq!(drums["hardware_resync"], "stop_and_start");
    assert_eq!(fresh["show_resync_button"], false, "off at first");

    shell
        .run(
            "midi.set_port",
            json!({ "direction": "output", "name": "Drum machine", "sync": true,
                    "sync_delay_ms": -35, "clock_type": "pattern",
                    "hardware_resync": "start_only" }),
        )
        .unwrap();
    // The options are kept while Sync is off.
    shell
        .run(
            "midi.set_port",
            json!({ "direction": "output", "name": "Drone", "sync_delay_ms": 12,
                    "hardware_resync": "dont_resync" }),
        )
        .unwrap();
    for (args, what) in [
        (json!({ "sync_delay_ms": 101 }), "past 100 ms"),
        (json!({ "sync_delay_ms": -101 }), "past -100 ms"),
    ] {
        let mut full = json!({ "direction": "output", "name": "Drone" });
        full.as_object_mut()
            .unwrap()
            .extend(args.as_object().unwrap().clone());
        assert!(shell.run("midi.set_port", full).is_err(), "{what}");
    }
    let input_type = shell.run(
        "midi.set_port",
        json!({ "direction": "input", "name": "Keys A", "clock_type": "pattern" }),
    );
    assert!(input_type.is_err(), "an input has no MIDI Clock Type");
    let on = shell
        .run("midi.set_show_resync_button", json!({ "on": true }))
        .unwrap();
    assert_eq!(on["on"], true);
    let outputs = shell.midi().sync_outputs();
    assert_eq!(outputs.len(), 1, "only an output with Sync on gets clock");
    assert_eq!(outputs[0].delay_ms, -35);
    drop(shell);

    let (mut shell, _) = start_with_outputs(&dir);
    let back = shell.run("midi.ports", json!({})).unwrap();
    let drums = port_named(&back, "Drum machine");
    assert_eq!(drums["sync"], true);
    assert_eq!(drums["sync_delay_ms"], -35);
    assert_eq!(drums["clock_type"], "pattern");
    assert_eq!(drums["hardware_resync"], "start_only");
    let drone = port_named(&back, "Drone");
    assert_eq!(
        (drone["sync"].clone(), drone["sync_delay_ms"].clone()),
        (json!(false), json!(12))
    );
    assert_eq!(drone["hardware_resync"], "dont_resync");
    assert_eq!(back["show_resync_button"], true);
    assert_eq!(shell.midi().sync_outputs().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: midi-clock R11, R25. `transport.resync_external_hardware` sends each
// port's own choice before the next bar line's clock while the song plays,
// and nothing while it is stopped.
#[test]
fn resync_sends_each_ports_choice_before_the_next_bar_line_and_nothing_while_stopped() {
    let mut rig = Rig::new();
    rig.test.add_port(Direction::Output, "Drone");
    rig.shell.refresh_midi();
    rig.run(
        "midi.set_port",
        json!({ "direction": "output", "name": "Synth", "sync": true,
                "hardware_resync": "stop_and_start" }),
    );
    rig.run(
        "midi.set_port",
        json!({ "direction": "output", "name": "Drone", "sync": true,
                "hardware_resync": "dont_resync" }),
    );
    let (producer, mut sent) = rtrb::RingBuffer::new(4096);
    rig.processor.set_midi_output(producer);
    rig.engine.sync_outputs(&rig.shell.midi().sync_outputs());
    assert!(rig.engine.flush(Duration::from_secs(5)));
    // The ID is the engine's command.
    let resync =
        bl_engine::transport::command_from_args("transport.resync_external_hardware", json!({}))
            .unwrap();
    let sent_bytes = |sent: &mut Consumer<bl_core::midi::MidiOutputEvent>| {
        std::iter::from_fn(|| sent.pop().ok()).collect::<Vec<_>>()
    };

    // Stopped: the button does nothing, and no clock goes.
    rig.engine.transport(resync);
    rig.blocks_run(4);
    assert!(sent_bytes(&mut sent).is_empty());

    rig.engine.transport(bl_engine::TransportCommand::Play);
    rig.blocks_run(100);
    sent_bytes(&mut sent);
    rig.engine.transport(resync);
    // 4/4 at 120 BPM: the bar line is 96,000 frames in, 375 blocks of 256.
    rig.blocks_run(300);
    let events = sent_bytes(&mut sent);
    let synth = bl_core::midi::PortId::from_name("Synth");
    let drone = bl_core::midi::PortId::from_name("Drone");
    let stop = events
        .iter()
        .find(|e| e.message == MidiMessage::STOP)
        .unwrap();
    let at_bar: Vec<_> = events
        .iter()
        .filter(|e| e.time == stop.time && e.port == synth)
        .map(|e| e.message.padded()[0])
        .collect();
    assert_eq!(
        at_bar,
        vec![0xFC, 0xFA, 0xF8],
        "Stop, Start, then the bar's clock"
    );
    assert!(
        events
            .iter()
            .filter(|e| e.port == drone)
            .all(|e| e.message == MidiMessage::CLOCK),
        "Don't Resync sends the clock alone"
    );
}

// Spec: midi-clock R13. An export is rendered offline, and sends no clock.
#[test]
fn an_export_sends_no_clock() {
    let mut rig = Rig::new();
    rig.run(
        "midi.set_port",
        json!({ "direction": "output", "name": "Synth", "sync": true }),
    );
    let dir = std::env::temp_dir().join(format!("bl-shell-sync-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    rig.run(
        "export.audio",
        json!({ "path": dir.join("out.wav").to_string_lossy(), "start": 0,
                "length": 705_600 * 4 }),
    );
    let stats = rig.shell.midi().output().stats().snapshot();
    assert_eq!((stats.sent, stats.late, stats.unrouted), (0, 0, 0));
    assert_eq!(rig.run("sync.state", json!({}))["clocks_sent"], 0);
    let _ = std::fs::remove_dir_all(&dir);
}
