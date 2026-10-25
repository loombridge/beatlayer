//! A Session clip's own launch settings (WP-7.29): its own launch
//! quantisation places its presses and releases, Legato takes over the clip
//! time of the clip playing on its track, and a nudge moves a playing clip
//! by a step of the global quantisation. Every block runs as a real-time
//! section under the allocation trap, at block sizes of 1, 64 and 512.

use crate::common;
use std::sync::Arc;

use bl_core::time::{Bpm, Frames, TempoMap, Ticks};
use bl_engine::{
    EngineConfig, MediaMap, NoteKind, Offline, Registry, SessionCommand, SlotState,
    TransportCommand, testing,
};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Editor, FollowAction, FollowPatch,
    LaunchMode, LaunchQuantization, LaunchSettings, MidiData, Note, SceneId, TrackId, TrackKind,
};
use common::{RATE, Rig, device, track};

/// The tests run at 240 BPM: a quarter note is 12,000 frames.
const QUARTER_FRAMES: i64 = 12_000;
const BAR_FRAMES: i64 = QUARTER_FRAMES * 4;
const BLOCKS: [usize; 3] = [1, 64, 512];

fn tempo() -> TempoMap {
    TempoMap::constant(Bpm::new(240.0).unwrap())
}

/// A rig with `tracks` MIDI tracks, each with the test instrument.
fn rig(tracks: usize) -> (Rig, Vec<TrackId>, Vec<SceneId>) {
    let mut rig = Rig::new(4096);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let tracks = (0..tracks)
        .map(|_| {
            let id = track(&mut rig.editor, TrackKind::Midi, None);
            device(&mut rig.editor, id, testing::INSTRUMENT, &[]);
            id
        })
        .collect();
    let scenes = rig.editor.document().scenes().map(|s| s.id).collect();
    (rig, tracks, scenes)
}

/// How a test's clip launches.
#[derive(Clone, Copy)]
struct Launch {
    mode: LaunchMode,
    legato: bool,
    quantization: Option<LaunchQuantization>,
}

fn launch(quantization: Option<LaunchQuantization>) -> Launch {
    Launch {
        mode: LaunchMode::Trigger,
        legato: false,
        quantization,
    }
}

/// Puts a clip of `bars` bars in the slot, holding `notes`, looping its whole
/// length when `looping`, launching as `launch` says.
fn clip(
    editor: &mut Editor,
    track: TrackId,
    scene: SceneId,
    notes: Vec<Note>,
    (bars, looping): (i64, bool),
    launch: Launch,
) -> ClipId {
    let id = editor.new_id();
    let length = Ticks::WHOLE * bars;
    let mut data = MidiData::new(notes);
    if looping {
        data.looping = true;
        data.loop_end = Some(length);
    }
    editor
        .apply(Command::CreateSlotClip {
            track,
            scene,
            clip: Clip::new(id, Ticks::ZERO, length, ClipData::Midi(data)),
        })
        .unwrap();
    editor
        .apply(Command::SetClipLaunchMode {
            clip: id,
            mode: launch.mode,
        })
        .unwrap();
    editor
        .apply(Command::SetClipLegato {
            clip: id,
            legato: launch.legato,
        })
        .unwrap();
    editor
        .apply(Command::SetClipLaunchQuantization {
            clip: id,
            quantization: launch.quantization,
        })
        .unwrap();
    id
}

/// One long note `key` from the clip's start.
fn long_note(key: u8) -> Vec<Note> {
    vec![Note::new(key, Ticks::ZERO, Ticks::WHOLE * 8, 100)]
}

/// A quarter note on every beat of four bars, its key `60` plus the beat.
fn beats() -> Vec<Note> {
    (0..16)
        .map(|beat| Note::new(60 + beat as u8, Ticks::QUARTER * beat, Ticks::QUARTER, 100))
        .collect()
}

fn settings(rig: &mut Rig, quantization: LaunchQuantization, enabled: bool) {
    rig.sender
        .session(SessionCommand::SetQuantization(LaunchSettings {
            quantization,
            enabled,
            ..LaunchSettings::default()
        }));
}

/// Starts the transport and plays a frame, so presses wait for the grid.
fn start(rig: &mut Rig) -> i64 {
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    rig.block(1);
    1
}

