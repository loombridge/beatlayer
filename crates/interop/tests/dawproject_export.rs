//! Writing a set as a DAWproject file and reading it back (WP-6.37; the
//! DAWproject spec, R65 to R82): a set built in code is exported and
//! imported again, and what the format carries must come back.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use bl_core::time::{
    Bpm, TempoMap, TempoSegment, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap,
};
use bl_interop::dawproject::{
    self, Context, ExportContext, Installed, MediaFiles, Plugins, read_container_strict,
};
use bl_model::automation::curve_value;
use bl_model::{
    AudioData, AutomationTarget, Breakpoint, Clip, ClipData, Command, ContentHash, Device,
    DeviceId, DeviceKind, DeviceRole, Document, Editor, EnvelopeKind, Input, Locator, MidiData,
    MixerParam, Monitor, NewTakeLane, Note, Output, ParamDescriptor, ParamId, ParamUnit,
    PluginFormat, PluginState, RackKind, SendTap, TakeClip, TakeClipData, TrackId, TrackKind,
    WarpMarker, WarpMode,
};
use proptest::prelude::*;

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-interop-export-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A 16-bit mono WAV file of a tone.
fn wav(seconds: f64, hertz: f64) -> Vec<u8> {
    let rate = 44_100u32;
    let frames = (seconds * f64::from(rate)) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + frames * 2).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(frames * 2).to_le_bytes());
    for frame in 0..frames {
        let phase = f64::from(frame) / f64::from(rate) * hertz * std::f64::consts::TAU;
        out.extend_from_slice(&((phase.sin() * 12_000.0) as i16).to_le_bytes());
    }
    out
}

/// The media of a set: where each file is.
struct Files(HashMap<ContentHash, PathBuf>);

impl MediaFiles for Files {
    fn path(&self, hash: ContentHash) -> Option<PathBuf> {
        self.0.get(&hash).cloned()
    }
}

/// A machine with a CLAP, a VST3 and an Audio Unit plug-in, each with the
/// parameters the automation names.
struct Machine {
    installed: Vec<Installed>,
}

impl Machine {
    fn new() -> Self {
        let plugin = |format, id: &str, name: &str, role| Installed {
            format,
            plugin_id: id.to_owned(),
            path: format!("/plug-ins/{name}"),
            name: name.to_owned(),
            vendor: Some("Acme".to_owned()),
            version: Some("1.2".to_owned()),
            role,
        };
        Self {
            installed: vec![
                plugin(
                    PluginFormat::Clap,
                    CLAP,
                    "Test Synth",
                    DeviceRole::Instrument,
                ),
                plugin(
                    PluginFormat::Vst3,
                    VST3,
                    "Test Verb",
                    DeviceRole::AudioEffect,
                ),
                plugin(
                    PluginFormat::AudioUnit,
                    AU,
                    "Test Delay",
                    DeviceRole::AudioEffect,
                ),
            ],
        }
    }
}

const CLAP: &str = "com.example.synth";
const VST3: &str = "01234567-89AB-CDEF-0123-456789ABCDEF";
const AU: &str = "aufx/dely/appl";

impl Plugins for Machine {
    fn installed(&self) -> &[Installed] {
        &self.installed
    }

    fn describe(&self, _plugin: &Installed) -> Result<Vec<ParamDescriptor>, String> {
        Ok(vec![
            ParamDescriptor::continuous(
                ParamId(0),
                "Cutoff",
                (20.0, 20_000.0),
                1_000.0,
                ParamUnit::Hertz,
            ),
            ParamDescriptor::toggle(ParamId(1), "Bypass", false),
        ])
    }
}

/// The numbers that vary from one set to the next.
#[derive(Clone, Debug)]
struct Spec {
    bpm: (u16, u16),
    signature: (u8, u8),
    volume: u32,
    pan: i32,
    send: u32,
    bend: i32,
    fade: u32,
    crossfade: u32,
    gain: u32,
    transpose: i32,
    detune: i32,
    notes: Vec<(u8, u32, u32, u8, u8)>,
    swatches: (u8, u8, u8, u8),
    loop_beats: u32,
    label: String,
}

fn specs() -> impl Strategy<Value = Spec> {
    (
        (60u16..180, 60u16..180),
        (1u8..12, prop::sample::select(vec![2u8, 4, 8])),
        (10u32..190, -100i32..100, 5u32..100, -100i32..100),
        (0u32..300, 20u32..200, 10u32..200),
        (-12i32..12, -50i32..50),
        prop::collection::vec((0u8..128, 0u32..32, 1u32..16, 1u8..128, 1u8..128), 1..8),
        (0u8..70, 0u8..70, 0u8..70, 0u8..70),
        2u32..6,
        "[a-zA-Z0-9 <>&\"'é漢]{1,10}",
    )
        .prop_map(
            |(
                bpm,
                signature,
                (volume, pan, send, bend),
                (fade, crossfade, gain),
                (transpose, detune),
                notes,
                swatches,
                loop_beats,
                label,
            )| {
                Spec {
                    bpm,
                    signature,
                    volume,
                    pan,
                    send,
                    bend,
                    fade,
                    crossfade,
                    gain,
                    transpose,
                    detune,
                    notes,
                    swatches,
                    loop_beats,
                    label,
                }
            },
        )
}

struct Built {
    editor: Editor,
}

impl Built {
    fn apply(&mut self, command: Command) {
        self.editor.apply(command).expect("the commands are valid");
    }

