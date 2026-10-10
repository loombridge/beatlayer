//! The golden fixtures: songs built in code, through the model's commands.
//!
//! Until clips play (WP-1.4), the engine's sound sources are test tones, so
//! each fixture is test tones through tracks, groups, returns, and faders,
//! with commands scheduled at musical positions. The transport's fixtures
//! (WP-1.5) use the beat tone, which sounds only while the song plays and
//! whose every sample follows the song position in ticks, so a render shows
//! the frame each start, stop, and loop lands on. Tones play at −24 dBFS
//! or so, louder than the tones' default of −60, so that a level error
//! shows above the cross-platform threshold. These renders are files for
//! tests, never played on a device.
//!
//! Changing a fixture changes its render: remake its reference with
//! `cargo xtask golden --update` and say why in the pull request.

use bl_core::time::{Bpm, TempoMap, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap};
use bl_engine::nodes::{BEAT_TONE, TEST_TONE, beat_tone, fader, test_tone};
use bl_engine::{Action, NodeKey, ParamValue, ScheduledCommand, TransportCommand};
use bl_model::{Command, Device, DeviceId, DeviceKind, Editor, ParamId, TrackId, TrackKind};

use super::{Fixture, SAMPLE_RATE, Scene};
use crate::render::Cue;

pub(super) const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "single-tone",
        about: "One test tone through a track and Main.",
        build: single_tone,
    },
    Fixture {
        name: "summed-tracks",
        about: "Four tracks summed into Main through their faders, at three volumes, one muted.",
        build: summed_tracks,
    },
    Fixture {
        name: "group-and-return",
        about: "A group holding two tracks and a group of its own, a return, and a track beside them, \
                each through its fader, then Main's.",
        build: group_and_return,
    },
    Fixture {
        name: "tempo-change",
        about: "A command on every eighth note through a tempo map that holds at 120 BPM, ramps \
                to 200, and jumps to 90, so where each one lands depends on the map.",
        build: tempo_change,
    },
    Fixture {
        name: "scheduled-commands",
        about: "Commands at bars, beats, and sixteenths of a song that changes from 4/4 to 7/8: \
                mute and unmute, a level, frequencies, and volumes, each on its exact frame.",
        build: scheduled_commands,
    },
    Fixture {
        name: "start-mid-bar",
        about: "The transport on frames no block starts on: Play from an insert marker in the \
                middle of bar 2, Stop, Continue Playback, Stop twice (back to 1.1.1), and Play \
                again, sounded by the beat tone.",
        build: start_mid_bar,
    },
    Fixture {
        name: "loop-wraps",
        about: "Playback from 1.1.1 into an Arrangement loop of a beat and a half at 130 BPM, \
                which wraps four times on its exact frames; the loop is in the set, so a saved \
                set renders the same.",
        build: loop_wraps,
    },
    Fixture {
        name: "tempo-ramp-loop",
        about: "A loop inside a tempo ramp from 90 to 150 BPM that runs on past the loop's \
                end: each pass speeds up, and the wrap lands where the tempo map puts the end.",
        build: tempo_ramp_loop,
    },
];

/// A song under construction.
struct Song {
    editor: Editor,
    schedule: Vec<ScheduledCommand>,
    cues: Vec<Cue>,
}

impl Song {
    fn new() -> Self {
        Self {
            editor: Editor::new(),
            schedule: Vec::new(),
            cues: Vec::new(),
        }
    }

