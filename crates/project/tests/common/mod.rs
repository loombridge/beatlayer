//! Shared helpers for bl-project's integration tests: random projects and
//! scratch folders.

#![allow(dead_code, reason = "each test file uses a different subset")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use bl_core::id::{Id, IdAllocator};
use bl_core::time::{
    Bpm, TempoMap, TempoSegment, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap,
};
use bl_model::{
    AudioData, Clip, ClipData, Command, ContentHash, Device, DeviceKind, Editor, MidiData, Note,
    ParamId, PluginFormat, TrackId, TrackKind,
};
use bl_project::{Media, MediaPath, MediaRef, Project};
use proptest::prelude::*;
use proptest::sample::Index;

/// A folder for one test under Cargo's per-target scratch folder, emptied
/// when it's made and deleted when it's dropped.
pub struct TestDir(PathBuf);

impl TestDir {
    pub fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("bl-project-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One step in building a random song. Targets are chosen from what the
/// song holds when the step runs.
#[derive(Clone, Debug)]
enum Step {
    Track {
        kind: TrackKind,
        parent: Index,
        name: String,
        volume: f64,
        pan: f64,
        switches: (bool, bool, bool),
    },
    Clip {
        track: Index,
        start: i64,
        length: i64,
        name: String,
        mute: bool,
        media: u8,
        notes: Vec<(u8, u8, i64, i64)>,
    },
    Device {
        track: Index,
        name: String,
        plugin: Option<(PluginFormat, String)>,
        enabled: bool,
        params: Vec<(u32, f64)>,
    },
}

/// The recipe for a random project.
#[derive(Clone, Debug)]
pub struct Recipe {
    steps: Vec<Step>,
    tempo: Vec<(i64, f64, Option<f64>)>,
    signatures: Vec<(i64, u8, u8)>,
    id_skip: u64,
    extra_media: Vec<u8>,
    unlisted_media: Index,
    paths: Vec<String>,
    sizes: Vec<u64>,
}

/// Any finite f64, with the awkward ones well represented: zeros of both
/// signs, subnormals, the extremes, and decimals binary can't hold exactly.
pub fn finite() -> impl Strategy<Value = f64> {
    prop_oneof![
        4 => any::<f64>().prop_filter("finite", |v| v.is_finite()),
        2 => -1.0e6..1.0e6,
        1 => prop::sample::select(vec![
            0.0,
            -0.0,
            0.1,
            0.1 + 0.2,
            1.0 / 3.0,
            -2.0 / 3.0,
            f64::MIN_POSITIVE,
            5e-324,
            -5e-324,
            f64::MAX,
            f64::MIN,
            f64::EPSILON,
            1.0 - f64::EPSILON / 2.0,
            123_456_789.123_456_79,
        ]),
    ]
}

fn volume() -> impl Strategy<Value = f64> {
    prop_oneof![
        3 => 0.0..4.0,
        1 => finite().prop_map(f64::abs),
        1 => prop::sample::select(vec![0.0, 1.0, 0.501_187_233_627_272_2, 1.0e-300]),
    ]
}

fn pan() -> impl Strategy<Value = f64> {
    prop_oneof![
        3 => -1.0..=1.0,
        1 => prop::sample::select(vec![-1.0, -0.0, 0.0, 1.0, 1.0e-300, 0.1]),
    ]
}

fn kind() -> impl Strategy<Value = TrackKind> {
    prop_oneof![
        4 => Just(TrackKind::Audio),
        4 => Just(TrackKind::Midi),
        2 => Just(TrackKind::Group),
        1 => Just(TrackKind::Return),
    ]
}

fn name() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "[A-Za-z0-9 ]{0,12}",
        1 => any::<String>(),
    ]
}

fn plugin() -> impl Strategy<Value = Option<(PluginFormat, String)>> {
    prop::option::of((
        prop::sample::select(vec![
            PluginFormat::Clap,
            PluginFormat::Vst3,
            PluginFormat::AudioUnit,
            PluginFormat::Lv2,
        ]),
        name(),
    ))
}

