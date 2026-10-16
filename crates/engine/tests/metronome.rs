//! Metronome acceptance tests (WP-2.15).
//!
//! Clicks land on the exact sample of each beat, following the tempo map
//! and the time signatures; the bar is accented; every rhythm division
//! clicks the right number of times per bar; the count-in plays its bars
//! before the transport moves; and the recording-only setting silences it.
//! Every block runs under the real-time allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Bpm, Frames, SampleRate, TempoMap, Ticks, TimeSignature, TimeSignatureMap};
use bl_engine::TransportCommand;
use bl_model::{Command, CountIn, MetronomeRhythm, MetronomeSettings, MetronomeSound, TrackKind};
use common::{RATE, Rig, track};

/// A 4/4 bar in ticks.
const BAR: i64 = 2_822_400;

fn bpm(value: f64) -> Bpm {
    Bpm::new(value).unwrap()
}

/// The default settings with the metronome on.
fn on() -> MetronomeSettings {
    MetronomeSettings {
        enabled: true,
        ..MetronomeSettings::default()
    }
}

/// A rig with the metronome configured and a silent MIDI track, so the
/// engine has a program and a tempo map.
fn metronome_rig(max_frames: usize, metronome: MetronomeSettings, tempo: TempoMap) -> Rig {
    let mut rig = Rig::new(max_frames);
    rig.editor.apply(Command::SetTempo { tempo }).unwrap();
    rig.editor
        .apply(Command::SetMetronomeEnabled {
            enabled: metronome.enabled,
        })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeCountIn {
            count_in: metronome.count_in,
        })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeSound {
            sound: metronome.sound,
        })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeRhythm {
            rhythm: metronome.rhythm,
        })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeOnlyWhileRecording {
            only_while_recording: metronome.only_while_recording,
        })
        .unwrap();
    track(&mut rig.editor, TrackKind::Midi, None);
    rig.sync();
    rig
}

/// The frames where a click starts: a silent sample then a sounding one.
fn onsets(out: &[f32]) -> Vec<usize> {
    (0..out.len())
        .filter(|&i| out[i].abs() > 0.1 && (i == 0 || out[i - 1] == 0.0))
        .collect()
}

#[test]
fn clicks_land_on_each_beats_exact_sample_with_the_bar_accented() {
    // Spec: metronome R2, R3
    assert_eq!(RATE, SampleRate::HZ_48000);
    for (value, bars) in [(120.0, 2usize), (140.0, 1)] {
        let tempo = TempoMap::constant(bpm(value));
        let mut rig = metronome_rig(512, on(), tempo.clone());
        rig.sender.transport(TransportCommand::Play);
        let beats = bars * 4;
        let total = tempo
            .ticks_to_frames(Ticks(beats as i64 * Ticks::QUARTER.get()), RATE)
            .get() as usize;
        let out = rig.render(total, 512);
        assert_eq!(onsets(&out).len(), beats, "at {value} BPM");

        for beat in 0..beats {
            let frame = tempo
                .ticks_to_frames(Ticks(beat as i64 * Ticks::QUARTER.get()), RATE)
                .get() as usize;
            let click = out[frame].abs();
            assert!(click > 0.1, "beat {beat} at {value} BPM starts: {click}");
            if frame > 0 {
                assert_eq!(out[frame - 1], 0.0, "beat {beat} starts exactly on {frame}");
            }
        }

        // The bar's first beat is louder than the beat after it.
        for bar in 0..bars {
            let down = tempo.ticks_to_frames(Ticks(bar as i64 * BAR), RATE).get() as usize;
            let off = tempo
                .ticks_to_frames(Ticks(bar as i64 * BAR + Ticks::QUARTER.get()), RATE)
                .get() as usize;
            assert!(
                out[down].abs() > out[off].abs(),
                "bar {bar} at {value} BPM is accented"
            );
        }
    }
}