    /// Adds a beat tone at the end of a track's chain.
    fn beat_tone(&mut self, track: TrackId, cycles_per_beat: f64, dbfs: f64) -> DeviceId {
        let id = self.editor.new_id();
        let kind = DeviceKind::Native {
            device_type: BEAT_TONE.to_owned(),
        };
        let mut device = Device::new(id, "Beat Tone", kind);
        device
            .params
            .insert(beat_tone::CYCLES_PER_BEAT, cycles_per_beat);
        device.params.insert(beat_tone::LEVEL, dbfs);
        self.apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        });
        id
    }

    /// Moves the transport on frame `at` of the render.
    fn cue(&mut self, at: u64, command: TransportCommand) {
        self.cues.push(Cue { at, command });
    }

    /// Puts the loop brace at `start` for `length`, and switches the loop
    /// on.
    fn loop_on(&mut self, start: Ticks, length: Ticks) {
        self.apply(Command::SetLoop { start, length });
        self.apply(Command::SetLoopEnabled { enabled: true });
    }

    fn apply(&mut self, command: Command) {
        self.editor
            .apply(command)
            .expect("a fixture's commands are valid");
    }

    /// Adds a track at the end of its list: the top level, a group, or the
    /// returns.
    fn track(&mut self, kind: TrackKind, name: &str, parent: Option<TrackId>) -> TrackId {
        let document = self.editor.document();
        let index = match (kind, parent) {
            (TrackKind::Return, _) => document.returns().len(),
            (_, Some(group)) => document
                .track(group)
                .expect("the group exists")
                .children()
                .len(),
            (_, None) => document.top_level().len(),
        };
        let id = self.editor.new_id();
        self.apply(Command::CreateTrack {
            id,
            kind,
            name: name.to_owned(),
            parent,
            index,
        });
        id
    }

    /// Adds a test tone at the end of a track's chain.
    fn tone(&mut self, track: TrackId, hz: f64, dbfs: f64) -> DeviceId {
        let id = self.editor.new_id();
        let kind = DeviceKind::Native {
            device_type: TEST_TONE.to_owned(),
        };
        let mut device = Device::new(id, "Test Tone", kind);
        device.params.insert(test_tone::FREQUENCY, hz);
        device.params.insert(test_tone::LEVEL, dbfs);
        let index = self
            .editor
            .document()
            .track(track)
            .expect("the track exists")
            .devices()
            .len();
        self.apply(Command::InsertDevice {
            track,
            index,
            device,
        });
        id
    }

    fn volume(&mut self, track: TrackId, volume: f64) {
        self.apply(Command::SetVolume { track, volume });
    }

    fn main_volume(&mut self, volume: f64) {
        let track = self.editor.document().main_id();
        self.volume(track, volume);
    }

    fn mute(&mut self, track: TrackId) {
        self.apply(Command::SetMute { track, mute: true });
    }

    /// A position given as bar, beat, and sixteenth (`"2.3.1"`), by the
    /// song's time signatures.
    fn at(&self, position: &str) -> Ticks {
        self.editor
            .document()
            .settings()
            .time_signatures
            .parse_position(position)
            .expect("a fixture's positions are valid")
    }

    /// Schedules a parameter change at `at`.
    fn schedule(&mut self, at: Ticks, key: NodeKey, param: ParamId, value: f64) {
        let id = self.schedule.len() as u64 + 1;
        self.schedule.push(ScheduledCommand {
            id,
            at,
            action: Action::Param {
                key,
                param,
                value: ParamValue::Set(value),
            },
        });
    }

    /// The finished song, `seconds` long. Without cues, it plays from its
    /// start.
    fn finish(self, seconds: f64) -> Scene {
        Scene {
            editor: self.editor,
            schedule: self.schedule,
            cues: if self.cues.is_empty() {
                vec![Cue::PLAY]
            } else {
                self.cues
            },
            frames: SAMPLE_RATE.seconds_to_frames(seconds).get() as u64,
        }
    }
}

fn bpm(bpm: f64) -> Bpm {
    Bpm::new(bpm).expect("a fixture's tempos are valid")
}

fn single_tone() -> Scene {
    let mut song = Song::new();
    let track = song.track(TrackKind::Midi, "Tone", None);
    song.tone(track, 440.0, -24.0);
    song.finish(2.0)
}

fn summed_tracks() -> Scene {
    let mut song = Song::new();
    for (name, hz, volume) in [
        ("Root", 220.0, 1.0),
        ("Fifth", 330.0, 0.5),
        ("Third", 550.0, 0.25),
    ] {
        let track = song.track(TrackKind::Midi, name, None);
        song.tone(track, hz, -24.0);
        song.volume(track, volume);
    }
    // Loud, and silent because it's muted.
    let muted = song.track(TrackKind::Audio, "Muted", None);
    song.tone(muted, 880.0, -12.0);
    song.mute(muted);
    song.finish(2.0)
}

fn group_and_return() -> Scene {
    let mut song = Song::new();
    let group = song.track(TrackKind::Group, "Group", None);
    song.volume(group, 0.5);
    let a = song.track(TrackKind::Midi, "A", Some(group));
    song.tone(a, 262.0, -24.0);
    let b = song.track(TrackKind::Midi, "B", Some(group));
    song.tone(b, 392.0, -24.0);
    song.volume(b, 0.7);
    let inner = song.track(TrackKind::Group, "Inner", Some(group));
    song.volume(inner, 0.8);
    let c = song.track(TrackKind::Audio, "C", Some(inner));
    song.tone(c, 523.25, -24.0);
    let beside = song.track(TrackKind::Midi, "Beside", None);
    song.tone(beside, 196.0, -30.0);
    // Returns have no sends yet (WP-2.19), so this one has a tone of its own.
    let ret = song.track(TrackKind::Return, "Return", None);
    song.tone(ret, 660.0, -30.0);
    song.volume(ret, 0.5);
    song.main_volume(0.8);
    song.finish(2.0)
}

