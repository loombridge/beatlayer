//! What Capture MIDI can bring back, and the clip it makes (WP-3.14).
//!
//! The ring: while the engine runs, the audio thread copies every note-on and
//! note-off a monitoring MIDI track's input receives into a lock-free ring
//! of [`CapturedMidi`], whether or not the transport runs and whether or not
//! anything records. The main thread drains the ring
//! ([`Engine::captured`](crate::Engine::captured)) into a bounded buffer per
//! track, so nothing on the audio thread allocates, locks, or waits. A full
//! ring drops the message and counts it
//! ([`MidiCounters::capture_dropped`](crate::MidiCounters)).
//!
//! The rules: [`Buffer`] keeps the latest notes per track, and [`pair`]
//! turns a track's messages into notes. [`fit`] decides the tempo a capture
//! with the transport stopped sets, [`place`] puts the notes in a new clip
//! and picks its loop, and [`merge`] adds them to a clip that plays. All of
//! it is arithmetic on frames and ticks, so the rules are tested without an
//! engine running.

use std::collections::{BTreeMap, VecDeque};

use bl_core::midi::MidiMessage;
use bl_core::time::{Bpm, Frames, SampleRate, TICKS_PER_QUARTER, Ticks};
use bl_model::{MidiData, Note, TrackId};

/// How many messages the ring holds between two drains.
pub const CAPTURE_RING: usize = 4096;

/// A note message a monitoring MIDI track's input received, and when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapturedMidi {
    /// The track whose input received it.
    pub track: TrackId,
    /// The engine clock frame it landed on: frames the engine has processed,
    /// running whether or not the transport does, so the spacing of notes
    /// played with the transport stopped is kept.
    pub clock: u64,
    /// The song frame it landed on, to the sample, if the song was playing;
    /// `None` while the transport was stopped or counting in.
    pub song: Option<Frames>,
    /// A note-on or a note-off.
    pub message: MidiMessage,
}

/// The most messages the buffer keeps for a track. The oldest go first.
pub const MAX_MESSAGES: usize = 4096;

/// The accepted range of a played tempo against the song's: outside it, a
/// capture doesn't touch the tempo (the capture MIDI spec, R6).
const RANGE: (f64, f64) = (2.0 / 3.0, 4.0 / 3.0);

/// How far a played tempo may be from the song's for the song to keep its
/// tempo and the notes to be stretched onto its beats, as a ratio.
const FIT: f64 = 1.09;

/// Notes this close, in seconds, are one onset: a chord is played at once.
const SAME_ONSET: f64 = 0.03;

/// Two bars of silence between notes start a new phrase.
const PHRASE_GAP_BARS: i64 = 2;

/// Float error in timing, as a hundredth of a beat: a note this far past a
/// bar line is on it.
const SLACK: i64 = TICKS_PER_QUARTER / 100;

/// What monitoring MIDI tracks received, kept until a capture takes it.
#[derive(Debug, Default)]
pub struct Buffer {
    tracks: BTreeMap<TrackId, VecDeque<CapturedMidi>>,
}

impl Buffer {
    /// Keeps `message`, letting go of the track's oldest when it is full.
    pub fn push(&mut self, message: CapturedMidi) {
        let kept = self.tracks.entry(message.track).or_default();
        if kept.len() == MAX_MESSAGES {
            kept.pop_front();
        }
        kept.push_back(message);
    }

    /// Forgets everything: a recording or a capture used it.
    pub fn clear(&mut self) {
        self.tracks.clear();
    }

    /// Whether a note was played: a note-off alone is nothing to capture.
    #[must_use]
    pub fn has_notes(&self) -> bool {
        self.tracks
            .values()
            .flatten()
            .any(|m| m.message.is_note_on())
    }

