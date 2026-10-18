//! Property tests for commands, transactions, and undo (WP-0.6 acceptance).
//!
//! Each case applies a random sequence of 1 to 200 commands to a small song,
//! then undoes every step and checks the song is back where it started, then
//! redoes every step and checks it's back where it ended. Commands pick their
//! targets from the song as it stands, so most succeed; the ones that are
//! rejected must leave the song untouched.
//!
//! Along the way every change set is checked against one worked out the slow
//! way, by flattening both versions into plain maps, and the start and end
//! are also compared as JSON, so the check doesn't rest on pointer sharing.
//!
//! `BL_TEST_DEPTH` sets how many cases each property runs: 1,000 with
//! `full` (pushes to main, manual CI runs, and the nightly run), 100 with
//! `quick`, the default for local runs and pull requests.

use std::collections::{BTreeMap, BTreeSet};

use bl_core::id::Id;
use bl_core::time::{Bpm, TempoMap, Ticks, TimeSignature, TimeSignatureMap};
use bl_model::{
    AudioData, BeatsUpdate, Change, ChangeSet, Clip, ClipData, ClipId, Command, ContentHash,
    Device, DeviceId, DeviceKind, Document, EditError, Editor, Input, LaunchMode, MidiData, Mixer,
    Monitor, Note, ParamId, PluginFormat, Routing, SceneId, SendTap, TonesUpdate, TrackId,
    TrackKind, WarpMode,
};
use proptest::prelude::*;
use proptest::sample::Index;
use proptest::test_runner::FileFailurePersistence;

/// Whether the tests run at full depth: `BL_TEST_DEPTH=full`, as on pushes
/// to main and the nightly run. Unset or `quick`, the default for local runs
/// and pull requests, they run fewer cases or rounds of the same checks.
fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Err(_) | Ok("" | "quick") => false,
        Ok(other) => panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full"),
    }
}

/// 1,000 cases each at full depth, 100 at quick. Failures are saved next to
/// this file and replayed first on later runs.
fn config() -> ProptestConfig {
    ProptestConfig {
        cases: if full_depth() { 1_000 } else { 100 },
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource(
            "proptest-regressions",
        ))),
        ..ProptestConfig::default()
    }
}

/// A command with its targets still to be chosen from the song.
#[derive(Clone, Debug)]
enum Op {
    CreateTrack {
        kind: TrackKind,
        parent: Index,
        index: Index,
    },
    DeleteTrack(Index),
    MoveTrack {
        track: Index,
        parent: Index,
        index: Index,
    },
    RenameTrack(Index, u8),
    SetVolume(Index, f64),
    SetPan(Index, f64),
    SetMute(Index, bool),
    SetSolo(Index, bool),
    SetArm(Index, bool),
    /// A track's input (WP-2.2): the first byte chooses the source (0 a
    /// port, 1 All Ins, 2 the computer keyboard, 3 No Input) and the second
    /// the channel (0 All Channels, 1 to 16, past that refused).
    SetInput(Index, u8, u8),
    SetMonitor(Index, u8),
    SoloExclusive(Index),
    ArmExclusive(Index),
    /// A send from one track to another, usually a return.
    SetSend(Index, Index, f64),
    SetSendTap(Index, bool),
    SetSoloInPlace(bool),
    CreateClip {
        track: Index,
        start: u16,
        length: u16,
        data: Data,
    },
    DeleteClip(Index),
    MoveClip {
        clip: Index,
        track: Index,
        start: u16,
    },
    MoveClips(Vec<Index>, i16),
    SetClipLength(Index, u16),
    RenameClip(Index, u8),
    SetClipMute(Index, bool),
    SetClipLaunchMode(Index, u8),
    SetClipData(Index, Data),
    TrimClipStart(Index, i16),
    /// One of an audio clip's settings (WP-1.4): its markers, loop brace,
    /// Loop switch, gain, or fades, chosen by the byte, from two numbers.
    SetAudio(Index, u8, f64, f64),
    InsertDevice {
        track: Index,
        index: Index,
        params: Vec<(u8, f64)>,
    },
    RemoveDevice(Index),
    MoveDevice {
        device: Index,
        track: Index,
        index: Index,
    },
    RenameDevice(Index, u8),
    SetDeviceEnabled(Index, bool),
    SetDeviceParam(Index, u8, f64),
    SetTempo(f64),
    SetTimeSignature(u8, u8),
    SetTempoBreakpoint(i16, f64),
    SetTimeSignatureChange(i16, u8, u8),
    SetLoop(i16, u16),
    SetLoopEnabled(bool),
    CreateScene {
        index: Index,
    },
    DeleteScene(Index),
    MoveScene {
        scene: Index,
        index: Index,
    },
    RenameScene(Index, u8),
    /// Duplicating a scene copies whatever clips it holds, with new IDs the
    /// command itself works out when it's built.
    DuplicateScene(Index),
    CreateSlotClip {
        track: Index,
        scene: Index,
        data: Data,
    },
    MoveSlotClip {
        clip: Index,
        track: Index,
        scene: Index,
    },
}

/// Clip data: usually whatever fits the track, sometimes the other kind.
#[derive(Clone, Debug)]
struct Data {
    swap_kind: bool,
    media: u8,
    notes: Vec<(u8, u8, u8, u8)>,
}

