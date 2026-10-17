//! The golden fixtures: songs built in code, through the model's commands.
//!
//! The first fixtures' sound sources are test tones, through tracks, groups,
//! returns, and their mixers, with commands scheduled at musical positions.
//! The transport's fixtures (WP-1.5) use the beat tone, which sounds only
//! while the song plays and whose every sample follows the song position in
//! ticks, so a render shows the frame each start, stop, and loop lands on.
//! The mixer's fixtures (WP-1.6) pan, send, solo, and move faders. The clip
//! fixtures (WP-1.4) play audio files made here, as 32-bit float WAV files
//! in memory: sine chirps, whose pitch moves all the time, so a render
//! shows exactly which frame of a file each frame of the song played, made
//! with `libm`'s pure-Rust sine, so they're the same to the bit on every
//! platform. One file is at 44.1 kHz, so it plays through sample-rate
//! conversion. Tones and files play at −24 dBFS or so, louder than the
//! tones' default of −60, so that a level error shows above the
//! cross-platform threshold. These renders are files for tests, never
//! played on a device.
//!
//! Changing a fixture changes its render: remake its reference with
//! `cargo xtask golden --update` and say why in the pull request.

use std::io::Cursor;
use std::sync::Arc;

use bl_core::time::{Bpm, TempoMap, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap};
use bl_engine::mixer::{gain_from_db, track as mixer};
use bl_engine::nodes::{BEAT_TONE, TEST_TONE, beat_tone, test_tone};
use bl_engine::{
    Action, MediaMap, NodeKey, ParamValue, ScheduledCommand, SessionCommand, TransportCommand,
};
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, CrossfadeAssign, Device, DeviceId,
    DeviceKind, Editor, ParamId, SendTap, TrackId, TrackKind,
};

use super::{Fixture, MediaFile, SAMPLE_RATE, Scene};
use crate::render::{Cue, Launch};

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
    Fixture {
        name: "scene-tempo",
        about: "A scene with its own tempo launched while the song plays at 120 BPM: the launch \
                lands on the next bar line, where the beat tone's beats lengthen to the \
                scene's 60 BPM and the playhead keeps its beat.",
        build: scene_tempo,
    },
    Fixture {
        name: "pan-and-sends",
        about: "The mixer: tones panned across the field by the constant-power law, a group \
                with its own volume and pan, a post-fader return and a pre-fader one, a \
                deactivated track heard only through its pre-fader send, and Main's volume and \
                pan.",
        build: pan_and_sends,
    },
    Fixture {
        name: "solo-in-place",
        about: "A track inside a group soloed with Solo in Place on: it plays through its \
                group and its send to the return, and the other tracks and their sends are \
                silent.",
        build: solo_in_place,
    },
    Fixture {
        name: "solo-a-return",
        about: "A return track soloed, with Solo in Place off: it plays every send it gets, \
                and no track is heard directly.",
        build: solo_a_return,
    },
    Fixture {
        name: "fader-gesture",
        about: "A fader dragged with the mouse in the Session mixer (WP-1.9): the volumes the \
                window's drag set on a 440 Hz tone, one 60 Hz frame apart, recorded in \
                tests/fixtures/ui/fader-gesture.json by bl-ui's mixer test, each gliding to its \
                value over 20 ms, so the render has no zipper noise.",
        build: fader_gesture,
    },
    Fixture {
        name: "fader-move",
        about: "A fader drag from 0 dB to -20 dB and back in 1 dB steps every 1/32 beat, a \
                mute and an unmute, and a pan sweep from left to right in steps, each gliding \
                to its value over 20 ms, so the render has no zipper noise or clicks.",
        build: fader_move,
    },
    Fixture {
        name: "crossfade",
        about: "Two tracks, one assigned to the crossfader's A side and one to B, swept from \
                the A end to the B end in 1/16 steps every 1/32 beat, closer than the 20 ms a \
                move takes so the glides run into each other, with a third track assigned to \
                neither playing through.",
        build: crossfade,
    },
    Fixture {
        name: "clip-mid-bar",
        about: "Audio clips starting mid-bar, on frames no block starts on: one plays a stretch \
                from the middle of its file, between its Start and End markers, and one is cut \
                off by its own end mid-file.",
        build: clip_mid_bar,
    },
    Fixture {
        name: "looped-clip",
        about: "A looped audio clip at -6 dB: it plays from its Start marker into its loop \
                brace and repeats the brace nine times, each wrap on its exact frame.",
        build: looped_clip,
    },
    Fixture {
        name: "overlapping-clips",
        about: "Clips overlapping on two audio tracks, one a mono file at 44.1 kHz that plays \
                through sample-rate conversion, on a track panned right at -6 dB.",
        build: overlapping_clips,
    },
    Fixture {
        name: "clip-fades",
        about: "Clip edges with fades off (hard edges mid-waveform), with Create Fades on Clip \
                Edges' 4 ms fades, and with long fades on a clip above 0 dB.",
        build: clip_fades,
    },
    Fixture {
        name: "clip-tempo-change",
        about: "An unwarped clip across a tempo ramp from 120 to 90 BPM, whose audio runs on \
                unbroken while its end lands where the tempo map puts it, and a looped clip \
                placed after the ramp.",
        build: clip_tempo_change,
    },
];