    fn track(&mut self, kind: TrackKind, name: &str, parent: Option<TrackId>) -> TrackId {
        let document = self.editor.document();
        let index = match (kind, parent) {
            (TrackKind::Return, _) => document.returns().len(),
            (_, Some(group)) => document.track(group).unwrap().children().len(),
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

    fn clip(
        &mut self,
        track: TrackId,
        start: Ticks,
        length: Ticks,
        name: &str,
        data: ClipData,
    ) -> Clip {
        let id = self.editor.new_id();
        let mut clip = Clip::new(id, start, length, data);
        clip.name = name.to_owned();
        self.apply(Command::CreateClip {
            track,
            clip: clip.clone(),
        });
        clip
    }

    fn device(&mut self, track: TrackId, device_type: &str, params: &[(ParamId, f64)]) -> DeviceId {
        let id: DeviceId = self.editor.new_id();
        let kind = DeviceKind::Native {
            device_type: device_type.to_owned(),
        };
        let types = bl_devices::device_types();
        let device = types
            .iter()
            .find(|t| t.kind == kind)
            .expect("a device type")
            .device(id);
        let index = self.editor.document().track(track).unwrap().devices().len();
        self.apply(Command::InsertDevice {
            track,
            index,
            device,
        });
        for (param, value) in params {
            self.apply(Command::SetDeviceParam {
                device: id,
                param: *param,
                value: *value,
            });
        }
        id
    }

    fn plugin(
        &mut self,
        track: TrackId,
        format: PluginFormat,
        plugin_id: &str,
        name: &str,
        role: DeviceRole,
        data: Vec<u8>,
    ) -> DeviceId {
        let id: DeviceId = self.editor.new_id();
        let mut device = Device::new(
            id,
            name,
            DeviceKind::Plugin {
                format,
                plugin_id: plugin_id.to_owned(),
            },
        )
        .with_role(role);
        device.plugin = Some(PluginState {
            path: format!("/plug-ins/{name}"),
            data,
        });
        let index = self.editor.document().track(track).unwrap().devices().len();
        self.apply(Command::InsertDevice {
            track,
            index,
            device,
        });
        id
    }
}

fn points(list: &[(i64, f64, f64)]) -> Vec<Breakpoint> {
    list.iter()
        .map(|&(at, value, bend)| Breakpoint {
            at: Ticks(at),
            value,
            bend,
        })
        .collect()
}

/// A set holding everything the format carries, with the numbers `spec`
/// gives, and the media it plays.
fn build(spec: &Spec, folder: &Path) -> (Document, Files) {
    let q = Ticks::QUARTER;
    let mut media = HashMap::new();
    let mut file = |name: &str, seconds: f64, hertz: f64| {
        let bytes = wav(seconds, hertz);
        let path = folder.join(name);
        std::fs::write(&path, &bytes).unwrap();
        let hash = ContentHash(*bl_media::ContentHash::of_bytes(&bytes).as_bytes());
        media.insert(hash, path);
        hash
    };
    let loop_file = file("loop.wav", 2.0, 220.0);
    let tone_file = file("tone.wav", 3.0, 330.0);
    let take_file = file("take.wav", 1.0, 440.0);

    let mut set = Built {
        editor: Editor::new(),
    };
    set.apply(Command::SetTempo {
        tempo: TempoMap::new([
            TempoSegment::Ramp {
                start: Ticks::ZERO,
                from: Bpm::new(f64::from(spec.bpm.0)).unwrap(),
                to: Bpm::new(f64::from(spec.bpm.1)).unwrap(),
            },
            TempoSegment::Constant {
                start: q * 8,
                bpm: Bpm::new(f64::from(spec.bpm.1)).unwrap(),
            },
            TempoSegment::Constant {
                start: q * 16,
                bpm: Bpm::new(f64::from(spec.bpm.0)).unwrap(),
            },
        ])
        .unwrap(),
    });
    let signature = TimeSignature::new(spec.signature.0, spec.signature.1).unwrap();
    set.apply(Command::SetTimeSignatures {
        time_signatures: TimeSignatureMap::new([
            TimeSignatureChange {
                at: Ticks::ZERO,
                signature: TimeSignature::FOUR_FOUR,
            },
            TimeSignatureChange {
                at: q * 16,
                signature,
            },
        ])
        .unwrap(),
    });
    for (index, name) in [spec.label.as_str(), "Chorus"].iter().enumerate() {
        let id = set.editor.new_id();
        set.apply(Command::AddLocator {
            locator: Locator::new(id, q * (4 * (index as i64 + 1)), (*name).to_owned()),
        });
    }

    let group = set.track(TrackKind::Group, "Band", None);
    let keys = set.track(TrackKind::Midi, "Keys", Some(group));
    let lead = set.track(TrackKind::Midi, "Lead", None);
    let looped = set.track(TrackKind::Audio, "Loop", Some(group));
    let drums = set.track(TrackKind::Audio, "Drums", None);
    let source = set.track(TrackKind::Audio, "Source", None);
    let bus = set.track(TrackKind::Audio, "Bus", None);
    let reverb = set.track(TrackKind::Return, "Reverb", None);
    let delay = set.track(TrackKind::Return, "Delay", None);
    let (a, b, c, d) = spec.swatches;
    set.apply(Command::SetTrackColour {
        tracks: vec![keys],
        colour: a,
    });
    set.apply(Command::SetTrackColour {
        tracks: vec![drums],
        colour: b,
    });

    // MIDI: notes of every kind of expression, a controller envelope, and a
    // loop.
    let notes: Vec<Note> = spec
        .notes
        .iter()
        .map(|&(key, start, length, velocity, release)| {
            let mut note = Note::new(
                key,
                Ticks::SIXTEENTH * i64::from(start),
                Ticks::SIXTEENTH * i64::from(length),
                velocity,
            );
            note.release_velocity = release;
            if length > 2 {
                note.pressure = points(&[(0, 10.0, 0.0), (Ticks::SIXTEENTH.get() * 2, 100.0, 0.0)]);
                note.slide = points(&[(0, 0.0, 0.0), (Ticks::SIXTEENTH.get(), 127.0, 0.0)]);
                note.pitch = points(&[(0, -2.0, 0.0), (Ticks::SIXTEENTH.get() * 2, 3.0, 0.0)]);
            }
            note
        })
        .collect();
    let mut midi = MidiData::new(notes);
    midi.start = Ticks::SIXTEENTH;
    midi.end = Some(q * 6);
    midi.looping = true;
    midi.loop_start = Ticks::SIXTEENTH * 2;
    midi.loop_end = Some(q * 5);
    let mut clip = set.clip(
        lead,
        q * 2,
        q * 8,
        &spec.label,
        ClipData::Midi(midi.clone()),
    );
    set.apply(Command::SetClipEnvelope {
        clip: clip.id,
        target: AutomationTarget::MidiCc { number: 74 },
        kind: EnvelopeKind::Modulation,
        points: points(&[
            (0, 0.0, 0.0),
            (q.get() * 2, 100.0, 0.0),
            (q.get() * 4, 20.0, 0.0),
        ]),
    });
    clip.colour = Some(c);
    set.clip(
        keys,
        Ticks::ZERO,
        q * 4,
        "Pad",
        ClipData::Midi(MidiData::new([Note::new(60, Ticks::ZERO, q, 90)])),
    );

    // Audio: warped, with fades and a crossfade, and a take lane.
    let warp = |hash: ContentHash, start: f64, looping: bool| AudioData {
        warp: true,
        warp_markers: vec![
            WarpMarker {
                time: start,
                beat: Ticks::ZERO,
            },
            WarpMarker {
                time: start + 1.0,
                beat: q * 2,
            },
        ],
        tempo: Some(2.0 / 1.0 * 60.0),
        warp_mode: WarpMode::ComplexPro,
        looping,
        loop_start: 0.0,
        loop_end: looping.then(|| q.get() as f64 * f64::from(spec.loop_beats)),
        gain: 0.5 + f64::from(spec.gain) / 1000.0,
        ..AudioData::new(hash)
    };
    let seconds = |ms: u32| f64::from(ms) / 1000.0;
    let first = set.clip(
        looped,
        Ticks::ZERO,
        q * 4,
        "Loop A",
        ClipData::Audio(AudioData {
            fade_in: seconds(spec.fade).min(0.2),
            ..warp(loop_file, 0.0, false)
        }),
    );
    let second = set.clip(
        looped,
        q * 4,
        q * 4,
        "Loop B",
        ClipData::Audio(warp(loop_file, 0.5, false)),
    );
    set.clip(
        looped,
        q * 12,
        q * 8,
        "Loop C",
        ClipData::Audio(warp(loop_file, 0.25, true)),
    );
    let length = f64::from(spec.crossfade) / 1000.0;
    set.apply(Command::SetCrossfade {
        track: looped,
        boundary: second.start,
        before: length / 2.0,
        after: length / 2.0,
        curve: 0.0,
    });
    let _ = first;
    set.apply(Command::AddTakeLane {
        track: looped,
        lane: NewTakeLane {
            id: None,
            name: "Take 2".to_owned(),
            clips: vec![TakeClip {
                id: None,
                name: "Retake".to_owned(),
                start: q * 2,
                length: q * 2,
                data: TakeClipData::Audio(AudioData::new(take_file)),
            }],
        },
        index: None,
    });

    // Unwarped clips: a transposed one with markers and a plain one.
    set.clip(
        drums,
        q,
        q * 4,
        "Beat",
        ClipData::Audio(AudioData {
            start: 0.25,
            end: Some(2.0),
            transpose: spec.transpose,
            detune: f64::from(spec.detune),
            fade_out: 0.1,
            ..AudioData::new(tone_file)
        }),
    );
    set.clip(
        source,
        Ticks::ZERO,
        q * 4,
        "Tone",
        ClipData::Audio(AudioData::new(tone_file)),
    );

    // The mixer and the routing.
    set.apply(Command::SetVolume {
        track: drums,
        volume: f64::from(spec.volume) / 100.0,
    });
    set.apply(Command::SetPan {
        track: drums,
        pan: f64::from(spec.pan) / 100.0,
    });
    set.apply(Command::SetMute {
        track: lead,
        mute: true,
    });
    set.apply(Command::SetSolo {
        track: source,
        solo: true,
    });
    set.apply(Command::SetSend {
        track: drums,
        to: reverb,
        level: f64::from(spec.send) / 100.0,
    });
    set.apply(Command::SetSend {
        track: looped,
        to: delay,
        level: 0.5,
    });
    set.apply(Command::SetSendTap {
        track: delay,
        tap: SendTap::Pre,
    });
    set.apply(Command::SetOutput {
        track: source,
        output: Output::Track { track: bus },
    });
    set.apply(Command::SetMonitor {
        track: bus,
        monitor: Monitor::In,
    });
    set.apply(Command::SetInput {
        track: bus,
        input: Input::None,
        channel: None,
    });

    // Devices: the plug-ins, the four generic devices, a native device and
    // a rack.
    let synth = set.plugin(
        keys,
        PluginFormat::Clap,
        CLAP,
        "Test Synth",
        DeviceRole::Instrument,
        b"clap-state\x00\x01".to_vec(),
    );
    let verb = set.plugin(
        looped,
        PluginFormat::Vst3,
        VST3,
        "Test Verb",
        DeviceRole::AudioEffect,
        bl_model::vst3_state::plugin_state(b"component", b"controller"),
    );
    set.plugin(
        drums,
        PluginFormat::AudioUnit,
        AU,
        "Test Delay",
        DeviceRole::AudioEffect,
        b"<?xml version=\"1.0\"?><plist version=\"1.0\"><dict/></plist>".to_vec(),
    );
    let compressor = set.device(
        drums,
        bl_devices::compressor::COMPRESSOR,
        &[
            (bl_devices::compressor::THRESHOLD, -18.0),
            (bl_devices::compressor::RATIO, 4.0),
            (bl_devices::compressor::ATTACK, 12.0),
        ],
    );
    set.device(
        drums,
        bl_devices::gate::GATE,
        &[(bl_devices::gate::THRESHOLD, -40.0)],
    );
    set.device(
        drums,
        bl_devices::limiter::LIMITER,
        &[(bl_devices::limiter::CEILING, -1.0)],
    );
    set.device(
        drums,
        bl_devices::eq::EQ,
        &[
            (bl_devices::eq::band_param(0, 3, bl_devices::eq::ON), 1.0),
            (
                bl_devices::eq::band_param(0, 3, bl_devices::eq::FREQ),
                1_200.0,
            ),
            (
                bl_devices::eq::band_param(0, 3, bl_devices::eq::GAIN_SLOT),
                4.0,
            ),
        ],
    );
    let utility = set.device(
        source,
        bl_devices::utility::UTILITY,
        &[(bl_devices::utility::WIDTH, 1.5)],
    );
    let rack: DeviceId = set.editor.new_id();
    let rack_device = Device::rack(rack, RackKind::AudioEffect);
    set.apply(Command::InsertDevice {
        track: bus,
        index: 0,
        device: rack_device,
    });
    set.apply(Command::SetDeviceParam {
        device: rack,
        param: bl_model::rack::macro_param(1),
        value: 64.0,
    });

    // Automation: a curve, steps, a switch, a send, a device and a plug-in.
    let bar = q.get() * 4;
    set.apply(Command::SetAutomationEnvelope {
        track: drums,
        target: AutomationTarget::Mixer {
            param: MixerParam::Volume,
        },
        points: points(&[
            (0, 0.2, f64::from(spec.bend) / 100.0),
            (bar, 1.2, -0.5),
            (bar * 2, 0.7, 0.0),
        ]),
    });
    set.apply(Command::SetAutomationEnvelope {
        track: drums,
        target: AutomationTarget::Mixer {
            param: MixerParam::Pan,
        },
        points: points(&[
            (0, -0.5, 0.0),
            (bar - 1, -0.5, 0.0),
            (bar, 0.5, 0.0),
            (bar * 2, 0.5, 0.0),
        ]),
    });
    set.apply(Command::SetAutomationEnvelope {
        track: lead,
        target: AutomationTarget::Mixer {
            param: MixerParam::Mute,
        },
        points: points(&[(0, 0.0, 0.0), (bar - 1, 0.0, 0.0), (bar, 1.0, 0.0)]),
    });
    set.apply(Command::SetAutomationEnvelope {
        track: drums,
        target: AutomationTarget::Send { to: reverb },
        points: points(&[(0, 0.1, 0.0), (bar, 0.9, 0.3)]),
    });
    set.apply(Command::SetAutomationEnvelope {
        track: drums,
        target: AutomationTarget::Device {
            device: compressor,
            param: bl_devices::compressor::THRESHOLD,
        },
        points: points(&[(0, -30.0, 0.0), (bar, -6.0, 0.7)]),
    });
    set.apply(Command::SetAutomationEnvelope {
        track: source,
        target: AutomationTarget::Device {
            device: utility,
            param: bl_devices::utility::WIDTH,
        },
        points: points(&[(0, 0.5, 0.0), (bar, 1.5, -0.4)]),
    });
    set.apply(Command::SetAutomationEnvelope {
        track: looped,
        target: AutomationTarget::Device {
            device: verb,
            param: ParamId(0),
        },
        points: points(&[(0, 100.0, 0.0), (bar, 15_000.0, 0.6)]),
    });
    let _ = synth;

    // The Session View: names, colours, a clip with automation, a slot
    // without its Stop button.
    let scenes: Vec<_> = set.editor.document().scenes().map(|s| s.id).collect();
    set.apply(Command::RenameScene {
        scene: scenes[0],
        name: spec.label.clone(),
    });
    set.apply(Command::SetSceneColour {
        scenes: vec![scenes[1]],
        colour: Some(d),
    });
    let slot_clip = {
        let id = set.editor.new_id();
        let mut clip = Clip::new(
            id,
            Ticks::ZERO,
            q * 4,
            ClipData::Audio(AudioData {
                looping: true,
                loop_start: 0.25,
                loop_end: Some(1.25),
                ..AudioData::new(tone_file)
            }),
        );
        clip.name = "Hit".to_owned();
        clip
    };
    set.apply(Command::CreateSlotClip {
        track: drums,
        scene: scenes[0],
        clip: slot_clip.clone(),
    });
    set.apply(Command::SetClipEnvelope {
        clip: slot_clip.id,
        target: AutomationTarget::Mixer {
            param: MixerParam::Volume,
        },
        kind: EnvelopeKind::Automation,
        points: points(&[(0, 0.4, 0.0), (q.get() * 2, 0.9, 0.0)]),
    });
    let slot_midi = {
        let id = set.editor.new_id();
        Clip::new(
            id,
            Ticks::ZERO,
            q * 2,
            ClipData::Midi(MidiData::new([Note::new(64, Ticks::ZERO, q, 100)])),
        )
    };
    set.apply(Command::CreateSlotClip {
        track: lead,
        scene: scenes[1],
        clip: slot_midi,
    });
    set.apply(Command::SetSlotStopButton {
        track: keys,
        scene: scenes[2],
        stop_button: false,
    });
    (set.editor.document().clone(), Files(media))
}

fn export_of(
    document: &Document,
    files: &Files,
    machine: &Machine,
    embed: bool,
) -> dawproject::Exported {
    let context = ExportContext {
        application_version: "0.2.0",
        title: "Test Set",
        track_colours: &bl_model::DEFAULT_TRACK_PALETTE,
        embed_media: embed,
        media: files,
        plugins: machine,
    };
    dawproject::export(document, &context).expect("the set exports")
}

fn import_of(bytes: &[u8], folder: &Path, machine: &Machine) -> dawproject::Imported {
    let container = read_container_strict(bytes).expect("the file reads in strict mode");
    let mut context = Context::new(folder, folder);
    context.plugins = machine;
    dawproject::import(&container, &context).expect("the file imports")
}

fn by_name(document: &Document) -> BTreeMap<String, &bl_model::Track> {
    document
        .tracks()
        .map(|track| (track.name().to_owned(), track))
        .collect()
}

/// The value of an envelope at a tick, with its segments' bends.
fn value_at(points: &[Breakpoint], at: i64) -> f64 {
    let after = points.partition_point(|p| p.at.get() <= at);
    match (after.checked_sub(1).map(|i| points[i]), points.get(after)) {
        (None, Some(first)) => first.value,
        (Some(last), None) => last.value,
        (Some(a), Some(b)) => {
            let along = (at - a.at.get()) as f64 / (b.at - a.at).get() as f64;
            curve_value(a.value, b.value, along, a.bend)
        }
        (None, None) => 0.0,
    }
}

/// The range a target's values span, which a segment's error is measured
/// against.
fn range_of(target: &AutomationTarget, document: &Document, track: &bl_model::Track) -> f64 {
    match target {
        AutomationTarget::Mixer {
            param: MixerParam::Volume,
        } => bl_model::fader::db_gain(bl_model::fader::TOP_DB),
        AutomationTarget::Mixer {
            param: MixerParam::Pan,
        } => 2.0,
        AutomationTarget::Send { .. } => 1.0,
        AutomationTarget::Device { device, param } => {
            let device = track.find_device(*device).expect("the device");
            let types = bl_devices::device_types();
            match types.iter().find(|t| t.kind == device.kind) {
                Some(kind) => {
                    let descriptor = kind.description.param(*param).expect("a parameter");
                    descriptor.max() - descriptor.min()
                }
                None => 19_980.0,
            }
        }
        _ => {
            let _ = document;
            1.0
        }
    }
}

/// What differs between a set and the one its export imports as, in
/// everything the format carries (R82).
fn differences(original: &Document, imported: &Document) -> Vec<String> {
    let mut found = Vec::new();
    let (left, right) = (by_name(original), by_name(imported));
    if left.keys().ne(right.keys()) {
        found.push(format!(
            "the tracks differ: {:?} against {:?}",
            left.keys(),
            right.keys()
        ));
        return found;
    }
    // The tracks' order, groups and returns.
    let order = |document: &Document| -> Vec<(String, Option<String>)> {
        document
            .track_order()
            .into_iter()
            .map(|id| {
                let track = document.track(id).unwrap();
                (
                    track.name().to_owned(),
                    document
                        .parent(id)
                        .and_then(|group| document.track(group))
                        .map(|group| group.name().to_owned()),
                )
            })
            .collect()
    };
    if order(original) != order(imported) {
        found.push(format!(
            "the order differs: {:?} against {:?}",
            order(original),
            order(imported)
        ));
    }
    let palette = bl_model::DEFAULT_TRACK_PALETTE;
    let rgb = |swatch: u8| bl_model::swatch_rgb(&palette, swatch);
    for (name, a) in &left {
        let b = right[name];
        let at = |what: &str| format!("{name}: {what}");
        if a.kind() != b.kind() {
            found.push(at("kind"));
        }
        if rgb(a.colour()) != rgb(b.colour()) {
            found.push(at("colour"));
        }
        let (ma, mb) = (a.mixer(), b.mixer());
        if a.kind() != TrackKind::Main {
            if (ma.volume - mb.volume).abs() > 1e-12
                || (ma.pan - mb.pan).abs() > 1e-12
                || ma.mute != mb.mute
                || ma.solo != mb.solo
            {
                found.push(at(&format!("mixer {ma:?} against {mb:?}")));
            }
        } else if (ma.volume - mb.volume).abs() > 1e-12 || (ma.pan - mb.pan).abs() > 1e-12 {
            found.push(at("Main's mixer"));
        }
        let sends = |document: &Document, track: &bl_model::Track| -> Vec<(String, f64)> {
            track
                .sends()
                .map(|(to, level)| (document.track(to).unwrap().name().to_owned(), level))
                .collect()
        };
        if sends(original, a) != sends(imported, b) {
            found.push(at("sends"));
        }
        if a.kind() == TrackKind::Return && ma.tap != mb.tap {
            found.push(at("send tap"));
        }
        if track_output(original, a) != track_output(imported, b) {
            found.push(at("output"));
        }
        clips_differ(original, imported, a, b, &mut found, name);
        devices_differ(a, b, &mut found, name);
        automation_differs(original, imported, a, b, &mut found, name);
        // Take lanes.
        let lanes = |track: &bl_model::Track| -> Vec<(String, usize)> {
            track
                .take_lanes()
                .map(|lane| (lane.name.clone(), lane.clips.len()))
                .collect()
        };
        if lanes(a) != lanes(b) {
            found.push(format!("{name}: take lanes"));
        }
        // Session slots.
        let slots =
            |document: &Document, track: &bl_model::Track| -> Vec<(usize, Option<String>, bool)> {
                document
                    .scenes()
                    .enumerate()
                    .map(|(index, scene)| {
                        (
                            index,
                            track.slot(scene.id).map(|clip| clip.name.clone()),
                            track.has_stop_button(scene.id),
                        )
                    })
                    .collect()
            };
        if track_holds(a) && slots(original, a) != slots(imported, b) {
            found.push(format!(
                "{name}: slots {:?} against {:?}",
                slots(original, a),
                slots(imported, b)
            ));
        }
    }
    // Scenes.
    let scenes = |document: &Document| -> Vec<(String, Option<[u8; 3]>)> {
        document
            .scenes()
            .map(|scene| (scene.name.clone(), scene.colour.map(rgb)))
            .collect()
    };
    if scenes(original) != scenes(imported) {
        found.push("the scenes differ".to_owned());
    }
    if original.tempo_map().segments().collect::<Vec<_>>()
        != imported.tempo_map().segments().collect::<Vec<_>>()
    {
        found.push(format!(
            "the tempo map differs: {:?} against {:?}",
            original.tempo_map().segments().collect::<Vec<_>>(),
            imported.tempo_map().segments().collect::<Vec<_>>()
        ));
    }
    if original.settings().time_signatures != imported.settings().time_signatures {
        found.push("the time signatures differ".to_owned());
    }
    let locators = |document: &Document| -> Vec<(Ticks, String)> {
        document
            .locators()
            .iter()
            .map(|l| (l.position, l.name.clone()))
            .collect()
    };
    if locators(original) != locators(imported) {
        found.push("the locators differ".to_owned());
    }
    found
}

fn track_output(document: &Document, track: &bl_model::Track) -> String {
    match track.routing().output {
        Output::Track { track: other } => document.track(other).unwrap().name().to_owned(),
        _ => "default".to_owned(),
    }
}

fn track_holds(track: &bl_model::Track) -> bool {
    track.kind().holds_clips()
}

fn clips_differ(
    original: &Document,
    imported: &Document,
    a: &bl_model::Track,
    b: &bl_model::Track,
    found: &mut Vec<String>,
    name: &str,
) {
    let palette = bl_model::DEFAULT_TRACK_PALETTE;
    let rgb = |swatch: u8| bl_model::swatch_rgb(&palette, swatch);
    let mut left: Vec<&Clip> = a.clips().collect();
    let mut right: Vec<&Clip> = b.clips().collect();
    left.sort_by_key(|c| c.start);
    right.sort_by_key(|c| c.start);
    if left.len() != right.len() {
        found.push(format!(
            "{name}: {} clips against {}",
            left.len(),
            right.len()
        ));
        return;
    }
    for (x, y) in left.iter().zip(&right) {
        let at = |what: &str| format!("{name}/{}: {what}", x.name);
        if (x.name.as_str(), x.start, x.length, x.mute)
            != (y.name.as_str(), y.start, y.length, y.mute)
        {
            found.push(at("place"));
        }
        if rgb(x.colour.unwrap_or(a.colour())) != rgb(y.colour.unwrap_or(b.colour())) {
            found.push(at("colour"));
        }
        clip_data_differs(&x.data, &y.data, found, &at(""));
    }
    let crossfades = |track: &bl_model::Track| -> Vec<(Ticks, f64)> {
        bl_model::crossfades(track.clips())
            .into_iter()
            .map(|c| (c.boundary, c.before + c.after))
            .collect()
    };
    let (ca, cb) = (crossfades(a), crossfades(b));
    if ca.len() != cb.len()
        || ca
            .iter()
            .zip(&cb)
            .any(|(x, y)| x.0 != y.0 || (x.1 - y.1).abs() > 1e-9)
    {
        found.push(format!("{name}: crossfades {ca:?} against {cb:?}"));
    }
    for lane in a.take_lanes() {
        let Some(other) = b.take_lanes().find(|l| l.name == lane.name) else {
            continue;
        };
        for (x, y) in lane.clips.iter().zip(&other.clips) {
            clip_data_differs(&x.data, &y.data, found, &format!("{name}/{}", lane.name));
        }
    }
    // The Session clips.
    for scene in original.scenes() {
        if let Some(x) = a.slot(scene.id) {
            let index = original.scene_position(scene.id).unwrap();
            let other = imported.scenes().nth(index).unwrap().id;
            let Some(y) = b.slot(other) else {
                found.push(format!("{name}: slot {index} is empty"));
                continue;
            };
            clip_data_differs(&x.data, &y.data, found, &format!("{name}/slot {index}"));
            for held in x
                .envelopes
                .iter()
                .filter(|e| e.kind == EnvelopeKind::Automation)
            {
                let other = y
                    .envelopes
                    .iter()
                    .find(|e| e.kind == EnvelopeKind::Automation && e.target == held.target);
                match other {
                    Some(other) => {
                        let (p, q): (Vec<_>, Vec<_>) = (
                            held.envelope.points().collect(),
                            other.envelope.points().collect(),
                        );
                        for tick in (0..q.last().map_or(0, |l| l.at.get())).step_by(23_131) {
                            if (value_at(&p, tick) - value_at(&q, tick)).abs() > 0.02 {
                                found.push(format!("{name}: slot automation at {tick}"));
                                break;
                            }
                        }
                    }
                    None => found.push(format!("{name}: slot automation is gone")),
                }
            }
        }
    }
}

fn clip_data_differs(x: &ClipData, y: &ClipData, found: &mut Vec<String>, at: &str) {
    match (x, y) {
        (ClipData::Midi(x), ClipData::Midi(y)) => {
            let notes = |data: &MidiData| -> Vec<(u8, Ticks, Ticks, u8, u8)> {
                let mut notes: Vec<_> = data
                    .notes
                    .iter()
                    .map(|n| (n.key, n.start, n.length, n.velocity, n.release_velocity))
                    .collect();
                notes.sort();
                notes
            };
            if notes(x) != notes(y) {
                found.push(format!("{at} notes {:?} against {:?}", notes(x), notes(y)));
            }
            if (x.start, x.end, x.looping, x.loop_start, x.loop_end)
                != (y.start, y.end, y.looping, y.loop_start, y.loop_end)
            {
                found.push(format!("{at} markers"));
            }
            // Each note's expressions, at their breakpoints.
            let expressions = |data: &MidiData| -> Vec<(Ticks, usize, usize, usize)> {
                let mut list: Vec<_> = data
                    .notes
                    .iter()
                    .map(|n| {
                        (
                            n.start,
                            n.pitch.len().min(1),
                            n.slide.len().min(1),
                            n.pressure.len().min(1),
                        )
                    })
                    .collect();
                list.sort();
                list
            };
            if expressions(x) != expressions(y) {
                found.push(format!("{at} note expressions"));
            }
        }
        (ClipData::Audio(x), ClipData::Audio(y)) => {
            let same = x.media == y.media
                && x.start == y.start
                && x.end == y.end
                && x.looping == y.looping
                && x.loop_start == y.loop_start
                && (x.loop_end == y.loop_end || x.loop_end.is_none())
                && (x.gain - y.gain).abs() < 1e-9
                && (x.fade_in - y.fade_in).abs() < 1e-9
                && x.warp == y.warp
                && x.warp_markers == y.warp_markers
                && x.transpose == y.transpose
                && (x.detune - y.detune).abs() < 1e-9;
            if !same {
                found.push(format!("{at} audio {x:?} against {y:?}"));
            }
            if x.warp
                && (x.warp_mode != y.warp_mode
                    || (x.tempo.unwrap() - y.tempo.unwrap()).abs() > 1e-9)
            {
                found.push(format!("{at} warp"));
            }
        }
        _ => found.push(format!("{at} kind")),
    }
}

fn devices_differ(a: &bl_model::Track, b: &bl_model::Track, found: &mut Vec<String>, name: &str) {
    let list = |track: &bl_model::Track| -> String {
        let devices: Vec<_> = track
            .devices()
            .map(|d| {
                (
                    d.kind.clone(),
                    d.name.clone(),
                    d.role,
                    d.enabled,
                    d.params
                        .iter()
                        .map(|(p, v)| (*p, v.to_bits()))
                        .collect::<Vec<_>>(),
                    d.plugin.as_ref().map(|p| p.data.clone()),
                )
            })
            .collect();
        format!("{devices:#?}")
    };
    if list(a) != list(b) {
        found.push(format!("{name}: devices {} against {}", list(a), list(b)));
    }
}

fn automation_differs(
    original: &Document,
    imported: &Document,
    a: &bl_model::Track,
    b: &bl_model::Track,
    found: &mut Vec<String>,
    name: &str,
) {
    for (target, held) in a.automation() {
        // The same target on the imported track: devices and returns have
        // new IDs, found by their place.
        let other = match target {
            AutomationTarget::Send { to } => {
                let wanted = original.track(*to).unwrap().name();
                b.automation().find(|(t, _)| {
                    matches!(t, AutomationTarget::Send { to } if imported.track(*to).unwrap().name() == wanted)
                })
            }
            AutomationTarget::Device { device, param } => {
                let at = a.device_position(*device).unwrap();
                let there = b.devices().nth(at).map(|d| d.id);
                b.automation().find(|(t, _)| matches!(t, AutomationTarget::Device { device, param: p } if Some(*device) == there && p == param))
            }
            other => b.automation().find(|(t, _)| *t == other),
        };
        let Some((_, other)) = other else {
            found.push(format!("{name}: automation of {target:?} is gone"));
            continue;
        };
        let (p, q): (Vec<_>, Vec<_>) = (
            held.envelope.points().collect(),
            other.envelope.points().collect(),
        );
        let range = range_of(target, original, a);
        let end = p.last().map_or(0, |l| l.at.get());
        let mut worst: f64 = 0.0;
        for tick in (0..=end + 1_000).step_by(997) {
            let (x, y) = (value_at(&p, tick), value_at(&q, tick));
            // A switch is read as on or off.
            let gap = if matches!(
                target,
                AutomationTarget::Mixer {
                    param: MixerParam::Mute
                }
            ) {
                f64::from(u8::from((x > 0.5) != (y > 0.5)))
            } else {
                (x - y).abs() / range
            };
            worst = worst.max(gap);
        }
        if worst > 0.0100001 {
            found.push(format!("{name}: automation of {target:?} strays {worst}"));
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    // Spec: dawproject R65 to R82. A set exports and imports as the same
    // set in everything the format carries, two exports of it are the same
    // bytes, and the file reads in strict mode.
    #[test]
    fn a_set_exports_and_imports_as_the_same_set(spec in specs()) {
        let folder = TempDir::new(&format!("property-{}", std::process::id()));
        let (document, files) = build(&spec, &folder.0);
        let machine = Machine::new();
        let first = export_of(&document, &files, &machine, true);
        let second = export_of(&document, &files, &machine, true);
        prop_assert!(first.bytes == second.bytes, "two exports differ");
        let imported = import_of(&first.bytes, &folder.0, &machine);
        let found = differences(&document, &imported.project.document);
        prop_assert!(found.is_empty(), "{found:#?}");
    }
}

/// The text of `project.xml` in a file.
fn project_text(bytes: &[u8]) -> String {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("a ZIP file");
    let mut text = String::new();
    archive
        .by_name("project.xml")
        .expect("project.xml")
        .read_to_string(&mut text)
        .expect("UTF-8");
    text
}

fn default_spec() -> Spec {
    Spec {
        bpm: (90, 120),
        signature: (7, 8),
        volume: 80,
        pan: 25,
        send: 40,
        bend: 50,
        fade: 20,
        crossfade: 50,
        gain: 100,
        transpose: 3,
        detune: 10,
        notes: vec![(60, 0, 4, 100, 64), (64, 4, 8, 90, 70)],
        swatches: (1, 2, 3, 4),
        loop_beats: 3,
        label: "Hook".to_owned(),
    }
}

/// Spec: dawproject R66. The container holds `metadata.xml`, `project.xml`,
/// the plug-ins' states and the media in that order, every entry with the
/// time stamp 1 January 1980, the XML deflated and the rest stored.
#[test]
fn the_container_holds_its_entries_in_the_order_the_spec_gives() {
    let folder = TempDir::new("container");
    let (document, files) = build(&default_spec(), &folder.0);
    let exported = export_of(&document, &files, &Machine::new(), true);
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&exported.bytes)).unwrap();
    let names: Vec<String> = archive
        .file_names()
        .map(|name| name.unwrap().into_owned())
        .collect();
    assert_eq!(&names[..2], ["metadata.xml", "project.xml"]);
    let first_audio = names.iter().position(|n| n.starts_with("audio/")).unwrap();
    assert!(
        names[2..first_audio]
            .iter()
            .all(|n| n.starts_with("plugins/"))
    );
    assert!(names[first_audio..].iter().all(|n| n.starts_with("audio/")));
    for index in 0..archive.len() {
        let entry = archive.by_index(index).unwrap();
        let entry_name = entry.name().unwrap().into_owned();
        let time = entry.last_modified().unwrap();
        assert_eq!(
            (
                time.year(),
                time.month(),
                time.day(),
                time.hour(),
                time.minute(),
                time.second()
            ),
            (1980, 1, 1, 0, 0, 0),
            "{entry_name}"
        );
        let xml = entry_name.ends_with(".xml");
        assert_eq!(
            entry.compression(),
            if xml {
                zip::CompressionMethod::Deflated
            } else {
                zip::CompressionMethod::Stored
            },
            "{entry_name}"
        );
    }
    // The text is UTF-8 without a byte-order mark.
    let text = project_text(&exported.bytes);
    assert!(text.starts_with("<?xml"));
}

/// Spec: dawproject R67, R68, R69. The structure holds the tracks and groups
/// in order, then the returns, then Main; each channel has its mixer,
/// destination and sends.
#[test]
fn the_structure_mixer_and_routing_are_written_as_the_spec_says() {
    let folder = TempDir::new("structure");
    let (document, files) = build(&default_spec(), &folder.0);
    let exported = export_of(&document, &files, &Machine::new(), true);
    let container = read_container_strict(&exported.bytes).unwrap();
    let project = &container.project;
    assert_eq!(project.version.as_deref(), Some("1.0"));
    let application = project.application.as_ref().unwrap();
    assert_eq!(application.name.as_deref(), Some("Beatlayer"));
    assert_eq!(application.version.as_deref(), Some("0.2.0"));
    let tracks: Vec<&dawproject::model::Track> = project
        .structure
        .iter()
        .map(|item| match item {
            dawproject::model::Structure::Track(track) => track,
            dawproject::model::Structure::Channel(_) => panic!("a bare channel"),
        })
        .collect();
    let names: Vec<_> = tracks
        .iter()
        .map(|t| t.info.name.clone().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Band", "Lead", "Drums", "Source", "Bus", "Reverb", "Delay", "Main"
        ]
    );
    use dawproject::model::{ContentKind, MixerRole};
    let role = |track: &dawproject::model::Track| track.channel.as_ref().unwrap().role.unwrap();
    assert_eq!(role(tracks[0]), MixerRole::Submix);
    assert_eq!(tracks[0].content_types, [ContentKind::Tracks]);
    assert_eq!(tracks[0].children.len(), 2);
    assert_eq!(tracks[0].children[0].content_types, [ContentKind::Notes]);
    assert_eq!(tracks[0].children[1].content_types, [ContentKind::Audio]);
    assert_eq!(role(tracks[5]), MixerRole::Effect);
    assert_eq!(role(tracks[7]), MixerRole::Master);
    // The mixer: linear volume, normalised pan, mute and solo, two channels.
    let drums = tracks[2].channel.as_ref().unwrap();
    let volume = drums.volume.as_ref().unwrap();
    assert_eq!(volume.unit, Some(dawproject::model::Unit::Linear));
    assert_eq!(volume.value, Some(0.8));
    let pan = drums.pan.as_ref().unwrap();
    assert_eq!(pan.unit, Some(dawproject::model::Unit::Normalized));
    assert_eq!((pan.min, pan.max), (Some(0.0), Some(1.0)));
    assert_eq!(pan.value, Some((0.25 + 1.0) / 2.0));
    assert_eq!(drums.audio_channels, Some(2));
    assert_eq!(
        tracks[1]
            .channel
            .as_ref()
            .unwrap()
            .mute
            .as_ref()
            .unwrap()
            .value,
        Some(true)
    );
    assert_eq!(tracks[3].channel.as_ref().unwrap().solo, Some(true));
    // The destination: the group's channel, Main's, or the channel of the
    // track whose Track In it feeds.
    let channel_id =
        |track: &dawproject::model::Track| track.channel.as_ref().unwrap().info.id.clone();
    let destination =
        |track: &dawproject::model::Track| track.channel.as_ref().unwrap().destination.clone();
    assert_eq!(destination(&tracks[0].children[0]), channel_id(tracks[0]));
    assert_eq!(destination(tracks[1]), channel_id(tracks[7]));
    assert_eq!(destination(tracks[3]), channel_id(tracks[4]));
    // Sends: linear level, pre or post from the return's switch.
    let send = &drums.sends[0];
    assert_eq!(send.destination, channel_id(tracks[5]));
    assert_eq!(send.send_type.as_deref(), Some("post"));
    assert_eq!(send.volume.as_ref().unwrap().value, Some(0.4));
    let delayed = &tracks[0].children[1].channel.as_ref().unwrap().sends[0];
    assert_eq!(delayed.send_type.as_deref(), Some("pre"));
    // A track routed to Sends Only or Ext. Out writes no destination of its
    // own kind and the report lists it.
}

/// Spec: dawproject R68. An output to Sends Only or Ext. Out is listed.
#[test]
fn an_output_to_sends_only_or_ext_out_is_listed() {
    let mut set = Built {
        editor: Editor::new(),
    };
    let a = set.track(TrackKind::Audio, "A", None);
    let b = set.track(TrackKind::Audio, "B", None);
    set.apply(Command::SetOutput {
        track: a,
        output: Output::SendsOnly,
    });
    set.apply(Command::SetOutput {
        track: b,
        output: Output::External {
            channel: bl_model::Channel::Pair(1),
        },
    });
    let folder = TempDir::new("outputs");
    let exported = export_of(
        set.editor.document(),
        &Files(HashMap::new()),
        &Machine::new(),
        true,
    );
    let _ = folder;
    let listed: Vec<_> = exported
        .report
        .entries
        .iter()
        .filter(|e| e.what == "an output to Sends Only or Ext. Out")
        .map(|e| e.place.clone())
        .collect();
    assert_eq!(listed, ["the track \"A\"", "the track \"B\""]);
}

/// Spec: dawproject R70, R71, R72, R74. Clips, notes, warps, fades,
/// take lanes and slots are written as the spec says.
#[test]
fn clips_notes_warps_fades_take_lanes_and_slots_are_written_as_the_spec_says() {
    let folder = TempDir::new("clips");
    let (document, files) = build(&default_spec(), &folder.0);
    let exported = export_of(&document, &files, &Machine::new(), true);
    let container = read_container_strict(&exported.bytes).unwrap();
    let text = project_text(&exported.bytes);
    // A MIDI clip: place, markers and loop in beats, notes from the clip's
    // contents with velocities over 127 and channel 0, muted notes left out.
    assert!(text.contains(
        "<Clip name=\"Hook\" color=\"#e0675a\" time=\"2\" duration=\"8\" contentTimeUnit=\"beats\" \
         playStart=\"0.25\" playStop=\"6\" loopStart=\"0.5\" loopEnd=\"5\">"
    ), "{text}");
    assert!(text.contains("channel=\"0\" key=\"60\""));
    // A warped clip: a Warps timeline of seconds with one Warp per marker and
    // the algorithm named as the reader reads it.
    assert!(text.contains("<Warps contentTimeUnit=\"seconds\">"));
    assert!(text.contains("algorithm=\"complex pro\""));
    assert!(text.contains("<Warp time=\"2\" contentTime=\"1\"/>"));
    // Fades in seconds; a crossfade is a negative fade-in of its length.
    assert!(text.contains("fadeInTime=\"0.02\""), "{text}");
    assert!(text.contains("fadeInTime=\"-0.05\""), "{text}");
    // A gain other than 0 dB is an expression with one point.
    assert!(text.contains("<Target expression=\"gain\"/>"));
    // The take lane is a further timeline of clips, named, after the lane.
    assert!(text.contains("<Clips name=\"Take 2\" timeUnit=\"beats\">"));
    // The Audio element names the file with its own rate, channels and
    // length.
    assert!(
        text.contains(
            "duration=\"2\" algorithm=\"complex pro\" channels=\"1\" sampleRate=\"44100\""
        )
    );
    // Scenes: names, colours, slots scoped to the clip-holding tracks, and a
    // slot whose Stop button is gone.
    let scenes = &container.project.scenes;
    assert_eq!(scenes.len(), 8);
    assert_eq!(scenes[0].info.name.as_deref(), Some("Hook"));
    assert!(scenes[1].info.color.is_some());
    assert!(scenes[2].info.color.is_none());
    assert!(text.contains("hasStop=\"false\""));
}

/// Spec: dawproject R73. The media go in `audio/` under their own names, a
/// clash gets " 2", and a medium that is missing leaves out its clips,
/// listed; without `embed_media` each is an external reference.
#[test]
fn media_are_embedded_with_free_names_or_referred_to_and_a_missing_one_is_listed() {
    let folder = TempDir::new("media");
    let one = folder.0.join("one");
    let two = folder.0.join("two");
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    let (a, b) = (wav(1.0, 200.0), wav(1.0, 300.0));
    std::fs::write(one.join("tone.wav"), &a).unwrap();
    std::fs::write(two.join("tone.wav"), &b).unwrap();
    let hash = |bytes: &[u8]| ContentHash(*bl_media::ContentHash::of_bytes(bytes).as_bytes());
    let missing = ContentHash([9; 32]);
    let files = Files(HashMap::from([
        (hash(&a), one.join("tone.wav")),
        (hash(&b), two.join("tone.wav")),
    ]));
    let mut set = Built {
        editor: Editor::new(),
    };
    let track = set.track(TrackKind::Audio, "Audio", None);
    for (index, media) in [hash(&a), hash(&b), missing].into_iter().enumerate() {
        set.clip(
            track,
            Ticks::QUARTER * (index as i64 * 4),
            Ticks::QUARTER * 2,
            &format!("Clip {index}"),
            ClipData::Audio(AudioData::new(media)),
        );
    }
    let exported = export_of(set.editor.document(), &files, &Machine::new(), true);
    let container = read_container_strict(&exported.bytes).unwrap();
    let mut names: Vec<_> = container.files.keys().cloned().collect();
    names.sort();
    assert_eq!(names, ["audio/tone 2.wav", "audio/tone.wav"]);
    assert_eq!(container.files["audio/tone.wav"], a);
    assert!(
        exported
            .report
            .entries
            .iter()
            .any(|e| e.what == "a clip whose file is missing" && e.place.contains("Clip 2")),
        "{:?}",
        exported.report
    );
    let text = project_text(&exported.bytes);
    assert!(!text.contains("Clip 2"), "the clip is left out");

    let referred = export_of(set.editor.document(), &files, &Machine::new(), false);
    let container = read_container_strict(&referred.bytes).unwrap();
    assert!(container.files.is_empty());
    let text = project_text(&referred.bytes);
    let absolute = std::fs::canonicalize(one.join("tone.wav")).unwrap();
    assert!(
        text.contains(&format!(
            "path=\"{}\" external=\"true\"",
            absolute.display()
        )),
        "{text}"
    );
}

/// Spec: dawproject R75. The Transport holds the tempo and signature at
/// 1.1.1; tempo automation writes a point at each segment's start, a hold
/// for a steady one and a line for a ramp, and a jump after a ramp holds the
/// ramp's end value up to the tick before it.
#[test]
fn tempo_and_signatures_are_written_as_points() {
    let q = Ticks::QUARTER;
    let mut set = Built {
        editor: Editor::new(),
    };
    set.apply(Command::SetTempo {
        tempo: TempoMap::new([
            TempoSegment::Ramp {
                start: Ticks::ZERO,
                from: Bpm::new(100.0).unwrap(),
                to: Bpm::new(140.0).unwrap(),
            },
            TempoSegment::Constant {
                start: q * 8,
                bpm: Bpm::new(150.0).unwrap(),
            },
            TempoSegment::Constant {
                start: q * 16,
                bpm: Bpm::new(90.0).unwrap(),
            },
        ])
        .unwrap(),
    });
    set.apply(Command::SetTimeSignatures {
        time_signatures: TimeSignatureMap::new([
            TimeSignatureChange {
                at: Ticks::ZERO,
                signature: TimeSignature::new(3, 4).unwrap(),
            },
            TimeSignatureChange {
                at: q * 12,
                signature: TimeSignature::new(6, 8).unwrap(),
            },
        ])
        .unwrap(),
    });
    let exported = export_of(
        set.editor.document(),
        &Files(HashMap::new()),
        &Machine::new(),
        true,
    );
    let container = read_container_strict(&exported.bytes).unwrap();
    let transport = container.project.transport.as_ref().unwrap();
    assert_eq!(transport.tempo.as_ref().unwrap().value, Some(100.0));
    assert_eq!(
        transport.tempo.as_ref().unwrap().unit,
        Some(dawproject::model::Unit::Bpm)
    );
    let signature = transport.time_signature.as_ref().unwrap();
    assert_eq!(
        (signature.numerator, signature.denominator),
        (Some(3), Some(4))
    );
    let arrangement = container.project.arrangement.as_ref().unwrap();
    let points: Vec<_> = arrangement
        .tempo_automation
        .as_ref()
        .unwrap()
        .points
        .iter()
        .map(|point| match point {
            dawproject::model::PointValue::Real {
                time,
                value,
                interpolation,
            } => (time.unwrap(), value.unwrap(), *interpolation),
            other => panic!("{other:?}"),
        })
        .collect();
    use dawproject::model::Interpolation::{Hold, Linear};
    let one_tick = 1.0 / 705_600.0;
    assert_eq!(
        points,
        [
            (0.0, 100.0, Some(Linear)),
            (8.0 - one_tick, 140.0, Some(Hold)),
            (8.0, 150.0, Some(Hold)),
            (16.0, 90.0, Some(Hold)),
        ]
    );
    let changes: Vec<_> = arrangement
        .signature_automation
        .as_ref()
        .unwrap()
        .points
        .iter()
        .map(|point| match point {
            dawproject::model::PointValue::Signature {
                time,
                numerator,
                denominator,
            } => (time.unwrap(), numerator.unwrap(), denominator.unwrap()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(changes, [(0.0, 3, 4), (12.0, 6, 8)]);
}

/// Spec: dawproject R77, R78. Plug-ins write their ID, vendor, version and
/// state in each format; the generic devices write their values with their
/// preset as the state; Beatlayer's other devices and racks are built-in
/// devices with their presets; an LV2 plug-in is listed.
#[test]
fn plug_ins_and_devices_are_written_as_the_spec_says() {
    let folder = TempDir::new("devices");
    let (document, files) = build(&default_spec(), &folder.0);
    let exported = export_of(&document, &files, &Machine::new(), true);
    let container = read_container_strict(&exported.bytes).unwrap();
    // The CLAP state is the stream unchanged.
    assert_eq!(
        container.files["plugins/1.clap-preset"],
        b"clap-state\x00\x01"
    );
    // The VST3 state is a `.vstpreset` with its component and controller.
    let preset =
        bl_model::vst3_state::VstPreset::read(&container.files["plugins/2.vstpreset"]).unwrap();
    assert!(bl_model::vst3_state::same_class_id(&preset.class_id, VST3));
    assert_eq!(preset.component, b"component");
    assert_eq!(preset.controller, b"controller");
    // The Audio Unit's state is its property list.
    assert!(container.files["plugins/3.aupreset"].starts_with(b"<?xml"));
    let text = project_text(&exported.bytes);
    assert!(text.contains("<ClapPlugin"));
    assert!(text.contains("deviceVendor=\"Acme\" pluginVersion=\"1.2\""));
    assert!(text.contains("deviceID=\"aufx/dely/appl\""));
    // The generic devices.
    for element in ["<Compressor", "<NoiseGate", "<Limiter", "<Equalizer"] {
        assert!(text.contains(element), "{element}");
    }
    assert!(text.contains("<Ratio unit=\"percent\" value=\"75\"/>"));
    assert!(
        text.contains("<Threshold id=\"id"),
        "an automated threshold has an id"
    );
    // The others are built-in devices with their presets.
    assert!(text.contains("<BuiltinDevice name=\"Utility\" deviceID=\"beatlayer.utility\""));
    assert!(text.contains("deviceID=\"Audio Effect Rack\""));
    let reported: Vec<_> = exported
        .report
        .entries
        .iter()
        .filter(|e| e.did.contains("Beatlayer preset"))
        .collect();
    assert_eq!(reported.len(), 2, "{reported:?}");
}

/// Spec: dawproject R78. An LV2 plug-in is left out and listed, and a
/// steeper EQ cut is listed.
#[test]
fn an_lv2_plug_in_and_a_steep_cut_are_listed() {
    let mut set = Built {
        editor: Editor::new(),
    };
    let track = set.track(TrackKind::Audio, "Audio", None);
    set.plugin(
        track,
        PluginFormat::Lv2,
        "urn:example",
        "Test LV2",
        DeviceRole::AudioEffect,
        vec![],
    );
    set.device(
        track,
        bl_devices::eq::EQ,
        &[(bl_devices::eq::band_param(0, 0, bl_devices::eq::ON), 1.0)],
    );
    let exported = export_of(
        set.editor.document(),
        &Files(HashMap::new()),
        &Machine::new(),
        true,
    );
    let whats: Vec<_> = exported
        .report
        .entries
        .iter()
        .map(|e| e.what.as_str())
        .collect();
    assert!(whats.contains(&"an LV2 plug-in"), "{whats:?}");
    assert!(
        whats.contains(&"a Low Cut or High Cut at 48 dB per octave"),
        "{whats:?}"
    );
    let container = read_container_strict(&exported.bytes).unwrap();
    assert_eq!(container.project.structure.len(), 2);
}

/// Spec: dawproject R79. Each locator is a marker with its time in beats and
/// its name; `metadata.xml` holds the set's name as its title.
#[test]
fn locators_are_markers_and_the_title_is_the_sets_name() {
    let folder = TempDir::new("markers");
    let (document, files) = build(&default_spec(), &folder.0);
    let exported = export_of(&document, &files, &Machine::new(), true);
    let container = read_container_strict(&exported.bytes).unwrap();
    let markers = container
        .project
        .arrangement
        .as_ref()
        .unwrap()
        .markers
        .as_ref()
        .unwrap();
    let list: Vec<_> = markers
        .markers
        .iter()
        .map(|m| (m.info.name.clone().unwrap(), m.time.unwrap()))
        .collect();
    assert_eq!(list, [("Hook".to_owned(), 4.0), ("Chorus".to_owned(), 8.0)]);
    let metadata = container.metadata.expect("metadata.xml");
    assert!(metadata.contains("<Title>Test Set</Title>"), "{metadata}");
}

/// Spec: dawproject R80, R81. What the format can't carry isn't written, and
/// the report lists each kind once in each place.
#[test]
fn the_report_lists_every_item_the_format_cannot_carry() {
    let folder = TempDir::new("report");
    let wav = wav(2.0, 100.0);
    let path = folder.0.join("t.wav");
    std::fs::write(&path, &wav).unwrap();
    let hash = ContentHash(*bl_media::ContentHash::of_bytes(&wav).as_bytes());
    let files = Files(HashMap::from([(hash, path)]));
    let q = Ticks::QUARTER;
    let mut set = Built {
        editor: Editor::new(),
    };
    let audio = set.track(TrackKind::Audio, "Audio", None);
    let midi = set.track(TrackKind::Midi, "MIDI", None);
    let other = set.track(TrackKind::Midi, "Other", None);
    let mut settings = bl_model::WarpSettings::default();
    settings.tones.grain_size = 50.0;
    let mut clip = Clip::new(
        set.editor.new_id(),
        Ticks::ZERO,
        q * 4,
        ClipData::Audio(AudioData {
            warp_settings: settings,
            ..AudioData::new(hash)
        }),
    );
    clip.launch.mode = bl_model::LaunchMode::Gate;
    clip.envelopes.push(bl_model::ClipEnvelope::new(
        AutomationTarget::Mixer {
            param: MixerParam::Volume,
        },
        bl_model::automation::Envelope::from_points(points(&[(0, 0.5, 0.0), (q.get(), 1.0, 0.0)]))
            .unwrap(),
    ));
    set.apply(Command::CreateClip { track: audio, clip });
    let mut notes = vec![Note::new(60, Ticks::ZERO, q, 100), Note::new(62, q, q, 100)];
    notes[1].mute = true;
    notes[0].probability = 0.5;
    set.clip(
        midi,
        Ticks::ZERO,
        q * 2,
        "Notes",
        ClipData::Midi(MidiData::new(notes)),
    );
    set.apply(Command::SetCrossfadeAssign {
        track: audio,
        assign: bl_model::CrossfadeAssign::A,
    });
    set.apply(Command::SetTrackDelay {
        track: audio,
        delay: bl_model::TrackDelay::Milliseconds { value: 10.0 },
    });
    set.apply(Command::SetArm {
        track: audio,
        arm: true,
    });
    set.apply(Command::SetInput {
        track: audio,
        input: Input::Resampling,
        channel: None,
    });
    set.apply(Command::SetMonitor {
        track: audio,
        monitor: Monitor::In,
    });
    set.apply(Command::SetMidiOutput {
        track: midi,
        to: Some(bl_model::MidiTo::Track(other)),
    });
    let squeeze = set.device(audio, bl_devices::compressor::COMPRESSOR, &[]);
    set.apply(Command::SetSidechainEnabled {
        device: squeeze,
        enabled: true,
    });
    let scene = set.editor.document().scenes().next().unwrap().id;
    set.apply(Command::SetSceneTempo {
        scene,
        tempo: Some(Bpm::new(100.0).unwrap()),
    });
    let exported = export_of(set.editor.document(), &files, &Machine::new(), true);
    let whats: Vec<String> = exported
        .report
        .entries
        .iter()
        .map(|e| format!("{} @ {}", e.what, e.place))
        .collect();
    for wanted in [
        "clips' launch settings and follow actions @ the track \"Audio\"",
        "a warp mode's settings @ the track \"Audio\"",
        "clip modulation envelopes @ the track \"Audio\"",
        "the crossfader assignment @ the track \"Audio\"",
        "the track delay @ the track \"Audio\"",
        "the Arm switch @ the track \"Audio\"",
        "the input @ the track \"Audio\"",
        "the monitoring @ the track \"Audio\"",
        "MIDI To @ the track \"MIDI\"",
        "muted notes @ the track \"MIDI\"",
        "notes' chance and velocity deviation @ the track \"MIDI\"",
        "a sidechain @ the device \"Compressor\" on the track \"Audio\"",
        "a scene's tempo @ scene 1",
    ] {
        assert!(whats.iter().any(|w| w == wanted), "{wanted}\n{whats:#?}");
    }
    // Once per kind and place: the report has no entry twice.
    let mut seen = std::collections::BTreeSet::new();
    for entry in &exported.report.entries {
        assert!(
            seen.insert((&entry.what, &entry.place, &entry.did)),
            "{entry:?}"
        );
    }
}

/// Spec: dawproject R4 (strict mode). A file with an element or attribute
/// the schema doesn't have is refused by the strict reader, and the
/// ordinary reader reads it in part.
#[test]
fn the_strict_reader_refuses_a_name_the_schema_does_not_have() {
    let xml = r#"<?xml version="1.0"?><Project version="1.0"><Application name="x" version="1"/><Structure><Track name="a" madeUp="1"/></Structure></Project>"#;
    let mut bytes = Vec::new();
    {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        writer
            .start_file("project.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(xml.as_bytes()).unwrap();
        writer.finish().unwrap();
    }
    assert!(dawproject::read_container(&bytes).is_ok());
    let error = read_container_strict(&bytes).expect_err("refused");
    assert!(error.to_string().contains("madeUp"), "{error}");
}

/// The elements of `Project.xsd` and `MetaData.xsd` with the attributes each
/// has, written out by hand from the schemas (version 1.0), with the values
/// the schemas enumerate.
fn schema() -> HashMap<&'static str, Vec<&'static str>> {
    let nameable = ["name", "color", "comment"];
    let referenceable = ["id", "name", "color", "comment"];
    let timeline = ["id", "name", "color", "comment", "timeUnit", "track"];
    let with = |base: &[&'static str], more: &[&'static str]| {
        base.iter().chain(more).copied().collect::<Vec<_>>()
    };
    let parameter = ["id", "name", "color", "comment", "parameterID"];
    let real = with(&parameter, &["max", "min", "unit", "value"]);
    let boolean = with(&parameter, &["value"]);
    let device = [
        "id",
        "name",
        "color",
        "comment",
        "deviceID",
        "deviceName",
        "deviceRole",
        "deviceVendor",
        "loaded",
    ];
    let plugin = with(&device, &["pluginVersion"]);
    let mut table: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
    for name in [
        "Tempo",
        "Volume",
        "Pan",
        "Freq",
        "Gain",
        "Q",
        "Attack",
        "Release",
        "Threshold",
        "Range",
        "Ratio",
        "InputGain",
        "OutputGain",
        "RealParameter",
    ] {
        table.insert(name, real.clone());
    }
    for name in ["Mute", "Enabled", "Enable", "AutoMakeup", "BoolParameter"] {
        table.insert(name, boolean.clone());
    }
    table.insert(
        "TimeSignature",
        with(&parameter, &["denominator", "numerator"]),
    );
    table.insert("Project", vec!["version"]);
    table.insert("Application", vec!["name", "version"]);
    for name in [
        "Transport",
        "Structure",
        "Scenes",
        "Devices",
        "Parameters",
        "Sends",
    ] {
        table.insert(name, vec![]);
    }
    table.insert("Track", with(&referenceable, &["contentType", "loaded"]));
    table.insert(
        "Channel",
        with(
            &referenceable,
            &["audioChannels", "destination", "role", "solo"],
        ),
    );
    table.insert("Send", with(&referenceable, &["destination", "type"]));
    for name in [
        "Device",
        "BuiltinDevice",
        "Equalizer",
        "Compressor",
        "NoiseGate",
        "Limiter",
    ] {
        table.insert(name, device.to_vec());
    }
    for name in ["Vst2Plugin", "Vst3Plugin", "ClapPlugin", "AuPlugin"] {
        table.insert(name, plugin.clone());
    }
    table.insert("State", vec!["path", "external"]);
    table.insert("File", vec!["path", "external"]);
    table.insert("Band", vec!["type", "order"]);
    table.insert("Arrangement", referenceable.to_vec());
    for name in ["Lanes", "Clips", "Notes", "Markers"] {
        table.insert(name, timeline.to_vec());
    }
    table.insert("ClipSlot", with(&timeline, &["hasStop"]));
    table.insert("Warps", with(&timeline, &["contentTimeUnit"]));
    for name in ["Audio", "Video"] {
        table.insert(
            name,
            with(
                &timeline,
                &["duration", "algorithm", "channels", "sampleRate"],
            ),
        );
    }
    for name in ["Points", "TempoAutomation", "TimeSignatureAutomation"] {
        table.insert(name, with(&timeline, &["unit"]));
    }
    table.insert("Scene", referenceable.to_vec());
    table.insert(
        "Clip",
        with(
            &nameable,
            &[
                "time",
                "duration",
                "contentTimeUnit",
                "playStart",
                "playStop",
                "loopStart",
                "loopEnd",
                "fadeTimeUnit",
                "fadeInTime",
                "fadeOutTime",
                "enable",
                "reference",
            ],
        ),
    );
    table.insert(
        "Note",
        vec!["time", "duration", "channel", "key", "vel", "rel"],
    );
    table.insert("Marker", with(&nameable, &["time"]));
    table.insert("Warp", vec!["time", "contentTime"]);
    table.insert(
        "Target",
        vec!["parameter", "expression", "channel", "key", "controller"],
    );
    table.insert("RealPoint", vec!["time", "value", "interpolation"]);
    table.insert("BoolPoint", vec!["time", "value"]);
    table.insert("IntegerPoint", vec!["time", "value"]);
    table.insert("EnumPoint", vec!["time", "value"]);
    table.insert(
        "TimeSignaturePoint",
        vec!["time", "numerator", "denominator"],
    );
    table
}

/// The values `Project.xsd` enumerates, by attribute.
fn enumerations() -> HashMap<&'static str, Vec<&'static str>> {
    HashMap::from([
        (
            "unit",
            vec![
                "linear",
                "normalized",
                "percent",
                "decibel",
                "hertz",
                "semitones",
                "seconds",
                "beats",
                "bpm",
            ],
        ),
        ("timeUnit", vec!["beats", "seconds"]),
        ("contentTimeUnit", vec!["beats", "seconds"]),
        ("fadeTimeUnit", vec!["beats", "seconds"]),
        (
            "deviceRole",
            vec!["instrument", "noteFX", "audioFX", "analyzer"],
        ),
        (
            "type",
            vec![
                "highPass",
                "lowPass",
                "bandPass",
                "highShelf",
                "lowShelf",
                "bell",
                "notch",
                "pre",
                "post",
            ],
        ),
        ("role", vec!["regular", "master", "effect", "submix", "vca"]),
        ("interpolation", vec!["hold", "linear"]),
        (
            "expression",
            vec![
                "gain",
                "pan",
                "transpose",
                "timbre",
                "formant",
                "pressure",
                "channelController",
                "channelPressure",
                "polyPressure",
                "pitchBend",
                "programChange",
            ],
        ),
    ])
}

/// Checks that a part's XML uses only the schema's names and enumerated
/// values.
fn assert_schema_names(text: &str) {
    use quick_xml::Reader;
    use quick_xml::events::Event;
    let table = schema();
    let enumerated = enumerations();
    let mut reader = Reader::from_str(text);
    loop {
        match reader.read_event().expect("well-formed XML") {
            Event::Start(element) | Event::Empty(element) => {
                let name = String::from_utf8_lossy(element.name().as_ref()).into_owned();
                let allowed = table
                    .get(name.as_str())
                    .unwrap_or_else(|| panic!("`{name}` isn't in Project.xsd"));
                for attribute in element.attributes().flatten() {
                    let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
                    assert!(
                        allowed.contains(&key.as_str()),
                        "`{key}` isn't an attribute of `{name}`"
                    );
                    if let Some(values) = enumerated.get(key.as_str()) {
                        let value = String::from_utf8_lossy(attribute.value.as_ref()).into_owned();
                        let list = key == "contentType";
                        assert!(
                            list || values.contains(&value.as_str()),
                            "`{value}` isn't a `{key}`"
                        );
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
}

/// Spec: dawproject R66, R67. Every export uses only the names and values
/// `Project.xsd` and `MetaData.xsd` give.
#[test]
fn an_export_uses_only_the_names_of_the_schemas() {
    let folder = TempDir::new("names");
    let (document, files) = build(&default_spec(), &folder.0);
    let exported = export_of(&document, &files, &Machine::new(), true);
    assert_schema_names(&project_text(&exported.bytes));
    let container = read_container_strict(&exported.bytes).unwrap();
    let metadata = container.metadata.unwrap();
    for name in ["MetaData", "Title"] {
        assert!(metadata.contains(&format!("<{name}>")));
    }
}

/// A track of a layout: its kind, whether it sits in the last group made,
/// its volume, its pan and whether it is soloed.
type TrackPlan = (u8, bool, u32, i32, bool);

/// The tracks of a layout and the levels of the sends to its returns.
type Layout = (Vec<TrackPlan>, Vec<u32>);

/// A layout of tracks: each is a kind (audio, MIDI or group), a mixer and
/// whether it sits in the last group made.
fn layouts() -> impl Strategy<Value = Layout> {
    (
        prop::collection::vec(
            (
                0u8..3,
                any::<bool>(),
                0u32..190,
                -100i32..100,
                any::<bool>(),
            ),
            0..9,
        ),
        prop::collection::vec(1u32..100, 0..4),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    // Spec: dawproject R67, R68, R69, R82. Any arrangement of tracks, groups
    // and returns with its mixer and sends comes back the same.
    #[test]
    fn any_layout_of_tracks_groups_and_returns_comes_back_the_same((tracks, returns) in layouts()) {
        let mut set = Built { editor: Editor::new() };
        let mut group = None;
        let mut sources = Vec::new();
        for (index, (kind, inside, volume, pan, solo)) in tracks.iter().enumerate() {
            let kind = [TrackKind::Audio, TrackKind::Midi, TrackKind::Group][usize::from(*kind)];
            let parent = if *inside { group } else { None };
            let id = set.track(kind, &format!("T{index}"), parent);
            if kind == TrackKind::Group && parent.is_none() {
                group = Some(id);
            }
            set.apply(Command::SetVolume { track: id, volume: f64::from(*volume) / 100.0 });
            set.apply(Command::SetPan { track: id, pan: f64::from(*pan) / 100.0 });
            set.apply(Command::SetSolo { track: id, solo: *solo });
            if kind == TrackKind::Audio {
                sources.push(id);
            }
        }
        for (index, level) in returns.iter().enumerate() {
            let id = set.track(TrackKind::Return, &format!("R{index}"), None);
            for source in &sources {
                set.apply(Command::SetSend { track: *source, to: id, level: f64::from(*level) / 100.0 });
            }
        }
        let document = set.editor.document().clone();
        let files = Files(HashMap::new());
        let machine = Machine::new();
        let first = export_of(&document, &files, &machine, true);
        prop_assert!(first.bytes == export_of(&document, &files, &machine, true).bytes);
        let folder = std::env::temp_dir();
        let imported = import_of(&first.bytes, &folder, &machine);
        let found = differences(&document, &imported.project.document);
        prop_assert!(found.is_empty(), "{found:#?}");
    }
}

/// Spec: dawproject R71. A warped clip whose loop brace runs to the end of
/// its file writes a loop end, so the loop is still on when it comes back.
#[test]
fn a_warped_loop_with_no_end_comes_back_as_a_loop() {
    let folder = TempDir::new("open-loop");
    let bytes = wav(2.0, 100.0);
    let path = folder.0.join("l.wav");
    std::fs::write(&path, &bytes).unwrap();
    let hash = ContentHash(*bl_media::ContentHash::of_bytes(&bytes).as_bytes());
    let files = Files(HashMap::from([(hash, path)]));
    let q = Ticks::QUARTER;
    let mut set = Built {
        editor: Editor::new(),
    };
    let track = set.track(TrackKind::Audio, "Audio", None);
    set.clip(
        track,
        Ticks::ZERO,
        q * 8,
        "Loop",
        ClipData::Audio(AudioData {
            warp: true,
            warp_markers: vec![
                WarpMarker {
                    time: 0.0,
                    beat: Ticks::ZERO,
                },
                WarpMarker {
                    time: 1.0,
                    beat: q * 2,
                },
            ],
            tempo: Some(120.0),
            looping: true,
            loop_start: 0.0,
            loop_end: None,
            ..AudioData::new(hash)
        }),
    );
    let machine = Machine::new();
    let exported = export_of(set.editor.document(), &files, &machine, true);
    let imported = import_of(&exported.bytes, &folder.0, &machine);
    let clip = imported
        .project
        .document
        .tracks()
        .find(|t| t.name() == "Audio")
        .and_then(|t| t.clips().next().cloned())
        .unwrap();
    let ClipData::Audio(data) = clip.data else {
        panic!("an audio clip")
    };
    // The file is two seconds, four beats at 120 BPM.
    assert!(data.looping);
    assert_eq!(data.loop_end, Some((q * 4).get() as f64));
}
