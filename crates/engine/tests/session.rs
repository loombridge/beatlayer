//! Session View launching (WP-3.2): a launch or a stop lands on the exact
//! frame of the launch quantisation grid at any block size, two launches on
//! one track act in order, a scene's clips start together, and the engine
//! publishes each slot's state. Every block runs as a real-time section
//! under the allocation trap.

use crate::common;
use std::sync::Arc;

use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::{
    EngineConfig, MediaMap, Offline, Registry, SessionCommand, SlotState, TransportCommand, testing,
};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, ClipLaunch, Command, ContentHash, Editor, LaunchMode,
    LaunchQuantization, LaunchSettings, MidiData, Note, SceneId, TrackId, TrackKind,
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

// Spec: session-context-menus R10
#[test]
fn cancelling_a_scene_launch_drops_what_it_queued_at_any_block_size() {
    for block in [1, 100, 512, 4096] {
        let mut rig = Rig::new(block);
        rig.editor
            .apply(Command::SetTempo { tempo: tempo() })
            .unwrap();
        let first = midi_track(&mut rig);
        let second = midi_track(&mut rig);
        let scenes: Vec<SceneId> = rig
            .editor
            .document()
            .scenes()
            .map(|s| s.id)
            .take(2)
            .collect();
        let long = |key| [Note::new(key, Ticks::ZERO, Ticks::WHOLE * 8, 100)];
        slot_clip(&mut rig.editor, first, scenes[0], &long(60));
        slot_clip(&mut rig.editor, first, scenes[1], &long(62));
        slot_clip(&mut rig.editor, second, scenes[1], &long(67));
        rig.sync();
        // The first scene plays; the second is launched, and cancelled
        // before its line.
        rig.sender.session(SessionCommand::LaunchScene(scenes[0]));
        play(&mut rig, 24_001, block);
        rig.sender.session(SessionCommand::LaunchScene(scenes[1]));
        play(&mut rig, 1_000, block);
        let queued = rig.monitor.state().session.clone();
        assert!(
            queued
                .iter()
                .any(|s| s.scene == scenes[1] && s.state == SlotState::Queued),
            "blocks of {block}: {queued:?}"
        );
        assert!(
            queued
                .iter()
                .any(|s| s.scene == scenes[0] && s.state == SlotState::QueuedToStop),
            "blocks of {block}: {queued:?}"
        );
        rig.sender
            .session(SessionCommand::CancelSceneLaunch(scenes[1]));
        play(&mut rig, BAR_FRAMES as usize + 8, block);
        let states = rig.monitor.state().session.clone();
        assert_eq!(states.len(), 1, "blocks of {block}: {states:?}");
        assert_eq!(
            (states[0].scene, states[0].state),
            (scenes[0], SlotState::Playing)
        );
        let heard = notes(&mut rig);
        assert_eq!(heard.len(), 1, "blocks of {block}: {heard:?}");
        assert_eq!((heard[0].1, heard[0].2), (60, true));
    }
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
    render_session_clip(AudioData::new(ContentHash([7; 32])), frames, block, cues)
}

/// [`render_session_audio`] for a slot clip with `audio`'s settings.
fn render_session_clip(
    audio: AudioData,
    frames: u64,
    block: usize,
    cues: impl FnOnce(TrackId, SceneId) -> Vec<(u64, Cue)>,
) -> Vec<f32> {
    render_session_file(ramp(96_000), audio, frames, block, cues)
}

/// As [`render_session_clip`], with `file` in place of the ramp.
fn render_session_file(
    file: Arc<SampleData>,
    audio: AudioData,
    frames: u64,
    block: usize,
    cues: impl FnOnce(TrackId, SceneId) -> Vec<(u64, Cue)>,
) -> Vec<f32> {
    render_session_launch(file, ClipLaunch::default(), audio, frames, block, cues)
}

/// As [`render_session_file`], for a slot clip with `launch`'s settings.
fn render_session_launch(
    file: Arc<SampleData>,
    launch: ClipLaunch,
    audio: AudioData,
    frames: u64,
    block: usize,
    cues: impl FnOnce(TrackId, SceneId) -> Vec<(u64, Cue)>,
) -> Vec<f32> {
    let mut media = MediaMap::new();
    media.insert(ContentHash([7; 32]), file);
    let registry = Registry::new().with_media(Arc::new(media));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    let mut editor = Editor::new();
    let track = track(&mut editor, TrackKind::Audio, None);
    let scene = editor.document().scenes().next().unwrap().id;
    let mut clip = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        Ticks::WHOLE,
        ClipData::Audio(audio),
    );
    clip.launch = launch;
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