/// A song under construction.
struct Song {
    editor: Editor,
    files: Vec<MediaFile>,
    media: MediaMap,
    schedule: Vec<ScheduledCommand>,
    cues: Vec<Cue>,
    launches: Vec<Launch>,
}

impl Song {
    fn new() -> Self {
        Self {
            editor: Editor::new(),
            files: Vec::new(),
            media: MediaMap::new(),
            schedule: Vec::new(),
            cues: Vec::new(),
            launches: Vec::new(),
        }
    }

    /// Makes an audio file of `channels` at `rate` as a 32-bit float WAV
    /// file, and returns the content hash clips name it by. The engine gets
    /// it as the media store would give it: decoded from the file's bytes.
    fn file(&mut self, name: &str, rate: u32, channels: &[Vec<f32>]) -> ContentHash {
        let spec = hound::WavSpec {
            channels: u16::try_from(channels.len()).expect("a few channels"),
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut cursor = Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut cursor, spec).expect("the WAV header writes");
        for frame in 0..channels[0].len() {
            for channel in channels {
                writer
                    .write_sample(channel[frame])
                    .expect("the WAV file writes");
            }
        }
        writer.finalize().expect("the WAV file finishes");
        let bytes = cursor.into_inner();
        let hash = ContentHash(*bl_media::ContentHash::of_bytes(&bytes).as_bytes());
        let decoded = bl_media::Decoder::from_reader(Cursor::new(bytes.clone()), Some("wav"))
            .and_then(bl_media::Decoder::decode_to_end)
            .expect("the WAV file decodes");
        self.media.insert(hash, Arc::new(decoded));
        self.files.push(MediaFile {
            name: name.to_owned(),
            hash,
            bytes,
        });
        hash
    }

    /// A stereo file at 48 kHz, 4.5 seconds long: a chirp up from 220 to
    /// 1,320 Hz on the left at -24 dBFS, and down from 990 to 330 Hz on the
    /// right, 2.5 dB quieter.
    fn chirp(&mut self) -> ContentHash {
        let frames = 216_000;
        let left = chirp(frames, 48_000, 220.0, 1_320.0, 0.0625);
        let right = chirp(frames, 48_000, 990.0, 330.0, 0.046_875);
        self.file("chirp.wav", 48_000, &[left, right])
    }

    /// A mono file at 44.1 kHz, 3 seconds long: a chirp up from 165 to
    /// 247.5 Hz at -24 dBFS.
    fn drone(&mut self) -> ContentHash {
        let mono = chirp(132_300, 44_100, 165.0, 247.5, 0.0625);
        self.file("drone.wav", 44_100, &[mono])
    }

    /// Puts an audio clip on `track` at `at` for `length`.
    fn audio_clip(&mut self, track: TrackId, at: Ticks, length: Ticks, audio: AudioData) -> ClipId {
        let id = self.editor.new_id();
        self.apply(Command::CreateClip {
            track,
            clip: Clip::new(id, at, length, ClipData::Audio(audio)),
        });
        id
    }

