//! MIDI clips under a groove (WP-6.19): where each note plays and how loud,
//! frame by frame at several block sizes, under the allocation trap.
//! Spec: grooves R6 to R11, R14 and R15.

use crate::common;
use bl_core::time::{TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::testing;
use bl_engine::{NoteKind, TransportCommand};
use bl_model::{
    Clip, ClipData, ClipId, Command, Groove, GrooveId, GrooveNote, MidiData, Note, TrackKind,
};
use common::{RATE, Rig, device, track};

const BAR: Ticks = Ticks(TICKS_PER_QUARTER * 4);
const STEP: i64 = TICKS_PER_QUARTER / 4;

/// The groove of the tests: a bar of sixteenths, every second one a quarter
/// of a step late, with the velocity of the on-beat ones loud and the rest
/// soft.
fn swing(id: GrooveId) -> Groove {
    let notes = (0..16)
        .map(|i| GrooveNote {
            position: Ticks(i * STEP + if i % 2 == 1 { STEP / 4 } else { 0 }),
            velocity: if i % 2 == 0 { 127 } else { 65 },
        })
        .collect();
    Groove::new(id, "Swing".to_owned(), BAR, notes)
}

/// A rig with a MIDI track playing `notes` in a clip of `bars` bars, looped
/// over one bar when `looped`, with the groove set by `shape` and its clip
/// grooved.
struct Song {
    rig: Rig,
    groove: GrooveId,
    clip: ClipId,
}

fn grooved(notes: Vec<Note>, bars: i64, looped: bool, shape: impl FnOnce(&mut Groove)) -> Song {
    let mut rig = Rig::new(4096);
    let trk = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, trk, testing::INSTRUMENT, &[]);
    let mut midi = MidiData::new(notes);
    if looped {
        midi.looping = true;
        midi.loop_end = Some(BAR);
    }
    let clip: ClipId = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateClip {
            track: trk,
            clip: Clip::new(clip, Ticks::ZERO, BAR * bars, ClipData::Midi(midi)),
        })
        .unwrap();
    let groove: GrooveId = rig.editor.new_id();
    let mut made = swing(groove);
    shape(&mut made);
    rig.editor
        .apply(Command::AddGroove { groove: made })
        .unwrap();
    rig.editor
        .apply(Command::SetClipGroove {
            clips: vec![clip],
            groove: Some(groove),
        })
        .unwrap();
    rig.sync();
    Song { rig, groove, clip }
}

/// A song of one clip of `notes` that is grooved, with Quantize 100 % and
/// Timing 100 %, or not.
fn grooved_song(notes: Vec<Note>, with_groove: bool) -> Song {
    let mut song = grooved(notes, 1, false, |groove| groove.quantize = 100.0);
    if !with_groove {
        song.rig.apply(Command::SetClipGroove {
            clips: vec![song.clip],
            groove: None,
        });
    }
    song
}

/// What a note-on was: its frame, key and velocity.
type On = (i64, u8, u8);

impl Song {
    /// Plays the first `ticks` of the song in blocks of `block`.
    fn render(&mut self, ticks: Ticks, block: usize) {
        let frames = TempoMap::default().ticks_to_frames(ticks, RATE).get() as usize;
        self.rig.sender.transport(TransportCommand::Play);
        let mut left = frames;
        while left > 0 {
            let n = left.min(block);
            self.rig.block(n);
            left -= n;
        }
    }

    /// Plays the first `ticks` of the song in blocks of `block`, and reads
    /// the note-ons.
    fn play(&mut self, ticks: Ticks, block: usize) -> Vec<On> {
        self.render(ticks, block);
        self.rig
            .probe
            .recorded_notes()
            .into_iter()
            .filter(|note| note.event.kind == NoteKind::On)
            .map(|note| {
                (
                    note.frame,
                    note.event.key,
                    (note.event.velocity * 127.0).round() as u8,
                )
            })
            .collect()
    }
}