// Spec: warp-modes R27, remote-mapping R35. An unwarped slot clip launched
// transposed up reads its file faster than the song and is silent once the
// file runs out; the render never stalls.
#[test]
fn a_transposed_launch_of_an_unwarped_audio_clip_is_silent_past_its_file() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = render_session_launch(
            ramp(4_096),
            ClipLaunch::default(),
            AudioData::new(ContentHash([7; 32])),
            3 * 4_096,
            512,
            |track, scene| vec![(0, Cue::Session(transposed(track, scene, 12)))],
        );
        tx.send(out).unwrap();
    });
    let out = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the render stalled once the clip read past its file");
    assert!(out[1_000] != 0.0, "the clip plays before its file ends");
    // An octave up reads the 4,096-frame file through by frame 2,048.
    for (i, &s) in out.iter().enumerate().skip(2_048 + 64) {
        assert_eq!(s, 0.0, "frame {i}");
    }
}

// Spec: remote-mapping R16. A launch at velocity v plays the clip at a gain of
// 1 - a * (127 - v) / 126: silent at velocity 1 with the amount at 100 %,
// unchanged at 127 and with the amount at 0, and the gain holds until the clip
// launches again, which restarts it at the next launch's velocity.
#[test]
fn a_launch_velocity_scales_the_clip_by_its_velocity_amount_at_any_block_size() {
    let launch = |amount: f64| ClipLaunch {
        velocity_amount: amount,
        ..ClipLaunch::default()
    };
    // The unlaunched-velocity ramp gives the reference.
    let render = |amount: f64, command: &dyn Fn(TrackId, SceneId) -> SessionCommand, block| {
        render_session_launch(
            ramp(96_000),
            launch(amount),
            AudioData::new(ContentHash([7; 32])),
            2_000,
            block,
            |track, scene| vec![(0, Cue::Session(command(track, scene)))],
        )
    };
    for block in [37, 64, 512] {
        let at_velocity = |amount, velocity| {
            render(
                amount,
                &|track, scene| SessionCommand::LaunchAt {
                    track,
                    scene,
                    velocity,
                    transpose: 0,
                },
                block,
            )
        };
        let silent = at_velocity(1.0, 1);
        assert!(
            silent[..1_500].iter().all(|&s| s == 0.0),
            "blocks of {block}: velocity 1 at 100 % is silent"
        );
        for (amount, velocity) in [(1.0, 127), (0.0, 1), (0.5, 127), (0.0, 64)] {
            let out = at_velocity(amount, velocity);
            for (i, sample) in out.iter().enumerate().take(1_500) {
                assert_eq!(*sample, at(i as i64), "frame {i}, blocks of {block}");
            }
        }
        // A plain launch is velocity 127.
        let plain = render(
            1.0,
            &|track, scene| SessionCommand::Launch { track, scene },
            block,
        );
        assert_eq!(plain[..1_500], at_velocity(1.0, 127)[..1_500]);
        // Half way: 1 - 0.5 * (127 - 64) / 126 = 0.75.
        let half = at_velocity(0.5, 64);
        for (i, sample) in half.iter().enumerate().take(1_500) {
            let want = f64::from(at(i as i64)) * 0.75;
            assert!(
                (f64::from(*sample) - want).abs() < 1e-7,
                "frame {i}, blocks of {block}: {sample} vs {want}"
            );
        }
    }
}

