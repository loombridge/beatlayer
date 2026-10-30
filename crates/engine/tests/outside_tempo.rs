//! Following an outside tempo (WP-7.30; the transport spec, R24 to R29): a
//! per-block song speed that everything placed in song time honours, starts
//! a follower decides, and Phase Nudge. A stand-in follower plays the part
//! of an incoming clock or a Link session. Every block runs as a real-time
//! section under the allocation trap.

use crate::common;
use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::{TransportCommand, testing};
use bl_model::{
    Clip, ClipData, Command, CountIn, MetronomeRhythm, MetronomeSound, MidiData, Note, TrackKind,
};
use common::{Rig, device, track};

const BEAT: Ticks = Ticks(TICKS_PER_QUARTER);
const BAR: Ticks = Ticks(TICKS_PER_QUARTER * 4);

/// The block sizes every test runs at.
const BLOCKS: [usize; 4] = [1, 61, 512, 4096];

fn tempo(bpm: f64) -> TempoMap {
    TempoMap::constant(Bpm::new(bpm).unwrap())
}

/// A rig under `map`, with a track whose notes sound as pulses and whose
/// clip holds `notes` (key, start in sixteenths, length in sixteenths), and
/// the metronome on when `click` is.
fn rig(max_frames: usize, map: TempoMap, notes: &[(u8, i64, i64)], click: bool) -> Rig {
    let mut rig = Rig::new(max_frames);
    rig.editor.apply(Command::SetTempo { tempo: map }).unwrap();
    rig.editor
        .apply(Command::SetMetronomeEnabled { enabled: click })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeCountIn {
            count_in: CountIn::None,
        })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeSound {
            sound: MetronomeSound::Classic,
        })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeRhythm {
            rhythm: MetronomeRhythm::Quarter,
        })
        .unwrap();
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, a, testing::NOTE_PULSE, &[]);
    let notes: Vec<Note> = notes
        .iter()
        .map(|&(key, start, length)| {
            Note::new(
                key,
                Ticks::SIXTEENTH * start,
                Ticks::SIXTEENTH * length,
                100,
            )
        })
        .collect();
    if !notes.is_empty() {
        let id = rig.editor.new_id();
        rig.editor
            .apply(Command::CreateClip {
                track: a,
                clip: Clip::new(
                    id,
                    Ticks::ZERO,
                    BAR * 4,
                    ClipData::Midi(MidiData::new(notes)),
                ),
            })
            .unwrap();
    }
    rig.sync();
    rig
}

const NOTES: [(u8, i64, i64); 6] = [
    (60, 0, 2),
    (62, 4, 4),
    (64, 6, 1),
    (65, 8, 3),
    (67, 16, 4),
    (69, 22, 2),
];

#[test]
fn a_followed_tempo_puts_notes_and_clicks_on_the_frames_that_tempo_gives_them() {
    // Spec: transport R24. A set at 120 BPM following 140 plays what the
    // same set at 140 BPM plays, at any block size.
    for block in BLOCKS {
        let mut reference = rig(4096, tempo(140.0), &NOTES, true);
        reference.sender.transport(TransportCommand::Play);
        let want = reference.render(110_000, block);

        let mut rig = rig(4096, tempo(120.0), &NOTES, true);
        rig.follow(testing::following(140.0));
        rig.sender.transport(TransportCommand::Play);
        let got = rig.render(110_000, block);
        assert!(want.iter().any(|&s| s != 0.0), "the reference sounds");
        same(&got, &want, &format!("at blocks of {block}"));

        // The map's own tempo isn't what played.
        let mut unfollowed = self::rig(4096, tempo(120.0), &NOTES, true);
        unfollowed.sender.transport(TransportCommand::Play);
        assert_ne!(unfollowed.render(110_000, block), want);
    }
}

/// Checks that two renders are the same, naming the first frame that isn't.
fn same(got: &[f32], want: &[f32], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: lengths");
    if let Some(at) = (0..got.len()).find(|&i| got[i] != want[i]) {
        panic!(
            "{what}: frame {at} is {} where {} is wanted; the frames that sound: {:?} against {:?}",
            got[at],
            want[at],
            &nonzero(got)[..nonzero(got).len().min(12)],
            &nonzero(want)[..nonzero(want).len().min(12)],
        );
    }
}

