//! Session View launching (WP-3.2): a launch or a stop lands on the exact
//! frame of the launch quantisation grid at any block size, two launches on
//! one track act in order, a scene's clips start together, and the engine
//! publishes each slot's state. Every block runs as a real-time section
//! under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;

use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::{
    EngineConfig, MediaMap, Offline, Registry, SessionCommand, SlotState, TransportCommand, testing,
};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, Command, ContentHash, Editor, LaunchMode, LaunchQuantization,
    LaunchSettings, MidiData, Note, SceneId, TrackId, TrackKind,
};
use common::{RATE, Rig, device, track};

/// A bar in ticks and in frames at 120 BPM and 48 kHz.
const BAR: Ticks = Ticks(Ticks::WHOLE.get() * 4);
const BAR_FRAMES: i64 = 96_000;

/// The tempo every test runs at.
fn tempo() -> TempoMap {
    TempoMap::constant(Bpm::new(120.0).unwrap())
}

/// The first scene of the rig's set.
fn scene(rig: &Rig) -> SceneId {
    rig.editor
        .document()
        .scenes()
        .next()
        .expect("a new set opens with scenes")
        .id
}

/// A MIDI track with the test instrument.
fn midi_track(rig: &mut Rig) -> TrackId {
    let track = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, track, testing::INSTRUMENT, &[]);
    track
}

/// Puts a clip holding `notes` in the track's slot at `scene`.
fn slot_clip(editor: &mut Editor, track: TrackId, scene: SceneId, notes: &[Note]) {
    let id = editor.new_id();
    let clip = Clip::new(
        id,
        Ticks::ZERO,
        BAR,
        ClipData::Midi(MidiData::new(notes.to_vec())),
    );
    editor
        .apply(Command::CreateSlotClip { track, scene, clip })
        .unwrap();
}

/// Plays `frames` frames in blocks of `block`, each a real-time section.
fn play(rig: &mut Rig, frames: usize, block: usize) {
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        rig.block(n);
        done += n;
    }
}

/// Plays to song frame `at`, then sends a command, so it arrives with the
/// playhead exactly there.
fn at_frame(rig: &mut Rig, at: usize, block: usize, send: impl FnOnce(&mut Rig)) {
    play(rig, at, block);
    send(rig);
}

/// The frames the test instrument has recorded, in order.
fn notes(rig: &mut Rig) -> Vec<(i64, u8, bool)> {
    rig.probe
        .recorded_notes()
        .into_iter()
        .map(|note| {
            (
                note.frame,
                note.event.key,
                note.event.kind == bl_engine::NoteKind::On,
            )
        })
        .collect()
}

// Spec: launch-quantisation R2, R3, R5, R9
#[test]
fn a_launch_lands_on_its_grid_frame_at_any_block_size_and_value() {
    // The launch is asked for one frame past 1.2.1, so every line below is
    // the next one strictly after it.
    let at = 24_001;
    // Each value, and the song frame its next grid line falls on at 120 BPM.
    let cases = [
        (LaunchQuantization::None, at as i64),
        (LaunchQuantization::Bars8, BAR_FRAMES * 8),
        (LaunchQuantization::Bars4, BAR_FRAMES * 4),
        (LaunchQuantization::Bars2, BAR_FRAMES * 2),
        (LaunchQuantization::Bar, BAR_FRAMES),
        (LaunchQuantization::Half, 48_000),
        (LaunchQuantization::HalfTriplet, 32_000),
        (LaunchQuantization::Quarter, 48_000),
        (LaunchQuantization::QuarterTriplet, 32_000),
        (LaunchQuantization::Eighth, 36_000),
        (LaunchQuantization::EighthTriplet, 32_000),
        (LaunchQuantization::Sixteenth, 30_000),
        (LaunchQuantization::SixteenthTriplet, 28_000),
        (LaunchQuantization::ThirtySecond, 27_000),
    ];
    for block in [1, 100, 512, 4096] {
        for (value, expected) in cases {
            let mut rig = Rig::new(4096);
            rig.editor
                .apply(Command::SetTempo { tempo: tempo() })
                .unwrap();
            let track = midi_track(&mut rig);
            let scene = scene(&rig);
            slot_clip(
                &mut rig.editor,
                track,
                scene,
                &[Note::new(60, Ticks::ZERO, Ticks::QUARTER, 100)],
            );
            rig.sync();
            rig.sender
                .session(SessionCommand::SetQuantization(LaunchSettings {
                    quantization: value,
                    enabled: true,
                    ..LaunchSettings::default()
                }));
            rig.sender.transport(TransportCommand::Play);
            at_frame(&mut rig, at, block, |rig| {
                rig.sender.session(SessionCommand::Launch { track, scene });
            });
            // The slot is queued until its frame, then plays; with no
            // quantisation it starts at once.
            play(&mut rig, 1, block);
            let queued = rig.monitor.state().session.clone();
            let wanted = if value == LaunchQuantization::None {
                SlotState::Playing
            } else {
                SlotState::Queued
            };
            assert!(
                queued
                    .iter()
                    .any(|slot| slot.state == wanted && slot.scene == scene),
                "{value:?} at blocks of {block}: {queued:?}"
            );
            play(&mut rig, (expected as usize) - at + 8, block);
            let recorded = notes(&mut rig);
            assert_eq!(
                recorded.first(),
                Some(&(expected, 60, true)),
                "{value:?} at blocks of {block}"
            );
            assert!(
                rig.monitor
                    .state()
                    .session
                    .iter()
                    .any(|slot| slot.state == SlotState::Playing && slot.scene == scene),
                "{value:?} at blocks of {block}"
            );
        }
    }
}

