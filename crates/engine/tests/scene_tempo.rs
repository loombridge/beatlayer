//! A scene's tempo and time signature (WP-3.5): launching a scene applies
//! them on the frame its clips start on, inside the running program and
//! without a rebuild, and they hold until Re-Enable Automation or Back to
//! Arrangement gives the set's own back. Every block runs as a real-time
//! section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Bpm, TempoMap, Ticks, TimeSignature};
use bl_engine::{SceneOverride, SessionCommand, TransportCommand, testing};
use bl_model::{
    Clip, ClipData, Command, Editor, LaunchQuantization, LaunchSettings, MidiData, Note, SceneId,
    TrackId, TrackKind,
};
use common::{Rig, device, track};

/// One 4/4 bar in frames at 120 BPM and 48 kHz.
const BAR_FRAMES: i64 = 96_000;

fn bpm(value: f64) -> Bpm {
    Bpm::new(value).unwrap()
}

fn scene_at(rig: &Rig, index: usize) -> SceneId {
    rig.editor.document().scenes().nth(index).unwrap().id
}

/// A MIDI track with the test instrument.
fn midi_track(rig: &mut Rig) -> TrackId {
    let track = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, track, testing::INSTRUMENT, &[]);
    track
}

/// Puts a one-bar clip with a note on the first beat and one on the second
/// in the track's slot at `scene`.
fn two_beat_clip(editor: &mut Editor, track: TrackId, scene: SceneId) {
    let id = editor.new_id();
    let notes = vec![
        Note::new(60, Ticks::ZERO, Ticks::QUARTER, 100),
        Note::new(62, Ticks::QUARTER, Ticks::QUARTER, 100),
    ];
    let clip = Clip::new(
        id,
        Ticks::ZERO,
        Ticks::WHOLE,
        ClipData::Midi(MidiData::new(notes)),
    );
    editor
        .apply(Command::CreateSlotClip { track, scene, clip })
        .unwrap();
}

fn play(rig: &mut Rig, frames: usize, block: usize) {
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        rig.block(n);
        done += n;
    }
}

/// The notes the test instrument played, as `(song frame, key, on)`.
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

/// A rig at 120 BPM whose first scene holds a two-beat clip on one MIDI
/// track, with `edit` applied to the first scene's settings.
fn rig_with_scene(block: usize, edit: impl FnOnce(&mut Rig, SceneId)) -> (Rig, TrackId, SceneId) {
    let mut rig = Rig::new(block);
    rig.editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(bpm(120.0)),
        })
        .unwrap();
    let track = midi_track(&mut rig);
    let scene = scene_at(&rig, 0);
    two_beat_clip(&mut rig.editor, track, scene);
    edit(&mut rig, scene);
    rig.sync();
    (rig, track, scene)
}

// Spec: session-view R22, R23
#[test]
fn a_scenes_tempo_applies_on_its_launch_frame_at_any_block_size() {
    for block in [1, 100, 512, 4096] {
        let (mut rig, _, scene) = rig_with_scene(block, |rig, scene| {
            rig.editor
                .apply(Command::SetSceneTempo {
                    scene,
                    tempo: Some(bpm(60.0)),
                })
                .unwrap();
        });
        rig.sender.transport(TransportCommand::Play);
        // Asked for a frame past 1.2.1: the 1-bar grid puts the launch on
        // 2.1.1, frame 96,000.
        play(&mut rig, 24_001, block);
        rig.sender.session(SessionCommand::LaunchScene(scene));
        play(&mut rig, 1, block);
        assert_eq!(
            rig.monitor.state().scene_override,
            SceneOverride::default(),
            "nothing applies before the launch frame (blocks of {block})"
        );
        play(&mut rig, BAR_FRAMES as usize - 24_002, block);
        assert!(
            rig.monitor.state().scene_override.is_none(),
            "the launch frame has not come yet (blocks of {block})"
        );
        // Past the launch frame the tempo is 60: the playhead kept its beat
        // (2.1.1, 16 quarters in) and moves at half the speed.
        play(&mut rig, 8, block);
        let state = rig.monitor.state();
        assert_eq!(
            state.scene_override.tempo,
            Some((scene, bpm(60.0))),
            "blocks of {block}"
        );
        let applied_ticks = state.position_ticks;
        assert!(
            applied_ticks >= Ticks::QUARTER * 4 && applied_ticks < Ticks::QUARTER * 5,
            "the beat held at 2.1.1: {applied_ticks:?} (blocks of {block})"
        );
        // The second note lands one beat after the first, at 60 BPM: a
        // second of 48,000 frames, not 24,000.
        play(&mut rig, 60_000, block);
        let ons: Vec<_> = notes(&mut rig).into_iter().filter(|n| n.2).collect();
        assert_eq!(ons.len(), 2, "{ons:?} (blocks of {block})");
        assert_eq!(ons[1].0 - ons[0].0, 48_000, "{ons:?} (blocks of {block})");
    }
}