/// The frames where the output isn't zero.
fn nonzero(out: &[f32]) -> Vec<usize> {
    (0..out.len()).filter(|&i| out[i] != 0.0).collect()
}

/// Whether two lists of frames hold the same number, each within `slack`.
fn near(got: &[usize], want: &[usize], slack: usize) {
    assert_eq!(got.len(), want.len(), "{got:?} against {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!(g.abs_diff(*w) <= slack, "{got:?} against {want:?}");
    }
}

/// Renders `total` frames of a rig playing from the start.
fn played(mut rig: Rig, total: usize, block: usize) -> Vec<f32> {
    rig.sender.transport(TransportCommand::Play);
    rig.render(total, block)
}

#[test]
fn the_loops_wrap_lands_on_the_frame_the_followed_tempo_gives_its_end() {
    // Spec: transport R13, R24. A one-bar loop round the notes.
    let looped = |mut rig: Rig| {
        rig.sender
            .transport(TransportCommand::SetLoop(Some(bl_engine::LoopRegion {
                start: Ticks::ZERO,
                end: BAR,
            })));
        rig
    };
    for block in BLOCKS {
        let want = played(
            looped(rig(4096, tempo(140.0), &NOTES, false)),
            200_000,
            block,
        );
        let mut following = looped(rig(4096, tempo(120.0), &NOTES, false));
        following.follow(testing::following(140.0));
        let got = played(following, 200_000, block);
        assert!(nonzero(&want).len() > 12, "the reference loops");
        same(&got, &want, &format!("at blocks of {block}"));
    }
}

#[test]
fn a_scheduled_command_fires_on_the_frame_the_followed_tempo_gives_its_beat() {
    // Spec: transport R24.
    use bl_engine::{Action, ParamValue, Report, ScheduledCommand};
    // Bar 3, beat 2 and a triplet eighth.
    let at = BAR * 2 + BEAT + Ticks(TICKS_PER_QUARTER / 3);
    let build = |map: TempoMap, block: usize| {
        let mut rig = Rig::new(4096);
        rig.editor.apply(Command::SetTempo { tempo: map }).unwrap();
        let a = track(&mut rig.editor, TrackKind::Midi, None);
        let level = common::dc(&mut rig.editor, a, 1.0);
        rig.sync();
        rig.sender.schedule(ScheduledCommand {
            id: 7,
            at,
            action: Action::Param {
                key: common::key(level),
                param: testing::VALUE,
                value: ParamValue::Set(0.5),
            },
        });
        (rig, block)
    };
    for block in BLOCKS {
        let (rig, _) = build(tempo(140.0), block);
        let mut want_rig = rig;
        want_rig.sender.transport(TransportCommand::Play);
        let want = want_rig.render(260_000, block);
        let fired = want.iter().position(|&s| s == 0.5).unwrap();

        let (mut rig, _) = build(tempo(120.0), block);
        rig.follow(testing::following(140.0));
        rig.sender.transport(TransportCommand::Play);
        let got = rig.render(260_000, block);
        same(&got, &want, &format!("at blocks of {block}"));
        let reports = rig.monitor.reports();
        assert!(
            matches!(reports.as_slice(), [Report::Fired { id: 7, clock, .. }] if *clock == fired as u64),
            "{reports:?}"
        );
    }
}

#[test]
fn an_envelope_reaches_its_target_on_the_frame_the_followed_tempo_gives_it() {
    // Spec: automation R2; transport R24.
    use bl_model::{AutomationTarget, MixerParam};
    let build = |map: TempoMap| {
        let mut rig = Rig::new(4096);
        rig.editor.apply(Command::SetTempo { tempo: map }).unwrap();
        let a = track(&mut rig.editor, TrackKind::Midi, None);
        common::dc(&mut rig.editor, a, 1.0);
        let volume = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        rig.apply(Command::SetVolume {
            track: a,
            volume: 0.0,
        });
        for (at, value) in [(Ticks::ZERO, 0.0), (BEAT, 1.0)] {
            rig.apply(Command::SetAutomationBreakpoint {
                track: a,
                target: volume,
                at,
                value,
            });
        }
        rig
    };
    for block in BLOCKS {
        let want = played(build(tempo(140.0)), 30_000, block);
        let mut following = build(tempo(120.0));
        following.follow(testing::following(140.0));
        let got = played(following, 30_000, block);
        let reached = |out: &[f32]| out.iter().position(|&s| s == 1.0);
        assert_eq!(reached(&got), reached(&want), "at blocks of {block}");
        // The value in between is the line, within half a frame of the
        // map's 24,000.
        for (frame, (g, w)) in got.iter().zip(&want).enumerate() {
            assert!((g - w).abs() < 4.2e-5, "frame {frame}: {g} against {w}");
        }
    }
}

