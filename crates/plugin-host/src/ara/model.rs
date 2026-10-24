//! What an ARA document controller is told about the set (the ARA hosting
//! spec, R2 to R5), worked out from the document alone: the musical context's
//! tempo and signatures, a region sequence for each track the plug-in is on,
//! an audio source for each file those tracks play, an audio modification for
//! each clip, and the playback regions that put the modifications on the
//! timeline. Nothing here calls a plug-in, so the mapping is tested without
//! one.

use std::collections::BTreeMap;

use bl_core::time::{TICKS_PER_QUARTER, TempoMap, TempoSegment, Ticks, TimeSignatureMap};
use bl_model::{
    AraSupport, AudioData, Clip, ClipData, ClipId, ContentHash, Device, Document, TrackId,
    ara_device, ara_renders,
};

/// Playback regions of a warped clip stop at this many pieces; past it the
/// clip's last piece runs on at its own speed. A clip that long has more
/// tempo changes and warp markers than any song.
const MAX_PIECES: usize = 4096;

/// A playback region plays its audio as it is.
pub const NO_CHANGES: i32 = 0;
/// A playback region plays its audio faster or slower than the audio runs.
pub const TIMESTRETCH: i32 = 1;

/// A track the plug-in is on.
#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    pub track: TrackId,
    pub name: String,
    /// The track's place among the plug-in's tracks.
    pub order: i32,
}

/// A file the plug-in's tracks play.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub media: ContentHash,
    pub name: String,
}

/// A clip's own version of its file's audio, where the plug-in keeps its
/// edits of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Modification {
    pub clip: ClipId,
    pub media: ContentHash,
    pub name: String,
}

/// One straight stretch of a clip's playback: a part of the audio put at a
/// place on the timeline. Times are seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub clip: ClipId,
    /// Which stretch of the clip: 0 for an unwarped clip, and for a warped
    /// one a piece for each warp marker, tempo change and ramp step.
    pub piece: u32,
    pub track: TrackId,
    pub flags: i32,
    pub start_in_modification: f64,
    pub duration_in_modification: f64,
    pub start_in_playback: f64,
    pub duration_in_playback: f64,
    pub name: String,
}

/// Names a region: the clip it is of, and which piece.
pub type RegionKey = (ClipId, u32);

/// What one ARA plug-in is told about the set.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub tempo: TempoMap,
    pub signatures: TimeSignatureMap,
    pub sequences: Vec<Sequence>,
    pub sources: BTreeMap<ContentHash, Source>,
    pub modifications: BTreeMap<ClipId, Modification>,
    pub regions: BTreeMap<RegionKey, Region>,
}

impl Model {
    /// A model with nothing in it, under the set's tempo and signatures.
    #[must_use]
    pub fn empty(document: &Document) -> Self {
        Self {
            tempo: document.settings().tempo.clone(),
            signatures: document.settings().time_signatures.clone(),
            sequences: Vec::new(),
            sources: BTreeMap::new(),
            modifications: BTreeMap::new(),
            regions: BTreeMap::new(),
        }
    }