fn data() -> impl Strategy<Value = Data> {
    (
        prop::bool::weighted(0.05),
        any::<u8>(),
        prop::collection::vec((0u8..=127, 0u8..=127, 0u8..64, 1u8..16), 0..6),
    )
        .prop_map(|(swap_kind, media, notes)| Data {
            swap_kind,
            media,
            notes,
        })
}

fn kind() -> impl Strategy<Value = TrackKind> {
    prop_oneof![
        4 => Just(TrackKind::Audio),
        4 => Just(TrackKind::Midi),
        3 => Just(TrackKind::Group),
        2 => Just(TrackKind::Return),
        1 => Just(TrackKind::Main),
    ]
}

/// Mostly in range, sometimes not.
fn value(range: core::ops::Range<f64>) -> impl Strategy<Value = f64> {
    prop_oneof![
        20 => range,
        1 => Just(-1.5),
        1 => Just(f64::NAN),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let i = any::<Index>;
    prop_oneof![
        4 => (kind(), i(), i()).prop_map(|(kind, parent, index)| Op::CreateTrack { kind, parent, index }),
        1 => i().prop_map(Op::DeleteTrack),
        2 => (i(), i(), i()).prop_map(|(track, parent, index)| Op::MoveTrack { track, parent, index }),
        1 => (i(), any::<u8>()).prop_map(|(t, n)| Op::RenameTrack(t, n)),
        2 => (i(), value(0.0..2.0)).prop_map(|(t, v)| Op::SetVolume(t, v)),
        2 => (i(), value(-1.0..1.0)).prop_map(|(t, v)| Op::SetPan(t, v)),
        1 => (i(), any::<bool>()).prop_map(|(t, b)| Op::SetMute(t, b)),
        1 => (i(), any::<bool>()).prop_map(|(t, b)| Op::SetSolo(t, b)),
        1 => (i(), any::<bool>()).prop_map(|(t, b)| Op::SetArm(t, b)),
        1 => (i(), 0u8..4, 0u8..18).prop_map(|(t, s, c)| Op::SetInput(t, s, c)),
        1 => (i(), 0u8..3).prop_map(|(t, m)| Op::SetMonitor(t, m)),
        1 => i().prop_map(Op::SoloExclusive),
        1 => i().prop_map(Op::ArmExclusive),
        3 => (i(), i(), value(0.0..1.0)).prop_map(|(t, to, v)| Op::SetSend(t, to, v)),
        1 => (i(), any::<bool>()).prop_map(|(t, b)| Op::SetSendTap(t, b)),
        1 => any::<bool>().prop_map(Op::SetSoloInPlace),
        5 => (i(), any::<u16>(), 1u16..256, data())
            .prop_map(|(track, start, length, data)| Op::CreateClip { track, start, length, data }),
        1 => i().prop_map(Op::DeleteClip),
        3 => (i(), i(), any::<u16>()).prop_map(|(clip, track, start)| Op::MoveClip { clip, track, start }),
        2 => (prop::collection::vec(i(), 1..5), any::<i16>()).prop_map(|(c, by)| Op::MoveClips(c, by)),
        2 => (i(), 0u16..256).prop_map(|(c, l)| Op::SetClipLength(c, l)),
        1 => (i(), any::<u8>()).prop_map(|(c, n)| Op::RenameClip(c, n)),
        1 => (i(), any::<bool>()).prop_map(|(c, b)| Op::SetClipMute(c, b)),
        1 => (i(), 0u8..4).prop_map(|(c, m)| Op::SetClipLaunchMode(c, m)),
        2 => (i(), data()).prop_map(|(c, d)| Op::SetClipData(c, d)),
        2 => (i(), -16i16..64).prop_map(|(c, by)| Op::TrimClipStart(c, by)),
        3 => (i(), any::<u8>(), value(0.0..4.0), value(0.0..4.0))
            .prop_map(|(c, which, a, b)| Op::SetAudio(c, which, a, b)),
        3 => (i(), i(), prop::collection::vec((0u8..8, value(-100.0..100.0)), 0..4))
            .prop_map(|(track, index, params)| Op::InsertDevice { track, index, params }),
        1 => i().prop_map(Op::RemoveDevice),
        2 => (i(), i(), i()).prop_map(|(device, track, index)| Op::MoveDevice { device, track, index }),
        1 => (i(), any::<u8>()).prop_map(|(d, n)| Op::RenameDevice(d, n)),
        1 => (i(), any::<bool>()).prop_map(|(d, b)| Op::SetDeviceEnabled(d, b)),
        3 => (i(), 0u8..8, value(-100.0..100.0)).prop_map(|(d, p, v)| Op::SetDeviceParam(d, p, v)),
        1 => (20.0..999.0).prop_map(Op::SetTempo),
        1 => (1u8..=12, 0u8..5).prop_map(|(n, d)| Op::SetTimeSignature(n, d)),
        // Sometimes before 1.1.1, which the document refuses.
        1 => (-2i16..64, 20.0..999.0).prop_map(|(at, bpm)| Op::SetTempoBreakpoint(at, bpm)),
        1 => (-2i16..64, 1u8..=12, 0u8..5)
            .prop_map(|(at, n, d)| Op::SetTimeSignatureChange(at, n, d)),
        // Sometimes before 1.1.1 or empty, which the document refuses.
        1 => (-2i16..64, 0u16..64).prop_map(|(start, length)| Op::SetLoop(start, length)),
        1 => any::<bool>().prop_map(Op::SetLoopEnabled),
        // Scenes and the clips in their slots (WP-3.1).
        1 => i().prop_map(|index| Op::CreateScene { index }),
        1 => i().prop_map(Op::DeleteScene),
        1 => (i(), i()).prop_map(|(scene, index)| Op::MoveScene { scene, index }),
        1 => (i(), any::<u8>()).prop_map(|(scene, n)| Op::RenameScene(scene, n)),
        1 => i().prop_map(Op::DuplicateScene),
        1 => (i(), i(), data())
            .prop_map(|(track, scene, data)| Op::CreateSlotClip { track, scene, data }),
        1 => (i(), i(), i())
            .prop_map(|(clip, track, scene)| Op::MoveSlotClip { clip, track, scene }),
    ]
}

/// The objects in a song, for choosing targets.
struct Targets {
    tracks: Vec<TrackId>,
    groups: Vec<TrackId>,
    clips: Vec<ClipId>,
    devices: Vec<DeviceId>,
    scenes: Vec<SceneId>,
    /// Tracks that can send to a return: audio, MIDI, and group tracks.
    senders: Vec<TrackId>,
    /// The return tracks.
    returns: Vec<TrackId>,
    /// The clips whose data is audio, for the audio-only clip commands.
    audio_clips: Vec<ClipId>,
    /// The clips on Arrangement lanes, for the commands that place a clip in
    /// the Arrangement.
    lane_clips: Vec<ClipId>,
}

impl Targets {
    fn of(doc: &Document) -> Self {
        let tracks = doc.track_order();
        let groups = tracks
            .iter()
            .copied()
            .filter(|t| doc.track(*t).unwrap().kind() == TrackKind::Group)
            .collect();
        let audio_clips = doc
            .clip_ids()
            .filter(|id| doc.clip(*id).is_some_and(|clip| clip.data.is_audio()))
            .collect();
        let lane_clips = doc
            .tracks()
            .flat_map(|track| track.clips().map(|clip| clip.id))
            .collect();
        Self {
            tracks,
            groups,
            clips: doc.clip_ids().collect(),
            devices: doc.device_ids().collect(),
            scenes: doc.scenes().map(|scene| scene.id).collect(),
            senders: doc
                .track_order()
                .into_iter()
                .filter(|&id| doc.track(id).is_some_and(|track| track.kind().has_sends()))
                .collect(),
            returns: doc.returns().collect(),
            audio_clips,
            lane_clips,
        }
    }

    fn track(&self, i: &Index) -> TrackId {
        *i.get(&self.tracks)
    }

    /// A group, or the top level.
    fn parent(&self, i: &Index) -> Option<TrackId> {
        match i.index(self.groups.len() + 1) {
            0 => None,
            n => Some(self.groups[n - 1]),
        }
    }

    /// An existing clip, or (if there are none) one that doesn't exist.
    fn clip(&self, i: &Index) -> ClipId {
        if self.clips.is_empty() {
            ClipId(Id::new(u64::MAX).unwrap())
        } else {
            *i.get(&self.clips)
        }
    }

    /// An existing audio clip, or one that doesn't exist.
    fn audio_clip(&self, i: &Index) -> ClipId {
        if self.audio_clips.is_empty() {
            ClipId(Id::new(u64::MAX).unwrap())
        } else {
            *i.get(&self.audio_clips)
        }
    }

    /// An existing clip on an Arrangement lane, or one that doesn't exist:
    /// the commands that place a clip in the Arrangement need one.
    fn lane_clip(&self, i: &Index) -> ClipId {
        if self.lane_clips.is_empty() {
            ClipId(Id::new(u64::MAX).unwrap())
        } else {
            *i.get(&self.lane_clips)
        }
    }

    /// An existing track that can send, or one that doesn't exist.
    fn sender(&self, i: &Index) -> TrackId {
        if self.senders.is_empty() {
            TrackId(Id::new(u64::MAX).unwrap())
        } else {
            *i.get(&self.senders)
        }
    }

    /// An existing return track, or one that doesn't exist.
    fn return_track(&self, i: &Index) -> TrackId {
        if self.returns.is_empty() {
            TrackId(Id::new(u64::MAX).unwrap())
        } else {
            *i.get(&self.returns)
        }
    }

    fn device(&self, i: &Index) -> DeviceId {
        if self.devices.is_empty() {
            DeviceId(Id::new(u64::MAX).unwrap())
        } else {
            *i.get(&self.devices)
        }
    }

    /// An existing scene, or (if there are none) one that doesn't exist.
    fn scene(&self, i: &Index) -> SceneId {
        if self.scenes.is_empty() {
            SceneId(Id::new(u64::MAX).unwrap())
        } else {
            *i.get(&self.scenes)
        }
    }

    /// A track that holds clips (audio or MIDI), or, if the song has none,
    /// any track: the target of a slot edit.
    fn clip_track(&self, i: &Index, doc: &Document) -> TrackId {
        self.fitting(i, doc, None)
    }

    /// A track whose kind fits the clip's data, or any track.
    fn fitting(&self, i: &Index, doc: &Document, audio: Option<bool>) -> TrackId {
        let candidates: Vec<TrackId> = self
            .tracks
            .iter()
            .copied()
            .filter(|&id| {
                doc.track(id).is_some_and(|track| {
                    track.kind().holds_clips()
                        && audio.is_none_or(|audio| (track.kind() == TrackKind::Audio) == audio)
                })
            })
            .collect();
        if candidates.is_empty() {
            self.track(i)
        } else {
            *i.get(&candidates)
        }
    }
}

/// An insertion index into a list of `len`, now and then one past the end.
fn slot(i: &Index, len: usize) -> usize {
    i.index(len + 2)
}

fn sixteenths(n: impl Into<i64>) -> Ticks {
    Ticks::SIXTEENTH * n.into()
}

fn clip_data(data: &Data, track_kind: Option<TrackKind>) -> ClipData {
    let audio = (track_kind == Some(TrackKind::Audio)) != data.swap_kind;
    if audio {
        ClipData::Audio(AudioData::new(ContentHash([data.media; 32])))
    } else {
        ClipData::Midi(MidiData::new(data.notes.iter().map(
            |&(key, velocity, start, length)| {
                Note::new(key, sixteenths(start), sixteenths(length), velocity)
            },
        )))
    }
}

impl Op {
    fn command(&self, editor: &mut Editor) -> Command {
        let doc = editor.document();
        let at = Targets::of(doc);
        let kind_of = |t: TrackId| doc.track(t).map(|t| t.kind());
        let list_len = |parent: Option<TrackId>, kind: TrackKind| match (kind, parent) {
            (TrackKind::Return, _) => doc.returns().len(),
            (_, None) => doc.top_level().len(),
            (_, Some(p)) => doc.track(p).unwrap().children().len(),
        };
        let chain_len = |t: TrackId| doc.track(t).unwrap().devices().len();
        let name = |n: u8| format!("name {n}");
        match self {
            Self::CreateTrack {
                kind,
                parent,
                index,
            } => {
                let parent = at.parent(parent);
                let index = slot(index, list_len(parent, *kind));
                Command::CreateTrack {
                    id: editor.new_id(),
                    kind: *kind,
                    name: name(0),
                    parent,
                    index,
                }
            }
            Self::DeleteTrack(t) => Command::DeleteTrack { track: at.track(t) },
            Self::MoveTrack {
                track,
                parent,
                index,
            } => {
                let track = at.track(track);
                let parent = at.parent(parent);
                let len = list_len(parent, kind_of(track).unwrap());
                Command::MoveTrack {
                    track,
                    parent,
                    index: slot(index, len.saturating_sub(1)),
                }
            }
            Self::RenameTrack(t, n) => Command::RenameTrack {
                track: at.track(t),
                name: name(*n),
            },
            Self::SetVolume(t, volume) => Command::SetVolume {
                track: at.track(t),
                volume: *volume,
            },
            Self::SetPan(t, pan) => Command::SetPan {
                track: at.track(t),
                pan: *pan,
            },
            Self::SetMute(t, mute) => Command::SetMute {
                track: at.track(t),
                mute: *mute,
            },
            Self::SetSolo(t, solo) => Command::SetSolo {
                track: at.track(t),
                solo: *solo,
            },
            Self::SoloExclusive(t) => Command::SoloExclusive { track: at.track(t) },
            Self::ArmExclusive(t) => Command::ArmExclusive { track: at.track(t) },
            Self::SetSend(t, to, level) => {
                // A track that can send, usually to a return.
                let to = if to.index(8) == 0 {
                    at.track(to)
                } else {
                    at.return_track(to)
                };
                Command::SetSend {
                    track: at.sender(t),
                    to,
                    level: *level,
                }
            }
            Self::SetSendTap(t, pre) => Command::SetSendTap {
                track: at.return_track(t),
                tap: if *pre { SendTap::Pre } else { SendTap::Post },
            },
            Self::SetSoloInPlace(enabled) => Command::SetSoloInPlace { enabled: *enabled },
            Self::SetArm(t, arm) => Command::SetArm {
                track: at.track(t),
                arm: *arm,
            },
            Self::SetInput(t, source, channel) => Command::SetInput {
                track: at.track(t),
                input: match source {
                    0 => Input::Port {
                        name: format!("Port {channel}"),
                    },
                    1 => Input::AllIns,
                    2 => Input::ComputerKeyboard,
                    _ => Input::None,
                },
                channel: (*channel > 0).then_some(*channel),
            },
            Self::SetMonitor(t, monitor) => Command::SetMonitor {
                track: at.track(t),
                monitor: match monitor {
                    0 => Monitor::In,
                    1 => Monitor::Auto,
                    _ => Monitor::Off,
                },
            },
            Self::CreateClip {
                track,
                start,
                length,
                data,
            } => {
                let track = at.track(track);
                let data = clip_data(data, kind_of(track));
                let mut clip = Clip::new(
                    editor.new_id(),
                    sixteenths(*start),
                    sixteenths(*length),
                    data,
                );
                clip.name = name(0);
                Command::CreateClip { track, clip }
            }
            Self::DeleteClip(c) => Command::DeleteClip { clip: at.clip(c) },
            Self::MoveClip { clip, track, start } => Command::MoveClip {
                clip: at.clip(clip),
                track: at.track(track),
                start: sixteenths(*start),
            },
            Self::MoveClips(clips, by) => Command::MoveClips {
                clips: clips
                    .iter()
                    .map(|c| at.lane_clip(c))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                by: sixteenths(*by),
            },
            Self::SetClipLength(c, length) => Command::SetClipLength {
                clip: at.clip(c),
                length: sixteenths(*length),
            },
            Self::RenameClip(c, n) => Command::RenameClip {
                clip: at.clip(c),
                name: name(*n),
            },
            Self::SetClipMute(c, mute) => Command::SetClipMute {
                clip: at.clip(c),
                mute: *mute,
            },
            Self::SetClipLaunchMode(c, mode) => Command::SetClipLaunchMode {
                clip: at.clip(c),
                mode: LaunchMode::ALL[usize::from(*mode)],
            },
            Self::SetClipData(c, data) => {
                let clip = at.clip(c);
                let track_kind = doc.clip_track(clip).and_then(kind_of);
                Command::SetClipData {
                    clip,
                    data: clip_data(data, track_kind),
                }
            }
            Self::TrimClipStart(c, by) => {
                // Mostly inside the clip, sometimes past either end.
                let clip = at.lane_clip(c);
                let start = doc.clip(clip).map_or(Ticks::ZERO, |clip| clip.start);
                Command::TrimClipStart {
                    clip,
                    start: start + sixteenths(*by),
                }
            }
            Self::SetAudio(c, which, a, b) => {
                let clip = at.audio_clip(c);
                // Past 3 seconds (or not a number), an end is the file's.
                let end = (*b <= 3.0).then_some(*b);
                // Warp edits (WP-4.1): most are refused on a clip that
                // isn't warped, or with these numbers, and must then change
                // nothing; the ones that land are one undo step.
                let ticks = Ticks((*a * 100_000.0) as i64);
                match which % 21 {
                    5 => {
                        return Command::SetClipWarp {
                            clip,
                            warp: *a > 1.0,
                        };
                    }
                    6 => {
                        return Command::InsertWarpMarker {
                            clips: vec![clip],
                            beat: Some(ticks),
                            time: None,
                            pin_neighbours: false,
                            transients: None,
                        };
                    }
                    7 => {
                        return Command::MoveWarpMarkers {
                            clips: vec![clip],
                            markers: vec![0],
                            by: ticks - Ticks(150_000),
                        };
                    }
                    8 => {
                        return Command::ShiftWarpMarker {
                            clips: vec![clip],
                            marker: 0,
                            by: *a - 1.0,
                        };
                    }
                    9 => {
                        return Command::DeleteWarpMarkers {
                            clips: vec![clip],
                            markers: vec![1],
                        };
                    }
                    10 => {
                        return Command::SetSegmentBpm {
                            clip,
                            marker: (*b > 1.0).then_some(0),
                            bpm: *a * 60.0,
                        };
                    }
                    11 => return Command::ScaleTempo { clip, factor: *a },
                    // Transient edits and ranged markers (WP-4.3).
                    12 => {
                        return Command::InsertTransient {
                            clip,
                            time: *a - 1.0,
                        };
                    }
                    13 => {
                        return Command::DeleteTransients {
                            clip,
                            times: vec![*a - 1.0, *b],
                        };
                    }
                    14 => {
                        return Command::MoveTransient {
                            clip,
                            from: *a - 1.0,
                            to: *b,
                        };
                    }
                    15 => return Command::ResetTransients { clip },
                    16 => {
                        return Command::InsertWarpMarkers {
                            clip,
                            from: ticks - Ticks(150_000),
                            to: ticks + Ticks(150_000),
                            transients: Some(vec![*a, *b]),
                        };
                    }
                    // Warp modes (WP-4.2): a value out of range is refused
                    // and changes nothing.
                    17 => {
                        return Command::SetClipWarpMode {
                            clip,
                            mode: match (*a * 3.0) as u8 % 6 {
                                0 => WarpMode::Beats,
                                1 => WarpMode::Tones,
                                2 => WarpMode::Texture,
                                3 => WarpMode::RePitch,
                                4 => WarpMode::Complex,
                                _ => WarpMode::ComplexPro,
                            },
                        };
                    }
                    18 => {
                        return Command::SetClipWarpSettings {
                            clip,
                            beats: Some(BeatsUpdate {
                                transient_envelope: Some((*a * 40.0) as u32),
                                ..BeatsUpdate::default()
                            }),
                            tones: (*b > 1.0).then_some(TonesUpdate {
                                grain_size: Some(*a * 40.0),
                            }),
                            texture: None,
                            complex_pro: None,
                        };
                    }
                    19 => {
                        return Command::SetClipRamMode {
                            clip,
                            ram: *a > 1.0,
                        };
                    }
                    20 => {
                        return Command::SetClipHighQuality {
                            clip,
                            high_quality: *a > 1.0,
                        };
                    }
                    _ => {}
                }
                match which % 5 {
                    0 => Command::SetClipRegion {
                        clip,
                        start: *a,
                        end,
                        ticks: false,
                    },
                    1 => Command::SetClipLoop {
                        clip,
                        start: *a,
                        end,
                        ticks: false,
                    },
                    2 => Command::SetClipLoopEnabled {
                        clip,
                        enabled: *a > 1.0,
                    },
                    3 => Command::SetClipGain { clip, gain: *a },
                    _ => Command::SetClipFades {
                        clip,
                        fade_in: *a,
                        fade_out: *b,
                    },
                }
            }
            Self::InsertDevice {
                track,
                index,
                params,
            } => {
                let track = at.track(track);
                let index = slot(index, chain_len(track));
                let kind = if params.len() % 2 == 0 {
                    DeviceKind::Native {
                        device_type: "beatlayer.test".to_owned(),
                    }
                } else {
                    DeviceKind::Plugin {
                        format: PluginFormat::Clap,
                        plugin_id: "org.example.test".to_owned(),
                    }
                };
                let mut device = Device::new(editor.new_id(), name(0), kind);
                for (param, value) in params {
                    device.params.insert(ParamId((*param).into()), *value);
                }
                Command::InsertDevice {
                    track,
                    index,
                    device,
                }
            }
            Self::RemoveDevice(d) => Command::RemoveDevice {
                device: at.device(d),
            },
            Self::MoveDevice {
                device,
                track,
                index,
            } => {
                let device = at.device(device);
                let track = at.track(track);
                let len = chain_len(track) - usize::from(doc.device_track(device) == Some(track));
                Command::MoveDevice {
                    device,
                    track,
                    index: slot(index, len),
                }
            }
            Self::RenameDevice(d, n) => Command::RenameDevice {
                device: at.device(d),
                name: name(*n),
            },
            Self::SetDeviceEnabled(d, enabled) => Command::SetDeviceEnabled {
                device: at.device(d),
                enabled: *enabled,
            },
            Self::SetDeviceParam(d, param, value) => Command::SetDeviceParam {
                device: at.device(d),
                param: ParamId((*param).into()),
                value: *value,
            },
            Self::SetTempo(bpm) => Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(*bpm).unwrap()),
            },
            Self::SetTimeSignature(numerator, d) => Command::SetTimeSignatures {
                time_signatures: TimeSignatureMap::constant(
                    TimeSignature::new(*numerator, 1 << d).unwrap(),
                ),
            },
            Self::SetTempoBreakpoint(at, bpm) => Command::SetTempoBreakpoint {
                at: Ticks::QUARTER * i64::from(*at),
                bpm: Bpm::new(*bpm).unwrap(),
            },
            Self::SetTimeSignatureChange(at, numerator, d) => Command::SetTimeSignatureChange {
                at: Ticks::QUARTER * i64::from(*at),
                signature: TimeSignature::new(*numerator, 1 << d).unwrap(),
            },
            Self::SetLoop(start, length) => Command::SetLoop {
                start: Ticks::QUARTER * i64::from(*start),
                length: Ticks::QUARTER * i64::from(*length),
            },
            Self::SetLoopEnabled(enabled) => Command::SetLoopEnabled { enabled: *enabled },
            Self::CreateScene { index } => {
                let scene_count = doc.scene_count();
                Command::CreateScene {
                    id: editor.new_id(),
                    index: slot(index, scene_count),
                }
            }
            Self::DeleteScene(s) => Command::DeleteScene { scene: at.scene(s) },
            Self::MoveScene { scene, index } => Command::MoveScene {
                scene: at.scene(scene),
                index: slot(index, doc.scene_count().saturating_sub(1)),
            },
            Self::RenameScene(s, n) => Command::RenameScene {
                scene: at.scene(s),
                name: name(*n),
            },
            Self::DuplicateScene(s) => {
                let scene = at.scene(s);
                let tracks: Vec<TrackId> = doc
                    .track_order()
                    .into_iter()
                    .filter(|&track| doc.slot(track, scene).is_some())
                    .collect();
                let clips = tracks
                    .into_iter()
                    .map(|track| (track, editor.new_id()))
                    .collect();
                Command::DuplicateScene {
                    scene,
                    id: editor.new_id(),
                    clips,
                }
            }
            Self::CreateSlotClip { track, scene, data } => {
                let track = at.clip_track(track, doc);
                let scene = at.scene(scene);
                let data = clip_data(data, kind_of(track));
                // A slot clip has no position in the Arrangement.
                let clip = Clip::new(editor.new_id(), Ticks::ZERO, Ticks::WHOLE, data);
                Command::CreateSlotClip { track, scene, clip }
            }
            Self::MoveSlotClip { clip, track, scene } => {
                let clip = at.clip(clip);
                let audio = doc.clip(clip).map(|clip| clip.data.is_audio());
                Command::MoveSlotClip {
                    clip,
                    track: at.fitting(track, doc, audio),
                    scene: at.scene(scene),
                }
            }
        }
    }
}