#[test]
fn a_launch_lands_on_the_quantisation_line_the_followed_tempo_gives() {
    // Spec: launch-quantisation R2; transport R24. A clip launched on the next
    // bar line, asked for the same frame of the clock.
    use bl_engine::SessionCommand;
    use bl_model::{LaunchQuantization, LaunchSettings};
    let build = |map: TempoMap| {
        let mut rig = Rig::new(4096);
        rig.editor.apply(Command::SetTempo { tempo: map }).unwrap();
        let a = track(&mut rig.editor, TrackKind::Midi, None);
        device(&mut rig.editor, a, testing::NOTE_PULSE, &[]);
        let scene = rig.editor.document().scenes().next().unwrap().id;
        let id = rig.editor.new_id();
        rig.editor
            .apply(Command::CreateSlotClip {
                track: a,
                scene,
                clip: Clip::new(
                    id,
                    Ticks::ZERO,
                    BAR,
                    ClipData::Midi(MidiData::new([
                        Note::new(60, Ticks::ZERO, BEAT, 100),
                        Note::new(64, BEAT, BEAT, 100),
                    ])),
                ),
            })
            .unwrap();
        rig.sync();
        rig.sender
            .session(SessionCommand::SetQuantization(LaunchSettings {
                quantization: LaunchQuantization::Bar,
                enabled: true,
                ..LaunchSettings::default()
            }));
        (rig, a, scene)
    };
    for block in BLOCKS {
        let render = |mut rig: Rig, track, scene| {
            rig.sender.transport(TransportCommand::Play);
            let mut out = rig.render(10_000, block);
            rig.sender.session(SessionCommand::Launch { track, scene });
            out.extend(rig.render(190_000, block));
            out
        };
        let (rig, a, scene) = build(tempo(140.0));
        let want = render(rig, a, scene);
        let (mut rig, a, scene) = build(tempo(120.0));
        rig.follow(testing::following(140.0));
        let got = render(rig, a, scene);
        assert_eq!(nonzero(&want).len(), 3, "the reference plays the clip");
        same(&got, &want, &format!("at blocks of {block}"));
    }
}

#[test]
fn a_ramp_in_the_map_does_not_bend_the_followed_ticks() {
    // Spec: transport R24. A map that ramps from 120 to 140 BPM, followed at a
    // steady 130, plays what a steady 130 map does.
    let ramp = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(120.0).unwrap()),
        (BAR * 4, Bpm::new(140.0).unwrap()),
    ])
    .unwrap();
    for block in BLOCKS {
        let want = played(rig(4096, tempo(130.0), &NOTES, false), 110_000, block);
        let mut following = rig(4096, ramp.clone(), &NOTES, false);
        following.follow(testing::following(130.0));
        let got = played(following, 110_000, block);
        // A note's frame is the ramp's rounded frame, which can sit a frame
        // from the steady map's.
        near(&nonzero(&got), &nonzero(&want), 1);
    }
}

#[test]
fn a_tempo_that_changes_between_blocks_moves_the_song_as_a_tempo_change_does() {
    // Spec: transport R24. 120 BPM until frame 249,856 (a multiple of every
    // block size), then 150: the song a map with that change plays.
    let change = 249_856u64;
    let at = Ticks(change as i64 * 2 * TICKS_PER_QUARTER / 48_000);
    let map = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(120.0).unwrap()),
        (at, Bpm::new(120.0).unwrap()),
        (at, Bpm::new(150.0).unwrap()),
    ])
    .unwrap();
    let notes: Vec<(u8, i64, i64)> = (0..40).map(|i| (60 + (i % 12) as u8, i * 4, 1)).collect();
    for block in BLOCKS {
        let want = played(rig(4096, map.clone(), &notes, false), 400_000, block);
        let mut following = rig(4096, tempo(110.0), &notes, false);
        // The map's own tempo isn't played, so any base will do.
        following.follow(testing::stand_in(move |ask, answer| {
            answer.tempo(if ask.clock < change { 120.0 } else { 150.0 });
        }));
        let got = played(following, 400_000, block);
        near(&nonzero(&got), &nonzero(&want), 1);
    }
}