// Spec: session-view R22
#[test]
fn a_scene_without_a_tempo_leaves_the_sets_and_a_tempo_switched_off_does_too() {
    for switch_off in [false, true] {
        let (mut rig, _, scene) = rig_with_scene(256, |rig, scene| {
            if switch_off {
                rig.editor
                    .apply(Command::SetSceneTempo {
                        scene,
                        tempo: Some(bpm(60.0)),
                    })
                    .unwrap();
                rig.editor
                    .apply(Command::SetSceneTempo { scene, tempo: None })
                    .unwrap();
            }
        });
        rig.sender.transport(TransportCommand::Play);
        play(&mut rig, 24_001, 256);
        rig.sender.session(SessionCommand::LaunchScene(scene));
        play(&mut rig, 120_000, 256);
        assert!(rig.monitor.state().scene_override.is_none());
        let ons: Vec<_> = notes(&mut rig).into_iter().filter(|n| n.2).collect();
        assert_eq!(
            ons[1].0 - ons[0].0,
            24_000,
            "the set's 120 BPM held: {ons:?}"
        );
    }
}

// Spec: session-view R22
#[test]
fn an_empty_scene_launched_with_the_song_stopped_applies_its_tempo_and_starts_nothing() {
    let (mut rig, _, _) = rig_with_scene(256, |_, _| {});
    let empty = scene_at(&rig, 1);
    rig.editor
        .apply(Command::SetSceneTempo {
            scene: empty,
            tempo: Some(bpm(90.0)),
        })
        .unwrap();
    rig.sync();
    rig.sender.session(SessionCommand::LaunchScene(empty));
    play(&mut rig, 256, 256);
    let state = rig.monitor.state();
    assert!(!state.playing, "launching an empty scene starts no song");
    assert_eq!(state.scene_override.tempo, Some((empty, bpm(90.0))));
    assert!(state.session.is_empty());
}

// Spec: session-view R22
#[test]
fn a_launched_tempo_holds_across_stop_and_play_until_it_is_cleared() {
    let (mut rig, _, scene) = rig_with_scene(256, |rig, scene| {
        rig.editor
            .apply(Command::SetSceneTempo {
                scene,
                tempo: Some(bpm(60.0)),
            })
            .unwrap();
    });
    rig.sender.session(SessionCommand::LaunchScene(scene));
    play(&mut rig, 1_000, 256);
    let applied = Some((scene, bpm(60.0)));
    assert_eq!(rig.monitor.state().scene_override.tempo, applied);
    // Stop and play again: the tempo stays.
    rig.sender.transport(TransportCommand::Stop);
    play(&mut rig, 256, 256);
    rig.sender.transport(TransportCommand::Play);
    play(&mut rig, 256, 256);
    assert_eq!(rig.monitor.state().scene_override.tempo, applied);
    // Re-Enable Automation (or Back to Arrangement) gives the set's own back.
    rig.sender.session(SessionCommand::ClearSceneOverride);
    play(&mut rig, 256, 256);
    assert!(rig.monitor.state().scene_override.is_none());
}

// Spec: session-view R23
#[test]
fn a_program_compiled_before_the_launch_keeps_the_tempo_and_a_recompile_follows_it() {
    let (mut rig, _, scene) = rig_with_scene(256, |rig, scene| {
        rig.editor
            .apply(Command::SetSceneTempo {
                scene,
                tempo: Some(bpm(60.0)),
            })
            .unwrap();
    });
    rig.sender.session(SessionCommand::LaunchScene(scene));
    play(&mut rig, 256, 256);
    let state = rig.monitor.state();
    let (ticks, override_) = (state.position_ticks, state.scene_override);
    // An ordinary edit compiles a program that knows nothing of the launch:
    // the audio thread still plays the scene's tempo, on the same beat.
    rig.editor
        .apply(Command::RenameScene {
            scene,
            name: "Verse".to_owned(),
        })
        .unwrap();
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    play(&mut rig, 256, 256);
    assert_eq!(rig.monitor.state().scene_override, override_);
    assert!(rig.monitor.state().position_ticks >= ticks);
    // The main thread hears the override and has the compiler follow it:
    // the update it brings is a whole program with the tempo in place.
    let update = rig.compiler.set_override(override_);
    assert!(update.program.is_some());
    rig.sender.send_update(update);
    assert!(rig.sender.flush());
    play(&mut rig, 256, 256);
    assert_eq!(rig.monitor.state().scene_override, override_);
    // Taking the scene's tempo away gives the override up on the next swap.
    rig.editor
        .apply(Command::SetSceneTempo { scene, tempo: None })
        .unwrap();
    rig.sync();
    play(&mut rig, 256, 256);
    assert!(rig.monitor.state().scene_override.is_none());
}

