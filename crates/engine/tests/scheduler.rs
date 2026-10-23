//! The multicore scheduler (WP-5.37; the architecture, "Scheduling and
//! multicore"; the racks spec, R15): a pool of real-time workers runs a
//! program's independent tasks, and the chains of a rack, beside the audio
//! thread, and what it renders is the same to the bit as the audio thread
//! alone renders. The workers' blocks run under the allocation trap too.

use crate::common;
use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::nodes::{TEST_TONE, test_tone};
use bl_engine::testing::{
    GAIN, GAIN_PARAM, INSTRUMENT, SEMITONES, SPIN, SPIN_MICROS, THROUGH, TRANSPOSE,
};
use bl_engine::{Pool, PoolConfig, TransportCommand};
use bl_model::{Command, DeviceRole, RackKind, TrackId, TrackKind};
use common::{Rig, device, effect_in, in_chain, rack, rack_in, track};

/// A song of `tracks` audio tracks, each with a tone and a gain, in a mix
/// that joins and splits: every third track in a group, every second track
/// sending to one of two returns, every fourth carrying an audio effect rack
/// of three chains, one of which holds another rack. Every track's tone is
/// at a frequency of its own, so a sum that came in another order would
/// round differently.
fn wide_song(rig: &mut Rig, tracks: usize) -> Vec<TrackId> {
    let returns = [
        track(&mut rig.editor, TrackKind::Return, None),
        track(&mut rig.editor, TrackKind::Return, None),
    ];
    for (n, ret) in returns.iter().enumerate() {
        device(
            &mut rig.editor,
            *ret,
            GAIN,
            &[(GAIN_PARAM, 0.6 + 0.1 * n as f64)],
        );
    }
    let group = track(&mut rig.editor, TrackKind::Group, None);
    let mut made = Vec::new();
    for n in 0..tracks {
        let parent = (n % 3 == 0).then_some(group);
        let id = track(&mut rig.editor, TrackKind::Audio, parent);
        device(
            &mut rig.editor,
            id,
            TEST_TONE,
            &[
                (test_tone::FREQUENCY, 110.0 + 37.3 * n as f64),
                (test_tone::LEVEL, -24.0),
            ],
        );
        device(
            &mut rig.editor,
            id,
            GAIN,
            &[(GAIN_PARAM, 0.5 + 0.013 * n as f64)],
        );
        if n % 2 == 0 {
            rig.editor
                .apply(Command::SetSend {
                    track: id,
                    to: returns[(n / 2) % 2],
                    level: 0.3 + 0.01 * n as f64,
                })
                .unwrap();
        }
        if n % 4 == 1 {
            let (_, chains) = rack(&mut rig.editor, id, RackKind::AudioEffect, 3);
            effect_in(&mut rig.editor, chains[0], GAIN, &[(GAIN_PARAM, 0.5)]);
            effect_in(&mut rig.editor, chains[1], THROUGH, &[]);
            let (_, inner) = rack_in(&mut rig.editor, chains[2], RackKind::AudioEffect, 2);
            for (chain, gain) in inner.iter().zip([0.25, 0.125]) {
                effect_in(&mut rig.editor, *chain, GAIN, &[(GAIN_PARAM, gain)]);
            }
        }
        made.push(id);
    }
    made
}

/// `total` frames of both channels, as the bits of their samples.
fn render(rig: &mut Rig, total: usize, size: usize) -> Vec<u32> {
    rig.sender.transport(TransportCommand::Play);
    let mut out = Vec::with_capacity(total * 2);
    let mut done = 0;
    while done < total {
        let n = size.min(total - done);
        rig.block(n);
        let output = rig.processor.output();
        for frame in 0..n {
            out.push(output.channel(0)[frame].to_bits());
            out.push(output.channel(1)[frame].to_bits());
        }
        done += n;
    }
    out
}

// Spec: racks R15.
#[test]
fn a_wide_song_renders_the_same_to_the_bit_with_and_without_workers() {
    for size in [1, 100, 512, 4096] {
        // Block size 1 is a block per frame: a shorter song keeps it quick.
        let total = if size == 1 { 600 } else { 9_000 };
        let mut alone = Rig::with_workers(4096, 0);
        wide_song(&mut alone, 24);
        alone.sync();
        let expected = render(&mut alone, total, size);
        assert!(expected.iter().any(|&bits| bits != 0), "the song is silent");

        for workers in [1, 3, 7] {
            let mut rig = Rig::with_workers(4096, workers);
            wide_song(&mut rig, 24);
            rig.sync();
            let got = render(&mut rig, total, size);
            assert_eq!(
                got, expected,
                "{workers} workers, blocks of {size}, differ from the audio thread alone"
            );
            let pool = rig.pool.as_ref().unwrap();
            assert_eq!(pool.heap_operations(), 0, "the workers touched the heap");
            assert_eq!(pool.panics(), 0);
        }
    }
}

/// A track with a rack of `chains` chains that each hold a [`SPIN`] node
/// that keeps a thread busy for 400 µs a block, so several threads are
/// needed to run them together.
fn spinning_rack(rig: &mut Rig, chains: usize) -> TrackId {
    let id = track(&mut rig.editor, TrackKind::Audio, None);
    device(
        &mut rig.editor,
        id,
        TEST_TONE,
        &[(test_tone::FREQUENCY, 440.0), (test_tone::LEVEL, -12.0)],
    );
    let (_, made) = rack(&mut rig.editor, id, RackKind::AudioEffect, chains);
    for (n, chain) in made.iter().enumerate() {
        in_chain(
            &mut rig.editor,
            *chain,
            SPIN,
            DeviceRole::AudioEffect,
            &[(SPIN_MICROS, 400.0)],
        );
        effect_in(
            &mut rig.editor,
            *chain,
            GAIN,
            &[(GAIN_PARAM, 0.1 * (n + 1) as f64)],
        );
    }
    id
}