    /// The messages per track, oldest first, leaving the buffer empty.
    pub fn take(&mut self) -> BTreeMap<TrackId, Vec<CapturedMidi>> {
        std::mem::take(&mut self.tracks)
            .into_iter()
            .map(|(track, kept)| (track, kept.into()))
            .collect()
    }
}

/// A note as it was played, timed in engine-clock frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Played {
    /// The key.
    pub key: u8,
    /// The velocity of the note-on.
    pub velocity: u8,
    /// The velocity of the note-off, 64 when it had none.
    pub release: u8,
    /// The engine-clock frame of the note-on.
    pub on: u64,
    /// The engine-clock frame of the note-off.
    pub off: u64,
    /// The song frame of the note-on, if the song's position is known for it.
    pub song_on: Option<Frames>,
    /// The song frame of the note-off.
    pub song_off: Option<Frames>,
}

/// The notes a track's messages played: each note-on starts a note on its
/// key, and the next note-off on that key (or a note-on on it again) ends
/// it. A note still held ends with the track's last message; a note-off
/// with nothing held is ignored. Song frames are those the engine stamped
/// while the song played, and, for messages before that, the first stamp's
/// less the clock frames between them.
#[must_use]
pub fn pair(messages: &[CapturedMidi]) -> Vec<Played> {
    let anchor = messages.iter().find_map(|m| m.song.map(|s| (m.clock, s)));
    let song = |m: &CapturedMidi| {
        m.song
            .or_else(|| anchor.map(|(clock, at)| at + Frames(m.clock as i64 - clock as i64)))
    };
    let end = messages.last().map_or(0, |m| m.clock);
    let end_song = messages.last().and_then(song);
    let mut held: [Option<&CapturedMidi>; 128] = [None; 128];
    let mut notes = Vec::new();
    let mut finish = |on: &CapturedMidi, off_clock: u64, off_song, release: u8| {
        notes.push(Played {
            key: on.message.data1(),
            velocity: on.message.data2().max(1),
            release: if release == 0 { 64 } else { release },
            on: on.clock,
            off: off_clock.max(on.clock),
            song_on: song(on),
            song_off: off_song,
        });
    };
    for m in messages {
        let key = usize::from(m.message.data1());
        let on = m.message.is_note_on();
        if let Some(started) = held[key].take() {
            finish(
                started,
                m.clock,
                song(m),
                if on { 0 } else { m.message.data2() },
            );
        }
        if on {
            held[key] = Some(m);
        }
    }
    for started in held.into_iter().flatten() {
        finish(started, end, end_song, 0);
    }
    notes.sort_by_key(|note| note.on);
    notes
}

/// The tempo a capture with the transport stopped puts the notes at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// The tempo the notes' times are counted at: a beat of the clip is
    /// `60 / bpm` seconds of what was played.
    pub bpm: f64,
    /// The new song tempo, if the capture changes it.
    pub set: Option<Bpm>,
}

/// Works out the tempo from the onsets of the last phrase, in seconds, in
/// order, against the song's `current` tempo (the capture MIDI spec, R6).
///
/// The played tempo is the one the median gap between onsets makes. It is
/// tried as a beat, and, outside the accepted range, as a half note (twice
/// the tempo). A candidate close to the song's tempo leaves it, and the
/// notes are stretched onto its beats; a farther one in range becomes the
/// song's tempo, to 0.5 BPM; with none in range the notes keep their raw
/// times at the song's tempo.
#[must_use]
pub fn fit(onsets: &[f64], current: f64) -> Fit {
    let keep = Fit {
        bpm: current,
        set: None,
    };
    let mut kept: Vec<f64> = Vec::new();
    for &onset in onsets {
        if kept.last().is_none_or(|&last| onset - last >= SAME_ONSET) {
            kept.push(onset);
        }
    }
    let mut gaps: Vec<f64> = kept.windows(2).map(|pair| pair[1] - pair[0]).collect();
    if gaps.is_empty() {
        return keep;
    }
    gaps.sort_by(f64::total_cmp);
    let middle = gaps.len() / 2;
    let median = if gaps.len() % 2 == 1 {
        gaps[middle]
    } else {
        (gaps[middle - 1] + gaps[middle]) / 2.0
    };
    let played = 60.0 / median;
    for candidate in [played, played * 2.0] {
        let ratio = candidate / current;
        if !(RANGE.0..=RANGE.1).contains(&ratio) {
            continue;
        }
        if (1.0 / FIT..=FIT).contains(&ratio) {
            return Fit {
                bpm: candidate,
                set: None,
            };
        }
        let set = (candidate * 2.0).round() / 2.0;
        return Bpm::new(set).map_or(keep, |bpm| Fit {
            bpm: set,
            set: Some(bpm),
        });
    }
    keep
}