// Spec: remote-mapping R16. A restart at another velocity plays at the new
// gain from the restart's frame, and the gain before it is the first's.
#[test]
fn a_restart_takes_the_gain_of_its_own_launch() {
    let out = render_session_launch(
        ramp(96_000),
        ClipLaunch {
            velocity_amount: 1.0,
            ..ClipLaunch::default()
        },
        AudioData::new(ContentHash([7; 32])),
        194_000,
        512,
        |track, scene| {
            let launch = |velocity| {
                Cue::Session(SessionCommand::LaunchAt {
                    track,
                    scene,
                    velocity,
                    transpose: 0,
                })
            };
            vec![
                (0, Cue::Transport(TransportCommand::Play)),
                (1, launch(127)),
                (150_000, launch(64)),
            ]
        },
    );
    // The first launch lands on the 2.1.1 line (96,000) and the restart on
    // the 3.1.1 line (192,000), at 1 - 63 / 126 = half the gain.
    assert!(out[..96_000].iter().all(|&s| s == 0.0));
    for (i, sample) in out[96_000..97_500].iter().enumerate() {
        assert_eq!(*sample, at(i as i64), "frame {i} of the first launch");
    }
    for (i, sample) in out[192_000..].iter().enumerate() {
        let want = at(i as i64) * 0.5;
        assert!((sample - want).abs() < 1e-9, "frame {i} of the restart");
    }
}

/// A launch at velocity 127 that transposes by `transpose` semitones.
fn transposed(track: TrackId, scene: SceneId, transpose: i8) -> SessionCommand {
    SessionCommand::LaunchAt {
        track,
        scene,
        velocity: 127,
        transpose,
    }
}

// Spec: remote-mapping R35, R37. A MIDI clip launched with a transposition
// plays every note that many semitones up from the launch frame, at any block
// size, a note pushed past 127 or below 0 doesn't play, a relaunch at another
// transposition replaces the first on its grid line, and the clip in the set
// is unchanged.
#[test]
fn a_transposed_launch_moves_a_midi_clips_notes_and_drops_those_outside_the_keys() {
    for block in [1, 64, 513] {
        let mut rig = Rig::new(4096);
        rig.editor
            .apply(Command::SetTempo { tempo: tempo() })
            .unwrap();
        let track = midi_track(&mut rig);
        let scene = scene(&rig);
        let quarter = |key, beat: i64| Note::new(key, Ticks::QUARTER * beat, Ticks::QUARTER, 100);
        slot_clip(
            &mut rig.editor,
            track,
            scene,
            &[
                quarter(60, 0),
                quarter(64, 1),
                quarter(122, 2),
                quarter(3, 3),
            ],
        );
        let before = rig.editor.document().clone();
        rig.sync();
        rig.sender.session(transposed(track, scene, 7));
        play(&mut rig, 48_000, block);
        let ons = |rig: &mut Rig| {
            notes(rig)
                .into_iter()
                .filter(|&(_, _, on)| on)
                .map(|(frame, key, _)| (frame, key))
                .collect::<Vec<_>>()
        };
        // Beats are 24,000 frames apart: 60 and 64 move up seven, 122 would
        // be 129 and doesn't play. Offsets are the launch frame's, which is 0
        // with the transport stopped; each note-off is of the moved key.
        let mut heard = notes(&mut rig);
        heard.sort_unstable();
        assert_eq!(
            heard,
            [(0, 67, true), (24_000, 67, false), (24_000, 71, true)],
            "blocks of {block}: the notes up seven and 122 dropped"
        );
        // The first launch's last note, 3 up seven, is 10. Then down seven
        // from the next bar line, where the relaunch replaces the first
        // transposition: 3 would be -4 and doesn't play.
        rig.sender.session(transposed(track, scene, -7));
        play(&mut rig, 130_000, block);
        assert_eq!(
            ons(&mut rig),
            [(72_000, 10), (96_000, 53), (120_000, 57), (144_000, 115)],
            "blocks of {block}: the relaunch's notes down seven, 3 dropped"
        );
        assert_eq!(
            rig.editor.document(),
            &before,
            "a transposed launch changes nothing in the set"
        );
    }
}

// Spec: remote-mapping R36. A second key pressed before the first launch, or
// a relaunch, has started launches at its own transposition on the same grid
// line.
#[test]
fn a_second_key_before_the_grid_line_launches_at_its_own_transposition() {
    for block in [1, 64, 513] {
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
        rig.sender.transport(TransportCommand::Play);
        play(&mut rig, 1, block);
        rig.sender.session(transposed(track, scene, 7));
        play(&mut rig, 10_000, block);
        rig.sender.session(transposed(track, scene, 2));
        play(&mut rig, BAR_FRAMES as usize, block);
        let on = notes(&mut rig)
            .into_iter()
            .find(|&(_, _, on)| on)
            .map(|(frame, key, _)| (frame, key));
        assert_eq!(on, Some((BAR_FRAMES, 62)), "blocks of {block}");
        // So does a second key while the clip plays and its relaunch waits.
        rig.sender.session(transposed(track, scene, 5));
        play(&mut rig, 10_000, block);
        rig.sender.session(transposed(track, scene, 9));
        play(&mut rig, BAR_FRAMES as usize, block);
        let relaunched = notes(&mut rig)
            .into_iter()
            .find(|&(frame, _, on)| on && frame >= 2 * BAR_FRAMES)
            .map(|(frame, key, _)| (frame, key));
        assert_eq!(relaunched, Some((2 * BAR_FRAMES, 69)), "blocks of {block}");
    }
}