// Spec: launch-quantisation R5
#[test]
fn a_launch_while_stopped_starts_the_transport_at_once() {
    let mut rig = Rig::new(256);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let track = midi_track(&mut rig);
    let scene = scene(&rig);
    slot_clip(
        &mut rig.editor,
        track,
        scene,
        &[Note::new(64, Ticks::ZERO, Ticks::WHOLE, 100)],
    );
    rig.sync();
    // Nothing plays yet, and the launch starts the song at once from where
    // the Arrangement position is: 1.1.1 here.
    rig.sender.session(SessionCommand::Launch { track, scene });
    play(&mut rig, 24_000, 256);
    assert!(rig.monitor.state().playing);
    assert_eq!(notes(&mut rig).first(), Some(&(0, 64, true)));
}

// Spec: launch-quantisation R6
#[test]
fn a_queued_stop_lands_on_its_grid_frame() {
    let mut rig = Rig::new(256);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let track = midi_track(&mut rig);
    let scene = scene(&rig);
    slot_clip(
        &mut rig.editor,
        track,
        scene,
        &[Note::new(60, Ticks::ZERO, Ticks::WHOLE * 8, 100)],
    );
    rig.sync();
    rig.sender.session(SessionCommand::Launch { track, scene });
    play(&mut rig, 24_000, 256);
    // The clip is playing from 1.1.1; stop it a frame past 1.2.1, so the
    // stop lands on the 2.1.1 line.
    at_frame(&mut rig, 24_001, 256, |rig| {
        rig.sender.session(SessionCommand::StopTrack(track));
    });
    play(&mut rig, BAR_FRAMES as usize - 24_001 + 8, 256);
    assert_eq!(
        notes(&mut rig),
        vec![(0, 60, true), (BAR_FRAMES, 60, false)],
        "the note plays from the launch and stops on the grid frame"
    );
    assert!(rig.monitor.state().session.is_empty(), "the slot stopped");
}