/// Where the notes of one track go: positions counted from the clip's start.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// The notes.
    pub notes: Vec<Note>,
    /// Where the loop begins.
    pub loop_start: Ticks,
    /// Where it ends, which is also the clip's length.
    pub loop_end: Ticks,
}

/// The index in `notes` of the first note of the last phrase: the notes
/// after the last silence of two bars. `bar_seconds` is a bar at the song's
/// tempo.
#[must_use]
pub fn last_phrase(notes: &[Played], rate: SampleRate, bar_seconds: f64) -> usize {
    let gap = PHRASE_GAP_BARS as f64 * bar_seconds;
    let seconds = |frames: u64| frames as f64 / rate.as_f64();
    (1..notes.len())
        .rev()
        .find(|&i| seconds(notes[i].on - notes[i - 1].on) >= gap)
        .unwrap_or(0)
}

/// Rounds `ticks` up to a whole number of bars of `bar`, within [`SLACK`].
fn bars_up(ticks: Ticks, bar: Ticks) -> i64 {
    ((ticks.get() - SLACK).max(0) + bar.get() - 1) / bar.get()
}

/// How a capture times a note.
#[derive(Clone, Copy, Debug)]
pub enum Timing {
    /// The transport was stopped: the clip's start is the engine-clock
    /// frame `from`, and a beat is `60 / bpm` seconds from there.
    Stopped {
        /// The earliest note's engine-clock frame.
        from: u64,
        /// The tempo the times are counted at.
        bpm: f64,
        /// The sample rate.
        rate: SampleRate,
    },
    /// The song played: the clip's start is the song tick `origin`, and a
    /// note stands where the song's frame is, through `ticks`.
    Running {
        /// A bar line at or before the earliest note.
        origin: Ticks,
    },
}

/// Puts one track's `notes` in a new clip: each at its time (`ticks` gives a
/// song frame's tick), the loop on the last phrase's bars. The loop is as
/// many whole bars of length `bar` as cover the phrase from its first
/// note-on to its last, ending on the bar line at or after the last note-on;
/// a note-on exactly on the loop's end stays in the clip, after it.
#[must_use]
pub fn place(
    notes: &[Played],
    timing: Timing,
    ticks: impl Fn(Frames) -> Ticks,
    phrase: usize,
    bar: Ticks,
) -> Placed {
    let at = |clock: u64, song: Option<Frames>| match timing {
        Timing::Stopped { from, bpm, rate } => {
            let seconds = (clock - from) as f64 / rate.as_f64();
            Ticks((seconds * bpm / 60.0 * TICKS_PER_QUARTER as f64).round() as i64)
        }
        Timing::Running { origin } => (song.map_or(Ticks::ZERO, &ticks) - origin).max(Ticks::ZERO),
    };
    let placed: Vec<(Ticks, Ticks, &Played)> = notes
        .iter()
        .map(|n| (at(n.on, n.song_on), at(n.off, n.song_off), n))
        .collect();
    let last = placed
        .iter()
        .map(|(on, ..)| *on)
        .max()
        .unwrap_or(Ticks::ZERO);
    let first = placed.get(phrase).map_or(Ticks::ZERO, |(on, ..)| *on);
    let end = bars_up(last, bar).max(1);
    let span = bars_up(last - first, bar).max(1);
    Placed {
        notes: placed
            .into_iter()
            .map(|(on, off, n)| {
                let mut note = Note::new(n.key, on, (off - on).max(Ticks(1)), n.velocity);
                note.release_velocity = n.release;
                note
            })
            .collect(),
        loop_start: bar * (end - span),
        loop_end: bar * end,
    }
}