/// The settings of a warped slot clip in `mode`, with its file at the set's
/// tempo so that it plays at speed 1.
fn warped(mode: bl_model::WarpMode) -> AudioData {
    AudioData {
        warp: true,
        warp_markers: vec![bl_model::WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(120.0),
        warp_mode: mode,
        high_quality: false,
        ..AudioData::new(ContentHash([7; 32]))
    }
}

// Spec: remote-mapping R35, R37, warp-modes R10. An audio clip in Re-Pitch
// launched an octave down reads its file at half speed, as the tape of a
// transposed clip does, from the launch frame, at any block size; a
// relaunch at another transposition replaces the first on its grid line, and
// a plain launch is untouched.
#[test]
fn a_transposed_launch_plays_a_re_pitch_clip_at_the_pitch_ratio() {
    let position = |sample: f32| f64::from(sample) / f64::from(STEP) - 1.0;
    for block in [37, 512] {
        let out = render_session_clip(
            warped(bl_model::WarpMode::RePitch),
            2_000,
            block,
            |track, scene| vec![(0, Cue::Session(transposed(track, scene, -12)))],
        );
        for frame in [0, 1, 100, 1_000, 1_900] {
            let got = position(out[frame]);
            let want = frame as f64 / 2.0;
            assert!(
                (got - want).abs() < 0.05,
                "blocks of {block}, frame {frame}: {got} vs {want}"
            );
        }
        // Up an octave doubles it.
        let up = render_session_clip(
            warped(bl_model::WarpMode::RePitch),
            2_000,
            block,
            |track, scene| vec![(0, Cue::Session(transposed(track, scene, 12)))],
        );
        assert!((position(up[1_000]) - 2_000.0).abs() < 0.05);
        // A plain launch reads frame for frame.
        let plain = render_session_clip(
            warped(bl_model::WarpMode::RePitch),
            2_000,
            block,
            |track, scene| vec![(0, Cue::Session(SessionCommand::Launch { track, scene }))],
        );
        assert!((position(plain[1_000]) - 1_000.0).abs() < 0.05);
    }
    // Launched at -12, then at +12 on the next bar line (192,000).
    let out = render_session_clip(
        warped(bl_model::WarpMode::RePitch),
        194_000,
        512,
        |track, scene| {
            vec![
                (0, Cue::Transport(TransportCommand::Play)),
                (1, Cue::Session(transposed(track, scene, -12))),
                (150_000, Cue::Session(transposed(track, scene, 12))),
            ]
        },
    );
    assert!(
        (position(out[96_000 + 1_000]) - 500.0).abs() < 0.05,
        "the first launch"
    );
    assert!(
        (position(out[192_000 + 500]) - 1_000.0).abs() < 0.05,
        "the relaunch"
    );
}

// Spec: remote-mapping R35, warp-modes R27. An unwarped clip reads its file
// at the pitch ratio, and a transposition past the 48 semitones Transpose
// allows stops there.
#[test]
fn a_transposed_launch_reads_an_unwarped_clip_at_the_ratio_to_48_semitones() {
    let position = |sample: f32| f64::from(sample) / f64::from(STEP) - 1.0;
    let read = |transpose: i8| {
        let audio = AudioData {
            high_quality: false,
            ..AudioData::new(ContentHash([7; 32]))
        };
        render_session_clip(audio, 400, 64, |track, scene| {
            vec![(0, Cue::Session(transposed(track, scene, transpose)))]
        })
    };
    assert!((position(read(-12)[200]) - 100.0).abs() < 0.05);
    // 100 semitones is held to 48: sixteen times the speed.
    assert!((position(read(100)[100]) - 1_600.0).abs() < 0.5);
}

// Spec: remote-mapping R35, R37, warp-modes R26. In Complex an octave down
// halves the pitch of a steady tone.
#[test]
fn a_transposed_launch_moves_a_complex_clips_pitch() {
    // A 1 kHz sine, long enough for the clip's whole bar.
    let sine = Arc::new(
        SampleData::new(
            48_000,
            vec![
                (0..96_000)
                    .map(|i| (std::f64::consts::TAU * 1_000.0 * i as f64 / 48_000.0).sin() as f32)
                    .collect(),
            ],
        )
        .unwrap(),
    );
    // The crossings of zero between frames 20,000 and 50,000: twice the
    // frequency over the 0.625 s it spans.
    let crossings = |out: &[f32]| {
        out[20_000..50_000]
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count()
    };
    for block in [64, 512] {
        let play = |transpose: i8| {
            render_session_file(
                sine.clone(),
                warped(bl_model::WarpMode::Complex),
                60_000,
                block,
                |track, scene| vec![(0, Cue::Session(transposed(track, scene, transpose)))],
            )
        };
        let (plain, down) = (crossings(&play(0)), crossings(&play(-12)));
        assert!(
            plain.abs_diff(1_250) <= 25,
            "blocks of {block}: untransposed {plain} crossings"
        );
        assert!(
            down.abs_diff(625) <= 15,
            "blocks of {block}: an octave down {down} crossings"
        );
    }
}

// Spec: warping R15. A launched warped clip plays its Start marker's beat on
// its launch frame and runs on at the set's tempo, its loop wrapping on exact
// frames.
#[test]
fn a_launched_warped_clip_plays_its_start_beat_on_its_launch_frame() {
    use bl_model::{WarpMarker, WarpMode};
    let quarter = Ticks::QUARTER.get() as f64;
    // The file runs at 60 BPM, so at the set's 120 BPM it is read at twice
    // its speed. Its Start marker is the file's 0.25 s (a quarter of a
    // beat) and its loop brace the first beat.
    let audio = AudioData {
        warp: true,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(60.0),
        warp_mode: WarpMode::RePitch,
        high_quality: false,
        looping: true,
        start: quarter / 4.0,
        loop_start: 0.0,
        loop_end: Some(quarter),
        ..AudioData::new(ContentHash([7; 32]))
    };
    let position = |sample: f32| f64::from(sample) / f64::from(STEP) - 1.0;
    let mut reference = None;
    for block in [1, 61, 512, 4_096] {
        // Launched while the song plays: on the next bar line, frame 96,000.
        let out = render_session_clip(audio.clone(), 160_000, block, |track, scene| {
            vec![
                (0, Cue::Transport(TransportCommand::Play)),
                (
                    24_001,
                    Cue::Session(SessionCommand::Launch { track, scene }),
                ),
            ]
        });
        assert!(out[..96_000].iter().all(|&sample| sample == 0.0));
        // The Start marker's frame of the file (0.25 s) is read on the
        // launch frame, and two file frames to each song frame after it.
        assert!((position(out[96_000]) - 12_000.0).abs() < 0.02);
        assert!((position(out[96_001]) - 12_002.0).abs() < 0.02);
        // The brace's end, the file's 1 s, comes after the three quarters of
        // a beat left of it: 18,000 song frames. The loop then starts again
        // from the file's start, and wraps every beat, 24,000 frames.
        assert!((position(out[96_000 + 17_999]) - 47_998.0).abs() < 0.02);
        // Reads this close to the file's start are within reach of the
        // interpolator's filter, which finds nothing before it: half a frame.
        assert!(position(out[96_000 + 18_000]).abs() < 0.5);
        assert!((position(out[96_000 + 18_001]) - 2.0).abs() < 0.5);
        assert!(position(out[96_000 + 18_000 + 24_000]).abs() < 0.5);
        match &reference {
            None => reference = Some(out),
            Some(reference) => assert_eq!(&out, reference, "blocks of {block}"),
        }
    }
}

// Spec: warp-modes R6, R12. A launched clip whose speed changes at its warp
// markers reads through the same filter at any block size: the
// interpolator's width doesn't depend on how the blocks fall.
#[test]
fn a_launched_clips_filter_doesnt_depend_on_the_block_size() {
    use bl_model::{WarpMarker, WarpMode};
    // A high tone, which the filter's width changes, unlike a ramp.
    let tone: Vec<f32> = (0..240_000)
        .map(|i| (f64::from(i) * 0.37).sin() as f32 * 0.5)
        .collect();
    let file = Arc::new(SampleData::new(48_000, vec![tone.clone(), tone]).unwrap());
    let audio = AudioData {
        warp: true,
        // Beats of the file's first second, its next three quarters, a second
        // and a half and a second: at 120 BPM, read at 2, 1.5, 3 and 2 times
        // its speed, never at its own, where a run reads frame for frame.
        warp_markers: vec![
            WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            },
            WarpMarker {
                time: 1.0,
                beat: Ticks::QUARTER,
            },
            WarpMarker {
                time: 1.75,
                beat: Ticks::QUARTER * 2,
            },
            WarpMarker {
                time: 3.25,
                beat: Ticks::QUARTER * 3,
            },
            WarpMarker {
                time: 4.25,
                beat: Ticks::QUARTER * 4,
            },
        ],
        tempo: Some(120.0),
        warp_mode: WarpMode::RePitch,
        ..AudioData::new(ContentHash([7; 32]))
    };
    let mut reference = None;
    for block in [100, 512, 4_096] {
        // Launched on the second bar line, frame 96,000.
        let out = render_session_file(
            file.clone(),
            audio.clone(),
            168_000,
            block,
            |track, scene| {
                vec![
                    (0, Cue::Transport(TransportCommand::Play)),
                    (1, Cue::Session(SessionCommand::Launch { track, scene })),
                ]
            },
        );
        assert!(out[96_000..].iter().any(|&sample| sample != 0.0));
        match &reference {
            None => reference = Some(out),
            Some(reference) => assert!(out == *reference, "blocks of {block}"),
        }
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

/// The frames of the test instrument's note-ons since the last call.
fn ons(rig: &mut Rig) -> Vec<i64> {
    notes(rig)
        .into_iter()
        .filter(|n| n.2)
        .map(|n| n.0)
        .collect()
}

/// Puts a looping clip of `notes` in the track's slot, its brace
/// `brace` long from the clip's start.
fn looping_slot_clip(
    editor: &mut Editor,
    track: TrackId,
    scene: SceneId,
    notes: &[Note],
    brace: Ticks,
) {
    let mut midi = MidiData::new(notes.to_vec());
    midi.looping = true;
    midi.loop_end = Some(brace);
    let clip = Clip::new(editor.new_id(), Ticks::ZERO, BAR, ClipData::Midi(midi));
    editor
        .apply(Command::CreateSlotClip { track, scene, clip })
        .unwrap();
}

/// A rig at 120 BPM with a MIDI track, and the first two scenes.
fn midi_rig(block: usize) -> (Rig, TrackId, SceneId, SceneId) {
    let mut rig = Rig::new(block);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let track = midi_track(&mut rig);
    let first = scene(&rig);
    let second = rig.editor.document().scenes().nth(1).unwrap().id;
    (rig, track, first, second)
}

/// Turns the Arrangement loop on from the start for `bars` bars.
fn loop_bars(rig: &mut Rig, bars: i64) {
    rig.editor
        .apply(Command::SetLoop {
            start: Ticks::ZERO,
            length: Ticks::WHOLE * bars,
        })
        .unwrap();
    rig.editor
        .apply(Command::SetLoopEnabled { enabled: true })
        .unwrap();
    rig.sync();
}

// Spec: launch-quantisation R9. T296 finding 1.
#[test]
fn a_launched_looping_midi_clip_goes_on_playing_after_a_hundred_and_twenty_eight_laps() {
    let (mut rig, track, scene, _) = midi_rig(4096);
    looping_slot_clip(
        &mut rig.editor,
        track,
        scene,
        &[Note::new(60, Ticks::ZERO, Ticks::SIXTEENTH, 100)],
        Ticks::QUARTER,
    );
    rig.sync();
    rig.sender.session(SessionCommand::Launch { track, scene });
    // A beat is 24,000 frames: 140 laps of a one-beat brace.
    let mut ons = Vec::new();
    for _ in 0..140 {
        play(&mut rig, 24_000, 4096);
        ons.extend(self::ons(&mut rig));
    }
    assert_eq!(
        ons,
        (0..140).map(|lap| lap * 24_000).collect::<Vec<_>>(),
        "a note-on on every lap"
    );
}

// Spec: launch-quantisation R2. T296 finding 2.
#[test]
fn a_launch_and_a_stop_queued_for_the_loops_end_land_as_the_playhead_wraps() {
    for block in [64, 512, 4096] {
        let (mut rig, track, first, second) = midi_rig(block);
        loop_bars(&mut rig, 4);
        let long = [Note::new(60, Ticks::ZERO, Ticks::WHOLE * 8, 100)];
        slot_clip(&mut rig.editor, track, first, &long);
        slot_clip(
            &mut rig.editor,
            track,
            second,
            &[Note::new(62, Ticks::ZERO, Ticks::WHOLE * 8, 100)],
        );
        rig.sync();
        rig.sender.transport(TransportCommand::Play);
        // In bar 2 the first clip is asked for, and it starts on the 3.1.1
        // line.
        play(&mut rig, BAR_FRAMES as usize + 1, block);
        rig.sender.session(SessionCommand::Launch {
            track,
            scene: first,
        });
        play(&mut rig, BAR_FRAMES as usize * 2, block);
        assert_eq!(
            notes(&mut rig),
            vec![(BAR_FRAMES * 2, 60, true)],
            "blocks of {block}"
        );
        // In bar 4 the second is asked for: the line it waits for is the
        // loop's end, and it takes over as the playhead wraps.
        play(&mut rig, BAR_FRAMES as usize - 1_000, block);
        rig.sender.session(SessionCommand::Launch {
            track,
            scene: second,
        });
        play(&mut rig, 1_000 + 4_096, block);
        assert_eq!(
            notes(&mut rig),
            vec![(0, 60, false), (0, 62, true)],
            "blocks of {block}: the launch lands as the loop wraps"
        );
        let states = rig.monitor.state().session.clone();
        assert_eq!(states.len(), 1, "{states:?}");
        assert_eq!(states[0].scene, second);
        assert_eq!(states[0].state, SlotState::Playing);
        // A stop asked for in bar 4 does the same.
        play(&mut rig, BAR_FRAMES as usize * 4 - 4_096 - 1_000, block);
        rig.sender.session(SessionCommand::StopTrack(track));
        play(&mut rig, 1_000 + 4_096, block);
        assert_eq!(
            notes(&mut rig),
            vec![(0, 62, false)],
            "blocks of {block}: the stop lands as the loop wraps"
        );
        assert!(rig.monitor.state().session.is_empty());
    }
}

// Spec: transport R15. T296 finding 3: this held on main already.
#[test]
fn a_tempo_change_keeps_a_queued_launch_and_a_playing_clip_on_their_beats() {
    let (mut rig, track, first, second) = midi_rig(256);
    let beats: Vec<Note> = (0..16)
        .map(|beat| Note::new(60, Ticks::QUARTER * beat, Ticks::SIXTEENTH, 100))
        .collect();
    slot_clip(&mut rig.editor, track, first, &beats);
    slot_clip(&mut rig.editor, track, second, &beats);
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    play(&mut rig, 24_001, 256);
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: first,
    });
    // Half a bar before its line, the tempo halves: 60 BPM, a beat is 48,000
    // frames, and the line is still the 2.1.1 beat, now at 192,000.
    play(&mut rig, 24_000, 256);
    rig.apply(Command::SetTempo {
        tempo: TempoMap::constant(Bpm::new(60.0).unwrap()),
    });
    play(&mut rig, 192_000 + 48_000 * 2 - 48_001, 256);
    assert_eq!(
        ons(&mut rig)[..3],
        [192_000, 240_000, 288_000],
        "the clip starts on the bar line and plays on beat by beat"
    );
    // The clip, playing, keeps its beats through a second change.
    rig.apply(Command::SetTempo {
        tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
    });
    play(&mut rig, 100_000, 256);
    let frames = ons(&mut rig);
    assert!(
        frames.windows(2).all(|w| w[1] - w[0] == 24_000) && frames.len() >= 3,
        "{frames:?}"
    );
}