/// Plays on from song frame `now` to `to`, in blocks of `block`.
fn play_to(rig: &mut Rig, now: &mut i64, to: i64, block: usize) {
    while *now < to {
        let n = block.min((to - *now) as usize);
        rig.block(n);
        *now += n as i64;
    }
}

/// Plays to `at`, then sends `command`, so it arrives with the playhead
/// there.
fn send_at(rig: &mut Rig, now: &mut i64, at: i64, block: usize, command: SessionCommand) {
    play_to(rig, now, at, block);
    rig.sender.session(command);
}

/// The note-ons so far, as `(frame, key)`, and the note-offs.
fn ons_and_offs(rig: &mut Rig) -> (Vec<(i64, u8)>, Vec<(i64, u8)>) {
    let (mut ons, mut offs) = (Vec::new(), Vec::new());
    for note in rig.probe.recorded_notes() {
        let list = if note.event.kind == NoteKind::On {
            &mut ons
        } else {
            &mut offs
        };
        list.push((note.frame, note.event.key));
    }
    (ons, offs)
}

fn press(track: TrackId, scene: SceneId) -> SessionCommand {
    SessionCommand::Launch { track, scene }
}

fn release(track: TrackId, scene: SceneId) -> SessionCommand {
    SessionCommand::Release { track, scene }
}

fn nudge(track: TrackId, scene: SceneId, steps: i32) -> SessionCommand {
    SessionCommand::Nudge {
        track,
        scene,
        steps,
    }
}

/// The clip time, in ticks, of the slot at song frame `now`, if it plays.
fn clip_time(rig: &mut Rig, scene: SceneId, now: i64) -> Option<Ticks> {
    let state = rig.monitor.state();
    let slot = state.session.iter().find(|slot| slot.scene == scene)?;
    let started = slot.started?;
    Some(
        slot.offset.ticks
            + tempo().frames_to_ticks(Frames(now), RATE)
            - tempo().frames_to_ticks(started, RATE),
    )
}

fn near(a: Ticks, b: Ticks) -> bool {
    (a - b).get().abs() <= 64
}

// Spec: launch-quantisation R25
#[test]
fn a_clip_on_a_quarter_launches_restarts_and_stops_on_the_quarter_with_the_global_at_a_bar() {
    let quarter = Some(LaunchQuantization::Quarter);
    for block in BLOCKS {
        // The switch makes no difference to a step of the clip's own.
        for enabled in [true, false] {
            // Gate: a press at 6,001 starts on the quarter at 12,000, and
            // the release at 18,001 stops on the one at 24,000.
            let (mut rig, tracks, scenes) = rig(1);
            let (track, scene) = (tracks[0], scenes[0]);
            let mode = Launch {
                mode: LaunchMode::Gate,
                ..launch(quarter)
            };
            clip(&mut rig.editor, track, scene, long_note(60), (1, false), mode);
            settings(&mut rig, LaunchQuantization::Bar, enabled);
            let mut now = start(&mut rig);
            send_at(&mut rig, &mut now, 6_001, block, press(track, scene));
            send_at(&mut rig, &mut now, 18_001, block, release(track, scene));
            play_to(&mut rig, &mut now, 30_000, block);
            assert_eq!(
                ons_and_offs(&mut rig),
                (vec![(12_000, 60)], vec![(24_000, 60)]),
                "Gate, blocks of {block}, switch {enabled}"
            );

            // Toggle: the second press stops on the quarter after it.
            let (mut rig, tracks, scenes) = self::rig(1);
            let (track, scene) = (tracks[0], scenes[0]);
            let mode = Launch {
                mode: LaunchMode::Toggle,
                ..launch(quarter)
            };
            clip(&mut rig.editor, track, scene, long_note(60), (1, false), mode);
            settings(&mut rig, LaunchQuantization::Bar, enabled);
            let mut now = start(&mut rig);
            send_at(&mut rig, &mut now, 6_001, block, press(track, scene));
            send_at(&mut rig, &mut now, 18_001, block, press(track, scene));
            play_to(&mut rig, &mut now, 30_000, block);
            assert_eq!(
                ons_and_offs(&mut rig),
                (vec![(12_000, 60)], vec![(24_000, 60)]),
                "Toggle, blocks of {block}, switch {enabled}"
            );

            // Trigger: a press of the playing clip restarts it on a quarter.
            let (mut rig, tracks, scenes) = self::rig(1);
            let (track, scene) = (tracks[0], scenes[0]);
            clip(&mut rig.editor, track, scene, long_note(60), (1, false), launch(quarter));
            settings(&mut rig, LaunchQuantization::Bar, enabled);
            let mut now = start(&mut rig);
            send_at(&mut rig, &mut now, 6_001, block, press(track, scene));
            send_at(&mut rig, &mut now, 18_001, block, press(track, scene));
            play_to(&mut rig, &mut now, 30_000, block);
            assert_eq!(
                ons_and_offs(&mut rig),
                (vec![(12_000, 60), (24_000, 60)], vec![(24_000, 60)]),
                "Trigger, blocks of {block}, switch {enabled}"
            );
        }
    }
}