#[test]
fn a_jump_the_follower_asks_for_is_a_locate_on_its_frame() {
    // Spec: transport R7, R24.
    let jump_to = BAR + BEAT;
    for block in BLOCKS {
        let mut reference = rig(4096, tempo(140.0), &NOTES, false);
        reference.sender.transport_at(
            TransportCommand::Locate(jump_to),
            bl_engine::When::Frame(61 * 512),
        );
        let want = played(reference, 100_000, block);

        let mut following = rig(4096, tempo(120.0), &NOTES, false);
        following.follow(testing::stand_in(move |ask, answer| {
            answer.tempo(140.0);
            if ask.clock <= 61 * 512 && 61 * 512 < ask.clock + ask.frames as u64 {
                answer.jump(61 * 512, jump_to);
            }
        }));
        let got = played(following, 100_000, block);
        // The jump's target falls on a fractional frame in the map that
        // plays, so a note after it can sit a frame from the reference's.
        near(&nonzero(&got), &nonzero(&want), 1);
    }
}

#[test]
fn the_slowest_and_fastest_tempos_hold_over_the_fastest_and_slowest_maps() {
    // Spec: transport R24. 20 BPM followed over a 999 BPM map, and 999 over 20.
    for (map, followed) in [(999.0, 20.0), (20.0, 999.0)] {
        for block in BLOCKS {
            let total = if followed < 100.0 { 290_000 } else { 30_000 };
            let want = played(rig(4096, tempo(followed), &NOTES, false), total, block);
            let mut following = rig(4096, tempo(map), &NOTES, false);
            following.follow(testing::following(followed));
            let got = played(following, total, block);
            assert!(!nonzero(&want).is_empty());
            // An event sits on a whole frame of the map that plays, which is
            // `map / followed` frames wide where the map is faster: half of
            // that is how far from the followed tempo's frame it can land.
            let slack = (0.5 * followed.max(map) / followed.min(map)) as usize / 2 + 2;
            let slack = if map > followed { slack * 2 } else { 1 };
            near(&nonzero(&got), &nonzero(&want), slack);
        }
    }
}

#[test]
fn when_the_follower_goes_the_next_block_plays_the_maps_tempo_on_the_same_beat() {
    // Spec: transport R26.
    // A multiple of the block sizes it is run at, so every block is asked the
    // same.
    let gone = 4 * 4096u64;
    for block in [1, 512, 4096] {
        let mut rig = rig(4096, tempo(120.0), &NOTES, false);
        rig.follow(testing::stand_in(move |ask, answer| {
            if ask.clock < gone {
                answer.tempo(140.0);
            }
        }));
        rig.sender.transport(TransportCommand::Play);
        let out = rig.render(60_000, block);
        // Where the song was at the frame it was let go, in ticks.
        let state_ticks = {
            let mut probe = self::rig(4096, tempo(120.0), &NOTES, false);
            probe.follow(testing::following(140.0));
            probe.sender.transport(TransportCommand::Play);
            probe.render(gone as usize, 4096);
            probe.monitor.state().position
        };
        let beat = tempo(120.0).frames_to_ticks(state_ticks, common::RATE);
        // After that, notes come at the map's tempo from that beat on.
        let later: Vec<usize> = nonzero(&out)
            .into_iter()
            .filter(|&f| f as u64 >= gone)
            .collect();
        assert!(!later.is_empty(), "the song goes on");
        let mut want = Vec::new();
        for &(_, start, length) in &NOTES {
            for edge in [start, start + length] {
                let ticks = Ticks::SIXTEENTH * edge;
                if ticks >= beat {
                    let frames =
                        tempo(120.0).ticks_to_frames(ticks, common::RATE).get() - state_ticks.get();
                    want.push((gone as i64 + frames) as usize);
                }
            }
        }
        want.retain(|&f| f < out.len());
        want.sort_unstable();
        want.dedup();
        near(&later, &want, 1);
    }
}