fn step() -> impl Strategy<Value = Step> {
    let ticks = |max: i64| prop_oneof![3 => 0..max, 1 => 0..i64::MAX / 4];
    prop_oneof![
        3 => (kind(), any::<Index>(), name(), volume(), pan(), any::<(bool, bool, bool)>())
            .prop_map(|(kind, parent, name, volume, pan, switches)| Step::Track {
                kind, parent, name, volume, pan, switches,
            }),
        5 => (
            any::<Index>(),
            ticks(1 << 32),
            1..(1i64 << 30),
            name(),
            any::<bool>(),
            any::<u8>(),
            prop::collection::vec((0u8..=127, 1u8..=127, ticks(1 << 28), 1..(1i64 << 24)), 0..8),
        )
            .prop_map(|(track, start, length, name, mute, media, notes)| Step::Clip {
                track, start, length, name, mute, media, notes,
            }),
        2 => (
            any::<Index>(),
            name(),
            plugin(),
            any::<bool>(),
            prop::collection::vec((any::<u32>(), finite()), 0..6),
        )
            .prop_map(|(track, name, plugin, enabled, params)| Step::Device {
                track, name, plugin, enabled, params,
            }),
    ]
}

/// A media path component: never empty, `.`, or `..`, and free of the
/// separators.
fn path_name() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "[A-Za-z0-9 _.-]{1,12}",
        1 => any::<String>(),
    ]
    .prop_map(|name| {
        let name: String = name
            .chars()
            .filter(|c| !matches!(c, '/' | '\\' | '\0'))
            .collect();
        if name.is_empty() || name == "." || name == ".." {
            format!("x{name}")
        } else {
            name
        }
    })
}

fn media_path() -> impl Strategy<Value = String> {
    (0usize..3, prop::collection::vec(path_name(), 1..4)).prop_map(|(up, names)| {
        let mut parts = vec![".."; up];
        parts.extend(names.iter().map(String::as_str));
        parts.join("/")
    })
}

/// Recipes for projects of up to `steps` building steps.
pub fn recipe(steps: usize) -> impl Strategy<Value = Recipe> {
    (
        prop::collection::vec(step(), 0..=steps),
        prop::collection::vec(
            (
                1i64..1 << 40,
                1.0f64..=1000.0,
                prop::option::of(1.0f64..=1000.0),
            ),
            0..4,
        ),
        prop::collection::vec((1i64..1 << 36, 1u8..=99, 0u8..5), 0..3),
        prop_oneof![3 => Just(0u64), 1 => 0u64..1 << 20, 1 => 0u64..1 << 62],
        prop::collection::vec(any::<u8>(), 0..3),
        any::<Index>(),
        prop::collection::vec(media_path(), 8),
        prop::collection::vec(prop_oneof![0u64..1 << 32, any::<u64>()], 8),
    )
        .prop_map(
            |(steps, tempo, signatures, id_skip, extra_media, unlisted_media, paths, sizes)| {
                Recipe {
                    steps,
                    tempo,
                    signatures,
                    id_skip,
                    extra_media,
                    unlisted_media,
                    paths,
                    sizes,
                }
            },
        )
}

fn hash(byte: u8) -> ContentHash {
    let mut bytes = [byte; 32];
    bytes[0] = 0xb1;
    ContentHash(bytes)
}

/// An audio clip's data from a generated byte: one of 16 files, and for odd
/// bytes, every setting a clip can have away from its default (WP-1.4), so a
/// round trip carries them all.
fn audio_data(byte: u8) -> AudioData {
    let media = byte % 16;
    let audio = AudioData::new(hash(media));
    if byte.is_multiple_of(2) {
        return audio;
    }
    let start = f64::from(byte) / 64.0;
    AudioData {
        start,
        end: Some(start + 1.5),
        looping: byte % 4 == 1,
        loop_start: 0.25 / f64::from(byte),
        loop_end: Some(start + 0.75),
        gain: 1.0 / f64::from(byte),
        ..audio.with_edge_fades()
    }
}