fn frame_of(tick: i64) -> i64 {
    TempoMap::default().ticks_to_frames(Ticks(tick), RATE).get()
}

/// Sixteenths a 1/64 off the grid, all at velocity 100.
fn sixteenths() -> Vec<Note> {
    (0..16)
        .map(|i| Note::new(60, Ticks(i * STEP + STEP / 16), Ticks(STEP / 2), 100))
        .collect()
}

/// Where R7 puts sixteenth `i` of `sixteenths`, worked out from the law's
/// words and not from the model's code: Quantize pulls it to its line, the
/// line offsets (0 or a quarter step, on the odd lines) run in straight
/// lines between the lines, and Timing scales them.
fn expected_tick(i: i64, quantize: f64, timing: f64) -> i64 {
    let offset = |line: i64| {
        if line % 2 == 1 {
            STEP as f64 / 4.0
        } else {
            0.0
        }
    };
    let p = (i * STEP + STEP / 16) as f64;
    let line = (i * STEP) as f64;
    let q = p + quantize * (line - p);
    let t = (q - line) / STEP as f64;
    let displacement = offset(i) + t * (offset(i + 1) - offset(i));
    (q + timing * displacement).round() as i64
}

#[test]
fn sixteenths_play_where_timing_and_quantize_put_them() {
    // Spec: grooves R7
    for quantize in [0.0, 0.5, 1.0] {
        for timing in [0.0, 0.5, 1.0] {
            let mut song = grooved(sixteenths(), 1, false, |groove| {
                groove.quantize = quantize * 100.0;
                groove.timing = timing * 100.0;
            });
            let played = song.play(BAR, 512);
            let want: Vec<i64> = (0..16)
                .map(|i| frame_of(expected_tick(i, quantize, timing)))
                .collect();
            assert_eq!(
                played.iter().map(|on| on.0).collect::<Vec<_>>(),
                want,
                "Quantize {quantize}, Timing {timing}"
            );
        }
    }
}

#[test]
fn the_global_amount_scales_timing_and_not_quantize() {
    // Spec: grooves R10
    for amount in [0.0, 0.5, 1.0, 1.3] {
        let mut song = grooved(sixteenths(), 1, false, |groove| {
            groove.quantize = 100.0;
            groove.timing = 100.0;
        });
        song.rig.apply(Command::SetGlobalGrooveAmount {
            amount: amount * 100.0,
        });
        let played = song.play(BAR, 512);
        // Quantize puts every note on its line; Timing moves the odd ones by
        // the line's offset, scaled by the amount and held under half a step.
        let want: Vec<i64> = (0..16)
            .map(|i| {
                let moved = if i % 2 == 1 {
                    STEP as f64 / 4.0 * amount
                } else {
                    0.0
                };
                frame_of(i * STEP + moved.round() as i64)
            })
            .collect();
        assert_eq!(
            played.iter().map(|on| on.0).collect::<Vec<_>>(),
            want,
            "Global Amount {amount}"
        );
    }
}

#[test]
fn velocity_follows_the_grooves_accents_and_negative_reverses_them() {
    // Spec: grooves R9, R10
    let notes = || {
        (0..16)
            .map(|i| Note::new(60, Ticks(i * STEP), Ticks(STEP / 2), 80))
            .collect::<Vec<_>>()
    };
    let velocities = |velocity: f64, amount: f64| {
        let mut song = grooved(notes(), 1, false, |groove| groove.velocity = velocity);
        song.rig.apply(Command::SetGlobalGrooveAmount {
            amount: amount * 100.0,
        });
        song.play(BAR, 512)
            .into_iter()
            .map(|on| on.2)
            .collect::<Vec<_>>()
    };
    // The groove's mean velocity is 96: its loud steps are 31 above it, the
    // soft ones 31 below.
    let louder: Vec<u8> = (0..16).map(|i| if i % 2 == 0 { 111 } else { 49 }).collect();
    assert_eq!(velocities(100.0, 1.0), louder);
    let reversed: Vec<u8> = (0..16).map(|i| if i % 2 == 0 { 49 } else { 111 }).collect();
    assert_eq!(velocities(-100.0, 1.0), reversed);
    let half: Vec<u8> = (0..16).map(|i| if i % 2 == 0 { 96 } else { 65 }).collect();
    assert_eq!(velocities(50.0, 1.0), half);
    // The Global Amount scales it.
    assert_eq!(velocities(100.0, 0.5), half);
    assert_eq!(velocities(0.0, 1.0), vec![80; 16]);
}