// Spec: session-view R22, R23
#[test]
fn a_scenes_signature_moves_the_bar_lines_the_next_launch_waits_for() {
    let (mut rig, track, scene) = rig_with_scene(256, |rig, scene| {
        rig.editor
            .apply(Command::SetSceneTimeSignature {
                scene,
                signature: Some(TimeSignature::new(3, 4).unwrap()),
            })
            .unwrap();
    });
    let second = scene_at(&rig, 1);
    two_beat_clip(&mut rig.editor, track, second);
    rig.sync();
    rig.sender
        .session(SessionCommand::SetQuantization(LaunchSettings {
            quantization: LaunchQuantization::Bar,
            enabled: true,
        }));
    rig.sender.transport(TransportCommand::Play);
    play(&mut rig, 24_001, 256);
    rig.sender.session(SessionCommand::LaunchScene(scene));
    // The first scene launches on 2.1.1, frame 96,000, and brings 3/4.
    play(&mut rig, 80_000, 256);
    assert_eq!(
        rig.monitor.state().scene_override.signature,
        Some((scene, TimeSignature::new(3, 4).unwrap()))
    );
    // A launch now waits for the next 3/4 bar line: the bars of 3/4 count
    // from 1.1.1, so 3 quarters (72,000 frames) apart, and the next line
    // after frame 104,000 is the sixth bar's (frame 144,000).
    rig.sender.session(SessionCommand::LaunchScene(second));
    play(&mut rig, 60_000, 256);
    let ons: Vec<_> = notes(&mut rig)
        .into_iter()
        .filter(|n| n.2 && n.1 == 60)
        .collect();
    assert_eq!(ons.len(), 2, "{ons:?}");
    assert_eq!(ons[1].0, 144_000, "{ons:?}");
}

// Spec: session-view R25
#[test]
fn playing_clips_move_to_the_captured_scene_and_play_on_without_a_break() {
    let (mut rig, track, first) = rig_with_scene(256, |_, _| {});
    let second = scene_at(&rig, 1);
    two_beat_clip(&mut rig.editor, track, second);
    rig.sync();
    rig.sender.session(SessionCommand::LaunchScene(first));
    play(&mut rig, 12_000, 256);
    rig.sender.session(SessionCommand::MoveToScene(second));
    play(&mut rig, 256, 256);
    let states = rig.monitor.state().session.clone();
    assert_eq!(states.len(), 1, "{states:?}");
    assert_eq!(states[0].scene, second);
    assert_eq!(states[0].state, bl_engine::SlotState::Playing);
    // The clip goes on from where it was: the second beat's note sounds
    // once, on time, and the first beat's isn't struck again.
    play(&mut rig, 24_000, 256);
    let ons: Vec<_> = notes(&mut rig)
        .into_iter()
        .filter(|n| n.2)
        .map(|n| (n.0, n.1))
        .collect();
    assert_eq!(ons, vec![(0, 60), (24_000, 62)]);
}

// Spec: session-view R23
#[test]
fn back_to_arrangement_gives_back_the_sets_tempo() {
    let (mut rig, _, scene) = rig_with_scene(256, |rig, scene| {
        rig.editor
            .apply(Command::SetSceneTempo {
                scene,
                tempo: Some(bpm(60.0)),
            })
            .unwrap();
    });
    rig.sender.session(SessionCommand::LaunchScene(scene));
    play(&mut rig, 1_000, 256);
    assert!(!rig.monitor.state().scene_override.is_none());
    rig.sender.session(SessionCommand::BackToArrangement);
    play(&mut rig, 256, 256);
    assert!(rig.monitor.state().scene_override.is_none());
}