// Spec: launch-quantisation R21, R25
#[test]
fn a_repeat_clip_on_an_eighth_restarts_every_eighth_and_on_none_acts_as_a_trigger() {
    for block in BLOCKS {
        let repeat = |quantization| Launch {
            mode: LaunchMode::Repeat,
            ..launch(quantization)
        };
        let (mut rig, tracks, scenes) = rig(1);
        let (track, scene) = (tracks[0], scenes[0]);
        clip(
            &mut rig.editor,
            track,
            scene,
            long_note(60),
            (1, false),
            repeat(Some(LaunchQuantization::Eighth)),
        );
        settings(&mut rig, LaunchQuantization::Bar, true);
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 1, block, press(track, scene));
        send_at(&mut rig, &mut now, 27_001, block, release(track, scene));
        play_to(&mut rig, &mut now, 40_000, block);
        let (ons, offs) = ons_and_offs(&mut rig);
        assert_eq!(
            ons,
            [6_000, 12_000, 18_000, 24_000].map(|frame| (frame, 60)),
            "blocks of {block}"
        );
        assert_eq!(offs.last(), Some(&(30_000, 60)), "blocks of {block}");

        let (mut rig, tracks, scenes) = self::rig(1);
        let (track, scene) = (tracks[0], scenes[0]);
        clip(
            &mut rig.editor,
            track,
            scene,
            long_note(60),
            (1, false),
            repeat(Some(LaunchQuantization::None)),
        );
        settings(&mut rig, LaunchQuantization::Bar, true);
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 1, block, press(track, scene));
        play_to(&mut rig, &mut now, 40_000, block);
        assert_eq!(
            ons_and_offs(&mut rig).0.len(),
            1,
            "one start and no restarts, blocks of {block}"
        );
    }
}

// Spec: launch-quantisation R26
#[test]
fn a_scene_and_a_stop_button_use_the_global_grid_whatever_the_clips_own() {
    for block in BLOCKS {
        // A scene launched at 6,001 starts a 2 Bars clip and a 1/4 clip on
        // the global bar line.
        let (mut rig, tracks, scenes) = rig(2);
        let scene = scenes[0];
        clip(
            &mut rig.editor,
            tracks[0],
            scene,
            long_note(60),
            (1, false),
            launch(Some(LaunchQuantization::Bars2)),
        );
        clip(
            &mut rig.editor,
            tracks[1],
            scene,
            long_note(67),
            (1, false),
            launch(Some(LaunchQuantization::Quarter)),
        );
        settings(&mut rig, LaunchQuantization::Bar, true);
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 6_001, block, SessionCommand::LaunchScene(scene));
        play_to(&mut rig, &mut now, BAR_FRAMES + 100, block);
        let mut ons = ons_and_offs(&mut rig).0;
        ons.sort_unstable();
        assert_eq!(
            ons,
            vec![(BAR_FRAMES, 60), (BAR_FRAMES, 67)],
            "blocks of {block}"
        );

        // A track's stop button stops its 1/4 clip on the bar line.
        let (mut rig, tracks, scenes) = self::rig(1);
        let (track, scene) = (tracks[0], scenes[0]);
        clip(
            &mut rig.editor,
            track,
            scene,
            long_note(60),
            (1, false),
            launch(Some(LaunchQuantization::Quarter)),
        );
        settings(&mut rig, LaunchQuantization::Bar, true);
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 1, block, press(track, scene));
        send_at(&mut rig, &mut now, 18_001, block, SessionCommand::StopTrack(track));
        play_to(&mut rig, &mut now, BAR_FRAMES + 100, block);
        assert_eq!(
            ons_and_offs(&mut rig).1,
            vec![(BAR_FRAMES, 60)],
            "blocks of {block}"
        );
    }
}

