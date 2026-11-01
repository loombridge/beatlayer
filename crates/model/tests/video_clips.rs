//! Video clips in the document (the video spec, R5 to R11): an audio clip
//! that names a movie, warped and leading by default, which the Session
//! View, Reverse and Crop turn back into an audio clip, and whose picture
//! follows its warp map.

use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Document, Editor, TrackId, TrackKind,
};

const MOVIE: ContentHash = ContentHash([5; 32]);
const SOUND: ContentHash = ContentHash([6; 32]);
/// The movie's length in seconds.
const SECONDS: f64 = 10.0;

fn track(editor: &mut Editor, index: usize) -> TrackId {
    let id: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index,
        })
        .unwrap();
    id
}

/// A video clip of the movie at `start`, 20 quarters long at 120 BPM (the
/// movie's 10 seconds).
fn video(editor: &mut Editor, track: TrackId, start: Ticks) -> ClipId {
    let id = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                id,
                start,
                Ticks::QUARTER * 20,
                ClipData::Audio(AudioData::new_video(MOVIE, 120.0).unwrap()),
            ),
        })
        .unwrap();
    id
}

fn audio(doc: &Document, clip: ClipId) -> AudioData {
    match &doc.clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

fn movie_time(doc: &Document, clip: ClipId, at: Ticks) -> Option<f64> {
    doc.clip(clip)
        .unwrap()
        .movie_time_at(at, &doc.tempo_map(), SECONDS)
}

#[test]
fn a_new_video_clip_is_warped_leads_and_saves_as_a_video_clip() {
    let data = AudioData::new_video(MOVIE, 120.0).unwrap();
    assert!(data.video && data.warp && data.tempo_leader);
    let saved = serde_json::to_value(&data).unwrap();
    assert_eq!(saved["video"], true);
    let back: AudioData = serde_json::from_value(saved).unwrap();
    assert_eq!(back, data);
    // An audio clip saves no `video` and stays a follower.
    let plain = serde_json::to_value(AudioData::new(SOUND)).unwrap();
    assert!(plain.get("video").is_none());
    assert!(!AudioData::new(SOUND).tempo_leader);

    let mut editor = Editor::new();
    let lane = track(&mut editor, 0);
    video(&mut editor, lane, Ticks::ZERO);
    assert!(editor.document().any_tempo_leader());
}

#[test]
fn a_video_clip_in_a_slot_is_an_audio_clip() {
    let mut editor = Editor::new();
    let lane = track(&mut editor, 0);
    let scene = editor.document().scenes().next().unwrap().id;
    let moved = video(&mut editor, lane, Ticks::ZERO);
    editor
        .apply(Command::MoveSlotClip {
            clip: moved,
            track: lane,
            scene,
        })
        .unwrap();
    assert!(!audio(editor.document(), moved).video);

    let created = editor.new_id();
    let other = editor.document().scenes().nth(1).unwrap().id;
    editor
        .apply(Command::CreateSlotClip {
            track: lane,
            scene: other,
            clip: Clip::new(
                created,
                Ticks::ZERO,
                Ticks::QUARTER * 4,
                ClipData::Audio(AudioData::new_video(MOVIE, 120.0).unwrap()),
            ),
        })
        .unwrap();
    assert!(!audio(editor.document(), created).video);
}

#[test]
fn reverse_and_crop_make_an_audio_clip() {
    let mut editor = Editor::new();
    let lane = track(&mut editor, 0);
    let reversed = video(&mut editor, lane, Ticks::ZERO);
    editor
        .apply(Command::ReverseClip {
            clip: reversed,
            media: Some(SOUND),
            seconds: SECONDS,
        })
        .unwrap();
    let data = audio(editor.document(), reversed);
    assert!(!data.video);
    assert_eq!(data.media, SOUND);

    let cropped = video(&mut editor, lane, Ticks::QUARTER * 20);
    editor
        .apply(Command::CropClip {
            clip: cropped,
            media: Some(SOUND),
            from: 0.0,
            to: (Ticks::QUARTER * 8).get() as f64,
        })
        .unwrap();
    let data = audio(editor.document(), cropped);
    assert!(!data.video);
    assert_eq!(data.media, SOUND);
}

#[test]
fn the_picture_plays_the_time_the_warp_map_gives() {
    let mut editor = Editor::new();
    let lane = track(&mut editor, 0);
    let start = Ticks::QUARTER * 4;
    let clip = video(&mut editor, lane, start);
    let doc = editor.document();
    // R7: at the clip's start the movie's first frame; at 120 BPM a quarter
    // is half a second, and the clip's own tempo leads, so the movie plays
    // at its own speed.
    assert_eq!(movie_time(doc, clip, start), Some(0.0));
    assert_eq!(movie_time(doc, clip, start + Ticks::QUARTER * 4), Some(2.0));
    // Outside the clip there is no picture.
    assert_eq!(movie_time(doc, clip, Ticks::ZERO), None);
    assert_eq!(movie_time(doc, clip, start + Ticks::QUARTER * 20), None);

    // R11: a warp marker 4 quarters in, whose time in the file moves half a
    // second later, moves which frame plays there.
    editor
        .apply(Command::InsertWarpMarker {
            clips: vec![clip],
            beat: Some(Ticks::QUARTER * 4),
            time: None,
            pin_neighbours: false,
            transients: None,
        })
        .unwrap();
    editor
        .apply(Command::ShiftWarpMarker {
            clips: vec![clip],
            marker: 1,
            by: 0.5,
        })
        .unwrap();
    let doc = editor.document();
    let at = start + Ticks::QUARTER * 4;
    assert_eq!(movie_time(doc, clip, at), Some(2.5));
    assert_eq!(movie_time(doc, clip, start), Some(0.0));
}

#[test]
fn a_looping_video_clip_plays_its_brace_again() {
    let mut editor = Editor::new();
    let lane = track(&mut editor, 0);
    let id = editor.new_id();
    let mut data = AudioData::new_video(MOVIE, 120.0).unwrap();
    data.looping = true;
    data.loop_start = 0.0;
    data.loop_end = Some((Ticks::QUARTER * 4).get() as f64);
    editor
        .apply(Command::CreateClip {
            track: lane,
            clip: Clip::new(id, Ticks::ZERO, Ticks::QUARTER * 12, ClipData::Audio(data)),
        })
        .unwrap();
    let doc = editor.document();
    // The brace is 2 seconds long: a quarter past its end is half a second
    // in.
    assert_eq!(movie_time(doc, id, Ticks::QUARTER * 5), Some(0.5));
}

#[test]
fn the_bottom_most_video_clip_shows_and_leads() {
    let mut editor = Editor::new();
    let upper = track(&mut editor, 0);
    let lower = track(&mut editor, 1);
    let top = video(&mut editor, upper, Ticks::ZERO);
    let bottom = video(&mut editor, lower, Ticks::QUARTER * 4);
    let doc = editor.document();
    assert_eq!(doc.video_clip_at(Ticks::QUARTER).map(|c| c.id), Some(top));
    assert_eq!(
        doc.video_clip_at(Ticks::QUARTER * 5).map(|c| c.id),
        Some(bottom)
    );
    assert_eq!(doc.video_clip_at(Ticks::QUARTER * 30), None);
    // A deactivated clip shows nothing.
    editor
        .apply(Command::SetClipMute {
            clip: bottom,
            mute: true,
        })
        .unwrap();
    assert_eq!(
        editor
            .document()
            .video_clip_at(Ticks::QUARTER * 5)
            .map(|c| c.id),
        Some(top)
    );
}
