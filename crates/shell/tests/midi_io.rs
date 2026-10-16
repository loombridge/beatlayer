//! MIDI input in the shell (WP-2.2, `docs/specs/midi-io.md`): a port's
//! switches decide whether its notes reach tracks, a MIDI track's MIDI From
//! and channel pick what it takes, the computer MIDI keyboard plays the
//! right notes, and the switches persist. Everything runs against the
//! in-process test backend and a real engine processor driven by hand.

use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
use bl_core::time::SampleRate;
use bl_engine::{Engine, EngineConfig, NoInput, Processor, Registry};
use bl_midi_io::{Direction, TestBackend};
use bl_model::{Input, TrackId};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::routing::{self, InOut};
use bl_shell::settings::Preferences;
use rtrb::Consumer;
use serde_json::{Value, json};

const BLOCK: usize = 256;

/// A shell with two input ports and an output, a MIDI track, and an engine
/// processor whose queue is the shell's.
struct Rig {
    shell: Shell,
    test: TestBackend,
    track: TrackId,
    engine: Engine,
    processor: Processor,
    /// When the next block starts, on the clock the messages are stamped on.
    next: Instant,
}

impl Rig {
    fn new() -> Self {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Keys A");
        test.add_port(Direction::Input, "Keys B");
        test.add_port(Direction::Output, "Synth");
        let mut shell = Shell::headless();
        shell.use_midi_backend(Box::new(test.clone()));
        let track = shell.editor().document().top_level().next().unwrap();
        let (engine, mut processor) = Engine::start(
            EngineConfig::new(SampleRate::HZ_48000, BLOCK),
            Registry::new(),
        )
        .unwrap();
        processor.set_midi_input(shell.midi().sender().attach(1024));
        let mut rig = Self {
            shell,
            test,
            track,
            engine,
            processor,
            next: Instant::now(),
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
    fn play(&mut self, port: &str, message: [u8; 3]) -> usize {
        self.test.send(port, Instant::now(), &message);
        self.blocks_run(2)
    }

    /// Runs `count` blocks, each starting after the last and after now, and
    /// returns how many messages the track's input received over them.
    fn blocks_run(&mut self, count: u32) -> usize {
        let mut received = 0;
        for _ in 0..count {
            let now = self.next.max(Instant::now());
            self.next = now + Duration::from_nanos(BLOCK as u64 * 1_000_000_000 / 48_000);
            self.processor.process_at(BLOCK, &NoInput, now);
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
    assert_eq!(rig.play("Keys B", message.bytes().try_into().unwrap()), 1);
}

/// The queue is attached to the engine the audio host starts, so a note
/// played with the audio running doesn't fill a dead queue.
#[test]
fn the_audio_hosts_engine_takes_the_queue() {
    let mut shell = Shell::headless();
    shell.start_audio();
    let sender = shell.midi().sender().clone();
    for _ in 0..200 {
        sender.send(MidiInputEvent {
            source: MidiSource::ComputerKeyboard,
            time: Instant::now(),
            message: MidiMessage::note_on(0, 60, 100),
        });
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(sender.stats().snapshot().dropped, 0);
}