#[test]
fn random_stays_inside_a_quarter_step_and_two_passes_differ() {
    // Spec: grooves R8
    let notes = || {
        (0..16)
            .map(|i| Note::new(60, Ticks(i * STEP), Ticks(STEP / 2), 100))
            .collect::<Vec<_>>()
    };
    let random = |groove: &mut Groove| {
        groove.timing = 0.0;
        groove.random = 100.0;
    };
    let moves = |ons: &[On], pass: i64| {
        (0..16)
            .map(|i| ons[(pass * 16 + i) as usize].0 - frame_of(pass * BAR.0 + i * STEP))
            .collect::<Vec<_>>()
    };
    let mut first = grooved(notes(), 2, true, random);
    let first_ons = first.play(BAR * 2, 512);
    assert_eq!(first_ons.len(), 32);
    let (pass0, pass1) = (moves(&first_ons, 0), moves(&first_ons, 1));
    let quarter_step = frame_of(STEP) / 4;
    assert!(pass0.iter().chain(&pass1).all(|m| m.abs() <= quarter_step));
    assert!(pass0.iter().any(|m| *m != 0), "Random moves notes");
    assert_ne!(pass0, pass1, "each pass draws its own numbers");
    // Notes that start together drift apart.
    let mut chord = grooved(
        (0..4)
            .map(|k| Note::new(60 + k, Ticks::ZERO, Ticks(STEP), 100))
            .collect(),
        1,
        false,
        random,
    );
    let at: Vec<i64> = chord
        .play(Ticks(STEP * 2), 512)
        .iter()
        .map(|on| on.0)
        .collect();
    assert_eq!(at.len(), 4);
    assert!(at.windows(2).any(|pair| pair[0] != pair[1]), "{at:?}");
    // A second render from the transport's start is the same.
    let mut second = grooved(notes(), 2, true, random);
    assert_eq!(second.play(BAR * 2, 512), first_ons);
}

#[test]
fn a_note_moved_past_its_passs_edge_plays_inside_it() {
    // Spec: grooves R11
    // Line 0 is a quarter step early, so the note on it would play before
    // its pass; a note just before the loop's end sits nearest the next
    // line 0, a quarter step late in this groove, past the pass's end.
    let late = |groove: &mut Groove| {
        groove.notes = vec![GrooveNote {
            position: Ticks(STEP / 4),
            velocity: 100,
        }];
        groove.quantize = 100.0;
    };
    let mut song = grooved(
        vec![
            Note::new(60, Ticks::ZERO, Ticks(STEP / 2), 100),
            Note::new(64, BAR - Ticks(100), Ticks(STEP / 2), 100),
        ],
        2,
        true,
        late,
    );
    let ons = song.play(BAR * 2 + Ticks(STEP), 512);
    let wrap = frame_of(BAR.0);
    let second: Vec<i64> = ons.iter().filter(|on| on.1 == 64).map(|on| on.0).collect();
    assert_eq!(second.len(), 2, "{ons:?}");
    // Moved to or past the end of its pass, it starts one tick before it.
    assert!((wrap - 1..=wrap).contains(&second[0]), "{second:?} {wrap}");
    // The first note, on line 0, moves a quarter step later and stays inside.
    let first: Vec<i64> = ons.iter().filter(|on| on.1 == 60).map(|on| on.0).collect();
    assert_eq!(first[0], frame_of(STEP / 4));
    // Moved before the clip's start, it starts there.
    let early = |groove: &mut Groove| {
        groove.notes = vec![GrooveNote {
            position: BAR - Ticks(STEP / 4),
            velocity: 100,
        }];
        groove.quantize = 100.0;
    };
    let mut song = grooved(
        vec![Note::new(60, Ticks::ZERO, Ticks(STEP / 2), 100)],
        1,
        false,
        early,
    );
    let ons = song.play(Ticks(STEP), 512);
    assert_eq!(ons[0].0, 0);
}