    /// How long the song plays to the end of its last clip, and a quarter of
    /// a second more, rounded up to a quarter of a second.
    fn to_last_clip(&self) -> f64 {
        let document = self.editor.document();
        let end = document
            .tracks()
            .flat_map(bl_model::Track::clips)
            .map(bl_model::Clip::end)
            .max()
            .expect("the song has clips");
        let seconds = document.settings().tempo.ticks_to_seconds(end) + 0.25;
        (seconds * 4.0).ceil() / 4.0
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

    /// Asks the Session View for `command` on frame `at` of the render.
    fn launch(&mut self, at: u64, command: SessionCommand) {
        self.launches.push(Launch { at, command });
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

    fn pan(&mut self, track: TrackId, pan: f64) {
        self.apply(Command::SetPan { track, pan });
    }

    fn send(&mut self, track: TrackId, to: TrackId, level: f64) {
        self.apply(Command::SetSend { track, to, level });
    }

    fn solo(&mut self, track: TrackId) {
        self.apply(Command::SetSolo { track, solo: true });
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
            files: self.files,
            media: self.media,
            schedule: self.schedule,
            cues: if self.cues.is_empty() {
                vec![Cue::PLAY]
            } else {
                self.cues
            },
            launches: self.launches,
            frames: SAMPLE_RATE.seconds_to_frames(seconds).get() as u64,
        }
    }
}

fn bpm(bpm: f64) -> Bpm {
    Bpm::new(bpm).expect("a fixture's tempos are valid")
}

/// A sine at `level` whose frequency moves in a straight line from `from` to
/// `to` hertz over `frames` frames at `rate`. Its sine is `libm`'s, so it's
/// the same to the bit on every platform.
fn chirp(frames: usize, rate: u32, from: f64, to: f64, level: f64) -> Vec<f32> {
    let mut phase = 0.0f64;
    let mut samples = Vec::with_capacity(frames);
    for i in 0..frames {
        samples.push((libm::sin(phase * std::f64::consts::TAU) * level) as f32);
        let hz = from + (to - from) * i as f64 / frames as f64;
        phase += hz / f64::from(rate);
        if phase >= 1.0 {
            phase -= 1.0;
        }
    }
    samples
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
    // Returns have no sends yet (WP-2.19b), so this one has a tone of its own.
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

    let lead_mixer = NodeKey::Mixer(lead);
    let pad_mixer = NodeKey::Mixer(pad);
    let tone = NodeKey::Device(tone);
    let commands = [
        ("1.3.1", lead_mixer, mixer::MUTE, 1.0),
        ("1.4.1", lead_mixer, mixer::MUTE, 0.0),
        ("1.4.3", pad_mixer, mixer::VOLUME, 0.5),
        ("2.1.1", tone, test_tone::FREQUENCY, 660.0),
        ("2.4.1", pad_mixer, mixer::VOLUME, 0.25),
        ("2.5.2", tone, test_tone::LEVEL, -36.0),
        ("3.1.1", tone, test_tone::FREQUENCY, 495.0),
        ("3.3.2", lead_mixer, mixer::VOLUME, 0.5),
        ("3.7.2", pad_mixer, mixer::MUTE, 1.0),
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

/// Spec: session-view R22, R23.
fn scene_tempo() -> Scene {
    let mut song = Song::new();
    song.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(120.0)),
    });
    let track = song.track(TrackKind::Midi, "Beat", None);
    song.beat_tone(track, 220.0, -24.0);
    let scene = song.editor.document().scenes().next().unwrap().id;
    song.apply(Command::SetSceneTempo {
        scene,
        tempo: Some(bpm(60.0)),
    });
    // Asked for a frame past 1.2.1: the 1 Bar grid puts the launch on
    // 2.1.1, frame 96,000, and the scene has no clips to launch.
    song.launch(24_001, SessionCommand::LaunchScene(scene));
    song.finish(4.0)
}