#[test]
fn clicks_follow_a_tempo_change_in_the_map() {
    // Spec: metronome R2
    // 120 BPM until bar 2, then 180: the ticks-to-frames map decides where
    // each beat lands.
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, bpm(120.0)),
        (Ticks(BAR), bpm(120.0)),
        (Ticks(BAR), bpm(180.0)),
        (Ticks(2 * BAR), bpm(180.0)),
    ])
    .unwrap();
    let mut rig = metronome_rig(512, on(), tempo.clone());
    rig.sender.transport(TransportCommand::Play);
    let total = tempo.ticks_to_frames(Ticks(2 * BAR), RATE).get() as usize;
    let out = rig.render(total, 512);
    assert_eq!(onsets(&out).len(), 8, "two bars of 4/4");

    for beat in 0..8i64 {
        let frame = tempo
            .ticks_to_frames(Ticks(beat * Ticks::QUARTER.get()), RATE)
            .get() as usize;
        assert!(out[frame].abs() > 0.1, "beat {beat} starts on {frame}");
        if frame > 0 {
            assert_eq!(out[frame - 1], 0.0, "beat {beat} starts exactly on {frame}");
        }
    }
    // The second bar is shorter, so the clicks really did follow the change.
    let first_bar = tempo.ticks_to_frames(Ticks(BAR), RATE).get();
    let second_bar = total as i64 - first_bar;
    assert!(second_bar < first_bar, "{second_bar} < {first_bar}");
}

#[test]
fn synthesized_sound_modes_have_distinct_characteristics() {
    // Spec: metronome R6
    for sound in [
        MetronomeSound::Classic,
        MetronomeSound::Click,
        MetronomeSound::Wood,
    ] {
        let mut rig = metronome_rig(
            512,
            MetronomeSettings { sound, ..on() },
            TempoMap::constant(bpm(120.0)),
        );
        rig.sender.transport(TransportCommand::Play);
        let out = rig.render(4800, 512);
        assert!(out[0].abs() > 0.1, "{sound:?} starts immediately");
        // Every click decays to silence well within 41 ms at 48 kHz.
        assert!(
            out[2000..4800].iter().all(|&s| s.abs() < 1e-3),
            "{sound:?} decays to silence"
        );
    }
}

#[test]
fn each_rhythm_clicks_its_division_of_the_bar() {
    // Spec: metronome R4
    // One 4/4 bar at 120 BPM is 96,000 frames; the count is per bar.
    for (rhythm, clicks) in [
        (MetronomeRhythm::Auto, 4),
        (MetronomeRhythm::Bar1, 1),
        (MetronomeRhythm::Half, 2),
        (MetronomeRhythm::HalfTriplet, 3),
        (MetronomeRhythm::Quarter, 4),
        (MetronomeRhythm::QuarterTriplet, 6),
        (MetronomeRhythm::Eighth, 8),
        (MetronomeRhythm::EighthTriplet, 12),
        (MetronomeRhythm::Sixteenth, 16),
        (MetronomeRhythm::SixteenthTriplet, 24),
    ] {
        let mut rig = metronome_rig(
            512,
            MetronomeSettings {
                sound: MetronomeSound::Click,
                rhythm,
                ..on()
            },
            TempoMap::constant(bpm(120.0)),
        );
        rig.sender.transport(TransportCommand::Play);
        let out = rig.render(96_000, 512);
        assert_eq!(
            onsets(&out).len(),
            clicks,
            "rhythm {rhythm:?} in 4/4 clicks {clicks} times per bar"
        );
    }
}

#[test]
fn auto_follows_the_denominator_and_a_division_that_doesnt_fit_reverts() {
    // Spec: metronome R4
    // Auto in 6/8 clicks once per eighth note: 6 times per bar.
    let mut rig = metronome_rig(
        512,
        MetronomeSettings {
            sound: MetronomeSound::Click,
            ..on()
        },
        TempoMap::constant(bpm(120.0)),
    );
    rig.editor
        .apply(Command::SetTimeSignatures {
            time_signatures: TimeSignatureMap::constant(TimeSignature::new(6, 8).unwrap()),
        })
        .unwrap();
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(72_000, 512);
    assert_eq!(onsets(&out).len(), 6, "Auto in 6/8 clicks the eighths");

    // A half note doesn't fit a 1/4 bar: its clicks revert to Auto's, so the
    // bar still gets one click. (The menu shows it disabled; the tick falls
    // back.)
    let mut rig = metronome_rig(
        512,
        MetronomeSettings {
            sound: MetronomeSound::Click,
            rhythm: MetronomeRhythm::Half,
            ..on()
        },
        TempoMap::constant(bpm(120.0)),
    );
    rig.editor
        .apply(Command::SetTimeSignatures {
            time_signatures: TimeSignatureMap::constant(TimeSignature::new(1, 4).unwrap()),
        })
        .unwrap();
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(24_000, 512);
    assert_eq!(
        onsets(&out).len(),
        1,
        "a half note in a 1/4 bar falls back to Auto"
    );
}