#[test]
fn every_block_size_plays_the_same_notes() {
    // Spec: grooves R8, R15
    let mut renders = Vec::new();
    for block in [1, 64, 512, 4096] {
        let mut song = grooved(sixteenths(), 2, true, |groove| {
            groove.quantize = 50.0;
            groove.timing = 100.0;
            groove.random = 60.0;
            groove.velocity = 70.0;
        });
        renders.push(song.play(BAR * 2, block));
    }
    assert_eq!(renders[0].len(), 32);
    for render in &renders[1..] {
        assert_eq!(render, &renders[0]);
    }
}

#[test]
fn chance_and_deviation_on_regrooved_chords_are_the_same_at_every_block_size() {
    // Spec: grooves R8, R15. Random pulls a chord's notes apart, so the block
    // meets them in a different order at each size.
    let chords: Vec<Note> = sixteenths()
        .into_iter()
        .flat_map(|note| {
            [60, 64, 67].map(|key| {
                let mut note = Note::new(key, note.start, note.length, 90);
                note.probability = 0.5;
                note.velocity_deviation = 30;
                note
            })
        })
        .collect();
    let mut renders = Vec::new();
    for block in [1, 64, 512, 4096] {
        let mut song = grooved(chords.clone(), 1, false, |groove| groove.random = 100.0);
        renders.push(song.play(BAR, block));
    }
    assert!(!renders[0].is_empty() && renders[0].len() < 48);
    for render in &renders[1..] {
        assert_eq!(render, &renders[0]);
    }
}

#[test]
fn a_clip_without_a_groove_plays_as_before() {
    // Spec: grooves R14
    let mut song = grooved(sixteenths(), 1, false, |_| {});
    song.rig.apply(Command::SetClipGroove {
        clips: vec![song.clip],
        groove: None,
    });
    let ons = song.play(BAR, 512);
    let want: Vec<i64> = (0..16).map(|i| frame_of(i * STEP + STEP / 16)).collect();
    assert_eq!(ons.iter().map(|on| on.0).collect::<Vec<_>>(), want);
    assert!(ons.iter().all(|on| on.2 == 100));
}

#[test]
fn a_groove_edit_while_playing_reaches_the_next_block_without_a_program() {
    // Spec: grooves R5, R15
    let mut song = grooved(sixteenths(), 1, false, |groove| groove.timing = 0.0);
    song.rig.sender.transport(TransportCommand::Play);
    song.rig.block(512);
    let synced = song.rig.apply(Command::SetGroove {
        groove: song.groove,
        base: None,
        quantize: Some(100.0),
        timing: Some(100.0),
        random: None,
        velocity: None,
    });
    assert!(!synced.program, "a groove edit makes no program");
    assert!(synced.data > 0, "it reaches the player as node data");
    let mut left = frame_of(BAR.0) as usize - 512;
    while left > 0 {
        let n = left.min(512);
        song.rig.block(n);
        left -= n;
    }
    let ons: Vec<i64> = song
        .rig
        .probe
        .recorded_notes()
        .into_iter()
        .filter(|note| note.event.kind == NoteKind::On)
        .map(|note| note.frame)
        .collect();
    // The first block's notes played as written (Timing 0, Quantize 0): the
    // first sixteenth is a 1/64 late. Everything after Quantize 100 and
    // Timing 100 is on its line, the odd ones moved a quarter step.
    assert_eq!(ons[0], frame_of(STEP / 16));
    let blocks = 512;
    let after: Vec<i64> = ons.iter().copied().filter(|f| *f >= blocks).collect();
    let want: Vec<i64> = (0..16)
        .map(|i| frame_of(i * STEP + if i % 2 == 1 { STEP / 4 } else { 0 }))
        .filter(|f| *f >= blocks)
        .collect();
    assert_eq!(after, want);
}