/// A playing clip with the notes played into it: `played` starts are counted
/// from the frame the clip launched on. A looping clip is unrolled to the
/// pass the last note falls in, each pass a copy of the loop, with the notes
/// added where they were played, and the loop moves to that last pass; the
/// loop keeps its length. Notes in the first pass leave the markers alone. A
/// clip that does not loop takes the notes that fall before its end.
///
/// Returns the data and the clip's new length.
#[must_use]
pub fn merge(data: &MidiData, length: Ticks, played: Vec<Note>) -> (MidiData, Ticks) {
    let marker = data.start;
    let loop_start = data.loop_start;
    let loop_end = data
        .loop_end
        .unwrap_or_else(|| (loop_start + length).max(loop_start + Ticks::SIXTEENTH));
    let mut merged = data.clone();
    if !data.looping {
        let end = data.end.unwrap_or(marker + length).min(marker + length);
        merged.notes.extend(
            played
                .into_iter()
                .map(|note| Note {
                    start: marker + note.start,
                    ..note
                })
                .filter(|note| note.start < end),
        );
        return (merged, length);
    }
    let pass = loop_end - loop_start;
    let played: Vec<Note> = played
        .into_iter()
        .map(|note| Note {
            start: marker + note.start,
            ..note
        })
        .collect();
    let last = played
        .iter()
        .map(|note| note.start)
        .max()
        .unwrap_or(Ticks::ZERO);
    let end = if last <= loop_end {
        loop_end
    } else {
        let passes = ((last - loop_start).get() + pass.get() - 1) / pass.get();
        loop_start + Ticks(passes * pass.get())
    };
    let copies = (end - loop_end).get() / pass.get();
    for copy in 1..=copies {
        let shift = Ticks(copy * pass.get());
        let repeated: Vec<Note> = data
            .notes
            .iter()
            .filter(|note| note.start >= loop_start && note.start < loop_end)
            .map(|note| Note {
                start: note.start + shift,
                ..*note
            })
            .collect();
        merged.notes.extend(repeated);
    }
    merged.notes.extend(played);
    if end > loop_end {
        merged.start = end - pass;
        merged.loop_start = end - pass;
        merged.loop_end = Some(end);
        merged.end = None;
    }
    (merged, length.max(end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate() -> SampleRate {
        SampleRate::HZ_48000
    }

    /// Evenly spaced onsets `count` long, `gap` seconds apart.
    fn onsets(count: usize, gap: f64) -> Vec<f64> {
        (0..count).map(|i| i as f64 * gap).collect()
    }

    // Spec: capture-midi R6. Every row of the tempo table observed in Live.
    #[test]
    fn the_tempo_follows_the_rows_observed_in_live() {
        // (song tempo, seconds between notes, the tempo that results, and
        // whether the song's own tempo is kept)
        let rows: [(f64, f64, f64, bool); 14] = [
            (120.0, 0.5, 120.0, true),
            (104.0, 0.6, 104.0, true),
            (145.0, 0.4, 145.0, true),
            (140.0, 0.4, 140.0, true),
            (135.0, 0.4, 150.0, false),
            (130.0, 0.4, 150.0, false),
            (125.0, 0.4, 150.0, false),
            (115.0, 0.4, 150.0, false),
            (100.0, 0.5018, 119.5, false),
            (120.0, 0.6, 100.0, false),
            (140.0, 0.6024, 99.5, false),
            (160.0, 0.6, 200.0, false),
            (108.0, 0.4, 108.0, true),
            (100.0, 0.4, 100.0, true),
        ];
        for (song, gap, result, kept) in rows {
            let fit = fit(&onsets(8, gap), song);
            let tempo = fit.set.map_or(song, Bpm::get);
            assert_eq!(tempo, result, "song {song}, gap {gap}");
            assert_eq!(fit.set.is_none(), kept, "song {song}, gap {gap}");
        }
    }

    // Spec: capture-midi R6. A song that keeps its tempo stretches the
    // notes onto its beats; one with no tempo in range leaves them raw.
    #[test]
    fn a_kept_tempo_stretches_the_notes_and_none_in_range_leaves_them_raw() {
        let stretched = fit(&onsets(8, 0.4), 145.0);
        assert!((stretched.bpm - 150.0).abs() < 1e-9);
        assert_eq!(stretched.set, None);
        let raw = fit(&onsets(8, 0.4), 108.0);
        assert_eq!(raw.bpm, 108.0);
        assert_eq!(raw.set, None);
        assert_eq!(fit(&[1.0], 120.0).bpm, 120.0, "one note has no tempo");
        let chord = fit(&[1.0, 1.001, 1.002], 120.0);
        assert_eq!(chord.bpm, 120.0, "a chord is one onset");
    }

    /// Notes of a quarter apart at 120 BPM: `count` of them, `gap` frames
    /// between, each a tenth of a second long.
    fn played(count: u64, gap: u64) -> Vec<Played> {
        (0..count)
            .map(|i| Played {
                key: 60,
                velocity: 100,
                release: 64,
                on: 10_000 + i * gap,
                off: 10_000 + i * gap + 4_800,
                song_on: None,
                song_off: None,
            })
            .collect()
    }

    fn stopped(notes: &[Played], bpm: f64) -> Placed {
        let bar = Ticks::WHOLE;
        let timing = Timing::Stopped {
            from: notes[0].on,
            bpm,
            rate: rate(),
        };
        place(notes, timing, |_| Ticks::ZERO, 0, bar)
    }

    // Spec: capture-midi R3. The loop is the fewest whole bars that cover
    // the first note-on to the last, from the first note on 1.1.1.
    #[test]
    fn the_loop_is_the_bars_that_cover_the_first_note_on_to_the_last() {
        let quarter = 24_000; // at 120 BPM
        for (count, gap, bars) in [
            (4, quarter, 1),
            (8, quarter, 2),
            (9, quarter, 2),
            (12, quarter, 3),
            (8, 16_000, 2),
        ] {
            let placed = stopped(&played(count, gap), 120.0);
            assert_eq!(placed.notes[0].start, Ticks::ZERO);
            assert_eq!(placed.loop_start, Ticks::ZERO, "{count} notes");
            assert_eq!(placed.loop_end, Ticks::WHOLE * bars, "{count} notes");
        }
        // The ninth note sits on the loop's end and stays.
        let nine = stopped(&played(9, quarter), 120.0);
        assert_eq!(nine.notes[8].start, Ticks::WHOLE * 2);
        // Eight half notes at 200 BPM: 14 beats, 4 bars.
        let half = stopped(&played(8, 28_800), 200.0);
        assert_eq!(half.loop_end, Ticks::WHOLE * 4);
    }

    // Spec: capture-midi R3, R8. Earlier notes stay before a loop that
    // covers the last phrase only.
    #[test]
    fn an_earlier_phrase_stays_before_the_loop() {
        let mut notes = played(3, 24_000);
        let proto = notes[0];
        let later = 10_000 + 20 * 48_000;
        notes.extend((0..8).map(|i| Played {
            on: later + i * 24_000,
            off: later + i * 24_000 + 4_800,
            ..proto
        }));
        let phrase = last_phrase(&notes, rate(), 2.0);
        assert_eq!(phrase, 3);
        let timing = Timing::Stopped {
            from: notes[0].on,
            bpm: 120.0,
            rate: rate(),
        };
        let placed = place(&notes, timing, |_| Ticks::ZERO, phrase, Ticks::WHOLE);
        // 20 s at 120 BPM is 10 bars; the phrase's last note is at 7 beats.
        assert_eq!(placed.loop_start, Ticks::WHOLE * 10);
        assert_eq!(placed.loop_end, Ticks::WHOLE * 12);
        assert_eq!(placed.notes[0].start, Ticks::ZERO);
    }

    // Spec: capture-midi R7. With the song playing, a new clip starts on the
    // bar line at or before the first note, and a note stands at its song
    // position; one played before the song started is placed by its time from
    // the first stamped note.
    #[test]
    fn a_running_capture_is_placed_on_the_songs_bar_grid() {
        use bl_core::midi::MidiMessage;
        let track = TrackId(bl_core::id::Id::new(1).unwrap());
        let message = |clock, song: Option<i64>, message| CapturedMidi {
            track,
            clock,
            song: song.map(Frames),
            message,
        };
        // 120 BPM at 48 kHz: a beat is 24,000 frames, a bar 96,000.
        let messages = [
            message(1_000, None, MidiMessage::note_on(0, 60, 100)),
            message(2_000, None, MidiMessage::note_off(0, 60)),
            message(50_000, Some(100_000), MidiMessage::note_on(0, 62, 100)),
            message(60_000, Some(110_000), MidiMessage::note_off(0, 62)),
        ];
        let notes = pair(&messages);
        assert_eq!(notes[0].song_on, Some(Frames(51_000)));
        let ticks = |frames: Frames| Ticks(frames.get() * TICKS_PER_QUARTER / 24_000);
        let origin = Ticks::WHOLE; // the bar line at or before the first note
        let placed = place(&notes, Timing::Running { origin }, ticks, 0, Ticks::WHOLE);
        // The earlier note is clamped to the clip's start; the second is
        // 100,000 frames in, 4 beats and a sixth, less the bar.
        assert_eq!(placed.notes[0].start, Ticks::ZERO);
        assert_eq!(
            placed.notes[1].start,
            ticks(Frames(100_000)) - origin,
            "at its song position"
        );
        assert_eq!(placed.loop_end, Ticks::WHOLE);
    }

    fn data(notes: &[(u8, i64)], loop_end: i64) -> MidiData {
        MidiData {
            looping: true,
            loop_end: Some(Ticks(loop_end)),
            ..MidiData::new(
                notes
                    .iter()
                    .map(|&(key, at)| Note::new(key, Ticks(at), Ticks(10), 100)),
            )
        }
    }

    fn at(key: u8, start: i64) -> Note {
        Note::new(key, Ticks(start), Ticks(10), 100)
    }

    // Spec: capture-midi R5. Notes played in the second pass unroll the
    // clip to it and move the loop there, keeping its length.
    #[test]
    fn notes_in_a_later_pass_unroll_the_clip_and_move_the_loop() {
        let clip = data(&[(60, 0)], 1_000);
        let (merged, length) = merge(&clip, Ticks(1_000), vec![at(72, 1_400)]);
        assert_eq!(length, Ticks(2_000));
        assert_eq!(
            (merged.loop_start, merged.loop_end),
            (Ticks(1_000), Some(Ticks(2_000)))
        );
        assert_eq!(merged.start, Ticks(1_000));
        let mut starts: Vec<(u8, i64)> = merged
            .notes
            .iter()
            .map(|n| (n.key, n.start.get()))
            .collect();
        starts.sort_unstable();
        assert_eq!(starts, [(60, 0), (60, 1_000), (72, 1_400)]);
    }

    // Spec: capture-midi R5. Notes in the first pass add to the clip as it is.
    #[test]
    fn notes_in_the_first_pass_leave_the_loop_alone() {
        let clip = data(&[(60, 0)], 1_000);
        let (merged, length) = merge(&clip, Ticks(1_000), vec![at(72, 400)]);
        assert_eq!(length, Ticks(1_000));
        assert_eq!(
            (merged.loop_start, merged.loop_end),
            (Ticks(0), Some(Ticks(1_000)))
        );
        assert_eq!(merged.notes.len(), 2);
    }

    // Spec: capture-midi R5. A note on the pass boundary is the last pass's
    // end, so the clip grows to cover it and the note stays after the loop.
    #[test]
    fn a_note_on_a_pass_boundary_stays_after_the_loop() {
        let clip = data(&[(60, 0)], 1_000);
        let (merged, length) = merge(&clip, Ticks(1_000), vec![at(72, 2_000)]);
        assert_eq!(length, Ticks(2_000));
        assert_eq!(merged.loop_end, Some(Ticks(2_000)));
        assert!(merged.notes.iter().any(|n| n.start == Ticks(2_000)));
    }

    // Spec: capture-midi R2. Pairing keeps velocities and ends held notes
    // with the last message.
    #[test]
    fn notes_pair_and_a_held_note_ends_with_the_last_message() {
        use bl_core::midi::MidiMessage;
        let message = |clock, message| CapturedMidi {
            track: TrackId(bl_core::id::Id::new(1).unwrap()),
            clock,
            song: None,
            message,
        };
        let messages = [
            message(100, MidiMessage::note_on(0, 60, 90)),
            message(200, MidiMessage::note_off(0, 60)),
            message(300, MidiMessage::note_on(0, 62, 80)),
            message(400, MidiMessage::note_off(0, 99)),
            message(500, MidiMessage::note_on(0, 64, 70)),
        ];
        let notes = pair(&messages);
        assert_eq!(
            notes
                .iter()
                .map(|n| (n.key, n.on, n.off, n.velocity))
                .collect::<Vec<_>>(),
            [(60, 100, 200, 90), (62, 300, 500, 80), (64, 500, 500, 70)]
        );
    }

    // Spec: capture-midi R2. A note held to the end ends on the last
    // message's song frame too, so a running capture keeps its length.
    #[test]
    fn a_held_note_ends_on_the_last_messages_song_frame() {
        use bl_core::midi::MidiMessage;
        let message = |clock, song, message| CapturedMidi {
            track: TrackId(bl_core::id::Id::new(1).unwrap()),
            clock,
            song: Some(Frames(song)),
            message,
        };
        let messages = [
            message(100, 1_100, MidiMessage::note_on(0, 60, 90)),
            message(300, 1_300, MidiMessage::note_on(0, 62, 80)),
            message(400, 1_400, MidiMessage::note_off(0, 62)),
        ];
        let notes = pair(&messages);
        assert_eq!(notes[0].song_off, Some(Frames(1_400)));
    }

    #[test]
    fn the_buffer_keeps_the_latest_and_a_lone_note_off_enables_nothing() {
        use bl_core::midi::MidiMessage;
        let mut buffer = Buffer::default();
        let track = TrackId(bl_core::id::Id::new(1).unwrap());
        let push = |buffer: &mut Buffer, clock, message| {
            buffer.push(CapturedMidi {
                track,
                clock,
                song: None,
                message,
            });
        };
        push(&mut buffer, 0, MidiMessage::note_off(0, 60));
        assert!(!buffer.has_notes());
        for clock in 0..MAX_MESSAGES as u64 + 10 {
            push(&mut buffer, clock, MidiMessage::note_on(0, 60, 100));
        }
        assert!(buffer.has_notes());
        let taken = buffer.take();
        assert_eq!(taken[&track].len(), MAX_MESSAGES);
        assert_eq!(taken[&track][0].clock, 10);
        assert!(!buffer.has_notes());
    }
}