// Spec: launch-quantisation R5
#[test]
fn two_launches_on_a_track_act_in_order() {
    let mut rig = Rig::new(256);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let track = midi_track(&mut rig);
    let first = scene(&rig);
    let second = rig.editor.document().scenes().nth(1).unwrap().id;
    slot_clip(
        &mut rig.editor,
        track,
        first,
        &[Note::new(60, Ticks::ZERO, Ticks::WHOLE * 8, 100)],
    );
    slot_clip(
        &mut rig.editor,
        track,
        second,
        &[Note::new(62, Ticks::ZERO, Ticks::WHOLE * 8, 100)],
    );
    rig.sync();
    // The first plays from 1.1.1; the second is asked for a frame past
    // 1.2.1 and takes over on the 2.1.1 line.
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: first,
    });
    at_frame(&mut rig, 24_001, 256, |rig| {
        rig.sender.session(SessionCommand::Launch {
            track,
            scene: second,
        });
    });
    // The block that reads the command queues the second and marks the
    // first to stop where it starts.
    play(&mut rig, 1, 256);
    let states = rig.monitor.state().session.clone();
    assert!(
        states
            .iter()
            .any(|slot| slot.scene == first && slot.state == SlotState::QueuedToStop),
        "{states:?}"
    );
    assert!(
        states
            .iter()
            .any(|slot| slot.scene == second && slot.state == SlotState::Queued),
        "{states:?}"
    );
    play(&mut rig, BAR_FRAMES as usize - 24_001 + 8, 256);
    assert_eq!(
        notes(&mut rig),
        vec![
            (0, 60, true),
            (BAR_FRAMES, 60, false),
            (BAR_FRAMES, 62, true),
        ],
        "the first plays until the second starts, both on the grid frame"
    );
    let states = rig.monitor.state().session.clone();
    assert_eq!(states.len(), 1, "{states:?}");
    assert_eq!(states[0].scene, second);
    assert_eq!(states[0].state, SlotState::Playing);
}

// Spec: launch-quantisation R7
#[test]
fn a_scene_launch_starts_its_clips_together() {
    let mut rig = Rig::new(512);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let first = midi_track(&mut rig);
    let second = midi_track(&mut rig);
    let scene = scene(&rig);
    slot_clip(
        &mut rig.editor,
        first,
        scene,
        &[Note::new(60, Ticks::ZERO, Ticks::WHOLE, 100)],
    );
    slot_clip(
        &mut rig.editor,
        second,
        scene,
        &[Note::new(67, Ticks::ZERO, Ticks::WHOLE, 100)],
    );
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    at_frame(&mut rig, 24_001, 512, |rig| {
        rig.sender.session(SessionCommand::LaunchScene(scene));
    });
    play(&mut rig, BAR_FRAMES as usize - 24_001 + 8, 512);
    let mut ons: Vec<_> = notes(&mut rig)
        .into_iter()
        .filter(|(_, _, on)| *on)
        .collect();
    ons.sort_unstable();
    assert_eq!(
        ons,
        vec![(BAR_FRAMES, 60, true), (BAR_FRAMES, 67, true)],
        "both clips start on the same grid frame"
    );
}

// Spec: launch-quantisation R10
#[test]
fn every_slot_stops_together_and_stop_all_clears_the_state() {
    let mut rig = Rig::new(256);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let first = midi_track(&mut rig);
    let second = midi_track(&mut rig);
    let scene = scene(&rig);
    slot_clip(
        &mut rig.editor,
        first,
        scene,
        &[Note::new(60, Ticks::ZERO, Ticks::WHOLE * 8, 100)],
    );
    slot_clip(
        &mut rig.editor,
        second,
        scene,
        &[Note::new(62, Ticks::ZERO, Ticks::WHOLE * 8, 100)],
    );
    rig.sync();
    rig.sender.session(SessionCommand::LaunchScene(scene));
    at_frame(&mut rig, 24_000, 256, |rig| {
        rig.sender.session(SessionCommand::StopAll);
    });
    play(&mut rig, BAR_FRAMES as usize - 24_000 + 8, 256);
    let mut offs: Vec<_> = notes(&mut rig)
        .into_iter()
        .filter(|(_, _, on)| !*on)
        .map(|(frame, key, _)| (frame, key))
        .collect();
    offs.sort_unstable();
    assert_eq!(offs, vec![(BAR_FRAMES, 60), (BAR_FRAMES, 62)]);
    assert!(rig.monitor.state().session.is_empty());
}

/// A file whose every sample says where it is: frame `i` plays `(i + 1)`
/// steps, so a render shows exactly which file frame each song frame read.
const STEP: f32 = 1.0 / 65_536.0;

fn ramp(frames: usize) -> Arc<SampleData> {
    Arc::new(
        SampleData::new(
            48_000,
            vec![
                (0..frames).map(|i| (i + 1) as f32 * STEP).collect(),
                (0..frames).map(|i| -((i + 1) as f32 * STEP)).collect(),
            ],
        )
        .unwrap(),
    )
}

