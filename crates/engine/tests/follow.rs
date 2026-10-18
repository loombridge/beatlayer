//! Follow actions (WP-3.10): each action lands on its frame at any block
//! size, linked and unlinked times fire where the follow-actions spec puts
//! them, the global switch silences them, scenes follow too, and the seeded
//! draw makes two renders of one set alike. Every block runs as a real-time
//! section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::{SessionCommand, SlotState, TransportCommand, testing};
use bl_model::{
    Clip, ClipData, ClipId, Command, Editor, FollowAction, FollowPatch, LaunchQuantization,
    LaunchSettings, MidiData, Note, SceneId, TrackId, TrackKind,
};
use common::{Rig, device, track};

/// A 4/4 bar in ticks, and in frames at 120 BPM and 48 kHz.
const BAR: Ticks = Ticks::WHOLE;
const BAR_FRAMES: i64 = 96_000;
const BEAT_FRAMES: i64 = 24_000;
const BLOCKS: [usize; 3] = [64, 700, 4096];

fn rig() -> (Rig, TrackId, Vec<SceneId>) {
    let mut rig = Rig::new(4096);
    rig.editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
        })
        .unwrap();
    let track = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, track, testing::INSTRUMENT, &[]);
    let scenes = rig.editor.document().scenes().map(|s| s.id).collect();
    (rig, track, scenes)
}

/// A one-bar clip in the slot, holding one quarter-note `key` at its start.
fn clip(editor: &mut Editor, track: TrackId, scene: SceneId, key: u8) -> ClipId {
    let id = editor.new_id();
    let notes = vec![Note::new(key, Ticks::ZERO, Ticks::QUARTER, 100)];
    editor
        .apply(Command::CreateSlotClip {
            track,
            scene,
            clip: Clip::new(id, Ticks::ZERO, BAR, ClipData::Midi(MidiData::new(notes))),
        })
        .unwrap();
    id
}

fn follow(editor: &mut Editor, clip: ClipId, patch: FollowPatch) {
    editor
        .apply(Command::SetClipFollowActions { clip, patch })
        .unwrap();
}

fn on() -> FollowPatch {
    FollowPatch {
        enabled: Some(true),
        ..FollowPatch::default()
    }
}

/// Plays `frames` frames in blocks of `block`.
fn play(rig: &mut Rig, frames: i64, block: usize) {
    let mut done = 0;
    while done < frames as usize {
        let n = block.min(frames as usize - done);
        rig.block(n);
        done += n;
    }
}

/// Starts the transport with launches immediate, so a launch lands on the
/// frame it is sent.
fn start(rig: &mut Rig, follow_actions: bool) {
    rig.sync();
    rig.sender
        .session(SessionCommand::SetQuantization(LaunchSettings {
            quantization: LaunchQuantization::None,
            enabled: false,
            follow_actions,
        }));
    rig.sender.transport(TransportCommand::Play);
}

/// The note-ons played so far, as `(frame, key)`.
fn starts(rig: &mut Rig) -> Vec<(i64, u8)> {
    rig.probe
        .recorded_notes()
        .into_iter()
        .filter(|note| note.event.kind == bl_engine::NoteKind::On)
        .map(|note| (note.frame, note.event.key))
        .collect()
}

// Spec: follow-actions R2, R5
#[test]
fn a_next_chain_follows_at_each_clips_end_and_wraps_at_any_block_size() {
    for block in BLOCKS {
        let (mut rig, track, scenes) = rig();
        for (n, key) in [60, 61, 62].into_iter().enumerate() {
            let id = clip(&mut rig.editor, track, scenes[n], key);
            follow(&mut rig.editor, id, on());
        }
        start(&mut rig, true);
        rig.sender.session(SessionCommand::Launch {
            track,
            scene: scenes[0],
        });
        play(&mut rig, BAR_FRAMES * 3 + 100, block);
        assert_eq!(
            starts(&mut rig),
            vec![
                (0, 60),
                (BAR_FRAMES, 61),
                (BAR_FRAMES * 2, 62),
                (BAR_FRAMES * 3, 60)
            ],
            "blocks of {block}"
        );
    }
}