/// A rig with one track whose output is the song position of each frame, or
/// −1 while stopped.
fn position_rig(max_frames: usize) -> Rig {
    let mut rig = Rig::new(max_frames);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, a, testing::POSITION, &[]);
    rig.sync();
    rig
}

fn stopped_through(out: &[f32], range: std::ops::Range<usize>) {
    assert!(
        out[range.clone()].iter().all(|&s| s == -1.0),
        "stopped through {range:?}"
    );
}

/// Checks that `out[range]` plays the song from `first` on, a frame a frame.
fn plays_on_from(out: &[f32], range: std::ops::Range<usize>, first: i64) {
    for i in range.clone() {
        assert_eq!(
            out[i],
            (first + (i - range.start) as i64) as f32,
            "frame {i} of {range:?}"
        );
    }
}

/// A follower that decides when starts happen and gives the frame a waiting
/// start begins on.
fn deciding(frame: u64) -> impl bl_engine::Follower {
    testing::stand_in(move |_, answer| {
        answer.decides_starts();
        answer.start_at(frame);
    })
}

#[test]
fn a_start_waits_for_the_frame_the_follower_gives_and_plays_from_the_marker_on_it() {
    // Spec: transport R25.
    for block in BLOCKS {
        let mut rig = position_rig(4096);
        rig.follow(deciding(20_000));
        rig.sender.transport(TransportCommand::Play);
        let mut out = rig.render(10_000, block);
        let waiting = rig.monitor.state();
        assert!(waiting.waiting_start && !waiting.playing);
        assert_eq!(waiting.waiting_frame, Some(20_000));
        out.extend(rig.render(20_000, block));
        stopped_through(&out, 0..20_000);
        plays_on_from(&out, 20_000..30_000, 0);
        let started = rig.monitor.state();
        assert!(started.playing && !started.waiting_start);
        assert_eq!(started.waiting_frame, None);
    }
}

#[test]
fn stop_cancels_a_waiting_start_and_a_second_start_keeps_the_wait() {
    // Spec: transport R25.
    for block in [1, 512] {
        let mut rig = position_rig(4096);
        rig.follow(deciding(20_000));
        rig.sender.transport(TransportCommand::Play);
        rig.render(5_000, block);
        // A second start changes nothing: the song still starts on the frame.
        rig.sender.transport(TransportCommand::Play);
        let mut out = rig.render(25_000, block);
        stopped_through(&out, 0..15_000);
        plays_on_from(&out, 15_000..25_000, 0);

        let mut rig = position_rig(4096);
        rig.follow(deciding(20_000));
        rig.sender.transport(TransportCommand::Play);
        rig.render(5_000, block);
        rig.sender.transport(TransportCommand::Stop);
        out = rig.render(25_000, block);
        stopped_through(&out, 0..25_000);
        let state = rig.monitor.state();
        assert!(!state.playing && !state.waiting_start);
    }
}

#[test]
fn the_followers_own_start_and_stop_act_on_the_frames_it_gives() {
    // Spec: transport R25.
    for block in BLOCKS {
        let mut rig = position_rig(4096);
        rig.follow(testing::stand_in(|ask, answer| {
            answer.decides_starts();
            for (frame, command) in [
                (15_000, TransportCommand::Play),
                (25_000, TransportCommand::Stop),
            ] {
                if ask.clock <= frame && frame < ask.clock + ask.frames as u64 {
                    answer.command(frame, command);
                }
            }
        }));
        let out = rig.render(30_000, block);
        stopped_through(&out, 0..15_000);
        plays_on_from(&out, 15_000..25_000, 0);
        stopped_through(&out, 25_000..30_000);
    }
}

#[test]
fn a_start_goes_ahead_at_once_when_the_follower_stops_deciding() {
    // Spec: transport R25, R26.
    let gone = 4 * 4096u64;
    for block in [1, 512, 4096] {
        let mut rig = position_rig(4096);
        rig.follow(testing::stand_in(move |ask, answer| {
            if ask.clock < gone {
                answer.decides_starts();
            }
        }));
        rig.sender.transport(TransportCommand::Play);
        let out = rig.render(20_000, block);
        stopped_through(&out, 0..gone as usize);
        plays_on_from(&out, gone as usize..20_000, 0);
    }
}