fn at(frame: i64) -> f32 {
    (frame + 1) as f32 * STEP
}

/// A command the render sends on a song frame.
enum Cue {
    Session(SessionCommand),
    Transport(TransportCommand),
}

/// Renders `frames` frames of an offline engine whose only track holds an
/// audio clip in a Session slot, sending the commands `cues` names, each on
/// the song frame it names, and answers with the left channel.
fn render_session_audio(
    frames: u64,
    block: usize,
    cues: impl FnOnce(TrackId, SceneId) -> Vec<(u64, Cue)>,
) -> Vec<f32> {
    let mut media = MediaMap::new();
    media.insert(ContentHash([7; 32]), ramp(96_000));
    let registry = Registry::new().with_media(Arc::new(media));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    let mut editor = Editor::new();
    let track = track(&mut editor, TrackKind::Audio, None);
    let scene = editor.document().scenes().next().unwrap().id;
    let clip = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        Ticks::WHOLE,
        ClipData::Audio(AudioData::new(ContentHash([7; 32]))),
    );
    editor
        .apply(Command::CreateSlotClip { track, scene, clip })
        .unwrap();
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    let mut cues = cues(track, scene);
    cues.sort_by_key(|&(frame, _)| frame);
    let mut out = Vec::with_capacity(frames as usize);
    let mut done = 0u64;
    let mut next = 0;
    while done < frames {
        while next < cues.len() && cues[next].0 <= done {
            match cues[next].1 {
                Cue::Session(command) => offline.sender().session(command),
                Cue::Transport(command) => offline.sender().transport(command),
            }
            next += 1;
        }
        // Stop the block at the next cue, so it arrives on its own frame.
        let until = cues.get(next).map_or(frames, |&(frame, _)| frame);
        let n = block
            .min((frames - done) as usize)
            .min((until - done) as usize);
        offline.render(n as u64, |output| out.extend_from_slice(output.channel(0)));
        done += n as u64;
    }
    assert_eq!(offline.heap_operations(), 0);
    out
}

// Spec: launch-quantisation R5, R9
#[test]
fn a_launched_audio_slot_clip_plays_its_file_from_the_launch_frame() {
    // While stopped the launch starts the transport, and the clip plays
    // from the Arrangement position at once.
    let out = render_session_audio(2_000, 512, |track, scene| {
        vec![(0, Cue::Session(SessionCommand::Launch { track, scene }))]
    });
    for (i, sample) in out.iter().enumerate().take(600) {
        assert_eq!(*sample, at(i as i64), "frame {i}");
    }
    // While playing it waits for the grid: with the default one-bar grid,
    // the clip starts on the 2.1.1 line.
    let out = render_session_audio(97_000, 512, |track, scene| {
        vec![
            (0, Cue::Transport(TransportCommand::Play)),
            (
                24_001,
                Cue::Session(SessionCommand::Launch { track, scene }),
            ),
        ]
    });
    assert_eq!(out[24_000], 0.0, "nothing plays before the grid");
    assert_eq!(out[95_999], 0.0);
    for (i, sample) in out[96_000..].iter().enumerate().take(600) {
        assert_eq!(*sample, at(i as i64), "frame {i} of the launch");
    }
}

/// An offline engine with `tracks` audio tracks, each holding a constant in
/// the Arrangement (0.25 on the first, 0.5 on the second) and the ramp in its
/// Session slot at the first scene.
fn over_rig(block: usize, tracks: usize) -> (Offline, Vec<TrackId>, SceneId) {
    let mut media = MediaMap::new();
    media.insert(ContentHash([7; 32]), ramp(96_000));
    let mut editor = Editor::new();
    let scene = editor.document().scenes().next().unwrap().id;
    let mut ids = Vec::new();
    for k in 0..tracks {
        let hash = ContentHash([8 + k as u8; 32]);
        let level = 0.25 * (k + 1) as f32;
        media.insert(
            hash,
            Arc::new(SampleData::new(48_000, vec![vec![level; 400_000]]).unwrap()),
        );
        let track = track(&mut editor, TrackKind::Audio, None);
        for (start, length, audio) in [
            (Ticks::ZERO, Ticks::WHOLE * 4, AudioData::new(hash)),
            (
                Ticks::ZERO,
                Ticks::WHOLE,
                AudioData::new(ContentHash([7; 32])),
            ),
        ] {
            let clip = Clip::new(editor.new_id(), start, length, ClipData::Audio(audio));
            // The second clip sits in the slot, the first in the Arrangement.
            let command = if length == Ticks::WHOLE {
                Command::CreateSlotClip { track, scene, clip }
            } else {
                Command::CreateClip { track, clip }
            };
            editor.apply(command).unwrap();
        }
        ids.push(track);
    }
    let registry = Registry::new().with_media(Arc::new(media));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    (offline, ids, scene)
}