// Spec: follow-actions R2, R9
#[test]
fn an_unlinked_time_shorter_than_the_clip_cuts_it_and_a_longer_one_waits() {
    for block in BLOCKS {
        let (mut rig, track, scenes) = rig();
        let first = clip(&mut rig.editor, track, scenes[0], 60);
        clip(&mut rig.editor, track, scenes[1], 61);
        follow(
            &mut rig.editor,
            first,
            FollowPatch {
                linked: Some(false),
                time: Some(Ticks::QUARTER * 2),
                ..on()
            },
        );
        start(&mut rig, true);
        rig.sender.session(SessionCommand::Launch {
            track,
            scene: scenes[0],
        });
        play(&mut rig, BAR_FRAMES, block);
        // 0.2.0 on a one-bar clip: the next clip starts two beats in.
        assert_eq!(
            starts(&mut rig),
            vec![(0, 60), (BEAT_FRAMES * 2, 61)],
            "blocks of {block}"
        );

        // Two bars on a one-bar clip: the clip ends and stays silent until the
        // follow time.
        let (mut rig, track, scenes) = self::rig();
        let first = clip(&mut rig.editor, track, scenes[0], 60);
        clip(&mut rig.editor, track, scenes[1], 61);
        follow(
            &mut rig.editor,
            first,
            FollowPatch {
                linked: Some(false),
                time: Some(BAR * 2),
                ..on()
            },
        );
        start(&mut rig, true);
        rig.sender.session(SessionCommand::Launch {
            track,
            scene: scenes[0],
        });
        play(&mut rig, BAR_FRAMES * 2 + 10, block);
        assert_eq!(
            starts(&mut rig),
            vec![(0, 60), (BAR_FRAMES * 2, 61)],
            "blocks of {block}"
        );
    }
}

// Spec: follow-actions R10
#[test]
fn a_target_with_its_own_quantisation_waits_for_its_grid_and_global_is_ignored() {
    let (mut rig, track, scenes) = rig();
    let first = clip(&mut rig.editor, track, scenes[0], 60);
    clip(&mut rig.editor, track, scenes[1], 61);
    follow(
        &mut rig.editor,
        first,
        FollowPatch {
            linked: Some(false),
            time: Some(Ticks::QUARTER * 2),
            ..on()
        },
    );
    // The global quantisation is a bar; the target's own is a bar too.
    rig.sync();
    rig.sender
        .session(SessionCommand::SetQuantization(LaunchSettings {
            quantization: LaunchQuantization::Bar,
            enabled: true,
            follow_actions: true,
        }));
    rig.sender.transport(TransportCommand::Play);
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: scenes[0],
    });
    play(&mut rig, BAR_FRAMES * 2, 512);
    // Launched on the bar line it was asked for; the follow at two beats
    // after it starts the target at once: the global grid doesn't apply.
    assert_eq!(
        starts(&mut rig),
        vec![(BAR_FRAMES, 60), (BAR_FRAMES + BEAT_FRAMES * 2, 61)]
    );

    let (mut rig, track, scenes) = self::rig();
    let first = clip(&mut rig.editor, track, scenes[0], 60);
    let second = clip(&mut rig.editor, track, scenes[1], 61);
    rig.editor
        .apply(Command::SetClipLaunchQuantization {
            clip: second,
            quantization: Some(LaunchQuantization::Bar),
        })
        .unwrap();
    follow(
        &mut rig.editor,
        first,
        FollowPatch {
            linked: Some(false),
            time: Some(Ticks::QUARTER * 2),
            ..on()
        },
    );
    start(&mut rig, true);
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: scenes[0],
    });
    play(&mut rig, BAR_FRAMES * 2, 512);
    assert_eq!(starts(&mut rig), vec![(0, 60), (BAR_FRAMES, 61)]);

    // The clip that followed stops at the follow time, so the track is
    // silent until the target's grid line: its notes on beats 3 and 4 never
    // play.
    let (mut rig, track, scenes) = self::rig();
    let id = rig.editor.new_id();
    let notes: Vec<_> = (0..4)
        .map(|beat| Note::new(60, Ticks::QUARTER * beat, Ticks::SIXTEENTH, 100))
        .collect();
    rig.editor
        .apply(Command::CreateSlotClip {
            track,
            scene: scenes[0],
            clip: Clip::new(id, Ticks::ZERO, BAR, ClipData::Midi(MidiData::new(notes))),
        })
        .unwrap();
    let second = clip(&mut rig.editor, track, scenes[1], 61);
    rig.editor
        .apply(Command::SetClipLaunchQuantization {
            clip: second,
            quantization: Some(LaunchQuantization::Bar),
        })
        .unwrap();
    follow(
        &mut rig.editor,
        id,
        FollowPatch {
            linked: Some(false),
            time: Some(Ticks::QUARTER * 2),
            ..on()
        },
    );
    start(&mut rig, true);
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: scenes[0],
    });
    play(&mut rig, BAR_FRAMES + BEAT_FRAMES, 512);
    assert_eq!(
        starts(&mut rig),
        vec![(0, 60), (BEAT_FRAMES, 60), (BAR_FRAMES, 61)]
    );
}