/// A two-track rig: an A clip of one bar looping, and a B clip of four bars
/// looping, both launching at once, B with Legato `legato`.
fn legato_pair(legato: bool) -> (Rig, TrackId, SceneId, SceneId) {
    let (mut rig, tracks, scenes) = rig(1);
    let track = tracks[0];
    let now = launch(Some(LaunchQuantization::None));
    clip(
        &mut rig.editor,
        track,
        scenes[0],
        vec![Note::new(40, Ticks::ZERO, Ticks::QUARTER, 100)],
        (1, true),
        now,
    );
    clip(
        &mut rig.editor,
        track,
        scenes[1],
        beats(),
        (4, true),
        Launch { legato, ..now },
    );
    (rig, track, scenes[0], scenes[1])
}

// Spec: launch-quantisation R27
#[test]
fn a_legato_clip_takes_over_the_clip_time_of_the_clip_on_its_track() {
    for block in BLOCKS {
        let (mut rig, track, a, b) = legato_pair(true);
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 1, block, press(track, a));
        // B starts at 3.3.1 of the song, ten beats after A: ten beats into
        // its own time, where its eleventh note begins.
        let at = 1 + 10 * QUARTER_FRAMES;
        send_at(&mut rig, &mut now, at, block, press(track, b));
        play_to(&mut rig, &mut now, at + 2 * QUARTER_FRAMES + 10, block);
        let (ons, _) = ons_and_offs(&mut rig);
        let from_b: Vec<_> = ons.into_iter().filter(|&(_, key)| key >= 60).collect();
        assert_eq!(
            from_b,
            vec![(at, 70), (at + QUARTER_FRAMES, 71), (at + 2 * QUARTER_FRAMES, 72)],
            "blocks of {block}"
        );
        assert!(near(
            clip_time(&mut rig, b, now).unwrap(),
            Ticks::QUARTER * 10 + tempo().frames_to_ticks(Frames(now - at), RATE)
        ));

        // With nothing playing on the track, B plays from its start.
        let (mut rig, track, _, b) = legato_pair(true);
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 1, block, press(track, b));
        play_to(&mut rig, &mut now, 100, block);
        assert_eq!(ons_and_offs(&mut rig).0, vec![(1, 60)], "blocks of {block}");
    }
}

// Spec: launch-quantisation R27. Without Legato the clip starts over.
#[test]
fn a_clip_without_legato_plays_from_its_start() {
    let (mut rig, track, a, b) = legato_pair(false);
    let mut now = start(&mut rig);
    send_at(&mut rig, &mut now, 1, 64, press(track, a));
    let at = 1 + 10 * QUARTER_FRAMES;
    send_at(&mut rig, &mut now, at, 64, press(track, b));
    play_to(&mut rig, &mut now, at + 100, 64);
    let from_b: Vec<_> = ons_and_offs(&mut rig)
        .0
        .into_iter()
        .filter(|&(_, key)| key >= 60)
        .collect();
    assert_eq!(from_b, vec![(at, 60)]);
}