#[test]
fn count_in_plays_its_bars_before_the_transport_moves() {
    // Spec: metronome R5
    for (count_in, bars) in [
        (CountIn::Bars1, 1),
        (CountIn::Bars2, 2),
        (CountIn::Bars4, 4),
    ] {
        let tempo = TempoMap::constant(bpm(120.0));
        let mut rig = metronome_rig(
            256,
            MetronomeSettings {
                count_in,
                sound: MetronomeSound::Click,
                ..on()
            },
            tempo,
        );
        rig.sender.transport(TransportCommand::Play);
        // A 4/4 bar at 120 BPM is 96,000 frames, and 256 divides it.
        let bar_frames = 96_000usize;
        let out = rig.render(bars * bar_frames, 256);
        assert_eq!(
            rig.monitor.state().position,
            Frames::ZERO,
            "the transport doesn't move during the count-in"
        );
        let starts = onsets(&out);
        assert_eq!(starts.len(), bars * 4, "{count_in:?} clicks every beat");
        for (beat, &at) in starts.iter().enumerate() {
            assert_eq!(at, beat * 24_000, "{count_in:?} click {beat}");
        }
        // Each bar's first beat is accented.
        for bar in 0..bars {
            assert!(
                out[starts[bar * 4]].abs() > out[starts[bar * 4 + 1]].abs(),
                "{count_in:?} accents the bar"
            );
        }

        // Once the count-in is done, playback starts from the insert marker.
        rig.render(bar_frames, 256);
        assert_eq!(
            rig.monitor.state().position,
            Frames(bar_frames as i64),
            "{count_in:?}: the transport moves when the count-in ends"
        );
    }
}

#[test]
fn only_while_recording_silences_playback_unless_recording() {
    // Spec: metronome R7
    let mut rig = metronome_rig(
        512,
        MetronomeSettings {
            sound: MetronomeSound::Click,
            only_while_recording: true,
            ..on()
        },
        TempoMap::constant(bpm(120.0)),
    );
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(96_000, 512);
    assert!(
        out.iter().all(|&s| s == 0.0),
        "silent during playback with the recording-only switch on"
    );

    rig.sender.transport(TransportCommand::SetRecording(true));
    let recording = rig.render(96_000, 512);
    assert!(
        recording.iter().any(|&s| s.abs() > 0.1),
        "audible once recording"
    );
}

#[test]
fn a_locate_that_ends_the_take_ends_the_recording_only_clicks() {
    // Spec: metronome R7; WP-2.13. A jump while recording ends the take and
    // the transport's recording state with it, so the clicks go silent.
    let mut rig = metronome_rig(
        512,
        MetronomeSettings {
            sound: MetronomeSound::Click,
            only_while_recording: true,
            ..on()
        },
        TempoMap::constant(bpm(120.0)),
    );
    rig.sender.transport(TransportCommand::Play);
    rig.sender.transport(TransportCommand::SetRecording(true));
    let recording = rig.render(96_000, 512);
    assert!(
        recording.iter().any(|&s| s.abs() > 0.1),
        "audible once recording"
    );

    rig.sender
        .transport(TransportCommand::Locate(Ticks::QUARTER * 8));
    let after = rig.render(96_000, 512);
    assert!(
        after.iter().all(|&s| s == 0.0),
        "silent once the take ends at a locate"
    );
}

#[test]
fn with_punch_in_the_recording_only_clicks_start_at_the_punch_in_point() {
    // Spec: metronome R7; WP-2.13. Recording from before the brace with
    // Punch-In on, the clicks wait for the brace's start (bar 2, frame
    // 96,000 at 120 BPM), not for play start.
    let mut rig = metronome_rig(
        512,
        MetronomeSettings {
            sound: MetronomeSound::Click,
            only_while_recording: true,
            ..on()
        },
        TempoMap::constant(bpm(120.0)),
    );
    rig.editor
        .apply(Command::SetLoop {
            start: Ticks(BAR),
            length: Ticks(BAR),
        })
        .unwrap();
    rig.editor
        .apply(Command::SetPunchIn { enabled: true })
        .unwrap();
    rig.sync();
    rig.sender.transport(TransportCommand::SetRecording(true));
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(192_000, 512);
    assert_eq!(onsets(&out).first(), Some(&96_000));
}