// Spec: launch-quantisation R11. T296 finding 10: through `session.launch`
// the later launch wins already.
#[test]
fn launching_the_playing_slot_again_drops_the_launch_queued_after_it() {
    let (mut rig, track, first, second) = midi_rig(256);
    let long = [Note::new(60, Ticks::ZERO, Ticks::WHOLE * 8, 100)];
    slot_clip(&mut rig.editor, track, first, &long);
    slot_clip(
        &mut rig.editor,
        track,
        second,
        &[Note::new(62, Ticks::ZERO, Ticks::WHOLE * 8, 100)],
    );
    rig.sync();
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: first,
    });
    play(&mut rig, 24_001, 256);
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: second,
    });
    play(&mut rig, 6_000, 256);
    rig.sender.session(SessionCommand::Launch {
        track,
        scene: first,
    });
    play(&mut rig, BAR_FRAMES as usize + 1_000, 256);
    let on: Vec<(u8, bool)> = notes(&mut rig).into_iter().map(|n| (n.1, n.2)).collect();
    assert!(!on.contains(&(62, true)), "the second never plays: {on:?}");
    let states = rig.monitor.state().session.clone();
    assert_eq!(states.len(), 1, "{states:?}");
    assert_eq!(states[0].scene, first);
}

// Spec: midi-clips R3. T296 finding 6: no sort on the audio thread on main.
#[test]
fn a_block_with_hundreds_of_note_events_touches_no_heap() {
    let (mut rig, track, scene, _) = midi_rig(4096);
    let cluster: Vec<Note> = (0..300)
        .map(|i| Note::new((i % 100) as u8 + 10, Ticks::ZERO, Ticks::SIXTEENTH, 100))
        .collect();
    slot_clip(&mut rig.editor, track, scene, &cluster);
    rig.sync();
    rig.sender.session(SessionCommand::Launch { track, scene });
    play(&mut rig, 4096, 4096);
    assert!(!notes(&mut rig).is_empty());
}