/// A small song to start from: a group holding an audio track, a MIDI track,
/// another audio track, and a return, with clips and devices, and an empty
/// history.
fn starting_song() -> Editor {
    let mut editor = Editor::new();
    let track = |editor: &mut Editor, kind, parent, index| {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: format!("{kind}"),
                parent,
                index,
            })
            .unwrap();
        id
    };
    let group = track(&mut editor, TrackKind::Group, None, 0);
    let audio = track(&mut editor, TrackKind::Audio, Some(group), 0);
    let midi = track(&mut editor, TrackKind::Midi, None, 1);
    track(&mut editor, TrackKind::Audio, None, 2);
    let ret = track(&mut editor, TrackKind::Return, None, 0);
    for (track, start) in [(audio, 0), (audio, 16), (midi, 0), (midi, 32)] {
        let data = Data {
            swap_kind: false,
            media: 1,
            notes: vec![(60, 100, 0, 4), (64, 90, 4, 4)],
        };
        let kind = editor.document().track(track).unwrap().kind();
        let clip = Clip::new(
            editor.new_id(),
            sixteenths(start),
            Ticks::WHOLE,
            clip_data(&data, Some(kind)),
        );
        editor.apply(Command::CreateClip { track, clip }).unwrap();
    }
    // A clip in a Session slot, so the slot edits have one to work on.
    let scene = editor
        .document()
        .scenes()
        .next()
        .expect("the default scenes")
        .id;
    for track in [audio, midi] {
        let data = Data {
            swap_kind: false,
            media: 2,
            notes: vec![(60, 100, 0, 4)],
        };
        let kind = editor.document().track(track).unwrap().kind();
        let clip = Clip::new(
            editor.new_id(),
            Ticks::ZERO,
            Ticks::WHOLE,
            clip_data(&data, Some(kind)),
        );
        editor
            .apply(Command::CreateSlotClip { track, scene, clip })
            .unwrap();
    }
    let main = editor.document().main_id();
    for track in [audio, midi, ret, main] {
        let device = Device::new(
            editor.new_id(),
            "Utility",
            DeviceKind::Native {
                device_type: "beatlayer.utility".to_owned(),
            },
        );
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device,
            })
            .unwrap();
    }
    editor.clear_history().unwrap();
    editor
}