    /// The model for the ARA plug-in `plugin`, over the tracks it is the ARA
    /// device of (R3): `hosts` says which devices are ARA plug-ins, with
    /// their factory IDs and what they support, and `file_seconds` how long
    /// a file plays, for a clip whose End marker is the file's end.
    ///
    /// A track belongs to the first ARA device of its chain, as the engine's
    /// compiler decides it, so what the plug-in renders and what the clip
    /// player leaves out are the same clips.
    #[must_use]
    pub fn of(
        document: &Document,
        plugin: &str,
        hosts: &dyn Fn(&Device) -> Option<(String, AraSupport)>,
        file_seconds: &mut dyn FnMut(ContentHash) -> Option<f64>,
    ) -> Self {
        let mut model = Self::empty(document);
        let tempo = &model.tempo.clone();
        for track in document.tracks() {
            let Some((device, support)) = ara_device(track, |d| hosts(d).map(|(_, s)| s)) else {
                continue;
            };
            if hosts(device).is_none_or(|(id, _)| id != plugin) {
                continue;
            }
            model.sequences.push(Sequence {
                track: track.id(),
                name: track.name().to_owned(),
                order: i32::try_from(model.sequences.len()).unwrap_or(i32::MAX),
            });
            for clip in track.playback_clips() {
                if !ara_renders(&clip, support) {
                    continue;
                }
                let ClipData::Audio(audio) = &clip.data else {
                    continue;
                };
                let Some(file) = file_seconds(audio.media) else {
                    continue;
                };
                let regions = if audio.warp {
                    warped(&clip, audio, tempo, file)
                } else {
                    unwarped(&clip, audio, tempo, file)
                };
                if regions.is_empty() {
                    continue;
                }
                model.sources.entry(audio.media).or_insert_with(|| Source {
                    media: audio.media,
                    name: media_name(audio.media),
                });
                model.modifications.insert(
                    clip.id,
                    Modification {
                        clip: clip.id,
                        media: audio.media,
                        name: clip.name.clone(),
                    },
                );
                for (piece, times) in regions.into_iter().enumerate() {
                    let piece = u32::try_from(piece).unwrap_or(u32::MAX);
                    model.regions.insert(
                        (clip.id, piece),
                        Region {
                            clip: clip.id,
                            piece,
                            track: track.id(),
                            flags: times.flags,
                            start_in_modification: times.modification.0,
                            duration_in_modification: times.modification.1,
                            start_in_playback: times.playback.0,
                            duration_in_playback: times.playback.1,
                            name: clip.name.clone(),
                        },
                    );
                }
            }
        }
        model
    }
}

/// A source's name: its hash's first digits, as no file name is known here.
fn media_name(media: ContentHash) -> String {
    let hex: String = media.0.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("media {hex}")
}

/// The times of one stretch of a clip's playback, before it is put on a
/// track as a [`Region`]: its flags, then its start and duration in the
/// audio, then its start and duration on the timeline.
struct Piece {
    flags: i32,
    modification: (f64, f64),
    playback: (f64, f64),
}

/// An unwarped clip plays its file at its own speed, from the Start marker
/// until the clip or the End marker ends (R4).
fn unwarped(clip: &Clip, audio: &AudioData, tempo: &TempoMap, file: f64) -> Vec<Piece> {
    let begin = tempo.ticks_to_seconds(clip.start);
    let length = tempo.ticks_to_seconds(clip.end()) - begin;
    let end = audio.end.unwrap_or(file).min(file);
    let duration = length.min(end - audio.start);
    if duration <= 0.0 {
        return Vec::new();
    }
    vec![Piece {
        flags: NO_CHANGES,
        modification: (audio.start, duration),
        playback: (begin, duration),
    }]
}