// Spec: racks R15.
#[test]
fn a_racks_chains_run_on_several_workers_and_sum_to_the_same_bits() {
    let mut alone = Rig::with_workers(256, 0);
    spinning_rack(&mut alone, 6);
    alone.sync();
    let expected = render(&mut alone, 256 * 40, 256);
    assert!(expected.iter().any(|&bits| bits != 0));
    assert_eq!(alone.probe.threads_spun_on(), 1);

    let mut rig = Rig::with_workers(256, 3);
    spinning_rack(&mut rig, 6);
    rig.sync();
    let got = render(&mut rig, 256 * 40, 256);
    assert_eq!(got, expected);
    let threads = rig.probe.threads_spun_on();
    assert!(
        threads > 1,
        "the six chains ran on {threads} thread: they were not shared"
    );
    assert_eq!(rig.pool.as_ref().unwrap().heap_operations(), 0);
}

#[test]
fn independent_tracks_run_on_several_workers() {
    let mut rig = Rig::with_workers(256, 3);
    for _ in 0..8 {
        let id = track(&mut rig.editor, TrackKind::Audio, None);
        device(&mut rig.editor, id, SPIN, &[(SPIN_MICROS, 400.0)]);
    }
    rig.sync();
    render(&mut rig, 256 * 40, 256);
    let threads = rig.probe.threads_spun_on();
    assert!(threads > 1, "eight tracks ran on {threads} thread");
}

/// A MIDI track with notes, a MIDI effect rack of four chains that shift
/// them by different amounts, and the test instrument after it.
fn midi_song(rig: &mut Rig) {
    let midi = track(&mut rig.editor, TrackKind::Midi, None);
    let (_, chains) = rack(&mut rig.editor, midi, RackKind::MidiEffect, 4);
    for (chain, semitones) in chains.iter().zip([0.0, 3.0, 7.0, 12.0]) {
        in_chain(
            &mut rig.editor,
            *chain,
            TRANSPOSE,
            DeviceRole::MidiEffect,
            &[(SEMITONES, semitones)],
        );
    }
    device(&mut rig.editor, midi, INSTRUMENT, &[]);
    let beat = Ticks(TICKS_PER_QUARTER);
    let notes = (0..8).map(|n| bl_model::Note::new(60 + n, beat * i64::from(n), beat, 100));
    let clip = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateClip {
            track: midi,
            clip: bl_model::Clip::new(
                clip,
                Ticks::ZERO,
                beat * 8,
                bl_model::ClipData::Midi(bl_model::MidiData::new(notes)),
            ),
        })
        .unwrap();
    // And enough audio tracks beside it for the workers to have work.
    wide_song(rig, 8);
}

// Spec: racks R4, R15.
#[test]
fn a_midi_effect_racks_notes_merge_the_same_with_workers() {
    let notes = |workers| {
        let mut rig = Rig::with_workers(256, workers);
        midi_song(&mut rig);
        rig.sync();
        render(&mut rig, 96_000, 256);
        rig.probe
            .recorded_notes()
            .iter()
            .map(|n| (n.frame, n.event.kind, n.event.key))
            .collect::<Vec<_>>()
    };
    let expected = notes(0);
    assert!(
        expected.len() > 16,
        "the chains' notes on and off: {expected:?}"
    );
    assert_eq!(notes(3), expected);
}

#[test]
fn an_edit_during_playback_swaps_a_program_in_under_the_workers() {
    let mut rig = Rig::with_workers(256, 3);
    wide_song(&mut rig, 12);
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    let mut tracks = Vec::new();
    for round in 0..40 {
        for _ in 0..4 {
            rig.block(256);
        }
        // A structural edit, then a mixer edit: a program swap and a
        // parameter change, between blocks the workers have run.
        let id = track(&mut rig.editor, TrackKind::Audio, None);
        device(&mut rig.editor, id, GAIN, &[(GAIN_PARAM, 0.5)]);
        tracks.push(id);
        rig.sync();
        rig.apply(Command::SetVolume {
            track: tracks[round / 2],
            volume: 0.5,
        });
    }
    let pool = rig.pool.as_ref().unwrap();
    assert_eq!(pool.heap_operations(), 0);
    assert_eq!(pool.panics(), 0);
    assert!(rig.last().iter().all(|s| s.is_finite()));
}

#[test]
fn a_pool_of_none_or_a_program_of_one_task_runs_on_the_audio_thread() {
    let mut rig = Rig::with_workers(256, 2);
    let id = track(&mut rig.editor, TrackKind::Audio, None);
    device(&mut rig.editor, id, SPIN, &[(SPIN_MICROS, 10.0)]);
    rig.sync();
    rig.block(256);
    let config = *rig.processor.config();
    let pool = Pool::start(&config, &PoolConfig::plain(0)).unwrap();
    assert_eq!(pool.workers(), 0);
    rig.processor.set_pool(Some(pool));
    rig.block(256);
    assert!(rig.last().iter().all(|s| s.is_finite()));
}