/// A track's own settings: name, mixer, sends, children, chain order, and
/// routing.
type OwnSettings = (
    String,
    Mixer,
    Vec<(TrackId, f64)>,
    Vec<TrackId>,
    Vec<DeviceId>,
    Routing,
);

/// A song flattened into plain maps, for working out change sets the slow
/// way.
struct Flat {
    main: TrackId,
    top: Vec<TrackId>,
    returns: Vec<TrackId>,
    tracks: BTreeMap<TrackId, OwnSettings>,
    scenes: BTreeMap<SceneId, (usize, String)>,
    /// Each clip and where it sits: its track, the scene whose slot holds it
    /// (`null` for an Arrangement lane), and the clip.
    clips: BTreeMap<ClipId, (TrackId, Option<SceneId>, Clip)>,
    devices: BTreeMap<DeviceId, (TrackId, Device)>,
}

impl Flat {
    fn of(doc: &Document) -> Self {
        let mut flat = Self {
            main: doc.main_id(),
            top: doc.top_level().collect(),
            returns: doc.returns().collect(),
            tracks: BTreeMap::new(),
            scenes: doc
                .scenes()
                .enumerate()
                .map(|(position, scene)| (scene.id, (position, scene.name.clone())))
                .collect(),
            clips: BTreeMap::new(),
            devices: BTreeMap::new(),
        };
        for track in doc.tracks() {
            let devices = track.devices().map(|d| d.id).collect();
            let own = (
                track.name().to_owned(),
                *track.mixer(),
                track.sends().collect(),
                track.children().collect(),
                devices,
                track.routing().clone(),
            );
            flat.tracks.insert(track.id(), own);
            for clip in track.clips() {
                flat.clips.insert(clip.id, (track.id(), None, clip.clone()));
            }
            for (scene, clip) in track.slots() {
                flat.clips
                    .insert(clip.id, (track.id(), Some(scene), clip.clone()));
            }
            for device in track.devices() {
                flat.devices.insert(device.id, (track.id(), device.clone()));
            }
        }
        flat
    }
}