/// Spec: mixer R1 to R9.
fn pan_and_sends() -> Scene {
    let mut song = Song::new();
    let post = song.track(TrackKind::Return, "Post", None);
    song.volume(post, 0.5);
    let pre = song.track(TrackKind::Return, "Pre", None);
    song.apply(Command::SetSendTap {
        track: pre,
        tap: SendTap::Pre,
    });
    song.pan(pre, 0.6);

    // Hard left: +3 dB on the left, nothing on the right.
    let left = song.track(TrackKind::Midi, "Left", None);
    song.tone(left, 220.0, -24.0);
    song.pan(left, -1.0);
    song.send(left, post, 0.5);
    // Half right at -6 dB; its pre-fader send ignores both.
    let right = song.track(TrackKind::Midi, "Half right", None);
    song.tone(right, 330.0, -24.0);
    song.pan(right, 0.5);
    song.volume(right, gain_from_db(-6.0));
    song.send(right, pre, 0.25);
    // A group with its own volume and pan, holding a panned track.
    let bus = song.track(TrackKind::Group, "Bus", None);
    song.volume(bus, 0.8);
    song.pan(bus, -0.3);
    let inner = song.track(TrackKind::Midi, "Inner", Some(bus));
    song.tone(inner, 440.0, -30.0);
    song.pan(inner, 0.25);
    song.send(inner, post, 1.0);
    song.send(bus, post, 0.3);
    // Deactivated: silent on Main and through its post-fader send, heard
    // through its pre-fader send.
    let off = song.track(TrackKind::Audio, "Deactivated", None);
    song.tone(off, 550.0, -24.0);
    song.mute(off);
    song.send(off, pre, 0.7);
    song.send(off, post, 1.0);

    song.main_volume(0.9);
    let main = song.editor.document().main_id();
    song.pan(main, 0.1);
    song.finish(2.0)
}

/// A group of two tracks and a track beside it, each a tone sent to a
/// return panned to the right: (kick, hat, bass, return).
fn drums_and_bass(song: &mut Song) -> [TrackId; 4] {
    let verb = song.track(TrackKind::Return, "Verb", None);
    song.pan(verb, 0.8);
    song.volume(verb, 0.5);
    let drums = song.track(TrackKind::Group, "Drums", None);
    let kick = song.track(TrackKind::Audio, "Kick", Some(drums));
    song.tone(kick, 110.0, -24.0);
    song.send(kick, verb, 1.0);
    let hat = song.track(TrackKind::Audio, "Hat", Some(drums));
    song.tone(hat, 880.0, -30.0);
    song.send(hat, verb, 0.5);
    let bass = song.track(TrackKind::Midi, "Bass", None);
    song.tone(bass, 165.0, -24.0);
    song.send(bass, verb, 1.0);
    [kick, hat, bass, verb]
}

/// Spec: mixer R10, R11.
fn solo_in_place() -> Scene {
    let mut song = Song::new();
    let [kick, ..] = drums_and_bass(&mut song);
    song.solo(kick);
    song.finish(2.0)
}

/// Spec: mixer R11, R12.
fn solo_a_return() -> Scene {
    let mut song = Song::new();
    let [.., verb] = drums_and_bass(&mut song);
    song.apply(Command::SetSoloInPlace { enabled: false });
    song.solo(verb);
    song.finish(2.0)
}

/// Spec: mixer R18. Every move is a parameter change on its exact frame, as
/// a UI drag sends them; the steps are 1/32 beat (15.625 ms at 120 BPM)
/// apart, closer than the 20 ms a change takes, so glides run into each
/// other as they do under a hand on a fader.
fn fader_move() -> Scene {
    let mut song = Song::new();
    let lead = song.track(TrackKind::Midi, "Lead", None);
    song.tone(lead, 440.0, -12.0);
    let key = NodeKey::Mixer(lead);
    let step = Ticks::QUARTER / 32;
    // From beat 1.5: down 1 dB a step to -20 dB, then back up to 0 dB.
    let drag_from = Ticks::QUARTER + Ticks::QUARTER / 2;
    for n in 1..=40i32 {
        let db = -f64::from(20 - (n - 20).abs());
        let at = drag_from + step * i64::from(n);
        song.schedule(at, key, mixer::VOLUME, gain_from_db(db));
    }
    // A mute on beat 4 and an unmute half a beat later.
    song.schedule(Ticks::QUARTER * 3, key, mixer::MUTE, 1.0);
    let unmute = Ticks::QUARTER * 3 + Ticks::QUARTER / 2;
    song.schedule(unmute, key, mixer::MUTE, 0.0);
    // From beat 5: across from left to right in 32 steps.
    for n in 0..=32i32 {
        let pan = -1.0 + f64::from(n) / 16.0;
        let at = Ticks::QUARTER * 4 + step * i64::from(n);
        song.schedule(at, key, mixer::PAN, pan);
    }
    song.finish(3.25)
}