/// Renders up to song frame `until`, appending the left channel to `out`.
fn render_to(offline: &mut Offline, out: &mut Vec<f32>, until: usize) {
    offline.render((until - out.len()) as u64, |output| {
        out.extend_from_slice(output.channel(0));
    });
}

// Spec: session-view R20, R22
#[test]
fn a_launched_clip_silences_the_arrangement_until_a_return() {
    for block in [64, 700, 4096] {
        let (mut offline, tracks, scene) = over_rig(block, 1);
        let track = tracks[0];
        let mut out = Vec::new();
        offline.sender().transport(TransportCommand::Play);
        render_to(&mut offline, &mut out, 24_001);
        offline
            .sender()
            .session(SessionCommand::Launch { track, scene });
        render_to(&mut offline, &mut out, 96_600);
        let at = |frame: usize| format!("{frame} at blocks of {block}");
        // The Arrangement sounds until the launch's grid line, and the clip
        // alone sounds from it.
        assert!(out[..96_000].iter().all(|&s| s == 0.25), "{}", at(0));
        for i in 0..600 {
            assert_eq!(out[96_000 + i], self::at(i as i64), "{}", at(96_000 + i));
        }
        assert_eq!(offline.state().session_over, [track]);
        // A stop queued now lands on the 3.1.1 line; the track stays taken
        // over after it, silent.
        render_to(&mut offline, &mut out, 100_000);
        offline.sender().session(SessionCommand::StopTrack(track));
        render_to(&mut offline, &mut out, 200_000);
        assert_eq!(
            out[100_000],
            self::at(4_000),
            "still playing before the line"
        );
        assert!(out[192_000..].iter().all(|&s| s == 0.0), "{}", at(192_000));
        assert!(offline.state().session.is_empty());
        assert_eq!(offline.state().session_over, [track], "{}", at(0));
        // The return gives the Arrangement back on its first frame.
        offline.sender().session(SessionCommand::BackToArrangement);
        render_to(&mut offline, &mut out, 201_000);
        assert!(out[200_000..].iter().all(|&s| s == 0.25), "{}", at(200_000));
        assert!(offline.state().session_over.is_empty());
        assert_eq!(offline.heap_operations(), 0);
    }
}

// Spec: session-view R21
#[test]
fn a_stop_on_an_idle_track_silences_its_arrangement_on_the_grid() {
    for block in [64, 700, 4096] {
        let (mut offline, tracks, _) = over_rig(block, 1);
        let mut out = Vec::new();
        offline.sender().transport(TransportCommand::Play);
        render_to(&mut offline, &mut out, 24_001);
        offline.sender().session(SessionCommand::StopAll);
        render_to(&mut offline, &mut out, 97_000);
        assert!(out[..96_000].iter().all(|&s| s == 0.25), "block {block}");
        assert!(out[96_000..].iter().all(|&s| s == 0.0), "block {block}");
        assert_eq!(offline.state().session_over, tracks);
    }
}

// Spec: session-view R22
#[test]
fn a_return_drops_a_queued_launch() {
    let (mut offline, tracks, scene) = over_rig(512, 1);
    let mut out = Vec::new();
    offline.sender().transport(TransportCommand::Play);
    render_to(&mut offline, &mut out, 24_001);
    offline.sender().session(SessionCommand::Launch {
        track: tracks[0],
        scene,
    });
    render_to(&mut offline, &mut out, 30_000);
    offline.sender().session(SessionCommand::BackToArrangement);
    render_to(&mut offline, &mut out, 100_000);
    assert!(out.iter().all(|&s| s == 0.25), "the launch never lands");
    assert!(offline.state().session.is_empty());
    assert!(offline.state().session_over.is_empty());
}