fn diff<K: Ord + Copy, V: PartialEq>(
    old: &BTreeMap<K, V>,
    new: &BTreeMap<K, V>,
) -> BTreeMap<K, Change> {
    let mut changes = BTreeMap::new();
    for (id, value) in old {
        match new.get(id) {
            None => {
                changes.insert(*id, Change::Removed);
            }
            Some(other) if other != value => {
                changes.insert(*id, Change::Modified);
            }
            Some(_) => {}
        }
    }
    for id in new.keys() {
        if !old.contains_key(id) {
            changes.insert(*id, Change::Added);
        }
    }
    changes
}

/// The change set between two versions, worked out without pointer sharing.
fn slow_changes(old: &Document, new: &Document) -> ChangeSet {
    let (a, b) = (Flat::of(old), Flat::of(new));
    let children = |flat: &Flat| -> BTreeMap<TrackId, Vec<TrackId>> {
        flat.tracks
            .iter()
            .map(|(id, own)| (*id, own.3.clone()))
            .collect()
    };
    ChangeSet {
        settings: old.settings() != new.settings(),
        layout: a.main != b.main
            || a.top != b.top
            || a.returns != b.returns
            || children(&a) != children(&b),
        tracks: diff(&a.tracks, &b.tracks),
        scenes: diff(&a.scenes, &b.scenes),
        clips: diff(&a.clips, &b.clips),
        devices: diff(&a.devices, &b.devices),
    }
}