/// Spec: mixer R23. The crossfader swept from the A end to the B end with
/// a track on each side and one on neither: every move is a parameter
/// change on its exact frame, as a UI drag sends one, and the steps are
/// 1/32 beat (15.625 ms at 120 BPM) apart, closer than the 20 ms a change
/// takes, so the glides run into each other. The position reaches every
/// track's mixer node, as the engine's does.
fn crossfade() -> Scene {
    let mut song = Song::new();
    let left = song.track(TrackKind::Midi, "Left", None);
    song.tone(left, 220.0, -24.0);
    song.apply(Command::SetCrossfadeAssign {
        track: left,
        assign: CrossfadeAssign::A,
    });
    let right = song.track(TrackKind::Midi, "Right", None);
    song.tone(right, 330.0, -24.0);
    song.apply(Command::SetCrossfadeAssign {
        track: right,
        assign: CrossfadeAssign::B,
    });
    let through = song.track(TrackKind::Midi, "Through", None);
    song.tone(through, 660.0, -30.0);
    let step = Ticks::QUARTER / 32;
    for n in 0..=32i32 {
        let position = f64::from(n) / 32.0;
        let at = Ticks::QUARTER + step * i64::from(n);
        for track in [left, right, through] {
            song.schedule(at, NodeKey::Mixer(track), mixer::CROSSFADER, position);
        }
    }
    song.finish(2.0)
}

/// The fader drag bl-ui's mixer test makes through the window and the
/// shell (`fader_gesture` in `crates/ui/tests/mixer.rs`): the track's volume
/// after each 60 Hz frame of the drag.
pub const FADER_GESTURE: &str = include_str!("../../../../tests/fixtures/ui/fader-gesture.json");

/// The frames a [`FADER_GESTURE`] volume lands on: one 60 Hz frame apart,
/// from half a second in.
#[must_use]
pub fn fader_gesture_frames() -> Vec<(u64, f64)> {
    let gesture: serde_json::Value =
        serde_json::from_str(FADER_GESTURE).expect("the recorded gesture is JSON");
    let rate = gesture["frame_rate"].as_u64().expect("a frame rate");
    let per_frame = u64::from(SAMPLE_RATE.hz()) / rate;
    gesture["volumes"]
        .as_array()
        .expect("the volumes")
        .iter()
        .enumerate()
        .map(|(i, volume)| {
            (
                u64::from(SAMPLE_RATE.hz()) / 2 + per_frame * i as u64,
                volume.as_f64().expect("a volume"),
            )
        })
        .collect()
}

/// Spec: mixer-view R18. The recorded drag's volumes, each as the
/// `mixer.set_volume` it was, scheduled as the engine's volume change on its
/// frame.
fn fader_gesture() -> Scene {
    let mut song = Song::new();
    let lead = song.track(TrackKind::Midi, "Lead", None);
    song.tone(lead, 440.0, -12.0);
    let key = NodeKey::Mixer(lead);
    let tempo = song.editor.document().settings().tempo.clone();
    for (frame, volume) in fader_gesture_frames() {
        let at = tempo
            .seconds_to_ticks(SAMPLE_RATE.frames_to_seconds(bl_core::time::Frames(frame as i64)));
        song.schedule(at, key, mixer::VOLUME, volume);
    }
    song.finish(2.0)
}

/// Spec: audio-clips R1, R3. Clips start on 1.2.3 (frame 36,000) and 2.3.2
/// (frame 150,000), neither on a block's first frame.
fn clip_mid_bar() -> Scene {
    let mut song = Song::new();
    let chirp = song.chirp();
    let track = song.track(TrackKind::Audio, "Clips", None);
    // From 0.5 to 1.75 seconds into the file: 2.5 beats at 120 BPM, as long
    // as the clip.
    let middle = AudioData {
        start: 0.5,
        end: Some(1.75),
        ..AudioData::new(chirp)
    };
    let at = song.at("1.2.3");
    song.audio_clip(track, at, Ticks::QUARTER * 5 / 2, middle);
    // From two seconds in, cut off a beat later, mid-file.
    let late = AudioData {
        start: 2.0,
        ..AudioData::new(chirp)
    };
    let at = song.at("2.3.2");
    song.audio_clip(track, at, Ticks::QUARTER, late);
    song.finish(4.0)
}