/// The song frames a rig plays in the first `total` frames with a button held
/// from the start until `release`, then released.
fn nudged(
    direction: bl_engine::NudgeDirection,
    release: u64,
    total: usize,
    block: usize,
) -> Vec<f32> {
    let mut rig = position_rig(4096);
    rig.sender.transport(TransportCommand::Play);
    rig.sender.transport(TransportCommand::PhaseNudge {
        direction,
        held: true,
    });
    rig.sender.transport_at(
        TransportCommand::PhaseNudge {
            direction,
            held: false,
        },
        bl_engine::When::Frame(release),
    );
    let out = rig.render(total, block);
    if release as usize > block {
        assert_eq!(rig.monitor.state().phase_nudge, None, "released");
    }
    out
}

#[test]
fn holding_phase_nudge_up_for_two_seconds_leaves_the_beat_four_percent_of_two_seconds_ahead() {
    // Spec: transport R27. At 120 BPM, 2 s are 4 quarter notes: 4 % of them
    // is 0.16 of a quarter note, 3,840 frames.
    use bl_engine::NudgeDirection::{Down, Up};
    for block in BLOCKS {
        let up = nudged(Up, 96_000, 100_000, block);
        // Held, the song runs 4 % fast; the frames after the release play at
        // the map's tempo again, from where the nudge moved the beat.
        assert!((up[95_999] - 99_839.0).abs() <= 1.0, "{}", up[95_999]);
        for i in 96_001..100_000 {
            assert_eq!(up[i] - up[i - 1], 1.0, "frame {i} plays at the map's tempo");
        }
        let ahead = f64::from(up[99_999]) - 99_999.0;
        assert!(
            (ahead / 24_000.0 - 0.16).abs() < 1e-4,
            "{ahead} frames ahead"
        );

        let down = nudged(Down, 96_000, 100_000, block);
        assert!((down[95_999] - 92_159.0).abs() <= 1.0, "{}", down[95_999]);
        let behind = 99_999.0 - f64::from(down[99_999]);
        assert!(
            (behind / 24_000.0 - 0.16).abs() < 1e-4,
            "{behind} frames behind"
        );
    }
}

#[test]
fn of_two_held_nudge_buttons_the_one_pressed_last_nudges() {
    // Spec: transport R27.
    use bl_engine::NudgeDirection::{Down, Up};
    let mut rig = position_rig(4096);
    rig.sender.transport(TransportCommand::Play);
    for direction in [Up, Down] {
        rig.sender.transport(TransportCommand::PhaseNudge {
            direction,
            held: true,
        });
    }
    rig.render(1_000, 100);
    assert_eq!(rig.monitor.state().phase_nudge, Some(Down));
    // Down is let go: Up, still held, nudges.
    rig.sender.transport(TransportCommand::PhaseNudge {
        direction: Down,
        held: false,
    });
    let out = rig.render(1_000, 100);
    assert_eq!(rig.monitor.state().phase_nudge, Some(Up));
    assert!(out[999] - out[0] > 1_000.0, "faster than the map");
}

#[test]
fn a_nudge_does_nothing_while_stopped_or_while_a_follower_leads() {
    // Spec: transport R27, R29.
    use bl_engine::NudgeDirection::Up;
    let press = TransportCommand::PhaseNudge {
        direction: Up,
        held: true,
    };
    let mut rig = position_rig(4096);
    rig.sender.transport(press);
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(2_000, 512);
    plays_on_from(&out, 0..2_000, 0);
    assert_eq!(rig.monitor.state().phase_nudge, None);

    let mut rig = position_rig(4096);
    rig.follow(testing::following(120.0));
    rig.sender.transport(TransportCommand::Play);
    rig.render(1_000, 100);
    assert_eq!(rig.monitor.state().following, Some(120.0));
    rig.sender.transport(press);
    rig.render(1_000, 100);
    assert_eq!(rig.monitor.state().phase_nudge, None);
}