// Spec: launch-quantisation R28
#[test]
fn a_relaunched_legato_clip_keeps_its_clip_time_and_its_follow_counts_again() {
    for block in BLOCKS {
        let (mut rig, track, _, b) = legato_pair(true);
        // After two bars unlinked, B stops by itself.
        let clip_id = rig
            .editor
            .document()
            .track(track)
            .unwrap()
            .slot(b)
            .unwrap()
            .id;
        rig.editor
            .apply(Command::SetClipFollowActions {
                clip: clip_id,
                patch: FollowPatch {
                    enabled: Some(true),
                    action_a: Some(FollowAction::Stop),
                    action_b: Some(FollowAction::Stop),
                    linked: Some(false),
                    time: Some(Ticks::WHOLE * 2),
                    ..FollowPatch::default()
                },
            })
            .unwrap();
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 1, block, press(track, b));
        // A press at 30,001 restarts it: no jump, no cut note.
        send_at(&mut rig, &mut now, 30_001, block, press(track, b));
        play_to(&mut rig, &mut now, 40_000, block);
        let (ons, offs) = ons_and_offs(&mut rig);
        assert_eq!(
            ons,
            [(1, 60), (12_001, 61), (24_001, 62), (36_001, 63)],
            "blocks of {block}"
        );
        assert!(offs.iter().all(|&(frame, _)| frame != 30_001), "{offs:?}");
        assert!(near(
            clip_time(&mut rig, b, now).unwrap(),
            tempo().frames_to_ticks(Frames(39_999), RATE)
        ));
        // The follow counted from the first start would stop it at 96,001;
        // counted again from 30,001 it stops at 126,001.
        play_to(&mut rig, &mut now, 100_000, block);
        assert_eq!(
            rig.monitor.state().session.first().map(|slot| slot.state),
            Some(SlotState::Playing),
            "blocks of {block}"
        );
        play_to(&mut rig, &mut now, 127_000, block);
        assert!(rig.monitor.state().session.is_empty(), "blocks of {block}");
    }
}

// Spec: launch-quantisation R28, follow-actions R10
#[test]
fn a_follow_next_between_two_legato_clips_keeps_the_clip_time() {
    for block in BLOCKS {
        let (mut rig, tracks, scenes) = rig(1);
        let track = tracks[0];
        let legato = Launch {
            legato: true,
            ..launch(Some(LaunchQuantization::None))
        };
        let first = clip(&mut rig.editor, track, scenes[0], beats(), (4, true), legato);
        clip(&mut rig.editor, track, scenes[1], beats(), (4, true), legato);
        rig.editor
            .apply(Command::SetClipFollowActions {
                clip: first,
                patch: FollowPatch {
                    enabled: Some(true),
                    action_a: Some(FollowAction::Next),
                    action_b: Some(FollowAction::Next),
                    linked: Some(false),
                    time: Some(Ticks::WHOLE),
                    ..FollowPatch::default()
                },
            })
            .unwrap();
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 1, block, press(track, scenes[0]));
        play_to(&mut rig, &mut now, BAR_FRAMES + 100, block);
        // The next clip begins a bar in, on the beat that follows the first
        // clip's fourth, instead of its own first.
        let (ons, _) = ons_and_offs(&mut rig);
        assert_eq!(
            ons.iter().find(|&&(frame, _)| frame >= 1 + BAR_FRAMES),
            Some(&(1 + BAR_FRAMES, 64)),
            "blocks of {block}: {ons:?}"
        );
    }
}

// Spec: launch-quantisation R30
#[test]
fn nudge_forward_moves_the_clip_time_by_a_step_of_the_global_value_at_once() {
    for block in BLOCKS {
        // The step each global value gives, and with it off or None a
        // thirty-second note.
        let cases = [
            (LaunchQuantization::Bar, true, Ticks::WHOLE),
            (LaunchQuantization::QuarterTriplet, true, Ticks::QUARTER * 2 / 3),
            (LaunchQuantization::None, true, Ticks::SIXTEENTH / 2),
            (LaunchQuantization::Bar, false, Ticks::SIXTEENTH / 2),
        ];
        for (value, enabled, step) in cases {
            let (mut rig, track, _, b) = legato_pair(false);
            settings(&mut rig, value, enabled);
            let mut now = start(&mut rig);
            // Launch at once (the clip's own step is None), then nudge.
            send_at(&mut rig, &mut now, 1, block, press(track, b));
            send_at(&mut rig, &mut now, 12_001, block, nudge(track, b, 1));
            play_to(&mut rig, &mut now, 12_002, block);
            let before = tempo().frames_to_ticks(Frames(12_001), RATE)
                - tempo().frames_to_ticks(Frames(1), RATE);
            let after = clip_time(&mut rig, b, 12_001).unwrap();
            assert!(
                near(after, before + step),
                "{value:?} switch {enabled}, blocks of {block}: {after:?} against {before:?} + {step:?}"
            );
        }
    }
}