/// Spec: audio-clips R4, R5. From 1.1.3, the clip plays 0.25 seconds of
/// pre-roll from its Start marker, then its 0.3-second brace over and over
/// until it ends, three seconds later.
fn looped_clip() -> Scene {
    let mut song = Song::new();
    let chirp = song.chirp();
    let track = song.track(TrackKind::Audio, "Loop", None);
    let looped = AudioData {
        start: 0.25,
        looping: true,
        loop_start: 0.5,
        loop_end: Some(0.8),
        gain: 0.5,
        ..AudioData::new(chirp)
    };
    let at = song.at("1.1.3");
    song.audio_clip(track, at, Ticks::QUARTER * 6, looped);
    song.finish(3.5)
}

/// Spec: audio-clips R2, R9, and the mixer under them. The drone is mono at
/// 44.1 kHz: it plays on both channels, converted to 48 kHz.
fn overlapping_clips() -> Scene {
    let mut song = Song::new();
    let chirp = song.chirp();
    let drone = song.drone();
    let first = song.track(TrackKind::Audio, "Chirp", None);
    song.audio_clip(
        first,
        Ticks::ZERO,
        Ticks::QUARTER * 5,
        AudioData::new(chirp),
    );
    let second = song.track(TrackKind::Audio, "Drone", None);
    song.volume(second, gain_from_db(-6.0));
    song.pan(second, 0.5);
    let late = AudioData {
        start: 0.1,
        ..AudioData::new(drone)
    };
    let at = song.at("1.3.1");
    song.audio_clip(second, at, Ticks::QUARTER * 5, late);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: audio-clips R5, R6, R7. Each clip starts and ends mid-waveform.
fn clip_fades() -> Scene {
    let mut song = Song::new();
    let chirp = song.chirp();
    let track = song.track(TrackKind::Audio, "Fades", None);
    let hard = AudioData {
        start: 0.3,
        ..AudioData::new(chirp)
    };
    let at = song.at("1.1.2");
    song.audio_clip(track, at, Ticks::QUARTER, hard);
    let edges = AudioData {
        start: 1.0,
        ..AudioData::new(chirp).with_edge_fades()
    };
    let at = song.at("1.3.1");
    song.audio_clip(track, at, Ticks::QUARTER, edges);
    let long = AudioData {
        start: 1.5,
        gain: 1.5,
        fade_in: 0.25,
        fade_out: 0.5,
        ..AudioData::new(chirp)
    };
    let at = song.at("2.1.1");
    song.audio_clip(track, at, Ticks::QUARTER * 2, long);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}

/// Spec: audio-clips R2, R4. 120 BPM for two beats, then a ramp to 90 BPM
/// over two beats, then 90 BPM.
fn clip_tempo_change() -> Scene {
    let mut song = Song::new();
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, bpm(120.0)),
        (Ticks::QUARTER * 2, bpm(120.0)),
        (Ticks::QUARTER * 4, bpm(90.0)),
    ])
    .expect("the tempo map is valid");
    song.apply(Command::SetTempo { tempo });
    let chirp = song.chirp();
    let across = song.track(TrackKind::Audio, "Across", None);
    let at = song.at("1.2.1");
    song.audio_clip(across, at, Ticks::QUARTER * 3, AudioData::new(chirp));
    let after = song.track(TrackKind::Audio, "After", None);
    let looped = AudioData {
        start: 2.0,
        looping: true,
        loop_start: 2.0,
        loop_end: Some(2.25),
        ..AudioData::new(chirp).with_edge_fades()
    };
    let at = song.at("2.1.1");
    song.audio_clip(after, at, Ticks::QUARTER * 2, looped);
    let seconds = song.to_last_clip();
    song.finish(seconds)
}