impl Recipe {
    /// Builds the project. Steps the model refuses are skipped.
    pub fn build(&self) -> Project {
        let mut editor = Editor::new();
        for step in &self.steps {
            apply(&mut editor, step);
        }
        self.settings(&mut editor);

        let mut ids = editor.ids();
        if self.id_skip > 0 {
            ids = IdAllocator::starting_at(Id::new(ids.peek().get() + self.id_skip).unwrap());
        }

        let document = editor.document().clone();
        let mut used: Vec<ContentHash> = document
            .tracks()
            .flat_map(|track| track.clips())
            .filter_map(|clip| match &clip.data {
                ClipData::Audio(audio) => Some(audio.media),
                ClipData::Midi(_) => None,
            })
            .collect();
        used.sort();
        used.dedup();
        // One used file is sometimes missing from the table, and some files
        // in it are used by no clip.
        if !used.is_empty() && self.id_skip % 2 == 1 {
            used.remove(self.unlisted_media.index(used.len()));
        }
        used.extend(self.extra_media.iter().map(|&b| hash(b.wrapping_add(128))));
        let media: Media = used
            .into_iter()
            .enumerate()
            .map(|(i, hash)| MediaRef {
                hash,
                path: MediaPath::new(self.paths[i % self.paths.len()].clone()).unwrap(),
                size: self.sizes[i % self.sizes.len()],
            })
            .collect();

        Project {
            document,
            ids,
            media,
        }
    }

    fn settings(&self, editor: &mut Editor) {
        if !self.tempo.is_empty() {
            let mut start = Ticks::ZERO;
            let mut segments = Vec::new();
            for (i, &(gap, bpm, ramp_to)) in self.tempo.iter().enumerate() {
                let bpm = Bpm::new(bpm).unwrap();
                let last = i + 1 == self.tempo.len();
                segments.push(match ramp_to {
                    Some(to) if !last => TempoSegment::Ramp {
                        start,
                        from: bpm,
                        to: Bpm::new(to).unwrap(),
                    },
                    _ => TempoSegment::Constant { start, bpm },
                });
                start = Ticks(start.get() + gap);
            }
            let tempo = TempoMap::new(segments).unwrap();
            editor.apply(Command::SetTempo { tempo }).unwrap();
        }
        if !self.signatures.is_empty() {
            let mut at = Ticks::ZERO;
            let mut changes = Vec::new();
            for &(gap, numerator, power) in &self.signatures {
                let signature = TimeSignature::new(numerator, 1 << power).unwrap();
                changes.push(TimeSignatureChange { at, signature });
                at = Ticks(at.get() + gap);
            }
            if let Ok(time_signatures) = TimeSignatureMap::new(changes) {
                editor
                    .apply(Command::SetTimeSignatures { time_signatures })
                    .unwrap();
            }
        }
    }
}