/// A warped clip plays the audio between its Start and End markers over the
/// clip's length, at the speed its warp map and the tempo map give. Each
/// stretch over which that speed is steady is a region of its own (R5): a
/// piece for each warp marker and tempo change, and for each quarter note of
/// a tempo ramp, which the straight pieces follow.
fn warped(clip: &Clip, audio: &AudioData, tempo: &TempoMap, file: f64) -> Vec<Piece> {
    let Ok(map) = audio.warp_map() else {
        return Vec::new();
    };
    let first = audio.start;
    let last = audio
        .end
        .unwrap_or_else(|| map.time_to_beat(file))
        .min(map.time_to_beat(file));
    let beats = (last - first).floor();
    if beats < 1.0 {
        return Vec::new();
    }
    let from = clip.start.get();
    let to = from.saturating_add(clip.length.get().min(beats as i64));
    // The song ticks the speed can change at.
    let mut breaks = vec![from, to];
    breaks.extend(map.markers().iter().filter_map(|marker| {
        let tick = from + (marker.beat as f64 - first).round() as i64;
        (from < tick && tick < to).then_some(tick)
    }));
    let segments: Vec<TempoSegment> = tempo.segments().collect();
    for (at, segment) in segments.iter().enumerate() {
        let start = segment.start().get();
        if from < start && start < to {
            breaks.push(start);
        }
        if let TempoSegment::Ramp { .. } = segment {
            let end = segments
                .get(at + 1)
                .map_or(to, |next| next.start().get())
                .min(to);
            let mut tick = start.max(from) + TICKS_PER_QUARTER;
            while tick < end && breaks.len() < MAX_PIECES {
                breaks.push(tick);
                tick += TICKS_PER_QUARTER;
            }
        }
    }
    breaks.sort_unstable();
    breaks.dedup();
    breaks
        .windows(2)
        .take(MAX_PIECES)
        .map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let seconds = |tick: i64| tempo.ticks_to_seconds(Ticks(tick));
            let time = |tick: i64| map.beat_to_time(first + (tick - from) as f64);
            Piece {
                flags: TIMESTRETCH,
                modification: (time(a), time(b) - time(a)),
                playback: (seconds(a), seconds(b) - seconds(a)),
            }
        })
        .filter(|piece| piece.playback.1 > 0.0 && piece.modification.1 > 0.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::host;
    use super::*;
    use bl_core::time::{Bpm, TempoMap, TempoSegment};
    use bl_model::{
        AudioData, Clip, ClipData, Command, DeviceKind, Editor, PluginFormat, TrackKind, WarpMarker,
    };

    const QUARTER: i64 = TICKS_PER_QUARTER;
    const FILE: ContentHash = ContentHash([7; 32]);

    fn kind(id: &str) -> DeviceKind {
        DeviceKind::Plugin {
            format: PluginFormat::Vst3,
            plugin_id: id.to_owned(),
        }
    }

    /// A set with an audio track holding the ARA plug-in `plugin`.
    struct Set {
        editor: Editor,
        track: TrackId,
    }

    impl Set {
        fn new(plugin: &str) -> Self {
            let mut editor = Editor::new();
            let track = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id: track,
                    kind: TrackKind::Audio,
                    name: "Vocal".to_owned(),
                    parent: None,
                    index: 0,
                })
                .unwrap();
            let device = editor.new_id();
            editor
                .apply(Command::InsertDevice {
                    track,
                    index: 0,
                    device: Device::new(device, "Plug-in", kind(plugin)),
                })
                .unwrap();
            Self { editor, track }
        }

        fn clip(&mut self, start: i64, length: i64, audio: AudioData) -> ClipId {
            let clip = self.editor.new_id();
            self.editor
                .apply(Command::CreateClip {
                    track: self.track,
                    clip: Clip::new(clip, Ticks(start), Ticks(length), ClipData::Audio(audio)),
                })
                .unwrap();
            clip
        }

        fn model(&self, plugin: &str, stretch: bool) -> Model {
            let hosts = |device: &Device| match &device.kind {
                DeviceKind::Plugin { plugin_id, .. } => Some((
                    plugin_id.clone(),
                    AraSupport {
                        timestretch: stretch,
                    },
                )),
                _ => None,
            };
            Model::of(self.editor.document(), plugin, &hosts, &mut |_| Some(2.0))
        }
    }

    // Spec: ara-hosting R3
    #[test]
    fn the_musical_context_gives_the_tempo_and_signatures_as_sync_points() {
        let tempo = TempoMap::new([
            TempoSegment::Constant {
                start: Ticks(0),
                bpm: Bpm::new(120.0).unwrap(),
            },
            TempoSegment::Constant {
                start: Ticks(4 * QUARTER),
                bpm: Bpm::new(60.0).unwrap(),
            },
        ])
        .unwrap();
        let content = host::Content::of(&tempo, &bl_core::time::TimeSignatureMap::default());
        // Quarter 0, the change at quarter 4 (two seconds in), and a quarter
        // past it, which the final tempo is read from.
        assert_eq!(
            content.tempo_entries(),
            vec![(0.0, 0.0), (2.0, 4.0), (3.0, 5.0)]
        );
    }

    // Spec: ara-hosting R4, R5
    #[test]
    fn an_unwarped_clip_is_one_region_at_its_place_playing_the_file_at_its_own_speed() {
        let mut set = Set::new("A");
        let mut audio = AudioData::new(FILE);
        audio.start = 0.25;
        audio.end = Some(1.25);
        // Two beats at 120 BPM are a second, and the clip starts one beat in.
        let clip = set.clip(QUARTER, 8 * QUARTER, audio);
        let model = set.model("A", false);
        assert_eq!(model.sequences.len(), 1);
        assert_eq!(model.sources.len(), 1);
        assert_eq!(model.modifications.len(), 1);
        let region = &model.regions[&(clip, 0)];
        assert_eq!(region.start_in_modification, 0.25);
        // The clip is longer than the audio between the markers: the audio
        // ends the region.
        assert_eq!(region.duration_in_modification, 1.0);
        assert_eq!(
            (region.start_in_playback, region.duration_in_playback),
            (0.5, 1.0)
        );
        assert_eq!(region.flags, NO_CHANGES);
    }

    // Spec: ara-hosting R4
    #[test]
    fn a_clip_shorter_than_its_audio_plays_part_of_it() {
        let mut set = Set::new("A");
        let clip = set.clip(0, QUARTER, AudioData::new(FILE));
        let model = set.model("A", false);
        let region = &model.regions[&(clip, 0)];
        assert_eq!(
            (region.duration_in_modification, region.duration_in_playback),
            (0.5, 0.5)
        );
    }

    // Spec: ara-hosting R4
    #[test]
    fn clips_the_plugin_cant_render_stay_out_of_its_document() {
        let mut set = Set::new("A");
        let mut quiet = AudioData::new(FILE);
        quiet.gain = 0.5;
        let mut looped = AudioData::new(FILE);
        looped.looping = true;
        let mut warped = AudioData::new(FILE);
        warped.warp = true;
        warped.warp_markers = vec![WarpMarker {
            time: 0.0,
            beat: Ticks(0),
        }];
        warped.tempo = Some(120.0);
        let plain = set.clip(0, QUARTER, AudioData::new(FILE));
        for audio in [quiet, looped, warped] {
            set.clip(2 * QUARTER, QUARTER, audio);
        }
        let model = set.model("A", false);
        assert_eq!(
            model.regions.keys().copied().collect::<Vec<_>>(),
            vec![(plain, 0)]
        );
        assert_eq!(model.modifications.len(), 1);
    }

    // Spec: ara-hosting R3
    #[test]
    fn a_track_belongs_to_the_plugin_that_is_first_in_its_chain() {
        let mut set = Set::new("A");
        let other = set.editor.new_id();
        set.editor
            .apply(Command::InsertDevice {
                track: set.track,
                index: 1,
                device: Device::new(other, "Other", kind("B")),
            })
            .unwrap();
        set.clip(0, QUARTER, AudioData::new(FILE));
        assert_eq!(set.model("A", false).sequences.len(), 1);
        assert!(set.model("B", false).sequences.is_empty());
        assert!(set.model("C", false).regions.is_empty());
    }

    // Spec: ara-hosting R5
    #[test]
    fn a_warped_clip_is_a_region_for_each_straight_stretch_of_its_warp_map() {
        let mut set = Set::new("A");
        // The file's first second is squeezed into a beat (half a second at
        // 120 BPM); after that marker it plays at the clip's tempo of 120.
        let mut audio = AudioData::new(FILE);
        audio.warp = true;
        audio.warp_markers = vec![
            WarpMarker {
                time: 0.0,
                beat: Ticks(0),
            },
            WarpMarker {
                time: 1.0,
                beat: Ticks(QUARTER),
            },
        ];
        audio.tempo = Some(120.0);
        audio.start = 0.0;
        audio.end = None;
        let clip = set.clip(0, 8 * QUARTER, audio);
        let model = set.model("A", true);
        let pieces: Vec<_> = model
            .regions
            .values()
            .map(|r| {
                (
                    r.piece,
                    r.start_in_modification,
                    r.duration_in_modification,
                    r.start_in_playback,
                    r.duration_in_playback,
                )
            })
            .collect();
        // The file is two seconds long, so it ends on beat 3.
        assert_eq!(
            pieces,
            vec![(0, 0.0, 1.0, 0.0, 0.5), (1, 1.0, 1.0, 0.5, 1.0)]
        );
        assert!(model.regions.values().all(|r| r.flags == TIMESTRETCH));
        assert!(model.regions.keys().all(|(c, _)| *c == clip));
    }
}