#[test]
fn every_devices_block_carries_the_global_amount() {
    // Spec: grooves R21
    let mut rig = Rig::new(512);
    let trk = track(&mut rig.editor, TrackKind::Audio, None);
    device(&mut rig.editor, trk, testing::GROOVE_AMOUNT, &[]);
    rig.sync();
    rig.block(64);
    // A set with no grooves has the amount at 100 %.
    assert!((rig.last()[0] - 1.0).abs() < 1e-6);
    for percent in [0.0, 62.5, 130.0] {
        let synced = rig.apply(Command::SetGlobalGrooveAmount { amount: percent });
        assert!(!synced.program, "the amount makes no program");
        rig.block(64);
        assert!(
            (f64::from(rig.last()[0]) - percent / 100.0).abs() < 1e-3,
            "{percent}: {}",
            rig.last()[0]
        );
    }
}

/// Every control-change and pressure event the track played, with its frame.
fn controls(rig: &mut Rig) -> Vec<(i64, String)> {
    rig.probe
        .recorded_notes()
        .into_iter()
        .filter_map(|note| match note.event.kind {
            NoteKind::Control { controller, value } => {
                Some((note.frame, format!("cc{controller}={value}")))
            }
            NoteKind::Expression { value, .. } => Some((note.frame, format!("pressure={value}"))),
            _ => None,
        })
        .collect()
}

#[test]
fn a_grooves_clip_envelopes_stay_where_they_are_and_a_notes_expressions_move_with_it() {
    // Spec: grooves R11, R14. The controller envelope plays on its own
    // timeline whatever the groove does; a note's pressure points are
    // measured from where the note plays.
    let mut with = Note::new(60, Ticks(STEP + STEP / 16), Ticks(STEP), 100);
    with.pressure = vec![bl_model::PressurePoint {
        at: Ticks(STEP / 8),
        value: 90,
    }];
    let build = |grooved: bool| {
        let mut song = grooved_song(vec![with.clone()], grooved);
        for (at, value) in [(Ticks(0), 10.0), (Ticks(STEP * 2), 100.0)] {
            song.rig.apply(Command::SetClipEnvelopeBreakpoint {
                clip: song.clip,
                target: bl_model::AutomationTarget::MidiCc { number: 1 },
                kind: bl_model::EnvelopeKind::Modulation,
                at,
                value,
            });
        }
        song
    };
    let mut plain = build(false);
    plain.render(BAR, 512);
    let plain_events = controls(&mut plain.rig);
    let mut grooved = build(true);
    grooved.render(BAR, 512);
    let grooved_events = controls(&mut grooved.rig);
    let cc = |events: &[(i64, String)]| {
        events
            .iter()
            .filter(|(_, what)| what.starts_with("cc"))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert!(!cc(&plain_events).is_empty(), "the envelope plays");
    assert_eq!(
        cc(&plain_events),
        cc(&grooved_events),
        "the envelope is not grooved"
    );
    // The note's line 1 is a quarter step late, which Timing moves it by.
    let pressure = |events: &[(i64, String)]| {
        events
            .iter()
            .find(|(_, what)| what.starts_with("pressure"))
            .map(|(frame, _)| *frame)
    };
    let moved = frame_of(STEP + STEP / 4 + STEP / 8) - frame_of(STEP + STEP / 16 + STEP / 8);
    assert_eq!(
        pressure(&grooved_events).unwrap() - pressure(&plain_events).unwrap(),
        moved,
        "the pressure point plays as far after the note as before"
    );
}