// Spec: session-view R23
#[test]
fn the_per_track_return_returns_one_track() {
    let (mut offline, tracks, scene) = over_rig(512, 2);
    let mut out = Vec::new();
    offline.sender().transport(TransportCommand::Play);
    render_to(&mut offline, &mut out, 24_001);
    offline.sender().session(SessionCommand::LaunchScene(scene));
    render_to(&mut offline, &mut out, 97_000);
    assert_eq!(offline.state().session_over.len(), 2);
    // Both tracks play the ramp; returning the first brings its 0.25 back
    // and leaves the second's clip playing.
    offline
        .sender()
        .session(SessionCommand::TrackBackToArrangement(tracks[0]));
    render_to(&mut offline, &mut out, 98_000);
    for i in 0..600 {
        let frame = 97_000 + i;
        let expected = 0.25 + self::at((frame - 96_000) as i64);
        assert!((out[frame] - expected).abs() < 1e-6, "{frame}");
    }
    assert_eq!(offline.state().session_over, [tracks[1]]);
    assert!(
        offline
            .state()
            .session
            .iter()
            .all(|slot| slot.track == tracks[1])
    );
}

// Spec: session-view R20, R22
#[test]
fn a_taken_over_midi_track_lets_its_arrangement_notes_go_and_plays_them_again_after_a_return() {
    let mut rig = Rig::new(512);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let track = midi_track(&mut rig);
    let scene = scene(&rig);
    let bar = Ticks::WHOLE;
    let arrangement = Clip::new(
        rig.editor.new_id(),
        Ticks::ZERO,
        bar * 4,
        ClipData::Midi(MidiData::new(vec![
            Note::new(60, Ticks::ZERO, bar * 2, 100),
            Note::new(61, bar + bar / 2, bar / 4, 100),
            Note::new(63, bar * 3, bar / 4, 100),
        ])),
    );
    rig.editor
        .apply(Command::CreateClip {
            track,
            clip: arrangement,
        })
        .unwrap();
    slot_clip(
        &mut rig.editor,
        track,
        scene,
        &[Note::new(62, Ticks::ZERO, Ticks::QUARTER, 100)],
    );
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    at_frame(&mut rig, 24_001, 512, |rig| {
        rig.sender.session(SessionCommand::Launch { track, scene });
    });
    // The clip takes over on the 2.1.1 line; stop it on the 3.1.1 line, and
    // return once the playhead is past it.
    at_frame(&mut rig, 150_000 - 24_001, 512, |rig| {
        rig.sender.session(SessionCommand::StopTrack(track));
    });
    at_frame(&mut rig, 200_000 - 150_000, 512, |rig| {
        rig.sender.session(SessionCommand::BackToArrangement);
    });
    play(&mut rig, 100_000, 512);
    let recorded = notes(&mut rig);
    let of = |key: u8| -> Vec<(i64, bool)> {
        recorded
            .iter()
            .filter(|note| note.1 == key)
            .map(|note| (note.0, note.2))
            .collect()
    };
    assert_eq!(
        of(60),
        [(0, true), (BAR_FRAMES, false)],
        "the Arrangement's note is let go where the clip takes over"
    );
    assert_eq!(of(62).first(), Some(&(BAR_FRAMES, true)));
    assert!(
        of(61).is_empty(),
        "the Arrangement is silent while taken over"
    );
    assert_eq!(of(63).first(), Some(&(BAR_FRAMES * 3, true)));
    assert!(rig.monitor.state().session_over.is_empty());
}

/// A playing rig whose only track holds, in its first scene, a 16-bar MIDI
/// clip of `mode` with one note that lasts all 16 bars, under `value`.
fn mode_rig(mode: LaunchMode, value: LaunchQuantization) -> (Rig, TrackId, SceneId) {
    let mut rig = Rig::new(4096);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let track = midi_track(&mut rig);
    let scene = scene(&rig);
    let mut clip = Clip::new(
        rig.editor.new_id(),
        Ticks::ZERO,
        BAR * 16,
        ClipData::Midi(MidiData::new(vec![Note::new(
            60,
            Ticks::ZERO,
            BAR * 16,
            100,
        )])),
    );
    clip.launch.mode = mode;
    rig.editor
        .apply(Command::CreateSlotClip { track, scene, clip })
        .unwrap();
    rig.sync();
    rig.sender
        .session(SessionCommand::SetQuantization(LaunchSettings {
            quantization: value,
            enabled: true,
            ..LaunchSettings::default()
        }));
    rig.sender.transport(TransportCommand::Play);
    (rig, track, scene)
}