// Spec: follow-actions R6
#[test]
fn stop_ends_the_clip_at_the_follow_time_and_the_transport_runs_on() {
    for block in BLOCKS {
        let (mut rig, track, scenes) = rig();
        let id = clip(&mut rig.editor, track, scenes[0], 60);
        follow(
            &mut rig.editor,
            id,
            FollowPatch {
                action_a: Some(FollowAction::Stop),
                ..on()
            },
        );
        start(&mut rig, true);
        rig.sender.session(SessionCommand::Launch {
            track,
            scene: scenes[0],
        });
        play(&mut rig, BAR_FRAMES, block);
        assert!(
            rig.monitor
                .state()
                .session
                .iter()
                .any(|slot| slot.state == SlotState::Playing),
            "blocks of {block}"
        );
        play(&mut rig, 1, block);
        let state = rig.monitor.state();
        assert!(
            state.session.is_empty(),
            "stopped on its frame: {:?}",
            state.session
        );
        assert!(state.playing, "the transport keeps running");
    }
}

// Spec: follow-actions R6
#[test]
fn play_again_restarts_the_clip_and_no_action_leaves_it() {
    let (mut rig, track, scenes) = rig();
    let id = clip(&mut rig.editor, track, scenes[0], 60);
    follow(
        &mut rig.editor,
        id,
        FollowPatch {
            action_a: Some(FollowAction::PlayAgain),
            ..on()
        },
    );
    start(&mut rig, true);
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: scenes[0],
    });
    play(&mut rig, BAR_FRAMES * 3 + 5, 700);
    assert_eq!(
        starts(&mut rig),
        vec![
            (0, 60),
            (BAR_FRAMES, 60),
            (BAR_FRAMES * 2, 60),
            (BAR_FRAMES * 3, 60)
        ]
    );

    let (mut rig, track, scenes) = self::rig();
    let id = clip(&mut rig.editor, track, scenes[0], 60);
    follow(
        &mut rig.editor,
        id,
        FollowPatch {
            action_a: Some(FollowAction::NoAction),
            ..on()
        },
    );
    start(&mut rig, true);
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: scenes[0],
    });
    play(&mut rig, BAR_FRAMES * 3, 700);
    assert_eq!(starts(&mut rig), vec![(0, 60)]);
    assert_eq!(rig.monitor.state().session.len(), 1, "it plays on");
}

// Spec: follow-actions R3
#[test]
fn jump_goes_to_its_slot_and_an_empty_slot_stops_the_clip() {
    let (mut rig, track, scenes) = rig();
    let first = clip(&mut rig.editor, track, scenes[0], 60);
    clip(&mut rig.editor, track, scenes[3], 63);
    follow(
        &mut rig.editor,
        first,
        FollowPatch {
            action_a: Some(FollowAction::Jump),
            jump_a: Some(4),
            ..on()
        },
    );
    start(&mut rig, true);
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: scenes[0],
    });
    play(&mut rig, BAR_FRAMES + 10, 512);
    assert_eq!(starts(&mut rig), vec![(0, 60), (BAR_FRAMES, 63)]);

    let (mut rig, track, scenes) = self::rig();
    let first = clip(&mut rig.editor, track, scenes[0], 60);
    follow(
        &mut rig.editor,
        first,
        FollowPatch {
            action_a: Some(FollowAction::Jump),
            jump_a: Some(2),
            ..on()
        },
    );
    start(&mut rig, true);
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: scenes[0],
    });
    play(&mut rig, BAR_FRAMES + 10, 512);
    assert_eq!(starts(&mut rig), vec![(0, 60)]);
    assert!(rig.monitor.state().session.is_empty());
}

