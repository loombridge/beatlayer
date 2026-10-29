//! Session View launching's audio-thread work under RealtimeSanitizer
//! (WP-3.2): launches and stops landing on the quantisation grid while the
//! song plays, scenes, the global quantisation changing under a queued
//! launch, the published slot states, and the track players sounding a
//! launched clip, Legato starts, a clip's own grid, and nudges. RTSan stops
//! the process at the first allocation, free, lock, or blocking call in any
//! of it, and the allocation trap, installed as in every Beatlayer test
//! binary, fails the test at any heap use it sees.

bl_rt::install_trap!();

use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::{
    Collector, Compiler, EngineConfig, Registry, SessionCommand, TransportCommand, channel,
};
use bl_model::{
    Clip, ClipData, Command, Editor, FollowAction, FollowActions, FollowPatch, LaunchMode,
    LaunchQuantization, LaunchSettings, MidiData, Note, TrackKind,
};

// Without `--cfg lspk_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "lspk-rt (through bl-rt) wasn't built with --cfg lspk_rtsan; run `cargo xtask rtsan`"
);

#[test]
fn the_session_views_launching_passes_under_rtsan() {
    let config = EngineConfig::new(SampleRate::HZ_48000, 512);
    let mut collector = Collector::new();
    let mut compiler = Compiler::new(config, Registry::new(), collector.handle());
    let (mut sender, mut processor, mut monitor) = channel(&config);

    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
        })
        .unwrap();
    // Two MIDI tracks, each with a clip in its first scene's slot.
    let scene = editor
        .document()
        .scenes()
        .next()
        .expect("a new set has scenes")
        .id;
    let mut tracks = Vec::new();
    for index in 0..2u8 {
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: usize::from(index),
            })
            .unwrap();
        let mut clip = Clip::new(
            editor.new_id(),
            Ticks::ZERO,
            Ticks::WHOLE,
            ClipData::Midi(MidiData::new([Note::new(
                60 + index,
                Ticks::ZERO,
                Ticks::QUARTER,
                100,
            )])),
        );
        // One track of each mode that acts on a release (WP-3.3).
        clip.launch.mode = [LaunchMode::Gate, LaunchMode::Repeat][usize::from(index)];
        // Legato, and on one track the clip's own grid, so the takeover of
        // a playing clip's time and the clip's grid run under the sanitizer
        // too (WP-7.29).
        clip.launch.legato = true;
        clip.launch.quantization = [Some(LaunchQuantization::Eighth), None][usize::from(index)];
        // Follow actions that fire every sixteenth, so their arming, draws
        // and launches run under the sanitizer too (WP-3.10).
        clip.launch.follow = FollowActions {
            enabled: true,
            action_a: FollowAction::Any,
            action_b: FollowAction::PlayAgain,
            chance_a: 50,
            linked: false,
            time: Ticks::SIXTEENTH,
            ..FollowActions::default()
        };
        editor
            .apply(Command::CreateSlotClip { track, scene, clip })
            .unwrap();
        tracks.push(track);
    }
    editor
        .apply(Command::SetSceneFollowActions {
            scene,
            patch: FollowPatch {
                enabled: Some(true),
                action_a: Some(FollowAction::PlayAgain),
                ..FollowPatch::default()
            },
        })
        .unwrap();
    sender.send_update(compiler.sync(editor.document()));
    sender.transport(TransportCommand::Play);

    let mut slots_playing = 0u64;
    for round in 0..1_000u64 {
        let track = tracks[(round as usize) % tracks.len()];
        let command = match round % 20 {
            0 | 10 => Some(SessionCommand::Launch { track, scene }),
            2 | 12 => Some(SessionCommand::Release { track, scene }),
            5 | 15 => Some(SessionCommand::StopTrack(track)),
            7 => Some(SessionCommand::LaunchScene(scene)),
            17 => Some(SessionCommand::StopAll),
            8 => Some(SessionCommand::TrackBackToArrangement(track)),
            // Nudges forward and back, which wrap a looping clip (WP-7.29).
            6 | 16 => Some(SessionCommand::Nudge {
                track,
                scene,
                steps: if round % 20 == 6 { 1 } else { -3 },
            }),
            19 => Some(SessionCommand::BackToArrangement),
            1 => Some(SessionCommand::SetQuantization(LaunchSettings {
                quantization: LaunchQuantization::None,
                enabled: true,
                ..LaunchSettings::default()
            })),
            3 => Some(SessionCommand::SetQuantization(LaunchSettings {
                quantization: LaunchQuantization::SixteenthTriplet,
                enabled: true,
                ..LaunchSettings::default()
            })),
            4 => Some(SessionCommand::SetQuantization(LaunchSettings::default())),
            13 => Some(SessionCommand::SetQuantization(LaunchSettings {
                enabled: false,
                ..LaunchSettings::default()
            })),
            _ => None,
        };
        if let Some(command) = command {
            sender.session(command);
        }
        assert!(sender.flush());

        bl_rt::rt_test!({
            processor.process(512);
        });
        if !monitor.state().session.is_empty() {
            slots_playing += 1;
        }
        if round % 50 == 0 {
            collector.collect();
        }
    }

    let state = monitor.state().clone();
    assert!(
        slots_playing > 100,
        "{slots_playing} blocks with a slot queued, playing, or stopping"
    );
    assert!(
        state.session.len() <= config.max_slots,
        "the published slots fit the engine's room: {}",
        state.session.len()
    );
    eprintln!(
        "1,000 blocks of the Session View under RealtimeSanitizer: {slots_playing} blocks with \
         slots launching or stopping, {} slots published at the end",
        state.session.len()
    );
}