// Spec: launch-quantisation R32
#[test]
fn nudge_backward_before_the_start_goes_a_loop_on_or_to_the_start() {
    for block in BLOCKS {
        // A looping clip half a beat in, nudged a beat back: a whole loop
        // (four bars) on.
        let (mut rig, track, _, b) = legato_pair(false);
        settings(&mut rig, LaunchQuantization::Quarter, true);
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 1, block, press(track, b));
        send_at(&mut rig, &mut now, 6_001, block, nudge(track, b, -1));
        play_to(&mut rig, &mut now, 6_002, block);
        assert!(
            near(
                clip_time(&mut rig, b, 6_001).unwrap(),
                Ticks::QUARTER * 16 - Ticks::QUARTER / 2
            ),
            "blocks of {block}: {:?}",
            clip_time(&mut rig, b, 6_001)
        );

        // A clip without Loop goes to its start.
        let (mut rig, tracks, scenes) = self::rig(1);
        let (track, scene) = (tracks[0], scenes[0]);
        clip(
            &mut rig.editor,
            track,
            scene,
            beats(),
            (4, false),
            launch(Some(LaunchQuantization::None)),
        );
        settings(&mut rig, LaunchQuantization::Quarter, true);
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 1, block, press(track, scene));
        send_at(&mut rig, &mut now, 6_001, block, nudge(track, scene, -1));
        play_to(&mut rig, &mut now, 6_002, block);
        assert!(near(clip_time(&mut rig, scene, 6_001).unwrap(), Ticks::ZERO));
        // And it goes on from there.
        send_at(&mut rig, &mut now, 6_002, block, nudge(track, scene, 2));
        play_to(&mut rig, &mut now, 6_003, block);
        assert!(near(
            clip_time(&mut rig, scene, 6_002).unwrap(),
            Ticks::QUARTER * 2
        ));
    }
}

// Spec: launch-quantisation R31, R33
#[test]
fn a_nudge_of_a_stopped_or_queued_slot_changes_nothing_and_a_queued_stop_keeps_its_frame() {
    for block in BLOCKS {
        // A stopped slot.
        let (mut rig, track, _, b) = legato_pair(false);
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 100, block, nudge(track, b, 1));
        play_to(&mut rig, &mut now, 200, block);
        assert!(rig.monitor.state().session.is_empty());

        // A queued one starts at its line with nothing added.
        let (mut rig, tracks, scenes) = self::rig(1);
        let (track, scene) = (tracks[0], scenes[0]);
        clip(
            &mut rig.editor,
            track,
            scene,
            beats(),
            (4, true),
            launch(None),
        );
        settings(&mut rig, LaunchQuantization::Bar, true);
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 6_001, block, press(track, scene));
        send_at(&mut rig, &mut now, 12_001, block, nudge(track, scene, 1));
        play_to(&mut rig, &mut now, BAR_FRAMES + 100, block);
        assert_eq!(ons_and_offs(&mut rig).0[0], (BAR_FRAMES, 60), "blocks of {block}");

        // A playing one with a stop queued is nudged, and still stops on
        // the frame it was queued for.
        let (mut rig, tracks, scenes) = self::rig(1);
        let (track, scene) = (tracks[0], scenes[0]);
        clip(
            &mut rig.editor,
            track,
            scene,
            long_note(60),
            (4, true),
            launch(Some(LaunchQuantization::None)),
        );
        settings(&mut rig, LaunchQuantization::Bar, true);
        let mut now = start(&mut rig);
        send_at(&mut rig, &mut now, 1, block, press(track, scene));
        send_at(&mut rig, &mut now, 6_001, block, SessionCommand::StopTrack(track));
        send_at(&mut rig, &mut now, 12_001, block, nudge(track, scene, 1));
        play_to(&mut rig, &mut now, BAR_FRAMES - 100, block);
        let states = rig.monitor.state().session.clone();
        assert_eq!(
            states.iter().map(|slot| slot.state).collect::<Vec<_>>(),
            [SlotState::QueuedToStop],
            "blocks of {block}"
        );
        play_to(&mut rig, &mut now, BAR_FRAMES + 100, block);
        assert!(rig.monitor.state().session.is_empty(), "blocks of {block}");
    }
}

/// A ramp file whose frame `i` holds `(i + 1) * STEP`, read back through
/// the clip's position.
const STEP: f32 = 1.0 / 65_536.0;