// Spec: follow-actions R8
#[test]
fn with_the_global_switch_off_no_action_runs_and_on_again_the_next_start_follows() {
    let (mut rig, track, scenes) = rig();
    let first = clip(&mut rig.editor, track, scenes[0], 60);
    clip(&mut rig.editor, track, scenes[1], 61);
    follow(&mut rig.editor, first, on());
    start(&mut rig, false);
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: scenes[0],
    });
    play(&mut rig, BAR_FRAMES * 2, 512);
    assert_eq!(starts(&mut rig), vec![(0, 60)], "off: nothing follows");

    rig.sender
        .session(SessionCommand::SetQuantization(LaunchSettings {
            quantization: LaunchQuantization::None,
            enabled: false,
            follow_actions: true,
        }));
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: scenes[0],
    });
    play(&mut rig, BAR_FRAMES + 10, 512);
    let again = starts(&mut rig);
    assert_eq!(again.len(), 2, "{again:?}");
    assert_eq!(again[1].1, 61, "on: the next start follows");
    assert_eq!(again[1].0 - again[0].0, BAR_FRAMES);
}

// Spec: follow-actions R11
#[test]
fn a_scenes_follow_action_runs_when_the_scene_launches_and_not_when_its_clip_does() {
    for block in BLOCKS {
        let (mut rig, track, scenes) = rig();
        clip(&mut rig.editor, track, scenes[0], 60);
        clip(&mut rig.editor, track, scenes[1], 61);
        rig.editor
            .apply(Command::SetSceneFollowActions {
                scene: scenes[0],
                patch: on(),
            })
            .unwrap();
        start(&mut rig, true);
        // Launching the clip alone runs no scene action.
        rig.sender.session(SessionCommand::Launch {
            track,
            scene: scenes[0],
        });
        play(&mut rig, BAR_FRAMES * 2, block);
        assert_eq!(starts(&mut rig), vec![(0, 60)], "blocks of {block}");

        // Launching the scene does: Longest is a bar, and Next goes to scene 2.
        rig.sender.session(SessionCommand::LaunchScene(scenes[0]));
        let launched = BAR_FRAMES * 2;
        play(&mut rig, BAR_FRAMES + 10, block);
        assert_eq!(
            starts(&mut rig),
            vec![(launched, 60), (launched + BAR_FRAMES, 61)],
            "blocks of {block}"
        );
    }
}

// Spec: follow-actions R4, R12
#[test]
fn two_renders_make_the_same_choices_and_the_split_matches_the_chance() {
    let run = || {
        let (mut rig, track, scenes) = rig();
        // Two clips of a sixteenth: each jumps to clip 1 with 70 % and to
        // clip 2 with the rest.
        for (n, key) in [(0, 60), (1, 61)] {
            let id = clip(&mut rig.editor, track, scenes[n], key);
            follow(
                &mut rig.editor,
                id,
                FollowPatch {
                    action_a: Some(FollowAction::Jump),
                    jump_a: Some(1),
                    action_b: Some(FollowAction::Jump),
                    jump_b: Some(2),
                    chance_a: Some(70),
                    linked: Some(false),
                    time: Some(Ticks::SIXTEENTH),
                    ..on()
                },
            );
        }
        start(&mut rig, true);
        rig.sender.session(SessionCommand::Launch {
            track,
            scene: scenes[0],
        });
        let mut all = Vec::new();
        for _ in 0..40 {
            play(&mut rig, 6_000 * 25, 4096);
            all.extend(starts(&mut rig));
        }
        all
    };
    let (first, second) = (run(), run());
    assert_eq!(first, second, "bit for bit the same choices");
    assert!(first.len() > 900, "{} follows", first.len());
    let ones = first.iter().skip(1).filter(|(_, key)| *key == 60).count();
    let share = ones as f64 / (first.len() - 1) as f64;
    assert!((share - 0.7).abs() < 0.05, "A took {share}");
}