fn json(doc: &Document) -> String {
    serde_json::to_string(doc).unwrap()
}

/// Checks one applied command: the result, the change set, and the rules.
fn check_applied(
    before: &Document,
    result: Result<ChangeSet, EditError>,
    editor: &Editor,
) -> Result<Option<ChangeSet>, TestCaseError> {
    let after = editor.document();
    match result {
        Ok(changes) => {
            after.check_invariants().map_err(TestCaseError::fail)?;
            prop_assert_eq!(&changes, &ChangeSet::between(before, after));
            prop_assert_eq!(&changes, &slow_changes(before, after));
            prop_assert_eq!(changes.is_empty(), before == after);
            Ok(Some(changes))
        }
        Err(_) => {
            prop_assert_eq!(before, after);
            Ok(None)
        }
    }
}

/// Undoes every step, checking each version on the way, then redoes them.
fn undo_and_redo_all(editor: &mut Editor, versions: &[Document]) -> Result<(), TestCaseError> {
    let start = &versions[0];
    let end = versions.last().unwrap();
    let (start_json, end_json) = (json(start), json(end));
    prop_assert_eq!(editor.document(), end);
    prop_assert_eq!(editor.undo_history().len(), versions.len() - 1);

    for expected in versions.iter().rev().skip(1) {
        editor
            .undo()
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(editor.document(), expected);
    }
    prop_assert_eq!(editor.undo(), Err(EditError::NothingToUndo));
    prop_assert_eq!(editor.document(), start);
    prop_assert_eq!(json(editor.document()), start_json);

    for expected in versions.iter().skip(1) {
        editor
            .redo()
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(editor.document(), expected);
    }
    prop_assert_eq!(editor.redo(), Err(EditError::NothingToRedo));
    prop_assert_eq!(editor.document(), end);
    prop_assert_eq!(json(editor.document()), end_json.as_str());

    let reloaded: Document = serde_json::from_str(&end_json).unwrap();
    prop_assert_eq!(&reloaded, end);
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    // Spec: session-view R12
    /// The acceptance property: undoing every command gets the starting
    /// document back, and redoing them gets the final one.
    #[test]
    fn undo_all_then_redo_all(ops in prop::collection::vec(op(), 1..=200)) {
        let mut editor = starting_song();
        let mut versions = vec![editor.document().clone()];
        let mut created = BTreeSet::new();
        for op in &ops {
            let before = editor.document().clone();
            let command = op.command(&mut editor);
            let new_ids = command.created_ids();
            let result = editor.apply(command);
            if let Some(changes) = check_applied(&before, result, &editor)? {
                for id in new_ids {
                    // Every object ever created has its own ID.
                    prop_assert!(created.insert(id));
                }
                if !changes.is_empty() {
                    versions.push(editor.document().clone());
                }
            }
        }
        undo_and_redo_all(&mut editor, &versions)?;

        // IDs used by undone objects stay used.
        editor.clear_history().unwrap();
        for id in created.iter().take(3) {
            let reuse = Command::CreateTrack {
                id: TrackId(*id),
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            };
            prop_assert_eq!(editor.apply(reuse), Err(EditError::IdNotIssued(*id)));
        }
    }

    /// Commands grouped into random, sometimes nested, sometimes cancelled
    /// transactions: each outermost commit that changed something is one
    /// step, a cancel puts the document back, and undoing every step still
    /// gets the start back.
    #[test]
    fn transactions_are_steps(steps in prop::collection::vec(prop_oneof![
        8 => op().prop_map(Some),
        3 => Just(None),
    ], 1..=200), closes in prop::collection::vec(any::<bool>(), 200)) {
        let mut editor = starting_song();
        let mut versions = vec![editor.document().clone()];
        let mut open: Vec<Document> = Vec::new();
        let mut closes = closes.into_iter().cycle();
        for step in &steps {
            match step {
                Some(op) => {
                    let before = editor.document().clone();
                    let command = op.command(&mut editor);
                    let result = editor.apply(command);
                    let changes = check_applied(&before, result, &editor)?;
                    if open.is_empty() && changes.is_some_and(|c| !c.is_empty()) {
                        versions.push(editor.document().clone());
                    }
                }
                // Open a transaction, or close one: commit or cancel.
                None if open.is_empty() || closes.next().unwrap() => {
                    editor.begin("Gesture");
                    open.push(editor.document().clone());
                }
                None => {
                    let start = open.pop().unwrap();
                    if closes.next().unwrap() {
                        let recorded = editor.commit().unwrap();
                        let expected = open.is_empty() && start != *editor.document();
                        prop_assert_eq!(recorded, expected);
                        if recorded {
                            versions.push(editor.document().clone());
                        }
                    } else {
                        editor.cancel().unwrap();
                        prop_assert_eq!(editor.document(), &start);
                    }
                }
            }
        }
        while let Some(start) = open.pop() {
            if editor.commit().unwrap() {
                prop_assert!(open.is_empty() && start != *editor.document());
                versions.push(editor.document().clone());
            }
        }
        prop_assert_eq!(editor.commit(), Err(EditError::NoTransaction));
        undo_and_redo_all(&mut editor, &versions)?;
    }
}

/// Most generated commands succeed, so the properties test real edits rather
/// than a stream of rejections.
#[test]
fn generated_commands_mostly_apply() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let mut editor = starting_song();
    let (mut applied, mut total) = (0, 0);
    for _ in 0..5_000 {
        let op = op().new_tree(&mut runner).unwrap().current();
        let command = op.command(&mut editor);
        total += 1;
        if editor.apply(command).is_ok() {
            applied += 1;
        }
    }
    let doc = editor.document();
    println!(
        "{applied} of {total} generated commands applied; the song ended with {} tracks, {} clips, and {} devices",
        doc.track_count(),
        doc.clip_count(),
        doc.device_count()
    );
    assert!(
        applied * 10 >= total * 7,
        "only {applied} of {total} applied"
    );
}