fn apply(editor: &mut Editor, step: &Step) {
    let doc = editor.document();
    let tracks: Vec<TrackId> = doc.track_order();
    let groups: Vec<TrackId> = tracks
        .iter()
        .copied()
        .filter(|t| doc.track(*t).unwrap().kind() == TrackKind::Group)
        .collect();
    let holders: Vec<TrackId> = tracks
        .iter()
        .copied()
        .filter(|t| doc.track(*t).unwrap().kind().holds_clips())
        .collect();
    match step {
        Step::Track {
            kind,
            parent,
            name,
            volume,
            pan,
            switches: (mute, solo, arm),
        } => {
            let parent = match (kind, parent.index(groups.len() + 1)) {
                (TrackKind::Return, _) | (_, 0) => None,
                (_, n) => Some(groups[n - 1]),
            };
            let index = match (kind, parent) {
                (TrackKind::Return, _) => doc.returns().len(),
                (_, None) => doc.top_level().len(),
                (_, Some(group)) => doc.track(group).unwrap().children().len(),
            };
            let id = editor.new_id();
            let created = editor.apply(Command::CreateTrack {
                id,
                kind: *kind,
                name: name.clone(),
                parent,
                index,
            });
            if created.is_ok() {
                for command in [
                    Command::SetVolume {
                        track: id,
                        volume: *volume,
                    },
                    Command::SetPan {
                        track: id,
                        pan: *pan,
                    },
                    Command::SetMute {
                        track: id,
                        mute: *mute,
                    },
                    Command::SetSolo {
                        track: id,
                        solo: *solo,
                    },
                    Command::SetArm {
                        track: id,
                        arm: *arm,
                    },
                ] {
                    let _ = editor.apply(command);
                }
            }
        }
        Step::Clip {
            track,
            start,
            length,
            name,
            mute,
            media,
            notes,
        } => {
            if holders.is_empty() {
                return;
            }
            let track = *track.get(&holders);
            let data = if doc.track(track).unwrap().kind() == TrackKind::Audio {
                ClipData::Audio(audio_data(*media))
            } else {
                ClipData::Midi(MidiData::new(notes.iter().map(
                    |&(key, velocity, start, length)| Note {
                        key,
                        velocity,
                        start: Ticks(start),
                        length: Ticks(length),
                    },
                )))
            };
            let mut clip = Clip::new(editor.new_id(), Ticks(*start), Ticks(*length), data);
            clip.name.clone_from(name);
            clip.mute = *mute;
            let _ = editor.apply(Command::CreateClip { track, clip });
        }
        Step::Device {
            track,
            name,
            plugin,
            enabled,
            params,
        } => {
            let track = *track.get(&tracks);
            let index = doc.track(track).unwrap().devices().len();
            let kind = match plugin {
                Some((format, plugin_id)) => DeviceKind::Plugin {
                    format: *format,
                    plugin_id: plugin_id.clone(),
                },
                None => DeviceKind::Native {
                    device_type: format!("beatlayer.{}", name.len()),
                },
            };
            let mut device = Device::new(editor.new_id(), name.clone(), kind);
            device.enabled = *enabled;
            for &(param, value) in params {
                device.params.insert(ParamId(param), value);
            }
            let _ = editor.apply(Command::InsertDevice {
                track,
                index,
                device,
            });
        }
    }
}

/// A song of `tracks` tracks, alternately audio and MIDI, each with
/// `clips_per_track` clips and two devices; every MIDI clip holds 32 notes.
pub fn large_song(tracks: u32, clips_per_track: i64) -> Project {
    let mut editor = Editor::new();
    for t in 0..tracks {
        let track = editor.new_id();
        let kind = if t % 2 == 0 {
            TrackKind::Audio
        } else {
            TrackKind::Midi
        };
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind,
                name: format!("Track {t}"),
                parent: None,
                index: usize::try_from(t).unwrap(),
            })
            .unwrap();
        editor
            .apply(Command::SetVolume {
                track,
                volume: 1.0 / f64::from(t + 1),
            })
            .unwrap();
        for c in 0..clips_per_track {
            let data = if kind == TrackKind::Audio {
                let mut hash = [0u8; 32];
                hash[..4].copy_from_slice(&t.to_le_bytes());
                ClipData::Audio(AudioData::new(ContentHash(hash)))
            } else {
                ClipData::Midi(MidiData::new((0..32i64).map(|n| Note {
                    key: u8::try_from(36 + n).unwrap(),
                    velocity: 100,
                    start: Ticks::SIXTEENTH * n,
                    length: Ticks::SIXTEENTH,
                })))
            };
            let clip = Clip::new(
                editor.new_id(),
                Ticks::WHOLE * (c * 2),
                Ticks::WHOLE * 2,
                data,
            );
            editor.apply(Command::CreateClip { track, clip }).unwrap();
        }
        for d in 0..2 {
            let mut device = Device::new(
                editor.new_id(),
                "Utility",
                DeviceKind::Native {
                    device_type: "beatlayer.utility".to_owned(),
                },
            );
            for p in 0..16u32 {
                device.params.insert(ParamId(p), f64::from(p * 7 + d) / 3.0);
            }
            editor
                .apply(Command::InsertDevice {
                    track,
                    index: 0,
                    device,
                })
                .unwrap();
        }
    }
    Project::from_editor(&editor, Media::new())
}
