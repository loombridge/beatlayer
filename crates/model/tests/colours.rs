//! The colours of tracks, clips and scenes in the document (WP-6.30b; the
//! themes spec, R14 to R17, R19 to R21, R25, R27).

use bl_core::time::Ticks;
use bl_model::{
    AudioData, ChangeSet, Clip, ClipData, ClipId, Command, ContentHash, Document, Editor, MidiData,
    NewTakeLane, SceneId, TakeClip, TakeClipData, TrackId, TrackKind,
};

const MEDIA: ContentHash = ContentHash([9; 32]);

struct Song {
    editor: Editor,
    /// An audio track, a MIDI track, and a group holding an audio track and
    /// a nested group with a MIDI track.
    audio: TrackId,
    midi: TrackId,
    group: TrackId,
    inner: TrackId,
    nested: TrackId,
    deep: TrackId,
}

impl Song {
    fn new() -> Self {
        let mut editor = Editor::new();
        let mut track = |kind, parent, index| {
            let id: TrackId = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id,
                    kind,
                    name: String::new(),
                    parent,
                    index,
                })
                .unwrap();
            id
        };
        let audio = track(TrackKind::Audio, None, 0);
        let midi = track(TrackKind::Midi, None, 1);
        let group = track(TrackKind::Group, None, 2);
        let inner = track(TrackKind::Audio, Some(group), 0);
        let nested = track(TrackKind::Group, Some(group), 1);
        let deep = track(TrackKind::Midi, Some(nested), 0);
        Self {
            editor,
            audio,
            midi,
            group,
            inner,
            nested,
            deep,
        }
    }

    fn doc(&self) -> &Document {
        self.editor.document()
    }

    fn track_colour(&self, track: TrackId) -> u8 {
        self.doc().track(track).unwrap().colour()
    }

    fn clip_colour(&self, clip: ClipId) -> Option<u8> {
        self.doc().clip(clip).unwrap().colour
    }

    fn clip(&mut self, track: TrackId, midi: bool) -> ClipId {
        let id: ClipId = self.editor.new_id();
        let data = if midi {
            ClipData::Midi(MidiData::new([]))
        } else {
            ClipData::Audio(AudioData::new(MEDIA))
        };
        self.editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(id, Ticks::ZERO, Ticks::WHOLE, data),
            })
            .unwrap();
        id
    }

    fn slot_clip(&mut self, track: TrackId, scene: SceneId) -> ClipId {
        let id: ClipId = self.editor.new_id();
        self.editor
            .apply(Command::CreateSlotClip {
                track,
                scene,
                clip: Clip::new(
                    id,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Audio(AudioData::new(MEDIA)),
                ),
            })
            .unwrap();
        id
    }

    fn take_clip(&mut self, track: TrackId) -> ClipId {
        let id: ClipId = self.editor.new_id();
        let lane = self.editor.new_id();
        self.editor
            .apply(Command::AddTakeLane {
                track,
                lane: NewTakeLane {
                    id: Some(lane),
                    name: "Take 1".to_owned(),
                    clips: vec![TakeClip {
                        id: Some(id),
                        name: String::new(),
                        start: Ticks::ZERO,
                        length: Ticks::WHOLE,
                        data: TakeClipData::Audio(AudioData::new(MEDIA)),
                    }],
                },
                index: None,
            })
            .unwrap();
        id
    }

    fn steps(&self) -> usize {
        self.editor.undo_history().len()
    }

    fn last_step(&self) -> String {
        self.editor.undo_history().next().unwrap().to_owned()
    }
}

fn first_scene(song: &Song) -> SceneId {
    song.doc().scenes().next().unwrap().id
}

// Spec: themes R14, R21. A new track takes swatch `id mod 14`, a new scene
// none.
#[test]
fn a_new_track_takes_swatch_id_mod_14_and_a_new_scene_none() {
    let song = Song::new();
    for track in [song.audio, song.midi, song.group, song.inner] {
        assert_eq!(
            u64::from(song.track_colour(track)),
            track.0.get() % 14,
            "{track:?}"
        );
    }
    assert!(song.doc().scenes().all(|scene| scene.colour.is_none()));
}

// Spec: themes R14, R16, R20. A clip handed over without a colour takes its
// track's, in the Arrangement, in a slot and on a take lane; one that brings
// its own keeps it.
#[test]
fn a_new_clip_takes_its_tracks_colour_unless_it_brings_its_own() {
    let mut song = Song::new();
    let scene = first_scene(&song);
    let main = song.clip(song.audio, false);
    let slot = song.slot_clip(song.audio, scene);
    let take = song.take_clip(song.audio);
    let own = song.track_colour(song.audio);
    for clip in [main, slot, take] {
        assert_eq!(song.clip_colour(clip), Some(own));
    }
    let id: ClipId = song.editor.new_id();
    song.editor
        .apply(Command::CreateClip {
            track: song.midi,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData::new([])),
            )
            .with_colour(Some(41)),
        })
        .unwrap();
    assert_eq!(song.clip_colour(id), Some(41));
}