/// Plays `total` frames in blocks of `block`, sending each command of
/// `cues` with the playhead exactly on its frame.
fn cued(rig: &mut Rig, block: usize, total: usize, cues: &[(usize, SessionCommand)]) {
    let mut at = 0;
    for &(frame, command) in cues {
        play(rig, frame - at, block);
        at = frame;
        rig.sender.session(command);
    }
    play(rig, total - at, block);
}

const BLOCKS: [usize; 4] = [1, 100, 512, 4096];

// Spec: launch-quantisation R17
#[test]
fn a_trigger_press_restarts_the_playing_clip_on_the_grid_line() {
    for block in BLOCKS {
        let (mut rig, track, scene) = mode_rig(LaunchMode::Trigger, LaunchQuantization::Bar);
        let press = SessionCommand::Launch { track, scene };
        let release = SessionCommand::Release { track, scene };
        // Starts on 2.1.1; the second press, 130,000 frames in, restarts it
        // on 3.1.1, and the release changes nothing.
        cued(
            &mut rig,
            block,
            200_000,
            &[(24_001, press), (130_000, press), (131_000, release)],
        );
        assert_eq!(
            notes(&mut rig),
            vec![
                (96_000, 60, true),
                (192_000, 60, false),
                (192_000, 60, true)
            ],
            "blocks of {block}"
        );
    }
}

// Spec: launch-quantisation R18, R19
#[test]
fn a_gate_clip_stops_on_the_grid_line_after_the_release() {
    for block in BLOCKS {
        for (value, press_at, release_at, start, stop) in [
            // Held across a line: starts on 2.1.1, released in bar 2, stops
            // on 3.1.1.
            (LaunchQuantization::Bar, 24_001, 130_000, 96_000, 192_000),
            // Pressed and released inside one interval: the launch still
            // fires, and the stop lands on the line after it.
            (LaunchQuantization::Bar, 24_001, 48_000, 96_000, 192_000),
            // Quantisation off: starts and stops on the frames of the
            // press and the release.
            (LaunchQuantization::None, 24_001, 60_000, 24_001, 60_000),
            // A quarter grid.
            (LaunchQuantization::Quarter, 10_000, 40_000, 24_000, 48_000),
        ] {
            let (mut rig, track, scene) = mode_rig(LaunchMode::Gate, value);
            cued(
                &mut rig,
                block,
                200_000,
                &[
                    (press_at, SessionCommand::Launch { track, scene }),
                    (release_at, SessionCommand::Release { track, scene }),
                ],
            );
            assert_eq!(
                notes(&mut rig),
                vec![(start, 60, true), (stop, 60, false)],
                "{value:?} at blocks of {block}"
            );
            assert!(rig.monitor.state().playing, "the transport keeps running");
        }
    }
}

// Spec: launch-quantisation R18
#[test]
fn a_gate_press_with_the_transport_stopped_starts_it_at_once() {
    for block in BLOCKS {
        let mut rig = Rig::new(4096);
        rig.editor
            .apply(Command::SetTempo { tempo: tempo() })
            .unwrap();
        let track = midi_track(&mut rig);
        let scene = scene(&rig);
        let mut clip = Clip::new(
            rig.editor.new_id(),
            Ticks::ZERO,
            BAR * 16,
            ClipData::Midi(MidiData::new(vec![Note::new(
                60,
                Ticks::ZERO,
                BAR * 16,
                100,
            )])),
        );
        clip.launch.mode = LaunchMode::Gate;
        rig.editor
            .apply(Command::CreateSlotClip { track, scene, clip })
            .unwrap();
        rig.sync();
        cued(
            &mut rig,
            block,
            120_000,
            &[
                (0, SessionCommand::Launch { track, scene }),
                (10_000, SessionCommand::Release { track, scene }),
            ],
        );
        // Released 10,000 frames in, the clip stops on the 2.1.1 line and
        // the transport runs on.
        assert_eq!(
            notes(&mut rig),
            vec![(0, 60, true), (96_000, 60, false)],
            "blocks of {block}"
        );
        assert!(rig.monitor.state().playing);
    }
}