#[test]
fn a_nudge_composes_with_the_followed_tempo() {
    // Spec: transport R24, R27. 4 % over a followed 140 BPM is 145.6 BPM.
    use bl_engine::NudgeDirection::Up;
    for block in BLOCKS {
        let mut want = position_rig(4096);
        want.sender.transport(TransportCommand::Play);
        want.follow(testing::following(145.6));
        let want = want.render(40_000, block);
        let mut rig = position_rig(4096);
        // Pressed while nothing is followed, and still held when the song
        // starts to follow.
        rig.sender.transport(TransportCommand::Play);
        rig.sender.transport(TransportCommand::PhaseNudge {
            direction: Up,
            held: true,
        });
        rig.follow(testing::following(140.0));
        let got = rig.render(40_000, block);
        same(&got, &want, &format!("at blocks of {block}"));
    }
}

/// A step between neighbouring samples of a ramp: 2⁻¹⁶, exact in `f32`.
const STEP: f32 = 1.0 / 65_536.0;

/// A file whose every sample says where it is: frame `i` is `i + 1` steps on
/// the left and minus that on the right.
fn ramp_file(frames: usize) -> std::sync::Arc<bl_media::SampleData> {
    let left: Vec<f32> = (0..frames).map(|i| (i + 1) as f32 * STEP).collect();
    let right = left.iter().map(|s| -s).collect();
    std::sync::Arc::new(bl_media::SampleData::new(48_000, vec![left, right]).unwrap())
}

/// A rig with one audio track playing the ramp as `audio` from `start`.
fn audio_rig(max_frames: usize, map: TempoMap, start: Ticks, audio: bl_model::AudioData) -> Rig {
    let hash = audio.media;
    let mut media = bl_engine::MediaMap::new();
    media.insert(hash, ramp_file(97_000));
    let mut rig = Rig::with_registry(max_frames, |registry| {
        registry.set_media(std::sync::Arc::new(media));
    });
    rig.editor.apply(Command::SetTempo { tempo: map }).unwrap();
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    let id = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateClip {
            track: a,
            clip: Clip::new(id, start, BAR * 4, ClipData::Audio(audio)),
        })
        .unwrap();
    rig.sync();
    rig
}

fn hash() -> bl_model::ContentHash {
    bl_model::ContentHash([1; 32])
}

#[test]
fn an_unwarped_clip_plays_unchanged_while_the_song_follows_a_tempo() {
    // Spec: transport R24. The file plays at its own speed, starting on the
    // frame the followed tempo gives its beat.
    for block in BLOCKS {
        for start in [Ticks::ZERO, BEAT * 2 + Ticks::SIXTEENTH] {
            let audio = || bl_model::AudioData::new(hash());
            let want = played(
                audio_rig(4096, tempo(140.0), start, audio()),
                130_000,
                block,
            );
            let mut following = audio_rig(4096, tempo(120.0), start, audio());
            following.follow(testing::following(140.0));
            let got = played(following, 130_000, block);
            assert!(got.iter().any(|&s| s != 0.0));
            same(&got, &want, &format!("from {start:?} at blocks of {block}"));
        }
    }
}

#[test]
fn an_unwarped_clip_goes_on_through_changes_of_the_followed_tempo_and_its_end() {
    // Spec: transport R24, R26. The clip plays one file frame a frame whatever
    // the tempo does, to the end of the file, and when the follower goes.
    let change = 4 * 4096u64;
    for block in [1, 512, 4096] {
        let mut rig = audio_rig(
            4096,
            tempo(120.0),
            Ticks::ZERO,
            bl_model::AudioData::new(hash()),
        );
        rig.follow(testing::stand_in(move |ask, answer| {
            if ask.clock < change {
                answer.tempo(150.0);
            } else if ask.clock < 2 * change {
                answer.tempo(90.0);
            }
        }));
        let got = played(rig, 100_000, block);
        for (i, &sample) in got.iter().enumerate().take(97_000) {
            assert_eq!(
                sample,
                (i + 1) as f32 * STEP,
                "frame {i} at blocks of {block}"
            );
        }
        assert!(
            got[97_000..].iter().all(|&s| s == 0.0),
            "and it ends with the file"
        );
    }
}