fn ramp(frames: usize) -> Arc<SampleData> {
    Arc::new(
        SampleData::new(
            48_000,
            vec![(0..frames).map(|i| (i + 1) as f32 * STEP).collect()],
        )
        .unwrap(),
    )
}

fn at(frame: i64) -> f32 {
    (frame + 1) as f32 * STEP
}

/// Renders an audio track holding two Legato clips on one file, A launched
/// at once and B at 20,000 (a Legato swap), then B nudged at 40,000, in
/// blocks of `block`; the left channel.
fn render_swap_and_nudge(audio: AudioData, block: usize) -> Vec<f32> {
    let mut media = MediaMap::new();
    media.insert(ContentHash([7; 32]), ramp(120_000));
    let registry = Registry::new().with_media(Arc::new(media));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    let mut editor = Editor::new();
    let track = track(&mut editor, TrackKind::Audio, None);
    let scenes: Vec<SceneId> = editor.document().scenes().take(2).map(|s| s.id).collect();
    for &scene in &scenes {
        let id = editor.new_id();
        editor
            .apply(Command::CreateSlotClip {
                track,
                scene,
                clip: Clip::new(
                    id,
                    Ticks::ZERO,
                    Ticks::WHOLE * 4,
                    ClipData::Audio(audio.clone()),
                ),
            })
            .unwrap();
        editor
            .apply(Command::SetClipLegato {
                clip: id,
                legato: true,
            })
            .unwrap();
        editor
            .apply(Command::SetClipLaunchQuantization {
                clip: id,
                quantization: Some(LaunchQuantization::None),
            })
            .unwrap();
    }
    editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    offline
        .sender()
        .session(SessionCommand::SetQuantization(LaunchSettings {
            quantization: LaunchQuantization::None,
            enabled: true,
            ..LaunchSettings::default()
        }));
    offline.sender().transport(TransportCommand::Play);
    let cues = [
        (1, press(track, scenes[0])),
        (20_000, press(track, scenes[1])),
        (40_000, nudge(track, scenes[1], 1)),
    ];
    let mut out = Vec::new();
    let mut done = 0u64;
    let mut next = 0;
    while done < 60_000 {
        while next < cues.len() && cues[next].0 <= done {
            offline.sender().session(cues[next].1);
            next += 1;
        }
        let until = cues.get(next).map_or(60_000, |&(frame, _)| frame);
        let n = block.min((60_000 - done) as usize).min((until - done) as usize);
        offline.render(n as u64, |output| out.extend_from_slice(output.channel(0)));
        done += n as u64;
    }
    assert_eq!(offline.heap_operations(), 0);
    out
}

// Spec: launch-quantisation R27, R29, R30, disk-streaming R8
#[test]
fn an_offline_render_of_a_legato_swap_and_a_nudge_is_the_same_at_every_block_size() {
    use bl_model::{WarpMarker, WarpMode};
    let unwarped = AudioData::new(ContentHash([7; 32]));
    let warped = AudioData {
        warp: true,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(240.0),
        warp_mode: WarpMode::RePitch,
        high_quality: false,
        ..AudioData::new(ContentHash([7; 32]))
    };
    // The unwarped render is bit for bit the same; a warped one reads
    // through its interpolator's window, which block sizes cut differently
    // by less than a file frame (the warped test in session.rs allows 0.02).
    for (audio, tolerance) in [(unwarped.clone(), 0.0), (warped, 1e-4)] {
        let reference = render_swap_and_nudge(audio.clone(), 512);
        for block in [1, 64] {
            let got = render_swap_and_nudge(audio.clone(), block);
            assert_eq!(got.len(), reference.len());
            for (i, (a, b)) in got.iter().zip(&reference).enumerate() {
                assert!(
                    (a - b).abs() <= tolerance,
                    "warp {} blocks of {block}: frame {i}: {a} against {b}",
                    audio.warp
                );
            }
        }
    }
    // Unwarped, the swap takes over B's time at the swap frame, and the
    // nudge moves it on by a thirty-second (1,500 frames) at once.
    let out = render_swap_and_nudge(unwarped, 64);
    assert_eq!(out[21_000], at(20_999), "B reads on from A's place, not from its start");
    assert_eq!(out[45_000], at(45_000 - 1 + 1_500), "after the nudge");
}