// Spec: launch-quantisation R20
#[test]
fn a_toggle_clip_stops_on_the_grid_line_after_its_second_press() {
    for block in BLOCKS {
        let (mut rig, track, scene) = mode_rig(LaunchMode::Toggle, LaunchQuantization::Bar);
        let press = SessionCommand::Launch { track, scene };
        let release = SessionCommand::Release { track, scene };
        // Starts on 2.1.1; the second press in bar 2 queues the stop for
        // 3.1.1; the release changes nothing.
        cued(
            &mut rig,
            block,
            200_000,
            &[(24_001, press), (100_000, press), (101_000, release)],
        );
        assert_eq!(
            notes(&mut rig),
            vec![(96_000, 60, true), (192_000, 60, false)],
            "blocks of {block}"
        );
        // A third press while the stop is queued restarts the clip on that
        // line instead.
        let (mut rig, track, scene) = mode_rig(LaunchMode::Toggle, LaunchQuantization::Bar);
        let press = SessionCommand::Launch { track, scene };
        cued(
            &mut rig,
            block,
            200_000,
            &[(24_001, press), (100_000, press), (150_000, press)],
        );
        assert_eq!(
            notes(&mut rig),
            vec![
                (96_000, 60, true),
                (192_000, 60, false),
                (192_000, 60, true)
            ],
            "blocks of {block}"
        );
    }
}

// Spec: launch-quantisation R21
#[test]
fn a_held_repeat_clip_restarts_every_step_until_the_release() {
    for block in BLOCKS {
        let (mut rig, track, scene) = mode_rig(LaunchMode::Repeat, LaunchQuantization::Quarter);
        // Pressed a frame past 1.1.1 it starts on the quarter at 24,000,
        // restarts at 48,000 and 72,000, and the release at 80,000 ends the
        // quantum it is in, at 96,000.
        cued(
            &mut rig,
            block,
            130_000,
            &[
                (1, SessionCommand::Launch { track, scene }),
                (80_000, SessionCommand::Release { track, scene }),
            ],
        );
        assert_eq!(
            notes(&mut rig),
            vec![
                (24_000, 60, true),
                (48_000, 60, false),
                (48_000, 60, true),
                (72_000, 60, false),
                (72_000, 60, true),
                (96_000, 60, false),
            ],
            "blocks of {block}"
        );
    }
}

// Spec: launch-quantisation R21
#[test]
fn a_repeat_clip_with_no_quantisation_acts_as_a_trigger() {
    for block in BLOCKS {
        let (mut rig, track, scene) = mode_rig(LaunchMode::Repeat, LaunchQuantization::None);
        cued(
            &mut rig,
            block,
            60_000,
            &[(10_000, SessionCommand::Launch { track, scene })],
        );
        assert_eq!(
            notes(&mut rig),
            vec![(10_000, 60, true)],
            "blocks of {block}"
        );
    }
}

// Spec: launch-quantisation R22
#[test]
fn the_mode_is_read_at_each_press_and_release() {
    for block in BLOCKS {
        // A Gate clip whose mode becomes Trigger while it plays isn't
        // touched by the change, and the release then does nothing.
        let (mut rig, track, scene) = mode_rig(LaunchMode::Gate, LaunchQuantization::Bar);
        let clip = rig
            .editor
            .document()
            .track(track)
            .unwrap()
            .slot(scene)
            .unwrap()
            .id;
        cued(
            &mut rig,
            block,
            130_000,
            &[(24_001, SessionCommand::Launch { track, scene })],
        );
        rig.editor
            .apply(Command::SetClipLaunchMode {
                clip,
                mode: LaunchMode::Trigger,
            })
            .unwrap();
        rig.sync();
        rig.sender.session(SessionCommand::Release { track, scene });
        play(&mut rig, 100_000, block);
        assert_eq!(
            notes(&mut rig),
            vec![(96_000, 60, true)],
            "blocks of {block}"
        );
    }
}