#[test]
fn a_warped_clip_follows_the_followed_tempo() {
    // Spec: transport R24, warping R12. The file position of each frame is
    // what the same set at the followed tempo reads there.
    let warped = || bl_model::AudioData {
        warp: true,
        warp_markers: vec![bl_model::WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(120.0),
        warp_mode: bl_model::WarpMode::RePitch,
        high_quality: false,
        ..bl_model::AudioData::new(hash())
    };
    let position = |sample: f32| f64::from(sample) / f64::from(STEP) - 1.0;
    for block in BLOCKS {
        let want = played(
            audio_rig(4096, tempo(140.0), Ticks::ZERO, warped()),
            80_000,
            block,
        );
        let mut following = audio_rig(4096, tempo(120.0), Ticks::ZERO, warped());
        following.follow(testing::following(140.0));
        let got = played(following, 80_000, block);
        for i in 600..80_000 {
            let (g, w) = (position(got[i]), position(want[i]));
            assert!(
                (g - w).abs() < 0.05,
                "frame {i}: file frame {g} against {w}"
            );
            assert!((g - i as f64 * 7.0 / 6.0).abs() < 0.05, "frame {i}: {g}");
        }
    }
}

#[test]
fn a_warped_clip_holds_at_the_slowest_and_fastest_followed_tempos() {
    // Spec: transport R24. 20 BPM over a 999 BPM map and 999 over 20: the song
    // runs from 20/999 to 999/20 as fast as the map, and neither the blocks'
    // buffers nor the clip's reads overflow.
    let warped = || bl_model::AudioData {
        warp: true,
        warp_markers: vec![bl_model::WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(120.0),
        warp_mode: bl_model::WarpMode::RePitch,
        high_quality: false,
        ..bl_model::AudioData::new(hash())
    };
    let position = |sample: f32| f64::from(sample) / f64::from(STEP) - 1.0;
    for (map, followed) in [(999.0, 20.0), (20.0, 999.0)] {
        for block in BLOCKS {
            let total = if followed > map { 4_000 } else { 100_000 };
            let want = played(
                audio_rig(4096, tempo(followed), Ticks::ZERO, warped()),
                total,
                block,
            );
            let mut following = audio_rig(4096, tempo(map), Ticks::ZERO, warped());
            following.follow(testing::following(followed));
            let got = played(following, total, block);
            // The file is read at the same frames, but for the rounding of a
            // frame the song is placed on, which at 50 song frames to a frame
            // is that much of the file.
            let slack = if map > followed { 1.0 } else { 60.0 };
            let mut checked = 0;
            for i in 600..total {
                if want[i] != 0.0 && got[i] != 0.0 {
                    let (g, w) = (position(got[i]), position(want[i]));
                    assert!(
                        (g - w).abs() < slack,
                        "frame {i}: {g} against {w} at {block}"
                    );
                    checked += 1;
                }
            }
            assert!(
                checked > total / 2,
                "the clip sounds ({checked} of {total})"
            );
        }
    }
}

#[test]
fn the_playhead_stays_inside_the_loop_through_a_stress_of_followed_tempos() {
    // Spec: transport R13, R24. A followed tempo from 20 to 999 BPM that
    // changes every few blocks, jumps, starts that wait, and a nudge pressed
    // and released: the playhead never gets past the loop's end.
    let mut rig = rig(512, tempo(100.0), &NOTES, true);
    rig.sender
        .transport(TransportCommand::SetLoop(Some(bl_engine::LoopRegion {
            start: Ticks::ZERO,
            end: BAR,
        })));
    rig.follow(testing::stand_in(move |ask, answer| {
        let block = ask.clock / 512;
        if block % 97 < 90 {
            answer.tempo(20.0 + ((block / 3) * 37 % 980) as f64);
        }
        if block % 200 < 120 {
            answer.decides_starts();
            answer.start_at(ask.clock + 700);
        }
        if block % 53 == 0 {
            answer.jump(ask.clock + 100, BEAT * 2);
        }
    }));
    let mut moved = 0;
    for round in 0..3_000u64 {
        let command = match round % 60 {
            0 => Some(TransportCommand::Play),
            45 => Some(TransportCommand::Stop),
            _ => None,
        };
        if let Some(command) = command {
            rig.sender.transport(command);
        }
        assert!(rig.sender.flush());
        let before = rig.processor.position();
        rig.block(512);
        let end = rig.processor.tempo_map().ticks_to_frames(BAR, common::RATE);
        assert!(rig.processor.position() <= end, "round {round}");
        moved += usize::from(rig.processor.position() != before);
    }
    assert!(moved > 500, "{moved} blocks moved the song");
    assert_eq!(rig.monitor.state().counters.transport_overflowed, 0);
}