fn tempo_change() -> Scene {
    let mut song = Song::new();
    // 120 BPM for two beats, a ramp to 200 over four, then a jump to 90.
    let beats = |n: i64| Ticks::QUARTER * n;
    let tempo = TempoMap::from_breakpoints([
        (beats(0), bpm(120.0)),
        (beats(2), bpm(120.0)),
        (beats(6), bpm(200.0)),
        (beats(6), bpm(90.0)),
    ])
    .expect("the tempo map is valid");
    song.apply(Command::SetTempo { tempo });
    let track = song.track(TrackKind::Midi, "Tone", None);
    let tone = song.tone(track, 440.0, -24.0);
    // Eight beats, which take about 3.9 seconds.
    for eighth in 0..16 {
        let hz = if eighth % 2 == 0 { 440.0 } else { 660.0 };
        let at = Ticks::QUARTER / 2 * eighth;
        song.schedule(at, NodeKey::Device(tone), test_tone::FREQUENCY, hz);
    }
    song.finish(4.0)
}

fn scheduled_commands() -> Scene {
    let mut song = Song::new();
    // A bar of 4/4, then 7/8, at 180 BPM: bars of 1⅓ seconds, then 1⅙.
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(180.0)),
    });
    let signature = |n, d| TimeSignature::new(n, d).expect("the time signature is valid");
    let time_signatures = TimeSignatureMap::new([
        TimeSignatureChange {
            at: Ticks::ZERO,
            signature: signature(4, 4),
        },
        TimeSignatureChange {
            at: Ticks::WHOLE,
            signature: signature(7, 8),
        },
    ])
    .expect("the time-signature map is valid");
    song.apply(Command::SetTimeSignatures { time_signatures });
    let lead = song.track(TrackKind::Midi, "Lead", None);
    let tone = song.tone(lead, 440.0, -24.0);
    let pad = song.track(TrackKind::Midi, "Pad", None);
    song.tone(pad, 220.0, -30.0);

    let lead_fader = NodeKey::Fader(lead);
    let pad_fader = NodeKey::Fader(pad);
    let tone = NodeKey::Device(tone);
    let commands = [
        ("1.3.1", lead_fader, fader::MUTE, 1.0),
        ("1.4.1", lead_fader, fader::MUTE, 0.0),
        ("1.4.3", pad_fader, fader::VOLUME, 0.5),
        ("2.1.1", tone, test_tone::FREQUENCY, 660.0),
        ("2.4.1", pad_fader, fader::VOLUME, 0.25),
        ("2.5.2", tone, test_tone::LEVEL, -36.0),
        ("3.1.1", tone, test_tone::FREQUENCY, 495.0),
        ("3.3.2", lead_fader, fader::VOLUME, 0.5),
        ("3.7.2", pad_fader, fader::MUTE, 1.0),
    ];
    for (position, key, param, value) in commands {
        let at = song.at(position);
        song.schedule(at, key, param, value);
    }
    // Three bars.
    song.finish(3.75)
}

/// Spec: transport R1 to R6, on exact frames.
fn start_mid_bar() -> Scene {
    let mut song = Song::new();
    let track = song.track(TrackKind::Midi, "Beat", None);
    // 440 Hz at 120 BPM.
    song.beat_tone(track, 220.0, -24.0);
    let marker = song.at("2.2.3");
    for (at, command) in [
        // The insert marker moves while the song is stopped, silently.
        (0, TransportCommand::Locate(marker)),
        (30_001, TransportCommand::Play),
        (90_017, TransportCommand::Stop),
        (100_003, TransportCommand::Continue),
        (140_009, TransportCommand::Stop),
        (140_500, TransportCommand::Stop),
        (150_011, TransportCommand::Play),
    ] {
        song.cue(at, command);
    }
    song.finish(3.75)
}

/// Spec: transport R13, R14, R16.
fn loop_wraps() -> Scene {
    let mut song = Song::new();
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(130.0)),
    });
    let track = song.track(TrackKind::Midi, "Beat", None);
    song.beat_tone(track, 220.0, -24.0);
    // From 1.2.3 to 1.4.1: frames 33,231 to 66,462.
    let start = song.at("1.2.3");
    song.loop_on(start, Ticks::QUARTER * 3 / 2);
    song.finish(4.0)
}

/// Spec: transport R13, R15.
fn tempo_ramp_loop() -> Scene {
    let mut song = Song::new();
    // 90 BPM for a bar, then a ramp to 150 over two bars.
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, bpm(90.0)),
        (Ticks::WHOLE, bpm(90.0)),
        (Ticks::WHOLE * 3, bpm(150.0)),
    ])
    .expect("the tempo map is valid");
    song.apply(Command::SetTempo { tempo });
    let track = song.track(TrackKind::Midi, "Beat", None);
    song.beat_tone(track, 220.0, -24.0);
    // From 2.2.1 to 2.3.3, inside the ramp.
    let start = song.at("2.2.1");
    song.loop_on(start, Ticks::QUARTER * 3 / 2);
    let bar_2 = song.at("2.1.1");
    song.cue(0, TransportCommand::Locate(bar_2));
    song.cue(0, TransportCommand::Play);
    song.finish(4.5)
}