// Spec: themes R16, R25. Each colour command is one undo step with its name,
// a track's clips keep their colours, and undo brings the colour back.
#[test]
fn the_colour_commands_are_one_undo_step_each() {
    let mut song = Song::new();
    let scene = first_scene(&song);
    let clip = song.clip(song.audio, false);
    let before = song.steps();
    let was = song.track_colour(song.audio);

    song.editor
        .apply(Command::SetTrackColour {
            tracks: vec![song.audio, song.midi],
            colour: 33,
        })
        .unwrap();
    assert_eq!(song.steps(), before + 1);
    assert_eq!(song.last_step(), "Change Track Color");
    assert_eq!(song.track_colour(song.audio), 33);
    assert_eq!(song.track_colour(song.midi), 33);
    assert_eq!(
        song.clip_colour(clip),
        Some(was),
        "a track's clips keep their colours"
    );

    song.editor
        .apply(Command::SetClipColour {
            clips: vec![clip],
            colour: 69,
        })
        .unwrap();
    assert_eq!(song.steps(), before + 2);
    assert_eq!(song.last_step(), "Change Clip Color");
    assert_eq!(song.clip_colour(clip), Some(69));

    song.editor
        .apply(Command::SetSceneColour {
            scenes: vec![scene],
            colour: Some(12),
        })
        .unwrap();
    assert_eq!(song.steps(), before + 3);
    assert_eq!(song.last_step(), "Change Scene Color");
    assert_eq!(song.doc().scene(scene).unwrap().colour, Some(12));
    song.editor
        .apply(Command::SetSceneColour {
            scenes: vec![scene],
            colour: None,
        })
        .unwrap();
    assert_eq!(song.doc().scene(scene).unwrap().colour, None);

    song.editor.undo().unwrap();
    assert_eq!(song.doc().scene(scene).unwrap().colour, Some(12));
    for _ in 0..3 {
        song.editor.undo().unwrap();
    }
    assert_eq!(song.track_colour(song.audio), was);
    assert_eq!(song.steps(), before);
}

// Spec: themes R14. A swatch past 69 is refused and the document is as it
// was.
#[test]
fn a_swatch_outside_the_palette_is_refused() {
    let mut song = Song::new();
    let clip = song.clip(song.audio, false);
    let scene = first_scene(&song);
    for command in [
        Command::SetTrackColour {
            tracks: vec![song.audio],
            colour: 70,
        },
        Command::SetClipColour {
            clips: vec![clip],
            colour: 70,
        },
        Command::SetSceneColour {
            scenes: vec![scene],
            colour: Some(70),
        },
    ] {
        let before = song.doc().clone();
        assert!(song.editor.apply(command).is_err());
        assert_eq!(song.doc(), &before);
    }
}

// Spec: themes R17. Assign Track Color to Clips reaches the Arrangement, the
// Session slots and the take lanes of each track, and no other track's.
#[test]
fn assign_track_color_to_clips_reaches_every_view() {
    let mut song = Song::new();
    let scene = first_scene(&song);
    let main = song.clip(song.audio, false);
    let slot = song.slot_clip(song.audio, scene);
    let take = song.take_clip(song.audio);
    let other = song.clip(song.inner, false);
    song.editor
        .apply(Command::SetTrackColour {
            tracks: vec![song.audio],
            colour: 50,
        })
        .unwrap();
    let before = song.steps();
    let untouched = song.clip_colour(other);

    song.editor
        .apply(Command::AssignColourToClips {
            tracks: vec![song.audio],
        })
        .unwrap();
    assert_eq!(song.steps(), before + 1);
    assert_eq!(song.last_step(), "Assign Track Color to Clips");
    for clip in [main, slot, take] {
        assert_eq!(song.clip_colour(clip), Some(50));
    }
    assert_eq!(song.clip_colour(other), untouched);
}

// Spec: themes R17. A group's Assign Track Color to Grouped Tracks and Clips
// colours every track inside it at any depth, and their clips, in one step.
#[test]
fn assign_to_a_group_reaches_nested_tracks_and_their_clips() {
    let mut song = Song::new();
    let scene = first_scene(&song);
    let in_group = song.clip(song.inner, false);
    let slot = song.slot_clip(song.inner, scene);
    let deep = song.clip(song.deep, true);
    let outside = song.clip(song.midi, true);
    song.editor
        .apply(Command::SetTrackColour {
            tracks: vec![song.group],
            colour: 61,
        })
        .unwrap();
    let before = song.steps();
    let outside_colour = song.clip_colour(outside);

    song.editor
        .apply(Command::AssignColourToGrouped {
            tracks: vec![song.group],
        })
        .unwrap();
    assert_eq!(song.steps(), before + 1);
    assert_eq!(
        song.last_step(),
        "Assign Track Color to Grouped Tracks and Clips"
    );
    for track in [song.inner, song.nested, song.deep] {
        assert_eq!(song.track_colour(track), 61, "{track:?}");
    }
    for clip in [in_group, slot, deep] {
        assert_eq!(song.clip_colour(clip), Some(61));
    }
    assert_eq!(song.clip_colour(outside), outside_colour);
    assert_ne!(song.track_colour(song.midi), 61);
    // Only a group has tracks inside it.
    assert!(
        song.editor
            .apply(Command::AssignColourToGrouped {
                tracks: vec![song.audio]
            })
            .is_err()
    );
}