// Spec: midi-clips R3. T296 finding 9: a note-on goes out only when its
// release has room.
#[test]
fn every_note_a_clip_starts_is_let_go_when_the_song_stops() {
    let (mut rig, track, scene, _) = midi_rig(256);
    // More long notes at once than the player can keep track of.
    let cluster: Vec<Note> = (0..300)
        .map(|i| Note::new((i % 100) as u8 + 10, Ticks::ZERO, Ticks::WHOLE * 8, 100))
        .collect();
    slot_clip(&mut rig.editor, track, scene, &cluster);
    rig.sync();
    rig.sender.session(SessionCommand::Launch { track, scene });
    play(&mut rig, 1_024, 256);
    rig.sender.transport(TransportCommand::Stop);
    play(&mut rig, 512, 256);
    let all = notes(&mut rig);
    let on = all.iter().filter(|n| n.2).count();
    let off = all.len() - on;
    assert!(on > 0);
    assert_eq!(on, off, "every note-on has its note-off");
}

// Spec: launch-quantisation R10. T296 finding 7: a launched audio clip is
// silent while the playhead is before its launch frame (after a locate back),
// as a launched MIDI clip is, and plays from its start when the playhead
// reaches that frame again.
#[test]
fn a_launched_audio_clip_is_silent_while_the_playhead_is_before_its_launch_frame() {
    for block in [64, 512] {
        // Play from bar 2; the clip launches on the 3.1.1 line (clock
        // 96,000), plays 1,000 frames, and the playhead goes back to 1.1.1.
        let out = render_session_audio(290_000, block, |track, scene| {
            vec![
                (0, Cue::Transport(TransportCommand::Locate(Ticks::WHOLE))),
                (0, Cue::Transport(TransportCommand::Play)),
                (1_000, Cue::Session(SessionCommand::Launch { track, scene })),
                (
                    97_000,
                    Cue::Transport(TransportCommand::Locate(Ticks::ZERO)),
                ),
            ]
        });
        for (i, sample) in out[96_000..97_000].iter().enumerate() {
            assert_eq!(*sample, at(i as i64), "frame {i} of the launch");
        }
        assert!(
            out[97_000..289_000].iter().all(|&s| s == 0.0),
            "blocks of {block}: silent until the playhead is back at the launch frame"
        );
        for (i, sample) in out[289_000..].iter().enumerate() {
            assert_eq!(*sample, at(i as i64), "frame {i} of the second pass");
        }
    }
}