// Spec: themes R27. A colour is never heard: the change set of a colour
// edit lists nothing but that a colour changed, so the engine compiles
// nothing; it is still a change, so the set is marked changed and the step
// is recorded.
#[test]
fn a_colour_edit_changes_nothing_the_engine_hears() {
    let mut song = Song::new();
    let scene = first_scene(&song);
    let main = song.clip(song.audio, false);
    let slot = song.slot_clip(song.audio, scene);
    let take = song.take_clip(song.audio);
    for command in [
        Command::SetTrackColour {
            tracks: vec![song.audio, song.group],
            colour: 20,
        },
        Command::SetClipColour {
            clips: vec![main, slot, take],
            colour: 21,
        },
        Command::SetSceneColour {
            scenes: vec![scene],
            colour: Some(22),
        },
        Command::AssignColourToClips {
            tracks: vec![song.audio],
        },
        Command::AssignColourToGrouped {
            tracks: vec![song.group],
        },
    ] {
        let changes = song.editor.apply(command.clone()).unwrap();
        // Only that a colour changed: nothing the engine compiles or plays.
        let only_colours = ChangeSet {
            colours: true,
            ..ChangeSet::default()
        };
        assert_eq!(changes, only_colours, "{command:?}");
    }
    // A change to what a clip plays is heard.
    let heard = song
        .editor
        .apply(Command::SetClipMute {
            clip: main,
            mute: true,
        })
        .unwrap();
    assert!(!heard.is_empty());
}

// Spec: themes R14. The colours are saved with the set and load as saved.
#[test]
fn the_colours_save_and_load() {
    let mut song = Song::new();
    let scene = first_scene(&song);
    let main = song.clip(song.audio, false);
    let slot = song.slot_clip(song.audio, scene);
    let take = song.take_clip(song.audio);
    song.editor
        .apply(Command::SetTrackColour {
            tracks: vec![song.audio],
            colour: 44,
        })
        .unwrap();
    song.editor
        .apply(Command::SetClipColour {
            clips: vec![main, take],
            colour: 3,
        })
        .unwrap();
    song.editor
        .apply(Command::SetSceneColour {
            scenes: vec![scene],
            colour: Some(68),
        })
        .unwrap();
    let saved = serde_json::to_value(song.doc()).unwrap();
    let loaded: Document = serde_json::from_value(saved).unwrap();
    assert_eq!(&loaded, song.doc());
    assert_eq!(loaded.track(song.audio).unwrap().colour(), 44);
    assert_eq!(loaded.clip(main).unwrap().colour, Some(3));
    assert_eq!(loaded.clip(take).unwrap().colour, Some(3));
    assert_ne!(loaded.clip(slot).unwrap().colour, Some(3));
    assert_eq!(loaded.scene(scene).unwrap().colour, Some(68));
}

// Spec: themes R21. A set saved before colours loads with each track at
// `id mod 14` and each clip in its track's colour, and a colour past 69
// doesn't load.
#[test]
fn a_set_from_before_colours_loads_in_the_colours_it_was_drawn_in() {
    let mut song = Song::new();
    let scene = first_scene(&song);
    let main = song.clip(song.audio, false);
    let slot = song.slot_clip(song.inner, scene);
    let take = song.take_clip(song.audio);
    let mut saved = serde_json::to_value(song.doc()).unwrap();
    // As an older version wrote it: no colour on any track, clip or scene.
    fn strip(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                map.remove("colour");
                map.values_mut().for_each(strip);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut saved);
    let loaded: Document = serde_json::from_value(saved).unwrap();
    for track in [song.audio, song.midi, song.group, song.inner, song.nested] {
        assert_eq!(
            u64::from(loaded.track(track).unwrap().colour()),
            track.0.get() % 14
        );
    }
    let audio = loaded.track(song.audio).unwrap().colour();
    let inner = loaded.track(song.inner).unwrap().colour();
    assert_eq!(loaded.clip(main).unwrap().colour, Some(audio));
    assert_eq!(loaded.clip(take).unwrap().colour, Some(audio));
    assert_eq!(loaded.clip(slot).unwrap().colour, Some(inner));
    assert!(loaded.scenes().all(|scene| scene.colour.is_none()));

    let mut bad = serde_json::to_value(song.doc()).unwrap();
    bad["tracks"][0]["colour"] = serde_json::json!(70);
    assert!(serde_json::from_value::<Document>(bad).is_err());
}
